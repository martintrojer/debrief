//! The commit list under the file list's tree (`[view] sidebar_commits`,
//! `<leader>E`): the log's rows (filtered, git's working-copy row
//! included), with a cursor of its own. Moving the cursor opens nothing
//! and marks nothing seen; Enter opens the row under it the way `]]`
//! does ([`App::open_index`]) and the list keeps the keys, an auto peek
//! staying open while you browse.

use super::commit::Focus;
use super::{App, Screen};

/// Fewest rows the file-list area needs for the commit list; below it
/// the tree takes the whole area.
pub const MIN_ROWS: u16 = 12;

/// The tree's and the list's heights in a file-list area `h` rows tall:
/// the tree 60% of the rows below the rule, the list the rest with the
/// rule on top. `None` under [`MIN_ROWS`].
pub fn split_rows(h: u16) -> Option<(u16, u16)> {
    if h < MIN_ROWS {
        return None;
    }
    let tree = (h - 1) * 3 / 5;
    Some((tree, h - tree))
}

/// The commit list's runtime state, owned by [`App`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CommitListState {
    /// Shown under the tree: `[view] sidebar_commits`, flipped by
    /// `<leader>E`.
    pub(super) on: bool,
    /// The list's cursor, a log row.
    pub(super) cursor: usize,
    /// The first log row drawn.
    pub(super) scroll: usize,
    /// A commit is being opened from the list (it keeps the keys).
    pub(super) opening: bool,
}

impl App {
    /// The commit list is on (`[view] sidebar_commits`, `<leader>E`).
    pub fn sidebar_commits(&self) -> bool {
        self.commit_list.on
    }

    /// The list is on and the terminal is tall enough for it: the file
    /// list's area shows it once the sidebar is shown.
    pub(super) fn commit_list_fits(&self) -> bool {
        self.commit_list.on && split_rows(self.size.1.saturating_sub(1)).is_some()
    }

    /// The commit list is drawn under the tree.
    pub fn commit_list_visible(&self) -> bool {
        self.commit_list_fits() && self.sidebar_visible()
    }

    /// Keys go to the commit list.
    pub(super) fn commits_focused(&self) -> bool {
        self.focus == Focus::Commits && self.commit_list_visible()
    }

    /// The list's cursor (a log row).
    pub fn commit_list_cursor(&self) -> usize {
        self.commit_list.cursor
    }

    /// The first log row the list draws.
    pub fn commit_list_offset(&self) -> usize {
        self.commit_list.scroll
    }

    /// The log row of the open commit, by its commit (or the working
    /// copy); `None` when it has left the log.
    pub fn commit_list_open_row(&self) -> Option<usize> {
        if !matches!(self.screen, Screen::Commit(_)) {
            return None;
        }
        self.commit.as_ref().and_then(|v| self.view_index(v))
    }

    /// Rows the list shows (under its rule), at least 1.
    fn commit_list_height(&self) -> usize {
        split_rows(self.size.1.saturating_sub(1)).map_or(1, |(_, list)| (list as usize - 1).max(1))
    }

    /// Rows and the `… more` row past the last screenful.
    fn commit_list_max_scroll(&self) -> usize {
        self.log_len().saturating_sub(self.commit_list_height())
    }

    /// Scroll so the cursor (clamped to the rows) is on screen (on the
    /// last row, the `… more` row under it too).
    pub(super) fn commit_list_keep_visible(&mut self) {
        let h = self.commit_list_height();
        let last = self.last_row();
        let max = self.commit_list_max_scroll();
        self.commit_list.cursor = self.commit_list.cursor.min(last);
        let bottom = self
            .bottom_of(self.commit_list.cursor)
            .min(self.commit_list.cursor + h - 1);
        let l = &mut self.commit_list;
        if l.cursor < l.scroll {
            l.scroll = l.cursor;
        } else if bottom >= l.scroll + h {
            l.scroll = bottom + 1 - h;
        }
        l.scroll = l.scroll.min(max);
    }

    /// A commit opened at log row `i`: the cursor goes there, centred.
    pub(super) fn commit_list_centre(&mut self, i: usize) {
        let h = self.commit_list_height();
        let max = self.commit_list_max_scroll();
        self.commit_list.cursor = i;
        self.commit_list.scroll = i.saturating_sub((h - 1) / 2).min(max);
    }

    /// The cursor to log row `row` (clamped), kept on screen.
    pub(super) fn commit_list_goto(&mut self, row: usize) {
        self.commit_list.cursor = row.min(self.last_row());
        self.commit_list_keep_visible();
    }

    /// `C-d` / `C-u` in the list: half its height.
    pub(super) fn commit_list_half(&self) -> isize {
        (self.commit_list_height() / 2).max(1) as isize
    }

    /// Move the list's cursor to log row `i` and give the list the keys
    /// (an auto list peeks); nothing opens.
    pub fn commit_list_select(&mut self, i: usize) {
        self.commit_list.cursor = i.min(self.last_row());
        self.focus_pane(Focus::Commits);
        self.commit_list_keep_visible();
    }

    /// Enter in the list: open the row under the cursor as `]]` would
    /// (seen marking, session restore); the list keeps the keys and an
    /// auto peek stays open.
    pub fn commit_list_open(&mut self) {
        if self.rows.get(self.commit_list.cursor).is_none() {
            return;
        }
        self.commit_list.opening = true;
        self.open_index(self.commit_list.cursor);
        self.commit_list.opening = false;
        if matches!(self.screen, Screen::Commit(_)) && self.focus != Focus::Commits {
            self.focus_pane(Focus::Commits);
        }
        self.commit_follow();
    }

    /// Scroll the list's view `delta` rows (down when positive); the
    /// cursor stays on screen. Down with the view at its end: the next
    /// page.
    pub fn commit_list_scroll(&mut self, delta: isize) {
        let h = self.commit_list_height();
        let max = self.commit_list_max_scroll();
        if delta > 0 && self.commit_list.scroll >= max {
            return self.page_wheel();
        }
        let last = self.last_row();
        let l = &mut self.commit_list;
        l.scroll = l.scroll.saturating_add_signed(delta).min(max);
        l.cursor = l.cursor.clamp(l.scroll, l.scroll + h - 1).min(last);
    }

    /// `<leader>E`: show or hide the commit list for this run.
    pub(super) fn toggle_commits(&mut self) {
        self.commit_list.on = !self.commit_list.on;
        self.commit_list_refocus();
    }

    /// The list went away (toggled off, too short, or the whole sidebar,
    /// an empty commit's): a hold it kept ends, and keys it or a
    /// now-hidden tree had go to the tree if shown, else the diff.
    pub(super) fn commit_list_refocus(&mut self) {
        if self.commit_list_visible() {
            return;
        }
        self.sidebar.held = false;
        let lost = match self.focus {
            Focus::Commits => true,
            Focus::Files => !self.sidebar_visible(),
            Focus::Diff => false,
        };
        if lost {
            let tree = self.sidebar_visible() && self.tree_has_files();
            self.set_focus(if tree { Focus::Files } else { Focus::Diff });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_gets_60_percent_below_the_rule() {
        assert_eq!(split_rows(11), None);
        assert_eq!(split_rows(12), Some((6, 6)));
        assert_eq!(split_rows(29), Some((16, 13)));
        assert_eq!(split_rows(101), Some((60, 41)));
    }
}
