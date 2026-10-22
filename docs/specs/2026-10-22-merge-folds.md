# Merge folds in the log

Agreed with the user (Q1–Q10, 2026-10-22).

## Problem

The log draws one row per commit with no graph, in the VCS's default order
(git by date, jj topologically). On a branch with merge bubbles or octopus
merges, that flat list loses the shape:

```
Octopus x y z      ← 4 parents; nothing says so
m2
z1                 ← side-branch work, indistinguishable from m2
y1
x1
Merge b
Merge a
m1
b2                 ← dated between m1 and a2, unrelated to either
a2
…
```

Opening a merge disagrees between backends: git shows the first-parent diff
(everything the bubble brought in), jj shows the diff from the auto-merge
of all parents, which is empty for a clean merge. And every commit in a
bubble carries its own `●`, so one agent merge leaves N new markers to
visit.

The concern is messy history on one branch, not branches spanning the
view. The user rebases their own work but reads repos that merge.

## Design

### The spine

- With folding on, the log lists the **first-parent spine** of whatever it
  lists today: git adds `--first-parent` to `log`; jj wraps the revset as
  `first_ancestors(<rev>)` intersected with it (`trunk()..b` becomes
  `first_ancestors(b) & trunk()..b`; the default `::@` becomes
  `first_ancestors(@)`).
- Branch scope, revsets, the text filter and paging work on the spine
  unchanged. A linear history looks exactly as it does today.
- **Path filter:** git `--first-parent -- <path>` lists the merges whose
  first-parent diff touches the path, not the side commits. That composes
  with folds (the merge stands for its bubble) as long as a fold's contents
  are fetched with the same path filter (below). jj lists the same
  commits: its `files()` matches a merge on its diff against the
  auto-merge (empty for a clean merge), so the spine is
  `first_ancestors(h) & (R) & files(P)` plus every spine merge `m` whose
  bubble touches the path (`(::m ~ ::first_parent(m)) & files(P)` not
  empty), checked per merge in batched revset unions. `h` is the one head
  of `R`; a revset with several heads is listed flat.

### A merge row

- A row whose commit has more than one parent is a merge. It shows a fold
  marker and its size before the summary: `▸ +5` closed, `▾ +5` open, where
  5 is the commits the merge brought in (`p1..merge` without the merge
  itself), counting nested ones.
- The count comes from one cheap query per merge on the page (git
  `rev-list --count <p1>..<merge>` minus one, as it includes the merge;
  jj the size of `::<merge> ~ ::first_parent(<merge>) ~ <merge>`), run
  with the page's load off the UI thread. If it isn't in yet,
  the row shows `▸` alone.

### An open fold

- Opening a fold fetches its contents lazily, off the UI thread, with the
  same path filter and page limit as the log. The fold has one group per
  non-first parent `pN` (N ≥ 2): that parent's own first-parent spine,
  down to where it meets the merge's first parent.
  - git: `log --first-parent <pN> --not <p1>`.
  - jj: `first_ancestors(<pN>) ~ ::first_parent(<merge>)`.

  Merges inside a group are themselves folds, so nested bubbles stay
  closed until opened.
- The rows sit under the merge, indented one step per nesting level:
  groups in parent order (`p2` first), each group newest first.
- **Octopus:** each group's rows get a one-column marker naming the group,
  `├` for every group but the last and `╰` for the last, so three incoming
  lines stay apart. A 2-parent merge has one group and no marker.
- A fold over the page limit ends with its own `… more` row, loading the
  next page of that fold.

### Keys

- On the log, `l` (and Right) opens the fold under the cursor, `h` (and
  Left) closes it or, on a row inside a fold, moves to its merge and
  closes that. This matches the file list's tree keys. On a non-merge row
  outside a fold they do nothing.
- `zR` opens every fold on the loaded page, nested ones included; `zM`
  closes all. `z` becomes a pending prefix on the log, as it is in the
  diff.
- Enter opens the row under the cursor as today; `]]` / `[[` step over the
  visible rows, so closed folds are skipped and open ones are walked.

### The toggle

- `[view] fold_merges = true` (default) sets it at start; `<leader>m`
  flips it for the session. Off is today's flat list in the VCS's default
  order. It follows the toggle pattern: a key, a config key in
  `config.default.toml`, a row in the guide's toggles table (enforced by a
  test).
- Flipping reloads the log; the cursor stays on its commit if it is still
  a visible row, else goes to the merge whose fold holds it, else the
  first row.

### Opening a merge

- Both backends show the diff against the **first parent**: git as today
  (`show --first-parent`); jj `diff --from first_parent(<merge>) --to
  <merge>` in place of `diff -r`. The stats pane uses the same base.
- Comments on removed lines of a merge anchor to the first parent, which
  the comment model's single old side already expresses. The jj `@`
  refusal (`MERGE_OLD_SIDE`) goes: `working_base` takes `@`'s first parent
  when it has several.

### Seen

- Opening a merge marks it and everything in its bubble seen (the
  commits its fold lists, nested included). The fold's contents are
  fetched for that if they aren't loaded yet.
- A closed fold's marker shows the bubble's state: `●` if any commit in it
  is new, `◐` if any is updated, else the merge's own.

## Out of scope

- A lane graph (`│ ├─╮`) and topological reordering of the flat list. jj
  and `git log --graph` already draw those.
- Branches spanning the view, remote branches.
- jj's "what the merge itself changed" (conflict resolution) diff. It
  could be a later per-merge toggle.

## Decisions

- **Fold, not graph (Q4).** A bubble is one landing; the spine answers
  "what landed". Folding keeps every motion, search and paging unchanged
  and needs no lane allocator; an octopus is a fold with groups. The graph
  octopus in the probe repo took four lanes, which a narrow pane can't
  spare.
- **First-parent diff for both (Q5).** It is git's current behaviour and
  the natural review of a landing. jj's auto-merge diff is empty for a
  clean merge, which reads as broken.
- **Opening a merge marks its bubble seen (Q6).** Reading the squashed diff
  is reading the commits; otherwise one merge leaves N stale markers.
- **Recursive folds, octopus group markers only (Q7).** Nested bubbles are
  common in messy history; indentation carries a 2-parent fold, and a
  one-column group marker keeps an octopus readable without lanes.
- **`h` / `l`, `zR` / `zM` (Q8).** The file list's tree keys and vim's
  fold keys. `h` / `l` do nothing on the log today.
- **A toggle, on by default (Q9).** Linear repos look the same either way;
  off restores today's view.
- **Lazy fold contents (Q10).** A page costs what it costs today plus one
  count per merge; big bubbles never stall the first paint.

## Open questions

- `<leader>m` is free among the built-in keys; a user `[[launch]]` key
  on it would win, as for every leader key.
- The count per merge is one subprocess each. On a page with many merges
  under a slow git, measure it, and if it shows, fold it into one
  `rev-list` call or show `▸` until opened.

## Checklist

1. `Vcs`: `log_in` gains a spine flag; `fold(merge, parent, path, limit)`
   lists one group; `merge_size(merge)`. git and jj, with `vcs_git` /
   `vcs_jj` tests on a fixture with two bubbles, a nested bubble and an
   octopus.
2. jj `show` / `stat` diff from the first parent; drop `MERGE_OLD_SIDE`
   in `working_base`.
3. `Row` gains a fold depth, group marker and parent merge; the log model
   keeps the open folds and splices their rows in; load fold contents off
   the UI thread like other loads.
4. `ui/log.rs` draws the marker, size, indentation and group glyphs.
5. Keys `h` / `l` / `zR` / `zM` on the log; `]]` / `[[` over visible rows.
6. Seen: mark the bubble on open; closed-fold marker from the bubble.
7. `[view] fold_merges` + `<leader>m`, default config, guide toggles row
   and a *Merges* section.
8. tmux e2e: a merge bubble repo opens folded, `l` opens, Enter shows the
   first-parent diff.
