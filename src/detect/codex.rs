//! Read Codex's visible UI, not conversation text or another session's files.
use super::Detection;
use crate::status::Status;

pub(super) fn detect(content: &str) -> Detection {
    let lines: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(2)..];
    let prompt = lines.iter().rposition(|line| {
        line.strip_prefix('›')
            .is_some_and(|text| !is_menu_choice(text))
    });
    let footer = prompt
        .filter(|&i| lines.len() - i <= 9)
        .map(|i| &lines[i + 1..])
        .unwrap_or(&[]);
    let footer_model = footer.iter().rev().find_map(|line| model_from_footer(line));
    let model = footer_model.clone().or_else(|| {
        // Older Codex versions show the model in their boxed startup banner.
        lines.iter().rev().find_map(|line| {
            let value = line
                .strip_prefix('│')?
                .trim()
                .strip_prefix("model:")?
                .trim();
            if !value.contains("/model") {
                return None;
            }
            model_token(value)
        })
    });
    let blocked = tail.iter().any(|line| {
        let lower = line.to_ascii_lowercase();
        let hint = lower.strip_prefix("press ").unwrap_or(&lower);
        (hint.starts_with("enter to confirm") && hint.contains("esc to cancel"))
            || (hint.starts_with("enter to select") && hint.contains("esc"))
            || hint.starts_with("enter to submit")
    });
    if blocked {
        return Detection {
            status: Some(Status::Blocked),
            model,
        };
    }
    let ready = footer.len() <= 8
        && (footer_model.is_some()
            || footer
                .iter()
                .any(|line| line.contains("? for shortcuts") || line.contains("context left")));
    let status = prompt.filter(|_| ready).map(|i| {
        // The activity indicator sits just above the composer, sometimes with
        // a one-line tip beneath it. Don't scan previous turns for these words.
        if lines[i.saturating_sub(3)..i].iter().any(|line| {
            line.starts_with('•') && line.contains('(') && line.ends_with("esc to interrupt)")
        }) {
            Status::Working
        } else {
            Status::Idle
        }
    });
    Detection { status, model }
}

/// Approval/menu choices ("› 1. Yes") use the same glyph as the composer.
fn is_menu_choice(text: &str) -> bool {
    text.trim_start()
        .split_once('.')
        .is_some_and(|(number, rest)| number.parse::<u32>().is_ok() && rest.starts_with(' '))
}

fn model_from_footer(line: &str) -> Option<String> {
    // Footer layout: "GPT-6-Astra medium · ~/repo · Thread title".
    // Also handles older "gpt-5.5 · 98% left · ~/repo" footers.
    let (label, _) = line.split_once('·')?;
    let model = model_token(label)?;
    let lower = model.to_ascii_lowercase();
    let reasoning = label.split_whitespace().nth(1);
    let known_model = lower.starts_with("gpt-")
        || lower
            .strip_prefix('o')
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    let has_effort = matches!(
        reasoning,
        Some("minimal" | "low" | "medium" | "high" | "xhigh" | "none")
    );
    (known_model || has_effort).then_some(model)
}

fn model_token(label: &str) -> Option<String> {
    let token = label.split_whitespace().next()?;
    (!token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:/".contains(c)))
    .then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOTER: &str = "\n› Ask Codex to do anything\n\n  GPT-6-Astra medium · ~/git/tmux-legion · Investigate Codex model status\n  ← for agents · ? for shortcuts\n";

    #[test]
    fn reads_model_and_working_from_live_codex_layout() {
        let screen = format!("• Ran a command\n• Working (19s • esc to interrupt)\n  └ Tip: Press f3 to search this conversation.\n{FOOTER}");
        assert_eq!(
            detect(&screen),
            Detection {
                status: Some(Status::Working),
                model: Some("GPT-6-Astra".into()),
            }
        );
        assert_eq!(detect(FOOTER).status, Some(Status::Idle));
    }

    #[test]
    fn approval_and_input_dialogs_are_blocked() {
        for screen in [
            "Would you like to run the following command?\n  $ cargo test\n› 1. Yes, proceed (y)\n  2. No (esc)\nPress enter to confirm or esc to cancel",
            "Choose an option\n› 1. Recommended\n  2. Alternative\nEnter to select · Esc to cancel",
            "Which approach?\n› 1. Recommended\n  2. Alternative\nEnter to submit answer · Tab to add notes",
        ] {
            assert_eq!(detect(screen).status, Some(Status::Blocked));
        }
    }

    #[test]
    fn conversation_text_does_not_drive_status_or_model() {
        let screen = format!("› Explain this output\n• Working (19s • esc to interrupt)\nPress enter to confirm or esc to cancel\nGPT-wrong high · ~/other\n1\n2\n3\n4\n5\n6\n7\n8\n{FOOTER}");
        assert_eq!(detect(&screen).status, Some(Status::Idle));
        assert_eq!(detect(&screen).model.as_deref(), Some("GPT-6-Astra"));
        let quoted_hint = format!("Enter to submit answer\n{FOOTER}");
        assert_eq!(detect(&quoted_hint).status, Some(Status::Idle));
        assert_eq!(
            detect("The output says esc to interrupt and gpt-5.5"),
            Detection::default()
        );
        assert_eq!(detect(""), Detection::default());
        assert_eq!(
            detect("› previous prompt\nplain response"),
            Detection::default()
        );
    }

    #[test]
    fn model_switches_override_banner_and_reasoning_is_not_part_of_model() {
        let banner = "╭────────────────────────────╮\n│ >_ OpenAI Codex (v0.160.1) │\n│ model: gpt-5.4 high /model to change │\n╰────────────────────────────╯";
        assert_eq!(detect(banner).model.as_deref(), Some("gpt-5.4"));
        let screen = format!("{banner}{FOOTER}");
        assert_eq!(detect(&screen).model.as_deref(), Some("GPT-6-Astra"));
        for (label, model) in [
            ("gpt-5.5 · 98% left · ~/repo", "gpt-5.5"),
            ("o3 high · ~/repo", "o3"),
            (
                "provider/custom-model medium · ~/repo",
                "provider/custom-model",
            ),
        ] {
            assert_eq!(
                detect(&format!("› \n{label}")).model.as_deref(),
                Some(model)
            );
        }
    }

    #[test]
    fn older_footer_without_model_still_reports_status() {
        let screen = "• Thinking (2m 1s • esc to interrupt)\n› Write tests\n100% context left · ? for shortcuts";
        assert_eq!(
            detect(screen),
            Detection {
                status: Some(Status::Working),
                model: None
            }
        );
        assert_eq!(
            detect("› 123 is my prompt\n100% context left · ? for shortcuts").status,
            Some(Status::Idle)
        );
    }
}
