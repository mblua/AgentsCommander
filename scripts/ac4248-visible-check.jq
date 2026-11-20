# #4248 recipe v2 visible-run checker (plan v3, S3). Prints exactly one token.
# Run: jq -rn --slurpfile s TAG.samples --slurpfile a0 TAG-pre-active.json
#   --slurpfile w0 TAG-pre-workspace.json --slurpfile c0 TAG-pre-cursor.json
#   --argjson focus true -f scripts/ac4248-visible-check.jq
# First matching row wins. Every token other than "ok" stops the run; the reader
# sets the class (O4). Addresses are compared as given (both sides from hyprctl -j).
($a0[0].address // "") as $B
| $w0[0] as $W
| ($c0[0] | {x, y}) as $C
| ([$s[] | .clients[] | .address] | unique) as $T
| if any($s[]; .active != $W.id or .wsName != $W.name) then "visible_workspace_changed"
  elif any($s[]; .active as $a | any(.clients[]; .ws != $a))
    then "visible_placement_off_active_workspace"
  elif $focus and any($s[]; .address != $B and (.address as $a | any($T[]; . == $a)))
    then "visible_focus_taken_by_test"
  elif $focus and any($s[]; .address != $B) then "visible_focus_changed"
  elif any($s[]; .cursor != $C) then "visible_pointer_changed"
  elif all($s[]; (.clients | length) == 0) then "visible_placement_unobserved"
  else "ok" end
