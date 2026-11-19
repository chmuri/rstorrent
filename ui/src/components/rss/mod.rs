//! RSS page panels. T8 ships empty stubs with the final component signatures;
//! T9-T12 fill them in and T13 mounts the page, so nothing here is reachable
//! from `main` yet.
#![allow(dead_code, unused_imports)]

pub mod feeds_panel;
pub mod history_panel;
pub mod items_panel;
pub mod rules_panel;

pub use feeds_panel::FeedsPanel;
pub use history_panel::HistoryPanel;
pub use items_panel::ItemsPanel;
pub use rules_panel::RulesPanel;
