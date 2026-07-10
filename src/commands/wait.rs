use crate::state::{self, Store};
use crate::status::Status;
use crate::tmux;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Reconcile against live panes at most this often while polling.
const RECONCILE_EVERY: u32 = 10;

/// Exit codes: 0 = status reached, 2 = timeout, 3 = pane vanished, 1 = error.
pub fn wait(pane: Option<String>, status: Status, timeout: Option<u64>) -> ExitCode {
    let Some(pane_id) = pane.or_else(tmux::current_pane) else {
        eprintln!("tmux-legion: no pane to watch (pass --pane or run inside tmux)");
        return ExitCode::FAILURE;
    };
    let store = match Store::for_current_server() {
        Ok(s) => s,
        Err(err) => {
            eprintln!("tmux-legion: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs(t));
    let mut ticks: u32 = 0;
    loop {
        if ticks.is_multiple_of(RECONCILE_EVERY) {
            let _ = state::reconcile(&store);
        }
        ticks = ticks.wrapping_add(1);

        match store.load().agents.get(&pane_id) {
            Some(entry) if entry.status == status => return ExitCode::SUCCESS,
            Some(_) => {}
            None => {
                if !tmux::pane_exists(&pane_id) {
                    eprintln!("tmux-legion: pane {pane_id} is gone");
                    return ExitCode::from(3);
                }
            }
        }

        if deadline.is_some_and(|d| Instant::now() >= d) {
            eprintln!(
                "tmux-legion: timed out waiting for {pane_id} to be {}",
                status.as_str()
            );
            return ExitCode::from(2);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}
