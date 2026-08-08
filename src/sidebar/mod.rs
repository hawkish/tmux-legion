mod app;
mod theme;
mod ui;

use crate::keyboard::{palette, Leds, SlotColors, SLOT_COUNT};
use crate::state::{self, AgentEntry, Store};
use crate::status::Status;
use crate::tmux;
use anyhow::Result;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Reconcile against live panes on a wall clock rather than a loop-iteration
/// count: the loop runs faster than its poll timeout whenever input arrives or
/// a blink edge is due, and the reconcile cadence should not follow it.
const RECONCILE_EVERY: Duration = Duration::from_secs(2);
/// Half of the blink cycle: keys for live agents alternate between their status
/// colour and the floor once a second.
const BLINK_PHASE: Duration = Duration::from_millis(500);

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

/// True on the lit half of the blink cycle. Driven by the wall clock rather
/// than a loop counter: the loop iterates faster than its poll timeout whenever
/// input arrives, and a blink that speeds up while you scroll looks like a
/// fault.
fn blink_lit(elapsed: Duration) -> bool {
    (elapsed.as_millis() / BLINK_PHASE.as_millis()).is_multiple_of(2)
}

/// How long the current blink phase has left to run.
fn blink_remaining(elapsed: Duration) -> Duration {
    BLINK_PHASE - Duration::from_millis((elapsed.as_millis() % BLINK_PHASE.as_millis()) as u64)
}

/// Whether a key would change colour at the next blink edge. False means the
/// loop can go back to sleeping for the full poll timeout.
fn any_blinking(entries: &[AgentEntry]) -> bool {
    entries.iter().any(blinks)
}

/// Live agents blink; settled ones hold their colour. Working and blocked are
/// the two states that mean something is happening right now, and they are the
/// ones worth catching out of the corner of your eye.
fn blinks(entry: &AgentEntry) -> bool {
    entry.slot.is_some()
        && entry.exited_at.is_none()
        && matches!(entry.status, Status::Working | Status::Blocked)
}

/// The colour each slot's key should show. Slots nobody holds go dark, and on
/// the unlit half of the cycle so do the blinking ones — the hardware has no
/// per-key brightness, so alternating with the floor colour is the only blink
/// available (see palette::OFF).
fn slot_colors(entries: &[AgentEntry], lit: bool) -> SlotColors {
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
        // on a dying agent would be misleading, and a blinking one doubly so.
        *cell = if entry.exited_at.is_some() {
            palette::UNKNOWN
        } else if blinks(entry) && !lit {
            palette::OFF
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
    let started = Instant::now();
    let mut last_reconcile = Instant::now();

    loop {
        app.spinner_tick = app.spinner_tick.wrapping_add(1);
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        // Wake on the blink edge rather than after it. The loop is not
        // phase-locked to the blink grid, so sampling it at the poll rate would
        // land each edge up to a poll late and make the two halves visibly
        // uneven. With nothing blinking this is the poll timeout as before.
        let timeout = match any_blinking(&app.entries) {
            true => POLL_TIMEOUT.min(blink_remaining(started.elapsed())),
            false => POLL_TIMEOUT,
        };

        // A key event, a SIGUSR1 poke, or the periodic tick each trigger one
        // reload+redraw per iteration, which coalesces bursts of pokes.
        if event::poll(timeout)? {
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

        // A poke or the periodic tick both reconcile: pokes from the
        // pane-exited/session-closed hooks have no state-file write behind them,
        // so a bare reload would keep showing an agent whose pane just closed.
        // The atomic flag coalesces poke bursts to one reconcile per poll window,
        // and reconcile preserves Hook/Reported status, so this stays cheap.
        if redraw.swap(false, Ordering::Relaxed) || last_reconcile.elapsed() >= RECONCILE_EVERY {
            last_reconcile = Instant::now();
            let _ = state::reconcile(store);
            app.reload(store);
        }

        // Unconditional: all the gating lives inside Leds, so an unchanged
        // frame costs three comparisons and no USB traffic. Deliberately not
        // in the draw path — the spinner advances every iteration, and the
        // keys must not flicker with it.
        leds.render(slot_colors(&app.entries, blink_lit(started.elapsed())));
        app.led_warning = leds.warning().is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Source;

    fn entry(pane: &str, status: Status, slot: Option<u8>) -> AgentEntry {
        let mut e = AgentEntry::new(pane, "claude", status, Source::Hook);
        e.slot = slot;
        e
    }

    /// OFF releases the key rather than darkening it — the hardware has no
    /// "off" for a single key.
    #[test]
    fn unheld_slots_are_released() {
        let colors = slot_colors(&[entry("%1", Status::Working, Some(2))], true);
        assert_eq!(colors[0], palette::OFF);
        assert_eq!(colors[1], palette::WORKING);
        assert_eq!(colors[2], palette::OFF);
    }

    #[test]
    fn exited_agents_show_as_unknown_not_their_last_status() {
        let mut e = entry("%1", Status::Idle, Some(1));
        e.exited_at = Some(crate::state::now());
        assert_eq!(slot_colors(&[e], true)[0], palette::UNKNOWN);
    }

    #[test]
    fn out_of_range_slots_are_ignored_rather_than_panicking() {
        let colors = slot_colors(
            &[
                entry("%1", Status::Working, Some(0)),
                entry("%2", Status::Working, Some(9)),
                entry("%3", Status::Blocked, Some(3)),
            ],
            true,
        );
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
            assert_eq!(
                slot_colors(&[entry("%1", status, Some(1))], true)[0],
                expected
            );
        }
    }

    /// The unlit half is the floor colour, which is also what an empty slot
    /// shows — that indistinguishability is the blink, since a single key
    /// cannot be dimmed.
    #[test]
    fn live_statuses_go_dark_on_the_unlit_phase() {
        for status in [Status::Working, Status::Blocked] {
            let one = [entry("%1", status, Some(1))];
            assert_ne!(slot_colors(&one, true)[0], palette::OFF, "{status:?}");
            assert_eq!(slot_colors(&one, false)[0], palette::OFF, "{status:?}");
        }
    }

    #[test]
    fn settled_statuses_ignore_the_blink_phase() {
        for status in [Status::Done, Status::Idle, Status::Unknown] {
            let one = [entry("%1", status, Some(1))];
            assert_eq!(
                slot_colors(&one, true),
                slot_colors(&one, false),
                "{status:?}"
            );
        }
    }

    /// An agent in the grace window is already gone. Blinking it would advertise
    /// activity it cannot have.
    #[test]
    fn exited_agents_do_not_blink() {
        let mut e = entry("%1", Status::Working, Some(1));
        e.exited_at = Some(crate::state::now());
        let one = [e];
        assert_eq!(slot_colors(&one, true)[0], palette::UNKNOWN);
        assert_eq!(slot_colors(&one, false)[0], palette::UNKNOWN);
    }

    /// Nothing to blink means the loop keeps its full poll timeout.
    #[test]
    fn only_live_slotted_agents_drive_the_blink() {
        assert!(any_blinking(&[entry("%1", Status::Working, Some(1))]));
        assert!(any_blinking(&[entry("%1", Status::Blocked, Some(1))]));
        assert!(!any_blinking(&[entry("%1", Status::Idle, Some(1))]));
        assert!(!any_blinking(&[entry("%1", Status::Working, None)]));
    }

    #[test]
    fn blink_alternates_on_the_phase_boundary() {
        let lit = |ms| blink_lit(Duration::from_millis(ms));
        assert!(lit(0));
        assert!(lit(499));
        assert!(!lit(500));
        assert!(!lit(999));
        assert!(lit(1000));
    }

    /// The loop sleeps up to the next edge, never past it.
    #[test]
    fn blink_remaining_counts_down_to_the_next_edge() {
        let left = |ms| blink_remaining(Duration::from_millis(ms));
        assert_eq!(left(0), BLINK_PHASE);
        assert_eq!(left(400), Duration::from_millis(100));
        assert_eq!(left(500), BLINK_PHASE);
        assert_eq!(left(900), Duration::from_millis(100));
    }
}
