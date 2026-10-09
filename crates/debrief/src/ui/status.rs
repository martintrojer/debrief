//! The status line: repo root, VCS kind, the new count, a status message,
//! the `g?` hint and the cursor position.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::palette;
use crate::app::App;
use crate::vcs::Kind;

/// The key that opens the help overlay, shown dimmed while it fits.
pub const HELP_HINT: &str = "g? help";

/// `s` in at most `max` characters, cut from the left behind `…`.
fn elide_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep: String = s.chars().skip(n - (max - 1)).collect();
    format!("…{keep}")
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let base = Style::new().fg(palette::TEXT).bg(palette::MANTLE);
    let kind = match app.kind() {
        Kind::Git => "git",
        Kind::Jj => "jj",
    };
    let (new, updated) = app.counts();
    let mut counts = format!("{new} new");
    if updated > 0 {
        counts.push_str(&format!(", {updated} updated"));
    }
    let right = match app.rows().len() {
        0 => " 0/0 ".to_string(),
        n => format!(" {}/{n} ", app.cursor() + 1),
    };
    let tail = [format!("  {kind}"), format!("  {counts}")];
    let fixed: usize = 1 + tail.iter().map(|t| t.chars().count()).sum::<usize>() + right.len();
    let root = elide_left(
        &app.root_label(),
        (area.width as usize).saturating_sub(fixed),
    );
    let [kind, counts] = tail;
    let mut spans = vec![
        Span::styled(format!(" {root}"), base.fg(palette::BLUE)),
        Span::styled(kind, base.fg(palette::OVERLAY)),
        Span::styled(counts, base.fg(palette::SKY)),
    ];
    if !app.status().is_empty() {
        spans.push(Span::styled(
            format!("  {}", app.status()),
            base.fg(palette::YELLOW),
        ));
    }
    let left_w: usize = spans.iter().map(Span::width).sum();
    let right_w = Span::raw(right.as_str()).width();
    let hint_w = HELP_HINT.len();
    let hint = left_w + right_w + hint_w + 2 <= area.width as usize;
    let used = left_w + right_w + if hint { hint_w } else { 0 };
    spans.push(Span::styled(
        " ".repeat((area.width as usize).saturating_sub(used)),
        base,
    ));
    if hint {
        spans.push(Span::styled(HELP_HINT, base.fg(palette::OVERLAY)));
    }
    spans.push(Span::styled(right, base.fg(palette::TEAL)));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

#[cfg(test)]
mod tests {
    use super::elide_left;

    #[test]
    fn elides_from_the_left() {
        assert_eq!(elide_left("~/src/repo", 20), "~/src/repo");
        assert_eq!(elide_left("/very/long/path/repo", 8), "…th/repo");
        assert_eq!(elide_left("abc", 0), "");
    }
}
