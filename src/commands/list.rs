use crate::state::{self, Store};
use anyhow::Result;

pub fn list(json: bool) -> Result<()> {
    let store = Store::for_current_server()?;
    state::reconcile(&store)?;
    let snapshot = store.load();

    if json {
        let agents: Vec<_> = snapshot.agents.values().collect();
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }

    if snapshot.agents.is_empty() {
        println!("no agents tracked");
        return Ok(());
    }
    println!(
        "{:<8} {:<12} {:<9} {:<20} MESSAGE",
        "PANE", "NAME", "STATUS", "WINDOW"
    );
    for entry in snapshot.agents.values() {
        println!(
            "{:<8} {:<12} {:<9} {:<20} {}",
            entry.pane_id,
            entry.name,
            entry.status.as_str(),
            format!(
                "{}:{}:{}",
                entry.session, entry.window_index, entry.window_name
            ),
            entry.message.as_deref().unwrap_or("")
        );
    }
    Ok(())
}
