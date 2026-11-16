# #4248 P03 Linux recipe (Hyprland + WebKitGTK), versioned with the linux_snapshot harness.
# Source: issue comment 6813009311 (three blocks, in order) plus AC-4248-008 deltas D1-D5.
# Run: REPO=<abs> E=<new ignored dir> bash -euo pipefail "$REPO/scripts/ac4248-linux-p03.sh" \
#   > "$E/run.log" 2>&1; echo $? > "$E/run.exit"
set -euo pipefail
cd "$REPO"
sha256sum "$REPO/scripts/ac4248-linux-p03.sh" > "$E/recipe-sha256.txt"
rustc +1.98.1 --version > "$E/rustc.txt"
sample_visible() {
  while :; do
    jq -cn --arg utc "$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)" \
      --argjson ws "$(hyprctl -j activeworkspace)" --argjson cl "$(hyprctl -j clients)" \
      '{utc: $utc, active: $ws.id, clients: [$cl[]
        | select(((.initialTitle // .title) // "") | startswith("ac4248 "))
        | {title, ws: .workspace.id, wsName: .workspace.name, at, size, fullscreen}]}'
    sleep 0.1
  done
}
readout() { printf '%s\n' "$*" >> "$E/ac4248-008-readout.txt"; }
incomplete() {
  printf '%s\n' "$2" > "$E/$1.incomplete"
  test ! -e "$E/ac4248-008-readout.txt" || readout "incomplete $1 $2"
  echo "INCOMPLETE $2" >&2; exit 2
}
write_baseline_readout() {
  local h
  if test "$(cat "$E/visible.exit")" -eq 0; then h=stall_not_reproduced
  elif test -s "$E/visible/receipt.json"; then
    h=$(jq -r '.timeoutStage.hypothesis // "none"' "$E/visible/receipt.json")
  else h=visible_receipt_missing; fi
  : > "$E/ac4248-008-readout.txt"; readout "baseline $h"
}
visible_run() {
  local tag=$1 token code sampler; shift
  hyprctl -j activewindow > "$E/$tag-pre-active.json"
  hyprctl -j activeworkspace > "$E/$tag-pre-workspace.json"
  token=$(jq -rn --slurpfile a "$E/$tag-pre-active.json" \
    --slurpfile w "$E/$tag-pre-workspace.json" '$a[0] as $a | $w[0] as $w |
    if ($a.address // "") == "" then "visible_precondition_no_focus"
    elif $a.workspace.id < 1 then "visible_precondition_special_workspace"
    elif $a.workspace.id != $w.id then "visible_precondition_inactive_workspace"
    elif $w.hasfullscreen != false then "visible_precondition_fullscreen"
    else "ok" end')
  test "$token" = ok || incomplete "$tag" "$token"
  (set -eu; sample_visible) > "$E/$tag.samples" 2> "$E/$tag.sample-errors" &
  sampler=$!
  set +e
  timeout --kill-after=5s 45s target/debug/examples/linux_snapshot \
    --fixture "$REPO/target/ac4248-fixture/index.html" --out "$E/$tag" \
    --placement visible "$@" > "$E/$tag.stdout" 2> "$E/$tag.stderr"
  code=$?
  kill "$sampler"; wait "$sampler"
  sleep 0.3
  set -e
  printf '%s\n' "$code" > "$E/$tag.exit"
  test ! -s "$E/$tag.sample-errors" || incomplete "$tag" visible_sampler_error
  token=$(jq -rs 'if any(.[]; . as $s | $s.active < 1
      or any($s.clients[]; .ws != $s.active)) then "visible_placement_off_active_workspace"
    elif all(.[]; (.clients | length) == 0) then "visible_placement_unobserved"
    else "ok" end' "$E/$tag.samples")
  test "$token" = ok || incomplete "$tag" "$token"
}
check_stall_probes() {
  local p tag
  for p in stall-module:H-a stall-raf:H-b; do
    tag=${p%%:*}
    test -s "$E/$tag/receipt.json" || incomplete "$tag" stall_probe_receipt_missing
    jq -e --arg h "${p#*:}" '.verdict == "incomplete" and .reason == "deadline_30s"
      and .timeoutStage.diagnostics.status == "ok" and .timeoutStage.hypothesis == $h' \
      "$E/$tag/receipt.json" > /dev/null || incomplete "$tag" "${tag//-/_}_not_${p#*:}"
  done
  readout detector_verified
}
node --version
npm --version
npm ci --ignore-scripts
[[ "$REPO" =~ ^/[A-Za-z0-9_./-]+$ ]]
git check-ignore -q target/ac4248-evidence/run/receipt.json
app_data="${XDG_DATA_HOME:-$HOME/.local/share}/dev.agentscommander.app"
app_inventory() {
  if test ! -e "$app_data"; then printf 'absent\n'; return; fi
  (cd "$app_data"; find . -printf '%y %P %s %T@ %l\n' | LC_ALL=C sort
   find . -type f -exec sha256sum {} + | LC_ALL=C sort)
}
app_inventory > "$E/app-data-before.txt"
finish() {
  code=$?; trap - EXIT; set +e
  if test -f "$E/active-before.json"; then
    hyprctl -j activewindow > "$E/active-after.json"
    hyprctl -j cursorpos > "$E/cursor-after.json"
    hyprctl -j activeworkspace > "$E/workspace-after.json"
  fi
  app_inventory > "$E/app-data-after.txt"
  cmp "$E/app-data-before.txt" "$E/app-data-after.txt" > "$E/app-data.diff" 2>&1
  same=$?; printf '%s\n' "$same" > "$E/app-data.diff-exit"
  if test "$same" -ne 0; then code=2; fi
  printf '%s\n' "$code" > "$E/script.exit"
  exit "$code"
}
trap finish EXIT
npm run build
npx --no-install vite build --config scripts/ac4248-fixture.vite.mjs
git check-ignore -q dist/ac4248/index.html
test ! -e dist/ac4248
cp -R target/ac4248-fixture dist/ac4248
cmp target/ac4248-fixture/index.html dist/ac4248/index.html
find dist/ac4248 -type f -exec sha256sum {} + > "$E/embedded-assets.sha256"
cargo +1.98.1 build --locked -p agentscommander --example linux_snapshot \
  --features tauri/custom-protocol
stat -c '%y %s %n' target/debug/examples/linux_snapshot > "$E/binary-stat.txt"
sha256sum target/debug/examples/linux_snapshot > "$E/binary-sha256.txt"
# Cleanup only this run's fixture copy while its bytes remain unchanged.
find dist/ac4248 -type f -exec sha256sum {} + > "$E/assets-before-clean.sha256"
cmp "$E/embedded-assets.sha256" "$E/assets-before-clean.sha256"
diff -qr target/ac4248-fixture dist/ac4248
rm -r -- dist/ac4248
cargo +1.98.1 test --locked -p agentscommander --example linux_snapshot -- --list \
  > "$E/example-tests.list" 2>&1
for name in image_bounds encoded_size_limit receipt_no_clobber late_generation \
  blank_rejected stale_rejected title_log_bounded probe_unanswered read_probe_unparsed \
  h_a_signature fixture_inconsistent_shapes frame_not_called h_b_signature h_c_signature \
  probe_names_match_fixture stall_probe_steps; do rg "$name: test" "$E/example-tests.list"; done
cargo +1.98.1 test --locked -p agentscommander --example linux_snapshot \
  > "$E/example-tests.log" 2>&1
git rev-parse HEAD > "$E/source-sha.txt"
hyprctl version > "$E/hyprland.txt"
pkg-config --modversion webkit2gtk-4.1 > "$E/webkit.txt"
visible_run visible
write_baseline_readout
visible_run stall-module --probe stall-module
visible_run stall-raf --probe stall-raf
check_stall_probes
test "$(cat "$E/visible.exit")" -eq 0
jq -e '.verdict == "pass"' "$E/visible/receipt.json"
timeout --kill-after=5s 45s bash -c '
  set -eu
  while :; do
    date -u +%Y-%m-%dT%H:%M:%S.%NZ
    hyprctl -j activewindow
    hyprctl -j cursorpos
    hyprctl -j activeworkspace
    hyprctl -j clients
    sleep 0.1
  done
' > "$E/off.samples" 2> "$E/off.sample-errors" &
sampler_pid=$!
hyprctl -j activewindow > "$E/active-before.json"
hyprctl -j cursorpos > "$E/cursor-before.json"
hyprctl -j activeworkspace > "$E/workspace-before.json"
# Timeout/streams/exit receipt belong inside the Lua-launched command.
cat > "$E/off-run.sh" <<'RUN'
#!/usr/bin/env bash
set +e
timeout --kill-after=5s 45s "$REPO/target/debug/examples/linux_snapshot" \
  --fixture "$REPO/target/ac4248-fixture/index.html" \
  --out "$E/off-workspace" --placement off-workspace \
  > "$E/off.stdout" 2> "$E/off.stderr"
code=$?
printf '%s\n' "$code" > "$E/off.exit.tmp"
mv -- "$E/off.exit.tmp" "$E/off.exit"
exit "$code"
RUN
# Explicit environment survives compositor launch; paths validated above.
off_cmd="env REPO=$REPO E=$E bash $E/off-run.sh"
hyprctl eval "hl.exec_cmd('$off_cmd', { workspace = 'special:ac4248 silent' })" \
  > "$E/lua.stdout" 2> "$E/lua.stderr"
set +e
wait "$sampler_pid"
sampler_exit=$?
set -e
printf '%s\n' "$sampler_exit" > "$E/sampler.exit"
# Wrapper can finish after sampler; bounded wait preserves missing-exit failure.
timeout 12s bash -c 'until test -f "$E/off.exit"; do sleep 0.1; done'
hyprctl -j activewindow > "$E/active-after.json"
hyprctl -j cursorpos > "$E/cursor-after.json"
hyprctl -j activeworkspace > "$E/workspace-after.json"
jq -S '{address}' "$E/active-before.json" > "$E/focus-before.json"
jq -S '{address}' "$E/active-after.json" > "$E/focus-after.json"
jq -S '{x,y}' "$E/cursor-before.json" > "$E/pointer-before.json"
jq -S '{x,y}' "$E/cursor-after.json" > "$E/pointer-after.json"
# Retain differences for classification, never short-circuit through errexit.
set +e
cmp "$E/focus-before.json" "$E/focus-after.json" > "$E/focus.diff" 2>&1
printf '%s\n' "$?" > "$E/focus.diff-exit"
cmp "$E/pointer-before.json" "$E/pointer-after.json" > "$E/pointer.diff" 2>&1
printf '%s\n' "$?" > "$E/pointer.diff-exit"
set -e
test "$sampler_exit" -eq 124
test ! -s "$E/off.sample-errors"
test "$(cat "$E/off.exit")" -eq 0
jq -e '.verdict == "pass"' "$E/off-workspace/receipt.json"
sha256sum --check "$E/binary-sha256.txt"
for probe in blank stale cancel; do
  case "$probe" in
    blank) expected=20; token=fixture_pixels_missing; taken=true; reached=true ;;
    stale) expected=21; token=fixture_frame_stale; taken=true; reached=true ;;
    cancel) expected=22; token=snapshot_cancelled; taken=false; reached=false ;;
  esac
  visible_run "probe-$probe" --probe "$probe"
  code=$(cat "$E/probe-$probe.exit")
  test "$code" -eq "$expected"
  rg -F "$token" "$E/probe-$probe.stderr"
  jq -e --arg p "$probe" --arg t "$token" --argjson s "$taken" \
    --argjson v "$reached" '.verdict == "probe_rejected" and .probe == $p and
      .error == $t and .scriptExecuted == true and .snapshotTaken == $s and
      .validatorReached == $v and .acceptedPngs == 0' "$E/probe-$probe/receipt.json"
  test -z "$(find "$E/probe-$probe" -type f -iname '*.png' -print)"
done
