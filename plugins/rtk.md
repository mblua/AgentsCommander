# RTK (Rust Token Killer): Plugin Notes

## What is RTK?

RTK is a CLI proxy installed on this machine that compresses verbose command outputs to reduce token consumption. It filters and condenses Bash tool output before it reaches the LLM context window.

- **Repo:** [https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk)
- RTK only compresses output from Bash tool calls, not native Claude Code tools (Read, Grep, Glob).
- If RTK has a dedicated filter for a command, it compresses the output. If not, it passes through unchanged. RTK is always safe to use.

## How AC integrates RTK

AC's plugin auto-injects the `PreToolUse` hook into every managed agent directory. You do **not** need to install a hook script, run `rtk init`, or edit your global `~/.claude/settings.json`. AC handles the wiring.

For each managed agent directory `<agent-dir>`, AC writes a single `PreToolUse.Bash` entry into `<agent-dir>/.claude/settings.local.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "node -e \"'@ac-rtk-marker-v2';…\"" }
        ]
      }
    ]
  }
}
```

The `command` is a self-contained `node -e "..."` one-liner: no companion shell script, no `jq` dependency, no file on disk. It reads Claude Code's tool input on stdin, prepends `rtk ` to any Bash invocation that does not already start with it (skipping shell built-ins like `cd`, `export`, `source`), and emits the rewritten command back to Claude Code in the v2 hook output schema (`hookSpecificOutput.updatedInput`).

The leading `'@ac-rtk-marker-v2'` string is a JS no-op identity marker. Every sweep matches on the marker substring, then decides whether to refresh, leave alone, or remove the hook, preserving any user-customized rewriter body across AC upgrades.

**Canonical source:** the full command string lives in `RTK_REWRITER_COMMAND` (`src-tauri/src/config/claude_settings.rs:52`) and is verified byte-identical to `repo-AgentsCommander/.claude/settings.json` by a source-of-truth test.

For the full user-facing description (startup probe, sweep modes, settings toggle, removal flow), see [`docs/features/rtk-integration.md`](../docs/features/rtk-integration.md).

## Setup for AC users

1. **Install RTK** so it's on `PATH`. Follow the instructions at [https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk).
2. **Verify:** `rtk --version` resolves from any terminal.
3. **Enable AC's auto-injection.** Restart AC; the sidebar banner offers to enable the hook. Or toggle directly under **Settings → General → RTK → Inject hook**.

From that point AC injects the hook into every managed agent at startup and applies it to new agents you create through the UI.

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.

## Optional: CLAUDE.md fallback instruction block

AC's auto-injected hook is what makes the integration work; the block below is **not** required. It is an optional manual fallback for two cases:

- You have AC's auto-injection disabled (Settings → General → RTK → Inject hook = off) but still want RTK behavior.
- You want to also remind the agent in-context, as belt-and-suspenders alongside the injected hook.

Add to the project's `CLAUDE.md`:

```markdown
<!-- rtk-instructions -->
## RTK (Token Optimizer)

`rtk` is a CLI proxy installed on this machine that compresses command outputs to reduce tokens.

**Rule:** ALWAYS prefix Bash commands with `rtk`. If RTK has a filter for that command, it compresses the output. If not, it passes through unchanged. It is always safe to use.

In command chains with &&, prefix each command:
rtk git add . && rtk git commit -m "msg" && rtk git push

Applies to: git, gh, cargo, npm, pnpm, npx, tsc, vitest, playwright, pytest, docker, kubectl, ls, grep, find, curl, and any other command.

Meta: `rtk gain` to view token savings statistics, `rtk discover` to find missed RTK usage opportunities.
<!-- /rtk-instructions -->
```

AC does **not** write this block into managed agents; it's purely user-authored. The injected `PreToolUse` hook is the source of truth for command rewriting; this block is just an in-context reminder.

## Notes

- The condensed CLAUDE.md block above (~200 tokens) is ~85% smaller than the full version `rtk init` ships (~1,400 tokens).
- The HTML comments (`<!-- rtk-instructions -->`) serve as markers to easily locate and update the block across repos.
- `rtk init --show` reports the current RTK configuration status for the repo.
- RTK version 0.31.0+ includes the rate-limited warning fix for Windows (PR #742).
