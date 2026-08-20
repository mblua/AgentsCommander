# RTK usage and per-agent statistics

For developers who already run RTK and want to know what each agent type actually executes. After this page, every session records its RTK history into its own agent's database, and one command tells you how much each agent type ran and saved.

[RTK](https://github.com/rtk-ai/rtk) (Rust Token Killer, Apache-2.0) is an optional third-party CLI proxy. It wraps common developer commands, compresses their output before it reaches a coding agent's context, and records every invocation in a SQLite history database.

AgentsCommander does not ship, install, update or require RTK, and no AC feature depends on it. What AC contributes is one existing mechanism: it expands `%AC_*%` path placeholders in a coding agent's environment values at spawn time, which is enough to give each agent type its own RTK database.

With `RTK_DB_PATH` unset, RTK writes every invocation on the machine into one database (`%LOCALAPPDATA%\rtk\history.db` on Windows). Its `commands` table has no agent identity column, so that shared database cannot answer "what does each agent type run?".

## Recommended configuration

Open **Settings → Coding Agents**, pick the coding agent your agents launch, and add this row under **ENVIRONMENT**:

```text
RTK_DB_PATH = %AC_MATRIX_ROOT%\rtk-matrix-history.db
```

Save, then start a new session for an agent that uses that coding agent. AC expands the placeholder at spawn, so the child process receives an absolute path such as `D:\myproject\.ac\_agent_tech-lead\rtk-matrix-history.db`.

RTK creates the file on the first wrapped command in that session. Verify it:

```bash
ls -l "<project>/.ac/_agent_<name>/rtk-matrix-history.db"
```

Once that agent has run its first `rtk ...` command, the command exits 0 and prints one line:

```text
-rw-r--r-- 1 <user> 197609 49152 Aug 18 00:15 <project>/.ac/_agent_<name>/rtk-matrix-history.db
```

Repeat the row for every coding agent whose sessions you want to measure. ENVIRONMENT rows belong to one coding agent, not to the whole installation.

## Why this value

| Property | What it buys you |
|---|---|
| `%AC_MATRIX_ROOT%` | Resolves to `<project>\.ac\_agent_<name>`, the agent's canonical Agent Matrix. Statistics aggregate **per agent type**, across every workgroup replica that agent ever ran in. |
| Absolute after expansion | RTK opens `RTK_DB_PATH` exactly as given. A relative value resolves against the session's current working directory, which changes as the agent moves between `repo-*` checkouts, so the history scatters into several files. |
| Outside `wg-*` | Workgroup directories are disposable. A database under the Agent Matrix survives a workgroup purge. |

The `project_path` column keeps the second dimension. Every row records the working directory the command ran in, so one agent's database still tells you which replica or repository checkout each command came from.

## Make your agents use RTK

A per-agent database records only what runs through RTK. Whatever an agent executes directly leaves no row, and `rtk gain` reports the smaller total with no warning, so an unenforced setup produces statistics that understate the work by an unknown margin. The configuration above is half the job; the other half is making the usage happen.

Force it from two sides, and use both: a rule in the agent's instructions, and RTK's rewrite hook in the coding agent's configuration.

### The rule: prefix every command with `rtk`

Put this in the agent's role or in the global policy of the installation, wherever your agents read their standing instructions from:

> Prefix every shell command with `rtk`. Always, with no exceptions.

The rule carries no branches on purpose. A rule the agent has to reason about is a rule the agent drops under load, and prefixing an unsupported command costs nothing:

- A command RTK has a filter for, such as `git status`, runs through the filter and reaches the agent's context compressed.
- Any other command runs raw and is recorded anyway.

Confirm that fallback yourself. The commands below were verified against rtk 0.42.4:

```bash
rtk whoami
rtk gain -F
```

`rtk whoami` prints your user name, and the parse-failure report accounts for the invocation:

```text
RTK Parse Failures
════════════════════════════════════════════════════════════

Total failures:    1
Recovery rate:     100.0%
```

The failure counter rises with each unrecognized command, and a recovery rate of 100% means every one of them still ran. Each one lands in `commands` with `rtk_cmd` set to `rtk fallback: <command>` and with `input_tokens` and `savings_pct` at zero, so the marker doubles as a list of the commands your agents run that have no filter yet:

```sql
SELECT original_cmd, COUNT(*) FROM commands
WHERE rtk_cmd LIKE 'rtk fallback:%'
GROUP BY original_cmd ORDER BY 2 DESC;
```

The wrapped command's own exit code does not change any of this. A command that fails is recorded the same way as one that succeeds.

### The hook covers the filtered set, and nothing else

This section is about RTK's own hook, the one `rtk init` installs. AgentsCommander seeds **different** hooks into every workgroup replica: a `PreToolUse` hook for the `Bash` tool and one for the `PowerShell` tool, which also write a log of the commands they declined to rewrite, and a third registered on both `PreToolUse` and `PostToolUse` for the native file tools, which writes its own rows straight into the database. If you run agents in replicas, read [The AgentsCommander RTK hook for Claude Code](rtk_claude/README.md) before you conclude which hook produced what.

`rtk init` installs both halves of the adoption problem: it writes RTK's instructions into the coding agent's context file, and it patches the agent's configuration with a `PreToolUse` hook that rewrites commands before they run. `--no-patch` skips the patching and prints manual instructions instead. This is the excerpt a patched Claude Code `settings.json` holds under `hooks.PreToolUse`, once per shell tool, not a complete settings file:

```json
{
  "matcher": "Bash",
  "hooks": [{ "type": "command", "command": "rtk hook claude" }]
}
```

With that hook active, a command the agent wrote as `ls -al` reaches RTK as `rtk ls -al`. The hook rewrites only what RTK has a filter for. Ask it directly:

```bash
rtk hook check "git status"
rtk hook check "hostname"
```

```text
rtk git status
No rewrite for: hostname
```

The first exits 0 with the rewritten command. The second exits 1 and rewrites nothing, so an unprefixed `hostname` runs outside RTK and leaves no row at all. That gap is what the prompt rule closes: the hook covers the filtered set at no cost to the agent, and the agent's own `rtk` prefix covers everything else.

Pick the target agent with `rtk init --agent <name>`, and run `rtk init --help` for the targets your version supports. Preview before you accept:

```bash
rtk init --dry-run
```

```text
[dry-run] would add rtk instructions to CLAUDE.md
[dry-run] would create .rtk/filters.toml template: .rtk\filters.toml

[dry-run] Nothing written.
```

The hook belongs to the coding agent's configuration, not to AgentsCommander. AC contributes the `RTK_DB_PATH` row; which database the rewritten command writes to still comes from the session environment.

### Interactive commands stay interactive

A command that needs a terminal fails when an agent runs it, with or without the prefix:

```text
Error: stdin is not a terminal
```

That message comes from the wrapped command, not from RTK. Running the same command without the `rtk` prefix fails identically, and RTK records the invocation either way. Keep interactive tools in a human terminal, and keep the prefix rule for everything the agent runs.

## Reading the statistics

Every command below reads the database named by `RTK_DB_PATH`. Run them inside a session of the agent you want to inspect, or set `RTK_DB_PATH` in your own shell first. Verified against rtk 0.42.4.

### One agent

```bash
rtk gain
```

First lines:

```text
RTK Token Savings (Global Scope)
════════════════════════════════════════════════════════════

Total commands:    14
```

| Flag | Effect |
|---|---|
| `-H` | Appends a **Recent Commands** list to the text report. |
| `-p` | Restricts the report to commands whose `project_path` is the current working directory. |
| `-d` / `-w` / `-m` | Daily, weekly or monthly breakdown. |
| `-a` | All three breakdowns at once. |
| `-g` | Adds an ASCII graph of daily savings. The per-command impact table is already in the report without it. |
| `-q` | Estimates what share of a monthly subscription quota the savings preserve. `-t pro\|5x\|20x` picks the tier, default `20x`. |
| `-F` | Prints the parse-failure log. |
| `-f json` / `-f csv` | Machine-readable output. See below: both depend on the breakdown flags. |
| `--reset` | **Permanently deletes every row** of `commands` and `parse_failures`. RTK confirms with `This will permanently delete all tracking data.` before it does; `--yes` skips the prompt. |

`rtk gain -f json` prints the summary object and nothing else:

```json
{
  "summary": {
    "total_commands": 14,
    "total_input": 5804,
    "total_output": 5140,
    "total_saved": 664,
    "avg_savings_pct": 11.44038594073053,
    "total_time_ms": 231,
    "avg_time_ms": 16
  }
}
```

Ask for a breakdown and the same object gains a key: `-d` adds `daily`, `-w` adds `weekly`, `-m` adds `monthly`, and `-a` adds all three. `-H` and `-p` add no key.

`rtk gain -f csv` follows the same rule, and its empty case is the one that surprises people. With no breakdown flag, and with `-H` or `-p`, it prints zero bytes and exits 0: there is no CSV rendering of the summary. Ask for a breakdown and it prints well-formed CSV:

```bash
rtk gain -f csv -d
```

```text
# Daily Data
date,commands,input_tokens,output_tokens,saved_tokens,savings_pct,total_time_ms,avg_time_ms
2026-08-18,3,413,117,296,71.67,103,34
```

`-a` prints the daily, weekly and monthly sections in one stream, each behind its own `# ...` comment line, so a consumer has to split on those lines. Neither format exposes individual command rows. For those, read the database.

### The `commands` table

```sql
CREATE TABLE commands (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    original_cmd TEXT NOT NULL,
    rtk_cmd TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    saved_tokens INTEGER NOT NULL,
    savings_pct REAL NOT NULL,
    exec_time_ms INTEGER DEFAULT 0,
    project_path TEXT DEFAULT ''
);
```

`timestamp` is RFC 3339 with an offset. On Windows, `project_path` carries the verbatim prefix `\\?\`, so strip that prefix before you compare paths.

The same file holds `parse_failures (id, timestamp, raw_command, error_message, fallback_succeeded)`, the log of commands that fell back to raw execution.

### All agents at once

Each agent has its own file, so aggregation is a loop over `_agent_*`. Save this as `rtk-per-agent.py` and run it with Python 3.9 or later, which `str.removeprefix` requires:

```python
import glob, os, sqlite3

pattern = r"<project>\.ac\_agent_*\rtk-matrix-history.db"
rows = []
for db in glob.glob(pattern):
    agent = os.path.basename(os.path.dirname(db)).removeprefix("_agent_")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    count, saved = con.execute(
        "SELECT COUNT(*), COALESCE(SUM(saved_tokens), 0) FROM commands"
    ).fetchone()
    con.close()
    rows.append((agent, count, saved))

for agent, count, saved in sorted(rows, key=lambda row: -row[2]):
    print(f"{agent:<20}{count:>8}{saved:>12}")
```

Output:

```text
tech-lead                 15        1519
architect                 22        1210
```

To break one agent down by replica or repository checkout, swap the query for `SELECT project_path, COUNT(*), COALESCE(SUM(saved_tokens), 0) FROM commands GROUP BY project_path`.

Always open the databases read-only, as `?mode=ro` above does. A live session may be writing to one of them.

## Failure modes

### A working relative path splits the history

`RTK_DB_PATH=rtk-history.db`, with no placeholder and no absolute path, is a valid configuration that nothing rejects. RTK opens the value as given, so it resolves against the session's current working directory, and the agent creates one database per directory it works in.

Both of these exit 0 and print their normal filtered output:

```bash
cd <project>/repo-one && rtk wc README.md
cd <project>/repo-two && rtk wc README.md
```

Each directory now holds its own `rtk-history.db`, and `rtk gain` reports the one it finds in the directory you run it from:

```text
RTK Token Savings (Global Scope)
════════════════════════════════════════════════════════════

Total commands:    1
```

The header reads **Global Scope** and the number reads like an answer. It is the count for one directory. This is the failure mode to watch for, because the other two announce themselves and this one never does: an agent that moves between `repo-*` checkouts scatters its history across all of them, and every report you read is a fraction whose size you cannot see.

Use `%AC_MATRIX_ROOT%`, or a literal absolute path, so every session of that agent writes to one file.

### A relative prefix loses tracking silently

A leftover `.\` in front of the placeholder, as in `.\%AC_MATRIX_ROOT%\rtk-matrix-history.db`, expands into an invalid path. AC does not reject the value, and **every wrapped command stays completely silent about it**: RTK filters the output as usual, stdout is intact, stderr is empty, the command exits 0, and no row is recorded. Nothing on the writing side tells you the history is being dropped.

The message appears on the reading side. `rtk gain` refuses to open the database and exits **1**:

```text
rtk: Failed to initialize tracking database: The filename, directory name, or volume label syntax is incorrect. (os error 123)
```

So the symptom is not a low command count, it is `rtk gain` failing outright. When you see that line, open the ENVIRONMENT row and confirm the value starts with `%AC_MATRIX_ROOT%` and nothing else.

### A Windows environment variable blocks the spawn

`%LOCALAPPDATA%`, `%USERPROFILE%` and every other non-AC `%WORD%` marker is rejected fail-closed. AC hands the value to the child process without a shell, so nothing would expand it, and the session refuses to start:

```text
Agent '<label>' env settings: unknown placeholder marker in value
```

Only `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%` and `%AC_MATRIX_ROOT%` are recognized. Use one of those, or a literal absolute path.

## When `%AC_MATRIX_ROOT%` does not resolve

`%AC_MATRIX_ROOT%` resolves only for a workgroup replica launch root, that is a `__agent_*` directory under a `wg-*` workgroup. A session launched from a `repo-*` checkout, a bare `wg-*` directory, an `_agent_*` matrix directory or the root agent fails at launch with:

```text
%AC_MATRIX_ROOT% requires an AC workgroup replica launch root
```

For those roots use `%AC_WORKSPACE_ROOT%\rtk-history.db` instead, which gives one database per project with no per-agent split, or a literal absolute path. `%AC_WORKSPACE_ROOT%` resolves for any launch root inside a `.ac` workspace; the root agent resolves only `%AC_REPLICA_ROOT%`.

## See also

- [Agent Matrix conventions §5](../agent-matrix-conventions.md#5-profile-path-placeholders) - the three path placeholders, where each one resolves, and the fail-closed rules
- [The AgentsCommander RTK hook for Claude Code](rtk_claude/README.md) - the hook AC seeds into every replica, and the two files it produces
- [Coding agents](coding-agents.md) - the coding-agent catalog and the ENVIRONMENT rows this page uses
- [Settings reference - coding agents](../reference/settings.md#coding-agents) - the `agents[].envs` shape behind the ENVIRONMENT panel
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
