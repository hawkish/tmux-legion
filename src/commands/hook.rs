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

    // Read the transcript outside the state lock, and only when the answer
    // could have changed: hooks fire on every tool use, but the model only
    // moves when the user runs /model, which Stop picks up within one turn.
    let known_model = store
        .load()
        .agents
        .get(&pane_id)
        .and_then(|e| e.model.clone());
    let session = (known_model.is_none() || event == "Stop")
        .then(|| {
            payload
                .transcript_path
                .as_deref()
                .and_then(claude::read_session)
        })
        .flatten();

    let mut registered = false;
    store.mutate(|state| match action {
        ClaudeAction::Register => {
            let entry = state.agents.entry(pane_id.clone()).or_insert_with(|| {
                registered = true;
                AgentEntry::new(&pane_id, "claude", Status::Idle, Source::Hook)
            });
            entry.last_event = Some(event.to_string());
            apply_session(entry, &session);
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
            apply_session(entry, &session);
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

/// Record what the transcript said, keeping the previous answer when this hook
/// didn't look (or looked at a transcript too young to name a model yet).
fn apply_session(entry: &mut AgentEntry, session: &Option<claude::Session>) {
    if let Some(session) = session {
        entry.model = Some(session.model.clone());
        if session.version.is_some() {
            entry.agent_version = session.version.clone();
        }
    }
}
