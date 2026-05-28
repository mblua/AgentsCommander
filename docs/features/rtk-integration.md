# RTK integration

For developers running Bash-heavy agent workflows who want to cut LLM token consumption sharply. AgentsCommander auto-wires [RTK (Rust Token Killer)](https://github.com/rtk-ai/rtk) into managed Claude Code agent directories on startup.

## What RTK does

RTK is a CLI proxy that compresses verbose command outputs before they reach the LLM context window. Run `rtk <command>` instead of `<command>` and RTK strips redundant blank lines, filters known noise patterns, and condenses lengthy lists into summaries. If RTK has a filter for the command, it compresses the output. If it does not, it passes through unchanged. **RTK is always safe to use.**

Typical savings on common operations (per the RTK project):

| Category | Typical savings |
|---|---|
| Tests (`vitest`, `cargo test`, `playwright`) | 90–99% |
| Build / lint (`tsc`, `prettier`, `next build`) | 70–87% |
| Git (`status`, `log`, `diff`) | 59–80% |
| GitHub CLI (`gh pr`, `gh run`) | 26–87% |
| Package managers (`pnpm`, `npm`, `npx`) | 70–90% |
| Files (`ls`, `find`, `grep`) | 60–75% |
| Network (`curl`, `wget`) | 65–70% |

Overall: **60–90% token reduction** on typical Bash-tool workloads.

Project home: [https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk).

## Why AC integrates it

Claude Code's Bash tool emits the full command output back into the model's context. On big repos this can saturate the context window in minutes. RTK fixes that at the source, but every agent directory needs the `PreToolUse` hook registered. AC automates the registration so a single setting switches RTK on for every managed agent.

## What AC does automatically

On every AC startup AC probes `PATH` for the `rtk` binary and runs one of these branches (`src-tauri/src/lib.rs:382-473`):

| `rtk` on PATH? | `injectRtkHook` setting | `rtkPromptDismissed` | Behavior |
|---|---|---|---|
| Yes | `false` | `false` | Emits `rtk_startup_status mode=prompt-enable` — the sidebar banner offers to enable injection. |
| Yes | `true` | any | Mode `active`. Sweeps every managed agent dir and (re)writes the `.claude/settings.local.json` inside each managed agent directory with the RTK `PreToolUse` hook. |
| No | `true` | any | Mode `auto-disabled`. Persists `injectRtkHook=false`, then sweeps to remove any stale hooks. |
| No | `false` | any | Mode `silent`. No-op. |

The sweep loop is idempotent and serialized under an internal lock so concurrent settings updates cannot race against it.

## What gets written

For each managed agent directory, AC writes (or removes) the following block in `<agent-dir>/.claude/settings.local.json` (the per-agent settings file under each managed agent directory in the matrix — **not** the user's global `~/.claude/`):

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

The `command` is a self-contained `node -e "..."` one-liner — no external script, no companion file on disk. The snippet reads Claude Code's tool input on stdin, prepends `rtk ` to any Bash invocation that does not already start with it (skipping shell built-ins like `cd`, `export`, `source`), and emits the rewritten command back to Claude Code in the v2 hook output schema (`hookSpecificOutput.updatedInput`).

The leading `'@ac-rtk-marker-v2'` string literal is a JS no-op that AC uses as an identity marker. Every sweep matches on the marker substring, then decides whether to refresh, leave alone, or remove the hook — which preserves any user-customized rewriter body across AC upgrades. The canonical command text lives in `RTK_REWRITER_COMMAND` (`src-tauri/src/config/claude_settings.rs:52`) and is verified byte-identical to `repo-AgentsCommander/.claude/settings.json` by a source-of-truth test.

You do not have to change agent prompts, install any rewriter script, or run any RTK init command. AC injects everything Claude Code needs.

## Installing RTK separately

AC does not bundle RTK; you install it yourself once:

1. Follow the RTK installation instructions at [https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk).
2. Verify it is on PATH: `rtk --version`.
3. Restart AC. The sidebar banner appears offering to enable the hook injection. Click **Enable**.

From that point on, AC injects the hook into every managed agent directory at startup. New agents you create through the UI inherit the hook automatically.

## Turning the integration off

**Settings → General → RTK → Inject hook** disables the integration. On the next startup AC sweeps every managed agent directory and removes the hook block from `.claude/settings.local.json`. No leftover files.

## Scope

The integration today covers **Claude Code** only. RTK works with other agents in principle, but the hook protocol (`PreToolUse`) is Claude Code's. Generalising the hook injection to Codex and Gemini is on the [roadmap](../../ROADMAP.md) under "AC Harness — deterministic command execution".

## Credit

RTK is built and maintained by the team at [https://github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk). AgentsCommander ships only the integration glue (`src-tauri/src/lib.rs`, `src-tauri/src/config/claude_settings.rs`, `plugins/rtk.md`). All compression logic and savings come from RTK itself.

## See also

- [RTK upstream](https://github.com/rtk-ai/rtk)
- [`README.md` — Acknowledgments](../../README.md#acknowledgments)
- [`plugins/rtk.md`](../../plugins/rtk.md) — the in-repo RTK plugin notes
