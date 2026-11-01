//! The MX Creative Keypad backend. tmux-legion never opens the keypad:
//! `mx-keypad run`, its own LaunchAgent, owns the device, and this module adds
//! pages to it over the agent's socket (`mx_keypad::remote`): tmux-legion's
//! own pages from its keypad config (see `own_pages`), then an "agents" page.
//! The agent draws them after the user's mx-keypad pages and flips through
//! all of them with the page buttons. It runs the own pages' actions itself,
//! with the Accessibility grant keystrokes need, and sends back presses on the
//! agents page, whose keys have no action. Nothing here ever posts input.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mx_keypad::config::Key;
use mx_keypad::remote::Client;
use mx_keypad::render::{self, IconSource};
use mx_keypad::{Button, Event};

use super::color::Rgb;
use super::own_pages::OwnPages;

/// Keys on the MX Creative Keypad's panel.
pub const KEY_COUNT: usize = 9;

/// The page's name in the agent's page list.
const PAGE: &str = "agents";

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
    /// The key sent to the agent for this slot, resolving its icon through
    /// `icons`. It has no action, so the agent sends its presses back.
    fn face(&self, icons: &mut IconCache) -> Key {
        match self {
            KeyFace::Empty => Key {
                background: Some(render::Rgb(0, 0, 0)),
                ..Key::default()
            },
            KeyFace::Agent {
                background,
                icon,
                dimmed,
            } => {
                let background = if *dimmed { Rgb(0, 0, 0) } else { *background };
                Key {
                    icon: Some(icon_name(&icons.resolve(icon))),
                    background: Some(to_mx_rgb(background)),
                    foreground: Some(readable_on(background)),
                    ..Key::default()
                }
            }
        }
    }
}

/// How the wire names an icon: a built-in's name, or a file's absolute path.
fn icon_name(icon: &IconSource) -> String {
    match icon {
        IconSource::Builtin(name) => name.clone(),
        IconSource::File(path) => path.display().to_string(),
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

/// `<dir>/<name>.{png,svg,jpg,jpeg}`, the first one that exists. The agent
/// resolves the path itself, so it is made absolute.
fn find_icon_file(dir: &Path, name: &str) -> Option<PathBuf> {
    ["png", "svg", "jpg", "jpeg"]
        .iter()
        .map(|ext| dir.join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
        .map(|path| std::path::absolute(&path).unwrap_or(path))
}

/// Keeps tmux-legion's pages on `mx-keypad run`'s keypad up to date.
pub struct KeypadPainter {
    client: Client,
    icons: IconCache,
    own: OwnPages,
    /// Names of the own pages last sent, to remove one the file drops.
    sent_own: Vec<String>,
    /// Set by `blank` for a sleep or quit: the page is removed and stays
    /// away until `wake`.
    asleep: bool,
}

impl KeypadPainter {
    pub fn new(icons_dir: Option<PathBuf>) -> KeypadPainter {
        KeypadPainter {
            client: Client::new("tmux-legion"),
            icons: IconCache::new(icons_dir),
            own: OwnPages::new(),
            sent_own: Vec::new(),
            asleep: false,
        }
    }

    /// Send the agents page. Safe to call every frame: the client sends only
    /// the keys that changed, and nothing while the agent isn't running.
    pub fn render(&mut self, want: &KeypadFaces) {
        if self.asleep {
            return;
        }
        if let Err(error) = self.own.refresh() {
            super::debug_log(&format!(
                "keypad config: {error:#}; keeping the previous pages"
            ));
        }
        self.send_own();
        let faces = std::array::from_fn(|i| want[i].face(&mut self.icons));
        self.client.set_page(PAGE, faces);
    }

    /// Send the own pages, before the agents page the first time so they
    /// come first, and take away any the file no longer has.
    fn send_own(&mut self) {
        let names: Vec<String> = self.own.pages().iter().map(|(n, _)| n.clone()).collect();
        for gone in self.sent_own.iter().filter(|n| !names.contains(n)) {
            self.client.remove(gone);
        }
        for (name, keys) in self.own.pages() {
            self.client.set_page(name, keys.clone());
        }
        self.sent_own = names;
    }

    /// Keys pressed on the agents page since the last call, 1–9, oldest
    /// first. Never blocks; also reconnects to a restarted agent.
    pub fn poll_presses(&mut self) -> Vec<u8> {
        let presses = self
            .client
            .poll()
            .into_iter()
            .filter_map(|(page, event)| match event {
                Event::Down(Button::Key(n)) if page == PAGE => Some(n),
                _ => None,
            })
            .collect();
        for error in self.client.take_errors() {
            super::debug_log(&format!("keypad: {error}"));
        }
        presses
    }

    /// Take the pages off the keypad, for a sleep or a quit. The agent turns
    /// back to the user's mx-keypad pages.
    pub fn blank(&mut self) {
        self.asleep = true;
        for name in std::mem::take(&mut self.sent_own) {
            self.client.remove(&name);
        }
        self.client.remove(PAGE);
    }

    /// Put the pages back on the next frame.
    pub fn wake(&mut self) {
        self.asleep = false;
    }
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
    use super::super::palette;
    use super::*;

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
    fn a_dimmed_agent_face_sends_a_black_background() {
        let mut icons = IconCache::new(None);
        let face = agent_face(palette::BLOCKED, "claude", true);
        assert_eq!(face.face(&mut icons).background, Some(render::Rgb(0, 0, 0)));
        let face = agent_face(palette::BLOCKED, "claude", false);
        assert_eq!(
            face.face(&mut icons).background,
            Some(to_mx_rgb(palette::BLOCKED))
        );
    }

    #[test]
    fn an_agent_face_names_its_icon_for_the_wire() {
        let mut icons = IconCache::new(None);
        let face = agent_face(palette::BLOCKED, "claude", false);
        assert_eq!(face.face(&mut icons).icon.as_deref(), Some(FALLBACK_ICON));
    }

    #[test]
    fn an_empty_key_has_no_icon() {
        let mut icons = IconCache::new(None);
        assert!(empty_face().face(&mut icons).icon.is_none());
    }
}
