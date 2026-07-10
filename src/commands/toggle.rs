use crate::tmux;
use anyhow::{Context, Result};

pub fn toggle() -> Result<()> {
    match live_sidebar_pane() {
        Some(pane) => {
            tmux::kill_pane(&pane)?;
            Ok(())
        }
        None => open(),
    }
}

pub fn open() -> Result<()> {
    if live_sidebar_pane().is_some() {
        return Ok(());
    }
    let width = tmux::get_option("@legion_width").unwrap_or_else(|| "15%".to_string());
    let position = tmux::get_option("@legion_position").unwrap_or_else(|| "left".to_string());
    let bin = std::env::current_exe().context("cannot resolve own binary path")?;
    let bin = bin.to_string_lossy();

    // Full-height (-f) horizontal split; -b places it before (left of) the
    // target. -P -F prints the new pane id so we can track it.
    let mut args = vec!["split-window", "-h", "-f"];
    if position == "left" {
        args.push("-b");
    }
    let cmd = format!("'{bin}' sidebar");
    args.extend(["-l", &width, "-P", "-F", "#{pane_id}", &cmd]);
    let pane_id = tmux::run(&args)?;
    tmux::set_option("@legion_pane", &pane_id)?;
    Ok(())
}

pub fn close() -> Result<()> {
    if let Some(pane) = live_sidebar_pane() {
        tmux::kill_pane(&pane)?;
    }
    Ok(())
}

/// The sidebar pane id, only if that pane still exists.
fn live_sidebar_pane() -> Option<String> {
    let pane = tmux::get_option("@legion_pane")?;
    if tmux::pane_exists(&pane) {
        Some(pane)
    } else {
        let _ = tmux::unset_option("@legion_pane");
        let _ = tmux::unset_option("@legion_pid");
        None
    }
}
