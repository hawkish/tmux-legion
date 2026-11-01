//! The keypad's "page 1": the user's own mx-keypad config (shortcuts, media
//! keys, Danish letters, shift), loaded once when tmux-legion takes the
//! keypad over, so switching to it doesn't give up the everyday layout.
//!
//! This is a thin wrapper: `mx_keypad::session::Session` owns the config and
//! every action it binds (keystrokes, media keys, hold/switch, sequences,
//! shift, long press, blink); this module only loads it once and forwards
//! the keypad's own press and release events to it.

use mx_keypad::config::Setup;
use mx_keypad::render::Face;
use mx_keypad::session::Session;
use mx_keypad::Button;

pub struct UtilityPage {
    /// `None` when the config failed to load or validate — every key is then
    /// blank and inert, same as an unreachable config would leave `run`.
    session: Option<Session>,
}

impl UtilityPage {
    /// Loads `mx-keypad`'s own config from its default path. Read once: a
    /// config edit needs the sidebar reopened to pick it up, same as
    /// `mx-keypad run` needs a restart for everything but the file itself.
    pub fn load() -> UtilityPage {
        UtilityPage {
            session: Setup::load(&mx_keypad::config::default_path())
                .ok()
                .map(Session::new),
        }
    }

    /// The current page's nine faces, following shift and blink.
    pub fn faces(&self) -> [Face; 9] {
        let now = std::time::Instant::now();
        std::array::from_fn(|i| {
            self.session
                .as_ref()
                .map_or_else(Face::default, |s| s.face(i, now))
        })
    }

    /// Key `n` (1–9) went down.
    pub fn press(&mut self, n: u8) {
        if let Some(session) = &mut self.session {
            let _ = session.down(Button::Key(n));
        }
    }

    /// Key `n` (1–9) came up: lets go of a held `hold`/`switch` shortcut, or
    /// a `hold`-mode shift.
    pub fn release(&mut self, n: u8) {
        if let Some(session) = &mut self.session {
            let _ = session.up(Button::Key(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_config_leaves_every_key_blank() {
        let page = UtilityPage { session: None };
        assert_eq!(page.faces(), std::array::from_fn(|_| Face::default()));
    }

    fn setup(json: &str) -> Setup {
        Setup::resolve(
            serde_json::from_str(json).unwrap(),
            std::path::Path::new("."),
        )
        .unwrap()
    }

    #[test]
    fn pressing_a_key_performs_its_action() {
        let mut page = UtilityPage {
            session: Some(Session::new(setup(
                r#"{ "pages": [{ "name": "a", "keys": {
                    "1": { "label": "æ", "action": { "text": "æ" } },
                    "8": { "action": { "shift": "toggle" } }
                } }] }"#,
            ))),
        };
        assert_eq!(page.faces()[0].label.as_deref(), Some("æ"));
        page.press(8);
        assert_eq!(page.faces()[0].label.as_deref(), Some("Æ"));
    }

    /// Regression test: a `hold`-mode shift that never saw a release stuck
    /// shift on permanently once pressed, back when this module performed
    /// actions itself instead of forwarding to a `Session`.
    #[test]
    fn a_hold_shift_releases_when_its_key_comes_up() {
        let mut page = UtilityPage {
            session: Some(Session::new(setup(
                r#"{ "pages": [{ "name": "a", "keys": {
                    "1": { "label": "æ", "action": { "text": "æ" } },
                    "8": { "action": { "shift": "hold" } }
                } }] }"#,
            ))),
        };
        page.press(8);
        assert_eq!(page.faces()[0].label.as_deref(), Some("Æ"));
        page.release(8);
        assert_eq!(page.faces()[0].label.as_deref(), Some("æ"));
    }
}
