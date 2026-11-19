#!/usr/bin/env bash
# #4248 AC-4248-010 offline proof of scripts/ac4248-off-check.jq (plan v3, S6). No GUI.
# Run: bash scripts/ac4248-off-check-test.sh   (exit 0 only when every case passes)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
checker="$here/ac4248-off-check.jq"
regress="$here/../target/ac4248-evidence/gui-20261119-072306"
mon='[.[] | {name, ws: .activeWorkspace.name, special: .specialWorkspace.name}] | sort_by(.name)'
fails=0
T=

# S4 command, as run by the recipe.
check() {
  jq -rn --slurpfile s "$T/off.samples" --rawfile ev "$T/off.events" \
    --slurpfile m0 "$T/monitors-before.json" --slurpfile a0 "$T/active-before.json" \
    --slurpfile c0 "$T/cursor-before.json" -f "$checker"
}

# Pass only on jq exit 0 and stdout exactly the expected token.
expect() {
  local name=$1 want=$2 got rc
  set +e
  got=$(check)
  rc=$?
  set -e
  if test "$rc" -eq 0 && test "$got" = "$want"; then
    echo "PASS $name"
  else
    echo "FAIL $name (exit $rc, got '$got', want '$want')"
    fails=$((fails + 1))
  fi
  rm -rf -- "$T"
}

# Good case G: both windows on special:ac4248, focus/cursor/monitors constant.
good() {
  T=$(mktemp -d)
  # Raw hyprctl -j monitors shape, reverse name order (proves the sort).
  jq -n '[{name: "eDP-1", activeWorkspace: {id: 1, name: "1"},
      specialWorkspace: {id: 0, name: ""}},
    {name: "HDMI-A-1", activeWorkspace: {id: 11, name: "11"},
      specialWorkspace: {id: 0, name: ""}}]' > "$T/monitors-before.json"
  jq -n '{address: "0xaaa", title: "user window"}' > "$T/active-before.json"
  jq -n '{x: 10, y: 20}' > "$T/cursor-before.json"
  jq -n --argjson m "$(jq -c "$mon" "$T/monitors-before.json")" '
    def win($t; $a): {initialTitle: $t, title: $t, address: $a, ws: -98,
      wsName: "special:ac4248"};
    [range(3) | {utc: "2026-11-19T07:05:0\(.).000000000Z", active: 1, address: "0xaaa",
      cursor: {x: 10, y: 20}, monitors: $m,
      clients: [win("ac4248 control"; "0x111"), win("ac4248 target"; "0x222")]}]' \
    > "$T/samples.json"
  printf '%s\n' 'createworkspace>>special:ac4248' 'createworkspacev2>>-98,special:ac4248' \
    'openwindow>>111,special:ac4248,agentscommander,ac4248 control' \
    'openwindow>>222,special:ac4248,agentscommander,ac4248 target' \
    'windowtitle>>222' 'windowtitlev2>>222,ac4248 target' \
    'openwindow>>bbb,1,firefox,other, with comma' \
    'closewindow>>111' 'closewindow>>222' \
    'destroyworkspace>>special:ac4248' 'destroyworkspacev2>>-98,special:ac4248' \
    > "$T/off.events"
  lines
}

# Write samples.json as one compact JSON line per sample (S2 shape).
lines() { jq -c '.[]' "$T/samples.json" > "$T/off.samples"; }
# Apply a jq update to samples.json, then rewrite off.samples.
samples() {
  jq "$1" "$T/samples.json" > "$T/samples.tmp"
  mv -- "$T/samples.tmp" "$T/samples.json"
  lines
}
event() { printf '%s\n' "$1" >> "$T/off.events"; }

good; expect G ok
good; sed -i 's/^openwindow>>222,special:ac4248,/openwindow>>222,1,/' "$T/off.events"
expect M1 off_placement_open_elsewhere
good; samples '.[1].clients[1].wsName = "1"'; expect M2 off_placement_elsewhere
good; samples '.[1].monitors[0].special = "special:ac4248"'
expect M3 off_special_or_workspace_shown
good; samples '.[1].monitors = []'; expect M3b off_special_or_workspace_shown
good; samples '(.[1].monitors[] | select(.name == "HDMI-A-1") | .ws) = "12"'
expect M4 off_special_or_workspace_shown
good; event 'activespecial>>special:ac4248,eDP-1'; expect M5 off_special_or_workspace_shown
good; event 'workspacev2>>2,2'; expect M5b off_special_or_workspace_shown
good; samples '.[1].address = "0x222"'; expect M6 off_focus_changed
good; event 'activewindowv2>>222'; expect M7 off_focus_changed
good; event 'activewindowv2>>'; expect M7a off_focus_changed
good; event 'activewindowv2>>aaa'; expect M7b ok
good; samples '.[1].cursor.x = 11'; expect M8 off_pointer_changed
good; sed -i '/>>222/d' "$T/off.events"; samples 'map(.clients |= .[0:1])'
expect M9 off_placement_unobserved
good; : > "$T/off.samples"; expect M10 off_placement_unobserved
good; event 'movewindowv2>>222,1,1'; expect M11 off_placement_elsewhere
good; event 'movewindow>>111,special:ac4248'; expect M11b off_placement_elsewhere

# S6.R: the real AC-4248-010 run (target on workspace 1) must give off_placement_elsewhere.
# Runs as an `if` condition, where errexit is off: every step checks its own status.
regression() {
  local n
  good
  : > "$T/off.events"
  sed -E 's/^([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:.]+Z)$/"\1"/' "$regress/off.samples" \
    | jq -s . > "$T/records.json" || return 1
  n=$(jq length "$T/records.json") || return 1
  test "$n" -gt 0 || return 1
  test $((n % 5)) -eq 0 || return 1
  # Groups of 5: utc, activewindow, cursorpos, activeworkspace, clients -> S2 shape.
  jq --argjson m "$(jq -c "$mon" "$T/monitors-before.json")" '
    . as $r | [range(0; length; 5) as $i | $r[$i:$i + 5]
    | {utc: .[0], active: .[3].id, address: (.[1].address // ""), cursor: (.[2] | {x, y}),
      monitors: $m,
      clients: [.[4][] | select((.initialTitle // .title // "") | startswith("ac4248 "))
        | {initialTitle, title, address, ws: .workspace.id, wsName: .workspace.name}]}]' \
    "$T/records.json" > "$T/samples.json" || return 1
  lines || return 1
  cp -- "$regress/active-before.json" "$regress/cursor-before.json" "$T/" || return 1
  echo "regression records=$n samples=$((n / 5))"
}
if test ! -d "$regress"; then
  echo "SKIP regression"
elif regression; then
  expect S6.R off_placement_elsewhere
else
  echo "FAIL S6.R (regression input)"
  rm -rf -- "$T"
  fails=$((fails + 1))
fi

echo "failures $fails"
test "$fails" -eq 0 || exit 1
