//! Launchers: `[[launch]]` commands
//! run with the TUI suspended, ramble's grammar and variables. `edit`
//! (`<leader>o`) opens `$EDITOR` at the working-copy file and line under
//! the cursor; `md` (Enter on a `.md` file header) hands the file to ramble.
//!
//! Actions only queue an [`Effect`]; `run` suspends the TUI and calls
//! [`App::run_pending_effect`], so tests run effects without a terminal.

use std::path::PathBuf;

use anyhow::Context;

use super::commit::{CommitView, RowKind};
use super::config::{Launcher, MD};
use super::{App, Focus, Screen};
use crate::vcs::{CommitDiff, LineKind};

/// A fully expanded launcher command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    /// The launcher's name (for status messages).
    pub name: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

/// How a launched command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

/// Runs a launcher command in the terminal and waits for it.
pub type Runner = dyn FnMut(&LaunchCommand) -> anyhow::Result<Exit>;

/// Reads an environment variable (injectable so tests never touch the
/// process environment).
pub type EnvLookup = dyn Fn(&str) -> Option<String>;

/// A side effect to run with the TUI suspended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Run a launcher.
    Launch(LaunchCommand),
    /// `C-e` in the comment prompt: edit the draft in the editor, then save.
    EditComment(super::comment::Draft),
    /// `C-e` in the review picker: edit comment `id`'s body.
    EditReviewComment { id: u64, initial: String },
    /// `<leader>rr`: export the batch and hand it back.
    SendReview,
}

/// Values for the `${...}` variables of a launcher command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchVars {
    /// The working-copy file, or the status saying why there is none.
    pub file: Result<PathBuf, String>,
    pub line: u32,
    pub root: PathBuf,
    /// `$VISUAL`, else `$EDITOR`, else `vi`, split on whitespace.
    pub editor: Vec<String>,
}

/// Substitute variables in each argument (no shell). An argument that is
/// exactly `${editor}` expands to the editor's words. `Err` is the status:
/// a missing file's own message, else why the command is malformed.
pub fn expand(command: &[String], vars: &LaunchVars) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for arg in command {
        if arg == "${editor}" {
            out.extend(vars.editor.iter().cloned());
            continue;
        }
        let mut s = String::new();
        let mut rest = arg.as_str();
        while let Some(i) = rest.find("${") {
            s.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            let Some(j) = after.find('}') else {
                return Err(format!("unterminated variable in {arg:?}"));
            };
            let value = match &after[..j] {
                "file" => vars.file.clone()?.display().to_string(),
                "line" => vars.line.to_string(),
                "root" => vars.root.display().to_string(),
                "editor" => vars.editor.join(" "),
                other => return Err(format!("unknown variable ${{{other}}}")),
            };
            s.push_str(&value);
            rest = &after[j + 1..];
        }
        s.push_str(rest);
        out.push(s);
    }
    Ok(out)
}

/// Parse a launcher key like `<leader>rr` into the chars after the leader.
pub fn parse_key(key: &str) -> Result<Vec<char>, String> {
    let rest = key
        .strip_prefix("<leader>")
        .ok_or_else(|| format!("key {key:?} must start with <leader>"))?;
    if let Some(i) = rest.find('<') {
        let tok = rest[i..].split_inclusive('>').next().unwrap_or(&rest[i..]);
        return Err(format!("unknown key {tok}"));
    }
    if rest.is_empty() {
        return Err(format!("key {key:?} has nothing after <leader>"));
    }
    Ok(rest.chars().collect())
}

/// Launcher key bindings: the chars after the leader and the index into
/// `Config.launch`. Bad keys are skipped with a message.
pub(super) fn bindings(launchers: &[Launcher]) -> (Vec<(Vec<char>, usize)>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (i, l) in launchers.iter().enumerate() {
        let Some(key) = &l.key else { continue };
        match parse_key(key) {
            Ok(seq) => out.push((seq, i)),
            Err(e) => errors.push(format!("launcher {}: {e}", l.name)),
        }
    }
    (out, errors)
}

/// What the keys typed after the leader mean for the launchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LeaderMatch {
    Launch(usize),
    Pending,
    NoMapping,
}

pub(super) fn match_leader(bindings: &[(Vec<char>, usize)], typed: &[char]) -> LeaderMatch {
    let longer = bindings
        .iter()
        .any(|(seq, _)| seq.len() > typed.len() && seq.starts_with(typed));
    if longer {
        return LeaderMatch::Pending;
    }
    match bindings.iter().find(|(seq, _)| seq == typed) {
        Some(&(_, i)) => LeaderMatch::Launch(i),
        None => LeaderMatch::NoMapping,
    }
}

/// Editor words: `$VISUAL`, else `$EDITOR`, else `vi` (ramble's rule).
pub(super) fn editor_words(env: &EnvLookup) -> Vec<String> {
    let cmd = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| env(v))
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    cmd.split_whitespace().map(str::to_string).collect()
}

/// The status for a file the working copy doesn't have.
pub fn not_in_working_copy(path: &str) -> String {
    format!("{path} isn't in the working copy")
}

/// Status for `<leader>o` on a row with no file (the message).
pub const NO_FILE_HERE: &str = "No file here";
/// Status for `<leader>o` in the log on a commit with no files.
pub const NO_FILES: &str = "No files in this commit";

/// The new-side line a diff row stands for in the working copy:
/// its own new line; a removed line the next new line in the
/// hunk, else the previous one's + 1, else the hunk's start; a hunk
/// header its start; anything else in a file line 1. `None`: no file.
fn row_line(diff: &CommitDiff, v: &CommitView, row: usize) -> Option<(usize, u32)> {
    let r = v.rows.get(row)?;
    let file = r.file?;
    let hunk = r.hunk.and_then(|h| diff.files.get(file)?.hunks.get(h));
    let line = match (r.kind, hunk, r.line) {
        // A side by side filler row stands for its paired line's place.
        (RowKind::Line(_) | RowKind::NoNewline | RowKind::Filler, Some(h), Some(li)) => {
            match h.lines.get(li).and_then(|l| l.new) {
                Some(n) => n,
                None => {
                    let next = h.lines[li..].iter().find_map(|l| l.new);
                    let prev = h.lines[..li].iter().rev().find_map(|l| l.new);
                    next.or(prev.map(|p| p + 1)).unwrap_or(h.new_start)
                }
            }
        }
        (RowKind::HunkHeader, Some(h), _) => h.new_start,
        _ => 1,
    };
    Some((file, line.max(1)))
}

/// Line of the first hunk's first new-side line (the log's `<leader>o`).
fn first_hunk_line(diff: &CommitDiff, file: usize) -> u32 {
    let f = &diff.files[file];
    f.hunks
        .first()
        .map(|h| {
            h.lines
                .iter()
                .find(|l| l.kind != LineKind::Removed)
                .and_then(|l| l.new)
                .unwrap_or(h.new_start)
        })
        .unwrap_or(1)
        .max(1)
}

impl App {
    /// Injects the launcher runner (tests record argv instead of running).
    pub fn with_runner(
        mut self,
        runner: impl FnMut(&LaunchCommand) -> anyhow::Result<Exit> + 'static,
    ) -> Self {
        self.runner = Box::new(runner);
        self
    }

    /// Injects the environment lookup (`$VISUAL`, `$EDITOR`).
    pub fn with_env(mut self, env: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.env = Box::new(env);
        self
    }

    /// The queued side effect, if any.
    pub fn pending_effect(&self) -> Option<&Effect> {
        self.effect.as_ref()
    }

    /// Run the queued side effect (if any). The caller has suspended the
    /// TUI; `run` does this. Afterwards the batch is re-read; the view
    /// stays where it was.
    pub fn run_pending_effect(&mut self) {
        match self.effect.take() {
            Some(Effect::Launch(cmd)) => {
                self.run_launch(&cmd);
                // The command may have changed files.
                self.after_launch();
            }
            Some(Effect::EditComment(d)) => self.run_comment_editor(d),
            Some(Effect::EditReviewComment { id, initial }) => {
                self.run_review_comment_editor(id, &initial);
            }
            // Its status says how it went; reloading would only repeat it.
            Some(Effect::SendReview) => return self.run_review_send(),
            None => return,
        }
        self.review_reload();
        self.picker_refresh();
    }

    /// After a launcher: reload as `r` does, keeping its status message.
    fn after_launch(&mut self) {
        let status = std::mem::take(&mut self.status);
        match self.screen {
            Screen::Commit(_) => self.reload_view(),
            Screen::Log => self.reload(true),
        }
        if !status.is_empty() {
            self.status = if self.status.is_empty() {
                status
            } else {
                format!("{status}; {}", self.status)
            };
        }
    }

    /// The working-copy file and line for `<leader>o` and `${file}`:
    /// the diff row under the cursor (the selected file in the file list),
    /// or on the log the commit's first changed file at its first hunk.
    pub fn edit_target(&self) -> Result<(PathBuf, u32), String> {
        let (path, line) = match self.screen {
            Screen::Log => self.log_target()?,
            Screen::Commit(_) => {
                let v = self.commit.as_ref().ok_or(NO_FILE_HERE)?;
                let diff = v.diff.as_ref().ok_or(NO_FILE_HERE)?;
                let (file, line) = if self.focus == Focus::Files {
                    self.selected_file().map(|f| (f, 1)).ok_or(NO_FILE_HERE)?
                } else {
                    row_line(diff, v, v.cur.row).ok_or(NO_FILE_HERE)?
                };
                let f = &diff.files[file];
                match &f.new_path {
                    Some(p) => (p.clone(), line),
                    None => return Err(not_in_working_copy(super::commit::file_path(f))),
                }
            }
        };
        self.inside_root(&path)?;
        let abs = self.vcs.root().join(&path);
        if !abs.exists() {
            return Err(not_in_working_copy(&path));
        }
        Ok((abs, line))
    }

    /// The log's target: the commit's first file still in the working copy.
    fn log_target(&self) -> Result<(String, u32), String> {
        let info = self.rows.get(self.cursor).ok_or(NO_FILES)?;
        // Hunks whatever `T` says: the line is the first change's. A
        // one-shot key press, so read here (git's working copy too).
        let diff = if info.info.commit == super::WORKING_ID {
            self.vcs
                .working_copy(crate::vcs::Context::Default)
                .map_err(|e| format!("cannot read the working copy: {e:#}"))?
                .map(|w| w.diff)
                .unwrap_or_else(|| crate::vcs::CommitDiff {
                    message: String::new(),
                    files: Vec::new(),
                })
        } else {
            self.vcs
                .show(&info.info.commit, crate::vcs::Context::Default)
                .map_err(|e| format!("cannot show the commit: {e:#}"))?
        };
        let Some(first) = diff.files.first() else {
            return Err(NO_FILES.into());
        };
        let root = self.vcs.root();
        let present =
            |f: &crate::vcs::FileDiff| f.new_path.as_ref().is_some_and(|p| root.join(p).exists());
        match diff.files.iter().position(present) {
            Some(i) => Ok((
                diff.files[i].new_path.clone().unwrap_or_default(),
                first_hunk_line(&diff, i),
            )),
            None => Err(not_in_working_copy(super::commit::file_path(first))),
        }
    }

    fn launch_vars(&self, target: Result<(PathBuf, u32), String>) -> LaunchVars {
        let (file, line) = match target {
            Ok((f, l)) => (Ok(f), l),
            Err(e) => (Err(e), 1),
        };
        LaunchVars {
            file,
            line,
            root: self.vcs.root().to_path_buf(),
            editor: editor_words(self.env.as_ref()),
        }
    }

    fn build_launch(
        &self,
        l: &Launcher,
        target: Result<(PathBuf, u32), String>,
    ) -> Result<LaunchCommand, String> {
        let argv = expand(&l.command, &self.launch_vars(target))?;
        if argv.is_empty() {
            return Err(format!("{}: empty command", l.name));
        }
        Ok(LaunchCommand {
            name: l.name.clone(),
            argv,
            cwd: self.vcs.root().to_path_buf(),
        })
    }

    /// Queue launcher `i` on the cursor's target (status if unavailable).
    pub(super) fn launch_index(&mut self, i: usize) {
        let Some(l) = self.config.launch.get(i).cloned() else {
            return;
        };
        // The log's target runs `show()`; only work it out when used.
        let uses_file = l
            .command
            .iter()
            .any(|a| a.contains("${file}") || a.contains("${line}"));
        let target = if uses_file {
            self.edit_target()
        } else {
            Err(NO_FILE_HERE.into())
        };
        self.queue_launch(&l, target);
    }

    fn queue_launch(&mut self, l: &Launcher, target: Result<(PathBuf, u32), String>) {
        match self.build_launch(l, target) {
            Ok(cmd) => self.effect = Some(Effect::Launch(cmd)),
            Err(msg) => self.set_status(msg),
        }
    }

    /// Hand the working-copy markdown file `path` (repo-relative) to the
    /// `md` launcher, if it exists and belongs to this review root.
    pub(super) fn launch_md(&mut self, path: &str) {
        let abs = self.vcs.root().join(path);
        if !abs.is_file() {
            return self.set_status("Not in the working copy");
        }
        let root = self.vcs.root().canonicalize().ok();
        if Some(debrief_review::repo_root(&abs)) != root {
            return self.set_status("File belongs to a different review root");
        }
        let Some(l) = self.config.launch.iter().find(|l| l.name == MD).cloned() else {
            return self.set_status(format!("No launcher named {MD}"));
        };
        self.queue_launch(&l, Ok((abs, 1)));
    }

    /// Enter in the diff: a `.md` file header goes to ramble.
    pub(super) fn diff_enter(&mut self) {
        let Some(v) = &self.commit else { return };
        let (Some(r), Some(diff)) = (v.rows.get(v.cur.row), v.diff.as_ref()) else {
            return;
        };
        if r.kind != RowKind::FileHeader {
            return;
        }
        let Some(f) = r.file.and_then(|i| diff.files.get(i)) else {
            return;
        };
        let path = super::commit::file_path(f).to_string();
        if !path.ends_with(".md") {
            return self.set_status("Enter opens .md files in ramble");
        }
        if f.new_path.is_none() {
            return self.set_status("Not in the working copy");
        }
        self.launch_md(&path);
    }

    /// Run a launcher command (TUI already suspended).
    pub(super) fn run_launch(&mut self, cmd: &LaunchCommand) {
        let result = (self.runner)(cmd);
        match result {
            Ok(Exit::Code(0)) => {}
            Ok(Exit::Code(n)) => self.set_status(format!("{}: exited with status {n}", cmd.name)),
            Ok(Exit::Signal(n)) => self.set_status(format!("{}: killed by signal {n}", cmd.name)),
            Err(e) => self.set_status(format!("{}: {e:#}", cmd.name)),
        }
    }

    /// Write `initial` to a temp file, run the editor on it through the
    /// runner, and return what it leaves. `Err` says why nothing came back.
    pub(super) fn edit_in_editor(&mut self, initial: &str) -> Result<String, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let file =
            std::env::temp_dir().join(format!("debrief-comment-{}-{nanos}.md", std::process::id()));
        std::fs::write(&file, initial).map_err(|e| e.to_string())?;
        let mut argv = editor_words(self.env.as_ref());
        argv.push(file.display().to_string());
        let cmd = LaunchCommand {
            name: "editor".into(),
            argv,
            cwd: self.vcs.root().to_path_buf(),
        };
        let result = (self.runner)(&cmd);
        let text = std::fs::read_to_string(&file);
        let _ = std::fs::remove_file(&file);
        match result {
            Ok(Exit::Code(0)) => text.map_err(|e| e.to_string()),
            Ok(Exit::Code(n)) => Err(format!("exited with status {n}")),
            Ok(Exit::Signal(n)) => Err(format!("killed by signal {n}")),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

/// Default runner: the command in the terminal, in its cwd.
pub fn system_run(cmd: &LaunchCommand) -> anyhow::Result<Exit> {
    let (prog, args) = cmd
        .argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let status = std::process::Command::new(prog)
        .args(args)
        .current_dir(&cmd.cwd)
        .status()
        .with_context(|| format!("running {prog}"))?;
    if let Some(code) = status.code() {
        return Ok(Exit::Code(code));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Ok(Exit::Signal(sig));
        }
    }
    Ok(Exit::Code(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> LaunchVars {
        LaunchVars {
            file: Ok("/n/a.md".into()),
            line: 7,
            root: "/n".into(),
            editor: vec!["code".into(), "-w".into()],
        }
    }

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn expands_per_argument() {
        let got = expand(
            &s(&["${editor}", "+${line}", "${file}", "${root}/x"]),
            &vars(),
        );
        assert_eq!(got.unwrap(), s(&["code", "-w", "+7", "/n/a.md", "/n/x"]));
    }

    #[test]
    fn a_missing_file_is_its_own_message() {
        let mut v = vars();
        v.file = Err("x isn't in the working copy".into());
        assert_eq!(
            expand(&s(&["${file}"]), &v).unwrap_err(),
            "x isn't in the working copy"
        );
        assert!(expand(&s(&["${nope}"]), &vars()).is_err());
        assert!(expand(&s(&["${line"]), &vars()).is_err());
    }

    #[test]
    fn key_grammar() {
        assert_eq!(parse_key("<leader>rr").unwrap(), vec!['r', 'r']);
        assert!(parse_key("<leader><C-x>").is_err());
        assert!(parse_key("x").is_err());
        assert!(parse_key("<leader>").is_err());
    }

    #[test]
    fn leader_waits_on_strict_prefix() {
        let b = vec![(vec!['o'], 0), (vec!['g', 'g'], 1)];
        assert_eq!(match_leader(&b, &['g']), LeaderMatch::Pending);
        assert_eq!(match_leader(&b, &['o']), LeaderMatch::Launch(0));
        assert_eq!(match_leader(&b, &['x']), LeaderMatch::NoMapping);
    }

    #[test]
    fn editor_fallbacks() {
        let none = |_: &str| None;
        assert_eq!(editor_words(&none), vec!["vi"]);
        let blank_visual = |k: &str| match k {
            "VISUAL" => Some("  ".into()),
            "EDITOR" => Some("nano -w".into()),
            _ => None,
        };
        assert_eq!(editor_words(&blank_visual), vec!["nano", "-w"]);
    }
}
