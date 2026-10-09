//! One batch of comments per repo root, stored as JSON in the user cache dir.
//!
//! Every mutation takes an exclusive `flock` on a sibling `.lock`, re-reads the
//! file, applies the change and writes by rename, so two processes on the same
//! root can't lose each other's comments. Reads take a shared lock when the lock
//! file exists. Opening never creates anything.

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

    /// Adds `c` with the next id (max + 1) and the current time; returns the id.
    pub fn add(&mut self, mut c: Comment) -> io::Result<u64> {
        c.created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.mutate(move |comments| {
            c.id = comments.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            let id = c.id;
            comments.push(c);
            Some(id)
        })
        .map(|id| id.expect("add always writes"))
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

    /// Removes comment `id`; false if there is no such comment.
    pub fn remove(&mut self, id: u64) -> io::Result<bool> {
        let r = self.mutate(|comments| {
            let i = comments.iter().position(|c| c.id == id)?;
            comments.remove(i);
            Some(())
        })?;
        Ok(r.is_some())
    }

    /// Drops every comment and removes the batch file. Like every mutation it
    /// re-reads the file first, so a batch for another root is an error, not deleted.
    pub fn clear(&mut self) -> io::Result<()> {
        let _lock = self.lock_exclusive()?;
        self.read_file()?;
        match fs::remove_file(&self.file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
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
        if let Some(dir) = self.lock.parent() {
            fs::create_dir_all(dir)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
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

fn sort(comments: &mut [Comment]) {
    comments.sort_by(|a, b| (&a.path, a.lines.0, a.id).cmp(&(&b.path, b.lines.0, b.id)));
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
