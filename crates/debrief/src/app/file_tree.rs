//! The commit view's file list as a tree, as lazygit's commit files
//! panel lays it out: files grouped by directory, directories before
//! files at each level, each alphabetical; a chain of directories that
//! hold one directory and no files is one row (`pkg/gui/controllers`).
//! Files are never compressed. Flat mode lists the paths in git's order.

use std::collections::{BTreeMap, BTreeSet};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// One row of the full list (every directory expanded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Nesting: 0 for the top level.
    pub depth: usize,
    /// What the row shows: a directory's (compressed) name, a file's
    /// name; in flat mode the file's whole path.
    pub name: String,
    /// A directory's path from the root (`src/app`); a file's path.
    pub path: String,
    /// The file's index in the diff; `None` for a directory.
    pub file: Option<usize>,
    /// Added and removed lines: the file's, or summed over a directory.
    pub add: usize,
    pub del: usize,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.file.is_none()
    }
}

/// A file to list: its path and line counts. Its index is its position.
pub type FileItem<'a> = (&'a str, usize, usize);

#[derive(Default)]
struct Dir<'a> {
    dirs: BTreeMap<&'a str, Dir<'a>>,
    /// (name, index)
    files: Vec<(&'a str, usize)>,
}

/// The tree of `files` (see the module doc), depth first.
pub fn build_tree(files: &[FileItem]) -> Vec<Entry> {
    let mut root = Dir::default();
    for (i, (path, _, _)) in files.iter().enumerate() {
        let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let name = parts.pop().unwrap_or(path);
        let mut d = &mut root;
        for p in parts {
            d = d.dirs.entry(p).or_default();
        }
        d.files.push((name, i));
    }
    let mut out = Vec::new();
    emit(&root, "", 0, files, &mut out);
    out
}

fn emit(dir: &Dir, prefix: &str, depth: usize, files: &[FileItem], out: &mut Vec<Entry>) {
    for (name, child) in &dir.dirs {
        let mut name = name.to_string();
        let mut node = child;
        while node.files.is_empty() && node.dirs.len() == 1 {
            let (n, c) = node.dirs.iter().next().expect("one dir");
            name = format!("{name}/{n}");
            node = c;
        }
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let at = out.len();
        out.push(Entry {
            depth,
            name,
            path: path.clone(),
            file: None,
            add: 0,
            del: 0,
        });
        emit(node, &path, depth + 1, files, out);
        let (add, del) = out[at + 1..]
            .iter()
            .filter(|e| !e.is_dir())
            .fold((0, 0), |(a, d), e| (a + e.add, d + e.del));
        out[at].add = add;
        out[at].del = del;
    }
    let mut own = dir.files.clone();
    own.sort();
    for (name, i) in own {
        let (path, add, del) = files[i];
        out.push(Entry {
            depth,
            name: name.to_string(),
            path: path.to_string(),
            file: Some(i),
            add,
            del,
        });
    }
}

/// The flat list: every path in git's order.
pub fn build_flat(files: &[FileItem]) -> Vec<Entry> {
    files
        .iter()
        .enumerate()
        .map(|(i, &(path, add, del))| Entry {
            depth: 0,
            name: path.to_string(),
            path: path.to_string(),
            file: Some(i),
            add,
            del,
        })
        .collect()
}

/// The rows on screen with the directories in `collapsed` closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visible {
    /// Indices into the entries, top to bottom.
    pub rows: Vec<usize>,
    /// Per entry, the row that shows it: its own, or the collapsed
    /// directory hiding it.
    pub rep: Vec<usize>,
}

/// Which `entries` show with `collapsed` (directory paths) closed.
pub fn visible(entries: &[Entry], collapsed: &BTreeSet<String>) -> Visible {
    let mut v = Visible::default();
    // The depth of the collapsed directory hiding what follows.
    let mut hide: Option<usize> = None;
    for e in entries {
        match hide {
            Some(d) if e.depth > d => {
                v.rep.push(v.rows.len() - 1);
                continue;
            }
            _ => hide = None,
        }
        v.rep.push(v.rows.len());
        v.rows.push(v.rep.len() - 1);
        if e.is_dir() && collapsed.contains(&e.path) {
            hide = Some(e.depth);
        }
    }
    v
}

/// The entry of the directory holding entry `i` (`None` at the top level).
pub fn parent(entries: &[Entry], i: usize) -> Option<usize> {
    let d = entries.get(i)?.depth;
    entries[..i].iter().rposition(|e| e.depth < d)
}

/// Columns kept for a name before indenting stops (ramble's).
pub const MIN_NAME: usize = 8;

/// The indent of a row at `depth` in a list `width` wide whose rows put
/// `lead` columns before the name: two columns a level, capped so the
/// name keeps [`MIN_NAME`] columns (deep rows share the deepest indent
/// that fits).
pub fn indent(depth: usize, width: usize, lead: usize) -> usize {
    let room = width.saturating_sub(lead + MIN_NAME);
    (2 * depth).min(room - room % 2)
}

fn head(s: &str, width: usize) -> &str {
    let mut used = 0;
    let mut end = 0;
    for (i, g) in s.grapheme_indices(true) {
        used += g.width();
        if used > width {
            break;
        }
        end = i + g.len();
    }
    &s[..end]
}

fn tail(s: &str, width: usize) -> &str {
    let mut used = 0;
    let mut start = s.len();
    for (i, g) in s.grapheme_indices(true).rev() {
        used += g.width();
        if used > width {
            break;
        }
        start = i;
    }
    &s[start..]
}

/// `s` in at most `width` columns, cut in the middle with `…` when it
/// does not fit (ramble's rule): the end keeps the longest of its last
/// 6 columns, its whole extension and half the room, capped at the room:
/// `long_mod…_name.rs`.
pub fn clip_middle(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let room = width - 1;
    let ext = match s.rfind('.') {
        Some(i) if i > 0 => s[i..].width(),
        _ => 0,
    };
    let keep = ext.max(6).max(room / 2).min(room);
    let end = tail(s, keep);
    let start = head(s, room - end.width());
    format!("{start}…{end}")
}

/// The file list as the UI draws it.
#[derive(Debug, Clone)]
pub struct FileList<'a> {
    pub entries: &'a [Entry],
    pub visible: Visible,
    pub collapsed: &'a BTreeSet<String>,
    /// The selected row (an index into `visible.rows`).
    pub selected: usize,
    /// The row of the file holding the diff cursor, or of the collapsed
    /// directory hiding it.
    pub current: Option<usize>,
}

impl FileList<'_> {
    /// The entry of visible row `row`.
    pub fn entry(&self, row: usize) -> Option<&Entry> {
        self.entries.get(*self.visible.rows.get(row)?)
    }

    pub fn is_collapsed(&self, e: &Entry) -> bool {
        e.is_dir() && self.collapsed.contains(&e.path)
    }
}

use super::App;
use super::commit::{CommitView, FilesTop, file_counts, file_path};
use crate::vcs::CommitDiff;

impl CommitView {
    /// Build the file list of `diff` in the view's mode, and the diff's
    /// file order from it.
    pub(super) fn build_entries(&mut self, diff: &CommitDiff) {
        let items: Vec<FileItem> = diff
            .files
            .iter()
            .map(|f| {
                let (a, d) = file_counts(f);
                (file_path(f), a, d)
            })
            .collect();
        self.entries = if self.tree {
            build_tree(&items)
        } else {
            build_flat(&items)
        };
        self.order = self.entries.iter().filter_map(|e| e.file).collect();
        self.side_sel = self.side_sel.min(self.entries.len().saturating_sub(1));
    }

    pub(super) fn file_visible(&self) -> Visible {
        visible(&self.entries, &self.collapsed)
    }

    /// The entry of diff file `file`.
    pub(super) fn entry_of(&self, file: usize) -> Option<usize> {
        self.entries.iter().position(|e| e.file == Some(file))
    }

    /// The first file at or under entry `e` (a directory's first file).
    pub(super) fn first_file(&self, e: usize) -> Option<usize> {
        self.entries.get(e..)?.iter().find_map(|x| x.file)
    }

    /// The selected entry as shown: itself, or the collapsed directory
    /// hiding it.
    pub(super) fn shown_sel(&self, vis: &Visible) -> usize {
        vis.rep
            .get(self.side_sel)
            .and_then(|&r| vis.rows.get(r))
            .copied()
            .unwrap_or(0)
    }

    /// Switch the list between tree and flat; the diff's files follow
    /// the new order, the cursor kept on its row and column (and its
    /// row on screen) and the selection on its file. A file's rows are
    /// the same in either order, so the cursor keeps its offset from
    /// its file's header: a hunk header stays that hunk header.
    pub(super) fn set_tree(&mut self, tree: bool) {
        if self.tree == tree {
            return;
        }
        self.tree = tree;
        let sel = self.first_file(self.side_sel);
        // The cursor's file and offset from its header (a blank row is
        // the one above the next header); rows above every file (the
        // message) don't move.
        let blank = self
            .rows
            .get(self.cur.row)
            .is_some_and(|r| r.kind == super::RowKind::Blank);
        let at = self
            .file_at(self.cur.row + usize::from(blank))
            .and_then(|f| {
                let h = self.header_row(f)?;
                Some((f, (self.cur.row + usize::from(blank)) - h))
            });
        let Some(diff) = self.diff.take() else {
            return;
        };
        let on_screen = self.cur.row.saturating_sub(self.scroll);
        self.build_entries(&diff);
        self.build_rows(&diff);
        self.diff = Some(diff);
        if let Some((f, off)) = at
            && let Some(h) = self.header_row(f)
        {
            // The column too: `want_col` is the cursor's.
            self.cur.row = (h + off)
                .saturating_sub(usize::from(blank))
                .min(self.last_row());
            self.clamp_cursor();
            self.scroll = self
                .cur
                .row
                .saturating_sub(on_screen)
                .min(self.max_scroll());
        }
        self.side_sel = sel.and_then(|f| self.entry_of(f)).unwrap_or(0);
    }
}

impl App {
    /// The open commit's file list, as drawn.
    pub fn file_list(&self) -> Option<FileList<'_>> {
        let v = self.commit.as_ref()?;
        v.diff.as_ref()?;
        let visible = v.file_visible();
        let selected = visible.rep.get(v.side_sel).copied().unwrap_or(0);
        let current = v
            .file_at(v.cur.row)
            .and_then(|f| v.entry_of(f))
            .and_then(|e| visible.rep.get(e).copied());
        Some(FileList {
            entries: &v.entries,
            visible,
            collapsed: &v.collapsed,
            selected,
            current,
        })
    }

    /// The file list is a tree (else flat).
    pub fn files_tree(&self) -> bool {
        self.files_tree
    }

    /// Select visible row `row` of the file list and jump the diff to
    /// its file's header (a directory's first file).
    pub(super) fn select_row(&mut self, row: usize) {
        let Some(v) = &mut self.commit else { return };
        let vis = v.file_visible();
        let Some(last) = vis.rows.len().checked_sub(1) else {
            return;
        };
        v.side_sel = vis.rows[row.min(last)];
        if let Some(r) = v.first_file(v.side_sel).and_then(|f| v.header_row(f)) {
            v.goto_row(r);
            v.cursor_to_top();
            v.keep_visible();
        }
    }

    /// The selected row and its entry, as shown.
    fn shown_entry(&self) -> Option<(usize, Entry)> {
        let v = self.commit.as_ref()?;
        let vis = v.file_visible();
        let e = v.shown_sel(&vis);
        Some((vis.rep[e], v.entries.get(e)?.clone()))
    }

    /// File list Enter: a directory opens or closes; a file is jumped to
    /// and the diff focused.
    pub(super) fn file_open(&mut self) {
        let Some((row, e)) = self.shown_entry() else {
            return;
        };
        if e.is_dir() {
            let Some(v) = &mut self.commit else { return };
            if !v.collapsed.remove(&e.path) {
                v.collapsed.insert(e.path);
            }
            return self.select_row(row);
        }
        self.select_row(row);
        self.set_focus(super::Focus::Diff);
    }

    /// File list `h`: close an open directory, else go to the parent.
    pub(super) fn file_collapse(&mut self) {
        let Some((row, e)) = self.shown_entry() else {
            return;
        };
        let Some(v) = &mut self.commit else { return };
        if e.is_dir() && !v.collapsed.contains(&e.path) {
            v.collapsed.insert(e.path);
            return self.select_row(row);
        }
        let vis = v.file_visible();
        if let Some(p) = parent(&v.entries, v.shown_sel(&vis)) {
            let r = vis.rep[p];
            self.select_row(r);
        }
    }

    /// File list `l`: open a closed directory; on a file, Enter.
    pub(super) fn file_expand(&mut self) {
        let Some((row, e)) = self.shown_entry() else {
            return;
        };
        if !e.is_dir() {
            return self.file_open();
        }
        if let Some(v) = &mut self.commit {
            v.collapsed.remove(&e.path);
        }
        self.select_row(row);
    }

    /// File list `-` (`collapse`) / `=`: close or open every directory.
    pub(super) fn file_all(&mut self, collapse: bool) {
        let Some(v) = &mut self.commit else { return };
        let vis = v.file_visible();
        let sel = v.shown_sel(&vis);
        v.collapsed = if collapse {
            v.entries
                .iter()
                .filter(|e| e.is_dir())
                .map(|e| e.path.clone())
                .collect()
        } else {
            BTreeSet::new()
        };
        // The selection moves to the row that shows it now.
        let vis = v.file_visible();
        v.side_sel = vis.rows[vis.rep[sel]];
    }

    /// File list `` ` ``: tree / flat.
    pub(super) fn toggle_files(&mut self) {
        self.files_tree = !self.files_tree;
        self.set_status(if self.files_tree { "tree" } else { "flat" });
        if let Some(v) = &mut self.commit {
            v.set_tree(self.files_tree);
        }
        self.refresh_search();
    }

    /// The file list's selected file: the selected entry's, or a
    /// directory's first file.
    pub(super) fn selected_file(&self) -> Option<usize> {
        let v = self.commit.as_ref()?;
        let vis = v.file_visible();
        v.first_file(v.shown_sel(&vis))
    }
}

impl App {
    fn files_follow(&self, l: &FileList) -> super::commit::FilesFollow {
        if self.focus == super::Focus::Files {
            (true, Some(l.selected))
        } else {
            (false, l.current)
        }
    }

    /// The file list's first drawn row in a list `h` rows tall. It keeps
    /// the selection on screen while the list has the keys, else the diff
    /// cursor's file: the least scroll that does, or, after a wheel scroll
    /// or a click, the least move from the row left there. That row holds
    /// as it was (even with the selection off screen) until what the view
    /// follows changes, a key moves in the list, or the height changes.
    pub fn files_skip(&self, h: usize) -> usize {
        let Some(l) = self.file_list() else { return 0 };
        let follow = self.files_follow(&l);
        let max = l.visible.rows.len().saturating_sub(h);
        let top = match self.commit.as_ref().and_then(|v| v.files_top) {
            Some(t) if t.follow == Some(follow) && t.h == h => return t.top.min(max),
            Some(t) => t.top,
            None => 0,
        };
        let top = match follow.1 {
            Some(r) if r < top => r,
            Some(r) if r >= top + h => r + 1 - h,
            _ => top,
        };
        top.min(max)
    }

    /// Leave the first row of the file list, `h` rows tall, at `top`
    /// until what it follows changes (see [`App::files_skip`]).
    pub(super) fn files_pin(&mut self, top: usize, h: usize) {
        let Some(l) = self.file_list() else { return };
        let follow = Some(self.files_follow(&l));
        if let Some(v) = &mut self.commit {
            v.files_top = Some(FilesTop { top, h, follow });
        }
    }

    /// A key moved in the file list: its view shows the selection again,
    /// moving as little as it must from where it is.
    pub(super) fn files_unpin(&mut self) {
        if let Some(t) = self.commit.as_mut().and_then(|v| v.files_top.as_mut()) {
            t.follow = None;
        }
    }

    /// Scroll the file list's view, `h` rows tall, `delta` rows (down
    /// when positive); the selection and the diff stay.
    pub(super) fn files_wheel(&mut self, delta: isize, h: usize) {
        let Some(len) = self.file_list().map(|l| l.visible.rows.len()) else {
            return;
        };
        let top = self
            .files_skip(h)
            .saturating_add_signed(delta)
            .min(len.saturating_sub(h));
        self.files_pin(top, h);
    }

    /// Rows `C-d` / `C-u` move in the file list: half its tree's height.
    pub(super) fn files_half(&self) -> usize {
        let area = ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(1));
        match self.commit_layout(area).sidebar {
            Some(tree) => (tree.height as usize / 2).max(1),
            None => self.commit.as_ref().map_or(1, |v| (v.view_h / 2).max(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| {
                let kind = if e.is_dir() { "d" } else { "f" };
                format!(
                    "{}{kind} {} +{} -{}",
                    "  ".repeat(e.depth),
                    e.name,
                    e.add,
                    e.del
                )
            })
            .collect()
    }

    const FILES: &[FileItem] = &[
        ("z.txt", 1, 0),
        ("pkg/gui/controllers/helpers/a.go", 2, 1),
        ("pkg/gui/controllers/helpers/b.go", 3, 0),
        ("src/main.rs", 1, 1),
        ("src/app/x.rs", 4, 0),
        ("a.txt", 0, 2),
    ];

    #[test]
    fn dirs_first_alphabetical_and_single_child_chains_compressed() {
        assert_eq!(
            show(&build_tree(FILES)),
            [
                "d pkg/gui/controllers/helpers +5 -1",
                "  f a.go +2 -1",
                "  f b.go +3 -0",
                "d src +5 -1",
                "  d app +4 -0",
                "    f x.rs +4 -0",
                "  f main.rs +1 -1",
                "f a.txt +0 -2",
                "f z.txt +1 -0",
            ]
        );
        let t = build_tree(FILES);
        assert_eq!(t[4].path, "src/app");
        assert_eq!(t[1].file, Some(1));
        // A file alone in a directory is never folded into it.
        let one = build_tree(&[("a/b/c.rs", 1, 0)]);
        assert_eq!(show(&one), ["d a/b +1 -0", "  f c.rs +1 -0"]);
    }

    #[test]
    fn flat_keeps_git_order_and_whole_paths() {
        let f = build_flat(FILES);
        let names: Vec<_> = f.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, FILES.iter().map(|f| f.0).collect::<Vec<_>>());
    }

    #[test]
    fn collapsed_dirs_hide_their_rows_and_represent_them() {
        let t = build_tree(FILES);
        let all = visible(&t, &BTreeSet::new());
        assert_eq!(all.rows, (0..t.len()).collect::<Vec<_>>());
        let closed: BTreeSet<String> = ["src".to_string()].into();
        let v = visible(&t, &closed);
        assert_eq!(v.rows, [0, 1, 2, 3, 7, 8]);
        // x.rs (entry 5) shows as the src row (row 3).
        assert_eq!(v.rep[5], 3);
        assert_eq!(v.rep[7], 4);
        let inner: BTreeSet<String> = ["src/app".to_string()].into();
        assert_eq!(visible(&t, &inner).rows, [0, 1, 2, 3, 4, 6, 7, 8]);
        assert_eq!(parent(&t, 5), Some(4));
        assert_eq!(parent(&t, 4), Some(3));
        assert_eq!(parent(&t, 3), None);
    }

    #[test]
    fn indent_stops_where_the_name_would_get_too_narrow() {
        assert_eq!(indent(2, 40, 3), 4);
        assert_eq!(indent(10, 20, 3), 8);
        assert_eq!(indent(3, 5, 3), 0);
    }

    #[test]
    fn middle_clipping_keeps_the_extension() {
        assert_eq!(clip_middle("short.rs", 20), "short.rs");
        let c = clip_middle("a_very_long_module_name.rs", 14);
        assert!(c.contains('…') && c.ends_with("ame.rs"), "{c}");
        assert_eq!(c.width(), 14);
        let c = clip_middle("report-final.markdown", 12);
        assert!(c.ends_with(".markdown"), "{c}");
        assert_eq!(clip_middle("abc.md", 1), "…");
        assert_eq!(clip_middle("abc.md", 0), "");
    }
}
