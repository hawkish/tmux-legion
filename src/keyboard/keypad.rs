//! The MX Creative Keypad backend: a second device on the same `HidApi`,
//! painted from the same frame as the Keychron. `mx-keypad` owns the wire
//! protocol and the hand-over claim; this module only decides what each of
//! the keypad's nine keys shows and relays its presses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mx_keypad::claim::Claim;
use mx_keypad::render::{self, Face, IconSource};
use mx_keypad::Keypad;

use super::color::Rgb;

/// Keys on the MX Creative Keypad's panel.
pub const KEY_COUNT: usize = 9;

/// Built-in icon shown for an agent whose name matches nothing in the icons
/// folder, or when there is no folder configured.
const FALLBACK_ICON: &str = "square-terminal";

/// What one of the keypad's nine keys shows.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyFace {
    /// No agent holds this slot: a plain dark key.
    Empty,
    /// An agent's slot: `background` is its status colour, already decided
    /// by the sidebar (including the exited violet); `icon` is the agent's
    /// name, which the painter resolves to a file or the built-in fallback
    /// and caches. `dimmed` is the unlit half of a working/blocked agent's
    /// blink: the background goes black, same as the Keychron, while the
    /// icon stays up so the slot is still identifiable.
    Agent {
        background: Rgb,
        icon: String,
        dimmed: bool,
    },
}

pub type KeypadFaces = [KeyFace; KEY_COUNT];

fn to_mx_rgb(c: Rgb) -> render::Rgb {
    render::Rgb(c.0, c.1, c.2)
}

/// White on a dark background, black on a light one, by sRGB luma — the same
/// rule a status badge would use, cheap enough to run per key per frame.
fn readable_on(bg: Rgb) -> render::Rgb {
    let luma = 0.299 * f32::from(bg.0) + 0.587 * f32::from(bg.1) + 0.114 * f32::from(bg.2);
    if luma > 140.0 {
        render::Rgb(0, 0, 0)
    } else {
        render::Rgb(255, 255, 255)
    }
}

impl KeyFace {
    /// The face `mx_keypad::render::render_tile` draws for this key, resolving
    /// its icon through `icons`.
    fn face(&self, icons: &mut IconCache) -> Face {
        match self {
            KeyFace::Empty => Face {
                background: render::Rgb(0, 0, 0),
                ..Face::default()
            },
            KeyFace::Agent {
                background,
                icon,
                dimmed,
            } => {
                let background = if *dimmed { Rgb(0, 0, 0) } else { *background };
                let background = to_mx_rgb(background);
                Face {
                    icon: Some(icons.resolve(icon)),
                    background,
                    foreground: readable_on(Rgb(background.0, background.1, background.2)),
                    ..Face::default()
                }
            }
        }
    }
}

/// Resolves an agent's name to an icon file in `@legion_keypad_icons`, or the
/// built-in fallback, and remembers the answer: the folder is read at most
/// once per name per process.
struct IconCache {
    dir: Option<PathBuf>,
    cache: HashMap<String, IconSource>,
}

impl IconCache {
    fn new(dir: Option<PathBuf>) -> IconCache {
        IconCache {
            dir,
            cache: HashMap::new(),
        }
    }

    fn resolve(&mut self, name: &str) -> IconSource {
        if let Some(found) = self.cache.get(name) {
            return found.clone();
        }
        let found = self
            .dir
            .as_deref()
            .and_then(|dir| find_icon_file(dir, name))
            .map(IconSource::File)
            .unwrap_or_else(|| IconSource::Builtin(FALLBACK_ICON.to_string()));
        self.cache.insert(name.to_string(), found.clone());
        found
    }
}

/// `<dir>/<name>.{png,svg,jpg,jpeg}`, the first one that exists.
fn find_icon_file(dir: &Path, name: &str) -> Option<PathBuf> {
    ["png", "svg", "jpg", "jpeg"]
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
}

/// How long to wait before looking for a keypad that wasn't there, or that
/// mx-keypad hasn't yielded yet.
const RETRY: Duration = Duration::from_secs(1);

/// Paints the MX Creative Keypad from the same `HidApi` as the Keychron,
/// taking the hand-over claim while it does.
pub struct KeypadPainter {
    keypad: Option<Keypad>,
    claim: Option<Claim>,
    icons: IconCache,
    last: [Option<KeyFace>; KEY_COUNT],
    next_probe: Instant,
    configured: bool,
}

impl KeypadPainter {
    pub fn new(icons_dir: Option<PathBuf>) -> KeypadPainter {
        KeypadPainter {
            keypad: None,
            claim: None,
            icons: IconCache::new(icons_dir),
            last: [const { None }; KEY_COUNT],
            next_probe: Instant::now(),
            configured: false,
        }
    }

    /// Paint the keys that changed since the last call. Safe to call every
    /// frame: an unchanged key costs one comparison and no USB traffic.
    pub fn render(&mut self, api: &mut hidapi::HidApi, want: &KeypadFaces) {
        if Instant::now() < self.next_probe {
            return;
        }
        if self.keypad.is_none() && !self.open(api) {
            self.next_probe = Instant::now() + RETRY;
            return;
        }
        for (i, face) in want.iter().enumerate() {
            if self.last[i].as_ref() == Some(face) {
                continue;
            }
            let tile = match render::render_tile(&face.face(&mut self.icons), geometry_key_size())
            {
                Ok(tile) => tile,
                Err(_) => continue,
            };
            let drawn = self
                .keypad
                .as_mut()
                .is_some_and(|k| k.draw(u8::try_from(i + 1).unwrap_or(1), &tile).is_ok());
            if drawn {
                self.last[i] = Some(face.clone());
            } else {
                // Lost the device mid-batch: forget everything, let the next
                // frame reopen and reconfigure from scratch.
                self.keypad = None;
                self.configured = false;
                self.claim = None;
                self.last = [const { None }; KEY_COUNT];
                self.next_probe = Instant::now() + RETRY;
                return;
            }
        }
    }

    /// Take the keypad over: acquire the claim, wait for mx-keypad to yield,
    /// then open it. `false` means try again later.
    fn open(&mut self, api: &mut hidapi::HidApi) -> bool {
        if self.claim.is_none() {
            self.claim = Claim::acquire("tmux-legion").ok();
        }
        if mx_keypad::claim::foreign().is_some() {
            // mx-keypad hasn't yielded yet.
            return false;
        }
        let _ = api.refresh_devices();
        match Keypad::open(api) {
            Ok(Some(keypad)) => {
                self.keypad = Some(keypad);
                self.configured = true;
                true
            }
            _ => false,
        }
    }

    /// Read key presses without blocking; empty when nothing has come in.
    /// `mx_keypad::Button::Key(n)` is the only kind this returns, since the
    /// keypad's two page buttons aren't diverted here.
    pub fn poll_presses(&mut self) -> Vec<u8> {
        let Some(keypad) = self.keypad.as_mut() else {
            return Vec::new();
        };
        let Ok(events) = keypad.poll(Duration::ZERO) else {
            return Vec::new();
        };
        events
            .into_iter()
            .filter_map(|event| match event {
                mx_keypad::Event::Down(mx_keypad::Button::Key(n)) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// Blank the keys and drop the device, releasing the claim so mx-keypad
    /// can take it back.
    pub fn blank(&mut self) {
        if let Some(keypad) = self.keypad.as_mut() {
            let _ = keypad.clear();
        }
        self.keypad = None;
        self.configured = false;
        self.claim = None;
        self.last = [const { None }; KEY_COUNT];
    }

    /// Forget the board and repaint at the next opportunity, same as waking
    /// the Keychron.
    pub fn wake(&mut self) {
        self.next_probe = Instant::now();
    }
}

/// `mx_keypad::geometry::DISPLAY.key_size`, named locally so this module reads
/// as being about faces, not about the wire geometry.
fn geometry_key_size() -> u16 {
    mx_keypad::geometry::DISPLAY.key_size
}

/// The background each keypad slot should show. Mirrors
/// `sidebar::slot_colors`'s rules (status colour, exited violet, dark empty),
/// but carries an agent name along for the icon and keeps every status its
/// own colour rather than collapsing to one hue per state — the keypad's
/// screen can show the name, so blocked vs. working need not share anything
/// beyond the Keychron's palette.
pub fn empty_face() -> KeyFace {
    KeyFace::Empty
}

pub fn agent_face(background: Rgb, icon: &str, dimmed: bool) -> KeyFace {
    KeyFace::Agent {
        background,
        icon: icon.to_string(),
        dimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::palette;

    #[test]
    fn icon_cache_falls_back_without_a_folder() {
        let mut icons = IconCache::new(None);
        assert_eq!(
            icons.resolve("claude"),
            IconSource::Builtin(FALLBACK_ICON.to_string())
        );
    }

    #[test]
    fn icon_cache_finds_a_matching_file_and_remembers_it() {
        let dir = std::env::temp_dir().join(format!("tmux-legion-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.png");
        std::fs::write(&path, b"not a real png, just a marker").unwrap();

        let mut icons = IconCache::new(Some(dir.clone()));
        assert_eq!(icons.resolve("claude"), IconSource::File(path.clone()));
        // Removing the file afterwards proves the cache, not the disk, answered.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(icons.resolve("claude"), IconSource::File(path));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn icon_cache_falls_back_for_an_unknown_name() {
        let dir = std::env::temp_dir().join(format!("tmux-legion-icons2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut icons = IconCache::new(Some(dir.clone()));
        assert_eq!(
            icons.resolve("nobody"),
            IconSource::Builtin(FALLBACK_ICON.to_string())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn readable_foreground_follows_background_luma() {
        assert_eq!(readable_on(Rgb(0, 0, 0)), render::Rgb(255, 255, 255));
        assert_eq!(readable_on(Rgb(255, 255, 255)), render::Rgb(0, 0, 0));
    }

    #[test]
    fn a_dimmed_agent_face_renders_a_black_background() {
        let mut icons = IconCache::new(None);
        let face = agent_face(palette::BLOCKED, "claude", true);
        assert_eq!(face.face(&mut icons).background, render::Rgb(0, 0, 0));
        let face = agent_face(palette::BLOCKED, "claude", false);
        assert_eq!(
            face.face(&mut icons).background,
            to_mx_rgb(palette::BLOCKED)
        );
    }

    #[test]
    fn an_empty_key_has_no_icon() {
        let mut icons = IconCache::new(None);
        assert!(empty_face().face(&mut icons).icon.is_none());
    }

    /// Hardware smoke test: drives a real MX Creative Keypad and reports what
    /// the keypad painter actually did. Ignored by default since it needs a
    /// keypad on USB with mx-keypad's LaunchAgent either stopped or willing to
    /// yield (give it a tick or two for the claim hand-over).
    /// `cargo test --release keypad::hardware -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn hardware_smoke() {
        let mut api = hidapi::HidApi::new().expect("hidapi unavailable");
        let mut painter = KeypadPainter::new(None);
        let mut faces: KeypadFaces = std::array::from_fn(|_| KeyFace::Empty);
        faces[0] = agent_face(crate::keyboard::palette::BLOCKED, "claude", false);

        // The claim hand-over isn't instant: mx-keypad notices our claim on
        // its next tick, so the first render or two may find nothing yet.
        for _ in 0..20 {
            painter.render(&mut api, &faces);
            if painter.configured {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(painter.configured, "keypad never opened");
        assert_eq!(painter.last[0], Some(faces[0].clone()), "key 1 not painted");
        painter.blank();
    }
}
