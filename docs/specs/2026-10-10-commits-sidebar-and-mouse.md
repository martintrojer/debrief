# Commit list in the sidebar, and mouse support

Two features for debrief's TUI, agreed with the user (Q48–Q53, 2026-10-10).
Each is one task; the commit list lands first because mouse hit-testing
covers its rects.

## 1. Commit list under the file tree (`[view] sidebar_commits`)

**Problem.** Moving to another commit from the commit view means `]]`/`[[`
blind, or `q` back to the log. The user wants the log's commits visible
beside the diff so they can flip between commits quickly.

**Design.**

- `[view] sidebar_commits = true | false` (default `false`); `<leader>E`
  toggles it for the run (ramble's key for its sidebar split). It is a
  layout preference: saved sessions do not store it.
- On: the file-list area (same `sidebar`, `sidebar_side`, width settings
  as today) splits horizontally: the file tree on top (60%), a one-row
  rule, the commit list below (40%). When the area is under 12 rows, the
  commit list hides and the tree takes it all. Fixed ratio, no config.
- Rows: the log's state marker (new / updated / seen) and session marker,
  short id, summary, clipped to the width. Data is the log's `rows`
  (filter and git working-copy row included), so the list, `]]`/`[[` and
  the log agree; it follows reloads (`commit_reindex`).
- The open commit's row has the `palette::SURFACE` background always.
  The list's cursor is a separate highlight, shown while the list has
  focus (the tree's selected-row style). The list keeps its cursor
  visible and re-centres on the open commit when a commit opens.
- Focus: `Focus` gains `Commits`. `C-j` from the file tree moves to the
  commit list, `C-k` back; `C-h`/`C-l` between the sidebar (either half)
  and the diff as today. With the list hidden, `C-j`/`C-k` hand off to
  tmux as today. In `sidebar = "auto"`, moving into the list peeks the
  sidebar like moving into the tree.
- Keys in the commit list: `j`/`k`, `gg`/`G`, `C-d`/`C-u` move the
  cursor; `Enter` opens the commit under it via the same path as `]]`
  (`open_index`: session restore, seen marking) and keeps the focus in
  the list (an auto peek stays open while you browse); `Esc` goes to the
  diff. Enter opens rather than following the cursor, because opening
  marks commits seen and restores sessions.
- Help, clue, the guide's toggles table, config.default.toml and the
  toggle guard test cover the key and config key.

## 2. Mouse (`[mouse] enabled`)

**Problem.** ramble has mouse support; debrief only takes the wheel over
the log's stats pane, and never enables capture.

**Design.**

- `[mouse] enabled = true` (default). On: `run.rs` enables crossterm
  mouse capture at start and disables it on exit and around suspends
  (editor, ramble launch), like bracketed paste. Off: no capture (the
  stats pane wheel goes too; the guide says so).
- Hit-testing: `App::hit(col, row) -> Hit` built from the same rects the
  draw uses (`log_layout`, `commit_layout`, the sidebar split of part 1,
  popups), so a click lands where the screen shows.
- Actions, each reusing an existing action:

  | Event | Effect |
  |---|---|
  | click a log row | log cursor there; double-click opens it |
  | click a file-list row | focus the list, select it; on a directory's arrow, toggle; double-click acts as Enter |
  | click a commit-list row | focus the list, cursor there; double-click opens |
  | click in the diff | focus the diff, cursor to that row (and side, side by side); closes an auto peek |
  | drag in the diff | visual line selection from the pressed row to the pointer row, scrolling at the top/bottom edge; then `c` / `y` work as with keys |
  | wheel | scrolls the pane under the pointer 3 rows (log, stats, file list, commit list, diff, picker, help) |
  | click outside an open picker / help | closes it; a click on a picker item selects it, double-click accepts |
  | any mouse event while typing (comment box, prompts) | ignored |

- Double-click: two left presses on the same cell within 400 ms
  (ramble's `MULTI_CLICK`). Shift+drag stays the terminal's selection
  (terminals do this themselves under capture).
- Out of scope: triple-click, double-click word copy, click to place the
  cursor in the comment box (textbox.rs is shared byte-identical with
  ramble), drag-resizing the sidebar.

## Constraints (both tasks)

- Follow the existing patterns: config keys through `set()`, toggles in
  the guide's table (the config test enforces it), help rows, clue.
- Tests drive `App` with synthesised events, as the key tests do; no
  test reads the user's real config or state dirs.
- Read-only rule unchanged: debrief never writes to the repo.
- Comments describe the code, no spec or question ids.
