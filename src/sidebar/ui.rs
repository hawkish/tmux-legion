//! Herdr-style agents panel: two lines per agent (bold name, dim colored
//! status with `·` separators), a spacer row between entries, braille spinner
//! while working. Rendered manually per row so the selection highlight covers
//! exactly the two content lines, like herdr's active-pane highlight.
use super::app::App;
use super::theme;
use crate::state::AgentEntry;
use crate::status::Status;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Rows per agent entry: name line + status line + spacer.
pub const ROWS_PER_ENTRY: u16 = 3;
/// Header (title) + footer (help) rows.
pub const CHROME_ROWS: u16 = 2;

pub fn visible_entries(area_height: u16) -> usize {
    (area_height.saturating_sub(CHROME_ROWS) / ROWS_PER_ENTRY) as usize
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.height < CHROME_ROWS {
        return;
    }

    render_header(frame, app, Rect::new(area.x, area.y, area.width, 1));

    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(CHROME_ROWS),
    );
    render_entries(frame, app, body);

    let footer_y = area.y + area.height - 1;
    render_footer(frame, app, Rect::new(area.x, footer_y, area.width, 1));
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " agents",
            Style::default()
                .fg(theme::OVERLAY0)
                .add_modifier(Modifier::BOLD),
        ))),
        area,
    );

    // Right-aligned count; blocked agents get a red attention badge.
    let blocked = app
        .entries
        .iter()
        .filter(|e| e.status == Status::Blocked)
        .count();
    let badge = if blocked > 0 {
        Line::from(vec![
            Span::styled(
                format!("● {blocked} "),
                Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("/ {} ", app.entries.len()),
                Style::default().fg(theme::OVERLAY0),
            ),
        ])
    } else {
        Line::from(Span::styled(
            format!("{} ", app.entries.len()),
            Style::default().fg(theme::OVERLAY0),
        ))
    };
    frame.render_widget(
        Paragraph::new(badge).alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

fn render_entries(frame: &mut Frame, app: &mut App, body: Rect) {
    if app.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no agents",
                Style::default().fg(theme::OVERLAY0),
            ))),
            Rect::new(body.x, body.y + 1, body.width, 1),
        );
        return;
    }

    // Keep the selection in view.
    let visible = visible_entries(body.height + CHROME_ROWS).max(1);
    if app.selected < app.scroll {
        app.scroll = app.selected;
    } else if app.selected >= app.scroll + visible {
        app.scroll = app.selected + 1 - visible;
    }

    let mut row_y = body.y;
    let bottom = body.y + body.height;
    for (idx, entry) in app.entries.iter().enumerate().skip(app.scroll) {
        if row_y + 1 >= bottom {
            break;
        }
        let selected = idx == app.selected;
        let row_style = if selected {
            Style::default().bg(theme::SURFACE0)
        } else {
            Style::default()
        };

        let (icon, icon_color) = status_icon(entry.status, app.spinner_tick);
        let name_style = if selected {
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme::SUBTEXT0)
                .add_modifier(Modifier::BOLD)
        };
        let name = truncate(&entry.name, body.width.saturating_sub(3) as usize);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw(" "),
                Span::styled(name, name_style),
            ]))
            .style(row_style),
            Rect::new(body.x, row_y, body.width, 1),
        );
        row_y += 1;

        if row_y < bottom {
            let mut label_style = Style::default().fg(status_color(entry.status));
            if !selected {
                label_style = label_style.add_modifier(Modifier::DIM);
            }
            let dim = Style::default()
                .fg(theme::OVERLAY0)
                .add_modifier(Modifier::DIM);

            let label = status_label(entry.status);
            let window = window_label(entry);
            let mut spans = vec![
                Span::raw("   "),
                Span::styled(label, label_style),
                Span::styled(" · ", dim),
                Span::styled(window.clone(), dim),
            ];
            if let Some(message) = &entry.message {
                // Ellipsize the message into whatever width remains.
                let used = 3 + label.chars().count() + 3 + window.chars().count() + 3;
                let room = (body.width as usize).saturating_sub(used);
                if room > 1 {
                    spans.push(Span::styled(" · ", dim));
                    spans.push(Span::styled(truncate(message, room), dim));
                }
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(row_style),
                Rect::new(body.x, row_y, body.width, 1),
            );
            row_y += 1;
        }

        // Spacer between entries (unstyled, like herdr).
        if row_y < bottom {
            row_y += 1;
        }
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let footer = match &app.confirm_kill {
        Some(pane) => Line::from(Span::styled(
            format!(" kill {pane}? y/n"),
            Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
        )),
        None => Line::from(Span::styled(
            " j/k ↵ jump  x kill  q quit",
            Style::default()
                .fg(theme::OVERLAY0)
                .add_modifier(Modifier::DIM),
        )),
    };
    frame.render_widget(Paragraph::new(footer), area);
}

/// herdr's agent_icon: blocked ◉ red, working spinner yellow, done ● teal,
/// idle ✓ green, unknown ○ gray.
fn status_icon(status: Status, tick: u32) -> (&'static str, Color) {
    match status {
        Status::Blocked => ("◉", theme::RED),
        Status::Working => (theme::spinner_frame(tick), theme::YELLOW),
        Status::Done => ("●", theme::TEAL),
        Status::Idle => ("✓", theme::GREEN),
        Status::Unknown => ("○", theme::OVERLAY0),
    }
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Blocked => theme::RED,
        Status::Working => theme::YELLOW,
        Status::Done => theme::TEAL,
        Status::Idle => theme::GREEN,
        Status::Unknown => theme::OVERLAY0,
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Blocked => "blocked",
        Status::Working => "working",
        Status::Done => "done",
        Status::Idle => "idle",
        Status::Unknown => "unknown",
    }
}

fn window_label(entry: &AgentEntry) -> String {
    format!("{}:{}", entry.window_index, entry.window_name)
}

fn truncate(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len <= max_width {
        return text.to_string();
    }
    match max_width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let prefix: String = text.chars().take(max_width - 1).collect();
            format!("{prefix}…")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate("claude", 10), "claude");
        assert_eq!(truncate("long-agent-name", 8), "long-ag…");
        assert_eq!(truncate("x", 1), "x");
        assert_eq!(truncate("xy", 1), "…");
        assert_eq!(truncate("xy", 0), "");
    }

    #[test]
    fn entry_capacity_per_height() {
        assert_eq!(visible_entries(11), 3);
        assert_eq!(visible_entries(2), 0);
    }
}
