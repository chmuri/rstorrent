//! Async enforcement of the quota-based [`Throttle`] on peer streams.
//!
//! [`SharedThrottle`] wraps a [`Throttle`] with a waker registry so readers and
//! writers parked on an empty quota resume when the runtime refills it.
//! [`ThrottledReader`] / [`ThrottledWriter`] gate every transfer on the tightest
//! quota across one or more levels (global plus an optional per-torrent named
//! throttle), mirroring libtorrent's chained throttle nodes.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::throttle::{Throttle, ThrottleNode};

/// A [`Throttle`] shared across peer sessions, plus the wakers of sessions
/// currently parked waiting for quota.
#[derive(Clone)]
pub struct SharedThrottle {
    throttle: Throttle,
    wakers: Arc<Mutex<Vec<Waker>>>,
}

impl SharedThrottle {
    pub fn new(max_rate: u64) -> Self {
        Self {
            throttle: Throttle::new(max_rate),
            wakers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set_max_rate(&self, v: u64) {
        self.throttle.set_max_rate(v);
        self.wake_all();
    }

    pub fn max_rate(&self) -> u64 {
        self.throttle.max_rate()
    }

    pub fn is_throttled(&self) -> bool {
        self.throttle.is_throttled()
    }

    pub fn rate_bytes_per_sec(&self) -> u64 {
        self.throttle.rate_bytes_per_sec()
    }

    pub fn create_node(&self) -> Arc<ThrottleNode> {
        self.throttle.create_node()
    }

    pub fn remove_node(&self, node: &Arc<ThrottleNode>) {
        self.throttle.remove_node(node);
    }

    pub fn node_used(&self, node: &ThrottleNode, used: u32) -> u32 {
        self.throttle.node_used(node, used)
    }

    /// Distribute `quota` bytes to active nodes and resume parked sessions.
    /// The runtime calls this once per refill tick.
    pub fn refill(&self, quota: u32) {
        self.throttle.update_quota(quota);
        self.wake_all();
    }

    fn register_waker(&self, waker: &Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        if !wakers.iter().any(|w| w.will_wake(waker)) {
            wakers.push(waker.clone());
        }
    }

    fn wake_all(&self) {
        let mut wakers = self.wakers.lock().unwrap();
        for waker in wakers.drain(..) {
            waker.wake();
        }
    }
}

/// One throttle level a stream is subject to: the shared throttle and this
/// session's node within it.
struct Level {
    throttle: SharedThrottle,
    node: Arc<ThrottleNode>,
}

impl Level {
    /// `Some(quota)` when this level limits transfers, `None` when unlimited.
    fn available(&self) -> Option<u32> {
        if self.throttle.is_throttled() {
            Some(self.node.quota())
        } else {
            None
        }
    }
}

fn make_levels(throttles: Vec<SharedThrottle>) -> Vec<Level> {
    throttles
        .into_iter()
        .map(|throttle| {
            let node = throttle.create_node();
            Level { throttle, node }
        })
        .collect()
}

/// The tightest quota across all limiting levels, or `None` when every level
/// is unlimited. Registers `waker` on each limiting level when the quota is
/// exhausted so a refill resumes the session.
fn gate(levels: &[Level], waker: &Waker) -> Option<usize> {
    let mut cap = usize::MAX;
    let mut gated = false;
    for level in levels {
        if let Some(quota) = level.available() {
            gated = true;
            cap = cap.min(quota as usize);
        }
    }
    if !gated {
        return None;
    }
    if cap == 0 {
        for level in levels {
            if level.throttle.is_throttled() {
                level.throttle.register_waker(waker);
            }
        }
    }
    Some(cap)
}

fn record(levels: &[Level], used: usize) {
    if used == 0 {
        return;
    }
    for level in levels {
        level.throttle.node_used(&level.node, used as u32);
    }
}

fn drop_levels(levels: &[Level]) {
    for level in levels {
        level.throttle.remove_node(&level.node);
    }
}

/// An [`AsyncRead`] wrapper that paces reads to the shared throttle quota.
pub struct ThrottledReader<R> {
    inner: R,
    levels: Vec<Level>,
}

impl<R> ThrottledReader<R> {
    pub fn new(inner: R, throttles: Vec<SharedThrottle>) -> Self {
        Self {
            inner,
            levels: make_levels(throttles),
        }
    }
}

impl<R> Drop for ThrottledReader<R> {
    fn drop(&mut self) {
        drop_levels(&self.levels);
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for ThrottledReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let cap = match gate(&this.levels, cx.waker()) {
            None => buf.remaining(),
            Some(0) => return Poll::Pending,
            Some(cap) => cap,
        };

        let mut limited = buf.take(cap);
        let buf_ptr = limited.filled().as_ptr();
        match Pin::new(&mut this.inner).poll_read(cx, &mut limited) {
            Poll::Ready(Ok(())) => {
                assert_eq!(limited.filled().as_ptr(), buf_ptr);
                let n = limited.filled().len();
                // SAFETY: `limited` shares `buf`'s unfilled memory and the
                // inner reader initialized the first `n` bytes of it.
                unsafe { buf.assume_init(n) };
                buf.advance(n);
                record(&this.levels, n);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

/// An [`AsyncWrite`] wrapper that paces writes to the shared throttle quota.
pub struct ThrottledWriter<W> {
    inner: W,
    levels: Vec<Level>,
}

impl<W> ThrottledWriter<W> {
    pub fn new(inner: W, throttles: Vec<SharedThrottle>) -> Self {
        Self {
            inner,
            levels: make_levels(throttles),
        }
    }
}

impl<W> Drop for ThrottledWriter<W> {
    fn drop(&mut self) {
        drop_levels(&self.levels);
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ThrottledWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Pin::new(&mut this.inner).poll_write(cx, buf);
        }
        let cap = match gate(&this.levels, cx.waker()) {
            None => buf.len(),
            Some(0) => return Poll::Pending,
            Some(cap) => cap.min(buf.len()),
        };
        match Pin::new(&mut this.inner).poll_write(cx, &buf[..cap]) {
            Poll::Ready(Ok(n)) => {
                record(&this.levels, n);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A named per-torrent throttle pair (upload and download).
#[derive(Clone)]
pub struct NamedThrottle {
    pub up: SharedThrottle,
    pub down: SharedThrottle,
}

/// Process-wide registry of the global and named throttles. Peer sessions
/// resolve their levels here; the runtime syncs configured rates into it and
/// drives [`ThrottleRegistry::refill`].
pub struct ThrottleRegistry {
    global_up: SharedThrottle,
    global_down: SharedThrottle,
    named: Mutex<HashMap<String, NamedThrottle>>,
}

impl Default for ThrottleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ThrottleRegistry {
    pub fn new() -> Self {
        Self {
            global_up: SharedThrottle::new(0),
            global_down: SharedThrottle::new(0),
            named: Mutex::new(HashMap::new()),
        }
    }

    pub fn global_up(&self) -> SharedThrottle {
        self.global_up.clone()
    }

    pub fn global_down(&self) -> SharedThrottle {
        self.global_down.clone()
    }

    pub fn set_global_rates(&self, up: u64, down: u64) {
        self.global_up.set_max_rate(up);
        self.global_down.set_max_rate(down);
    }

    pub fn named(&self, name: &str) -> NamedThrottle {
        let mut named = self.named.lock().unwrap();
        named
            .entry(name.to_string())
            .or_insert_with(|| NamedThrottle {
                up: SharedThrottle::new(0),
                down: SharedThrottle::new(0),
            })
            .clone()
    }

    pub fn set_named_rate(&self, name: &str, up: u64, down: u64) {
        let throttle = self.named(name);
        throttle.up.set_max_rate(up);
        throttle.down.set_max_rate(down);
    }

    pub fn remove_named(&self, name: &str) {
        self.named.lock().unwrap().remove(name);
    }

    /// Download levels for a torrent: the global throttle, plus its named
    /// throttle when one is assigned.
    pub fn download_levels(&self, name: &str) -> Vec<SharedThrottle> {
        let mut levels = vec![self.global_down.clone()];
        if !name.is_empty() {
            levels.push(self.named(name).down);
        }
        levels
    }

    /// Upload levels for a torrent: the global throttle, plus its named
    /// throttle when one is assigned.
    pub fn upload_levels(&self, name: &str) -> Vec<SharedThrottle> {
        let mut levels = vec![self.global_up.clone()];
        if !name.is_empty() {
            levels.push(self.named(name).up);
        }
        levels
    }

    fn all_throttles(&self) -> Vec<SharedThrottle> {
        let mut throttles = vec![self.global_up.clone(), self.global_down.clone()];
        let named = self.named.lock().unwrap();
        for throttle in named.values() {
            throttles.push(throttle.up.clone());
            throttles.push(throttle.down.clone());
        }
        throttles
    }

    /// Refill every throttle with the bytes it may dispense over `interval`.
    pub fn refill(&self, interval: Duration) {
        let secs = interval.as_secs_f64();
        for throttle in self.all_throttles() {
            let quota = (throttle.max_rate() as f64 * secs) as u32;
            throttle.refill(quota);
        }
    }
}

static REGISTRY: OnceLock<Arc<ThrottleRegistry>> = OnceLock::new();

/// The process-wide throttle registry, created on first use.
pub fn throttle_registry() -> Arc<ThrottleRegistry> {
    REGISTRY
        .get_or_init(|| Arc::new(ThrottleRegistry::new()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An AsyncRead that immediately yields as much of `data` as the buffer
    /// allows, so quota gating is the only thing that can pace it.
    struct ReadyReader {
        data: Vec<u8>,
        pos: usize,
    }

    impl AsyncRead for ReadyReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let n = buf.remaining().min(self.data.len() - self.pos);
            buf.put_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Poll::Ready(Ok(()))
        }
    }

    #[derive(Default)]
    struct ReadyWriter {
        written: Arc<AtomicUsize>,
    }

    impl AsyncWrite for ReadyWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.written.fetch_add(buf.len(), Ordering::SeqCst);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn reader_paces_to_quota() {
        let throttle = SharedThrottle::new(1024);
        let mut reader = ThrottledReader::new(
            ReadyReader {
                data: vec![7u8; 4096],
                pos: 0,
            },
            vec![throttle.clone()],
        );
        let mut cx = Context::from_waker(Waker::noop());
        let mut out = [0u8; 4096];
        let mut buf = ReadBuf::new(&mut out);

        // No quota allocated yet: the read parks.
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_pending());
        assert_eq!(buf.filled().len(), 0);

        // One tick refills 1024 bytes; the read is capped to that quota.
        throttle.refill(1024);
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_ready());
        assert_eq!(buf.filled().len(), 1024);

        // Quota is spent: the next read parks until another refill.
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_pending());
        throttle.refill(1024);
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_ready());
        assert_eq!(buf.filled().len(), 2048);
    }

    #[test]
    fn unlimited_reader_passes_through() {
        let throttle = SharedThrottle::new(0);
        let mut reader = ThrottledReader::new(
            ReadyReader {
                data: vec![1u8; 100],
                pos: 0,
            },
            vec![throttle],
        );
        let mut cx = Context::from_waker(Waker::noop());
        let mut out = [0u8; 100];
        let mut buf = ReadBuf::new(&mut out);
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_ready());
        assert_eq!(buf.filled().len(), 100);
    }

    #[test]
    fn writer_paces_to_quota() {
        let throttle = SharedThrottle::new(1024);
        let written = Arc::new(AtomicUsize::new(0));
        let mut writer = ThrottledWriter::new(
            ReadyWriter {
                written: Arc::clone(&written),
            },
            vec![throttle.clone()],
        );
        let mut cx = Context::from_waker(Waker::noop());
        let data = [3u8; 4096];

        assert!(Pin::new(&mut writer)
            .poll_write(&mut cx, &data)
            .is_pending());
        assert_eq!(written.load(Ordering::SeqCst), 0);

        throttle.refill(1024);
        let res = Pin::new(&mut writer).poll_write(&mut cx, &data);
        assert!(matches!(res, Poll::Ready(Ok(1024))));
        assert_eq!(written.load(Ordering::SeqCst), 1024);
    }

    #[test]
    fn tightest_level_wins() {
        let global = SharedThrottle::new(4096);
        let named = SharedThrottle::new(1024);
        let mut reader = ThrottledReader::new(
            ReadyReader {
                data: vec![9u8; 8192],
                pos: 0,
            },
            vec![global.clone(), named.clone()],
        );
        let mut cx = Context::from_waker(Waker::noop());
        let mut out = [0u8; 8192];
        let mut buf = ReadBuf::new(&mut out);

        global.refill(4096);
        named.refill(1024);
        assert!(Pin::new(&mut reader)
            .poll_read(&mut cx, &mut buf)
            .is_ready());
        // The named throttle (1024) is tighter than the global (4096).
        assert_eq!(buf.filled().len(), 1024);
    }

    #[test]
    fn node_removed_on_drop() {
        let throttle = SharedThrottle::new(1024);
        {
            let _reader = ThrottledReader::new(
                ReadyReader {
                    data: vec![0u8; 16],
                    pos: 0,
                },
                vec![throttle.clone()],
            );
            throttle.refill(1024);
        }
        // After the reader drops, its node is detached: a refill allocates
        // nothing because no nodes remain.
        let node = throttle.create_node();
        throttle.refill(1024);
        assert_eq!(node.quota(), 1024);
    }

    #[test]
    fn registry_levels_include_named() {
        let registry = ThrottleRegistry::new();
        registry.set_global_rates(2048, 4096);
        registry.set_named_rate("slow", 512, 1024);

        let down = registry.download_levels("slow");
        assert_eq!(down.len(), 2);
        assert_eq!(down[0].max_rate(), 4096);
        assert_eq!(down[1].max_rate(), 1024);

        let up = registry.upload_levels("");
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].max_rate(), 2048);
    }
}
