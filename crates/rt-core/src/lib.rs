// This crate contains documented low-level unsafe invariants (plan.md :264).
// Every member inherits the workspace lints table (unsafe_code = "deny");
// this crate opts out at the crate root — source attributes override the
// lint flags cargo injects.
#![allow(unsafe_code)]

//! Crate `rt-core`

pub mod commands;
pub mod download;
pub mod download_factory;
pub mod download_list;
pub mod hash_scheduler;
pub mod manager;
pub mod network_runtime;
pub mod seed_context;
pub mod session;
pub mod tiering;
pub mod view;
pub mod view_manager;

pub use commands::{
    dynamic::register_dynamic_commands,
    groups::ChokeGroupManager,
    initialize_builtin_commands,
    ip::{ipv4_range_parse, register_ip_commands, IpTableManager},
    string::register_string_commands,
    system::{register_system_commands, resolve_path, resolve_path_or_throw},
    throttle::ThrottleManager,
    tracker::{checked_port_value, DhtConfig},
    CommandRegistry,
};
pub use download::Download;
pub use download_factory::DownloadFactory;
pub use download_list::{DownloadEventListener, DownloadList};
pub use manager::Manager;
pub use network_runtime::NetworkRuntime;
pub use session::{DirectoryEntry, DownloadStorer, SessionManager};
pub use tiering::{FileMover, MoveError, MoveStatus, TieringManager, TorrentTierState};
pub use view::View;
pub use view_manager::ViewManager;
