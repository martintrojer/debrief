//! `[view] tmux_zoom`: inside tmux, zoom debrief's pane (`PREFIX z`) at
//! start and unzoom it on exit, for the room full diffs need.
//!
//! Only a zoom debrief made is undone: a window already zoomed at start
//! is left alone, and so is one the user (or a `C-h/j/k/l` move to
//! another pane, which tmux unzooms on) has unzoomed meanwhile.

/// Runs `tmux <args>` and returns its stdout, `None` on any failure.
pub type Tmux<'a> = dyn FnMut(&[&str]) -> Option<String> + 'a;

/// A zoom debrief made, to undo on exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zoom {
    pane: String,
}

/// The window of `pane` is zoomed.
fn zoomed(tmux: &mut Tmux, pane: &str) -> Option<bool> {
    let out = tmux(&["display-message", "-p", "-t", pane, "#{window_zoomed_flag}"])?;
    Some(out.trim() == "1")
}

/// Zoom `pane` (`$TMUX_PANE`) unless its window is already zoomed.
/// `Some` only when this call zoomed it; a window of one pane can't
/// zoom, so it gives `None` too.
pub fn zoom_in(tmux: &mut Tmux, pane: &str) -> Option<Zoom> {
    if zoomed(tmux, pane)? {
        return None;
    }
    tmux(&["resize-pane", "-Z", "-t", pane])?;
    zoomed(tmux, pane)?.then(|| Zoom { pane: pane.into() })
}

impl Zoom {
    /// Unzoom, if the window is still zoomed.
    pub fn zoom_out(&self, tmux: &mut Tmux) {
        if zoomed(tmux, &self.pane) == Some(true) {
            let _ = tmux(&["resize-pane", "-Z", "-t", &self.pane]);
        }
    }
}

/// The zoom to make at start: the setting is on and debrief runs inside
/// tmux (`$TMUX` and `$TMUX_PANE` set).
pub fn wanted(on: bool, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |k| env(k).filter(|v: &String| !v.is_empty());
    (on && set("TMUX").is_some()).then(|| set("TMUX_PANE"))?
}

/// Run the real `tmux`, no terminal I/O.
pub fn tmux(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("tmux")
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

    /// A fake tmux: one window of `panes`, zoomed or not; records calls.
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

        fn run(&mut self, args: &[&str]) -> Option<String> {
            self.calls.push(args.join(" "));
            match args[0] {
                "display-message" => Some(format!("{}\n", u8::from(self.zoomed))),
                "resize-pane" => {
                    if self.panes > 1 {
                        self.zoomed = !self.zoomed;
                    }
                    Some(String::new())
                }
                _ => None,
            }
        }

        fn resizes(&self) -> usize {
            self.calls
                .iter()
                .filter(|c| c.starts_with("resize"))
                .count()
        }
    }

    #[test]
    fn zooms_then_unzooms_its_own_zoom() {
        let mut f = Fake::new(false, 2);
        let z = zoom_in(&mut |a: &[&str]| f.run(a), "%3").unwrap();
        assert!(f.zoomed);
        assert!(f.calls.contains(&"resize-pane -Z -t %3".to_string()));
        z.zoom_out(&mut |a: &[&str]| f.run(a));
        assert!(!f.zoomed);
        assert_eq!(f.resizes(), 2);
    }

    #[test]
    fn leaves_a_window_already_zoomed_alone() {
        let mut f = Fake::new(true, 2);
        assert_eq!(zoom_in(&mut |a: &[&str]| f.run(a), "%3"), None);
        assert!(f.zoomed);
        assert_eq!(f.resizes(), 0);
    }

    #[test]
    fn a_lone_pane_makes_no_zoom_to_undo() {
        let mut f = Fake::new(false, 1);
        assert_eq!(zoom_in(&mut |a: &[&str]| f.run(a), "%3"), None);
    }

    #[test]
    fn does_not_rezoom_a_window_unzoomed_meanwhile() {
        let mut f = Fake::new(false, 2);
        let z = zoom_in(&mut |a: &[&str]| f.run(a), "%3").unwrap();
        f.zoomed = false;
        z.zoom_out(&mut |a: &[&str]| f.run(a));
        assert!(!f.zoomed);
        assert_eq!(f.resizes(), 1);
    }

    #[test]
    fn tmux_failing_zooms_nothing() {
        assert_eq!(zoom_in(&mut |_: &[&str]| None, "%3"), None);
    }

    #[test]
    fn wanted_needs_the_setting_and_tmux() {
        let env = |tmux: bool| {
            move |k: &str| match k {
                "TMUX" if tmux => Some("/tmp/tmux-1/default,1,0".to_string()),
                "TMUX_PANE" => Some("%3".to_string()),
                _ => None,
            }
        };
        assert_eq!(wanted(true, env(true)), Some("%3".into()));
        assert_eq!(wanted(false, env(true)), None);
        assert_eq!(wanted(true, env(false)), None);
        assert_eq!(wanted(true, |_: &str| Some(String::new())), None);
    }
}
