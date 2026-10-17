# Path filter on the log

Agreed with the user (Q1–Q9, 2026-10-11).

## Problem

`debrief PATH` only picks the repo; there is no way to review just the
history of one file or subtree. The log's `f` filter is text only (summary,
author, IDs). The user wants a path filter on the log, set by the CLI path
and added, edited and removed in the TUI, combined with the text filter
and both visible.

## Design

### Two filters, two kinds

- **Text filter** (`f`, unchanged): client-side over the loaded rows, live
  as you type.
- **Path filter** (new): part of the VCS query. Only commits touching the
  path load; `--limit` and paging count matching commits. Changing it
  reloads the log.

They combine: the path decides what loads, the text filter narrows that.

### What a path matches

- A file: commits touching that file.
- A directory: commits touching anything under it.
- No globs, no rename following (`git --follow` is single-file and git
  only; jj has no equivalent, so the two would disagree).
- Stored relative to the repo root, `/`-separated. In the TUI it is typed
  relative to the root; it need not exist on disk (deleted files have
  history).
- git: `git log … -- <path>`. jj: `jj log -r <revset> 'root:"<path>"'`
  (fileset, quoted; one argv element as today's revset). Both cover file
  and directory.

### CLI

- No `PATH`: find the root from the working directory, no path filter
  (today).
- `PATH` is the repo (workspace) root: no path filter (today).
- `PATH` is a directory or file below the root: open the repo above it as
  today, and start with the path filter set to it (canonicalized, made
  relative to the root).
- A `PATH` that does not exist still fails as today (exit 2).

### TUI: add / edit / remove

- `F` on the log (and in the commit list, as `f` is): opens a `path: …`
  prompt pre-filled with the current path. Readline-ish editing as the
  other prompts, paste as one line.
- `Enter`: apply; the log reloads with the path (page limit back to the
  start limit), the cursor stays on its commit if still listed, else the
  first row. `Enter` on an empty prompt removes the path filter.
- `Esc`: cancel the edit, keep the old path (unlike `f`'s Esc, since a
  reload costs more than retyping).
- `<leader>F`: clear both filters at once.
- `r` and file-watch reloads keep the path. Saved sessions don't store it.

### Visualising both

- Status line, log and commit screen: `path: src/app/log.rs  filter: foo
  (3/41)`, each in its own colour; the counts are of the combined result
  (rows shown / rows loaded under the path). When status space runs out,
  the text filter elides first, then the path (left-elided).
- While a path filter is set, a one-row chip tops the log (and the commit
  list): `⌕ src/app/log.rs` in a distinct colour, so a narrowed history
  can't be missed even when the status line is cut.
- `… more not loaded` notes keep working against the path-filtered load.

### Behaviour under a path filter

- **git working-copy row:** shown only when the path is dirty (tracked
  change or untracked file under it; `git status -- <path>`).
- **Commit view:** opens the whole commit as today, with the cursor on the
  first file under the path (its header row,
  as the file list and `]f` land). A saved session's cursor
  still wins.
- **Seen state:** `w` and `m` act on the loaded (matching) commits only,
  as they already act on loaded rows. A filtered first run marks the
  rows it lists and the unfiltered log's first page seen (one extra
  `log` call), so neither it nor the next unfiltered start shows old
  commits as new. (Changed from "skip the first-run marking" while
  building it: skipping would show everything as new later.)
- **Stats pane, `]]` / `[[`, commit list:** follow the filtered rows as
  they already follow `rows`.

## Changes while building and after review

Recorded so the spec matches the code (review panel, 2026-10-17):

- `Vcs::log` keeps its signature; `Vcs::log_in(revset, path, limit)` and
  `Vcs::working_status_in(path)` are added beside `log` /
  `working_status`, with defaults, so test fakes and callers without a
  path stay as they were. Paths are `&str` (repo-relative, `/`).
- The log's status line reads `path: …  0 new  filter: …`: the path
  before the counts, the text filter after them, as before.
- Typed paths: `..` segments are refused, an absolute path must be inside
  the root (made relative). A query that fails keeps the old path and
  rows. A CLI symlink inside the repo names itself, one outside names its
  target.
- After a path change the cursor goes to the first row when its commit is
  gone. If the open commit is gone it stays open and `]]` / `[[` step from
  its place in the log before the change.
- A working-copy diff (opening, `T`, `r`) never brings back a row the
  path hides.
- First-run seen marking: the filtered rows plus the unfiltered first
  page.
- jj tests cover the limit and deleted-file history too.

## Out of scope

Globs, several paths, rename following, narrowing the diff / file list to
the path, storing the path in sessions or config.

## Implementation checklist

1. `Vcs::log` takes the path (`Option<&Path>` relative to root); git and
   jj implementations; `working_status` / `working_copy` take it for the
   dirty check. VCS tests for file, directory, deleted file.
2. CLI: resolve `PATH` to (repo, optional relative path); `StartOptions`
   gains `path`. cli tests: none / root / subdir / file.
3. App state: `opts.path` beside `opts.revset`, used by `load`,
   `page_request`, `spawn_page`; a filtered fresh run marks the unfiltered log.
4. `F` prompt (`Mode::PathFilter`), Enter / empty Enter / Esc, `<leader>F`;
   keys, help, clue entries; mouse ignored while typing, as other prompts.
5. Status line labels on both screens and the chip row in the log and
   commit list (layout height minus one).
6. Commit view: initial cursor on the first file under the path.
7. Guide: Starting, The log, The commit list, Help tables.
