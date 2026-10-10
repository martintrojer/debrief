//! One batch of comments per repo root, stored as JSON in the user cache dir.
//!
//! Every mutation takes an exclusive `flock` on a sibling `.lock`, re-reads the
//! file, applies the change and writes by rename, so two processes on the same
//! root can't lose each other's comments. Reads take a shared lock when the lock
//! file exists. Opening never creates anything.
//!
//! Ids are unique within a batch. [`Review::add`] never reuses an id for a
//! root; [`Review::restore`] may bring a comment back under its former id.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};

use crate::comment::Comment;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Batch {
    version: u32,
    root: PathBuf,
    comments: Vec<Comment>,
}

/// The open batch of comments for one repo root.
#[derive(Debug)]
pub struct Review {
    root: PathBuf,
    file: PathBuf,
    lock: PathBuf,
    comments: Vec<Comment>,
}

impl Review {
    /// Opens the batch for `root` (canonicalised) under `cache_dir`. Reads the file if present; creates nothing.
    pub fn open(cache_dir: &Path, root: &Path) -> io::Result<Review> {
        let root = root.canonicalize()?;
        let hash = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
        let dir = cache_dir.join("reviews");
        let stem = &hash.as_str()[..16];
        let mut review = Review {
            file: dir.join(format!("{stem}.json")),
            lock: dir.join(format!("{stem}.lock")),
            root,
            comments: Vec::new(),
        };
        review.comments = review.read_shared()?;
        Ok(review)
    }

    /// The canonical repo root this batch belongs to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The comments, sorted by path, then first line, then id.
    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    /// Adds `c` with the current time and a fresh id; returns the id.
    ///
    /// The id is one past the largest of the existing ids, the last id issued
    /// for this root (kept in the lock file, which outlives the batch file) and
    /// the current unix time in milliseconds. So ids never repeat for a root,
    /// even after the batch file is deleted and a new one started within the
    /// same millisecond.
    pub fn add(&mut self, mut c: Comment) -> io::Result<u64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        c.created = now.as_secs();
        let now_ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX - 1);
        let mut lock = self.lock_exclusive()?;
        let mut comments = self.read_file()?;
        let max = comments.iter().map(|c| c.id).max().unwrap_or(0);
        c.id = max.max(last_issued(&mut lock)).max(now_ms) + 1;
        let id = c.id;
        comments.push(c);
        sort(&mut comments);
        self.write_file(&comments)?;
        record_issued(&mut lock, id)?;
        self.comments = comments;
        Ok(id)
    }

    /// Adds back comments saved elsewhere (a review session), under the
    /// exclusive lock after re-reading the file. A comment already in the
    /// batch (same id, path, revision, side, lines and creation time) is
    /// skipped: the batch's version wins. Any other comment keeps its id
    /// when no comment in the batch (or restored before it in this call)
    /// has it, else it gets a fresh id as [`Review::add`] would issue.
    /// The last issued id becomes the largest of it and every id restored,
    /// never lower. Returns `(old id, new id)` per comment added.
    pub fn restore(&mut self, restored: &[Comment]) -> io::Result<Vec<(u64, u64)>> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX - 1))
            .unwrap_or_default();
        let mut lock = self.lock_exclusive()?;
        let mut comments = self.read_file()?;
        let mut last = last_issued(&mut lock);
        let mut out = Vec::new();
        for c in restored {
            if comments.iter().any(|b| same_comment(b, c)) {
                continue;
            }
            let old = c.id;
            let mut c = c.clone();
            if comments.iter().any(|b| b.id == c.id) {
                let max = comments.iter().map(|b| b.id).max().unwrap_or(0);
                c.id = max.max(last).max(now_ms) + 1;
            }
            last = last.max(c.id);
            out.push((old, c.id));
            comments.push(c);
        }
        if !out.is_empty() {
            sort(&mut comments);
            self.write_file(&comments)?;
            record_issued(&mut lock, last)?;
        }
        self.comments = comments;
        Ok(out)
    }

    /// Replaces the body of comment `id`; false if there is no such comment.
    pub fn edit(&mut self, id: u64, body: &str) -> io::Result<bool> {
        let r = self.mutate(|comments| {
            let c = comments.iter_mut().find(|c| c.id == id)?;
            c.body = body.to_owned();
            Some(())
        })?;
        Ok(r.is_some())
    }

    /// Sets the kind of comment `id` (`None` is untyped); false if there is no such comment.
    pub fn set_kind(&mut self, id: u64, kind: Option<&str>) -> io::Result<bool> {
        let r = self.mutate(|comments| {
            let c = comments.iter_mut().find(|c| c.id == id)?;
            c.kind = kind.map(str::to_owned);
            Some(())
        })?;
        Ok(r.is_some())
    }

    /// Removes comment `id`; false if there is no such comment.
    pub fn remove(&mut self, id: u64) -> io::Result<bool> {
        let r = self.mutate(|comments| {
            let i = comments.iter().position(|c| c.id == id)?;
            comments.remove(i);
            Some(())
        })?;
        Ok(r.is_some())
    }

    /// Removes the comments whose id is in `ids` (ids already gone are ignored)
    /// and returns how many were removed. Comments added since this handle last
    /// read the file survive, so a sender can drop exactly what it sent.
    /// Deletes the batch file when nothing is left.
    pub fn remove_all(&mut self, ids: &[u64]) -> io::Result<usize> {
        let _lock = self.lock_exclusive()?;
        let mut comments = self.read_file()?;
        let before = comments.len();
        comments.retain(|c| !ids.contains(&c.id));
        let removed = before - comments.len();
        if removed > 0 {
            if comments.is_empty() {
                remove_if_exists(&self.file)?;
            } else {
                self.write_file(&comments)?;
            }
        }
        self.comments = comments;
        Ok(removed)
    }

    /// Drops every comment and removes the batch file. Like every mutation it
    /// re-reads the file first, so a batch for another root is an error, not deleted.
    pub fn clear(&mut self) -> io::Result<()> {
        let _lock = self.lock_exclusive()?;
        self.read_file()?;
        remove_if_exists(&self.file)?;
        self.comments.clear();
        Ok(())
    }

    /// Re-reads the file; true if the comments changed on disk.
    pub fn reload(&mut self) -> io::Result<bool> {
        let comments = self.read_shared()?;
        let changed = comments != self.comments;
        self.comments = comments;
        Ok(changed)
    }

    /// Runs `f` on the on-disk comments under the exclusive lock and writes the
    /// result if `f` returns `Some`. Always refreshes `self.comments`.
    fn mutate<T>(
        &mut self,
        f: impl FnOnce(&mut Vec<Comment>) -> Option<T>,
    ) -> io::Result<Option<T>> {
        let _lock = self.lock_exclusive()?;
        let mut comments = self.read_file()?;
        let r = f(&mut comments);
        if r.is_some() {
            sort(&mut comments);
            self.write_file(&comments)?;
        }
        self.comments = comments;
        Ok(r)
    }

    fn lock_exclusive(&self) -> io::Result<File> {
        // The lock file also holds the last id issued (see `add`); it is
        // opened read-write and never truncated on open.
        if let Some(dir) = self.lock.parent() {
            fs::create_dir_all(dir)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock)?;
        flock(&f, FlockOperation::LockExclusive)?;
        Ok(f)
    }

    fn read_shared(&self) -> io::Result<Vec<Comment>> {
        let _lock = match File::open(&self.lock) {
            Ok(f) => {
                flock(&f, FlockOperation::LockShared)?;
                Some(f)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        self.read_file()
    }

    fn read_file(&self) -> io::Result<Vec<Comment>> {
        let bytes = match fs::read(&self.file) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let batch: Batch = serde_json::from_slice(&bytes).map_err(invalid)?;
        if batch.version != VERSION {
            return Err(invalid(format!(
                "{}: unsupported version {}",
                self.file.display(),
                batch.version
            )));
        }
        if batch.root != self.root {
            return Err(invalid(format!(
                "{} belongs to {}, not {}",
                self.file.display(),
                batch.root.display(),
                self.root.display()
            )));
        }
        let mut comments = batch.comments;
        sort(&mut comments);
        Ok(comments)
    }

    fn write_file(&self, comments: &[Comment]) -> io::Result<()> {
        let batch = Batch {
            version: VERSION,
            root: self.root.clone(),
            comments: comments.to_vec(),
        };
        let json = serde_json::to_vec_pretty(&batch).map_err(invalid)?;
        let mut tmp = self.file.clone().into_os_string();
        tmp.push(".tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.file)
    }
}

/// `b` is `c` as saved: the same id and anchor (body and kind may have
/// been edited since).
fn same_comment(b: &Comment, c: &Comment) -> bool {
    b.id == c.id
        && b.path == c.path
        && b.rev.as_ref().map(|r| &r.commit) == c.rev.as_ref().map(|r| &r.commit)
        && b.side == c.side
        && b.lines == c.lines
        && b.created == c.created
}

fn sort(comments: &mut [Comment]) {
    comments.sort_by(|a, b| (&a.path, a.lines.0, a.id).cmp(&(&b.path, b.lines.0, b.id)));
}

/// The last id issued for this root, from the held lock file (0 if none).
fn last_issued(lock: &mut File) -> u64 {
    use std::io::{Read, Seek, SeekFrom};
    let mut text = String::new();
    if lock.seek(SeekFrom::Start(0)).is_err() || lock.read_to_string(&mut text).is_err() {
        return 0;
    }
    text.trim().parse().unwrap_or(0)
}

/// Records `id` as the last id issued, in the held lock file.
fn record_issued(lock: &mut File, id: u64) -> io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    lock.set_len(0)?;
    lock.seek(SeekFrom::Start(0))?;
    writeln!(lock, "{id}")
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn invalid(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// `$DEBRIEF_CACHE_DIR` if set, else the user cache dir plus `debrief`.
pub fn cache_dir() -> Option<PathBuf> {
    match std::env::var_os("DEBRIEF_CACHE_DIR") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => dirs::cache_dir().map(|d| d.join("debrief")),
    }
}

/// The nearest ancestor of `path` (or of its parent, if a file) holding `.jj` or
/// `.git`; else that directory. Canonicalised when possible.
pub fn repo_root(path: &Path) -> PathBuf {
    let dir = if path.is_file() {
        match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        }
    } else {
        path
    };
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors()
        .find(|d| d.join(".jj").exists() || d.join(".git").exists())
        .unwrap_or(&dir)
        .to_path_buf()
}
