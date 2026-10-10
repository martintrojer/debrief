//! Syntax highlighting of diff lines, per hunk as tuicr does it.
//!
//! Each hunk is highlighted on its own, from a fresh parser state, as two
//! runs: the old side (context and removed lines) and the new side
//! (context and added lines). Removed lines take their colours from the
//! old run, added and context lines from the new one, so a removed `/*`
//! never turns the added lines into a comment. A context line's old-run
//! colours are kept too, for the left side of side by side. A hunk that starts inside a
//! multi-line string or comment is coloured wrongly; tuicr accepts that.
//!
//! The result is foreground colours only, as byte ranges into
//! `DiffLine::text`; the add/remove row backgrounds stay the renderer's.
//! The syntect setup is ramble's (`render.rs`): two-face's syntax set and
//! its Catppuccin Mocha theme.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::vcs::{CommitDiff, FileDiff, Hunk, LineKind};

/// A file with more diff lines than this is not highlighted.
pub const MAX_FILE_LINES: usize = 10_000;
/// A line longer than this (in bytes; minified code) is not highlighted.
pub const MAX_LINE_BYTES: usize = 2_000;

/// One line's colours: styles over byte ranges of its text, in order.
pub type Spans = Vec<(Style, Range<usize>)>;

/// Colours of one hunk, indexed like `Hunk::lines`; `None` is a plain
/// line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HunkSpans {
    /// Each line's colours: removed lines the old run's, the rest the new.
    pub lines: Vec<Option<Spans>>,
    /// A context line's colours in the old run (`None` on other lines).
    pub old: Vec<Option<Spans>>,
}

/// Colours of a whole diff, indexed `[file][hunk]` like
/// `CommitDiff::files[f].hunks[h]`.
pub type DiffSpans = Vec<Vec<HunkSpans>>;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn code_theme() -> &'static syntect::highlighting::Theme {
    static THEME: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::CatppuccinMocha)
            .clone()
    })
}

fn syntect_style(st: syntect::highlighting::Style) -> Style {
    let fg = st.foreground;
    let mut style = Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if st.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if st.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if st.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// The syntax for `path`: by extension (as is, then lowercased), by file
/// name (`Makefile`), else by `first_line` (a shebang). `None` for plain
/// text.
fn syntax_for(path: &str, first_line: Option<&str>) -> Option<&'static SyntaxReference> {
    let ss = syntaxes();
    let p = Path::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let found = [ext.to_string(), ext.to_lowercase()]
        .iter()
        .filter(|e| !e.is_empty())
        .find_map(|e| ss.find_syntax_by_extension(e))
        .or_else(|| {
            (!name.is_empty())
                .then(|| ss.find_syntax_by_token(name))
                .flatten()
        })
        .or_else(|| first_line.and_then(|l| ss.find_syntax_by_first_line(l)));
    found.filter(|s| s.name != ss.find_syntax_plain_text().name)
}

/// The file's first line: the new side's line 1, else (a deleted or
/// emptied file, or an empty new line 1) the old side's. `None` when the
/// first hunk does not start at line 1 on either side.
fn first_line(f: &FileDiff) -> Option<&str> {
    let h = f.hunks.first()?;
    let line1 = |start: u32, side: LineKind| {
        (start == 1)
            .then(|| {
                h.lines
                    .iter()
                    .find(|l| l.kind == side || l.kind == LineKind::Context)
            })
            .flatten()
            .map(|l| l.text.as_str())
            .filter(|t| !t.is_empty())
    };
    line1(h.new_start, LineKind::Added).or_else(|| line1(h.old_start, LineKind::Removed))
}

/// Colours for every line of `diff`; `None` when no line has any (all
/// plain text, or too large), so there is nothing to send.
pub fn highlight_diff(diff: &CommitDiff) -> Option<DiffSpans> {
    let spans: DiffSpans = diff.files.iter().map(highlight_file).collect();
    let any = spans
        .iter()
        .flatten()
        .any(|h| h.lines.iter().any(Option::is_some));
    any.then_some(spans)
}

/// Colours for one file's hunks; all `None` for plain text and for files
/// over [`MAX_FILE_LINES`].
pub fn highlight_file(f: &FileDiff) -> Vec<HunkSpans> {
    let plain = || {
        f.hunks
            .iter()
            .map(|h| HunkSpans {
                lines: vec![None; h.lines.len()],
                old: vec![None; h.lines.len()],
            })
            .collect()
    };
    let total: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
    if total > MAX_FILE_LINES {
        return plain();
    }
    let path = super::commit::file_path(f);
    match syntax_for(path, first_line(f)) {
        Some(syntax) => f.hunks.iter().map(|h| highlight_hunk(h, syntax)).collect(),
        None => plain(),
    }
}

/// One hunk, from a fresh state: the old side and the new side apart.
fn highlight_hunk(h: &Hunk, syntax: &SyntaxReference) -> HunkSpans {
    let ss = syntaxes();
    let mut old = HighlightLines::new(syntax, code_theme());
    let mut new = HighlightLines::new(syntax, code_theme());
    let run = |hl: &mut HighlightLines, text: &str| -> Option<Spans> {
        if text.len() > MAX_LINE_BYTES {
            return None;
        }
        // The syntax set expects each line with its newline.
        let line = format!("{text}\n");
        let pieces = hl.highlight_line(&line, ss).ok()?;
        let mut out = Vec::new();
        let mut at = 0;
        for (st, piece) in pieces {
            let end = (at + piece.len()).min(text.len());
            if at < end {
                out.push((syntect_style(st), at..end));
            }
            at += piece.len();
        }
        Some(out)
    };
    let mut out = HunkSpans::default();
    for l in &h.lines {
        let (line, old_ctx) = match l.kind {
            LineKind::Removed => (run(&mut old, &l.text), None),
            LineKind::Added => (run(&mut new, &l.text), None),
            LineKind::Context => {
                let o = run(&mut old, &l.text);
                (run(&mut new, &l.text), o)
            }
        };
        out.lines.push(line);
        out.old.push(old_ctx);
    }
    out
}

/// The style `spans` give byte `b`, if any.
pub fn style_at(spans: &[(Style, Range<usize>)], b: usize) -> Option<Style> {
    let i = spans.partition_point(|(_, r)| r.end <= b);
    spans
        .get(i)
        .filter(|(_, r)| r.contains(&b))
        .map(|&(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::DiffLine;

    fn line(kind: LineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old: None,
            new: None,
            text: text.into(),
            no_newline: false,
        }
    }

    fn file(path: &str, start: u32, lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            old_path: Some(path.into()),
            new_path: Some(path.into()),
            binary: false,
            note: None,
            mode_change: None,
            copied: false,
            hunks: vec![Hunk {
                header: "@@".into(),
                old_start: start,
                new_start: start,
                lines,
            }],
        }
    }

    /// The colour of the first byte of `word` in line `l`.
    fn fg_of(spans: &[Option<Spans>], l: usize, text: &str, word: &str) -> Option<Color> {
        let b = text.find(word).unwrap();
        style_at(spans[l].as_ref()?, b)?.fg
    }

    #[test]
    fn rust_keywords_coloured_on_both_sides() {
        let lines = vec![
            line(LineKind::Removed, "fn old() -> u32 { 1 }"),
            line(LineKind::Added, "fn new() -> u32 { 2 }"),
            line(LineKind::Context, "let x = 3;"),
        ];
        let f = file("src/a.rs", 1, lines);
        let s = &highlight_file(&f)[0].lines;
        let rem = fg_of(s, 0, "fn old", "fn").unwrap();
        let add = fg_of(s, 1, "fn new", "fn").unwrap();
        let name = fg_of(s, 1, "fn new", "new").unwrap();
        assert_eq!(rem, add, "the keyword gets one colour on both sides");
        assert_ne!(add, name, "keyword and name differ");
        assert_eq!(fg_of(s, 2, "let x", "let"), Some(add));
    }

    #[test]
    fn a_hunk_starting_mid_function_still_highlights() {
        let lines = vec![
            line(LineKind::Context, "        let y = x + 1;"),
            line(LineKind::Context, "    }"),
            line(LineKind::Added, "    return y;"),
        ];
        let f = file("lib.rs", 40, lines);
        let s = &highlight_file(&f)[0].lines;
        let kw = fg_of(s, 2, "    return y;", "return").unwrap();
        let var = fg_of(s, 2, "    return y;", "y").unwrap();
        assert_ne!(kw, var);
        assert!(fg_of(s, 0, "        let y = x + 1;", "let").is_some());
    }

    #[test]
    fn the_sides_do_not_leak_into_each_other() {
        let lines = vec![
            line(LineKind::Removed, "/* opened"),
            line(LineKind::Added, "fn ok() {}"),
        ];
        let s = &highlight_file(&file("a.rs", 1, lines))[0].lines;
        let comment = fg_of(s, 0, "/* opened", "opened").unwrap();
        assert_ne!(fg_of(s, 1, "fn ok() {}", "fn"), Some(comment));
    }

    #[test]
    fn a_context_line_keeps_its_old_side_colours_too() {
        let lines = vec![
            line(LineKind::Removed, "/*"),
            line(LineKind::Added, "// head"),
            line(LineKind::Context, "int shared = 1;"),
        ];
        let h = &highlight_file(&file("x.c", 1, lines))[0];
        let t = "int shared = 1;";
        let new = fg_of(&h.lines, 2, t, "int").unwrap();
        let old = fg_of(&h.old, 2, t, "int").unwrap();
        let comment = fg_of(&h.lines, 1, "// head", "head").unwrap();
        assert_eq!(old, comment, "inside the old side's comment");
        assert_ne!(new, old, "the new side's keyword");
        assert!(h.old[0].is_none() && h.old[1].is_none(), "context only");
    }

    #[test]
    fn unknown_extension_and_shebang() {
        let f = file("notes.zzzz", 1, vec![line(LineKind::Added, "fn x() {}")]);
        assert_eq!(highlight_file(&f)[0].lines, [None]);
        let f = file(
            "run",
            1,
            vec![
                line(LineKind::Added, "#!/bin/sh"),
                line(LineKind::Added, "echo hi"),
            ],
        );
        assert!(highlight_file(&f)[0].lines[1].is_some(), "shebang picks sh");
        let f = file("Makefile", 5, vec![line(LineKind::Added, "all: x")]);
        assert!(highlight_file(&f)[0].lines[0].is_some());
    }

    #[test]
    fn a_deleted_or_emptied_script_uses_its_old_first_line() {
        let gone = vec![
            line(LineKind::Removed, "#!/bin/sh"),
            line(LineKind::Removed, "echo hello"),
        ];
        // `git rm run`: `@@ -1,2 +0,0 @@`.
        let mut f = file("run", 1, gone);
        f.hunks[0].new_start = 0;
        f.new_path = None;
        let s = &highlight_file(&f)[0].lines;
        assert!(s.iter().all(Option::is_some), "shell colours: {s:?}");
        let echo = fg_of(s, 1, "echo hello", "echo").unwrap();
        assert_ne!(fg_of(s, 1, "echo hello", "hello"), Some(echo));
        // The new line 1 is empty: the old one still decides.
        let f = file(
            "run",
            1,
            vec![
                line(LineKind::Removed, "#!/bin/sh"),
                line(LineKind::Added, ""),
                line(LineKind::Removed, "echo hello"),
            ],
        );
        assert!(highlight_file(&f)[0].lines[2].is_some());
    }

    #[test]
    fn caps_leave_lines_plain() {
        let long = "x".repeat(MAX_LINE_BYTES + 1);
        let f = file(
            "a.rs",
            1,
            vec![
                line(LineKind::Added, &long),
                line(LineKind::Added, "fn a() {}"),
            ],
        );
        let s = &highlight_file(&f)[0].lines;
        assert!(s[0].is_none());
        assert!(s[1].is_some());
        let many = (0..=MAX_FILE_LINES)
            .map(|_| line(LineKind::Added, "fn a() {}"))
            .collect();
        let s = highlight_file(&file("a.rs", 1, many));
        assert!(s[0].lines.iter().all(Option::is_none));
    }

    #[test]
    fn spans_cover_the_text_without_its_newline() {
        let text = "let s = \"hi\";\r";
        let s = &highlight_file(&file("a.rs", 1, vec![line(LineKind::Added, text)]))[0].lines;
        let spans = s[0].as_ref().unwrap();
        assert_eq!(spans.first().unwrap().1.start, 0);
        assert_eq!(spans.last().unwrap().1.end, text.len());
        assert!(style_at(spans, text.len()).is_none());
    }
}
