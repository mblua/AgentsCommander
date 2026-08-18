# The AgentsCommander RTK hook for Claude Code

For operators who want to know what a Claude Code agent actually ran, not what it reported. AgentsCommander seeds a `PreToolUse` hook into every workgroup replica; after this page you can read the two files it produces and say which shell command landed in which one, and why.

The hook and its registration are copied into this directory so you can review them without opening a replica:

- [`hooks/ac_rtk_claude.js`](hooks/ac_rtk_claude.js), 132 lines
- [`settings.local.json`](settings.local.json)

Both are faithful copies of `<workspace>/.ac/default.claude/`, the seed AC installs from. Do not edit them here: this directory is a mirror, not the source.

## What the hook is and where it lands

AC seeds `.ac/default.claude/` into each workgroup replica when it creates the replica. A replica ends up with:

```text
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/hooks/ac_rtk_claude.js
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/settings.local.json
```

`settings.local.json` registers the hook, and the whole registration is this:

```json
"hooks": {
  "PreToolUse": [
    {
      "matcher": "Bash",
      "hooks": [{ "type": "command", "command": "node .claude/hooks/ac_rtk_claude.js" }]
    }
  ]
}
```

Before every `Bash` tool call, Claude Code runs the hook with the tool input as JSON on stdin. The hook decides whether to rewrite the command so it runs through RTK, and answers with `permissionDecision: "allow"`.

Two files come out of that decision, both in the agent's **origin Agent Matrix**, not in the replica:

| File | Holds |
|---|---|
| `<workspace>/.ac/_agent_<name>/rtk-matrix-history.db` | The RTK history database, tables `commands` and `parse_failures`, written by `rtk` itself. See [RTK usage and per-agent statistics](../rtk.md). |
| `<workspace>/.ac/_agent_<name>/rtk_ignored_tools.md` | One line per command the hook handed back untouched, written by the hook. |

The pair exists so you can tell "RTK covered this command" from "RTK never saw it". Neither file alone answers that.

## This is not the hook `rtk init` installs

RTK ships its own Claude Code hook, and [RTK usage and per-agent statistics](../rtk.md#the-hook-covers-the-filtered-set-and-nothing-else) documents it. The two are separate pieces of software that happen to do related work:

| | AC hook | RTK hook |
|---|---|---|
| Command | `node .claude/hooks/ac_rtk_claude.js` | `rtk hook claude` |
| Installed by | AgentsCommander, when it seeds a replica | You, by running `rtk init` |
| Registered in | the replica's `.claude/settings.local.json` | the Claude Code settings you point `rtk init` at |
| Writes `rtk_ignored_tools.md` | yes | no |

Both are `PreToolUse` hooks and both route commands to RTK, so a machine can have both active at once. Only the AC hook produces `rtk_ignored_tools.md`, so that file is the one that tells you the AC hook ran.

## What it covers: the `Bash` tool, and nothing else

The registration has a single matcher, `Bash`. Every other tool the model calls goes straight to its implementation and reaches neither file:

`Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebFetch`, `Task`, every MCP tool, and any other shell tool the harness exposes, including `PowerShell`.

Take that seriously before you read either file as a record of the agent's work. An agent that splits its shell work between the `Bash` tool and the `PowerShell` tool leaves half of it out of both files, with nothing marking the gap.

Inside the `Bash` tool the coverage is complete: every non-empty command reaches one of the two files. The exceptions are the hook's two early exits, an unparseable JSON payload and a command that is empty after leading whitespace is stripped. Both leave no trace anywhere.

## The routing rule

A command is handed back untouched and written to `rtk_ignored_tools.md` when **both** of these hold:

1. `rtk rewrite "<command>"` prints nothing on stdout, and
2. the command is not safe to prefix, meaning any one of: it contains shell syntax (`\n ; | & < > ( ) { }` or a backtick), its first word contains `=` (the `FOO=bar cmd` form), or the shell resolves its first word to a `builtin`, `keyword`, `function` or `alias`.

If `rtk rewrite` prints nothing but the command **is** safe to prefix, the hook prefixes `rtk ` anyway and writes no log line. That fallback is what puts an unknown binary into the statistics.

Two details worth knowing:

- The hook keys on **stdout**, not on the exit code, because `rtk rewrite` returns a good rewrite with exit 3 for compound commands.
- There is no builtin list in the hook. It asks the shell with `type -t`, so the two never drift apart.

### The rule the informal description gets wrong

Being a compound command sends nothing to the ignored log. `rtk rewrite` splits on `;` and `&&` and handles `|` without help, and the hook rewrites all three. What pushes a command into the ignored log is a **redirection**, a subshell, a heredoc, command substitution, or a first word the shell owns.

Measured against the installed RTK, driving the hook the way Claude Code does:

| Command | Result |
|---|---|
| `ls -la` | rewritten to `rtk ls -la` |
| `cat file.txt` | rewritten to `rtk read file.txt` |
| `ls \| sort` | rewritten to `rtk ls \| sort` |
| `ls; cat x` | rewritten to `rtk ls; rtk read x` |
| `git status && ls` | rewritten to `rtk git status && rtk ls` |
| `FOO=bar ls` | rewritten to `FOO=bar rtk ls` |
| `node --version` | prefixed to `rtk node --version` by the fallback |
| `nosuchbinary-xyz` | prefixed to `rtk nosuchbinary-xyz` by the fallback |
| `echo hola` | **ignored log**, `echo` is a builtin |
| `cd /tmp && export X=1` | **ignored log**, builtins |
| `ls > /tmp/x` | **ignored log**, redirection |
| `ls \| sort > /tmp/now.txt` | **ignored log**, redirection, not the pipe |
| `(ls)` | **ignored log**, subshell |
| `echo "total=$(wc -l < f)"` | **ignored log**, command substitution |

Reproduce any row from inside a replica:

```bash
echo '{"tool_input":{"command":"ls -la"}}' | node .claude/hooks/ac_rtk_claude.js
```

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ac_rtk_claude","updatedInput":{"command":"exec 2> >(grep --line-buffered -v 'No hook installed' >&2)\nrtk ls -la"}}}
```

A passed-through command prints nothing and exits 0. Run one from a real replica and it appends a line to that agent's real `rtk_ignored_tools.md`.

The `exec 2> >(grep ...)` line prepended to every rewrite silences a known RTK false positive on stderr. It is a separate statement rather than a wrapper, so heredocs keep working and the command's exit code and remaining stderr survive.

**The list above is not a contract.** The hook delegates the decision to `rtk rewrite` and treats it as the source of truth. Which shapes `rtk rewrite` declines is a property of your installed RTK, so a new RTK version can move commands between the two files with no change to the hook. Re-measure after an RTK upgrade rather than trusting this table.

## `rtk_ignored_tools.md`

### Only workgroup replicas write it

The hook derives the target from its own location: two levels up from `.claude/hooks/` is the replica root, and the Matrix folder is the replica's name with one leading underscore dropped, so `__agent_foo` writes to `_agent_foo`.

It requires the replica root to start with `__`. An agent running directly in its Matrix, or anywhere else, writes no line at all, and nothing reports that. An empty or missing `rtk_ignored_tools.md` therefore means either "nothing was ignored" or "this agent never writes here", and the file cannot tell you which.

### Format

One line per entry, no header:

```text
20260818_110844: ls | sort > /tmp/now.txt
```

- The timestamp is **local time** in `YYYYMMDD_HHMMSS`, with no zone and no offset.
- The command is the **original**, not the rewrite.
- All whitespace collapses to single spaces, newlines included, so a heredoc stays one entry. A multi-line script therefore survives as one unindented line and is no longer runnable as written.
- The hook appends and does nothing else: no truncation, no escaping, no rotation, no deduplication, and no locking. Two live replicas of the same agent append to the same file.

### The timestamps do not line up with the database

`rtk_ignored_tools.md` stamps local time with no zone. The `timestamp` column of `commands` is RFC 3339 with an offset, as documented in [RTK usage and per-agent statistics](../rtk.md#the-commands-table). The two artifacts do not share a time format, so **you cannot correlate them by string comparison**. Convert one side before you line up a session across both files.

### An entry does not prove the command ran

The hook is a `PreToolUse` hook: it writes the line before the shell starts, and it never sees the command's exit code. If you deny the permission prompt afterwards, or the command dies, the line is already written and nothing corrects it. Read the file as "the model asked to run this and the hook declined to rewrite it".

## What reaches the database

The hook never touches the database. It reads no `RTK_DB_PATH`, knows nothing about SQLite, and only rewrites the command and returns `allow`. The rows are written by `rtk` when the rewritten command runs, into the `RTK_DB_PATH` of that session. [RTK usage and per-agent statistics](../rtk.md) covers how to point that variable at the agent's Matrix and how to read the results.

Which table a rewritten command lands in, measured against a scratch database:

| Invocation | `commands` | `parse_failures` |
|---|---|---|
| `rtk ls -la .` | yes | no |
| `rtk ls /noexiste`, exit 2 | yes | no |
| `rtk node --version`, via the fallback | yes, as `rtk fallback: node --version` | yes, `fallback_succeeded=1` |
| `rtk nosuchbinary-xyz`, exit 127 | no | yes, `fallback_succeeded=0` |
| `rtk read /noexiste`, exit 1 | no | no |

A failing command is recorded like a passing one: the first four rows above cover exits 0, 2, 0 and 127. The pattern is that the hook's `rtk ` fallback produces a `parse_failures` row, because RTK cannot parse its own argv and falls back to direct execution, and that execution adds a `commands` row only when the binary exists.

The last row is the exception, and it is RTK's behaviour rather than the hook's: a failing `rtk ls` is recorded and a failing `rtk read` is not. So do not read either table as a complete ledger of everything routed through RTK.

## Failure modes

### RTK missing from PATH breaks simple commands

This is the one to watch. The hook calls `rtk rewrite` through `spawnSync`, which does not throw when the binary is absent, it returns empty stdout. Empty stdout is the fallback path, so a plain command still comes back rewritten:

```bash
echo '{"tool_input":{"command":"ls -la"}}' | node .claude/hooks/ac_rtk_claude.js
```

With `rtk` off the PATH the hook still answers `rtk ls -la`, and that dies with exit 127 when it runs. Commands carrying shell syntax keep working, because they take the passthrough path. So losing RTK does not degrade tracking quietly, it breaks the agent's simple shell commands while the complicated ones keep running.

### An invalid `RTK_DB_PATH` loses the record silently

The hook ignores the variable entirely. The rewritten command runs, prints its normal output and exits 0, and the row is never written. Nothing on this side reports it. [RTK usage and per-agent statistics](../rtk.md#failure-modes) covers how that surfaces when you read the statistics.

### A failed write to the ignored log is silent

Writing the log line is wrapped in an empty `catch`. A missing Matrix directory, a locked file or a full disk costs you the line and nothing else. That is deliberate: the hook must not cost you the command you asked for.

### The hook never blocks a tool

Every path out of the hook is either a silent `exit(0)` or `permissionDecision: "allow"`. There is no `deny` and no blocking exit code, so a broken hook cannot stop an agent from working.

## See also

- [RTK usage and per-agent statistics](../rtk.md) - configuring `RTK_DB_PATH` per agent type and reading the database this hook feeds
- [Agent Matrix conventions](../../agent-matrix-conventions.md) - replica and Matrix layout, which is what the hook derives the log path from
- [Coding agents](../coding-agents.md) - the coding-agent catalog and its ENVIRONMENT rows
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
