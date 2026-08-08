//! LED colours per agent status.
//!
//! **Every colour here needs non-zero saturation and non-zero value.** A write
//! with either at zero is ignored and the key keeps what it had, so a black,
//! white or grey status would silently show stale colour rather than a wrong
//! one. Measured on a Q0 Max: a fully saturated colour renders even at value 1,
//! while any grey renders at no value at all, and `s=255 v=0` is a no-op too.
//! `renders_on_hardware` guards this.
//!
//! A consequence worth knowing: a single key cannot be turned off or dimmed.
//! Brightness is global, so the board is floored to one quiet colour and the
//! agent keys stand out by hue alone — see `configure`.
//!
//! Deliberately not `sidebar::theme`: those are Catppuccin pastels chosen for
//! a terminal, and at ~30% saturation an RGB key reads as dirty white with
//! green and teal nearly indistinguishable. These are the same hues at full
//! saturation, so the keyboard and the sidebar agree perceptually. Keeping the
//! palettes separate also keeps ratatui out of the hardware path.

use crate::keyboard::Rgb;
use crate::status::Status;

/// The dimmest colour the hardware will actually render, used for slots no
/// agent holds and for every key that is not a slot. Value 0 is a no-op — the
/// key keeps whatever it had — so "off" has to be value 1 and the board is
/// floored here rather than darkened.
///
/// Value 1 is not dim: the firmware treats the value byte as a flag, so every
/// lit key renders at the global brightness whatever value it was given. The
/// floor can therefore only be a *colour*, not a darkness. Blue because it is
/// the quietest hue to sit under a numpad and is far from every status colour.
pub const OFF: Rgb = Rgb(0, 0, 1);

/// Reverts a key to the colour stored in the keyboard's own profile, rather
/// than overriding it. The deliberate exception to the rule above: it works
/// *because* saturation and value are zero, which the firmware reads as "drop
/// my override" instead of "paint this". If the stored profile has the key
/// black, this is the only way to make a single key genuinely dark.
pub const REVERT: Rgb = Rgb(0, 0, 0);
pub const BLOCKED: Rgb = Rgb(255, 0, 0);
/// Amber rather than yellow: unmistakably not red at a glance, and it avoids
/// the one hue that rounds differently from the Python reference.
pub const WORKING: Rgb = Rgb(255, 160, 0);
pub const DONE: Rgb = Rgb(0, 255, 200);
pub const IDLE: Rgb = Rgb(0, 255, 0);
/// Occupied but unclassified. Violet because it has to be saturated to render
/// at all, and because it is far from every other hue here — a dim grey would
/// have been the obvious choice and would never have shown up.
pub const UNKNOWN: Rgb = Rgb(80, 0, 120);

pub fn status_color(status: Status) -> Rgb {
    match status {
        Status::Blocked => BLOCKED,
        Status::Working => WORKING,
        Status::Done => DONE,
        Status::Idle => IDLE,
        Status::Unknown => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two keys showing one colour would be a silent misreading, so the five
    /// must stay distinct. Note this says nothing about sidebar::ui::status_color:
    /// that is a deliberately separate table (see the module docs) and nothing
    /// couples the two — they are free to diverge, and only agree by intent.
    #[test]
    fn every_status_maps_to_a_distinct_colour() {
        let all = [
            Status::Working,
            Status::Blocked,
            Status::Done,
            Status::Idle,
            Status::Unknown,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(status_color(*a), status_color(*b), "{a:?} vs {b:?}");
            }
        }
    }

    /// A write with saturation 0 or value 0 is ignored by the firmware and the
    /// key silently keeps its previous colour. That is invisible to any test
    /// that only checks the bytes we send, and it is exactly the mistake this
    /// palette made first time round (black for empty, grey for unknown).
    #[test]
    fn renders_on_hardware() {
        use crate::keyboard::rgb_to_hsv;
        for (name, color) in [
            ("working", status_color(Status::Working)),
            ("blocked", status_color(Status::Blocked)),
            ("done", status_color(Status::Done)),
            ("idle", status_color(Status::Idle)),
            ("unknown", status_color(Status::Unknown)),
            ("off", OFF),
        ] {
            let (_, sat, val) = rgb_to_hsv(color);
            assert!(sat > 0, "{name} is unsaturated and would not render");
            assert!(val > 0, "{name} has value 0 and would not render");
        }
    }

    /// Empty slots must be the dimmest thing that still renders, or the board
    /// floor stops looking like a floor.
    #[test]
    fn off_is_the_dimmest_renderable_value() {
        assert_eq!(crate::keyboard::rgb_to_hsv(OFF).2, 1);
    }

    #[test]
    fn no_status_renders_as_an_empty_slot() {
        for status in [
            Status::Working,
            Status::Blocked,
            Status::Done,
            Status::Idle,
            Status::Unknown,
        ] {
            assert_ne!(status_color(status), OFF, "{status:?} would look empty");
        }
    }
}
