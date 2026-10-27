//! Terminal ownership: raw mode, alternate screen and panic safety.
//!
//! [`TerminalSession`] is an RAII guard, so every `?` early-return path restores
//! the terminal. [`install_panic_hook`] covers the paths that unwind instead of
//! returning.

use std::io::{self, Stdout};

use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// Terminal type used by the real binary.
pub type ClientTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Owns the terminal for the lifetime of the TUI.
///
/// Entering enables raw mode and the alternate screen; dropping the guard (or a
/// panic, via [`install_panic_hook`]) restores both.
pub struct TerminalSession {
    terminal: ClientTerminal,
}

impl TerminalSession {
    /// Puts the terminal into raw mode on the alternate screen.
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(io::stdout())) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore_terminal();
                Err(error)
            }
        }
    }

    /// The ratatui terminal driven by the event loop.
    pub fn terminal_mut(&mut self) -> &mut ClientTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Leaves the alternate screen and disables raw mode, ignoring failures.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// Makes every panic restore the terminal before the previous hook prints.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}
