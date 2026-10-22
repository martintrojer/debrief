//! Merge folds over the first-parent spine.
//!
//! With folding on (`[view] fold_merges`, [`App::set_folding`]) the log
//! lists the spine (`Vcs::log_in(.., spine = true)`), kept apart in
//! [`Folds::spine`] as the source of truth: every rebuild splices the open
//! folds' loaded groups into `all_rows` from it, so the filter, cursor,
//! motions and paging keep working over a flat `Vec<Row>`.
//!
//! A merge row's fold has one group per non-first parent
//! ([`Vcs::fold`](crate::vcs::Vcs::fold)), loaded when the fold first
//! opens: on a worker with an event sender ([`AppEvent::FoldLoaded`]),
//! inline without one. Merges inside a group are folds too, keyed by ID.
//! A group of a full page ends with a `… more` row (ID
//! [`FOLD_MORE_PREFIX`]`<merge>:<parent>`) that loads the group's next
//! page ([`AppEvent::FoldPage`]). Merge sizes load once per merge after
//! the rows that list it ([`AppEvent::MergeSizes`]).
//!
//! Open folds and loaded groups survive a reload for merges still
//! listed; a new query (path, branch) drops the loaded groups and
//! re-reads the open ones. A revset whose spine fell back to a flat list
//! (several heads) is shown without folds.

use std::collections::{HashMap, HashSet};

use crate::seen::Status;
use crate::vcs::{CommitInfo, OpenOptions};

use super::log::{FOLD_MORE_PREFIX, RowKey, is_fold_more, is_zero_id};
use super::{App, AppEvent, Fold, Group, MORE_ROW, Row};

/// Said when a revset's spine fell back to a flat list.
pub(super) const FLAT: &str = "merges flat: several heads";

/// One group's paging.
#[derive(Debug, Clone, Copy)]
struct GroupPage {
    /// Commits asked of the VCS for the group.
    limit: usize,
    /// It returned as many as asked: more may exist.
    more: bool,
}

/// The fold state, owned by [`App`].
#[derive(Debug, Default)]
pub(super) struct Folds {
    /// Folding wanted (`[view] fold_merges`, `set_folding`).
    pub(super) on: bool,
    /// The spine fell back to a flat list: folding is off for this query.
    flat: bool,
    /// What the log query returned (trimmed): the spine while folding.
    pub(super) spine: Vec<CommitInfo>,
    /// Open merges, by ID.
    open: HashSet<String>,
    /// Loaded groups by merge: (parent, commits newest first).
    loaded: HashMap<String, Vec<(String, Vec<CommitInfo>)>>,
    pages: HashMap<(String, String), GroupPage>,
    /// Merge sizes by ID (a size is the commit's, whatever the query).
    sizes: HashMap<String, usize>,
    /// Merges whose size is loading or failed to load.
    sizing: HashSet<String>,
    /// Merges whose load opens their nested merges too (`zR`).
    cascade: HashSet<String>,
    /// Fold loads in flight: merge -> (request, parents, limit).
    pending: HashMap<String, (u64, Vec<String>, usize)>,
    /// Group pages in flight: (merge, parent) -> (request, limit).
    page_pending: HashMap<(String, String), (u64, usize)>,
    request: u64,
    /// Per `all_rows` entry, its group instance (0: the spine).
    pub(super) row_group: Vec<usize>,
}

/// Drop zero parents (jj's root) and zero IDs.
fn trim(mut commits: Vec<CommitInfo>) -> Vec<CommitInfo> {
    commits.retain(|c| !is_zero_id(&c.commit));
    for c in &mut commits {
        c.parents.retain(|p| !is_zero_id(p));
    }
    commits
}

/// The spine `commits` fell back to a flat list (a revset with several
/// heads). Without a path a spine is a first-parent chain; with one it
/// skips commits, so the sign is a listed commit that is a non-first
/// parent of a listed merge.
fn is_flat(commits: &[CommitInfo], path: bool) -> bool {
    if !path {
        return commits
            .windows(2)
            .any(|w| w[0].parents.first() != Some(&w[1].commit));
    }
    let ids: HashSet<&str> = commits.iter().map(|c| c.commit.as_str()).collect();
    commits
        .iter()
        .any(|c| c.parents.iter().skip(1).any(|p| ids.contains(p.as_str())))
}

/// The `… more` row ID of `merge`'s group of `parent`.
fn more_id(merge: &str, parent: &str) -> String {
    format!("{FOLD_MORE_PREFIX}{merge}:{parent}")
}

/// `(merge, parent)` of a `… more` row ID.
fn parse_more(id: &str) -> Option<(String, String)> {
    let (m, p) = id.strip_prefix(FOLD_MORE_PREFIX)?.split_once(':')?;
    Some((m.to_string(), p.to_string()))
}

/// Where the cursor goes after a rebuild: its row, else the enclosing
/// merges (innermost first), else the `above` rows (nearest first).
pub(super) struct Anchor {
    key: Option<RowKey>,
    fallback: Vec<String>,
}

/// Read every group of `merge` on a worker thread.
fn spawn_fold(
    (root, opts, pin): (std::path::PathBuf, OpenOptions, Option<String>),
    (merge, parents, path, limit): (String, Vec<String>, Option<String>, usize),
    request: u64,
    tx: std::sync::mpsc::Sender<AppEvent>,
) {
    std::thread::spawn(move || {
        let result = crate::vcs::reopen(&root, opts, pin)
            .and_then(|v| {
                parents
                    .iter()
                    .map(|p| v.fold(&merge, p, path.as_deref(), limit))
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(AppEvent::FoldLoaded {
            request,
            merge,
            result,
        });
    });
}

impl App {
    /// Merges fold: the log lists the spine.
    pub fn folding(&self) -> bool {
        self.folds.on
    }

    /// Folds show on this query: folding is on and the spine did not
    /// fall back to a flat list.
    fn folds_shown(&self) -> bool {
        self.folds.on && !self.folds.flat
    }

    /// Turn folding on or off and reload. Off drops the open folds. The
    /// cursor stays on its commit, else goes to its merge, else to the
    /// nearest row above it that is still listed (turning on: the spine
    /// commit that landed it), else row 0. An open commit stays open.
    pub fn set_folding(&mut self, on: bool) {
        if self.folds.on == on {
            return;
        }
        let anchor = self.anchor(self.cursor, true);
        let was = self.folds.on;
        self.folds.on = on;
        if !on {
            self.folds.open.clear();
            self.folds.cascade.clear();
        }
        self.page_reset();
        match self.load(false) {
            Ok(commits) => {
                self.set_rows(commits);
                if !self.restore(anchor) {
                    self.cursor = 0;
                }
                self.commit_reindex();
                self.keep_visible();
            }
            Err(e) => {
                self.folds.on = was;
                self.set_status(format!("reload failed: {e:#}"));
            }
        }
    }

    /// The anchor of row `i` (see [`Anchor`]); `above` adds the rows
    /// above it.
    pub(super) fn anchor(&self, i: usize, above: bool) -> Anchor {
        let mut fallback = self.enclosing(i);
        if above {
            let n = i.min(self.rows.len());
            fallback.extend(self.rows[..n].iter().rev().map(|r| r.info.commit.clone()));
        }
        Anchor {
            key: self.log_key(i),
            fallback,
        }
    }

    /// Put the cursor on `a`'s row or its first listed fallback; false if
    /// none is listed.
    pub(super) fn restore(&mut self, a: Anchor) -> bool {
        if let Some(i) = a.key.and_then(|k| self.row_of(&k)) {
            self.cursor = i;
            return true;
        }
        for id in a.fallback {
            if let Some(i) = self.rows.iter().position(|r| r.info.commit == id) {
                self.cursor = i;
                return true;
            }
        }
        false
    }

    /// The merges whose folds hold row `i`, innermost first.
    fn enclosing(&self, i: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut m = self.rows.get(i).and_then(|r| r.merge.clone());
        while let Some(id) = m {
            m = self
                .all_rows
                .iter()
                .find(|r| r.info.commit == id)
                .and_then(|r| r.merge.clone());
            out.push(id);
        }
        out
    }

    /// Open the fold of the merge at row `i`. On a fold's `… more` row,
    /// load that group's next page instead.
    pub fn fold_open(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else { return };
        if is_fold_more(&row.info.commit) {
            let id = row.info.commit.clone();
            return self.fold_more(&id);
        }
        if row.fold.is_none_or(|f| f.open) {
            return;
        }
        self.folds.open.insert(row.info.commit.clone());
        self.fold_rebuild();
    }

    /// Close the fold of the open merge at row `i`; on a row inside a
    /// fold, close its merge and move the cursor there.
    pub fn fold_close(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else { return };
        let id = if row.fold.is_some_and(|f| f.open) {
            row.info.commit.clone()
        } else if let Some(m) = &row.merge {
            m.clone()
        } else {
            return;
        };
        self.folds.open.remove(&id);
        self.folds.cascade.remove(&id);
        self.fold_rebuild();
        if let Some(j) = self.rows.iter().position(|r| r.info.commit == id) {
            self.cursor = j;
            self.commit_reindex();
            self.keep_visible();
        }
    }

    /// Open every fold listed, nested ones included (those loading open
    /// theirs once in).
    pub fn folds_open_all(&mut self) {
        if !self.folds_shown() {
            return;
        }
        let mut stack: Vec<String> = self
            .all_rows
            .iter()
            .filter(|r| r.fold.is_some())
            .map(|r| r.info.commit.clone())
            .collect();
        let mut visited = HashSet::new();
        while let Some(id) = stack.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            self.folds.open.insert(id.clone());
            match self.folds.loaded.get(&id) {
                Some(groups) => {
                    for (_, g) in groups {
                        let nested = g.iter().filter(|c| c.parents.len() > 1);
                        stack.extend(nested.map(|c| c.commit.clone()));
                    }
                }
                None => {
                    self.folds.cascade.insert(id);
                }
            }
        }
        self.fold_rebuild();
    }

    /// Close every fold; the cursor stays on its commit, else goes to its
    /// outermost merge.
    pub fn folds_close_all(&mut self) {
        let mut anchor = self.anchor(self.cursor, false);
        anchor.fallback.reverse();
        self.folds.open.clear();
        self.folds.cascade.clear();
        self.set_rows(self.folds.spine.clone());
        self.restore(anchor);
        self.commit_reindex();
        self.keep_visible();
    }

    /// The commits loaded so far in the fold of the merge at row `i`,
    /// nested ones where their own fold is loaded; `None` while the fold
    /// is not loaded.
    pub fn bubble(&self, i: usize) -> Option<Vec<CommitInfo>> {
        let row = self.rows.get(i)?;
        self.folds.loaded.get(&row.info.commit)?;
        let mut out = Vec::new();
        self.loaded_bubble(&row.info.commit, &mut out, &mut HashSet::new());
        Some(out)
    }

    /// Push `merge`'s loaded fold commits (nested where loaded) to `out`.
    fn loaded_bubble(&self, merge: &str, out: &mut Vec<CommitInfo>, seen: &mut HashSet<String>) {
        let Some(groups) = self.folds.loaded.get(merge) else {
            return;
        };
        for (_, g) in groups {
            for c in g {
                if seen.insert(c.commit.clone()) {
                    out.push(c.clone());
                    self.loaded_bubble(&c.commit, out, seen);
                }
            }
        }
    }

    /// `commits` followed by the loaded fold commits of the merges among
    /// them, each once.
    pub(super) fn with_bubbles(&self, commits: Vec<CommitInfo>) -> Vec<CommitInfo> {
        let mut seen: HashSet<String> = commits.iter().map(|c| c.commit.clone()).collect();
        let mut out = Vec::new();
        for c in &commits {
            self.loaded_bubble(&c.commit, &mut out, &mut seen);
        }
        let mut all = commits;
        all.extend(out);
        all
    }

    /// The spine row that row `i` landed with: itself on the spine, else
    /// its outermost enclosing merge.
    pub(super) fn spine_of(&self, i: usize) -> Option<String> {
        let row = self.rows.get(i)?;
        Some(
            self.enclosing(i)
                .pop()
                .unwrap_or_else(|| row.info.commit.clone()),
        )
    }

    /// Rebuild the rows from the spine, the cursor kept on its commit
    /// (else its merge).
    pub(super) fn fold_rebuild(&mut self) {
        let anchor = self.anchor(self.cursor, false);
        self.set_rows(self.folds.spine.clone());
        self.restore(anchor);
        self.commit_reindex();
        self.keep_visible();
    }

    /// A new query: loaded groups are dropped (open folds re-read).
    pub(super) fn folds_requery(&mut self) {
        self.folds.loaded.clear();
        self.folds.pages.clear();
        self.folds.pending.clear();
        self.folds.page_pending.clear();
    }

    /// The spine as all rows: git's working row, then each spine commit
    /// with its open folds' rows. Drops fold state for merges no longer
    /// listed.
    pub(super) fn splice(&mut self, commits: Vec<CommitInfo>) -> Vec<Row> {
        let was_flat = self.folds.flat;
        self.folds.flat = self.folds.on && is_flat(&commits, self.opts.path.is_some());
        if self.folds.flat && !was_flat {
            self.set_status(FLAT);
        }
        self.folds.spine = commits;
        let mut rows: Vec<Row> = self.working_row().into_iter().collect();
        let mut groups = vec![0; rows.len()];
        let mut next_group = 0;
        let shown = self.folds_shown();
        let spine = std::mem::take(&mut self.folds.spine);
        for info in &spine {
            self.push_row(&mut rows, &mut groups, info, (0, &[], None), 0);
            if shown {
                self.push_fold(&mut rows, &mut groups, &mut next_group, info, 0, &[]);
            }
        }
        self.folds.spine = spine;
        // Keep state only for merges listed now or reachable through
        // loaded groups (nested in a closed fold).
        let mut keep: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = self
            .folds
            .spine
            .iter()
            .filter(|c| c.parents.len() > 1)
            .map(|c| c.commit.clone())
            .collect();
        while let Some(id) = stack.pop() {
            if !keep.insert(id.clone()) {
                continue;
            }
            for (_, g) in self.folds.loaded.get(&id).into_iter().flatten() {
                stack.extend(
                    g.iter()
                        .filter(|c| c.parents.len() > 1)
                        .map(|c| c.commit.clone()),
                );
            }
        }
        if !shown {
            keep.clear();
        }
        self.folds.open.retain(|id| keep.contains(id));
        self.folds.cascade.retain(|id| keep.contains(id));
        self.folds.loaded.retain(|id, _| keep.contains(id));
        self.folds.pages.retain(|(m, _), _| keep.contains(m));
        self.folds.row_group = groups;
        rows
    }

    fn push_row(
        &self,
        rows: &mut Vec<Row>,
        groups: &mut Vec<usize>,
        info: &CommitInfo,
        (depth, enclosing, merge): (u8, &[Group], Option<&str>),
        group: usize,
    ) {
        let fold = (self.folds_shown() && info.parents.len() > 1).then(|| Fold {
            size: self.folds.sizes.get(&info.commit).copied(),
            open: self.folds.open.contains(&info.commit),
        });
        rows.push(Row {
            status: self.seen.status(info),
            working: self.working_commit.as_deref() == Some(info.commit.as_str()),
            info: info.clone(),
            depth,
            group: enclosing.last().copied(),
            groups: enclosing.to_vec(),
            fold,
            merge: merge.map(str::to_string),
        });
        groups.push(group);
    }

    /// `merge`'s rows if its fold is open and loaded, recursively.
    fn push_fold(
        &self,
        rows: &mut Vec<Row>,
        groups: &mut Vec<usize>,
        next_group: &mut usize,
        merge: &CommitInfo,
        depth: u8,
        enclosing: &[Group],
    ) {
        if merge.parents.len() < 2 || !self.folds.open.contains(&merge.commit) {
            return;
        }
        let Some(loaded) = self.folds.loaded.get(&merge.commit) else {
            return;
        };
        let n = loaded.len();
        let mut seen = HashSet::new();
        let depth = depth.saturating_add(1);
        for (gi, (parent, commits)) in loaded.iter().enumerate() {
            *next_group += 1;
            let g = *next_group;
            let mut marks = enclosing.to_vec();
            if n > 1 {
                marks.push(if gi + 1 < n { Group::Mid } else { Group::Last });
            }
            let at = (depth, marks.as_slice(), Some(merge.commit.as_str()));
            for c in commits {
                if !seen.insert(c.commit.clone()) {
                    continue;
                }
                self.push_row(rows, groups, c, at, g);
                self.push_fold(rows, groups, next_group, c, depth, &marks);
            }
            let key = (merge.commit.clone(), parent.clone());
            if self.folds.pages.get(&key).is_some_and(|p| p.more) {
                rows.push(Row {
                    info: CommitInfo {
                        commit: more_id(&merge.commit, parent),
                        change: None,
                        author: String::new(),
                        time: 0,
                        summary: MORE_ROW.to_string(),
                        parents: Vec::new(),
                    },
                    status: Status::Seen,
                    working: false,
                    depth,
                    group: marks.last().copied(),
                    groups: marks.clone(),
                    fold: None,
                    merge: Some(merge.commit.clone()),
                });
                groups.push(g);
            }
        }
    }

    /// After a rebuild: load the open folds not loaded yet and the sizes
    /// of merges listed without one.
    pub(super) fn folds_follow(&mut self) {
        if !self.folds_shown() {
            return;
        }
        let todo: Vec<CommitInfo> = self
            .all_rows
            .iter()
            .filter(|r| r.fold.is_some_and(|f| f.open))
            .filter(|r| {
                !self.folds.loaded.contains_key(&r.info.commit)
                    && !self.folds.pending.contains_key(&r.info.commit)
            })
            .map(|r| r.info.clone())
            .collect();
        for m in todo {
            self.fold_load(m);
        }
        self.sizes_request();
    }

    /// Load every group of merge `m` (inline without a sender).
    fn fold_load(&mut self, m: CommitInfo) {
        self.folds.request += 1;
        let request = self.folds.request;
        let limit = self.page_size();
        let parents = m.parents[1..].to_vec();
        self.folds
            .pending
            .insert(m.commit.clone(), (request, parents.clone(), limit));
        match self.sender.clone() {
            Some(tx) => {
                let at = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                let q = (m.commit, parents, self.opts.path.clone(), limit);
                spawn_fold(at, q, request, tx);
            }
            None => {
                let path = self.opts.path.as_deref();
                let result = parents
                    .iter()
                    .map(|p| self.vcs.fold(&m.commit, p, path, limit))
                    .collect::<anyhow::Result<Vec<_>>>()
                    .map_err(|e| format!("{e:#}"));
                self.fold_loaded(request, &m.commit, result);
            }
        }
    }

    /// The worker's fold contents: kept if they answer the last request
    /// for `merge`.
    pub(super) fn fold_loaded(
        &mut self,
        request: u64,
        merge: &str,
        result: Result<Vec<Vec<CommitInfo>>, String>,
    ) {
        if self.folds.pending.get(merge).map(|p| p.0) != Some(request) {
            return;
        }
        let Some((_, parents, limit)) = self.folds.pending.remove(merge) else {
            return;
        };
        let groups = match result {
            Ok(g) => g,
            Err(e) => {
                self.folds.open.remove(merge);
                self.set_status(format!("cannot load the fold: {e}"));
                return self.fold_rebuild();
            }
        };
        let mut loaded = Vec::new();
        for (p, g) in parents.into_iter().zip(groups) {
            let more = limit > 0 && g.len() >= limit;
            self.folds
                .pages
                .insert((merge.to_string(), p.clone()), GroupPage { limit, more });
            loaded.push((p, trim(g)));
        }
        if self.folds.cascade.remove(merge) {
            for (_, g) in &loaded {
                for c in g.iter().filter(|c| c.parents.len() > 1) {
                    self.folds.open.insert(c.commit.clone());
                    self.folds.cascade.insert(c.commit.clone());
                }
            }
        }
        self.folds.loaded.insert(merge.to_string(), loaded);
        self.fold_rebuild();
    }

    /// A fold's `… more` row `id`: load its group's next page.
    fn fold_more(&mut self, id: &str) {
        let Some(key) = parse_more(id) else { return };
        if self.folds.page_pending.contains_key(&key) {
            return;
        }
        let Some(page) = self.folds.pages.get(&key).filter(|p| p.more) else {
            return;
        };
        let limit = page.limit + self.page_size();
        self.folds.request += 1;
        let request = self.folds.request;
        self.folds
            .page_pending
            .insert(key.clone(), (request, limit));
        let (merge, parent) = key;
        match self.sender.clone() {
            Some(tx) => {
                let (root, opts, pin) = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                let path = self.opts.path.clone();
                std::thread::spawn(move || {
                    let result = crate::vcs::reopen(&root, opts, pin)
                        .and_then(|v| v.fold(&merge, &parent, path.as_deref(), limit))
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::FoldPage {
                        request,
                        merge,
                        parent,
                        result,
                    });
                });
            }
            None => {
                let result = self
                    .vcs
                    .fold(&merge, &parent, self.opts.path.as_deref(), limit)
                    .map_err(|e| format!("{e:#}"));
                self.fold_page_loaded(request, &merge, &parent, result);
            }
        }
    }

    /// The worker's group page: kept if it answers the last request for
    /// the group. A cursor on the group's `… more` row stays at its
    /// place, now the first new commit.
    pub(super) fn fold_page_loaded(
        &mut self,
        request: u64,
        merge: &str,
        parent: &str,
        result: Result<Vec<CommitInfo>, String>,
    ) {
        let key = (merge.to_string(), parent.to_string());
        if self.folds.page_pending.get(&key).map(|p| p.0) != Some(request) {
            return;
        }
        let Some((_, limit)) = self.folds.page_pending.remove(&key) else {
            return;
        };
        let commits = match result {
            Ok(c) => c,
            Err(e) => return self.set_status(format!("cannot load more: {e}")),
        };
        let Some(group) = self
            .folds
            .loaded
            .get_mut(merge)
            .and_then(|gs| gs.iter_mut().find(|(p, _)| p == parent))
        else {
            return;
        };
        let more = limit > 0 && commits.len() >= limit;
        group.1 = trim(commits);
        self.folds.pages.insert(key, GroupPage { limit, more });
        let on_more = self
            .rows
            .get(self.cursor)
            .is_some_and(|r| r.info.commit == more_id(merge, parent));
        if on_more {
            let at = self.cursor;
            self.set_rows(self.folds.spine.clone());
            self.cursor = at.min(self.last_row());
            self.commit_reindex();
            self.keep_visible();
        } else {
            self.fold_rebuild();
        }
    }

    /// Ask for the sizes of listed merges without one (inline without a
    /// sender); nothing when there are none.
    fn sizes_request(&mut self) {
        let ids: Vec<String> = self
            .all_rows
            .iter()
            .filter(|r| r.fold.is_some_and(|f| f.size.is_none()))
            .map(|r| r.info.commit.clone())
            .filter(|id| !self.folds.sizing.contains(id))
            .collect();
        if ids.is_empty() {
            return;
        }
        self.folds.sizing.extend(ids.iter().cloned());
        self.folds.request += 1;
        let request = self.folds.request;
        match self.sender.clone() {
            Some(tx) => {
                let (root, opts, pin) = (
                    self.vcs.root().to_path_buf(),
                    self.vcs.open_options(),
                    self.vcs.pinned_op(),
                );
                std::thread::spawn(move || {
                    let result = crate::vcs::reopen(&root, opts, pin)
                        .and_then(|v| {
                            ids.into_iter()
                                .map(|id| v.merge_size(&id).map(|n| (id, n)))
                                .collect()
                        })
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::MergeSizes { request, result });
                });
            }
            None => {
                let result = ids
                    .into_iter()
                    .map(|id| self.vcs.merge_size(&id).map(|n| (id, n)))
                    .collect::<anyhow::Result<Vec<_>>>()
                    .map_err(|e| format!("{e:#}"));
                self.sizes_loaded(request, result);
            }
        }
    }

    /// Merge sizes from the worker: set on the rows in place (a size is
    /// the commit's, so any request's answer holds).
    pub(super) fn sizes_loaded(
        &mut self,
        _request: u64,
        result: Result<Vec<(String, usize)>, String>,
    ) {
        let sizes = match result {
            Ok(s) => s,
            Err(e) => return self.set_status(format!("cannot count merges: {e}")),
        };
        for (id, n) in sizes {
            self.folds.sizing.remove(&id);
            self.folds.sizes.insert(id, n);
        }
        let sizes = &self.folds.sizes;
        for r in self.rows.iter_mut().chain(self.all_rows.iter_mut()) {
            if let Some(f) = &mut r.fold {
                f.size = sizes.get(&r.info.commit).copied();
            }
        }
    }
}
