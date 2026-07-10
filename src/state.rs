use crate::status::{Source, Status};
use crate::tmux;
use anyhow::{Context, Result};
use rustix::fs::FlockOperation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Prune entries this long after their agent exited but the pane lives on.
const EXITED_PRUNE_SECS: u64 = 60;

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu",
];

pub const DEFAULT_AGENTS: &str = "claude,copilot,codex,opencode,aider";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub pane_id: String,
    pub name: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<String>,
    pub first_seen: u64,
    pub status_changed_at: u64,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub window_index: u32,
    #[serde(default)]
    pub window_name: String,
    /// Set when the agent process exited but its pane is still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<u64>,
}

impl AgentEntry {
    pub fn new(pane_id: &str, name: &str, status: Status, source: Source) -> Self {
        let now = now();
        AgentEntry {
            pane_id: pane_id.to_string(),
            name: name.to_string(),
            status,
            message: None,
            source,
            last_event: None,
            first_seen: now,
            status_changed_at: now,
            session: String::new(),
            window_index: 0,
            window_name: String::new(),
            exited_at: None,
        }
    }

    pub fn set_status(&mut self, status: Status, message: Option<String>, source: Source) {
        if self.status != status {
            self.status_changed_at = now();
        }
        self.status = status;
        self.message = message;
        self.source = source;
        self.exited_at = None;
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentEntry>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct Store {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Store {
    /// Store for the current tmux server (keyed by socket path).
    pub fn for_current_server() -> Result<Store> {
        Ok(Store::for_socket(&tmux::socket_path()?))
    }

    pub fn for_socket(socket: &str) -> Store {
        let dir = state_dir();
        let key = socket.replace('/', "%");
        Store {
            path: dir.join(format!("{key}.json")),
            lock_path: dir.join(format!("{key}.lock")),
        }
    }

    /// Read a snapshot without locking; rename-based writes keep it consistent.
    pub fn load(&self) -> StateFile {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => StateFile::default(),
        }
    }

    /// Locked read-modify-write with atomic replace.
    pub fn mutate(&self, f: impl FnOnce(&mut StateFile)) -> Result<()> {
        fs::create_dir_all(self.path.parent().unwrap()).context("cannot create state directory")?;
        let lock = File::create(&self.lock_path).context("cannot open lock file")?;
        rustix::fs::flock(&lock, FlockOperation::LockExclusive).context("cannot lock state")?;

        let mut state = self.load();
        state.version = 1;
        f(&mut state);

        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&state)?).context("cannot write state")?;
        fs::rename(&tmp, &self.path).context("cannot replace state file")?;
        Ok(()) // lock released when `lock` drops
    }
}

fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(".local/state")
        });
    base.join("tmux-legion")
}

fn is_shell(command: &str) -> bool {
    let cmd = command.strip_prefix('-').unwrap_or(command);
    SHELLS.contains(&cmd)
}

fn known_agents() -> Vec<String> {
    let list = tmux::get_option("@legion_agents").unwrap_or_else(|| DEFAULT_AGENTS.to_string());
    list.split(',').map(|s| s.trim().to_string()).collect()
}

/// Sync state with the live tmux server: drop entries for dead panes, mark
/// exited agents, discover untracked agent panes, refresh window metadata.
pub fn reconcile(store: &Store) -> Result<()> {
    let panes = tmux::list_panes()?;
    let agents = known_agents();
    let now = now();

    store.mutate(|state| {
        // Remove entries whose pane is gone, or whose agent exited long ago.
        state.agents.retain(|pane_id, entry| {
            let Some(pane) = panes.iter().find(|p| &p.pane_id == pane_id) else {
                return false;
            };
            if is_shell(&pane.current_command) {
                match entry.exited_at {
                    Some(t) => return now.saturating_sub(t) < EXITED_PRUNE_SECS,
                    None => {
                        entry.exited_at = Some(now);
                        entry.status = Status::Done;
                        entry.message = Some("exited".to_string());
                        entry.status_changed_at = now;
                    }
                }
            } else {
                entry.exited_at = None;
            }
            true
        });

        for pane in &panes {
            // Discover agent panes we aren't tracking yet.
            if !state.agents.contains_key(&pane.pane_id) {
                let name = if !pane.pane_agent.is_empty() {
                    Some(pane.pane_agent.clone())
                } else if agents.iter().any(|a| a == &pane.current_command) {
                    Some(pane.current_command.clone())
                } else {
                    None
                };
                if let Some(name) = name {
                    if !is_shell(&pane.current_command) {
                        state.agents.insert(
                            pane.pane_id.clone(),
                            AgentEntry::new(
                                &pane.pane_id,
                                &name,
                                Status::Unknown,
                                Source::Detected,
                            ),
                        );
                    }
                }
            }
            // Refresh cached location fields.
            if let Some(entry) = state.agents.get_mut(&pane.pane_id) {
                entry.session = pane.session.clone();
                entry.window_index = pane.window_index;
                entry.window_name = pane.window_name.clone();
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("tmux-legion-test-{}", std::process::id()));
        std::env::set_var("XDG_STATE_HOME", &dir);
        let store = Store::for_socket("/tmp/tmux-1/test");

        store
            .mutate(|state| {
                let mut e = AgentEntry::new("%1", "claude", Status::Working, Source::Hook);
                e.message = Some("hello".into());
                state.agents.insert("%1".into(), e);
            })
            .unwrap();

        let state = store.load();
        let entry = &state.agents["%1"];
        assert_eq!(entry.name, "claude");
        assert_eq!(entry.status, Status::Working);
        assert_eq!(entry.message.as_deref(), Some("hello"));

        store
            .mutate(|state| {
                state.agents.get_mut("%1").unwrap().set_status(
                    Status::Done,
                    None,
                    Source::Reported,
                );
            })
            .unwrap();
        let state = store.load();
        assert_eq!(state.agents["%1"].status, Status::Done);
        assert_eq!(state.agents["%1"].source, Source::Reported);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_state_file_starts_empty() {
        let dir = std::env::temp_dir().join(format!("tmux-legion-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store {
            path: dir.join("s.json"),
            lock_path: dir.join("s.lock"),
        };
        std::fs::write(&store.path, b"not json").unwrap();
        assert!(store.load().agents.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shell_detection() {
        assert!(is_shell("zsh"));
        assert!(is_shell("-zsh"));
        assert!(is_shell("bash"));
        assert!(!is_shell("claude"));
        assert!(!is_shell("node"));
    }
}
