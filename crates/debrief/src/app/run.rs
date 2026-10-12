//! The terminal and the event loop (no tokio: crossterm polling plus an
//! mpsc channel for watcher events).

use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use ratatui::DefaultTerminal;

use super::config::Config;
use super::{App, AppEvent, zoom};

/// How long one loop iteration waits for terminal input.
const POLL: Duration = Duration::from_millis(100);

/// Run the interactive TUI until the user quits.
pub fn run(mut app: App) -> anyhow::Result<()> {
    let mouse = capture_wanted(app.config());
    // `[view] tmux_zoom`: zoom before the terminal is set up, so the first
    // draw already has the whole window.
    let zoom = zoom::wanted(app.config().view.tmux_zoom, |k| std::env::var(k).ok())
        .and_then(|pane| zoom::zoom_in(&mut zoom::tmux, &pane));
    // Installs a panic hook that restores the terminal before unwinding.
    let mut terminal = match ratatui::try_init().context("initialising terminal") {
        Ok(t) => t,
        Err(e) => {
            unzoom(zoom.as_ref());
            return Err(e);
        }
    };
    let hook = std::panic::take_hook();
    let panic_zoom = zoom.clone();
    std::panic::set_hook(Box::new(move |info| {
        let _ = input_off(mouse);
        unzoom(panic_zoom.as_ref());
        hook(info);
    }));
    let result = input_on(mouse)
        .map_err(anyhow::Error::from)
        .and_then(|()| event_loop(&mut terminal, &mut app, mouse));
    let _ = input_off(mouse);
    ratatui::restore();
    unzoom(zoom.as_ref());
    result
}

fn unzoom(zoom: Option<&zoom::Zoom>) {
    if let Some(z) = zoom {
        z.zoom_out(&mut zoom::tmux);
    }
}

/// Capture the mouse (clicks, drags, the wheel arrive as
/// [`Event::Mouse`]): `[mouse] enabled`. Off, the terminal keeps its own
/// selection and scrolling.
pub fn capture_wanted(cfg: &Config) -> bool {
    cfg.mouse.enabled
}

/// Bracketed paste (a paste arrives as one [`Event::Paste`], not as keys:
/// a pasted newline would otherwise be Enter), and mouse capture when
/// `mouse`.
fn input_on(mouse: bool) -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;
    if mouse {
        crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    Ok(())
}

/// Undo [`input_on`]; both are turned off even when one fails.
fn input_off(mouse: bool) -> std::io::Result<()> {
    let capture = if mouse {
        crossterm::execute!(std::io::stdout(), DisableMouseCapture)
    } else {
        Ok(())
    };
    crossterm::execute!(std::io::stdout(), DisableBracketedPaste)?;
    capture
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, mouse: bool) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();
    app.set_sender(tx);
    if let Err(e) = app.start_watcher() {
        app.set_status(format!("live reload off: {e:#}"));
    }
    while !app.should_quit() {
        if app.pending_effect().is_some() {
            suspend_and_run(terminal, mouse, || app.run_pending_effect())?;
        }
        terminal.draw(|f| app.render(f))?;
        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) => app.event(AppEvent::Key(key)),
                Event::Resize(cols, rows) => app.event(AppEvent::Resize(cols, rows)),
                Event::Paste(text) => app.event(AppEvent::Paste(text)),
                Event::Mouse(m) => app.event(AppEvent::Mouse(m)),
                _ => {}
            }
        }
        // Several watcher events in one iteration are one reload.
        let mut reload = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Reload => reload = true,
                other => app.event(other),
            }
        }
        if reload {
            app.event(AppEvent::Reload);
        }
        app.tick(Instant::now());
        let out = app.take_output();
        if !out.is_empty() {
            // OSC 52 clipboard writes go straight to the terminal.
            let mut stdout = std::io::stdout();
            stdout.write_all(&out)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Leave raw mode, bracketed paste, mouse capture (when `mouse`) and the
/// alternate screen, run `f` (which may use the terminal), then re-enter
/// and clear so the next draw repaints fully (ramble's `suspend_and_run`).
pub fn suspend_and_run<R>(
    terminal: &mut DefaultTerminal,
    mouse: bool,
    f: impl FnOnce() -> R,
) -> anyhow::Result<R> {
    input_off(mouse)?;
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    let r = f();
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::terminal::enable_raw_mode()?;
    input_on(mouse)?;
    terminal.clear()?;
    Ok(r)
}
