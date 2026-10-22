//! The commit screen's header row (`[view] header`): a conventional
//! commit's `type(scope)` as a coloured badge and the subject, centred,
//! and the short ID (jj: the change ID, as in the log) and the branch on
//! the right. The subject is cut to fit, the row staying centred; with no
//! room for the ID the row is blank. The status line leaves out what the
//! header shows ([`shows`]).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::theme::Palette;

/// The badge colour of a conventional-commit type.
fn type_color(pal: &Palette, ty: &str) -> Color {
    match ty {
        "feat" => pal.green,
        "fix" => pal.red,
        "perf" => pal.orange,
        "refactor" => pal.purple,
        "docs" => pal.blue,
        "test" | "tests" => pal.cyan,
        "build" | "ci" => pal.sky,
        "revert" => pal.pink,
        "style" => pal.yellow,
        _ => pal.muted,
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

/// The right of the row in `width` columns: the short ID, then the branch
/// the log lists (dim), the branch only while the row stays centred with
/// room for some subject.
fn right_side(app: &App, id: &str, width: usize) -> (String, Option<String>) {
    let branch = app
        .branch_name()
        .map(|b| format!(" {} {} ", super::log::BRANCH_MARKER, fit(b, 24)))
        .filter(|b| {
            let w = Span::raw(b.as_str()).width() + id.chars().count() + 1;
            2 * (w + 1) + 12 <= width
        });
    let right = match branch {
        Some(_) => format!(" {id}"),
        None => format!(" {id} "),
    };
    (right, branch)
}

/// What a header row `width` columns wide shows: (the short ID, the
/// branch). `None` when the commit has no header (no subject). The status
/// line leaves out what this shows.
pub(super) fn shows(app: &App, width: u16) -> Option<(bool, bool)> {
    let (id, _) = app.commit_header()?;
    let width = width as usize;
    let (right, branch) = right_side(app, &id, width);
    let right_w = right.chars().count() + branch.as_deref().map_or(0, |b| Span::raw(b).width());
    // `draw` leaves the row blank when the ID does not fit.
    let id = right_w <= width;
    Some((id, id && branch.is_some()))
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let pal = &app.theme().palette;
    let base = Style::new().fg(pal.text).bg(pal.bg_dark);
    let Some((id, subject)) = app.commit_header() else {
        return;
    };
    let width = area.width as usize;
    let text = subject.text;
    let (right, branch) = right_side(app, &id, width);
    let branch_w = branch.as_deref().map_or(0, |b| Span::raw(b).width());
    let right_w = right.chars().count() + branch_w;
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
                    .fg(pal.bg)
                    .bg(type_color(pal, subject.ty))
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
        // Centred, kept clear of the right side.
        let pad = ((width - mid_w) / 2).min(width - mid_w - right_w);
        spans.push(Span::styled(" ".repeat(pad), base));
        spans.extend(mid);
        spans.push(Span::styled(
            " ".repeat(width - pad - mid_w - right_w),
            base,
        ));
        spans.push(Span::styled(right, base.fg(pal.purple)));
        if let Some(b) = branch {
            spans.push(Span::styled(b, base.fg(pal.muted)));
        }
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
