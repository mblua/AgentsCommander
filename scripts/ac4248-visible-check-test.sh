#!/usr/bin/env bash
# Recipe v2 offline proof of scripts/ac4248-visible-check.jq (plan v3). No GUI.
# Run: bash scripts/ac4248-visible-check-test.sh   (exit 0 only when every case passes)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
checker="$here/ac4248-visible-check.jq"
fails=0
T=

# S3 command, as run by the recipe; $1 is the GATE_FOCUS value.
check() {
  local focus=$1
  jq -rn --slurpfile s "$T/visible.samples" --slurpfile a0 "$T/visible-pre-active.json" \
    --slurpfile w0 "$T/visible-pre-workspace.json" \
    --slurpfile c0 "$T/visible-pre-cursor.json" \
    --argjson focus "$focus" -f "$checker"
}

result() {
  local status=$1 name=$2
  if [[ "$status" = ok ]]; then echo "PASS $name"; else
    echo "FAIL $name ($status)"; fails=$((fails + 1)); fi
  rm -rf -- "$T"
}

# Pass only on jq exit 0 and stdout exactly the expected token.
expect() {
  local name=$1 want=$2 focus=${3:-true} got rc
  set +e
  got=$(check "$focus")
  rc=$?
  set -e
  if [[ "$rc" -eq 0 && "$got" = "$want" ]]; then result ok "$name"
  else result "exit $rc, got '$got', want '$want'" "$name"; fi
}

# Pass only when the recipe wrapper would map the output to visible_check_error.
expect_error() {
  local name=$1 out rc
  set +e
  out=$(check true; echo "rc=$?")
  set -e
  rc=${out##*rc=}
  out=${out%rc=*}
  if [[ "$rc" -ne 0 || "$(printf '%s' "$out" | wc -l)" -ne 1 ]]; then
    result ok "$name"
  else result "exit $rc, output '$out' would be accepted" "$name"; fi
}

# Good case G: both windows on the pre-run active workspace; focus/cursor constant.
good() {
  T=$(mktemp -d)
  jq -n '{address: "0xaaa", title: "user window", workspace: {id: 1, name: "1"}}' \
    > "$T/visible-pre-active.json"
  jq -n '{id: 1, name: "1", hasfullscreen: false}' > "$T/visible-pre-workspace.json"
  jq -n '{x: 10, y: 20}' > "$T/visible-pre-cursor.json"
  jq -n '
    def win($t; $a): {initialTitle: $t, title: $t, address: $a, ws: 1, wsName: "1",
      at: [0, 0], size: [800, 600], fullscreen: 0};
    [range(3) | {utc: "2026-11-19T12:00:0\(.).000000000Z", active: 1, wsName: "1",
      address: "0xaaa", cursor: {x: 10, y: 20},
      clients: (if . == 0 then [] else
        [win("ac4248 control"; "0x111"), win("ac4248 target"; "0x222")] end)}]' \
    > "$T/samples.json"
  lines
}

# Write samples.json as one compact JSON line per sample (S2 shape).
lines() { jq -c '.[]' "$T/samples.json" > "$T/visible.samples"; }
# Apply a jq update to samples.json, then rewrite visible.samples.
samples() {
  local update=$1
  jq "$update" "$T/samples.json" > "$T/samples.tmp"
  mv -- "$T/samples.tmp" "$T/samples.json"
  lines
}

good; expect G ok
good; expect Gf ok false
good; samples '.[1].active = 2'; expect M1 visible_workspace_changed
good; samples '.[1].wsName = "2"'; expect M1b visible_workspace_changed
good; samples '.[1].clients[1].ws = 2'; expect M2 visible_placement_off_active_workspace
good; samples '.[2].address = "0x222"'; expect M3 visible_focus_taken_by_test
good; samples '.[1].address = "0x111"'; expect M3b visible_focus_taken_by_test
good; samples '.[1].address = "0xbbb"'; expect M4 visible_focus_changed
good; samples '.[1].address = ""'; expect M4b visible_focus_changed
good; samples '.[1].address = "0x222"'; expect F3 ok false
good; samples '.[1].address = "0xbbb"'; expect F4 ok false
good; samples '.[1].cursor.x = 11'; expect M5 visible_pointer_changed
good; samples '.[1].cursor.x = 11'; expect F5 visible_pointer_changed false
good; samples 'map(.clients = [])'; expect M6 visible_placement_unobserved
good; : > "$T/visible.samples"; expect M6b visible_placement_unobserved
good; printf '{"utc": \n' >> "$T/visible.samples"; expect_error B1
good; printf 'not json\n' > "$T/visible-pre-cursor.json"; expect_error B2

echo "failures $fails"
[[ "$fails" -eq 0 ]] || exit 1
