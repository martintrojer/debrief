//! The comment box: a bordered multi-line editor over the diff pane, next
//! to the rows being commented on (placement in
//! [`crate::app::App::comment_box`]). The title names the kind and the
//! lines; the bottom border lists the keys; the terminal cursor sits on
//! the text cursor (so an IME pops up there).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::App;

pub(super) fn draw(frame: &mut Frame, app: &App, pane: Rect) {
    let pal = &app.theme().palette;
    let Some(b) = app.comment_box(pane) else {
        return;
    };
    let base = Style::new().fg(pal.text).bg(pal.bg);
    frame.render_widget(Clear, b.rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(base.fg(pal.purple))
        .title(Line::styled(
            b.title,
            base.fg(pal.purple).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(b.hint, base.fg(pal.muted)))
        .style(base);
    let lines: Vec<Line> = b.lines.into_iter().map(Line::raw).collect();
    frame.render_widget(Paragraph::new(lines).style(base).block(block), b.rect);
    frame.set_cursor_position(b.cursor);
}
