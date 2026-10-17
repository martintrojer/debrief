//! The status line: repo root, VCS kind, the new count, a status message
//! (`loading more…` while a page of the log loads), the `g?` hint and the
//! cursor position; while a prompt is open, the prompt and its note.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::palette;
use crate::app::{App, Mode, Screen};
use crate::vcs::Kind;

/// The key that opens the help overlay, shown dimmed while it fits.
pub const HELP_HINT: &str = "g? help";

/// `s` in at most `max` columns, cut from the left behind `…`.
pub(super) fn elide_left(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut keep: Vec<char> = Vec::new();
    let mut used = 1;
    for c in s.chars().rev() {
        used += c.width().unwrap_or(0);
        if used > max {
            break;
        }
        keep.push(c);
    }
    format!("…{}", keep.iter().rev().collect::<String>())
}

/// `s` in at most `max` columns, cut from the right before `…`.
fn elide_right(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut keep = String::new();
    let mut used = 1;
    for c in s.chars() {
        used += c.width().unwrap_or(0);
        if used > max {
            break;
        }
        keep.push(c);
    }
    format!("{keep}…")
}

/// The log's path filter and kept text filter in `room` columns:
/// `  path: src/app` and `  filter: x (3/41)`. The text filter is cut
/// first (from the right), down to what the path leaves; the path keeps
/// at least 12 columns of the text filter's room and is cut from the
/// left.
fn filter_spans(app: &App, base: Style, room: usize) -> Vec<Span<'static>> {
    const LEAD: &str = "  path: ";
    let filter = app.filter_label();
    let mut spans = Vec::new();
    let mut room = room;
    if let Some(p) = app.path_filter() {
        let text = filter.as_ref().map_or(0, |f| f.width() + 2);
        let fit = room.saturating_sub(text).max(room.min(LEAD.len() + 12));
        if fit > LEAD.len() {
            let label = format!("{LEAD}{}", elide_left(p, fit - LEAD.len()));
            room = room.saturating_sub(label.width());
            spans.push(Span::styled(label, base.fg(palette::PEACH)));
        }
    }
    if let Some(f) = filter {
        let room = room.saturating_sub(2);
        if room > 0 {
            spans.push(Span::styled(
                format!("  {}", elide_right(&f, room)),
                base.fg(palette::SKY),
            ));
        }
    }
    spans
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let base = Style::new().fg(palette::TEXT).bg(palette::MANTLE);
    // The quit prompt draws over a search or filter prompt it interrupted.
    if app.mode() == Mode::QuitPrompt {
        let p = format!(" {}", crate::app::session::QUIT_PROMPT);
        frame.render_widget(Paragraph::new(p).style(base.fg(palette::YELLOW)), area);
        return;
    }
    if let Some(p) = app.search_prompt() {
        let x = area.x + Span::raw(p.as_str()).width() as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
        let mut spans = vec![Span::raw(p)];
        if let Some(note) = app.log_prompt_note() {
            spans.push(Span::styled(format!("  {note}"), base.fg(palette::YELLOW)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
        return;
    }
    if let Screen::Commit(_) = app.screen() {
        return draw_commit(frame, app, area, base);
    }
    let kind = match app.kind() {
        Kind::Git => "git",
        Kind::Jj => "jj",
    };
    let (new, updated) = app.counts();
    let mut counts = format!("{new} new");
    if updated > 0 {
        counts.push_str(&format!(", {updated} updated"));
    }
    if app.working_copy().is_some() {
        counts.push_str(", working copy dirty");
    }
    let right = match app.rows().len() {
        0 => " 0/0 ".to_string(),
        n => format!(" {}/{n} ", app.cursor() + 1),
    };
    let mut msg = String::new();
    if app.loading_more() {
        msg.push_str("  loading more…");
    }
    if !app.status().is_empty() {
        msg.push_str(&format!("  {}", app.status()));
    }
    // The root is elided first, so the message shows whole while it
    // fits; the filters get what the rest leaves (see `filter_spans`).
    let tail = [format!("  {kind}"), format!("  {counts}")];
    let fixed: usize =
        1 + tail.iter().map(|t| t.width()).sum::<usize>() + msg.width() + right.len();
    let filters = filter_spans(app, base, (area.width as usize).saturating_sub(fixed));
    let filters_w: usize = filters.iter().map(Span::width).sum();
    let root = elide_left(
        &app.root_label(),
        (area.width as usize).saturating_sub(fixed + filters_w),
    );
    let [kind, counts] = tail;
    let mut spans = vec![
        Span::styled(format!(" {root}"), base.fg(palette::BLUE)),
        Span::styled(kind, base.fg(palette::OVERLAY)),
    ];
    // `path: …` before the counts, the text filter after them.
    let mut filters = filters.into_iter().peekable();
    if app.path_filter().is_some()
        && let Some(p) = filters.next_if(|s| s.content.starts_with("  path: "))
    {
        spans.push(p);
    }
    spans.push(Span::styled(counts, base.fg(palette::SKY)));
    spans.extend(filters);
    if !msg.is_empty() {
        spans.push(Span::styled(msg, base.fg(palette::YELLOW)));
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

/// The commit screen: mode, commit ID, the log's path filter and kept
/// text filter, loading/status message, position.
fn draw_commit(frame: &mut Frame, app: &App, area: Rect, base: Style) {
    let id = app.commit_id().unwrap_or("");
    let id = if app.commit_id() == Some(crate::app::WORKING_ID) {
        "working copy"
    } else {
        id.get(..8).unwrap_or(id)
    };
    let mut spans = Vec::new();
    if app.mode() == Mode::Visual {
        spans.push(Span::styled(
            " V-LINE ",
            base.fg(palette::BASE).bg(palette::MAUVE),
        ));
    }
    spans.push(Span::styled(format!(" {id}"), base.fg(palette::MAUVE)));
    let preview = app.review_preview();
    let msg = if app.commit_loading() {
        "loading…"
    } else if app.loading_more() {
        "loading more…"
    } else if !app.status().is_empty() {
        app.status()
    } else {
        preview.as_deref().unwrap_or("")
    };
    let n = app.commit_rows().len();
    let right = match n {
        0 => " ".to_string(),
        n => format!(" {}/{n} ", app.commit_cursor().row + 1),
    };
    // The log's filters narrow the commit list and `]]` / `[[` here too;
    // they get what the ID, the message and the position leave.
    let used: usize = spans.iter().map(Span::width).sum::<usize>()
        + if msg.is_empty() { 0 } else { 2 + msg.width() }
        + right.width();
    spans.extend(filter_spans(
        app,
        base,
        (area.width as usize).saturating_sub(used),
    ));
    if !msg.is_empty() {
        spans.push(Span::styled(format!("  {msg}"), base.fg(palette::YELLOW)));
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
        // Columns, not characters: each 界 takes two.
        assert_eq!(elide_left("界界界界", 5), "…界界");
        assert_eq!(super::elide_right("界界界界", 5), "界界…");
    }
}
