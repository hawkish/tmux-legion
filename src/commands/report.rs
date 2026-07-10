use crate::notify;
use crate::state::{AgentEntry, Store};
use crate::status::{Source, Status};
use crate::tmux;
use anyhow::Result;

pub fn report(
    status: Status,
    message: Option<String>,
    name: Option<String>,
    pane: Option<String>,
) -> Result<()> {
    // Silently succeed outside tmux so scripts can call this unconditionally.
    let Some(pane_id) = pane.or_else(tmux::current_pane) else {
        return Ok(());
    };

    let store = Store::for_current_server()?;
    store.mutate(|state| {
        let entry = state.agents.entry(pane_id.clone()).or_insert_with(|| {
            AgentEntry::new(
                &pane_id,
                name.as_deref().unwrap_or("agent"),
                status,
                Source::Reported,
            )
        });
        if let Some(name) = &name {
            entry.name = name.clone();
        }
        entry.set_status(status, message, Source::Reported);
    })?;

    let _ = tmux::set_pane_option(&pane_id, "@pane_agent", name.as_deref().unwrap_or(""));
    let _ = notify::poke();
    Ok(())
}
