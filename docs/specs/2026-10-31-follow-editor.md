# Follow the editor

Agreed with the user 2026-10-31. Calls marked *(my call)* were filled in
by the agent while writing the spec. They aren't user decisions; check them.

## Problem

You read a commit in debrief, press `<leader>o`, and the TUI is suspended
while you edit. You can't see the diff and the file at the same time.
ramble solved this for markdown with `--follow`
(`../ramble/docs/specs/2026-10-22-follow-cursor.md`): it runs beside the
editor and moves to the editor's cursor. debrief should be able to do the
same for diffs, and opening an editor beside it should be a single key.

## Approach

- Move ramble's `follow_sock` into a new crate, `debrief-follow`, in this
  workspace. debrief and ramble both depend on it, so one editor hook
  reaches every following debrief and ramble in its tmux window.
- debrief gets `--follow`, `--goto FILE:LINE`, and a launcher option,
  `follow = true`. A follow launcher runs in a tmux split instead of
  suspending the TUI, and starts the listener if it isn't running.
- The default `<leader>O` is the editor as a follow launcher. `<leader>o`
  is unchanged.
- ramble makes the same change in its own spec
  (`../ramble/docs/specs/2026-10-31-follow-launcher.md`).

## Decisions

1. **Shared crate** (agreed): `crates/debrief-follow`, published along
   with `debrief-review`. It contains ramble's `follow_sock` unchanged
   (socket dir, scope, `parse_line`, `message`, `send_all`, `listen`),
   plus `split_argv` (decision 6). It's `cfg(unix)`; elsewhere `listen`
   and `send_all` return or log an "unsupported" error. It depends only
   on std *(my call)*.
2. **One socket directory** (my call; the user agreed to "shared"):
   `<base>/debrief-follow-<uid>/` instead of `ramble-<uid>/`, with the same
   base rule, mode checks and scope. Socket names gain the tool's name,
   `<scope>.<tool>.<pid>.sock`, which tells you who is listening. Senders
   ignore the tool part. An old ramble and a new debrief don't hear each
   other, which is acceptable for pre-1.0 tools.
3. **Message**: unchanged, `<line> <abs path>\n`. Each listener ignores
   paths it doesn't care about. ramble ignores non-markdown, and debrief
   ignores files outside its root or not in the open diff (decision 8).
4. **`--follow`**: debrief listens from startup. It works with `PATH`,
   `--branch` and `--revset`. A bind failure is a status message
   ("follow off: …"), as in ramble.
5. **`--goto FILE:LINE`**: the same client as `ramble --goto`, calling
   `debrief_follow::send_all`. It exits 0 even when nobody is listening,
   splits on the last `:`, and conflicts with every other flag. Both
   binaries have it, so a hook can call whichever one is installed
   *(my call)*.
6. **`follow = true` on a `[[launch]]`** (agreed): inside tmux (`$TMUX`
   and `$TMUX_PANE` set), the expanded argv runs as
   `tmux split-window -h -t $TMUX_PANE -c <root> -- <argv…>`. tmux execs
   the argv without a shell; it would hand a lone word to `sh -c`, so a
   one-word argv runs as `env <word>`, and `#` in the root is doubled
   (tmux expands formats in `-c`). A tmux still running after the pane
   runner's 200 ms is not an error: the split still opens. debrief stays on screen, doesn't suspend or
   reload (the watchers see saves), and the new pane gets focus (tmux's
   default). A split that fails to start is a status message. The
   listener starts if it isn't already running. Outside tmux, the
   launcher runs as a plain suspend-and-run and nothing listens
   *(my call)*. The split is always `-h` until someone needs a config key
   for it *(my call)*.
7. **Default** (agreed): a third built-in launcher:
   ```toml
   [[launch]]
   name = "edit-follow"
   key = "<leader>O"
   command = ["${editor}", "+${line}", "${file}"]
   follow = true
   ```
   It has the same target as `<leader>o` (`edit_target`). It merges by
   name and can be disabled like the others.
8. **What an update does** (best effort, agreed):
   - **Log screen**: ignored (agreed). `<leader>O` from the log still opens
     the editor and starts listening. Following begins once a commit is
     open.
   - **Commit view, path not a file in the diff**: ignored. The path is
     made relative to the VCS root and compared with each file's
     `new_path`. Renames match on the new path only *(my call)*.
   - **File in the diff**: the cursor goes to the row whose new-side line
     equals `line`. If no row has it (outside the hunks, or a removed-only
     stretch), it goes to the row of the closest new-side line in that
     file, and ties go to the earlier one *(my call)*. A file with no
     line rows (binary) gets its file header. A deleted file has no new
     path, so it never matches.
   - **Side by side**: if the cursor is on the old side, switch to the new
     side first, as `goto_comment` does.
   - **Folded context or merge folds**: land on the fold row and leave it
     folded *(my call)*.
   - **Older commits**: no mapping through later changes. The working
     copy's line numbers may have drifted since the commit, and the
     closest-line rule absorbs that (agreed: best effort).
   - It never sets a status, because the hook fires on every cursor move.
9. **Pauses**: the same as ramble. Updates are dropped for 2 s after a
   key, a mouse event (except bare motion) or a paste. They're also
   dropped outside `Mode::Normal`, with a key sequence or count half
   typed, or while the commit's diff is still loading. Focus on the
   files or commits pane doesn't pause *(my call)*. The cursor moves in
   the diff, and the file list selection follows it, as on any diff
   move.
10. **Landing**: if the target row is visible, only the cursor moves.
    Otherwise the view centres on it.
11. **nvim hook**: the guide's snippet calls `debrief --goto` (or
    `ramble --goto`) on `CursorMoved`, `CursorMovedI` and `BufEnter` for
    every normal file buffer (`buftype == ""`), not just markdown. It's
    debounced to 50 ms and skips unchanged positions. The same snippet is
    in both guides.

## Architecture

- `crates/debrief-follow/src/lib.rs`: the moved `follow_sock` plus
  `split_argv(pane, cwd, argv) -> Vec<String>`.
- `app::editor_follow` *(name my call: `follow` already means
  `[view] follow_working` and the view keeping up with the cursor)*:
  `editor_quiet_until`, `editor_touch(now)` (called from key, mouse and
  paste handling), and `editor_at(path, line, now)` (decisions 8–10).
  A pure `follow_row(diff, rows, rel_path, line) -> Option<usize>` holds
  the mapping, so it's unit-testable.
- `AppEvent::EditorAt(PathBuf, usize, Instant)`. `App::editor_follow_start`
  runs after `start_watcher` (`--follow`) or at the first follow launch,
  before the split, so the editor's first `--goto` can't be missed. The
  launch also clears the grace window the `O` keypress started. The
  listener is held in `App` as an `Option`, and its drop guard removes
  the socket.
- `Launcher.follow: bool`, parsed from `[[launch]] follow`. `Effect` gets
  `Split(LaunchCommand)`: `run` runs it without suspending, through the
  pane runner (null stdio, 200 ms cap, as `C-h/j/k/l`'s tmux calls), so
  tests record argv.
- `cli`: `--follow`, and `--goto` as a separate plan mode needing no TTY.

## Testing

- `debrief-follow`: ramble's existing unit and integration tests move
  with the code (a short tempdir under `/tmp` because of the 104-byte
  socket path limit), plus `split_argv`.
- Unit tests for `follow_row`: an exact new-side line, a removed line, a
  line between hunks, past the end, a binary file, and a file not in the
  diff. They also cover the grace window and mode gating with injected
  `Instant`s, the log screen ignoring updates, the switch to the new side
  in split view, and landing (visible vs centred). Diffs are built in
  memory or with `tests/common` `git_repo`.
- Launch: the default `<leader>O` expands to the tmux argv with
  `$TMUX`/`$TMUX_PANE` from the injected env. Without tmux it falls back
  to `Effect::Launch`. `follow` parses from config.
- `tests/cli.rs`: `--goto` exits 0 with no listener, and conflicts are
  rejected.
- `tui_e2e`: one case: debrief `--follow` in tmux on a commit, then
  `debrief --goto` on a changed line, then check that the cursor row
  moved. It's skipped without tmux, and a skip isn't a pass.

## Checklist

1. `crates/debrief-follow`: move the code and tests from ramble, add the
   tool in the socket name, rename the directory, add `split_argv`.
2. debrief: `AppEvent::EditorAt`, `app::editor_follow` + `follow_row` + tests.
3. debrief: `--follow` / `--goto` + `tests/cli.rs`.
4. debrief: the `follow` launcher option, `Effect::Split`, the default
   `<leader>O`, help and clue entries + tests.
5. e2e case.
6. Docs: a guide section "Following your editor" (snippet, `<leader>O`,
   limits), the launcher table (`follow`), and a design note.
7. Publish `debrief-follow`, then do ramble's spec.

## Known limits

- The editor sends buffer lines, while debrief shows saved or committed
  text. For the working copy, a save brings them back in line. For older
  commits they drift (best effort).
- No reverse sync (debrief → editor) beyond `<leader>o` / `<leader>O`.
- Follow launchers need tmux. Elsewhere they behave like a plain launcher.
