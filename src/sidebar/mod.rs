mod app;
mod ui;

use crate::state::{self, Store};
use crate::tmux;
use anyhow::Result;
use crossterm::event::{self, Event};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Reconcile against live panes every N poll iterations (~2 s).
const RECONCILE_TICKS: u32 = 8;

/// Unregisters the sidebar from tmux even on panic; ratatui's init handles
/// terminal restore via its own panic hook.
struct Registration;

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = tmux::unset_option("@legion_pid");
        let _ = tmux::unset_option("@legion_pane");
    }
}

pub fn run() -> Result<()> {
    let store = Store::for_current_server()?;

    tmux::set_option("@legion_pid", &std::process::id().to_string())?;
    if let Some(pane) = tmux::current_pane() {
        let _ = tmux::set_option("@legion_pane", &pane);
    }
    let _registration = Registration;

    let redraw = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(&redraw))?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &store, &redraw);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    store: &Store,
    redraw: &AtomicBool,
) -> Result<()> {
    let mut app = app::App::new();
    let _ = state::reconcile(store);
    app.reload(store);
    let mut ticks: u32 = 0;

    loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // A key event, a SIGUSR1 poke, or the periodic tick each trigger one
        // reload+redraw per iteration, which coalesces bursts of pokes.
        if event::poll(POLL_TIMEOUT)? {
            if let Event::Key(key) = event::read()? {
                match app.handle_key(key, store) {
                    app::Outcome::Quit => return Ok(()),
                    app::Outcome::Reconcile => {
                        let _ = state::reconcile(store);
                        app.reload(store);
                    }
                    app::Outcome::Continue => {}
                }
            }
        }

        ticks = ticks.wrapping_add(1);
        if redraw.swap(false, Ordering::Relaxed) {
            app.reload(store);
        } else if ticks.is_multiple_of(RECONCILE_TICKS) {
            let _ = state::reconcile(store);
            app.reload(store);
        }
    }
}
