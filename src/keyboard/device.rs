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

    pub fn set_led(&self, led: u8, hsv: (u8, u8, u8)) -> Result<()> {
        self.send(&set_led_payload(led, hsv))
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

pub fn set_led_payload(led: u8, hsv: (u8, u8, u8)) -> [u8; 7] {
    debug_assert!(
        led <= MAX_LED,
        "LED {led} is out of range and would be ignored"
    );
    // Byte 3 is a key count; the protocol may accept a run of LEDs in one
    // report, but that is unverified and the render diff already keeps traffic
    // to near zero, so we always send exactly one.
    [CMD_KEYCHRON, SUB_SET_KEY, led, 0x01, hsv.0, hsv.1, hsv.2]
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
