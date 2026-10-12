# debrief guide

What debrief does and how to drive it. For in-app key help, see
[Help](#help). Every config option, with its default, is in
[config.default.toml](../crates/debrief/config.default.toml).

## Starting

```sh
debrief                    # the repo holding the working directory
debrief path/to/repo       # or any path inside a repo
debrief --revset 'main..'  # jj revset, or git revision range ([log] revset)
debrief --limit 500        # commits to load at first and per page ([log] limit, default 200)
debrief --no-snapshot      # jj: don't snapshot; @ as of the last jj command
debrief --init-config      # write ~/.config/debrief/config.toml
```

jj is used when a `.jj` is found above the path, else git. Both list the
current commit and its ancestors by default: jj `::@` (not jj's own
`revsets.log`), git `HEAD`. `[log] revset` and `[log] limit` in the
[config file](#config) change the defaults; `--revset` and `--limit`
override them.

debrief keeps seen state and the review batch in the user cache dir
(`--cache-dir` or `$DEBRIEF_CACHE_DIR` moves it), and saved sessions in
`~/.local/state/debrief` (on Linux `$XDG_STATE_HOME/debrief` when set;
`--state-dir` or `$DEBRIEF_STATE_DIR` moves it, used as is).

## The log

The log lists the newest commits first. `●` marks a new commit, `◐` an
updated one (jj: you saw the change, but it has been rewritten since). The
cursor starts on the oldest unseen commit. The first run on a repo marks
everything seen. Opening a commit marks it seen, and `m` marks the whole
list seen. `w` sets the watermark at the cursor: that commit and every
loaded commit older than it become seen, every newer one new (`●`)
again, so the next start lands just above it. It counts the whole loaded
log, not just the filtered rows; on git's working-copy row it marks
everything seen. The log reloads when the repo changes.

The log loads `--limit` commits (`[log] limit`, 200 by default). While
more may exist, a dim `… more` row ends the log. Moving past the last row
(`j`, `G` without a count, `C-d`, or the wheel once the `… more` row is on
screen) loads the next page in the background, with `loading more…` on
the status line. The cursor stays on its commit. A reload keeps however
many have loaded. The commit list pages the same way.

| Key | Does |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | move (with counts) |
| `Enter` | open the commit |
| `m` | mark every listed commit seen |
| `w` | watermark here: this commit and older seen, newer ones new |
| `r` | reload (jj: snapshot first) |
| `<leader>e` | show / hide the stats pane |
| `J` `K` | scroll the stats pane |
| `/` `?` `n` `N` | search |
| `f` | filter; `Enter` keeps it, `Esc` clears it |
| `<leader>o` | `$EDITOR` at the commit's first changed file still in the working copy, first hunk |
| `<leader>rl` `<leader>rr` | list / send the review |
| `<leader>rs` | list the saved review sessions |
| `q` | quit (asks first when there is unsaved review work) |

The stats pane shows the commit under the cursor like `git log --stat`:
its ID (and jj change ID), author and date, the message (cut at half the
pane when there are files; `J` scrolls to the rest), a `path | N ++--` row
per file (renames as `old → new`, binary files as `Bin`) and a summary
line. It sits beside the log from 120 columns and below it on narrower
terminals, and loads once the cursor rests on a commit.

Search and filter match the summary, the author, the commit ID and the jj
change ID as substrings, case-insensitive unless the pattern has a
capital, over the loaded commits; neither loads more. When nothing loaded
matches and more may exist, the prompt and then the status line say
`… more not loaded`. The filter narrows the list as you type,
and the status line shows `filter: x (12/200)`. `m`, `Enter` and `]]` /
`[[` act on the filtered list, and `r` keeps the filter.

## The working copy

Uncommitted changes show in the log and take comments like any commit.

- **git:** while the tree is dirty (tracked files changed against `HEAD`,
  or untracked files that aren't ignored), the log has a
  `✎ Working copy  N files` row on top, never new or seen. It opens like a commit, headed
  `Working copy (uncommitted) on <id> <summary>`. Untracked files show as
  added; untracked files over 1 MiB, binary untracked files and special
  files get a one-line placeholder, and a symlink shows its target.
  debrief watches the tree and refreshes the row when files change.
- **jj:** `@` is the working copy, marked `✎`. Before it loads the log,
  opens `@`, reloads, or comes back from the editor or a launcher, debrief
  runs `jj util snapshot`, so `@` shows your uncommitted edits. If the
  snapshot fails (another jj holds the lock, say), the status line says so
  and debrief shows the last snapshot.
- **Comments:** a new-side comment carries no revision, only the path
  and the line numbers on disk, as ramble's comments do, so ramble shows
  it too. An old-side comment (a removed line, or side by side any line on
  the left) anchors to the old side of `HEAD` (git) or `@-` (jj); if jj's
  `@` is a merge, that is refused. Paths that resolve outside the repo through a symlinked
  directory are refused for comments and for `<leader>o`.
- **Refresh:** `r` and a return from the editor reload the working copy's
  view in place, keeping the cursor row. Any other commit's view stays as
  it was opened.

## The commit view

The message, then every file's diff in one stream, with a file list on the
left.

| Key | Does |
|---|---|
| `j` `k` `h` `l` `w` `b` `e` `0` `$` `{` `}` | move |
| `gg` `G` `C-d` `C-u` `C-f` `C-b` `C-e` `C-y` `H` `M` `L` `zz` `zt` `zb` | jump and scroll |
| `]f` `[f` / `]c` `[c` | next / previous file / hunk (full file: change) |
| `T` | hunks / full file |
| `t` | unified / side by side |
| `Tab` | side by side: move the cursor to the other side |
| `]]` `[[` | open the next / previous commit |
| `r` | on the working copy: reload its diff in place |
| `/` `?` `n` `N` | search |
| `v` `V`, then `y` | select rows, copy them as diff text (OSC 52) |
| `<leader>e` | show / hide the file list (`sidebar = "auto"`: pin a hidden one) |
| `<leader>E` | show / hide the commit list under the file list |
| `C-h` / `C-l` (or `C-w h` / `C-w l`) | move between the file list and the diff |
| `Enter` | on a `.md` file header: open it in ramble |
| `<leader>o` | `$EDITOR` at this file and line in the working copy |
| `<leader>ss` | save this commit's review session |
| `<leader>rs` | list the saved review sessions |
| `q` `Esc` | back to the log |

`T` and `t` last for the rest of the session; `[view] context` and
`[view] layout` set them at start.

**Full file** (`T`) reloads the diff with unlimited context, so every row
is still a diff line and comments, markers, `<leader>o`, search and copy
work as before. The cursor stays on its line. Each file keeps one hunk
header, and `]c` `[c` jump between runs of added and removed lines.

**Side by side** (`t`) shows old lines on the left and new lines on the
right, each with its own line numbers. Context lines appear on both sides;
the removed and added lines of a change are paired row by row, and the
shorter side is hatched. It needs a diff pane at least `[view]
split_min_width` (140) columns wide, not counting the file list: below
that `t` says so, and a terminal shrunk below it shows unified until it
grows back. The cursor is on one side (the new side to start with); `Tab`
moves it across, and each side scrolls sideways on its own. Selections,
`cc`, markers, `]r` `[r`, `K` and `<leader>o` act on the cursor's side.
Search matches both sides and lands on the side of the match.

**Highlighting** (catppuccin-mocha) colours each hunk on its own, the old
and new side apart. Files over 10k diff lines and lines over 2k bytes stay
plain. The diff shows at once and the colours follow.

**`<leader>o`** works on every row. An added or context line opens at its
line, a removed line at the new-side line where it was, a hunk header at
its first new line and a file header at line 1. Deleted files and files
missing from the working copy are refused. The line number comes from the
commit, so it can drift if the file has changed since. The TUI suspends
while the editor runs and comes back on the same row.

### The file list

`[view] sidebar` is `"always"` (shown, `<leader>e` hides it), `"never"`
(hidden, `<leader>e` shows it) or `"auto"`, a peek: hidden while you read
the diff, shown while you move toward it (`C-h` or `C-w h`; on the right
`C-l`), and hidden again when you jump to a file (`Enter`), press `Esc`
in it or move back to the diff. On an auto list `<leader>e` pins a hidden
list and hides a shown one (a peek or a pin), back to peeking. A peek goes
beside the diff when the diff keeps at least 80 columns (`split_min_width`
side by side); otherwise it draws over the diff without reflowing it.

`sidebar_side = "right"` puts the list after the diff. Below
`sidebar_auto_hide_below` (80) columns an always list hides, and comes
back when the terminal grows unless `<leader>e` hid it; an auto list
still peeks and pins there, drawn over the diff. `sidebar_width = "auto"`
fits most rows within `sidebar_min_width` (16) and `sidebar_max_width`
(48), and at most 35% of the terminal.

The list is a tree, as in lazygit: files grouped by directory, directories
first, each alphabetical, and a chain of directories holding only one
directory shown as one row (`src/app/ui`). A directory row is `▾ name`
(open) or `▸ name` (closed) with the summed `+n −m` of its files; a file
row is its coloured status letter (`A` `M` `D` `R` `C`), its name and its counts. Every
directory starts open. The diff lists the files in the tree's order, so
`]f` `[f` and the list agree. `▎` marks the file holding the diff cursor,
or the closed directory hiding it. Names too long for the list are cut in
the middle, keeping the extension. `` ` `` switches to flat paths in git's
order (and the diff with it); `[view] files` sets the start. A saved
session keeps tree or flat and the closed directories.

| Key in the list | Does |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | move over the rows (jumps the diff) |
| `Enter` | on a file: jump to it; on a directory: open / close |
| `h` | close the directory, or go to its parent |
| `l` | open the directory; on a file: jump to it |
| `-` / `=` | close / open every directory |
| `` ` `` | tree / flat |
| `Esc` | back to the diff |

### The commit list

`[view] sidebar_commits = true` (or `<leader>E`, for the run) splits the
file list's area: the tree on top (60%), a rule, and the log's commits
below, so you can flip between commits without going back to the log.
It uses the same side and width as the file list, and hides when the
area is under 12 rows. A row is the log's marker (`●` new, `◐` updated,
`✎` the working copy), the short ID, the session marker and the
summary, cut at the list's edge. The rows are the log's: a filter
narrows them, git's working-copy row is there, and a reload keeps them
in step, so the list, `]]` / `[[` and the log agree.

The open commit's row has the cursor-row background. While the list has
the keys, its own cursor is bold on that background; it starts on the
open commit and comes back to it, centred, when a commit opens. `C-j`
from the tree moves into the list and `C-k` back; `C-h` / `C-l` go
between either half and the diff. With `sidebar = "auto"` a move into
the list peeks the file list, and the peek stays open while you open
commits from it. While a commit loads, or when it has no files, the
tree half is empty and the list stays. Saved sessions don't store the
setting; a session saved from the list restores focus to the tree.

| Key in the commit list | Does |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | move the cursor (opens nothing, marks nothing seen) |
| `Enter` | open the commit under it (as `]]`: marks it seen, restores its session); the list keeps the keys |
| `w` | watermark at the cursor, as on the log |
| `/` `?` `n` `N` | search the rows, as on the log (moves the cursor, opens nothing) |
| `f` | the log's filter; `Enter` keeps it, `Esc` clears it |
| `Esc` | back to the diff |
| `q` | back to the log |

Search and filter in the list are the log's: the same matching (summary,
author, commit and change ID), the same pattern and the same filter, over
the loaded commits. A search highlights its matches as you type and
moves the list's cursor on `Enter`; the diff's own search is still `/`
with the diff focused. The filter narrows the list and the log together,
keeping the list's cursor and the open commit on their rows; a kept
filter shows on the commit screen's status line as on the log's. If it hides
the open commit, the diff stays open, no row is marked as open, and `]]`
/ `[[` go to the nearest shown commit after / before it in the log. A
miss while more history exists says `… more not loaded`.

## Panes and tmux

`C-h` / `C-j` / `C-k` / `C-l` move to the pane left / below / above /
right, as in nvim. On the commit screen the panes are the file list and
the diff, and side by side the diff's two halves: file list | old | new
(on the right: old | new | file list). `C-w h` / `C-w l` do the same. The
log's stats pane never takes the keys.

With the commit list shown, `C-j` / `C-k` move between the tree and the
list. Where there is no pane that way (the screen edge, `C-j` / `C-k`
past the list or without it, the log) and debrief runs inside tmux, the move goes on to the tmux pane there, as
[vim-tmux-navigator](https://github.com/christoomey/vim-tmux-navigator)
does. In the comment box the keys stay text input: `C-j` is a new line.
`[keys] pane_nav = false` unbinds them; the `C-w` keys stay.

vim-tmux-navigator only forwards these keys to panes it thinks run vim.
Add debrief to its pattern in `~/.tmux.conf`, before TPM's `run` line:

```tmux
set -g @vim_navigator_pattern '(\S+/)?g?\.?(view|l?n?vim?x?|fzf|debrief|ramble)(diff)?(-wrapped)?'
```

## Mouse

With `[mouse] enabled = true` (the default) debrief captures the mouse:

| Mouse | Does |
|---|---|
| click a log row | the cursor there; double-click opens it |
| click a file-list row | focus the list and select it (the diff jumps to it); on a directory's arrow, open / close it; double-click acts as `Enter` |
| click a commit-list row | focus the list, its cursor there; double-click opens the commit |
| click in the diff | focus the diff, the cursor to that row and column (and side, side by side); closes an auto peek |
| drag in the diff | select whole lines from the pressed row to the pointer, scrolling at the top and bottom edge; then `c` / `y` as in visual mode |
| wheel | scroll the pane under the pointer 3 rows: the log, the stats pane, the file list, the commit list, the diff, help, the `K` popup |
| wheel over a picker | move its selection a row a notch |
| click a picker row | select it; double-click picks it, as `Enter` |
| click outside a picker, help or the `K` popup | close it (the click does nothing else) |

A double-click is two presses on the same cell within 400 ms. The wheel
moves the log's, the diff's and the commit list's cursor only to keep it
on screen (the commit list opens nothing). Over the file list it scrolls
the list and leaves the selection and the diff alone, even with the
selection scrolled out of view; a key in the list shows it again. Neither
list's wheel moves the diff. While you type (the comment box, a `/` `?`
search, the log's `f` filter, the help filter) the mouse does nothing;
the picker takes it even with filter text typed.

Under capture the terminal's own selection still works with Shift held
(Shift+drag in most terminals). `[mouse] enabled = false` turns capture
off: the terminal keeps its selection and scrolling, and the wheel no
longer scrolls the log's stats pane.

## Comments

| Key | Does |
|---|---|
| `cc` | comment on the diff line under the cursor |
| `v`/`V` … `c` | comment on the selected diff lines (one file) |
| box: `Enter` / `Esc` / `C-e` | save / cancel / finish in `$EDITOR` |
| box: `C-j` or `Alt-Enter` | new line |
| box: `Tab` / `S-Tab` | next / previous kind, back round to untyped |
| box: `PageUp` / `PageDown` | scroll the diff behind the box |
| `]r` `[r` | next / previous commented line (`●` in the gutter) |
| `K` | show the comments on this line; `j` `k` `C-d` `C-u` `gg` `G` scroll, `q` `K` `Esc` close |
| `<leader>rl` | list the batch: type to filter, `Enter` go, `C-d` remove, `C-e` edit |
| `<leader>rr` | send the batch |

The comment box opens directly below the commented lines, or above them
when there's no room; if neither side has room the diff scrolls so the
box doesn't cover them. It can still cover them when the selection fills
the pane, after you scroll the diff behind the box, or in a pane too
short for the box and a row. Its title shows the kind and the lines
(`comment [ISSUE] L12-18`). It grows with the text up to 8 rows, then
scrolls; readline keys and Up/Down edit it.

**Kinds.** A new comment starts untyped. `Tab` steps through
`[review] kinds` (by default `issue`, `suggestion`, `question`, `nit`) and
back to untyped. The sent review tags each item (`## Item 1 [ISSUE]`) and
adds a legend of the kinds it uses, with their definitions.

**Anchoring.** A comment uses the new-side line numbers of the selected
lines, or the old side if you select only removed lines; a mixed selection
uses the new side and puts both in the excerpt. Side by side, it uses the
cursor's side. It stores the first 40 selected lines as its excerpt, and a
comment on a commit also stores that commit (and jj change ID); a
working-copy comment is anchored as described under [The working
copy](#the-working-copy). Comments are never re-anchored: after a jj
rewrite a comment shows in `<leader>rl` but not in the new commit's
gutter. `Enter` in the list opens a comment's own commit at its line, if
that commit is in the log.

**Pasting** types into the comment box with its newlines, or into a
search prompt or filter as one line. A paste anywhere else is ignored,
never run as keys.

## Review sessions

A session is the saved state of one review of one commit: its comments and
the view. There is one per repo root and commit, in
`<state dir>/sessions/<16 hex of blake3(root)>/<commit id>.json`; the
working copy's is `working-copy.json`. Nothing is saved automatically.

- **Save:** `<leader>ss` in the commit view saves the batch's comments on
  that commit (for the working copy: its comments without a revision and
  removed-line comments on its base) and the view: context, layout, file
  list, focused pane and cursor. Saving again replaces it.
- **Indicator:** log rows of commits with a session show a bookmark glyph
  (Nerd Font `nf-fa-bookmark`, U+F02E) before the summary, led by the
  number of unsent comments when there are any; git's working-copy row
  ends with it.
- **Restore:** opening a commit with a session puts its view back. The
  context, layout, file list and tree or flat mode apply app-wide, as if
  you pressed the keys, so later commits without a session open the same
  way. The first time this run, its unsent comments that the batch lacks
  are added back. Opening the same commit again in this run shows the view
  you left. If the saved cursor's file is gone, the cursor starts on the
  first row.
- **Sent comments:** comments debrief sends or copies stay in their
  session marked sent and are never restored. debrief doesn't see comments
  sent from ramble, so those may come back.
- **Quit warning:** `q` on the log or `C-c` anywhere with unsaved review
  work shows `Unsaved review: s save · y copy · q quit · Esc cancel`. `s`
  saves every such session and quits, `y` copies the batch to the
  clipboard and quits, `q` or a second `C-c` quits, `Esc` stays. Unsaved
  work is a commit you changed this run (cursor, `T`, `t`, `<leader>e`,
  comments) since it was last saved or sent, if it has a session or
  comments, or a commit visited this run with comments from an earlier
  run and no session.
- **List:** `<leader>rs` lists the repo's sessions, newest first, also
  for commits no longer in the log (a jj rewrite is a new commit with no
  session). `Enter` opens the commit (`Commit not in the log` if it's
  gone) and `C-d` deletes the session. A session file that can't be read
  is skipped and reported once.

## Sending a review

Comments collect in one batch per repo root, in the cache dir.
`<leader>rr` exports the batch as markdown and pipes it to `[send]
command`, with `DEBRIEF_ROOT=<repo root>` in its environment. If no
command is set, or it fails or takes over 10 s, the markdown goes to the
clipboard (OSC 52). Once the review is delivered and marked sent in its
sessions, the exported comments leave the batch; comments added during
the send stay. If the clipboard write, the session update or the removal
fails, the batch keeps the comments and the status line says why.

`scripts/debrief-send` routes the review to the mu agent working in that
repo: it finds the one workspace in `mu workspace list --all --json` whose
path is `$DEBRIEF_ROOT` and runs `mu agent send <agent> -w <workstream> -`.
It exits 1 when no workspace or several match, or when `mu` or `jq` is
missing, so the review falls back to the clipboard. Put it on your `PATH`,
or give its full path:

```toml
[send]
command = ["debrief-send"]
```

## Sharing comments with ramble

ramble and debrief read and write the same batch file for a repo root,
under a lock, so you can comment on a plan in ramble and on its commit in
debrief, then send both from either tool. ramble shows its working-copy
comments in its gutter and lists debrief's diff comments in its
`<leader>rl`; debrief lists ramble's, and `Enter` on one in an existing
`.md` file opens it in ramble (on any other working-copy comment it shows
the path and line).

Both tools must use the same cache dir: if you move it, set
`DEBRIEF_CACHE_DIR` for both (`--cache-dir` only changes debrief).

ramble files comments under the nearest ancestor with `.jj` or `.git`,
debrief under the repo it opened. They differ only for a git repo nested
in another, and there debrief refuses to hand those `.md` files to ramble
("File belongs to a different review root").

## Config

`~/.config/debrief/config.toml` (`$XDG_CONFIG_HOME` is respected,
`--config PATH` overrides it). Every key is optional. `debrief
--init-config` writes
[config.default.toml](../crates/debrief/config.default.toml) there, every
option commented out, and refuses to overwrite a file. An unknown key or a
bad value stops debrief at start (exit 2) with one line naming the file,
line, key and what it allows. Two exceptions start anyway with a warning
on the status line: a launcher with a bad `key` is left unbound, and an
empty or repeated `[review] kinds` id is dropped.

The sections are `[view]` (what shows at start), `[render]`
(highlighting), `[keys]`, `[mouse]`, `[log]` (which commits load),
`[review]` (comment kinds), `[send]`
(where `<leader>rr` goes) and `[[launch]]`.

### Toggles

Each toggle's config key sets its state at start; the key flips it for the
session. The rows without a key are set only in the file.

| Toggle | Key | Config key | Default |
|---|---|---|---|
| file list on the commit screen | `<leader>e` | `view.sidebar` | `"always"` (or `"never"`, `"auto"`) |
| the file list's screen edge | — | `view.sidebar_side` | `"left"` (or `"right"`) |
| hide an always file list below this width (0: never) | — | `view.sidebar_auto_hide_below` | `80` |
| the file list's width | — | `view.sidebar_width` | `"auto"` (or columns) |
| narrowest auto file list | — | `view.sidebar_min_width` | `16` |
| widest auto file list (also ≤ 35%) | — | `view.sidebar_max_width` | `48` |
| commit list under the file list | `<leader>E` | `view.sidebar_commits` | `false` |
| `C-h/j/k/l` move between panes (and tmux panes) | — | `keys.pane_nav` | `true` |
| stats pane on the log | `<leader>e` | `view.stats` | `true` |
| hunks / the full file | `T` | `view.context` | `"hunks"` (or `"full"`) |
| unified / side by side | `t` | `view.layout` | `"unified"` (or `"split"`) |
| the file list as a tree / flat paths | `` ` `` | `view.files` | `"tree"` (or `"flat"`) |
| narrowest diff pane shown side by side | — | `view.split_min_width` | `140` |
| syntax highlighting | — | `render.highlight` | `true` |
| key clue after a pause in a sequence | — | `keys.clue` | `true` |
| the `<leader>` key | — | `keys.leader` | `" "` |
| mouse clicks, drags and wheel (see [Mouse](#mouse)) | — | `mouse.enabled` | `true` |
| comment kinds Tab cycles through | — | `review.kinds` | issue, suggestion, question, nit |


### Launchers

A launcher runs a command with the TUI suspended, then reloads. Arguments
are not passed through a shell. Variables: `${file}` `${line}` `${root}`
`${editor}` (`$VISUAL`, `$EDITOR` or `vi`). The defaults are `edit`
(`<leader>o`, the editor) and `md` (`Enter` on a `.md` file header,
ramble). A `key` is `<leader>` followed by plain characters. Entries
merge into the defaults by name; `disabled = true` drops one. A launcher
key wins over a built-in `<leader>` key it shares a
prefix with.

```toml
[[launch]]
name = "lazygit"
key = "<leader>g"
command = ["lazygit"]
```

## Help

- `g?` lists every key that works where you are.
- Pause in a key sequence (`g`, `<leader>`, `C-w`, …) and a box lists what
  can follow.

## Build and test

```sh
bash scripts/check.sh   # fmt, clippy -D warnings, tests
```

CI (`.github/workflows/ci.yml`) runs the same script on Ubuntu 26.04 for
every push to `main` and every pull request, with jj and tmux installed so
the jj and end-to-end tests run.

## Releases

Pushing a tag that starts with `v` runs `.github/workflows/release.yml`.
It builds `debrief` for `aarch64-apple-darwin` (macOS 15) and
`x86_64-unknown-linux-gnu` (Ubuntu 24.04, so it runs with glibc 2.39 or
newer) and attaches `debrief-<tag>-<target>.tar.gz` (binary, README,
LICENSE, THIRD-PARTY.md) and its `.sha256` to a GitHub Release named after
the tag.

```sh
git tag v0.1.0 && git push origin v0.1.0
```
