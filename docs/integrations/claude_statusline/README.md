# Claude Code status line

Install [`statusline.sh`](statusline.sh) so Claude Code shows context usage and subscription limits, and AgentsCommander can display the remaining seven-day quota.

## Install

Copy the script byte-for-byte to `<workspace>/.ac/default.claude/statusline.sh`. With Claude's Config folder set to `.claude`, AC copies this workspace template into each replica as `.claude/statusline.sh` on spawn, unless a higher-priority profile or OS template wins. See [config seeding](../../features/config-seed.md) for precedence.

Merge this entry into `<workspace>/.ac/default.claude/settings.local.json`, preserving your other settings:

```json
{
  "statusLine": {
    "type": "command",
    "command": "bash \"${CLAUDE_PROJECT_DIR:-.}/.claude/statusline.sh\""
  }
}
```

This matches [the RTK settings mirror](../rtk_claude/settings.local.json). For an existing replica, copy the script into its `.claude/` folder and merge the entry into its `.claude/settings.local.json`, or respawn it to apply the selected template. Updating a staging copy alone does not update the workspace template or running replicas.

## Output and quota

Ignoring ANSI colors, a populated status line looks like:

```text
project (main) [Claude · high] ctx 10% | 5h 3% | 7d 28%
```

The `7d N%` segment reports **used** quota from `rate_limits.seven_day.used_percentage`. Its format lets AC find the Claude subscription's remaining seven-day quota: AC's default Claude pattern, `CLAUDE_WEEKLY_QUOTA_REGEX`, captures the used percentage, and the quota display computes `100 - used`. Thus `7d 28%` means 72% remaining.

The pattern in [profile-utils.ts](../../../src/shared/profile-utils.ts) is:

```regex
(?:^|[ |])7d (\d{1,3})%
```

The conversion is in [agent-quota.ts](../../../src/sidebar/components/agent-quota.ts). If Claude omits the seven-day limit, the script omits the segment; it does not invent a quota reading.

## Dependencies and behavior

The `statusline.sh` script requires Bash and Node, already used by the hooks; it does not require jq. Git is optional: a missing Git executable, non-repository workspace or detached HEAD omits the branch. Git lookup has a two-second timeout and uses the supplied workspace, never the invocation directory.

The script reads JSON from stdin and shows the workspace directory name, branch, model, optional effort, context usage, and present five-hour and seven-day limits. It preserves ANSI colors, floors percentages without clamping, strips carriage returns and emits one newline. A trailing workspace slash deliberately gives an empty directory name. Empty-string and zero effort remain visible; absent, null or false effort and limit sections are omitted. Absent, null or false percentages display 0%.

Incomplete objects such as `{}` now fall back to empty directory/model and `ctx 0%`. Empty limit objects display 0%, as before; numeric zero and empty-string limit sections now also display 0%, whereas the old jq implementation errored on those section types. Malformed or empty JSON and incompatible field types produce empty stdout, one diagnostic line on stderr and a nonzero exit.
