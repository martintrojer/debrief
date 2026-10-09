//! The `g?` help overlay (spec §3 "Help"): every binding that works now,
//! grouped, scrollable and filterable.
//!
//! Dispatch stays in the `match`-based keymaps; [`BINDINGS`] describes
//! them. A unit test resolves every row through [`App::keymap`] in its
//! context, so a row that no longer matches a binding fails the build.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::keys::{Action, KeyResult, plain};
use super::{App, Mode, Screen};

/// Where a binding works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctx {
    /// The log screen.
    Log,
    /// The help overlay is open.
    Help,
}

/// Help sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    General,
    Motions,
    Log,
    Help,
}

impl Group {
    const ALL: [Group; 4] = [Group::General, Group::Motions, Group::Log, Group::Help];

    pub(super) fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Motions => "Motions",
            Group::Log => "Log",
            Group::Help => "Help",
        }
    }
}

/// One help row. `keys` lists alternatives separated by `", "`; `C-x` is
/// Ctrl+x, `{1-9}` a count digit, and `Enter Esc Down Up` name keys.
pub(crate) struct Binding {
    pub keys: &'static str,
    pub context: Ctx,
    pub group: Group,
    pub desc: &'static str,
}

const fn b(keys: &'static str, context: Ctx, group: Group, desc: &'static str) -> Binding {
    Binding {
        keys,
        context,
        group,
        desc,
    }
}

/// Every key binding, as the help overlay and the key clue list them.
pub(crate) const BINDINGS: &[Binding] = {
    use Ctx::{Help as H, Log as L};
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
        b("j, Down", H, G::Help, "in help: scroll down"),
        b("k, Up", H, G::Help, "in help: scroll up"),
        b("C-d, C-u", H, G::Help, "in help: half page"),
        b("gg, G", H, G::Help, "in help: top, bottom"),
        b("/", H, G::Help, "in help: filter"),
        b("q, g?, Esc", H, G::Help, "in help: close"),
    ]
};

/// Named keys of the `keys` notation, as whole alternatives.
pub(super) const NAMED: [(&str, KeyCode); 4] = [
    ("Enter", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Down", KeyCode::Down),
    ("Up", KeyCode::Up),
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
        if let Some(r) = rest.strip_prefix("C-")
            && let Some(c) = r.chars().next()
        {
            out.push(Tok::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            )));
            rest = &r[c.len_utf8()..];
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
            Ctx::Log => matches!(self.screen, Screen::Log | Screen::Commit(_)),
            Ctx::Help => true,
        }
    }

    /// Every help line for the current state, before the filter.
    pub fn help_lines(&self) -> Vec<HelpLine> {
        let mut out = Vec::new();
        for g in Group::ALL {
            let items: Vec<HelpLine> = BINDINGS
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

    fn in_ctx(ctx: Ctx) -> (tempfile::TempDir, App) {
        let (t, mut a) = app();
        if ctx == Ctx::Help {
            a.help_action(HelpAction::Open);
        }
        (t, a)
    }

    #[test]
    fn every_row_resolves_in_its_context_and_has_a_description() {
        for b in BINDINGS {
            assert!(!b.desc.trim().is_empty(), "{} has no description", b.keys);
            let (_t, a) = in_ctx(b.context);
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

    /// The reverse direction: every single key the log keymap binds has
    /// a help row.
    #[test]
    fn every_log_key_has_a_row() {
        let (_t, a) = app();
        let mut keys: Vec<KeyEvent> = (' '..='~')
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .collect();
        keys.extend(('a'..='z').map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
        keys.extend(
            NAMED
                .iter()
                .map(|(_, c)| KeyEvent::new(*c, KeyModifiers::NONE)),
        );
        let listed: Vec<Vec<KeyEvent>> = BINDINGS
            .iter()
            .filter(|b| b.context == Ctx::Log)
            .flat_map(|b| b.keys.split(", ").map(parse))
            .collect();
        for k in &keys {
            match a.keymap(&[*k]) {
                KeyResult::Action(_) | KeyResult::Count(_) => {
                    let digit = matches!(k.code, KeyCode::Char('1'..='9'));
                    assert!(
                        digit || listed.contains(&vec![*k]),
                        "{} has no help row",
                        show_key(k)
                    );
                }
                KeyResult::Pending => {
                    for k2 in &keys {
                        if let KeyResult::Action(_) = a.keymap(&[*k, *k2]) {
                            assert!(
                                listed.contains(&vec![*k, *k2]),
                                "{}{} has no help row",
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
