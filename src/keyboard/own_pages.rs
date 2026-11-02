//! tmux-legion's own keypad pages: shortcuts, media keys, Danish letters,
//! shift — whatever `$XDG_CONFIG_HOME/tmux-legion/keypad.json`, or
//! `~/.config/tmux-legion/keypad.json`, defines, in mx-keypad's own config
//! format. They belong to tmux-legion, so they live in its config, but
//! tmux-legion doesn't run them: it sends them to mx-keypad's agent as
//! remote pages, and the agent performs every action itself. That's what
//! keeps keystrokes working without granting tmux-legion Accessibility.
//!
//! The file is read again whenever it changes, so editing it doesn't need
//! the sidebar reopened. A file that doesn't parse leaves the last good
//! pages up. When there's no file at all, the starter pages built into the
//! binary are written there first, so a fresh install has something to edit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use mx_keypad::config::{Config, Key};

/// How often to look at the file's modification time.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Where the config lives: `$XDG_CONFIG_HOME/tmux-legion/keypad.json`, else
/// `~/.config/tmux-legion/keypad.json`. Mirrors `mx_keypad::config::default_path`,
/// under tmux-legion's own name so the two never collide.
fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".config")
        });
    base.join("tmux-legion/keypad.json")
}

/// The starter pages, `keypad/keypad.json`, built in so every install has
/// them whichever way it was built.
const STARTER: &str = include_str!("../../keypad/keypad.json");

/// Write the starter pages to `path` unless something is already there.
/// `Ok(true)` when it wrote them.
fn seed(path: &Path) -> Result<bool> {
    if path.symlink_metadata().is_ok() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    std::fs::write(path, STARTER).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(true)
}

/// One page, ready to send: its name and nine keys.
pub type OwnPage = (String, [Key; 9]);

/// The pages in tmux-legion's keypad config, kept in step with the file.
pub struct OwnPages {
    path: PathBuf,
    loaded: Option<SystemTime>,
    next_check: Instant,
    pages: Vec<OwnPage>,
}

impl OwnPages {
    pub fn new() -> OwnPages {
        let path = config_path();
        match seed(&path) {
            Ok(true) => super::debug_log(&format!(
                "keypad: wrote the starter pages to {}",
                path.display()
            )),
            Ok(false) => {}
            Err(error) => super::debug_log(&format!("keypad: {error:#}")),
        }
        OwnPages::at(path)
    }

    fn at(path: PathBuf) -> OwnPages {
        OwnPages {
            path,
            loaded: None,
            next_check: Instant::now(),
            pages: Vec::new(),
        }
    }

    /// The pages as last read.
    pub fn pages(&self) -> &[OwnPage] {
        &self.pages
    }

    /// Read the file again if it changed since the last call, at most once
    /// a second. `Ok(true)` when the pages changed; a missing file means no
    /// pages. On an error the previous pages stay.
    pub fn refresh(&mut self) -> Result<bool> {
        if Instant::now() < self.next_check {
            return Ok(false);
        }
        self.next_check = Instant::now() + CHECK_EVERY;
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if modified == self.loaded {
            return Ok(false);
        }
        self.loaded = modified;
        let pages = if modified.is_some() {
            load(&self.path)?
        } else {
            Vec::new()
        };
        let changed = pages != self.pages;
        self.pages = pages;
        Ok(changed)
    }
}

/// Read `path` into pages to send. The file's `defaults` are written into
/// each key, since a remote page gets mx-keypad's built-in defaults, and
/// relative icon paths are made absolute against the file's folder, since
/// the agent resolves them in its own working directory.
fn load(path: &Path) -> Result<Vec<OwnPage>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let config: Config = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid keypad config", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let defaults = &config.defaults;
    let mut pages = Vec::new();
    for page in config.pages {
        let mut keys: [Key; 9] = std::array::from_fn(|_| Key::default());
        for (index, mut key) in page.keys {
            let slot = index
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .with_context(|| format!("{}: keys are \"1\" to \"9\"", page.name))?;
            key.background.get_or_insert(defaults.background);
            key.foreground.get_or_insert(defaults.foreground);
            key.glyph_scale.get_or_insert(defaults.glyph_scale);
            absolute_icon(&mut key.icon, base);
            if let Some(shifted) = key.shifted.as_mut() {
                absolute_icon(&mut shifted.icon, base);
            }
            keys[slot - 1] = key;
        }
        // An empty key still gets the file's colours.
        for key in &mut keys {
            key.background.get_or_insert(defaults.background);
            key.foreground.get_or_insert(defaults.foreground);
        }
        pages.push((page.name, keys));
    }
    Ok(pages)
}

/// `icon` made absolute against `base` when it's a relative file path.
/// Built-in names and `~/` paths are left for the agent.
fn absolute_icon(icon: &mut Option<String>, base: &Path) {
    let Some(name) = icon.as_deref() else {
        return;
    };
    let looks_like_path = name.contains('/') || Path::new(name).extension().is_some();
    if looks_like_path && !Path::new(name).is_absolute() && !name.starts_with("~/") {
        *icon = Some(base.join(name).display().to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mx_keypad::config::Action;
    use mx_keypad::render::Rgb;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tmux-legion-own-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_path_is_under_tmux_legion_not_mx_keypad() {
        assert!(config_path().ends_with("tmux-legion/keypad.json"));
    }

    #[test]
    fn the_starter_pages_load() {
        let dir = temp_dir("starter");
        let path = dir.join("tmux-legion/keypad.json");
        assert!(seed(&path).unwrap());
        let mut own = OwnPages::at(path);
        assert!(own.refresh().unwrap());
        assert!(!own.pages().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seeding_leaves_an_existing_file_alone() {
        let dir = temp_dir("seeded");
        let path = dir.join("keypad.json");
        std::fs::write(&path, r#"{ "pages": [] }"#).unwrap();
        assert!(!seed(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{ "pages": [] }"#
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_means_no_pages() {
        let mut own = OwnPages::at(temp_dir("missing").join("keypad.json"));
        assert!(!own.refresh().unwrap());
        assert!(own.pages().is_empty());
    }

    #[test]
    fn pages_carry_the_file_defaults_and_absolute_icons() {
        let dir = temp_dir("load");
        let path = dir.join("keypad.json");
        std::fs::write(
            &path,
            r##"{
              "defaults": { "background": "#112233" },
              "pages": [{ "name": "page-1", "keys": {
                "1": { "label": "Esc", "action": { "keys": "escape" } },
                "2": { "icon": "icons/x.svg" },
                "3": { "icon": "bell", "background": "#ff0000" }
              } }]
            }"##,
        )
        .unwrap();
        let mut own = OwnPages::at(path);
        assert!(own.refresh().unwrap());
        let (name, keys) = &own.pages()[0];
        assert_eq!(name, "page-1");
        assert_eq!(keys[0].action, Action::Keys("escape".into()));
        assert_eq!(keys[0].background, Some(Rgb(0x11, 0x22, 0x33)));
        assert_eq!(keys[0].foreground, Some(Rgb(0xff, 0xff, 0xff)));
        assert_eq!(
            keys[1].icon.as_deref(),
            Some(dir.join("icons/x.svg").display().to_string().as_str())
        );
        assert_eq!(keys[2].icon.as_deref(), Some("bell"));
        assert_eq!(keys[2].background, Some(Rgb(0xff, 0, 0)));
        assert_eq!(keys[8].background, Some(Rgb(0x11, 0x22, 0x33)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_edit_keeps_the_last_good_pages() {
        let dir = temp_dir("broken");
        let path = dir.join("keypad.json");
        std::fs::write(&path, r#"{ "pages": [{ "name": "a" }] }"#).unwrap();
        let mut own = OwnPages::at(path.clone());
        assert!(own.refresh().unwrap());
        std::fs::write(&path, "{ not json").unwrap();
        own.loaded = None; // the edit may land within the same mtime tick
        own.next_check = Instant::now();
        assert!(own.refresh().is_err());
        assert_eq!(own.pages()[0].0, "a");
        std::fs::remove_dir_all(&dir).ok();
    }
}
