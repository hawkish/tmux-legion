use crate::state::{AgentEntry, Store};
use crate::tmux;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

pub enum Outcome {
    Continue,
    Reconcile,
    Quit,
}

pub struct App {
    pub entries: Vec<AgentEntry>,
    pub selected: usize,
    /// Pane id awaiting kill confirmation (`x` pressed, y/n pending).
    pub confirm_kill: Option<String>,
}

impl App {
    pub fn new() -> App {
        App {
            entries: Vec::new(),
            selected: 0,
            confirm_kill: None,
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
    }

    pub fn selected_entry(&self) -> Option<&AgentEntry> {
        self.entries.get(self.selected)
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
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < self.entries.len() {
                    self.selected += 1;
                }
                Outcome::Continue
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
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
            _ => Outcome::Continue,
        }
    }
}
