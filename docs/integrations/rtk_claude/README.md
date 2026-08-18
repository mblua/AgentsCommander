# The AgentsCommander RTK hook for Claude Code

For operators who want to know what a Claude Code agent actually ran, not what it reported. AgentsCommander seeds two `PreToolUse` hooks into every workgroup replica, one for the `Bash` tool and one for the `PowerShell` tool; after this page you can read the two files they produce and say which shell command landed in which one, from which shell, and why.

The hooks, the module they share and their registration are copied into this directory so you can review them without opening a replica:

- [`hooks/ac_rtk_claude_Bash.js`](hooks/ac_rtk_claude_Bash.js), 83 lines
- [`hooks/ac_rtk_claude_PowerShell.js`](hooks/ac_rtk_claude_PowerShell.js), 123 lines
- [`hooks/ac_rtk_shared.js`](hooks/ac_rtk_shared.js), 129 lines, the half that does not depend on a shell, required by both hooks
- [`settings.local.json`](settings.local.json)

All four are faithful copies of `<workspace>/.ac/default.claude/`, the seed AC installs from. Do not edit them here: this directory is a mirror, not the source.

## What the hooks are and where they land

AC seeds `.ac/default.claude/` into each workgroup replica when it creates the replica. A replica ends up with:

```text
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/hooks/ac_rtk_claude_Bash.js
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/hooks/ac_rtk_claude_PowerShell.js
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/hooks/ac_rtk_shared.js
<workspace>/.ac/<wg-N-name>/__agent_<name>/.claude/settings.local.json
```

`settings.local.json` registers both hooks, and the whole registration is this:

```json
"hooks": {
  "PreToolUse": [
    {
      "matcher": "Bash",
      "hooks": [{ "type": "command", "command": "node .claude/hooks/ac_rtk_claude_Bash.js" }]
    },
    {
      "matcher": "PowerShell",
      "hooks": [{ "type": "command", "command": "node .claude/hooks/ac_rtk_claude_PowerShell.js" }]
    }
  ]
}
```

Before every `Bash` tool call and every `PowerShell` tool call, Claude Code runs the matching hook with the tool input as JSON on stdin. The hook decides whether to rewrite the command so it runs through RTK, and answers with `permissionDecision: "allow"`.

Two files come out of that decision, both in the agent's **origin Agent Matrix**, not in the replica:

| File | Holds |
|---|---|
| `<workspace>/.ac/_agent_<name>/rtk-matrix-history.db` | The RTK history database, tables `commands` and `parse_failures`, written by `rtk` itself. See [RTK usage and per-agent statistics](../rtk.md). |
| `<workspace>/.ac/_agent_<name>/rtk_ignored_tools.md` | One line per command a hook handed back untouched, written by the hook. Both hooks append to this one file. |

The pair exists so you can tell "RTK covered this command" from "RTK never saw it". Neither file alone answers that, and a command missing from both is not proof it never ran: see [what they cover](#what-they-cover-the-two-shell-tools-and-nothing-else).

## These are not the hook `rtk init` installs

RTK ships its own Claude Code hook, and [RTK usage and per-agent statistics](../rtk.md#the-hook-covers-the-filtered-set-and-nothing-else) documents it. The two are separate pieces of software that happen to do related work:

| | AC hooks | RTK hook |
|---|---|---|
| Command | `node .claude/hooks/ac_rtk_claude_Bash.js` and `node .claude/hooks/ac_rtk_claude_PowerShell.js` | `rtk hook claude` |
| Installed by | AgentsCommander, when it seeds a replica | You, by running `rtk init` |
| Registered in | the replica's `.claude/settings.local.json` | the Claude Code settings you point `rtk init` at |
| Matchers registered | `Bash` and `PowerShell` | `Bash` only |
| Writes `rtk_ignored_tools.md` | yes | no |

Both are `PreToolUse` hooks and both route commands to RTK, so a machine can have both active at once. Only the AC hooks produce `rtk_ignored_tools.md`, so that file is the one that tells you an AC hook ran.

**Running `rtk init` does not close the `PowerShell` gap.** The `rtk` 0.42.4 binary contains the literal `matcher": "Bash` and zero occurrences of the byte string `PowerShell`, so the hook it installs is registered against the `Bash` tool alone. RTK itself is not the limitation: feed `rtk hook claude` a payload of `{"tool_name":"PowerShell","tool_input":{"command":"git status"}}` and it answers with `updatedInput.command` of `rtk git status`. The missing matcher is what leaves the gap, which is why the AC hooks register their own.

## What they cover: the two shell tools, and nothing else

The registration has two matchers, `Bash` and `PowerShell`. Every other tool the model calls goes straight to its implementation and reaches neither file:

`Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebFetch`, `Task`, every MCP tool, and any other tool the harness exposes that is not one of those two shell tools.

Take that seriously before you read either file as a record of the agent's work. An agent that does its work through `Read` and `Edit` leaves it out of both files, with nothing marking the gap.

Inside the two shell tools the coverage is close to complete but not total. Three kinds of command reach neither file:

- an unparseable JSON payload, and a command that is empty after leading whitespace is stripped, each hook's two early exits;
- a command a hook rewrites whose RTK invocation then records nothing. `cat /noexiste` becomes `rtk read /noexiste`, which leaves no line in the ignored log because it was rewritten, and no row in either table. It does not even create the database file.

The third kind is the one to remember, because it is indistinguishable from a command that was never issued.

## The routing rules

There is one rule per shell, and they are not the same rule. Read the one for the tool you are looking at.

### The `Bash` rule

A command is handed back untouched and written to `rtk_ignored_tools.md` when **both** of these hold:

1. `rtk rewrite "<command>"` prints nothing on stdout, and
2. the command is not safe to prefix, meaning any one of: it contains shell syntax (`\n ; | & < > ( ) { }` or a backtick), its first word contains `=` (the `FOO=bar cmd` form), or the shell resolves its first word to a `builtin`, `keyword`, `function` or `alias`.

If `rtk rewrite` prints nothing but the command **is** safe to prefix, the hook prefixes `rtk ` anyway and writes no log line. That fallback is what puts an unknown binary into the statistics.

Two details worth knowing:

- The hook keys on **stdout**, not on the exit code, because `rtk rewrite` returns a good rewrite with exit 3 for compound commands.
- There is no builtin list in the hook. It asks the shell with `type -t`, so the two never drift apart.

#### The rule the informal description gets wrong

Being a compound command is not what decides. Neither is any particular piece of shell syntax. **The first question is whether `rtk rewrite` produced any output**, and only when it produced none does anything else get looked at.

When `rtk rewrite` recognises something, the command is rewritten whatever its shape. Pipes, `;`, `&&` and redirections do not send it to the ignored log:

```bash
rtk rewrite "grep '(foo)' f.txt"
```

```text
rtk grep '(foo)' f.txt
```

When `rtk rewrite` comes back empty, the character test decides, and **that test is textual, not syntactic**. Any of `\n ; | & < > ( ) { }` or a backtick counts wherever it appears, quoted or not. Quoting changes what the shell will do with the character, never whether the hook counts it, and the two are worth keeping apart: inside double quotes most of these are inert, but a backtick and `$(` still run a command. So a logged line means the hook handed the command back untouched, not that the shell did nothing interesting with it.

An everyday invocation that redirects nothing, spawns no subshell and substitutes nothing still lands in the ignored log:

```bash
rtk rewrite "python -c \"print(1)\""
```

prints nothing, and the parenthesis inside the quoted argument then sends the command to the log. `node -e "console.log(1)"` goes the same way, and so does `nosuchbinary-xyz "a;b"` on its quoted semicolon.

The two halves of the rule are easiest to see as pairs that differ by one thing:

| Pair | `rtk rewrite` | Result |
|---|---|---|
| `grep '(foo)' f.txt` | `rtk grep '(foo)' f.txt` | rewritten, the quoted `(` never gets looked at |
| `python -c "print(1)"` | empty | **ignored log**, on the same quoted `(` |
| `nosuchbinary-xyz` | empty | prefixed to `rtk nosuchbinary-xyz` |
| `nosuchbinary-xyz \| sort` | empty | **ignored log**, and the pipe is the only difference |

The second pair is a compound command that does reach the ignored log, which is the case the informal rule denies outright. The first pair carries a parenthesis to both outcomes, which is why no list of shell constructs can describe this correctly.

So when you find a `Bash:` command in `rtk_ignored_tools.md`, do not go looking for a redirection. Look for any of those characters anywhere in the line, quoted or not, or an `=` in the first word, or a first word the shell owns.

Measured against the installed RTK, driving the hook the way Claude Code does:

The second column is the first stage, and it is what decides the third. Where it is empty, the deciding characters are named.

| Command | `rtk rewrite` prints | Result |
|---|---|---|
| `ls -la` | `rtk ls -la` | rewritten |
| `cat file.txt` | `rtk read file.txt` | rewritten |
| `ls \| sort` | `rtk ls \| sort` | rewritten, the pipe is never consulted |
| `ls; cat x` | `rtk ls; rtk read x` | rewritten |
| `git status && ls` | `rtk git status && rtk ls` | rewritten |
| `FOO=bar ls` | `FOO=bar rtk ls` | rewritten |
| `grep foo f.txt 2>/dev/null` | `rtk grep foo f.txt 2>/dev/null` | rewritten, and the `>` is never reached |
| `node --version` | nothing | prefixed to `rtk node --version`, nothing in the character class |
| `nosuchbinary-xyz` | nothing | prefixed to `rtk nosuchbinary-xyz` |
| `echo hola` | nothing | **ignored log**, `echo` is a builtin |
| `cd /tmp && export X=1` | nothing | **ignored log**, on `&` and on a builtin head |
| `ls > /tmp/x` | nothing | **ignored log**, on `>` |
| `ls \| sort > /tmp/now.txt` | nothing | **ignored log**, on `\|` and `>` |
| `(ls)` | nothing | **ignored log**, on `(` and `)` |
| `echo "total=$(wc -l < f)"` | nothing | **ignored log**, on several, and a builtin head |

### The `PowerShell` rule

The PowerShell hook asks the safety question **first**, then asks RTK. That is the opposite of the Bash order, and the inversion is the whole design:

1. the command already starts with `rtk `, so it is returned unchanged;
2. otherwise the head is not an external binary, so the command goes to the ignored log;
3. otherwise `rtk rewrite` prints something, so that output is the routed command;
4. otherwise the hook prefixes `rtk `.

**Why the order is inverted.** `rtk rewrite` decides on the head of each `;` / `&&` segment and does not know which shell will run the result, so it turns `ls` into `rtk ls`. Under bash every head it rewrites is a real binary emitting text, so the substitution is close to behaviour-preserving. Under PowerShell `ls`, `ps` and `diff` are aliases for `Get-ChildItem`, `Get-Process` and `Compare-Object`, so the rewrite replaces an **object** stream with a **text** stream: `ps | Where-Object { $_.CPU -gt 1 }` then matches nothing and returns empty at exit 0. `rtk ls` does not even run on Windows outside Git Bash, where it fails with `rtk: Failed to run ls: Failed to spawn process: program not found` at exit 1. Asking the safety question after the rewrite, the way the Bash hook does, would not catch a single one of those, because the rewrite path is where they land. So under PowerShell anything that does not clear the gate goes to the ignored log: a missed rewrite costs one statistic, a wrong rewrite silently breaks the agent's command.

**What the gate asks.** The hook spawns `pwsh` once per command it has not already routed, and asks the PowerShell parser and `Get-Command`, so there is no character class and no keyword list in the hook to drift out of step with the shell. The command clears the gate when it is a single statement, that statement is a single pipeline, the pipeline's first element is a plain command invocation with no call operator, its command name is a bare name containing no `=`, and that name either resolves to `CommandType = Application` or does not resolve at all. The command travels to the probe in the `AC_RTK_CMD` environment variable rather than on the command line, so there is no quoting boundary to get wrong.

The probe loads your PowerShell profile on purpose, because it has to answer for the command table the real session will use. A profile that defines `function git { ... }` correctly flips `git status` from routed to logged. A profile that runs `Remove-Alias -Name ls -Force`, which is an ordinary thing to do on a machine with `eza` or `lsd` installed, moves the verdict the other way and lets `ls` route.

Measured, driving the hook the way Claude Code does:

| Command | Result | Deciding guard |
|---|---|---|
| `git status` | `rtk git status` | head is `git.cmd`, an `Application` |
| `git log --oneline -5` | `rtk git log --oneline -5` | as above |
| `git status \| Select-Object -First 2` | `rtk git status \| Select-Object -First 2` | one pipeline, external head, tail untouched |
| `git status > out.txt` | `rtk git status > out.txt` | the redirection is left in place |
| `git log --pretty=format:%h` | `rtk git log --pretty=format:%h` | the `=` guard reads the command **name**, not an argument |
| `node --version` | `rtk node --version` | `rtk rewrite` empty, prefix fallback |
| `nosuchbinary-xyz` | `rtk nosuchbinary-xyz` | an unresolved head is treated as external |
| `python -c "print(1)"` | `rtk python -c "print(1)"` | the parenthesis is inside an argument, not syntax |
| `git commit -m "line one<LF>line two"` | `rtk git commit -m "line one<LF>line two"` | a newline **inside a string** does not split a statement |
| `rtk git status` | `rtk git status`, unchanged | already routed, nothing to do |
| `ls` | **ignored log** | `Alias` |
| `ls -la` | **ignored log** | `Alias` |
| `echo hola` | **ignored log** | `Alias` |
| `Get-ChildItem` | **ignored log** | `Cmdlet` |
| `ls \| Where-Object { $_.Name -like "*.md" }` | **ignored log** | `Alias` head |
| `git status; ls` | **ignored log** | two statements |
| `git status && ls` | **ignored log** | a pipeline chain, not a pipeline |
| `git status \|\| ls` | **ignored log** | a pipeline chain, not a pipeline |
| `$x = 1; git status` | **ignored log** | two statements, the first an assignment |
| `(Get-Date)` | **ignored log** | a bare expression, not a command |
| `& "C:\Program Files\Git\cmd\git.exe" status` | **ignored log** | call operator |
| `. .\script.ps1` | **ignored log** | call operator |
| `FOO=bar ls` | **ignored log** | the command name contains `=` |
| a newline used as a statement separator | **ignored log** | two statements |

Two divergences from the `Bash` rule are deliberate:

- `python -c "print(1)"` and `git status > out.txt` reach the ignored log under `Bash`, because the Bash hook tests characters textually, and are routed under `PowerShell`, because the parser tells the difference between an argument and syntax. PowerShell coverage is wider here.
- Every compound command the hook has not already routed reaches the ignored log under `PowerShell`, while `Bash` routes many of them. PowerShell coverage is narrower here, on purpose.

**Clearing the gate does not make a routed command correct**, only unambiguous to PowerShell. Three commands are routed and come out wrong, and they are listed here rather than left out so this table is not read as an all-clear:

| Command | What happens | Why |
|---|---|---|
| `tree` | routed to `rtk tree`, which prints `Too many parameters - node_modules\|.git\|target\|...` and **no listing**, at exit 0 | `tree` resolves to `C:\WINDOWS\system32\tree.com`, an `Application`, so the gate passes it, and `rtk` hands it GNU-`tree` ignore flags it rejects |
| `find "NAG" f.js` | routed to `rtk find "NAG" f.js`, which prints **nothing at all**, at exit 0 | `find` resolves to `C:\WINDOWS\system32\find.exe`; `rtk` applies GNU `find` semantics to arguments meant for it |
| `git status \| Select-String "nothing added to commit"` | routed, and returns **0 results where the command you typed returns 1**, at exit 0 | the head is fine; `rtk` reformats `git status` from six lines into two, and the untouched tail parses the old format |

The first two are names Windows owns with different semantics from the POSIX tool `rtk` assumes. The third does not depend on the head at all: it is any pipeline tail that parses the head's **text**, so `Select-String`, `-match`, `ConvertFrom-*`, `Where-Object { $_ -like ... }`, `Measure-Object -Line`, or a `> file` whose contents you parse later. Both classes exist today under `Bash` as well, wherever the resolved binary is the Windows one and wherever a `grep` tail parses reformatted output. Neither is created or widened by the PowerShell hook, and neither has a fix inside a hook: a deny-list keys on the head, while the third case is decided by the tail, so `git` breaks with `Select-String` and does not break with `Select-Object -First 2`.

### Reproduce any row

The copies in this repository **cannot be run in place.** This repository's `package.json:5` is `"type": "module"`, so Node treats every `.js` under it as an ES module and the hooks fail with:

```text
ReferenceError: require is not defined in ES module scope, you can use import instead
```

The hooks are correct; the repository is the wrong place to run them from. A replica has no `package.json` above `.claude/hooks/`, so they load as CommonJS there. Run them from inside a replica, or copy all three files to a scratch directory outside any Node project and run them there.

```bash
echo '{"tool_input":{"command":"ls -la"}}' | node .claude/hooks/ac_rtk_claude_Bash.js
```

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ac_rtk_claude_Bash","updatedInput":{"command":"exec 2> >(grep --line-buffered -v 'No hook installed' >&2)\nrtk ls -la"}}}
```

```bash
echo '{"tool_input":{"command":"git status"}}' | node .claude/hooks/ac_rtk_claude_PowerShell.js
```

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ac_rtk_claude_PowerShell","updatedInput":{"command":"rtk git status"}}}
```

A passed-through command prints nothing and exits 0. Run one from a real replica and it appends a line to that agent's real `rtk_ignored_tools.md`.

### What each hook prepends

The `exec 2> >(grep ...)` line the Bash hook prepends to every rewrite silences a known RTK false positive on stderr. It is a separate statement rather than a wrapper, so heredocs keep working and the command's exit code and remaining stderr survive.

**The PowerShell hook prepends nothing at all**, so the `[rtk] /!\ No hook installed` notice appears on stderr once per routed command, and as one extra record in a `$c = <command> 2>&1` capture. The notice is not throttled. That is a deliberate trade, not an omission: PowerShell has no statement that reassigns the rest of a script's error stream, so every construct that filters the notice has to turn `rtk` from an external binary into a shell function, and measured, that wrapper reports `$?` as `True` after a failing command, empties a `2>&1` capture, defeats `2>` and `2>$null`, rewrites every LF to CRLF in redirected stdout and destroys redirected binary output, and splits an unquoted comma in an argument into two argv entries. Four broken channels, three of them silent, against one cosmetic line. The positive half is the point of the trade: a routed PowerShell command is otherwise **native in every channel**. `$?`, `$LASTEXITCODE`, `2>`, `2>&1`, `2>$null`, the bytes of redirected stdout, argv fidelity and background jobs all behave as if you had run the command without the hook.

**Neither table above is a contract.** Each hook delegates the rewrite to `rtk rewrite` and treats it as the source of truth. Which shapes `rtk rewrite` declines is a property of your installed RTK, so a new RTK version can move commands between the two files with no change to either hook. The PowerShell gate adds a second moving part: which names resolve to `Application` is a property of your PATH and your profile, so the same command can route on one machine and be logged on another. Re-measure after an RTK upgrade rather than trusting these tables.

## `rtk_ignored_tools.md`

### Only workgroup replicas write it

Each hook derives the target from its own location: two levels up from `.claude/hooks/` is the replica root, and the Matrix folder is the replica's name with one leading underscore dropped, so `__agent_foo` writes to `_agent_foo`.

It requires the replica root to start with `__`. An agent running directly in its Matrix, or anywhere else, writes no line at all, and nothing reports that. An empty or missing `rtk_ignored_tools.md` therefore means either "nothing was ignored" or "this agent never writes here", and the file cannot tell you which.

### Format

Both hooks append to the same file. One line per entry, no header:

```text
20260818_110844 Bash: ls | sort > /tmp/now.txt
20260818_110844 PowerShell: ls | Where-Object { $_.Name }
```

- The timestamp is **local time** in `YYYYMMDD_HHMMSS`, with no zone and no offset.
- The field after the timestamp is the shell tool, `Bash` or `PowerShell`, exactly as it appears in the matcher. It comes from the hook's own constant rather than from the payload, so the line is labelled correctly even when the payload carries no `tool_name`.
- The command is the **original**, not the rewrite.
- All whitespace collapses to single spaces, newlines included, so a heredoc stays one entry. A multi-line script therefore survives as one unindented line and is no longer runnable as written.
- The hooks append and do nothing else: no truncation, no escaping, no rotation, no deduplication, and no locking. Two live replicas of the same agent append to the same file, and within a single replica the two hooks can append at the same moment, because `Bash` and `PowerShell` tool calls issued in one assistant turn run in parallel. `fs.appendFileSync` is not guaranteed atomic on Windows, so an interleaved line is possible. The cost is one garbled line in a file a human reads, which is why there is no locking.

**Lines written before the hook was split carry no tool field**, in the older `20260818_110844: ls | sort` shape. Any reader has to tolerate both.

### The timestamps do not line up with the database

`rtk_ignored_tools.md` stamps local time with no zone. The `timestamp` column of `commands` is RFC 3339 with an offset, as documented in [RTK usage and per-agent statistics](../rtk.md#the-commands-table). The two artifacts do not share a time format, so **you cannot correlate them by string comparison**. Convert one side before you line up a session across both files.

### An entry does not prove the command ran

These are `PreToolUse` hooks: the line is written before the shell starts, and neither hook ever sees the command's exit code. If you deny the permission prompt afterwards, or the command dies, the line is already written and nothing corrects it. Read the file as "the model asked to run this and the hook declined to rewrite it".

## What reaches the database

Neither hook touches the database. They read no `RTK_DB_PATH`, know nothing about SQLite, and only rewrite the command and return `allow`. The rows are written by `rtk` when the rewritten command runs, into the `RTK_DB_PATH` of that session. [RTK usage and per-agent statistics](../rtk.md) covers how to point that variable at the agent's Matrix and how to read the results.

Which table a rewritten command lands in, measured against a scratch database:

| Invocation | `commands` | `parse_failures` |
|---|---|---|
| `rtk ls -la .` | yes | no |
| `rtk ls /noexiste`, exit 2 | yes | no |
| `rtk node --version`, via the fallback | yes, as `rtk fallback: node --version` | yes, `fallback_succeeded=1` |
| `rtk nosuchbinary-xyz`, exit 127 | no | yes, `fallback_succeeded=0` |
| `rtk read /noexiste`, exit 1 | no | no |

A failing command is recorded like a passing one: the first four rows above cover exits 0, 2, 0 and 127. The pattern is that a hook's `rtk ` fallback produces a `parse_failures` row, because RTK cannot parse its own argv and falls back to direct execution, and that execution adds a `commands` row only when the binary exists.

Expect `parse_failures` to grow faster once `PowerShell` traffic arrives. Under PowerShell an unresolved head is more often a mistyped cmdlet than a missing binary, and the hook routes an unresolved head rather than logging it, so more of those reach the fallback. That growth is expected, not a regression.

The last row is the exception, and it is RTK's behaviour rather than a hook's: a failing `rtk ls` is recorded and a failing `rtk read` is not. So do not read either table as a complete ledger of everything routed through RTK.

## Failure modes

### RTK missing from PATH breaks simple commands

This is the one to watch. The hook calls `rtk rewrite` through `spawnSync`, which does not throw when the binary is absent, it returns empty stdout. Empty stdout is the fallback path, so a plain command still comes back rewritten:

```bash
echo '{"tool_input":{"command":"ls -la"}}' | node .claude/hooks/ac_rtk_claude_Bash.js
```

With `rtk` off the PATH the hook still answers `rtk ls -la`, and that dies with exit 127 when it runs. Commands carrying shell syntax keep working, because they take the passthrough path. So losing RTK does not degrade tracking quietly, it breaks the agent's simple shell commands while the complicated ones keep running.

The `PowerShell` hook fails the same way and is equally loud: the routed command is `rtk ...`, so PowerShell reports `CommandNotFoundException` on stderr and the command fails.

### The PowerShell probe fails closed

The `PowerShell` hook spawns `pwsh` once per command it has not already routed. If `pwsh` is absent, fails, hangs past its five-second `timeout`, or prints no `AC_RTK_VERDICT:` line, the hook treats the answer as "not safe", hands the command back untouched and logs it. That is the safe direction: the cost is one statistic, and the alternative is routing a command whose shape was never checked.

The degenerate case reads alarming and is not. With `pwsh` missing entirely, **every** `PowerShell` command the hook has not already routed goes to `rtk_ignored_tools.md`, and `commands` gains rows only from the commands the model wrote as `rtk ...` itself, which take the early return before the probe. That is still better than the situation before these hooks, where those calls produced nothing anywhere, and a file full of `PowerShell:` lines beside a near-empty database makes the failure obvious on first read instead of invisible.

One assumption the hook cannot check: it probes `pwsh`, and it has no way to confirm that the harness's `PowerShell` tool is that same PowerShell. Where the tool runs Windows PowerShell 5.1, `pwsh` is either absent, and everything the hook probes fails closed as above, or present and answering for a different command table than the one that will run.

### The `rtk` notice is not filtered under PowerShell

One `[rtk] /!\ No hook installed` line lands on stderr per routed `PowerShell` command, and one extra record lands in a `2>&1` capture. It is not throttled, so it appears every time. This is the deliberate trade described above and the one behavioural difference an agent sees between the two hooks.

What you keep in exchange: after a routed `PowerShell` command, both `$?` and `$LASTEXITCODE` are reliable, because the hook prepends nothing and `rtk` stays an ordinary external binary. A failing routed command reports `$?` as `False` and the same `$LASTEXITCODE` the unrouted command would have reported.

### A routed PowerShell command can be wrong while looking fine

`tree` and `find` clear the gate, lose their output and exit 0. Any pipeline tail that parses the head's text breaks when `rtk` reformats that text, `git status | Select-String "nothing added to commit"` being the cleanest case: zero results where the command you typed returns one, exit 0, nothing on stderr. The routing tables above list all three. Neither class is created by these hooks, and neither can be fixed inside one.

### An invalid `RTK_DB_PATH` loses the record silently

The hooks ignore the variable entirely. The rewritten command runs, prints its normal output and exits 0, and the row is never written. Nothing on this side reports it. [RTK usage and per-agent statistics](../rtk.md#failure-modes) covers how that surfaces when you read the statistics.

### A failed write to the ignored log is silent

Writing the log line is wrapped in an empty `catch`. A missing Matrix directory, a locked file or a full disk costs you the line and nothing else. That is deliberate: a hook must not cost you the command you asked for.

### Neither hook ever blocks a tool

Every path out of either hook is a silent `exit(0)` or `permissionDecision: "allow"`. There is no `deny` and no blocking exit code, so a hook that runs cannot stop an agent from working, however badly it misjudges a command. What Claude Code does when a hook does not run at all, with `node` missing or the file removed, is not something these files answer.

## See also

- [RTK usage and per-agent statistics](../rtk.md) - configuring `RTK_DB_PATH` per agent type and reading the database these hooks feed
- [Agent Matrix conventions](../../agent-matrix-conventions.md) - replica and Matrix layout, which is what the hooks derive the log path from
- [Coding agents](../coding-agents.md) - the coding-agent catalog and its ENVIRONMENT rows
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
