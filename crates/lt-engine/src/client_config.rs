//! Per-download peer and connection limits.
//!
//! Mirrors the legacy `ClientConfig` values used by `network.*` and `throttle.*`
//! commands. The listen port range mirrors the runtime client configuration
//! from `torrent/runtime/client_config.{h,cc}`.

use std::time::Duration;

use thiserror::Error;

/// Invalid client configuration value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClientConfigError {
    #[error("Invalid listen port range: {first}-{last}.")]
    InvalidListenPortRange { first: u16, last: u16 },
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    max_peers: u32,
    min_peers: u32,
    max_peers_seed: i32,
    min_peers_seed: i32,
    max_uploads: u32,
    max_downloads: u32,
    /// Hard cap on simultaneous peer connections. `0` means unlimited (the
    /// per-torrent `max_peers` value is still applied).
    max_connections: u32,
    /// Cap on dials that have not completed their handshake. `0` means derive
    /// the value from `max_connections`/`max_peers`.
    max_half_open: u32,
    handshake_timeout: Duration,
    connect_timeout: Duration,
    listen_port_first: u16,
    listen_port_last: u16,
    listen_port_random: bool,
    pex_enabled: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_peers: 200,
            min_peers: 10,
            max_peers_seed: -1,
            min_peers_seed: -1,
            max_uploads: 50,
            max_downloads: 50,
            max_connections: 0,
            max_half_open: 0,
            handshake_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            listen_port_first: 6881,
            listen_port_last: 6999,
            listen_port_random: true,
            pex_enabled: true,
        }
    }
}

impl ClientConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_peers(&self) -> u32 {
        self.max_peers
    }

    pub fn set_max_peers(&mut self, value: u32) {
        // Mirror libtorrent's `ConnectionList` hard ceiling.
        self.max_peers = value.clamp(1, 1 << 16);
        if self.min_peers > self.max_peers {
            self.min_peers = self.max_peers;
        }
    }

    pub fn min_peers(&self) -> u32 {
        self.min_peers
    }

    pub fn set_min_peers(&mut self, value: u32) {
        self.min_peers = value.min(self.max_peers);
    }

    /// `-1` means unlimited.
    pub fn max_peers_seed(&self) -> i32 {
        self.max_peers_seed
    }

    pub fn set_max_peers_seed(&mut self, value: i32) {
        self.max_peers_seed = if value < 0 { -1 } else { value };
    }

    pub fn min_peers_seed(&self) -> i32 {
        self.min_peers_seed
    }

    pub fn set_min_peers_seed(&mut self, value: i32) {
        self.min_peers_seed = if value < 0 { -1 } else { value };
    }

    pub fn max_uploads(&self) -> u32 {
        self.max_uploads
    }

    pub fn set_max_uploads(&mut self, value: u32) {
        self.max_uploads = value;
    }

    pub fn max_downloads(&self) -> u32 {
        self.max_downloads
    }

    pub fn set_max_downloads(&mut self, value: u32) {
        self.max_downloads = value;
    }

    /// Hard cap on peer connections. `0` means no additional cap beyond
    /// [`ClientConfig::max_peers`].
    pub fn max_connections(&self) -> u32 {
        self.max_connections
    }

    pub fn set_max_connections(&mut self, value: u32) {
        self.max_connections = value;
    }

    /// Alias for [`ClientConfig::max_connections`].
    pub fn connection_limit(&self) -> u32 {
        self.max_connections
    }

    /// Alias for [`ClientConfig::set_max_connections`].
    pub fn set_connection_limit(&mut self, value: u32) {
        self.set_max_connections(value);
    }

    /// Maximum number of concurrent half-open (dialing/handshaking) peers.
    /// A value of `0` derives a sensible bound from the connection limit.
    pub fn max_half_open(&self) -> u32 {
        self.max_half_open
    }

    pub fn set_max_half_open(&mut self, value: u32) {
        self.max_half_open = value;
    }

    /// Effective peer limit after applying the optional hard connection cap.
    pub fn effective_max_peers(&self) -> usize {
        let max_peers = self.max_peers as usize;
        if self.max_connections == 0 {
            max_peers
        } else {
            max_peers.min(self.max_connections as usize)
        }
    }

    /// Whether another peer may be admitted at `current` connections.
    pub fn allows_peer_count(&self, current: usize) -> bool {
        current < self.effective_max_peers()
    }

    /// Whether another upload slot may be handed out at `active_uploads`.
    /// A limit of `0` means unlimited.
    pub fn allows_upload_count(&self, active_uploads: usize) -> bool {
        self.max_uploads == 0 || active_uploads < self.max_uploads as usize
    }

    /// Whether another download slot may be handed out at `active_downloads`.
    /// A limit of `0` means unlimited.
    pub fn allows_download_count(&self, active_downloads: usize) -> bool {
        self.max_downloads == 0 || active_downloads < self.max_downloads as usize
    }

    /// Number of half-open connections the dialer may keep in flight.
    pub fn effective_max_half_open(&self) -> usize {
        if self.max_half_open != 0 {
            return self.max_half_open as usize;
        }
        let limit = self.effective_max_peers().max(1);
        // Match the legacy connection scheduler: a fraction of the peer limit,
        // never more than 50 concurrent dials.
        limit.div_ceil(4).clamp(1, 50)
    }

    pub fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }

    pub fn set_handshake_timeout(&mut self, timeout: Duration) {
        self.handshake_timeout = timeout.max(Duration::from_secs(1));
    }

    pub fn set_handshake_timeout_secs(&mut self, seconds: u64) {
        self.handshake_timeout = Duration::from_secs(seconds.max(1));
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    pub fn set_connect_timeout(&mut self, timeout: Duration) {
        self.connect_timeout = timeout.max(Duration::from_secs(1));
    }

    pub fn set_connect_timeout_secs(&mut self, seconds: u64) {
        self.connect_timeout = Duration::from_secs(seconds.max(1));
    }

    /// First and last port used by the inbound listener.
    pub fn listen_port_range(&self) -> (u16, u16) {
        (self.listen_port_first, self.listen_port_last)
    }

    /// Alias for [`ClientConfig::listen_port_range`].
    pub fn port_range(&self) -> (u16, u16) {
        self.listen_port_range()
    }

    pub fn listen_port_first(&self) -> u16 {
        self.listen_port_first
    }

    pub fn listen_port_last(&self) -> u16 {
        self.listen_port_last
    }

    /// Sets the inclusive listener port range.
    ///
    /// Mirrors libtorrent's validation: port 0 and reversed ranges are
    /// rejected.
    pub fn set_listen_port_range(
        &mut self,
        first: u16,
        last: u16,
    ) -> Result<(), ClientConfigError> {
        if first == 0 || first > last {
            return Err(ClientConfigError::InvalidListenPortRange { first, last });
        }
        self.listen_port_first = first;
        self.listen_port_last = last;
        Ok(())
    }

    pub fn listen_port_random(&self) -> bool {
        self.listen_port_random
    }

    pub fn set_listen_port_random(&mut self, value: bool) {
        self.listen_port_random = value;
    }

    pub fn is_pex_enabled(&self) -> bool {
        self.pex_enabled
    }

    pub fn set_pex_enabled(&mut self, value: bool) {
        self.pex_enabled = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_clamped_consistently() {
        let mut config = ClientConfig::default();
        config.set_max_peers(5);
        config.set_min_peers(50);
        assert_eq!(config.max_peers(), 5);
        assert_eq!(config.min_peers(), 5);

        config.set_max_peers(0);
        assert_eq!(config.max_peers(), 1);

        config.set_max_peers_seed(-10);
        assert_eq!(config.max_peers_seed(), -1);
    }

    #[test]
    fn listen_port_range_is_validated() {
        let mut config = ClientConfig::default();
        assert_eq!(config.listen_port_range(), (6881, 6999));
        assert!(config.set_listen_port_range(7000, 7100).is_ok());
        assert_eq!(config.port_range(), (7000, 7100));
        assert!(config.set_listen_port_range(0, 100).is_err());
        assert!(config.set_listen_port_range(1000, 999).is_err());
    }

    #[test]
    fn connection_cap_is_applied_on_top_of_peer_limit() {
        let mut config = ClientConfig::default();
        config.set_max_peers(100);
        config.set_max_connections(25);
        assert_eq!(config.effective_max_peers(), 25);
        assert!(config.allows_peer_count(24));
        assert!(!config.allows_peer_count(25));

        config.set_max_connections(0);
        assert_eq!(config.effective_max_peers(), 100);
    }
}
