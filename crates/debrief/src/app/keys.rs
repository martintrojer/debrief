//! Key dispatch: [`App::keymap`] turns the pending key sequence into a
//! [`KeyResult`]; [`App::apply`] runs an [`Action`]. A new screen adds
//! `Action` variants, a keymap for its screen, and arms in `apply`. Every
//! binding also gets a row in `help::BINDINGS` (a test resolves each row
//! through `keymap`).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::help::HelpAction;
use super::{App, Mode, Screen};

/// Cap on a typed count.
const MAX_COUNT: usize = 1_000_000;

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
    }

    /// Meaning of `keys` in the current mode and screen.
    pub fn keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        match (self.mode, self.screen) {
            (Mode::Help, _) => super::help::help_keymap(self, keys),
            (Mode::Normal, Screen::Log | Screen::Commit(_)) => self.log_keymap(keys),
        }
    }

    fn log_keymap(&self, keys: &[KeyEvent]) -> KeyResult {
        use Action as A;
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
            _ => return KeyResult::None,
        };
        KeyResult::Action(action)
    }

    /// Run one action.
    pub fn apply(&mut self, action: Action) {
        use Action as A;
        let n = |c: Option<usize>| c.unwrap_or(1).max(1) as isize;
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
            A::Reload => self.reload(),
            A::Help(a) => self.help_action(a),
        }
    }
}
