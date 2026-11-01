mod app;
mod theme;
mod ui;

use crate::keyboard::{self, palette, Leds, SlotColors, SLOT_COUNT};
use crate::power::{self, Power};
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
/// Reconcile against live panes on a wall clock, not a loop-iteration count.
/// The loop runs faster than its poll timeout whenever input arrives or a blink
/// edge is due, and the reconcile cadence should not follow it.
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

/// The face each MX Creative Keypad key (1–9) should show. Keys past the
/// Keychron's four are the only place slots 5-9 ever render, so every agent
/// still gets a key — nothing past slot 9 does.
///
/// Unlike the Keychron's single colour per key, the keypad can show the
/// agent's icon too, so a blink dims to black rather than disappearing to the
/// empty-slot floor: the slot stays identifiable through the blink.
fn keypad_faces(entries: &[AgentEntry], lit: bool) -> keyboard::KeypadFaces {
    let mut faces = std::array::from_fn(|_| keyboard::empty_face());
    for entry in entries {
        let Some(slot) = entry.slot else { continue };
        let Some(cell) = (slot as usize)
            .checked_sub(1)
            .and_then(|i| faces.get_mut(i))
        else {
            continue;
        };
        let background = if entry.exited_at.is_some() {
            palette::UNKNOWN
        } else {
            palette::status_color(entry.status)
        };
        let dimmed = entry.exited_at.is_none() && blinks(entry) && !lit;
        *cell = keyboard::agent_face(background, icon_key(entry), dimmed);
    }
    faces
}

/// What picks an entry's icon: its model's family when known, so a Claude
/// session shows the same icon everywhere it runs — opencode, Copilot CLI,
/// anywhere — rather than one icon per launcher. Falls back to the agent's
/// own name when no model is known, which is also what makes `claude` and
/// `codex` entries (which run their own, eponymous CLI and whose model is
/// not always known) show their own icon by default.
fn icon_key(entry: &AgentEntry) -> &str {
    entry
        .model
        .as_deref()
        .and_then(model_family)
        .unwrap_or(&entry.name)
}

/// A short icon-folder key for a model name, matched by substring since
/// labels vary a lot across agents and providers ("claude-opus-5",
/// "Claude Sonnet 5", "o3", "gpt-5.5", "GPT-6 Astra"). `None` when the
/// family isn't recognised, so the caller falls back to the agent's name.
fn model_family(model: &str) -> Option<&'static str> {
    let lower = model.to_lowercase();
    if lower.contains("claude") {
        return Some("claude");
    }
    let openai = lower.contains("gpt") || lower.contains("codex")
        // Reasoning-model short names: "o3", "o4-mini", not words that merely
        // start with "o", by requiring a digit right after it.
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| {
                word.strip_prefix('o')
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            });
    openai.then_some("codex")
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    store: &Store,
    redraw: &AtomicBool,
) -> Result<()> {
    let mut app = app::App::new();
    let mut leds = Leds::new();
    // None off macOS, and on a Mac where registering failed. Either way the
    // loop below behaves as it always did.
    let power = power::watch();
    let mut asleep = false;
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
        let had_input = event::poll(timeout)?;
        if had_input {
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

        // Between the notification and the machine actually suspending there
        // are a few hundred milliseconds of ordinary loop iterations, so this
        // has to come before the reconcile and the render below: acking a
        // sleep and then repainting would light the keys again on the way out.
        if let Some(rx) = &power {
            while let Ok(event) = rx.try_recv() {
                match event {
                    Power::Sleeping => {
                        asleep = true;
                        leds.sleep(); // blocks until the keys are dark
                        power::allow_sleep();
                    }
                    Power::Woke => {
                        asleep = false;
                        leds.wake();
                        // Panes and agents may not have survived. Reconcile
                        // before the next frame rather than showing a sidebar
                        // full of things that died while we were frozen.
                        last_reconcile = Instant::now();
                        let _ = state::reconcile(store);
                        app.reload(store);
                    }
                }
            }
        }

        // A wake notification that never arrives would leave the keys dark for
        // the rest of the session, which is a far worse failure than a stray
        // repaint. Input is proof the machine is running, so trust it.
        if asleep && had_input {
            asleep = false;
            leds.wake();
        }

        // Nothing below here is worth doing on a machine that is suspending.
        if asleep {
            continue;
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
        // frame costs one comparison per slot and no USB traffic. Deliberately
        // not in the draw path — the spinner advances every iteration, and the
        // keys must not flicker with it.
        let lit = blink_lit(started.elapsed());
        leds.render(keyboard::Frame {
            leds: slot_colors(&app.entries, lit),
            keypad: Some(keypad_faces(&app.entries, lit)),
        });
        app.led_warning = leds.warning().is_some();

        // Presses from the MX Creative Keypad, if the backend found one: the
        // same jump a Keychron slot key does, just arriving over a channel
        // instead of a terminal key event (the keypad never has terminal
        // focus).
        for slot in leds.take_presses() {
            app.focus_slot(slot);
        }
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
        assert_eq!(colors[3], palette::OFF);
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
        assert_eq!(
            colors,
            [palette::OFF, palette::OFF, palette::BLOCKED, palette::OFF]
        );
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

    /// Slots past the Keychron's four are the only place 5-9 ever render, and
    /// an empty one is a plain dark key, same shape as `slot_colors`.
    #[test]
    fn keypad_faces_place_agents_on_their_own_slot() {
        let faces = keypad_faces(&[entry("%1", Status::Working, Some(6))], true);
        assert_eq!(
            faces[5],
            keyboard::agent_face(palette::WORKING, "claude", false)
        );
        assert_eq!(faces[0], keyboard::empty_face());
        assert_eq!(faces[8], keyboard::empty_face());
    }

    /// Unlike the Keychron, a blinking keypad key keeps its icon and only the
    /// background goes dark, so the slot stays identifiable through the blink.
    #[test]
    fn keypad_faces_dim_rather_than_empty_on_the_unlit_phase() {
        let one = [entry("%1", Status::Blocked, Some(3))];
        assert_eq!(
            keypad_faces(&one, true)[2],
            keyboard::agent_face(palette::BLOCKED, "claude", false)
        );
        assert_eq!(
            keypad_faces(&one, false)[2],
            keyboard::agent_face(palette::BLOCKED, "claude", true)
        );
    }

    /// An exited agent shows violet without blinking, same as the Keychron.
    #[test]
    fn keypad_faces_show_exited_agents_as_unknown_and_steady() {
        let mut e = entry("%1", Status::Working, Some(1));
        e.exited_at = Some(crate::state::now());
        let one = [e];
        assert_eq!(
            keypad_faces(&one, true)[0],
            keyboard::agent_face(palette::UNKNOWN, "claude", false)
        );
        assert_eq!(
            keypad_faces(&one, false)[0],
            keyboard::agent_face(palette::UNKNOWN, "claude", false)
        );
    }

    /// The model picks the icon, not the launcher: an opencode or Copilot CLI
    /// session running Claude shows the Claude icon, same as Claude Code
    /// itself.
    #[test]
    fn keypad_faces_pick_the_icon_by_model_not_by_launcher() {
        let mut e = entry("%1", Status::Idle, Some(1));
        e.name = "opencode".into();
        e.model = Some("Claude Sonnet 5".into());
        assert_eq!(
            keypad_faces(&[e], true)[0],
            keyboard::agent_face(palette::IDLE, "claude", false)
        );

        let mut e = entry("%1", Status::Idle, Some(1));
        e.name = "copilot".into();
        e.model = Some("gpt-5.5".into());
        assert_eq!(
            keypad_faces(&[e], true)[0],
            keyboard::agent_face(palette::IDLE, "codex", false)
        );

        // o3, o4-mini etc: an "o" followed by a digit, not any word starting
        // with "o".
        let mut e = entry("%1", Status::Idle, Some(1));
        e.model = Some("o3".into());
        assert_eq!(
            keypad_faces(&[e], true)[0],
            keyboard::agent_face(palette::IDLE, "codex", false)
        );
        let mut e = entry("%1", Status::Idle, Some(1));
        e.model = Some("opus-5".into());
        assert_ne!(
            keypad_faces(&[e.clone()], true)[0],
            keyboard::agent_face(palette::IDLE, "codex", false)
        );
    }

    /// An unrecognised or absent model falls back to the agent's own name,
    /// which is how `claude` and `codex` entries get their own icon before
    /// a model is ever known.
    #[test]
    fn keypad_faces_fall_back_to_the_agent_name_without_a_known_model() {
        let one = [entry("%1", Status::Idle, Some(1))]; // name "claude", no model
        assert_eq!(
            keypad_faces(&one, true)[0],
            keyboard::agent_face(palette::IDLE, "claude", false)
        );

        let mut e = entry("%1", Status::Idle, Some(1));
        e.name = "opencode".into();
        e.model = Some("Llama 4".into());
        assert_eq!(
            keypad_faces(&[e], true)[0],
            keyboard::agent_face(palette::IDLE, "opencode", false)
        );
    }
}
