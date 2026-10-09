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
        }
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
