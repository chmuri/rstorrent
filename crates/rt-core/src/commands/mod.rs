pub mod convert;
pub mod dht;
pub mod download;
pub mod dynamic;
pub mod exec;
pub mod file;
pub mod groups;
pub mod ip;
pub mod log;
pub mod network;
pub mod peer;
pub mod pieces;
pub mod protocol;
pub mod ratio;
pub mod scheduler;
pub mod session;
pub mod string;
pub mod system;
pub mod throttle;
pub mod tracker;
pub mod view;

use std::sync::{Arc, RwLock};
use lt_net::config::NetworkConfig;
use rt_rpc::{CommandMap, ObjectStorage, RpcError};
use self::convert::register_convert_commands;
use self::dht::register_dht_commands;
use self::download::register_download_commands;
use self::dynamic::register_dynamic_commands;
use self::exec::register_exec_commands;
use self::file::register_file_commands;
use self::groups::{register_group_commands, ChokeGroupManager};
use self::ip::{register_ip_commands, IpTableManager};
use self::log::register_log_commands;
use self::network::register_network_commands;
use self::pieces::register_pieces_commands;
use self::protocol::register_protocol_commands;
use self::ratio::register_ratio_commands;
use self::peer::register_peer_commands;
use self::string::register_string_commands;
use self::system::register_system_commands;
use self::throttle::{register_throttle_commands, ThrottleManager};
use self::tracker::{register_tracker_commands, DhtConfig};
use self::view::register_view_commands;
use crate::download_list::DownloadList;
use crate::view_manager::ViewManager;

pub fn initialize_builtin_commands(
    map: &mut CommandMap,
    storage: Arc<RwLock<ObjectStorage>>,
    ip_manager: Arc<RwLock<IpTableManager>>,
    download_list: Arc<RwLock<DownloadList>>,
    view_manager: Arc<RwLock<ViewManager>>,
    group_manager: Arc<RwLock<ChokeGroupManager>>,
    throttle_manager: Arc<RwLock<ThrottleManager>>,
    dht_config: Arc<RwLock<DhtConfig>>,
    network_config: NetworkConfig,
) -> Result<(), RpcError> {
    register_string_commands(map)?;
    register_convert_commands(map)?;
    register_ratio_commands(map)?;
    register_log_commands(map)?;
    register_system_commands(map)?;
    register_network_commands(map, network_config.clone())?;
    register_pieces_commands(map)?;
    register_protocol_commands(map, network_config)?;
    register_ip_commands(map, ip_manager)?;
    register_dynamic_commands(map, storage)?;
    register_exec_commands(map)?;
    register_group_commands(map, group_manager)?;
    register_throttle_commands(map, throttle_manager)?;
    register_tracker_commands(
        map,
        Arc::clone(&dht_config),
        Arc::clone(&download_list),
    )?;
    // Replaces the legacy DHT commands registered with the tracker domain with
    // the runtime-aware versions (statistics, node/bootstrap/refresh control).
    register_dht_commands(map, dht_config)?;
    register_download_commands(
        map,
        Arc::clone(&download_list),
        Arc::clone(&view_manager),
    )?;
    register_file_commands(map, Arc::clone(&download_list))?;
    register_peer_commands(map, Arc::clone(&download_list))?;

    // View filters evaluate arbitrary d.* commands, so this registration must
    // happen after the rest of the command map exists.
    let view_map = Arc::new(RwLock::new(map.clone()));
    register_view_commands(
        map,
        Arc::clone(&view_manager),
        download_list,
        view_map,
    )?;

    Ok(())
}
