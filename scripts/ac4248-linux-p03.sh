# P03 Linux recipe (Hyprland + WebKitGTK), versioned with the linux_snapshot harness.
# v2 = visible-only: comment 6813009311 + AC-4248-008 D1-D5; plan v3 (.ac-relative path):
#   project-shared/4248-testable-mode-linux/plan-v3-ac-4248-p03-recipe-v2-visible.md
# Run: REPO=<abs> E=<new ignored dir> bash -euo pipefail "$REPO/scripts/ac4248-linux-p03.sh" \
#   > "$E/run.log" 2>&1; echo $? > "$E/run.exit"
set -euo pipefail
cd "$REPO"
sha256sum "$REPO/scripts/ac4248-linux-p03.sh" > "$E/recipe-sha256.txt"
rustc +1.98.1 --version > "$E/rustc.txt"
GATE_FOCUS=true  # D1: no_initial_focus rule on, so checker rows 3-4 gate focus.
sample_visible() {
  while :; do
    jq -cn --arg utc "$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)" \
      --argjson ws "$(hyprctl -j activeworkspace)" --argjson aw "$(hyprctl -j activewindow)" \
      --argjson cur "$(hyprctl -j cursorpos)" --argjson cl "$(hyprctl -j clients)" \
      '{utc: $utc, active: $ws.id, wsName: $ws.name, address: ($aw.address // ""),
        cursor: ($cur | {x, y}), clients: [$cl[]
        | select(((.initialTitle // .title) // "") | startswith("ac4248 "))
        | {initialTitle, title, address, ws: .workspace.id, wsName: .workspace.name,
          at, size, fullscreen}]}'
    sleep 0.1
  done
}
readout() { printf '%s\n' "$*" >> "$E/ac4248-008-readout.txt"; }
incomplete() {
  local tag=$1 reason=$2
  printf '%s\n' "$reason" > "$E/$tag.incomplete"
  test ! -e "$E/ac4248-008-readout.txt" || readout "incomplete $tag $reason"
  echo "INCOMPLETE $reason" >&2; exit 2
}
write_baseline_readout() {
  local h
  if test "$(cat "$E/visible.exit")" -eq 0; then h=stall_not_reproduced
  elif test -s "$E/visible/receipt.json"; then
    h=$(jq -r '.timeoutStage.hypothesis // "none"' "$E/visible/receipt.json")
  else h=visible_receipt_missing; fi
  : > "$E/ac4248-008-readout.txt"; readout "baseline $h"
}
# Without D1 focus is not gated; the reader gets the count of changed samples.
focus_readout() {
  local tag=$1
  test "$GATE_FOCUS" = true || readout "$tag focus_samples_changed=$(jq -rn \
    --slurpfile s "$E/$tag.samples" --slurpfile a0 "$E/$tag-pre-active.json" \
    '[$s[] | select(.address != ($a0[0].address // ""))] | length')"
}
visible_run() {
  local tag=$1 token code rc sampler; shift
  hyprctl -j activewindow > "$E/$tag-pre-active.json"
  hyprctl -j activeworkspace > "$E/$tag-pre-workspace.json"
  hyprctl -j cursorpos > "$E/$tag-pre-cursor.json"
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
  hyprctl -j activewindow > "$E/$tag-post-active.json"
  hyprctl -j cursorpos > "$E/$tag-post-cursor.json"
  hyprctl -j activeworkspace > "$E/$tag-post-workspace.json"
  sleep 0.3
  set -e
  printf '%s\n' "$code" > "$E/$tag.exit"
  test ! -s "$E/$tag.sample-errors" || incomplete "$tag" visible_sampler_error
  set +e
  jq -rn --slurpfile s "$E/$tag.samples" --slurpfile a0 "$E/$tag-pre-active.json" \
    --slurpfile w0 "$E/$tag-pre-workspace.json" --slurpfile c0 "$E/$tag-pre-cursor.json" \
    --argjson focus "$GATE_FOCUS" -f "$REPO/scripts/ac4248-visible-check.jq" \
    > "$E/$tag-check.token"
  rc=$?
  set -e
  test "$rc" -eq 0 && test "$(wc -l < "$E/$tag-check.token")" -eq 1 \
    || incomplete "$tag" visible_check_error
  token=$(cat "$E/$tag-check.token")
  test "$token" = ok || incomplete "$tag" "$token"
  test "$tag" = visible || focus_readout "$tag"
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
# Trace logs must be on in each visible run; page console lines are a readout only.
check_trace() {
  local tag=$1
  rg -q 'link=load' "$E/$tag.stderr" || incomplete "$tag" trace_logs_missing
  readout "pagetrace $(cat "$E/$tag.stdout" "$E/$tag.stderr" | rg -c \
    'CONSOLE LOG ac4248trace ' || echo 0)"
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
  if test -f "$E/vis-rule.stdout" && test ! -f "$E/vis-rule-off.stdout"; then
    hyprctl eval "ac4248_vis_rule:set_enabled(false)" > "$E/vis-rule-off.stdout" \
      2> "$E/vis-rule-off.stderr"
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
  probe_names_match_fixture stall_probe_steps frame_script_shape link_break_tokens \
  trace_target_matches_recipe rejected_saved_once checker_hidpi_accepted \
  checker_mutations_rejected; do rg "$name: test" "$E/example-tests.list"; done
cargo +1.98.1 test --locked -p agentscommander --example linux_snapshot \
  > "$E/example-tests.log" 2>&1
bash "$REPO/scripts/ac4248-visible-check-test.sh" > "$E/visible-check-test.log" 2>&1 \
  || incomplete visible visible_check_selftest
git rev-parse HEAD > "$E/source-sha.txt"
hyprctl version > "$E/hyprland.txt"
pkg-config --modversion webkit2gtk-4.1 > "$E/webkit.txt"
export RUST_LOG=ac4248::link=trace
vis_rule="ac4248_vis_rule = hl.window_rule({ name = 'ac4248-visible',"
vis_rule+=" match = { initial_title = '^ac4248 (control|target)\$' }, no_initial_focus = true })"
hyprctl eval "$vis_rule" > "$E/vis-rule.stdout" 2> "$E/vis-rule.stderr" || true
test "$(cat "$E/vis-rule.stdout")" = ok && test ! -s "$E/vis-rule.stderr" \
  || incomplete visible vis_rule_not_registered
visible_run visible
write_baseline_readout
focus_readout visible
readout "link $(jq -r '.timeoutStage.linkBreak // "none"' "$E/visible/receipt.json" \
  2>/dev/null || echo visible_receipt_missing)"
check_trace visible
visible_run stall-module --probe stall-module
check_trace stall-module
visible_run stall-raf --probe stall-raf
check_trace stall-raf
check_stall_probes
test "$(cat "$E/visible.exit")" -eq 0
jq -e '.verdict == "pass"' "$E/visible/receipt.json"
sha256sum --check "$E/binary-sha256.txt"
for probe in blank stale cancel; do
  case "$probe" in
    blank) expected=20; token=fixture_pixels_missing; taken=true; reached=true ;;
    stale) expected=21; token=fixture_frame_stale; taken=true; reached=true ;;
    cancel) expected=22; token=snapshot_cancelled; taken=false; reached=false ;;
    *) ;;
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
hyprctl eval "ac4248_vis_rule:set_enabled(false)" > "$E/vis-rule-off.stdout" \
  2> "$E/vis-rule-off.stderr" || true
test "$(cat "$E/vis-rule-off.stdout")" = ok && test ! -s "$E/vis-rule-off.stderr" \
  || incomplete visible vis_rule_not_removed
readout "v2 gates_complete"
