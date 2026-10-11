//! The mouse (`[mouse] enabled`), ramble's mouse.rs in debrief's terms:
//! click to focus a pane and put its cursor or selection there, double
//! click to open, drag in the diff to select lines (scrolling at the
//! edges), the wheel to scroll the pane under the pointer. Clicks outside
//! an open picker, help or `K` popup close it. While typing (the comment
//! box, a search or filter prompt, help's filter) the mouse does nothing.
//!
//! [`App::hit`] finds what is under a cell from the rects the draw uses,
//! top-most first, so a click lands where the screen shows. Effects reuse
//! the key actions.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use debrief_review::Side;
use ratatui::layout::{Position, Rect};

use super::commit::{Focus, gutter_width, side_gutter_width};
use super::config::SidebarSide;
use super::help::HelpAction;
use super::picker::PickerAction;
use super::review::PopupMove;
use super::run::capture_wanted;
use super::sidebar::lead_col;
use super::split::spans_both;
use super::{App, Mode, Screen, help_rect};

/// Two left presses on the same cell this close together are a double
/// click.
pub const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Rows one wheel notch scrolls a pane (the picker moves one item).
pub const WHEEL_ROWS: usize = 3;

/// What is under a screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    None,
    /// The status line.
    Status,
    /// The key clue box.
    Clue,
    /// Log row `.0` (scroll added).
    Log(usize),
    /// The log pane below its last row.
    LogPane,
    /// The log's stats pane.
    Stats,
    /// Visible row `row` of the file list; `x` is the column inside the
    /// list (border excluded).
    Files {
        row: usize,
        x: u16,
    },
    /// The file list, not on a row (its border, below its rows).
    FilesPane,
    /// Log row `.0` in the commit list.
    Commits(usize),
    /// The commit list, not on a row (its rule, border, below its rows).
    CommitsPane,
    /// The diff: row `row` (scroll added; may be past the last), text
    /// column `col` (horizontal scroll added; the gutter is column 0 of
    /// the text) and, side by side, the half (the cursor's on a row drawn
    /// across both).
    Diff {
        row: usize,
        col: usize,
        side: Option<Side>,
    },
    /// The comment box.
    Comment,
    /// The `K` popup.
    Popup,
    /// Inside the picker box, not on an item.
    Picker,
    /// Item `.0` of the picker's filtered list.
    PickerItem(usize),
    /// The help overlay.
    Help,
}

/// A left press in the diff that a drag turns into a line selection.
#[derive(Debug, Clone, Copy)]
struct Drag {
    anchor: usize,
    moved: bool,
}

/// Mouse state owned by [`App`].
#[derive(Debug, Clone, Default)]
pub(super) struct MouseState {
    /// The last left press: cell, time and click count (1 or 2).
    last_press: Option<(u16, u16, Instant, u8)>,
    drag: Option<Drag>,
}

/// The rect inside a file-list rect's border (on its edge toward the
/// diff).
fn list_inner(r: Rect, side: SidebarSide) -> Rect {
    let w = r.width.saturating_sub(1);
    match side {
        SidebarSide::Left => Rect { width: w, ..r },
        SidebarSide::Right => Rect {
            x: r.x + 1.min(r.width),
            width: w,
            ..r
        },
    }
}

impl MouseState {
    /// The diff row a drag in progress was pressed on.
    pub(super) fn drag_anchor(&self) -> Option<usize> {
        self.drag.map(|d| d.anchor)
    }

    /// The pressed row after a relayout moved its line.
    pub(super) fn set_drag_anchor(&mut self, row: usize) {
        if let Some(d) = &mut self.drag {
            d.anchor = row;
        }
    }
}

impl App {
    /// The area above the status line.
    fn main_area(&self) -> Rect {
        Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1))
    }

    /// The file list's tree inside its border, as drawn.
    fn tree_inner(&self) -> Option<Rect> {
        let side = self.config.view.sidebar_side;
        let l = self.commit_layout(self.main_area());
        l.sidebar.map(|r| list_inner(r, side))
    }

    /// What is under (`col`, `row`): the overlays top-most first (help,
    /// picker, `K` popup, key clue), the status line, then the screen's
    /// panes (on the commit screen the comment box, the commit list and
    /// the file list over the diff, as drawn).
    pub fn hit(&self, col: u16, row: u16) -> Hit {
        let p = Position::new(col, row);
        let main = self.main_area();
        if self.help_view().is_some()
            && main.width >= 3
            && main.height >= 3
            && help_rect(main).contains(p)
        {
            return Hit::Help;
        }
        if let Some(v) = self.picker()
            && main.width >= 6
            && main.height >= 5
        {
            let rect = help_rect(main);
            if rect.contains(p) {
                // Inside the border: the filter line, the items, the keys.
                let list = Rect::new(
                    rect.x + 1,
                    rect.y + 2,
                    rect.width.saturating_sub(2),
                    rect.height.saturating_sub(4),
                );
                if list.contains(p) {
                    let i = v.skip(list.height as usize) + (row - list.y) as usize;
                    if i < v.items.len() {
                        return Hit::PickerItem(i);
                    }
                }
                return Hit::Picker;
            }
        }
        if self.popup_view().is_some_and(|v| v.rect.contains(p)) {
            return Hit::Popup;
        }
        if crate::ui::clue::rect(self, main).is_some_and(|r| r.contains(p)) {
            return Hit::Clue;
        }
        if !main.contains(p) {
            return if row == main.bottom() {
                Hit::Status
            } else {
                Hit::None
            };
        }
        match self.screen {
            Screen::Log => self.hit_log(main, p),
            Screen::Commit(_) => self.hit_commit(main, p),
        }
    }

    fn hit_log(&self, main: Rect, p: Position) -> Hit {
        let l = self.log_layout(main);
        if l.stats.is_some_and(|s| s.contains(p)) {
            return Hit::Stats;
        }
        if !l.log.contains(p) {
            return Hit::None;
        }
        let i = self.scroll + (p.y - l.log.y) as usize;
        if i < self.rows.len() {
            Hit::Log(i)
        } else {
            Hit::LogPane
        }
    }

    fn hit_commit(&self, main: Rect, p: Position) -> Hit {
        let l = self.commit_layout(main);
        if self
            .comment_box(self.side_pane(l.diff))
            .is_some_and(|b| b.rect.contains(p))
        {
            return Hit::Comment;
        }
        let side = self.config.view.sidebar_side;
        if let Some(c) = l.commits.filter(|c| c.contains(p)) {
            // The rule is the inner rect's top row.
            let inner = list_inner(c, side);
            if !inner.contains(p) || p.y == inner.y {
                return Hit::CommitsPane;
            }
            let i = self.commit_list.scroll + (p.y - inner.y - 1) as usize;
            return if i < self.rows.len() {
                Hit::Commits(i)
            } else {
                Hit::CommitsPane
            };
        }
        if let Some(t) = l.sidebar.filter(|t| t.contains(p)) {
            let inner = list_inner(t, side);
            if !inner.contains(p) {
                return Hit::FilesPane;
            }
            let rows = self.file_list().map_or(0, |l| l.visible.rows.len());
            let r = self.files_skip(inner.height as usize) + (p.y - inner.y) as usize;
            return if r < rows {
                Hit::Files {
                    row: r,
                    x: p.x - inner.x,
                }
            } else {
                Hit::FilesPane
            };
        }
        if l.diff.contains(p) {
            return self.hit_diff(l.diff, p);
        }
        Hit::None
    }

    /// A cell of the diff pane `d`: unified, or side by side the half it
    /// is in (a row drawn across both is the cursor's side's, with its
    /// gutter and horizontal scroll).
    fn hit_diff(&self, d: Rect, p: Position) -> Hit {
        let (scroll, hscroll) = self.commit_scroll();
        let row = scroll + (p.y - d.y) as usize;
        let marks = self.review_gutter();
        let num_w = self.commit_num_width();
        let text_col = |start: u16, gutter: usize, hscroll: usize| {
            let text = start as usize + marks + gutter;
            hscroll + (p.x as usize).saturating_sub(text)
        };
        let Some(mine) = self.split_side() else {
            return Hit::Diff {
                row,
                col: text_col(d.x, gutter_width(num_w), hscroll),
                side: None,
            };
        };
        let gutter = side_gutter_width(num_w);
        let across = self
            .commit_rows()
            .get(row)
            .is_some_and(|r| spans_both(r.kind));
        if across {
            return Hit::Diff {
                row,
                col: text_col(d.x, gutter, self.split_hscroll(mine)),
                side: Some(mine),
            };
        }
        let (left, right) = self.split_panes(d);
        let (side, pane) = if p.x < right.x {
            (Side::Old, left)
        } else {
            (Side::New, right)
        };
        Hit::Diff {
            row,
            col: text_col(pane.x, gutter, self.split_hscroll(side)),
            side: Some(side),
        }
    }

    /// Handle one mouse event that arrived at `now`.
    pub fn mouse(&mut self, ev: MouseEvent, now: Instant) {
        if !capture_wanted(&self.config) || self.typing() {
            self.mouse = MouseState::default();
            return;
        }
        self.stat_follow();
        let (col, row) = (ev.column, ev.row);
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_press(col, row, now),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(row),
            MouseEventKind::Up(MouseButton::Left) => self.mouse.drag = None,
            MouseEventKind::ScrollDown => self.mouse_wheel(col, row, true),
            MouseEventKind::ScrollUp => self.mouse_wheel(col, row, false),
            _ => return,
        }
        self.keep_visible();
        self.commit_follow();
        self.stat_follow();
        self.clue_sync();
    }

    /// Text is being typed: the comment box, a search or filter prompt,
    /// help's filter, the quit prompt.
    fn typing(&self) -> bool {
        matches!(
            self.mode,
            Mode::Comment | Mode::Search | Mode::Filter | Mode::QuitPrompt
        ) || self.help_filtering()
    }

    /// Count this press as a single or double click.
    fn click_count(&mut self, col: u16, row: u16, now: Instant) -> u8 {
        let n = match self.mouse.last_press {
            Some((c, r, t, 1))
                if (c, r) == (col, row) && now.saturating_duration_since(t) <= MULTI_CLICK =>
            {
                2
            }
            _ => 1,
        };
        self.mouse.last_press = Some((col, row, now, n));
        n
    }

    /// Close the overlay a press outside it lands next to. True when one
    /// was closed (the press then does nothing else).
    fn close_outside(&mut self, hit: Hit) -> bool {
        let closed = match self.mode {
            Mode::Picker if !matches!(hit, Hit::Picker | Hit::PickerItem(_)) => {
                self.picker_action(PickerAction::Close);
                true
            }
            Mode::Help if hit != Hit::Help => {
                self.help_action(HelpAction::Close);
                true
            }
            Mode::Popup if hit != Hit::Popup => {
                self.popup_close();
                true
            }
            _ => false,
        };
        if closed {
            self.mouse.last_press = None;
        }
        closed
    }

    fn mouse_press(&mut self, col: u16, row: u16, now: Instant) {
        self.mouse.drag = None;
        let hit = self.hit(col, row);
        if self.close_outside(hit) {
            return;
        }
        let n = self.click_count(col, row, now);
        if let Hit::PickerItem(i) = hit {
            self.picker_select(i);
            if n == 2 {
                self.picker_action(PickerAction::Accept);
            }
            return;
        }
        if !matches!(self.mode, Mode::Normal | Mode::Visual) {
            return;
        }
        // Only clicks that act drop a half-typed key sequence and count.
        if matches!(
            hit,
            Hit::Log(_) | Hit::Files { .. } | Hit::Commits(_) | Hit::Diff { .. }
        ) {
            self.pending.clear();
            self.count = None;
            self.status.clear();
        }
        match hit {
            Hit::Log(i) => {
                self.goto_row(i);
                if n == 2 {
                    self.open_commit();
                }
            }
            Hit::Files { row, x } => self.click_files(row, x, n),
            Hit::Commits(i) => {
                self.visual_leave();
                self.commit_list_select(i);
                if n == 2 {
                    self.commit_list_open();
                }
            }
            Hit::Diff { row, col, side } => self.click_diff(row, col, side),
            _ => {}
        }
    }

    fn visual_leave(&mut self) {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    /// A press on visible file-list row `row`, `x` columns into the list:
    /// focus the list and select the row (its diff jump included) without
    /// scrolling it; a directory's arrow toggles it, and a double click
    /// acts as Enter.
    fn click_files(&mut self, row: usize, x: u16, n: u8) {
        let Some(inner) = self.tree_inner() else {
            return;
        };
        self.visual_leave();
        let h = inner.height as usize;
        let top = self.files_skip(h);
        self.focus_pane(Focus::Files);
        self.select_row(row);
        self.files_pin(top, h);
        let on_arrow = self
            .file_list()
            .and_then(|l| l.entry(row).filter(|e| e.is_dir()).cloned())
            .is_some_and(|e| {
                let a = lead_col(&e, inner.width as usize);
                (a..a + 2).contains(&(x as usize))
            });
        if on_arrow || n == 2 {
            self.file_open();
            self.sidebar_refit();
        }
        self.session_touch_open();
    }

    /// A press in the diff: focus it (closing an auto peek), the cursor
    /// to the row and column (and side, side by side); a drag from here
    /// selects lines.
    fn click_diff(&mut self, row: usize, col: usize, side: Option<Side>) {
        self.visual_leave();
        self.set_focus(Focus::Diff);
        let Some(v) = &mut self.commit else { return };
        if side.is_some() && v.split_side() != side {
            v.swap_side();
        }
        v.cur.row = row.min(v.last_row());
        v.set_col(col);
        v.keep_visible();
        self.mouse.drag = Some(Drag {
            anchor: v.cur.row,
            moved: false,
        });
        self.session_touch_open();
    }

    /// The pointer dragged to screen row `row` after a press in the diff:
    /// select the lines from the pressed row to it. On or above the
    /// diff's top row, or below its bottom, the view scrolls a row and
    /// the selection follows.
    fn mouse_drag(&mut self, row: u16) {
        let Some(mut d) = self.mouse.drag else { return };
        if !matches!(self.mode, Mode::Normal | Mode::Visual) {
            self.mouse.drag = None;
            return;
        }
        let diff = self.commit_layout(self.main_area()).diff;
        let Some(v) = self.commit.as_mut().filter(|v| !v.rows.is_empty()) else {
            return;
        };
        let target = if row <= diff.y {
            v.line_up();
            v.scroll
        } else if row >= diff.bottom() {
            v.line_down();
            v.scroll + v.view_h - 1
        } else {
            v.scroll + (row - diff.y) as usize
        };
        let target = target.min(v.last_row());
        if !d.moved && target == d.anchor {
            return;
        }
        v.anchor = d.anchor;
        v.cur.row = target;
        v.clamp_cursor();
        v.keep_visible();
        self.mode = Mode::Visual;
        d.moved = true;
        self.mouse.drag = Some(d);
        self.session_touch_open();
    }

    /// The wheel over (`col`, `row`): the picker moves its selection, help
    /// and the `K` popup scroll; otherwise the pane under the pointer
    /// scrolls [`WHEEL_ROWS`].
    fn mouse_wheel(&mut self, col: u16, row: u16, down: bool) {
        let hit = self.hit(col, row);
        let rows = if down {
            WHEEL_ROWS as isize
        } else {
            -(WHEEL_ROWS as isize)
        };
        match self.mode {
            Mode::Picker => {
                if matches!(hit, Hit::Picker | Hit::PickerItem(_)) {
                    self.picker_step(if down { 1 } else { -1 });
                }
                return;
            }
            Mode::Help => {
                if hit == Hit::Help {
                    let a = if down {
                        HelpAction::Down
                    } else {
                        HelpAction::Up
                    };
                    (0..WHEEL_ROWS).for_each(|_| self.help_action(a));
                }
                return;
            }
            Mode::Popup => {
                if hit == Hit::Popup {
                    let m = if down { PopupMove::Down } else { PopupMove::Up };
                    (0..WHEEL_ROWS).for_each(|_| self.popup_move(m));
                }
                return;
            }
            Mode::Normal | Mode::Visual => {}
            _ => return,
        }
        match hit {
            Hit::Log(_) | Hit::LogPane => self.log_wheel(rows),
            Hit::Stats => self.stat_scroll_by(rows),
            Hit::Files { .. } | Hit::FilesPane => {
                if let Some(inner) = self.tree_inner() {
                    self.files_wheel(rows, inner.height as usize);
                }
            }
            Hit::Commits(_) | Hit::CommitsPane => self.commit_list_scroll(rows),
            Hit::Diff { .. } => {
                let Some(v) = &mut self.commit else { return };
                for _ in 0..WHEEL_ROWS {
                    if down {
                        v.line_down();
                    } else {
                        v.line_up();
                    }
                }
                self.session_touch_open();
            }
            _ => {}
        }
    }

    /// Scroll the log `delta` rows; the cursor moves only to stay on
    /// screen.
    fn log_wheel(&mut self, delta: isize) {
        let h = self.view_h.max(1);
        let max = self.rows.len().saturating_sub(h);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
        self.cursor = self
            .cursor
            .clamp(self.scroll, self.scroll + h - 1)
            .min(self.last_row());
    }
}
