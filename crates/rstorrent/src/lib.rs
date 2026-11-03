//! Crate `rstorrent`: Main entry point, CLI, Control, Setup, Signals

pub mod cli;
pub mod control;
pub mod grpc;
pub mod metrics;
pub mod scgi;
pub mod setup;
pub mod signals;
pub mod toml_config;
pub mod ui_commands;

pub use cli::Cli;
pub use control::Control;
pub use grpc::register_grpc_commands;
pub use metrics::Metrics;
pub use scgi::register_scgi_commands;
pub use setup::{
    find_config_file, parse_config_file_comments, setup_default_schedule, setup_default_views,
    SetupError,
};
pub use signals::{spawn_signal_handlers, SignalState};
pub use toml_config::RtorrentTomlConfig;

use clap::Parser;
use rt_tui::display::terminal::{TerminalGuard, TerminalInputOptions};
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

pub async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    signals::install_panic_hook();

    let is_interactive =
        !cli.daemon && std::io::stdout().is_terminal() && std::io::stdin().is_terminal();

    if !is_interactive {
        tracing_subscriber::fmt::init();
    }

    let mut control = Control::new();
    let signal_task = spawn_signal_handlers(control.signals.clone());

    if let Err(e) = control.initialize(&cli) {
        control.metrics.failure("startup.failed", &e);
        eprintln!("Initialization error: {}", e);
        std::process::exit(1);
    }

    if !is_interactive {
        tracing::info!("rstorrent running in non-interactive (daemon) mode...");
        while !control.signals.is_shutdown() {
            control.inc_tick();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        control.shutdown().await;
        signal_task.abort();
        control.metrics.summary(
            "exit.summary",
            &[
                ("wall_ms", control.metrics.elapsed_ms()),
                ("interactive", 0),
                ("forced", 0),
            ],
        );
        tracing::info!("rstorrent shutdown completed cleanly.");
        return Ok(());
    }

    // Interactive Terminal UI Mode. Create the restore guard before entering
    // raw mode so a failure inside `TerminalGuard::enter` still unwinds
    // cleanly.
    let _terminal_session = signals::TerminalSessionGuard::enter();
    let mut terminal_guard = TerminalGuard::enter_with_options(TerminalInputOptions::all())?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    let mut app = rt_tui::NextGenTuiApp::new("local".into());
    let mut sequence: u64 = 0;

    app.set_status_message("rstorrent ready. Press '?' for shortcuts, ':' for commands.");

    let mut last_snapshot_time = std::time::Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(std::time::Instant::now);
    let snapshot_interval = Duration::from_millis(250);
    let mut needs_draw = true;

    'main_loop: while !control.signals.is_shutdown() {
        control.inc_tick();

        // Process Crossterm events with responsive 16ms poll (60 FPS target)
        while crossterm::event::poll(Duration::from_millis(16))? {
            let event = crossterm::event::read()?;
            match event {
                crossterm::event::Event::Key(key) => {
                    needs_draw = true;
                    let action = rt_tui::input::router::KeyRouter::route_key(key, &app.mode);
                    if let Some(cmd) = app.dispatch_action(action) {
                        match cmd {
                            rt_tui::EngineCommand::Quit => {
                                control.signals.request_normal_shutdown();
                                break 'main_loop;
                            }
                            rt_tui::EngineCommand::StartTorrent(hashes) => {
                                let dl_list = control.download_list.read().unwrap();
                                for h in hashes {
                                    let hs = lt_common::hash_string::HashString::new(h);
                                    if let Some(d) = dl_list.find(&hs) {
                                        dl_list.start(&d);
                                    }
                                }
                            }
                            rt_tui::EngineCommand::PauseTorrent(hashes) => {
                                let dl_list = control.download_list.read().unwrap();
                                for h in hashes {
                                    let hs = lt_common::hash_string::HashString::new(h);
                                    if let Some(d) = dl_list.find(&hs) {
                                        dl_list.stop(&d);
                                    }
                                }
                            }
                            rt_tui::EngineCommand::RemoveTorrent {
                                hashes,
                                delete_data,
                            } => {
                                let hash_set: std::collections::HashSet<_> = hashes
                                    .iter()
                                    .map(|h| lt_common::hash_string::HashString::new(*h))
                                    .collect();
                                let erased = control
                                    .download_list
                                    .write()
                                    .unwrap()
                                    .erase_batch(&hash_set);
                                control
                                    .view_manager
                                    .write()
                                    .unwrap()
                                    .erase_batch_from_all(&hash_set, None);

                                if let Ok(sm) = control.session_manager.lock() {
                                    for h in &hash_set {
                                        let _ = sm.erase_entry(&h.to_hex());
                                    }
                                }

                                if delete_data {
                                    use rayon::prelude::*;
                                    erased.par_iter().for_each(|dl| {
                                        let guard = dl.read().unwrap();
                                        let dir = guard.directory();
                                        let name = guard.name();
                                        let base_path = std::path::Path::new(dir).join(name);
                                        if base_path.exists() {
                                            if base_path.is_dir() {
                                                let _ = std::fs::remove_dir_all(&base_path);
                                            } else {
                                                let _ = std::fs::remove_file(&base_path);
                                            }
                                        }
                                        for f in guard.download().file_list().files() {
                                            let p = f.path();
                                            if p.exists() && p.is_file() {
                                                let _ = std::fs::remove_file(p);
                                            }
                                        }
                                    });
                                }
                            }
                            rt_tui::EngineCommand::RecheckHash(hashes) => {
                                let dl_list = control.download_list.read().unwrap();
                                for h in hashes {
                                    let hs = lt_common::hash_string::HashString::new(h);
                                    if let Some(d) = dl_list.find(&hs) {
                                        dl_list.begin_metadata_recheck(&d);
                                    }
                                }
                            }
                            rt_tui::EngineCommand::AdjustDownloadThrottle(delta) => {
                                let map = control.command_map.read().unwrap();
                                let cur = map
                                    .call_command(
                                        "throttle.global_down.max_rate",
                                        &lt_common::Object::create_empty(),
                                        &rt_rpc::Target::None,
                                        true,
                                    )
                                    .ok()
                                    .and_then(|o| o.as_value().ok())
                                    .unwrap_or(0);
                                let new_val = (cur + delta).max(0);
                                let _ = map.call_command(
                                    "throttle.global_down.max_rate.set_kb",
                                    &lt_common::Object::from(new_val / 1024),
                                    &rt_rpc::Target::None,
                                    true,
                                );
                            }
                            rt_tui::EngineCommand::AdjustUploadThrottle(delta) => {
                                let map = control.command_map.read().unwrap();
                                let cur = map
                                    .call_command(
                                        "throttle.global_up.max_rate",
                                        &lt_common::Object::create_empty(),
                                        &rt_rpc::Target::None,
                                        true,
                                    )
                                    .ok()
                                    .and_then(|o| o.as_value().ok())
                                    .unwrap_or(0);
                                let new_val = (cur + delta).max(0);
                                let _ = map.call_command(
                                    "throttle.global_up.max_rate.set_kb",
                                    &lt_common::Object::from(new_val / 1024),
                                    &rt_rpc::Target::None,
                                    true,
                                );
                            }
                            rt_tui::EngineCommand::ToggleUncappedThrottle => {
                                let map = control.command_map.read().unwrap();
                                let cur = map
                                    .call_command(
                                        "throttle.global_down.max_rate",
                                        &lt_common::Object::create_empty(),
                                        &rt_rpc::Target::None,
                                        true,
                                    )
                                    .ok()
                                    .and_then(|o| o.as_value().ok())
                                    .unwrap_or(0);
                                let new_val = if cur > 0 { 0 } else { 10 * 1024 * 1024 };
                                let _ = map.call_command(
                                    "throttle.global_down.max_rate.set_kb",
                                    &lt_common::Object::from(new_val / 1024),
                                    &rt_rpc::Target::None,
                                    true,
                                );
                                let _ = map.call_command(
                                    "throttle.global_up.max_rate.set_kb",
                                    &lt_common::Object::from(new_val / 1024),
                                    &rt_rpc::Target::None,
                                    true,
                                );
                            }
                            rt_tui::EngineCommand::SetPriority {
                                hashes,
                                priority_step,
                            } => {
                                let dl_list = control.download_list.read().unwrap();
                                for h in hashes {
                                    let hs = lt_common::hash_string::HashString::new(h);
                                    if let Some(d) = dl_list.find(&hs) {
                                        let mut d_write = d.write().unwrap();
                                        let cur = d_write.priority();
                                        let new_p =
                                            (cur as i32 + priority_step as i32).clamp(0, 3) as u32;
                                        d_write.set_priority(new_p);
                                    }
                                }
                            }
                            rt_tui::EngineCommand::SetFilePriority {
                                hash,
                                file_index,
                                priority,
                            } => {
                                let hs = lt_common::hash_string::HashString::new(hash);
                                if let Some(d) = control.download_list.read().unwrap().find(&hs) {
                                    let mut d_write = d.write().unwrap();
                                    let mut dl_guard = d_write.download_mut();
                                    if let Some(file) = dl_guard
                                        .file_list_mut()
                                        .files_mut()
                                        .get_mut(file_index as usize)
                                    {
                                        file.set_priority(match priority {
                                            0 => lt_storage::Priority::Off,
                                            2 => lt_storage::Priority::High,
                                            _ => lt_storage::Priority::Normal,
                                        });
                                    }
                                }
                            }
                            rt_tui::EngineCommand::AddTorrent {
                                source,
                                destination: _,
                                start,
                                sequential: _,
                            } => {
                                if let Err(e) = control.try_load_torrent(&source, start) {
                                    app.set_status_message(format!("Error: {e}"));
                                } else {
                                    app.set_status_message("Torrent added successfully!");
                                }
                            }
                            rt_tui::EngineCommand::ToggleSequential(hash) => {
                                let hs = lt_common::hash_string::HashString::new(hash);
                                if let Some(d) = control.download_list.read().unwrap().find(&hs) {
                                    let mut d_write = d.write().unwrap();
                                    let cur = d_write.is_sequential();
                                    d_write.set_sequential(!cur);
                                }
                            }
                        }
                    }
                }
                crossterm::event::Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                    let max_count = app.category_counts.iter().copied().max().unwrap_or(0);
                    let layout = rt_tui::ui::layout::calculate_responsive_layout_dynamic(
                        area,
                        app.show_inspector,
                        max_count,
                    );
                    let action = rt_tui::input::mouse::MouseHandler::handle_mouse(
                        mouse,
                        &layout,
                        &app.mode,
                        area,
                        app.category_counts,
                        app.selected_row,
                        app.visible_indices.len(),
                    );
                    app.dispatch_action(action);
                    needs_draw = true;
                }
                crossterm::event::Event::Resize(_w, _h) => {
                    terminal.autoresize()?;
                    needs_draw = true;
                }
                crossterm::event::Event::Paste(text) => {
                    app.dispatch_action(rt_tui::input::router::AppAction::PasteInput(text));
                    needs_draw = true;
                }
                _ => {}
            }
        }

        if control.signals.is_shutdown() {
            break 'main_loop;
        }

        // Sample engine snapshot at 4 Hz (every 250ms) rather than 60 Hz to avoid
        // re-allocating tens of thousands of records and re-sorting 60 times a second.
        if last_snapshot_time.elapsed() >= snapshot_interval {
            sequence += 1;
            last_snapshot_time = std::time::Instant::now();

            let dl_guard = control.download_list.read().unwrap();
            let snapshot = rt_tui::concurrency::sample_engine_snapshot_with_stats(
                &dl_guard,
                control.listen_port,
                sequence,
            );
            let inspector_details = if app.show_inspector {
                if let Some(focused) = app.focused_torrent() {
                    rt_tui::concurrency::aggregator::sample_torrent_details(
                        &dl_guard,
                        &focused.hash,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            drop(dl_guard);

            app.update_snapshot(std::sync::Arc::new(snapshot));
            app.active_torrent_details = inspector_details;
            needs_draw = true;
        }

        // Render frame with Ratatui 0.30 diffing engine only when needed
        if needs_draw {
            terminal.draw(|f| app.render(f))?;
            needs_draw = false;
        }
    }

    // Immediately restore the terminal so raw mode, alternate screen and mouse
    // capture are disabled before any long-running disk or network shutdown.
    terminal_guard.leave()?;
    signals::mark_terminal_restored();

    // Drain any trailing input events (such as buffered SGR mouse tracking codes)
    // from the OS stdin queue so they do not spill into the shell prompt.
    while crossterm::event::poll(Duration::from_millis(20)).unwrap_or(false) {
        let _ = crossterm::event::read();
    }

    eprintln!("rstorrent: saving session and shutting down...");

    // A second ^Q forces an immediate exit without waiting for the graceful
    // network/session shutdown sequence.
    let force_shutdown = control.root_ui.lock().unwrap().force_shutdown_requested;
    if force_shutdown {
        tracing::warn!("Forced shutdown requested (double ^Q)");
        // Best-effort resume checkpoint so a forced exit does not throw away
        // progress since the last periodic save. Bounded: the exit stays
        // immediate if the disk is wedged.
        let session_manager = Arc::clone(&control.session_manager);
        let download_list = Arc::clone(&control.download_list);
        let save = tokio::task::spawn_blocking(move || {
            Control::save_resume_checkpoint_state(session_manager, download_list)
        });
        match tokio::time::timeout(std::time::Duration::from_secs(10), save).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => {
                tracing::warn!(error = %err, "Could not checkpoint resume data before forced exit")
            }
            Ok(Err(err)) => tracing::warn!(error = %err, "Forced checkpoint task failed"),
            Err(_) => tracing::warn!("Timed out checkpointing resume data before forced exit"),
        }
        control
            .metrics
            .failure("shutdown.forced", "double ^Q force exit");
        control.metrics.summary(
            "exit.summary",
            &[
                ("wall_ms", control.metrics.elapsed_ms()),
                ("interactive", 1),
                ("forced", 1),
            ],
        );
        std::process::exit(0);
    }

    // Ordered shutdown: network -> disk/session commit/lock release.
    control.shutdown().await;
    signal_task.abort();
    control.metrics.summary(
        "exit.summary",
        &[
            ("wall_ms", control.metrics.elapsed_ms()),
            ("interactive", 1),
            ("forced", 0),
        ],
    );
    println!("rstorrent shutdown completed cleanly.");

    Ok(())
}
