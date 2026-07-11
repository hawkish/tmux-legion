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
    let mut resolved_name = String::new();
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
        resolved_name = entry.name.clone();
    })?;

    // The tag is the pane's identity for reconciliation (it outlives command
    // name changes under node wrappers), so always set it to the entry name.
    let _ = tmux::set_pane_option(&pane_id, "@pane_agent", &resolved_name);
    let _ = notify::poke();
    Ok(())
}
