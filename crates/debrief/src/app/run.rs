//! The terminal and the event loop (no tokio: crossterm polling plus an
//! mpsc channel for watcher events).

use std::io::Write;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use ratatui::DefaultTerminal;

use super::{App, AppEvent};

/// How long one loop iteration waits for terminal input.
const POLL: Duration = Duration::from_millis(100);

/// Run the interactive TUI until the user quits.
pub fn run(mut app: App) -> anyhow::Result<()> {
    // Installs a panic hook that restores the terminal before unwinding.
    let mut terminal = ratatui::try_init().context("initialising terminal")?;
    paste_on()?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = paste_off();
        hook(info);
    }));
    let result = event_loop(&mut terminal, &mut app);
    let _ = paste_off();
    ratatui::restore();
    result
}

/// Bracketed paste: a paste arrives as one [`Event::Paste`], not as keys
/// (a pasted newline would otherwise be Enter).
fn paste_on() -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)
}

fn paste_off() -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), DisableBracketedPaste)
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();
    app.set_sender(tx);
    if let Err(e) = app.start_watcher() {
        app.set_status(format!("live reload off: {e:#}"));
    }
    while !app.should_quit() {
        if app.pending_effect().is_some() {
            suspend_and_run(terminal, || app.run_pending_effect())?;
        }
        terminal.draw(|f| app.render(f))?;
        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) => app.event(AppEvent::Key(key)),
                Event::Resize(cols, rows) => app.event(AppEvent::Resize(cols, rows)),
                Event::Paste(text) => app.event(AppEvent::Paste(text)),
                Event::Mouse(m) => {
                    use crossterm::event::MouseEventKind as K;
                    let down = match m.kind {
                        K::ScrollDown => Some(true),
                        K::ScrollUp => Some(false),
                        _ => None,
                    };
                    if let Some(down) = down {
                        app.event(AppEvent::Wheel {
                            col: m.column,
                            row: m.row,
                            down,
                        });
                    }
                }
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

/// Leave raw mode, bracketed paste and the alternate screen, run `f`
/// (which may use the terminal), then re-enter and clear so the next draw
/// repaints fully (ramble's `suspend_and_run`; debrief captures no mouse).
pub fn suspend_and_run<R>(
    terminal: &mut DefaultTerminal,
    f: impl FnOnce() -> R,
) -> anyhow::Result<R> {
    paste_off()?;
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    let r = f();
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::terminal::enable_raw_mode()?;
    paste_on()?;
    terminal.clear()?;
    Ok(r)
}
