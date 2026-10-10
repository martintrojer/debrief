//! The key clue: after a pause in a pending key sequence, a box lists the
//! keys that can come next. The rows come from `help::BINDINGS`, so they
//! never drift from the keymap.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::help::{Tok, placeholder_key, show_key, tokens};
use super::keys::KeyResult;
use super::{App, Mode};

/// How long a sequence must pause before the clue opens.
pub const CLUE_DELAY: Duration = Duration::from_millis(500);

/// Timing of the clue box, owned by [`App`].
#[derive(Debug, Clone, Default)]
pub(super) struct ClueState {
    /// When the current sequence started.
    since: Option<Instant>,
    /// The pause has passed; stays set while the sequence grows.
    shown: bool,
}

/// One line of the clue box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClueRow {
    pub key: String,
    pub desc: String,
    /// More keys follow this one.
    pub group: bool,
}

fn same_key(a: &KeyEvent, b: &KeyEvent) -> bool {
    let ctrl = |k: &KeyEvent| k.modifiers.contains(KeyModifiers::CONTROL);
    a.code == b.code && ctrl(a) == ctrl(b)
}

impl App {
    fn clue_active(&self) -> bool {
        self.config.keys.clue
            && !self.pending.is_empty()
            && matches!(self.mode, Mode::Normal | Mode::Visual)
    }

    /// Called after every key: start timing a new sequence, or forget the
    /// one that ended.
    pub(super) fn clue_sync(&mut self) {
        if !self.clue_active() {
            self.clue = ClueState::default();
        } else if self.clue.since.is_none() {
            self.clue.since = Some(Instant::now());
        }
    }

    /// Open the box once the pause is long enough.
    pub(super) fn clue_tick(&mut self, now: Instant) {
        if !self.clue_active() {
            self.clue = ClueState::default();
        } else if let Some(since) = self.clue.since
            && now.saturating_duration_since(since) >= CLUE_DELAY
        {
            self.clue.shown = true;
        }
    }

    /// Pretend the pending sequence started at `at` (tests).
    pub fn clue_started_at(&mut self, at: Instant) {
        if self.clue_active() {
            self.clue.since = Some(at);
        }
    }

    /// Whether the clue box is drawn now.
    pub fn clue_visible(&self) -> bool {
        self.clue.shown && self.clue_active() && !self.clue_rows().is_empty()
    }

    /// The typed keys, as the box title.
    pub fn clue_title(&self) -> String {
        let keys: Vec<String> = self.pending.iter().map(show_key).collect();
        keys.join(" ")
    }

    /// The keys that can follow the pending sequence right now.
    pub fn clue_rows(&self) -> Vec<ClueRow> {
        if !self.clue_active() {
            return Vec::new();
        }
        let mut rows = self.clue_rows_for(&self.pending);
        // After `C-w`, name the one-key forms of `h` and `l` (`pane_nav`).
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        if self.config.keys.pane_nav && matches!(&self.pending[..], [k] if same_key(k, &ctrl_w)) {
            for r in &mut rows {
                if matches!(r.key.as_str(), "h" | "l") && !r.group {
                    r.desc = format!("{} (or C-{})", r.desc, r.key);
                }
            }
        }
        rows
    }

    fn clue_rows_for(&self, pending: &[KeyEvent]) -> Vec<ClueRow> {
        let mut out: Vec<ClueRow> = Vec::new();
        for b in self.bindings() {
            if !self.ctx_active(b.context) {
                continue;
            }
            for alt in b.keys.split(", ") {
                let toks = tokens(alt);
                let starts = toks.len() > pending.len()
                    && pending.iter().zip(&toks).all(|(k, t)| match t {
                        Tok::Key(e) => same_key(k, e),
                        Tok::Placeholder(_) => true,
                    });
                if !starts {
                    continue;
                }
                let keys: Vec<KeyEvent> = toks
                    .iter()
                    .map(|t| match t {
                        Tok::Key(k) => *k,
                        Tok::Placeholder(p) => placeholder_key(p),
                    })
                    .collect();
                if matches!(self.keymap(&keys), KeyResult::None) {
                    continue;
                }
                let next = &toks[pending.len()];
                let key = match next {
                    Tok::Key(k) => show_key(k),
                    Tok::Placeholder(p) => p.to_string(),
                };
                let group = toks.len() > pending.len() + 1;
                if out.iter().any(|r| r.key == key) {
                    continue;
                }
                out.push(ClueRow {
                    key,
                    desc: if group {
                        "+more".into()
                    } else {
                        b.desc.to_string()
                    },
                    group,
                });
            }
        }
        let class = |r: &ClueRow| {
            let plain = r.key.chars().count() == 1;
            (!plain, r.key.clone())
        };
        out.sort_by_key(class);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::test_support::app;

    fn ch(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn shown_exactly_at_the_delay() {
        let (_t, mut a) = app();
        a.key(ch('g'));
        let t0 = Instant::now();
        a.clue_started_at(t0);
        a.tick(t0 + Duration::from_millis(499));
        assert!(!a.clue_visible(), "not yet at 499 ms");
        a.tick(t0 + CLUE_DELAY);
        assert!(a.clue_visible(), "shown at 500 ms");
        assert_eq!(a.clue_title(), "g");
        let keys: Vec<_> = a.clue_rows().into_iter().map(|r| r.key).collect();
        assert_eq!(keys, ["?", "g"]);
        a.key(ch('g'));
        assert!(!a.clue_visible(), "the sequence ended");
    }

    /// Every prefix the keymap answers `Pending` for has clue rows, on
    /// the log, the diff, the file list and in visual mode.
    #[test]
    fn every_pending_prefix_has_clue_rows() {
        use crate::app::help::{Ctx, tests::in_ctx};
        for ctx in [Ctx::Log, Ctx::Commit, Ctx::Files, Ctx::Visual] {
            let (_t, a) = in_ctx(ctx);
            pending_prefixes_have_clue_rows(&a);
        }
    }

    #[test]
    fn commit_clue_lists_the_bracket_keys() {
        use crate::app::help::{Ctx, tests::in_ctx};
        let (_t, mut a) = in_ctx(Ctx::Commit);
        a.key(ch(']'));
        let rows = a.clue_rows();
        let keys: Vec<_> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["]", "c", "f", "r"]);
        assert_eq!(rows[2].desc, "next file");
    }

    #[test]
    fn after_c_w_the_clue_names_c_h_and_c_l_while_pane_nav_is_on() {
        use crate::app::help::{Ctx, tests::in_ctx};
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let (_t, mut a) = in_ctx(Ctx::Commit);
        a.key(ctrl_w);
        let rows = a.clue_rows();
        let descs: Vec<_> = rows
            .iter()
            .map(|r| (r.key.as_str(), r.desc.as_str()))
            .collect();
        assert_eq!(
            descs,
            [
                ("h", "to the pane on the left (or C-h)"),
                ("l", "to the pane on the right (or C-l)")
            ]
        );
        let (_t, mut a) = in_ctx(Ctx::Commit);
        a.config.keys.pane_nav = false;
        a.key(ctrl_w);
        assert_eq!(a.clue_rows()[0].desc, "to the pane on the left");
    }

    fn pending_prefixes_have_clue_rows(a: &App) {
        let mut keys: Vec<KeyEvent> = (' '..='~').map(ch).collect();
        keys.extend(('a'..='z').map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
        let mut checked = 0;
        for k in &keys {
            if a.keymap(&[*k]) == KeyResult::Pending {
                checked += 1;
                assert!(
                    !a.clue_rows_for(&[*k]).is_empty(),
                    "no clue rows after {}",
                    show_key(k)
                );
            }
        }
        assert!(checked >= 1);
    }
}
