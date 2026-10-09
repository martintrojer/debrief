//! Comments on diff lines (spec §3 "Comments"): normal `cc` on the cursor
//! row, visual `c` on the selection. Side, lines and excerpt are frozen
//! when `c` is pressed; a one-line prompt takes the body (Enter saves, Esc
//! cancels, `C-e` moves it into `$EDITOR`, Tab / S-Tab pick the kind from
//! `[review] kinds`), as in ramble.
//!
//! On the working copy (spec Q23, K11) new and context lines make
//! ramble-style comments: no revision, the new path, the line numbers of
//! the file on disk. Removed lines anchor to the base (git `HEAD`, jj
//! `@-`) on its old side and old path; a jj merge `@` has no single base,
//! so those are refused.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief_review::{Comment, Kind, Rev, Side};

use super::commit::{RowKind, file_path};
use super::keys::{Action, KeyResult};
use super::launch::Effect;
use super::{App, Mode};
use crate::vcs::{CommitInfo, DiffLine, FileDiff, LineKind};

/// The revision a comment on `info` records.
fn rev_of(info: &CommitInfo) -> Rev {
    Rev {
        commit: info.commit.clone(),
        change: info.change.clone(),
        summary: Some(info.summary.clone()),
    }
}

/// The prompt's label.
pub const COMMENT_PROMPT: &str = "comment: ";
/// Status when Enter (or the editor) leaves an empty body.
pub const EMPTY_COMMENT: &str = "Empty comment not saved";
/// Status when the selection holds no diff line.
pub const NOT_A_LINE: &str = "Comment on a diff line";
/// Status when the selection's diff lines are in more than one file.
pub const SPANS_FILES: &str = "Selection spans files";
/// Status for removed lines of a jj `@` with several parents.
pub const MERGE_OLD_SIDE: &str = "This working copy is a merge; comment on the new side";
/// Status for a path that resolves outside the repo (a symlinked dir).
pub const OUTSIDE_ROOT: &str = "The file is outside the repository";
/// The excerpt keeps at most this many selected diff lines.
pub const EXCERPT_LINES: usize = 40;

/// Comment-prompt actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentAction {
    /// `cc` (false: the cursor row) or visual `c` (true: the selection).
    Start(bool),
    Input(char),
    Backspace,
    /// Enter: save a non-empty body.
    Save,
    /// Esc: drop the comment.
    Cancel,
    /// `C-e`: edit the body in `$VISUAL` / `$EDITOR`.
    Editor,
    /// Tab (true) / S-Tab: the next / previous kind, through untyped.
    Kind(bool),
}

/// A comment being typed: where it goes, frozen at `c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub path: String,
    /// `None`: a working-copy comment (as ramble makes them).
    pub rev: Option<Rev>,
    /// `None` with `rev`: lines of the file on disk.
    pub side: Option<Side>,
    pub lines: (u32, u32),
    pub excerpt: String,
    pub text: String,
    /// The kind id; new comments start untyped.
    pub kind: Option<String>,
}

/// Keys while typing a comment.
pub(super) fn comment_keymap(keys: &[KeyEvent]) -> KeyResult {
    let Some(key) = keys.last() else {
        return KeyResult::None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let a = match key.code {
        KeyCode::Char('e') if ctrl => CommentAction::Editor,
        KeyCode::Enter => CommentAction::Save,
        KeyCode::Esc => CommentAction::Cancel,
        KeyCode::Backspace => CommentAction::Backspace,
        KeyCode::Tab | KeyCode::Char('\t') => CommentAction::Kind(true),
        KeyCode::BackTab => CommentAction::Kind(false),
        KeyCode::Char(c) if !ctrl => CommentAction::Input(c),
        _ => return KeyResult::None,
    };
    KeyResult::Action(Action::Comment(a))
}

impl App {
    /// The typed comment as the prompt shows it, its kind in the label
    /// (`comment [ISSUE]: `).
    pub fn comment_prompt(&self) -> Option<String> {
        let d = self.draft.as_ref()?;
        Some(match &d.kind {
            Some(k) => format!("comment [{}]: {}", Kind::label(k), d.text),
            None => format!("{COMMENT_PROMPT}{}", d.text),
        })
    }

    /// `[review] kinds` lists some (Tab cycles them).
    pub(super) fn has_comment_kinds(&self) -> bool {
        !self.config.review.kinds.is_empty()
    }

    /// Tab (`forward`) / S-Tab: move the draft's kind along untyped ->
    /// `[review] kinds` in order -> untyped. No kinds: nothing happens.
    fn comment_cycle_kind(&mut self, forward: bool) {
        let kinds = &self.config.review.kinds;
        let Some(d) = &mut self.draft else { return };
        if kinds.is_empty() {
            return;
        }
        // Positions 0..n are the kinds, n is untyped.
        let n = kinds.len();
        let at = d
            .kind
            .as_deref()
            .and_then(|k| kinds.iter().position(|i| i.id == k))
            .unwrap_or(n);
        let next = if forward {
            (at + 1) % (n + 1)
        } else {
            (at + n) % (n + 1)
        };
        d.kind = kinds.get(next).map(|k| k.id.clone());
    }

    /// The comment being typed.
    pub fn comment_draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// Pasted text (one line) appended to the draft.
    pub(super) fn comment_insert(&mut self, text: &str) {
        if let Some(d) = &mut self.draft {
            d.text.push_str(text);
        }
    }

    pub(super) fn comment_action(&mut self, a: CommentAction) {
        match a {
            CommentAction::Start(visual) => self.comment_start(visual),
            CommentAction::Input(c) => {
                if let Some(d) = &mut self.draft {
                    d.text.push(c);
                }
            }
            CommentAction::Backspace => {
                if let Some(d) = &mut self.draft {
                    d.text.pop();
                }
            }
            CommentAction::Kind(forward) => self.comment_cycle_kind(forward),
            CommentAction::Cancel => {
                self.comment_end();
            }
            CommentAction::Save => {
                if let Some(d) = self.comment_end() {
                    self.comment_save(d);
                }
            }
            CommentAction::Editor => {
                if let Some(d) = self.comment_end() {
                    self.effect = Some(Effect::EditComment(d));
                }
            }
        }
    }

    fn comment_end(&mut self) -> Option<Draft> {
        if self.mode == Mode::Comment {
            self.mode = Mode::Normal;
        }
        self.draft.take()
    }

    /// Freeze the target side, lines and excerpt, then open the prompt.
    fn comment_start(&mut self, visual: bool) {
        let cur = self.commit_cursor().row;
        let (lo, hi) = match (visual, self.selection_rows()) {
            (true, Some(rows)) => rows,
            _ => (cur, cur),
        };
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
        match self.draft_for(lo, hi) {
            Ok(d) => {
                self.draft = Some(d);
                self.status.clear();
                self.mode = Mode::Comment;
            }
            Err(msg) => self.set_status(msg),
        }
    }

    /// The draft for diff rows `lo..=hi` (spec §3, D9): one file's Line
    /// rows; the new side when any is added or context, else the old side;
    /// the excerpt is the first [`EXCERPT_LINES`] of them as diff text.
    fn draft_for(&self, lo: usize, hi: usize) -> Result<Draft, String> {
        let (Some(v), Some(diff)) = (self.commit.as_ref(), self.commit_diff()) else {
            return Err(NOT_A_LINE.into());
        };
        let rows = &v.rows[lo.min(v.rows.len())..(hi + 1).min(v.rows.len())];
        let lines: Vec<(usize, &DiffLine)> = rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Line(_)))
            .filter_map(|r| {
                let (f, h, l) = (r.file?, r.hunk?, r.line?);
                Some((f, diff.files.get(f)?.hunks.get(h)?.lines.get(l)?))
            })
            .collect();
        let Some(&(file, _)) = lines.first() else {
            return Err(NOT_A_LINE.into());
        };
        if lines.iter().any(|&(f, _)| f != file) {
            return Err(SPANS_FILES.into());
        }
        let new_side = lines.iter().any(|(_, l)| l.kind != LineKind::Removed);
        let (side, nums): (Side, Vec<u32>) = if new_side {
            (Side::New, lines.iter().filter_map(|(_, l)| l.new).collect())
        } else {
            (Side::Old, lines.iter().filter_map(|(_, l)| l.old).collect())
        };
        let (Some(&a), Some(&b)) = (nums.iter().min(), nums.iter().max()) else {
            return Err(NOT_A_LINE.into());
        };
        let excerpt: Vec<String> = lines
            .iter()
            .take(EXCERPT_LINES)
            .map(|(_, l)| {
                let sign = match l.kind {
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                    LineKind::Context => ' ',
                };
                format!("{sign}{}", l.text)
            })
            .collect();
        let f = &diff.files[file];
        let excerpt = excerpt.join("\n");
        if v.working {
            return self.working_draft(f, side, (a, b), excerpt);
        }
        let info = self
            .rows
            .iter()
            .find(|r| r.info.commit == v.commit)
            .map(|r| &r.info)
            .ok_or("The commit is no longer in the log")?;
        Ok(Draft {
            path: file_path(f).to_string(),
            rev: Some(rev_of(info)),
            side: Some(side),
            lines: (a, b),
            excerpt,
            text: String::new(),
            kind: None,
        })
    }

    /// A draft on the working copy (Q23, K11).
    fn working_draft(
        &self,
        f: &FileDiff,
        side: Side,
        lines: (u32, u32),
        excerpt: String,
    ) -> Result<Draft, String> {
        let draft = |path: &str, rev, side| Draft {
            path: path.to_string(),
            rev,
            side,
            lines,
            excerpt: excerpt.clone(),
            text: String::new(),
            kind: None,
        };
        if side == Side::New {
            let path = f.new_path.as_deref().ok_or(NOT_A_LINE)?;
            self.inside_root(path)?;
            return Ok(draft(path, None, None));
        }
        let path = f.old_path.as_deref().ok_or(NOT_A_LINE)?;
        let base = self.working_base()?;
        Ok(draft(path, Some(rev_of(&base)), Some(Side::Old)))
    }

    /// The working copy's base commit: git's `HEAD`, jj's single `@-`.
    pub(super) fn working_base(&self) -> Result<CommitInfo, String> {
        let v = self.commit.as_ref().ok_or(NOT_A_LINE)?;
        if let Some(b) = &v.base {
            return Ok(b.clone());
        }
        let at = self
            .rows
            .iter()
            .find(|r| r.working && r.info.commit == v.commit)
            .ok_or("The working copy is no longer in the log")?;
        let parent = match &at.info.parents[..] {
            [one] => one.clone(),
            [] => return Err("The working copy has no parent".into()),
            _ => return Err(MERGE_OLD_SIDE.into()),
        };
        if let Some(r) = self.rows.iter().find(|r| r.info.commit == parent) {
            return Ok(r.info.clone());
        }
        self.vcs
            .log(Some(&parent), 1)
            .ok()
            .and_then(|l| l.into_iter().next())
            .ok_or_else(|| "Cannot read the working copy's parent".to_string())
    }

    /// `Err` when `path` (repo-relative) resolves outside the repo root.
    pub(super) fn inside_root(&self, path: &str) -> Result<(), String> {
        let root = self.vcs.root();
        let abs = root.join(path);
        // The deepest existing ancestor decides (the file may be gone).
        let Some(existing) = abs.ancestors().find(|p| p.exists()) else {
            return Ok(());
        };
        let (Ok(c), Ok(r)) = (existing.canonicalize(), root.canonicalize()) else {
            return Ok(());
        };
        if c.starts_with(&r) {
            Ok(())
        } else {
            Err(OUTSIDE_ROOT.into())
        }
    }

    /// Save `d` with its text (trimmed); an empty body saves nothing.
    pub(super) fn comment_save(&mut self, d: Draft) {
        let body = d.text.trim();
        if body.is_empty() {
            return self.set_status(EMPTY_COMMENT);
        }
        let Some(review) = &mut self.review else {
            return self.set_status(super::review::NO_BATCH);
        };
        let (a, b) = d.lines;
        let c = match (d.rev, d.side) {
            (Some(rev), Some(side)) => Comment::on_diff(d.path, rev, side, a..=b, d.excerpt, body),
            _ => Comment::on_file(d.path, a..=b, d.excerpt, body),
        }
        .with_kind(d.kind);
        let msg = match review.add(c) {
            Ok(_) => format!("Comment added ({} in batch)", review.comments().len()),
            Err(e) => format!("comment not saved: {e}"),
        };
        self.set_status(msg);
    }

    /// `C-e` (TUI suspended): edit the text in the editor, save what it
    /// leaves.
    pub(super) fn run_comment_editor(&mut self, d: Draft) {
        match self.edit_in_editor(&d.text) {
            Ok(text) => self.comment_save(Draft { text, ..d }),
            Err(e) => self.set_status(format!("editor: {e}; comment not saved")),
        }
    }
}
