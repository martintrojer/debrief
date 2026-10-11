//! The commit view's file-list sidebar: its auto width (ramble's
//! sidebar_width.rs), visibility, focus and the jumps it makes.

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use debrief_review::Side;

use super::commit::Focus;
use super::config::{SidebarShow, SidebarSide, SidebarWidth};
use super::pane_nav::Dir;
use super::{App, Screen};

/// The narrowest unified diff pane an auto peek leaves beside it; with
/// less room the peek draws over the diff. Side by side the minimum is
/// `[view] split_min_width`.
pub const DIFF_MIN_WIDTH: u16 = 80;
/// The current-file marker gutter at the left of each row.
pub const GUTTER_COLS: usize = 1;
/// The status letter and its space after the gutter.
pub const STATUS_COLS: usize = 2;

/// Where the commit screen's panes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitLayout {
    /// The file list's tree, border included (on its edge toward the
    /// diff); the top of the file-list area when the commit list shows.
    pub sidebar: Option<Rect>,
    /// The commit list under the tree, its rule (the top row) and border
    /// included.
    pub commits: Option<Rect>,
    pub diff: Rect,
    /// The file list is drawn over the diff (an auto peek without room
    /// beside it); `diff` is then the whole area.
    pub overlay: bool,
}

/// The file list's runtime state, owned by [`App`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct SidebarState {
    /// always / never / auto: `[view] sidebar`, flipped by `<leader>e`
    /// (always / never) and set by a restored session.
    pub(super) show: SidebarShow,
    /// auto: `<leader>e` pinned the list (shown until toggled).
    pub(super) pinned: bool,
    /// auto: the peek is open.
    pub(super) peeked: bool,
    /// The open peek draws over the diff; decided when it opens and on a
    /// resize.
    pub(super) overlay: bool,
    /// A `<leader>e` choice made below `sidebar_auto_hide_below`; it
    /// holds until the terminal width changes.
    pub(super) narrow_override: Option<bool>,
    /// The auto width of the last file list shown, kept while the next
    /// commit loads (or has no files) under the commit list.
    pub(super) last_auto: Option<u16>,
}

/// A pane of the commit screen, left to right on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Files,
    /// The unified diff.
    Diff,
    /// Side by side: the old (left) or new (right) half.
    Half(Side),
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
    /// The terminal is narrower than `[view] sidebar_auto_hide_below`.
    fn sidebar_narrow(&self) -> bool {
        let below = self.config.view.sidebar_auto_hide_below;
        below > 0 && self.size.0 < below
    }

    /// A loaded commit with files is on screen, or the commit list fits:
    /// there is a list to show (with the commit list, the tree may be
    /// empty while a commit loads or when it has no files).
    fn sidebar_has_files(&self) -> bool {
        matches!(self.screen, Screen::Commit(_))
            && (self.tree_has_files() || self.commit_list_fits())
    }

    /// The open commit has files for the tree.
    pub(super) fn tree_has_files(&self) -> bool {
        self.commit.as_ref().is_some_and(|v| v.file_count() > 0)
    }

    /// `[view] sidebar` as the session has it now (`<leader>e` flips
    /// always / never; a pinned auto list counts as always).
    pub fn sidebar_show(&self) -> SidebarShow {
        match self.sidebar.show {
            SidebarShow::Auto if self.sidebar.pinned => SidebarShow::Always,
            s => s,
        }
    }

    /// Put back a saved `sidebar` (a session): it wins over `[view]
    /// sidebar` and [`App::sidebar_show`] reports it unchanged. While auto,
    /// a saved always pins the list (the pin [`App::sidebar_show`] saves
    /// as always); otherwise the saved value is the mode. No peek is open
    /// after.
    pub(super) fn set_sidebar_show(&mut self, show: SidebarShow) {
        match (self.sidebar.show, show) {
            (SidebarShow::Auto, SidebarShow::Always) => self.sidebar.pinned = true,
            (_, s) => {
                self.sidebar.show = s;
                self.sidebar.pinned = false;
            }
        }
        self.sidebar.peeked = false;
        self.sidebar.narrow_override = None;
    }

    /// `sidebar = "auto"`: the list is hidden and a move into it would
    /// open a peek.
    pub fn peek_could_open(&self) -> bool {
        self.sidebar.show == SidebarShow::Auto
            && !self.sidebar.pinned
            && !self.sidebar.peeked
            && self.sidebar_has_files()
    }

    /// The file list is shown, or a move into it would peek.
    pub(super) fn files_reachable(&self) -> bool {
        self.sidebar_visible() || self.peek_could_open()
    }

    /// `<leader>e`: show or hide the file list. always / never flip (a
    /// choice made below `sidebar_auto_hide_below` holds until the width
    /// changes); auto pins a hidden list and hides a shown one (a peek or
    /// a pin), back to peeking.
    pub(super) fn toggle_sidebar(&mut self) {
        let shown = self.sidebar_visible();
        match self.sidebar.show {
            SidebarShow::Auto => {
                self.sidebar.pinned = !shown;
                self.sidebar.peeked = false;
            }
            _ => {
                self.sidebar.show = if shown {
                    SidebarShow::Never
                } else {
                    SidebarShow::Always
                };
                if self.sidebar_narrow() {
                    self.sidebar.narrow_override = Some(!shown);
                }
            }
        }
        if !self.sidebar_visible() {
            self.set_focus(Focus::Diff);
        }
    }

    /// The file list is drawn: a loaded commit with files, and then by
    /// `[view] sidebar`: always (unless the terminal is narrower than
    /// `sidebar_auto_hide_below`, or `<leader>e` chose otherwise while it
    /// was), never (unless `<leader>e` showed it), auto (pinned or
    /// peeking).
    pub fn sidebar_visible(&self) -> bool {
        if !self.sidebar_has_files() {
            return false;
        }
        match self.sidebar.show {
            SidebarShow::Auto => self.sidebar.pinned || self.sidebar.peeked,
            show => match self.sidebar.narrow_override {
                Some(v) => v,
                None => show == SidebarShow::Always && !self.sidebar_narrow(),
            },
        }
    }

    /// The file list is drawn over the diff instead of beside it: auto,
    /// shown, and a narrow terminal or a peek without room beside.
    pub fn sidebar_overlay(&self) -> bool {
        self.sidebar.show == SidebarShow::Auto
            && self.sidebar_visible()
            && (self.sidebar_narrow() || (self.sidebar.peeked && self.sidebar.overlay))
    }

    /// The terminal width changed: a narrow `<leader>e` choice ends and an
    /// open peek decides again between beside and over the diff.
    pub(super) fn sidebar_resized(&mut self, cols: u16) {
        if cols != self.size.0 {
            self.sidebar.narrow_override = None;
        }
        self.size.0 = cols;
        self.sidebar_refit();
    }

    /// The file list's width may have changed (` or a directory opened
    /// or closed): an open peek decides again between beside and over
    /// the diff, as on a resize.
    pub(super) fn sidebar_refit(&mut self) {
        if self.sidebar.peeked {
            self.sidebar.overlay = !self.peek_fits_beside();
        }
    }

    /// An auto peek goes beside the diff when the diff keeps its minimum
    /// width next to it, so it doesn't reflow: `[view] split_min_width`
    /// when the wanted layout is side by side and the whole width shows
    /// it (the layout without the peek, not the one on screen, which a
    /// resize hasn't laid out yet), else [`DIFF_MIN_WIDTH`].
    fn peek_fits_beside(&self) -> bool {
        let cols = self.size.0;
        let split_min = self.config.view.split_min_width;
        let min = if self.split_wanted() && cols >= split_min {
            split_min
        } else {
            DIFF_MIN_WIDTH
        };
        let side = self.sidebar_cols(cols) + 1;
        cols.saturating_sub(side) >= min
    }

    /// Open an auto peek (focus is the caller's).
    pub(super) fn peek_open(&mut self) {
        if self.peek_could_open() {
            self.sidebar.peeked = true;
            self.sidebar.overlay = !self.peek_fits_beside();
        }
    }

    /// Give `f` the keys; leaving the list closes an auto peek.
    pub(super) fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        if f == Focus::Diff {
            self.sidebar.peeked = false;
        }
    }

    /// Keys go to the file list.
    pub(super) fn files_focused(&self) -> bool {
        self.focus == Focus::Files && self.sidebar_visible()
    }

    /// Focus the file list's tree or commit list (opening an auto peek),
    /// or the diff. An empty tree sends the keys on to the commit list.
    pub(super) fn focus_pane(&mut self, f: Focus) {
        let mut f = f;
        if matches!(f, Focus::Files | Focus::Commits) {
            self.peek_open();
            if !self.sidebar_visible() {
                self.set_status("no file list");
                return;
            }
            if f == Focus::Files && !self.tree_has_files() && self.commit_list_visible() {
                f = Focus::Commits;
            }
            if f == Focus::Commits && !self.commit_list_visible() {
                self.set_status("no commit list");
                return;
            }
        }
        self.set_focus(f);
        if let Some(v) = &mut self.commit
            && f == Focus::Files
            && let Some(cur) = v.file_at(v.cur.row).and_then(|f| v.entry_of(f))
        {
            v.side_sel = cur;
        }
    }

    /// The commit screen's panes left to right: the file list (when shown
    /// or a peek could open) on its side, and the diff, or its old and
    /// new halves side by side. Empty on the log.
    pub fn panes(&self) -> Vec<Pane> {
        if !matches!(self.screen, Screen::Commit(_)) {
            return Vec::new();
        }
        let mut out = if self.commit_split() {
            vec![Pane::Half(Side::Old), Pane::Half(Side::New)]
        } else {
            vec![Pane::Diff]
        };
        if self.files_reachable() {
            match self.config.view.sidebar_side {
                SidebarSide::Left => out.insert(0, Pane::Files),
                SidebarSide::Right => out.push(Pane::Files),
            }
        }
        out
    }

    /// The pane with the keys.
    pub fn current_pane(&self) -> Option<Pane> {
        if !matches!(self.screen, Screen::Commit(_)) {
            return None;
        }
        Some(if self.files_focused() || self.commits_focused() {
            Pane::Files
        } else {
            match self.split_side() {
                Some(s) => Pane::Half(s),
                None => Pane::Diff,
            }
        })
    }

    /// Move to the pane toward `dir`; false when there is none (the
    /// screen edge, or up / down outside the file list's two halves).
    pub(super) fn pane_move(&mut self, dir: Dir) -> bool {
        let step: isize = match dir {
            Dir::Left => -1,
            Dir::Right => 1,
            Dir::Down if self.files_focused() && self.commit_list_visible() => {
                self.set_focus(Focus::Commits);
                return true;
            }
            Dir::Up if self.commits_focused() && self.tree_has_files() => {
                self.set_focus(Focus::Files);
                return true;
            }
            Dir::Up | Dir::Down => return false,
        };
        let panes = self.panes();
        let Some(cur) = self.current_pane() else {
            return false;
        };
        let Some(i) = panes.iter().position(|p| *p == cur) else {
            return false;
        };
        let Some(&to) = i.checked_add_signed(step).and_then(|j| panes.get(j)) else {
            return false;
        };
        match to {
            Pane::Files => self.focus_pane(Focus::Files),
            Pane::Diff => self.set_focus(Focus::Diff),
            Pane::Half(s) => {
                self.set_focus(Focus::Diff);
                if self.split_side() != Some(s) {
                    self.switch_side();
                }
            }
        }
        true
    }

    /// `C-w h` / `C-w l`: [`App::pane_move`]; at the edge toward a hidden
    /// list, say so.
    pub(super) fn pane_move_or_say(&mut self, dir: Dir) {
        if self.pane_move(dir) {
            return;
        }
        let toward_list = matches!(
            (dir, self.config.view.sidebar_side),
            (Dir::Left, SidebarSide::Left) | (Dir::Right, SidebarSide::Right)
        );
        if toward_list && !self.files_focused() && !self.commits_focused() {
            self.set_status("no file list");
        }
    }

    /// The selected row of the file list (its visible rows, top to
    /// bottom).
    pub fn sidebar_selected(&self) -> usize {
        self.file_list().map_or(0, |l| l.selected)
    }

    /// Columns each visible row needs: gutter, status letter or folder
    /// arrow, indent, name and counts.
    fn sidebar_needs(&self) -> Vec<u16> {
        let Some(l) = self.file_list() else {
            return Vec::new();
        };
        l.visible
            .rows
            .iter()
            .map(|&i| {
                let e = &l.entries[i];
                let n = GUTTER_COLS
                    + 2 * e.depth
                    + STATUS_COLS
                    + e.name.width()
                    + counts_label(e.add, e.del).width();
                n.min(u16::MAX as usize) as u16
            })
            .collect()
    }

    /// The list's width on a terminal `cols` wide, not counting its
    /// border: the auto fit or `[view] sidebar_width`, at most `cols - 1`.
    fn sidebar_cols(&self, cols: u16) -> u16 {
        let v = &self.config.view;
        let w = match v.sidebar_width {
            SidebarWidth::Auto => match self.sidebar_needs() {
                need if need.is_empty() && self.commit_list_fits() => {
                    self.sidebar.last_auto.unwrap_or(v.sidebar_min_width)
                }
                need => sidebar_width(&need, cols, v.sidebar_min_width, v.sidebar_max_width),
            },
            SidebarWidth::Fixed(n) => n,
        };
        w.min(cols.saturating_sub(1))
    }

    /// Remember the auto width of a file list with rows, for the commits
    /// that follow it under the commit list.
    pub(super) fn sidebar_remember_width(&mut self) {
        if !self.sidebar_needs().is_empty() {
            self.sidebar.last_auto = Some(self.sidebar_cols(self.size.0));
        }
    }

    /// Split the main area (above the status line) into the file list
    /// (on `[view] sidebar_side`) and the diff; an overlay leaves the
    /// diff the whole area.
    pub fn commit_layout(&self, area: Rect) -> CommitLayout {
        if !self.sidebar_visible() {
            return CommitLayout {
                sidebar: None,
                commits: None,
                diff: area,
                overlay: false,
            };
        }
        let w = (self.sidebar_cols(area.width) + 1).min(area.width);
        let rest = area.width - w;
        let overlay = self.sidebar_overlay();
        let (side, diff) = match self.config.view.sidebar_side {
            SidebarSide::Left => (
                Rect::new(area.x, area.y, w, area.height),
                Rect::new(area.x + w, area.y, rest, area.height),
            ),
            SidebarSide::Right => (
                Rect::new(area.x + rest, area.y, w, area.height),
                Rect::new(area.x, area.y, rest, area.height),
            ),
        };
        let (tree, commits) = match super::commit_list::split_rows(side.height) {
            Some((t, l)) if self.commit_list_fits() => (
                Rect { height: t, ..side },
                Some(Rect::new(side.x, side.y + t, side.width, l)),
            ),
            _ => (side, None),
        };
        CommitLayout {
            sidebar: Some(tree),
            commits,
            diff: if overlay { area } else { diff },
            overlay,
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
