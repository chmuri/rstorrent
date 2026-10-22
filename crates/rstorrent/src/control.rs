use crate::cli::Cli;
use crate::setup::{
    find_config_file, parse_config_file_comments, setup_default_schedule, setup_default_views,
};
use crate::signals::SignalState;
use lt_common::Object;
use lt_net::config::NetworkConfig;
use rt_core::commands::download::register_download_session_commands;
use rt_core::commands::groups::ChokeGroupManager;
use rt_core::commands::ip::IpTableManager;
use rt_core::commands::scheduler::register_scheduler_commands;
use rt_core::commands::session::register_session_commands;
use rt_core::commands::throttle::ThrottleManager;
use rt_core::commands::tracker::DhtConfig;
use rt_core::commands::{initialize_builtin_commands, CommandRegistry};
use rt_core::network_runtime::{DhtLifecycleConfig, DhtMode};
use rt_core::session::SessionManager;
use rt_core::{DownloadFactory, DownloadList, Manager as CoreManager, ViewManager};
use rt_rpc::{parse_command, CommandMap, CommandScheduler, ObjectStorage, Target};
use rt_tui::RootUI;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// Bounded deadlines for the graceful shutdown phases. A stage that overruns
/// its deadline is logged and its tasks are aborted rather than blocking
/// process exit forever.
const LISTENER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const DHT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const COORDINATOR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const DISK_FLUSH_TIMEOUT: Duration = Duration::from_secs(35);
const DISK_THREAD_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_SAVE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Control {
    pub core_manager: Arc<CoreManager>,
    pub view_manager: Arc<RwLock<ViewManager>>,
    pub download_list: Arc<RwLock<DownloadList>>,
    pub command_map: Arc<RwLock<CommandMap>>,
    pub command_scheduler: Arc<Mutex<CommandScheduler>>,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub object_storage: Arc<RwLock<ObjectStorage>>,
    pub ip_manager: Arc<RwLock<IpTableManager>>,
    pub group_manager: Arc<RwLock<ChokeGroupManager>>,
    pub throttle_manager: Arc<RwLock<ThrottleManager>>,
    pub dht_config: Arc<RwLock<DhtConfig>>,
    pub network_config: NetworkConfig,
    pub root_ui: Arc<Mutex<RootUI>>,
    pub signals: SignalState,
    pub tick: AtomicU64,
    pub is_shutdown_completed: AtomicBool,
    pub our_peer_id: lt_common::hash_string::HashString,
    pub listen_port: u16,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Join handles for the network runtime. Taken exactly once by
    /// [`Control::shutdown`] so every spawned task is drained or aborted.
    pub network_runtime: Mutex<Option<rt_core::network_runtime::NetworkRuntimeHandle>>,
    shutdown_lock: tokio::sync::Mutex<()>,
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl Control {
    pub fn new() -> Self {
        let command_map = Arc::new(RwLock::new(CommandMap::new()));
        let view_manager = Arc::new(RwLock::new(ViewManager::new()));
        let download_list = Arc::new(RwLock::new(DownloadList::new()));
        let core_manager = Arc::new(CoreManager::new());
        let command_scheduler = Arc::new(Mutex::new(CommandScheduler::new()));
        let session_manager = Arc::new(Mutex::new(SessionManager::new()));
        let object_storage = Arc::new(RwLock::new(ObjectStorage::new()));
        let ip_manager = Arc::new(RwLock::new(IpTableManager::new()));
        let group_manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
        let throttle_manager = Arc::new(RwLock::new(ThrottleManager::new()));
        let dht_config = Arc::new(RwLock::new(DhtConfig::new()));
        let network_config = NetworkConfig::new();
        let root_ui = Arc::new(Mutex::new(RootUI::new()));
        let signals = SignalState::new();

        let our_peer_id =
            lt_common::hash_string::HashString::new(lt_common::version::new_peer_id());

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        Self {
            core_manager,
            view_manager,
            download_list,
            command_map,
            command_scheduler,
            session_manager,
            object_storage,
            ip_manager,
            group_manager,
            throttle_manager,
            dht_config,
            network_config,
            root_ui,
            signals,
            tick: AtomicU64::new(0),
            is_shutdown_completed: AtomicBool::new(false),
            our_peer_id,
            listen_port: 6881,
            shutdown_tx,
            network_runtime: Mutex::new(None),
            shutdown_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn initialize(&mut self, cli: &Cli) -> Result<(), String> {
        // 1. Setup default views
        {
            let mut vm = self.view_manager.write().unwrap();
            setup_default_views(&mut vm);
        }

        // 2. Register all standard builtin commands into command_map
        {
            let mut map = self.command_map.write().unwrap();

            // Bind d.save_resume/d.save_full_session to the live session
            // manager before the builtin download registration. That
            // registration snapshots the command map for d.multicall, so it
            // must already contain the real handlers rather than placeholders.
            register_download_session_commands(
                &mut map,
                Arc::clone(&self.download_list),
                Arc::clone(&self.session_manager),
            )
            .map_err(|e| format!("Failed to register download session commands: {}", e))?;

            initialize_builtin_commands(
                &mut map,
                CommandRegistry {
                    storage: Arc::clone(&self.object_storage),
                    ip_manager: Arc::clone(&self.ip_manager),
                    download_list: Arc::clone(&self.download_list),
                    view_manager: Arc::clone(&self.view_manager),
                    group_manager: Arc::clone(&self.group_manager),
                    throttle_manager: Arc::clone(&self.throttle_manager),
                    dht_config: Arc::clone(&self.dht_config),
                    network_config: self.network_config.clone(),
                },
            )
            .map_err(|e| format!("Failed to initialize commands: {}", e))?;

            register_session_commands(
                &mut map,
                Arc::clone(&self.session_manager),
                Arc::clone(&self.download_list),
            )
            .map_err(|e| format!("Failed to register session commands: {}", e))?;

            register_scheduler_commands(&mut map, Arc::clone(&self.command_scheduler))
                .map_err(|e| format!("Failed to register scheduler commands: {}", e))?;

            crate::scgi::register_scgi_commands(&mut map, Arc::clone(&self.command_map))
                .map_err(|e| format!("Failed to register SCGI commands: {}", e))?;

            crate::grpc::register_grpc_commands(
                &mut map,
                rt_grpc::GrpcContext::new(
                    Arc::clone(&self.command_map),
                    Arc::clone(&self.download_list),
                    Arc::clone(&self.view_manager),
                    Arc::clone(&self.session_manager),
                    self.shutdown_tx.clone(),
                ),
                self.shutdown_tx.subscribe(),
            )
            .map_err(|e| format!("Failed to register gRPC commands: {}", e))?;

            crate::ui_commands::register_ui_commands(
                &mut map,
                Arc::clone(&self.root_ui),
                Arc::clone(&self.view_manager),
            )
            .map_err(|e| format!("Failed to register UI commands: {}", e))?;

            // `system.listMethods` is registered last so its snapshot contains
            // every command, including session/scheduler/SCGI/UI commands.
            let mut method_names: Vec<Object> = map
                .iter()
                .map(|(name, _entry)| Object::from(name.as_str()))
                .collect();
            method_names.push(Object::from("system.listMethods"));
            let method_names = Arc::new(method_names);
            map.insert_func(
                "system.listMethods",
                move |_target, _obj| Ok(Object::create_list_with((*method_names).clone())),
                rt_rpc::command_flags::FLAG_PUBLIC_RPC | rt_rpc::command_flags::FLAG_UNTRUSTED_SAFE,
                "",
                "List all registered methods.",
            )
            .map_err(|e| format!("Failed to register system.listMethods: {}", e))?;
        }

        // 3. Setup default schedule
        {
            let mut sched = self.command_scheduler.lock().unwrap();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            sched.set_cached_time(now);
            setup_default_schedule(&mut sched);
        }

        // 4. Parse config files
        if !cli.ignore_rc_file() {
            let config_path = cli
                .config_file
                .as_ref()
                .map(std::path::PathBuf::from)
                .or_else(|| find_config_file(cli.no_rc));
            if let Some(path) = config_path {
                let path_str = path.to_string_lossy().to_string();
                if path.exists() {
                    if let Err(e) = parse_config_file_comments(&path_str) {
                        tracing::warn!("Config comments error in {}: {}", path_str, e);
                    }
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        tracing::info!("Loaded configuration from {}", path_str);
                        let map = self.command_map.read().unwrap();
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() || trimmed.starts_with('#') {
                                continue;
                            }
                            if let Err(e) = parse_command(trimmed, &Target::None, &map, true) {
                                tracing::warn!("Config error in {}: {}", path_str, e);
                            }
                        }
                    }
                } else if cli.config_file.is_some() {
                    tracing::warn!(
                        "Specified configuration file '{}' does not exist.",
                        path_str
                    );
                }
            }
        }

        // 5. Apply CLI overrides
        {
            let map = self.command_map.read().unwrap();
            // CLI overrides are local, trusted configuration; some setters are
            // not marked untrusted-safe, so they must be called trusted or they
            // fail silently (e.g. used to lose `-d/--directory`).
            if let Some(port) = cli.port() {
                let _ = map.call_command(
                    "network.port_range.set",
                    &Object::from(format!("{}-{}", port, port).as_str()),
                    &Target::None,
                    true,
                );
            }

            if let Some(dir) = &cli.directory {
                let _ = map.call_command(
                    "directory.default.set",
                    &Object::from(dir.as_str()),
                    &Target::None,
                    true,
                );
            }

            if let Some(session) = &cli.session {
                let _ = map.call_command(
                    "session.path.set",
                    &Object::from(session.as_str()),
                    &Target::None,
                    true,
                );
            }

            if let Some(grpc_port) = cli.grpc_port {
                let _ = map.call_command(
                    "network.grpc.open_port",
                    &Object::from(grpc_port.to_string().as_str()),
                    &Target::None,
                    false,
                );
            }

            if let Some(grpc_socket) = &cli.grpc_socket {
                let _ = map.call_command(
                    "network.grpc.open_local",
                    &Object::from(grpc_socket.as_str()),
                    &Target::None,
                    false,
                );
            }

            // `-o key=value` options are rc-style configuration commands and
            // are applied before explicit `-O` commands.
            for opt in &cli.options {
                if let Err(e) = parse_command(opt, &Target::None, &map, true) {
                    tracing::warn!("CLI -o option error in '{}': {}", opt, e);
                }
            }

            for opt in &cli.command_options {
                if let Err(e) = parse_command(opt, &Target::None, &map, true) {
                    tracing::warn!("CLI -O command error in '{}': {}", opt, e);
                }
            }
        }

        // 6. Connect RootUI with view manager and download list
        {
            let mut ui = self.root_ui.lock().unwrap();
            ui.set_view_manager(Arc::clone(&self.view_manager));
            ui.set_download_list(Arc::clone(&self.download_list));
            ui.set_command_map(Arc::clone(&self.command_map));
            if let Some(port) = cli.port() {
                ui.listen_port = port;
            }
            ui.update_statusbar();
        }

        // 6.5 Remove session files when a download is erased. The listener
        // captures the manager, so it follows the path configured below.
        {
            let session_manager = Arc::clone(&self.session_manager);
            let view_manager = Arc::clone(&self.view_manager);
            self.download_list
                .write()
                .unwrap()
                .add_event_listener(Arc::new(move |event, download| {
                    match event {
                        "event.download.erased" => {
                            let hash = download
                                .read()
                                .map(|guard| guard.info_hash_hex())
                                .unwrap_or_default();
                            if hash.is_empty() {
                                return;
                            }
                            // The download list removes the entry; views keep
                            // their own Arc and must drop it as well, otherwise
                            // `d.multicall2` keeps listing a dead download.
                            if let Some(parsed) = lt_common::HashString::from_hex(&hash) {
                                view_manager
                                    .write()
                                    .unwrap()
                                    .erase_download_from_all(&parsed, None);
                            }
                            match session_manager.lock().unwrap().erase_entry(&hash) {
                                Ok(_) => {}
                                Err(err) => tracing::warn!(
                                    error = %err,
                                    info_hash = %hash,
                                    "Could not remove erased download from session"
                                ),
                            }
                        }
                        "event.download.finished" => {
                            let session = session_manager.lock().unwrap();
                            if !session.on_completion() || !session.is_active() {
                                return;
                            }
                            let guard = download.read().unwrap();
                            match CoreManager::build_commit_entry(&guard) {
                                Ok(entry) => {
                                    if let Err(err) = session.commit(&[entry]) {
                                        tracing::warn!(
                                            error = %err,
                                            "Could not save completed download to session"
                                        );
                                    }
                                }
                                Err(err) => tracing::warn!(
                                    error = %err,
                                    "Could not serialize completed download"
                                ),
                            }
                        }
                        "event.download.metadata" => {
                            // Magnet metadata just arrived: write the missing
                            // `<info-hash>.torrent` immediately instead of
                            // waiting for the periodic session save.
                            let session = session_manager.lock().unwrap();
                            if !session.is_active() {
                                return;
                            }
                            let guard = download.read().unwrap();
                            match CoreManager::build_commit_entry(&guard) {
                                Ok(entry) => {
                                    if let Err(err) = session.commit(&[entry]) {
                                        tracing::warn!(
                                            error = %err,
                                            "Could not persist fetched magnet metadata to session"
                                        );
                                    } else {
                                        tracing::info!(
                                            info_hash = %guard.info_hash_hex(),
                                            "Persisted fetched magnet metadata to session"
                                        );
                                    }
                                }
                                Err(err) => tracing::warn!(
                                    error = %err,
                                    "Could not serialize fetched magnet metadata"
                                ),
                            }
                        }
                        "event.download.paused" | "event.download.hash_done" => {
                            // Persist immediately when a transfer stops or a
                            // verification pass completes instead of waiting
                            // for the periodic checkpoint.
                            let session = session_manager.lock().unwrap();
                            if !session.is_active() {
                                return;
                            }
                            let guard = download.read().unwrap();
                            if guard.is_hash_checking() || guard.is_erasing() {
                                return;
                            }
                            match CoreManager::build_commit_entry(&guard) {
                                Ok(entry) => {
                                    if let Err(err) = session.save_resume(&entry) {
                                        tracing::warn!(
                                            error = %err,
                                            event = %event,
                                            "Could not checkpoint download state"
                                        );
                                    }
                                }
                                Err(err) => tracing::warn!(
                                    error = %err,
                                    event = %event,
                                    "Could not serialize download for checkpoint"
                                ),
                            }
                        }
                        _ => {}
                    }
                }));
        }

        // 7. Start session manager if session directory is configured, or use XDG default
        {
            let mut sm = self.session_manager.lock().unwrap();
            // Read the path from the manager directly. Calling the
            // `session.path` RPC here would re-lock the same manager and
            // deadlock while the mutex is held.
            let session_path = if let Some(session) = &cli.session {
                session.clone()
            } else if !sm.path().is_empty() {
                sm.path().to_string()
            } else {
                let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
                    if !xdg.is_empty() {
                        std::path::PathBuf::from(xdg).join("rstorrent/session")
                    } else if let Ok(home) = std::env::var("HOME") {
                        std::path::PathBuf::from(home).join(".local/share/rstorrent/session")
                    } else {
                        std::path::PathBuf::from("./.session")
                    }
                } else if let Ok(home) = std::env::var("HOME") {
                    std::path::PathBuf::from(home).join(".local/share/rstorrent/session")
                } else {
                    std::path::PathBuf::from("./.session")
                };
                base.to_string_lossy().to_string()
            };

            if !session_path.is_empty() {
                let _ = std::fs::create_dir_all(&session_path);
                if let Err(e) = sm.set_path(&session_path) {
                    tracing::warn!("Failed to set session path '{}': {}", session_path, e);
                } else if let Err(e) = sm.start() {
                    // Continuing without an active session would silently drop
                    // every torrent state change. Refuse to start instead; the
                    // lock is the only guard against two writers.
                    return Err(format!("Failed to lock/start session: {}", e));
                }
            }
        }

        // 8. Restore session entries and their resume data first. Loading CLI
        // torrents into a not-yet-restored session would commit a fresh, empty
        // resume and overwrite the saved progress.
        {
            let session_path = {
                let session = self.session_manager.lock().unwrap();
                session.path().to_string()
            };

            if !session_path.is_empty() {
                let entries = {
                    let session = self.session_manager.lock().unwrap();
                    // Restore every valid base, including magnet placeholders
                    // that never received metadata. Symlinks, temporary files
                    // and malformed names are ignored instead of aborting
                    // startup.
                    session.get_restorable_entries().unwrap_or_default()
                };

                for entry in entries {
                    let torrent_path = format!("{}{}", session_path, entry.base_name);
                    let hash_hex = &entry.base_name[..40];

                    if self
                        .download_list
                        .read()
                        .unwrap()
                        .find_hex(hash_hex)
                        .is_some()
                    {
                        // The download was just loaded from the command line
                        // (and may already have been written to the session).
                        tracing::debug!(
                            path = %torrent_path,
                            "Session entry already loaded; skipping duplicate."
                        );
                        continue;
                    }

                    let restored = match rt_core::session::restore_session_entry(
                        &torrent_path,
                        hash_hex,
                        entry.has_torrent,
                    ) {
                        Ok(restored) => restored,
                        Err(err) => {
                            tracing::warn!(
                                path = %torrent_path,
                                has_torrent = entry.has_torrent,
                                has_sidecar = entry.has_sidecar,
                                error = %err,
                                "Skipping invalid session torrent entry."
                            );
                            continue;
                        }
                    };

                    if let Some(err) = &restored.resume_error {
                        tracing::warn!(
                            path = %torrent_path,
                            error = %err,
                            "Session resume data was rejected; torrent restored without saved progress."
                        );
                    }

                    let mut download = restored.download;
                    let was_active = restored.was_active;
                    let resume_accepted = restored.resume_error.is_none();
                    if download.directory().is_empty() {
                        let default_dir = {
                            let map = self.command_map.read().unwrap();
                            map.call_command(
                                "directory.default",
                                &Object::create_empty(),
                                &Target::None,
                                false,
                            )
                            .ok()
                            .and_then(|obj| obj.as_str().map(|s| s.to_string()).ok())
                            .unwrap_or_else(|| ".".to_string())
                        };
                        download.set_directory(default_dir);
                    }

                    // The directory may have come from the sidecar, so verify
                    // existing payload regardless of whether we just filled in
                    // a default.
                    download.queue_recheck_if_files_exist();

                    let download = Arc::new(RwLock::new(download));
                    {
                        let mut dl_list = self.download_list.write().unwrap();
                        if !dl_list.insert(Arc::clone(&download)) {
                            tracing::warn!(
                                path = %torrent_path,
                                "Skipping duplicate session torrent entry."
                            );
                            continue;
                        }
                    }

                    {
                        let mut vm = self.view_manager.write().unwrap();
                        vm.insert_download_to_all(Arc::clone(&download));
                    }

                    if was_active && resume_accepted {
                        self.download_list.read().unwrap().start(&download);
                    }

                    self.root_ui
                        .lock()
                        .unwrap()
                        .push_log(format!("Restored session torrent '{}'", torrent_path));
                }
            }
        }

        // 8.5 Load files from CLI positional arguments. Torrents already
        // restored above are started from their existing entry instead of
        // being re-committed from scratch.
        for file in &cli.files {
            if let Err(e) = self.try_load_torrent(file, true) {
                tracing::warn!("Failed to load argument torrent '{}': {}", file, e);
            }
        }

        // 9. Spawn NetworkRuntime supervisor
        let dht_cache_path = {
            let session = self.session_manager.lock().unwrap();
            if session.path().is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(format!(
                    "{}dht.cache",
                    session.path()
                )))
            }
        };

        let dht_lifecycle = {
            let config = self.dht_config.read().unwrap();
            DhtLifecycleConfig {
                mode: DhtMode::parse(&config.mode).unwrap_or(DhtMode::Auto),
                port_override: config.override_port,
                ..DhtLifecycleConfig::default()
            }
        };

        let listen_port = if cli.port().is_some() {
            self.listen_port
        } else {
            let map = self.command_map.read().unwrap();
            map.call_command(
                "network.port_range",
                &Object::create_empty(),
                &Target::None,
                false,
            )
            .ok()
            .and_then(|obj| {
                obj.as_str().ok().and_then(|s| {
                    let first = s.split_once('-').map(|(f, _)| f).unwrap_or(s);
                    first.parse::<u16>().ok().filter(|&p| p != 0)
                })
            })
            .unwrap_or(self.listen_port)
        };
        self.listen_port = listen_port;

        let runtime = rt_core::NetworkRuntime::spawn_with_network_config_and_dht(
            Arc::clone(&self.download_list),
            self.our_peer_id,
            self.listen_port,
            self.shutdown_tx.subscribe(),
            dht_cache_path,
            self.network_config.clone(),
            dht_lifecycle,
        );
        *self.network_runtime.lock().unwrap() = Some(runtime);

        Ok(())
    }

    pub fn try_load_torrent(&self, path: &str, start: bool) -> Result<(), String> {
        let trimmed = path.trim().trim_matches('\'').trim_matches('"').trim();
        if trimmed.is_empty() {
            return Err("Empty torrent path or magnet URI".to_string());
        }

        let mut factory = DownloadFactory::new();
        factory.set_start(false);
        factory.set_init_load(true);

        if trimmed.starts_with("data:") {
            let pos = trimmed
                .find("base64,")
                .ok_or_else(|| "Invalid data uri: not base64 encoded".to_string())?;
            let start_idx = pos + 7;
            let b64 = &trimmed[start_idx..];
            let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            use base64::prelude::*;
            let decoded = BASE64_STANDARD
                .decode(&cleaned)
                .map_err(|e| format!("Invalid base64 in data URI: {e}"))?;
            factory
                .load_raw_data(&decoded)
                .map_err(|e| format!("Load data failed: {}", e))?;
        } else if trimmed.starts_with("magnet:?") {
            factory
                .load_magnet(trimmed)
                .map_err(|e| format!("Load magnet failed: {}", e))?;
        } else {
            factory
                .load_file(trimmed)
                .map_err(|e| format!("Load failed: {}", e))?;
        }

        let mut dl_list = self.download_list.write().unwrap();
        let download = match factory.commit(&mut dl_list) {
            Ok(dl) => dl,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(placeholder) = factory.create_download() {
                    let hash = *placeholder.info_hash();
                    if let Some(existing_dl) = dl_list.find(&hash) {
                        if start {
                            dl_list.start(&existing_dl);
                        }
                        self.root_ui.lock().unwrap().push_log(format!(
                            "Torrent already exists, updated/started: '{}'",
                            trimmed
                        ));
                        return Ok(());
                    }
                }
                return Err(format!("Commit failed: {}", e));
            }
            Err(e) => return Err(format!("Commit failed: {}", e)),
        };

        let default_dir = {
            let map = self.command_map.read().unwrap();
            map.call_command(
                "directory.default",
                &Object::create_empty(),
                &Target::None,
                false,
            )
            .ok()
            .and_then(|obj| obj.as_str().map(|s| s.to_string()).ok())
            .unwrap_or_else(|| ".".to_string())
        };
        download.write().unwrap().set_directory(&default_dir);
        download.write().unwrap().queue_recheck_if_files_exist();

        if start {
            dl_list.start(&download);
        }
        drop(dl_list);

        let mut vm = self.view_manager.write().unwrap();
        vm.insert_download_to_all(Arc::clone(&download));
        let _ = self.save_session();
        self.root_ui
            .lock()
            .unwrap()
            .push_log(format!("Loaded torrent from '{}'", trimmed));
        Ok(())
    }

    /// Applies the `change_directory` prompt (`^O`) to the focused download.
    pub fn set_download_directory(&self, path: &str) -> Result<(), String> {
        let hash = self
            .root_ui
            .lock()
            .unwrap()
            .selected_download_hash()
            .ok_or_else(|| "No download selected".to_string())?;
        let map = self.command_map.read().unwrap();
        map.call_command(
            "d.directory.set",
            &Object::from(path),
            &Target::Download(hash),
            true,
        )
        .map(|_| ())
        .map_err(|error| format!("Failed to change directory: {error}"))
    }

    pub fn execute_command(&self, cmd: &str) -> Result<String, String> {
        let map = self.command_map.read().unwrap();
        match parse_command(cmd, &Target::None, &map, true) {
            Ok((obj, _)) => {
                let res = obj
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| format!("{:?}", obj));
                self.root_ui
                    .lock()
                    .unwrap()
                    .push_log(format!("Command success: {}", res));
                Ok(res)
            }
            Err(e) => {
                let err = format!("Command error: {}", e);
                self.root_ui.lock().unwrap().push_log(err.clone());
                Err(err)
            }
        }
    }

    pub fn inc_tick(&self) -> u64 {
        let t = self.tick.fetch_add(1, Ordering::SeqCst);
        if t % 20 == 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // Run due scheduled commands once per second. Commands are
            // dispatched without holding the scheduler lock so a command may
            // call `schedule`/`schedule.remove` itself.
            let due = {
                let mut scheduler = self.command_scheduler.lock().unwrap();
                scheduler.due_commands(Duration::from_secs(now))
            };
            for (key, command) in due {
                {
                    let map = self.command_map.read().unwrap();
                    if let Err(err) =
                        rt_rpc::scheduler::call_command_object(&command, &map, &Target::None, true)
                    {
                        tracing::warn!("Scheduled command '{}' failed: {}", key, err);
                    }
                }
                self.command_scheduler
                    .lock()
                    .unwrap()
                    .complete_command(Duration::from_secs(now), &key);
            }

            let dl_list = self.download_list.read().unwrap();
            for dl in dl_list.all() {
                dl.write().unwrap().download_mut().tick(now);
            }
        }
        t
    }

    /// Saves the current session and resume data.
    pub fn save_session(&self) -> std::io::Result<()> {
        Self::save_session_state(
            Arc::clone(&self.session_manager),
            Arc::clone(&self.download_list),
        )
    }

    /// Writes resume data for every download that is not mid-verification.
    ///
    /// This is the bounded-work variant used by the periodic checkpoint and the
    /// forced-exit path: it never re-encodes `.torrent` files.
    pub fn save_resume_checkpoint_state(
        session_manager: Arc<Mutex<SessionManager>>,
        download_list: Arc<RwLock<DownloadList>>,
    ) -> std::io::Result<()> {
        let session = session_manager.lock().unwrap();
        if !session.is_used() || !session.is_active() {
            return Ok(());
        }

        let mut first_error = None;
        for download in download_list.read().unwrap().all() {
            let guard = download.read().unwrap();
            if guard.is_hash_checking() || guard.is_erasing() {
                continue;
            }
            match CoreManager::build_commit_entry(&guard) {
                Ok(entry) => {
                    if let Err(err) = session.save_resume(&entry) {
                        if first_error.is_none() {
                            first_error = Some(err);
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(error = %err, "Could not serialize checkpoint entry");
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn save_session_state(
        session_manager: Arc<Mutex<SessionManager>>,
        download_list: Arc<RwLock<DownloadList>>,
    ) -> std::io::Result<()> {
        let session = session_manager.lock().unwrap();
        if !session.is_used() || !session.is_active() {
            return Ok(());
        }

        let downloads = download_list.read().unwrap().all();
        let mut entries = Vec::with_capacity(downloads.len());
        let mut first_error = None;

        for download in downloads {
            let mut guard = download.write().unwrap();
            // Refresh magnet metadata (and its bencode) that may have arrived
            // since the last network supervisor tick, so a shutdown always
            // persists the static torrent as well.
            guard.sync_metadata_from_engine();
            match CoreManager::build_commit_entry(&guard) {
                Ok(entry) => entries.push(entry),
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        "Could not serialize download session data."
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        if !entries.is_empty() {
            if let Err(err) = session.commit(&entries) {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Ordered graceful shutdown: listeners → DHT → peer coordinators → disk
    /// flush/sync → session commit → lock release.
    ///
    /// Every stage is bounded. The network tasks are owned by
    /// [`rt_core::network_runtime::NetworkRuntimeHandle`], so correctness no
    /// longer depends on a fixed delay before touching the disk or session.
    pub async fn shutdown(&self) {
        // Serialize concurrent shutdown callers and make repeated calls a
        // no-op once cleanup has completed.
        let _shutdown_guard = self.shutdown_lock.lock().await;
        if self.is_shutdown_completed() {
            tracing::debug!("Graceful shutdown already completed; skipping");
            return;
        }

        tracing::info!("Graceful shutdown in progress...");
        let _ = self.shutdown_tx.send(true);

        // 1-3. Listeners, DHT and the coordinator supervisor (which stops
        // every per-torrent coordinator) in dependency order.
        let runtime = self.network_runtime.lock().unwrap().take();
        if let Some(runtime) = runtime {
            let report = runtime
                .drain(rt_core::network_runtime::ShutdownTimeouts {
                    listeners: LISTENER_SHUTDOWN_TIMEOUT,
                    dht: DHT_SHUTDOWN_TIMEOUT,
                    coordinators: COORDINATOR_SHUTDOWN_TIMEOUT,
                })
                .await;

            if !report.listeners_completed {
                tracing::warn!("Peer listener did not stop before its deadline");
            }
            if !report.dht_completed {
                tracing::warn!("DHT tasks did not stop before their deadline");
            }
            if !report.coordinators_completed {
                tracing::warn!("Network coordinator supervisor did not stop before its deadline");
            }
        } else {
            tracing::debug!("No network runtime was started; skipping network shutdown");
        }

        // 4. Drain acknowledged piece writes and sync file handles.
        self.flush_downloads().await;

        // 5. Stop the process-wide disk worker after all downloads have been
        // flushed so queued writes are synced before the session is committed.
        match tokio::time::timeout(
            DISK_THREAD_SHUTDOWN_TIMEOUT,
            tokio::task::spawn_blocking(lt_storage::shutdown_global_disk_thread),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(error = %err, "Disk worker shutdown task failed"),
            Err(_) => tracing::warn!("Timed out waiting for the disk worker to stop"),
        }

        // 6. Commit session/resume data, then release the session lock. Run
        // the file I/O on the blocking pool so a second SIGINT can still be
        // serviced while the session is written.
        let session_save = tokio::task::spawn_blocking({
            let session_manager = Arc::clone(&self.session_manager);
            let download_list = Arc::clone(&self.download_list);
            move || Self::save_session_state(session_manager, download_list)
        });

        match tokio::time::timeout(SESSION_SAVE_TIMEOUT, session_save).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => {
                tracing::error!(error = %err, "Could not save session during shutdown")
            }
            Ok(Err(err)) => tracing::warn!(error = %err, "Session save task failed"),
            Err(_) => tracing::warn!("Timed out saving the session during shutdown"),
        }

        {
            let mut session = self.session_manager.lock().unwrap();
            if session.is_active() {
                if let Err(err) = session.stop() {
                    tracing::warn!(error = %err, "Could not stop session during shutdown");
                }
            }
        }

        self.is_shutdown_completed.store(true, Ordering::SeqCst);
        tracing::info!("Graceful shutdown completed.");
    }

    async fn flush_downloads(&self) {
        let downloads = self.download_list.read().unwrap().all();
        if downloads.is_empty() {
            return;
        }

        let flush = tokio::task::spawn_blocking(move || {
            for download in downloads {
                match download.write().unwrap().flush_disk() {
                    Ok(()) => {}
                    Err(err) => tracing::warn!(error = %err, "Could not flush download"),
                }
            }
        });

        match tokio::time::timeout(DISK_FLUSH_TIMEOUT, flush).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(error = %err, "Disk flush task failed"),
            Err(_) => tracing::warn!("Timed out flushing downloads during shutdown"),
        }
    }

    /// Immediate shutdown used by the synchronous fallback path. It cannot
    /// await task completion, so it aborts the supervised network tasks and
    /// releases the session lock without committing new state.
    pub fn handle_shutdown(&self) {
        if self.is_shutdown_completed() {
            return;
        }

        tracing::info!("Immediate shutdown requested...");
        let _ = self.shutdown_tx.send(true);

        if let Some(runtime) = self.network_runtime.lock().unwrap().take() {
            runtime.abort_all();
        }

        {
            let mut session = self.session_manager.lock().unwrap();
            if session.is_active() {
                let _ = session.stop();
            }
        }

        self.is_shutdown_completed.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown_completed(&self) -> bool {
        self.is_shutdown_completed.load(Ordering::SeqCst)
    }
}
