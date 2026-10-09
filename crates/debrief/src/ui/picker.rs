//! The `<leader>rl` picker: a centered box with the filter line, the
//! matching comments (`path:lines @commit  body`) and the keys.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::palette;
use crate::app::{App, help_rect};

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(view) = app.picker() else { return };
    if area.width < 6 || area.height < 5 {
        return;
    }
    let rect = help_rect(area);
    frame.render_widget(Clear, rect);
    let base = Style::new().fg(palette::TEXT).bg(palette::BASE);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(palette::OVERLAY))
        .title(format!(" {} ({}) ", view.title, view.items.len()))
        .style(base);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let [query, list, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let q = format!("> {}", view.input);
    let x = query.x + Span::raw(q.as_str()).width() as u16;
    frame.set_cursor_position((x.min(query.right().saturating_sub(1)), query.y));
    frame.render_widget(Paragraph::new(q).style(base.fg(palette::BLUE)), query);
    let h = list.height as usize;
    let skip = (view.selected + 1).saturating_sub(h);
    let lines: Vec<Line> = if view.items.is_empty() {
        vec![Line::from(Span::styled(
            "No comments",
            base.fg(palette::OVERLAY),
        ))]
    } else {
        view.items
            .iter()
            .enumerate()
            .skip(skip)
            .take(h)
            .map(|(i, it)| {
                let s = if i == view.selected {
                    base.bg(palette::SURFACE).add_modifier(Modifier::BOLD)
                } else {
                    base
                };
                Line::from(vec![
                    Span::styled(it.label.clone(), s.fg(palette::BLUE)),
                    Span::styled(format!("  {}", it.detail), s),
                ])
                .style(s)
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).style(base), list);
    frame.render_widget(
        Paragraph::new(view.footer).style(base.fg(palette::OVERLAY)),
        footer,
    );
}
