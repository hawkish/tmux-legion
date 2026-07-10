use serde::Deserialize;

/// The subset of Claude Code hook payload fields we care about. Payloads vary
/// per event; every field is optional so unknown shapes never fail.
#[derive(Debug, Default, Deserialize)]
pub struct Payload {
    #[serde(default)]
    pub message: Option<String>,
}

pub fn read_payload_from_stdin() -> Payload {
    use std::io::Read;
    let mut buf = String::new();
    // Cap at 1 MiB: hook payloads are small; never block on a runaway stream.
    let _ = std::io::stdin().take(1024 * 1024).read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or_default()
}
