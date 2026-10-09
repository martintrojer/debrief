//! The `K` popup: the review comments on the cursor row, in a centered box.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::palette;
use crate::app::App;

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(text) = app.popup() else { return };
    if area.width < 6 || area.height < 3 {
        return;
    }
    let w = (area.width.saturating_sub(4)).clamp(1, 72);
    let lines = text.lines().count() as u16;
    let h = (lines + 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    frame.render_widget(Clear, rect);
    let base = Style::new().fg(palette::TEXT).bg(palette::BASE);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(palette::OVERLAY))
        .title(" review ")
        .style(base);
    frame.render_widget(
        Paragraph::new(text.to_string())
            .block(block)
            .style(base)
            .wrap(Wrap { trim: false }),
        rect,
    );
}
