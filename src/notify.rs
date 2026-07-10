use crate::tmux;
use anyhow::Result;
use rustix::process::{Pid, Signal};

/// Ask a running sidebar to redraw by sending SIGUSR1 to the pid stored in
/// @legion_pid. A closed sidebar (no option, dead pid) is not an error.
pub fn poke() -> Result<()> {
    let Some(pid_str) = tmux::get_option("@legion_pid") else {
        return Ok(());
    };
    let Some(pid) = pid_str.parse::<i32>().ok().and_then(Pid::from_raw) else {
        let _ = tmux::unset_option("@legion_pid");
        return Ok(());
    };
    if rustix::process::kill_process(pid, Signal::USR1).is_err() {
        // Stale pid from a sidebar that died without cleanup.
        let _ = tmux::unset_option("@legion_pid");
        let _ = tmux::unset_option("@legion_pane");
    }
    Ok(())
}
