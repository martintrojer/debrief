# debrief design

How debrief works and why. For what it does from the user's side, see the
[guide](guide.md); for every option,
[config.default.toml](../crates/debrief/config.default.toml).

## Why it exists

Catching up with coding agents means noticing new commits, reading them,
commenting on their lines and handing the comments back. What ties that
together is review: a comment on some lines of a file at a revision,
collected into a batch and handed back to whoever made the change. So the
comment model and the hand-back are shared, and the viewers stay
separate: ramble reads prose, debrief reads commits.

## Principles

- **Read-only.** debrief never checks out, amends, rebases or writes into
  a repo, so it is safe to run while agents commit. Review state lives in
  the user cache and state dirs. The one exception is jj's snapshot (see
  [Read-only VCS access](#read-only-vcs-access)).
- **git and jj are both first class.** Both run as CLIs behind a small
  `Vcs` trait. jj's templates are stable across versions and jj-lib's API
  is not; the CLIs are fast enough for a log and a diff. No git2, gix or
  jj-lib.
- **Threads and channels, no tokio.** Slow work runs on plain threads that
  send tagged events to the event loop.
- **Comments are never re-anchored.** Reviews are short-lived hand-backs,
  and the excerpt carries the meaning after lines move.
- **No shared UI crate with ramble.** debrief follows ramble's patterns
  (keymap, key clue, `g?`, motions, visual mode, sidebar, launchers) in
  its own code. The one shared file, `app/textbox.rs`, is kept
  byte-identical in both repos.

## Architecture

```
crates/
├── debrief-review   comment model, one batch per repo root, markdown export, hand-back; no UI, no VCS
│   ├── comment      Comment { id, path, rev, side, lines, excerpt, body, created, kind }
│   ├── store        Review: open / add / edit / remove / restore / clear, flock + rename
│   ├── export       batch → markdown for an agent
│   └── handback     [send] command on stdin, else OSC 52
└── debrief          the binary
    ├── cli          args → start options
    ├── vcs          Vcs trait; git.rs and jj.rs run the CLIs; diff.rs parses unified diffs
    ├── seen         new / updated / seen state per repo root
    ├── session      saved review sessions on disk
    ├── app          state and event loop: log, commit, split, comment, review, picker,
    │                session, sidebar, file_tree, pane_nav, highlight, stat, watch, launch,
    │                keys, help
    └── ui           ratatui widgets only, no logic
```

ramble depends on `debrief-review` for its comment mode.

## Data flow

1. `vcs::open` finds `.jj` above the path (else `.git`) and returns a
   `Box<dyn Vcs>`. jj snapshots first unless `--no-snapshot`.
2. The log loads `log(revset, limit)` and classifies each commit against
   the seen state. A `notify` watcher on git's `HEAD`, `packed-refs` and
   `refs/`, or jj's `op_heads/heads`, triggers a reload. For git's working
   copy a second watcher covers the tree, skipping the git dir and ignored
   untracked paths.
3. Opening a commit runs `show(commit, context)` on a loader thread
   (`git show -p`, `jj diff --git`), parsed into files, hunks and lines with
   old and new line numbers. Rows render at once; syntax colours follow as
   a second event.
4. A comment freezes its side, lines and excerpt when `c` is pressed and
   goes into the batch through `Review::add`.
5. `<leader>rr` re-reads the batch, exports it and hands it back; the sent
   comments leave the batch and are marked sent in their sessions.

## Storage

| What | Where |
|---|---|
| review batch | `<cache>/reviews/<16 hex of blake3(root)>.json`, plus a `.lock` |
| seen state | `<cache>/seen/<16 hex of blake3(root)>.json` |
| sessions | `<state>/sessions/<16 hex of blake3(root)>/<commit id>.json`, `working-copy.json` |

`<cache>` is `$DEBRIEF_CACHE_DIR`, else the user cache dir's `debrief`;
`<state>` is `$DEBRIEF_STATE_DIR`, else `$XDG_STATE_HOME/debrief` on
Linux, else `~/.local/state/debrief`.

- **The batch** is shared with ramble. Every mutation takes an exclusive
  `flock` on the `.lock`, re-reads the file, applies the change and writes
  by rename, so two processes can't lose each other's comments. The lock
  file also holds the last id issued, so ids are never reused for a root
  even after the batch is cleared. Opening never creates a file.
- **The root** is the canonicalised repo root (`Vcs::root()`); ramble uses
  the nearest `.jj` or `.git` above the file. They differ only for a git
  repo nested inside another, where debrief refuses to hand `.md` files to
  ramble.
- **Seen state** holds seen commit IDs and, for jj, change IDs, written by
  rename; one writer is assumed. A commit is new if neither is seen, and
  updated (jj) if its change is seen but this commit isn't.
- **Sessions** are written by tmp + rename. Each holds a version, the
  root, commit, change and summary, the save time, its comments (each
  with a sent time once sent), and the view. The cursor is anchored by
  (file, side, line), because hunk indices change under `T`, with the row
  and column as a fallback. A file of another version, root or shape is
  an error for that file only.

## Read-only VCS access

- **git** runs every command with `--no-optional-locks` and config
  overrides that fix the diff format and turn off fsmonitor, signatures,
  colour and automatic maintenance. Commands that print a diff add
  `--no-textconv`, `--no-ext-diff` and `--no-color`. The working copy is
  read without refreshing the index; untracked files are read directly (binary
  when a NUL is in the first 8 KiB, over 1 MiB a placeholder, symlinks
  never followed). git itself may touch split-index `sharedindex.*` mtimes
  on a read, with contents unchanged; that is accepted.
- **jj** reads with `--ignore-working-copy`, pinned to one operation with
  `--at-op`. To pick the pin, debrief reads the operation heads itself:
  jj would merge divergent heads, and even `--at-op=@` takes a lock when
  they diverge, so the reads refuse divergent heads. When the heads store
  is not jj's default plain directory, the pin is `@`.
- **The snapshot exception.** jj sees uncommitted edits only after a
  snapshot, and the agents snapshot constantly anyway. So by default
  `Vcs::refresh` runs `jj util snapshot` before the log loads, `@` opens,
  a reload, and a return from a launcher, then pins reads to the head it
  leaves. The snapshot command runs without the read-only flags. It
  leaves files alone but, as jj commands that see the working copy do,
  records an operation, rewrites `@` (same change, new commit) and moves
  bookmarks on it and descendants. When a later snapshot fails, reads stay
  on the old pin and the status line says so. `--no-snapshot` turns all
  of this off.

## Threading

The event loop polls crossterm and drains one mpsc channel of
`AppEvent`s. `Vcs` handles are not `Send`, so each worker reopens the repo
from its root with the same options and jj operation pin. Workers load
commit diffs, highlighting, git's working copy and its log row, and log
stats (after the cursor rests; the most recent are cached per commit).
Every request carries a number, and an answer for a request that is no
longer current is dropped. The watchers debounce into one event. Unit
tests run without an event sender, so the same work runs inline in the
action that starts it.

## The commit view

- **One row model.** The message, file headers, hunk headers and diff
  lines form one list of rows; side by side is a second row model built
  from the same diff, with filler rows on the shorter side of a change.
  Full file is only a context setting (unlimited `-U`), so comments,
  search and `<leader>o` work the same in all four combinations.
- **Display cells.** Row text stays raw for yank, search and excerpts.
  Rendering and cursor columns go through one mapping: tabs to the next
  multiple of the tab width, a trailing `\r` hidden, other control
  characters a dimmed `?`. Without it, ratatui drops control characters
  and tab-indented code loses its indentation.
- **Highlighting per hunk.** Each hunk starts from a fresh syntect state,
  as two runs: old (context and removed) and new (context and added). A
  removed `/*` then never turns the added lines into a comment. A hunk
  that starts inside a multi-line string is coloured wrongly, which is
  accepted: per hunk is cheap and the diff shows before the colours.
  Highlighting yields foreground colours only; row backgrounds stay the
  renderer's.
- **No wrapping.** Long lines scroll horizontally by display width.
  Wrapping would break the cell-column cursor model.
- **The file list drives the diff's order.** `app/file_tree` builds the
  list from the diff's paths, as lazygit's commit files panel does: a
  tree of directories, each level directories first and then files,
  each alphabetical, emitted depth first; a chain of directories that
  hold one directory and no files is one row (`pkg/gui/controllers`),
  and files are never folded in. A directory's counts are its files'
  sums. Flat mode is the paths in git's order. The order of the list's
  files is the order the row model lays out the diff, unified and side
  by side, so `]f`, the rows and the list read the same way. Collapsed
  directories only hide list rows: each hidden entry maps to the row
  that shows it, which carries the current-file marker. `` ` ``
  rebuilds the rows in the other order and keeps the cursor's offset
  from its file header and its column, since a file's rows are the
  same in either order. The auto width fits the visible rows' names
  (indent plus name, not whole paths), so ` and opening or closing a
  directory make an open peek decide again between beside and over the
  diff.

## Comments and export

A `Comment` has a repo-relative path, an optional revision (`None` is the
working copy, as ramble uses), an optional side (diff comments), a 1-based
inclusive line range, an excerpt of at most 40 lines, the body and an
optional kind.

- Side by side, a diff comment takes the cursor's side. In unified view
  it uses the new side unless every selected line was removed. On the
  working copy, a new-side comment has no revision (on-disk line numbers,
  so ramble shows it); an old-side comment anchors to the base's old side. A renamed file's old-side
  comments use its old path.
- A comment marks gutter rows when its revision is the displayed commit.
  On the working copy, revision-less comments and old-side comments on
  its base mark rows too.
- **The comment box** is an overlay anchored beside the selected rows, so
  the code being commented on stays visible; an overlay keeps the
  renderer simple. The view scrolls to keep the rows clear; the box
  covers them only when the selection fills the pane, after the user
  scrolls behind the box, or in a pane too short for the box and a row.
  The editor is a small readline-style box adapted from tuicr's
  `text_edit.rs` (MIT, see THIRD-PARTY.md); `C-e` hands longer text to `$EDITOR`. Bracketed
  paste goes into the box with its newlines, so a paste never runs as
  keys.
- **Kinds** are free-form ids from `[review] kinds`, with definitions
  aimed at the agent; untyped is first class. The export tags each item
  `[KIND]` and adds a legend of the kinds used.
- **Export** is `# Review feedback`, a preamble (`[send] preamble`), then
  one `## Item N` per comment ordered by file and line, with its file
  (and side), lines, revision and excerpt, then the body.
- **Hand-back** pipes the markdown to `[send] command` with
  `DEBRIEF_ROOT` set. No command, a non-zero exit or a 10 s timeout falls
  back to OSC 52. mu knowledge lives only in `scripts/debrief-send`, never
  in the Rust code.

## Sessions

The batch stays the live working set, shared with ramble and sent by
`<leader>rr`. A session is written when the user saves (`<leader>ss`, or
`s` at the quit warning), and updated afterwards when a restore remaps a
comment id or a send or copy (`<leader>rr`, `y` at the quit warning)
marks its comments sent, so a review keeps its
history and is never resent. Restoring adds back unsent comments the
batch lacks, under their old ids when free. Each commit visited this run
has a dirty bit, set by user actions on it and cleared by a save or a
successful send; the quit warning reads those bits. There is no autosave:
the warning is the safety net.

A session saves the view as well as comments: context, layout, file
list show/hide, focus, the cursor (as a file-and-line anchor, plus row
and column), the file list's mode (`files`: tree or flat) and its
collapsed directories (paths from the root). Opening the commit applies
them app-wide before the diff loads, as the keys would. A session saved
before `files` and `collapsed` existed opens in `[view] files` mode with
every directory open, not in the mode the previous commit left.

## Keys and help

Dispatch is `match`-based per screen. `help::BINDINGS` describes every
binding, and a unit test resolves each row through `App::keymap`, so `g?`
and the key clue can't drift from the keymap. `keys::TOGGLES` lists each
toggle with its key and config key; a test checks every one has a key in
`config.default.toml` and a row in the [guide](guide.md#toggles)'s toggles
table, and that every `[view]` and `[keys]` key has a row.

## Testing

- **debrief-review:** ordering, export, add / edit / remove / clear, a
  two-process test that interleaves adds under the lock, and hand-back
  fallback and timeout.
- **vcs:** git and jj backends against throwaway repos created per test
  (jj tests skip when `jj` is absent), with before/after snapshots of the
  repo to prove nothing was written; the diff parser on renames, binary
  files, no newline at EOF and empty commits.
- **app:** state-machine tests drive `App` with events and no terminal:
  seen and updated across a jj rewrite, comment ranges on mixed
  selections, sessions, the file list, side by side, launchers with
  recording scripts, rendering through ratatui's `TestBackend`.
- **End to end:** the real binary in an isolated tmux server.

## Out of scope

Forges and PRs; staging or any write to a repo; word-level diff;
re-anchoring comments after rewrites; comment threads or replies; a
cross-workspace inbox; hg and Sapling.
