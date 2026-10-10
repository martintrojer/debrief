//! Visual mode in the commit view, line-granular (`v` behaves like `V`),
//! and `y` copying the selected rows as diff text over OSC 52 (ramble's
//! effect.rs).

use super::commit::diff_text;
use super::{App, Mode};

/// The OSC 52 "set clipboard" sequence for `text`.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl App {
    /// `v` / `V`: start a selection at the cursor row; again leaves.
    pub(super) fn visual_start(&mut self) {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
            return;
        }
        let Some(v) = self.commit.as_mut().filter(|v| !v.rows.is_empty()) else {
            return;
        };
        v.anchor = v.cur.row;
        self.mode = Mode::Visual;
    }

    /// The selected rows, ordered and inclusive; `None` outside visual mode.
    pub fn selection_rows(&self) -> Option<(usize, usize)> {
        if self.mode != Mode::Visual {
            return None;
        }
        let v = self.commit.as_ref()?;
        Some((v.anchor.min(v.cur.row), v.anchor.max(v.cur.row)))
    }

    /// `y`: copy the selection's diff text (one `+`/`-`/space prefix on
    /// diff lines), one row per line, and leave visual mode.
    pub(super) fn yank(&mut self) {
        let Some((lo, hi)) = self.selection_rows() else {
            return;
        };
        let rows = self.commit_rows();
        let hi = hi.min(rows.len().saturating_sub(1));
        // Side by side: the selected side's rows, no padding.
        let text: String = rows[lo..=hi]
            .iter()
            .filter(|r| r.kind != super::commit::RowKind::Filler)
            .map(|r| diff_text(r) + "\n")
            .collect();
        if text.is_empty() {
            return;
        }
        let n = hi - lo + 1;
        self.output.extend(osc52(&text).into_bytes());
        self.mode = Mode::Normal;
        if let Some(v) = &mut self.commit {
            v.goto_row(lo);
        }
        self.set_status(format!("copied {n} line{}", if n == 1 { "" } else { "s" }));
    }

    /// Bytes for the terminal (OSC 52) queued since the last call; the
    /// event loop writes them after each event.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
