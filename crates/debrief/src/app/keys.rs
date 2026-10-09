//! Key dispatch: [`App::keymap`] turns the pending key sequence into a
//! [`KeyResult`]; [`App::apply`] runs an [`Action`]. A new screen adds
//! `Action` variants, a keymap for its screen, and arms in `apply`. Every
//! binding also gets a row in `help::BINDINGS` (a test resolves each row
//! through `keymap`).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::commit::{Focus, RowKind};
use super::help::HelpAction;
use super::{App, Mode, Screen};

/// Cap on a typed count.
const MAX_COUNT: usize = 1_000_000;

/// The `<leader>` key (Space, as in ramble).
pub const LEADER: char = ' ';

/// Everything a key can do. `Option<usize>` is the typed count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Down(Option<usize>),
    Up(Option<usize>),
    /// `gg`: row `count` (1-based), else the first.
    GotoTop(Option<usize>),
    /// `G`: row `count` (1-based), else the last.
    GotoBottom(Option<usize>),
    HalfPageDown,
    HalfPageUp,
    /// `Enter`: open the commit under the cursor.
    Open,
    /// `m`: mark every listed commit seen.
    MarkAll,
    /// `r`: re-read the log.
    Reload,
    /// `g?` opens the help overlay; its own keys.
    Help(HelpAction),
    /// Commit view: Esc / q back to the log.
    Back,
    Left(Option<usize>),
    Right(Option<usize>),
    /// `0`.
    LineStart,
    /// `$`: end of the line `count - 1` rows down.
    LineEnd(Option<usize>),
    WordForward(Option<usize>),
    WordBackward(Option<usize>),
    WordEnd(Option<usize>),
    ParagraphForward(Option<usize>),
    ParagraphBackward(Option<usize>),
    PageDown,
    PageUp,
    /// `C-e`.
    ScrollDown,
    /// `C-y`.
    ScrollUp,
    /// `H`.
    ScreenTop(Option<usize>),
    /// `M`.
    ScreenMiddle,
    /// `L`.
    ScreenBottom(Option<usize>),
    /// `zz`.
    Center,
    /// `zt`.
    CursorTop,
    /// `zb`.
    CursorBottom,
    /// `]f` / `[f`.
    File(bool, Option<usize>),
    /// `]c` / `[c`.
    Hunk(bool, Option<usize>),
    /// `]]` / `[[`: the next (older) / previous commit of the log.
    Commit(bool),
    /// `/` (true) or `?`.
    SearchStart(bool),
    /// `n` (true) or `N`.
    SearchNext(bool, Option<usize>),
    SearchInput(char),
    SearchBackspace,
    SearchCommit,
    SearchCancel,
    /// `v` / `V`.
    Visual,
    VisualCancel,
    /// `y` in visual mode.
    Yank,
    /// `<leader>e`.
    ToggleSidebar,
    /// `C-w h` / `C-w l`.
    Focus(Focus),
    /// File list: select the next / previous file (jumps the diff).
    FileDown(Option<usize>),
    FileUp(Option<usize>),
    /// File list: first / last file.
    FileTop,
    FileBottom,
    /// File list Enter: jump to the file and focus the diff.
    FileOpen,
    /// A `[[launch]]` key: run launcher `i` of the config.
    Launch(usize),
    /// Enter in the diff: a `.md` file header goes to ramble.
    DiffEnter,
    /// `cc`, visual `c`, and the comment prompt's keys.
    Comment(super::comment::CommentAction),
    /// `]r` / `[r`.
    ReviewJump(bool, Option<usize>),
    /// `K`: the comments on the cursor row.
    Hover,
    /// Closes the `K` popup.
    PopupClose,
    /// Scrolls the `K` popup.
    PopupMove(super::review::PopupMove),
    /// `<leader>rl`.
    ReviewList,
    /// `<leader>rr`.
    ReviewSend,
    /// Keys in the `<leader>rl` picker.
    Picker(super::picker::PickerAction),
    /// Log: `f` opens the filter prompt.
    FilterStart,
    FilterInput(char),
    FilterBackspace,
    /// Enter in the filter prompt: keep the filter.
    FilterCommit,
    /// Esc in the filter prompt, or on the log: clear the filter.
    FilterClear,
}

/// What a key sequence means so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyResult {
    Action(Action),
    /// A prefix of a longer sequence: keep the keys and wait.
    Pending,
    /// A count digit to append to the typed count.
    Count(usize),
    /// Not bound: drop the sequence and the count.
    None,
}

pub(super) fn plain(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Some(c),
        _ => None,
    }
}

impl App {
    /// Handle one key press.
    pub fn key(&mut self, key: KeyEvent) {
        self.key_inner(key);
        self.clue_sync();
    }

    fn key_inner(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if key.code == KeyCode::Esc && (!self.pending.is_empty() || self.count.is_some()) {
            // Esc first cancels a half-typed sequence or count.
            self.pending.clear();
            self.count = None;
            return;
        }
        self.pending.push(key);
        let keys = std::mem::take(&mut self.pending);
        match self.keymap(&keys) {
            KeyResult::Count(d) => {
                let n = self.count.unwrap_or(0).saturating_mul(10).saturating_add(d);
                self.count = Some(n.min(MAX_COUNT));
                return;
            }
            KeyResult::Pending => self.pending = keys,
            KeyResult::Action(a) => {
                self.count = None;
                self.status.clear();
                self.apply(a);
            }
            KeyResult::None => self.count = None,
        }
        self.keep_visible();
        self.commit_follow();
    }

    /// Meaning of `keys` in the current mode and screen.
    pub fn keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        match (self.mode, self.screen) {
            (Mode::Help, _) => super::help::help_keymap(self, keys),
            (Mode::Picker, _) => super::picker::picker_keymap(keys),
            (Mode::Filter, _) => filter_keymap(keys),
            (Mode::Search, Screen::Log) => search_keymap(keys),
            (_, Screen::Log) => self.log_keymap(keys),
            (Mode::Search, Screen::Commit(_)) => search_keymap(keys),
            (Mode::Comment, Screen::Commit(_)) => super::comment::comment_keymap(keys),
            (Mode::Popup, Screen::Commit(_)) => popup_keymap(keys),
            (Mode::Visual, Screen::Commit(_)) => self.visual_keymap(keys),
            (Mode::Normal, Screen::Commit(_)) if self.files_focused() => self.files_keymap(keys),
            (Mode::Normal, Screen::Commit(_)) => self.commit_keymap(keys),
        }
    }

    /// Two-key sequences shared by the commit screen's panes: `<leader>e`,
    /// `C-w h/l`, `g?`. `None` when `keys` don't start one.
    fn pane_keymap(&self, keys: &[KeyEvent]) -> Option<KeyResult> {
        use Action as A;
        let first = keys.first()?;
        let second = keys.get(1);
        let ctrl_w =
            first.modifiers.contains(KeyModifiers::CONTROL) && first.code == KeyCode::Char('w');
        if plain(first) == Some(LEADER) {
            return Some(self.leader_keymap(&keys[1..]));
        }
        let r = if ctrl_w {
            match second.map(plain) {
                None => KeyResult::Pending,
                Some(Some('h')) => KeyResult::Action(A::Focus(Focus::Files)),
                Some(Some('l')) => KeyResult::Action(A::Focus(Focus::Diff)),
                _ => KeyResult::None,
            }
        } else if let [a, b] = keys
            && plain(a) == Some('g')
            && plain(b) == Some('?')
        {
            KeyResult::Action(A::Help(HelpAction::Open))
        } else {
            return None;
        };
        Some(if keys.len() > 2 { KeyResult::None } else { r })
    }

    /// Keys after `<leader>`: the `[[launch]]` keys first (a launcher
    /// prefix waits for more keys), then the built-ins (`e` on the commit
    /// screen, `rl`, `rr`), as in ramble. `e` toggles the file list on
    /// the commit screen and the stats pane on the log.
    pub(super) fn leader_keymap(&self, after: &[KeyEvent]) -> KeyResult {
        let Some(typed) = after.iter().map(plain).collect::<Option<Vec<char>>>() else {
            return KeyResult::None;
        };
        use super::launch::LeaderMatch;
        match super::launch::match_leader(&self.launch_keys, &typed) {
            LeaderMatch::Launch(i) => KeyResult::Action(Action::Launch(i)),
            LeaderMatch::Pending => KeyResult::Pending,
            LeaderMatch::NoMapping => match typed.as_slice() {
                [] | ['r'] => KeyResult::Pending,
                ['e'] => KeyResult::Action(Action::ToggleSidebar),
                ['r', 'l'] => KeyResult::Action(Action::ReviewList),
                ['r', 'r'] => KeyResult::Action(Action::ReviewSend),
                _ => KeyResult::None,
            },
        }
    }

    /// The diff pane in normal mode.
    fn commit_keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        use Action as A;
        if let Some(r) = self.pane_keymap(keys) {
            return r;
        }
        let count = self.count;
        let [key] = keys else {
            let (Some(prefix), Some(c), 2) = (
                keys.first().and_then(plain),
                keys.get(1).and_then(plain),
                keys.len(),
            ) else {
                return KeyResult::None;
            };
            let action = match (prefix, c) {
                ('g', 'g') => A::GotoTop(count),
                ('z', 'z') => A::Center,
                ('z', 't') => A::CursorTop,
                ('z', 'b') => A::CursorBottom,
                (']', 'f') => A::File(true, count),
                ('[', 'f') => A::File(false, count),
                (']', 'c') => A::Hunk(true, count),
                ('[', 'c') => A::Hunk(false, count),
                (']', ']') => A::Commit(true),
                ('[', '[') => A::Commit(false),
                (']', 'r') => A::ReviewJump(true, count),
                ('[', 'r') => A::ReviewJump(false, count),
                ('c', 'c') => A::Comment(super::comment::CommentAction::Start(false)),
                _ => return KeyResult::None,
            };
            return KeyResult::Action(action);
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('d') => KeyResult::Action(A::HalfPageDown),
                KeyCode::Char('u') => KeyResult::Action(A::HalfPageUp),
                KeyCode::Char('f') => KeyResult::Action(A::PageDown),
                KeyCode::Char('b') => KeyResult::Action(A::PageUp),
                KeyCode::Char('e') => KeyResult::Action(A::ScrollDown),
                KeyCode::Char('y') => KeyResult::Action(A::ScrollUp),
                _ => KeyResult::None,
            };
        }
        let action = match key.code {
            KeyCode::Char(c @ '0'..='9') if c != '0' || count.is_some() => {
                return KeyResult::Count(c as usize - '0' as usize);
            }
            KeyCode::Char('g' | 'z' | ']' | '[' | 'c') => return KeyResult::Pending,
            KeyCode::Char('q') | KeyCode::Esc => A::Back,
            KeyCode::Char('j') | KeyCode::Down => A::Down(count),
            KeyCode::Char('k') | KeyCode::Up => A::Up(count),
            KeyCode::Char('h') | KeyCode::Left => A::Left(count),
            KeyCode::Char('l') | KeyCode::Right => A::Right(count),
            KeyCode::Char('0') => A::LineStart,
            KeyCode::Char('$') => A::LineEnd(count),
            KeyCode::Char('w') => A::WordForward(count),
            KeyCode::Char('b') => A::WordBackward(count),
            KeyCode::Char('e') => A::WordEnd(count),
            KeyCode::Char('}') => A::ParagraphForward(count),
            KeyCode::Char('{') => A::ParagraphBackward(count),
            KeyCode::Char('G') => A::GotoBottom(count),
            KeyCode::Char('H') => A::ScreenTop(count),
            KeyCode::Char('M') => A::ScreenMiddle,
            KeyCode::Char('L') => A::ScreenBottom(count),
            KeyCode::Char('/') => A::SearchStart(true),
            KeyCode::Char('?') => A::SearchStart(false),
            KeyCode::Char('n') => A::SearchNext(true, count),
            KeyCode::Char('N') => A::SearchNext(false, count),
            KeyCode::Char('v' | 'V') => A::Visual,
            KeyCode::Enter => A::DiffEnter,
            KeyCode::Char('K') => A::Hover,
            KeyCode::Char('r') => A::Reload,
            _ => return KeyResult::None,
        };
        KeyResult::Action(action)
    }

    /// Visual mode: `y`, `v`/`V`, Esc, and the diff pane's motions.
    fn visual_keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        use Action as A;
        if let [k] = keys {
            match (k.code, plain(k)) {
                (KeyCode::Esc, _) => return KeyResult::Action(A::VisualCancel),
                (_, Some('y')) => return KeyResult::Action(A::Yank),
                (_, Some('c')) => {
                    return KeyResult::Action(A::Comment(super::comment::CommentAction::Start(
                        true,
                    )));
                }
                _ => {}
            }
        }
        if self.pane_keymap(keys).is_some() {
            // `<leader>e`, `C-w h/l` and `g?` don't apply in visual mode.
            return KeyResult::None;
        }
        match self.commit_keymap(keys) {
            KeyResult::Action(a) if visual_ok(a) => KeyResult::Action(a),
            KeyResult::Action(_) => KeyResult::None,
            r => r,
        }
    }

    /// The file list.
    fn files_keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        use Action as A;
        if let Some(r) = self.pane_keymap(keys) {
            return r;
        }
        let count = self.count;
        match keys {
            [a, b] if plain(a) == Some('g') && plain(b) == Some('g') => {
                KeyResult::Action(A::FileTop)
            }
            [k] if k.modifiers.contains(KeyModifiers::CONTROL) => KeyResult::None,
            [k] => KeyResult::Action(match (k.code, plain(k)) {
                (_, Some(c @ '1'..='9')) => return KeyResult::Count(c as usize - '0' as usize),
                (_, Some('0')) if count.is_some() => return KeyResult::Count(0),
                (_, Some('g')) => return KeyResult::Pending,
                (_, Some('j')) | (KeyCode::Down, _) => A::FileDown(count),
                (_, Some('k')) | (KeyCode::Up, _) => A::FileUp(count),
                (_, Some('G')) => A::FileBottom,
                (KeyCode::Enter, _) => A::FileOpen,
                (KeyCode::Esc, _) => A::Focus(Focus::Diff),
                (_, Some('q')) => A::Back,
                _ => return KeyResult::None,
            }),
            _ => KeyResult::None,
        }
    }

    fn log_keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        use Action as A;
        if keys.first().and_then(plain) == Some(LEADER) {
            return self.leader_keymap(&keys[1..]);
        }
        let count = self.count;
        let [key] = keys else {
            let (Some(prefix), Some(c)) =
                (keys.first().and_then(plain), keys.get(1).and_then(plain))
            else {
                return KeyResult::None;
            };
            return match (prefix, c) {
                ('g', 'g') => KeyResult::Action(A::GotoTop(count)),
                ('g', '?') => KeyResult::Action(A::Help(HelpAction::Open)),
                _ => KeyResult::None,
            };
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('d') => KeyResult::Action(A::HalfPageDown),
                KeyCode::Char('u') => KeyResult::Action(A::HalfPageUp),
                _ => KeyResult::None,
            };
        }
        let action = match key.code {
            KeyCode::Char(c @ '0'..='9') if c != '0' || count.is_some() => {
                return KeyResult::Count(c as usize - '0' as usize);
            }
            KeyCode::Char('g') => return KeyResult::Pending,
            KeyCode::Char('q') => A::Quit,
            KeyCode::Char('j') | KeyCode::Down => A::Down(count),
            KeyCode::Char('k') | KeyCode::Up => A::Up(count),
            KeyCode::Char('G') => A::GotoBottom(count),
            KeyCode::Enter => A::Open,
            KeyCode::Char('m') => A::MarkAll,
            KeyCode::Char('r') => A::Reload,
            KeyCode::Char('/') => A::SearchStart(true),
            KeyCode::Char('?') => A::SearchStart(false),
            KeyCode::Char('n') => A::SearchNext(true, count),
            KeyCode::Char('N') => A::SearchNext(false, count),
            KeyCode::Char('f') => A::FilterStart,
            KeyCode::Esc => A::FilterClear,
            _ => return KeyResult::None,
        };
        KeyResult::Action(action)
    }

    /// Run one action.
    pub fn apply(&mut self, action: Action) {
        use Action as A;
        let n = |c: Option<usize>| c.unwrap_or(1).max(1) as isize;
        let shared = matches!(
            action,
            A::Quit | A::Help(_) | A::Launch(_) | A::ReviewList | A::ReviewSend | A::Picker(_)
        );
        if matches!(self.screen, Screen::Commit(_)) && !shared {
            return self.apply_commit(action);
        }
        match action {
            A::Quit => self.quit = true,
            A::Down(c) => self.vertical(n(c)),
            A::Up(c) => self.vertical(-n(c)),
            A::GotoTop(c) => self.goto_row(c.map_or(0, |c| c.saturating_sub(1))),
            A::GotoBottom(c) => self.goto_row(c.map_or(self.last_row(), |c| c.saturating_sub(1))),
            A::HalfPageDown => self.half_page(true),
            A::HalfPageUp => self.half_page(false),
            A::Open => self.open_commit(),
            A::MarkAll => self.mark_all(),
            A::Reload => self.reload(true),
            A::Help(a) => self.help_action(a),
            A::Launch(i) => self.launch_index(i),
            A::ReviewList => self.picker_open(),
            A::ReviewSend => self.review_send(),
            A::Picker(a) => self.picker_action(a),
            A::SearchStart(f) => self.log_search_start(f),
            A::SearchNext(same, c) => self.log_search_next(same, n(c) as usize),
            A::SearchInput(c) => self.log_search_edit(Some(c)),
            A::SearchBackspace => self.log_search_edit(None),
            A::SearchCommit => self.log_search_commit(),
            A::SearchCancel => self.log_search_cancel(),
            A::FilterStart => self.filter_start(),
            A::FilterInput(c) => self.filter_edit(Some(c)),
            A::FilterBackspace => self.filter_edit(None),
            A::FilterCommit => self.filter_commit(),
            A::FilterClear => self.filter_clear(),
            A::ToggleSidebar => self.toggle_stats(),
            other => self.apply_commit(other),
        }
    }

    /// Commit-screen actions.
    fn apply_commit(&mut self, action: Action) {
        use Action as A;
        let n = |c: Option<usize>| c.unwrap_or(1).max(1);
        match action {
            A::Back => return self.close_commit(),
            A::Reload => return self.reload_view(),
            A::Commit(forward) => return self.step_commit(forward),
            A::SearchStart(f) => return self.search_start(f),
            A::SearchNext(same, c) => return self.search_next(same, n(c)),
            A::SearchInput(c) => return self.search_edit(Some(c)),
            A::SearchBackspace => return self.search_edit(None),
            A::SearchCommit => return self.search_commit(),
            A::SearchCancel => return self.search_cancel(),
            A::Visual => return self.visual_start(),
            A::VisualCancel => {
                self.mode = Mode::Normal;
                return;
            }
            A::Yank => return self.yank(),
            A::DiffEnter => return self.diff_enter(),
            A::Comment(a) => return self.comment_action(a),
            A::ReviewJump(f, c) => return self.review_jump(f, n(c)),
            A::Hover => return self.review_hover(),
            A::PopupClose => return self.popup_close(),
            A::PopupMove(m) => return self.popup_move(m),
            A::ToggleSidebar => return self.toggle_sidebar(),
            A::Focus(f) => return self.focus_pane(f),
            A::FileDown(c) => return self.select_file(self.sidebar_selected() + n(c)),
            A::FileUp(c) => {
                return self.select_file(self.sidebar_selected().saturating_sub(n(c)));
            }
            A::FileTop => return self.select_file(0),
            A::FileBottom => return self.select_file(usize::MAX),
            A::FileOpen => {
                self.select_file(self.sidebar_selected());
                self.focus = Focus::Diff;
                return;
            }
            _ => {}
        }
        let Some(v) = &mut self.commit else { return };
        let mut missing = None;
        match action {
            A::Down(c) => v.vertical(n(c) as isize),
            A::Up(c) => v.vertical(-(n(c) as isize)),
            A::Left(c) => v.horizontal(-(n(c) as isize)),
            A::Right(c) => v.horizontal(n(c) as isize),
            A::LineStart => v.set_col(0),
            A::LineEnd(c) => v.line_end(n(c)),
            A::WordForward(c) => v.repeat(n(c), super::commit::CommitView::word_forward),
            A::WordBackward(c) => v.repeat(n(c), super::commit::CommitView::word_backward),
            A::WordEnd(c) => v.repeat(n(c), super::commit::CommitView::word_end),
            A::ParagraphForward(c) => {
                v.repeat(n(c), super::commit::CommitView::paragraph_forward);
            }
            A::ParagraphBackward(c) => {
                v.repeat(n(c), super::commit::CommitView::paragraph_backward);
            }
            A::GotoTop(c) => v.goto_row(c.map_or(0, |c| c.saturating_sub(1))),
            A::GotoBottom(c) => v.goto_row(c.map_or(v.last_row(), |c| c.saturating_sub(1))),
            A::HalfPageDown => v.half_page(true),
            A::HalfPageUp => v.half_page(false),
            A::PageDown => v.page_down(),
            A::PageUp => v.page_up(),
            A::ScrollDown => v.line_down(),
            A::ScrollUp => v.line_up(),
            A::ScreenTop(c) => v.screen_top(n(c)),
            A::ScreenMiddle => v.screen_middle(),
            A::ScreenBottom(c) => v.screen_bottom(n(c)),
            A::Center => v.center_cursor(),
            A::CursorTop => v.cursor_to_top(),
            A::CursorBottom => v.cursor_to_bottom(),
            A::File(f, c) if !v.jump_kind(RowKind::FileHeader, f, n(c)) => {
                missing = Some(if f {
                    "no next file"
                } else {
                    "no previous file"
                });
            }
            A::Hunk(f, c) if !v.jump_kind(RowKind::HunkHeader, f, n(c)) => {
                missing = Some(if f {
                    "no next hunk"
                } else {
                    "no previous hunk"
                });
            }
            _ => {}
        }
        if let Some(m) = missing {
            self.set_status(m);
        }
    }
}

/// Actions visual mode keeps: motions, scrolling, search and `v`.
fn visual_ok(a: Action) -> bool {
    use Action as A;
    !matches!(
        a,
        A::Back
            | A::Commit(_)
            | A::ToggleSidebar
            | A::Focus(_)
            | A::Help(_)
            | A::SearchStart(_)
            | A::DiffEnter
            | A::Launch(_)
            | A::Hover
            | A::PopupClose
            | A::PopupMove(_)
            | A::ReviewList
            | A::ReviewSend
    )
}

/// Typing a search pattern: every plain key is input.
fn search_keymap(keys: &[KeyEvent]) -> KeyResult {
    use Action as A;
    let Some(k) = keys.last() else {
        return KeyResult::None;
    };
    KeyResult::Action(match k.code {
        KeyCode::Enter => A::SearchCommit,
        KeyCode::Esc => A::SearchCancel,
        KeyCode::Backspace => A::SearchBackspace,
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => A::SearchInput(c),
        _ => return KeyResult::None,
    })
}

/// Typing the log's filter: every plain key is input.
fn filter_keymap(keys: &[KeyEvent]) -> KeyResult {
    use Action as A;
    let Some(k) = keys.last() else {
        return KeyResult::None;
    };
    KeyResult::Action(match k.code {
        KeyCode::Enter => A::FilterCommit,
        KeyCode::Esc => A::FilterClear,
        KeyCode::Backspace => A::FilterBackspace,
        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => A::FilterInput(c),
        _ => return KeyResult::None,
    })
}

/// The `K` popup: Esc, q or K close it; j/k, C-d/C-u, gg/G scroll it.
fn popup_keymap(keys: &[KeyEvent]) -> KeyResult {
    use super::review::PopupMove as P;
    let act = |m| KeyResult::Action(Action::PopupMove(m));
    match keys {
        [k] if matches!(k.code, KeyCode::Esc) || matches!(plain(k), Some('q' | 'K')) => {
            KeyResult::Action(Action::PopupClose)
        }
        [k] if k.modifiers.contains(KeyModifiers::CONTROL) => match k.code {
            KeyCode::Char('d') => act(P::HalfDown),
            KeyCode::Char('u') => act(P::HalfUp),
            _ => KeyResult::None,
        },
        [k] => match k.code {
            KeyCode::Char('j') | KeyCode::Down => act(P::Down),
            KeyCode::Char('k') | KeyCode::Up => act(P::Up),
            KeyCode::Char('G') => act(P::Bottom),
            KeyCode::Char('g') => KeyResult::Pending,
            _ => KeyResult::None,
        },
        [a, b] if plain(a) == Some('g') && plain(b) == Some('g') => act(P::Top),
        _ => KeyResult::None,
    }
}
