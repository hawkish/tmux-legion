//! Read Copilot CLI's visible key hints.
use super::Detection;
use crate::status::Status;

/// Detection rules, by priority:
///
/// - **blocked** (priority 300): ESC-cancel hint AND ENTER-accept hint visible
///   → Copilot is showing an interactive selection waiting for user input.
/// - **working** (priority 100): ESC-cancel hint only
///   → Copilot is actively processing (shows "esc to cancel" while running).
/// - **idle** (fallback): neither hint visible.
pub(super) fn detect(content: &str) -> Detection {
    let lower = content.to_lowercase();

    let has_esc_cancel = lower.contains("esc to cancel")
        || lower.contains("esc cancel")
        || lower.contains("esc again to cancel")
        || lower.contains("esc interrupt");

    let has_enter_accept = lower.contains("enter to select")
        || lower.contains("enter to confirm")
        || lower.contains("enter to submit")
        || lower.contains("enter accept");

    let status = if has_esc_cancel && has_enter_accept {
        Status::Blocked
    } else if has_esc_cancel {
        Status::Working
    } else {
        Status::Idle
    };
    Detection {
        status: Some(status),
        model: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(content: &str) -> Option<Status> {
        detect(content).status
    }

    #[test]
    fn idle_when_no_hints() {
        assert_eq!(status("Welcome to GitHub Copilot\n> "), Some(Status::Idle));
    }

    #[test]
    fn working_on_esc_cancel() {
        assert_eq!(status("Running...\n  Esc to cancel"), Some(Status::Working));
    }

    #[test]
    fn working_on_esc_again() {
        assert_eq!(
            status("processing  Esc again to cancel"),
            Some(Status::Working)
        );
    }

    #[test]
    fn blocked_on_selection_ui() {
        let screen = "? Pick a file\n  > src/main.rs\n  Esc to cancel  Enter to select";
        assert_eq!(status(screen), Some(Status::Blocked));
    }

    #[test]
    fn blocked_on_confirm_ui() {
        let screen = "Are you sure?\n  Esc Cancel   Enter to confirm";
        assert_eq!(status(screen), Some(Status::Blocked));
    }

    #[test]
    fn hints_are_case_insensitive() {
        assert_eq!(status("ESC TO CANCEL"), Some(Status::Working));
        assert_eq!(
            status("ESC TO CANCEL  ENTER TO SELECT"),
            Some(Status::Blocked)
        );
    }

    #[test]
    fn never_reports_a_model() {
        assert_eq!(detect("copilot --model gpt-5.5\nEsc to cancel").model, None);
    }
}
