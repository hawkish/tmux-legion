//! Read OpenCode's visible footer and permission dialog. OpenCode has no
//! shell-hook system, so the reconciler reads the pane's screen instead.
use super::Detection;
use crate::status::Status;

/// Detection rules, by priority:
///
/// - **blocked**: the permission dialog is up ("Permission required", or one
///   of its option labels "Allow once"/"Always allow"/"Reject permission")
///   → OpenCode is waiting on a tool-use approval.
/// - **working**: the footer's "esc interrupt" hint is visible while a turn
///   runs.
/// - **idle** (fallback): neither is visible — ready for a prompt, or a turn
///   just finished. Screen detection doesn't distinguish those, same as
///   Codex and Copilot.
pub(super) fn detect(content: &str) -> Detection {
    let status = if is_permission_dialog(content) {
        Status::Blocked
    } else if content.contains("esc interrupt") {
        Status::Working
    } else {
        Status::Idle
    };
    Detection {
        status: Some(status),
        model: model_label(content),
    }
}

fn is_permission_dialog(content: &str) -> bool {
    content.contains("Permission required")
        || content.contains("Allow once")
        || content.contains("Always allow")
        || content.contains("Reject permission")
}

/// Provider names OpenCode's status line tacks onto the model, longest first
/// so "Azure OpenAI" strips before a bare "OpenAI" would. Not exhaustive —
/// an unlisted provider just leaves its name attached to the model, which
/// still lets a family match (see `sidebar::model_family`) find "claude" or
/// "gpt" inside the combined string.
const PROVIDERS: &[&str] = &[
    "Amazon Bedrock",
    "Azure OpenAI",
    "Google Vertex AI",
    "GitHub Copilot",
    "Hugging Face",
    "Moonshot AI",
    "Mistral AI",
    "Together AI",
    "Fireworks AI",
    "Venice AI",
    "OpenRouter",
    "Anthropic",
    "Perplexity",
    "DeepSeek",
    "Cerebras",
    "Groq",
    "OpenAI",
    "Google",
    "Azure",
    "xAI",
];

/// The model OpenCode's status line names, stripped of its trailing provider
/// name where recognised: "Build · Claude Sonnet 5 GitHub Copilot" is the
/// line directly above the footer's bottom border, mode and model-provider
/// joined by `·`.
fn model_label(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    let border = lines
        .iter()
        .position(|line| line.contains("▀▀▀▀") || line.trim_start().starts_with('╹'))?;
    let line = lines.get(border.checked_sub(1)?)?.trim();
    let (_, rest) = line.split_once('·')?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    let stripped = PROVIDERS
        .iter()
        .find_map(|provider| rest.strip_suffix(provider))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(rest);
    Some(stripped.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(content: &str) -> Option<Status> {
        detect(content).status
    }

    #[test]
    fn idle_when_the_prompt_is_ready() {
        let screen = "\n\n\nBuild · Claude Sonnet 5 GitHub Copilot\n~/git/tmux-legion:main";
        assert_eq!(status(screen), Some(Status::Idle));
    }

    #[test]
    fn working_while_a_turn_runs() {
        let screen = "⬝⬝⬝⬝⬝⬝⬝⬝ esc interrupt     ↓ 2 shells · 325.0K (32%) · $8.91  ctrl+p commands";
        assert_eq!(status(screen), Some(Status::Working));
    }

    #[test]
    fn blocked_on_a_permission_prompt() {
        for screen in [
            "△ Permission required\n\nAllow once   Always allow   Reject",
            "△ Permission\n\nAllow once   Always allow   Reject",
            "△ Always allow\n\nConfirm   Cancel",
            "△ Reject permission\n\nTell OpenCode what to do differently",
        ] {
            assert_eq!(status(screen), Some(Status::Blocked), "{screen:?}");
        }
    }

    #[test]
    fn model_is_read_from_the_status_line_and_the_provider_stripped() {
        let screen = "Build · Claude Sonnet 5 GitHub Copilot\n╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n";
        assert_eq!(detect(screen).model.as_deref(), Some("Claude Sonnet 5"));

        let screen = " ⬝⬝⬝⬝⬝⬝⬝⬝ esc interrupt\nBuild · GPT-6 Astra OpenAI\n╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n";
        assert_eq!(detect(screen).model.as_deref(), Some("GPT-6 Astra"));
    }

    #[test]
    fn an_unrecognised_provider_is_left_in_the_model() {
        let screen = "Build · Llama 4 Groq\n╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n";
        assert_eq!(detect(screen).model.as_deref(), Some("Llama 4"));
        let screen = "Build · Some Future Model Zed AI\n╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n";
        assert_eq!(
            detect(screen).model.as_deref(),
            Some("Some Future Model Zed AI")
        );
    }

    #[test]
    fn no_status_line_means_no_model() {
        assert_eq!(detect("nothing here").model, None);
        assert_eq!(detect("").model, None);
    }
}
