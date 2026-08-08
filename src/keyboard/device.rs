//! The Keychron raw-HID wire protocol.
//!
//! Reverse-engineered in ../hid by hooking `HIDDevice.sendReport` while
//! driving Keychron Launcher; see that repo's README if firmware ever moves
//! the offsets. Per-key colour is a Keychron vendor extension (`0xA8`), not
//! VIA — VIA's own command space stops at 0x15 — so only the effect and
//! brightness calls below are standard.

use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};

const VID: u16 = 0x3434;
/// QMK's raw HID / VIA vendor interface. A Keychron exposes several HID
/// interfaces and only this one speaks the protocol.
const USAGE_PAGE: u16 = 0xFF60;
const USAGE: u16 = 0x61;

/// QMK's RAW_EPSIZE. Every report is exactly this long, zero-padded.
pub const REPORT_LEN: usize = 32;

const CMD_KEYCHRON: u8 = 0xA8;
const SUB_SET_KEY: u8 = 0x0A;
const SUB_COMMIT: u8 = 0x02;

const VIA_SET: u8 = 0x07;
const VIA_RGB_CHANNEL: u8 = 0x03;
pub const VIA_RGB_BRIGHTNESS: u8 = 0x01;
pub const VIA_RGB_EFFECT: u8 = 0x02;

/// Milliseconds to wait after each report. See `Device::send`.
const PACE_MS: i32 = 8;

/// Hue, saturation, value as the wire carries them.
pub type Hsv = (u8, u8, u8);

/// Most LEDs one report can physically carry: the payload less its four-byte
/// header, three bytes of HSV each.
pub const RUN_LIMIT: usize = (REPORT_LEN - 4) / 3;

/// How many LEDs we actually put in one report.
///
/// Byte 3 of the set-key payload is a count, and the firmware honours it:
/// confirmed on a Q0 Max by `count_field_paints_a_run`, which paints three keys
/// three colours from a single report. Keychron Launcher never does this — every
/// report in ../hid/capture.jsonl sends one key — so it is not something the
/// capture could have told us.
///
/// It matters because reports, not bytes, are the scarce resource: the device
/// needs 8 ms between them (see `Device::send`) and stops acting on colour
/// writes after a couple of dozen on one handle. Filling the report takes the
/// floor burst in `configure` from 24 reports to 4, and a slot repaint from 4
/// to 2.
///
/// Drop this to 1 to get the old one-key-per-report behaviour back; everything
/// downstream is written for runs and degrades to exactly that.
pub const MAX_RUN: usize = RUN_LIMIT;

/// Highest LED index in the Q0 Max's g_led_config. Writes past it are accepted
/// and silently dropped by firmware, which is the worst kind of failure.
pub const MAX_LED: u8 = 25;

/// The per-key effect measured on a Q0 Max (0x3434:0x0800, VIA protocol 12)
/// with per-key colours confirmed rendering. A firmware update can move this;
/// `@legion_led_effect` overrides it. Note RGB_MATRIX_SOLID_COLOR would *not*
/// work here — solid-colour mode paints every LED one HSV and ignores per-key
/// values, so it looks like a wrong colour rather than an obvious failure.
pub const DEFAULT_EFFECT: u8 = 0x17;

pub struct Device {
    handle: HidDevice,
}

impl Device {
    pub fn open(api: &HidApi) -> Result<Device> {
        let info = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.usage_page() == USAGE_PAGE && d.usage() == USAGE)
            // Backends that don't parse report descriptors report usage_page 0.
            // QMK exposes raw HID as the highest interface number, so fall back
            // to that rather than giving up. Heuristic, hence second.
            .or_else(|| {
                api.device_list()
                    .filter(|d| d.vendor_id() == VID && d.usage_page() == 0)
                    .max_by_key(|d| d.interface_number())
            })
            .context("no Keychron raw-HID interface (wired mode only)")?;

        let handle = api
            .open_path(info.path())
            .context("cannot open the keyboard (is Keychron Launcher connected?)")?;
        Ok(Device { handle })
    }

    /// Fire-and-forget, but paced. The keyboard acks a colour report and then
    /// silently ignores it if the next one arrives too quickly, so reports
    /// need a gap between them — writes that "succeed" but never light a key
    /// are what this timeout buys. The reference implementation in ../hid gets
    /// the same effect accidentally, by blocking 200 ms on a reply that never
    /// comes; that would cost seconds per repaint here, so the wait is short
    /// and doubles as the drain for the reply we do not expect.
    fn send(&self, payload: &[u8]) -> Result<()> {
        self.handle
            .write(&frame(payload)?)
            .context("HID write failed")?;
        let mut scratch = [0u8; REPORT_LEN];
        let _ = self.handle.read_timeout(&mut scratch, PACE_MS);
        Ok(())
    }

    pub fn set_led(&self, led: u8, hsv: Hsv) -> Result<()> {
        self.send(&set_led_payload(led, hsv))
    }

    /// Paint `hsv.len()` consecutive LEDs starting at `start`, in one report.
    /// Deliberately not clamped to `MAX_RUN` — that is the splitter's job, and
    /// the `count_field` experiment needs to exceed it.
    pub fn set_leds(&self, start: u8, hsv: &[Hsv]) -> Result<()> {
        self.send(&set_leds_payload(start, hsv)?)
    }

    pub fn commit(&self) -> Result<()> {
        self.send(&commit_payload())
    }

    pub fn via_set(&self, value_id: u8, data: u8) -> Result<()> {
        self.send(&via_set_payload(value_id, data))
    }
}

/// The 33 bytes handed to hid_write: report id 0x00, then exactly REPORT_LEN
/// payload bytes zero-padded. The protocol itself carries no report id — QMK
/// raw HID always uses 0.
pub fn frame(payload: &[u8]) -> Result<[u8; REPORT_LEN + 1]> {
    if payload.len() > REPORT_LEN {
        bail!("payload is {} bytes, max {REPORT_LEN}", payload.len());
    }
    let mut out = [0u8; REPORT_LEN + 1];
    out[1..=payload.len()].copy_from_slice(payload);
    Ok(out)
}

/// The single-key form, kept as its own function because these exact seven
/// bytes are what was captured from Launcher.
pub fn set_led_payload(led: u8, hsv: Hsv) -> [u8; 7] {
    debug_assert!(
        led <= MAX_LED,
        "LED {led} is out of range and would be ignored"
    );
    [CMD_KEYCHRON, SUB_SET_KEY, led, 0x01, hsv.0, hsv.1, hsv.2]
}

/// The same payload with byte 3 as a real count and one HSV triple per LED.
/// See `MAX_RUN` for why nothing sends more than one yet.
pub fn set_leds_payload(start: u8, hsv: &[Hsv]) -> Result<Vec<u8>> {
    if hsv.is_empty() {
        bail!("a run needs at least one LED");
    }
    if hsv.len() > RUN_LIMIT {
        bail!("a run of {} LEDs does not fit in one report", hsv.len());
    }
    debug_assert!(
        start as usize + hsv.len() - 1 <= MAX_LED as usize,
        "run from LED {start} runs past {MAX_LED} and would be ignored"
    );
    let mut out = vec![CMD_KEYCHRON, SUB_SET_KEY, start, hsv.len() as u8];
    out.extend(hsv.iter().flat_map(|c| [c.0, c.1, c.2]));
    Ok(out)
}

/// Group LEDs that can share one report: `(first LED, its colours)`.
///
/// A run breaks on a gap in the LED indices and at `MAX_RUN`. Input must be
/// sorted by LED index; anything out of order simply starts a new run, so the
/// worst an unsorted caller gets is the traffic it would have had anyway.
pub fn runs(painted: &[(u8, Hsv)]) -> Vec<(u8, Vec<Hsv>)> {
    let mut out: Vec<(u8, Vec<Hsv>)> = Vec::new();
    for &(led, hsv) in painted {
        match out.last_mut() {
            Some((start, colors))
                if colors.len() < MAX_RUN && Some(led) == start.checked_add(colors.len() as u8) =>
            {
                colors.push(hsv);
            }
            _ => out.push((led, vec![hsv])),
        }
    }
    out
}

pub fn commit_payload() -> [u8; 2] {
    [CMD_KEYCHRON, SUB_COMMIT]
}

fn via_set_payload(value_id: u8, data: u8) -> [u8; 4] {
    [VIA_SET, VIA_RGB_CHANNEL, value_id, data]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_is_report_id_then_padded_payload() {
        let f = frame(&[0xA8, 0x02]).unwrap();
        assert_eq!(f.len(), REPORT_LEN + 1);
        assert_eq!(f[0], 0x00, "report id");
        assert_eq!(&f[1..3], &[0xA8, 0x02]);
        assert!(f[3..].iter().all(|b| *b == 0), "tail must be zero-padded");
    }

    #[test]
    fn frame_rejects_oversized_payloads_instead_of_truncating() {
        assert!(frame(&[0u8; REPORT_LEN]).is_ok());
        assert!(frame(&[0u8; REPORT_LEN + 1]).is_err());
    }

    /// The exact bytes captured from Keychron Launcher for numpad 5 in red.
    #[test]
    fn set_led_matches_the_captured_packet() {
        assert_eq!(
            set_led_payload(16, (0x00, 0xff, 0xff)),
            [0xA8, 0x0A, 0x10, 0x01, 0x00, 0xff, 0xff]
        );
        assert_eq!(
            set_led_payload(17, (0x55, 0xff, 0xff)),
            [0xA8, 0x0A, 0x11, 0x01, 0x55, 0xff, 0xff]
        );
        assert_eq!(commit_payload(), [0xA8, 0x02]);
    }

    /// A run of one has to be the packet Launcher sends, or MAX_RUN = 1 would
    /// not be the no-op it is meant to be.
    #[test]
    fn a_run_of_one_is_the_captured_packet() {
        assert_eq!(
            set_leds_payload(16, &[(0x00, 0xff, 0xff)]).unwrap(),
            set_led_payload(16, (0x00, 0xff, 0xff)).to_vec()
        );
    }

    #[test]
    fn a_run_carries_the_count_and_every_triple() {
        assert_eq!(
            set_leds_payload(15, &[(0, 255, 255), (85, 255, 255), (170, 255, 255)]).unwrap(),
            vec![0xA8, 0x0A, 15, 3, 0, 255, 255, 85, 255, 255, 170, 255, 255]
        );
    }

    #[test]
    fn a_run_must_fit_in_one_report() {
        let color = (0, 255, 255);
        assert!(set_leds_payload(0, &[color; RUN_LIMIT]).is_ok());
        assert!(set_leds_payload(0, &[color; RUN_LIMIT + 1]).is_err());
        assert!(set_leds_payload(0, &[]).is_err());
    }

    /// Whatever RUN_LIMIT works out to, a full run still has to frame.
    #[test]
    fn a_full_run_still_fits_a_report() {
        let payload = set_leds_payload(0, &[(0, 255, 255); RUN_LIMIT]).unwrap();
        assert!(payload.len() <= REPORT_LEN);
        assert!(frame(&payload).is_ok());
    }

    #[test]
    fn runs_break_on_a_gap() {
        let c = (0, 255, 255);
        // MAX_RUN is 1 until the hardware says otherwise, so pin the grouping
        // to it rather than to a hardcoded shape.
        let painted = [(15, c), (16, c), (18, c)];
        let got = runs(&painted);
        match MAX_RUN {
            1 => assert_eq!(got.len(), 3, "one report each"),
            _ => assert_eq!(
                got,
                vec![(15, vec![c, c]), (18, vec![c])],
                "15-16 adjacent, 18 after a gap"
            ),
        }
    }

    #[test]
    fn runs_break_at_max_run() {
        let c = (0, 255, 255);
        let painted: Vec<(u8, Hsv)> = (0..=MAX_LED).map(|led| (led, c)).collect();
        let got = runs(&painted);
        assert!(
            got.iter().all(|(_, colors)| colors.len() <= MAX_RUN),
            "no run may exceed MAX_RUN"
        );
        // Every LED is painted exactly once, in order, whatever the grouping.
        let flat: Vec<u8> = got
            .iter()
            .flat_map(|(start, colors)| (0..colors.len()).map(move |i| start + i as u8))
            .collect();
        assert_eq!(flat, (0..=MAX_LED).collect::<Vec<u8>>());
    }

    #[test]
    fn runs_of_nothing_is_nothing() {
        assert!(runs(&[]).is_empty());
    }

    #[test]
    fn via_payloads() {
        assert_eq!(
            via_set_payload(VIA_RGB_EFFECT, 0x17),
            [0x07, 0x03, 0x02, 0x17]
        );
        assert_eq!(
            via_set_payload(VIA_RGB_BRIGHTNESS, 0x30),
            [0x07, 0x03, 0x01, 0x30]
        );
    }
}
