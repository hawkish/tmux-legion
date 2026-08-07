mod app;
mod theme;
mod ui;

use crate::keyboard::{palette, Leds, SlotColors, SLOT_COUNT};
use crate::state::{self, AgentEntry, Store};
use crate::tmux;
use anyhow::Result;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
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

    // SIGHUP is deliberately left at its default disposition. Handling it so
    // Drop could blank the LEDs does not work: once the pane dies, crossterm's
    // event::poll spins on EOF and never returns, so no flag the loop checks is
    // ever observed — the process just burns a core forever. Killing the pane
    // must kill us. Whoever does the killing blanks the keys instead; see
    // keyboard::cleanup and commands::toggle.

    let mut terminal = ratatui::init();
    // Route clicks in this pane to us instead of tmux, so list rows are
    // clickable. tmux forwards mouse events to a pane in mouse mode.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &store, &redraw);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// The colour each slot's key should show. Slots nobody holds go dark.
fn slot_colors(entries: &[AgentEntry]) -> SlotColors {
    let mut colors: SlotColors = [palette::OFF; SLOT_COUNT];
    for entry in entries {
        let Some(slot) = entry.slot else { continue };
        let Some(cell) = (slot as usize)
            .checked_sub(1)
            .and_then(|i| colors.get_mut(i))
        else {
            continue;
        };
        // During the grace window the agent is already gone; a confident green
        // on a dying agent would be misleading.
        *cell = if entry.exited_at.is_some() {
            palette::UNKNOWN
        } else {
            palette::status_color(entry.status)
        };
    }
    colors
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    store: &Store,
    redraw: &AtomicBool,
) -> Result<()> {
    let mut app = app::App::new();
    let mut leds = Leds::new();
    let _ = state::reconcile(store);
    app.reload(store);
    let mut ticks: u32 = 0;

    loop {
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // A key event, a SIGUSR1 poke, or the periodic tick each trigger one
        // reload+redraw per iteration, which coalesces bursts of pokes.
        if event::poll(POLL_TIMEOUT)? {
            let outcome = match event::read()? {
                Event::Key(key) => Some(app.handle_key(key, store)),
                Event::Mouse(m) => Some(app.handle_mouse(m, terminal.size()?.height)),
                _ => None,
            };
            match outcome {
                Some(app::Outcome::Quit) => return Ok(()),
                Some(app::Outcome::Reconcile) => {
                    let _ = state::reconcile(store);
                    app.reload(store);
                }
                Some(app::Outcome::Continue) | None => {}
            }
        }

        ticks = ticks.wrapping_add(1);
        // A poke or the periodic tick both reconcile: pokes from the
        // pane-exited/session-closed hooks have no state-file write behind them,
        // so a bare reload would keep showing an agent whose pane just closed.
        // The atomic flag coalesces poke bursts to one reconcile per poll window,
        // and reconcile preserves Hook/Reported status, so this stays cheap.
        if redraw.swap(false, Ordering::Relaxed) || ticks.is_multiple_of(RECONCILE_TICKS) {
            let _ = state::reconcile(store);
            app.reload(store);
        }

        // Unconditional: all the gating lives inside Leds, so an unchanged
        // frame costs three comparisons and no USB traffic. Deliberately not
        // in the draw path — the spinner advances every iteration, and the
        // keys must not flicker with it.
        leds.render(slot_colors(&app.entries));
        app.led_warning = leds.warning().is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{Source, Status};

    fn entry(pane: &str, status: Status, slot: Option<u8>) -> AgentEntry {
        let mut e = AgentEntry::new(pane, "claude", status, Source::Hook);
        e.slot = slot;
        e
    }

    /// OFF releases the key rather than darkening it — the hardware has no
    /// "off" for a single key.
    #[test]
    fn unheld_slots_are_released() {
        let colors = slot_colors(&[entry("%1", Status::Working, Some(2))]);
        assert_eq!(colors[0], palette::OFF);
        assert_eq!(colors[1], palette::WORKING);
        assert_eq!(colors[2], palette::OFF);
    }

    #[test]
    fn exited_agents_show_as_unknown_not_their_last_status() {
        let mut e = entry("%1", Status::Idle, Some(1));
        e.exited_at = Some(crate::state::now());
        assert_eq!(slot_colors(&[e])[0], palette::UNKNOWN);
    }

    #[test]
    fn out_of_range_slots_are_ignored_rather_than_panicking() {
        let colors = slot_colors(&[
            entry("%1", Status::Working, Some(0)),
            entry("%2", Status::Working, Some(9)),
            entry("%3", Status::Blocked, Some(3)),
        ]);
        assert_eq!(colors, [palette::OFF, palette::OFF, palette::BLOCKED]);
    }

    #[test]
    fn every_status_reaches_the_keys() {
        for (status, expected) in [
            (Status::Working, palette::WORKING),
            (Status::Blocked, palette::BLOCKED),
            (Status::Done, palette::DONE),
            (Status::Idle, palette::IDLE),
            (Status::Unknown, palette::UNKNOWN),
        ] {
            assert_eq!(slot_colors(&[entry("%1", status, Some(1))])[0], expected);
        }
    }
}
