# What debrief's commit view could take from tuicr's diff rendering

Source: tuicr 0.27.0 (`~/hacking/tuicr`, MIT). Paths below are relative to
that repo unless prefixed `debrief:`. The target is debrief's commit view:
`debrief:crates/debrief/src/ui/commit.rs`, `app/commit.rs`, `ui/sidebar.rs`,
and `vcs/{mod,diff}.rs`.

## Summary

- tuicr gets its look from three things. It highlights syntax at parse time,
  one hunk at a time, with separate old and new sequences. It blends add and
  remove backgrounds into the theme base and paints them across the full row.
  And it renders tabs as spaces.
- tuicr has **no word-level diff and no moved-code detection**. For those it
  has nothing to copy, so the notes below sketch them from first principles
  and from delta.
- tuicr's wrap and row-height machinery (`row_height.rs`, ~1000 lines of
  parity code) is expensive. debrief's cell-column cursor model would not
  survive it. Skip it.
- debrief already does several things better than tuicr. It keeps
  `\ No newline` rows (tuicr drops them, `src/vcs/diff_parser.rs:128`). It
  shows renames as `old → new` (tuicr shows only the new path plus `[R]`). It
  scrolls horizontally by display width (tuicr's `apply_horizontal_scroll`
  counts chars, `src/ui/diff_view.rs:1117`, which breaks on wide glyphs). And
  it renders only the visible rows (tuicr pushes a placeholder `Line` for
  every off-screen row on every frame).
- **A bug in debrief turned up during this research:** tabs, carriage returns
  and other control characters do not render. `unicode-width` gives `"\t"` and
  `"\r"` a str width of 1, so `row_cells` and `visible()` count them as one
  column each. ratatui then drops every grapheme that contains a control
  character (`ratatui-core` `Span::styled_graphemes`). Checked with
  `TestBackend`: `"a\tb\rc"` renders as `abc`. So tab-indented code (Go, C,
  Makefiles) shows with no indentation, and the cursor and search columns
  drift off the visible text.
- debrief's cursor-row background stops where the text ends: `Line::style`
  only styles the graphemes that are drawn (checked with `TestBackend`). An
  added or removed background done the same way would stop there too.

---

## 1. Syntax highlighting inside diffs

**What.** Every diff line gets syntect foreground colours, and the add or
remove background goes on top of them.

**Where.**
- `src/syntax/mod.rs:146` `highlight_file_lines`, `:174`
  `highlight_lines_with`, `:231` `split_diff_lines_for_highlighting`, `:276`
  `highlighted_line_for_diff_with_background`, `:457`
  `apply_diff_background`.
- Called once per hunk from the parser: `src/vcs/diff_parser.rs:182-221`.
- Full-file pass for container grammars: `src/vcs/mod.rs:168-321`.

**How.**
- **A hunk that starts mid-file.** tuicr does not try to recover the true
  parser state. It starts a fresh `HighlightLines` at the first line of each
  hunk, so no state leaks from one hunk to the next. This goes wrong inside
  a multi-line string or block comment that opened above the hunk, and tuicr
  accepts that.
  - The exception is "container" grammars (vue, svelte, astro, mdx, php, erb,
    eex, heex; `needs_full_file_highlight`, `src/syntax/mod.rs:31`). Their
    nested languages only switch on after an opening tag, so tuicr fetches
    the whole old and new files and highlights them start to finish.
  - It then indexes the result by `old_lineno - 1` / `new_lineno - 1`
    (`apply_full_file_spans`, `src/vcs/mod.rs:301`).
  - This pass is capped at 1 MiB and skips files that contain NUL
    (`src/vcs/mod.rs:168,294`). It runs files in parallel with
    `std::thread::scope` (`:241`).
- **Old side and new side.** Each hunk is split into two runs:
  `old = context + removed` and `new = context + added`. Each run is
  highlighted on its own. Removed lines take their spans from the old run;
  added and context lines take theirs from the new run. A removed `/*` can
  therefore never colour the added lines as a comment.
  ```rust
  match origin {
      Context  => { old.push(c); new.push(c); /* index both */ }
      Addition => { new.push(c); }
      Deletion => { old.push(c); }
  }
  ```
- **Choosing the syntax** (`get_syntax`, `src/syntax/mod.rs:414`). tuicr
  tries, in order: the extension, then the lowercased extension, then a
  fallback table (jsx/mjs → js, jsonc → json, ...; `:391`), then the file
  name as token and as name (Makefile, BUILD), then a fallback by file name
  (Justfile → sh), and finally the shebang of the first line.
- **Details worth keeping:**
  - syntect's trailing `\n` is stripped from the last span (`:195`).
    Otherwise ratatui gets an extra cell.
  - The Oniguruma backtracking limit is capped at 100k (`:54`). This only
    matters with the onig backend. ramble, and so debrief, would use
    `fancy-regex` (`default-fancy`), which has its own backtrack limit.
  - Base16 "ANSI slot" placeholder colours are mapped to `Color::Black..White`
    (`:364`). Not needed for Catppuccin.
- **Backgrounds.**
  - Highlighted add/remove lines use `syntax_add_bg` / `syntax_del_bg`.
  - Lines with no highlighting use the slightly stronger `diff_add_bg` /
    `diff_del_bg`, with green or red foreground.
  - Context lines get no background.
  - The background is applied again at render time from the live theme
    (`src/ui/styles.rs:35-50`) so that a theme switch does not leave old
    colours baked in. debrief has a single fixed theme and does not need this.

**Cost.**
- Dependencies: syntect plus two-face. ramble already ships both
  (`ramble:Cargo.toml:22-23`, `default-fancy`).
- Work is eager: every line of every file, at load. tuicr's own measurement
  of a 4,000-line diff was 197 ms highlighted against 3.1 ms parse-only
  (`src/syntax/mod.rs:121-127`). That is roughly 50 µs per line.
- Memory: tuicr stores `Vec<(Style, String)>` per line, which duplicates the
  text.
- tuicr filters `.tuicrignore` *before* highlighting so that minified bundles
  never pay the cost (`src/tuicrignore.rs:23`).

**Fit for debrief.**
- Highlight in the worker that already runs `show()` (`spawn_show`,
  `app/commit.rs`). Do it per hunk with tuicr's old/new split.
- Store the result alongside `rows` in `CommitView`, for example
  `spans: Vec<Option<Vec<(Style, Range<usize>)>>>` with byte ranges into
  `Row.text`. That avoids copying the text, and `Row.text` stays raw for
  search, yank and comments.
- `ui/commit.rs::visible()` then looks up each grapheme's style by byte
  offset and sets its foreground. The row background, cursor, selection and
  search-hit styles stay on top as they are now.
- Copy ramble's `syntaxes()`, `code_theme()` and `syntect_style()`
  (`ramble:src/render.rs:511-540`, about 30 lines) into a new
  `debrief:app/highlight.rs`. Add tuicr's path-based `get_syntax` chain. The
  spec rules out a shared crate for v1, so copy rather than share.
  catppuccin-mocha already matches debrief's `palette`.
- Large diffs:
  - Send the diff first and the spans in a second request-tagged
    `AppEvent`, so a big commit still opens instantly.
  - Skip highlighting for any file over about 1 MiB of patch or about 5k
    lines, for lines over about 2,000 bytes (minified code), and for
    lockfiles.
- Update the spec's "Out of scope (v1)" list.

**Recommendation: ADOPT NOW** (after §5). It is the largest visible gain;
the dependency and theme are already proven in ramble, and the per-hunk
approach is simple and good enough.

## 2. Word-level / intra-line change highlighting

**What.** tuicr does none. There is no `similar`, `imara-diff` or
`dissimilar` in `Cargo.lock`, and no word-diff code anywhere.

**Where to start anyway.** Side-by-side mode already pairs each run of
removed lines with the run of added lines that follows it, matching them by
position (`render_deletion_addition_pair_side_by_side`,
`src/ui/diff_side_by_side.rs:1539-1570`). Those pairs are exactly the
candidates for a word diff.

**How (design, not from tuicr).**
1. For each removed run followed by an added run, pair the lines by index.
2. Tokenise each line into words, whitespace and punctuation. debrief's
   `motion::Class` already sorts cells into Word, Blank and Punct.
3. Diff the tokens with Myers or LCS (the `similar` crate,
   `TextDiff::from_slices`).
4. Skip a pair when its normalised edit distance is above about 0.6 (delta's
   default `--max-line-distance`,
   https://manpages.debian.org/testing/git-delta/delta.1.en.html), or when
   either line is over about 500 columns.
5. Paint the changed tokens with a stronger blend, about 35% instead of 16%.

delta has found that a short shared suffix can push the distance over the
threshold. For equal-length runs it now pairs naively at a threshold of 1.0
(https://github.com/dandavison/delta/pull/2245).

**Cost.** One small dependency (or about 60 lines of LCS). Work is
O(n·m) per token pair, run only over changed-line pairs, so it is cheap if
done in the worker.

**Fit.** It produces a second set of ranges per Line row and changes only
the background in `visible()`. It shares the span plumbing from §1.

**Recommendation: LATER.** It is valuable but builds on the span and
background layering from §1 and §5; do it right after them.

## 3. Tabs, wide characters, control characters, CRLF

**What tuicr does.**
- **Tabs.** Every tab becomes 4 spaces at parse time, in `tabify`
  (`src/vcs/mod.rs:70`), called at `src/vcs/diff_parser.rs:182`,
  `src/vcs/git/diff.rs:409`, `src/vcs/git/cli.rs:830` and in expanded
  context (`src/vcs/mod.rs:89`). This is a flat replacement, not tab stops.
  Highlighting runs after tabify, so the span widths match what is drawn.
- **Wide characters.** Padding and truncation use `unicode-width`
  (`truncate_or_pad_spans`, `src/ui/text_utils.rs:69`). A wide glyph that
  would overflow the edge is dropped. Panning in the side-by-side view is
  also width-aware (`pan_spans`, `src/ui/diff_side_by_side.rs:159`). The
  unified view's `apply_horizontal_scroll` is not.
- **CRLF.** Trailing `\r` is stripped silently (`trim_end_matches(['\n','\r'])`,
  and `str::lines()` in the patch parser). Nothing tells you a file uses
  CRLF.
- **Other control characters.** tuicr does nothing; ratatui drops them.

**debrief today.**
- Tabs, `\r`, ESC and other control characters each take one column in
  `row_cells` and `visible()`, but render as nothing.
- The visible effects: indentation disappears, text shifts left by one
  column per tab, the reverse-video cursor sits on the wrong screen cell, a
  CRLF line has an invisible last cell, and lines end before `text_w`.
- ramble already handles this. It turns a tab into 4 space cells and any
  control grapheme into `?` (`ramble:src/render.rs:306,831`).

**Fit.**
- Keep `Row.text` raw so yank, search and comment excerpts stay exact.
- Add one display rule and use it in all three places that map text to
  columns: `motion::row_cells`, `ui/commit.rs::visible()` and
  `search::col_of` (which currently does `text[..b].width()`).
- The rule:
  - A tab expands to the next multiple of 4 from the start of the text, or
    to a flat 4 to match tuicr and ramble.
  - A trailing `\r` is not drawn, or is drawn as a dim `␍`.
  - Any other control character is drawn as `?` or as caret notation (`^[`)
    in the overlay colour.

**Recommendation: ADOPT NOW.** It is a correctness bug and a small change
(about 40 lines plus tests).

## 4. Long lines: wrap or truncate, row heights

**What tuicr does.**
- `wrap = false` is the default (`docs/CONFIG.md:109`). With it, every
  logical line is one row, scrolled horizontally by `scroll_x`, which is
  clamped to the widest *visible* line (`src/ui/diff_unified.rs:1224-1278`).
- With `:set wrap!`:
  - `wrap_spans` (`src/ui/text_utils.rs:373`) word-wraps styled spans while
    keeping their styles, splitting oversized words by width.
  - Every overlay pass (row backgrounds, cursor line, comment bars, header
    fill) then walks a `row_heights` vector (`src/ui/diff_view.rs:700`).
  - `src/ui/row_height.rs` (1041 lines) rebuilds each row's text without
    rendering so that jumps such as `G` and `zz` can add up heights. Parity
    tests (`:805-940`) keep it in step with the renderer.

**Cost.** High. Two code paths must agree, every overlay needs to know about
wrapping, and side-by-side wrapping needs a separate max-of-both-sides
height.

**Fit.** debrief's cursor is a (row, display column) pair. Visual mode,
`w`/`b` motions, search hits and comment anchors all assume one row per
line. Wrapping would touch all of them.

**Recommendation: SKIP wrapping.** debrief's horizontal scroll that follows
the cursor is enough. A cheap idea that is not from tuicr: draw a dim `›` in
the last column when a row continues past the right edge, and `‹` when
`hscroll > 0` (LATER, about 10 lines in `ui/commit.rs`).

## 5. Line-number gutter, +/- markers, row backgrounds

**What tuicr does.**
- **Unified gutter** (`unified_line_number_field`, `src/ui/diff_view.rs:69`;
  `unified_gutter`, `src/app/mod.rs:202`):
  - A one-cell `▶` cursor indicator, then *one* number column.
  - The number is the new side for added and context lines and the old side
    for removed lines.
  - Then a `▌` marker coloured green or red, and a space. Context lines get
    a blank marker (`:108`).
  - The width is `max(4, digits of the largest line number)`, the same rule
    as debrief's `MIN_NUM_W`.
  - Relative numbers are optional.
- **Full-row backgrounds.**
  - The add or remove background is painted on the buffer for the whole row
    width *before* the `Paragraph` is drawn (`paint_unified_diff_rows_with`,
    `src/ui/diff_view.rs:682`; `unified_line_bg_style`, `:625`). An empty
    added line therefore still shows a green band.
  - A zero-width end-of-line marker span carries the background colour
    (`src/ui/diff_unified.rs:662-694`).
  - The cursor-line background is painted *after* the paragraph so that it
    wins over the explicit span backgrounds. It leaves search-hit cells alone
    (`:646`).
- **Hunk headers.** The row is tinted with `section_highlight_bg` (the panel
  background lightened or darkened by 18; `src/theme/mod.rs:2516-2534`) with
  a dim foreground (`src/ui/styles.rs:56`). This makes hunk boundaries easy
  to spot.

**Fit for debrief.**
- Keep the two number columns. Old-side comment anchors and the "removed
  line → new-side line" editor jump make both numbers useful. The `▌`
  marker is a matter of taste (SKIP).
- *Do* adopt full-row backgrounds:
  - In `ui/commit.rs::draw`, call `frame.buffer_mut().set_style(row_rect, bg)`
    for each visible row before rendering the `Paragraph`. Alternatively, pad
    each `Line` with styled spaces up to `text_w`.
  - Do this for added rows (green blend), removed rows (red blend), hunk
    headers (surface tint) and the cursor row (`SURFACE`, which today stops
    at the end of the text).
  - Colour the `±` sign green or red instead of overlay grey.
- Add the background colours to `palette` (`ui/mod.rs`) using tuicr's
  `blend` (`src/theme/mod.rs:1377`) on Mocha base `#1e1e2e`:

  | blend | add       | del       | use                     |
  |-------|-----------|-----------|-------------------------|
  | 16%   | `#333d40` | `#402f41` | under syntax colours    |
  | 20%   | `#394545` | `#483346` | plain text, no syntax   |

**Recommendation: ADOPT NOW.** It is small (about 40 lines) and §1 builds on
it.

## 6. Colours and styles; light and dark terminals

**What tuicr does.**
- A `Theme` struct holds about 45 colours (`src/theme/mod.rs:26`), and there
  are 24 built-in themes. Catppuccin flavours are generated from a palette
  struct using blends (`catppuccin_theme`, `:1391-1468`): `diff_add_bg` is
  20%, `syntax_add_bg` is 16%, `search_match_bg` is 30% yellow.
- Appearance can be `system`. It asks the terminal for its background colour
  over OSC 11 with `terminal-colorsaurus` (treating perceived lightness below
  0.5 as dark; `:1899`). If that fails it falls back to macOS
  `defaults read -g AppleInterfaceStyle` (`:1914`). It then picks
  `theme_dark` or `theme_light`.
- Custom TOML themes and custom `.tmTheme` files are supported.

**Fit.** debrief and ramble are Mocha-only by design.

**Recommendation: SKIP for now.** It means another dependency, a terminal
query with a timeout at startup, and a theme abstraction. Revisit together
with ramble if light-terminal users ask; tuicr's blend-from-palette approach
is the right model then.

## 7. Context expansion between hunks

**What tuicr does.**
- Before each hunk, and at end of file, there are expander rows
  (`… ↓ expand (20 lines) …`, `↑`, `↕`) and a `… N lines hidden …` row
  (`src/ui/diff_unified.rs:463-585`; `src/ui/diff_view.rs:52-66,296-329`).
- Each press reveals `GAP_EXPAND_BATCH = 20` lines (`src/app/mod.rs:37`)
  from the top or bottom of the gap.
- `expand_gap` (`src/app/gaps.rs:228`):
  - Computes the gap's new-side range from the hunk headers
    (`gap_boundaries`, `:176`; `calculate_gap`,
    `src/vcs/git/context.rs:84`).
  - Reads the whole file at the reviewed revision and slices it
    (`slice_context_lines`, `src/vcs/mod.rs:75`).
  - Derives the old-side numbers from a constant offset,
    `delta = prev.new_end - prev.old_end`.
  - Stores the lines in `expanded_top` / `expanded_bottom` maps keyed by
    `GapId{file, hunk}`, then rebuilds the row annotations.
- The end-of-file gap needs the file's line count, which is cached.
- Expanded lines are drawn dim and are *not* highlighted
  (`render_expanded_context_line`, `src/ui/diff_unified.rs:1447`).

**Cost.**
- A new VCS read: `git show <id>:<path>` or `jj file show -r <id> <path>`.
- Two side maps, plus gap rows that every navigation and anchor path has to
  understand. It is moderate work.

**Fit for debrief.** There is a simpler model than tuicr's side maps:
- Insert the fetched lines into `CommitDiff` itself as `Context` `DiffLine`s.
  Prepend or append them to the neighbouring hunk, and merge two hunks when
  the gap between them closes. Then re-run `flatten()`.
- Comments, the editor jump, search, yank, highlighting and the sidebar
  counts then all work unchanged.
- Only one new row is needed, `RowKind::Gap { hidden }`, which is the
  expander target.

An even cheaper first step: a key that reloads the commit with more context.
Use `git show -U<n>` and `jj diff --context <n>`, or git's `-W`
(`--function-context`) for whole functions. It needs only a `context: u32`
argument to `Vcs::show`.

**Recommendation.** The context-size reload: **ADOPT NOW** (small). True gap
expansion: **LATER**.

## 8. File headers and sidebar

**What tuicr does.**
- **File header** (`file_header_prefix_text`, `src/ui/diff_view.rs:34`):
  `═══ ✓ path [M] ════…`. An overlay fills the `═` rule out to the right
  edge (`paint_file_header_fill`, `:1047`). Status letters: A/M/D/R/C.
- **Renames** show only the new path plus `[R]`; debrief's `old → new` is
  better.
- **Placeholders** (`binary_or_empty_label`, `:127`): `(binary file)`,
  `(file too large to display)` for untracked files over 10 MiB
  (`src/vcs/git/cli.rs:34`), and `(no changes)` for files with no hunks
  (mode-only changes, pure renames).
- **Collapsed files.** Files marked reviewed collapse to their header in the
  continuous view, and so do reviewed hunks (`should_collapse_file`,
  `src/app/reviewed.rs:440`).
- **Sidebar** (`src/ui/file_list.rs:19`):
  - A directory tree with `▼`/`▶` folders and optional compact chains.
  - Each file row shows a `▣`/`▢` reviewed box, a status letter coloured by
    `file_status_style` (A green, M yellow, D red, R pink;
    `src/ui/styles.rs:105`), and the base name.
  - No per-file stats or stats bars. The diff panel's title carries
    `+N -M` for the current file or for all files (`diff_stat_title`,
    `src/ui/diff_view.rs:240`).
  - Paths are truncated `…/tail`, keeping the base name
    (`truncate_path_smart`, `:209`).

**Fit for debrief.**
- **Sidebar.** Add a coloured status letter before the path (2 columns;
  `ui/sidebar.rs`). debrief's `+n −m` already beats tuicr. A small stats bar
  (`+++−−`) is optional.
- **Placeholders.**
  - Parse `old mode` / `new mode` in `vcs/diff.rs::header_line` and show
    `(mode 100644 → 100755)` in the header.
  - Emit a `(no content changes)` row for files that have no hunks and are
    not binary. Today these show a bare header.
- **Header rule.** Fill the `──` rule to the pane width in `ui/commit.rs`,
  together with §5's full-row painting.
- **Collapsing.** debrief has no per-file review state. A manual fold (for
  example `za` on a header), or auto-collapsing lockfiles and generated
  files, could come later and would reuse tuicr's "header only" idea.

**Recommendation.** Status letter, mode/empty placeholders and the full-width
rule: **ADOPT NOW** (small). Tree view and folding: **LATER**.

## 9. Performance on large diffs

**What tuicr does.**
- **Virtualisation, partial.** The renderer walks *every* row on every frame
  but builds spans only for rows in `[scroll, scroll + height)` and pushes
  `Line::default()` for the rest (`src/ui/diff_unified.rs:603`;
  `diff_visible_range`, `src/ui/diff_view.rs:158`). Comment boxes are culled
  the same way.
- A perf harness checks that scaling is O(visible) rather than O(diff)
  (`src/app/tests/render_perf_tests.rs`).
- **Caching.** Highlight spans are computed once at load and kept in
  `DiffLine`. The `SyntaxHighlighter`, which carries the expensive grammar
  set, sits in a `OnceLock<Arc<_>>` on the theme
  (`src/theme/mod.rs:2469-2484`).
- **Diff watch.** A periodic reload first parses with a no-syntax
  `SyntaxHighlighter::plain()` to fingerprint the diff (3.1 ms against
  197 ms; `src/syntax/mod.rs:128`, `src/app/diff_load.rs:1591`). It only
  re-highlights when the content has changed.
- **Frame level.** Each frame is wrapped in DEC 2026 synchronised output,
  tuicr redraws only when state changes, and queued input events are merged
  into one repaint (CHANGELOG #322, #428, #617).
- **Filtering.** `.tuicrignore` drops files before they are parsed or
  highlighted.

**debrief today.** debrief is already better virtualised: it renders
`rows.iter().skip(scroll).take(h)`. Some costs remain:
- `draw` calls `review_marked_rows()` (all rows × comments) and
  `search_hits()` (all hits) on every frame.
- The event loop redraws every 100 ms even when idle (`app/run.rs:36`).

**Fit.**
- With §1: highlight in the worker, deliver the spans as a second event, and
  cap by file size and line length.
- Cache `review_marked_rows` in `CommitView` and invalidate it when comments
  change.
- Optionally, redraw only after an event (§1 makes frames more expensive).

**Recommendation.** Async highlighting with caps: **ADOPT NOW** as part of
§1. Caching the marked rows and redrawing only on change: **LATER** (cheap,
but no measured problem yet).

## 10. Other notable points

- **Ignoring whitespace** (`src/vcs/whitespace.rs:30,84`; config
  `ignore_whitespace = false | true | "auto"`, `docs/CONFIG.md:94`). `auto`
  runs the diff twice, normally and with `-w`, and picks each file's version
  by extension (json, js, ts, rs ignore whitespace; Python and YAML do not).
  For debrief, a `-w` toggle that reloads (`git show -w`, `jj diff -w`) is
  cheap, and comment anchors stay valid because line numbers are real.
  **LATER** (small; could share the §7 reload plumbing).
- **Moved-code detection.** None in tuicr. git has `--color-moved`, but
  debrief parses with `--no-color`. **SKIP.**
- **Commit message.** tuicr turns the message into a synthetic file with
  `Context` lines and no line numbers. debrief's `Message` rows are cleaner.
  Nothing to adopt.
- **Strict hunk parsing.** tuicr treats the counts in the hunk header as the
  grammar (`src/vcs/diff_parser.rs:115-180`). Removed `-- comment` lines are
  kept, and truncated hunks are rejected. debrief's parser already counts
  `old_left` / `new_left`, so it is equivalent.

---

## Prioritised changes for debrief

1. **Render tabs, CR and control characters** (§3).
   - Files: `app/motion.rs` (`row_cells`), `ui/commit.rs` (`visible`),
     `app/search.rs` (`col_of`).
   - Size: S, about 40 lines plus tests. This is a bug fix: tab-indented code
     currently loses its indentation and the cursor drifts.
2. **Full-row add/remove/hunk/cursor backgrounds and a coloured `±`** (§5),
   plus the full-width file header rule (§8).
   - Files: `ui/commit.rs`, `ui/mod.rs` (palette: Mocha blends `#333d40`,
     `#402f41`, `#394545`, `#483346`).
   - Size: S, about 40–60 lines.
3. **Syntax highlighting** (§1): per hunk with old/new sequences, computed in
   the worker and delivered as a second event, with size and line-length
   caps. syntect, two-face and catppuccin-mocha are copied from ramble's
   `render.rs:511-540`, with tuicr's path-to-syntax fallback chain added.
   - Files: `crates/debrief/Cargo.toml`, new `app/highlight.rs`,
     `app/commit.rs` (`CommitView.spans`, `spawn_show`, a new `AppEvent`),
     `app/mod.rs`, `ui/commit.rs`, `docs/specs/2026-10-09-debrief.md`.
   - Size: M, about 250 lines.
4. **Context-size reload** (§7): `-U<n>` / `--context <n>` with keys to
   widen and reset. It could later carry a `-w` toggle (§10).
   - Files: `vcs/mod.rs` (`Vcs::show` options), `vcs/git.rs`, `vcs/jj.rs`,
     `app/commit.rs`, `app/keys.rs`, `app/help.rs`.
   - Size: S–M, about 100 lines.
5. **Header and sidebar polish** (§8): a coloured A/M/D/R letter in the
   sidebar, a mode-change label, and a `(no content changes)` row.
   - Files: `vcs/diff.rs`, `vcs/mod.rs` (`FileDiff` mode fields),
     `app/commit.rs` (`header_text`, `flatten`), `ui/sidebar.rs`.
   - Size: S, about 60 lines.

Next after these: word-level diff (§2) on top of the span plumbing from item
3, then true gap expansion (§7) by merging fetched context lines into the
hunks.
