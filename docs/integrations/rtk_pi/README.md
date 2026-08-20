# The AgentsCommander RTK integration for pi

For operators who want to know what a pi agent actually ran, not what it reported. AgentsCommander seeds one pi extension and one wrapper script into every workgroup replica; the extension intercepts pi's `tool_call` events, rewrites commands so they run through RTK, and registers file reads and writes. After this page you can read the two files they produce and say which tool call landed in which one, and why.

The two seeded files are copied into this directory so you can review them without opening a replica:

- [`extensions/tool-hook.ts`](extensions/tool-hook.ts), 294 lines, the pi extension (`tool_call` event)
- [`rtk/ac-rtk.sh`](rtk/ac-rtk.sh), 317 bytes, the RTK launcher that strips the banner at the source

Both are faithful copies of `<workspace>/.ac/default.pi/`, the seed AC installs from. Do not edit them here: this directory is a mirror, not the source.

There is no registration file to mirror. The Claude Code integration needs `settings.local.json` to register its hooks; pi has no equivalent because it auto-discovers project-local extensions: any `.pi/extensions/*.ts` file is loaded automatically (Extension Locations in the pi extensions documentation). The wrapper needs no registration either: the extension resolves it by path at load time (see [What the extension prepends, and what the wrapper does](#what-the-extension-prepends-and-what-the-wrapper-does)).

## What the integration is and where it lands

AC seeds `.ac/default.pi/` into each workgroup replica when it creates the replica. A replica ends up with:

```text
<workspace>/.ac/<wg-N-name>/__agent_<name>/.pi/extensions/tool-hook.ts
<workspace>/.ac/<wg-N-name>/__agent_<name>/.pi/rtk/ac-rtk.sh
```

pi loads `tool-hook.ts` because it sits in `.pi/extensions/` (project-local, loaded only after the project is trusted). Before every tool call, pi fires the `tool_call` event and the extension decides what to do with the call. It mutates `event.input.command` for shell commands it routes, and pi executes the mutated input. It never blocks: no path out of the handler denies a call.

Two files come out of that decision, both in the agent's **origin Agent Matrix**, not in the replica:

| File | Holds |
|---|---|
| `<workspace>/.ac/_agent_<name>/rtk-matrix-history-pi.db` | The RTK history database, tables `commands` and `parse_failures`, written by `rtk` itself. See [RTK usage and per-agent statistics](../rtk.md). Note the `-pi` suffix: Claude Code agents write `rtk-matrix-history.db`, pi agents write `rtk-matrix-history-pi.db`, so the two never collide in the same Matrix. |
| `<workspace>/.ac/_agent_<name>/rtk-ignored-tools-pi.md` | One line per command a hook handed back untouched, plus one line per `write`/`edit` tool call, written by the extension. |

The pair exists so you can tell "RTK covered this call" from "RTK never saw it". Neither file alone answers that, and a call missing from both is not proof it never happened: see [what they cover](#what-they-cover-the-four-pi-tools-and-nothing-else).

## This is not the extension `rtk init` installs

RTK ships its own hook machinery, and [RTK usage and per-agent statistics](../rtk.md#the-hook-covers-the-filtered-set-and-nothing-else) documents it. The two are separate pieces of software that happen to do related work. Measured against `rtk` 0.42.4, there are two halves to RTK's own machinery:

- `rtk hook` subprocess processors, for `claude`, `cursor`, `gemini`, `copilot` and `droid`. There is no `hook` processor for pi, because pi's extension mechanism does not run subprocess hooks.
- `rtk init --agent pi`, which **does** target pi: it creates `.pi/extensions/rtk.ts`, a delegating pi extension that rewrites `bash` commands through `rtk rewrite` and nothing else.

The AC extension and RTK's own pi extension can coexist in the same replica; both register a `tool_call` handler and both mutate `event.input.command` for bash. They differ in what they do with the rewrite and what they record:

| | AC pi extension | `rtk init --agent pi` extension |
|---|---|---|
| File | `.pi/extensions/tool-hook.ts`, seeded by AgentsCommander | `.pi/extensions/rtk.ts`, created by `rtk init --agent pi` |
| Installed by | AgentsCommander, when it seeds a replica | You, by running `rtk init --agent pi` |
| Rewrites `bash` through `rtk rewrite` | yes | yes |
| Runs rewrites through the banner-stripping launcher | yes | no, plain `rtk` on PATH |
| Prefix fallback for unknown binaries | yes | no, exit 1 from `rtk rewrite` passes the command through |
| Already-routed `rtk ...` commands | rewritten to the launcher | left untouched |
| Compound commands | each `rtk <sub>` segment routed through the launcher | the whole command replaced by the rewrite output |
| PowerShell rule | yes, when `shellPath` points at pwsh | no |
| Writes `rtk-ignored-tools-pi.md` | yes | no |
| Registers `read` / `write` / `edit` | yes | no |
| Version gate | none | disabled below `rtk` 0.23.0, or when `RTK_DISABLED=1` |

RTK's own extension is a thin delegator: it asks `rtk rewrite`, mutates the command when stdout is non-empty and the exit code is 0 or 3, and fails open on any error. It keeps no record of what it routed and no record of what it passed through. The AC extension is the one that writes `rtk-ignored-tools-pi.md`, so that file is the one that tells you an AC extension ran. Both extensions on one machine is a supported arrangement, but expect double handling: both handlers see the same `tool_call`, and the second one to run sees the first one's mutation (later `tool_call` handlers see mutations made by earlier handlers).

## What they cover: the four pi tools, and nothing else

pi exposes exactly four tools, verified in the pi README: `read`, `write`, `edit` and `bash`. The extension's handler (`tool-hook.ts:268-294`) covers all four, and every other tool name returns early:

- `bash` is wrapped or rewritten (the routing rules below).
- `read` is registered alongside the native read: the extension spawns `rtk read <path>` as a side effect and pi's own read proceeds untouched (see [The read rule](#the-read-rule)).
- `write` and `edit` append a `Write:` / `Edit:` line with the target path to the ignored log (see [The write and edit rule](#the-write-and-edit-rule)).
- everything else, including any future custom tool pi exposes, is ignored by the handler's early returns.

Two consequences of pi's four-tool set are worth stating, because they differ from Claude Code:

- There are no `Grep` or `Glob` tools in pi. A model that would call `Grep` under Claude Code runs `grep` inside `bash` under pi, and that invocation is covered by the bash rule like any other.
- The read, write and edit coverage is the pi port's addition. Claude Code's hooks never see file tools, so file traffic is invisible in `rtk_ignored_tools.md` there. The pi extension registers reads into the database and writes and edits into the ignored log, so the two artifacts cover every one of pi's four tools, not just the shell.

Take the timing seriously before you read either file as a record of the agent's work. `tool_call` fires before the tool executes, so both artifacts are written pre-flight: the ignored log line and the `rtk read` spawn happen before the actual tool runs, and neither ever sees an exit code. See [An entry does not prove the command ran](#an-entry-does-not-prove-the-command-ran).

## The routing rules

There is one rule per shell, chosen at load time by `configuredShell` (`tool-hook.ts:245-263`), which reads `shellPath` from the project `.pi/settings.json` and then `~/.pi/agent/settings.json`. The default is the `bash` rule; the `powershell` rule applies only when a settings file points `shellPath` at `pwsh`/`powershell`. Read the one for the shell your session runs.

### The `bash` rule

A command is decided in this order, exactly as `decideBash` (`tool-hook.ts:164-173`):

1. **Already routed** when it starts with `rtk ` (`/^rtk\s/`): the command gets the FILTER prepended and its leading `rtk` replaced with the launcher path, then runs.
2. Otherwise `rtk rewrite "<command>"` is asked. **Non-empty stdout** means the rewrite is the routed command: FILTER + `routeRtk` output.
3. Otherwise, if the command is `prefixable` (`tool-hook.ts:151-162`, a single plain invocation whose head the shell does not resolve to a `builtin`, `keyword`, `function` or `alias`, asked with `type -t`), it gets FILTER + `rtk ` prefix anyway. That fallback is what puts an unknown binary into the statistics.
4. Otherwise the command is handed back untouched and written to `rtk-ignored-tools-pi.md`.

The FILTER (`tool-hook.ts:146`) is this line, prepended as its own statement so heredocs keep working and the command's exit code and remaining stderr survive:

```bash
exec 2> >(grep --line-buffered -v 'No hook installed' >&2)
```

`routeRtk` (`tool-hook.ts:78-127`) rewrites the segments `rtk rewrite` produced: every `rtk <sub> ...` segment is routed through the launcher, whole separator runs (`&&`, `||`, `;`, `|`) are consumed so compound syntax survives, and quoted regions are masked so an `rtk` inside a string literal is never rewritten (verified: `grep -n 'rtk' f.txt` routes exactly one segment, and the quoted `'rtk'` argument is left alone).

Three details worth knowing:

- The rule keys on **stdout**, not on the exit code, because `rtk rewrite` returns a good rewrite with exit 3 for compound commands (measured below). Exit 1 means "nothing to rewrite", and the character test decides.
- There is no builtin list in the extension. It asks the shell with `type -t`, so the two never drift apart.
- When `rtk rewrite` comes back empty, the character test is **textual, not syntactic**. Any of `\n ; | & < > ( ) { }` or a backtick counts wherever it appears, quoted or not. Quoting changes what the shell will do with the character, never whether the extension counts it. So a logged line means the extension handed the command back untouched, not that the shell did nothing interesting with it.

Measured against the installed RTK (`rtk` 0.42.4, Git Bash, 2026-08-20), driving the extension the way pi does, through a harness that calls the real `tool-hook.ts`:

| Command | `rtk rewrite` prints | Result |
|---|---|---|
| `ls -la /tmp` | `rtk ls -la /tmp`, exit 0 | rewritten: FILTER + launcher `ls -la /tmp` |
| `cat file.txt` | `rtk read file.txt`, exit 3 | rewritten |
| `ls \| sort` | `rtk ls \| sort`, exit 3 | rewritten, the pipe is never consulted |
| `ls; cat x` | `rtk ls; rtk read x`, exit 3 | rewritten |
| `git status && ls` | `rtk git status && rtk ls`, exit 3 | rewritten |
| `FOO=bar ls` | `FOO=bar rtk ls`, exit 3 | rewritten, the `=` in the first word is never reached |
| `grep foo f.txt 2>/dev/null` | `rtk grep foo f.txt 2>/dev/null`, exit 3 | rewritten, and the `>` is never reached |
| `grep -n 'rtk' f.txt` | `rtk grep -n 'rtk' f.txt`, exit 3 | rewritten, the quoted `rtk` is not re-routed |
| `node --version` | nothing, exit 1 | prefixed to launcher `node --version`, nothing in the character class |
| `nosuchbinary-xyz` | nothing, exit 1 | prefixed to launcher `nosuchbinary-xyz` |
| `echo hi` | nothing, exit 1 | **ignored log**, `echo` is a builtin |
| `cd /tmp && export X=1` | nothing, exit 1 | **ignored log**, on `&` and on a builtin head |
| `ls \| sort > /tmp/now.txt` | nothing, exit 1 | **ignored log**, on `\|` and `>` |
| `ls > /tmp/x` | nothing, exit 1 | **ignored log**, on `>` |
| `(ls)` | nothing, exit 1 | **ignored log**, on `(` and `)` |
| `echo "total=$(wc -l < f)"` | nothing, exit 1 | **ignored log**, on several, and a builtin head |
| `python -c "print(1)"` | nothing, exit 1 | **ignored log**, on the quoted `(` |
| `nosuchbinary-xyz "a;b"` | nothing, exit 1 | **ignored log**, on the quoted `;` |

The pairs that differ by one thing are the easiest way to see the rule:

| Pair | `rtk rewrite` | Result |
|---|---|---|
| `grep '(foo)' f.txt` | `rtk grep '(foo)' f.txt` | rewritten, the quoted `(` never gets looked at |
| `python -c "print(1)"` | empty | **ignored log**, on the same quoted `(` |
| `nosuchbinary-xyz` | empty | prefixed to `rtk nosuchbinary-xyz` |
| `nosuchbinary-xyz \| sort` | empty | **ignored log**, and the pipe is the only difference |

The first pair carries a parenthesis to both outcomes, which is why no list of shell constructs can describe this correctly. When you find a `Bash:` command in `rtk-ignored-tools-pi.md`, do not go looking for a redirection. Look for any of those characters anywhere in the line, quoted or not, or an `=` in the first word, or a first word the shell owns.

### The `PowerShell` rule (dormant on the default shell)

The rule used when `shellPath` points at `pwsh`/`powershell` is the Claude Code PowerShell hook ported: it asks the safety question **first**, then asks RTK (`decidePwsh`, `tool-hook.ts:233`). The order is inverted on purpose, and the reason is the same as on the Claude page: under PowerShell `ls`, `ps` and `diff` are aliases, so a rewrite swaps an object stream for a text stream, and a wrong rewrite silently breaks the agent's command where a missed rewrite costs one statistic. Nothing is prepended under this rule, so `$?` and redirected bytes behave natively.

The safety question is asked of the PowerShell parser and of `Get-Command` through a probe (`headIsExternal`), never of a character class kept in the extension. The command clears the gate when it is a single statement, that statement is a single pipeline, the pipeline's first element is a plain command invocation with no call operator, its name is a bare name containing no `=`, and that name resolves to `Application` or does not resolve at all. Anything that goes wrong reads as false: fail closed, the cost is one statistic.

Under the default Git Bash configuration this rule is never active: `configuredShell` returns `bash`, the `bash` rule decides, and logged lines carry the `Bash:` tool field. The `PowerShell:` field appears only when a session actually runs pwsh.

### Reproduce any row

The mirrored `tool-hook.ts` **cannot be exercised by typing at a shell**. It is a pi extension: pi calls its default export with an `ExtensionAPI` and the extension registers a `tool_call` handler that mutates `event.input` in place. To drive it the way pi does, load it with a minimal harness that captures the handler and feeds it events. Node 24 runs the TypeScript directly (type stripping); older Node needs `--experimental-strip-types`.

```js
import { pathToFileURL } from "node:url";
const mod = await import(pathToFileURL("./extensions/tool-hook.ts").href);
let h; const pi = { on: (e, f) => { if (e === "tool_call") h = f; } };
mod.default(pi);
const ev = { toolName: "bash", input: { command: "ls -la" } };
h(ev);
console.log(ev.input.command);
```

```text
exec 2> >(grep --line-buffered -v 'No hook installed' >&2)
D:/<workspace>/<wg-N-name>/__agent_<name>/.pi/rtk/ac-rtk.sh ls -la
```

Run it from a scratch replica layout, not from a real one: the extension derives `LOG_FILE` and the launcher path from its own location, so where you copy it decides where the log line and the `rtk read` spawn go (see [the log derivation](#only-workgroup-replicas-write-it)). Set `RTK_DB_PATH` to a scratch file before the run if you want to watch the database rows land. A passed-through command leaves `event.input.command` untouched and appends a line to the derived log.

### What the extension prepends, and what the wrapper does

Every routed command gets the FILTER line prepended, as a separate statement, so heredocs keep working and the command's exit code and remaining stderr survive. That is the same trade the Claude Bash hook makes.

The difference from Claude Code is the wrapper. In pi the `rtk` launcher itself strips the banner, because the FILTER alone only owns fd2:

- The FILTER greps the command's **stderr** for `No hook installed`. It cannot help when a caller-side merge (`2>&1`) pushes rtk's stderr into stdout.
- `ac-rtk.sh` strips the banner at rtk's **own stderr**, before any merge can happen.

Measured, `rtk ls /tmp > out 2>&1` contains the banner once; `ac-rtk.sh ls /tmp > out 2>&1` contains it zero times. The wrapper is transparent otherwise: measured with `ac-rtk.sh ls /definitely-nope`, the error is rtk's and the exit code is rtk's own, exit 2.

The launcher is resolved once at module load (`WRAPPER`/`RTK_CMD`, `tool-hook.ts:66-76`): if `.pi/rtk/ac-rtk.sh` exists, every routed segment runs through it; if it is missing at load time, `RTK_CMD` falls back to the bare binary `rtk`. The fallback is checked once, which is also the failure mode described in [The wrapper moved mid-session](#the-wrapper-moved-mid-session).

One wart in the seed is worth knowing: the module header comment says rtk invocations go through `.pi/hooks/ac-rtk.sh`, while the code resolves `.pi/rtk/ac-rtk.sh`. The mirror copies the seed byte for byte, comment included. The runtime directory is `.pi/rtk/`, and `.pi/hooks/` is a name pi actively warns about (see below), so trust the code, not the comment.

## The `.pi/hooks/` naming pitfall

pi warns at startup when a project `.pi/hooks/` directory exists:

```text
Project hooks/ directory found. Hooks have been renamed to extensions.
```

The warning comes from pi's startup migration check: what pi used to call hooks are now extensions, and the runtime directory must NOT be named `hooks`. The seed complies: the launcher lives in `.pi/rtk/`, not `.pi/hooks/`. If you ever find a `.pi/hooks/` directory in a replica (older replicas created before the rename, or a manual copy), move the files to `.pi/extensions/` and `.pi/rtk/` and reload the session: the extension will not load from `.pi/hooks/`, and the warning will keep appearing until the directory is gone.

## The read rule

pi's native `read` tool is not a shell tool, so rtk cannot wrap it. The extension registers the usage instead (`registerRead`, `tool-hook.ts:175-186`): on every `read` tool call it spawns `rtk read <path>` asynchronously with `stdio: "ignore"`, an `'error'` listener and `unref()`, purely for the record, and pi's own read proceeds untouched. The spawn never blocks the read, never emits output, and never fails the call.

Two things to know about the record it leaves:

- The database row is rtk's normalization, not the tool call. The `commands` row for a registered read carries `original_cmd` of `cat <path>` (verified: row 43 of the live tech-lead database is `cat D:\...\.pi\rtk\ac-rtk.sh` against `rtk_cmd` `rtk read`, and a scratch harness run produced the same shape). Do not search the DB for `read` to find pi's file reads; search for `cat`.
- Only the path is registered. `offset`, `limit` and file content never reach rtk, and a read of a nonexistent path records nothing at all (see [What reaches the database](#what-reaches-the-database)).

## The write and edit rule

`write` and `edit` calls append one line to the ignored log (`tool-hook.ts:275-281`), with the tool field `Write:` or `Edit:` and the target path, timestamped in the same local-time format as bash entries. Path only, no content. Verified lines from the live tech-lead log:

```text
20260820_034823 Write: D:\0_repos\AgentsCommander_iac\.ac\wg-19-dev-v5-team\__agent_tech-lead\scratch-verify.txt
20260820_034825 Edit: D:\0_repos\AgentsCommander_iac\.ac\wg-19-dev-v5-team\__agent_tech-lead\scratch-verify.txt
```

These are the pi port's answer to the gap the Claude page describes: under Claude Code, an agent that does its work through `Read` and `Edit` leaves nothing in either file, with nothing marking the gap. Under pi, file writes and edits are on the record.

## `rtk-ignored-tools-pi.md`

### Only workgroup replicas write it

The extension derives the target from its own location (`LOG_FILE`, `tool-hook.ts:56-64`): two levels up from `.pi/extensions/` is the replica root, and the Matrix folder is the replica's name with one leading `_` dropped, so `__agent_foo` writes to `_agent_foo`. Verified end to end: an extension run from a scratch `__pi-replica/.pi/extensions/` wrote its lines to the derived `_pi-replica/rtk-ignored-tools-pi.md`.

The derivation is mechanical, with no shape check, and that has one consequence worth measuring rather than assuming: from a root that does not start with `__`, the formula still points somewhere and the append still happens. `mkdirSync` on the parent (`tool-hook.ts:131`) creates the directory before the append, so the line lands wherever the formula points. In the real layout an agent running from its Matrix root `_agent_foo` derives a target at the top level of the workspace, as a sibling of `.ac`: `<workspace>/__agent_foo/rtk-ignored-tools-pi.md` (a stray `__agent_foo` directory that `mkdirSync` creates silently). Verified from a scratch `.ac/_matrix-root/`: the line landed in the stray `<workspace>/__matrix-root/rtk-ignored-tools-pi.md`, right next to `.ac`, not in the Matrix. Only a `__agent_*` replica root lands in the Matrix, which is what the `__` requirement means in practice. An empty or missing `rtk-ignored-tools-pi.md` therefore means either "nothing was ignored and nothing was written or edited" or "this agent never writes here", and the file cannot tell you which.

### Format

The extension appends to the file. One line per entry, no header:

```text
20260820_034343 Write: D:\0_repos\AgentsCommander_iac\.ac\wg-19-dev-v5-team\__agent_tech-lead\scratch-test-we.txt
20260820_035046 Bash: "$AGENTSCOMMANDER_BINARY_PATH" list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" 2>&1
20260820_035847 Bash: cd /tmp && export X=1
```

- The timestamp is **local time** in `YYYYMMDD_HHMMSS`, with no zone and no offset.
- The field after the timestamp is the tool, `Bash` (or `PowerShell` when the pwsh rule is active), `Write` or `Edit`, exactly as the handler emits it.
- The command is the **original**, not the rewrite; for `Write`/`Edit` entries it is the target path.
- All whitespace collapses to single spaces, newlines included, so a multi-line script survives as one unindented line and is no longer runnable as written.
- The extension appends and does nothing else: no truncation, no escaping, no rotation, no deduplication, and no locking. Two live replicas of the same agent append to the same file. `appendFileSync` is not guaranteed atomic on Windows, so an interleaved line is possible. The cost is one garbled line in a file a human reads, which is why there is no locking.

### The timestamps do not line up with the database

`rtk-ignored-tools-pi.md` stamps local time with no zone. The `timestamp` column of `commands` is RFC 3339 with an offset, as documented in [RTK usage and per-agent statistics](../rtk.md#the-commands-table). Measured at the same instant, two separate invocations: the passed-through `echo hi` wrote the log line `20260820_204317` (local time; the measuring machine is UTC-3), and the routed `ls -la .` landed in `commands` with `timestamp` `2026-08-20T23:43:17.828209500+00:00`. One invocation can never produce both: a command is either routed (database row) or handed back untouched (log line), never both. The two artifacts do not share a time format, so **you cannot correlate them by string comparison**. Convert one side before you line up a session across both files.

### An entry does not prove the command ran

`tool_call` fires before the tool executes. The log line is written before the shell starts, and the extension never sees the command's exit code. If the command dies, or the shell never runs it, the line is already written and nothing corrects it. Read the file as "the model asked for this and the extension declined to rewrite it".

## What reaches the database

The extension never touches the database. It reads no `RTK_DB_PATH`, knows nothing about SQLite, and only rewrites commands, spawns `rtk read`, and appends log lines. The rows are written by `rtk` when the routed command runs, into the `RTK_DB_PATH` of that session. [RTK usage and per-agent statistics](../rtk.md) covers how to point that variable at the agent's Matrix and how to read the results.

Which table a rewritten command lands in, measured against a scratch database with `rtk` 0.42.4:

| Invocation | exit | `commands` | `parse_failures` |
|---|---|---|---|
| `rtk ls -la .` | 0 | yes | no |
| `rtk ls /noexiste` | 2 | yes | no |
| `rtk node --version`, via the fallback | 0 | yes, as `rtk fallback: node --version` | yes, `fallback_succeeded=1` |
| `rtk nosuchbinary-xyz` | 127 | no | yes, `fallback_succeeded=0` |
| `rtk read /noexiste` | 1 | no | no |
| `rtk read <existing file>` | 0 | yes, as `cat <path>` | no |

A failing command is recorded like a passing one: the first four rows above cover exits 0, 2, 0 and 127. The pattern is that the extension's `rtk ` fallback produces a `parse_failures` row, because RTK cannot parse its own argv and falls back to direct execution, and that execution adds a `commands` row only when the binary exists.

The last two rows are the exceptions, and they are RTK's behaviour rather than the extension's: a failing `rtk ls` is recorded, a failing `rtk read` is not, and a successful `rtk read` is recorded under `cat`. So do not read either table as a complete ledger of everything routed through RTK.

## Failure modes

### RTK missing from PATH breaks simple commands

This is the one to watch. The extension calls `rtk rewrite` through `spawnSync`, which does not throw when the binary is absent, it returns empty stdout. Empty stdout is the fallback path, so a plain command still comes back rewritten. Measured with `rtk` off PATH: `ls -la` still mutated to the routed form, and the routed form dies with exit 127 when it runs: the wrapper's own `exec rtk` reports `rtk: not found`, and 127 is exactly what a missing wrapper path gives too (see below). The seed ships `ac-rtk.sh` executable (755); with a wrapper that is not executable the shell reports its permission-denied exit 126 instead, so treat the precise code as environment-dependent. Commands carrying shell syntax keep working, because they take the passthrough path. So losing RTK does not degrade tracking quietly, it breaks the agent's simple shell commands while the complicated ones keep running.

The `rtk read` registration spawn fails without breaking the session. `registerRead` attaches an `'error'` listener to the spawn (`tool-hook.ts:179`), so with `rtk` off PATH the ENOENT error event is consumed: the `read` tool call proceeds untouched, no record is written, nothing crashes, nothing prints. Best effort stays silent. An earlier seed without that listener died on the first `read` call (unhandled spawn `'error'` event); that crash is fixed in the current seed and is no longer reproducible.

### The wrapper moved mid-session

`RTK_CMD` is resolved once, at module load (`tool-hook.ts:72`). If the wrapper file is present at load, the running session keeps that path even if the file is later moved or deleted: every routed `bash` command then references a path that no longer exists and dies with exit 127. Verified mechanism, and a verified incident on 2026-08-20: after `ac-rtk.sh` was moved out of a live replica, all bash calls returned 127 until the session was reloaded. Recovery is a session reload (or `/reload`), which re-evaluates the module and re-resolves the path. If the wrapper is missing at load time, the fallback to bare `rtk` makes the session work without it, banner and all.

### Extension changes need a session reload

The extension is loaded once per session. Editing `.pi/extensions/tool-hook.ts` does not affect a running session; the module-level constants (`LOG_FILE`, `RTK_CMD`, `RULE`) are all captured at load. pi supports hot-reloading auto-discovered project-local extensions with `/reload` (or a session restart), which re-runs the module. This is also the recovery for the previous failure mode.

### The read, write and edit registration is best-effort and silent

`registerRead` and `logIgnored` are wrapped in `try/catch`, the read spawn uses `stdio: "ignore"` and `unref()`, and a failed write to the ignored log costs you the line and nothing else. A missing Matrix directory, a locked file or a full disk does not fail the tool call. That is deliberate: an extension must not cost you the command you asked for.

### An invalid `RTK_DB_PATH` loses the record silently

The extension ignores the variable entirely. The routed command runs, prints its normal output and exits 0, and what happens to the row is rtk's business: a nonexistent directory is created silently, database included, and the row is recorded (measured with `RTK_DB_PATH` pointing at a missing directory: `rtk ls -la .` exits 0 and the row lands in the freshly created database). The record is lost silently only when the path cannot be created: a plain file blocking the directory, or an existing non-SQLite file at the database path (both measured: exit 0, no row, no message). Nothing on this side reports either outcome. [RTK usage and per-agent statistics](../rtk.md#failure-modes) covers how that surfaces when you read the statistics.

### The extension never blocks a tool

Every path out of the handler is a mutation or a silent return. There is no deny path, so an extension that runs cannot stop an agent from working, however badly it misjudges a command. What pi does when an extension does not load at all, with the file removed or a project untrusted, is not something this file answers: project-local extensions load only after project trust, so an untrusted project silently runs with no routing at all.

## See also

- [RTK usage and per-agent statistics](../rtk.md) - configuring `RTK_DB_PATH` per agent type and reading the database this integration feeds
- [Agent Matrix conventions](../../agent-matrix-conventions.md) - replica and Matrix layout, which is what the extension derives the log path from
- [Coding agents](../coding-agents.md) - the coding-agent catalog and its ENVIRONMENT rows
- [The AgentsCommander RTK hook for Claude Code](../rtk_claude/README.md) - the Claude Code counterpart; same design, different tool set, different artifacts
- [pi extensions documentation](https://github.com/earendil-works/pi) - Extension Locations for project-local `.pi/extensions/*.ts` and the `tool_call` event with mutable `event.input` (in the installed package at `docs/extensions.md`)
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
