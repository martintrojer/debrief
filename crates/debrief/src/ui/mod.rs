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
    pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
    pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
    pub const PINK: Color = Color::Rgb(0xf5, 0xc2, 0xe7);
    /// Background of added rows: green blended 20% into base (tuicr's
    /// `diff_add_bg` for Catppuccin), `#394545`.
    pub const ADD_BG: Color = blend(GREEN, 20);
    /// Background of removed rows: red blended 20% into base, `#483346`.
    pub const DEL_BG: Color = blend(RED, 20);
    /// Background of hunk headers: mauve blended 12% into base.
    pub const HUNK_BG: Color = blend(MAUVE, 12);

    /// `fg` blended `pct` percent over [`BASE`] (tuicr's `blend`).
    pub const fn blend(fg: Color, pct: u16) -> Color {
        let (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) = (fg, BASE) else {
            return fg;
        };
        Color::Rgb(mix(r, br, pct), mix(g, bg, pct), mix(b, bb, pct))
    }

    const fn mix(c: u8, base: u8, pct: u16) -> u8 {
        ((c as u16 * pct + base as u16 * (100 - pct)) / 100) as u8
    }
}

/// Draw one full screen: the log or the commit, the status line, overlays.
pub fn draw(frame: &mut Frame, app: &App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    match app.screen() {
        Screen::Log => {
            let l = app.log_layout(main);
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

#[cfg(test)]
mod tests {
    use super::palette::*;
    use ratatui::style::Color;

    #[test]
    fn blends_match_the_mocha_values() {
        assert_eq!(ADD_BG, Color::Rgb(0x39, 0x45, 0x45));
        assert_eq!(DEL_BG, Color::Rgb(0x48, 0x33, 0x46));
        assert_eq!(blend(GREEN, 16), Color::Rgb(0x33, 0x3d, 0x40));
        assert_eq!(blend(RED, 16), Color::Rgb(0x40, 0x2f, 0x41));
    }
}
