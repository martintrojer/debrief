//! How a row's text maps to screen cells (spec Q25). `Row.text` stays raw
//! (yank, search, comments and excerpts use it as is); rendering, cursor
//! columns and search highlights all go through [`cells`]:
//! - a tab runs to the next multiple of [`TAB_WIDTH`] columns;
//! - a trailing `\r` (a CRLF line) is not drawn and takes no column;
//! - any other control character is one column, drawn as a dimmed `?`;
//! - other zero-width graphemes are skipped.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Tab stops every this many columns (ramble draws a tab as 4 cells).
pub const TAB_WIDTH: usize = 4;

/// What a cell draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph<'a> {
    /// The grapheme itself.
    Text(&'a str),
    /// Spaces up to the next tab stop.
    Tab,
    /// A control character, drawn as `?`.
    Control,
}

/// One grapheme of a row's text on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayCell<'a> {
    /// Byte offset of the grapheme in the text.
    pub byte: usize,
    /// First display column.
    pub col: usize,
    /// Columns it takes (at least 1).
    pub width: usize,
    pub glyph: Glyph<'a>,
}

/// The screen cells of `text`, in order.
pub fn cells(text: &str) -> Vec<DisplayCell<'_>> {
    let body = text.strip_suffix('\r').unwrap_or(text);
    let mut col = 0;
    let mut out = Vec::new();
    for (byte, g) in body.grapheme_indices(true) {
        let (glyph, width) = if g == "\t" {
            (Glyph::Tab, TAB_WIDTH - col % TAB_WIDTH)
        } else if g.chars().any(char::is_control) {
            (Glyph::Control, 1)
        } else {
            (Glyph::Text(g), g.width())
        };
        if width == 0 {
            continue;
        }
        out.push(DisplayCell {
            byte,
            col,
            width,
            glyph,
        });
        col += width;
    }
    out
}

/// Display column of byte `b` in `text`: the columns of every cell that
/// starts before it.
pub fn col_of(text: &str, b: usize) -> usize {
    cells(text)
        .iter()
        .take_while(|c| c.byte < b)
        .map(|c| c.width)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str) -> Vec<(usize, usize, usize)> {
        cells(text)
            .iter()
            .map(|c| (c.byte, c.col, c.width))
            .collect()
    }

    #[test]
    fn tabs_run_to_the_next_stop() {
        assert_eq!(layout("\tx"), [(0, 0, 4), (1, 4, 1)]);
        assert_eq!(
            layout("ab\tx"),
            [(0, 0, 1), (1, 1, 1), (2, 2, 2), (3, 4, 1)]
        );
        assert_eq!(layout("abcd\tx")[4], (4, 4, 4));
        assert_eq!(layout("日\t")[1], (3, 2, 2));
    }

    #[test]
    fn cr_and_control_characters() {
        let c = cells("a\tb\rc");
        let g: Vec<_> = c.iter().map(|c| (c.col, c.glyph)).collect();
        assert_eq!(
            g,
            [
                (0, Glyph::Text("a")),
                (1, Glyph::Tab),
                (4, Glyph::Text("b")),
                (5, Glyph::Control),
                (6, Glyph::Text("c")),
            ]
        );
        assert_eq!(layout("ab\r"), [(0, 0, 1), (1, 1, 1)], "trailing CR hidden");
        assert!(cells("\r").is_empty());
        assert_eq!(cells("\x1b[0m")[0].glyph, Glyph::Control);
        assert_eq!(cells("x\u{7f}")[1].glyph, Glyph::Control);
    }

    #[test]
    fn col_of_maps_bytes_through_tabs() {
        assert_eq!(col_of("\tfoo", 1), 4);
        assert_eq!(col_of("\tfoo", 4), 7);
        assert_eq!(col_of("a\tb\rc", 4), 6);
        assert_eq!(col_of("a日b", 4), 3);
    }
}
