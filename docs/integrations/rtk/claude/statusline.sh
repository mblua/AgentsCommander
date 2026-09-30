#!/usr/bin/env bash
# Claude Code status line: cwd (branch) [model · effort] ctx% | 5h% | 7d%
input=$(cat)
dir=$(printf '%s' "$input" | jq -r '.workspace.current_dir')
dir=${dir%$'\r'}
branch=$(git -C "$dir" --no-optional-locks branch --show-current 2>/dev/null)
branch=${branch%$'\r'}
printf '%s' "$input" | jq -r --arg br "$branch" '
  def f(v): ((v // 0) | floor | tostring) + "%";
  (.workspace.current_dir | gsub("\\\\"; "/") | split("/") | last) as $cwd
  | (if $br == "" then "" else " (\($br))" end) as $b
  | (.model.display_name + (if .effort.level then " · \(.effort.level)" else "" end)) as $m
  | ("ctx " + f(.context_window.used_percentage)) as $c
  | (if .rate_limits.five_hour then " | 5h " + f(.rate_limits.five_hour.used_percentage) else "" end) as $h
  | (if .rate_limits.seven_day then " | 7d " + f(.rate_limits.seven_day.used_percentage) else "" end) as $w
  | "[2;36m\($cwd)\($b)[0m [2;33m[\($m)][0m [2m\($c)\($h)\($w)[0m"
' | tr -d '\r'
