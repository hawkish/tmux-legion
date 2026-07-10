use super::app::App;
use crate::state::{self, AgentEntry};
use crate::status::Status;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, app: &mut App) {
    let [list_area, footer_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    let items: Vec<ListItem> = app.entries.iter().map(entry_item).collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .title(Line::from(" legion ").bold()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut list_state = ListState::default();
    if !app.entries.is_empty() {
        list_state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list, list_area, &mut list_state);

    let footer = match &app.confirm_kill {
        Some(pane) => Line::from(format!("kill {pane}? y/n")).red().bold(),
        None => Line::from("j/k ↵jump x kill q quit").dim(),
    };
    frame.render_widget(Paragraph::new(footer), footer_area);
}

fn entry_item(entry: &AgentEntry) -> ListItem<'static> {
    let (glyph, color) = match entry.status {
        Status::Working => ("●", Color::Green),
        Status::Blocked => ("●", Color::Red),
        Status::Done => ("●", Color::Cyan),
        Status::Idle => ("○", Color::DarkGray),
        Status::Unknown => ("?", Color::DarkGray),
    };

    let mut name_style = Style::default();
    if entry.status == Status::Blocked {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }

    let mut lines = vec![Line::from(vec![
        Span::styled(glyph, Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(entry.name.clone(), name_style),
        Span::raw(" "),
        Span::styled(
            format!("{}:{}", entry.window_index, entry.window_name),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(
            elapsed(entry.status_changed_at),
            Style::default().fg(Color::DarkGray),
        ),
    ])];

    if let Some(message) = &entry.message {
        lines.push(Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    ListItem::new(lines)
}

fn elapsed(since: u64) -> String {
    let secs = state::now().saturating_sub(since);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}
