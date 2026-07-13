use crate::process::ProcessSnapshot;
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
/// Long enough to notice the exit, short enough not to clutter the sidebar
/// (and to survive an agent's shell tool briefly holding the tty foreground —
/// the Alive verdict resets the timer as soon as the agent is back).
const EXITED_PRUNE_SECS: u64 = 15;

const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "nu",
];

const DEFAULT_AGENTS: &str = "claude,copilot,codex,opencode,aider";

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

    /// Locked read-modify-write with atomic replace. Skips the write when the
    /// mutation was a no-op (reconcile runs every ~2s; don't churn the disk).
    pub fn mutate(&self, f: impl FnOnce(&mut StateFile)) -> Result<()> {
        fs::create_dir_all(self.path.parent().unwrap()).context("cannot create state directory")?;
        let lock = File::create(&self.lock_path).context("cannot open lock file")?;
        rustix::fs::flock(&lock, FlockOperation::LockExclusive).context("cannot lock state")?;

        let before = fs::read(&self.path).unwrap_or_default();
        let mut state: StateFile = serde_json::from_slice(&before).unwrap_or_default();
        state.version = 1;
        f(&mut state);

        let after = serde_json::to_vec_pretty(&state)?;
        if after != before {
            let tmp = self.path.with_extension("json.tmp");
            fs::write(&tmp, after).context("cannot write state")?;
            fs::rename(&tmp, &self.path).context("cannot replace state file")?;
        }
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

#[derive(Debug, PartialEq, Eq)]
enum PaneVerdict {
    /// The agent (or its node wrapper, per the @pane_agent tag) is foreground.
    Alive,
    /// A shell is foreground: the agent process ended, the pane lives on.
    AgentExited,
    /// The pane id was recycled for an unrelated program; the entry is stale.
    Replaced,
}

/// Name the agent in an untracked pane: the @pane_agent tag, a direct
/// command-name match, or — for interpreter wrappers like "node" — a
/// process-tree search for any known agent.
fn discover_name(
    pane: &tmux::Pane,
    agents: &[String],
    snapshot: Option<&ProcessSnapshot>,
) -> Option<String> {
    if !pane.pane_agent.is_empty() {
        return Some(pane.pane_agent.clone());
    }
    if agents.iter().any(|a| a == &pane.current_command) {
        return Some(pane.current_command.clone());
    }
    if crate::process::is_interpreter(&pane.current_command) {
        if let (Some(snap), Some(pid)) = (snapshot, pane.pane_pid) {
            return snap.find_agent_in_tree(pid, agents);
        }
    }
    None
}

fn judge_pane(
    entry_name: &str,
    pane: &tmux::Pane,
    snapshot: Option<&ProcessSnapshot>,
) -> PaneVerdict {
    // Direct command match: the agent binary is the foreground process.
    if pane.current_command == entry_name {
        return PaneVerdict::Alive;
    }

    // Tag doesn't match either: pane was recycled for an unrelated program.
    if pane.pane_agent != entry_name {
        return PaneVerdict::Replaced;
    }

    // pane_agent matches but current_command differs (wrapper / shell / something else).
    // Use the process tree to determine truth when we have a snapshot + pid.
    if let (Some(snap), Some(pid)) = (snapshot, pane.pane_pid) {
        return if snap.tree_has_agent(pid, entry_name) {
            PaneVerdict::Alive
        } else if is_shell(&pane.current_command) {
            PaneVerdict::AgentExited
        } else {
            PaneVerdict::Replaced
        };
    }

    // Fallback without snapshot: shell → exited, anything else → alive (wrapper case).
    if is_shell(&pane.current_command) {
        PaneVerdict::AgentExited
    } else {
        PaneVerdict::Alive
    }
}

/// Sync state with the live tmux server: drop entries for dead panes, mark
/// exited agents, discover untracked agent panes, refresh window metadata.
pub fn reconcile(store: &Store) -> Result<()> {
    let panes = tmux::list_panes()?;
    let agents = known_agents();
    let now = now();

    // Only pay the ps cost when a pane needs process-tree verification
    // (tagged) or could hide a wrapped agent (interpreter foreground).
    let snapshot = panes
        .iter()
        .any(|p| !p.pane_agent.is_empty() || crate::process::is_interpreter(&p.current_command))
        .then(ProcessSnapshot::scan)
        .flatten();

    let mut panes_to_clear: Vec<String> = Vec::new();
    let mut panes_to_tag: Vec<(String, String)> = Vec::new();

    store.mutate(|state| {
        // Remove entries whose pane is gone or recycled, or whose agent
        // exited a while ago.
        state.agents.retain(|pane_id, entry| {
            let Some(pane) = panes.iter().find(|p| &p.pane_id == pane_id) else {
                return false;
            };
            match judge_pane(&entry.name, pane, snapshot.as_ref()) {
                PaneVerdict::Alive => {
                    entry.exited_at = None;
                    true
                }
                // A shell (or verified-gone agent) is foreground. Start a grace
                // timer; clear the tag and remove the row once expired.
                PaneVerdict::AgentExited => match entry.exited_at {
                    Some(t) if now.saturating_sub(t) >= EXITED_PRUNE_SECS => {
                        panes_to_clear.push(pane_id.clone());
                        false
                    }
                    Some(_) => true,
                    None => {
                        entry.exited_at = Some(now);
                        true
                    }
                },
                PaneVerdict::Replaced => {
                    panes_to_clear.push(pane_id.clone());
                    false
                }
            }
        });

        for pane in &panes {
            // Discover agent panes we aren't tracking yet.
            if !state.agents.contains_key(&pane.pane_id) {
                if let Some(name) = discover_name(pane, &agents, snapshot.as_ref()) {
                    if !is_shell(&pane.current_command) {
                        // When the tag is set but the command doesn't match, verify
                        // via the process tree before trusting the stale tag.
                        let verified = pane.current_command == name
                            || match (snapshot.as_ref(), pane.pane_pid) {
                                (Some(snap), Some(pid)) => snap.tree_has_agent(pid, &name),
                                _ => true, // no snapshot: give benefit of the doubt
                            };
                        if verified {
                            // Tree-discovered wrapper panes carry no tag yet; set
                            // one so the next reconcile's liveness check takes
                            // the tag + process-tree path instead of Replaced.
                            if pane.pane_agent.is_empty() && pane.current_command != name {
                                panes_to_tag.push((pane.pane_id.clone(), name.clone()));
                            }
                            state.agents.insert(
                                pane.pane_id.clone(),
                                AgentEntry::new(
                                    &pane.pane_id,
                                    &name,
                                    Status::Unknown,
                                    Source::Detected,
                                ),
                            );
                        } else {
                            panes_to_clear.push(pane.pane_id.clone());
                        }
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
    })?;

    // Tmux calls happen outside the state lock so they don't block the write
    // path. Once cleared, reconcile won't re-discover the pane.
    for pane_id in panes_to_clear {
        let _ = tmux::unset_pane_option(&pane_id, "@pane_agent");
    }
    for (pane_id, name) in panes_to_tag {
        let _ = tmux::set_pane_option(&pane_id, "@pane_agent", &name);
    }

    Ok(())
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

    fn pane(command: &str, tag: &str) -> tmux::Pane {
        tmux::Pane {
            pane_id: "%9".into(),
            current_command: command.into(),
            session: "s".into(),
            window_index: 1,
            window_name: "w".into(),
            pane_agent: tag.into(),
            pane_pid: None,
        }
    }

    fn pane_with_pid(command: &str, tag: &str, pid: u32) -> tmux::Pane {
        tmux::Pane {
            pane_pid: Some(pid),
            ..pane(command, tag)
        }
    }

    #[test]
    fn pane_verdicts() {
        // Agent foreground, matched by command name or tag (node wrappers, no snapshot).
        assert_eq!(
            judge_pane("claude", &pane("claude", "claude"), None),
            PaneVerdict::Alive
        );
        assert_eq!(
            judge_pane("pi", &pane("node", "pi"), None),
            PaneVerdict::Alive
        );
        // Shell foreground: agent exited, keep briefly.
        assert_eq!(
            judge_pane("claude", &pane("zsh", "claude"), None),
            PaneVerdict::AgentExited
        );
        // Pane id recycled by an unrelated program (e.g. the sidebar itself).
        assert_eq!(
            judge_pane("claude", &pane("tmux-legion", ""), None),
            PaneVerdict::Replaced
        );
        assert_eq!(
            judge_pane("pi", &pane("vim", ""), None),
            PaneVerdict::Replaced
        );
    }

    #[test]
    fn process_tree_detects_alive_wrapper() {
        let snap = ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 node node /usr/local/bin/pi\n",
        );
        assert_eq!(
            judge_pane("pi", &pane_with_pid("node", "pi", 100), Some(&snap)),
            PaneVerdict::Alive
        );
    }

    #[test]
    fn process_tree_detects_replaced_by_top() {
        // top is running, claude is nowhere in the process tree
        let snap = ProcessSnapshot::from_ps_output("100 1 top top\n");
        assert_eq!(
            judge_pane("claude", &pane_with_pid("top", "claude", 100), Some(&snap)),
            PaneVerdict::Replaced
        );
    }

    #[test]
    fn process_tree_detects_agent_exited_via_shell() {
        // shell is foreground and claude is not in the tree
        let snap = ProcessSnapshot::from_ps_output("100 1 zsh zsh\n");
        assert_eq!(
            judge_pane("claude", &pane_with_pid("zsh", "claude", 100), Some(&snap)),
            PaneVerdict::AgentExited
        );
    }

    #[test]
    fn discover_name_prefers_tag_then_command_then_tree() {
        let agents: Vec<String> = vec!["claude".into(), "opencode".into()];
        let snap = ProcessSnapshot::from_ps_output(
            "100 1 node node /usr/local/bin/opencode\n101 100 node node worker.js\n",
        );

        // Tag wins outright.
        assert_eq!(
            discover_name(&pane("node", "pi"), &agents, Some(&snap)),
            Some("pi".into())
        );
        // Direct command-name match, no snapshot needed.
        assert_eq!(
            discover_name(&pane("claude", ""), &agents, None),
            Some("claude".into())
        );
        // Interpreter wrapper: found via the process tree.
        assert_eq!(
            discover_name(&pane_with_pid("node", "", 100), &agents, Some(&snap)),
            Some("opencode".into())
        );
        // Interpreter with no agent underneath: not an agent pane.
        assert_eq!(
            discover_name(&pane_with_pid("node", "", 101), &agents, Some(&snap)),
            None
        );
        // Interpreter but no snapshot/pid: don't guess.
        assert_eq!(discover_name(&pane("node", ""), &agents, Some(&snap)), None);
        // Non-interpreter, non-agent command stays invisible.
        assert_eq!(
            discover_name(&pane_with_pid("vim", "", 100), &agents, Some(&snap)),
            None
        );
    }

    #[test]
    fn process_tree_shell_with_agent_subprocess_is_alive() {
        // Agent spawned a shell tool; claude is still in the tree
        let snap =
            ProcessSnapshot::from_ps_output("100 1 claude claude\n101 100 bash bash -c ls\n");
        assert_eq!(
            judge_pane("claude", &pane_with_pid("bash", "claude", 100), Some(&snap)),
            PaneVerdict::Alive
        );
    }
}
