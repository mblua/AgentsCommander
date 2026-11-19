# #4248 AC-4248-010 off-workspace checker (plan v3, S4). Prints exactly one token.
# Run: jq -rn --slurpfile s off.samples --rawfile ev off.events
#   --slurpfile m0 monitors-before.json --slurpfile a0 active-before.json
#   --slurpfile c0 cursor-before.json -f scripts/ac4248-off-check.jq
# First matching row wins; any token other than "ok" is INCOMPLETE (harness).
def norm(a): (a // "") | ltrimstr("0x");
def ac4248: (. // "") | startswith("ac4248 ");
def holds($x): any(.[]; . == $x);
# Same text as the S2 sampler in scripts/ac4248-linux-p03.sh.
def mon:
  [.[] | {name, ws: .activeWorkspace.name, special: .specialWorkspace.name}] | sort_by(.name);
# Socket2 line "name>>data"; names are compared with == only.
def events:
  [$ev | split("\n")[] | select(length > 0) | index(">>") as $i
    | if $i == null then {name: ., data: ""} else {name: .[:$i], data: .[$i + 2:]} end];
# openwindow>>ADDR,WSNAME,CLASS,TITLE (TITLE may hold commas).
def opened: split(",") as $p
  | {addr: norm($p[0]), ws: ($p[1] // ""), title: ($p[3:] | join(","))};
def ours($e): ((.initialTitle // .title) | ac4248) or (norm(.address) as $a | $e | holds($a));
def shown: ["workspace", "workspacev2", "activespecial", "activespecialv2",
  "focusedmon", "focusedmonv2"];

events as $evs
| ($evs | map(select(.name == "openwindow") | .data | opened)) as $opens
| ($opens | map(select(.title | ac4248) | .addr) | unique) as $E
| ([$s[] | .clients[] | select(ours($E)) | norm(.address)] + $E | unique) as $A
| norm($a0[0].address) as $B
| ($m0[0] | mon | map({name, ws})) as $M0
| if any($opens[]; ((.addr as $a | $A | holds($a)) or (.title | ac4248))
      and .ws != "special:ac4248") then "off_placement_open_elsewhere"
  elif any($evs[]; (.name == "movewindow" or .name == "movewindowv2")
      and (norm(.data | split(",")[0]) as $a | $A | holds($a))) then "off_placement_elsewhere"
  elif any($s[]; any(.clients[]; ours($E) and .wsName != "special:ac4248"))
    then "off_placement_elsewhere"
  elif ($M0 | length) == 0
      or any($s[]; ((.monitors // []) | sort_by(.name) | map({name, ws})) != $M0)
      or any($s[]; any((.monitors // [])[]; .special == "special:ac4248"))
    then "off_special_or_workspace_shown"
  elif any($evs[]; .name as $n | shown | holds($n)) then "off_special_or_workspace_shown"
  elif any($s[]; norm(.address) != $B)
      or any($evs[]; .name == "activewindowv2" and (norm(.data) as $d | $d == "" or $d != $B))
    then "off_focus_changed"
  elif any($s[]; .cursor != ($c0[0] | {x, y})) then "off_pointer_changed"
  elif ($E | length) < 2
      or (any($s[]; [.clients[] | select(ours($E)) | norm(.address)] as $c
        | all($E[]; . as $e | $c | holds($e))) | not)
    then "off_placement_unobserved"
  else "ok" end
