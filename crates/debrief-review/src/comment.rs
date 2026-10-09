//! The comment model: a body anchored to lines of a file, optionally at a revision.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// The revision a diff comment was made on: a git commit, or a jj commit plus change ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rev {
    pub commit: String,
    pub change: Option<String>,
    pub summary: Option<String>,
}

/// Which side of a diff the comment's lines refer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

/// One review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    /// Repo-relative, `/` separators.
    pub path: String,
    /// `None` means the working copy.
    pub rev: Option<Rev>,
    /// Set only for diff comments.
    pub side: Option<Side>,
    /// 1-based, inclusive.
    pub lines: (u32, u32),
    pub excerpt: String,
    pub body: String,
    /// Unix seconds.
    pub created: u64,
    /// Lowercase kind id (`"issue"`, …); `None` is untyped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// A comment kind: a lowercase id and, optionally, what it asks of the agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Kind {
    pub id: String,
    pub definition: Option<String>,
}

impl Kind {
    /// The label shown for kind `id`: the id uppercased.
    pub fn label(id: &str) -> String {
        id.to_uppercase()
    }
}

/// The built-in kinds, used when a tool's config doesn't list its own.
pub fn builtin_kinds() -> Vec<Kind> {
    [
        ("issue", "Something is wrong. Fix it."),
        (
            "suggestion",
            "A better way to do it. Apply it, or say in your reply why not.",
        ),
        (
            "question",
            "The reviewer wants an answer. Answer in your reply; change code only if the answer shows a problem.",
        ),
        ("nit", "Minor or cosmetic. Fix it if it's cheap."),
    ]
    .into_iter()
    .map(|(id, def)| Kind {
        id: id.to_owned(),
        definition: Some(def.to_owned()),
    })
    .collect()
}

impl Comment {
    /// A comment on lines of a file in the working copy. `id` and `created` are set by `Review::add`.
    pub fn on_file(
        path: impl Into<String>,
        lines: RangeInclusive<u32>,
        excerpt: impl Into<String>,
        body: impl Into<String>,
    ) -> Comment {
        Comment {
            id: 0,
            path: path.into(),
            rev: None,
            side: None,
            lines: (*lines.start(), *lines.end()),
            excerpt: excerpt.into(),
            body: body.into(),
            created: 0,
            kind: None,
        }
    }

    /// The same comment with kind `kind` (`None` is untyped).
    pub fn with_kind(self, kind: Option<String>) -> Comment {
        Comment { kind, ..self }
    }

    /// A comment on one side of a diff at `rev`. `id` and `created` are set by `Review::add`.
    pub fn on_diff(
        path: impl Into<String>,
        rev: Rev,
        side: Side,
        lines: RangeInclusive<u32>,
        excerpt: impl Into<String>,
        body: impl Into<String>,
    ) -> Comment {
        Comment {
            rev: Some(rev),
            side: Some(side),
            ..Comment::on_file(path, lines, excerpt, body)
        }
    }
}
