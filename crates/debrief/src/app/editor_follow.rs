//! Following the editor's cursor (`--follow`, `follow = true` launchers):
//! what an [`AppEvent::EditorAt`](super::AppEvent::EditorAt) does to the
//! commit view. The socket side is the `debrief-follow` crate; the spec is
//! docs/specs/2026-10-31-follow-editor.md. Best effort: the editor's line
//! is a working-copy line, mapped onto the open diff's new side.

use std::path::Path;
use std::time::{Duration, Instant};

use super::commit::{Row, RowKind};
use super::{App, AppEvent, Mode, Screen};
use crate::vcs::CommitDiff;

/// After a key, mouse event or paste in debrief, editor updates are
/// dropped for this long, so following never fights the reader.
pub const FOLLOW_GRACE: Duration = Duration::from_secs(2);

/// The tool name in debrief's socket file names.
const TOOL: &str = "debrief";

/// The row for new-side `line` of file `file` in `rows`: the row of that
/// line, else of the closest new-side line in the file (the earlier on a
/// tie), else the file's header. `None`: the file has no rows.
pub fn follow_row(diff: &CommitDiff, rows: &[Row], file: usize, line: u32) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    let mut header = None;
    for (i, r) in rows.iter().enumerate() {
        if r.file != Some(file) {
            continue;
        }
        if r.kind == RowKind::FileHeader {
            header = Some(i);
        }
        if !matches!(r.kind, RowKind::Line(_)) {
            continue;
        }
        let new = r
            .hunk
            .zip(r.line)
            .and_then(|(h, l)| diff.files.get(file)?.hunks.get(h)?.lines.get(l)?.new);
        let Some(n) = new else { continue };
        let d = n.abs_diff(line);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i).or(header)
}

impl App {
    /// The reader used debrief at `now`: start the grace window.
    pub(super) fn editor_touch(&mut self, now: Instant) {
        self.editor_quiet_until = Some(now + FOLLOW_GRACE);
    }

    /// Listen for the editor's cursor (once): `--follow`, or the first
    /// follow launcher. A failure is a status message.
    pub fn editor_follow_start(&mut self) {
        #[cfg(unix)]
        self.editor_follow_start_in(&debrief_follow::default_dir());
        #[cfg(not(unix))]
        self.set_status("follow off: needs Linux or macOS");
    }

    /// [`App::editor_follow_start`] with the socket directory given (tests).
    #[cfg(unix)]
    pub fn editor_follow_start_in(&mut self, dir: &Path) {
        if self.editor_listener.is_some() {
            return;
        }
        let Some(tx) = self.sender.clone() else {
            return;
        };
        let emit = move |path, line| {
            let _ = tx.send(AppEvent::EditorAt(path, line, Instant::now()));
        };
        match debrief_follow::listen(dir, &debrief_follow::current_scope(), TOOL, emit) {
            Ok(l) => self.editor_listener = Some(l),
            Err(e) => self.set_status(format!("follow off: {e}")),
        }
    }

    /// `--follow` was given.
    pub(super) fn opts_follow(&self) -> bool {
        self.opts.follow
    }

    /// Until when editor updates are dropped (the grace window).
    pub fn editor_quiet_until(&self) -> Option<Instant> {
        self.editor_quiet_until
    }

    /// Whether debrief is listening for the editor.
    pub fn editor_following(&self) -> bool {
        self.editor_listener.is_some()
    }

    /// The editor's cursor is on 1-based `line` of `path` at `now`.
    /// Ignored on the log, during the grace window, outside Normal mode,
    /// with a key sequence or count half typed, while the diff loads, and
    /// for a file not in the open diff. Never sets a status: the hook
    /// fires on every cursor move.
    pub fn editor_at(&mut self, path: &Path, line: usize, now: Instant) {
        let busy = self.mode != Mode::Normal || !self.pending.is_empty() || self.count.is_some();
        if busy || self.editor_quiet_until.is_some_and(|t| now < t) {
            return;
        }
        if !matches!(self.screen, Screen::Commit(_)) {
            return;
        }
        let Some(rel) = self.repo_relative(path) else {
            return;
        };
        let Some(v) = &self.commit else { return };
        if v.loading {
            return;
        }
        let Some(diff) = &v.diff else { return };
        let Some(file) = diff
            .files
            .iter()
            .position(|f| f.new_path.as_deref() == Some(rel.as_str()))
        else {
            return;
        };
        // Side by side: the new side holds the editor's lines.
        if self.split_side() == Some(debrief_review::Side::Old)
            && let Some(v) = &mut self.commit
        {
            v.swap_side();
            self.refresh_search();
        }
        let line = u32::try_from(line).unwrap_or(u32::MAX);
        let Some(v) = &mut self.commit else { return };
        let Some(diff) = &v.diff else { return };
        let Some(row) = follow_row(diff, &v.rows, file, line) else {
            return;
        };
        let visible = row >= v.scroll && row < v.scroll + v.view_h.max(1);
        v.goto_row(row);
        if !visible {
            v.center_cursor();
        }
        self.commit_follow();
    }

    /// `path` (absolute) relative to the VCS root, `/`-separated as diffs
    /// name files; `None` outside it. Symlinks resolve through the deepest
    /// part that exists, so a file gone from disk still matches through a
    /// symlinked repo path.
    fn repo_relative(&self, path: &Path) -> Option<String> {
        let (root, path) = (canonical(self.vcs.root()), canonical(path));
        let rel = path.strip_prefix(&root).ok()?;
        let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
        Some(parts?.join("/"))
    }
}

/// `p` canonicalised, or its deepest existing ancestor canonicalised with
/// the rest appended.
fn canonical(p: &Path) -> std::path::PathBuf {
    let mut tail = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(c) = cur.canonicalize() {
            return tail.iter().rev().fold(c, |acc, t| acc.join(t));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::canonical;

    #[test]
    fn a_missing_file_resolves_through_a_symlinked_parent() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("link")).unwrap();
        let real = real.canonicalize().unwrap();
        assert_eq!(
            canonical(&d.path().join("link/sub/gone.rs")),
            real.join("sub/gone.rs")
        );
        std::fs::write(real.join("here.rs"), "").unwrap();
        assert_eq!(
            canonical(&d.path().join("link/here.rs")),
            real.join("here.rs")
        );
    }
}
