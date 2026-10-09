//! Helpers shared by the VCS integration tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Run a command in `dir` and return stdout; panic with stderr on failure.
pub fn sh(dir: &Path, program: &str, args: &[&str]) -> String {
    sh_env(dir, program, args, &[])
}

pub fn sh_env(dir: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args).env("LC_ALL", "C");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Every file under `dir`: relative path -> (content hash, size, mtime).
pub type Snapshot = BTreeMap<PathBuf, (u64, u64, SystemTime)>;

pub fn snapshot(dir: &Path) -> Snapshot {
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn walk(base: &Path, dir: &Path, out: &mut Snapshot) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let meta = std::fs::symlink_metadata(&p).unwrap();
        if meta.is_dir() {
            walk(base, &p, out);
            out.insert(
                p.strip_prefix(base).unwrap().to_path_buf(),
                (0, 0, meta.modified().unwrap()),
            );
        } else {
            let mut h = DefaultHasher::new();
            h.write(&std::fs::read(&p).unwrap_or_default());
            out.insert(
                p.strip_prefix(base).unwrap().to_path_buf(),
                (h.finish(), meta.len(), meta.modified().unwrap()),
            );
        }
    }
}

pub fn assert_same(before: &Snapshot, after: &Snapshot) {
    if before != after {
        let changed: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|k| before.get(*k) != after.get(*k))
            .collect();
        panic!("repo metadata changed: {changed:?}");
    }
}
