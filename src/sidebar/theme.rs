//! Catppuccin Mocha, matching herdr's default palette so the sidebar looks
//! native next to it.
use ratatui::style::Color;

pub const SURFACE0: Color = Color::Rgb(49, 50, 68);
pub const OVERLAY0: Color = Color::Rgb(108, 112, 134);
pub const TEXT: Color = Color::Rgb(205, 214, 244);
pub const SUBTEXT0: Color = Color::Rgb(166, 173, 200);
pub const GREEN: Color = Color::Rgb(166, 227, 161);
pub const YELLOW: Color = Color::Rgb(249, 226, 175);
pub const RED: Color = Color::Rgb(243, 139, 168);
pub const TEAL: Color = Color::Rgb(148, 226, 213);

pub const SPINNERS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: u32) -> &'static str {
    SPINNERS[tick as usize % SPINNERS.len()]
}
