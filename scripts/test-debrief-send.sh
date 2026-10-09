#!/usr/bin/env bash
# Tests scripts/debrief-send against a fake `mu` on PATH. Needs jq.
set -euo pipefail
cd "$(dirname "$0")"
send="$PWD/debrief-send"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

mkdir "$tmp/bin"
cat >"$tmp/bin/mu" <<'MU'
#!/usr/bin/env bash
case "$1 $2" in
  "workspace list") cat "$FAKE_MU_LISTING" ;;
  "agent send") printf '%s\n' "${@:3}" >"$FAKE_MU_DIR/args"; cat >"$FAKE_MU_DIR/stdin" ;;
  *) exit 2 ;;
esac
MU
chmod +x "$tmp/bin/mu"
export PATH="$tmp/bin:$PATH" FAKE_MU_DIR="$tmp" FAKE_MU_LISTING="$tmp/listing.json"

cat >"$FAKE_MU_LISTING" <<'JSON'
{"items":[
  {"agentName":"worker-1","workstreamName":"ws-a","path":"/w/a/worker-1"},
  {"agentName":"worker-2","workstreamName":"ws-a","path":"/w/a/worker-2/"},
  {"agentName":"dup-1","workstreamName":"ws-b","path":"/w/dup"},
  {"agentName":"dup-2","workstreamName":"ws-c","path":"/w/dup/"}
],"count":4}
JSON

fail() { echo "FAIL: $*" >&2; exit 1; }

# A unique match: sends stdin to that agent in its workstream.
err=$(printf '# Review\nit\x27s $HOME `x`\n' | DEBRIEF_ROOT=/w/a/worker-1 "$send" 2>&1) || fail "unique match exited $?"
[[ "$err" == "sent to worker-1" ]] || fail "stderr: $err"
[[ "$(cat "$tmp/args")" == $'worker-1\n-w\nws-a\n-' ]] || fail "args: $(cat "$tmp/args")"
[[ "$(cat "$tmp/stdin")" == $'# Review\nit\'s $HOME `x`' ]] || fail "stdin: $(cat "$tmp/stdin")"

# Trailing slashes are ignored on either side.
rm -f "$tmp/args"
echo hi | DEBRIEF_ROOT=/w/a/worker-2 "$send" 2>/dev/null || fail "trailing slash in listing"
[[ "$(head -1 "$tmp/args")" == worker-2 ]] || fail "wrong agent for worker-2"
echo hi | DEBRIEF_ROOT=/w/a/worker-1/ "$send" 2>/dev/null || fail "trailing slash in root"

# No match, several matches, unset root: exit non-zero and send nothing.
for root in /w/none /w/dup /w/a; do
  rm -f "$tmp/args"
  if echo hi | DEBRIEF_ROOT=$root "$send" 2>/dev/null; then fail "$root should fail"; fi
  [[ ! -e "$tmp/args" ]] || fail "$root sent something"
done
set +e
echo hi | env -u DEBRIEF_ROOT "$send" 2>/dev/null
code=$?
set -e
[[ $code -eq 1 ]] || fail "unset root exited $code"

# mu missing: exit 1.
mkdir "$tmp/nomu"
for tool in bash cat jq; do ln -s "$(command -v $tool)" "$tmp/nomu/$tool"; done
set +e
echo hi | DEBRIEF_ROOT=/w/a/worker-1 PATH="$tmp/nomu" "$send" 2>/dev/null
code=$?
set -e
[[ $code -eq 1 ]] || fail "missing mu exited $code"

echo "debrief-send: ok"
