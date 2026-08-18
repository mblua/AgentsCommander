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

Exits 0 and prints one line once that agent has run its first `rtk ...` command.

Repeat the row for every coding agent whose sessions you want to measure. ENVIRONMENT rows belong to one coding agent, not to the whole installation.

## Why this value

| Property | What it buys you |
|---|---|
| `%AC_MATRIX_ROOT%` | Resolves to `<project>\.ac\_agent_<name>`, the agent's canonical Agent Matrix. Statistics aggregate **per agent type**, across every workgroup replica that agent ever ran in. |
| Absolute after expansion | RTK opens `RTK_DB_PATH` exactly as given. A relative value resolves against the session's current working directory, which changes as the agent moves between `repo-*` checkouts, so the history scatters into several files. |
| Outside `wg-*` | Workgroup directories are disposable. A database under the Agent Matrix survives a workgroup purge. |

The `project_path` column keeps the second dimension. Every row records the working directory the command ran in, so one agent's database still tells you which replica or repository checkout each command came from.

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
| `-F` | Prints the parse-failure log. |
| `-f json` | Prints one JSON object. |

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

`-H`, `-d`, `-w` and `-m` add nothing to that JSON. `rtk gain -f csv` prints zero bytes and exits 0. Do not build a pipeline on either flag. For anything past the summary, read the database.

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

Each agent has its own file, so aggregation is a loop over `_agent_*`. Save this as `rtk-per-agent.py` and run it with any Python 3:

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

### A relative prefix loses tracking silently

A leftover `.\` in front of the placeholder, as in `.\%AC_MATRIX_ROOT%\rtk-matrix-history.db`, expands into an invalid path. RTK reports:

```text
Failed to initialize tracking database ... (os error 123)
```

and then **runs the wrapped command anyway and exits 0**. Nothing else fails and no row is recorded, so the loss stays invisible until you read the statistics. If `rtk gain` reports far fewer commands than the agent ran, open the ENVIRONMENT row and confirm the value starts with `%AC_MATRIX_ROOT%` and nothing else.

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
- [Coding agents](coding-agents.md) - the coding-agent catalog and the ENVIRONMENT rows this page uses
- [Settings reference - coding agents](../reference/settings.md#coding-agents) - the `agents[].envs` shape behind the ENVIRONMENT panel
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
