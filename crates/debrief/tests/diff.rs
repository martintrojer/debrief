use debrief::vcs::{DiffLine, FileDiff, LineKind, parse_unified};

fn one(text: &str) -> FileDiff {
    let mut files = parse_unified(text);
    assert_eq!(files.len(), 1, "{files:#?}");
    files.remove(0)
}

fn paths(f: &FileDiff) -> (Option<&str>, Option<&str>) {
    (f.old_path.as_deref(), f.new_path.as_deref())
}

fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old,
        new,
        text: text.to_string(),
        no_newline: false,
    }
}

#[test]
fn empty_input() {
    assert!(parse_unified("").is_empty());
    assert!(parse_unified("\n").is_empty());
}

#[test]
fn modify() {
    let f = one("diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@ fn main() {
 one
-two
+TWO
 three
");
    assert_eq!(paths(&f), (Some("src/a.rs"), Some("src/a.rs")));
    assert!(!f.binary);
    assert_eq!(f.hunks.len(), 1);
    let h = &f.hunks[0];
    assert_eq!(h.header, "@@ -1,3 +1,3 @@ fn main() {");
    assert_eq!((h.old_start, h.new_start), (1, 1));
    use LineKind::*;
    assert_eq!(
        h.lines,
        vec![
            line(Context, Some(1), Some(1), "one"),
            line(Removed, Some(2), None, "two"),
            line(Added, None, Some(2), "TWO"),
            line(Context, Some(3), Some(3), "three"),
        ]
    );
}

#[test]
fn add() {
    let f = one("diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..3b18e51
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
");
    assert_eq!(paths(&f), (None, Some("new.txt")));
    let h = &f.hunks[0];
    assert_eq!((h.old_start, h.new_start), (0, 1));
    assert_eq!(h.lines[1], line(LineKind::Added, None, Some(2), "world"));
}

#[test]
fn delete() {
    let f = one("diff --git a/old.txt b/old.txt
deleted file mode 100644
index 3b18e51..0000000
--- a/old.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-hello
-world
");
    assert_eq!(paths(&f), (Some("old.txt"), None));
    assert_eq!(
        f.hunks[0].lines[1],
        line(LineKind::Removed, Some(2), None, "world")
    );
}

#[test]
fn rename_with_edit() {
    let f = one("diff --git a/a.txt b/b.txt
similarity index 80%
rename from a.txt
rename to b.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/b.txt
@@ -1,2 +1,2 @@
 keep
-old
+new
");
    assert_eq!(paths(&f), (Some("a.txt"), Some("b.txt")));
    assert_eq!(f.hunks[0].lines.len(), 3);
}

#[test]
fn pure_rename() {
    let f = one("diff --git a/dir/a.txt b/dir/b.txt
similarity index 100%
rename from dir/a.txt
rename to dir/b.txt
");
    assert_eq!(paths(&f), (Some("dir/a.txt"), Some("dir/b.txt")));
    assert!(f.hunks.is_empty());
}

#[test]
fn pure_rename_with_spaces() {
    let f = one("diff --git a/bin b/b in
similarity index 100%
rename from bin
rename to b in
");
    assert_eq!(paths(&f), (Some("bin"), Some("b in")));
}

#[test]
fn binary() {
    let f = one("diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
");
    assert!(f.binary);
    assert!(f.hunks.is_empty());
    assert_eq!(paths(&f), (Some("img.png"), Some("img.png")));
}

#[test]
fn binary_added() {
    let f = one("diff --git a/bin b/bin
new file mode 100644
index 0000000..bdc955b
Binary files /dev/null and b/bin differ
");
    assert!(f.binary);
    assert_eq!(paths(&f), (None, Some("bin")));
}

#[test]
fn no_newline_at_eof() {
    let f = one("diff --git a/a b/a
index 1111111..2222222 100644
--- a/a
+++ b/a
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
");
    let l = &f.hunks[0].lines;
    assert_eq!(l.len(), 2);
    assert!(l[0].no_newline && l[1].no_newline);
    assert_eq!((l[0].old, l[1].new), (Some(1), Some(1)));
}

#[test]
fn no_newline_only_on_old_side() {
    let f = one("diff --git a/a b/a
--- a/a
+++ b/a
@@ -1 +1,2 @@
-x
\\ No newline at end of file
+x
+y
");
    let l = &f.hunks[0].lines;
    assert!(l[0].no_newline);
    assert!(!l[1].no_newline && !l[2].no_newline);
}

#[test]
fn two_hunks() {
    let f = one("diff --git a/a b/a
--- a/a
+++ b/a
@@ -1,2 +1,3 @@
 a
+b
 c
@@ -10,2 +11,1 @@ ctx
 j
-k
");
    assert_eq!(f.hunks.len(), 2);
    let h = &f.hunks[1];
    assert_eq!((h.old_start, h.new_start), (10, 11));
    assert_eq!(h.lines[0], line(LineKind::Context, Some(10), Some(11), "j"));
    assert_eq!(h.lines[1], line(LineKind::Removed, Some(11), None, "k"));
}

#[test]
fn hunk_lines_that_look_like_headers() {
    // A removed line "-- x" and an added "++ y" are content, not headers.
    let f = one("diff --git a/a b/a
--- a/a
+++ b/a
@@ -1 +1 @@
--- x
+++ y
");
    let l = &f.hunks[0].lines;
    assert_eq!(l[0], line(LineKind::Removed, Some(1), None, "-- x"));
    assert_eq!(l[1], line(LineKind::Added, None, Some(1), "++ y"));
}

#[test]
fn several_files() {
    let files = parse_unified(
        "diff --git a/a b/a
--- a/a
+++ b/a
@@ -1 +1 @@
-1
+2
diff --git a/b b/b
deleted file mode 100644
index e69de29..0000000
",
    );
    assert_eq!(files.len(), 2);
    assert_eq!(paths(&files[1]), (Some("b"), None));
    assert!(files[1].hunks.is_empty());
}

#[test]
fn path_with_space_and_tab_delimiter() {
    let f = one("diff --git a/x y.txt b/z w.txt
similarity index 50%
rename from x y.txt
rename to z w.txt
--- a/x y.txt\t
+++ b/z w.txt\t
@@ -1 +1,2 @@
 a
+b
");
    assert_eq!(paths(&f), (Some("x y.txt"), Some("z w.txt")));
}

#[test]
fn trailing_space_in_path_is_kept() {
    let f = one("diff --git a/x  b/x 
--- a/x \t
+++ b/x \t
@@ -1 +1 @@
-a
+b
");
    assert_eq!(paths(&f), (Some("x "), Some("x ")));
}

#[test]
fn empty_added_file_with_space_in_name() {
    // Header only: paths come from the diff --git line, split in halves.
    let f = one("diff --git a/e b/x b/e b/x
new file mode 100644
index 0000000..e69de29
");
    assert_eq!(paths(&f), (None, Some("e b/x")));
}

#[test]
fn empty_deleted_file() {
    let f = one("diff --git a/gone b/gone
deleted file mode 100644
index e69de29..0000000
");
    assert_eq!(paths(&f), (Some("gone"), None));
}

#[test]
fn quoted_paths() {
    let f = one(r#"diff --git "a/\303\251 \"q\".txt" "b/\303\251 \"q\".txt"
new file mode 100644
index 0000000..4b04fff
--- /dev/null
+++ "b/\303\251 \"q\".txt"
@@ -0,0 +1 @@
+x
"#);
    assert_eq!(paths(&f), (None, Some("é \"q\".txt")));
}

#[test]
fn quoted_header_only_with_escapes() {
    let f = one(r#"diff --git "a/tab\there" "b/new\\line\n"
similarity index 100%
rename from "tab\there"
rename to "new\\line\n"
"#);
    assert_eq!(paths(&f), (Some("tab\there"), Some("new\\line\n")));
}

#[test]
fn quoted_header_only_from_diff_line() {
    let f = one(r#"diff --git "a/\303\251" "b/\303\251"
deleted file mode 100644
index e69de29..0000000
"#);
    assert_eq!(paths(&f), (Some("é"), None));
}

#[test]
fn mixed_quoting_on_diff_line() {
    let f = one(r#"diff --git a/plain "b/\303\251"
new file mode 100644
"#);
    assert_eq!(paths(&f), (None, Some("é")));
}

#[test]
fn empty_context_line_without_space() {
    let f = one("diff --git a/a b/a
--- a/a
+++ b/a
@@ -1,3 +1,3 @@
 a

-b
+c
");
    let l = &f.hunks[0].lines;
    assert_eq!(l[1], line(LineKind::Context, Some(2), Some(2), ""));
    assert_eq!(l[2].old, Some(3));
}

#[test]
fn jj_style_header() {
    // jj prints explicit lengths and 10-digit index hashes.
    let f = one("diff --git a/a.txt b/a.txt
index 7898192261..422c2b7ab3 100644
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,2 @@
 a
+b
");
    assert_eq!(f.hunks[0].lines.len(), 2);
    assert_eq!(f.hunks[0].lines[1].new, Some(2));
}

#[test]
fn mode_change_and_status() {
    use debrief::vcs::FileStatus;
    let f = one("diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
");
    assert_eq!(
        f.mode_change,
        Some(("100644".to_string(), "100755".to_string()))
    );
    assert!(f.hunks.is_empty());
    assert_eq!(f.status(), FileStatus::Modified);
    let f = one("diff --git a/a b/b
similarity index 90%
copy from a
copy to b
");
    assert_eq!(paths(&f), (Some("a"), Some("b")));
    assert_eq!((f.status(), f.status().letter()), (FileStatus::Copied, 'C'));
    let f = one("diff --git a/a b/b
similarity index 100%
rename from a
rename to b
");
    assert_eq!(f.status().letter(), 'R');
    assert_eq!(f.mode_change, None);
}
