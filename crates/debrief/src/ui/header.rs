//! The commit screen's header row (`[view] header`): a conventional
//! commit's `type(scope)` as a coloured badge and the subject, centred,
//! and the short ID on the right. The subject is cut to fit, the row
//! staying centred; with no room for the ID the row is blank.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

use super::palette;
use crate::app::App;

/// The badge colour of a conventional-commit type.
fn type_color(ty: &str) -> Color {
    match ty {
        "feat" => palette::GREEN,
        "fix" => palette::RED,
        "perf" => palette::PEACH,
        "refactor" => palette::MAUVE,
        "docs" => palette::BLUE,
        "test" | "tests" => palette::TEAL,
        "build" | "ci" => palette::SKY,
        "revert" => palette::PINK,
        "style" => palette::YELLOW,
        _ => palette::OVERLAY,
    }
}

/// `s` cut to `max` display columns, with `…` when cut.
fn fit(s: &str, max: usize) -> String {
    let w: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if w <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > max {
            break;
        }
        used += cw;
        out.push(c);
    }
    if max > 0 {
        out.push('…');
    }
    out
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let base = Style::new().fg(palette::TEXT).bg(palette::MANTLE);
    let Some((id, subject)) = app.commit_header() else {
        return;
    };
    let width = area.width as usize;
    let text = subject.text;
    let right = format!(" {id} ");
    let right_w = right.chars().count();
    // The badge and subject are centred on the row, kept clear of the ID.
    let mut mid = Vec::new();
    let mut mid_w = 0;
    if let Some(kind) = subject.kind {
        let badge = format!(" {kind} ");
        let w = badge.chars().count();
        if w + 2 * (right_w + 1) <= width {
            mid.push(Span::styled(
                badge,
                Style::new()
                    .fg(palette::BASE)
                    .bg(type_color(subject.ty))
                    .add_modifier(Modifier::BOLD),
            ));
            mid_w += w;
        }
    }
    // Room for the text with the row centred: the ID's width on both sides.
    let room = width.saturating_sub(mid_w + 2 * (right_w + 1) + 1);
    if room > 0 {
        let t = format!(" {}", fit(text, room));
        mid_w += Span::raw(t.as_str()).width();
        mid.push(Span::styled(t, base.add_modifier(Modifier::BOLD)));
    }
    let mut spans = Vec::new();
    if mid_w + right_w <= width {
        let pad = (width - mid_w) / 2;
        spans.push(Span::styled(" ".repeat(pad), base));
        spans.extend(mid);
        spans.push(Span::styled(
            " ".repeat(width - pad - mid_w - right_w),
            base,
        ));
        spans.push(Span::styled(right, base.fg(palette::MAUVE)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fits_by_columns() {
        assert_eq!(fit("hello", 5), "hello");
        assert_eq!(fit("hello", 4), "hel…");
        assert_eq!(fit("日本語", 4), "日…");
        assert_eq!(fit("abc", 0), "");
    }
}
