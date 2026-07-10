use crate::cli::Direction;
use crate::notify;
use crate::state::{AgentEntry, Store};
use crate::status::{Source, Status};
use crate::tmux;
use anyhow::{bail, Result};

pub fn spawn(
    name: Option<String>,
    direction: Direction,
    window: bool,
    cwd: Option<String>,
    focus: bool,
    command: Vec<String>,
) -> Result<()> {
    let Some(caller) = tmux::current_pane() else {
        bail!("spawn only works inside tmux");
    };
    if tmux::get_option("@legion_pane").as_deref() == Some(caller.as_str()) {
        bail!("refusing to spawn from the sidebar pane");
    }

    let mut args: Vec<String> = if window {
        vec!["new-window".into()]
    } else {
        let mut a = vec!["split-window".into()];
        match direction {
            Direction::Right => a.push("-h".into()),
            Direction::Left => a.extend(["-h".into(), "-b".into()]),
            Direction::Down => a.push("-v".into()),
            Direction::Up => a.extend(["-v".into(), "-b".into()]),
        }
        a.extend(["-t".into(), caller.clone()]);
        a
    };
    if !focus {
        args.push("-d".into());
    }
    if let Some(cwd) = &cwd {
        args.extend(["-c".into(), cwd.clone()]);
    }
    args.extend(["-P".into(), "-F".into(), "#{pane_id}".into()]);
    args.push("--".into());
    args.extend(command.iter().cloned());

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let pane_id = tmux::run(&arg_refs)?;

    let agent_name = name.unwrap_or_else(|| {
        command
            .first()
            .map(|c| c.rsplit('/').next().unwrap_or(c).to_string())
            .unwrap_or_else(|| "agent".to_string())
    });
    let _ = tmux::set_pane_option(&pane_id, "@pane_agent", &agent_name);

    let store = Store::for_current_server()?;
    store.mutate(|state| {
        state.agents.insert(
            pane_id.clone(),
            AgentEntry::new(&pane_id, &agent_name, Status::Unknown, Source::Detected),
        );
    })?;
    let _ = notify::poke();

    // The new pane id is the contract with callers (skill scripts capture it).
    println!("{pane_id}");
    Ok(())
}
