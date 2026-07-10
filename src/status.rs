use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }
}

/// Which channel last set an entry's status. `Detected` entries come from
/// pane scanning and are never allowed to overwrite a live hook/reported status.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Hook,
    Reported,
    Detected,
}

/// What a Claude Code hook event means for the pane's entry.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaudeAction {
    Register,
    Set(Status),
    Remove,
    Ignore,
}

pub fn claude_event_action(event: &str, message: Option<&str>) -> ClaudeAction {
    match event {
        "SessionStart" => ClaudeAction::Register,
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => ClaudeAction::Set(Status::Working),
        "Notification" => {
            let msg = message.unwrap_or("").to_lowercase();
            if msg.contains("idle") {
                ClaudeAction::Set(Status::Idle)
            } else {
                // Permission requests and "waiting for your input" both mean
                // the agent cannot proceed without the user.
                ClaudeAction::Set(Status::Blocked)
            }
        }
        "Stop" => ClaudeAction::Set(Status::Done),
        "SessionEnd" => ClaudeAction::Remove,
        _ => ClaudeAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_events_map_to_statuses() {
        assert_eq!(
            claude_event_action("SessionStart", None),
            ClaudeAction::Register
        );
        assert_eq!(
            claude_event_action("UserPromptSubmit", None),
            ClaudeAction::Set(Status::Working)
        );
        assert_eq!(
            claude_event_action("PostToolUse", None),
            ClaudeAction::Set(Status::Working)
        );
        assert_eq!(
            claude_event_action("Stop", None),
            ClaudeAction::Set(Status::Done)
        );
        assert_eq!(
            claude_event_action("SessionEnd", None),
            ClaudeAction::Remove
        );
        assert_eq!(
            claude_event_action("SubagentStop", None),
            ClaudeAction::Ignore
        );
    }

    #[test]
    fn notification_message_classification() {
        assert_eq!(
            claude_event_action(
                "Notification",
                Some("Claude needs your permission to use Bash")
            ),
            ClaudeAction::Set(Status::Blocked)
        );
        assert_eq!(
            claude_event_action("Notification", Some("Claude is waiting for your input")),
            ClaudeAction::Set(Status::Blocked)
        );
        assert_eq!(
            claude_event_action("Notification", Some("Claude has been idle for 60 seconds")),
            ClaudeAction::Set(Status::Idle)
        );
    }
}
