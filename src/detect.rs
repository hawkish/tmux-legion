use crate::status::Status;
use crate::tmux;

mod codex;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Detection {
    pub status: Option<Status>,
    pub model: Option<String>,
}

/// Capture only supported agents, once per reconcile. Missing UI signals leave
/// existing state intact; a failed capture must not look like an idle agent.
pub fn detect(pane_id: &str, agent_name: &str) -> Option<Detection> {
    if !matches!(agent_name, "codex" | "copilot" | "github-copilot" | "ghcs") {
        return None;
    }
    let content = tmux::capture_pane(pane_id).ok()?;
    Some(if agent_name == "codex" {
        codex::detect(&content)
    } else {
        Detection {
            status: detect_from_content(agent_name, &content),
            model: None,
        }
    })
}

/// Pure detection logic — takes the already-captured screen content so it can
/// be called from unit tests without a live tmux server.
pub fn detect_from_content(agent_name: &str, content: &str) -> Option<Status> {
    match agent_name {
        "copilot" | "github-copilot" | "ghcs" => Some(copilot_status(content)),
        _ => None,
    }
}

/// Detection rules, by priority:
///
/// - **blocked** (priority 300): ESC-cancel hint AND ENTER-accept hint visible
///   → Copilot is showing an interactive selection waiting for user input.
/// - **working** (priority 100): ESC-cancel hint only
///   → Copilot is actively processing (shows "esc to cancel" while running).
/// - **idle** (fallback): neither hint visible.
fn copilot_status(content: &str) -> Status {
    let lower = content.to_lowercase();

    let has_esc_cancel = lower.contains("esc to cancel")
        || lower.contains("esc cancel")
        || lower.contains("esc again to cancel")
        || lower.contains("esc interrupt");

    let has_enter_accept = lower.contains("enter to select")
        || lower.contains("enter to confirm")
        || lower.contains("enter to submit")
        || lower.contains("enter accept");

    if has_esc_cancel && has_enter_accept {
        Status::Blocked
    } else if has_esc_cancel {
        Status::Working
    } else {
        Status::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_idle_when_no_hints() {
        assert_eq!(
            detect_from_content("copilot", "Welcome to GitHub Copilot\n> "),
            Some(Status::Idle)
        );
    }

    #[test]
    fn copilot_working_on_esc_cancel() {
        assert_eq!(
            detect_from_content("copilot", "Running...\n  Esc to cancel"),
            Some(Status::Working)
        );
    }

    #[test]
    fn copilot_working_on_esc_again() {
        assert_eq!(
            detect_from_content("copilot", "processing  Esc again to cancel"),
            Some(Status::Working)
        );
    }

    #[test]
    fn copilot_blocked_on_selection_ui() {
        let screen = "? Pick a file\n  > src/main.rs\n  Esc to cancel  Enter to select";
        assert_eq!(
            detect_from_content("copilot", screen),
            Some(Status::Blocked)
        );
    }

    #[test]
    fn copilot_blocked_on_confirm_ui() {
        let screen = "Are you sure?\n  Esc Cancel   Enter to confirm";
        assert_eq!(
            detect_from_content("copilot", screen),
            Some(Status::Blocked)
        );
    }

    #[test]
    fn copilot_hints_are_case_insensitive() {
        assert_eq!(
            detect_from_content("copilot", "ESC TO CANCEL"),
            Some(Status::Working)
        );
        assert_eq!(
            detect_from_content("copilot", "ESC TO CANCEL  ENTER TO SELECT"),
            Some(Status::Blocked)
        );
    }

    #[test]
    fn unknown_agent_returns_none() {
        assert_eq!(detect_from_content("aider", "anything"), None);
        assert_eq!(detect_from_content("claude", "anything"), None);
    }

    #[test]
    fn copilot_aliases_recognised() {
        assert_eq!(
            detect_from_content("github-copilot", "Esc to cancel"),
            Some(Status::Working)
        );
        assert_eq!(
            detect_from_content("ghcs", "Esc to cancel"),
            Some(Status::Working)
        );
    }
}
