//! The log's stats pane: the commit under the cursor as `git log --stat`
//! shows it: IDs, author, date, the full message, then `path | N ++--`
//! per file and a summary line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::palette;
use crate::app::{App, StatPane, WORKING_ID, age};
use crate::vcs::{CommitStat, FileStat};

/// `(plus, minus)` bar lengths for a file's counts, scaled so the
/// widest count (`max` changed lines) fits `width` columns, as git does:
/// unscaled when it fits; scaled, a non-zero side keeps at least one.
/// Counts are `u64` so two `u32` sides never overflow.
pub fn bar(a: u64, r: u64, max: u64, width: usize) -> (usize, usize) {
    let w = width as u64;
    if max <= w {
        return (a as usize, r as usize);
    }
    let scale = |n: u64| {
        if n == 0 {
            0
        } else {
            (u128::from(n) * u128::from(w) / u128::from(max)).max(1) as u64
        }
    };
    let total = scale(a + r).min(w);
    let plus = scale(a).min(total);
    let minus = total - plus;
    let minus = if r > 0 { minus.max(1) } else { minus };
    let plus = if minus + plus > total {
        total - minus
    } else {
        plus
    };
    (plus as usize, minus as usize)
}

/// Days since 1970-01-01 as (year, month, day) (Howard Hinnant's
/// `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `then` as `2026-10-09 18:02 UTC (3h ago)`.
pub fn date_label(now: i64, then: i64) -> String {
    let (y, mo, d) = civil(then.div_euclid(86_400));
    let s = then.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02} UTC ({} ago)",
        s / 3600,
        s / 60 % 60,
        age(now, then)
    )
}

/// `N files changed, X insertions(+), Y deletions(-)`.
pub fn summary(files: &[FileStat]) -> String {
    let add: u64 = files.iter().filter_map(|f| f.added).map(u64::from).sum();
    let del: u64 = files.iter().filter_map(|f| f.removed).map(u64::from).sum();
    let s = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!(
        "{}, {}, {}",
        s(files.len() as u64, "file changed", "files changed"),
        s(add, "insertion(+)", "insertions(+)"),
        s(del, "deletion(-)", "deletions(-)")
    )
}

fn file_label(f: &FileStat) -> String {
    match &f.old_path {
        Some(o) => format!("{o} → {}", f.path),
        None => f.path.clone(),
    }
}

/// `s` cut from the left behind `…`, or padded, to `w` columns.
fn fit_start(s: &str, w: usize) -> String {
    let n = s.width();
    if n <= w {
        return format!("{s}{}", " ".repeat(w - n));
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.reverse();
    let body: String = out.into_iter().collect();
    format!("…{body}{}", " ".repeat(w.saturating_sub(used)))
}

/// The `path | N ++--` rows for a pane `width` columns wide.
pub fn file_lines(files: &[FileStat], width: usize) -> Vec<Line<'static>> {
    let changed =
        |f: &FileStat| u64::from(f.added.unwrap_or(0)) + u64::from(f.removed.unwrap_or(0));
    let max = files.iter().map(changed).max().unwrap_or(0);
    let num_w = files
        .iter()
        .map(|f| match f.added {
            None => 3,
            Some(_) => changed(f).to_string().len(),
        })
        .max()
        .unwrap_or(1);
    let path_w =
        files
            .iter()
            .map(|f| file_label(f).width())
            .max()
            .unwrap_or(0)
            // Leave the bar a third of the pane (or all it needs).
            .min(width.saturating_sub(
                num_w + 4 + usize::try_from(max).unwrap_or(usize::MAX).min(width / 3),
            ))
            .max(1);
    let bar_w = width.saturating_sub(path_w + 3 + num_w + 1).max(1);
    let text = Style::new().fg(palette::TEXT);
    files
        .iter()
        .map(|f| {
            let mut spans = vec![
                Span::styled(fit_start(&file_label(f), path_w), text),
                Span::styled(" | ", Style::new().fg(palette::OVERLAY)),
            ];
            match (f.added, f.removed) {
                (Some(a), Some(r)) => {
                    let (p, m) = bar(a.into(), r.into(), max, bar_w);
                    spans.push(Span::styled(format!("{:>num_w$} ", changed(f)), text));
                    spans.push(Span::styled("+".repeat(p), Style::new().fg(palette::GREEN)));
                    spans.push(Span::styled("-".repeat(m), Style::new().fg(palette::RED)));
                }
                _ => spans.push(Span::styled(
                    format!("{:>num_w$}", "Bin"),
                    Style::new().fg(palette::OVERLAY),
                )),
            }
            Line::from(spans)
        })
        .collect()
}

fn stat_lines(app: &App, s: &CommitStat, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let row = app.rows().get(app.cursor());
    let working = row.is_some_and(|r| r.info.commit == WORKING_ID);
    if working {
        lines.push(Line::styled(
            "Working copy (uncommitted)",
            Style::new().fg(palette::YELLOW),
        ));
    } else if let Some(r) = row {
        let c = &r.info.commit;
        let mut ids = vec![Span::styled(
            c.get(..12).unwrap_or(c).to_string(),
            Style::new().fg(palette::MAUVE),
        )];
        if let Some(ch) = &r.info.change {
            ids.push(Span::styled(
                format!("  change {}", ch.get(..12).unwrap_or(ch)),
                Style::new().fg(palette::PINK),
            ));
        }
        lines.push(Line::from(ids));
        lines.push(Line::from(vec![
            Span::styled(s.author.clone(), Style::new().fg(palette::BLUE)),
            Span::styled(format!(" <{}>", s.email), Style::new().fg(palette::OVERLAY)),
        ]));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        lines.push(Line::styled(
            date_label(now, s.time),
            Style::new().fg(palette::GREEN),
        ));
        lines.push(Line::raw(""));
        let msg = Style::new().fg(palette::TEXT);
        for (i, l) in s.message.lines().enumerate() {
            let st = if i == 0 {
                msg.add_modifier(Modifier::BOLD)
            } else {
                msg
            };
            lines.push(Line::styled(l.to_string(), st));
        }
    }
    lines.push(Line::raw(""));
    if s.files.is_empty() {
        lines.push(Line::styled(
            "(no files changed)",
            Style::new().fg(palette::OVERLAY),
        ));
        return lines;
    }
    lines.extend(file_lines(&s.files, width));
    lines.push(Line::styled(
        summary(&s.files),
        Style::new().fg(palette::OVERLAY),
    ));
    lines
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect, side_by_side: bool) {
    let block = Block::new()
        .borders(if side_by_side {
            Borders::LEFT
        } else {
            Borders::TOP
        })
        .border_style(Style::new().fg(palette::OVERLAY));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    let lines = match app.stat_pane() {
        None => return,
        Some(StatPane::Loading) => {
            vec![Line::styled("loading…", Style::new().fg(palette::OVERLAY))]
        }
        Some(StatPane::Error(e)) => vec![Line::styled(e, Style::new().fg(palette::RED))],
        Some(StatPane::Ready(s)) => stat_lines(app, &s, inner.width as usize),
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_scale_to_the_widest_count() {
        assert_eq!(bar(3, 2, 5, 40), (3, 2), "fits: unscaled");
        assert_eq!(bar(100, 0, 100, 10), (10, 0));
        assert_eq!(bar(50, 50, 100, 10), (5, 5));
        assert_eq!(bar(1, 0, 1000, 10), (1, 0), "non-zero keeps one");
        assert_eq!(bar(0, 1, 1000, 10), (0, 1));
        assert_eq!(bar(999, 1, 1000, 10), (9, 1));
        assert_eq!(bar(0, 0, 1000, 10), (0, 0));
    }

    #[test]
    fn huge_counts_sum_in_u64_and_scale() {
        let f = |p: &str, a, r| FileStat {
            path: p.into(),
            old_path: None,
            added: Some(a),
            removed: Some(r),
        };
        let files = vec![f("a", u32::MAX, 1), f("b", 1, u32::MAX)];
        assert_eq!(
            summary(&files),
            "2 files changed, 4294967296 insertions(+), 4294967296 deletions(-)"
        );
        let text: Vec<String> = file_lines(&files, 40)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text[0].starts_with("a | 4294967296 +"), "{text:?}");
        assert!(text[0].ends_with("+-"), "{text:?}");
        assert!(text.iter().all(|l| l.chars().count() <= 40), "{text:?}");
        let big = u64::from(u32::MAX) + 1;
        assert_eq!(bar(big, 0, big, 10), (10, 0));
    }

    #[test]
    fn dates_are_utc_with_age() {
        assert_eq!(date_label(60, 0), "1970-01-01 00:00 UTC (1m ago)");
        assert_eq!(
            date_label(1_791_581_658, 1_791_581_658 - 7200),
            "2026-10-09 19:34 UTC (2h ago)"
        );
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn summary_and_rows() {
        let f = |p: &str, o: Option<&str>, a, r| FileStat {
            path: p.into(),
            old_path: o.map(str::to_string),
            added: a,
            removed: r,
        };
        let files = vec![
            f("a.rs", None, Some(1), Some(0)),
            f("new.rs", Some("old.rs"), Some(2), Some(2)),
            f("img.png", None, None, None),
        ];
        assert_eq!(
            summary(&files),
            "3 files changed, 3 insertions(+), 2 deletions(-)"
        );
        assert_eq!(
            summary(&files[..1]),
            "1 file changed, 1 insertion(+), 0 deletions(-)"
        );
        let text: Vec<String> = file_lines(&files, 40)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            text,
            [
                "a.rs            |   1 +",
                "old.rs → new.rs |   4 ++--",
                "img.png         | Bin",
            ]
        );
    }
}
