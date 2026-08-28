use super::ui::ROWS_PER_ENTRY;
use crate::keyboard;
use crate::state::{AgentEntry, Store};
use crate::tmux;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};

pub enum Outcome {
    Continue,
    Reconcile,
    Quit,
}

pub struct App {
    pub entries: Vec<AgentEntry>,
    pub selected: usize,
    /// First visible entry index (kept in view range by the renderer).
    pub scroll: usize,
    /// Advances every render loop iteration; drives the working spinner.
    pub spinner_tick: u32,
    /// Pane id awaiting kill confirmation (`x` pressed, y/n pending).
    pub confirm_kill: Option<String>,
    /// Set when the keyboard was found and then stopped working. The keyboard
    /// module cannot print, so this footer marker is the only way the user
    /// learns their LEDs went stale.
    pub led_warning: bool,
}

impl App {
    pub fn new() -> App {
        App {
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            spinner_tick: 0,
            confirm_kill: None,
            led_warning: false,
        }
    }

    pub fn reload(&mut self, store: &Store) {
        let selected_pane = self.selected_entry().map(|e| e.pane_id.clone());

        let mut entries: Vec<AgentEntry> = store.load().agents.into_values().collect();
        // Stable order: no row jumping when statuses change.
        entries.sort_by(|a, b| {
            (&a.session, a.window_index, &a.pane_id).cmp(&(&b.session, b.window_index, &b.pane_id))
        });
        self.entries = entries;

        // Keep the selection on the same agent across reloads.
        self.selected = selected_pane
            .and_then(|pane| self.entries.iter().position(|e| e.pane_id == pane))
            .unwrap_or_else(|| self.selected.min(self.entries.len().saturating_sub(1)));

        // Follow tmux focus: when the user switches to a tracked agent's
        // pane, move the highlight there. Focus on the sidebar itself or on
        // a non-agent pane matches no entry and leaves the selection alone,
        // so -/+ navigation inside the sidebar is never fought.
        if let Some(focused) = tmux::focused_pane() {
            if let Some(idx) = self.entries.iter().position(|e| e.pane_id == focused) {
                self.selected = idx;
            }
        }
    }

    pub fn selected_entry(&self) -> Option<&AgentEntry> {
        self.entries.get(self.selected)
    }

    /// Entry index at a screen row, or None if the row is the header, footer,
    /// or below the last entry. Rendering: header on row 0, spacer on row 1,
    /// entries in ROWS_PER_ENTRY-row blocks from row 2, footer on the last row.
    /// Every block is the same height even when an agent's model is unknown,
    /// which is what keeps this arithmetic honest.
    fn entry_at_row(&self, row: u16, term_height: u16) -> Option<usize> {
        if row < 2 || row + 1 >= term_height {
            return None;
        }
        let block = (row - 2) as usize / ROWS_PER_ENTRY as usize;
        let idx = self.scroll + block;
        (idx < self.entries.len()).then_some(idx)
    }

    fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Left-click an entry: move the highlight there and focus that agent's
    /// pane (same as -/+ + Enter). The wheel moves the highlight like -/+ —
    /// the view scrolls with it, since scroll always follows the selection.
    /// `term_height` locates the footer row so header/footer clicks are ignored.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, term_height: u16) -> Outcome {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.entry_at_row(mouse.row, term_height) {
                    self.selected = idx;
                    let _ = tmux::select_pane(&self.entries[idx].pane_id);
                }
            }
            MouseEventKind::ScrollDown => self.select_next(),
            MouseEventKind::ScrollUp => self.select_prev(),
            _ => {}
        }
        Outcome::Continue
    }

    pub fn handle_key(&mut self, key: KeyEvent, store: &Store) -> Outcome {
        if key.kind != KeyEventKind::Press {
            return Outcome::Continue;
        }

        if let Some(pane) = self.confirm_kill.take() {
            if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                let _ = tmux::kill_pane(&pane);
                let _ = store.mutate(|state| {
                    state.agents.remove(&pane);
                });
                return Outcome::Reconcile;
            }
            return Outcome::Continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Outcome::Quit,
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Down => {
                self.select_next();
                Outcome::Continue
            }
            KeyCode::Char('-') | KeyCode::Up => {
                self.select_prev();
                Outcome::Continue
            }
            KeyCode::Char('g') => {
                self.selected = 0;
                Outcome::Continue
            }
            KeyCode::Char('G') => {
                self.selected = self.entries.len().saturating_sub(1);
                Outcome::Continue
            }
            KeyCode::Enter => {
                if let Some(entry) = self.selected_entry() {
                    let _ = tmux::select_pane(&entry.pane_id);
                }
                Outcome::Continue
            }
            KeyCode::Char('x') => {
                self.confirm_kill = self.selected_entry().map(|e| e.pane_id.clone());
                Outcome::Continue
            }
            KeyCode::Char('r') => Outcome::Reconcile,
            // Slot keys. The numpad 4/5/6/1 keys are remapped to F16-F19 in
            // firmware, and a root-table tmux binding normally swallows those
            // before they reach us — this is the fallback for terminals that
            // drop F13+, plus the top-row digits. The digits come from
            // SLOT_KEYS rather than a range: the slots are not in key order.
            KeyCode::F(n @ 16..=19) => self.focus_slot(n - 15),
            KeyCode::Char(c) => match keyboard::SLOT_KEYS.iter().position(|&k| k == c) {
                Some(i) => self.focus_slot(i as u8 + 1),
                None => Outcome::Continue,
            },
            _ => Outcome::Continue,
        }
    }

    fn focus_slot(&self, slot: u8) -> Outcome {
        if let Some(entry) = self.entries.iter().find(|e| e.slot == Some(slot)) {
            let _ = tmux::select_pane(&entry.pane_id);
        }
        Outcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentEntry;
    use crate::status::{Source, Status};

    fn app_with(n: usize, scroll: usize) -> App {
        let mut app = App::new();
        app.entries = (0..n)
            .map(|i| AgentEntry::new(&format!("%{i}"), "a", Status::Working, Source::Reported))
            .collect();
        app.scroll = scroll;
        app
    }

    #[test]
    fn row_maps_to_entry_block() {
        // 3 entries, term height 30, footer at row 29. Blocks: entry 0 = rows
        // 2-5, entry 1 = rows 6-9, entry 2 = rows 10-13.
        let app = app_with(3, 0);
        assert_eq!(app.entry_at_row(0, 30), None); // header
        assert_eq!(app.entry_at_row(1, 30), None); // spacer below header
        assert_eq!(app.entry_at_row(2, 30), Some(0)); // first name line
        assert_eq!(app.entry_at_row(3, 30), Some(0)); // its model line
        assert_eq!(app.entry_at_row(5, 30), Some(0)); // spacer of block 0
        assert_eq!(app.entry_at_row(6, 30), Some(1)); // second name line
        assert_eq!(app.entry_at_row(10, 30), Some(2));
        assert_eq!(app.entry_at_row(29, 30), None); // footer row
        assert_eq!(app.entry_at_row(14, 30), None); // below last entry
    }

    #[test]
    fn row_accounts_for_scroll() {
        let app = app_with(10, 2);
        // First visible block now shows entry 2.
        assert_eq!(app.entry_at_row(2, 30), Some(2));
        assert_eq!(app.entry_at_row(6, 30), Some(3));
    }
}
