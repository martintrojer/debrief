use debrief_review::{Comment, Kind, Rev, Review, Side, builtin_kinds};
use tempfile::TempDir;

struct Fixture {
    _cache: TempDir,
    _root: TempDir,
    review: Review,
}

fn review(comments: Vec<Comment>) -> Fixture {
    let (cache, root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let mut review = Review::open(cache.path(), root.path()).unwrap();
    for c in comments {
        review.add(c).unwrap();
    }
    Fixture {
        _cache: cache,
        _root: root,
        review,
    }
}

fn md(comments: Vec<Comment>) -> String {
    review(comments)
        .review
        .to_markdown(Some(""), &builtin_kinds())
}

#[test]
fn empty_review_is_empty() {
    assert_eq!(
        review(vec![]).review.to_markdown(None, &builtin_kinds()),
        ""
    );
}

#[test]
fn default_preamble_names_the_root() {
    let f = review(vec![Comment::on_file("a.md", 3..=3, "x", "b")]);
    let root = f.review.root().display();
    assert_eq!(
        f.review.to_markdown(None, &builtin_kinds()),
        format!(
            "# Review feedback\n\n\
             The reviewer left these comments on your work in {root}.\n\
             Address each item, and reference its number in your reply.\n\n\
             ## Item 1\n- File: a.md\n- Line: 3\n- Revision: (working copy)\n```\nx\n```\nb\n\n"
        )
    );
}

#[test]
fn custom_preamble_replaces_default() {
    let f = review(vec![Comment::on_file("a.md", 3..=3, "x", "b")]);
    assert_eq!(
        f.review
            .to_markdown(Some("Fix these, please."), &builtin_kinds()),
        "# Review feedback\n\nFix these, please.\n\n\
         ## Item 1\n- File: a.md\n- Line: 3\n- Revision: (working copy)\n```\nx\n```\nb\n\n"
    );
}

#[test]
fn file_comment_single_line() {
    assert_eq!(
        md(vec![Comment::on_file(
            "docs/plan.md",
            7..=7,
            "Step one.\n",
            "Too vague."
        )]),
        "# Review feedback\n\n\
         ## Item 1\n- File: docs/plan.md\n- Line: 7\n- Revision: (working copy)\n\
         ```\nStep one.\n```\nToo vague.\n\n"
    );
}

#[test]
fn file_comment_range() {
    assert_eq!(
        md(vec![Comment::on_file(
            "docs/plan.md",
            12..=18,
            "line 12\nline 18",
            "Split this."
        )]),
        "# Review feedback\n\n\
         ## Item 1\n- File: docs/plan.md\n- Lines: 12-18\n- Revision: (working copy)\n\
         ```\nline 12\nline 18\n```\nSplit this.\n\n"
    );
}

#[test]
fn diff_new_side_jj_rev() {
    let rev = Rev {
        commit: "3f2a9c1e77aa".into(),
        change: Some("kmxqpwrz".into()),
        summary: Some("feat: parse things".into()),
    };
    assert_eq!(
        md(vec![Comment::on_diff(
            "src/x.rs",
            rev,
            Side::New,
            40..=44,
            "+fn parse() {}",
            "Name it parse_all."
        )]),
        "# Review feedback\n\n\
         ## Item 1\n- File: src/x.rs (new side)\n- Lines: 40-44\n\
         - Revision: kmxqpwrz 3f2a9c1e \"feat: parse things\"\n\
         ```diff\n+fn parse() {}\n```\nName it parse_all.\n\n"
    );
}

#[test]
fn diff_old_side_git_rev() {
    let rev = Rev {
        commit: "abcdef0123456789".into(),
        change: None,
        summary: None,
    };
    assert_eq!(
        md(vec![Comment::on_diff(
            "src/y.rs",
            rev,
            Side::Old,
            5..=5,
            "-old()",
            "Why remove this?"
        )]),
        "# Review feedback\n\n\
         ## Item 1\n- File: src/y.rs (old side)\n- Line: 5\n- Revision: abcdef01\n\
         ```diff\n-old()\n```\nWhy remove this?\n\n"
    );
}

#[test]
fn excerpt_with_fence_gets_a_longer_fence() {
    assert_eq!(
        md(vec![Comment::on_file(
            "README.md",
            1..=3,
            "```sh\nls\n```",
            "b"
        )]),
        "# Review feedback\n\n\
         ## Item 1\n- File: README.md\n- Lines: 1-3\n- Revision: (working copy)\n\
         ````\n```sh\nls\n```\n````\nb\n\n"
    );
}

#[test]
fn items_numbered_in_comment_order() {
    let out = md(vec![
        Comment::on_file("b.md", 1..=1, "b", "second"),
        Comment::on_file("a.md", 1..=1, "a", "first"),
    ]);
    let first = out.find("## Item 1\n- File: a.md").unwrap();
    let second = out.find("## Item 2\n- File: b.md").unwrap();
    assert!(first < second);
}

fn typed(path: &str, kind: Option<&str>) -> Comment {
    Comment::on_file(path, 1..=1, "x", "b").with_kind(kind.map(str::to_owned))
}

#[test]
fn typed_items_get_a_label_and_a_legend_of_used_kinds_only() {
    assert_eq!(
        md(vec![
            typed("a.md", Some("question")),
            typed("b.md", None),
            typed("c.md", Some("issue")),
            typed("d.md", Some("issue")),
        ]),
        "# Review feedback\n\n\
         Items are tagged with a type:\n\
         - ISSUE: Something is wrong. Fix it.\n\
         - QUESTION: The reviewer wants an answer. Answer in your reply; \
         change code only if the answer shows a problem.\n\n\
         ## Item 1 [QUESTION]\n- File: a.md\n- Line: 1\n- Revision: (working copy)\n```\nx\n```\nb\n\n\
         ## Item 2\n- File: b.md\n- Line: 1\n- Revision: (working copy)\n```\nx\n```\nb\n\n\
         ## Item 3 [ISSUE]\n- File: c.md\n- Line: 1\n- Revision: (working copy)\n```\nx\n```\nb\n\n\
         ## Item 4 [ISSUE]\n- File: d.md\n- Line: 1\n- Revision: (working copy)\n```\nx\n```\nb\n\n"
    );
}

#[test]
fn legend_follows_the_preamble() {
    let f = review(vec![typed("a.md", Some("nit"))]);
    assert_eq!(
        f.review.to_markdown(Some("Fix these."), &builtin_kinds()),
        "# Review feedback\n\nFix these.\n\n\
         Items are tagged with a type:\n- NIT: Minor or cosmetic. Fix it if it's cheap.\n\n\
         ## Item 1 [NIT]\n- File: a.md\n- Line: 1\n- Revision: (working copy)\n```\nx\n```\nb\n\n"
    );
}

#[test]
fn undefined_kind_is_labelled_but_not_in_the_legend() {
    let kinds = [Kind {
        id: "praise".into(),
        definition: None,
    }];
    let out = review(vec![
        typed("a.md", Some("praise")),
        typed("b.md", Some("gone")),
    ])
    .review
    .to_markdown(Some(""), &kinds);
    assert!(!out.contains("Items are tagged"));
    assert!(out.contains("## Item 1 [PRAISE]\n"));
    assert!(out.contains("## Item 2 [GONE]\n"));
}

#[test]
fn untyped_batch_has_no_legend() {
    let out = md(vec![typed("a.md", None)]);
    assert!(!out.contains("Items are tagged"));
}
