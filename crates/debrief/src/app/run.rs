//! The terminal and the event loop (no tokio: crossterm polling plus an
//! mpsc channel for watcher events).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use super::{App, AppEvent};

/// How long one loop iteration waits for terminal input.
const POLL: Duration = Duration::from_millis(100);

/// Run the interactive TUI until the user quits.
pub fn run(mut app: App) -> anyhow::Result<()> {
    // Installs a panic hook that restores the terminal before unwinding.
    let mut terminal = ratatui::try_init().context("initialising terminal")?;
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();
    app.set_sender(tx);
    if let Err(e) = app.start_watcher() {
        app.set_status(format!("live reload off: {e:#}"));
    }
    while !app.should_quit() {
        terminal.draw(|f| app.render(f))?;
        if event::poll(POLL)? {
            match event::read()? {
                Event::Key(key) => app.event(AppEvent::Key(key)),
                Event::Resize(cols, rows) => app.event(AppEvent::Resize(cols, rows)),
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
    }
    Ok(())
}
