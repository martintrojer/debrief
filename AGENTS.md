# AGENTS.md

## Gates

- **Fast** (while iterating): `cargo nextest run -p debrief -E 'binary(<test file>)'`, or `cargo test -p debrief --lib <module>` for unit tests.
- **Slow** (before closing work, and once per merged batch): `bash scripts/check.sh` — fmt, clippy `-D warnings`, `cargo test --workspace`. ~1 min warm with a fast git (below); ~4.5 min through a slow git wrapper. CI runs it on Linux with tmux and jj installed.
- **Targeted slow**: `cargo nextest run -p debrief --test tui_e2e` (real binary in tmux) and `--test vcs_jj` (needs jj; skipped without it). A skip is not a pass: say so in the gate record.
- **Budget**: slow gate 5 min. Over budget is a trigger for test upkeep, not a stop.
- **History**: read the latest `docs/test-health/` report before test upkeep; it holds what was measured, merged and refuted. End an upkeep pass with a new dated report.

Record each gate run in the task note (`GATE: commit tree command scope`, `RESULT: exit duration counts skips`, `CONDITIONS: host, git, load, cache`).

Fast git: the suite spawns ~11k git processes, so the git binary dominates its time. If the `git` on PATH is a slow wrapper, point both the app and the fixtures at a faster binary for local runs:

```sh
d=$(mktemp -d); ln -s <fast git> "$d/git"
DEBRIEF_GIT="$d/git" PATH="$d:$PATH" bash scripts/check.sh
```

`DEBRIEF_GIT` (or `[vcs] git` in config) picks the git debrief runs; test fixtures honour it through `tests/common` `git_program()`.

Test notes:
- Build fixture repos with `tests/common` `git_repo` (one `fast-import`) and jj ones with `JjEnv::with_history`, not add+commit loops.
- Wait for the known number of worker events (`settle_events(app, rx, n)`), not for a quiet period. Watcher negative checks use `quiet()` (5x the 300 ms debounce); state the timing contract when adding a wait.
- Keymap, help and clue unit tests run on `test_support::fake_app` (no git).
- `tests/tui_e2e.rs` `jj_snapshot_and_no_snapshot`'s default-mode run is the only check that `main.rs` passes `cli.open_options()` to `vcs::open`.
