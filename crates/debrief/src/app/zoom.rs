//! `[view] tmux_zoom`: inside tmux or rex, zoom debrief's pane (tmux's
//! `PREFIX z`) at start and unzoom it on exit, for the room full diffs
//! need.
//!
//! Only a zoom debrief made is undone: a window already zoomed at start
//! is left alone, and so is one the user (or a `C-h/j/k/l` move to
//! another pane, which both multiplexers unzoom on) has unzoomed
//! meanwhile.

use debrief_follow::Mux;

/// Runs a multiplexer command and returns its stdout, `None` on any
/// failure.
pub type Run<'a> = dyn FnMut(&[String]) -> Option<String> + 'a;

/// A zoom debrief made, to undo on exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zoom {
    mux: Mux,
}

/// Our pane is zoomed (tmux: its window is).
fn zoomed(run: &mut Run, mux: &Mux) -> Option<bool> {
    mux.parse_zoomed(&run(&mux.zoomed_argv())?)
}

/// Zoom our pane unless it is already zoomed. `Some` only when this call
/// zoomed it; a tmux window of one pane can't zoom, so it gives `None`
/// too.
pub fn zoom_in(run: &mut Run, mux: &Mux) -> Option<Zoom> {
    if zoomed(run, mux)? {
        return None;
    }
    run(&mux.zoom_argv(true))?;
    zoomed(run, mux)?.then(|| Zoom { mux: mux.clone() })
}

impl Zoom {
    /// Unzoom, if still zoomed.
    pub fn zoom_out(&self, run: &mut Run) {
        if zoomed(run, &self.mux) == Some(true) {
            let _ = run(&self.mux.zoom_argv(false));
        }
    }
}

/// The zoom to make at start: the setting is on and debrief runs inside
/// tmux or rex.
pub fn wanted(on: bool, env: impl Fn(&str) -> Option<String>) -> Option<Mux> {
    on.then(|| Mux::detect(env))?
}

/// Run the real multiplexer, no terminal I/O.
pub fn run(argv: &[String]) -> Option<String> {
    let (prog, args) = argv.split_first()?;
    let out = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: &str = "%3";
    const BLOCK: &str = "block:b";

    fn tmux() -> Mux {
        Mux::Tmux { pane: PANE.into() }
    }

    fn rex() -> Mux {
        Mux::Rex {
            program: "rex".into(),
            session: "session:s".into(),
            block: BLOCK.into(),
        }
    }

    /// A fake multiplexer: one window of `panes`, zoomed or not; records
    /// calls. Answers as tmux does, or as rex does (`--on` / `--off`, and
    /// a lone block zooms too).
    struct Fake {
        zoomed: bool,
        panes: usize,
        calls: Vec<String>,
    }

    impl Fake {
        fn new(zoomed: bool, panes: usize) -> Fake {
            Fake {
                zoomed,
                panes,
                calls: Vec::new(),
            }
        }

        fn run(&mut self, argv: &[String]) -> Option<String> {
            let a: Vec<&str> = argv.iter().map(String::as_str).collect();
            self.calls.push(a.join(" "));
            match a.as_slice() {
                ["tmux", "display-message", ..] => Some(format!("{}\n", u8::from(self.zoomed))),
                ["tmux", "resize-pane", ..] => {
                    if self.panes > 1 {
                        self.zoomed = !self.zoomed;
                    }
                    Some(String::new())
                }
                ["rex", _, "session", "inspect", ..] => {
                    let z = if self.zoomed {
                        format!("\"{BLOCK}\"")
                    } else {
                        "null".into()
                    };
                    Some(format!(r#"{{"windows":[{{"zoomed_block_id":{z}}}]}}"#))
                }
                ["rex", _, "zoom", .., state] => {
                    self.zoomed = *state == "--on";
                    Some(String::new())
                }
                _ => None,
            }
        }

        fn zooms(&self) -> usize {
            self.calls
                .iter()
                .filter(|c| c.contains("resize-pane") || c.contains(" zoom "))
                .count()
        }
    }

    #[test]
    fn zooms_then_unzooms_its_own_zoom() {
        for mux in [tmux(), rex()] {
            let mut f = Fake::new(false, 2);
            let z = zoom_in(&mut |a: &[String]| f.run(a), &mux).unwrap();
            assert!(f.zoomed, "{mux:?}");
            z.zoom_out(&mut |a: &[String]| f.run(a));
            assert!(!f.zoomed, "{mux:?}");
            assert_eq!(f.zooms(), 2, "{mux:?}");
        }
    }

    #[test]
    fn leaves_a_window_already_zoomed_alone() {
        for mux in [tmux(), rex()] {
            let mut f = Fake::new(true, 2);
            assert_eq!(zoom_in(&mut |a: &[String]| f.run(a), &mux), None);
            assert!(f.zoomed);
            assert_eq!(f.zooms(), 0, "{mux:?}");
        }
    }

    #[test]
    fn a_lone_tmux_pane_makes_no_zoom_to_undo() {
        let mut f = Fake::new(false, 1);
        assert_eq!(zoom_in(&mut |a: &[String]| f.run(a), &tmux()), None);
    }

    #[test]
    fn does_not_rezoom_a_window_unzoomed_meanwhile() {
        for mux in [tmux(), rex()] {
            let mut f = Fake::new(false, 2);
            let z = zoom_in(&mut |a: &[String]| f.run(a), &mux).unwrap();
            f.zoomed = false;
            z.zoom_out(&mut |a: &[String]| f.run(a));
            assert!(!f.zoomed);
            assert_eq!(f.zooms(), 1, "{mux:?}");
        }
    }

    #[test]
    fn a_failing_multiplexer_zooms_nothing() {
        for mux in [tmux(), rex()] {
            assert_eq!(zoom_in(&mut |_: &[String]| None, &mux), None);
        }
    }

    #[test]
    fn wanted_needs_the_setting_and_a_multiplexer() {
        let env = |tmux: bool| {
            move |k: &str| match k {
                "TMUX" if tmux => Some("/tmp/tmux-1/default,1,0".to_string()),
                "TMUX_PANE" => Some(PANE.to_string()),
                _ => None,
            }
        };
        assert_eq!(wanted(true, env(true)), Some(tmux()));
        assert_eq!(wanted(false, env(true)), None);
        assert_eq!(wanted(true, env(false)), None);
        assert_eq!(wanted(true, |_: &str| Some(String::new())), None);
        let rex_env = |k: &str| match k {
            "REX_SESSION" => Some("session:s".to_string()),
            "REX_BLOCK" => Some(BLOCK.to_string()),
            _ => None,
        };
        assert_eq!(wanted(true, rex_env), Some(rex()));
    }
}
