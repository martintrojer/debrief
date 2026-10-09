//! The `g?` help overlay (spec §3 "Help"): every binding that works now,
//! grouped, scrollable and filterable.
//!
//! Dispatch stays in the `match`-based keymaps; [`BINDINGS`] describes
//! them. A unit test resolves every row through [`App::keymap`] in its
//! context, so a row that no longer matches a binding fails the build.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::keys::{Action, KeyResult, LEADER, plain};
use super::{App, Mode, Screen};

/// Where a binding works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctx {
    /// The log screen.
    Log,
    /// The commit screen's diff pane.
    Commit,
    /// The commit screen's file list has focus.
    Files,
    /// Visual mode in the commit view.
    Visual,
    /// Typing a search pattern.
    Search,
    /// Typing a comment.
    Comment,
    /// The `K` popup is open.
    Popup,
    /// The `<leader>rl` picker is open.
    Picker,
    /// The help overlay is open.
    Help,
}

/// Help sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    General,
    Motions,
    Log,
    Commit,
    Files,
    Search,
    Visual,
    Review,
    Launch,
    Help,
}

impl Group {
    const ALL: [Group; 10] = [
        Group::General,
        Group::Motions,
        Group::Log,
        Group::Commit,
        Group::Files,
        Group::Search,
        Group::Visual,
        Group::Review,
        Group::Launch,
        Group::Help,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Motions => "Motions",
            Group::Log => "Log",
            Group::Commit => "Commit",
            Group::Files => "File list",
            Group::Search => "Search",
            Group::Visual => "Visual",
            Group::Review => "Review",
            Group::Launch => "Launchers",
            Group::Help => "Help",
        }
    }
}

/// One help row. `keys` lists alternatives separated by `", "`; `C-x` is
/// Ctrl+x (`C-w h` is Ctrl+w then h), `<leader>` the leader key (Space),
/// `{1-9}` a count digit, and `Enter Esc Down Up Left Right Backspace`
/// name keys.
#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub keys: Cow<'static, str>,
    pub context: Ctx,
    pub group: Group,
    pub desc: Cow<'static, str>,
}

const fn b(keys: &'static str, context: Ctx, group: Group, desc: &'static str) -> Binding {
    Binding {
        keys: Cow::Borrowed(keys),
        context,
        group,
        desc: Cow::Borrowed(desc),
    }
}

/// Every key binding, as the help overlay and the key clue list them.
pub(crate) const BINDINGS: &[Binding] = {
    use Ctx::{
        Comment as P, Commit as C, Files as F, Help as H, Log as L, Picker as R, Popup as K,
        Search as S, Visual as V,
    };
    use Group as G;
    &[
        b("q", L, G::General, "quit"),
        b("C-c", L, G::General, "quit"),
        b("g?", L, G::General, "help"),
        b("j, Down", L, G::Motions, "down"),
        b("k, Up", L, G::Motions, "up"),
        b("gg", L, G::Motions, "first commit (or row {count})"),
        b("G", L, G::Motions, "last commit (or row {count})"),
        b("C-d", L, G::Motions, "half page down"),
        b("C-u", L, G::Motions, "half page up"),
        b("{1-9}", L, G::Motions, "count for the next motion"),
        b("Enter", L, G::Log, "open the commit (marks it seen)"),
        b("m", L, G::Log, "mark every listed commit seen"),
        b("r", L, G::Log, "reload the log"),
        b("q, Esc", C, G::General, "back to the log"),
        b("C-c", C, G::General, "quit"),
        b("g?", C, G::General, "help"),
        b("j, Down", C, G::Motions, "down"),
        b("k, Up", C, G::Motions, "up"),
        b("h, Left", C, G::Motions, "left"),
        b("l, Right", C, G::Motions, "right"),
        b("0", C, G::Motions, "start of line"),
        b("$", C, G::Motions, "end of line"),
        b("w", C, G::Motions, "next word"),
        b("b", C, G::Motions, "previous word"),
        b("e", C, G::Motions, "end of word"),
        b("}", C, G::Motions, "next blank row"),
        b("{", C, G::Motions, "previous blank row"),
        b("gg", C, G::Motions, "first row (or row {count})"),
        b("G", C, G::Motions, "last row (or row {count})"),
        b("C-d", C, G::Motions, "half page down"),
        b("C-u", C, G::Motions, "half page up"),
        b("C-f", C, G::Motions, "page down"),
        b("C-b", C, G::Motions, "page up"),
        b("C-e", C, G::Motions, "scroll down a row"),
        b("C-y", C, G::Motions, "scroll up a row"),
        b("H", C, G::Motions, "top of the screen"),
        b("M", C, G::Motions, "middle of the screen"),
        b("L", C, G::Motions, "bottom of the screen"),
        b("zz", C, G::Motions, "cursor row to the middle"),
        b("zt", C, G::Motions, "cursor row to the top"),
        b("zb", C, G::Motions, "cursor row to the bottom"),
        b("{1-9}", C, G::Motions, "count for the next motion"),
        b("]f", C, G::Commit, "next file"),
        b("[f", C, G::Commit, "previous file"),
        b("]c", C, G::Commit, "next hunk"),
        b("[c", C, G::Commit, "previous hunk"),
        b("r", C, G::Commit, "refresh the working-copy diff in place"),
        b("]]", C, G::Commit, "open the next commit"),
        b("[[", C, G::Commit, "open the previous commit"),
        b("/", C, G::Commit, "search forward"),
        b("?", C, G::Commit, "search backward"),
        b("n", C, G::Commit, "next match"),
        b("N", C, G::Commit, "previous match"),
        b("v, V", C, G::Commit, "select rows"),
        b(
            "Enter",
            C,
            G::Launch,
            "on a .md file header: open it in ramble",
        ),
        b("<leader>e", C, G::Files, "show or hide the file list"),
        b("C-w h", C, G::Files, "to the file list"),
        b("C-w l", C, G::Files, "to the diff"),
        b("C-w h", F, G::Files, "to the file list"),
        b("C-w l", F, G::Files, "to the diff"),
        b("<leader>e", F, G::Files, "show or hide the file list"),
        b("g?", F, G::General, "help"),
        b("j, Down", F, G::Files, "next file"),
        b("k, Up", F, G::Files, "previous file"),
        b("gg", F, G::Files, "first file"),
        b("G", F, G::Files, "last file"),
        b("Enter", F, G::Files, "jump to the file"),
        b("Esc", F, G::Files, "back to the diff"),
        b("q", F, G::Files, "back to the log"),
        b("{1-9}", F, G::Files, "count for the next motion"),
        b("Enter", S, G::Search, "in search: jump to the match"),
        b("Esc", S, G::Search, "in search: cancel"),
        b("Backspace", S, G::Search, "in search: delete a character"),
        b("y", V, G::Visual, "copy the rows as diff text"),
        b("v, V", V, G::Visual, "leave visual mode"),
        b("Esc", V, G::Visual, "cancel"),
        b("c", V, G::Review, "comment on the selected diff lines"),
        b("cc", C, G::Review, "comment on the diff line"),
        b("]r", C, G::Review, "next commented line"),
        b("[r", C, G::Review, "previous commented line"),
        b("K", C, G::Review, "show the comments on this line"),
        b("Enter", P, G::Review, "in a comment: save"),
        b("Esc", P, G::Review, "in a comment: cancel"),
        b("C-e", P, G::Review, "in a comment: edit in $EDITOR"),
        b(
            "Backspace",
            P,
            G::Review,
            "in a comment: delete a character",
        ),
        b("q, K, Esc", K, G::Review, "in the comment popup: close"),
        b("j, Down", K, G::Review, "in the comment popup: scroll down"),
        b("k, Up", K, G::Review, "in the comment popup: scroll up"),
        b("C-d", K, G::Review, "in the comment popup: half page down"),
        b("C-u", K, G::Review, "in the comment popup: half page up"),
        b("gg", K, G::Review, "in the comment popup: top"),
        b("G", K, G::Review, "in the comment popup: bottom"),
        b("<leader>rl", L, G::Review, "list the review comments"),
        b("<leader>rr", L, G::Review, "send the review"),
        b("<leader>rl", C, G::Review, "list the review comments"),
        b("<leader>rr", C, G::Review, "send the review"),
        b("<leader>rl", F, G::Review, "list the review comments"),
        b("<leader>rr", F, G::Review, "send the review"),
        b("Enter", R, G::Review, "in the list: go to the comment"),
        b("C-d", R, G::Review, "in the list: remove the comment"),
        b(
            "C-e",
            R,
            G::Review,
            "in the list: edit the comment in $EDITOR",
        ),
        b(
            "C-n, C-p, Down, Up",
            R,
            G::Review,
            "in the list: next, previous",
        ),
        b("Esc", R, G::Review, "in the list: close"),
        b(
            "Backspace",
            R,
            G::Review,
            "in the list: delete a filter character",
        ),
        b("j, Down", H, G::Help, "in help: scroll down"),
        b("k, Up", H, G::Help, "in help: scroll up"),
        b("C-d, C-u", H, G::Help, "in help: half page"),
        b("gg, G", H, G::Help, "in help: top, bottom"),
        b("/", H, G::Help, "in help: filter"),
        b("q, g?, Esc", H, G::Help, "in help: close"),
    ]
};

impl App {
    /// [`BINDINGS`] plus a row per `[[launch]]` key on the log, the diff
    /// and the file list.
    pub(crate) fn bindings(&self) -> Vec<Binding> {
        let mut out: Vec<Binding> = BINDINGS.to_vec();
        for &(_, i) in &self.launch_keys {
            let l = &self.config.launch[i];
            let Some(key) = &l.key else { continue };
            let desc = if l.name == super::config::EDIT {
                "open $EDITOR at this file and line".to_string()
            } else {
                format!("run launcher {}", l.name)
            };
            for ctx in [Ctx::Log, Ctx::Commit, Ctx::Files] {
                out.push(Binding {
                    keys: Cow::Owned(key.clone()),
                    context: ctx,
                    group: Group::Launch,
                    desc: Cow::Owned(desc.clone()),
                });
            }
        }
        out
    }
}

/// Named keys of the `keys` notation, as whole alternatives.
pub(super) const NAMED: [(&str, KeyCode); 7] = [
    ("Enter", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Down", KeyCode::Down),
    ("Up", KeyCode::Up),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Backspace", KeyCode::Backspace),
];

/// One token of a row's key alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tok<'a> {
    Key(KeyEvent),
    /// `{1-9}`, braces included.
    Placeholder(&'a str),
}

/// One alternative of a row's `keys` as tokens.
pub(super) fn tokens(alt: &str) -> Vec<Tok<'_>> {
    let ev = |c| Tok::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    if let Some(&(_, code)) = NAMED.iter().find(|(n, _)| *n == alt) {
        return vec![Tok::Key(KeyEvent::new(code, KeyModifiers::NONE))];
    }
    let mut out = Vec::new();
    let mut rest = alt;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<leader>") {
            out.push(ev(LEADER));
            rest = r;
        } else if let Some(r) = rest.strip_prefix("C-")
            && let Some(c) = r.chars().next()
        {
            out.push(Tok::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            )));
            rest = r[c.len_utf8()..].trim_start();
        } else if rest.starts_with('{') && rest.len() > 2 {
            let end = rest.find('}').expect("closing }") + 1;
            out.push(Tok::Placeholder(&rest[..end]));
            rest = &rest[end..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(ev(c));
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A key that stands for a placeholder: the first character of the range.
pub(super) fn placeholder_key(p: &str) -> KeyEvent {
    let c = p[1..].chars().next().unwrap_or('1');
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// A key as the help shows it: `C-d`, `Esc`, `g`.
pub(super) fn show_key(key: &KeyEvent) -> String {
    if let Some(&(name, _)) = NAMED.iter().find(|(_, c)| *c == key.code) {
        return name.to_string();
    }
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => format!("C-{c}"),
        KeyCode::Char(LEADER) => "<leader>".to_string(),
        KeyCode::Char(c) => c.to_string(),
        other => format!("{other:?}"),
    }
}

/// Keys while the help overlay is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpAction {
    Open,
    Close,
    Down,
    Up,
    HalfDown,
    HalfUp,
    Top,
    Bottom,
    FilterStart,
    FilterInput(char),
    FilterBackspace,
    /// Enter while typing: keep the filter.
    FilterCommit,
    /// Esc while typing, or with a kept filter: clear it.
    FilterClear,
}

/// Help overlay state owned by [`App`].
#[derive(Debug, Clone, Default)]
pub(super) struct HelpState {
    scroll: usize,
    filter: String,
    filtering: bool,
}

/// One drawn help line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    Group(&'static str),
    Item { keys: String, desc: String },
}

/// What the UI draws for the help overlay.
#[derive(Debug, Clone)]
pub struct HelpView<'a> {
    pub lines: Vec<HelpLine>,
    pub scroll: usize,
    pub filter: &'a str,
    pub filtering: bool,
}

/// The overlay's rectangle inside `area`. Never larger than `area`.
pub fn help_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(1, 72).min(area.width);
    let height = area.height.saturating_sub(2).clamp(1, 30).min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// List rows inside a help rect: borders and the filter line excluded.
pub fn help_list_rows(rect: Rect) -> usize {
    rect.height.saturating_sub(3) as usize
}

/// Keys in the help overlay.
pub(super) fn help_keymap(app: &App, keys: &[KeyEvent]) -> KeyResult {
    use HelpAction as H;
    let act = |a| KeyResult::Action(Action::Help(a));
    let Some(key) = keys.last() else {
        return KeyResult::None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if app.help.filtering {
        return match key.code {
            KeyCode::Enter => act(H::FilterCommit),
            KeyCode::Esc => act(H::FilterClear),
            KeyCode::Backspace => act(H::FilterBackspace),
            KeyCode::Char(c) if !ctrl => act(H::FilterInput(c)),
            _ => KeyResult::None,
        };
    }
    if let [first, second] = keys
        && plain(first) == Some('g')
    {
        return match plain(second) {
            Some('g') => act(H::Top),
            Some('?') => act(H::Close),
            _ => KeyResult::None,
        };
    }
    if keys.len() != 1 {
        return KeyResult::None;
    }
    match key.code {
        KeyCode::Char('d') if ctrl => act(H::HalfDown),
        KeyCode::Char('u') if ctrl => act(H::HalfUp),
        _ if ctrl => KeyResult::None,
        KeyCode::Char('j') | KeyCode::Down => act(H::Down),
        KeyCode::Char('k') | KeyCode::Up => act(H::Up),
        KeyCode::Char('g') => KeyResult::Pending,
        KeyCode::Char('G') => act(H::Bottom),
        KeyCode::Char('/') => act(H::FilterStart),
        KeyCode::Char('q') => act(H::Close),
        KeyCode::Esc if !app.help.filter.is_empty() => act(H::FilterClear),
        KeyCode::Esc => act(H::Close),
        _ => KeyResult::None,
    }
}

impl App {
    /// Rows of `ctx` apply now (the help rows only describe the overlay).
    pub(super) fn ctx_shown(&self, ctx: Ctx) -> bool {
        match ctx {
            Ctx::Log => self.screen == Screen::Log,
            Ctx::Commit | Ctx::Files | Ctx::Visual | Ctx::Search | Ctx::Comment | Ctx::Popup => {
                matches!(self.screen, Screen::Commit(_))
            }
            Ctx::Picker | Ctx::Help => true,
        }
    }

    /// Rows of `ctx` apply to the keys typed now (the key clue).
    pub(super) fn ctx_active(&self, ctx: Ctx) -> bool {
        let commit = matches!(self.screen, Screen::Commit(_));
        match ctx {
            Ctx::Log => self.screen == Screen::Log && self.mode == Mode::Normal,
            Ctx::Commit => {
                commit && !self.files_focused() && matches!(self.mode, Mode::Normal | Mode::Visual)
            }
            Ctx::Files => commit && self.files_focused() && self.mode == Mode::Normal,
            Ctx::Visual => commit && self.mode == Mode::Visual,
            Ctx::Search => commit && self.mode == Mode::Search,
            Ctx::Comment => commit && self.mode == Mode::Comment,
            Ctx::Popup => commit && self.mode == Mode::Popup,
            Ctx::Picker => self.mode == Mode::Picker,
            Ctx::Help => false,
        }
    }

    /// Every help line for the current state, before the filter.
    pub fn help_lines(&self) -> Vec<HelpLine> {
        let mut out = Vec::new();
        for g in Group::ALL {
            let items: Vec<HelpLine> = self
                .bindings()
                .iter()
                .filter(|b| b.group == g && self.ctx_shown(b.context))
                .map(|b| HelpLine::Item {
                    keys: b.keys.to_string(),
                    desc: b.desc.to_string(),
                })
                .collect();
            if !items.is_empty() {
                out.push(HelpLine::Group(g.title()));
                out.extend(items);
            }
        }
        out
    }

    /// Help lines matching the filter (case-insensitive, over keys and
    /// description); a group heading stays when any of its rows match.
    fn help_filtered(&self) -> Vec<HelpLine> {
        let f = self.help.filter.to_lowercase();
        if f.is_empty() {
            return self.help_lines();
        }
        let mut out = Vec::new();
        let mut group = None;
        for line in self.help_lines() {
            match &line {
                HelpLine::Group(_) => group = Some(line),
                HelpLine::Item { keys, desc } => {
                    if keys.to_lowercase().contains(&f) || desc.to_lowercase().contains(&f) {
                        if let Some(g) = group.take() {
                            out.push(g);
                        }
                        out.push(line);
                    }
                }
            }
        }
        out
    }

    /// The open help overlay, for drawing and tests.
    pub fn help_view(&self) -> Option<HelpView<'_>> {
        if self.mode != Mode::Help {
            return None;
        }
        let lines = self.help_filtered();
        let scroll = self.help.scroll.min(self.help_max_scroll(lines.len()));
        Some(HelpView {
            lines,
            scroll,
            filter: &self.help.filter,
            filtering: self.help.filtering,
        })
    }

    fn help_page(&self) -> usize {
        let area = Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1));
        help_list_rows(help_rect(area))
    }

    fn help_max_scroll(&self, lines: usize) -> usize {
        lines.saturating_sub(self.help_page())
    }

    pub(super) fn help_action(&mut self, a: HelpAction) {
        use HelpAction as H;
        let max = self.help_max_scroll(self.help_filtered().len());
        let half = (self.help_page() / 2).max(1);
        let s = self.help.scroll.min(max);
        match a {
            H::Open => {
                self.help = HelpState::default();
                self.count = None;
                self.pending.clear();
                self.mode = Mode::Help;
            }
            H::Close => {
                self.help = HelpState::default();
                self.mode = Mode::Normal;
            }
            H::Down => self.help.scroll = (s + 1).min(max),
            H::Up => self.help.scroll = s.saturating_sub(1),
            H::HalfDown => self.help.scroll = (s + half).min(max),
            H::HalfUp => self.help.scroll = s.saturating_sub(half),
            H::Top => self.help.scroll = 0,
            H::Bottom => self.help.scroll = max,
            H::FilterStart => self.help.filtering = true,
            H::FilterInput(c) => {
                self.help.filter.push(c);
                self.help.scroll = 0;
            }
            H::FilterBackspace => {
                if self.help.filter.pop().is_none() {
                    self.help.filtering = false;
                }
                self.help.scroll = 0;
            }
            H::FilterCommit => self.help.filtering = false,
            H::FilterClear => {
                self.help.filter.clear();
                self.help.filtering = false;
                self.help.scroll = 0;
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::app::test_support::app;

    /// One alternative of a row's `keys` as key events.
    pub(crate) fn parse(alt: &str) -> Vec<KeyEvent> {
        tokens(alt)
            .into_iter()
            .map(|t| match t {
                Tok::Key(k) => k,
                Tok::Placeholder(p) => placeholder_key(p),
            })
            .collect()
    }

    pub(crate) fn in_ctx(ctx: Ctx) -> (tempfile::TempDir, App) {
        let (t, mut a) = app();
        if !matches!(ctx, Ctx::Log | Ctx::Help | Ctx::Picker) {
            a.open_index(0);
            assert!(!a.commit_rows().is_empty());
        }
        match ctx {
            Ctx::Help => a.help_action(HelpAction::Open),
            Ctx::Files => a.focus_pane(crate::app::Focus::Files),
            Ctx::Visual => a.visual_start(),
            Ctx::Search => a.search_start(true),
            // The keymaps depend only on the mode.
            Ctx::Comment => a.mode = Mode::Comment,
            Ctx::Popup => a.mode = Mode::Popup,
            Ctx::Picker => a.picker_open(),
            Ctx::Log | Ctx::Commit => {}
        }
        (t, a)
    }

    #[test]
    fn every_row_resolves_in_its_context_and_has_a_description() {
        let ctxs = [
            Ctx::Log,
            Ctx::Commit,
            Ctx::Files,
            Ctx::Visual,
            Ctx::Search,
            Ctx::Comment,
            Ctx::Popup,
            Ctx::Picker,
            Ctx::Help,
        ];
        let apps: Vec<_> = ctxs.iter().map(|&c| (c, in_ctx(c))).collect();
        for b in apps[0].1.1.bindings() {
            assert!(!b.desc.trim().is_empty(), "{} has no description", b.keys);
            let (_, (_t, a)) = apps.iter().find(|(c, _)| *c == b.context).unwrap();
            for alt in b.keys.split(", ") {
                let keys = parse(alt);
                let r = a.keymap(&keys);
                let ok = matches!(r, KeyResult::Action(_) | KeyResult::Count(_))
                    // C-c is handled before the keymap.
                    || alt == "C-c";
                assert!(ok, "{alt:?} ({:?}) resolved to {r:?}", b.context);
            }
        }
    }

    /// The reverse direction: every key (and two-key sequence) the keymap
    /// binds in each context has a help row there.
    #[test]
    fn every_key_has_a_row() {
        for ctx in [Ctx::Log, Ctx::Commit, Ctx::Files, Ctx::Visual, Ctx::Popup] {
            let (_t, a) = in_ctx(ctx);
            every_key_has_a_row_in(&a, ctx);
        }
    }

    fn every_key_has_a_row_in(a: &App, ctx: Ctx) {
        let mut keys: Vec<KeyEvent> = (' '..='~')
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .collect();
        keys.extend(('a'..='z').map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
        keys.extend(
            NAMED
                .iter()
                .map(|(_, c)| KeyEvent::new(*c, KeyModifiers::NONE)),
        );
        let listed: Vec<Vec<KeyEvent>> = a
            .bindings()
            .iter()
            // Visual mode keeps the diff pane's motions.
            .filter(|b| b.context == ctx || (ctx == Ctx::Visual && b.context == Ctx::Commit))
            .flat_map(|b| b.keys.split(", ").map(parse))
            .collect();
        for k in &keys {
            match a.keymap(&[*k]) {
                KeyResult::Action(_) | KeyResult::Count(_) => {
                    let digit = matches!(k.code, KeyCode::Char('1'..='9'));
                    assert!(
                        digit || listed.contains(&vec![*k]),
                        "{} has no help row in {ctx:?}",
                        show_key(k)
                    );
                }
                KeyResult::Pending => {
                    for k2 in &keys {
                        if let KeyResult::Action(_) = a.keymap(&[*k, *k2]) {
                            assert!(
                                listed.contains(&vec![*k, *k2]),
                                "{}{} has no help row in {ctx:?}",
                                show_key(k),
                                show_key(k2)
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn filter_keeps_matching_rows_under_their_group() {
        let (_t, mut a) = app();
        a.help_action(HelpAction::Open);
        for c in "reload".chars() {
            a.help_action(HelpAction::FilterInput(c));
        }
        let v = a.help_view().unwrap();
        assert_eq!(
            v.lines,
            [
                HelpLine::Group("Log"),
                HelpLine::Item {
                    keys: "r".into(),
                    desc: "reload the log".into()
                }
            ]
        );
    }
}
