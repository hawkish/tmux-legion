use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

struct ProcessInfo {
    comm: String,
    args: String,
}

pub struct ProcessSnapshot {
    children_of: HashMap<u32, Vec<u32>>,
    info_by_pid: HashMap<u32, ProcessInfo>,
}

impl ProcessSnapshot {
    pub fn scan() -> Option<Self> {
        let output = Command::new("ps")
            .args(["-eo", "pid=,ppid=,comm=,args="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(Self::from_ps_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    pub fn from_ps_output(ps_output: &str) -> Self {
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut info_by_pid: HashMap<u32, ProcessInfo> = HashMap::new();

        for line in ps_output.lines() {
            let mut parts = line.split_whitespace();
            let (Some(pid_str), Some(ppid_str), Some(comm)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(pid), Ok(ppid)) = (pid_str.parse::<u32>(), ppid_str.parse::<u32>()) else {
                continue;
            };
            children_of.entry(ppid).or_default().push(pid);
            info_by_pid.insert(
                pid,
                ProcessInfo {
                    comm: comm.to_string(),
                    args: parts.collect::<Vec<_>>().join(" "),
                },
            );
        }

        Self {
            children_of,
            info_by_pid,
        }
    }

    fn descendants(&self, seed_pid: u32) -> HashSet<u32> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(seed_pid);
        while let Some(pid) = queue.pop_front() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(children) = self.children_of.get(&pid) {
                queue.extend(children.iter().copied());
            }
        }
        seen
    }

    pub fn tree_has_agent(&self, seed_pid: u32, agent_name: &str) -> bool {
        self.descendants(seed_pid)
            .iter()
            .any(|&pid| match self.info_by_pid.get(&pid) {
                Some(info) => matches_agent(info, agent_name),
                None => false,
            })
    }

    /// Search the pane's process tree for any of the known agents. Used to
    /// discover interpreter-wrapped agents whose pane command is just "node".
    pub fn find_agent_in_tree(&self, seed_pid: u32, agent_names: &[String]) -> Option<String> {
        self.descendants(seed_pid).iter().find_map(|&pid| {
            let info = self.info_by_pid.get(&pid)?;
            agent_names
                .iter()
                .find(|name| matches_agent(info, name))
                .cloned()
        })
    }
}

/// Runtimes that hide the real agent name behind their own process name.
const INTERPRETERS: &[&str] = &["node", "bun", "deno"];

pub fn is_interpreter(command: &str) -> bool {
    INTERPRETERS.contains(&cmd_basename(command))
}

fn cmd_basename(s: &str) -> &str {
    Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(s)
}

fn matches_agent(info: &ProcessInfo, agent_name: &str) -> bool {
    if cmd_basename(&info.comm) == agent_name {
        return true;
    }
    let mut tokens = info.args.split_whitespace();
    let Some(first) = tokens.next().map(|a| cmd_basename(a.trim_matches('"'))) else {
        return false;
    };
    if first == agent_name {
        return true;
    }
    // For interpreter wrappers (e.g. "node /path/to/agent"), check the second
    // token too — but only behind a known interpreter, so "less ./claude"
    // doesn't count as claude.
    is_interpreter(first)
        && tokens.next().map(|a| cmd_basename(a.trim_matches('"'))) == Some(agent_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_walks_tree() {
        let snap = ProcessSnapshot {
            children_of: HashMap::from([(1, vec![2, 3]), (2, vec![4])]),
            info_by_pid: HashMap::new(),
        };
        let seen = snap.descendants(1);
        assert!(seen.contains(&1));
        assert!(seen.contains(&2));
        assert!(seen.contains(&3));
        assert!(seen.contains(&4));
    }

    #[test]
    fn tree_has_agent_finds_descendant() {
        let snap = ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 node node /usr/bin/opencode\n",
        );
        assert!(snap.tree_has_agent(100, "opencode"));
        assert!(!snap.tree_has_agent(100, "claude"));
    }

    #[test]
    fn matches_agent_comm_and_args() {
        assert!(matches_agent(
            &ProcessInfo {
                comm: "claude".into(),
                args: "/opt/homebrew/bin/claude".into()
            },
            "claude"
        ));
        assert!(matches_agent(
            &ProcessInfo {
                comm: "node".into(),
                args: "/usr/local/bin/opencode".into()
            },
            "opencode"
        ));
        assert!(!matches_agent(
            &ProcessInfo {
                comm: "top".into(),
                args: "top".into()
            },
            "claude"
        ));
        // Second token only counts behind a known interpreter.
        assert!(matches_agent(
            &ProcessInfo {
                comm: "node".into(),
                args: "node /usr/local/bin/opencode".into()
            },
            "opencode"
        ));
        assert!(!matches_agent(
            &ProcessInfo {
                comm: "less".into(),
                args: "less ./claude".into()
            },
            "claude"
        ));
        assert!(!matches_agent(
            &ProcessInfo {
                comm: "vim".into(),
                args: "vim claude".into()
            },
            "claude"
        ));
    }

    #[test]
    fn interpreter_detection() {
        assert!(is_interpreter("node"));
        assert!(is_interpreter("/usr/local/bin/bun"));
        assert!(!is_interpreter("zsh"));
        assert!(!is_interpreter("claude"));
    }

    #[test]
    fn find_agent_in_tree_names_wrapped_agent() {
        let snap = ProcessSnapshot::from_ps_output(
            "100 1 fish fish\n101 100 node node /usr/local/bin/opencode\n",
        );
        let agents: Vec<String> = vec!["claude".into(), "opencode".into()];
        assert_eq!(
            snap.find_agent_in_tree(100, &agents),
            Some("opencode".into())
        );
        assert_eq!(snap.find_agent_in_tree(100, &["claude".into()]), None);
    }
}
