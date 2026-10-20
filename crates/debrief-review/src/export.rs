//! Markdown export of a review batch, for an agent receiving feedback.

use std::fmt::Write;

use crate::comment::{Comment, Kind, Side};
use crate::store::Review;

impl Review {
    /// The batch as markdown. `None` uses the default preamble, `Some(p)` replaces
    /// it, `Some("")` drops it. An empty batch exports as `""`.
    ///
    /// Typed items get a `[KIND]` label in their heading, and a legend after the
    /// preamble lists the used kinds that `kinds` defines, in `kinds` order.
    pub fn to_markdown(&self, preamble: Option<&str>, kinds: &[Kind]) -> String {
        render(
            &self.root().display().to_string(),
            self.comments(),
            preamble,
            kinds,
        )
    }
}

pub(crate) fn render(
    root: &str,
    comments: &[Comment],
    preamble: Option<&str>,
    kinds: &[Kind],
) -> String {
    if comments.is_empty() {
        return String::new();
    }
    let mut out = String::from("# Review feedback\n\n");
    let default;
    let preamble = match preamble {
        Some(p) => p,
        None => {
            default = format!(
                "The reviewer left these comments on your work in {root}.\n\
                 Address each item, and reference its number in your reply."
            );
            &default
        }
    };
    if !preamble.is_empty() {
        out.push_str(preamble.trim_end_matches('\n'));
        out.push_str("\n\n");
    }
    legend(&mut out, comments, kinds);
    for (i, c) in comments.iter().enumerate() {
        item(&mut out, i + 1, c);
    }
    out
}

fn legend(out: &mut String, comments: &[Comment], kinds: &[Kind]) {
    let used: Vec<(&str, &str)> = kinds
        .iter()
        .filter(|k| comments.iter().any(|c| c.kind.as_deref() == Some(&k.id)))
        .filter_map(|k| Some((k.id.as_str(), k.definition.as_deref()?)))
        .collect();
    if used.is_empty() {
        return;
    }
    out.push_str("Items are tagged with a type:\n");
    for (id, def) in used {
        let _ = writeln!(out, "- {}: {def}", Kind::label(id));
    }
    out.push('\n');
}

fn item(out: &mut String, n: usize, c: &Comment) {
    match &c.kind {
        Some(k) => {
            let _ = writeln!(out, "## Item {n} [{}]", Kind::label(k));
        }
        None => {
            let _ = writeln!(out, "## Item {n}");
        }
    }
    let side = match c.side {
        Some(Side::Old) => " (old side)",
        Some(Side::New) => " (new side)",
        None => "",
    };
    let _ = writeln!(out, "- File: {}{side}", c.path);
    let (a, b) = c.lines;
    if a == b {
        let _ = writeln!(out, "- Line: {a}");
    } else {
        let _ = writeln!(out, "- Lines: {a}-{b}");
    }
    match &c.rev {
        None => out.push_str("- Revision: (working copy)\n"),
        Some(rev) => {
            let mut parts = Vec::new();
            if let Some(change) = &rev.change {
                parts.push(change.clone());
            }
            parts.push(rev.commit.chars().take(8).collect());
            if let Some(summary) = &rev.summary {
                parts.push(format!("\"{summary}\""));
            }
            let _ = writeln!(out, "- Revision: {}", parts.join(" "));
            if let Some(branch) = &rev.branch {
                let _ = writeln!(out, "- Branch: {}", one_line(branch));
            }
        }
    }
    let fence = "`".repeat(longest_backtick_run(&c.excerpt).max(2) + 1);
    let lang = if c.side.is_some() { "diff" } else { "" };
    let _ = writeln!(out, "{fence}{lang}");
    out.push_str(c.excerpt.trim_end_matches('\n'));
    let _ = writeln!(out, "\n{fence}");
    out.push_str(c.body.trim_end_matches('\n'));
    out.push_str("\n\n");
}

/// `s` on one line: control characters (newlines, carriage returns,
/// escapes) as Rust escapes, so a name can't start a line of its own.
fn one_line(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn longest_backtick_run(s: &str) -> usize {
    s.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}
