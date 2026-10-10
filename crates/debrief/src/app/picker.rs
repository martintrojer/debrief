//! `<leader>rl`: a picker over the whole batch (ramble's comments too),
//! and `<leader>rr`: the hand-back (spec §3 "Comments", AMEND H).
//!
//! Typed text filters (case-insensitive substring over label and body);
//! `C-n`/`C-p` or Down/Up move; Enter goes to the comment; `C-d` removes
//! it; `C-e` edits its body in `$EDITOR`; Esc closes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use debrief_review::Comment;
use debrief_review::handback::{self, Sent};

use super::keys::{Action, KeyResult};
use super::launch::Effect;
use super::{App, Mode, Screen};

/// The picker's title.
pub const PICKER_TITLE: &str = "Review comments";
/// The keys shown under the list.
pub const PICKER_FOOTER: &str = "Enter go  C-d remove  C-e edit  Esc close";
/// Status when `<leader>rr` finds an empty batch.
pub const NOTHING_TO_SEND: &str = "No comments to send";
/// Status when Enter picks a diff comment whose commit isn't listed.
pub const NOT_IN_LOG: &str = "Comment's revision is not in the log";

/// Keys while the picker is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerAction {
    Input(char),
    Backspace,
    Next,
    Prev,
    Accept,
    Close,
    Remove,
    Edit,
}

/// One row of the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub id: u64,
    /// `path:lines` and, for diff comments, ` @<short commit>` and the side.
    pub label: String,
    /// The kind (`[ISSUE] `) and the body's first line.
    pub detail: String,
}

/// Picker state owned by [`App`].
#[derive(Debug, Clone, Default)]
pub(super) struct Picker {
    items: Vec<PickerItem>,
    /// Indices into `items` matching `input`.
    filtered: Vec<usize>,
    selected: usize,
    input: String,
}

/// What the UI draws for the open picker.
#[derive(Debug, Clone)]
pub struct PickerView<'a> {
    pub title: &'static str,
    pub input: &'a str,
    pub items: Vec<&'a PickerItem>,
    pub selected: usize,
    pub footer: &'static str,
}

impl Picker {
    fn refilter(&mut self) {
        let q = self.input.to_lowercase();
        self.filtered = (0..self.items.len())
            .filter(|&i| {
                let it = &self.items[i];
                q.is_empty()
                    || it.label.to_lowercase().contains(&q)
                    || it.detail.to_lowercase().contains(&q)
            })
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    fn selected_id(&self) -> Option<u64> {
        self.filtered.get(self.selected).map(|&i| self.items[i].id)
    }
}

fn item(c: &Comment) -> PickerItem {
    let (a, b) = c.lines;
    let lines = if a == b {
        a.to_string()
    } else {
        format!("{a}-{b}")
    };
    let rev = c.rev.as_ref().map_or(String::new(), |r| {
        let short: String = r.commit.chars().take(8).collect();
        let side = match c.side {
            Some(debrief_review::Side::Old) => " (old)",
            _ => "",
        };
        format!(" @{short}{side}")
    });
    PickerItem {
        id: c.id,
        label: format!("{}:{lines}{rev}", c.path),
        detail: format!(
            "{}{}",
            super::review::kind_tag(c),
            c.body.lines().next().unwrap_or("")
        ),
    }
}

/// Keys in the picker: typed text filters, no counts.
pub(super) fn picker_keymap(keys: &[KeyEvent]) -> KeyResult {
    use PickerAction as P;
    let Some(key) = keys.last() else {
        return KeyResult::None;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let a = match key.code {
        KeyCode::Char('n') if ctrl => P::Next,
        KeyCode::Char('p') if ctrl => P::Prev,
        KeyCode::Char('d') if ctrl => P::Remove,
        KeyCode::Char('e') if ctrl => P::Edit,
        KeyCode::Down => P::Next,
        KeyCode::Up => P::Prev,
        KeyCode::Enter => P::Accept,
        KeyCode::Esc => P::Close,
        KeyCode::Backspace => P::Backspace,
        KeyCode::Char(c) if !ctrl => P::Input(c),
        _ => return KeyResult::None,
    };
    KeyResult::Action(Action::Picker(a))
}

impl App {
    /// The open picker, for drawing and tests.
    pub fn picker(&self) -> Option<PickerView<'_>> {
        let p = self.picker.as_ref()?;
        Some(PickerView {
            title: PICKER_TITLE,
            input: &p.input,
            items: p.filtered.iter().map(|&i| &p.items[i]).collect(),
            selected: p.selected,
            footer: PICKER_FOOTER,
        })
    }

    /// `<leader>rl`: re-read the batch and list it.
    pub(super) fn picker_open(&mut self) {
        if self.review.is_none() {
            return self.set_status(super::review::NO_BATCH);
        }
        self.review_reload();
        self.picker = Some(Picker::default());
        self.picker_refresh();
        self.mode = Mode::Picker;
    }

    /// Rebuild the items from the batch, keeping the filter and, as near
    /// as it can, the selection.
    pub(super) fn picker_refresh(&mut self) {
        let items: Vec<PickerItem> = self
            .review
            .as_ref()
            .map(|r| r.comments().iter().map(item).collect())
            .unwrap_or_default();
        if let Some(p) = &mut self.picker {
            p.items = items;
            p.refilter();
        }
    }

    fn picker_close(&mut self) {
        self.picker = None;
        if self.mode == Mode::Picker {
            self.mode = Mode::Normal;
        }
    }

    /// Pasted text (one line) appended to the filter.
    pub(super) fn picker_insert(&mut self, text: &str) {
        if let Some(p) = &mut self.picker {
            p.input.push_str(text);
            p.selected = 0;
            p.refilter();
        }
    }

    pub(super) fn picker_action(&mut self, a: PickerAction) {
        let Some(p) = &mut self.picker else {
            self.mode = Mode::Normal;
            return;
        };
        match a {
            PickerAction::Input(c) => {
                p.input.push(c);
                p.selected = 0;
                p.refilter();
            }
            PickerAction::Backspace => {
                p.input.pop();
                p.refilter();
            }
            PickerAction::Next if !p.filtered.is_empty() => {
                p.selected = (p.selected + 1) % p.filtered.len();
            }
            PickerAction::Prev if !p.filtered.is_empty() => {
                p.selected = (p.selected + p.filtered.len() - 1) % p.filtered.len();
            }
            PickerAction::Next | PickerAction::Prev => {}
            PickerAction::Close => self.picker_close(),
            PickerAction::Accept => {
                let id = p.selected_id();
                self.picker_close();
                if let Some(id) = id {
                    self.review_goto(id);
                }
            }
            PickerAction::Remove => {
                let Some(id) = p.selected_id() else { return };
                let Some(r) = &mut self.review else { return };
                let msg = match r.remove(id) {
                    Ok(_) => format!("Comment removed ({} in batch)", r.comments().len()),
                    Err(e) => format!("comment not removed: {e}"),
                };
                self.set_status(msg);
                self.picker_refresh();
            }
            PickerAction::Edit => {
                let Some(id) = p.selected_id() else { return };
                let body = self
                    .review
                    .as_ref()
                    .and_then(|r| r.comments().iter().find(|c| c.id == id))
                    .map(|c| c.body.clone());
                if let Some(initial) = body {
                    self.effect = Some(Effect::EditReviewComment { id, initial });
                }
            }
        }
    }

    /// Enter in the picker (D8). A working-copy comment on an existing
    /// `.md` file goes to ramble; other working-copy comments only say
    /// where they are. A diff comment opens its exact commit at its first
    /// line, if that commit is in the log; another version of the change
    /// is never substituted.
    fn review_goto(&mut self, id: u64) {
        let Some(c) = self
            .review
            .as_ref()
            .and_then(|r| r.comments().iter().find(|c| c.id == id))
            .cloned()
        else {
            return self.set_status("Comment is gone");
        };
        let Some(rev) = &c.rev else {
            if c.path.ends_with(".md") && self.vcs.root().join(&c.path).is_file() {
                return self.launch_md(&c.path);
            }
            return self.set_status(format!("Working-copy comment: {}:{}", c.path, c.lines.0));
        };
        // A kept log filter hiding the commit is cleared.
        if self.unfilter_for(&rev.commit) {
            self.commit_reindex();
        }
        let Some(i) = self.rows.iter().position(|r| r.info.commit == rev.commit) else {
            return self.set_status(NOT_IN_LOG);
        };
        let open =
            matches!(self.screen, Screen::Commit(_)) && self.commit_id() == Some(&rev.commit);
        if !open {
            self.open_index(i);
        }
        match &mut self.commit {
            Some(v) if v.loading => v.goto_comment = Some(c),
            _ => self.goto_comment(&c),
        }
    }

    /// Put the cursor on the first row of `c` in the open commit (its file
    /// header when no row shows its lines).
    pub(super) fn goto_comment(&mut self, c: &Comment) {
        // Side by side: the comment's side.
        let want = c.side.unwrap_or(debrief_review::Side::New);
        if let Some(v) = &mut self.commit
            && v.split_side().is_some_and(|s| s != want)
        {
            v.swap_side();
            self.refresh_search();
        }
        let n = self.commit_rows().len();
        let row = (0..n).find(|&r| self.row_in(r, c)).or_else(|| {
            let diff = self.commit_diff()?;
            let f = diff.files.iter().position(|f| {
                super::commit::file_path(f) == c.path
                    || f.old_path.as_deref() == Some(c.path.as_str())
            })?;
            self.commit.as_ref()?.header_row(f)
        });
        if let (Some(row), Some(v)) = (row, &mut self.commit) {
            v.goto_row(row);
            self.commit_follow();
        }
    }

    /// `C-e` in the picker (TUI suspended): edit comment `id`'s body. An
    /// emptied body or a failing editor changes nothing.
    pub(super) fn run_review_comment_editor(&mut self, id: u64, initial: &str) {
        let text = match self.edit_in_editor(initial) {
            Ok(t) => t,
            Err(e) => return self.set_status(format!("editor: {e}; comment not changed")),
        };
        let body = text.trim();
        if body.is_empty() {
            return self.set_status("Empty comment not saved (C-d removes it)");
        }
        let Some(r) = &mut self.review else { return };
        let msg = match r.edit(id, body) {
            Ok(true) => "Comment edited".to_string(),
            Ok(false) => "Comment is gone".to_string(),
            Err(e) => format!("comment not changed: {e}"),
        };
        self.set_status(msg);
        self.picker_refresh();
    }

    /// `<leader>rr`: queue the hand-back unless there is nothing to send.
    pub(super) fn review_send(&mut self) {
        self.review_reload();
        match &self.review {
            None => self.set_status(super::review::NO_BATCH),
            Some(r) if r.comments().is_empty() => self.set_status(NOTHING_TO_SEND),
            Some(_) => self.effect = Some(Effect::SendReview),
        }
    }

    /// The hand-back (TUI suspended): re-read and export the batch, pipe it
    /// to `[send] command` (falling back to the clipboard), then remove
    /// exactly the exported comments. Comments added meanwhile stay; a
    /// failed hand-back keeps everything.
    pub(super) fn run_review_send(&mut self) {
        let Some(r) = &mut self.review else { return };
        if let Err(e) = r.reload() {
            return self.set_status(format!("Review not sent: {e}"));
        }
        let ids: Vec<u64> = r.comments().iter().map(|c| c.id).collect();
        if ids.is_empty() {
            return self.set_status(NOTHING_TO_SEND);
        }
        let n = ids.len();
        let md = r.to_markdown(
            self.config.send.preamble.as_deref(),
            &self.config.review.kinds,
        );
        let root = r.root().to_path_buf();
        let terminal = &mut self.terminal;
        let mut copy =
            |text: &str| -> std::io::Result<()> { terminal(super::osc52(text).as_bytes()) };
        let how = match handback::send(&self.config.send.command, &md, &root, &mut copy) {
            Ok(Sent::Command) => "sent",
            Ok(Sent::Clipboard) => "copied to clipboard",
            Err(e) => return self.set_status(format!("Review not sent: {e}")),
        };
        self.after_export();
        let Some(r) = &mut self.review else { return };
        let msg = match r.remove_all(&ids) {
            Ok(_) => format!("Review {how} ({n} comments)"),
            Err(e) => format!(
                "Review {how}, but the batch wasn't cleared: {e} (sending again repeats it)"
            ),
        };
        self.set_status(msg);
        self.picker_refresh();
    }

    /// Test hook: runs between the export and the removal of a send.
    fn after_export(&mut self) {
        if let Some(f) = self.after_export.as_mut() {
            f();
        }
    }

    /// Run `f` after a send exports the batch and before it removes the
    /// sent comments (tests add a comment there from another handle).
    pub fn with_after_export(mut self, f: impl FnMut() + 'static) -> Self {
        self.after_export = Some(Box::new(f));
        self
    }

    /// Injects the terminal writer a send's clipboard copy goes through
    /// (stdout in production).
    pub fn with_terminal(mut self, w: impl FnMut(&[u8]) -> std::io::Result<()> + 'static) -> Self {
        self.terminal = Box::new(w);
        self
    }
}
