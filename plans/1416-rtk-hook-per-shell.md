# Plan #1416: Split the RTK hook per shell and cover the PowerShell tool

Author: architect, wg-17. Authored 2026-08-18 UTC on the Full delivery path.

Status: DRAFT_FOR_ENRICHMENT. This plan is complete as a specification but is **not** certified. It goes to `dev-rust` and `dev-rust-grinch` for enrichment and returns to the architect for the consensus verdict.

Issue: [mblua/AgentsCommander#1416](https://github.com/mblua/AgentsCommander/issues/1416), `feat: split the RTK hook per shell and cover the PowerShell tool`.

This change touches four files under `docs/integrations/rtk_claude/` and one sentence of `docs/integrations/rtk.md`. It touches no Rust, no TypeScript, no CSS, no build script, no CI workflow and nothing under `src-tauri/`. It adds no crate, no npm dependency, no Tauri command, no IPC surface, no event and no migration.

---

## 1. Frozen authority and entry gate

The implementation working tree is `repo-AgentsCommander`, branch `feat/1416-rtk-hook-per-shell`, targeting `main`.

At authoring time all three of the following resolved exactly to `90c429f8190aa5c973944abd18127b566af28eb2`:

- committed `HEAD` of `feat/1416-rtk-hook-per-shell`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

Every line number in this plan refers to that SHA. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if any of the three no longer equals the frozen SHA. Do not rebase, merge a moved base, or silently substitute a newer commit.

Binding facts about the repository mechanics, verified at the frozen SHA:

- Root `.gitignore:11` is `/plans/`. This plan file must be force-added: `git add -f plans/1416-rtk-hook-per-shell.md`. Do not remove or weaken that ignore rule.
- `scripts/validate-branch-name.mjs:15` accepts type `feat`. `feat/1416-rtk-hook-per-shell` parses as type `feat`, number `1416`, slug `rtk-hook-per-shell` (18 characters, under the 50-character `MAX_SLUG` cap). The `validate-branch-name` check will pass.
- `git status --porcelain=v1 --untracked-files=all` at freeze time reported exactly one entry: `?? rtk-replica-history.db`. **Do not commit it, do not delete it, and do not add a `.gitignore` rule for it in this issue.** Its continued presence and untracked state is a verification criterion (section 9.1).

### 1.1 Working-copy rule for anyone who tries the hooks

A replica's `.claude/` directory is destroyed and rebuilt on every spawn: `perform_seed` stages a fresh tree, renames the whole existing `.claude` aside as `.claude.acseed-old-<sfx>`, installs atomically and drops the old one (`src-tauri/src/config/config_seed.rs:463-687`, covered by `perform_seeds_from_highest_present_tier_and_clean_replaces_dest`).

**Keep every scratch copy, prototype and test harness at your replica root, never inside `.claude/`.** Work left in `.claude/` is gone at the next spawn.

---

## 2. Objective

Close the blind spot documented by #1414: commands the model runs through the `PowerShell` tool reach neither `rtk-matrix-history.db` nor `rtk_ignored_tools.md`, so both artifacts silently under-report by an unmeasurable amount.

After this change, every `PowerShell` tool call leaves a trace in exactly one of the two artifacts, exactly as every `Bash` tool call already does, and each trace names which shell tool produced it.

---

## 3. Evidence and current-state gap

### 3.1 What exists today

`docs/integrations/rtk_claude/` holds three tracked entries:

```text
docs/integrations/rtk_claude/README.md
docs/integrations/rtk_claude/hooks/ac_rtk_claude.js      (132 lines)
docs/integrations/rtk_claude/settings.local.json
```

`settings.local.json:10-20` declares exactly one `PreToolUse` entry, matcher `Bash`, command `node .claude/hooks/ac_rtk_claude.js`. Nothing matches `PowerShell`.

### 3.2 The directory is a mirror, not a source

Confirmed by `dev-rust` and recorded in issue comment [#issuecomment-5333106663](https://github.com/mblua/AgentsCommander/issues/1416#issuecomment-5333106663):

- `config_seed.rs:463-687` copies a hand-authored directory verbatim. Nothing about the hook or the matcher block is generated, embedded or templated in Rust. The literal `default.claude` does not exist in the Rust source at all; `resolve_config_seed` (`config_seed.rs:151-211`) builds `"default" + dest_name`.
- The workspace source, this tracked docs copy and a live seeded replica copy are byte-identical after CRLF normalisation (`ac_rtk_claude.js` = `7BCFAB7987971C89`, `settings.local.json` = `05DF72E2A91E5724`).
- The only Rust hit for `PreToolUse` is a test fixture, `seed_settings_local_expands_user_home_but_not_hook_markers` (`config_seed.rs:2117-2154`).

**Editing the docs alone changes nothing at runtime; editing the workspace tree alone leaves the docs lying.** Section 8.4 states who applies the runtime half.

### 3.3 The seeder copies a plain tree, so a sibling module works

`copy_tree_internal` (`config_seed.rs:1155-1201`) walks `read_dir` and copies **every** regular file in **every** subdirectory, with no filter on name or extension. It recurses into directories (`:1184-1193`) and copies files through `copy_file_substituted` (`:1195`). Symlinks and Windows reparse points are skipped (`:1171-1178`); nothing else is.

Therefore a fourth file placed beside the two hooks in `hooks/` is seeded into every replica, and `require("./ac_rtk_shared.js")` resolves from either hook, because Node resolves a relative `require` against the requiring module's own directory.

### 3.4 Content substitution cannot corrupt the new files

`copy_file_substituted` (`config_seed.rs:1204-1243`) substitutes exactly four literal tokens, and `expand_placeholders_in_content` (`src-tauri/src/config/placeholders.rs:173-193`) names them:

| Token | Constant |
|---|---|
| `%AC_REPLICA_ROOT%` | `placeholders.rs:4` |
| `%AC_WORKSPACE_ROOT%` | `placeholders.rs:5` |
| `%AC_MATRIX_ROOT%` | `placeholders.rs:6` |
| `%USER_HOME%` | `placeholders.rs:13` |

Unknown `%...%` text is left literal (`placeholders.rs:169-172`). Files over `CONTENT_SUBSTITUTION_CAP` (5 MiB, `config_seed.rs:29`) and files with a NUL in the first 8 KiB are copied verbatim.

**Binding constraint on the implementer: none of the four literals above may appear anywhere in `ac_rtk_claude_Bash.js`, `ac_rtk_claude_PowerShell.js`, `ac_rtk_shared.js` or `settings.local.json`.** None of the specified content contains them.

### 3.5 The three shell-specific behaviours, re-verified

Verified by direct execution on the authoring workstation: Windows 11 Pro 10.0.26200, `rtk` 0.42.4, PowerShell 7.6.5, Node 24.13.0.

1. **The stderr filter.** `ac_rtk_claude.js:35` builds `FILTER` as `exec 2> >(grep --line-buffered -v 'No hook installed' >&2)\n` and `:120` prepends it to every rewrite. `exec 2> >(...)` is bash process substitution. PowerShell has no statement that redirects the rest of a script's error stream.

2. **The builtin question.** `ac_rtk_claude.js:76-81` asks `bash -c 'type -t -- "$1"'` and treats `builtin`, `keyword`, `function` and `alias` as untouchable. That answer says nothing about PowerShell. Measured in a real PowerShell 7.6.5 session, `ls`, `cat`, `echo`, `rm`, `ps`, `diff`, `sort`, `where`, `tee`, `man` and `sleep` are all `CommandType = Alias`; `curl` is `CommandType = Application` (`C:\WINDOWS\system32\curl.exe`); `wget` does not resolve at all.

3. **The character class.** `ac_rtk_claude.js:73` rejects `[\n;|&<>(){}` + backtick]. In PowerShell `{}` delimits a script block that appears in ordinary idiomatic commands (`ls | Where-Object { $_.Name }`), backtick is the escape character rather than command substitution, and `&&` / `||` are pipeline-chain operators rather than a bare `&`.

### 3.6 The hazard that is not in the issue text, and is the reason for the design in section 4

`rtk rewrite` decides on the head of each `;` / `&&` segment. It does not know which shell will run the result. Measured directly:

| Input to `rtk rewrite` | stdout |
|---|---|
| `ls` | `rtk ls` |
| `ls -la` | `rtk ls -la` |
| `cat file.txt` | `rtk read file.txt` |
| `ps` | `rtk ps` |
| `diff a b` | `rtk diff a b` |
| `ls \| Where-Object { $_.Name -like "*.md" }` | `rtk ls \| Where-Object { $_.Name -like "*.md" }` |
| `ps \| Where-Object { $_.CPU -gt 1 }` | `rtk ps \| Where-Object { $_.CPU -gt 1 }` |
| `git status; ls` | `rtk git status; rtk ls` |
| `Get-ChildItem` | (empty) |

Under bash every one of those heads is a real binary emitting text, so the substitution is close to behaviour-preserving. Under PowerShell:

- `ls`, `ps` and `diff` are aliases for `Get-ChildItem`, `Get-Process` and `Compare-Object`. Replacing them with `rtk ls` / `rtk ps` / `rtk diff` replaces an **object** stream with a **text** stream. A tail such as `Where-Object { $_.CPU -gt 1 }` then matches nothing and the pipeline returns empty with exit code 0. The agent's command is silently wrong, which is worse than the reporting gap this issue is closing.
- `rtk ls` does not even run on Windows outside Git Bash. Measured in a real PowerShell tool call: `rtk: Failed to resolve 'ls' via PATH, falling back to direct exec: Binary 'ls' not found on PATH` then `rtk: Failed to run ls: Failed to spawn process: program not found`, exit 1. RTK's `ls` subcommand proxies to a native `ls` binary, and Windows has none on the PowerShell PATH.

**This hazard lives on the rewrite path, not on the prefix path.** `prefixable()` (`ac_rtk_claude.js:72-82`) only runs when `rtk rewrite` printed nothing. Porting `prefixable()` to PowerShell and leaving the order alone would not catch a single case in the table above. Section 4.3 inverts the order for exactly this reason.

### 3.7 `rtk init` still registers only `Bash`

Re-verified against the installed `rtk` 0.42.4 binary: it contains the literal `matcher": "Bash` and **zero** occurrences of the byte string `PowerShell`. RTK's own hook is not an alternative to this change.

Separately, `rtk hook claude` does accept a PowerShell payload. Feeding it `{"tool_name":"PowerShell","tool_input":{"command":"git status"}}` returns `updatedInput.command = "rtk git status"`. RTK is not the blocker; the missing matcher is.

### 3.8 The nag is real and has no off switch

Measured: `rtk ls` and `rtk git status` both write `[rtk] /!\ No hook installed ... run 'rtk init -g' for automatic token savings` to stderr. A scan of the `rtk` 0.42.4 binary for `RTK_[A-Z0-9_]+` yields `RTK_AUDIT_DIR`, `RTK_DB_PATH`, `RTK_DISABLED`, `RTK_HOOK_AUDIT`, `RTK_MINOR`, `RTK_NO_TOML`, `RTK_TEE`, `RTK_TEE_DIR`, `RTK_TELEMETRY_DISABLED`, `RTK_TOML_DEBUG` and `RTK_TRUST_PROJECT_FILTERS`. None suppresses the notice. The filter stays necessary.

### 3.9 The mirror copies cannot be executed in place

`package.json:5` of this repository is `"type": "module"`. Node resolves a `.js` file's module type by walking up to the nearest `package.json`, so running the mirror copy from inside the repository fails:

```text
ReferenceError: require is not defined in ES module scope, you can use import instead
```

A live replica has no `package.json` in any ancestor of `.claude/hooks/`, so the hooks load as CommonJS there and `require` works. **The hooks are correct; the repository is simply the wrong place to run them from.** Section 9.2 gives the reviewer the supported way to drive them.

---

## 4. The decided solution

### 4.1 File layout

`docs/integrations/rtk_claude/hooks/` ends with three files:

| File | Contents |
|---|---|
| `ac_rtk_shared.js` | Everything shell-independent. Required by both hooks. |
| `ac_rtk_claude_Bash.js` | The bash stderr filter, the bash safety predicate, and a `decide` of four lines. |
| `ac_rtk_claude_PowerShell.js` | The PowerShell stderr filter, the PowerShell safety predicate, and a `decide` of three lines. |

`ac_rtk_claude.js` is renamed to `ac_rtk_claude_Bash.js`. Use `git mv` so the rename is recorded.

No cleanup step is needed for replicas seeded before the rename. Re-seeding renames the whole existing `.claude` aside and installs a fresh tree (`config_seed.rs:463-687`), so a file removed from the source cannot survive the next spawn. Three caveats, all documented in the same function and all outside this change: a skipped seed (`StaleReplica`, `NoSource`, `DestinationInUse`, `GateUnavailable`, `InvalidDestination`) leaves the old tree intact; a locked `.claude.acseed-old-<sfx>` directory can linger until a later run sweeps it by prefix; and a replica that is never spawned again keeps the stale file forever. **Do not add a delete step.**

### 4.2 `ac_rtk_shared.js`

Exports exactly six symbols. Nothing shell-specific belongs here.

```js
module.exports = { NAG, ALREADY_RTK, rtkRewrite, ignoredLogPath, logIgnored, runHook };
```

| Symbol | Contract |
|---|---|
| `NAG` | The string `"No hook installed"`. Each hook interpolates it into its own filter. |
| `ALREADY_RTK` | The regex `/^rtk\s/`. |
| `rtkRewrite(cmd)` | `spawnSync("rtk", ["rewrite", cmd], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], shell: false })`, returns `(r.stdout \|\| "").trim()`. Passing `cmd` as a single argv entry with `shell: false` is what keeps the shell from re-parsing it. |
| `ignoredLogPath(hookDir)` | Today's `ignoredLogPath()` with `__dirname` replaced by the `hookDir` argument. Unchanged logic: two levels up from `hookDir` is the replica root; return `null` unless its basename starts with `__`; otherwise join two more levels up with the basename minus one leading underscore and `rtk_ignored_tools.md`. |
| `logIgnored(hookDir, tool, cmd)` | Today's `logIgnored()` plus the `tool` field in the line. Same local-time `YYYYMMDD_HHMMSS` stamp, same whitespace folding, same empty `catch`. Line format in section 6.4. |
| `runHook(hookDir, tool, decide)` | The stdin read, the JSON parse, the two early exits, the call to `decide`, the ignored-log write, and the response JSON. Contract in section 6.1. |

`spawnSync`, `fs` and `path` are required here, not in the hooks, except that each hook still requires `spawnSync` for its own probe.

### 4.3 The two routing rules

The Bash rule is today's rule, unchanged in every observable respect.

```text
Bash:
  1. body starts with "rtk "        -> FILTER + body
  2. rtk rewrite prints something   -> FILTER + that output
  3. prefixable(body)               -> FILTER + "rtk " + body
  4. otherwise                      -> ignored log
```

The PowerShell rule asks the safety question **first**, because under PowerShell the rewrite path is the dangerous one (section 3.6).

```text
PowerShell:
  1. body starts with "rtk "        -> FILTER + body
  2. NOT headIsExternal(body)       -> ignored log
  3. rtk rewrite prints something   -> FILTER + that output
  4. otherwise                      -> FILTER + "rtk " + body
```

`headIsExternal` returning true means: the command is a single statement, that statement is a single pipeline, the pipeline's **first** element is a plain command invocation with no call operator, its command name is a bare name containing no `=`, and that name either resolves to `CommandType = Application` or does not resolve at all.

Once that holds, accepting `rtk rewrite`'s output is safe:

- Only one `;` / `&&` segment exists, so `rtk rewrite` can only replace that one head. There is no second segment for it to mis-rewrite.
- The head was already an external binary, so it already emitted **strings** into the PowerShell pipeline. Replacing it with `rtk <head>`, which also emits strings, preserves the pipeline's type contract. Only the content is compacted, which is the point of RTK.
- Any pipeline tail is left untouched by `rtk rewrite`, verified in section 3.6.

Everything else goes to the ignored log. That is deliberate: a missed rewrite costs one statistic, a wrong rewrite silently breaks the agent's command.

### 4.4 The PowerShell stderr filter

A prepended `rtk` shadow function, emitted as its own statement exactly as the bash `exec` line is, followed by `\n` and then the command:

```powershell
function rtk { $e = (Get-Command rtk -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source; if (-not $e) { $e = 'rtk' }; & $e @args 2>&1 | ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { if ("$_" -notmatch 'No hook installed') { [Console]::Error.WriteLine("$_") } } else { $_ } } }
```

Why this shape:

- `-CommandType Application` resolves the real executable and cannot recurse into the function being defined. Verified.
- `2>&1` on a native command yields `ErrorRecord` objects for stderr lines and plain strings for stdout lines, so the two streams can be separated again. Error records that are not the notice are written back to the real stderr with `[Console]::Error.WriteLine`; everything else is passed down the pipeline unchanged.
- `'No hook installed'` contains no regex metacharacter, so `-notmatch` is safe with it as a literal.
- It shadows only `rtk`. After the section 4.3 rule the emitted command always begins with `rtk `, so the shadow always applies, and it never touches any other command in the pipeline.
- The `if (-not $e) { $e = 'rtk' }` fallback keeps the failure mode identical to today's: with `rtk` off the PATH the command fails loudly rather than silently succeeding.

Verified in a real `PowerShell` tool call in this harness:

- With the function defined, the notice is gone and other stderr survives verbatim. Without it, the notice is printed. Same command, same session.
- `$LASTEXITCODE` is preserved exactly: `rtk git status` outside a repository still reports `128`, and a wrapped `cmd /c exit 7` is reported by the harness as `Exit code 7`, identical to the unwrapped baseline. The failure signal is not lost.
- `rtk git status | Select-Object -First 3` runs end to end and returns RTK's compacted git output.

**Do not append `exit $LASTEXITCODE`.** It was measured and is unnecessary: the harness already reports the correct exit code through the wrapper. Adding it would put an `exit` at the end of every routed command for no gain.

### 4.5 The PowerShell safety predicate

`headIsExternal(cmd)` spawns PowerShell once and asks the PowerShell parser and `Get-Command`, so there is no character class and no keyword list in the hook to drift. This is the same principle the bash hook states at `ac_rtk_claude.js:69-71`, expressed in the other shell.

The command is passed through the environment, not through the command line, so no quoting question arises:

```js
const r = spawnSync("pwsh", ["-NonInteractive", "-NoLogo", "-Command", PROBE], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "ignore"],
  shell: false,
  env: { ...process.env, AC_RTK_CMD: cmd },
});
return (r.stdout || "").trim() === "APP";
```

`PROBE` is these seventeen statements joined with `\n`:

```powershell
$ErrorActionPreference = 'SilentlyContinue'
$t = $null; $e = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($env:AC_RTK_CMD, [ref]$t, [ref]$e)
if ($e.Count) { 'SHELL'; exit 0 }
$st = $ast.EndBlock.Statements
if ($st.Count -ne 1) { 'SHELL'; exit 0 }
$p = $st[0] -as [System.Management.Automation.Language.PipelineAst]
if (-not $p) { 'SHELL'; exit 0 }
$c0 = $p.PipelineElements[0] -as [System.Management.Automation.Language.CommandAst]
if (-not $c0) { 'SHELL'; exit 0 }
if ($c0.InvocationOperator -ne 'Unknown') { 'SHELL'; exit 0 }
$n = $c0.GetCommandName()
if (-not $n) { 'SHELL'; exit 0 }
if ($n -like '*=*') { 'SHELL'; exit 0 }
$g = Get-Command -Name $n -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $g) { 'APP'; exit 0 }
if ($g.CommandType -eq 'Application') { 'APP' } else { 'SHELL' }
```

What each guard buys, all measured:

| Guard | Rejects |
|---|---|
| `$e.Count` | Text PowerShell cannot parse. |
| `$st.Count -ne 1` | `git status; ls`, and any command containing a newline. |
| not a `PipelineAst` | `$x = 1`, `git status && ls`, `git status \|\| ls` (both become a `PipelineChainAst`), `if (...) { ... }`, `foreach (...) { ... }`. |
| not a `CommandAst` | A bare expression such as `(Get-Date)`. |
| `InvocationOperator -ne 'Unknown'` | `& "C:\Program Files\Git\cmd\git.exe" status` and `. .\script.ps1`. |
| `GetCommandName()` empty | A head that is not a literal, such as `& $exe`. |
| `$n -like '*=*'` | The bash `FOO=bar cmd` form, which PowerShell would otherwise parse as a command literally named `FOO=bar` and `rtk rewrite` would turn into the bash-only `FOO=bar rtk ls`. |
| `CommandType -ne 'Application'` | Every PowerShell alias, function, filter, cmdlet, external script and configuration, which is the section 3.6 hazard. |

Two deliberate choices inside the probe:

- **`-NoProfile` is not passed.** The probe must see the same command table the real session will use. A profile that defines a function or alias would otherwise be invisible to the probe, and the hook would prefix `rtk ` onto a name PowerShell owns. Loading the profile can only make the probe more conservative, never less. On the authoring workstation no profile file exists, so the cost measured zero, but the correctness argument holds where one does.
- **No textual fast path.** A cheap JS pre-filter on `;`, `&&`, newline and so on would skip the probe for most commands and save roughly 240 ms, but it would be a second copy of the shell-syntax question living beside the parser, which is exactly the drift `ac_rtk_claude.js:69-71` was written to avoid. Rejected deliberately. See section 7.3 for the measured cost that is being accepted.

### 4.6 Per-hook identification

Two hooks writing indistinguishable output into one shared log would leave the PowerShell shortfall as unmeasurable as it is today, which is the defect this issue exists to fix. Both artifacts therefore name the shell:

- `permissionDecisionReason` becomes `ac_rtk_claude_Bash` or `ac_rtk_claude_PowerShell`, replacing today's `ac_rtk_claude`.
- Each `rtk_ignored_tools.md` line gains a tool field (section 6.4).

Nothing in the repository parses either value. `git grep` for `rtk_ignored_tools`, `ignored_tools` and `ignoredTools` across `src-tauri/`, `src/` and `scripts/` returns no hits; the only references are prose in `docs/integrations/rtk_claude/README.md` and the path construction inside the hook itself.

---

## 5. Affected surfaces

### 5.1 `docs/integrations/rtk_claude/hooks/ac_rtk_claude.js` → `ac_rtk_claude_Bash.js`

Renamed with `git mv`. Changes to the content:

1. The header comment gains, as its own paragraph, the environment statement the issue requires: this hook runs under the `Bash` tool, which means Windows under Git Bash, Linux and macOS. Name all three.
2. The header comment gains one sentence pointing at `ac_rtk_claude_PowerShell.js` as the sibling that covers the other shell tool, and one naming `ac_rtk_shared.js` as where the shell-independent half lives.
3. `NAG`, `ignoredLogPath`, `logIgnored` and the whole `process.stdin` block move to `ac_rtk_shared.js`. What stays: the `FILTER` constant (built from the imported `NAG`), `prefixable()` verbatim including `ac_rtk_claude.js:73`'s character class and `:76-81`'s `bash -c 'type -t -- "$1"'` probe, and a `decide` callback implementing the four-step Bash rule of section 4.3.
4. `require("node:fs")` and `require("node:path")` are dropped; `require("node:child_process")` stays for `prefixable`.

Routing behaviour must be observably identical to the frozen file for every command. The only observable differences are `permissionDecisionReason` and the ignored-log tool field.

### 5.2 `docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` (new)

Header comment states: it covers the `PowerShell` tool; PowerShell 7 or newer on any operating system; why the safety question is asked before the rewrite rather than after (section 3.6, with the `ls`-is-`Get-ChildItem` example); and what the prepended function does.

Body: the `FILTER` constant of section 4.4, the `PROBE` constant and `headIsExternal` of section 4.5, and a `decide` callback implementing the three-step PowerShell rule of section 4.3.

### 5.3 `docs/integrations/rtk_claude/hooks/ac_rtk_shared.js` (new)

Exactly section 4.2. Header comment states that it is required by both hooks, that it must stay free of anything shell-specific, and that it is seeded into replicas by the same plain-tree copy that carries the hooks (`config_seed.rs:1155-1201`).

### 5.4 `docs/integrations/rtk_claude/settings.local.json`

`hooks.PreToolUse` becomes two entries. Everything above `"hooks"` (lines 2-8: `includeCoAuthoredBy`, `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `mcpServers`, `claudeMdExcludes`) is unchanged.

```json
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "node .claude/hooks/ac_rtk_claude_Bash.js"
          }
        ]
      },
      {
        "matcher": "PowerShell",
        "hooks": [
          {
            "type": "command",
            "command": "node .claude/hooks/ac_rtk_claude_PowerShell.js"
          }
        ]
      }
    ]
  }
```

Keep the file's existing two-space indentation and its trailing newline.

### 5.5 `docs/integrations/rtk_claude/README.md`

The page currently documents a single Bash-only hook throughout. Every passage below changes. Line numbers are at the frozen SHA. `docs/style-guide.md` binds: lead with a concrete outcome, second person present tense active voice, one concept per H2, show the exact command and its expected first lines, name the exact error string, and avoid the banned word list.

| Lines | Required change |
|---|---|
| 3 | The promise sentence covers two hooks and both shell tools. |
| 5-8 | The copied-file list becomes three entries: `hooks/ac_rtk_claude_Bash.js`, `hooks/ac_rtk_claude_PowerShell.js`, `hooks/ac_rtk_shared.js`, plus `settings.local.json`. State each file's real line count as landed; do not carry `132` forward. |
| 10 | Unchanged. The mirror statement stays true. |
| 14-19 | The replica tree gains the two new files under `.claude/hooks/`. |
| 21-32 | The registration excerpt becomes the two-matcher block of section 5.4. |
| 34 | "Before every `Bash` tool call" becomes both shell tools. |
| 43 | The inline link target changes with the heading at line 58. Update both together or the anchor breaks. |
| 45-56 | The comparison table's "Command" cell lists both hook files. Add that `rtk init` registers `Bash` only, with the section 3.7 evidence, so a reader does not expect RTK's own hook to close the gap. |
| 58-71 | The heading and section stop saying "the `Bash` tool, and nothing else". State what is now covered (`Bash` and `PowerShell`) and what is still not (`Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebFetch`, `Task`, every MCP tool). Keep the "close to complete but not total" list of the two early exits and the rewritten-but-unrecorded case; that text is still correct. |
| 73-160 | The routing rule becomes two rules under one H2, one H3 per shell. The Bash H3 keeps today's text and today's measured table verbatim. The PowerShell H3 is new: the four-step rule of section 4.3, why the order is inverted, and its own measured table (section 9.2). Keep the closing "not a contract" paragraph at 160 and make it cover both. |
| 146-154 | The reproduce snippet becomes one per shell, naming the new filenames, and gains the section 3.9 warning that the copies in this repository cannot be run in place. |
| 158 | The `exec 2> >(grep ...)` paragraph becomes one paragraph per shell, describing the PowerShell shadow function of section 4.4. |
| 162-189 | The `rtk_ignored_tools.md` section states that both hooks append to the same file, gains the tool field in the format list and in the example at 175, and states that lines written before this change carry no tool field. |
| 191-207 | Unchanged. The database section is shell-independent. |
| 209-231 | Failure modes gain the two new ones of section 6.5. The existing three are unchanged. |
| 233-238 | Unchanged. |

### 5.6 `docs/integrations/rtk.md`

One sentence, line 91: it says AgentsCommander seeds "a **different** `PreToolUse` hook" (singular). After this change there are two, one per shell tool. Reword to the plural and keep the link to `rtk_claude/README.md` intact. Nothing else on the page names `ac_rtk_claude.js` or claims the AC hook is Bash-only.

Out of scope, recorded here so it is not lost: line 93 says the `rtk init` excerpt is installed "once per shell tool". Section 3.7 shows RTK registers `Bash` only. That sentence is about **RTK's** hook, not AC's, so correcting it belongs in its own issue. Do not change it here.

---

## 6. Required behaviour, edge cases and failure behaviour

### 6.1 `runHook(hookDir, tool, decide)`

1. Concatenate stdin.
2. `JSON.parse(text || "{}")`. On throw, `process.exit(0)` with no output. Unchanged early exit.
3. `ti = { ...(data.tool_input || {}) }`. Spreading preserves every other field the tool carries (`description`, `timeout`, `run_in_background`, `dangerouslyDisableSandbox`).
4. `body = (typeof ti.command === "string" ? ti.command : "").replace(/^\s+/, "")`. If empty, `process.exit(0)`. Unchanged early exit.
5. `final = decide(body)`.
6. If `final === null`, call `logIgnored(hookDir, tool, body)` and `process.exit(0)` with no output.
7. Otherwise set `ti.command = final` and write the response JSON.

`decide` returns `null` for "hand it back untouched and log it", or the complete replacement command string. It never writes to stdout and never throws.

Response JSON, unchanged apart from the reason:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ac_rtk_claude_<Tool>","updatedInput":{...}}}
```

There is no `deny` path and no blocking exit code in either hook. A hook that runs cannot stop a tool call, however badly it misjudges a command.

### 6.2 Bash edge cases

Every row of the README's measured table (`docs/integrations/rtk_claude/README.md:128-144`) must produce the same outcome after the change as before it. That table is the Bash regression suite.

### 6.3 PowerShell edge cases

Measured against the prototype of section 9.2. `<log>` means handed back untouched and written to `rtk_ignored_tools.md`.

| Command | Outcome | Deciding guard |
|---|---|---|
| `git status` | `rtk git status` | head is `git.cmd`, an Application |
| `git log --oneline -5` | `rtk git log --oneline -5` | as above |
| `git status \| Select-Object -First 2` | `rtk git status \| Select-Object -First 2` | one pipeline, external head, tail untouched |
| `git status > out.txt` | `rtk git status > out.txt` | redirection is left in place |
| `node --version` | `rtk node --version` | `rtk rewrite` empty, prefix fallback |
| `nosuchbinary-xyz` | `rtk nosuchbinary-xyz` | unresolved head is treated as external |
| `python -c "print(1)"` | `rtk python -c "print(1)"` | the parenthesis is inside an argument, not syntax |
| `rtk git status` | `rtk git status` | already routed, filter only |
| `ls` | `<log>` | `Alias` |
| `ls -la` | `<log>` | `Alias` |
| `echo hola` | `<log>` | `Alias` |
| `Get-ChildItem` | `<log>` | `Cmdlet` |
| `git status; ls` | `<log>` | two statements |
| `git status && ls` | `<log>` | `PipelineChainAst` |
| `$x = 1; git status` | `<log>` | two statements, first an assignment |
| `& "C:\Program Files\Git\cmd\git.exe" status` | `<log>` | call operator |
| a command containing a newline | `<log>` | two statements |

Two divergences from Bash are intentional and must be stated in the README, not smoothed over:

- `python -c "print(1)"` and `git status > out.txt` reach the ignored log under Bash, because `ac_rtk_claude.js:73` tests characters textually, and are rewritten under PowerShell, because the parser distinguishes an argument from syntax. PowerShell coverage is wider here.
- Every compound command reaches the ignored log under PowerShell, while Bash rewrites many of them. PowerShell coverage is narrower here, on purpose (section 4.3).

### 6.4 The ignored-log line

```text
20260818_110844 PowerShell: ls | Where-Object { $_.Name }
20260818_110844 Bash: ls | sort > /tmp/now.txt
```

- `YYYYMMDD_HHMMSS`, **local time**, no zone and no offset. Unchanged, and still not comparable by string against the RFC 3339 `timestamp` column of `commands`.
- One space, the tool name exactly as it appears in the matcher (`Bash` or `PowerShell`), then a colon and one space.
- The command is the **original**, not the rewrite, with all whitespace folded to single spaces.
- The tool name comes from the hook's own constant, not from `data.tool_name`. The payload may omit `tool_name` entirely, as the README's own reproduce snippet does, and the hook must still label the line correctly.
- Append only. No truncation, no escaping, no rotation, no deduplication, no locking. Two live replicas of the same agent still append to the same file.
- Lines written before this change carry no tool field. Any reader must tolerate both shapes.

### 6.5 Failure behaviour

Existing modes are unchanged and stay documented: a missing `rtk` on PATH breaks simple commands loudly rather than degrading tracking quietly; an invalid `RTK_DB_PATH` loses the row silently; a failed write to the ignored log is swallowed by an empty `catch`; neither hook can block a tool.

Two new modes, both PowerShell only:

- **The probe cannot run.** If `pwsh` is absent, fails, or prints anything other than `APP`, `headIsExternal` returns false and the command is handed back untouched and logged. **Fail closed.** The cost is one statistic; the alternative is rewriting a command whose shape was never verified, which can silently break it. State this in the README.
- **`rtk` is absent from PATH under PowerShell.** `Get-Command` finds nothing, `$e` falls back to the literal `'rtk'`, and `& 'rtk'` fails with a PowerShell `CommandNotFoundException` on stderr. This mirrors the Bash mode, where the same situation produces exit 127, and it is equally loud.

---

## 7. Compatibility, security and cost

### 7.1 Compatibility

- **Nothing in the product reads these files.** No Rust, no TypeScript, no test and no script references `ac_rtk_claude.js`, the ignored-log format, or `permissionDecisionReason`. The rename cannot break a build.
- **Replicas already on disk** keep working until their next spawn, when the seed is replaced wholesale. During the gap they run the old single hook, which is exactly today's behaviour.
- **The two hooks coexist with RTK's own hook.** Both are `PreToolUse`; a machine can have both active. Only the AC hooks write `rtk_ignored_tools.md`.
- **A reader of an existing `rtk_ignored_tools.md`** sees both line shapes in one file. Section 6.4 requires the README to say so.
- **`docs/integrations/rtk_claude/` stays a byte-faithful mirror** of the workspace `default.claude` tree after section 8.4 is done. Any drift makes the page lie.

### 7.2 Security

No new attack surface. Both hooks pass the command to `spawnSync` as a **single argv entry** with `shell: false`, so nothing re-parses it. The probe passes the command through the environment rather than the command line, so there is no quoting boundary to escape. Neither hook writes anything except an append to a file inside the agent's own origin Matrix, and that append is already wrapped in an empty `catch`. Neither hook can widen a permission: both only ever answer `allow`, which is what the tool call would have got anyway, and neither has a `deny` path.

The prepended PowerShell function shadows the name `rtk` for the duration of one tool call in a shell whose state does not persist between calls. It shadows nothing else.

### 7.3 Cost

Measured on the authoring workstation, three runs each, warm:

| Hook | Per tool call | Spawns |
|---|---|---|
| `ac_rtk_claude_Bash.js` | about 105 ms | `bash` + `rtk` |
| `ac_rtk_claude_PowerShell.js` | about 385 ms | `pwsh` + `rtk` |

The delta is PowerShell's own start-up: the probe alone measures about 240 ms, against about 55 ms for `rtk rewrite`. The `rtk `-prefixed passthrough branch skips both spawns. This cost is accepted deliberately; section 4.5 records why the obvious optimisation is rejected.

### 7.4 Dependency-cycle gate

No Rust or TypeScript module arc is added, removed or moved. Nothing under `src-tauri/`, `src/` or `scripts/` is touched, so no SCC can change, no `cyclicSccs` count can move, and no arc can cross a previously clean SCC boundary. No lower-layer module gains an `AppHandle` or `tauri` dependency, because no Rust module is edited at all.

The only new module relationship is inside the seeded JavaScript tree: `ac_rtk_claude_Bash.js` requires `ac_rtk_shared.js`, and `ac_rtk_claude_PowerShell.js` requires `ac_rtk_shared.js`. `ac_rtk_shared.js` requires neither hook. That is two arcs into one sink, a tree, and it is acyclic by construction.

**Binding acceptance criterion carried into section 9: `ac_rtk_shared.js` must not require either hook file, and the two hooks must not require each other.** A reviewer checks it with `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`, which must show `ac_rtk_shared.js` requiring only `node:fs`, `node:path` and `node:child_process`.

This section states the gate's result at authoring time. The certification pass re-states it after enrichment.

---

## 8. Implementation order

### 8.1 Step 1: extract the shared module, Bash still alone

1. `git mv docs/integrations/rtk_claude/hooks/ac_rtk_claude.js docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js`
2. Create `ac_rtk_shared.js` per section 4.2, moving the code out of the renamed file rather than retyping it.
3. Reduce `ac_rtk_claude_Bash.js` to its filter, `prefixable`, and its `decide`, and add the three-environment statement the issue requires.
4. Point `settings.local.json`'s single existing entry at the new filename.

Gate before continuing: the Bash regression check of section 9.2 passes with the same outcome for every row as the frozen file.

### 8.2 Step 2: add the PowerShell hook

1. Create `ac_rtk_claude_PowerShell.js` per sections 4.4, 4.5 and 5.2.
2. Add the second `PreToolUse` entry to `settings.local.json` per section 5.4.

Gate before continuing: the PowerShell table of section 6.3 reproduces exactly.

### 8.3 Step 3: the documentation

Rewrite `docs/integrations/rtk_claude/README.md` per section 5.5 and touch the one sentence of `docs/integrations/rtk.md` per section 5.6. Do this **after** the hooks are final, so every line count, every quoted snippet and every table row is copied from what actually landed rather than from this plan.

### 8.4 Step 4: the runtime half, which is not ours

The files that take effect live in the workspace `default.claude` tree, outside every repository and outside every agent's write zone. **No agent may write them.** State this verbatim in the pull request description, naming the exact operation:

The user copies, from the landed branch to the workspace tree, replacing what is there:

```text
docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js        ->  <workspace>/.ac/default.claude/hooks/ac_rtk_claude_Bash.js
docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js  ->  <workspace>/.ac/default.claude/hooks/ac_rtk_claude_PowerShell.js
docs/integrations/rtk_claude/hooks/ac_rtk_shared.js             ->  <workspace>/.ac/default.claude/hooks/ac_rtk_shared.js
docs/integrations/rtk_claude/settings.local.json                ->  <workspace>/.ac/default.claude/settings.local.json
```

and deletes `<workspace>/.ac/default.claude/hooks/ac_rtk_claude.js`.

For this workspace, `<workspace>` is `D:\0_repos\AgentsCommander_iac`.

The change takes effect for a given replica at that replica's **next spawn**, when the seeder replaces its `.claude` tree. Already-running sessions keep the old hook until then.

---

## 9. Tests and acceptance criteria

No automated test is added. There is no JavaScript test runner wired to `docs/`, and `package.json:5` is `"type": "module"` while these hooks are CommonJS, so adding one would mean new tooling for four mirror files that no build consumes. The acceptance criteria below are scripted, exact, and each names its owner.

### 9.1 Checkable from the branch alone, by any reviewer, no runtime

| # | Criterion | How |
|---|---|---|
| 1 | `docs/integrations/rtk_claude/hooks/` holds exactly `ac_rtk_claude_Bash.js`, `ac_rtk_claude_PowerShell.js`, `ac_rtk_shared.js`. `ac_rtk_claude.js` is gone. | `git ls-tree -r --name-only HEAD -- docs/integrations/rtk_claude/` |
| 2 | The rename is recorded as a rename, not as a delete plus an add. | `git log --follow --oneline -- docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js` |
| 3 | `settings.local.json` parses and declares exactly two `PreToolUse` entries, matchers `Bash` and `PowerShell`, each pointing at its own hook file. | `node -e "const s=require('fs').readFileSync('docs/integrations/rtk_claude/settings.local.json','utf8');const h=JSON.parse(s).hooks.PreToolUse;console.log(h.map(e=>e.matcher+' -> '+e.hooks[0].command).join('\n'))"` |
| 4 | No `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%`, `%AC_MATRIX_ROOT%` or `%USER_HOME%` appears in any of the four files (section 3.4). | `git grep -n -e '%AC_REPLICA_ROOT%' -e '%AC_WORKSPACE_ROOT%' -e '%AC_MATRIX_ROOT%' -e '%USER_HOME%' -- docs/integrations/rtk_claude/` returns nothing. |
| 5 | The JavaScript module graph is a tree (section 7.4). | `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`: `ac_rtk_shared.js` requires only `node:` builtins; neither hook requires the other. |
| 6 | `ac_rtk_claude_Bash.js` states in the file that it runs under Windows with Git Bash, Linux and macOS. | Read the header comment. Required by the issue. |
| 7 | The README names three hook files with their real line counts, shows the two-matcher block, and no longer claims the hook covers the `Bash` tool and nothing else. | `git grep -n "ac_rtk_claude" -- docs/` returns no hit on the bare old filename. |
| 8 | `docs/integrations/rtk.md:91` no longer says a single hook is seeded, and its link to `rtk_claude/README.md` still resolves. | Read the line. |
| 9 | `rtk-replica-history.db` is still present at the repository root and still untracked. | `git status --porcelain=v1 --untracked-files=all` reports exactly `?? rtk-replica-history.db`. |

### 9.2 Behaviour, provable from the branch, by any reviewer with `node`, `rtk` and `pwsh`

The hooks **cannot** be run in place: `package.json:5` makes every `.js` in this repository an ES module and `require` is not defined there (section 3.9). Copy them out first. Use a scratch directory at your **replica root**, never inside `.claude/`.

```bash
mkdir -p "$AGENTSCOMMANDER_ROOT/1416-check"
cp docs/integrations/rtk_claude/hooks/*.js "$AGENTSCOMMANDER_ROOT/1416-check/"
cd "$AGENTSCOMMANDER_ROOT/1416-check"
echo '{"tool_name":"PowerShell","tool_input":{"command":"git status"}}' | node ac_rtk_claude_PowerShell.js
```

Expected: one line of JSON whose `updatedInput.command` is the section 4.4 function definition, a newline, then `rtk git status`, and whose `permissionDecisionReason` is `ac_rtk_claude_PowerShell`.

```bash
echo '{"tool_name":"PowerShell","tool_input":{"command":"ls"}}' | node ac_rtk_claude_PowerShell.js
```

Expected: no output at all, exit 0. `ls` is a PowerShell alias.

| # | Criterion | Owner |
|---|---|---|
| 10 | Every row of section 6.3 reproduces exactly from the scratch directory. | reviewer |
| 11 | Every row of `README.md:128-144` still reproduces through `ac_rtk_claude_Bash.js`. This is the Bash no-regression suite. | reviewer |
| 12 | No run in the scratch directory writes to any `rtk_ignored_tools.md`. `ignoredLogPath` returns `null` unless the hook sits two levels under a directory whose name starts with `__`, so the whole check is side-effect free. Confirm by listing the origin Matrix before and after. | reviewer |
| 13 | The emitted PowerShell command runs. Paste one full `updatedInput.command` from criterion 10 into a real `PowerShell` tool call inside a git repository. The RTK notice must be absent, RTK's compacted output present, and `$LASTEXITCODE` must equal what the unwrapped command returns. | reviewer |

### 9.3 Checkable only after the runtime copies are applied

These cannot be proved from the branch, because the files that take effect are outside every repository and are applied by hand.

| # | Criterion | Owner |
|---|---|---|
| 14 | The four files of section 8.4 are in `<workspace>/.ac/default.claude/`, and `ac_rtk_claude.js` is deleted there. | **the user**, per the decision recorded in the issue comment |
| 15 | A freshly spawned replica has all three hook files under `.claude/hooks/` and the two-matcher `settings.local.json`. | tech-lead, or `ac-cli-tester`, after the user reports criterion 14 done |
| 16 | In that replica, a `PowerShell` tool call running `git status` produces a `commands` row in that agent's `rtk-matrix-history.db`, and a `PowerShell` tool call running `Get-ChildItem` produces a `rtk_ignored_tools.md` line ending in `PowerShell: Get-ChildItem`. | tech-lead, or `ac-cli-tester` |
| 17 | In the same replica, a `Bash` tool call still behaves as it did before, and its ignored-log lines now carry the `Bash` field. | tech-lead, or `ac-cli-tester` |

Criterion 16 is the one that proves the issue is closed. Until it is reported, this change is landed but unproven, and the pull request description must say so rather than implying the gap is measured shut.

---

## 10. Non-goals, binding on the implementer

- **No Rust, no TypeScript, no `src-tauri/`, no `src/`, no `scripts/`, no `.github/`.** The seeder needs no change; it already copies whatever is in the tree.
- **Do not touch the repository's own `.claude/settings.json`.** Its inline `@ac-rtk-marker-v2` one-liner at `:3-9` has the identical `Bash`-only blind spot and is filed as #1424. Keeping it out is what makes this issue reviewable against its own criteria.
- **Do not write anything under `<workspace>/.ac/default.claude/`.** It is outside every agent's write zone. Section 8.4 is the user's step.
- **Do not add a cleanup step for stale `ac_rtk_claude.js` files in existing replicas.** Section 4.1 explains why none is needed.
- **Do not commit, delete or `.gitignore` `rtk-replica-history.db`.**
- **Do not rename the hooks to `.cjs`** to work around section 3.9. The issue fixes the two filenames, and `.cjs` would change what the seeder installs and what `settings.local.json` invokes for no runtime benefit.
- **Do not add a `package.json` to `docs/integrations/rtk_claude/hooks/`.** The seeder would copy it into every replica.
- **Do not add a textual pre-filter to the PowerShell probe** (section 4.5).
- **Do not append `exit $LASTEXITCODE`** to the emitted PowerShell command (section 4.4).
- **Do not correct `docs/integrations/rtk.md:93`** (section 5.6).
- **Do not add a test runner, a markdown linter or a link checker.** None exists in `.github/workflows/`.
