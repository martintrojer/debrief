# debrief

Catch up with what the agents committed, comment on it, and send the
comments back. A terminal tool for git and jj, and `debrief-review`, the
comment crate it shares with [ramble](https://github.com/martintrojer/ramble).

Status: design approved, nothing built yet. See
[docs/specs/2026-10-09-debrief.md](docs/specs/2026-10-09-debrief.md).

## Sending a review

Comments collect in one batch per repo root (in the user cache dir,
`$DEBRIEF_CACHE_DIR` overrides it; nothing is written into the repo).
Sending exports the batch as markdown and hands it to one configured
command:

```toml
[send]
command = ["debrief-send"]   # default: none, copy to the clipboard
# preamble = "..."           # replaces the paragraph under the title; "" drops it
```

The command gets the markdown on stdin and `DEBRIEF_ROOT=<repo root>` in
its environment. If no command is set, it can't start, it exits non-zero,
or it takes longer than 10 s, the markdown goes to the clipboard instead.
The batch is cleared after either.

`scripts/debrief-send` routes the review to the mu agent working in that
repo: it finds the one workspace in `mu workspace list --all --json` whose
path is `$DEBRIEF_ROOT` and runs `mu agent send <agent> -w <workstream> -`
with the markdown. With no matching workspace, several, or no `mu` or
`jq` on `PATH`, it exits 1, so the review falls back to the clipboard.
Put it on your `PATH` (or give its full path in `command`).

## Build and test

```sh
bash scripts/check.sh   # fmt, clippy -D warnings, tests
```

## License

MIT
