//! Search and filter on the log:
//! `/` `?` `n` `N` move between matching rows, `f` narrows the list as you
//! type. Both match the summary, the author, the commit ID and the change
//! ID (substring, smart case, see [`find_all`]), over the loaded commits.
//!
//! The filter keeps every loaded row in `all_rows`; `rows` is the
//! narrowed list, so the cursor, Enter, `m` and `]]` / `[[` act on what
//! is shown.
//!
//! The commit list (focused, on the commit screen) uses the same search
//! and filter: there the search moves the list's cursor and opens
//! nothing, and the filter keeps the list's cursor and the open commit's
//! index on their commits.
//!
//! Neither loads more commits. When one finds nothing among the loaded
//! commits and more may exist, the prompt line and then the status line
//! say so ([`NOT_LOADED`]).

use super::log::{NOT_LOADED, RowKey, is_fold_more};
use super::search::find_all;
use super::{App, Mode, Row, Screen};

/// Search and filter state of the log.
#[derive(Debug, Clone, Default)]
pub(super) struct LogSearch {
    /// The active search pattern.
    pattern: String,
    /// Direction of the last search: `/` forward.
    forward: bool,
    /// The search prompt while typing.
    prompt: Option<String>,
    /// The pattern before the prompt opened (restored by Esc).
    saved: Option<String>,
    /// The filter pattern; empty shows every row.
    filter: String,
    /// The filter prompt is open.
    filtering: bool,
    /// The commit under the cursor when the filter prompt opened.
    anchor: Option<String>,
    /// The commit under the commit list's cursor when the filter prompt
    /// opened.
    list_anchor: Option<RowKey>,
}

/// `pat` matches one of `r`'s fields (never a fold's `… more` row).
fn row_matches(r: &Row, pat: &str) -> bool {
    let i = &r.info;
    if is_fold_more(&i.commit) {
        return false;
    }
    [
        i.summary.as_str(),
        i.author.as_str(),
        i.commit.as_str(),
        i.change.as_deref().unwrap_or(""),
    ]
    .iter()
    .any(|f| !find_all(f, pat).is_empty())
}

impl App {
    /// The log's open prompt: `path: foo` (see `path_filter`), `/foo`,
    /// `?foo` or `filter: foo`.
    pub(super) fn log_prompt(&self) -> Option<String> {
        if let Some(p) = self.path_prompt() {
            return Some(p);
        }
        let s = &self.log_search;
        if s.filtering {
            return Some(format!("filter: {}", s.filter));
        }
        let p = s.prompt.as_ref()?;
        Some(format!("{}{p}", if s.forward { '/' } else { '?' }))
    }

    /// Said after the open prompt: [`NOT_LOADED`] when its pattern
    /// matches no loaded row and more may exist.
    pub fn log_prompt_note(&self) -> Option<&'static str> {
        if self.mode == Mode::PathFilter {
            return None;
        }
        let s = &self.log_search;
        let pat = if s.filtering {
            &s.filter
        } else {
            s.prompt.as_ref()?
        };
        let miss = !pat.is_empty() && !self.rows.iter().any(|r| row_matches(r, pat));
        (miss && self.log_more()).then_some(NOT_LOADED)
    }

    /// A kept filter for the status line: `filter: x (12/200)`, then
    /// [`NOT_LOADED`] when nothing loaded matches and more may exist.
    pub fn filter_label(&self) -> Option<String> {
        let f = &self.log_search.filter;
        if f.is_empty() || self.log_search.filtering {
            return None;
        }
        let mut label = format!("filter: {f} ({}/{})", self.rows.len(), self.all_rows.len());
        if self.rows.is_empty() && self.log_more() {
            label.push_str(&format!("  {NOT_LOADED}"));
        }
        Some(label)
    }

    /// Patterns to highlight in log rows: the search's and the filter's.
    pub fn log_patterns(&self) -> Vec<&str> {
        [&self.log_search.pattern, &self.log_search.filter]
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(String::as_str)
            .collect()
    }

    /// Rows shown out of rows loaded.
    pub fn filter_counts(&self) -> (usize, usize) {
        (self.rows.len(), self.all_rows.len())
    }

    /// Narrow `rows` from `all_rows`; the cursor goes to `keep` when it is
    /// still listed, else the first row. The commit list's cursor stays on
    /// its commit (while typing the filter, the one it was on when the
    /// prompt opened) and the open commit's index follows.
    pub(super) fn refilter(&mut self, keep: Option<String>) {
        let listed = [
            self.log_search
                .list_anchor
                .clone()
                .filter(|_| self.log_search.filtering),
            self.log_key(self.commit_list.cursor),
        ];
        let shown = self.filter_keep();
        self.rows = self
            .all_rows
            .iter()
            .zip(shown)
            .filter(|(_, k)| *k)
            .map(|(r, _)| r.clone())
            .collect();
        self.cursor = keep
            .and_then(|id| self.rows.iter().position(|r| r.info.commit == id))
            .unwrap_or(0);
        if let Some(i) = listed.iter().flatten().find_map(|k| self.row_of(k)) {
            self.commit_list.cursor = i;
        }
        self.commit_reindex();
        self.keep_visible();
    }

    /// Which of `all_rows` the filter shows: the hits, the merges whose
    /// open folds hold one, and a group's `… more` row when its group has
    /// a hit. Unloaded folds are not searched.
    fn filter_keep(&self) -> Vec<bool> {
        let pat = &self.log_search.filter;
        let rows = &self.all_rows;
        if pat.is_empty() {
            return vec![true; rows.len()];
        }
        let mut keep: Vec<bool> = rows.iter().map(|r| row_matches(r, pat)).collect();
        let index: std::collections::HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.info.commit.as_str(), i))
            .collect();
        let mut hit_groups = std::collections::HashSet::new();
        for i in 0..rows.len() {
            if !keep[i] || rows[i].merge.is_none() {
                continue;
            }
            hit_groups.insert(self.folds.row_group.get(i).copied().unwrap_or(0));
            let mut m = rows[i].merge.as_deref();
            while let Some(j) = m.and_then(|id| index.get(id).copied()) {
                keep[j] = true;
                hit_groups.insert(self.folds.row_group.get(j).copied().unwrap_or(0));
                m = rows[j].merge.as_deref();
            }
        }
        for (i, r) in rows.iter().enumerate() {
            let g = self.folds.row_group.get(i).copied().unwrap_or(0);
            if is_fold_more(&r.info.commit) && g != 0 && hit_groups.contains(&g) {
                keep[i] = true;
            }
        }
        keep
    }

    /// Where `]]` (`forward`) / `[[` go while the filter hides the open
    /// commit: the first shown row after (before) the open commit's place
    /// in the unfiltered log, `Some(None)` when none is. `None` when the
    /// open commit is shown or not loaded.
    pub(super) fn step_past_hidden(&self, forward: bool) -> Option<Option<usize>> {
        let v = self.commit.as_ref()?;
        if self.view_index(v).is_some() {
            return None;
        }
        let shown = |id: &String| self.rows.iter().position(|s| &s.info.commit == id);
        let ids: Vec<String> = self
            .all_rows
            .iter()
            .map(|r| r.info.commit.clone())
            .collect();
        let found = self.all_rows.iter().position(|r| {
            if v.working {
                r.working
            } else {
                r.info.commit == v.commit
            }
        });
        // A path change dropped it from the log: its place in the log
        // before (see `set_path`).
        let (ids, at) = match (found, &self.hidden_open) {
            (Some(at), _) => (&ids, at),
            (None, Some((id, before))) if *id == v.commit => {
                (before, before.iter().position(|c| c == id)?)
            }
            _ => return None,
        };
        Some(if forward {
            ids[at + 1..].iter().find_map(shown)
        } else {
            ids[..at].iter().rev().find_map(shown)
        })
    }

    pub(super) fn cursor_id(&self) -> Option<String> {
        self.rows.get(self.cursor).map(|r| r.info.commit.clone())
    }

    /// `f`: open the filter prompt on the current filter.
    pub(super) fn filter_start(&mut self) {
        self.log_search.anchor = self.cursor_id();
        self.log_search.list_anchor = self.log_key(self.commit_list.cursor);
        self.log_search.filtering = true;
        self.mode = Mode::Filter;
    }

    /// A typed character (or `None` for Backspace) in the filter prompt.
    pub(super) fn filter_edit(&mut self, c: Option<char>) {
        match c {
            Some(c) => self.log_search.filter.push(c),
            None => {
                self.log_search.filter.pop();
            }
        }
        self.refilter(self.log_search.anchor.clone());
    }

    /// Pasted text (one line) appended to the filter.
    pub(super) fn filter_insert(&mut self, text: &str) {
        self.log_search.filter.push_str(text);
        self.refilter(self.log_search.anchor.clone());
    }

    /// Enter: keep the filter.
    pub(super) fn filter_commit(&mut self) {
        self.log_search.list_anchor = None;
        self.log_search.filtering = false;
        self.mode = Mode::Normal;
    }

    /// Esc: clear the filter, the cursor on the same commit.
    pub(super) fn filter_clear(&mut self) {
        let keep = self.cursor_id().or(self.log_search.anchor.take());
        self.log_search.list_anchor = None;
        self.log_search.filtering = false;
        self.log_search.filter.clear();
        self.mode = Mode::Normal;
        self.refilter(keep);
    }

    /// A text filter is kept.
    pub(super) fn log_filtered(&self) -> bool {
        !self.log_search.filter.is_empty()
    }

    /// Clear the filter when it hides `id`, a loaded commit (a picked
    /// comment's). Returns whether the list changed.
    pub(super) fn unfilter_for(&mut self, id: &str) -> bool {
        let listed = |rows: &[Row]| rows.iter().any(|r| r.info.commit == id);
        if listed(&self.rows) || !listed(&self.all_rows) {
            return false;
        }
        self.log_search.filter.clear();
        self.refilter(Some(id.to_string()));
        true
    }

    /// The search prompt is open (on the log or the commit list).
    pub(super) fn log_searching(&self) -> bool {
        self.log_search.prompt.is_some()
    }

    /// `/` (forward) or `?` on the log or the commit list.
    pub(super) fn log_search_start(&mut self, forward: bool) {
        let s = &mut self.log_search;
        s.saved = Some(s.pattern.clone());
        s.prompt = Some(String::new());
        s.forward = forward;
        self.mode = Mode::Search;
    }

    /// A typed character (or `None` for Backspace) in the search prompt.
    pub(super) fn log_search_edit(&mut self, c: Option<char>) {
        let s = &mut self.log_search;
        let Some(prompt) = &mut s.prompt else { return };
        match c {
            Some(c) => prompt.push(c),
            None if prompt.is_empty() => return self.log_search_cancel(),
            None => {
                prompt.pop();
            }
        }
        s.pattern = prompt.clone();
    }

    pub(super) fn log_search_insert(&mut self, text: &str) {
        let s = &mut self.log_search;
        let Some(prompt) = &mut s.prompt else { return };
        prompt.push_str(text);
        s.pattern = prompt.clone();
    }

    /// Enter: the first match from the cursor in the search's direction.
    pub(super) fn log_search_commit(&mut self) {
        self.mode = Mode::Normal;
        let s = &mut self.log_search;
        let typed = s.prompt.take().unwrap_or_default();
        let saved = s.saved.take();
        if typed.is_empty()
            && let Some(p) = saved
        {
            // An empty pattern repeats the last one, as in vim.
            s.pattern = p;
        }
        self.log_search_next(true, 1);
    }

    pub(super) fn log_search_cancel(&mut self) {
        self.mode = Mode::Normal;
        let s = &mut self.log_search;
        s.prompt = None;
        if let Some(p) = s.saved.take() {
            s.pattern = p;
        }
    }

    /// `n` (`same`) / `N`, `count` times; wraps around the ends. On the
    /// commit screen it moves the commit list's cursor.
    pub(super) fn log_search_next(&mut self, same: bool, count: usize) {
        let pat = self.log_search.pattern.clone();
        if pat.is_empty() {
            return self.set_status("no previous search");
        }
        let hits: Vec<usize> = (0..self.rows.len())
            .filter(|&i| row_matches(&self.rows[i], &pat))
            .collect();
        let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
            let more = if self.log_more() {
                format!("  {NOT_LOADED}")
            } else {
                String::new()
            };
            return self.set_status(format!("pattern not found: {pat}{more}"));
        };
        let forward = self.log_search.forward == same;
        let list = matches!(self.screen, Screen::Commit(_));
        let mut cur = if list {
            self.commit_list.cursor
        } else {
            self.cursor
        };
        let mut wrapped = false;
        for _ in 0..count.max(1) {
            let next = if forward {
                hits.iter().find(|&&h| h > cur)
            } else {
                hits.iter().rev().find(|&&h| h < cur)
            };
            cur = *next.unwrap_or_else(|| {
                wrapped = true;
                if forward { &first } else { &last }
            });
        }
        if list {
            self.commit_list.cursor = cur;
            self.commit_list_keep_visible();
        } else {
            self.cursor = cur;
        }
        if wrapped {
            self.set_status(if forward {
                "search hit BOTTOM, continuing at TOP"
            } else {
                "search hit TOP, continuing at BOTTOM"
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::test_support::app;
    use crate::app::{Mode, Screen};

    fn summaries(a: &crate::app::App) -> Vec<String> {
        a.rows().iter().map(|r| r.info.summary.clone()).collect()
    }

    #[test]
    fn search_moves_between_matches_and_wraps() {
        let (_t, mut a) = app();
        assert_eq!(summaries(&a), ["c1", "c0"]);
        a.cursor = 0;
        a.log_search_start(true);
        for c in "C".chars() {
            a.log_search_edit(Some(c));
        }
        assert_eq!(a.log_prompt().as_deref(), Some("/C"));
        a.log_search_commit();
        assert_eq!(a.status(), "pattern not found: C", "smart case");
        a.log_search_start(true);
        a.log_search_edit(Some('c'));
        a.log_search_commit();
        assert_eq!(a.cursor(), 1);
        a.log_search_next(true, 1);
        assert_eq!(a.cursor(), 0);
        assert_eq!(a.status(), "search hit BOTTOM, continuing at TOP");
        a.log_search_next(false, 1);
        assert_eq!(a.cursor(), 1);
        assert_eq!(a.status(), "search hit TOP, continuing at BOTTOM");
        // The full commit ID matches too.
        let id = a.rows()[0].info.commit[20..30].to_string();
        a.log_search_start(false);
        a.log_search_insert(&id);
        a.log_search_commit();
        assert_eq!(a.cursor(), 0);
        assert_eq!(a.screen(), Screen::Log);
    }

    #[test]
    fn filter_narrows_keeps_and_clears() {
        let (_t, mut a) = app();
        // c1's full commit ID: no other row has it.
        let id = a.rows()[0].info.commit.clone();
        a.cursor = 1;
        a.filter_start();
        assert_eq!(a.mode(), Mode::Filter);
        a.filter_insert(&id[..id.len() - 1]);
        a.filter_edit(id.chars().last());
        assert_eq!(summaries(&a), ["c1"]);
        assert_eq!(a.cursor(), 0, "c0 is hidden: the first row");
        assert_eq!(a.log_prompt(), Some(format!("filter: {id}")));
        for _ in 0..id.len() {
            a.filter_edit(None);
        }
        assert_eq!(summaries(&a), ["c1", "c0"]);
        assert_eq!(a.cursor(), 1, "back on c0, where the prompt opened");
        a.filter_insert(&id);
        a.filter_commit();
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.filter_label(), Some(format!("filter: {id} (1/2)")));
        a.filter_start();
        a.filter_clear();
        assert_eq!(summaries(&a), ["c1", "c0"]);
        assert_eq!(a.cursor(), 0, "still on c1");
        assert_eq!(a.filter_label(), None);
    }
}
