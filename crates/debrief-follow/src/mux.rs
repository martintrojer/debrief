//! The terminal multiplexer debrief or ramble runs in: tmux or rex.
//!
//! Both offer the same three things here: move focus to the pane beside
//! ours (`C-h/j/k/l` past the edge), split ours to run a command (a
//! follow launcher), and zoom ours (debrief's `[view] tmux_zoom`). Their
//! windows also scope the follow sockets.

use std::path::Path;

use crate::ANY;

/// A pane direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Down,
    Up,
    Right,
}

/// The multiplexer of this process and its pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mux {
    /// `$TMUX` and `$TMUX_PANE`.
    Tmux { pane: String },
    /// `$REX_SESSION` and `$REX_BLOCK`; `program` is `$REX_BIN_DIR/rex`
    /// when set, else `rex` from `$PATH`.
    Rex {
        program: String,
        session: String,
        block: String,
    },
}

impl Mux {
    /// The multiplexer `env` says we run in. With both tmux's and rex's
    /// variables (one runs inside the other, or a server inherited the
    /// other's environment), `$TERM_PROGRAM`, which the innermost sets,
    /// picks; otherwise tmux.
    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Option<Mux> {
        let set = |k| env(k).filter(|v: &String| !v.is_empty());
        let tmux = set("TMUX")
            .and(set("TMUX_PANE"))
            .map(|pane| Mux::Tmux { pane });
        let rex = set("REX_SESSION")
            .zip(set("REX_BLOCK"))
            .map(|(session, block)| {
                let program = set("REX_BIN_DIR").map_or_else(
                    || "rex".to_string(),
                    |d| Path::new(&d).join("rex").display().to_string(),
                );
                Mux::Rex {
                    program,
                    session,
                    block,
                }
            });
        match (tmux, rex) {
            (Some(t), Some(r)) => Some(if set("TERM_PROGRAM").as_deref() == Some("rex") {
                r
            } else {
                t
            }),
            (t, r) => t.or(r),
        }
    }

    /// [`Mux::detect`] on this process's environment.
    pub fn current() -> Option<Mux> {
        Mux::detect(|k| std::env::var(k).ok())
    }

    /// `tmux` or `rex`, for messages.
    pub fn name(&self) -> &'static str {
        match self {
            Mux::Tmux { .. } => "tmux",
            Mux::Rex { .. } => "rex",
        }
    }

    /// `rex --autostart=false` (never start a server that has gone away)
    /// and `args`.
    fn rex(program: &str, args: &[&str]) -> Vec<String> {
        [program, "--autostart=false"]
            .iter()
            .chain(args)
            .map(|s| s.to_string())
            .collect()
    }

    /// Move focus to the pane toward `dir`. Neither fails or wraps when
    /// there is none.
    pub fn focus_argv(&self, dir: Dir) -> Vec<String> {
        match self {
            Mux::Tmux { .. } => {
                let flag = match dir {
                    Dir::Left => "-L",
                    Dir::Down => "-D",
                    Dir::Up => "-U",
                    Dir::Right => "-R",
                };
                ["tmux", "select-pane", flag].map(String::from).into()
            }
            Mux::Rex {
                program, session, ..
            } => {
                let dir = match dir {
                    Dir::Left => "left",
                    Dir::Down => "down",
                    Dir::Up => "up",
                    Dir::Right => "right",
                };
                Mux::rex(program, &["focus", "-s", session, dir])
            }
        }
    }

    /// Run `argv` in a new pane right of ours, starting in `cwd`, never
    /// through a shell. tmux hands a lone word to `sh -c` and execs two
    /// or more, so a one-word `argv` runs through `env`; and the cwd's `#`
    /// is doubled, as tmux expands formats (`#{…}`) in `-c`. rex passes
    /// both as given.
    pub fn split_argv(&self, cwd: &Path, argv: &[String]) -> Vec<String> {
        let mut out: Vec<String> = match self {
            Mux::Tmux { pane } => {
                let mut out: Vec<String> = ["tmux", "split-window", "-h", "-t", pane, "-c"]
                    .map(String::from)
                    .into();
                out.push(cwd.display().to_string().replace('#', "##"));
                out.push("--".into());
                if argv.len() == 1 {
                    out.push("env".into());
                }
                out
            }
            Mux::Rex {
                program,
                session,
                block,
            } => {
                let cwd = cwd.display().to_string();
                let args = [
                    "split",
                    "-s",
                    session,
                    "-b",
                    block,
                    "--split=right",
                    "-c",
                    &cwd,
                    "--",
                ];
                Mux::rex(program, &args)
            }
        };
        out.extend(argv.iter().cloned());
        out
    }

    /// Print whether our pane's window is zoomed; [`Mux::parse_zoomed`]
    /// reads the output.
    pub fn zoomed_argv(&self) -> Vec<String> {
        match self {
            Mux::Tmux { pane } => ["tmux", "display-message", "-p", "-t", pane]
                .map(String::from)
                .into_iter()
                .chain(["#{window_zoomed_flag}".to_string()])
                .collect(),
            Mux::Rex {
                program, session, ..
            } => Mux::rex(program, &["session", "inspect", session, "--json"]),
        }
    }

    /// Whether [`Mux::zoomed_argv`]'s `out` says our pane is zoomed.
    /// `None` when `out` makes no sense.
    pub fn parse_zoomed(&self, out: &str) -> Option<bool> {
        match self {
            Mux::Tmux { .. } => match out.trim() {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            },
            Mux::Rex { block, .. } => {
                let v: serde_json::Value = serde_json::from_str(out).ok()?;
                let windows = v.get("windows")?.as_array()?;
                Some(windows.iter().any(|w| {
                    w.get("zoomed_block_id").and_then(|z| z.as_str()) == Some(block.as_str())
                }))
            }
        }
    }

    /// Zoom our pane (`on`) or undo the zoom. tmux only toggles: check
    /// [`Mux::zoomed_argv`] first.
    pub fn zoom_argv(&self, on: bool) -> Vec<String> {
        match self {
            Mux::Tmux { pane } => ["tmux", "resize-pane", "-Z", "-t", pane]
                .map(String::from)
                .into(),
            Mux::Rex {
                program,
                session,
                block,
            } => {
                let state = if on { "--on" } else { "--off" };
                Mux::rex(program, &["zoom", "-s", session, block, state])
            }
        }
    }

    /// Print our pane's window ([`Mux::scope`] reads the output).
    pub fn window_argv(&self) -> Vec<String> {
        match self {
            Mux::Tmux { pane } => ["tmux", "display-message", "-p", "-t", pane, "#{window_id}"]
                .map(String::from)
                .into(),
            Mux::Rex {
                program,
                session,
                block,
            } => Mux::rex(
                program,
                &["block", "inspect", "-s", session, block, "--json"],
            ),
        }
    }

    /// The follow scope for [`Mux::window_argv`]'s output `out`, given
    /// `$TMUX` for tmux; [`ANY`] when it makes no sense.
    pub fn scope(&self, tmux: Option<&str>, out: Option<&str>) -> String {
        match self {
            Mux::Tmux { .. } => crate::scope_from(tmux, out),
            Mux::Rex { .. } => {
                let win = out
                    .and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok())
                    .and_then(|v| v.pointer("/block/window_id")?.as_str().map(String::from));
                rex_scope(win.as_deref())
            }
        }
    }
}

/// The scope for a rex window id: `r` and 12 hex digits of its FNV-1a
/// hash. The id is a UUID, too long for a socket path (macOS allows 104
/// bytes, and `$TMPDIR` takes half); 48 bits keep the windows apart.
pub fn rex_scope(window_id: Option<&str>) -> String {
    let Some(id) = window_id.map(str::trim).filter(|w| !w.is_empty()) else {
        return ANY.into();
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("r{:012x}", h & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    const TMUX: &[(&str, &str)] = &[("TMUX", "/s,1,0"), ("TMUX_PANE", "%3")];
    const REX: &[(&str, &str)] = &[("REX_SESSION", "session:s"), ("REX_BLOCK", "block:b")];

    fn rex() -> Mux {
        Mux::detect(env(REX)).unwrap()
    }

    fn tmux() -> Mux {
        Mux::detect(env(TMUX)).unwrap()
    }

    #[test]
    fn detect_needs_both_variables_of_one() {
        assert_eq!(tmux(), Mux::Tmux { pane: "%3".into() });
        assert_eq!(
            rex(),
            Mux::Rex {
                program: "rex".into(),
                session: "session:s".into(),
                block: "block:b".into(),
            }
        );
        assert_eq!(Mux::detect(env(&[("TMUX_PANE", "%3")])), None);
        assert_eq!(
            Mux::detect(env(&[("TMUX", "/s,1,0"), ("TMUX_PANE", "")])),
            None
        );
        assert_eq!(Mux::detect(env(&[("REX_SESSION", "session:s")])), None);
        assert_eq!(Mux::detect(env(&[])), None);
    }

    #[test]
    fn rex_runs_from_its_bin_dir_when_set() {
        let m = Mux::detect(env(&[
            ("REX_SESSION", "s"),
            ("REX_BLOCK", "b"),
            ("REX_BIN_DIR", "/A B/MacOS"),
        ]));
        assert!(matches!(m, Some(Mux::Rex { program, .. }) if program == "/A B/MacOS/rex"));
    }

    #[test]
    fn with_both_term_program_picks_else_tmux() {
        let both = |tp: &'static str| {
            move |k: &str| match k {
                "TERM_PROGRAM" => Some(tp.to_string()),
                _ => env(TMUX)(k).or_else(|| env(REX)(k)),
            }
        };
        assert_eq!(Mux::detect(both("rex")).unwrap().name(), "rex");
        assert_eq!(Mux::detect(both("tmux")).unwrap().name(), "tmux");
        assert_eq!(Mux::detect(both("")).unwrap().name(), "tmux");
    }

    #[test]
    fn focus_moves_by_direction() {
        let all = [Dir::Left, Dir::Down, Dir::Up, Dir::Right];
        let t: Vec<_> = all.map(|d| tmux().focus_argv(d).join(" ")).into();
        assert_eq!(
            t,
            [
                "tmux select-pane -L",
                "tmux select-pane -D",
                "tmux select-pane -U",
                "tmux select-pane -R"
            ]
        );
        assert_eq!(
            rex().focus_argv(Dir::Up),
            ["rex", "--autostart=false", "focus", "-s", "session:s", "up"]
        );
    }

    #[test]
    fn tmux_split_runs_a_lone_word_through_env_and_escapes_hashes() {
        let one = ["/r/a;touch x.md".to_string()];
        assert_eq!(
            tmux().split_argv(Path::new("/r/#{x}"), &one)[6..],
            ["/r/##{x}", "--", "env", "/r/a;touch x.md"]
        );
        let argv = ["nvim", "+3", "a b.rs"].map(String::from);
        assert_eq!(
            tmux().split_argv(Path::new("/r"), &argv),
            [
                "tmux",
                "split-window",
                "-h",
                "-t",
                "%3",
                "-c",
                "/r",
                "--",
                "nvim",
                "+3",
                "a b.rs"
            ]
        );
    }

    #[test]
    fn rex_split_passes_cwd_and_argv_as_given() {
        let one = ["/r/a;touch x.md".to_string()];
        assert_eq!(
            rex().split_argv(Path::new("/r/#{x}"), &one),
            [
                "rex",
                "--autostart=false",
                "split",
                "-s",
                "session:s",
                "-b",
                "block:b",
                "--split=right",
                "-c",
                "/r/#{x}",
                "--",
                "/r/a;touch x.md"
            ]
        );
    }

    #[test]
    fn zoom_commands_and_state() {
        assert_eq!(tmux().zoom_argv(true), tmux().zoom_argv(false), "a toggle");
        assert_eq!(
            tmux().zoom_argv(true).join(" "),
            "tmux resize-pane -Z -t %3"
        );
        assert_eq!(
            tmux().zoomed_argv().join(" "),
            "tmux display-message -p -t %3 #{window_zoomed_flag}"
        );
        assert_eq!(tmux().parse_zoomed("1\n"), Some(true));
        assert_eq!(tmux().parse_zoomed("0\n"), Some(false));
        assert_eq!(tmux().parse_zoomed(""), None);

        assert_eq!(
            rex().zoom_argv(false).join(" "),
            "rex --autostart=false zoom -s session:s block:b --off"
        );
        assert_eq!(
            rex().zoomed_argv().join(" "),
            "rex --autostart=false session inspect session:s --json"
        );
        let view = |z: &str| {
            format!(r#"{{"windows":[{{"zoomed_block_id":null}},{{"zoomed_block_id":{z}}}]}}"#)
        };
        assert_eq!(rex().parse_zoomed(&view(r#""block:b""#)), Some(true));
        assert_eq!(rex().parse_zoomed(&view(r#""block:c""#)), Some(false));
        assert_eq!(rex().parse_zoomed(&view("null")), Some(false));
        assert_eq!(rex().parse_zoomed("Error: no server"), None);
    }

    #[test]
    fn scopes_by_window() {
        assert_eq!(
            tmux().window_argv().join(" "),
            "tmux display-message -p -t %3 #{window_id}"
        );
        assert_eq!(
            tmux().scope(Some("/tmp/tmux-1/default,4242,0"), Some("@12\n")),
            "t4242w12"
        );
        assert_eq!(
            rex().window_argv().join(" "),
            "rex --autostart=false block inspect -s session:s block:b --json"
        );
        let out = |w: &str| format!(r#"{{"block":{{"block_id":"block:b","window_id":"{w}"}}}}"#);
        let a = rex().scope(None, Some(&out("window:01a1-aa")));
        assert_eq!(a.len(), 13);
        assert!(a.starts_with('r') && a[1..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(a, rex().scope(None, Some(&out("window:01a1-aa"))));
        assert_ne!(a, rex().scope(None, Some(&out("window:01a1-ab"))));
        assert_eq!(rex().scope(None, Some("Error")), ANY);
        assert_eq!(rex().scope(None, None), ANY);
        assert_eq!(rex_scope(Some("  ")), ANY);
    }
}
