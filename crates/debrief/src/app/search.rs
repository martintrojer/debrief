//! `/ ? n N` over the commit view's row text (ramble's search.rs, on rows
//! instead of a source map): literal, smartcase, hits as row + byte range.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use super::{App, Mode};

/// Search state of the open commit.
#[derive(Debug, Clone, Default)]
pub(super) struct Search {
    pattern: String,
    /// Direction of the last search: `/` forward.
    forward: bool,
    /// (row, byte range in the row's text), in order.
    hits: Vec<(usize, Range<usize>)>,
    /// The prompt text while typing.
    prompt: Option<String>,
    /// The pattern before the prompt opened (restored by Esc).
    saved: Option<String>,
}

/// Literal matches of `pat` in `text`. Case-insensitive unless `pat` has
/// an uppercase letter (smartcase). Non-overlapping, in order.
pub fn find_all(text: &str, pat: &str) -> Vec<Range<usize>> {
    if pat.is_empty() {
        return Vec::new();
    }
    let fold = !pat.chars().any(char::is_uppercase);
    let eq = |a: char, b: char| a == b || (fold && a.to_lowercase().eq(b.to_lowercase()));
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let mut src = rest.char_indices();
        let ok = pat
            .chars()
            .all(|p| matches!(src.next(), Some((_, c)) if eq(c, p)));
        let step = rest.chars().next().map_or(1, char::len_utf8);
        if ok {
            let e = i + src.next().map_or(rest.len(), |(j, _)| j);
            out.push(i..e);
            i = e.max(i + step);
        } else {
            i += step;
        }
    }
    out
}

/// Display column of byte `b` in `text`.
fn col_of(text: &str, b: usize) -> usize {
    text[..b].width()
}

impl App {
    /// The search prompt (`/foo` or `?foo`) while typing.
    pub fn search_prompt(&self) -> Option<String> {
        let s = &self.commit.as_ref()?.search;
        let p = s.prompt.as_ref()?;
        Some(format!("{}{p}", if s.forward { '/' } else { '?' }))
    }

    /// Search hits as (row, first column, end column) of the row's text.
    pub fn search_hits(&self) -> Vec<(usize, usize, usize)> {
        let Some(v) = &self.commit else {
            return Vec::new();
        };
        v.search
            .hits
            .iter()
            .map(|(r, h)| {
                let t = &v.rows[*r].text;
                (*r, col_of(t, h.start), col_of(t, h.end))
            })
            .collect()
    }

    /// Recompute the hits of the active pattern over the current rows.
    pub(super) fn refresh_search(&mut self) {
        let Some(v) = &mut self.commit else { return };
        let pat = v.search.pattern.clone();
        v.search.hits = v
            .rows
            .iter()
            .enumerate()
            .flat_map(|(i, r)| find_all(&r.text, &pat).into_iter().map(move |h| (i, h)))
            .collect();
    }

    /// `/` (forward) or `?`: open the prompt.
    pub(super) fn search_start(&mut self, forward: bool) {
        let Some(v) = &mut self.commit else { return };
        v.search.saved = Some(v.search.pattern.clone());
        v.search.prompt = Some(String::new());
        v.search.forward = forward;
        self.mode = Mode::Search;
    }

    /// A typed character (or `None` for Backspace) in the prompt.
    pub(super) fn search_edit(&mut self, c: Option<char>) {
        let Some(v) = &mut self.commit else { return };
        let Some(prompt) = &mut v.search.prompt else {
            return;
        };
        match c {
            Some(c) => prompt.push(c),
            None if prompt.is_empty() => return self.search_cancel(),
            None => {
                prompt.pop();
            }
        }
        v.search.pattern = prompt.clone();
        self.refresh_search();
    }

    /// Pasted text (one line) appended to the prompt; the hits follow.
    pub(super) fn search_insert(&mut self, text: &str) {
        let Some(v) = &mut self.commit else { return };
        let Some(prompt) = &mut v.search.prompt else {
            return;
        };
        prompt.push_str(text);
        v.search.pattern = prompt.clone();
        self.refresh_search();
    }

    /// Enter: commit the pattern and jump to the first hit.
    pub(super) fn search_commit(&mut self) {
        self.mode = Mode::Normal;
        let Some(v) = &mut self.commit else { return };
        let typed = v.search.prompt.take().unwrap_or_default();
        let saved = v.search.saved.take();
        if typed.is_empty() {
            // An empty pattern repeats the last one, as in vim.
            if let Some(p) = saved {
                v.search.pattern = p;
            }
            self.refresh_search();
        }
        self.search_next(true, 1);
    }

    /// Esc in the prompt: close it and restore the previous pattern.
    pub(super) fn search_cancel(&mut self) {
        self.mode = Mode::Normal;
        let Some(v) = &mut self.commit else { return };
        v.search.prompt = None;
        if let Some(p) = v.search.saved.take() {
            v.search.pattern = p;
        }
        self.refresh_search();
    }

    /// `n` (`same`) / `N`, `count` times; wraps around the ends.
    pub(super) fn search_next(&mut self, same: bool, count: usize) {
        let Some(v) = &self.commit else { return };
        if v.search.pattern.is_empty() {
            self.set_status("no previous search");
            return;
        }
        let mut starts: Vec<(usize, usize)> = self
            .search_hits()
            .into_iter()
            .map(|(r, c, _)| (r, c))
            .collect();
        starts.dedup();
        let (Some(&first), Some(&last)) = (starts.first(), starts.last()) else {
            let p = v.search.pattern.clone();
            self.set_status(format!("pattern not found: {p}"));
            return;
        };
        let forward = v.search.forward == same;
        let mut cur = (v.cur.row, v.cur.col);
        let mut wrapped = false;
        for _ in 0..count.max(1) {
            let next = if forward {
                starts.iter().find(|&&h| h > cur)
            } else {
                starts.iter().rev().find(|&&h| h < cur)
            };
            cur = *next.unwrap_or_else(|| {
                wrapped = true;
                if forward { &first } else { &last }
            });
        }
        let Some(v) = &mut self.commit else { return };
        v.cur.row = cur.0;
        v.set_col(cur.1);
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
    use super::*;

    #[test]
    fn smartcase_and_utf8() {
        assert_eq!(find_all("Foo foo FOO", "foo"), vec![0..3, 4..7, 8..11]);
        assert_eq!(find_all("Foo foo FOO", "Foo"), vec![0..3]);
        assert_eq!(find_all("é É x", "é"), vec![0..2, 3..5]);
        assert!(find_all("abc", "").is_empty());
    }
}
