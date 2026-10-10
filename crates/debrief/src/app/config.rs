//! `~/.config/debrief/config.toml` over built-in defaults: `[send]` (how
//! `<leader>rr` hands the review back, shared in meaning with ramble's),
//! `[review]` (the comment kinds, the same shape as ramble's), `[render]`
//! (syntax highlighting), `[view]` (what shows at start: the stats pane,
//! hunks or the full file, unified or side by side) and `[[launch]]` (commands run with the TUI suspended, like ramble's).
//!
//! `[[launch]]` entries merge into the defaults by name; `disabled = true`
//! drops one.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use debrief_review::Kind;
use serde::Deserialize;

use crate::vcs::Context as DiffContext;

/// The built-in config, [`Config::defaults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub send: SendConfig,
    pub review: ReviewConfig,
    pub render: RenderConfig,
    pub view: ViewConfig,
    pub keys: KeysConfig,
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
}

impl Default for KeysConfig {
    fn default() -> Self {
        KeysConfig {
            leader: crate::app::LEADER,
            clue: true,
        }
    }
}

/// `[view]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewConfig {
    /// The commit view's file list at start (on by default; `<leader>e`
    /// toggles).
    pub sidebar: bool,
    /// The log's stats pane at start (on by default; `<leader>e` toggles).
    pub stats: bool,
    /// The commit view's context at start: `"hunks"` (the default) or
    /// `"full"` (the whole file); `T` toggles.
    pub context: DiffContext,
    /// The commit view's layout at start: `"unified"` (the default) or
    /// `"split"` (side by side); `t` toggles.
    pub layout: Layout,
    /// The narrowest diff pane (not counting the file list) side by side
    /// shows in; below it the view is unified.
    pub split_min_width: u16,
}

/// The default `[view] split_min_width`.
pub const SPLIT_MIN_WIDTH: u16 = 140;

/// `[view] layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Unified,
    Split,
}

impl Default for ViewConfig {
    fn default() -> Self {
        ViewConfig {
            sidebar: true,
            stats: true,
            context: DiffContext::Default,
            layout: Layout::Unified,
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

    /// [`Config::parse`]; the error carries its byte offset if known.
    fn parse_at(src: &str) -> Result<Config, (Option<usize>, String)> {
        let raw: RawConfig = toml::from_str(src).map_err(|e| {
            let at = e.span().map(|s| s.start);
            let msg = e.message().trim_end();
            match at.and_then(|at| key_at(src, at)) {
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
            if let Some(leader) = k.leader {
                c.keys.leader = leader;
            }
            if let Some(clue) = k.clue {
                c.keys.clue = clue;
            }
        }
        if let Some(v) = raw.view {
            if let Some(sidebar) = v.sidebar {
                c.view.sidebar = sidebar;
            }
            if let Some(stats) = v.stats {
                c.view.stats = stats;
            }
            match v.context {
                Some(RawContext::Hunks) => c.view.context = DiffContext::Default,
                Some(RawContext::Full) => c.view.context = DiffContext::Full,
                None => {}
            }
            if let Some(l) = v.layout {
                c.view.layout = l;
            }
            if let Some(w) = v.split_min_width {
                c.view.split_min_width = w;
            }
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

/// 1-based line of byte offset `at` in `src`.
fn line_of(src: &str, at: usize) -> usize {
    src.as_bytes()[..at.min(src.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// The `table.key` whose `key = value` line holds byte offset `at`
/// (the table from the last `[table]` / `[[table]]` header above it).
fn key_at(src: &str, at: usize) -> Option<String> {
    let at = at.min(src.len());
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = src[start..].lines().next().unwrap_or("");
    let (key, _) = line.split_once('=')?;
    let key = key.trim().trim_matches('"');
    if key.is_empty() || key.starts_with('#') || key.starts_with('[') {
        return None;
    }
    let table = src[..start].lines().rev().find_map(|l| {
        let l = l.trim();
        l.starts_with('[')
            .then(|| l.trim_matches(|c| c == '[' || c == ']').trim().to_string())
    });
    Some(match table {
        Some(t) => format!("{t}.{key}"),
        None => key.to_string(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    send: Option<RawSend>,
    review: Option<RawReview>,
    render: Option<RawRender>,
    view: Option<RawView>,
    keys: Option<RawKeys>,
    launch: Option<Vec<RawLauncher>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKeys {
    leader: Option<char>,
    clue: Option<bool>,
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
    sidebar: Option<bool>,
    stats: Option<bool>,
    context: Option<RawContext>,
    layout: Option<Layout>,
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
    fn view_stats_defaults_on() {
        assert!(Config::defaults().view.stats);
        assert!(!Config::parse("[view]\nstats = false\n").unwrap().view.stats);
        assert!(Config::parse("[view]\nnope = 1\n").is_err());
    }

    #[test]
    fn view_sidebar_and_keys_default_on() {
        let d = Config::defaults();
        assert!(d.view.sidebar && d.keys.clue);
        assert_eq!(d.keys.leader, ' ');
        let c = Config::parse("[view]\nsidebar = false\n[keys]\nleader = \",\"\nclue = false\n")
            .unwrap();
        assert!(!c.view.sidebar && !c.keys.clue);
        assert_eq!(c.keys.leader, ',');
        assert!(Config::parse("[keys]\nleader = \"\"\n").is_err());
    }

    #[test]
    fn errors_name_the_table_and_key() {
        let e = Config::parse("[view]\nsidebar = 1\n").unwrap_err();
        assert!(e.starts_with("view.sidebar: invalid type"), "{e}");
        let e = Config::parse("[[launch]]\nname = \"x\"\n\"kye\" = 1\n").unwrap_err();
        assert!(e.starts_with("launch.kye: unknown field"), "{e}");
        assert_eq!(key_at("# x = 1\n", 0), None);
        assert_eq!(key_at("a = 1\n", 4).as_deref(), Some("a"));
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
