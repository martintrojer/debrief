//! `~/.config/debrief/config.toml` over built-in defaults: `[send]` (how
//! `<leader>rr` hands the review back, shared in meaning with ramble's) and
//! `[[launch]]` (commands run with the TUI suspended, like ramble's).
//!
//! `[[launch]]` entries merge into the defaults by name; `disabled = true`
//! drops one.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub send: SendConfig,
    pub launch: Vec<Launcher>,
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
    /// The built-in config: no send command, the `md` and `edit` launchers.
    pub fn defaults() -> Config {
        Config {
            send: SendConfig::default(),
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

    /// Load `path` over the defaults; a missing file is the defaults.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::defaults()),
            Err(e) => return Err(anyhow!("cannot read config {}: {e}", path.display())),
        };
        Config::parse(&src).map_err(|e| anyhow!("config error in {}: {e}", path.display()))
    }

    /// Parse `src` over the defaults.
    pub fn parse(src: &str) -> Result<Config, String> {
        let raw: RawConfig = toml::from_str(src).map_err(|e| e.to_string().trim().to_string())?;
        let mut c = Config::defaults();
        if let Some(s) = raw.send {
            if let Some(cmd) = s.command {
                c.send.command = cmd;
            }
            if s.preamble.is_some() {
                c.send.preamble = s.preamble;
            }
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
                    l.command
                        .ok_or_else(|| format!("launcher `{}` is missing `command`", l.name))?,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    send: Option<RawSend>,
    launch: Option<Vec<RawLauncher>>,
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
