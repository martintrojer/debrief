//! Parser for git-style unified diffs (`git show -p`, `jj diff --git`).

use super::{DiffLine, FileDiff, Hunk, LineKind};

/// Parse git-style unified diff text into files, hunks and lines.
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut cur: Option<Section> = None;
    let mut lines = text.split('\n').peekable();

    while let Some(line) = lines.next() {
        // The final "" after a trailing newline is not a line.
        if line.is_empty() && lines.peek().is_none() {
            break;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(s) = cur.take() {
                files.push(s.finish());
            }
            cur = Some(Section::new(rest));
            continue;
        }
        let Some(s) = cur.as_mut() else {
            continue; // text before the first section
        };
        if s.in_hunk() {
            s.hunk_line(line);
        } else if line.starts_with('\\') {
            s.no_newline();
        } else if line.starts_with("@@") {
            s.start_hunk(line);
        } else {
            s.header_line(line);
        }
    }
    if let Some(s) = cur.take() {
        files.push(s.finish());
    }
    files
}

struct Section {
    old_path: Option<String>,
    new_path: Option<String>,
    added: bool,
    deleted: bool,
    binary: bool,
    copied: bool,
    old_mode: Option<String>,
    new_mode: Option<String>,
    hunks: Vec<Hunk>,
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
}

impl Section {
    fn new(header: &str) -> Self {
        let (old, new) = match header_paths(header) {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        };
        Section {
            old_path: old,
            new_path: new,
            added: false,
            deleted: false,
            binary: false,
            copied: false,
            old_mode: None,
            new_mode: None,
            hunks: Vec::new(),
            old_left: 0,
            new_left: 0,
            old_no: 0,
            new_no: 0,
        }
    }

    fn in_hunk(&self) -> bool {
        self.old_left > 0 || self.new_left > 0
    }

    fn header_line(&mut self, line: &str) {
        if line.starts_with("new file mode") {
            self.added = true;
        } else if line.starts_with("deleted file mode") {
            self.deleted = true;
        } else if let Some(m) = line.strip_prefix("old mode ") {
            self.old_mode = Some(m.to_string());
        } else if let Some(m) = line.strip_prefix("new mode ") {
            self.new_mode = Some(m.to_string());
        } else if let Some(p) = line.strip_prefix("copy from ") {
            self.copied = true;
            self.old_path = Some(decode_path(p));
        } else if let Some(p) = line.strip_prefix("rename from ") {
            self.old_path = Some(decode_path(p));
        } else if let Some(p) = line
            .strip_prefix("rename to ")
            .or_else(|| line.strip_prefix("copy to "))
        {
            self.new_path = Some(decode_path(p));
        } else if (line.starts_with("Binary files ") && line.ends_with(" differ"))
            || line == "GIT binary patch"
        {
            self.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            match marker_path(p, "a/") {
                Some(p) => self.old_path = Some(p),
                None => self.added = true,
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            match marker_path(p, "b/") {
                Some(p) => self.new_path = Some(p),
                None => self.deleted = true,
            }
        }
    }

    fn start_hunk(&mut self, line: &str) {
        let Some((old_start, old_len, new_start, new_len)) = parse_hunk_header(line) else {
            return;
        };
        self.old_left = old_len;
        self.new_left = new_len;
        self.old_no = old_start;
        self.new_no = new_start;
        self.hunks.push(Hunk {
            header: line.to_string(),
            old_start,
            new_start,
            lines: Vec::new(),
        });
    }

    fn hunk_line(&mut self, line: &str) {
        // Some tools strip the space off empty context lines.
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &line[1..]),
            Some(b'-') => (LineKind::Removed, &line[1..]),
            Some(b' ') => (LineKind::Context, &line[1..]),
            None => (LineKind::Context, ""),
            Some(b'\\') => {
                self.no_newline();
                return;
            }
            Some(_) => {
                // Malformed: end the hunk and treat the line as a header.
                self.old_left = 0;
                self.new_left = 0;
                self.header_line(line);
                return;
            }
        };
        let (old, new) = match kind {
            LineKind::Context => (Some(self.take_old()), Some(self.take_new())),
            LineKind::Added => (None, Some(self.take_new())),
            LineKind::Removed => (Some(self.take_old()), None),
        };
        if let Some(h) = self.hunks.last_mut() {
            h.lines.push(DiffLine {
                kind,
                old,
                new,
                text: text.to_string(),
                no_newline: false,
            });
        }
    }

    fn take_old(&mut self) -> u32 {
        self.old_left = self.old_left.saturating_sub(1);
        self.old_no += 1;
        self.old_no - 1
    }

    fn take_new(&mut self) -> u32 {
        self.new_left = self.new_left.saturating_sub(1);
        self.new_no += 1;
        self.new_no - 1
    }

    fn no_newline(&mut self) {
        if let Some(l) = self.hunks.last_mut().and_then(|h| h.lines.last_mut()) {
            l.no_newline = true;
        }
    }

    fn finish(self) -> FileDiff {
        FileDiff {
            old_path: if self.added { None } else { self.old_path },
            new_path: if self.deleted { None } else { self.new_path },
            binary: self.binary,
            hunks: self.hunks,
            note: None,
            mode_change: self.old_mode.zip(self.new_mode),
            copied: self.copied,
        }
    }
}

/// `@@ -a[,b] +c[,d] @@ ctx` -> (a, b, c, d).
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// Path on a `---` / `+++` line; `None` for `/dev/null`.
fn marker_path(p: &str, prefix: &str) -> Option<String> {
    // git appends a TAB when the path contains a space; strip only that.
    let p = p.strip_suffix('\t').unwrap_or(p);
    if p == "/dev/null" {
        return None;
    }
    let p = decode_path(p);
    Some(p.strip_prefix(prefix).map(str::to_string).unwrap_or(p))
}

/// Paths from `a/X b/Y` on the `diff --git` line.
fn header_paths(rest: &str) -> Option<(String, String)> {
    let (a, b) = if let Some(after) = rest.strip_prefix('"') {
        let (a, used) = unquote(after)?;
        let tail = after[used..].strip_prefix(' ')?;
        let b = match tail.strip_prefix('"') {
            Some(q) => unquote(q)?.0,
            None => tail.to_string(),
        };
        (a, b)
    } else if rest.ends_with('"') {
        let i = rest.rfind(" \"")?;
        (rest[..i].to_string(), unquote(&rest[i + 2..])?.0)
    } else {
        split_unquoted(rest)?
    };
    let strip = |s: String, p: &str| s.strip_prefix(p).map(str::to_string).unwrap_or(s);
    Some((strip(a, "a/"), strip(b, "b/")))
}

/// Split `a/X b/Y` without quotes: prefer equal halves (git guarantees
/// them when there is no rename), else the first ` b/`.
fn split_unquoted(rest: &str) -> Option<(String, String)> {
    if rest.len() >= 5 && (rest.len() - 5).is_multiple_of(2) {
        let n = (rest.len() - 5) / 2;
        if rest.is_char_boundary(2 + n) && rest.is_char_boundary(3 + n) {
            let (a, b) = (&rest[..2 + n], &rest[3 + n..]);
            if rest.as_bytes()[2 + n] == b' '
                && a.starts_with("a/")
                && b.starts_with("b/")
                && a[2..] == b[2..]
            {
                return Some((a.to_string(), b.to_string()));
            }
        }
    }
    let i = rest.find(" b/")?;
    Some((rest[..i].to_string(), rest[i + 1..].to_string()))
}

/// Decode a path that may be git C-quoted.
fn decode_path(p: &str) -> String {
    match p.strip_prefix('"').and_then(unquote) {
        Some((s, _)) => s,
        None => p.to_string(),
    }
}

/// Decode the body of a C-quoted string (after the opening quote).
/// Returns the decoded string and the bytes consumed including the
/// closing quote.
fn unquote(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), i + 1)),
            b'\\' => {
                let c = *b.get(i + 1)?;
                i += 2;
                match c {
                    b'0'..=b'7' => {
                        let mut v = u32::from(c - b'0');
                        for _ in 0..2 {
                            match b.get(i) {
                                Some(d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'v' => out.push(11),
                    other => out.push(other),
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    None
}
