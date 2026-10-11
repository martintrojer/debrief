//! `~/.config/debrief/config.toml` over built-in defaults: `[send]` (how
//! `<leader>rr` hands the review back, shared in meaning with ramble's),
//! `[review]` (the comment kinds, the same shape as ramble's), `[render]`
//! (syntax highlighting), `[view]` (what shows at start: the stats pane,
//! hunks or the full file, unified or side by side), `[log]` (which
//! commits load), `[mouse]` (capture on or off) and `[[launch]]` (commands
//! run with the TUI suspended, like ramble's).
//!
//! `[[launch]]` entries merge into the defaults by name; `disabled = true`
//! drops one. `config.default.toml` documents every key with its default
//! (`debrief --init-config` writes it).

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};
use debrief_review::Kind;
use serde::{Deserialize, Serialize};

use crate::vcs::Context as DiffContext;

/// The commented default config written by `--init-config`.
pub const DEFAULT_TOML: &str = include_str!("../../config.default.toml");

/// The built-in config, [`Config::defaults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub send: SendConfig,
    pub review: ReviewConfig,
    pub render: RenderConfig,
    pub view: ViewConfig,
    pub keys: KeysConfig,
    pub mouse: MouseConfig,
    pub log: LogConfig,
    pub launch: Vec<Launcher>,
    /// Problems the file had that didn't stop it loading (dropped comment
    /// kinds); the app shows them in the status line at start.
    pub warnings: Vec<String>,
}

/// `[review]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewConfig {
    /// Kinds Tab cycles through in the comment box, in order; the
    /// export's legend defines them. Omitted: debrief-review's built-ins;
    /// a list replaces them; `[]` leaves comments untyped.
    pub kinds: Vec<Kind>,
}

/// `[render]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderConfig {
    /// Syntax-highlight diff lines (on by default).
    pub highlight: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        RenderConfig { highlight: true }
    }
}

/// `[keys]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysConfig {
    /// The `<leader>` key (Space by default).
    pub leader: char,
    /// After a pause in a key sequence, show the keys that can come next.
    pub clue: bool,
    /// `C-h/j/k/l` move between panes, and on into tmux panes at the
    /// edge; false leaves them unbound (the `C-w` keys stay).
    pub pane_nav: bool,
}

impl Default for KeysConfig {
    fn default() -> Self {
        KeysConfig {
            leader: crate::app::LEADER,
            clue: true,
            pane_nav: true,
        }
    }
}

/// `[mouse]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseConfig {
    /// Capture the mouse: clicks, drags and the wheel act in debrief (on
    /// by default); false leaves the terminal its own selection and
    /// scrolling.
    pub enabled: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        MouseConfig { enabled: true }
    }
}

/// `[log]`; `--revset` and `--limit` override it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// A jj revset or git revision range; `None` is the current commit
    /// and its ancestors (jj `::@`, git `HEAD`).
    pub revset: Option<String>,
    /// Commits loaded at first and per page (at least 1).
    pub limit: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        LogConfig {
            revset: None,
            limit: crate::app::DEFAULT_LIMIT,
        }
    }
}

/// `[view]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewConfig {
    /// The commit view's file list: always, never or auto (a peek).
    pub sidebar: SidebarShow,
    /// The screen edge the file list sits at.
    pub sidebar_side: SidebarSide,
    /// Below this many columns the file list hides (a peek draws over the
    /// diff); 0 never hides it.
    pub sidebar_auto_hide_below: u16,
    /// The file list's width, not counting its border.
    pub sidebar_width: SidebarWidth,
    /// Bounds of the auto width (the max is also capped at 35% of the
    /// terminal).
    pub sidebar_min_width: u16,
    pub sidebar_max_width: u16,
    /// The log's commits under the file list's tree (off by default;
    /// `<leader>E` toggles).
    pub sidebar_commits: bool,
    /// The log's stats pane at start (on by default; `<leader>e` toggles).
    pub stats: bool,
    /// The commit view's context at start: `"hunks"` (the default) or
    /// `"full"` (the whole file); `T` toggles.
    pub context: DiffContext,
    /// The commit view's layout at start: `"unified"` (the default) or
    /// `"split"` (side by side); `t` toggles.
    pub layout: Layout,
    /// The commit view's file list at start: `"tree"` (the default) or
    /// `"flat"` (whole paths in git's order); `` ` `` toggles.
    pub files: FilesLayout,
    /// The narrowest diff pane (not counting the file list) side by side
    /// shows in; below it the view is unified.
    pub split_min_width: u16,
}

/// The default `[view] split_min_width`.
pub const SPLIT_MIN_WIDTH: u16 = 140;

/// `[view] sidebar`: `"always"`, `"never"` or `"auto"`. Anything else,
/// `true`/`false` included, is an error naming the three values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarShow {
    /// Shown; `<leader>e` hides it.
    #[default]
    Always,
    /// Hidden; `<leader>e` shows it.
    Never,
    /// Peek: hidden while you read the diff, shown while you use it.
    Auto,
}

/// The `[view] sidebar` error, before what was found.
pub const SIDEBAR_EXPECTED: &str = "expected \"always\", \"never\" or \"auto\"";

impl SidebarShow {
    fn named(v: &str) -> Option<SidebarShow> {
        Some(match v {
            "always" => SidebarShow::Always,
            "never" => SidebarShow::Never,
            "auto" => SidebarShow::Auto,
            _ => return None,
        })
    }
}

/// [`SidebarShow`]'s values; `compat` also reads the old `true` /
/// `false` (always / never), as saved sessions hold them.
struct ShowVisitor {
    compat: bool,
}

impl serde::de::Visitor<'_> for ShowVisitor {
    type Value = SidebarShow;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(r#""always", "never" or "auto""#)
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SidebarShow, E> {
        SidebarShow::named(v).ok_or_else(|| E::custom(format!("{SIDEBAR_EXPECTED}, got \"{v}\"")))
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<SidebarShow, E> {
        match (self.compat, v) {
            (true, true) => Ok(SidebarShow::Always),
            (true, false) => Ok(SidebarShow::Never),
            _ => Err(E::custom(format!("{SIDEBAR_EXPECTED}, got {v}"))),
        }
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<SidebarShow, E> {
        Err(E::custom(format!("{SIDEBAR_EXPECTED}, got {v}")))
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<SidebarShow, E> {
        Err(E::custom(format!("{SIDEBAR_EXPECTED}, got {v}")))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<SidebarShow, E> {
        Err(E::custom(format!("{SIDEBAR_EXPECTED}, got {v}")))
    }
}

impl<'de> Deserialize<'de> for SidebarShow {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ShowVisitor { compat: false })
    }
}

/// A saved session's `view.sidebar`: [`SidebarShow`], or the `true` /
/// `false` of sessions saved before it (always / never).
pub fn sidebar_show_compat<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<SidebarShow, D::Error> {
    d.deserialize_any(ShowVisitor { compat: true })
}

/// `[view] sidebar_side`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSide {
    #[default]
    Left,
    Right,
}

/// `[view] sidebar_width`: `"auto"` fits the rows, a number is used as
/// given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarWidth {
    #[default]
    Auto,
    Fixed(u16),
}

/// The `[view] sidebar_width` error, before what was found.
const WIDTH_EXPECTED: &str = "expected \"auto\" or a number of columns";

impl<'de> Deserialize<'de> for SidebarWidth {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SidebarWidth;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(r#""auto" or a number"#)
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SidebarWidth, E> {
                match v {
                    "auto" => Ok(SidebarWidth::Auto),
                    _ => Err(E::custom(format!("{WIDTH_EXPECTED}, got \"{v}\""))),
                }
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<SidebarWidth, E> {
                u16::try_from(v)
                    .ok()
                    .filter(|&n| n > 0)
                    .map(SidebarWidth::Fixed)
                    .ok_or_else(|| E::custom(format!("{WIDTH_EXPECTED}, got {v}")))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<SidebarWidth, E> {
                self.visit_i64(i64::try_from(v).unwrap_or(-1))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<SidebarWidth, E> {
                Err(E::custom(format!("{WIDTH_EXPECTED}, got {v}")))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<SidebarWidth, E> {
                Err(E::custom(format!("{WIDTH_EXPECTED}, got {v}")))
            }
        }
        d.deserialize_any(V)
    }
}

/// `[view] layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Unified,
    Split,
}

/// `[view] files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesLayout {
    #[default]
    Tree,
    Flat,
}

impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig {
            sidebar: SidebarShow::Always,
            sidebar_side: SidebarSide::Left,
            sidebar_auto_hide_below: 80,
            sidebar_width: SidebarWidth::Auto,
            sidebar_min_width: 16,
            sidebar_max_width: 48,
            sidebar_commits: false,
            stats: true,
            context: DiffContext::Default,
            layout: Layout::Unified,
            files: FilesLayout::Tree,
            split_min_width: SPLIT_MIN_WIDTH,
        }
    }
}

/// `[send]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SendConfig {
    /// Run with the markdown on stdin; empty (or a failure) copies it to
    /// the clipboard instead.
    pub command: Vec<String>,
    /// Replaces the export's opening paragraph; `""` drops it.
    pub preamble: Option<String>,
}

/// One `[[launch]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub name: String,
    /// `<leader>` and plain keys, as in ramble; `None` means no key.
    pub key: Option<String>,
    /// Arguments, no shell. `${file}`, `${line}`, `${root}`, and `${editor}`
    /// (an argument that is exactly `${editor}` expands to its words).
    pub command: Vec<String>,
}

/// The editor launcher's name; `<leader>o` by default.
pub const EDIT: &str = "edit";
/// The markdown hand-off launcher's name; Enter on a `.md` file header.
pub const MD: &str = "md";

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

impl Config {
    /// The built-in config: no send command, the built-in comment kinds,
    /// the `md` and `edit` launchers.
    pub fn defaults() -> Config {
        Config {
            send: SendConfig::default(),
            render: RenderConfig::default(),
            view: ViewConfig::default(),
            keys: KeysConfig::default(),
            mouse: MouseConfig::default(),
            log: LogConfig::default(),
            review: ReviewConfig {
                kinds: debrief_review::builtin_kinds(),
            },
            warnings: Vec::new(),
            launch: vec![
                Launcher {
                    name: MD.into(),
                    key: None,
                    command: strings(&["ramble", "${file}"]),
                },
                Launcher {
                    name: EDIT.into(),
                    key: Some("<leader>o".into()),
                    command: strings(&["${editor}", "+${line}", "${file}"]),
                },
            ],
        }
    }

    /// `$XDG_CONFIG_HOME/debrief/config.toml`, else `~/.config/debrief/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        Some(base.join("debrief").join("config.toml"))
    }

    /// Load `path` over the defaults; a missing file is the defaults. An
    /// error names the file, the line and the key: `config error at PATH
    /// line N: view.layout: unknown variant `side`, expected `unified` or
    /// `split``.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::defaults()),
            Err(e) => return Err(anyhow!("cannot read config {}: {e}", path.display())),
        };
        Config::parse_at(&src).map_err(|(at, msg)| match at {
            Some(at) => anyhow!(
                "config error at {} line {}: {msg}",
                path.display(),
                line_of(&src, at)
            ),
            None => anyhow!("config error at {}: {msg}", path.display()),
        })
    }

    /// Parse `src` over the defaults; the error is [`Config::load`]'s
    /// message without the file and line.
    pub fn parse(src: &str) -> Result<Config, String> {
        Config::parse_at(src).map_err(|(_, msg)| msg)
    }

    /// [`Config::parse`]; the error carries its byte offset if known and
    /// is one line (values quoted in it may hold newlines).
    fn parse_at(src: &str) -> Result<Config, (Option<usize>, String)> {
        Config::parse_raw(src).map_err(|(at, msg)| (at, one_line(&msg)))
    }

    fn parse_raw(src: &str) -> Result<Config, (Option<usize>, String)> {
        let raw: RawConfig = toml::from_str(src).map_err(|e| {
            let at = e.span().map(|s| s.start);
            let msg = e.message().trim_end();
            // The key comes from the parsed document, not the text around
            // the error; a syntax error has none.
            let key = at.and_then(|at| {
                let doc = toml::de::DeTable::parse(src).ok()?;
                key_at(doc.get_ref(), at)
            });
            match key {
                Some(key) => (at, format!("{key}: {msg}")),
                None => (at, msg.to_string()),
            }
        })?;
        let mut c = Config::defaults();
        if let Some(s) = raw.send {
            if let Some(cmd) = s.command {
                c.send.command = cmd;
            }
            if s.preamble.is_some() {
                c.send.preamble = s.preamble;
            }
        }
        if let Some(h) = raw.render.and_then(|r| r.highlight) {
            c.render.highlight = h;
        }
        if let Some(k) = raw.keys {
            set(&mut c.keys.leader, k.leader);
            set(&mut c.keys.clue, k.clue);
            set(&mut c.keys.pane_nav, k.pane_nav);
        }
        if let Some(m) = raw.mouse {
            set(&mut c.mouse.enabled, m.enabled);
        }
        if let Some(l) = raw.log {
            if l.revset.is_some() {
                c.log.revset = l.revset;
            }
            set(&mut c.log.limit, l.limit.map(NonZeroUsize::get));
        }
        if let Some(v) = raw.view {
            set(&mut c.view.sidebar, v.sidebar);
            set(&mut c.view.sidebar_side, v.sidebar_side);
            set(
                &mut c.view.sidebar_auto_hide_below,
                v.sidebar_auto_hide_below,
            );
            set(&mut c.view.sidebar_width, v.sidebar_width);
            set(&mut c.view.sidebar_min_width, v.sidebar_min_width);
            set(&mut c.view.sidebar_max_width, v.sidebar_max_width);
            set(&mut c.view.sidebar_commits, v.sidebar_commits);
            set(&mut c.view.stats, v.stats);
            match v.context {
                Some(RawContext::Hunks) => c.view.context = DiffContext::Default,
                Some(RawContext::Full) => c.view.context = DiffContext::Full,
                None => {}
            }
            set(&mut c.view.layout, v.layout);
            set(&mut c.view.files, v.files);
            set(&mut c.view.split_min_width, v.split_min_width);
        }
        if let Some(kinds) = raw.review.and_then(|r| r.kinds) {
            c.review.kinds = clean_kinds(kinds, &mut c.warnings);
        }
        for l in raw.launch.unwrap_or_default() {
            let pos = c.launch.iter().position(|d| d.name == l.name);
            if l.disabled {
                if let Some(i) = pos {
                    c.launch.remove(i);
                }
                continue;
            }
            let (key, command) = match pos {
                // Overriding a default keeps whatever the entry leaves out.
                Some(i) => (
                    l.key.or_else(|| c.launch[i].key.clone()),
                    l.command.unwrap_or_else(|| c.launch[i].command.clone()),
                ),
                None => (
                    l.key,
                    l.command.ok_or_else(|| {
                        (None, format!("launcher `{}` is missing `command`", l.name))
                    })?,
                ),
            };
            let new = Launcher {
                name: l.name,
                key: key.filter(|k| !k.is_empty()),
                command,
            };
            match pos {
                Some(i) => c.launch[i] = new,
                None => c.launch.push(new),
            }
        }
        Ok(c)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::defaults()
    }
}

/// `to = v` when the file sets `v`.
fn set<T>(to: &mut T, v: Option<T>) {
    if let Some(v) = v {
        *to = v;
    }
}

/// Write the commented default config to `path`, creating parent
/// directories. Refuses to overwrite an existing file.
pub fn write_default(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow!("cannot create {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| anyhow!("cannot write {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, DEFAULT_TOML.as_bytes())
        .map_err(|e| anyhow!("cannot write {}: {e}", path.display()))?;
    Ok(())
}

/// `s` with control characters and the Unicode line and paragraph
/// separators (newlines from a quoted value) escaped.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// 1-based line of byte offset `at` in `src`.
fn line_of(src: &str, at: usize) -> usize {
    src.as_bytes()[..at.min(src.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// The dotted path (`view.layout`, `launch.command`) of the deepest key
/// in the parsed `doc` whose key or value span holds byte offset `at`;
/// array items add no segment.
fn key_at(doc: &toml::de::DeTable, at: usize) -> Option<String> {
    fn walk(t: &toml::de::DeTable, at: usize, path: &mut Vec<String>, best: &mut Vec<String>) {
        for (k, v) in t {
            path.push(k.get_ref().to_string());
            if (k.span().contains(&at) || v.span().contains(&at)) && path.len() > best.len() {
                *best = path.clone();
            }
            value(v, at, path, best);
            path.pop();
        }
    }
    fn value(
        v: &toml::Spanned<toml::de::DeValue>,
        at: usize,
        path: &mut Vec<String>,
        best: &mut Vec<String>,
    ) {
        match v.get_ref() {
            toml::de::DeValue::Table(t) => walk(t, at, path, best),
            toml::de::DeValue::Array(a) => a.iter().for_each(|i| {
                // An item of `[[launch]]` holding `at` names its array,
                // even when the array's own span covers only the first.
                if i.span().contains(&at) && path.len() > best.len() {
                    *best = path.clone();
                }
                value(i, at, path, best)
            }),
            _ => {}
        }
    }
    let mut best = Vec::new();
    walk(doc, at, &mut Vec::new(), &mut best);
    (!best.is_empty()).then(|| best.join("."))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    send: Option<RawSend>,
    review: Option<RawReview>,
    render: Option<RawRender>,
    view: Option<RawView>,
    keys: Option<RawKeys>,
    mouse: Option<RawMouse>,
    log: Option<RawLog>,
    launch: Option<Vec<RawLauncher>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLog {
    #[serde(default, deserialize_with = "non_empty")]
    revset: Option<String>,
    limit: Option<NonZeroUsize>,
}

/// A string with something in it besides whitespace.
fn non_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let s = String::deserialize(d)?;
    if s.trim().is_empty() {
        return Err(serde::de::Error::custom("expected a non-empty string"));
    }
    Ok(Some(s))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMouse {
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKeys {
    leader: Option<char>,
    clue: Option<bool>,
    pane_nav: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReview {
    kinds: Option<Vec<RawKind>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRender {
    highlight: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawView {
    sidebar: Option<SidebarShow>,
    sidebar_side: Option<SidebarSide>,
    sidebar_auto_hide_below: Option<u16>,
    sidebar_width: Option<SidebarWidth>,
    sidebar_min_width: Option<u16>,
    sidebar_max_width: Option<u16>,
    sidebar_commits: Option<bool>,
    stats: Option<bool>,
    context: Option<RawContext>,
    layout: Option<Layout>,
    files: Option<FilesLayout>,
    split_min_width: Option<u16>,
}

/// `[view] context`.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawContext {
    Hunks,
    Full,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKind {
    id: String,
    definition: Option<String>,
}

/// `[review] kinds` with ids trimmed and lowercased; an empty or repeated
/// id is dropped with a warning (tuicr's rule).
fn clean_kinds(kinds: Vec<RawKind>, warnings: &mut Vec<String>) -> Vec<Kind> {
    let mut out: Vec<Kind> = Vec::new();
    for k in kinds {
        let id = k.id.trim().to_lowercase();
        if id.is_empty() {
            warnings.push("[review] kinds: dropped a kind with an empty id".into());
        } else if out.iter().any(|o| o.id == id) {
            warnings.push(format!("[review] kinds: dropped duplicate kind `{id}`"));
        } else {
            out.push(Kind {
                id,
                definition: k.definition,
            });
        }
    }
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSend {
    command: Option<Vec<String>>,
    preamble: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLauncher {
    name: String,
    key: Option<String>,
    command: Option<Vec<String>>,
    #[serde(default)]
    disabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_md_and_edit() {
        let c = Config::defaults();
        assert!(c.send.command.is_empty());
        assert_eq!(c.launch[0].name, MD);
        assert_eq!(c.launch[0].key, None);
        assert_eq!(c.launch[1].key.as_deref(), Some("<leader>o"));
        assert_eq!(Config::parse("").unwrap(), c);
    }

    #[test]
    fn user_file_overrides_and_merges_by_name() {
        let c = Config::parse(
            r#"
[send]
command = ["debrief-send"]
preamble = ""

[[launch]]
name = "md"
command = ["glow", "${file}"]

[[launch]]
name = "edit"
disabled = true

[[launch]]
name = "lg"
key = "<leader>g"
command = ["lazygit"]
"#,
        )
        .unwrap();
        assert_eq!(c.send.command, ["debrief-send"]);
        assert_eq!(c.send.preamble.as_deref(), Some(""));
        let names: Vec<_> = c.launch.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["md", "lg"]);
        assert_eq!(c.launch[0].command, ["glow", "${file}"]);
    }

    #[test]
    fn review_kinds_default_replace_or_empty() {
        let ids =
            |c: &Config| -> Vec<String> { c.review.kinds.iter().map(|k| k.id.clone()).collect() };
        assert_eq!(
            ids(&Config::parse("").unwrap()),
            ["issue", "suggestion", "question", "nit"]
        );
        let c = Config::parse(
            r#"
[review]
kinds = [
  { id = " Praise ", definition = "Keep doing this." },
  { id = "issue" },
  { id = "praise" },
  { id = "" },
]
"#,
        )
        .unwrap();
        assert_eq!(ids(&c), ["praise", "issue"]);
        assert_eq!(
            c.review.kinds[0].definition.as_deref(),
            Some("Keep doing this.")
        );
        assert_eq!(c.review.kinds[1].definition, None);
        assert_eq!(
            c.warnings,
            [
                "[review] kinds: dropped duplicate kind `praise`",
                "[review] kinds: dropped a kind with an empty id"
            ]
        );
        let c = Config::parse("[review]\nkinds = []\n").unwrap();
        assert!(c.review.kinds.is_empty());
        assert!(c.warnings.is_empty());
        // `[review]` without `kinds` keeps the built-ins.
        assert_eq!(ids(&Config::parse("[review]\n").unwrap()).len(), 4);
        assert!(Config::parse("[review]\nkinds = [{ id = \"x\", color = \"red\" }]\n").is_err());
        assert!(Config::parse("[review]\nenabled = true\n").is_err());
    }

    #[test]
    fn render_highlight_defaults_on() {
        assert!(Config::defaults().render.highlight);
        assert!(Config::parse("[render]\n").unwrap().render.highlight);
        assert!(
            !Config::parse("[render]\nhighlight = false\n")
                .unwrap()
                .render
                .highlight
        );
        assert!(Config::parse("[render]\nhighlight = \"no\"\n").is_err());
        assert!(Config::parse("[render]\nwrap = true\n").is_err());
    }

    #[test]
    fn view_sidebar_is_always_never_or_auto() {
        let c = |src: &str| Config::parse(src).map(|c| c.view.sidebar);
        assert_eq!(c("[view]\n"), Ok(SidebarShow::Always));
        for (v, want) in [
            ("always", SidebarShow::Always),
            ("never", SidebarShow::Never),
            ("auto", SidebarShow::Auto),
        ] {
            assert_eq!(c(&format!("[view]\nsidebar = \"{v}\"\n")), Ok(want));
        }
        let want = r#"view.sidebar: expected "always", "never" or "auto", got"#;
        for (v, got) in [("true", "true"), ("false", "false"), ("\"on\"", "\"on\"")] {
            let e = c(&format!("[view]\nsidebar = {v}\n")).unwrap_err();
            assert_eq!(e, format!("{want} {got}"));
        }
    }

    #[test]
    fn view_sidebar_geometry() {
        let d = Config::defaults().view;
        assert_eq!(
            (
                d.sidebar_side,
                d.sidebar_auto_hide_below,
                d.sidebar_width,
                d.sidebar_min_width,
                d.sidebar_max_width
            ),
            (SidebarSide::Left, 80, SidebarWidth::Auto, 16, 48)
        );
        let v = Config::parse(
            "[view]\nsidebar_side = \"right\"\nsidebar_auto_hide_below = 0\nsidebar_width = 30\nsidebar_min_width = 10\nsidebar_max_width = 60\n",
        )
        .unwrap()
        .view;
        assert_eq!(
            (
                v.sidebar_side,
                v.sidebar_auto_hide_below,
                v.sidebar_width,
                v.sidebar_min_width,
                v.sidebar_max_width
            ),
            (SidebarSide::Right, 0, SidebarWidth::Fixed(30), 10, 60)
        );
        let w = |s: &str| {
            Config::parse(&format!("[view]\nsidebar_width = {s}\n")).map(|c| c.view.sidebar_width)
        };
        assert_eq!(w("\"auto\""), Ok(SidebarWidth::Auto));
        for bad in ["\"wide\"", "0", "-1", "true", "70000"] {
            let e = w(bad).unwrap_err();
            assert!(
                e.starts_with("view.sidebar_width: expected \"auto\" or a number"),
                "{bad}: {e}"
            );
        }
        assert!(Config::parse("[view]\nsidebar_side = \"top\"\n").is_err());
    }

    #[test]
    fn view_sidebar_commits_defaults_off() {
        assert!(!Config::defaults().view.sidebar_commits);
        let c = |src: &str| Config::parse(src).map(|c| c.view.sidebar_commits);
        assert_eq!(c("[view]\nsidebar_commits = true\n"), Ok(true));
        assert_eq!(c("[view]\nsidebar_commits = false\n"), Ok(false));
        assert!(c("[view]\nsidebar_commits = \"on\"\n").is_err());
    }

    #[test]
    fn mouse_enabled_defaults_on() {
        assert!(Config::defaults().mouse.enabled);
        let c = |src: &str| Config::parse(src).map(|c| c.mouse.enabled);
        assert_eq!(c("[mouse]\n"), Ok(true));
        assert_eq!(c("[mouse]\nenabled = false\n"), Ok(false));
        assert!(c("[mouse]\nenabled = \"no\"\n").is_err());
        assert!(c("[mouse]\nwheel = 3\n").is_err());
    }

    #[test]
    fn log_defaults_to_no_revset_and_200() {
        assert_eq!(
            Config::defaults().log,
            LogConfig {
                revset: None,
                limit: 200
            }
        );
        let c = Config::parse("[log]\nrevset = \"main..\"\nlimit = 50\n").unwrap();
        assert_eq!(c.log.revset.as_deref(), Some("main.."));
        assert_eq!(c.log.limit, 50);
        for (src, want) in [
            ("[log]\nlimit = 0\n", "log.limit: "),
            ("[log]\nlimit = -1\n", "log.limit: "),
            ("[log]\nlimit = \"all\"\n", "log.limit: "),
            ("[log]\nrevset = \"\"\n", "log.revset: "),
            ("[log]\nrevset = 3\n", "log.revset: "),
            ("[log]\nrevsets = \"x\"\n", "log.revsets: unknown field"),
        ] {
            let e = Config::parse(src).unwrap_err();
            assert!(e.starts_with(want), "{src:?}: {e}");
        }
    }

    #[test]
    fn view_stats_defaults_on() {
        assert!(Config::defaults().view.stats);
        assert!(!Config::parse("[view]\nstats = false\n").unwrap().view.stats);
        assert!(Config::parse("[view]\nnope = 1\n").is_err());
    }

    #[test]
    fn view_sidebar_and_keys_default_on() {
        let d = Config::defaults();
        assert!(d.view.sidebar == SidebarShow::Always && d.keys.clue && d.keys.pane_nav);
        assert_eq!(d.keys.leader, ' ');
        let c = Config::parse(
            "[view]\nsidebar = \"never\"\n[keys]\nleader = \",\"\nclue = false\npane_nav = false\n",
        )
        .unwrap();
        assert!(c.view.sidebar == SidebarShow::Never && !c.keys.clue && !c.keys.pane_nav);
        assert_eq!(c.keys.leader, ',');
        assert!(Config::parse("[keys]\nleader = \"\"\n").is_err());
    }

    #[test]
    fn errors_name_the_table_and_key() {
        let e = Config::parse("[view]\nsidebar = 1\n").unwrap_err();
        assert!(e.starts_with("view.sidebar: expected"), "{e}");
        let e = Config::parse("[[launch]]\nname = \"x\"\n\"kye\" = 1\n").unwrap_err();
        assert!(e.starts_with("launch.kye: unknown field"), "{e}");
        let key = |src: &str, at| key_at(toml::de::DeTable::parse(src).unwrap().get_ref(), at);
        assert_eq!(key("# x = 1\n", 0), None);
        assert_eq!(key("a = 1\n", 4).as_deref(), Some("a"));
        // The path comes from the parser, whatever the text looks like.
        for (src, want) in [
            (
                "[view] # preferences\nlayout = \"side\"\n",
                "view.layout: unknown variant",
            ),
            (
                "view = { layout = \"side\" }\n",
                "view.layout: unknown variant",
            ),
            (
                "[[launch]]\nname = \"edit\"\ncommand = [\n  1\n]\n",
                "launch.command: invalid type",
            ),
            ("[ view ]\nstats = 1\n", "view.stats: invalid type"),
            (
                "[[launch]]\nname = \"x\"\nk = { a = 1 }\n",
                "launch.k: unknown field",
            ),
            (
                "[review]\nkinds = [{ id = \"x\", color = 1 }]\n",
                "review.kinds.color: unknown field",
            ),
            // A later [[launch]] entry names its array too.
            (
                "[[launch]]\nname = \"a\"\ncommand = [\"x\"]\n[[launch]]\ncommand = [\"y\"]\n",
                "launch: missing field `name`",
            ),
        ] {
            let e = Config::parse(src).unwrap_err();
            assert!(e.starts_with(want), "{src:?}: {e}");
        }
    }

    #[test]
    fn one_line_escapes_unicode_line_separators() {
        assert_eq!(
            one_line("a\u{2028}b\u{2029}c\nd"),
            "a\\u{2028}b\\u{2029}c\\nd"
        );
    }

    #[test]
    fn write_default_creates_dirs_and_refuses_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a/b/config.toml");
        write_default(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), DEFAULT_TOML);
        std::fs::write(&p, "keep").unwrap();
        let e = format!("{:#}", write_default(&p).unwrap_err());
        assert_eq!(e, format!("{} already exists", p.display()));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep");
    }

    #[test]
    fn view_context_defaults_to_hunks() {
        assert_eq!(Config::defaults().view.context, DiffContext::Default);
        let c = |src: &str| Config::parse(src).map(|c| c.view.context);
        assert_eq!(c("[view]\n"), Ok(DiffContext::Default));
        assert_eq!(c("[view]\ncontext = \"hunks\"\n"), Ok(DiffContext::Default));
        assert_eq!(c("[view]\ncontext = \"full\"\n"), Ok(DiffContext::Full));
        assert!(c("[view]\ncontext = \"all\"\n").is_err());
        assert!(c("[view]\ncontext = 3\n").is_err());
    }

    #[test]
    fn view_layout_defaults_to_unified_at_140() {
        let d = Config::defaults();
        assert_eq!(
            (d.view.layout, d.view.split_min_width),
            (Layout::Unified, 140)
        );
        let c = Config::parse("[view]\nlayout = \"split\"\nsplit_min_width = 100\n").unwrap();
        assert_eq!(
            (c.view.layout, c.view.split_min_width),
            (Layout::Split, 100)
        );
        assert_eq!(
            Config::parse("[view]\nlayout = \"unified\"\n")
                .unwrap()
                .view
                .layout,
            Layout::Unified
        );
        assert!(Config::parse("[view]\nlayout = \"side\"\n").is_err());
        assert!(Config::parse("[view]\nsplit_min_width = -1\n").is_err());
    }

    #[test]
    fn errors() {
        assert!(Config::parse("[nope]\n").is_err());
        let e = Config::parse("[[launch]]\nname = \"x\"\n").unwrap_err();
        assert!(e.contains("missing `command`"), "{e}");
    }

    #[test]
    fn missing_file_is_the_defaults() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&d.path().join("none.toml")).unwrap(),
            Config::defaults()
        );
    }
}
