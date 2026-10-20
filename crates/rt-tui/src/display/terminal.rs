//! Terminal setup, rendering and input-event translation.
//!
//! # Main-loop wiring
//!
//! `rtorrent/src/main.rs` should:
//!
//! 1. Choose capture options and enter terminal mode with
//!    [`TerminalGuard::enter_with_options`]. Mouse and focus reporting are only
//!    enabled when requested, and are restored by [`TerminalGuard::leave`] /
//!    `Drop` on every exit path.
//! 2. Create a [`TerminalEventReader`] with the initial terminal size.
//! 3. Each frame call [`TerminalEventReader::poll`] with the frame timeout, then
//!    drain events with [`TerminalEventReader::pop`].
//! 4. On [`TerminalEvent::Resize`], resize the canvas and call
//!    [`TerminalEventReader::set_bounds`] (the translator already clamps mouse
//!    events to the new size, but the caller must keep its own canvas in sync).
//! 5. Ignore [`TerminalEvent::Unknown`] (or log/count it) and keep looping.
//!
//! ```no_run
//! use rt_tui::display::terminal::{
//!     EventQueueConfig, TerminalEvent, TerminalEventReader, TerminalGuard,
//!     TerminalInputOptions,
//! };
//! use std::time::Duration;
//!
//! # fn dispatch(_event: TerminalEvent) {}
//! let mut guard = TerminalGuard::enter_with_options(TerminalInputOptions {
//!     mouse: true,
//!     focus: true,
//!     bracketed_paste: true,
//! })
//! .unwrap();
//! let (cols, rows) = rt_tui::display::terminal::TerminalBackend::size();
//! let mut events = TerminalEventReader::new(cols, rows, EventQueueConfig::default());
//!
//! loop {
//!     events.poll(Duration::from_millis(50)).unwrap();
//!     while let Some(event) = events.pop() {
//!         dispatch(event);
//!     }
//!     // render here
//! #   break;
//! }
//! # let _ = guard.leave();
//! ```
//!
//! `TerminalGuard` and `TerminalEventReader` are independent so the renderer
//! can be used in daemon mode without enabling mouse/focus capture.

use crate::display::Canvas;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, Event, KeyEvent,
};
use crossterm::execute;
use crossterm::style::{
    Color as CrossColor, Print, ResetColor, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::style::Color as RataColor;
use std::io::{self, stdout, Stdout, Write};

pub mod events;

pub use self::events::{
    translate_key_event, EventPushOutcome, EventQueue, EventQueueConfig, EventQueueStats,
    EventTranslator, ResizePolicy, TerminalEvent, TerminalEventReader,
};

/// Which optional terminal input protocols should be enabled.
///
/// All protocols are disabled by default; this preserves the keyboard-only
/// behaviour of the legacy UI and avoids sending escape sequences to terminals
/// that do not need them. Mouse/focus/paste events can only arrive when the
/// corresponding protocol is enabled here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalInputOptions {
    /// Enable SGR/legacy mouse reporting (crossterm `EnableMouseCapture`).
    pub mouse: bool,
    /// Enable focus in/out reporting (crossterm `EnableFocusChange`).
    pub focus: bool,
    /// Enable bracketed paste reporting (crossterm `EnableBracketedPaste`).
    pub bracketed_paste: bool,
}

impl TerminalInputOptions {
    /// No optional input protocols.
    pub const fn none() -> Self {
        Self {
            mouse: false,
            focus: false,
            bracketed_paste: false,
        }
    }

    /// Every optional input protocol.
    pub const fn all() -> Self {
        Self {
            mouse: true,
            focus: true,
            bracketed_paste: true,
        }
    }

    /// Builder helper for mouse reporting.
    pub const fn with_mouse(mut self, enabled: bool) -> Self {
        self.mouse = enabled;
        self
    }

    /// Builder helper for focus reporting.
    pub const fn with_focus(mut self, enabled: bool) -> Self {
        self.focus = enabled;
        self
    }

    /// Builder helper for bracketed paste.
    pub const fn with_bracketed_paste(mut self, enabled: bool) -> Self {
        self.bracketed_paste = enabled;
        self
    }
}

/// A terminal protocol transition produced by [`capture_commands`].
///
/// Kept public and separate from the I/O in [`TerminalGuard`] so enable/disable
/// behaviour can be tested without a real terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureCommand {
    /// Enable mouse reporting.
    EnableMouse,
    /// Disable mouse reporting.
    DisableMouse,
    /// Enable focus reporting.
    EnableFocus,
    /// Disable focus reporting.
    DisableFocus,
    /// Enable bracketed paste.
    EnableBracketedPaste,
    /// Disable bracketed paste.
    DisableBracketedPaste,
}

/// Compute the commands needed to move from `from` to `to`.
///
/// Disables are emitted before enables so a protocol is never left in an
/// ambiguous half-toggled state. Unchanged options produce no command, which
/// makes repeated API calls cheap and idempotent.
pub fn capture_commands(
    from: TerminalInputOptions,
    to: TerminalInputOptions,
) -> Vec<CaptureCommand> {
    let mut commands = Vec::with_capacity(6);

    if from.mouse && !to.mouse {
        commands.push(CaptureCommand::DisableMouse);
    }
    if from.focus && !to.focus {
        commands.push(CaptureCommand::DisableFocus);
    }
    if from.bracketed_paste && !to.bracketed_paste {
        commands.push(CaptureCommand::DisableBracketedPaste);
    }
    if !from.mouse && to.mouse {
        commands.push(CaptureCommand::EnableMouse);
    }
    if !from.focus && to.focus {
        commands.push(CaptureCommand::EnableFocus);
    }
    if !from.bracketed_paste && to.bracketed_paste {
        commands.push(CaptureCommand::EnableBracketedPaste);
    }

    commands
}

/// Owns raw mode, the alternate screen and optional input protocols.
///
/// Dropping the guard always attempts to restore the terminal to its previous
/// state, including disabling mouse/focus/paste reporting if they were enabled.
pub struct TerminalGuard {
    active: bool,
    options: TerminalInputOptions,
}

impl TerminalGuard {
    /// Enter the alternate screen with raw mode and no optional input capture.
    ///
    /// Equivalent to [`enter_with_options`](Self::enter_with_options) with
    /// [`TerminalInputOptions::none`].
    pub fn enter() -> io::Result<Self> {
        Self::enter_with_options(TerminalInputOptions::none())
    }

    /// Enter the alternate screen with raw mode and the requested capture.
    ///
    /// If any protocol cannot be enabled, the terminal is restored before the
    /// error is returned.
    pub fn enter_with_options(options: TerminalInputOptions) -> io::Result<Self> {
        enable_raw_mode()?;

        let mut out = stdout();
        if let Err(error) = execute!(out, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }

        let mut guard = Self {
            active: true,
            options: TerminalInputOptions::none(),
        };
        if let Err(error) = guard.set_input_options(options) {
            // Best-effort restore of whatever was enabled before the failure.
            let _ = guard.leave();
            return Err(error);
        }
        Ok(guard)
    }

    /// Restore the terminal. Idempotent and safe to call from `Drop`.
    pub fn leave(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        let mut out = stdout();

        if self.options.mouse {
            let _ = execute!(out, DisableMouseCapture);
            self.options.mouse = false;
        }
        if self.options.focus {
            let _ = execute!(out, DisableFocusChange);
            self.options.focus = false;
        }
        if self.options.bracketed_paste {
            let _ = execute!(out, DisableBracketedPaste);
            self.options.bracketed_paste = false;
        }

        let _ = execute!(out, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
        self.active = false;
        Ok(())
    }

    /// Whether terminal mode is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Currently enabled optional input protocols.
    pub fn input_options(&self) -> TerminalInputOptions {
        self.options
    }

    /// Enable or disable optional input protocols.
    ///
    /// Commands are applied atomically per protocol; if crossterm returns an
    /// error, the guard keeps the state of the protocols that did succeed so
    /// `Drop`/`leave` can still restore them. No-op when the guard is inactive.
    pub fn set_input_options(&mut self, options: TerminalInputOptions) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        let commands = capture_commands(self.options, options);
        let mut out = stdout();
        for command in commands {
            match command {
                CaptureCommand::EnableMouse => {
                    execute!(out, EnableMouseCapture)?;
                    self.options.mouse = true;
                }
                CaptureCommand::DisableMouse => {
                    execute!(out, DisableMouseCapture)?;
                    self.options.mouse = false;
                }
                CaptureCommand::EnableFocus => {
                    execute!(out, EnableFocusChange)?;
                    self.options.focus = true;
                }
                CaptureCommand::DisableFocus => {
                    execute!(out, DisableFocusChange)?;
                    self.options.focus = false;
                }
                CaptureCommand::EnableBracketedPaste => {
                    execute!(out, EnableBracketedPaste)?;
                    self.options.bracketed_paste = true;
                }
                CaptureCommand::DisableBracketedPaste => {
                    execute!(out, DisableBracketedPaste)?;
                    self.options.bracketed_paste = false;
                }
            }
        }
        Ok(())
    }

    /// Enable or disable mouse reporting.
    pub fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options;
        options.mouse = enabled;
        self.set_input_options(options)
    }

    /// Enable or disable focus in/out reporting.
    pub fn set_focus_reporting(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options;
        options.focus = enabled;
        self.set_input_options(options)
    }

    /// Enable or disable bracketed paste reporting.
    pub fn set_bracketed_paste(&mut self, enabled: bool) -> io::Result<()> {
        let mut options = self.options;
        options.bracketed_paste = enabled;
        self.set_input_options(options)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.leave();
    }
}

pub struct TerminalBackend {
    stdout: Stdout,
}

impl Default for TerminalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalBackend {
    pub fn new() -> Self {
        Self { stdout: stdout() }
    }

    pub fn size() -> (u32, u32) {
        if let Ok((w, h)) = crossterm::terminal::size() {
            (w as u32, h as u32)
        } else {
            (80, 24)
        }
    }

    pub fn flush_canvas(&mut self, canvas: &Canvas) -> io::Result<()> {
        let width = canvas.full_width();
        let height = canvas.full_height();

        crossterm::queue!(self.stdout, MoveTo(0, 0))?;

        for y in 0..height {
            crossterm::queue!(self.stdout, MoveTo(0, y as u16))?;
            let mut current_fg = CrossColor::Reset;
            let mut current_bg = CrossColor::Reset;

            for x in 0..width {
                if let Some(cell) = canvas.get_raw_cell(x, y) {
                    let fg = cell
                        .style
                        .fg
                        .map(Self::map_color)
                        .unwrap_or(CrossColor::Reset);
                    let bg = cell
                        .style
                        .bg
                        .map(Self::map_color)
                        .unwrap_or(CrossColor::Reset);

                    if fg != current_fg {
                        crossterm::queue!(self.stdout, SetForegroundColor(fg))?;
                        current_fg = fg;
                    }
                    if bg != current_bg {
                        crossterm::queue!(self.stdout, SetBackgroundColor(bg))?;
                        current_bg = bg;
                    }
                    crossterm::queue!(self.stdout, Print(cell.ch))?;
                } else {
                    crossterm::queue!(self.stdout, Print(' '))?;
                }
            }
            crossterm::queue!(self.stdout, ResetColor)?;
        }

        self.stdout.flush()?;
        Ok(())
    }

    fn map_color(c: RataColor) -> CrossColor {
        match c {
            RataColor::Reset => CrossColor::Reset,
            RataColor::Black => CrossColor::Black,
            RataColor::Red => CrossColor::DarkRed,
            RataColor::Green => CrossColor::DarkGreen,
            RataColor::Yellow => CrossColor::DarkYellow,
            RataColor::Blue => CrossColor::DarkBlue,
            RataColor::Magenta => CrossColor::DarkMagenta,
            RataColor::Cyan => CrossColor::DarkCyan,
            RataColor::Gray => CrossColor::Grey,
            RataColor::DarkGray => CrossColor::DarkGrey,
            RataColor::LightRed => CrossColor::Red,
            RataColor::LightGreen => CrossColor::Green,
            RataColor::LightYellow => CrossColor::Yellow,
            RataColor::LightBlue => CrossColor::Blue,
            RataColor::LightMagenta => CrossColor::Magenta,
            RataColor::LightCyan => CrossColor::Cyan,
            RataColor::White => CrossColor::White,
            RataColor::Rgb(r, g, b) => CrossColor::Rgb { r, g, b },
            RataColor::Indexed(i) => CrossColor::AnsiValue(i),
        }
    }

    /// Translate a raw event using the current terminal size for mouse
    /// clamping.
    ///
    /// This is a convenience compatibility helper. The interactive loop should
    /// prefer [`EventTranslator`] or [`TerminalEventReader`] so that bounds stay
    /// in sync with the canvas and resize events are queued rather than lost.
    pub fn translate_event(event: Event) -> Option<TerminalEvent> {
        if matches!(&event, Event::Mouse(_)) {
            let (width, height) = Self::size();
            EventTranslator::new(width, height).translate(event)
        } else {
            EventTranslator::default().translate(event)
        }
    }

    /// Translate a raw event, clamping mouse coordinates to the given bounds.
    pub fn translate_event_with_bounds(
        event: Event,
        width: u32,
        height: u32,
    ) -> Option<TerminalEvent> {
        EventTranslator::new(width, height).translate(event)
    }

    /// Translate a crossterm key event into the legacy integer key code.
    ///
    /// Retained for compatibility; [`EventTranslator`] is preferred for new
    /// code because it also handles mouse/focus/paste/resize events.
    pub fn translate_key_event(key_event: KeyEvent) -> Option<i32> {
        events::translate_key_event(key_event)
    }
}
