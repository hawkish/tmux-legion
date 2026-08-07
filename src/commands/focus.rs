use crate::state::{self, Store};
use crate::tmux;
use anyhow::Result;

/// Focus the agent holding a keyboard slot.
///
/// Always succeeds. This runs from `run-shell -b`, where stderr is invisible
/// and a non-zero exit can pop an error pane — neither is what anyone wants
/// from a mistyped keypress, so misses go to the status line instead.
pub fn focus(slot: u8) -> Result<()> {
    let store = Store::for_current_server()?;

    let mut pane = resolve(&store, slot);
    // Slots are normally assigned by the sidebar's reconcile loop. With the
    // sidebar closed the state file can be stale or carry no slots at all, so
    // pay for one list-panes rather than silently doing nothing.
    if pane.is_none() {
        let _ = state::reconcile(&store);
        pane = resolve(&store, slot);
    }

    let Some(pane) = pane else {
        return notice(&format!("legion: no agent on slot {slot}"));
    };
    if tmux::select_pane(&pane).is_err() {
        return notice(&format!("legion: slot {slot} pane {pane} is gone"));
    }
    Ok(())
}

fn resolve(store: &Store, slot: u8) -> Option<String> {
    store
        .load()
        .agents
        .values()
        .find(|e| e.slot == Some(slot))
        .map(|e| e.pane_id.clone())
}

fn notice(message: &str) -> Result<()> {
    let _ = tmux::run(&["display-message", message]);
    Ok(())
}
