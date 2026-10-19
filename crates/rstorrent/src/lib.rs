//! Crate `rtorrent`: Main entry point, CLI, Control, Setup, Signals

pub mod cli;
pub mod control;
pub mod grpc;
pub mod scgi;
pub mod setup;
pub mod ui_commands;
pub mod signals;

pub use cli::Cli;
pub use control::Control;
pub use grpc::register_grpc_commands;
pub use scgi::register_scgi_commands;
pub use setup::{
    find_config_file, parse_config_file_comments, setup_default_schedule, setup_default_views,
    SetupError,
};
pub use signals::{spawn_signal_handlers, SignalState};

use clap::Parser;
use rt_tui::display::terminal::{
    EventQueueConfig, ResizePolicy, TerminalBackend, TerminalEvent, TerminalEventReader,
    TerminalGuard, TerminalInputOptions,
};
use rt_tui::display::Canvas;
use rt_tui::ui::InputPromptMode;
use rt_tui::Window;
use std::io::IsTerminal;
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
        tracing::info!("rstorrent shutdown completed cleanly.");
        return Ok(());
    }

    // Interactive Terminal UI Mode. Create the restore guard before entering
    // raw mode so a failure inside `TerminalGuard::enter` still unwinds
    // cleanly.
    let _terminal_session = signals::TerminalSessionGuard::enter();
    let mut terminal_guard =
        TerminalGuard::enter_with_options(TerminalInputOptions::all())?;
    let mut terminal_backend = TerminalBackend::new();

    let (cols, rows) = TerminalBackend::size();
    let mut canvas = Canvas::new(0, 0, cols, rows);
    let mut events = TerminalEventReader::new(
        cols,
        rows,
        EventQueueConfig {
            capacity: 256,
            resize_policy: ResizePolicy::Coalesce,
        },
    );

    control
        .root_ui
        .lock()
        .unwrap()
        .push_log("rstorrent initialized. Press 'l' for log, Backspace to load, ^X for command, ^Q to exit.");

    while !control.signals.is_shutdown() && !control.root_ui.lock().unwrap().shutdown_requested {
        control.inc_tick();

        // SIGWINCH only sets a flag; actual resize/redraw work happens here in
        // the event loop.
        if control.signals.take_sigwinch() {
            let (cols, rows) = TerminalBackend::size();
            canvas.resize(cols, rows);
            events.set_bounds(cols, rows);
            let ui = control.root_ui.lock().unwrap();
            ui.window_title.lock().unwrap().mark_dirty();
            ui.window_download_list.lock().unwrap().mark_dirty();
            ui.window_details.lock().unwrap().mark_dirty();
            ui.window_statusbar.lock().unwrap().mark_dirty();
        }

        // Check terminal events (bounded, coalesced queue with a 50ms poll).
        events.poll(Duration::from_millis(50))?;
        while let Some(event) = events.pop() {
            match event {
                TerminalEvent::Resize(w, h) => {
                    canvas.resize(w, h);
                    events.set_bounds(w, h);
                    let ui = control.root_ui.lock().unwrap();
                    ui.window_title.lock().unwrap().mark_dirty();
                    ui.window_download_list.lock().unwrap().mark_dirty();
                    ui.window_details.lock().unwrap().mark_dirty();
                    ui.window_statusbar.lock().unwrap().mark_dirty();
                }
                TerminalEvent::Key(key) => {
                    let action = {
                        let mut ui = control.root_ui.lock().unwrap();
                        ui.handle_key(key)
                    };

                    if let Some((mode, text)) = action {
                        match mode {
                            InputPromptMode::Command => {
                                let _ = control.execute_command(&text);
                            }
                            InputPromptMode::LoadStart => {
                                let _ = control.try_load_torrent(&text, true);
                            }
                            InputPromptMode::LoadNormal => {
                                let _ = control.try_load_torrent(&text, false);
                            }
                            InputPromptMode::Filter => {
                                let win = control
                                    .root_ui
                                    .lock()
                                    .unwrap()
                                    .window_download_list
                                    .clone();
                                let mut win_lock = win.lock().unwrap();
                                let view_arc = win_lock.view.clone();
                                if let Some(v_arc) = &view_arc {
                                    let mut v = v_arc.write().unwrap();
                                    if text.is_empty() {
                                        v.set_filter_temp(lt_common::Object::create_empty());
                                    } else {
                                        let pattern = rt_tui::ui::create_filter_pattern(&text);
                                        v.set_filter_temp(rt_tui::ui::create_filter_command(&pattern));
                                    }
                                    v.filter(None);
                                }
                                win_lock.mark_dirty();
                            }
                            InputPromptMode::None => {}
                        }
                    }
                }
                // Mouse/paste/focus handlers are not required yet; consuming
                // them here keeps the event loop alive and bounded.
                TerminalEvent::Mouse(_)
                | TerminalEvent::Paste(_)
                | TerminalEvent::FocusGained
                | TerminalEvent::FocusLost
                | TerminalEvent::Unknown => {}
            }
        }

        // Render frame to canvas and flush to physical terminal
        {
            let mut ui = control.root_ui.lock().unwrap();
            ui.balance_and_redraw(&mut canvas);
        }

        terminal_backend.flush_canvas(&canvas)?;
    }

    // Ordered shutdown: network -> disk/session commit/lock release. The
    // terminal is restored only after state has been committed.
    control.shutdown().await;
    signal_task.abort();
    terminal_guard.leave()?;
    // `TerminalGuard::leave` already emitted the restore sequence; mark the
    // session so the panic hook and our guard do not repeat it.
    signals::mark_terminal_restored();
    println!("rstorrent shutdown completed cleanly.");

    Ok(())
}
