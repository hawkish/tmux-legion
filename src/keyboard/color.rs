/// An sRGB colour as authored in the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// QMK's HSV bytes, where hue runs 0-255 over the full circle rather than in
/// degrees. This is a port of Python's `colorsys.rgb_to_hsv` followed by
/// `round(x * 255)`, which is what the reference implementation in ../hid does.
///
/// Python's `round` is ties-to-even and Rust's is half-away-from-zero, so the
/// two can differ by one when `x * 255` lands exactly on .5 — pure yellow is
/// the one such colour in reach (hue 42.5). Value is never affected, since
/// `v * 255` is always an integer. The palette avoids the tie and the tests
/// pin every entry's exact bytes.
pub fn rgb_to_hsv(c: Rgb) -> (u8, u8, u8) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let value = (maxc * 255.0).round() as u8;

    // Greys (including black and white) have no meaningful hue.
    if maxc <= minc {
        return (0, 0, value);
    }

    let delta = maxc - minc;
    let sat = (delta / maxc * 255.0).round() as u8;

    let (rc, gc, bc) = ((maxc - r) / delta, (maxc - g) / delta, (maxc - b) / delta);
    let hue = if r >= maxc {
        bc - gc
    } else if g >= maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    // rem_euclid keeps the negative sixth from the red branch in range.
    let hue = (hue / 6.0).rem_euclid(1.0);

    (((hue * 255.0).round() as u16 % 256) as u8, sat, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five vectors asserted by ../hid/tests/test_analyze.py.
    #[test]
    fn matches_python_reference_vectors() {
        assert_eq!(rgb_to_hsv(Rgb(255, 0, 0)), (0x00, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(Rgb(0, 255, 0)), (0x55, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(Rgb(0, 0, 255)), (0xaa, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(Rgb(255, 255, 255)), (0x00, 0x00, 0xff));
        assert_eq!(rgb_to_hsv(Rgb(0, 0, 0)), (0x00, 0x00, 0x00));
    }

    #[test]
    fn greys_have_zero_hue_and_saturation() {
        assert_eq!(rgb_to_hsv(Rgb(48, 48, 48)), (0x00, 0x00, 0x30));
        assert_eq!(rgb_to_hsv(Rgb(1, 1, 1)), (0x00, 0x00, 0x01));
    }

    /// Every palette entry, so a future palette edit that lands on a rounding
    /// tie is caught rather than silently drifting.
    #[test]
    fn palette_entries_encode_exactly() {
        use crate::keyboard::palette;
        assert_eq!(rgb_to_hsv(palette::BLOCKED), (0x00, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(palette::WORKING), (0x1b, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(palette::DONE), (0x76, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(palette::IDLE), (0x55, 0xff, 0xff));
        assert_eq!(rgb_to_hsv(palette::UNKNOWN), (0xc6, 0xff, 0x78));
        // Saturated blue at value 1: the dimmest write the firmware honours.
        assert_eq!(rgb_to_hsv(palette::OFF), (0xaa, 0xff, 0x01));
    }

    /// Hues must stay far enough apart to read as different colours on a
    /// diffused keycap.
    #[test]
    fn palette_hues_are_distinguishable() {
        use crate::keyboard::palette;
        let hues: Vec<u8> = [
            palette::BLOCKED,
            palette::WORKING,
            palette::DONE,
            palette::IDLE,
            palette::UNKNOWN,
        ]
        .iter()
        .map(|c| rgb_to_hsv(*c).0)
        .collect();
        for (i, a) in hues.iter().enumerate() {
            for b in &hues[i + 1..] {
                let gap = a.abs_diff(*b).min(255 - a.abs_diff(*b));
                assert!(gap >= 20, "hues {a:#04x} and {b:#04x} are too close");
            }
        }
    }
}
