//! The log's path filter: only commits touching a file, or anything
//! under a directory, load. `debrief PATH` sets it when `PATH` is below
//! the repo's root; `F` edits it in a `path: …` prompt (Enter applies
//! and reloads, an empty Enter removes it, Esc keeps the old one) and
//! `<leader>F` clears it with the text filter.
//!
//! Unlike the `f` filter it is part of the VCS query (see
//! [`crate::vcs::Vcs::log_in`]), so `--limit` and paging count matching
//! commits; the `f` filter then narrows what loaded.

use super::{App, Mode};

/// `path` as typed, relative to the repo's `root`: trimmed,
/// `/`-separated, without `.` or empty segments or a trailing `/`. An
/// absolute path must be inside `root`. `Ok(None)` when nothing is left
/// (the whole repo); `Err` says why the path can't be used (`..`, outside
/// the repo).
pub fn normalize(root: &std::path::Path, path: &str) -> Result<Option<String>, String> {
    let path = path.trim();
    let rel = if path.starts_with('/') {
        let root = root.to_string_lossy();
        match crate::vcs::under(path, root.trim_end_matches('/'))
            .then(|| &path[root.trim_end_matches('/').len()..])
        {
            Some(rest) => rest,
            None => return Err(format!("not in the repo: {path}")),
        }
    } else {
        path
    };
    let parts: Vec<&str> = rel
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if parts.contains(&"..") {
        return Err(format!("no .. in a path filter: {path}"));
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

impl App {
    /// The log's path filter, repo-relative.
    pub fn path_filter(&self) -> Option<&str> {
        self.opts.path.as_deref()
    }

    /// The `F` prompt's text while it is open.
    pub(super) fn path_prompt(&self) -> Option<String> {
        self.path_edit.as_ref().map(|p| format!("path: {p}"))
    }

    /// `F`: open the path prompt on the current path.
    pub(super) fn path_start(&mut self) {
        self.path_edit = Some(self.opts.path.clone().unwrap_or_default());
        self.mode = Mode::PathFilter;
    }

    /// A typed character (or `None` for Backspace) in the path prompt.
    pub(super) fn path_type(&mut self, c: Option<char>) {
        let Some(p) = &mut self.path_edit else { return };
        match c {
            Some(c) => p.push(c),
            None => {
                p.pop();
            }
        }
    }

    /// `C-u` in the path prompt: clear what is typed.
    pub(super) fn path_kill(&mut self) {
        if let Some(p) = &mut self.path_edit {
            p.clear();
        }
    }

    /// Pasted text (one line) appended to the path.
    pub(super) fn path_insert(&mut self, text: &str) {
        if let Some(p) = &mut self.path_edit {
            p.push_str(text);
        }
    }

    /// Esc: close the prompt, the path as it was.
    pub(super) fn path_cancel(&mut self) {
        self.path_edit = None;
        self.mode = Mode::Normal;
    }

    /// Enter: apply the typed path (empty: no path filter).
    pub(super) fn path_commit(&mut self) {
        let typed = self.path_edit.take().unwrap_or_default();
        self.mode = Mode::Normal;
        match normalize(self.vcs.root(), &typed) {
            Ok(p) => self.set_path(p),
            Err(e) => self.set_status(e),
        }
    }

    /// `<leader>F`: clear the path and the text filter.
    pub(super) fn filters_clear(&mut self) {
        if !self.log_filtered() && self.opts.path.is_none() {
            return self.set_status("no filter");
        }
        self.filter_clear();
        self.set_path(None);
    }

    /// Load the log with `path` as its path filter. The page size goes
    /// back to the first page's; the cursor stays on its commit when it
    /// is still listed, else goes to the first row. A query that fails
    /// keeps the old path (and its rows).
    fn set_path(&mut self, path: Option<String>) {
        if path == self.opts.path {
            return;
        }
        let old = std::mem::replace(&mut self.opts.path, path);
        if let Err(e) = self.requery() {
            self.opts.path = old;
            self.set_status(format!("path filter not applied: {e:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn normalizes_typed_paths() {
        let n = |p| super::normalize(Path::new("/r/repo"), p);
        assert_eq!(n(""), Ok(None));
        assert_eq!(n("  ./ "), Ok(None));
        assert_eq!(n("src/"), Ok(Some("src".into())));
        assert_eq!(n("./src//app/x.rs"), Ok(Some("src/app/x.rs".into())));
        assert_eq!(n("a b/c"), Ok(Some("a b/c".into())));
        // Absolute: inside the root only.
        assert_eq!(n("/r/repo"), Ok(None));
        assert_eq!(n("/r/repo/src/x.rs"), Ok(Some("src/x.rs".into())));
        assert!(n("/r/repository/x").is_err());
        assert!(n("/etc").is_err());
        assert!(n("a/../b").is_err());
        assert!(n("..").is_err());
    }
}
