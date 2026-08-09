//! Mirrors agent status onto four keys of a Keychron Q0 Max.
//!
//! Nothing here prints or panics: the only caller is the sidebar, which owns
//! the terminal, so a stray write would corrupt the TUI. Failures are recorded
//! in `Painter::last_error` and the device is retried a few seconds later,
//! which also covers unplugging the keyboard mid-session.
//!
//! The device lives on its own thread. Pacing alone costs several milliseconds
//! per report (see `Device::send`), and the sidebar drives this continuously
//! while an agent blinks. So `Leds` is only a handle: it drops a frame in a
//! mailbox and returns. `Painter`, on the far side, does the USB work and holds
//! all the state worth testing.

mod color;
mod device;
pub mod palette;

pub use color::{rgb_to_hsv, Rgb};

use crate::tmux;
use anyhow::{Context, Result};
use device::Device;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const SLOT_COUNT: usize = 4;

/// Firmware LED indices for the numpad 4/5/6/1 keys, from g_led_config in
/// keyboards/keychron/q0_max/encoder/encoder.c. Ascending, but not contiguous:
/// 18 is the extra-column key left of numpad 1, so the block splits into two
/// runs (see `Painter::apply`).
pub const SLOT_LEDS: [u8; SLOT_COUNT] = [15, 16, 17, 19];

/// The physical key each slot lives on. Shown in the sidebar, since that is
/// what the user presses — the slot number itself is an implementation detail.
pub const SLOT_KEYS: [char; SLOT_COUNT] = ['4', '5', '6', '1'];

/// How long to wait before looking for a keyboard that wasn't there.
const RETRY: Duration = Duration::from_secs(5);

/// Fallback used only by tests; the real default is "leave the backlight
/// alone" (see `Config::from_options`).
#[cfg(test)]
const DEFAULT_BRIGHTNESS: u8 = 0x80;

/// What to write to a slot's key.
pub type SlotColors = [Rgb; SLOT_COUNT];

/// What the keyboard should be set to, read once from tmux options.
#[derive(Clone, Copy)]
struct Config {
    /// Effect to force, or `None` to leave the keyboard's own alone.
    effect: Option<u8>,
    /// Backlight level to force, or `None` to leave it alone — the default.
    /// The backlight is global, so it is the user's only dimming control and
    /// the one they reach for on the keyboard itself; overriding it on every
    /// startup just fights them.
    brightness: Option<u8>,
    /// Whether to floor the non-slot keys. When false the keyboard's own
    /// stored colours are left alone and empty slots revert to them, which is
    /// how you get a genuinely dark board: set the keys black in Launcher once
    /// and tmux-legion only ever lights the agents.
    floor: bool,
}

impl Config {
    /// Reads tmux options, so it belongs on the caller's thread rather than
    /// the worker's — shelling out to tmux is not what the device thread is for.
    fn from_options() -> Config {
        let effect = match tmux::get_option("@legion_led_effect") {
            Some(v) if v.eq_ignore_ascii_case("keep") => None,
            Some(v) => parse_u8(&v).or(Some(device::DEFAULT_EFFECT)),
            None => Some(device::DEFAULT_EFFECT),
        };
        Config {
            effect,
            brightness: tmux::get_option("@legion_led_brightness").and_then(|v| parse_u8(&v)),
            floor: !matches!(
                tmux::get_option("@legion_led_floor").as_deref(),
                Some("keep") | Some("off") | Some("no")
            ),
        }
    }
}

/// The frame waiting to be painted, or the signal to stop.
///
/// Deliberately one slot rather than a queue: a frame that arrives while the
/// worker is mid-batch *replaces* the pending one, because only the newest
/// status is worth showing. A channel cannot express that — a bounded one
/// blocks the UI or drops the new value, and an unbounded one would let the
/// keyboard fall arbitrarily far behind the sidebar.
#[derive(Default)]
struct Pending {
    want: Option<SlotColors>,
    quit: bool,
}

struct Mailbox {
    pending: Mutex<Pending>,
    ready: Condvar,
}

/// A handle on the device thread. Cheap to call: `render` takes a lock, stores
/// a frame and returns, so the terminal never waits on USB.
pub struct Leds {
    mailbox: Arc<Mailbox>,
    /// Published by the worker after each batch, so the sidebar footer can show
    /// a keyboard that stopped answering.
    warning: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
}

impl Leds {
    /// Never fails; a missing keyboard or hidapi just means no LEDs. The device
    /// is opened lazily by the worker, on its first frame.
    pub fn new() -> Leds {
        let config = Config::from_options();
        let mailbox = Arc::new(Mailbox {
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
        });
        let warning = Arc::new(Mutex::new(None));

        let worker = std::thread::Builder::new()
            .name("legion-leds".into())
            .spawn({
                let mailbox = Arc::clone(&mailbox);
                let warning = Arc::clone(&warning);
                move || paint_loop(config, &mailbox, &warning)
            })
            .ok();

        Leds {
            mailbox,
            warning,
            worker,
        }
    }

    /// Hand the worker the frame to paint. Latest wins; an unchanged frame is
    /// discarded on the far side by `Painter::render`.
    pub fn render(&mut self, want: SlotColors) {
        let Ok(mut pending) = self.mailbox.pending.lock() else {
            return; // worker panicked; nothing to be done from here
        };
        pending.want = Some(want);
        self.mailbox.ready.notify_one();
    }

    /// A problem worth showing the user, or None. Stays quiet on machines with
    /// no Keychron attached, which is most of them.
    pub fn warning(&self) -> Option<String> {
        self.warning.lock().ok().and_then(|w| w.clone())
    }

    /// Stop the worker and wait for it to drop the keys back to the floor.
    /// Joining matters: without it the process can exit before that last batch
    /// reaches the device. Idempotent, because both `Drop` and the caller may
    /// invoke it.
    pub fn shutdown(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        if let Ok(mut pending) = self.mailbox.pending.lock() {
            pending.quit = true;
            self.mailbox.ready.notify_one();
        }
        let _ = worker.join();
    }
}

impl Drop for Leds {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Owns the device for the life of the sidebar. Ends by flooring the keys, so
/// a clean quit leaves no stale status showing.
fn paint_loop(config: Config, mailbox: &Mailbox, warning: &Mutex<Option<String>>) {
    let mut painter = Painter::new(config);
    loop {
        let want = {
            let Ok(mut pending) = mailbox.pending.lock() else {
                return;
            };
            loop {
                // Quit wins over a pending frame: on the way out the only
                // paint that matters is the floor.
                if pending.quit {
                    drop(pending);
                    painter.shutdown();
                    return;
                }
                if let Some(want) = pending.want.take() {
                    break want;
                }
                let Ok(next) = mailbox.ready.wait(pending) else {
                    return;
                };
                pending = next;
            }
        };

        // Outside the lock: a batch takes milliseconds and `render` must never
        // block behind it.
        painter.render(want);
        if let Ok(mut slot) = warning.lock() {
            *slot = painter.warning().map(str::to_string);
        }
    }
}

/// The device-side state machine. Everything that talks to hardware or
/// remembers what the hardware was told lives here, on the worker thread.
struct Painter {
    api: Option<hidapi::HidApi>,
    /// Colour last written per slot. `None` forces a repaint, which is how a
    /// fresh handle recovers from whatever the previous process left behind.
    last: [Option<Rgb>; SLOT_COUNT],
    config: Config,
    /// Whether the board has been taken over yet. Cleared on any failure so a
    /// replugged keyboard gets floored again.
    configured: bool,
    next_probe: Instant,
    last_error: Option<String>,
    /// Whether a keyboard was ever found. Until it is, failures are just "no
    /// Keychron plugged in" — the normal case for everyone else — and must
    /// stay silent.
    seen_device: bool,
}

impl Painter {
    /// Builds the `HidApi` here rather than in `Leds::new`, so exactly one
    /// exists and only this thread ever touches it.
    fn new(config: Config) -> Painter {
        let (api, last_error) = match hidapi::HidApi::new() {
            Ok(api) => (Some(api), None),
            Err(e) => (None, Some(format!("hidapi unavailable: {e}"))),
        };
        Painter {
            api,
            last: [None; SLOT_COUNT],
            config,
            configured: false,
            next_probe: Instant::now(),
            last_error,
            seen_device: false,
        }
    }

    fn warning(&self) -> Option<&str> {
        self.seen_device
            .then_some(self.last_error.as_deref())
            .flatten()
    }

    /// Paint the slot keys, if any of them changed. Safe to call for every
    /// frame the sidebar produces: an unchanged one is one comparison per slot
    /// and touches no USB at all, which is what keeps a board of settled agents
    /// silent.
    fn render(&mut self, want: SlotColors) {
        let changed = (0..SLOT_COUNT).any(|i| self.last[i] != Some(want[i]));
        if !changed && self.configured {
            return;
        }
        if Instant::now() < self.next_probe {
            return;
        }
        self.next_probe = Instant::now() + RETRY;

        debug_log(&format!("apply want={want:?} last={:?}", self.last));
        match self.apply(&want) {
            Ok(()) => {
                self.last = want.map(Some);
                self.last_error = None;
                // Nothing failed, so don't sit out the retry window.
                self.next_probe = Instant::now();
                debug_log("  -> ok");
            }
            Err(e) => {
                // Usually the cable came out. Forget everything so the next
                // attempt reconfigures and repaints from scratch.
                debug_log(&format!("  -> {e:#}"));
                self.last_error = Some(format!("{e:#}"));
                self.last = [None; SLOT_COUNT];
                self.configured = false;
            }
        }
    }

    /// Drop the slot keys back to the floor, leaving the whole board one
    /// uniform colour. The effect and brightness stay as we set them — there
    /// is nothing sensible to restore them to, since the stored per-key
    /// colours they applied to are gone.
    fn shutdown(&mut self) {
        if !self.configured {
            return;
        }
        // Clearing `configured` is what makes a second call a no-op; the cost
        // is that `apply` reconfigures on the way out, which is a fair price
        // once per quit.
        self.configured = false;
        self.last = [None; SLOT_COUNT];
        let result = self.apply(&[palette::OFF; SLOT_COUNT]);
        // The only trace of the last batch: without it a clean quit looks
        // identical to a blink that happened to end on the unlit phase.
        debug_log(&match result {
            Ok(()) => "shutdown -> ok".to_string(),
            Err(e) => format!("shutdown -> {e:#}"),
        });
    }

    /// One batch of work on its own handle.
    ///
    /// The handle is deliberately not kept: after a couple of dozen colour
    /// reports the keyboard keeps acking writes but stops acting on them, and
    /// only a reopen clears it. A batch is three reports (LED 18 splits the
    /// slots into two runs, plus a commit), which makes an open per batch far
    /// cheaper than the alternative — LEDs that silently freeze after a while.
    fn apply(&mut self, want: &SlotColors) -> Result<()> {
        let api = self.api.as_mut().context("hidapi unavailable")?;

        if !self.configured {
            // hidapi caches the device list, so a keyboard plugged in after
            // startup is invisible until this. Only the discovery path needs
            // it: once configured, the cached path is still the right one.
            // Every way the device can go away — unplug, a failed write —
            // clears `configured`, so the retry re-enumerates anyway.
            // Enumerating on every batch would mean a full HID sweep twice a
            // second while anything is blinking.
            api.refresh_devices()
                .context("cannot enumerate HID devices")?;

            let dev = Device::open(api)?;
            self.seen_device = true;
            configure(
                &dev,
                self.config.effect,
                self.config.brightness,
                self.config.floor,
            )?;
            self.configured = true;
            // The floor is the burst that poisons a handle, so start fresh.
            drop(dev);
            api.refresh_devices().ok();
        }

        let dev = Device::open(api)?;
        let painted: Vec<(u8, device::Hsv)> = want
            .iter()
            .enumerate()
            .map(|(i, color)| {
                // Without a floor, an empty slot hands the key back to the
                // keyboard's own colour instead of painting it — the only way a
                // single key can end up dark.
                let color = if !self.config.floor && *color == palette::OFF {
                    palette::REVERT
                } else {
                    *color
                };
                (SLOT_LEDS[i], rgb_to_hsv(color))
            })
            .collect();
        for (start, colors) in device::runs(&painted) {
            dev.set_leds(start, &colors)?;
        }
        dev.commit()
    }
}

/// Take the board over: a per-key effect so our colours render at all, a
/// global brightness, and every non-slot key floored to one quiet colour so
/// the agent keys are the only ones that differ.
///
/// A key cannot be individually darkened. The value byte behaves as a flag
/// rather than a level — value 0 is ignored and leaves the key alone, and
/// every non-zero value renders identically at whatever the global brightness
/// is — so the floor can only change hue, never dim. Switching the effect off
/// darkens the board but renders nothing at all, per-key colours included.
///
/// This overwrites the keyboard's stored per-key colours, and the protocol
/// offers no way to read them first, so it cannot be undone from here.
fn configure(dev: &Device, effect: Option<u8>, brightness: Option<u8>, floor: bool) -> Result<()> {
    if let Some(effect) = effect {
        // Per-key colours are stored and acked even when nothing renders
        // them, so a wrong effect looks exactly like a broken write.
        dev.via_set(device::VIA_RGB_EFFECT, effect)?;
    }
    if let Some(brightness) = brightness {
        dev.via_set(device::VIA_RGB_BRIGHTNESS, brightness)?;
    }

    if !floor {
        return Ok(());
    }
    let color = rgb_to_hsv(palette::OFF);
    let painted: Vec<(u8, device::Hsv)> = (0..=device::MAX_LED)
        // The slots are painted by the caller, on a fresh handle.
        .filter(|led| !SLOT_LEDS.contains(led))
        .map(|led| (led, color))
        .collect();
    for (start, colors) in device::runs(&painted) {
        dev.set_leds(start, &colors)?;
    }
    dev.commit()
}

/// Drop the slot keys to the floor from a process that is not the sidebar.
/// Call this right after killing the sidebar pane: the sidebar dies on SIGHUP
/// without unwinding, so this is the only cleanup that runs on that path, and
/// without it the keys keep showing the last agent's status forever. Silent
/// and best-effort — no keyboard just means no work.
///
/// The dying sidebar still holds the device exclusively for a moment, hence
/// the short retry.
pub fn cleanup() {
    let Ok(mut api) = hidapi::HidApi::new() else {
        return;
    };
    for attempt in 0..CLEANUP_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(CLEANUP_BACKOFF);
        }
        if api.refresh_devices().is_err() {
            continue;
        }
        let Ok(dev) = Device::open(&api) else {
            continue;
        };
        for led in SLOT_LEDS {
            let _ = dev.set_led(led, rgb_to_hsv(palette::OFF));
        }
        let _ = dev.commit();
        return;
    }
}

const CLEANUP_ATTEMPTS: u32 = 6;
const CLEANUP_BACKOFF: Duration = Duration::from_millis(150);

/// Appends to $TMUX_LEGION_LED_DEBUG when set. The sidebar owns the terminal
/// and cannot print, so this is the only way to see what the LED path did.
fn debug_log(line: &str) {
    let Some(path) = std::env::var_os("TMUX_LEGION_LED_DEBUG") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Accepts decimal or 0x-prefixed hex, matching how these ids are documented.
fn parse_u8(value: &str) -> Option<u8> {
    let v = value.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => v.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first three slots spelled out, the rest left at OFF. These tests are
    /// about the diffing, not about how many slots there are, so naming only
    /// the slots they exercise keeps them readable as SLOT_COUNT grows.
    fn colors(a: Rgb, b: Rgb, c: Rgb) -> SlotColors {
        let mut out: SlotColors = [palette::OFF; SLOT_COUNT];
        out[..3].copy_from_slice(&[a, b, c]);
        out
    }

    /// A Painter that believes it is already set up, so `render` decides purely
    /// on the diff. Built by hand rather than via `new`, which would open
    /// hidapi — none of these tests want a real device.
    fn configured_painter() -> Painter {
        Painter {
            api: None, // any attempt to reach a device fails loudly
            last: [None; SLOT_COUNT],
            config: Config {
                effect: None,
                brightness: Some(DEFAULT_BRIGHTNESS),
                floor: true,
            },
            configured: true,
            next_probe: Instant::now(),
            last_error: None,
            seen_device: false,
        }
    }

    /// What keeps a board of settled agents silent. The redraw loop calls this
    /// several times a second and the device stops responding when written to
    /// too often, so an unchanged frame must not even look for a keyboard.
    ///
    /// It is no longer the only limit: the sidebar's blink deliberately alters
    /// the frame for working and blocked agents, so those are rate-limited by
    /// `sidebar::BLINK_PHASE` instead.
    #[test]
    fn unchanged_frame_touches_no_device() {
        let mut leds = configured_painter();
        let want = colors(palette::IDLE, palette::WORKING, palette::OFF);
        leds.last = want.map(Some);

        leds.render(want);

        assert!(leds.last_error.is_none(), "should not have tried to open");
        assert_eq!(leds.last, want.map(Some));
    }

    #[test]
    fn a_changed_slot_triggers_a_write() {
        let mut leds = configured_painter();
        let before = colors(palette::IDLE, palette::WORKING, palette::OFF);
        leds.last = before.map(Some);

        leds.render(colors(palette::IDLE, palette::BLOCKED, palette::OFF));

        // No hidapi, so the attempt fails — but it was made, which is the point.
        assert!(leds.last_error.is_some(), "a change must reach the device");
    }

    /// After a failure everything is forgotten, so the next attempt
    /// reconfigures and repaints rather than trusting stale bookkeeping.
    #[test]
    fn failure_forgets_state_so_the_retry_is_a_full_repaint() {
        let mut leds = configured_painter();
        leds.last = [Some(palette::IDLE); SLOT_COUNT];

        leds.render(colors(palette::BLOCKED, palette::BLOCKED, palette::BLOCKED));

        assert_eq!(leds.last, [None; SLOT_COUNT]);
        assert!(!leds.configured);
    }

    /// Failures are silent until a keyboard has actually been seen, so people
    /// without one never get a warning about hardware they do not own.
    #[test]
    fn warning_stays_quiet_until_a_device_is_found() {
        let mut leds = configured_painter();
        leds.render(colors(palette::BLOCKED, palette::OFF, palette::OFF));
        assert!(leds.last_error.is_some());
        assert_eq!(leds.warning(), None, "no keyboard, no complaint");

        leds.seen_device = true;
        assert!(leds.warning().is_some());
    }

    #[test]
    fn slot_leds_are_within_firmware_range() {
        // Out-of-range writes are accepted and silently dropped, so this
        // mistake would have no runtime symptom at all.
        for led in SLOT_LEDS {
            assert!(led <= device::MAX_LED, "LED {led} out of range");
        }
    }

    /// `device::runs` groups adjacent LEDs only when they arrive in order, so
    /// unsorted slots would quietly cost a report each.
    #[test]
    fn slot_leds_are_sorted_so_they_can_share_a_report() {
        assert!(SLOT_LEDS.windows(2).all(|w| w[0] < w[1]));
    }

    /// The plugin binds one key per slot in shell, where it cannot see
    /// `SLOT_COUNT`. Raising the constant without touching that loop would
    /// leave the extra slots lit but unreachable, with nothing to notice it.
    #[test]
    fn the_shell_binding_loop_stops_at_the_last_slot() {
        let plugin = include_str!("../../tmux-legion.tmux");
        let cap = plugin
            .lines()
            .find_map(|l| l.trim().strip_prefix("[ \"$slot\" -le "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<usize>().ok())
            .expect("tmux-legion.tmux has no `[ \"$slot\" -le N ]` cap");
        assert_eq!(cap, SLOT_COUNT, "tmux-legion.tmux caps slots at {cap}");
    }

    /// Reports, not bytes, are what the device is slow at — 8 ms of pacing
    /// each, and a couple of dozen poisons a handle. Both real call sites are
    /// pinned here so a regression in `MAX_RUN` or the splitter shows up as a
    /// number rather than as a keyboard that feels sluggish.
    #[test]
    fn the_real_frames_cost_the_reports_we_think_they_do() {
        let color = rgb_to_hsv(palette::OFF);

        let slots: Vec<(u8, device::Hsv)> = SLOT_LEDS.iter().map(|&led| (led, color)).collect();
        assert_eq!(
            device::runs(&slots).len(),
            2,
            "a slot repaint is two runs plus a commit — three reports; \
             LED 18 sits between numpad 6 and numpad 1"
        );

        let floor: Vec<(u8, device::Hsv)> = (0..=device::MAX_LED)
            .filter(|led| !SLOT_LEDS.contains(led))
            .map(|led| (led, color))
            .collect();
        assert_eq!(
            device::runs(&floor).len(),
            4,
            "the floor burst is four runs plus a commit — five reports, was 24"
        );
    }

    /// A handle with nobody on the far side, so frames pile up in the mailbox
    /// where a test can look at them.
    fn detached_leds() -> Leds {
        Leds {
            mailbox: Arc::new(Mailbox {
                pending: Mutex::new(Pending::default()),
                ready: Condvar::new(),
            }),
            warning: Arc::new(Mutex::new(None)),
            worker: None,
        }
    }

    /// The mailbox holds one frame, not a queue. A sidebar that blinks faster
    /// than the device can paint must not build a backlog of stale statuses.
    #[test]
    fn a_new_frame_replaces_the_one_still_waiting() {
        let mut leds = detached_leds();
        let stale = colors(palette::WORKING, palette::OFF, palette::OFF);
        let fresh = colors(palette::BLOCKED, palette::OFF, palette::OFF);

        leds.render(stale);
        leds.render(fresh);

        let pending = leds.mailbox.pending.lock().unwrap();
        assert_eq!(pending.want, Some(fresh), "the stale frame must be gone");
    }

    #[test]
    fn shutdown_without_a_worker_is_a_no_op() {
        let mut leds = detached_leds();
        leds.shutdown();
        leds.shutdown(); // Drop will make a third call
    }

    /// The whole handle lifecycle on a machine with no keyboard, which is the
    /// common case. Really a deadlock check: `shutdown` joins the worker, so a
    /// mailbox that failed to wake it would hang here rather than fail.
    #[test]
    fn a_worker_starts_paints_and_stops_without_a_keyboard() {
        let mut leds = Leds::new();
        assert!(leds.worker.is_some(), "worker thread did not start");

        leds.render(colors(palette::BLOCKED, palette::OFF, palette::OFF));
        leds.shutdown();

        assert!(leds.worker.is_none(), "shutdown must consume the handle");
        assert_eq!(leds.warning(), None, "no keyboard, no complaint");
    }

    /// Hardware smoke test: drives a real keyboard and reports what the
    /// device path actually did. Ignored by default since it needs a Q0 Max on
    /// USB with the sidebar closed.
    /// `cargo test --release hardware -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hardware_smoke() {
        // Painter rather than Leds: synchronous, so the assertions below are
        // about a batch that has actually been sent.
        let mut painter = Painter::new(Config {
            effect: Some(device::DEFAULT_EFFECT),
            brightness: None,
            floor: true,
        });
        println!("after new(): error={:?}", painter.last_error);

        painter.render(colors(palette::BLOCKED, palette::IDLE, palette::DONE));
        println!(
            "after render(): configured={} error={:?} last={:?}",
            painter.configured, painter.last_error, painter.last
        );
        assert!(
            painter.configured,
            "not configured: {:?}",
            painter.last_error
        );
        assert_eq!(
            painter.last[0],
            Some(palette::BLOCKED),
            "slot 1 not painted"
        );
    }

    /// Hardware experiment: does byte 3 of the set-key payload really take a
    /// count? Paints the three slot keys red, green and blue in a **single**
    /// report, which is the one thing `MAX_RUN = 1` never does.
    ///
    /// This cannot assert — the firmware accepts and silently drops writes it
    /// does not understand, so the only readout is the keyboard itself. Look at
    /// the numpad after running it:
    ///
    /// - red, green, blue  => the count works; set `device::MAX_RUN` to RUN_LIMIT
    /// - only `4` changed, or nothing changed => it does not; MAX_RUN stays 1
    ///
    /// Needs a Q0 Max on USB with the sidebar closed and Launcher quit.
    /// `cargo test --release count_field -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn count_field_paints_a_run() {
        let mut api = hidapi::HidApi::new().expect("hidapi unavailable");
        api.refresh_devices().ok();
        let dev = Device::open(&api).expect("no keyboard (wired mode only)");

        // Without the right effect nothing renders at all, which would look
        // exactly like the count field failing.
        configure(&dev, Some(device::DEFAULT_EFFECT), None, true).expect("configure");
        drop(dev);
        api.refresh_devices().ok();
        let dev = Device::open(&api).expect("reopen after the floor burst");

        let run = [
            rgb_to_hsv(Rgb(255, 0, 0)),
            rgb_to_hsv(Rgb(0, 255, 0)),
            rgb_to_hsv(Rgb(0, 0, 255)),
        ];
        dev.set_leds(SLOT_LEDS[0], &run).expect("set_leds");
        dev.commit().expect("commit");

        println!("\nSent LEDs {:?} as ONE report.", &SLOT_LEDS[..run.len()]);
        println!("Now look at the numpad 4/5/6 keys:");
        println!(
            "  red green blue -> count works, set MAX_RUN to {}",
            device::RUN_LIMIT
        );
        println!("  only 4 red, or no change -> count is ignored, MAX_RUN stays 1");
    }

    #[test]
    fn option_values_parse_as_decimal_or_hex() {
        assert_eq!(parse_u8("23"), Some(23));
        assert_eq!(parse_u8("0x17"), Some(23));
        assert_eq!(parse_u8(" 0x17 "), Some(23));
        assert_eq!(parse_u8("300"), None);
        assert_eq!(parse_u8("nonsense"), None);
    }
}
