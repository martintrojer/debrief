# debrief

Catch up with what the agents committed, comment on it, and send the
comments back. A terminal tool for git and jj, and `debrief-review`, the
comment crate it shares with [ramble](https://github.com/martintrojer/ramble).

When several coding agents work in their own repos, catching up meant
four tools: something to notice a new commit, lazygit to read it, tuicr to
comment on its lines, and a paste back into the agent's pane. debrief does
the middle of that loop. It shows which commits are new since you last
looked, shows each one as a single diff, takes comments on diff lines, and
hands them back as one markdown review. Markdown files go to ramble, which
adds its comments to the same batch.

Design: [docs/specs/2026-10-09-debrief.md](docs/specs/2026-10-09-debrief.md).

## Install

Prebuilt binaries for macOS (Apple silicon) and Linux (x86_64) are on the
[releases page](https://github.com/martintrojer/debrief/releases), each
with a `.sha256`. Or build it:

```sh
cargo install --path crates/debrief
```

You need `git` or `jj` on `PATH`. debrief never changes your files,
checks out, amends, rebases or writes into a git repo. In jj it
snapshots the working copy, as every jj command does (see
[Working copy](#working-copy)); `--no-snapshot` turns that off.

## Use

```sh
debrief                    # the repo holding the working directory
debrief path/to/repo       # or any path inside a repo
debrief --revset 'main..'  # jj revset, or git revision range (default: jj's
                           # revsets.log, git's HEAD; --limit N, default 200)
debrief --no-snapshot      # jj: don't snapshot; @ as of the last jj command
```

jj wins when a `.jj` is found above the path, else git. The log shows the
newest commits first. `●` marks a new commit and `◐` an updated one (jj:
you saw the change, but it has been rewritten since). The cursor starts on
the oldest unseen commit. The first run on a repo marks everything seen.
Opening a commit marks it seen, and `m` marks the whole list seen. Seen
state and reviews live in the user cache dir (`--cache-dir` or
`$DEBRIEF_CACHE_DIR` overrides it). Saved review sessions live in the
user state dir: `$XDG_STATE_HOME/debrief`, else `~/.local/state/debrief`
(`--state-dir` or `$DEBRIEF_STATE_DIR` overrides it, used as is). See
[Review sessions](#review-sessions).

## Working copy

Uncommitted changes show in the log too, and take comments like any
commit.

- **git:** while the tree is dirty (changes to tracked files against
  `HEAD`, or untracked files that aren't ignored), the log has a
  `✎ Working copy  N files` row on top and the status line says
  `working copy dirty`. The row is never new or seen. `Enter` opens it
  like a commit, headed `Working copy (uncommitted) on <id> <summary>`.
  Untracked files show as added. Files over 1 MiB, binary files and
  special files get a one-line placeholder, and a symlink shows its
  target. debrief watches the tree (skipping `.git` and ignored
  untracked paths) and refreshes the row when files change. Nothing is
  written, and the index isn't refreshed either. git itself may touch
  the timestamps of split-index files when it reads.
- **jj:** `@` is the working copy, marked `✎`. Before it loads the log,
  opens `@`, reloads (`r`) or comes back from the editor or a launcher,
  debrief runs `jj util snapshot`, so `@` shows your uncommitted edits.
  The snapshot leaves your files alone, but like any jj command it
  records an operation and rewrites `@` (same change, new commit ID).
  Anything on top of `@` is rebased and bookmarks on it move. If the
  snapshot fails (say, another jj holds the lock), the status line says
  so and debrief shows the last snapshot. `--no-snapshot` skips it:
  debrief then writes nothing, and `@` is as of the last jj command.
- **Comments:** on an added or context line the comment carries no
  revision, just the file's path and its line numbers on disk, as
  ramble's comments do, so ramble shows it in its gutter. On a removed
  line it anchors to `HEAD` (git) or `@-` (jj) on the old side. If jj's
  `@` is a merge, removed-line comments are refused. Paths that resolve
  outside the repo (through a symlinked directory) are refused for
  comments and for `<leader>o`.
- **Refresh:** `r` and a return from the editor reload the working
  copy's view in place, keeping the cursor row. The view of any other
  commit stays as it was opened.

## Keys

`<leader>` is Space (`[keys] leader`). `g?` lists every key that works where you are, and
after a pause in a key sequence a box shows what can follow.

**Log**

| Key | Does |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | move (with counts) |
| `Enter` | open the commit |
| `m` | mark every listed commit seen |
| `r` | reload (also automatic when the repo changes; jj: snapshot first) |
| `<leader>e` | show / hide the stats pane |
| `J` `K` | scroll the stats pane (the wheel over it too, when the mouse is captured) |
| `/` `?` `n` `N` | search |
| `f` | filter; `Enter` keeps it, `Esc` clears it |
| `<leader>o` | `$EDITOR` at the commit's first changed file still in the working copy, first hunk |
| `<leader>rl` `<leader>rr` | list / send the review |
| `<leader>rs` | list the saved review sessions |
| `<leader>ss` | says to open a commit first (sessions are saved from the commit view) |
| `q` | quit (asks first when there is unsaved review work) |

The stats pane shows the commit under the cursor like `git log --stat`:
its ID (and jj change ID), author and date on one line, the full message
(cut at half the pane with `…` when there are files; `J` scrolls to the
rest), then a
`path | N ++--` row per file (renames as `old → new`, binary files as
`Bin`) and a summary line. It sits beside the log from 120 columns and
below it on narrower terminals, and loads once the cursor has rested for
a moment (cached per commit).

Search and filter match the summary, the author, the commit ID and the jj
change ID, as substrings, case-insensitive unless the pattern has a
capital. They cover the loaded commits only (`--limit`). The filter
narrows the list as you type and highlights the matches. The status line
shows `filter: x (12/200)`. `m`, `Enter` and `]]` / `[[` act on the
filtered list, and `r` keeps the filter.

**Commit view**: the message, then every file's diff in one stream, with
a file list on the left (hidden below 80 columns).

| Key | Does |
|---|---|
| `j` `k` `h` `l` `w` `b` `e` `0` `$` `{` `}` | move |
| `gg` `G` `C-d` `C-u` `C-f` `C-b` `C-e` `C-y` `H` `M` `L` `zz` `zt` `zb` | jump and scroll |
| `]f` `[f` / `]c` `[c` | next / previous file / hunk (full file: change) |
| `T` | hunks / full file (for the rest of the session) |
| `t` | unified / side by side (for the rest of the session) |
| `Tab` | side by side: move the cursor to the other side |
| `]]` `[[` | open the next / previous commit |
| `r` | on the working copy: reload its diff in place |
| `/` `?` `n` `N` | search |
| `v` `V`, then `y` | select rows, copy them as diff text (OSC 52) |
| `<leader>e`, `C-w h` / `C-w l` | toggle the file list, move between panes |
| `Enter` | on a `.md` file header: open it in ramble |
| `<leader>o` | `$EDITOR` at this file and line in the working copy |
| `<leader>ss` | save this commit's review session |
| `<leader>rs` | list the saved review sessions |
| `q` `Esc` | back to the log |

`T` shows each file whole: the diff is reloaded with unlimited context, so
every row is still a diff line and comments, markers, `<leader>o`, search
and copy work as before. The cursor stays on its line (or the nearest
shown line of its file). Each file keeps one hunk header; `]c` `[c` jump
between runs of added and removed lines instead. Highlighting then starts
at the file's first line. The start mode is `[view] context`.

`t` shows old lines on the left and new lines on the right, each with its
own line numbers. Context lines appear on both sides; the removed and
added lines of a change are paired row by row and the shorter side is
hatched. It works with `T` too. Side by side needs a diff pane at least
`[view] split_min_width` (140) columns wide, not counting the file list:
below that `t` says so, and a terminal shrunk below it shows unified until
it grows back. The cursor is on one side (the new side to start with);
`Tab` moves it across, `h` `l` `w` `b` `e` `0` `$` move within that side's
text, and each side scrolls sideways on its own. Selections, `cc`,
markers, `]r` `[r`, `K` and `<leader>o` act on the cursor's side: left is
the old side with old line numbers, right the new side. Search matches
both sides and lands on the side of the match. The start layout is
`[view] layout`.

**Comments**

| Key | Does |
|---|---|
| `cc` | comment on the diff line under the cursor |
| `v`/`V` … `c` | comment on the selected diff lines (one file) |
| box: `Enter` / `Esc` / `C-e` | save / cancel / finish in `$EDITOR` |
| box: `C-j` or `Alt-Enter` | new line |
| box: `Tab` / `S-Tab` | next / previous kind (`comment [ISSUE] L3`), back round to untyped |
| box: `PageUp` / `PageDown` | scroll the diff behind the box |
| `]r` `[r` | next / previous commented line (`●` in the gutter) |
| `K` | show the comments on this line (j/k, C-d/C-u, gg/G scroll a long one; q or Esc closes) |
| `<leader>rl` | list the batch: type to filter, `Enter` go, `C-d` remove, `C-e` edit |
| `<leader>rr` | send the batch |

`cc` and `c` open a comment box directly below the commented lines (above
them when there's no room), so the lines stay in view and stay
highlighted. The box never covers them: with no room on either side the
diff scrolls until the last selected line sits just above the box. Its title shows the kind and the lines (`comment [ISSUE]
L12-18`). It grows with the text up to 8 rows, then scrolls; the usual
readline keys and Up/Down edit it.

A new comment starts untyped. `Tab` steps through the kinds in
`[review] kinds` (by default `issue`, `suggestion`, `question`, `nit`)
and back to untyped; `S-Tab` goes the other way. The kind shows as
`[ISSUE]` in the box's title, the `<leader>rl` list, the `K` popup and the
status-line preview. The sent review tags each item (`## Item 1 [ISSUE]`)
and adds a legend of the kinds it uses, with their definitions.

Pasting (bracketed paste) types into the comment box with its newlines,
or into the `/` `?` search prompt, the log's filter or the `<leader>rl`
filter as one line (newlines and tabs become spaces). A paste anywhere
else is ignored, never run as keys.

A comment uses the new-side line numbers of the selected lines. If you
select only removed lines it uses the old side. A selection that mixes
sides uses the new side and puts both in the excerpt. The comment stores
the commit (and jj change ID) it was made on, and the first 40 selected
lines as its excerpt. Comments are never re-anchored. After a jj rewrite,
a comment keeps its original revision. It shows in `<leader>rl` but not in
the gutter of the new commit. `Enter` in the list opens a diff comment's
own commit at its line, and only if that commit is in the log.

`<leader>o` works on every row. An added or context line opens at its
line (exact on the working copy, which is the file on disk). A removed line opens at the new-side line where it was. A hunk
header opens at its first new line, and a file header at line 1. Deleted
files and files missing from the working copy are refused. The line
number comes from the commit, so it can drift if the file has changed
since then. The TUI suspends while the editor runs and comes back on the
same row.

## Review sessions

A session is the saved state of one review of one commit: its comments
and the view. There is one per repo root and commit, in
`<state dir>/sessions/<16 hex of blake3(root)>/<commit id>.json`; the
working copy has one, `working-copy.json`. Nothing is written until the
first save, and nothing is saved automatically.

- **Save:** `<leader>ss` in the commit view saves the open commit's
  session: the batch's comments on that commit (for the working copy,
  its comments without a revision and removed-line comments on its base)
  and the view (`T` context, `t` layout, the file list, the focused pane,
  and the cursor by file, side and line, plus its row and column).
  Saving again replaces it. The status line says
  `Session saved (3 comments)`.
- **Indicator:** log rows of commits with a session end in `▣`, with the
  number of unsent comments when there are any (`▣3`). Filtered and
  searched lists keep it, and so does the working-copy row.
- **Restore:** opening a commit with a session puts its view back once the
  diff loads (unified while the pane is too narrow for side by side, side
  by side again when it is wide enough). If the anchored file is gone,
  the cursor goes to the first row. The first time this run, the
  session's unsent comments that the batch lacks are added back to it,
  under their old ids when free. The status says
  `Restored session (saved 2h ago, 3 comments)`. Opening the same commit
  again in this run shows the view you left, not the saved one.
- **Sent comments:** comments debrief sends or copies (`<leader>rr`, or
  `y` when quitting) stay in their session marked sent and are never
  restored; `▣N` counts only unsent ones. debrief doesn't see comments
  sent from ramble, so those may come back when a session is restored.
- **Quit warning:** `q` on the log or `C-c` anywhere, with unsaved review
  work, shows `Unsaved review: s save · y copy · q quit · Esc cancel`.
  `s` saves every such commit's session and quits (a failed save stays
  and says why), `y` copies the batch to the clipboard (OSC 52), marks it
  sent and quits, `q` or a second `C-c` quits, `Esc` stays. Unsaved work
  is a commit visited this run that you changed since it was last saved,
  sent or copied (moved the cursor, `T`, `t`, `<leader>e`, added, edited
  or removed its comments) and that has a session or comments, or a
  commit with comments from before this run and no session (it stays
  unsaved until you save or send it). Only looking at a commit with no
  comments and no session never warns.
- **List:** `<leader>rs` lists the repo's sessions, newest first: short
  ID and summary, age, and comment count (`3 comments (2 sent)`), also for
  commits no longer in the log (a jj rewrite is a new commit with no
  session). `Enter` opens the commit and restores it (`Commit not in the
  log` if it's gone); `C-d` deletes the session. A session file that
  can't be read is skipped and reported once.

## Config

`~/.config/debrief/config.toml` (`$XDG_CONFIG_HOME` is respected,
`--config PATH` overrides it). Every key is optional.
`debrief --init-config` writes
[`crates/debrief/config.default.toml`](crates/debrief/config.default.toml)
there: every option commented out with its default and a line on what it
does. It refuses to overwrite a file that exists (exit 1). An unknown key
or a bad value stops debrief at start (exit 2) with one line on stderr
naming the file, the line, the key and what it allows, e.g.
`debrief: config error at ~/.config/debrief/config.toml line 3:
view.layout: unknown variant `side`, expected `unified` or `split``.

### Toggles

Each toggle's config key sets its state at start; the key flips it for the
session. The rows without a key are set only in the file.

| Toggle | Key | Config key | Default |
|---|---|---|---|
| file list on the commit screen | `<leader>e` | `view.sidebar` | `true` |
| stats pane on the log | `<leader>e` | `view.stats` | `true` |
| hunks / the full file | `T` | `view.context` | `"hunks"` (or `"full"`) |
| unified / side by side | `t` | `view.layout` | `"unified"` (or `"split"`) |
| narrowest diff pane shown side by side | — | `view.split_min_width` | `140` |
| syntax highlighting | — | `render.highlight` | `true` |
| key clue after a pause in a sequence | — | `keys.clue` | `true` |
| the `<leader>` key | — | `keys.leader` | `" "` |
| comment kinds Tab cycles through | — | `review.kinds` | issue, suggestion, question, nit |

debrief never turns on mouse capture, so there is no mouse toggle: the
terminal keeps its own selection and scrolling.

An example:

```toml
[send]
command = ["debrief-send"]   # default: none, copy to the clipboard
# preamble = "..."           # replaces the paragraph under the title; "" drops it

# Comment kinds Tab cycles through, in order, and the legend of the sent
# review (the same shape as ramble's). Omitted: issue, suggestion,
# question, nit. A list replaces them; `kinds = []` keeps comments
# untyped. Ids are trimmed and lowercased; empty or duplicate ids are
# dropped with a warning in the status line.
[review]
kinds = [
  { id = "issue",    definition = "Something is wrong. Fix it." },
  { id = "question", definition = "Answer in your reply." },
]

# Syntax highlighting of diff lines (catppuccin-mocha), on by default.
# Each hunk is coloured on its own, the old and new side apart; files over
# 10k diff lines and lines over 2k bytes stay plain. The diff shows at
# once and the colours follow.
[render]
highlight = true

[view]
sidebar = true               # the commit view's file list at start (<leader>e toggles)
stats = true                 # the log's stats pane at start (<leader>e toggles)
context = "hunks"            # the commit view: "hunks" or "full" (whole files; T toggles)
layout = "unified"           # the commit view: "unified" or "split" (side by side; t toggles)
split_min_width = 140        # narrowest diff pane (without the file list) shown side by side

[keys]
leader = " "                 # one character
clue = true                  # after a pause in a key sequence, show what can follow

# Launchers run with the TUI suspended, as in ramble. Arguments are not
# passed through a shell. Variables: ${file} ${line} ${root} ${editor}
# (an argument that is exactly "${editor}" expands to $VISUAL / $EDITOR /
# vi, split into words). Entries merge into the defaults by name;
# `disabled = true` drops one. A launcher key wins over a built-in
# <leader> key it shares a prefix with (<leader>rx shadows nothing,
# <leader>ex replaces <leader>e on the commit screen), as in ramble.
[[launch]]
name = "edit"                # default: <leader>o
key = "<leader>o"
command = ["${editor}", "+${line}", "${file}"]

[[launch]]
name = "md"                  # Enter on a .md file header; no key
command = ["ramble", "${file}"]

[[launch]]
name = "lazygit"
key = "<leader>g"
command = ["lazygit"]
```

## Sending a review

Comments collect in one batch per repo root, in the user cache dir.
Nothing is written into the repo. `<leader>rr` re-reads the batch,
exports it as markdown, and hands it to `[send] command`. The command
gets the markdown on stdin and `DEBRIEF_ROOT=<repo root>` in its
environment. If no command is set, or it can't start, exits non-zero, or
takes longer than 10 s, the markdown goes to the clipboard (OSC 52)
instead. Either way, the sent comments are then removed from the batch.
Comments added while the send ran stay in the batch.

`scripts/debrief-send` routes the review to the mu agent working in that
repo. It finds the one workspace in `mu workspace list --all --json`
whose path is `$DEBRIEF_ROOT` and runs
`mu agent send <agent> -w <workstream> -` with the markdown. It exits 1
if there is no matching workspace, if several match, or if `mu` or `jq`
is missing from `PATH`, so the review falls back to the clipboard. Put it
on your `PATH`, or give its full path in `command`.

## Sharing comments with ramble

ramble (comment mode is on by default, `[review] enabled`) and debrief read and write the
same batch file for a repo root, under a lock, so you can comment on a
plan in ramble and on the commit that implemented it in debrief, then
send both at once from either tool. ramble shows its working-copy
comments in its gutter and lists debrief's diff comments in its
`<leader>rl`. debrief lists ramble's comments, and `Enter` on one in a
`.md` file opens it in ramble.

Both tools must use the same cache dir. If you override it, set
`DEBRIEF_CACHE_DIR` for both (`--cache-dir` only changes debrief).

The review root is the repo root debrief opened (`Vcs::root()`). ramble
uses the nearest ancestor with `.jj` or `.git`. The two agree except for
a git repo nested inside another repo. There, ramble would file the
comments under the inner repo, so debrief refuses to hand those `.md`
files to ramble ("File belongs to a different review root").

## Build and test

```sh
bash scripts/check.sh   # fmt, clippy -D warnings, tests
```

CI (`.github/workflows/ci.yml`) runs the same script on Ubuntu 26.04 for
every push to `main` and every pull request, with jj and tmux installed so
the jj and end-to-end tests run.

## Releases

Pushing a tag that starts with `v` (`v0.1.0`, `v001`, …) runs
`.github/workflows/release.yml`: it builds `debrief` for
`aarch64-apple-darwin` (macOS 15) and `x86_64-unknown-linux-gnu`
(built on Ubuntu 24.04 LTS), and attaches `debrief-<tag>-<target>.tar.gz` (binary,
README, LICENSE, THIRD-PARTY.md) and its `.sha256` to a GitHub Release
named after the tag.

```sh
git tag v0.1.0 && git push origin v0.1.0
```

The Linux binary is built on Ubuntu 24.04 so it runs on 24.04, 26.04 and
other distributions with glibc 2.39 or newer; on older systems, build
from source. CI itself runs on Ubuntu 26.04, the latest LTS.

## License

MIT
