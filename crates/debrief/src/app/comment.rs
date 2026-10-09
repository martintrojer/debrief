//! Comments on diff lines (spec §3 "Comments"): normal `cc` on the cursor
//! row, visual `c` on the selection. Side, lines and excerpt are frozen
//! when `c` is pressed; a one-line prompt takes the body (Enter saves, Esc
//! cancels, `C-e` moves it into `$EDITOR`), as in ramble.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief_review::{Comment, Rev, Side};

use super::commit::{RowKind, file_path};
use super::keys::{Action, KeyResult};
use super::launch::Effect;
use super::{App, Mode};
use crate::vcs::{DiffLine, LineKind};

/// The prompt's label.
pub const COMMENT_PROMPT: &str = "comment: ";
/// Status when Enter (or the editor) leaves an empty body.
pub const EMPTY_COMMENT: &str = "Empty comment not saved";
/// Status when the selection holds no diff line.
pub const NOT_A_LINE: &str = "Comment on a diff line";
/// Status when the selection's diff lines are in more than one file.
pub const SPANS_FILES: &str = "Selection spans files";
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
}

/// A comment being typed: where it goes, frozen at `c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub path: String,
    pub rev: Rev,
    pub side: Side,
    pub lines: (u32, u32),
    pub excerpt: String,
    pub text: String,
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
        KeyCode::Char(c) if !ctrl => CommentAction::Input(c),
        _ => return KeyResult::None,
    };
    KeyResult::Action(Action::Comment(a))
}

impl App {
    /// The typed comment as the prompt shows it.
    pub fn comment_prompt(&self) -> Option<String> {
        let d = self.draft.as_ref()?;
        Some(format!("{COMMENT_PROMPT}{}", d.text))
    }

    /// The comment being typed.
    pub fn comment_draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
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
        let info = self
            .rows
            .iter()
            .find(|r| r.info.commit == v.commit)
            .map(|r| &r.info)
            .ok_or("The commit is no longer in the log")?;
        Ok(Draft {
            path: file_path(&diff.files[file]).to_string(),
            rev: Rev {
                commit: info.commit.clone(),
                change: info.change.clone(),
                summary: Some(info.summary.clone()),
            },
            side,
            lines: (a, b),
            excerpt: excerpt.join("\n"),
            text: String::new(),
        })
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
        let c = Comment::on_diff(d.path, d.rev, d.side, a..=b, d.excerpt, body);
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
