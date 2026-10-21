//! ratatui widgets only, no logic. `draw` lays out the screen for the
//! current screen and calls one widget per area.

pub mod clue;
mod comment_box;
mod commit;
mod header;
mod help;
mod log;
mod picker;
mod popup;
mod sidebar;
mod split;
mod stat;
mod status;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;

use crate::app::{App, Screen};

/// With `[theme] fill`, paint the theme's background and text colour under
/// `area`. Called first for the whole frame, and again for an area a widget
/// clears before drawing.
pub(crate) fn fill(frame: &mut Frame, app: &App, area: Rect) {
    let t = app.theme();
    if t.fill {
        let style = Style::new().fg(t.palette.text).bg(t.palette.bg);
        frame.buffer_mut().set_style(area, style);
    }
}

/// Draw one full screen: the log or the commit, the status line, overlays.
pub fn draw(frame: &mut Frame, app: &App) {
    fill(frame, app, frame.area());
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    match app.screen() {
        Screen::Log => {
            let l = app.log_layout(main);
            if let Some(chip) = l.chip {
                log::draw_chip(frame, app, chip);
            }
            log::draw(frame, app, l.log);
            if let Some(pane) = l.stats {
                stat::draw(frame, app, pane, l.side_by_side);
            }
        }
        Screen::Commit(_) => {
            let l = app.commit_layout(main);
            if let Some(h) = l.header {
                header::draw(frame, app, h);
            }
            // The diff first: an overlaid file list is drawn over it.
            commit::draw(frame, app, l.diff);
            if let Some(side) = l.sidebar {
                sidebar::draw(frame, app, side);
            }
            if let Some(list) = l.commits {
                sidebar::draw_commits(frame, app, list);
            }
            // Side by side, the box sits on the cursor's half, by its rows.
            comment_box::draw(frame, app, app.side_pane(l.diff));
        }
    }
    status::draw(frame, app, status);
    clue::draw(frame, app, main);
    popup::draw(frame, app, main);
    picker::draw(frame, app, main);
    help::draw(frame, app, main);
}
