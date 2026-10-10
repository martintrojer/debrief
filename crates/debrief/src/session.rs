//! Saved review sessions: one JSON file
//! per (repo root, commit) under
//! `<state>/sessions/<16 hex of blake3(root)>/<commit id>.json`; the
//! working copy's is `working-copy.json`. Written by tmp + rename; nothing
//! is written until the first save. Unknown fields are ignored on read; a
//! file of another version, for another root, or not JSON is an error for
//! that file only.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use debrief_review::{Comment, Side};
use serde::{Deserialize, Serialize};

use crate::app::config::SidebarShow;

/// The only session format.
pub const VERSION: u32 = 1;
/// The session key (and file stem) of the working copy.
pub const WORKING: &str = "working-copy";

/// Where the cursor was, in terms that survive a change of context
/// (`T`): message rows by index, everything else by file and line (hunk
/// indices change under `T`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    /// A message row (the message is the same in both modes).
    Row(usize),
    /// A file's header, placeholder or hunk header: the file's header.
    File(String),
    /// A diff line: the side's line number (side by side's left prefers
    /// old numbers; else the new side when the line has one).
    Line { path: String, side: Side, line: u32 },
}

/// `[view] context` as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewContext {
    #[default]
    Hunks,
    Full,
}

/// `[view] layout` as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewLayout {
    #[default]
    Unified,
    Split,
}

/// `[view] files` as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewFiles {
    #[default]
    Tree,
    Flat,
}

/// The pane with the keys, as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewFocus {
    #[default]
    Diff,
    Files,
}

/// The commit view as saved.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub context: ViewContext,
    pub layout: ViewLayout,
    /// The file list: always / never / auto, as `[view] sidebar` (and
    /// `<leader>e`) left it. Sessions saved before held `true` / `false`,
    /// read as always / never.
    #[serde(deserialize_with = "crate::app::config::sidebar_show_compat")]
    pub sidebar: SidebarShow,
    pub focus: ViewFocus,
    /// The file list: tree or flat (`` ` ``); `None` in sessions saved
    /// before it, which open as `[view] files` says.
    pub files: Option<ViewFiles>,
    /// The file list's collapsed directories (paths from the root);
    /// sessions saved before it open with every directory expanded.
    pub collapsed: Vec<String>,
    /// The cursor row; `None` when the diff hadn't loaded.
    pub anchor: Option<Anchor>,
    /// Fallbacks: the cursor's row index and display column.
    pub row: usize,
    pub col: usize,
}

/// A comment of the session; `sent` (unix secs) once debrief sent or
/// copied it. A sent comment is never restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedComment {
    #[serde(flatten)]
    pub comment: Comment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<u64>,
}

/// One saved review of one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub root: PathBuf,
    /// The commit ID, or [`WORKING`].
    pub commit: String,
    #[serde(default)]
    pub change: Option<String>,
    #[serde(default)]
    pub summary: String,
    /// Unix seconds.
    pub saved: u64,
    #[serde(default)]
    pub comments: Vec<SavedComment>,
    #[serde(default)]
    pub view: View,
}

impl Session {
    /// The comments not sent yet.
    pub fn unsent(&self) -> impl Iterator<Item = &Comment> {
        self.comments
            .iter()
            .filter(|c| c.sent.is_none())
            .map(|c| &c.comment)
    }

    pub fn unsent_count(&self) -> usize {
        self.unsent().count()
    }

    pub fn sent_count(&self) -> usize {
        self.comments.len() - self.unsent_count()
    }
}

/// `$DEBRIEF_STATE_DIR` as is if set, else the user state dir (Linux:
/// `$XDG_STATE_HOME` or `~/.local/state`; elsewhere `~/.local/state`)
/// plus `debrief`.
pub fn state_dir() -> Option<PathBuf> {
    resolve_state_dir(
        std::env::var_os("DEBRIEF_STATE_DIR"),
        dirs::state_dir(),
        dirs::home_dir(),
    )
}

/// [`state_dir`] from `$DEBRIEF_STATE_DIR` (`env`), the platform's
/// state dir (`state`) and the home dir (`home`).
fn resolve_state_dir(
    env: Option<std::ffi::OsString>,
    state: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    match env {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => state
            .or_else(|| home.map(|h| h.join(".local").join("state")))
            .map(|d| d.join("debrief")),
    }
}

/// The sessions of one repo root.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    dir: PathBuf,
}

/// Unix seconds now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl Store {
    /// The store for `root` (canonicalised when possible, as the review
    /// batch does) under `state_dir`. Creates nothing.
    pub fn new(state_dir: &Path, root: &Path) -> Store {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let hash = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
        Store {
            dir: state_dir.join("sessions").join(&hash.as_str()[..16]),
            root,
        }
    }

    /// The directory holding this root's sessions.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The canonical root the sessions belong to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file of commit `key` ([`WORKING`] for the working copy).
    pub fn path(&self, key: &str) -> io::Result<PathBuf> {
        let ok = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(invalid(format!("not a commit id: {key:?}")));
        }
        Ok(self.dir.join(format!("{key}.json")))
    }

    /// The session of `key`; `None` when there is none.
    pub fn load(&self, key: &str) -> io::Result<Option<Session>> {
        let path = self.path(key)?;
        match fs::read(&path) {
            Ok(bytes) => self.parse(&path, &bytes).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn parse(&self, path: &Path, bytes: &[u8]) -> io::Result<Session> {
        let s: Session = serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        if s.version != VERSION {
            return Err(invalid(format!(
                "{}: unsupported version {}",
                path.display(),
                s.version
            )));
        }
        if s.root != self.root {
            return Err(invalid(format!(
                "{} belongs to {}, not {}",
                path.display(),
                s.root.display(),
                self.root.display()
            )));
        }
        Ok(s)
    }

    /// Write `s` as is, replacing the commit's session.
    pub fn save(&self, s: &Session) -> io::Result<()> {
        let path = self.path(&s.commit)?;
        fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(s).map_err(|e| invalid(e.to_string()))?;
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)
    }

    /// Every readable session, newest save first, and an error per file
    /// that can't be read.
    pub fn list(&self) -> (Vec<Session>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return (out, errors),
            Err(e) => return (out, vec![format!("{}: {e}", self.dir.display())]),
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            match fs::read(&path).and_then(|b| self.parse(&path, &b)) {
                Ok(s) => out.push(s),
                Err(e) => errors.push(e.to_string()),
            }
        }
        out.sort_by(|a, b| b.saved.cmp(&a.saved).then_with(|| a.commit.cmp(&b.commit)));
        errors.sort();
        (out, errors)
    }

    /// Delete the session of `key` (none is fine).
    pub fn delete(&self, key: &str) -> io::Result<()> {
        match fs::remove_file(self.path(key)?) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(st: &Store, commit: &str, saved: u64) -> Session {
        Session {
            version: VERSION,
            root: st.root().to_path_buf(),
            commit: commit.into(),
            change: None,
            summary: "s".into(),
            saved,
            comments: vec![SavedComment {
                comment: Comment::on_file("a.txt", 1..=2, "x", "body"),
                sent: None,
            }],
            view: View {
                anchor: Some(Anchor::Line {
                    path: "a.txt".into(),
                    side: Side::Old,
                    line: 3,
                }),
                ..View::default()
            },
        }
    }

    #[test]
    fn round_trip_one_file_per_commit_under_the_state_dir() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(state.path(), root.path());
        assert!(st.dir().starts_with(state.path().join("sessions")));
        assert_eq!(st.load("abc1").unwrap(), None);
        assert!(!state.path().join("sessions").exists(), "nothing written");
        let mut s = session(&st, "abc1", 5);
        st.save(&s).unwrap();
        assert_eq!(st.load("abc1").unwrap(), Some(s.clone()));
        assert_eq!(st.path("abc1").unwrap(), st.dir().join("abc1.json"));
        s.summary = "again".into();
        st.save(&s).unwrap();
        let files: Vec<_> = fs::read_dir(st.dir()).unwrap().collect();
        assert_eq!(files.len(), 1, "saving twice overwrites");
        assert_eq!(st.load("abc1").unwrap().unwrap().summary, "again");
        st.save(&session(&st, WORKING, 9)).unwrap();
        assert!(st.dir().join("working-copy.json").is_file());
        let (list, errors) = st.list();
        assert!(errors.is_empty());
        let keys: Vec<_> = list.iter().map(|s| s.commit.as_str()).collect();
        assert_eq!(keys, [WORKING, "abc1"], "newest first");
        st.delete("abc1").unwrap();
        st.delete("abc1").unwrap();
        assert_eq!(st.load("abc1").unwrap(), None);
    }

    #[test]
    fn corrupt_and_foreign_files_are_errors_and_skipped() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(state.path(), root.path());
        st.save(&session(&st, "good", 1)).unwrap();
        fs::write(st.dir().join("bad.json"), "{not json").unwrap();
        let mut v2 = serde_json::to_value(st.load("good").unwrap().unwrap()).unwrap();
        v2["version"] = 2.into();
        v2["commit"] = "v2".into();
        fs::write(st.dir().join("v2.json"), v2.to_string()).unwrap();
        assert!(st.load("bad").is_err());
        let e = st.load("v2").unwrap_err().to_string();
        assert!(e.contains("unsupported version 2"), "{e}");
        let (list, errors) = st.list();
        assert_eq!(list.len(), 1);
        assert_eq!(errors.len(), 2, "{errors:?}");
    }

    #[test]
    fn unknown_fields_are_ignored_and_keys_are_checked() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let st = Store::new(state.path(), root.path());
        st.save(&session(&st, "c", 1)).unwrap();
        let mut v = serde_json::to_value(st.load("c").unwrap().unwrap()).unwrap();
        v["future"] = 1.into();
        v["view"]["later"] = true.into();
        fs::write(st.path("c").unwrap(), v.to_string()).unwrap();
        assert!(st.load("c").unwrap().is_some());
        assert!(st.path("../x").is_err());
        assert!(st.path("").is_err());
    }

    #[test]
    fn the_state_dir_is_the_env_var_else_the_state_dir_else_home() {
        let state = Some(PathBuf::from("/state"));
        let home = Some(PathBuf::from("/home/me"));
        let got = resolve_state_dir(Some("/env".into()), state.clone(), home.clone());
        assert_eq!(got, Some(PathBuf::from("/env")), "set: as is");
        let got = resolve_state_dir(Some("".into()), state.clone(), home.clone());
        assert_eq!(got, Some(PathBuf::from("/state/debrief")), "empty: unset");
        let got = resolve_state_dir(None, state, home.clone());
        assert_eq!(got, Some(PathBuf::from("/state/debrief")));
        let got = resolve_state_dir(None, None, home);
        assert_eq!(got, Some(PathBuf::from("/home/me/.local/state/debrief")));
        assert_eq!(resolve_state_dir(None, None, None), None);
    }
}
