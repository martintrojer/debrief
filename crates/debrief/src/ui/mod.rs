//! ratatui widgets only, no logic. `draw` lays out the screen and calls one
//! widget per area; later screens add a file and one call here.

mod clue;
mod commit;
mod help;
mod log;
mod sidebar;
mod status;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::{App, Screen};

/// Catppuccin Mocha colours, as ramble uses them.
pub mod palette {
    use ratatui::style::Color;

    pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
    pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
    pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
    pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
    pub const OVERLAY: Color = Color::Rgb(0x6c, 0x70, 0x86);
    pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
    pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
    pub const SKY: Color = Color::Rgb(0x89, 0xdc, 0xeb);
    /// Background of the cursor row (Catppuccin Mocha surface0).
    pub const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
    /// Background of overlays (Catppuccin Mocha base).
    pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
    /// Background of the status line (Catppuccin Mocha mantle).
    pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
}

/// Draw one full screen: the log or the commit, the status line, overlays.
pub fn draw(frame: &mut Frame, app: &App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    match app.screen() {
        Screen::Log => log::draw(frame, app, main),
        Screen::Commit(_) => {
            let l = app.commit_layout(main);
            if let Some(side) = l.sidebar {
                sidebar::draw(frame, app, side);
            }
            commit::draw(frame, app, l.diff);
        }
    }
    status::draw(frame, app, status);
    clue::draw(frame, app, main);
    help::draw(frame, app, main);
}
