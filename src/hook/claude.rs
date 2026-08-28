use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};

/// The subset of Claude Code hook payload fields we care about. Payloads vary
/// per event; every field is optional so unknown shapes never fail.
#[derive(Debug, Default, Deserialize)]
pub struct Payload {
    #[serde(default)]
    pub message: Option<String>,
    /// Path to the session's JSONL transcript. The payload itself carries no
    /// model, so this file is the only place the model and CLI version show up.
    #[serde(default)]
    pub transcript_path: Option<String>,
}

pub fn read_payload_from_stdin() -> Payload {
    let mut buf = String::new();
    // Cap at 1 MiB: hook payloads are small; never block on a runaway stream.
    let _ = std::io::stdin().take(1024 * 1024).read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or_default()
}

/// What the transcript says the session is running.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    pub model: String,
    /// The Claude Code CLI's own version, e.g. "2.1.220".
    pub version: Option<String>,
}

/// Transcripts reach several MB; a hook must never read one whole. The last
/// assistant record lives at the end, and this is comfortably more than one.
const TAIL_BYTES: u64 = 128 * 1024;

/// Read the model and CLI version off the tail of a session transcript.
/// Best effort throughout: a missing, empty or truncated file yields None.
pub fn read_session(path: &str) -> Option<Session> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from_start = len <= TAIL_BYTES;
    if !from_start {
        file.seek(SeekFrom::Start(len - TAIL_BYTES)).ok()?;
    }
    let mut buf = String::new();
    // Lossy: a mid-character seek must not cost us the whole tail.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    buf.push_str(&String::from_utf8_lossy(&bytes));

    // Unless we started at the top, the first line is a partial record.
    let tail = if from_start {
        buf.as_str()
    } else {
        buf.split_once('\n').map(|(_, rest)| rest)?
    };
    session_from_tail(tail)
}

/// Newest-first scan of whole transcript lines. Split out from the file
/// handling so it can be tested without a transcript on disk.
fn session_from_tail(tail: &str) -> Option<Session> {
    tail.lines().rev().find_map(|line| {
        let record: Record = serde_json::from_str(line).ok()?;
        // Subagents run their own models — one Opus session's transcript is
        // laced with fable-5 sidechain records — and "<synthetic>" marks a
        // message Claude Code wrote itself, not one a model produced.
        if record.record_type.as_deref() != Some("assistant") || record.is_sidechain {
            return None;
        }
        let model = record.message?.model?;
        if model.starts_with('<') {
            return None;
        }
        Some(Session {
            model,
            version: record.version,
        })
    })
}

/// One transcript line. Every field is optional: the file mixes several record
/// shapes and gains new ones between Claude Code releases.
#[derive(Debug, Default, Deserialize)]
struct Record {
    #[serde(default, rename = "type")]
    record_type: Option<String>,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Debug, Default, Deserialize)]
struct Message {
    #[serde(default)]
    model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = concat!(
        r#"{"type":"mode","mode":"normal"}"#,
        "\n",
        r#"{"type":"assistant","version":"2.1.100","message":{"model":"claude-opus-4-8","role":"assistant"}}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
        "\n",
        r#"{"type":"assistant","version":"2.1.220","message":{"model":"claude-opus-5","role":"assistant"}}"#,
        "\n",
        r#"{"type":"assistant","isSidechain":true,"version":"2.1.220","message":{"model":"claude-fable-5"}}"#,
        "\n",
        r#"{"type":"assistant","version":"2.1.220","message":{"model":"<synthetic>"}}"#,
        "\n",
    );

    #[test]
    fn reads_the_newest_main_thread_model() {
        assert_eq!(
            session_from_tail(TRANSCRIPT),
            Some(Session {
                model: "claude-opus-5".into(),
                version: Some("2.1.220".into()),
            })
        );
    }

    #[test]
    fn transcripts_without_an_assistant_record_say_nothing() {
        assert_eq!(session_from_tail(""), None);
        assert_eq!(session_from_tail(r#"{"type":"user"}"#), None);
        assert_eq!(session_from_tail("not json at all\n"), None);
    }

    #[test]
    fn a_versionless_record_still_yields_a_model() {
        let line = r#"{"type":"assistant","message":{"model":"claude-opus-5"}}"#;
        assert_eq!(
            session_from_tail(line),
            Some(Session {
                model: "claude-opus-5".into(),
                version: None,
            })
        );
    }

    #[test]
    fn payload_ignores_unknown_fields() {
        let payload: Payload = serde_json::from_str(
            r#"{"session_id":"s","transcript_path":"/tmp/t.jsonl","hook_event_name":"Stop"}"#,
        )
        .unwrap();
        assert_eq!(payload.transcript_path.as_deref(), Some("/tmp/t.jsonl"));
        assert_eq!(payload.message, None);
    }

    #[test]
    fn reads_a_transcript_from_disk_and_survives_a_missing_one() {
        let path = std::env::temp_dir().join(format!("legion-transcript-{}", std::process::id()));
        std::fs::write(&path, TRANSCRIPT).unwrap();
        let session = read_session(path.to_str().unwrap()).unwrap();
        assert_eq!(session.model, "claude-opus-5");
        std::fs::remove_file(&path).unwrap();

        assert_eq!(read_session(path.to_str().unwrap()), None);
    }
}
