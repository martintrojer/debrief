# tuicr: comment types and comment entry, and what debrief and ramble should take

Source: `~/hacking/tuicr` at `fb6fbe6` (v0.27.0, 2026-10-07), MIT,
"Copyright (c) 2025 tuicr contributors". Paths below are relative to that
checkout unless prefixed `debrief/` or `ramble/`.

## Summary

- **Types.** In tuicr a type is a free-form string id; untyped is a
  first-class `None`. Types come from config and each has a `definition`
  meant for LLMs. The export prints `**[LABEL]**` on each item and a
  legend listing only the types used. Tab and S-Tab cycle the type while
  typing. Since v0.19.1 tuicr ships **no** built-in types by default.
  We should add `kind: Option<String>` to `debrief_review::Comment`
  (backward compatible), ship four built-ins, start every comment
  untyped, and cycle with Tab/S-Tab in the prompt.
- **Editor.** tuicr's default editor (not the vim one) is a plain
  `String` plus a byte cursor, with readline keys, wrap-aware Up/Down
  and bracketed paste. It is about 400 non-test lines, and about 250 of
  those are reusable outside tuicr. The vim mode wraps `edtui`, a
  dependency of about 10k lines. tuicr started with a centred overlay and
  moved to an inline box because "the overlay obscured the code being
  commented on" (#137).
- **Q17 revisited.** "No in-app text editor" holds for vim mode. A small
  editor without vim costs about 300 shared lines plus about 250 per
  tool, and it fixes three real problems in today's prompt:
  1. Text past the terminal width can't be seen.
  2. There is no cursor movement, only append and backspace.
  3. A multi-line paste fires Enter: it saves the first line as the
     comment and runs the rest as normal-mode keys.

## (A) Comment types in tuicr

### Model

| What | Where |
|---|---|
| `enum CommentType { None (default), Custom(String) }`; `NONE_ID = "none"`; `from_id` trims and maps `""`/`none` (any case) to `None`; `as_str()` = id uppercased; `is_none()` | `src/model/comment.rs:72-117` |
| Serialised as a bare string id (`"issue"`, `"none"`), deserialised through `from_id` | `src/model/comment.rs:119-136` |
| `Comment.comment_type: CommentType`. No `serde(default)`: it was always present. Later fields (`author`, `lifecycle_state`, `commit_id`, …) all use `#[serde(default)]` for old session JSON | `src/model/comment.rs:155-199` |
| `CommentTypeDefinition { id, label, definition: Option, color: Option }` (resolved, at runtime) | `src/app/mod.rs:1452-1458` |
| App state: the type being typed (`comment_type`) and the ordered list (`comment_types`) | `src/app/mod.rs:1196-1197` |

### Built-ins and the untyped default

- Until #460 (`057b336`, v0.19.1, 2026-07-10) the defaults were
  `note` "observations", `suggestion` "improvements", `issue` "problems
  to fix" and `praise` "positive feedback", each with a theme colour (see
  `git show 057b336 -- src/app/init.rs`).
- Since then the defaults are **untyped**:
  `resolve_comment_types` returns `[None]` when `comment_types` is
  missing or every entry is invalid (`src/app/init.rs:659-709`).
  Configured types fully replace the defaults. `None` is appended to the
  cycle unless the user declared `none` themselves. **The first
  configured type becomes the default for new comments**
  (`first_comment_type`, `src/app/init.rs:711-720`).
- Neither the commit message nor the release note gives a reason for
  #460. A likely one: the `[TYPE]` prefix was noise on comments
  submitted to forges for human readers. The README example lost its
  `Comment types:` legend in the same change.
- Theme colours still exist for the four old ids (`src/theme/mod.rs:67-70`,
  `src/app/init.rs:787-793`). Any other id falls back to `fg_secondary`.

### Config (`src/config/mod.rs`)

- `CommentTypeConfig { id, label?, definition?, color? }` (`:59-66`),
  `AppConfig.comment_types: Option<Vec<…>>` (`:159`).
- `parse_comment_types` (`:761-863`):
  - lowercases and trims `id`;
  - drops entries with an empty or duplicate id, with a warning;
  - warns on unknown keys;
  - `label` and `definition` must be non-empty strings;
  - `color` is a terminal name or `#RRGGBB`.
  - If nothing is valid it falls back to the default.
- `export_legend` (`:195`), now `[export] legend` (`ExportConfig::legend`,
  `:142-144`, default true).
- `[forge] comment_type_prefix` (`:69-84`) prepends `[TYPE] ` to bodies
  on submit.
- Documented in `docs/CONFIG.md:211-262`: "`definition` — Guidance text
  for LLMs, included in the exported `Comment types:` legend". `label`
  defaults to the id uppercased.

### Choosing a type while typing

- `Tab` → `CycleCommentType`, `BackTab` → reverse. Any modifier is
  matched, and so is `Char('\t')`, because tmux without extended-keys
  sends raw 0x09 (`src/input/keybindings.rs:351-358`).
- `cycle_comment_type` / `_reverse` wrap around the list and post the
  status "Comment type: ISSUE", or `none` for the untyped type. With a
  single type they post "Only one comment type configured"
  (`src/app/comments.rs:1121-1178`).
- Vim box: Tab cycles in Normal mode and inserts `comment_tab_width`
  spaces in Insert mode (`src/main.rs:990-994`).
- A new comment starts at the default type
  (`enter_comment_mode`, `src/app/comments.rs:960-971`). Editing restores
  the comment's own type, and Tab can change it
  (`enter_edit_mode`, `:886-958`). `save_comment` writes the type back
  on edit (`:999-1119`).
- CLI: `tuicr review add --type <id>`, default `none` (`src/cli.rs:233-238`).
  An id missing from a configured list only warns and is stored anyway
  (`src/review_cli.rs:194-215`).

### Display

- **Box header badge.** `[LABEL]` in bold in the type colour, then the
  line info (`L12-L18`) and key hints. The badge is omitted for `None`
  (`src/ui/comment_panel.rs:212-238`, saved comments `:525-593`,
  badge text `:543-548`).
- **Type colour.** It colours only the label. The border uses the
  file-header style so the box "reads as a structural divider"
  (`src/ui/styles.rs:132-141`).
- **Comment navigator panel.** Each row's marker is the first letter of
  the label in the type colour (`I`, `S`, …), or `•` for an untyped
  comment (`src/ui/comment_navigator.rs:60-100`).
- **Summary popup and submit resolver.** They show `[LABEL]` when it is
  non-empty (`src/ui/summary_popup.rs:254-262`,
  `src/ui/submit_modals.rs:222-238`).
- **No gutter glyph per type.** Comments render inline as boxes, so
  there's no gutter column to mark.

### Export (`src/output/markdown.rs`)

- **Legend.** `Comment types: ISSUE (problems to fix), QUESTION (ask for
  clarification)` sits after the intro and scope, before the list.
  - It is filtered to used ids (`collect_used_comment_type_ids`,
    `:517-533`). `None` is never listed, and the line is omitted when
    nothing typed was used (`:319-344`).
  - A type with no definition falls back to its id.
  - The legend was added for agents, and #242 (`dae3f94`) cut it down to
    the types actually used.
- **Per item.** `1. **[ISSUE]** \`src/a.rs:42\` - body`. An untyped item
  has no marker (`:449-461`).
- **Label lookup.** `export_comment_type_label` (`:537-555`) uses the
  configured `label`, uppercased. It returns `""` for `None`, and an
  unknown id comes out uppercased.
- **Lesson from #611 (`2c38469`).** The label logic lives in three
  places: the UI (`src/app/init.rs:761-775`), the export and the forge
  submit (`src/forge/submit.rs:35-51`). A comment says "those three must
  agree", and they didn't (a configured label was dropped on submit).
  **Ours should live in one place, `debrief-review`.**

### Persistence

The session JSON stores `"comment_type": "issue"` or `"none"`. The id is
stable, while label, definition and colour are resolved from config at
render and export time. A comment whose id is later removed from config
still loads, renders `[ID]`, and is left out of the legend.

### Recommendation for debrief-review (A)

**Field.** ADOPT NOW, because it is additive and old batches still load.

```rust
/// Lowercase type id (`"issue"`, …); `None` is untyped.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub kind: Option<String>,
```

- **Name.** `kind`, because `type` is a keyword and `comment_type` stutters.
- **Untyped.** Never store `"none"`: absent means untyped.
- **Constructors.** Keep `on_file` and `on_diff` as they are and add a
  builder, `Comment::with_kind(self, Option<String>)` (tuicr's
  `with_author` pattern).
- **Edits.** Add `Review::set_kind(id, Option<&str>) -> io::Result<bool>`,
  built on `mutate` like `edit` (`debrief/crates/debrief-review/src/store.rs:85-93`).

**Backward compatibility.** ADOPT NOW.

- Old JSON has no `kind`, so it deserialises to `None`. Keep `VERSION = 1`.
  Bumping it would make older binaries refuse the file
  (`store.rs:192-197`).
- Caveat: an older binary that writes a batch re-serialises every
  comment (`mutate`, `store.rs:145-160`) and silently drops `kind`.
  Rebuild ramble and debrief together. There is no lock-step issue
  beyond that, since ramble path-depends on the crate.
- LATER: a `#[serde(flatten)] extra: serde_json::Map<String, Value>` on
  `Comment` would let future binaries keep fields they don't know.

**Built-in kinds, defined in the crate.** ADOPT NOW: one definition list
for both tools and the export (the lesson from #611).

```rust
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Kind { pub id: String, pub definition: Option<String> }
impl Kind { pub fn builtin() -> Vec<Kind>; pub fn label(id: &str) -> String /* uppercased */ }
```

The definitions are written for the agent reading the hand-back:

| id | definition |
|---|---|
| `issue` | Something is wrong. Fix it. |
| `suggestion` | A better way to do it. Apply it, or say in your reply why not. |
| `question` | The reviewer wants an answer. Answer in your reply; change code only if the answer shows a problem. |
| `nit` | Minor or cosmetic. Fix it if it's cheap. |

- `praise`: SKIP as a built-in. It doesn't tell the agent what to do, but
  users can add it.
- **Default selection = untyped.** This differs from tuicr, where the
  first configured type is the default. Every new comment starts as
  `None` and Tab moves to `issue`. Anyone who never presses Tab gets
  exactly today's output, so the built-ins cost nothing until they're
  used. ADOPT NOW.

**Config shape.** The same in both `config.toml` files. ADOPT NOW, with
the semantics copied from tuicr.

```toml
[review]
# Omitted: the four built-ins. Present: replaces them. `kinds = []`: untyped only.
kinds = [
  { id = "issue",    definition = "Something is wrong. Fix it." },
  { id = "question", definition = "Answer in your reply." },
]
```

- ramble already has `[review]` (`ramble/src/config.rs:234-237`).
  debrief gains the section.
- Ids are lowercased and trimmed. Empty or duplicate ids are dropped with
  a warning, as in tuicr's `parse_comment_types`.
- The crate's `Kind` derives `Deserialize`, so each tool's Raw config
  embeds `Option<Vec<Kind>>`.
- `label`: SKIP (show the id uppercased).
- `color`: LATER (see display below).

**Export.** ADOPT NOW.

- Change the signature to `to_markdown(&self, preamble: Option<&str>,
  kinds: &[Kind])`.
- Add the label to the item heading so an agent can echo it:
  `## Item 1 [ISSUE]`. Untyped items keep `## Item N`.
- Print a legend of the used kinds only, after the preamble and before
  Item 1. It stays even when the preamble is `""`, because it describes
  the items, not the request.

  ```markdown
  Items are tagged with a type:
  - ISSUE: Something is wrong. Fix it.
  - QUESTION: The reviewer wants an answer. …
  ```

- Leave a kind out of the legend when no entry in `kinds` defines it,
  as tuicr does. It still gets its `[ID]` heading label.
- No `legend = false` switch: SKIP until someone asks.
- `debrief/crates/debrief-review/src/export.rs:20-80`;
  update the snapshot tests in `crates/debrief-review/tests/export.rs`.

**Key to pick a type.** ADOPT NOW: `Tab` / `S-Tab` in the comment
prompt, matching `Char('\t')` too.

- Order: untyped → configured kinds in order → untyped.
- Both prompt keymaps currently ignore Tab
  (`debrief/crates/debrief/src/app/comment.rs:53-68`,
  `ramble/src/app/comment.rs:49-63`), and normal-mode Tab bindings don't
  apply in `Mode::Comment`.
- Show the kind in the prompt label: `comment [ISSUE]: …`.
- SKIP magic prefixes typed in the body (e.g. `?` or `!issue`). They are
  hard to discover and they collide with real text.

**Display of kinds.**

- ADOPT NOW: `[ISSUE]` in the picker detail, the `K` popup header and
  the status preview. That's one helper per tool on top of
  `Kind::label`.
- LATER: colour the gutter `●` by kind. That needs `color` in config and
  a palette mapping.
- LATER: Tab in the `<leader>rl` picker to re-type the selected comment
  (`set_kind`).
- SKIP: tuicr's first-letter navigator marker. We have no navigator
  panel.

## (B) Comment entry in tuicr

### Where the box goes

- **Inline, not a popup.** The input box is rendered as diff rows
  directly after the commented line: on the old side for removed lines,
  after any existing comments on that line. For a range,
  `comment_line = range.end` (`src/app/visual.rs:133-136`).
- **Range marker.** The box's top-left corner becomes `├` and a bar is
  painted up through the selected rows, ending in `╭`, so the range stays
  visible (`src/ui/comment_panel.rs:206-210`, `src/ui/diff_view.rs:825+`,
  `:929+`).
- **No excerpt.** The code itself sits right above the box. The header
  reads `Add [ISSUE] L12-L18 (Tab/S-Tab:type Enter:save
  Alt-Enter:newline Esc:cancel)`. The newline hint adapts to whether
  kitty keyboard enhancement is on (#641).
- **Scrolling.** `scroll_comment_input_into_view` keeps the box, or the
  cursor row when the box is taller than the viewport, on screen as it
  grows (`src/ui/diff_view.rs:348-400`).
- **IME.** The terminal cursor is always set to the text cursor so IME
  candidates appear at the right place (`src/ui/app_layout.rs:99-114`).
- **Cost of inline.** `format_comment_input_lines` is called from
  8 sites in `diff_unified.rs` and 6 in `diff_side_by_side.rs`, and the
  annotation model has to count the box's rows. That cost only makes
  sense with tuicr's annotated-row renderer.
- **History.** `31eff14` (#137, 2026-01) replaced the original centred
  overlay with this inline box. The overlay was `centered_rect(60, 40)`
  with a title, `Type: X (Tab to cycle)` and a wrapped `Paragraph`,
  about 130 lines in `git show 31eff14^:src/ui/comment_panel.rs`. The
  reason given: "obscured the code being commented on".

### Editing an existing comment

`i` on a comment box swaps that box for the input box in place
(`enter_edit_mode`, `src/app/comments.rs:886-958`). The text cursor lands
on the comment line the diff cursor points at
(`comment_current_line_cursor`, `:840-882`). In vim mode `A` puts the
cursor at the end of that line. The comment's type comes back with it.

### The default editor (no vim)

**State.** `comment_buffer: String`, `comment_cursor: usize` (a byte
offset) and `comment_preferred_column: Option<usize>`
(`src/app/mod.rs:1175-1178`).

| Capability | How / where |
|---|---|
| Insert char; paste (CRLF/CR → LF) | `src/handler.rs:1195-1204` |
| Backspace, UTF-8 safe | `text_edit::delete_char_before` (`src/text_edit.rs:37-44`) |
| Delete word back: `C-w`, `Alt-Backspace`, `Cmd-Backspace` | `text_edit::delete_word_before` (`:48-79`); whitespace-delimited |
| `C-u` | clears the **whole buffer** (named `ClearLine`, `src/handler.rs:1242-1245`) |
| Left/Right | `prev/next_char_boundary` (`src/text_edit.rs:8-33`), by code point (not grapheme) |
| Word left/right: `C-`/`Alt-←→`, `Alt-b/f` | `comment_word_left/right` (`src/handler.rs:483-552`) |
| Line start/end: `Home`/`End`, `C-a`/`C-e`, `Cmd-←→` | `comment_line_start/end` (`src/handler.rs:467-481`) |
| Up/Down by **displayed** (wrapped) row with a sticky column | `move_comment_cursor` (`src/ui/comment_panel.rs:62-108`, #744, 2026-10) |
| Wrapping | hard wrap by display width (`unicode-width`), not word wrap: `wrap_segments` (`src/ui/comment_panel.rs:25-51`); pre-wrapped so ratatui never re-wraps (#318) |
| Newline | `Shift-Enter` (only with the kitty protocol), `Alt-Enter`, `C-j`, `C-k` (`src/input/keybindings.rs:341-350`) |
| Save / cancel | `Enter`, `C-Enter`, `C-s` / `Esc`, `C-c` (`:333-340`) |
| Bracketed paste | `EnableBracketedPaste` (`src/terminal_state.rs:217-220`); `Event::Paste` routed to the comment box (`src/main.rs:848-857`) |
| Markdown highlighting in the box | syntect via `highlight_markdown_body` (`src/ui/comment_panel.rs:256-258`) |
| Undo | **none** |
| Delete forward (`Delete`) | **not mapped** |
| Empty body | "Comment cannot be empty"; the body is trimmed on save (`src/app/comments.rs:999-1006`) |

The follow-up fixes show where the hidden cost is: UTF-8 (#147), border
overflow from wrapping (#318), newline keys per terminal (#641) and
vertical movement (#744). Most of that is terminals reporting
modified Enter differently, not the editing itself.

### Vim mode (opt-in: `comment_vim`, `:vim`)

- `src/comment_vim.rs` (109 lines without tests) wraps
  `edtui::EditorState` and `EditorEventHandler`. `edtui` 0.11.7 is about
  10.2k lines and pulls in `edtui-jagged`, `enum_dispatch` and
  `ratatui-core`. tuicr uses it as an engine only and keeps its own
  rendering. Conversions between byte offsets and `Index2` are at
  `:68-109`.
- `src/app/comment_vim.rs` (174 lines): lazy overlay synced back into the
  canonical buffer after every key, plus:
  - a `:w`/`:q` command line;
  - double-Enter to save and double-Esc/`q` to cancel, with a red confirm
    hint;
  - soft tab.
- `src/main.rs:928-1001` routes keys. `Alt-Enter` saves and `Alt-Esc`
  discards directly (#440).
- What it adds: Normal/Insert/Visual, `hjkl`, `w b e`, `dd`, `D`, `ciw`,
  `x`, `u`/`C-r` undo, and visual yank/delete/paste.

### Comment panel and navigator

- `src/ui/comment_panel.rs` (636 lines without tests) holds:
  - the box formatters (input box, saved comment, remote thread, remote
    review summary);
  - `wrap_segments` and `move_comment_cursor`;
  - a confirm dialog.
- `src/ui/comment_navigator.rs` (117 lines) is a list panel under the
  file tree: marker, author dot, `path:line`. It is built from the
  annotation rows (`build_comment_navigator_items`,
  `src/app/comments.rs:195-218`). `Enter` jumps; `m`/`M` go to the
  next/previous comment, wrapping, and post "Comment i/n"
  (`:476-528`).
- `src/ui/submit_modals.rs` holds the forge submit modals (centred
  `Clear` boxes). They have nothing to do with entry.

### Size (non-test lines)

| Piece | Lines |
|---|---|
| `text_edit.rs` | 82 |
| handler motions + `handle_comment_action` | ~95 + 73 |
| `map_comment_mode` | 73 |
| `wrap_segments` + `move_comment_cursor` + width helper | ~85 |
| input box render (`format_comment_input_lines`, with highlighting) | 176 |
| **default editor, total** | **~585**, of which ~250 is reusable logic and the rest is tuicr's keymap and rendering |
| vim: `comment_vim.rs` + `app/comment_vim.rs` + `main.rs` routing | 109 + 174 + ~74, plus `edtui` |

Dependencies: `unicode-width` and `crossterm` (bracketed paste), both of
which debrief and ramble already have. For vim mode, `edtui` as well.

### What our prompt lacks today

- **Long text can't be seen.** The status-row prompt is a `Paragraph`
  without wrap or scroll, and the cursor is clamped to the right edge.
  Anything typed past the terminal width is invisible
  (`debrief/crates/debrief/src/ui/status.rs:31-36`,
  `ramble/src/ui/picker.rs:103-113`).
- **Append and backspace only.** There is no cursor movement, no word
  delete and no clear (`Draft.text.push` / `.pop`:
  `debrief/crates/debrief/src/app/comment.rs:82-94`,
  `ramble/src/app/comment.rs:80-89`).
- **Multi-line paste fires Enter.** Neither tool enables bracketed paste
  (`debrief/crates/debrief/src/app/run.rs:36-41`, `ramble/src/app/run.rs:125-130`).
  A pasted newline arrives as `\r`, which crossterm reads as `Enter`. The
  prompt saves the first line, and the rest of the paste is replayed as
  normal-mode keys.
- **The selection disappears.** Starting a comment drops the visual
  selection, so while typing you can't see which lines you're commenting
  on (`debrief/…/comment.rs:125-127`, `ramble/src/app/comment.rs:124`).
- **`Shift-Enter` won't work as a newline key for us.** Neither tool
  pushes keyboard-enhancement flags, so Shift-Enter arrives as plain
  Enter. In raw mode `C-j` arrives as `Char('j')+CONTROL` (crossterm
  `parse.rs`), and `Alt-Enter` arrives with ALT. Those are the two that
  work.
- **Conflict.** tuicr uses `C-e` for end of line, but ours escalates to
  `$EDITOR`. Keep ours and use `End` (and `C-a`/`Home`) for line
  start and end.

### Recommendation (B)

**Q17 revisited.** "A multi-line popup editor … is building a text
editor" is true of tuicr's vim mode (an `edtui` dependency of about
10k lines plus 357 lines of glue). It is not true of the readline-level
editor. The reusable part is about 250 lines, and our version would be
about 300 shared lines with tests plus about 250 per tool. `$EDITOR` via
`C-e` stays for anything long. Proposed amendment to Q17: "Comment entry:
a small popup editor anchored next to the lines (Enter saves, `C-j` /
`Alt-Enter` newline, Tab cycles the kind); `C-e` escalates to `$EDITOR`;
no vim mode."

| Option | Verdict | Reason |
|---|---|---|
| Bracketed paste routed to the prompt (newlines kept in the popup, turned into spaces in a one-line prompt) | **ADOPT NOW** | Fixes a live bug where a paste saves half a comment and runs the rest as keys. About 15 lines per tool; disable it around `suspend_and_run`. |
| Shared `TextBox` model (readline keys, wrap rows, Up/Down by displayed row) | **ADOPT NOW** | Cursor movement and visible long text are table stakes. The model is free of ratatui and unit-testable. |
| An anchored popup replacing the status-row prompt | **ADOPT NOW** (after the model) | Wraps long comments, has room for `[KIND] L12-18` and key hints, and costs about 120 lines per tool. Anchored below the last selected row, or above the first when it doesn't fit, so it never covers the lines (the reason for tuicr's #137). |
| Keep the draft range highlighted while typing | **ADOPT NOW** | Gives the context an inline box would give, at about 15 lines per tool. No excerpt is needed in the popup. |
| Multi-line (newline on `C-j` / `Alt-Enter`) | **ADOPT NOW** | Almost free once wrapping and Up/Down exist. Enter still saves, so one-liners are unchanged. |
| Keep the one-line status prompt as is | SKIP | The three defects above. |
| Inline box in the diff or document, as tuicr does | SKIP | Needs a row/annotation model in both renderers. tuicr touches 14 call sites, and ramble's rendered markdown layout makes it worse. |
| Vim mode via `edtui` | SKIP | About 10k lines of dependency plus about 360 of glue. `C-e` already gives the user's real vim. |
| Undo | SKIP | tuicr's default editor has none either. Esc, `C-u` and `C-e` cover it. |
| Markdown highlighting in the box | SKIP | Needs syntect, and neither tool's spec has syntax highlighting. |
| Edit an existing comment in the popup (`K` then `e`, or picker `C-e` opening the popup) | LATER | Useful, but `$EDITOR` editing already exists. Do it once the popup is in. |
| Comment navigator panel | SKIP | `<leader>rl` and `]r` / `[r` already cover it. |
| tuicr's double-Enter/Esc confirms, `:w`/`:q` | SKIP | Only needed with modal editing. |

**What to copy, and attribution.**

- Copy the ideas and small functions, not tuicr's app-state code:
  - `text_edit.rs` (boundaries, delete char and word);
  - `comment_line_start/end` and `comment_word_left/right`
    (`src/handler.rs:467-552`);
  - `wrap_segments` and `move_comment_cursor`
    (`src/ui/comment_panel.rs:25-108`).
- Turn them into methods on one struct:

  ```rust
  pub struct TextBox { text: String, cursor: usize, goal_col: Option<usize> }
  // insert, insert_str (paste; CRLF→LF; newline-free variant), backspace,
  // delete_word, clear, left, right, word_left, word_right, home, end,
  // up(width), down(width), rows(width) -> Vec<Range<usize>>, cursor_cell(width) -> (row, col)
  ```

- MIT requires the copyright and permission notice in "all copies or
  substantial portions". Head the file with:

  ```rust
  //! Adapted from tuicr (https://github.com/agavra/tuicr), src/text_edit.rs,
  //! src/handler.rs and src/ui/comment_panel.rs.
  //! Copyright (c) 2025 tuicr contributors. MIT License; see THIRD-PARTY.md.
  ```

  Add a `THIRD-PARTY.md` (tuicr's LICENSE text) to each repo that holds
  a copy. If the code ends up rewritten rather than copied, a
  "design after tuicr" note is enough. Being explicit is cheap either
  way.

**Where it lives.**

- The spec says no shared UI crate (Q6, Q8), so copy
  `textbox.rs` into `debrief/crates/debrief/src/app/textbox.rs` and
  `ramble/src/app/textbox.rs`. Keep the two copies byte-identical, with
  the tests inside the file.
- The popup renderers stay per tool. Each tool's palette and anchoring
  differ, and ramble's `ui/hover.rs:23-31` already has the
  below-else-above placement to copy.
- Tempting but SKIP: putting `TextBox` into `debrief-review` (ratatui-free).
  It would end the duplication, but it puts key-editing semantics into
  the store crate. Revisit if a shared UI crate ever happens.

**Cost.**

| Item | Lines |
|---|---|
| Shared `textbox.rs` | ~180 code + ~120 tests (×2 copies) |
| Per tool: `comment.rs` (`Draft.text` → `TextBox`; the `CommentAction` set grows: motions, `Newline`, `Paste`, `CycleKind`; keymap) | ~90 |
| Per tool: popup draw (anchor, `Clear`, title ` comment [ISSUE] L12-18 `, wrapped rows capped at ~10 and scrolled to the cursor, footer `Enter save · C-j newline · Tab type · C-e editor · Esc cancel`, terminal cursor for IME) | ~80 |
| Per tool: bracketed paste in `run.rs` + `AppEvent::Paste` | ~20 |
| Per tool: draft-range highlight | ~15 |
| Per tool: e2e tests | ~100 |
| **Total** | **~300 shared + ~300 per tool** |

## First changes, in order

1. **`kind` in debrief-review.** Add the field, the `with_kind` builder,
   `set_kind`, `Kind` with `builtin()` and `label()`, and the export
   changes: `## Item N [KIND]` and a legend of the used kinds, with
   `to_markdown(preamble, kinds)`. Old JSON keeps working; add a test
   that loads a v1 file without `kind`.
   Files: `crates/debrief-review/src/{comment.rs,store.rs,export.rs,lib.rs}`,
   `tests/{export.rs,store.rs}`, and the callers of `to_markdown` in
   both tools. About 130 lines of code and 100 of tests.
2. **Bracketed paste in both tools.** Enable and disable it in
   `run.rs`, including around `suspend_and_run`. Route `Event::Paste`
   to the comment prompt and the picker/filter inputs; newlines become
   spaces until the popup lands.
   Files: `debrief/crates/debrief/src/app/{run.rs,mod.rs,comment.rs}`,
   `ramble/src/app/{run.rs,mod.rs,comment.rs}`. About 25 lines each,
   plus a test.
3. **Kind selection and display.** `[review] kinds` config, Tab/S-Tab
   in the prompt, a `comment [ISSUE]: ` label, and `[ISSUE]` in the
   picker, the `K` popup and the status preview.
   Files: `debrief/crates/debrief/src/app/{config.rs,comment.rs,picker.rs,review.rs}`,
   `ramble/src/{config.rs,app/comment.rs,app/picker.rs,app/review_glue.rs}`,
   `ramble/config.default.toml`, and both READMEs. About 80 lines each,
   plus tests.
4. **Shared `textbox.rs`.** Adapted from tuicr with attribution and
   `THIRD-PARTY.md`, and wired into the existing prompt first (cursor
   motions, `C-w`, `C-u`, `Home`/`End`, horizontal scroll so long text
   stays visible).
   Files: new `app/textbox.rs` in each repo, plus `comment.rs`,
   `ui/status.rs` (debrief) and `ui/picker.rs` (ramble). About 300 lines
   shared (with tests) and 60 per tool.
5. **Anchored multi-line popup.** It replaces the status-row prompt,
   keeps the draft range highlighted, and adds `C-j`/`Alt-Enter`
   newlines. Amend spec Q17 and §2 "Entry" to match.
   Files: new `ui/comment_box.rs` in each tool, `ui/mod.rs`,
   `app/comment.rs`, the commit/content row highlight, and
   `docs/specs/2026-10-09-debrief.md`. About 150 lines each, plus about
   100 of e2e tests.
