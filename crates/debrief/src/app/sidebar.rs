//! The commit view's file-list sidebar: its auto width (ramble's
//! sidebar_width.rs), visibility, focus and the jumps it makes.

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::commit::{Focus, file_counts, file_path};
use super::{App, Screen};

/// Narrowest and widest sidebar, in columns (not counting its border).
pub const SIDEBAR_MIN: u16 = 16;
pub const SIDEBAR_MAX: u16 = 48;
/// Below this terminal width the sidebar is hidden.
pub const SIDEBAR_HIDE_BELOW: u16 = 80;
/// The current-file marker gutter at the left of each row.
pub const GUTTER_COLS: usize = 1;
/// The status letter and its space after the gutter.
pub const STATUS_COLS: usize = 2;

/// Where the commit screen's panes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitLayout {
    /// The file list, border included (on its right edge).
    pub sidebar: Option<Rect>,
    pub diff: Rect,
}

/// The sidebar width for rows that need `need` columns each, on a
/// terminal `cols` wide: the 90th percentile of `need` (the maximum under
/// 10 rows) plus one column of padding, clamped to `[min, min(max, 35% of
/// cols)]`.
pub fn sidebar_width(need: &[u16], cols: u16, min: u16, max: u16) -> u16 {
    let mut sorted = need.to_vec();
    sorted.sort_unstable();
    let fit = match sorted.len() {
        0 => 0,
        n if n < 10 => sorted[n - 1],
        n => sorted[(n * 9).div_ceil(10) - 1],
    };
    let pct = (u32::from(cols) * 35 / 100) as u16;
    fit.saturating_add(1).min(pct.min(max)).max(min)
}

/// A file row's count column: ` +n −m`.
pub fn counts_label(add: usize, del: usize) -> String {
    format!(" +{add} −{del}")
}

impl App {
    /// `<leader>e`: show or hide the file list.
    pub(super) fn toggle_sidebar(&mut self) {
        self.sidebar_on = !self.sidebar_on;
        if !self.sidebar_on {
            self.focus = Focus::Diff;
        }
    }

    /// The file list is drawn: switched on, a loaded commit with files,
    /// and a terminal at least 80 columns wide.
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_on
            && self.size.0 >= SIDEBAR_HIDE_BELOW
            && matches!(self.screen, Screen::Commit(_))
            && self.commit.as_ref().is_some_and(|v| v.file_count() > 0)
    }

    /// Keys go to the file list.
    pub(super) fn files_focused(&self) -> bool {
        self.focus == Focus::Files && self.sidebar_visible()
    }

    /// `C-w h` / `C-w l`.
    pub(super) fn focus_pane(&mut self, f: Focus) {
        if f == Focus::Files && !self.sidebar_visible() {
            self.set_status("no file list");
            return;
        }
        self.focus = f;
        if let Some(v) = &mut self.commit
            && f == Focus::Files
            && let Some(cur) = v.file_at(v.cur.row)
        {
            v.side_sel = cur;
        }
    }

    /// The selected row of the file list.
    pub fn sidebar_selected(&self) -> usize {
        self.commit.as_ref().map_or(0, |v| v.side_sel)
    }

    /// Select file `i` in the list and jump the diff to its header.
    pub(super) fn select_file(&mut self, i: usize) {
        let Some(v) = &mut self.commit else { return };
        let n = v.file_count();
        if n == 0 {
            return;
        }
        v.side_sel = i.min(n - 1);
        if let Some(r) = v.header_row(v.side_sel) {
            v.goto_row(r);
            v.cursor_to_top();
            v.keep_visible();
        }
    }

    /// Columns each file row needs: gutter, status letter, path and counts.
    fn sidebar_needs(&self) -> Vec<u16> {
        let Some(d) = self.commit_diff() else {
            return Vec::new();
        };
        d.files
            .iter()
            .map(|f| {
                let (a, r) = file_counts(f);
                let n =
                    GUTTER_COLS + STATUS_COLS + file_path(f).width() + counts_label(a, r).width();
                n.min(u16::MAX as usize) as u16
            })
            .collect()
    }

    /// Split the main area (above the status line) into sidebar and diff.
    pub fn commit_layout(&self, area: Rect) -> CommitLayout {
        if !self.sidebar_visible() {
            return CommitLayout {
                sidebar: None,
                diff: area,
            };
        }
        let w = sidebar_width(&self.sidebar_needs(), area.width, SIDEBAR_MIN, SIDEBAR_MAX);
        let side = Rect::new(area.x, area.y, (w + 1).min(area.width), area.height);
        let diff = Rect::new(
            side.right(),
            area.y,
            area.width.saturating_sub(side.width),
            area.height,
        );
        CommitLayout {
            sidebar: Some(side),
            diff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_width() {
        assert_eq!(sidebar_width(&[7, 5, 9], 120, 16, 48), 16, "min");
        assert_eq!(sidebar_width(&[10, 30, 12], 120, 16, 48), 31, "max of few");
        assert_eq!(sidebar_width(&[60], 100, 16, 48), 35, "35%");
        assert_eq!(sidebar_width(&[60], 400, 16, 48), 48, "max");
        let mut need = vec![20; 30];
        need.push(120);
        assert_eq!(sidebar_width(&need, 120, 16, 48), 21, "p90");
    }
}
