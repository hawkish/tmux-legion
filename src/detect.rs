use crate::tmux;

mod codex;
mod copilot;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Detection {
    pub status: Option<crate::status::Status>,
    pub model: Option<String>,
}

/// Capture only supported agents, once per reconcile. Missing UI signals leave
/// existing state intact; a failed capture must not look like an idle agent.
pub fn detect(pane_id: &str, agent_name: &str) -> Option<Detection> {
    let parse = parser(agent_name)?;
    let content = tmux::capture_pane(pane_id).ok()?;
    Some(parse(&content))
}

/// The screen parser for an agent, or `None` when it reports through hooks or
/// for itself. Each parser is pure so tests need no live tmux server.
fn parser(agent_name: &str) -> Option<fn(&str) -> Detection> {
    match agent_name {
        "codex" => Some(codex::detect),
        "copilot" | "github-copilot" | "ghcs" => Some(copilot::detect),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Status;

    #[test]
    fn unknown_agent_has_no_parser() {
        assert!(parser("aider").is_none());
        assert!(parser("claude").is_none());
    }

    #[test]
    fn copilot_aliases_recognised() {
        for name in ["copilot", "github-copilot", "ghcs"] {
            let parse = parser(name).expect(name);
            assert_eq!(parse("Esc to cancel").status, Some(Status::Working));
        }
    }
}
