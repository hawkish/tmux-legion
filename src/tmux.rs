use anyhow::{bail, Context, Result};
use std::process::Command;

const LIST_FORMAT: &str = "#{pane_id}\t#{pane_current_command}\t#{session_name}\t#{window_index}\t#{window_name}\t#{@pane_agent}\t#{pane_pid}\t#{pane_current_path}";

#[derive(Debug, Clone)]
pub struct Pane {
    pub pane_id: String,
    pub current_command: String,
    pub session: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_agent: String,
    pub pane_pid: Option<u32>,
    pub path: String,
}

pub fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to run tmux")?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The pane this process runs in, from $TMUX_PANE.
pub fn current_pane() -> Option<String> {
    std::env::var("TMUX_PANE").ok().filter(|p| !p.is_empty())
}

/// The tmux server socket path: first field of $TMUX, else asked from the server.
pub fn socket_path() -> Result<String> {
    if let Ok(tmux) = std::env::var("TMUX") {
        if let Some(path) = tmux.split(',').next() {
            if !path.is_empty() {
                return Ok(path.to_string());
            }
        }
    }
    run(&["display-message", "-p", "#{socket_path}"])
}

pub fn get_option(name: &str) -> Option<String> {
    run(&["show-option", "-gqv", name])
        .ok()
        .filter(|v| !v.is_empty())
}

pub fn set_option(name: &str, value: &str) -> Result<()> {
    run(&["set-option", "-g", name, value]).map(|_| ())
}

pub fn unset_option(name: &str) -> Result<()> {
    run(&["set-option", "-gu", name]).map(|_| ())
}

pub fn set_pane_option(pane: &str, name: &str, value: &str) -> Result<()> {
    run(&["set-option", "-p", "-t", pane, name, value]).map(|_| ())
}

pub fn unset_pane_option(pane: &str, name: &str) -> Result<()> {
    run(&["set-option", "-pu", "-t", pane, name]).map(|_| ())
}

pub fn list_panes() -> Result<Vec<Pane>> {
    let out = run(&["list-panes", "-a", "-F", LIST_FORMAT])?;
    Ok(out.lines().filter_map(parse_pane_line).collect())
}

fn parse_pane_line(line: &str) -> Option<Pane> {
    let mut f = line.split('\t');
    Some(Pane {
        pane_id: f.next()?.to_string(),
        current_command: f.next()?.to_string(),
        session: f.next()?.to_string(),
        window_index: f.next()?.parse().ok()?,
        window_name: f.next()?.to_string(),
        pane_agent: f.next().unwrap_or("").to_string(),
        pane_pid: f.next().and_then(|s| s.parse().ok()),
        path: f.next().unwrap_or("").to_string(),
    })
}

/// The pane the user is focused on: active pane of the active window of an
/// attached session. Not `display-message -p '#{pane_id}'` — that resolves
/// via $TMUX_PANE to the calling process's own pane (i.e. the sidebar).
pub fn focused_pane() -> Option<String> {
    run(&[
        "list-panes",
        "-a",
        "-f",
        "#{&&:#{session_attached},#{&&:#{window_active},#{pane_active}}}",
        "-F",
        "#{pane_id}",
    ])
    .ok()?
    .lines()
    .next()
    .map(str::to_string)
}

pub fn select_pane(pane_id: &str) -> Result<()> {
    // Focus the window containing the pane first, then the pane itself.
    run(&["select-window", "-t", pane_id])?;
    run(&["select-pane", "-t", pane_id])?;
    Ok(())
}

pub fn kill_pane(pane_id: &str) -> Result<()> {
    run(&["kill-pane", "-t", pane_id]).map(|_| ())
}

pub fn pane_exists(pane_id: &str) -> bool {
    list_panes()
        .map(|panes| panes.iter().any(|p| p.pane_id == pane_id))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pane_line() {
        let p = parse_pane_line("%5\tclaude\tmain\t2\tapi\tclaude\t12345\t/home/u/proj").unwrap();
        assert_eq!(p.pane_id, "%5");
        assert_eq!(p.current_command, "claude");
        assert_eq!(p.session, "main");
        assert_eq!(p.window_index, 2);
        assert_eq!(p.window_name, "api");
        assert_eq!(p.pane_agent, "claude");
        assert_eq!(p.pane_pid, Some(12345));
        assert_eq!(p.path, "/home/u/proj");
    }

    #[test]
    fn parses_pane_line_without_agent() {
        let p = parse_pane_line("%0\tzsh\tmain\t1\tshell\t\t99").unwrap();
        assert_eq!(p.pane_agent, "");
        assert_eq!(p.pane_pid, Some(99));
    }

    #[test]
    fn parses_pane_line_without_pid() {
        let p = parse_pane_line("%0\tzsh\tmain\t1\tshell\t").unwrap();
        assert_eq!(p.pane_pid, None);
    }
}
