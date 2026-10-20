//! The log's branch: which history it lists. The current branch at start
//! (or `--branch`); `<leader>b` opens a picker over the local branches
//! (jj: bookmarks), the default history (`HEAD` / `@`) and a `--revset`
//! when one was given. A branch lists its commits not in trunk (see
//! [`crate::vcs::Vcs::branch_revset`]).
//!
//! Like the path filter it is part of the VCS query: picking reloads the
//! log, `r` and the watchers keep it. It never checks anything out.

use super::App;
use super::picker::{PickerItem, PickerKind};
use crate::vcs::{Kind, NoSuchBranch};

/// What the log lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The default history: `HEAD` (git), `::@` (jj).
    Default,
    /// `--revset` / `[log] revset`.
    Revset(String),
    /// A branch (jj bookmark).
    Branch(String),
}

/// The picker's key for [`Selection::Default`] (branch names are keyed by
/// themselves; neither can start with a NUL).
const DEFAULT_KEY: &str = "\0default";
/// The picker's key for [`Selection::Revset`].
const REVSET_KEY: &str = "\0revset";

/// Status when a selected branch went away on a reload.
pub fn gone(name: &str) -> String {
    format!("branch {name} is gone")
}

impl App {
    /// The selection at start: `--branch`, a revset, else the current
    /// branch, else the default history.
    pub(super) fn branch_start(&mut self) -> anyhow::Result<()> {
        self.branch_current = self.vcs.current_branch().unwrap_or(None);
        self.selection = match (&self.opts.branch, &self.opts.revset) {
            (Some(b), _) => Selection::Branch(b.clone()),
            (None, Some(r)) => Selection::Revset(r.clone()),
            (None, None) => match &self.branch_current {
                Some(b) => Selection::Branch(b.clone()),
                None => Selection::Default,
            },
        };
        let explicit = self.opts.branch.is_some();
        match self.selection_revset() {
            Ok(r) => self.query = r,
            // An unknown `--branch` is a start error; a current branch
            // the query can't use (no commits) is the default history.
            Err(e) if explicit => return Err(e),
            Err(_) => {
                self.selection = Selection::Default;
                self.query = None;
            }
        }
        Ok(())
    }

    /// The query the selection stands for: `None` is the default history.
    fn selection_revset(&self) -> anyhow::Result<Option<String>> {
        Ok(match &self.selection {
            Selection::Default => None,
            Selection::Revset(r) => Some(r.clone()),
            Selection::Branch(b) => {
                // jj: `@` on the current bookmark's line is listed too.
                let at = self.vcs.kind() == Kind::Jj && self.branch_current.as_ref() == Some(b);
                Some(
                    self.vcs
                        .branch_revset(b, self.config.log.trunk.as_deref(), at)?,
                )
            }
        })
    }

    /// Before a reload: re-read the current branch and the selection's
    /// query (branch tips move). A selected branch that is gone falls back
    /// to the current branch, saying so. Any other error (a trunk that
    /// can't be read) keeps the selection and its last query, and says
    /// why.
    pub(super) fn branch_refresh(&mut self) {
        self.branch_current = self.vcs.current_branch().unwrap_or(None);
        match self.selection_revset() {
            Ok(r) => self.query = r,
            Err(e) if !e.is::<NoSuchBranch>() => {
                self.set_status(format!("{e:#}"));
            }
            Err(_) => {
                let name = self.branch_name().unwrap_or_default().to_string();
                self.selection = match &self.branch_current {
                    Some(b) => Selection::Branch(b.clone()),
                    None => Selection::Default,
                };
                self.query = self.selection_revset().unwrap_or(None);
                if self.query.is_none() {
                    self.selection = Selection::Default;
                }
                self.set_status(gone(&name));
            }
        }
    }

    /// The selected branch's name; `None` for the default history and a
    /// revset.
    pub fn branch_name(&self) -> Option<&str> {
        match &self.selection {
            Selection::Branch(b) => Some(b),
            _ => None,
        }
    }

    /// The branch reviews made now record: the selected branch, `None`
    /// for the default history and a revset.
    pub(super) fn selection_branch(&self) -> Option<String> {
        self.branch_name().map(str::to_string)
    }

    /// What the log lists, as the branch row and status line name it:
    /// `feat/x`, `HEAD` / `@`, or `revset: R`.
    pub fn branch_label(&self) -> String {
        match &self.selection {
            Selection::Branch(b) => b.clone(),
            Selection::Revset(r) => format!("revset: {r}"),
            Selection::Default => match self.vcs.kind() {
                Kind::Git => "HEAD".to_string(),
                Kind::Jj => "@".to_string(),
            },
        }
    }

    /// The checked-out branch (jj: the bookmark on `@` or `@-`).
    pub fn current_branch(&self) -> Option<&str> {
        self.branch_current.as_deref()
    }

    /// Whether the log lists the checked-out line: git's working-copy row
    /// shows only then.
    pub(super) fn on_checked_out(&self) -> bool {
        match &self.selection {
            Selection::Default => true,
            Selection::Revset(_) => true,
            Selection::Branch(b) => self.branch_current.as_ref() == Some(b),
        }
    }

    /// The picker's rows: the default history, a revset when one was
    /// given, then the branches (most recent first, the current one on
    /// top). The selected row is marked `●`, the current branch `(current)`.
    pub(super) fn branch_items(&self) -> Vec<PickerItem> {
        let mark = |on: bool| if on { "● " } else { "  " };
        let mut items = vec![PickerItem {
            id: 0,
            key: DEFAULT_KEY.to_string(),
            label: format!(
                "{}{}",
                mark(self.selection == Selection::Default),
                match self.vcs.kind() {
                    Kind::Git => "HEAD",
                    Kind::Jj => "@",
                }
            ),
            detail: "the current commit and its ancestors".to_string(),
        }];
        if let Some(r) = &self.opts.revset {
            items.push(PickerItem {
                id: 0,
                key: REVSET_KEY.to_string(),
                label: format!(
                    "{}revset: {r}",
                    mark(matches!(self.selection, Selection::Revset(_)))
                ),
                detail: String::new(),
            });
        }
        let mut names = self.vcs.branches().unwrap_or_default();
        if let Some(c) = &self.branch_current
            && let Some(i) = names.iter().position(|n| n == c)
        {
            let c = names.remove(i);
            names.insert(0, c);
        }
        items.extend(names.into_iter().map(|n| PickerItem {
            id: 0,
            label: format!("{}{n}", mark(self.branch_name() == Some(n.as_str()))),
            detail: if self.branch_current.as_ref() == Some(&n) {
                "(current)".to_string()
            } else {
                String::new()
            },
            key: n,
        }));
        items
    }

    /// `<leader>b`: list the branches.
    pub(super) fn branch_list(&mut self) {
        self.branch_current = self.vcs.current_branch().unwrap_or(None);
        self.picker_open_kind(PickerKind::Branches);
        // Start on the selected row.
        let at = self
            .picker()
            .and_then(|p| p.items.iter().position(|it| it.label.starts_with('●')));
        if let Some(i) = at {
            self.picker_select(i);
        }
    }

    /// Enter in the branch picker.
    pub(super) fn branch_pick(&mut self, key: &str) {
        let sel = match key {
            DEFAULT_KEY => Selection::Default,
            REVSET_KEY => match &self.opts.revset {
                Some(r) => Selection::Revset(r.clone()),
                None => return,
            },
            name => Selection::Branch(name.to_string()),
        };
        self.set_selection(sel);
    }

    /// Select branch `name` (a session's), if it still exists. `false`
    /// when it doesn't; the log is unchanged then.
    pub(super) fn branch_switch(&mut self, name: &str) -> bool {
        if self.branch_name() == Some(name) {
            return true;
        }
        if !self
            .vcs
            .branches()
            .unwrap_or_default()
            .iter()
            .any(|b| b == name)
        {
            return false;
        }
        self.set_selection(Selection::Branch(name.to_string()));
        self.branch_name() == Some(name)
    }

    /// Load the log with `sel`. The page size goes back to the first
    /// page's; the cursor stays on its commit when it is still listed,
    /// else goes to the first row; an open commit stays open (`]]` / `[[`
    /// step from its old place if the new history lacks it). A query that
    /// fails keeps the old selection (and its rows).
    fn set_selection(&mut self, sel: Selection) {
        if sel == self.selection {
            return;
        }
        let old = (
            std::mem::replace(&mut self.selection, sel),
            self.query.clone(),
        );
        self.query = match self.selection_revset() {
            Ok(q) => q,
            Err(e) => {
                (self.selection, self.query) = old;
                return self.set_status(format!("branch not selected: {e:#}"));
            }
        };
        if let Err(e) = self.requery() {
            (self.selection, self.query) = old;
            self.set_status(format!("branch not selected: {e:#}"));
        }
    }
}
