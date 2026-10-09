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

```sh
cargo install --path crates/debrief
```

You need `git` or `jj` on `PATH`. debrief only reads the repo. It never
checks out, amends, rebases or writes anything into it.

## Use

```sh
debrief                    # the repo holding the working directory
debrief path/to/repo       # or any path inside a repo
debrief --revset 'main..'  # jj revset, or git revision range (default: jj's
                           # revsets.log, git's HEAD; --limit N, default 200)
```

jj wins when a `.jj` is found above the path, else git. The log shows the
newest commits first. `●` marks a new commit and `◐` an updated one (jj:
you saw the change, but it has been rewritten since). The cursor starts on
the oldest unseen commit. The first run on a repo marks everything seen.
Opening a commit marks it seen, and `m` marks the whole list seen. Seen
state and reviews live in the user cache dir (`--cache-dir` or
`$DEBRIEF_CACHE_DIR` overrides it).

## Keys

`<leader>` is Space. `g?` lists every key that works where you are, and
after a pause in a key sequence a box shows what can follow.

**Log**

| Key | Does |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | move (with counts) |
| `Enter` | open the commit |
| `m` | mark every listed commit seen |
| `r` | reload (also automatic when the repo changes) |
| `<leader>o` | `$EDITOR` at the commit's first changed file, first hunk |
| `<leader>rl` `<leader>rr` | list / send the review |
| `q` | quit |

**Commit view**: the message, then every file's diff in one stream, with
a file list on the left (hidden below 80 columns).

| Key | Does |
|---|---|
| `j` `k` `h` `l` `w` `b` `e` `0` `$` `{` `}` | move |
| `gg` `G` `C-d` `C-u` `C-f` `C-b` `C-e` `C-y` `H` `M` `L` `zz` `zt` `zb` | jump and scroll |
| `]f` `[f` / `]c` `[c` | next / previous file / hunk |
| `]]` `[[` | open the next / previous commit |
| `/` `?` `n` `N` | search |
| `v` `V`, then `y` | select rows, copy them as diff text (OSC 52) |
| `<leader>e`, `C-w h` / `C-w l` | toggle the file list, move between panes |
| `Enter` | on a `.md` file header: open it in ramble |
| `<leader>o` | `$EDITOR` at this file and line in the working copy |
| `q` `Esc` | back to the log |

**Comments**

| Key | Does |
|---|---|
| `cc` | comment on the diff line under the cursor |
| `v`/`V` … `c` | comment on the selected diff lines (one file) |
| prompt: `Enter` / `Esc` / `C-e` | save / cancel / finish in `$EDITOR` |
| `]r` `[r` | next / previous commented line (`●` in the gutter) |
| `K` | show the comments on this line (j/k, C-d/C-u, gg/G scroll a long one; q or Esc closes) |
| `<leader>rl` | list the batch: type to filter, `Enter` go, `C-d` remove, `C-e` edit |
| `<leader>rr` | send the batch |

A comment uses the new-side line numbers of the selected lines. If you
select only removed lines it uses the old side. A selection that mixes
sides uses the new side and puts both in the excerpt. The comment stores
the commit (and jj change ID) it was made on, and the first 40 selected
lines as its excerpt. Comments are never re-anchored. After a jj rewrite,
a comment keeps its original revision. It shows in `<leader>rl` but not in
the gutter of the new commit. `Enter` in the list opens a diff comment's
own commit at its line, and only if that commit is in the log.

`<leader>o` works on every row. An added or context line opens at its
line. A removed line opens at the new-side line where it was. A hunk
header opens at its first new line, and a file header at line 1. Deleted
files and files missing from the working copy are refused. The line
number comes from the commit, so it can drift if the file has changed
since then. The TUI suspends while the editor runs and comes back on the
same row.

## Config

`~/.config/debrief/config.toml` (`$XDG_CONFIG_HOME` is respected,
`--config PATH` overrides it). Every key is optional:

```toml
[send]
command = ["debrief-send"]   # default: none, copy to the clipboard
# preamble = "..."           # replaces the paragraph under the title; "" drops it

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

## License

MIT
