use crate::hook::claude;
use crate::notify;
use crate::state::{AgentEntry, Store};
use crate::status::{claude_event_action, ClaudeAction, Source, Status};
use crate::tmux;

/// Handle a platform hook event. Must be fast, silent, and infallible from
/// the caller's point of view — a broken hook must never break the agent.
pub fn handle(agent: &str, event: &str) {
    let _ = try_handle(agent, event);
}

fn try_handle(agent: &str, event: &str) -> anyhow::Result<()> {
    if agent != "claude" {
        return Ok(());
    }
    // Claude running outside tmux: nothing to track.
    let Some(pane_id) = tmux::current_pane() else {
        return Ok(());
    };

    let payload = claude::read_payload_from_stdin();
    let action = claude_event_action(event, payload.message.as_deref());
    if action == ClaudeAction::Ignore {
        return Ok(());
    }

    let store = Store::for_current_server()?;
    let mut registered = false;
    store.mutate(|state| match action {
        ClaudeAction::Register => {
            let entry = state.agents.entry(pane_id.clone()).or_insert_with(|| {
                registered = true;
                AgentEntry::new(&pane_id, "claude", Status::Idle, Source::Hook)
            });
            entry.last_event = Some(event.to_string());
        }
        ClaudeAction::Set(status) => {
            let entry = state.agents.entry(pane_id.clone()).or_insert_with(|| {
                registered = true;
                AgentEntry::new(&pane_id, "claude", status, Source::Hook)
            });
            let message = match status {
                Status::Blocked => payload.message.clone(),
                _ => None,
            };
            entry.set_status(status, message, Source::Hook);
            entry.last_event = Some(event.to_string());
        }
        ClaudeAction::Remove => {
            state.agents.remove(&pane_id);
        }
        ClaudeAction::Ignore => {}
    })?;

    // Tag the pane so discovery/reconciliation can identify it; once is
    // enough — hooks fire on every tool use, so don't shell out each time.
    if registered {
        let _ = tmux::set_pane_option(&pane_id, "@pane_agent", "claude");
    }
    let _ = notify::poke();
    Ok(())
}
