use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tokio::task::JoinHandle;

/// Conventional exit code for a process terminated by SIGINT (128 + 2).
pub const SIGINT_EXIT_CODE: i32 = 130;

/// Outcome of recording a SIGINT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigintOutcome {
    /// First SIGINT: begin an ordered graceful shutdown.
    GracefulShutdown,
    /// Second SIGINT: stop waiting for cleanup and exit immediately.
    ImmediateExit,
}

#[derive(Clone)]
pub struct SignalState {
    pub shutdown_requested: Arc<AtomicBool>,
    pub quick_shutdown: Arc<AtomicBool>,
    pub sigwinch_received: Arc<AtomicBool>,
    pub sigint_count: Arc<AtomicU8>,
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalState {
    pub fn new() -> Self {
        Self {
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            quick_shutdown: Arc::new(AtomicBool::new(false)),
            sigwinch_received: Arc::new(AtomicBool::new(false)),
            sigint_count: Arc::new(AtomicU8::new(0)),
        }
    }

    pub fn request_normal_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
    }

    pub fn request_quick_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::SeqCst);
        self.quick_shutdown.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown_requested.load(Ordering::SeqCst)
    }

    pub fn is_quick(&self) -> bool {
        self.quick_shutdown.load(Ordering::SeqCst)
    }

    /// Records one SIGINT. The second (and any later) SIGINT escalates to an
    /// immediate exit so a stuck graceful shutdown cannot deadlock the user.
    pub fn note_sigint(&self) -> SigintOutcome {
        let count = self
            .sigint_count
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);

        if count >= 2 {
            self.request_quick_shutdown();
            SigintOutcome::ImmediateExit
        } else {
            self.request_normal_shutdown();
            SigintOutcome::GracefulShutdown
        }
    }

    pub fn note_sigterm(&self) {
        self.request_quick_shutdown();
    }

    pub fn note_sighup(&self) {
        self.request_normal_shutdown();
    }

    /// Called from the event loop, never from the signal handler itself.
    pub fn note_sigwinch(&self) {
        self.sigwinch_received.store(true, Ordering::SeqCst);
    }

    /// Returns and clears the pending redraw flag.
    pub fn take_sigwinch(&self) -> bool {
        self.sigwinch_received.swap(false, Ordering::SeqCst)
    }

    pub fn sigint_count(&self) -> u8 {
        self.sigint_count.load(Ordering::SeqCst)
    }
}

/// Installs process-wide POSIX defaults that Rust cannot express via Tokio:
/// ignore SIGPIPE (so socket writes fail with `EPIPE` instead of killing the
/// process) and reap children automatically with `SA_NOCLDWAIT` so
/// `execute.*.bg` helpers never become zombies.
pub fn install_posix_signal_defaults() {
    unsafe {
        let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        let _ = sigaction(Signal::SIGPIPE, &ignore);

        let reap = SigAction::new(
            SigHandler::SigIgn,
            SaFlags::SA_NOCLDWAIT | SaFlags::SA_RESTART,
            SigSet::empty(),
        );
        let _ = sigaction(Signal::SIGCHLD, &reap);
    }
}

/// True when no terminal session needs restoring. Initially true so a panic
/// before the TUI starts does not emit terminal escape sequences.
static TERMINAL_RESTORED: AtomicBool = AtomicBool::new(true);

/// Marks the beginning of a raw-mode/alternate-screen session.
pub fn begin_terminal_session() {
    TERMINAL_RESTORED.store(false, Ordering::SeqCst);
}

pub fn terminal_is_restored() -> bool {
    TERMINAL_RESTORED.load(Ordering::SeqCst)
}

/// Marks the terminal as restored without emitting any escape sequences.
///
/// `TerminalGuard::leave` cannot notify us that it already restored the
/// terminal, so the event loop calls this immediately afterwards to keep the
/// panic hook and the guard from restoring a second time.
pub fn mark_terminal_restored() {
    TERMINAL_RESTORED.store(true, Ordering::SeqCst);
}

/// Restores raw mode and the alternate screen at most once per session.
///
/// The `swap` makes this safe to call from the panic hook, the event loop and
/// `TerminalGuard`'s drop path concurrently: exactly one caller performs the
/// restore and the others observe the already-restored state.
pub fn restore_terminal_once() -> bool {
    if TERMINAL_RESTORED.swap(true, Ordering::SeqCst) {
        return false;
    }

    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    true
}

/// Pair with `TerminalGuard`: created before entering raw mode so a failure
/// inside `TerminalGuard::enter` still restores the terminal on unwind.
pub struct TerminalSessionGuard {
    active: bool,
}

impl TerminalSessionGuard {
    pub fn enter() -> Self {
        begin_terminal_session();
        Self { active: true }
    }

    /// Idempotently restores the terminal. Returns true if this call performed
    /// the restore.
    pub fn restore(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        restore_terminal_once()
    }
}

impl Drop for TerminalSessionGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Restores the terminal before the default panic handler runs. The restore is
/// idempotent and coordinates with any live `TerminalGuard`.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let restored = restore_terminal_once();
        if let Some(location) = info.location() {
            eprintln!(
                "rtorrent: panic at {}:{}:{} (terminal restored: {})",
                location.file(),
                location.line(),
                location.column(),
                restored
            );
        } else {
            eprintln!("rtorrent: panic (terminal restored: {})", restored);
        }
        default_hook(info);
    }));
}

/// Spawns the async signal loop. All real work is deferred here; the handlers
/// themselves only wake this task (SIGWINCH merely sets a flag).
pub fn spawn_signal_handlers(state: SignalState) -> JoinHandle<()> {
    install_posix_signal_defaults();

    tokio::spawn(async move {
        let mut sigint = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGINT handler: {}", e);
                return;
            }
        };

        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGTERM handler: {}", e);
                return;
            }
        };

        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGHUP handler: {}", e);
                return;
            }
        };

        let mut sigwinch = match signal(SignalKind::window_change()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to register SIGWINCH handler: {}", e);
                return;
            }
        };

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    match state.note_sigint() {
                        SigintOutcome::ImmediateExit => {
                            eprintln!("\nReceived double SIGINT: immediate exit.");
                            // `process::exit` skips destructors, so restore
                            // here rather than relying on TerminalGuard::drop.
                            let _ = restore_terminal_once();
                            std::process::exit(SIGINT_EXIT_CODE);
                        }
                        SigintOutcome::GracefulShutdown => {
                            eprintln!("\nShutting down gracefully... (Press Ctrl+C again to abort)");
                        }
                    }
                }
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM: quick shutdown.");
                    state.note_sigterm();
                }
                _ = sighup.recv() => {
                    tracing::info!("Received SIGHUP: normal shutdown.");
                    state.note_sighup();
                }
                _ = sigwinch.recv() => {
                    state.note_sigwinch();
                }
            }
        }
    })
}
