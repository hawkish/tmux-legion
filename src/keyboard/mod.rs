//! Mirrors agent status onto three keys of a Keychron Q0 Max.
//!
//! Nothing here prints or panics: the only caller is the sidebar, which owns
//! the terminal, so a stray write would corrupt the TUI. Failures are recorded
//! in `Leds::last_error` and the device is retried a few seconds later, which
//! also covers unplugging the keyboard mid-session.

mod color;
mod device;
pub mod palette;

pub use color::{rgb_to_hsv, Rgb};

use crate::tmux;
use anyhow::{Context, Result};
use device::Device;
use std::time::{Duration, Instant};

pub const SLOT_COUNT: usize = 3;

/// Firmware LED indices for the numpad 4/5/6 keys, from g_led_config in
/// keyboards/keychron/q0_max/encoder/encoder.c.
pub const SLOT_LEDS: [u8; SLOT_COUNT] = [15, 16, 17];

/// The physical key each slot lives on. Shown in the sidebar, since that is
/// what the user presses — the slot number itself is an implementation detail.
pub const SLOT_KEYS: [char; SLOT_COUNT] = ['4', '5', '6'];

/// How long to wait before looking for a keyboard that wasn't there.
const RETRY: Duration = Duration::from_secs(5);

/// Fallback used only by tests; the real default is "leave the backlight
/// alone" (see `Leds::new`).
#[cfg(test)]
const DEFAULT_BRIGHTNESS: u8 = 0x80;

/// What to write to a slot's key.
pub type SlotColors = [Rgb; SLOT_COUNT];

pub struct Leds {
    api: Option<hidapi::HidApi>,
    /// Colour last written per slot. `None` forces a repaint, which is how a
    /// fresh handle recovers from whatever the previous process left behind.
    last: [Option<Rgb>; SLOT_COUNT],
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

impl Leds {
    /// Never fails; a missing keyboard or hidapi just means no LEDs. The
    /// device is opened lazily on the first render.
    pub fn new() -> Leds {
        let effect = match tmux::get_option("@legion_led_effect") {
            Some(v) if v.eq_ignore_ascii_case("keep") => None,
            Some(v) => parse_u8(&v).or(Some(device::DEFAULT_EFFECT)),
            None => Some(device::DEFAULT_EFFECT),
        };
        let brightness = tmux::get_option("@legion_led_brightness").and_then(|v| parse_u8(&v));
        let floor = !matches!(
            tmux::get_option("@legion_led_floor").as_deref(),
            Some("keep") | Some("off") | Some("no")
        );

        let (api, last_error) = match hidapi::HidApi::new() {
            Ok(api) => (Some(api), None),
            Err(e) => (None, Some(format!("hidapi unavailable: {e}"))),
        };

        Leds {
            api,
            last: [None; SLOT_COUNT],
            effect,
            brightness,
            floor,
            configured: false,
            next_probe: Instant::now(),
            last_error,
            seen_device: false,
        }
    }

    /// A problem worth showing the user, or None. Stays quiet on machines with
    /// no Keychron attached, which is most of them.
    pub fn warning(&self) -> Option<&str> {
        self.seen_device
            .then_some(self.last_error.as_deref())
            .flatten()
    }

    /// Paint the slot keys, if any of them changed. Safe to call every loop
    /// iteration: an unchanged frame is three comparisons and touches no USB
    /// at all, which is what keeps the working spinner from hammering the
    /// device.
    pub fn render(&mut self, want: SlotColors) {
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
    /// colours they applied to are gone. Idempotent, because both `Drop` and
    /// the caller may invoke it.
    pub fn shutdown(&mut self) {
        if !self.configured {
            return;
        }
        self.configured = false;
        self.last = [None; SLOT_COUNT];
        let _ = self.apply(&[palette::OFF; SLOT_COUNT]);
    }

    /// One batch of work on its own handle.
    ///
    /// The handle is deliberately not kept: after a couple of dozen colour
    /// reports the keyboard keeps acking writes but stops acting on them, and
    /// only a reopen clears it. Since a batch is at most three keys plus a
    /// commit — and only happens when a status actually changes — paying for
    /// an open each time is far cheaper than the alternative, which is LEDs
    /// that silently freeze after a while.
    fn apply(&mut self, want: &SlotColors) -> Result<()> {
        let api = self.api.as_mut().context("hidapi unavailable")?;
        // hidapi caches the device list; without this a keyboard plugged in
        // after startup is never seen.
        api.refresh_devices()
            .context("cannot enumerate HID devices")?;

        if !self.configured {
            let dev = Device::open(api)?;
            self.seen_device = true;
            configure(&dev, self.effect, self.brightness, self.floor)?;
            self.configured = true;
            // The floor is the burst that poisons a handle, so start fresh.
            drop(dev);
            api.refresh_devices().ok();
        }

        let dev = Device::open(api)?;
        for (i, color) in want.iter().enumerate() {
            // Without a floor, an empty slot hands the key back to the
            // keyboard's own colour instead of painting it — the only way a
            // single key can end up dark.
            let color = if !self.floor && *color == palette::OFF {
                palette::REVERT
            } else {
                *color
            };
            dev.set_led(SLOT_LEDS[i], rgb_to_hsv(color))?;
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
    for led in 0..=device::MAX_LED {
        if SLOT_LEDS.contains(&led) {
            continue; // painted by the caller, on a fresh handle
        }
        dev.set_led(led, color)?;
    }
    dev.commit()
}

impl Drop for Leds {
    fn drop(&mut self) {
        self.shutdown();
    }
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

    fn colors(a: Rgb, b: Rgb, c: Rgb) -> SlotColors {
        [a, b, c]
    }

    /// A Leds that believes it is already set up, so `render` decides purely
    /// on the diff. Built by hand rather than via `new`, which would open
    /// hidapi — several of those at once from the test threads aborts the
    /// process, and none of these tests want a real device anyway.
    fn configured_leds() -> Leds {
        Leds {
            api: None, // any attempt to reach a device fails loudly
            last: [None; SLOT_COUNT],
            effect: None,
            brightness: Some(DEFAULT_BRIGHTNESS),
            floor: true,
            configured: true,
            next_probe: Instant::now(),
            last_error: None,
            seen_device: false,
        }
    }

    /// The diff is the only thing standing between the 4 Hz redraw loop and a
    /// device that stops responding when written to to often, so an unchanged
    /// frame must not even look for a keyboard.
    #[test]
    fn unchanged_frame_touches_no_device() {
        let mut leds = configured_leds();
        let want = colors(palette::IDLE, palette::WORKING, palette::OFF);
        leds.last = want.map(Some);

        leds.render(want);

        assert!(leds.last_error.is_none(), "should not have tried to open");
        assert_eq!(leds.last, want.map(Some));
    }

    #[test]
    fn a_changed_slot_triggers_a_write() {
        let mut leds = configured_leds();
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
        let mut leds = configured_leds();
        leds.last = [Some(palette::IDLE); SLOT_COUNT];

        leds.render(colors(palette::BLOCKED, palette::BLOCKED, palette::BLOCKED));

        assert_eq!(leds.last, [None; SLOT_COUNT]);
        assert!(!leds.configured);
    }

    /// Failures are silent until a keyboard has actually been seen, so people
    /// without one never get a warning about hardware they do not own.
    #[test]
    fn warning_stays_quiet_until_a_device_is_found() {
        let mut leds = configured_leds();
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

    /// Hardware smoke test: drives a real keyboard and reports what the
    /// device path actually did. Ignored by default since it needs a Q0 Max on
    /// USB with the sidebar closed.
    /// `cargo test --release hardware -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hardware_smoke() {
        let mut leds = Leds::new();
        println!("after new(): error={:?}", leds.last_error);

        leds.render([palette::BLOCKED, palette::IDLE, palette::DONE]);
        println!(
            "after render(): configured={} error={:?} last={:?}",
            leds.configured, leds.last_error, leds.last
        );
        assert!(leds.configured, "not configured: {:?}", leds.last_error);
        assert_eq!(leds.last[0], Some(palette::BLOCKED), "slot 1 not painted");
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
