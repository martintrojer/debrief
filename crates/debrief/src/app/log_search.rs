//! Search and filter on the log:
//! `/` `?` `n` `N` move between matching rows, `f` narrows the list as you
//! type. Both match the summary, the author, the commit ID and the change
//! ID (substring, smart case, see [`find_all`]), over the loaded commits.
//!
//! The filter keeps every loaded row in `all_rows`; `rows` is the
//! narrowed list, so the cursor, Enter, `m` and `]]` / `[[` act on what
//! is shown.

use super::search::find_all;
use super::{App, Mode, Row};

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
}

/// `pat` matches one of `r`'s fields.
fn row_matches(r: &Row, pat: &str) -> bool {
    let i = &r.info;
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
    /// The log's open prompt: `/foo`, `?foo` or `filter: foo`.
    pub(super) fn log_prompt(&self) -> Option<String> {
        let s = &self.log_search;
        if s.filtering {
            return Some(format!("filter: {}", s.filter));
        }
        let p = s.prompt.as_ref()?;
        Some(format!("{}{p}", if s.forward { '/' } else { '?' }))
    }

    /// A kept filter for the status line: `filter: x (12/200)`.
    pub fn filter_label(&self) -> Option<String> {
        let f = &self.log_search.filter;
        if f.is_empty() || self.log_search.filtering {
            return None;
        }
        Some(format!(
            "filter: {f} ({}/{})",
            self.rows.len(),
            self.all_rows.len()
        ))
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
    /// still listed, else the first row.
    pub(super) fn refilter(&mut self, keep: Option<String>) {
        let pat = &self.log_search.filter;
        self.rows = self
            .all_rows
            .iter()
            .filter(|r| pat.is_empty() || row_matches(r, pat))
            .cloned()
            .collect();
        self.cursor = keep
            .and_then(|id| self.rows.iter().position(|r| r.info.commit == id))
            .unwrap_or(0);
        self.keep_visible();
    }

    fn cursor_id(&self) -> Option<String> {
        self.rows.get(self.cursor).map(|r| r.info.commit.clone())
    }

    /// `f`: open the filter prompt on the current filter.
    pub(super) fn filter_start(&mut self) {
        self.log_search.anchor = self.cursor_id();
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
        self.log_search.filtering = false;
        self.mode = Mode::Normal;
    }

    /// Esc: clear the filter, the cursor on the same commit.
    pub(super) fn filter_clear(&mut self) {
        let keep = self.cursor_id().or(self.log_search.anchor.take());
        self.log_search.filtering = false;
        self.log_search.filter.clear();
        self.mode = Mode::Normal;
        self.refilter(keep);
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

    /// `/` (forward) or `?` on the log.
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

    /// `n` (`same`) / `N`, `count` times; wraps around the ends.
    pub(super) fn log_search_next(&mut self, same: bool, count: usize) {
        let pat = self.log_search.pattern.clone();
        if pat.is_empty() {
            return self.set_status("no previous search");
        }
        let hits: Vec<usize> = (0..self.rows.len())
            .filter(|&i| row_matches(&self.rows[i], &pat))
            .collect();
        let (Some(&first), Some(&last)) = (hits.first(), hits.last()) else {
            return self.set_status(format!("pattern not found: {pat}"));
        };
        let forward = self.log_search.forward == same;
        let mut cur = self.cursor;
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
        self.cursor = cur;
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
