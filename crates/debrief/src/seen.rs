//! Seen state: which commits, and for jj which
//! changes, the user has looked at. One JSON file per repo root under
//! `<cache>/seen/`, written by rename. A single writer is assumed.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::vcs::CommitInfo;

const VERSION: u32 = 1;

/// How a commit shows in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Neither the commit nor its change was seen.
    New,
    /// jj only: the change was seen, this commit wasn't (it was rewritten).
    Updated,
    Seen,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    version: u32,
    root: String,
    commits: BTreeSet<String>,
    changes: BTreeSet<String>,
}

#[derive(Debug)]
pub struct Seen {
    path: PathBuf,
    data: File,
    fresh: bool,
}

/// `<cache>/seen/<16 hex of blake3(root)>.json`.
pub fn seen_path(cache_dir: &Path, root: &Path) -> PathBuf {
    let hash = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    cache_dir
        .join("seen")
        .join(format!("{}.json", &hash.as_str()[..16]))
}

impl Seen {
    /// Load the seen state of `root` (already canonical, as `vcs::open`
    /// returns it). No file yet gives an empty, fresh state; nothing is
    /// written until the first mark.
    pub fn open(cache_dir: &Path, root: &Path) -> io::Result<Seen> {
        let path = seen_path(cache_dir, root);
        let (data, fresh) = match fs::read(&path) {
            Ok(bytes) => {
                let data: File = serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: {e}", path.display()),
                    )
                })?;
                if data.version != VERSION {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: unknown version {}", path.display(), data.version),
                    ));
                }
                (data, false)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (
                File {
                    version: VERSION,
                    root: root.to_string_lossy().into_owned(),
                    ..File::default()
                },
                true,
            ),
            Err(e) => return Err(e),
        };
        Ok(Seen { path, data, fresh })
    }

    /// No seen file existed when this state was opened (the repo was
    /// never opened before) and nothing was marked since.
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn status(&self, c: &CommitInfo) -> Status {
        if self.data.commits.contains(&c.commit) {
            Status::Seen
        } else if c
            .change
            .as_ref()
            .is_some_and(|ch| self.data.changes.contains(ch))
        {
            Status::Updated
        } else {
            Status::New
        }
    }

    pub fn mark(&mut self, c: &CommitInfo) -> io::Result<()> {
        self.mark_all(std::slice::from_ref(c))
    }

    pub fn mark_all(&mut self, commits: &[CommitInfo]) -> io::Result<()> {
        for c in commits {
            self.data.commits.insert(c.commit.clone());
            if let Some(ch) = &c.change {
                self.data.changes.insert(ch.clone());
            }
        }
        self.write()?;
        self.fresh = false;
        Ok(())
    }

    fn write(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?;
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)
    }
}
