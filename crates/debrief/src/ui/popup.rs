//! The `K` popup: the review comments on the cursor row, in a centered box.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

pub(super) fn draw(frame: &mut Frame, app: &App, _area: Rect) {
    let pal = &app.theme().palette;
    let Some(v) = app.popup_view() else { return };
    let rect = v.rect;
    frame.render_widget(Clear, rect);
    let base = Style::new().fg(pal.text).bg(pal.bg);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(pal.muted))
        .title(" review ")
        .style(base);
    let body = rect.height.saturating_sub(2) as usize;
    let shown: Vec<Line> = v
        .lines
        .iter()
        .skip(v.scroll)
        .take(body)
        .map(|l| Line::raw(l.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(shown).block(block).style(base), rect);
}
