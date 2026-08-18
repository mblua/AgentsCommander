# Plan #1416: Split the RTK hook per shell and cover the PowerShell tool

Author: architect, wg-17. Authored 2026-08-18 UTC on the Full delivery path.

Status: READY_FOR_IMPLEMENTATION

Certified by architect on 2026-08-18 UTC after the `dev-rust` and `dev-rust-grinch` enrichment passes. Both reviewers' findings are resolved into sections 1 to 10, which are the **sole** implementation specification. Appendices A and B preserve their measurements as evidence and are **not** specification; where an appendix conflicts with sections 1 to 10, sections 1 to 10 govern. Section 11 maps every finding to where it landed.

One decision in this plan overrules both reviewers' recommendation: section 4.4 no longer prepends anything to a routed PowerShell command. The reasoning and the measurements are in section 4.4 and section 11.

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

1. **The stderr filter.** `ac_rtk_claude.js:35` builds `FILTER` as `exec 2> >(grep --line-buffered -v 'No hook installed' >&2)\n` and `:120` prepends it to every rewrite. `exec 2> >(...)` is bash process substitution. PowerShell has no statement that redirects the rest of a script's error stream, and every construct that emulates one costs more than the notice it removes. Section 4.4 settles this: the PowerShell hook prepends nothing.

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

**`rtk rewrite` never touches an element after a pipe.** This is load-bearing under section 4.3 and it is measured across seven shapes, including ones where the element after the pipe is itself in the hazard class:

| Input to `rtk rewrite` | stdout |
|---|---|
| `git status \| ls` | `rtk git status \| ls` |
| `git status \| sort` | `rtk git status \| sort` |
| `git status \| cat` | `rtk git status \| cat` |
| `git status \| diff a b` | `rtk git status \| diff a b` |
| `git status \| ps` | `rtk git status \| ps` |
| `git log \| head -5` | `rtk git log \| head -5` |
| `curl -s http://x \| cat` | `rtk curl -s http://x \| cat` |

`rtk rewrite` rewrites only the head of each `;` / `&&` segment. Since the section 4.5 guards already reduce the command to one statement containing one pipeline, `rtk rewrite` can only ever touch the single head the probe just cleared.

That closes the hazard in this section, which is about names PowerShell **owns**. It does not close the two hazards in section 3.10, and section 4.3 must not be read as if it did.

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

### 3.10 Two hazards the `Application` gate does not close

Both are measured, both survive every guard in section 4.5, and both fail at the section 3.6 bar: empty or wrong output, exit 0, nothing on stderr. Neither is a blocker, for the reason given at the end of this section, but the plan must not read as an all-clear and neither must the README.

**Hazard A: names Windows owns with different semantics from the POSIX tool `rtk` assumes.** These resolve as `CommandType = Application`, so the gate passes them, the rewrite applies, and the command breaks silently.

| Command | Native, in a real `PowerShell` tool call | After the rewrite |
|---|---|---|
| `tree` | folder listing, exit 0 | `Too many parameters - node_modules\|.git\|target\|...`, **exit 0**, no listing |
| `find "NAG" ac_rtk_shared.js` | the two matching lines, exit 0 | **no output at all, exit 0** |

`tree` resolves to `C:\WINDOWS\system32\tree.com` and `find` to `C:\WINDOWS\system32\find.exe`. `rtk tree` passes GNU-`tree` ignore flags (`-I <pattern>`) that `tree.com` rejects while still exiting 0; `rtk find` applies GNU `find` semantics to arguments meant for `find.exe`.

The class was bounded by measurement, not estimated. Of the heads `rtk rewrite` actually rewrites, these resolve as `Application` on this workstation: `git`, `curl`, `rg`, `jq`, `find`, `tree`. `curl`, `jq`, `rg` and `tar` were compared native against `rtk <head>` and are byte-identical on `--version`; `more` through the section 4.3 step 4 prefix fallback is also identical. **`find` and `tree` are the whole class here.** `ls`, `ps`, `diff`, `cat` and `sort` never reach the rewrite because they are aliases, which is section 4.5 working as designed.

**Hazard B: the pipeline tail parses content the head no longer produces.** This one does not depend on the head at all.

```text
git status | Select-String "nothing added to commit"
```

Every guard passes: it parses, it is one statement, that statement is one `PipelineAst`, the first element is a `CommandAst`, the invocation operator is `Unknown`, `GetCommandName()` returns `git`, the name has no `=`, and `git` resolves as `Application`. The hook routes it to `rtk git status | Select-String "nothing added to commit"`. Measured, running both verbatim:

| | results | exit | `$?` | stderr |
|---|---|---|---|---|
| what the agent typed | **1** | 0 | True | none |
| what the hook makes the tool run | **0** | 0 | True | none |

The cause is that `rtk` does not merely compact `git status`, it reformats it: six lines become two in a different shape. The **type** contract is preserved, because both sides emit strings. The **content** contract is not, and a pipeline tail consumes content, not type.

Bounded by measurement. Comparing native against `rtk` for the same argv: `git rev-parse HEAD`, `git status --porcelain` (apart from a dropped trailing newline), `git log --oneline -3`, `git diff --name-only`, `git branch --show-current`, `git branch`, `git show --stat HEAD` and `rg -n <pat> <file>` are all identical. `git status`, `git log` in its default format, and `git diff HEAD~1 --stat` are **not**. The class is those reformatted subcommands combined with any tail that reads the text: `Select-String`, `-match`, `ConvertFrom-*`, `Where-Object { $_ -like ... }`, `Measure-Object -Line`, or a `> file` whose contents are parsed later.

**Why neither is a blocker, and why no in-hook fix is specified.** Both classes exist today under Bash: the current hook rewrites `git status | grep "nothing added"` identically, and `ac_rtk_claude_Bash.js` under Git Bash has the same `tree` and `find` exposure wherever the resolved binary is the Windows one. **This issue neither creates nor widens either class.** The only in-hook fix would be a deny-list, and it cannot work: a list keys on the **head**, while hazard B is decided by the **tail**. `git` breaks with `Select-String` and does not break with `Select-Object -First 2`. No enumeration of names can express that, and the list would be exactly the drifting second copy of a platform question that section 4.5 exists to avoid.

Disposition, binding:

1. Section 4.3's safety argument covers names PowerShell owns and nothing more. It is worded that way and points here.
2. Section 6.3's table carries `tree`, `find` and `git status | Select-String` as known-broken rows rather than omitting them.
3. Section 6.5 carries both as failure modes and the README carries both, `tree` and `git status | Select-String` being the cleanest one-line demonstrations.
4. The upstream fix belongs in `rtk`, whose subcommands assume POSIX tools and reformat content. **Filing that against `rtk` is a follow-up for the tech-lead, not work in this issue, and not a gate on it.**

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
| `NAG` | The string `"No hook installed"`. Only `ac_rtk_claude_Bash.js` uses it, to build its filter. It lives here because the notice is a property of `rtk`, not of a shell, and section 4.4 explains why the PowerShell hook does not filter it. |
| `ALREADY_RTK` | The regex `/^rtk\s/`. |
| `rtkRewrite(cmd)` | `spawnSync("rtk", ["rewrite", cmd], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], shell: false, timeout: 5000 })`, returns `(r.stdout \|\| "").trim()`. Passing `cmd` as a single argv entry with `shell: false` is what keeps the shell from re-parsing it. The `timeout` is required, not optional: a killed call yields empty stdout, which both routing rules already handle. |
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
  1. body starts with "rtk "        -> body, unchanged
  2. NOT headIsExternal(body)       -> ignored log
  3. rtk rewrite prints something   -> that output
  4. otherwise                      -> "rtk " + body
```

The PowerShell hook prepends nothing. That is not an omission; section 4.4 is the decision and its measurements.

`headIsExternal` returning true means: the command is a single statement, that statement is a single pipeline, the pipeline's **first** element is a plain command invocation with no call operator, its command name is a bare name containing no `=`, and that name either resolves to `CommandType = Application` or does not resolve at all.

Once that holds, accepting `rtk rewrite`'s output is safe **against the section 3.6 hazard, which is names PowerShell owns**, and against nothing more:

- Only one `;` / `&&` segment exists, so `rtk rewrite` can only replace that one head. There is no second segment for it to mis-rewrite.
- Any pipeline tail is left untouched by `rtk rewrite`, measured across seven shapes in section 3.6.
- The head was already an external binary, so it already emitted **strings** into the PowerShell pipeline. Replacing it with `rtk <head>`, which also emits strings, preserves the pipeline's **type** contract.

**The gate does not preserve the content contract, and it does not know what the resolved binary is.** Section 3.10 measures both gaps: `tree` and `find` resolve as `Application` and break silently, and any tail that parses the head's text breaks when `rtk` reformats it. Read this gate as "PowerShell will not misinterpret the rewritten command", never as "the rewritten command does what the agent asked". Neither gap is created or widened by this issue and neither has an in-hook fix; section 3.10 carries the disposition.

Everything else goes to the ignored log. That is deliberate: a missed rewrite costs one statistic, a wrong rewrite silently breaks the agent's command.

### 4.4 The PowerShell hook prepends nothing, and why

**Decision: `ac_rtk_claude_PowerShell.js` emits the routed command on its own, with no prepended statement of any kind.** The `rtk` notice is not silenced under PowerShell. This overrules the recommendation both reviewers reached; the measurements that changed the answer are below.

#### What the bash hook does, and why it does not port

`ac_rtk_claude.js:35` prepends `exec 2> >(grep --line-buffered -v 'No hook installed' >&2)` as its own statement. It costs nothing: bash's fd 2 is reassigned for the rest of the script, `rtk` stays an ordinary external command, and a per-command `2> file` still overrides it.

PowerShell has no equivalent statement. Every construct that filters the notice has to turn `rtk` from an `Application` into a **function**, and that is what does the damage. The candidate that survived the longest, and that earlier drafts of this plan specified, was:

```powershell
function rtk { $e = (Get-Command rtk -CommandType Application ...).Source; & $e @args 2>&1 | ForEach-Object { ... } }
```

#### What the shadow function costs, measured

Same failing command, same session, filter absent then present. Every row was re-measured at certification time, independently of the two enrichment passes that found them:

| Channel | `rtk` invoked directly | Through the shadow function | Visible to the agent? |
|---|---|---|---|
| `$?` after a failure | `False`, correct | **`True`** | no |
| `$LASTEXITCODE` | `1` | `1`, preserved | n/a |
| `$c = cmd 2>&1` | 2 records, error present | **0 records** | no |
| `cmd 2> file` | 140 bytes captured | **0 bytes**, text leaks to the console | partly |
| `cmd 2>$null` | silenced | **still printed** | yes |
| `cmd > file`, LF text | 16 bytes, LF preserved | **20 bytes, every `0a` became `0d 0a`** | no |
| `cmd > file`, binary | bytes preserved | **`89`, `fe`, `ff` each became `ef bf bd`**, unrecoverable | no |
| `--format=a,b` argv | `argc=1` | **`argc=2`, split on the comma** | sometimes |
| `cmd &` background job | completes, returns output | completes; the function does not reach the job's runspace, so the notice leaks there | yes |

`$?` breaking is the worst of them, because `cmd; if ($?) { ... }` is an ordinary shape that then takes the wrong branch with no output difference. The `> file` re-encoding is next: `rtk git diff > fix.patch` followed by `git apply fix.patch` fails or applies wrongly, and `rtk curl -s <url> > archive.zip` is destroyed outright.

Cause, for each: `$?` after a **function** reflects whether that function raised an error, not the wrapped binary's exit code, and the shadow raises none. `[Console]::Error.WriteLine` writes to the process stderr **handle**, while `2>` redirects PowerShell's **error stream**; they are different channels. `ForEach-Object` decodes the binary's stdout into PowerShell strings and re-emits them, so anything downstream receives re-encoded text rather than bytes. And PowerShell evaluates each argument before `@args` splats it, so in argument mode an unquoted comma builds an array that splats as two argv entries.

The `Write-Error -ErrorRecord $_` variant was measured too. It restores `2>`, `2>&1` and `2>$null`, and **only** those: `$?` stays `True`, and redirected stdout is corrupted identically, because the damage is on the stdout branch. It also decorates every error line with PowerShell's ConciseView position block, turning a 140-byte message into 155 bytes across five lines, in a hook whose purpose is compacting output. One channel restored out of four broken, at a per-error-line tax.

#### What is given up instead

The notice, `[rtk] /!\ No hook installed`, appears on stderr. Measured, it is **not** throttled: it appears on every filtered `rtk` invocation, six out of six in a row. So the cost is one stderr line per routed PowerShell command, and one extra record in a `2>&1` capture.

Measured with no prepended statement at all, every channel is native:

| Channel | Result |
|---|---|
| `$?` after a failure | `False`, correct |
| `$LASTEXITCODE` | `1`, correct |
| `$c = cmd 2>&1` | full capture, of which exactly 1 record is the notice |
| `cmd 2> file` | 753 bytes captured, works |
| `cmd > file`, LF text | 16 bytes, `61 6c 70 68 61 0a ...`, LF preserved |
| `--format=a,b` argv | `argc=1`, correct |
| `cmd &` background job | completes, returns `f0031f53` |

#### The trade, stated plainly

One cosmetic stderr line per routed command, against four broken channels of which three are silent, one of which corrupts files. **The notice is not worth it.** This is a deliberate, measured asymmetry with `ac_rtk_claude_Bash.js`, and both hooks' header comments and the README must say so rather than leaving a reader to assume the PowerShell hook simply forgot.

Everything sections 11.2, 12.2, 12.3 and 12.4 of the appendices required to be documented is dissolved by this decision rather than annotated: those defects do not exist in a hook that prepends nothing. What survives from them is the one line above about the notice, and the section 3.10 hazards, which are unrelated to the filter.

**Binding on the implementer:** do not add a `rtk` shadow function, a `Write-Error` variant, an `exit $LASTEXITCODE` trailer, a `2>$null` suffix, or any other prepended or appended statement to the PowerShell hook's output. `ti.command` is exactly the routed command and nothing else.

### 4.5 The PowerShell safety predicate

`headIsExternal(cmd)` spawns PowerShell once and asks the PowerShell parser and `Get-Command`, so there is no character class and no keyword list in the hook to drift. This is the same principle the bash hook states at `ac_rtk_claude.js:69-71`, expressed in the other shell.

The command is passed through the environment, not through the command line, so no quoting question arises:

```js
const r = spawnSync("pwsh", ["-NonInteractive", "-NoLogo", "-Command", PROBE], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "ignore"],
  shell: false,
  timeout: 5000,
  env: { ...process.env, AC_RTK_CMD: cmd },
});
const marked = (r.stdout || "").split(/\r?\n/).filter((l) => l.startsWith("AC_RTK_VERDICT:"));
return marked.length > 0 && marked[marked.length - 1].trim() === "AC_RTK_VERDICT:APP";
```

`PROBE` is these seventeen statements joined with `\n`:

```powershell
$ErrorActionPreference = 'SilentlyContinue'
$t = $null; $e = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($env:AC_RTK_CMD, [ref]$t, [ref]$e)
if ($e.Count) { 'AC_RTK_VERDICT:SHELL'; exit 0 }
$st = $ast.EndBlock.Statements
if ($st.Count -ne 1) { 'AC_RTK_VERDICT:SHELL'; exit 0 }
$p = $st[0] -as [System.Management.Automation.Language.PipelineAst]
if (-not $p) { 'AC_RTK_VERDICT:SHELL'; exit 0 }
$c0 = $p.PipelineElements[0] -as [System.Management.Automation.Language.CommandAst]
if (-not $c0) { 'AC_RTK_VERDICT:SHELL'; exit 0 }
if ($c0.InvocationOperator -ne 'Unknown') { 'AC_RTK_VERDICT:SHELL'; exit 0 }
$n = $c0.GetCommandName()
if (-not $n) { 'AC_RTK_VERDICT:SHELL'; exit 0 }
if ($n -like '*=*') { 'AC_RTK_VERDICT:SHELL'; exit 0 }
$g = Get-Command -Name $n -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $g) { 'AC_RTK_VERDICT:APP'; exit 0 }
if ($g.CommandType -eq 'Application') { 'AC_RTK_VERDICT:APP' } else { 'AC_RTK_VERDICT:SHELL' }
```

**Both the sentinel and the `timeout` are required, and each closes a measured way the hook silently stops working.**

The sentinel exists because the probe loads the user profile (see below), and a profile that writes to stdout would otherwise prepend its text to the verdict. Comparing the whole trimmed buffer against `APP` then fails for **every** command: the probe answers correctly, the hook discards the answer, every `PowerShell` call goes to the ignored log, `commands` gains zero rows, and nothing reports why. `oh-my-posh`, `starship`, `conda init`, PSReadLine tips and corporate login banners all print on profile load. Measured against four preludes, including one that prints the literal `APP`, the sentinel form returns the correct verdict in every case while a whole-buffer comparison fails in three of them. Scanning for the **last** line carrying the marker also survives output emitted after the verdict, which a last-non-empty-line comparison does not.

The `timeout` exists because the profile runs on every tool call, and a profile that blocks, prompts or waits on the network would otherwise hang the hook and with it the tool call, with nothing in any log. Measured: a probe sleeping 30 s hangs with no `timeout`, and is killed at 1514 ms with `timeout: 1500`, yielding empty stdout, which the fail-closed disposition of section 6.5 already handles unchanged. `5000` is ample: the probe measures 264 ms here, 203 ms of which is bare `pwsh` startup, so a five-second budget tolerates a profile 20 times slower than this machine's.

What each guard buys, all measured:

| Guard | Rejects |
|---|---|
| `$e.Count` | Text PowerShell cannot parse. |
| `$st.Count -ne 1` | `git status; ls`, and any command whose newline is a **statement separator**. A newline inside a string literal, a here-string, or after a backtick line continuation keeps the input a single statement, so `git commit -m "line one\nline two"` routes, and measurement confirms `rtk rewrite` handles the embedded newline correctly. |
| not a `PipelineAst` | `$x = 1`, `git status && ls`, `git status \|\| ls` (both become a `PipelineChainAst`), `if (...) { ... }`, `foreach (...) { ... }`. |
| not a `CommandAst` | A bare expression such as `(Get-Date)`. |
| `InvocationOperator -ne 'Unknown'` | `& "C:\Program Files\Git\cmd\git.exe" status` and `. .\script.ps1`. |
| `GetCommandName()` empty | A head that is not a literal, such as `& $exe`. |
| `$n -like '*=*'` | The bash `FOO=bar cmd` form, which PowerShell would otherwise parse as a command literally named `FOO=bar` and `rtk rewrite` would turn into the bash-only `FOO=bar rtk ls`. It fires on the command **name** only and deliberately not on an argument, so `git log --pretty=format:%h` routes, and must. |
| `CommandType -ne 'Application'` | Every PowerShell alias, function, filter, cmdlet, external script and configuration, which is the section 3.6 hazard. |

Two deliberate choices inside the probe:

- **`-NoProfile` is not passed.** The probe must see the same command table the real session will use. A profile that defines a function or alias would otherwise be invisible to the probe, and the hook would prefix `rtk ` onto a name PowerShell owns; measured, a prelude of `function git { 'shadowed' }` correctly flips `git status` from `APP` to `SHELL`.

  An earlier draft of this plan claimed that loading the profile "can only make the probe more conservative, never less". **That claim is false and is struck.** A profile that *removes* a name moves the verdict the other way: measured, a prelude of `Remove-Alias -Name ls -Force` makes the probe answer `APP` for `ls`, which section 3.6 measured as broken on Windows outside Git Bash. `Remove-Alias ls` is a real idiom on machines with `eza` or `lsd` installed. The consequence there is loud rather than silent, so this is a correction to the argument rather than a new hazard, and the reason for loading the profile still stands on its remaining leg: the probe must answer for the command table that will actually run.

- **No textual fast path.** A cheap JS pre-filter on `;`, `&&`, newline and so on would skip the probe for most commands and save roughly 240 ms, but it would be a second copy of the shell-syntax question living beside the parser, which is exactly the drift `ac_rtk_claude.js:69-71` was written to avoid. Rejected deliberately, and the measurement strengthens the rejection rather than weakening it: **203 ms of the probe's 264 ms is bare `pwsh` process startup**, before the probe does any work, so no cheaper probe exists and the only lever is not spawning `pwsh` at all. See section 7.3 for the cost that is being accepted.

- **One assumption the hook cannot verify, to be stated in its header comment.** The probe spawns `pwsh`, and the harness's `PowerShell` tool is `pwsh` 7.6.5 Core here, so probe and session share a command table. On a host where the tool runs Windows PowerShell 5.1, `pwsh` is either absent, in which case every command fails closed to the ignored log, which is degraded but honest, or present and answering for a different command table than the one that will run. The hook cannot detect this. Say it once, in the file.

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
2. The header comment gains one sentence pointing at `ac_rtk_claude_PowerShell.js` as the sibling that covers the other shell tool, one naming `ac_rtk_shared.js` as where the shell-independent half lives, and one stating that this hook filters the `rtk` notice while the PowerShell hook deliberately does not, with a pointer to section 4.4's reason.
3. `NAG`, `ignoredLogPath`, `logIgnored` and the whole `process.stdin` block move to `ac_rtk_shared.js`. What stays: the `FILTER` constant (built from the imported `NAG`), `prefixable()` verbatim including `ac_rtk_claude.js:73`'s character class and `:76-81`'s `bash -c 'type -t -- "$1"'` probe, and a `decide` callback implementing the four-step Bash rule of section 4.3.
4. `require("node:fs")` and `require("node:path")` are dropped; `require("node:child_process")` stays for `prefixable`.

Routing behaviour must be observably identical to the frozen file for every command. The only observable differences are `permissionDecisionReason` and the ignored-log tool field.

### 5.2 `docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` (new)

Header comment states, each as its own short paragraph:

1. It covers the `PowerShell` tool, PowerShell 7 or newer, on any operating system.
2. Why the safety question is asked before the rewrite rather than after, with the `ls`-is-`Get-ChildItem` example from section 3.6.
3. That this hook prepends nothing, unlike `ac_rtk_claude_Bash.js`, so the `rtk` notice is not filtered, and why: every construct that filters it turns `rtk` into a function, which breaks `$?`, stderr redirection, redirected stdout bytes and comma-bearing arguments. Section 4.4.
4. That the probe spawns `pwsh` and cannot verify that the harness's `PowerShell` tool is the same PowerShell, per section 4.5.
5. That the gate does not make a routed command correct, only unambiguous to PowerShell, with a pointer to the two section 3.10 hazards.

Body: the `PROBE` constant and `headIsExternal` of section 4.5, and a `decide` callback implementing the four-step PowerShell rule of section 4.3. There is no `FILTER` constant in this file.

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
| 158 | The `exec 2> >(grep ...)` paragraph becomes one paragraph per shell. The Bash paragraph is unchanged. The PowerShell paragraph states that this hook prepends **nothing**, that the `rtk` notice therefore appears on stderr once per routed command, and why: every construct that filters it turns `rtk` into a function, which breaks `$?`, `2>`, redirected stdout bytes and comma-bearing arguments (section 4.4). It must also state the positive half, that a routed PowerShell command is otherwise native in every channel. |
| 162-189 | The `rtk_ignored_tools.md` section states that both hooks append to the same file, gains the tool field in the format list and in the example at 175, and states that lines written before this change carry no tool field. |
| 191-207 | Unchanged. The database section is shell-independent. |
| 209-231 | Failure modes gain **all four** new ones of section 6.5: the probe failing or timing out and its degenerate zero-rows case; `rtk` absent from PATH under PowerShell; the unfiltered notice; and the two section 3.10 hazards. The last is carried with both demonstrations, `tree` losing its listing at exit 0 and `git status \| Select-String` returning zero results where the native command returns one. The existing three modes are unchanged. |
| 191-207 | The database section gains one sentence: under PowerShell an unresolved head is more often a mistyped cmdlet than a missing binary, so `parse_failures` will see more traffic than it does from Bash. That growth is expected, not a regression. |
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
| `rtk git status` | `rtk git status`, unchanged | already routed, nothing to do |
| `ls` | `<log>` | `Alias` |
| `ls -la` | `<log>` | `Alias` |
| `echo hola` | `<log>` | `Alias` |
| `Get-ChildItem` | `<log>` | `Cmdlet` |
| `git status; ls` | `<log>` | two statements |
| `git status && ls` | `<log>` | `PipelineChainAst` |
| `$x = 1; git status` | `<log>` | two statements, first an assignment |
| `& "C:\Program Files\Git\cmd\git.exe" status` | `<log>` | call operator |
| a newline used as a statement separator | `<log>` | two statements |
| `git commit -m "line one<LF>line two"` | `rtk git commit -m "line one<LF>line two"` | a newline **inside a string** does not split a statement |

All of the rows above reproduce exactly against the prototype, independently confirmed by `dev-rust`.

**Known-broken rows.** These are routed and the routed command is wrong. They are listed rather than omitted so this table is not read as an all-clear. Section 3.10 has the measurements and the reason no in-hook fix is specified.

| Command | Outcome | Why it still breaks |
|---|---|---|
| `tree` | `rtk tree`, and the listing is **lost**, exit 0 | `tree.com` is an `Application`, so the gate passes it; `rtk` passes it GNU-`tree` flags |
| `find "NAG" f.js` | `rtk find "NAG" f.js`, and the matches are **lost**, exit 0 | `find.exe` is an `Application`; `rtk` applies GNU `find` semantics |
| `git status \| Select-String "nothing added to commit"` | routed, and returns **0 results instead of 1**, exit 0 | the head is fine; `rtk` reformats `git status`, and the untouched tail parses the old format |

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
- Append only. No truncation, no escaping, no rotation, no deduplication, no locking. Two live replicas of the same agent still append to the same file, and after this change the two hooks can also append concurrently **within one replica**, because `Bash` and `PowerShell` tool calls issued in the same assistant turn run in parallel. `fs.appendFileSync` with the default `a` flag is not guaranteed atomic on Windows. The blast radius is one interleaved line in a human-read log, so no locking is warranted; the README should say both cases rather than only the cross-replica one.
- Lines written before this change carry no tool field. Any reader must tolerate both shapes.

### 6.5 Failure behaviour

Existing modes are unchanged and stay documented: a missing `rtk` on PATH breaks simple commands loudly rather than degrading tracking quietly; an invalid `RTK_DB_PATH` loses the row silently; a failed write to the ignored log is swallowed by an empty `catch`; neither hook can block a tool.

Four new modes, all PowerShell only, all measured. The README must carry all four.

- **The probe cannot run, or is killed by its `timeout`.** If `pwsh` is absent, fails, hangs past 5 s, or emits no `AC_RTK_VERDICT:` line, `headIsExternal` returns false and the command is handed back untouched and logged. **Fail closed.** The cost is one statistic; the alternative is rewriting a command whose shape was never verified, which can silently break it. The degenerate case reads alarming and is not: with `pwsh` missing entirely, **every** `PowerShell` command goes to the ignored log and `commands` gains zero rows. That is still strictly better than today, where those calls produce nothing anywhere, and the ignored log makes the total failure obvious on first read instead of invisible.

- **`rtk` is absent from PATH under PowerShell.** The routed command is `rtk ...`, so PowerShell reports `CommandNotFoundException` on stderr and the command fails. This mirrors the Bash mode, where the same situation produces exit 127, and it is equally loud.

- **The `rtk` notice is not filtered.** One `[rtk] /!\ No hook installed` line on stderr per routed command, measured as not throttled, and one extra record in a `$c = cmd 2>&1` capture. This is the deliberate trade of section 4.4 and the one behavioural difference from the Bash hook that an agent will see.

- **A routed command can be wrong while looking fine.** The two section 3.10 hazards: `tree` and `find` lose their output with exit 0, and any pipeline tail that parses text breaks when `rtk` reformats the head's output, `git status | Select-String` being the cleanest case. Neither is created by this issue and neither has an in-hook fix.

One mode that does **not** exist, recorded because an earlier draft of this plan created it and it may be reintroduced by someone porting the Bash hook's filter: routed commands under PowerShell preserve `$?`, `$LASTEXITCODE`, `2>`, `2>&1`, `2>$null`, redirected stdout bytes, argv fidelity and background jobs, because the hook prepends nothing. Section 4.4 measures each. Adding any wrapper breaks four of them, three silently.

---

## 7. Compatibility, security and cost

### 7.1 Compatibility

- **Nothing in the product reads these files.** No Rust, no TypeScript, no test and no script references `ac_rtk_claude.js`, the ignored-log format, or `permissionDecisionReason`. The rename cannot break a build.
- **Replicas already on disk** keep working until their next spawn, when the seed is replaced wholesale. During the gap they run the old single hook, which is exactly today's behaviour.
- **The two hooks coexist with RTK's own hook.** Both are `PreToolUse`; a machine can have both active. Only the AC hooks write `rtk_ignored_tools.md`.
- **A reader of an existing `rtk_ignored_tools.md`** sees both line shapes in one file. Section 6.4 requires the README to say so.
- **`docs/integrations/rtk_claude/` stays a byte-faithful mirror** of the workspace `default.claude` tree after section 8.4 is done. Any drift makes the page lie.
- **A routed PowerShell command behaves natively in every channel.** `$?`, `$LASTEXITCODE`, `2>`, `2>&1`, `2>$null`, redirected stdout bytes, argv fidelity and background jobs are all unchanged from running the command without the hook, because the hook prepends nothing (section 4.4). The single observable difference is one unfiltered `rtk` notice line on stderr per routed command.
- **A routed command is not guaranteed to be *correct*.** The gate makes it unambiguous to PowerShell, not equivalent to what the agent asked for. Section 3.10's two hazards survive, unchanged from their existing Bash exposure.

### 7.2 Security

No new attack surface. Both hooks pass the command to `spawnSync` as a **single argv entry** with `shell: false`, so nothing re-parses it. The probe passes the command through the environment rather than the command line, so there is no quoting boundary to escape. Neither hook writes anything except an append to a file inside the agent's own origin Matrix, and that append is already wrapped in an empty `catch`. Neither hook can widen a permission: both only ever answer `allow`, which is what the tool call would have got anyway, and neither has a `deny` path.

The PowerShell hook prepends no statement at all, so it defines no function, shadows no name and mutates no shell state. That is a smaller surface than the Bash hook's `exec` line, not a larger one.

### 7.3 Cost

Measured on the authoring workstation and independently re-measured by `dev-rust` over five warm runs each:

| Step | Median | Min | Max |
|---|---|---|---|
| `pwsh` probe, profile loaded | 264 ms | 260 | 271 |
| `pwsh` probe, `-NoProfile` | 268 ms | 262 | 275 |
| `pwsh -Command "exit 0"`, bare startup | 203 ms | 201 | 220 |
| `rtk rewrite "git status"` | 34 ms | 33 | 36 |
| `bash -c 'type -t'`, the Bash hook's probe | 18 ms | 18 | 22 |

End to end through the hook, including Node startup:

| Hook and path | Per tool call | Spawns |
|---|---|---|
| `ac_rtk_claude_Bash.js` | about 105 ms | `bash` + `rtk` |
| `ac_rtk_claude_PowerShell.js`, rewrite path | 324 to 423 ms | `pwsh` + `rtk` |
| `ac_rtk_claude_PowerShell.js`, ignored path | 260 to 339 ms | `pwsh` |
| `ac_rtk_claude_PowerShell.js`, already-`rtk ` path | 39 ms | none |

**Position: accept the cost.** The decomposition is what settles it. 203 of the probe's 264 ms is bare `pwsh` process startup, before the probe does any work, so the probe's own parse plus `Get-Command` is about 60 ms and there is no version of "make the probe cheaper" that recovers meaningful time. The only lever is not spawning `pwsh` at all, which is the textual pre-filter section 4.5 rejects on drift grounds. It is a fixed per-call addition against `PowerShell` tool calls that already spend about 200 ms starting `pwsh` and typically run for seconds.

**These figures are a floor, not a typical.** No PowerShell profile exists on the authoring workstation, so the profile load measured zero; `-NoProfile` came out 4 ms *slower*, which is noise. On a machine with a profile, the probe pays that profile's load on every call. Section 4.5's `timeout: 5000` bounds the tail.

### 7.4 Dependency-cycle gate

No Rust or TypeScript module arc is added, removed or moved. Nothing under `src-tauri/`, `src/` or `scripts/` is touched, so no SCC can change, no `cyclicSccs` count can move, and no arc can cross a previously clean SCC boundary. No lower-layer module gains an `AppHandle` or `tauri` dependency, because no Rust module is edited at all.

The only new module relationship is inside the seeded JavaScript tree: `ac_rtk_claude_Bash.js` requires `ac_rtk_shared.js`, and `ac_rtk_claude_PowerShell.js` requires `ac_rtk_shared.js`. `ac_rtk_shared.js` requires neither hook. That is two arcs into one sink, a tree, and it is acyclic by construction.

**Binding acceptance criterion carried into section 9: `ac_rtk_shared.js` must not require either hook file, and the two hooks must not require each other.** A reviewer checks it with `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`, which must show `ac_rtk_shared.js` requiring only `node:fs`, `node:path` and `node:child_process`.

**Re-stated at certification.** Neither enrichment pass added, moved or removed a module arc, and the section 4.4 decision removes code rather than adding it. The gate's result is unchanged: no Rust or TypeScript SCC can change, and the JavaScript graph remains two arcs into one sink.

---

## 8. Implementation order

### 8.1 Step 1: extract the shared module, Bash still alone

1. `git mv docs/integrations/rtk_claude/hooks/ac_rtk_claude.js docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js`
2. Create `ac_rtk_shared.js` per section 4.2, moving the code out of the renamed file rather than retyping it. **Implement section 4.2's six-symbol contract, not the four-export shape of the architect prototype at `<replica root>/1416-proto/`.** The prototype exports `{ NAG, ignoredLogPath, logIgnored, runHook }` and inlines the `rtk rewrite` `spawnSync` in the hook; the six-symbol contract adds `ALREADY_RTK` and `rtkRewrite`, and `rtkRewrite` is where section 4.2's `timeout` is set once for both hooks. The prototype is evidence of behaviour, not a template for structure.
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

No automated test is added. There is no JavaScript test runner wired to `docs/`, and `package.json:5` is `"type": "module"` while these hooks are CommonJS, so adding one would mean new tooling for four mirror files that no build consumes.

The list below is the consolidated set across all three authoring passes, renumbered once and deduplicated. Every criterion names who checks it and how. Criteria 1 to 21 are provable from the branch by any reviewer; 22 to 28 require the runtime copies, which the user applies by hand, and each names the agent that reads the result afterwards.

### 9.1 Branch-only, static. Owner: reviewer

| # | Criterion | How |
|---|---|---|
| 1 | `docs/integrations/rtk_claude/hooks/` holds exactly `ac_rtk_claude_Bash.js`, `ac_rtk_claude_PowerShell.js`, `ac_rtk_shared.js`. `ac_rtk_claude.js` is gone. | `git ls-tree -r --name-only HEAD -- docs/integrations/rtk_claude/` |
| 2 | The rename is recorded as a rename, not a delete plus an add. | `git log --follow --oneline -- docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js` |
| 3 | `settings.local.json` parses and declares exactly two `PreToolUse` entries, matchers `Bash` and `PowerShell`, each pointing at its own hook file. | `node -e "const h=JSON.parse(require('fs').readFileSync('docs/integrations/rtk_claude/settings.local.json','utf8')).hooks.PreToolUse;console.log(h.map(e=>e.matcher+' -> '+e.hooks[0].command).join('\n'))"` |
| 4 | None of the four seed placeholders appears in any of the four files (section 3.4). | `git grep -n -e '%AC_REPLICA_ROOT%' -e '%AC_WORKSPACE_ROOT%' -e '%AC_MATRIX_ROOT%' -e '%USER_HOME%' -- docs/integrations/rtk_claude/` returns nothing |
| 5 | The JavaScript module graph is a tree (section 7.4). | `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`: `ac_rtk_shared.js` requires only `node:` builtins, and neither hook requires the other |
| 6 | `ac_rtk_claude_Bash.js` states in the file that it runs under Windows with Git Bash, Linux and macOS. Required by the issue. | read the header comment |
| 7 | **The PowerShell hook prepends nothing** (section 4.4). No shadow function, no `Write-Error` variant, no `exit $LASTEXITCODE`, no `2>$null` suffix. `ti.command` is exactly the routed command. | `grep -nE 'function rtk\|Error\.WriteLine\|Write-Error\|LASTEXITCODE' docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` returns nothing |
| 8 | `headIsExternal` scans for the **last** `AC_RTK_VERDICT:` line, not a bare comparison of the whole trimmed buffer (section 4.5). | `grep -n "AC_RTK_VERDICT" docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` shows the marker in the probe and the filter in the JS |
| 9 | All three `spawnSync` calls carry a `timeout` (sections 4.2 and 4.5). | `grep -n "timeout" docs/integrations/rtk_claude/hooks/*.js` shows one in `rtkRewrite`, one in the probe, and one in the Bash hook's `prefixable` |
| 10 | Section 6.3's known-broken rows are present in the README, not omitted: `tree`, `find` and `git status \| Select-String`. Absence is a documentation defect, not a pass. | read the README routing section |
| 11 | The README failure-modes section names all four section 6.5 modes and states that `$LASTEXITCODE` and `$?` are both reliable after a routed PowerShell command, because the hook prepends nothing. | read the README failure-modes section |
| 12 | The README names three hook files with their real line counts, shows the two-matcher block, and no longer claims the hook covers the `Bash` tool and nothing else. | `git grep -n "ac_rtk_claude" -- docs/` returns no hit on the bare old filename |
| 13 | `docs/integrations/rtk.md:91` no longer says a single hook is seeded, and its link to `rtk_claude/README.md` still resolves. | read the line |
| 14 | `rtk-replica-history.db` is still present at the repository root and still untracked. | `git status --porcelain=v1 --untracked-files=all` reports exactly `?? rtk-replica-history.db` |

### 9.2 Branch-only, behavioural. Owner: any reviewer with `node`, `rtk` and `pwsh`

The hooks **cannot** be run in place: `package.json:5` makes every `.js` in this repository an ES module and `require` is not defined there (section 3.9). Copy them out first, to a scratch directory at your **replica root**, never inside `.claude/`.

```bash
mkdir -p "$AGENTSCOMMANDER_ROOT/1416-check"
cp docs/integrations/rtk_claude/hooks/*.js "$AGENTSCOMMANDER_ROOT/1416-check/"
cd "$AGENTSCOMMANDER_ROOT/1416-check"
echo '{"tool_name":"PowerShell","tool_input":{"command":"git status"}}' | node ac_rtk_claude_PowerShell.js
```

Expected, on one line: JSON whose `updatedInput.command` is exactly `rtk git status`, with **no** prepended statement and no leading newline, and whose `permissionDecisionReason` is `ac_rtk_claude_PowerShell`.

```bash
echo '{"tool_name":"PowerShell","tool_input":{"command":"ls"}}' | node ac_rtk_claude_PowerShell.js
```

Expected: no output at all, exit 0. `ls` is a PowerShell alias.

| # | Criterion | How |
|---|---|---|
| 15 | Every row of section 6.3 reproduces exactly, known-broken rows included. | drive each row from the scratch directory |
| 16 | Every row of `README.md:128-144` still reproduces through `ac_rtk_claude_Bash.js`. This is the Bash no-regression suite. | drive each row from the scratch directory |
| 17 | No run in the scratch directory writes to any `rtk_ignored_tools.md`, because `ignoredLogPath` returns `null` outside a `__`-prefixed replica root. | `grep -cE '^[0-9]{8}_[0-9]{6} (Bash\|PowerShell):' <matrix>/rtk_ignored_tools.md` returns 0 after the whole suite. The tool field cannot appear except from the new hooks, so its absence proves the scratch runs wrote nothing |
| 18 | A routed command runs end to end. Paste one `updatedInput.command` from criterion 15 into a real `PowerShell` tool call inside a git repository. | RTK's compacted output is present, the `rtk` notice **is** present on stderr, and `$LASTEXITCODE` equals what the unrouted command returns |
| 19 | The four native-channel results of section 4.4 reproduce for a routed command. | `$?` is `False` after a failing routed command; `$c = <routed> 2>&1` captures the real error plus exactly one notice record; `<routed> > f` writes LF-separated bytes unchanged; `<routed with> --format=a,b` arrives as one argv entry |
| 20 | Section 4.5's probe survives a printing profile. | run the probe with a `Write-Host 'banner'` prelude and confirm the verdict is still read correctly |
| 21 | Section 4.5's `timeout` fires. | run the probe against a sleeping prelude and confirm it is killed and the command falls to the ignored log |

### 9.3 After the runtime copies are applied

These cannot be proved from the branch, because the files that take effect are outside every repository.

| # | Criterion | Owner |
|---|---|---|
| 22 | The four files of section 8.4 are in `<workspace>/.ac/default.claude/`, and `ac_rtk_claude.js` is deleted there. | **the user**, per the decision recorded in the issue comment |
| 23 | A freshly spawned replica has all three hook files under `.claude/hooks/` and the two-matcher `settings.local.json`. | tech-lead, or `ac-cli-tester`, once the user reports 22 done |
| 24 | That replica's `rtk_ignored_tools.md` shows both `Bash:` and `PowerShell:` lines from the same agent, proving both hooks are registered and both write to the one file. This is the shared precondition for 25 and 26 and is the cheapest of the three to read. | tech-lead, or `ac-cli-tester` |
| 25 | In that replica, a `PowerShell` tool call running `git status` produces a `commands` row in that agent's `rtk-matrix-history.db`, and one running `Get-ChildItem` produces a `rtk_ignored_tools.md` line ending in `PowerShell: Get-ChildItem`. | tech-lead, or `ac-cli-tester` |
| 26 | In the same replica, a `Bash` tool call still behaves as it did before, and its ignored-log lines now carry the `Bash` field. | tech-lead, or `ac-cli-tester` |
| 27 | `git status \| Select-String "nothing added to commit"` in a real `PowerShell` tool call returns **zero** results with exit 0, and the README predicted it. This is section 3.10's hazard B observed end to end. It is **expected to fail**; the criterion is that the documentation said so in advance. | tech-lead, or `ac-cli-tester` |
| 28 | `$c = git status 2>&1; $c.Count` in a real `PowerShell` tool call matches the native count plus exactly one notice record, as the README predicts. | tech-lead, or `ac-cli-tester` |

Criterion 25 is the one that proves the issue is closed. Until it is reported, this change is landed but unproven, and the pull request description must say so rather than implying the gap is measured shut.

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
- **Do not prepend or append anything to the PowerShell hook's output** (section 4.4). No `rtk` shadow function, no `Write-Error -ErrorRecord` variant, no `exit $LASTEXITCODE` trailer, no `2>$null` suffix. `ti.command` is exactly the routed command. The appendices contain an earlier design that did prepend one; it is superseded, and section 4.4 measures why.
- **Do not add a name deny-list to either hook** for the section 3.10 hazards. A list keys on the head, while hazard B is decided by the tail, so it cannot express the condition, and it is the drifting second copy section 4.5 exists to avoid.
- **Do not correct `docs/integrations/rtk.md:93`** (section 5.6).
- **Do not file the upstream `rtk` issue as part of this work.** Section 3.10 records it as a follow-up for the tech-lead, and it is not a gate on this issue.
- **Do not treat the prototype at `<replica root>/1416-proto/` as a template.** It is evidence of behaviour. Its module shape predates section 4.2's six-symbol contract and its PowerShell hook predates section 4.4.
- **Do not add a test runner, a markdown linter or a link checker.** None exists in `.github/workflows/`.

---

## 11. Review findings and where each one landed

Two enrichment passes ran against the DRAFT_FOR_ENRICHMENT plan. Every finding is resolved into sections 1 to 10 above. This table is the map. **Sections 1 to 10 are the specification; Appendices A and B are evidence and are not.** Where an appendix conflicts with sections 1 to 10, sections 1 to 10 govern.

| Finding | Disposition | Landed in |
|---|---|---|
| A.1 `tree` and `find` pass the `Application` gate and break silently | accepted as recommended: document, no deny-list, escalate upstream | 3.10 hazard A, 6.3 known-broken rows, 6.5, 10 |
| A.2 `[Console]::Error.WriteLine` defeats every `2>` form | **overruled.** Both reviewers recommended keeping the filter and documenting. Re-measured at certification: the shadow function breaks four channels, three silently. The filter is removed entirely | 4.4, and the finding is dissolved rather than documented |
| A.3 no `timeout` on either `spawnSync` | accepted verbatim, `timeout: 5000` | 4.2, 4.5, 6.5, criterion 9 |
| A.4 cost decomposition, 203 ms of 264 ms is `pwsh` startup | accepted, folded in, and it strengthens the pre-filter rejection | 7.3, 4.5 |
| A.5 sign-off on the conservative rule and on fail-closed | accepted, both stand | 4.3, 6.5 |
| A.6 `rtk rewrite` never touches an element after a pipe, seven shapes | accepted as evidence; its **conclusion** is inverted by B.1 | 3.6, 4.3 |
| A.7 six-symbol contract, not the prototype's four | accepted | 4.2, 8.1, 10 |
| A.7 concurrent appends within one replica | accepted | 6.4 |
| A.7 `parse_failures` will see more traffic under PowerShell | accepted | 5.5 |
| B.1 any pipeline whose tail parses the head's text | accepted: not a blocker, no rule change, but 4.3, 6.3 and A.6 must stop reading as an all-clear | 3.10 hazard B, 4.3, 6.3, 6.5, criteria 10 and 27 |
| B.2 `$?` is `True` after every failing routed command | **dissolved** by the 4.4 decision; recorded as a mode that does not exist and must not be reintroduced | 4.4, 6.5, 7.1 |
| B.3 redirected stdout re-encoded, binary destroyed | **dissolved** by the 4.4 decision | 4.4, 6.5, 7.1 |
| B.4 unquoted commas split into two arguments | **dissolved** by the 4.4 decision | 4.4, 7.1 |
| B.5 a printing `$PROFILE` silently disables the hook | **required**, and strengthened: a `AC_RTK_VERDICT:` sentinel with last-match scanning, which also survives output emitted after the verdict | 4.5, criteria 8 and 20 |
| B.6 "the profile can only make the probe more conservative" is false | accepted, claim struck and replaced with the measured behaviour | 4.5 |
| B.7 the newline guard row is wrong; the `=` guard is name-only | accepted, both corrected | 4.5, 6.3 |
| B.8 the nag filter drops a whole `ErrorRecord`, not a line | **dissolved** by the 4.4 decision | 4.4 |
| B.8 `git status &` routes, behaviour unmeasured | **measured at certification.** With the shadow function the job completes and only the notice leaks, because the function does not reach the job's runspace. With the 4.4 decision there is no function, and the job is native | 4.4, 7.1 |
| B.9 positions on the two escalated decisions | one accepted (A.1), one overruled (A.2) | 3.10, 4.4 |

Acceptance criteria from all three passes are consolidated and renumbered once in section 9. The per-appendix numbering below is superseded; do not work from it.

---

## Appendix A. `dev-rust` enrichment, as written

**Evidence, not specification.** Preserved as written for its measurements, apart from em dash characters normalised to the house style. Superseded by sections 1 to 10 wherever the two differ, and by section 11's disposition table. In particular, everything this appendix says about the shape or documentation of a prepended PowerShell filter is superseded by section 4.4, which removes the filter.

Added after the plan reached DRAFT_FOR_ENRICHMENT. Everything below was measured on the same workstation the plan was authored on (Windows 11 Pro 10.0.26200, `rtk` 0.42.4, PowerShell 7.6.5 Core, Node 24.13.0), against a copy of the architect prototype at `<replica root>/1416-check/`, per section 9.2.

Nothing here reopens a decision the issue comment closed. Sections 11.1 to 11.3 are defects in the specified content and each names the section it amends. Sections 11.4 to 11.6 are sign-offs the plan asked for. Section 11.7 is minor. Section 11.8 adds acceptance criteria, numbered on from criterion 17.

### 11.1 The `Application` gate is necessary but not sufficient: `find` and `tree` still break silently

**Amends section 4.3 and section 6.3. The claim in section 4.3 is too strong as written.**

Section 4.3 justifies accepting `rtk rewrite`'s output once `headIsExternal` holds:

> The head was already an external binary, so it already emitted **strings** into the PowerShell pipeline. Replacing it with `rtk <head>`, which also emits strings, preserves the pipeline's type contract.

That is true and it does close the section 3.6 hazard, which is about PowerShell **owning** the name. It does not close a second hazard of the same shape: names Windows owns with a **different meaning** from the POSIX tool `rtk` assumes. Those resolve as `CommandType = Application`, so the gate passes them, the rewrite applies, and the command breaks silently with exit 0.

Two members of the class exist on this workstation. Both are reproducible through the hook exactly as specified:

```text
383ms | "tree"                        ==>  rtk tree
355ms | "find \"NAG\" ac_rtk_shared.js" ==>  rtk find "NAG" ac_rtk_shared.js
```

`tree` resolves to `C:\WINDOWS\system32\tree.com`, `find` to `C:\WINDOWS\system32\find.exe`. Measured before and after:

| Command | Native, in a real `PowerShell` tool call | After the rewrite |
|---|---|---|
| `tree` | folder listing, exit 0 | `Too many parameters - node_modules\|.git\|target\|...`, **exit 0**, no listing |
| `find "NAG" ac_rtk_shared.js` | the two matching lines, exit 0 | **no output at all, exit 0** |

`rtk tree` passes its GNU-`tree` ignore flags (`-I <pattern>`) to `tree.com`, which rejects them and still exits 0. `rtk find` applies GNU `find` semantics to arguments meant for `find.exe`.

This is failure by the exact criterion section 3.6 sets out: empty result, exit 0, no error anywhere. It is worse than the reporting gap the issue closes.

Scope of the class, measured. Of the heads `rtk rewrite` actually rewrites, these resolve as `Application` here: `git`, `curl`, `rg`, `jq`, `find`, `tree`. `curl`, `jq`, `rg` and `tar` were compared native against `rtk <head>` and are byte-identical on `--version`; `more` through the section 4.3 step 4 prefix fallback is also identical. **`find` and `tree` are the whole class on this machine.** `ls`, `ps`, `diff`, `cat` and `sort` never reach the rewrite because they are aliases, which is section 4.5 working as designed.

The class is not PowerShell-specific in origin. It comes from `rtk`'s own subcommands assuming POSIX tools, so `ac_rtk_claude_Bash.js` under Git Bash has the same exposure for any name where the resolved binary is the Windows one. **It is therefore not a blocker for this issue and does not justify a change to the section 4.3 rule.** What it does justify:

1. Section 4.3 must not claim the gate makes accepting the rewrite safe. It makes it safe against names PowerShell owns. Reword to that, and point at this section.
2. Section 6.3's table gains two rows, `tree` and `find`, marked as known-broken rather than omitted, so the table is not read as an all-clear.
3. Section 6.5 gains this as a third new failure mode, and the README failure-modes section carries it with the `tree` example, which is the cleanest one-line demonstration.

A name deny-list inside the hook would fix it and should be rejected for the reason section 4.5 already gives: it is a second copy of a shell-and-platform question that drifts. The honest fix belongs upstream in `rtk`. Filing it as a follow-up issue against `rtk` is the right disposition, not a change here.

### 11.2 The section 4.4 filter function defeats PowerShell stderr redirection

**Amends section 4.4, section 6.5 and section 7.1. This is the one item I would not land undocumented.**

The specified filter ends each error record with `[Console]::Error.WriteLine("$_")`. That writes to the process stderr **handle**. PowerShell's `2>` redirects PowerShell's **error stream**. They are not the same channel, so every form of stderr redirection stops working for any command the hook routes.

Measured in one real `PowerShell` tool call inside `repo-AgentsCommander`, same command, filter absent then present:

```text
A: rtk git nosuchsubcmd 2> e1.txt     (no filter)   exit=1  captured 140 bytes
B: rtk git nosuchsubcmd 2> e2.txt     (filter)      exit=1  captured   0 bytes
                                                    error text leaked to the console instead
```

Both other redirection forms fail the same way:

```text
$captured = rtk git nosuchsubcmd 2>&1   ->  $captured.Count = 0, text on the console
rtk git nosuchsubcmd 2>$null            ->  error still printed
```

`$captured.Count = 0` is the serious one. An agent capturing a command's diagnostics into a variable gets an empty collection and no error, which is the same silent-and-empty shape as the `ps | Where-Object` case in section 3.6. `2>$null` failing to silence is noisy rather than dangerous, but it is a common idiom.

Section 6.3 already shows redirection passes the gate (`git status > out.txt` is a passing row), so `2>` is reachable, not hypothetical. This is also a **regression against Bash**: `exec 2> >(grep ...)` sets the shell's fd 2 for the rest of the script, and a per-command `2> file` still overrides it, so the Bash hook preserves redirection.

The one-token fix restores redirection. Measured, same command:

```powershell
if ("$_" -notmatch 'No hook installed') { Write-Error -ErrorRecord $_ -ErrorAction Continue }
```

```text
C: rtk git nosuchsubcmd 2> e3.txt     (Write-Error)  exit=1  captured 155 bytes
   nag still suppressed, exit code still preserved
```

It carries a real cost. `Write-Error` decorates every error with PowerShell's ConciseView position block, so the 140-byte native message becomes 155 bytes across five lines:

```text
rtk:
Line |
   6 |  rtk git nosuchsubcmd 2> $g
     |  ~~~~~~~~~~~~~~~~~~~~~~~~~~
     | git: 'nosuchsubcmd' is not a git command. See 'git --help'.
```

That tax is paid on **every** error line of **every** routed command, in a hook whose purpose is compacting output. Redirection is used on a minority of commands; errors are not.

**My recommendation: keep `[Console]::Error.WriteLine` and document the limitation.** The verbatim-stderr property is worth more than redirection fidelity, and the failure is visible (the text appears, in the wrong place) rather than silent, except for the `2>&1`-into-a-variable case. Required if that is the call:

- Section 4.4 currently says "other stderr survives verbatim". True, but incomplete. Add: it survives on the console and no longer honours `2>`, `2>&1` or `2>$null`.
- Section 7.1 lists compatibility guarantees and does not mention this. Add it.
- Section 6.5 gains it as a failure mode, and the README failure-modes section carries the `$captured.Count = 0` example, because that is the one an agent cannot see.

If architect prefers redirection fidelity over verbatim text, the `Write-Error` variant above is measured and works; it is a one-line substitution in section 4.4. Either choice is defensible. Choosing neither, and shipping section 4.4 as written with no note, is not.

### 11.3 Neither `spawnSync` call carries a `timeout`, and the probe is the one that can hang

**Amends section 4.5 and section 6.5. Cheap, and it is what makes the section 6.5 fail-closed promise true in every case rather than most.**

Section 4.5 deliberately does not pass `-NoProfile`, so that the probe sees the same command table the real session will use. The argument is correct. The consequence is that the probe executes an arbitrary user profile on every `PowerShell` tool call, and `spawnSync` as specified has no `timeout`. A profile that blocks, prompts, or waits on the network hangs the hook, and with it the tool call, with nothing in the log.

Measured, both failure paths:

```text
missing pwsh                  -> error=ENOENT, stdout=undefined     => headIsExternal = false   (fail closed, works today)
probe sleeping 30s, no timeout -> hangs
probe sleeping 30s, timeout 1500 -> killed at 1514ms, SIGTERM       => headIsExternal = false   (fail closed)
```

**Add `timeout` to the probe `spawnSync` in section 4.5, and to the `rtk rewrite` call in section 4.2 for symmetry.** A killed probe already produces empty stdout, so `headIsExternal` returns false and the existing fail-closed disposition handles it with no other change. `5000` is ample: the probe measures 264 ms here and 203 ms of that is bare `pwsh` startup, so a five-second budget tolerates a profile 20 times slower than this machine's before it gives up.

No profile file exists on this workstation (`$PROFILE`, `AllUsersAllHosts` and `CurrentUserAllHosts` all absent), which is why the cost measured zero in section 4.5. That makes the section 7.3 figure a floor, not a typical, on any machine that has one.

One workstation-specific assumption worth stating in the hook's header comment rather than leaving implicit: the probe spawns `pwsh`, and the harness's `PowerShell` tool is `pwsh` 7.6.5 Core here, so probe and session share a command table. On a host where the tool runs Windows PowerShell 5.1, `pwsh` is either absent (every command fails closed to the ignored log, which is degraded but honest) or present and answering for a different command table than the one that will run. The hook cannot detect this. Say so once.

### 11.4 Cost: I sign off on about 385 ms, with the decomposition the plan is missing

**Amends section 7.3.**

Section 7.3 gives probe about 240 ms and `rtk rewrite` about 55 ms. My medians over five warm runs each:

| Step | Median | Min | Max |
|---|---|---|---|
| `pwsh` probe, profile loaded | 264 ms | 260 | 271 |
| `pwsh` probe, `-NoProfile` | 268 ms | 262 | 275 |
| `pwsh -Command "exit 0"`, bare startup | 203 ms | 201 | 220 |
| `rtk rewrite "git status"` | 34 ms | 33 | 36 |
| `bash -c 'type -t'`, the Bash hook's probe | 18 ms | 18 | 22 |

End to end through the hook, including Node startup: 324 to 423 ms on the rewrite path, 260 to 339 ms on the ignored path, **39 ms on the already-`rtk ` path**. Section 7.3's 385 ms is a fair upper figure.

The decomposition matters more than the total, and it supports architect's rejection of the textual pre-filter rather than undermining it: **203 of the 264 ms is bare `pwsh` process startup**, before the probe does any work. The probe's actual parse plus `Get-Command` is about 60 ms. There is no version of "make the probe cheaper" that recovers meaningful time; the only lever is not spawning `pwsh` at all, which is exactly the pre-filter section 4.5 rejects on drift grounds, and I agree with that rejection.

`-NoProfile` measures 4 ms slower than loading the profile here, which is noise, so on this machine keeping the profile costs nothing and the correctness argument in section 4.5 stands unopposed.

**Position: accept the cost.** 385 ms is a fixed per-call addition against `PowerShell` tool calls that already spend about 200 ms starting `pwsh` and typically run for seconds. It is paid once per tool call, not per token, and the passthrough path is 39 ms. I would not trade the drift-free probe for it. The tail risk, not the median, is the thing to guard, and section 11.3 guards it for the price of one option.

### 11.5 The two open dispositions: I sign off on both

**How conservative the PowerShell rule is (section 4.3).** Sign off, unchanged. The asymmetry is the correct trade and the plan already states why: a missed rewrite costs one statistic, a wrong rewrite silently breaks the agent's command. Two things make it safe to accept rather than merely defensible:

- The gap is **measured, not estimated**. Every compound command lands in `rtk_ignored_tools.md` with its own `PowerShell:` field, so the size of the coverage difference is readable off the artifact at any time. That is the property this issue exists to create, and the conservative rule is what preserves it. A wider rule that guessed would trade a visible gap for an invisible one.
- The difference is smaller than "compound versus simple" suggests. Coverage is wider than Bash in the other direction: `python -c "print(1)"` and `git status > out.txt` reach the ignored log under Bash and are rewritten under PowerShell, because the parser distinguishes an argument from syntax where a character class cannot. Section 6.3 already records this. The net is not uniformly narrower.

If the gap later proves too large in practice, the ignored log is the evidence needed to argue for widening it, and the argument can be made from data instead of intuition. That is the right order.

**Fail-closed on probe failure (section 6.5).** Sign off, and I would not accept fail-open. The disposition differs from the Bash hook for a reason that is not arbitrary: `prefixable()` failing open costs at worst a `rtk cd` style loud 127, whereas `headIsExternal` failing open means rewriting a command whose shape was never verified, which is section 3.6 and section 11.1 territory, silent and exit 0. Different consequence, different default.

The degenerate case is worth stating plainly because it reads alarming and is not: if `pwsh` is missing entirely, **every** `PowerShell` command fails the probe and goes to the ignored log. Zero `commands` rows. That is still strictly better than today, where those calls produce nothing anywhere, and the ignored log makes the total failure obvious on the first read rather than invisible. Fail-closed degrades to honest, which is the whole thesis of the issue. With section 11.3's timeout it also degrades to honest when the probe hangs rather than not degrading at all.

### 11.6 Confirmations

**`rtk rewrite` never reaches past a pipe.** This is the load-bearing fact under section 4.3 and the plan asserts the tail is untouched from a single example. Measured across seven shapes, including ones where the element after the pipe is exactly the hazard class:

```text
git status | ls          =>  rtk git status | ls
git status | sort        =>  rtk git status | sort
git status | cat         =>  rtk git status | cat
git status | diff a b    =>  rtk git status | diff a b
git status | ps          =>  rtk git status | ps
git log | head -5        =>  rtk git log | head -5
curl -s http://x | cat   =>  rtk curl -s http://x | cat
```

`rtk rewrite` rewrites only the head of each `;` / `&&` segment and never an element after a `|`. Since the section 4.5 guards already reduce the command to one statement containing one pipeline, `rtk rewrite` can only ever touch the single head the probe just cleared. **The section 4.3 ordering asymmetry does close the section 3.6 hazard, and this is why.** Add these rows to section 3.6; the argument currently rests on one example and it deserves the wider evidence.

**Section 6.3 reproduces exactly.** All 17 rows, run through the architect prototype from `<replica root>/1416-check/`, produce the stated outcome with `permissionDecisionReason = ac_rtk_claude_PowerShell` on every rewritten row. No discrepancies.

**Criterion 12 holds.** After every run above, `_agent_dev-rust/rtk_ignored_tools.md` contains 122 lines and **zero** lines matching `^[0-9]{8}_[0-9]{6} (Bash|PowerShell):`. The tool field cannot appear except from the new hooks, so its absence proves the scratch directory wrote nothing. That grep is a better check than listing the Matrix before and after; section 9.2 criterion 12 should use it.

**The zero-consumer grep re-runs clean, and it is wider than section 4.6 claims.** Section 4.6 says the grep covered `src-tauri/`, `src/` and `scripts/`. I ran it over the whole tracked tree plus the untracked `.claude/`, `.github/` and `scripts/`:

```text
git grep -n -I -e rtk_ignored_tools -e ignoredTools -e ignored_tools \
               -e permissionDecisionReason -e ac_rtk_claude -- .
```

Every hit is in `docs/integrations/rtk_claude/README.md` prose, the hook itself, `settings.local.json`, or this plan. No Rust, no TypeScript, no test, no script, no workflow. The repository's own `.claude/settings.json` inline hook, the one #1424 covers, does not reference either value either, so it is not a hidden consumer. **Both of architect's decisions are safe: the ignored-log `tool` field and the `permissionDecisionReason` rename. No consumer was missed.**

### 11.7 Minor

- **The prototype exports four symbols, section 4.2 specifies six.** `ac_rtk_shared.js` in the prototype exports `{ NAG, ignoredLogPath, logIgnored, runHook }` and the PowerShell hook calls `spawnSync("rtk", ["rewrite", ...])` inline. Section 4.2's contract of six, adding `ALREADY_RTK` and `rtkRewrite`, is the better one: both hooks need both, and `rtkRewrite` is where section 11.3's `timeout` belongs so it is set once. Implement section 4.2, not the prototype. Worth one line in section 8.1 so the implementer does not copy the prototype's shape.

- **Concurrent appends to the ignored log are now possible inside one replica.** Section 6.4 notes that two live replicas of the same agent append to the same file. After this change the two hooks can also run concurrently within one replica, because `Bash` and `PowerShell` tool calls issued in the same assistant turn execute in parallel. `fs.appendFileSync` with the default `a` flag is not guaranteed atomic on Windows. The blast radius is one interleaved line in a human-read log, so no locking is warranted, but section 6.4's sentence should say "and two hooks in one replica" rather than implying only the cross-replica case.

- **Unresolved heads reaching `parse_failures` is expected, and PowerShell will produce more of them.** Section 4.5's `if (-not $g) { 'APP' }` treats an unresolved head as external, matching the Bash hook, where `type -t` also returns empty for not-found. The `rtk <unknown-binary>` fallback writes a `parse_failures` row. Under PowerShell an unresolved name is more often a mistyped cmdlet than a missing binary, so that table will see more traffic than it does from Bash. Not a defect and no change is needed; it is worth one sentence in the README's database section so a reader does not misread the growth as a regression.

### 11.8 Acceptance criteria added

Numbered on from criterion 17. Criteria 18 and 19 are reviewer checks against the branch; 20 and 21 belong with the section 9.3 runtime checks.

| # | Criterion | Owner |
|---|---|---|
| 18 | `tree` and `find` appear in the section 6.3 table marked as known-broken, and the README failure-modes section carries the `tree` example with its exit 0. Absence is a documentation defect, not a pass. | reviewer |
| 19 | Both `spawnSync` calls in the PowerShell hook pass a `timeout`, and the `rtk rewrite` call in `ac_rtk_shared.js` passes one too. `grep -n "timeout" docs/integrations/rtk_claude/hooks/*.js` shows all three. | reviewer |
| 20 | In a real `PowerShell` tool call, `$c = <routed command> 2>&1; $c.Count` matches what the README says it will be. Whichever disposition section 11.2 takes, the README must predict the observed number. | tech-lead, or `ac-cli-tester` |
| 21 | In the seeded replica, `rtk_ignored_tools.md` shows both `Bash:` and `PowerShell:` lines from the same agent, proving both hooks are registered and both write to the one file. This is criterion 16 and 17's shared precondition and is cheaper to read than either. | tech-lead, or `ac-cli-tester` |

---

## Appendix B. `dev-rust-grinch` enrichment, as written

**Evidence, not specification.** Preserved as written for its measurements, apart from em dash characters normalised to the house style. Superseded by sections 1 to 10 wherever the two differ, and by section 11's disposition table. Sections 12.2, 12.3 and 12.4 describe defects in a design section 4.4 no longer specifies; they are retained because they are the measurements that justify removing it.

Added after Appendix A. Adversarial pass, measured on the same workstation the plan and section 11 were measured on (Windows 11 Pro 10.0.26200, `rtk` 0.42.4, PowerShell 7.6.5 Core, Node 24.13.0), driving a copy of the architect prototype at `<replica root>/1416-grinch/` per section 9.2.

Nothing here reopens a settled decision. The two hooks, the shared module, the tracked surface and the hand-applied runtime copies are all unchanged by everything below.

Scope of what I looked for: the architect asked for a PowerShell command shape that passes all seven section 4.5 guards and still reaches `rtk rewrite` wrongly, at the section 3.6 bar - empty output, exit 0, no error anywhere. **I found one, and it does not depend on the head at all.** It is section 12.1, and it is the reason sections 12.2 to 12.4 matter: once a command is routed, three separate channels between the agent and the command are altered, and none of them announces itself.

Sections 12.1 to 12.6 are defects in the specified content. Section 12.7 corrects two factual claims. Section 12.8 is minor. Section 12.9 gives my position on the two decisions section 11 escalated. Section 12.10 adds acceptance criteria, numbered on from 21.

### 12.1 A shape that defeats all seven guards: any pipeline whose tail parses the head's text

**Amends section 4.3, section 6.3 and section 11.6. This is the answer to the question I was given.**

```text
git status | Select-String "nothing added to commit"
```

Every guard passes: it parses, it is one statement, that statement is one `PipelineAst`, the first element is a `CommandAst`, the invocation operator is `Unknown`, `GetCommandName()` returns `git`, the name has no `=`, and `git` resolves as `CommandType = Application`. The hook routes it. Measured through the prototype:

```text
ROUTED | "git status | Select-String \"nothing added to commit\""
       ==> "rtk git status | Select-String \"nothing added to commit\""
```

The emitted command was then executed verbatim, `FILTER` line included, against what the agent actually typed:

| | results | exit | `$?` | stderr |
|---|---|---|---|---|
| what the agent typed | **1** | 0 | True | none |
| what the hook makes the tool run | **0** | 0 | True | none |

Empty output, exit 0, no error anywhere. This is the section 3.6 bar met exactly, and unlike the `ps \| Where-Object` and `tree` cases it is reached through the plan's own model-citizen head, `git`, on a shape section 6.3 already blesses.

**Why the section 4.3 argument does not cover it.** Section 4.3 says:

> The head was already an external binary, so it already emitted **strings** into the PowerShell pipeline. Replacing it with `rtk <head>`, which also emits strings, preserves the pipeline's type contract. Only the content is compacted, which is the point of RTK.

Both sentences are true and together they are not sufficient. The **type** contract is preserved. The **content** contract is not, and a pipeline tail is a consumer of content, not of type. "Only the content is compacted" is the hazard, stated as if it were the mitigation.

Measured, `rtk` does not merely compact `git`'s output, it reformats it:

```text
git status   native: On branch feat/1416-rtk-hook-per-shell | Untracked files: |   (use "git add <file>...") |
                     rtk-replica-history.db | | nothing added to commit but untracked files present (use "git add" to track)
git status   rtk   : * feat/1416-rtk-hook-per-shell | ?? rtk-replica-history.db
```

Bounded by measurement, so the class is not overstated. Comparing native against `rtk` for the same argv:

| Command | Identical? |
|---|---|
| `git rev-parse HEAD` | yes |
| `git status --porcelain` | yes, apart from a dropped trailing newline |
| `git log --oneline -3` | yes |
| `git diff --name-only` | yes |
| `git branch --show-current` | yes |
| `git branch` | yes |
| `git show --stat HEAD` | yes |
| `rg -n <pat> <file>` | yes |
| **`git status`** | **no** - 6 lines become 2, different format |
| **`git log -1`** | **no** - 32 lines become 1 |
| **`git diff HEAD~1 --stat`** | **no** |

So the class is the subcommands `rtk` reformats - on this machine `git status`, `git log` in its default format, and `git diff --stat` - combined with any tail that reads the text: `Select-String`, `-match`, `ConvertFrom-*`, `Where-Object { $_ -like ... }`, `Measure-Object -Line`, or a `> file` whose contents are parsed later.

**Section 11.6 is load-bearing in the opposite direction from the one it concludes.** Section 11.6 proves `rtk rewrite` never touches an element after a `|` and reads that as what makes the section 4.3 gate sufficient. The untouched tail is exactly the problem: the tail is left as the agent wrote it, against the format the head no longer produces. A tail that had been rewritten alongside the head would at least fail loudly. Section 11.6's measurement is correct and its conclusion needs inverting.

**Disposition - not a blocker, and no rule change.** The same exposure exists today under Bash: `git status | grep "nothing added"` is rewritten to `rtk git status | grep ...` by the current hook and breaks identically. This issue neither creates nor widens it, and no in-hook fix is available that is not a content-aware deny-list, which section 4.5 rightly rejects on drift grounds. What must change is that the plan stops reading as an all-clear:

1. Section 4.3 must not present "only the content is compacted" as part of the safety argument. State that the gate preserves the pipeline's type contract and **not** its content contract, and point here.
2. Section 6.3's `git status | Select-Object -First 2` row passes only because `Select-Object -First` is format-agnostic. Add the `Select-String` row as known-broken beside the section 11.1 `tree` and `find` rows, so the table is not read as clearance for pipelines generally.
3. Section 11.6's concluding sentence needs inverting as above.
4. The README failure-modes section carries it. `git status | Select-String` is the cleanest one-line demonstration, because the native command returns a match and the routed one returns nothing with no error.

### 12.2 `$?` reports success for every routed command that fails

**Amends section 4.4, section 6.5 and section 7.1. New, and distinct from section 11.2.**

Section 11.2 covers the **error** channel: `2>`, `2>&1` and `2>$null` stop working. This is the **success** channel, and it is not the same defect. Measured, same failing command, three ways:

```text
$null = & <real rtk.exe> git nosuchsubcmd 2>$null    ->  $? = False   $LASTEXITCODE = 1
$null = rtk git nosuchsubcmd 2>$null   (section 4.4) ->  $? = True    $LASTEXITCODE = 1
```

The agent-visible consequence, run as written:

```powershell
$null = rtk git nosuchsubcmd 2>$null
if ($?) { "command succeeded" }   # taken. The command exited 1.
```

Cause: `$?` after a native command reflects its exit code, but after a **function** it reflects whether that function raised a terminating or non-terminating error. The section 4.4 shadow raises neither - it writes through `[Console]::Error.WriteLine` - so `$?` is `True` regardless of what the wrapped binary did. `git ...; if ($?) { ... }` is an ordinary shape and it silently takes the wrong branch, with no output difference to notice.

**This one is not fixable inside the section 4.4 design, and I measured that rather than assuming it.** The section 11.2 `Write-Error` alternative does **not** restore it:

```text
$null = rtkB git nosuchsubcmd 2>$null   (Write-Error variant)  ->  $? = True
```

Any wrapper that turns `rtk` from an Application into a function loses `$?`, whichever way the error text is emitted. So this is a documentation obligation under **either** section 11.2 disposition, not an argument for one over the other.

`$LASTEXITCODE` **is** preserved, measured here and in section 4.4. Required:

- Section 4.4 gains it, beside the section 11.2 note.
- Section 7.1's compatibility list gains it: it is a behaviour change for routed commands.
- Section 6.5 gains it as a failure mode, and the README states plainly that after a routed command the reliable signal is `$LASTEXITCODE`, not `$?`.

### 12.3 The filter corrupts redirected stdout: line endings always, binary irrecoverably

**Amends section 4.4, section 6.5 and section 7.1.**

`& $e @args 2>&1 | ForEach-Object { ... }` decodes the binary's stdout into PowerShell strings and re-emits them. Anything downstream, including `> file`, then receives re-encoded text rather than the bytes the binary wrote. Section 6.3 lists `git status > out.txt` as a **passing** row, so redirection is reachable, not hypothetical.

Measured. Same producer, output redirected to a file, bytes compared:

```text
text, LF-separated, no trailing newline
  native : 16 bytes  61 6c 70 68 61 0a 62 65 74 61 0a 67 61 6d 6d 61
  routed : 20 bytes  61 6c 70 68 61 0d 0a 62 65 74 61 0d 0a 67 61 6d 6d 61 0d 0a
```

Every `0a` became `0d 0a` and a trailing `0d 0a` was appended. Exit 0, no error, and the console rendering looks identical.

```text
binary, first 12 bytes of a PNG header
  native : 12 bytes  89 50 4e 47 0d 0a 1a 0a 00 01 fe ff
  routed : 21 bytes  ef bf bd 50 4e 47 0d 0a 1a 0d 0a 00 01 ef bf bd ef bf bd 0d 0a
```

`89` and each of `fe`, `ff` became `ef bf bd`, the UTF-8 replacement character. The file is unrecoverable and nothing reports it.

Reachable shapes an agent writes routinely: `rtk git diff > fix.patch` then `git apply fix.patch`, which fails or applies wrongly because the patch body gained CRLF; `rtk curl -s <url> > archive.zip`; `rtk git show HEAD:script.sh > script.sh` on a repository with LF endings.

This channel is **independent of the section 11.2 decision** - I measured the `Write-Error` variant and it corrupts identically (16 bytes native against 20 routed), because the damage is on the stdout branch, not the error branch. Whoever fixes it has to stop passing stdout through `ForEach-Object` at all, which is a redesign of section 4.4 rather than a token change, and I am not asking for that in this issue.

Required: section 4.4, section 6.5 and section 7.1 state it, and the README failure-modes section carries the `> file` line-ending case, which is the one an agent will hit first.

### 12.4 Unquoted commas in an argument are split into two arguments

**Amends section 4.4.**

Because the shadow is a function, PowerShell evaluates each argument before `@args` splats it, and in argument mode an unquoted comma builds an **array**. Splatting then passes each element as a separate argv entry. Measured against an argv-echoing binary:

```text
native : node argv.js --format=a,b     ->  argc=1   [0] "--format=a,b"
routed : rtk  argv.js --format=a,b     ->  argc=2   [0] "--format=a"   [1] "b"
```

A three-way control isolates the cause to the shadow function, not to `rtk`:

```text
git log --pretty=format:%h,%s -2                    ->  CSV output, exit 0
<real rtk.exe> git log --pretty=format:%h,%s -2     ->  identical CSV output, exit 0
rtk git log --pretty=format:%h,%s -2  (section 4.4) ->  fatal: ambiguous argument '%s', exit 128
```

Here it is loud, which is the good case. Whether it is loud depends entirely on how the wrapped tool reacts to one extra positional argument, so it is not safe to assume it always will be.

Fidelity is otherwise good, and I checked rather than assuming: empty-string arguments, embedded double quotes, trailing backslashes, leading-zero numerals, `$undefined` variables, parenthesised expressions and the `--%` stop-parsing token all round-trip **identically** through the shadow. The unquoted comma is the only divergence I found.

Required: one line in section 4.4 and in the README, naming `--pretty=format:%h,%s` as the concrete case. No code change is available that keeps the shadow a function.

### 12.5 A `$PROFILE` that prints anything silently disables the PowerShell hook entirely

**Amends section 4.5 and section 6.5. One-token fix, and it is the cheapest item on this page.**

`headIsExternal` returns `(r.stdout || "").trim() === "APP"`. Section 4.5 deliberately loads the profile. A profile that writes to stdout therefore prepends its text to the probe's verdict and the equality fails for **every** command. Measured, simulating profile statements as the prelude a real `$PROFILE` executes before `-Command`:

| Prelude | probe stdout | verdict |
|---|---|---|
| none | `"APP\r\n"` | route |
| `Write-Host 'oh-my-posh banner'` | `"oh-my-posh banner\r\nAPP\r\n"` | **ignore** |
| `Write-Output 'starship init'` | `"starship init\r\nAPP\r\n"` | **ignore** |

The probe answered `APP` correctly in all three. The hook threw the answer away. Every `PowerShell` tool call then goes to the ignored log, `commands` gains zero rows, and nothing anywhere reports why.

This is not exotic. `oh-my-posh`, `starship`, `conda init`, `PSReadLine` tips and corporate login banners all write to the host on profile load, and section 11.3 already establishes that the profile runs on every single call.

Section 11.5 argues the missing-`pwsh` degenerate case is acceptable because the ignored log makes total failure obvious. That argument holds here too and I do not dispute it. It is still the wrong trade when the fix is one token:

```js
const lines = (r.stdout || "").trim().split(/\r?\n/);
return lines[lines.length - 1].trim() === "APP";
```

The probe already terminates every branch with `exit 0` immediately after printing, so its verdict is always the last line. Taking the last line instead of the whole buffer costs nothing and removes an entire class of silent total disablement. Add it to section 4.5.

### 12.6 Section 4.5's claim that the profile can only make the probe more conservative is false

**Amends section 4.5.**

Section 4.5 states:

> Loading the profile can only make the probe more conservative, never less.

A profile that **removes** a name from the command table moves the verdict the other way. Measured:

```text
prelude: Remove-Alias -Name ls -Force
cmd    : ls                             ->  probe answers APP, so the hook routes it
baseline (no prelude)                   ->  probe answers SHELL, so the hook ignores it
```

With the alias gone, `Get-Command ls` finds nothing, and section 4.5's `if (-not $g) { 'APP' }` treats an unresolved head as external. `ls` is then rewritten to `rtk ls`, which section 3.6 measured as failing on Windows outside Git Bash. The direction of the profile's influence is not one-way: adding to the command table makes the probe more conservative, removing from it makes the probe less conservative.

`Remove-Alias ls` is a real idiom on machines where someone has installed `eza` or `lsd`. The consequence there is loud rather than silent, so this is a correctness note on the argument rather than a new hazard. Section 4.5's justification for loading the profile still stands on its other leg - the probe must see the session's real command table - but the sentence quoted above must be struck or qualified, because an implementer reading it will not add the guard the section 12.5 fix provides.

For completeness, a profile that **adds** a name does behave as section 4.5 claims: prelude `function git { 'shadowed' }` flips `git status` from `APP` to `SHELL`. Measured.

### 12.7 Two factual corrections to the section 4.5 guard table

**Amends section 4.5.**

**The newline claim is wrong.** The guard table says `$st.Count -ne 1` rejects "`git status; ls`, and any command containing a newline". A newline inside a string literal or a here-string keeps the input a single statement, and both route. Measured through the hook:

```text
ROUTED | "git commit -m \"line one\nline two\""   ==>  "rtk git commit -m \"line one\nline two\""
ROUTED | "git commit -m @\"\nhello\n\"@"          ==>  "rtk git commit -m @\"\nhello\n\"@"
ROUTED | "git `\n  status"                        ==>  "rtk git `\n  status"
```

The third is a backtick line continuation, also one statement. In all three cases `rtk rewrite` handled the embedded newline correctly and the emitted command is right, so this is a **documentation defect, not a hazard** - but it is the kind of wrong statement that a later reader builds a wrong assumption on. Reword to "every command whose newline is a statement separator", and note that a newline inside a string does not split a statement.

**The `$n -like '*=*'` guard rejects less than it looks like it does.** It fires on the command **name** only, which is the bash `FOO=bar cmd` form, exactly as section 4.5 says. It is worth stating explicitly that it does not fire on an argument containing `=`, because `git log --pretty=format:%h` routes and must. No change beyond one clarifying clause; I flag it only because section 12.4's failing case is an `=` argument and a reader may otherwise expect this guard to have caught it.

### 12.8 Minor

- **The nag filter drops real stderr that happens to contain the phrase.** `-notmatch 'No hook installed'` is an unanchored substring test over the whole error record. Measured: a producer writing `fatal: cannot open No hook installed.txt` then `fatal: second real error` emits only the second line through the filter. The bash hook's `grep -v 'No hook installed'` has the identical property, so this is **parity, not a regression**, and I am not asking for a change. One difference is worth a line in the README: the bash filter drops a matching **line**, the PowerShell filter drops the entire **ErrorRecord**, which for a multi-line error is more than the matching line.

- **`git status &` routes and I did not verify what happens next.** Measured: the background operator is not checked by any guard - `PipelineAst` carries it as a `Background` flag rather than as a different node type - so `git status &` is rewritten to `rtk git status &`. Whether the prepended function reaches the job's runspace is a real question and I have **not** measured it; I am recording it as untested rather than guessing. One measurement settles it, and if the function does not reach the job the only consequence is a leaked nag line, which is cosmetic. Adding `if ($p.Background) { 'SHELL'; exit 0 }` to the probe would close it for the price of one line if the measurement comes back badly.

- **Section 11.7's export-count point is right and I confirmed the prototype's shape.** The prototype exports `{ NAG, ignoredLogPath, logIgnored, runHook }` and inlines the `rtk rewrite` `spawnSync` in the PowerShell hook. Section 4.2's six-symbol contract is the one to implement, and it is where section 11.3's `timeout` belongs.

### 12.9 My position on the two decisions section 11 escalated

**Section 11.1, the `Application` gate and the `tree` / `find` class. I agree with `dev-rust`: document the known-broken rows, file it upstream against `rtk`, add no deny-list.** Section 12.1 strengthens that conclusion rather than weakening it. A deny-list would have to be keyed on the **head**, and section 12.1's failure is not determined by the head - `git`, the plan's own safe example, breaks when the tail parses text and does not break when the tail is `Select-Object -First 2`. The property that decides it lives in the tail, which no list of names can enumerate. Whatever a deny-list bought, it would still leave section 12.1 open while creating exactly the drifting second copy of a platform question that section 4.5 exists to avoid. Document and escalate upstream is the right disposition, and it should now cover section 12.1's class as well as section 11.1's.

**Section 11.2, `[Console]::Error.WriteLine` against `Write-Error -ErrorRecord`. I agree with `dev-rust`: keep `[Console]::Error.WriteLine` and document the limitation.** I went in expecting to argue the other side, on the theory that `Write-Error` would also restore `$?` and so buy two channels for one cost. I measured it and it does not: `$?` stays `True` under both variants (section 12.2), and redirected stdout is corrupted identically under both (section 12.3). So `Write-Error` buys exactly the one channel section 11.2 already credits it with - `2>`, `2>&1` and `2>$null` - and pays a ConciseView position block on every error line of every routed command, in a hook whose purpose is compacting output. One channel restored out of three broken, at a per-error-line tax, is not a good trade. `dev-rust`'s recommendation stands, and it stands more firmly with the measurement than without it.

The condition `dev-rust` attached is the important half and I restate it: shipping section 4.4 as written **with no note** is not acceptable. With sections 11.2, 12.2 and 12.3 the note is now three items, and the README must carry all three, because an agent cannot see any of them.

### 12.10 Acceptance criteria added

Numbered on from criterion 21. Criteria 22 to 25 are reviewer checks against the branch; 26 belongs with the section 9.3 runtime checks.

| # | Criterion | Owner |
|---|---|---|
| 22 | The section 6.3 table carries `git status \| Select-String "nothing added to commit"` as a known-broken row beside the section 11.1 `tree` and `find` rows, and section 4.3 no longer offers "only the content is compacted" as part of its safety argument. | reviewer |
| 23 | `headIsExternal` compares the **last non-empty line** of the probe's stdout against `APP`, not the whole trimmed buffer (section 12.5). `grep -n "APP" docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` shows the split, not a bare `=== "APP"`. | reviewer |
| 24 | The README failure-modes section names all three routed-command behaviour changes an agent cannot see: `$?` always True (section 12.2), stderr redirection inert (section 11.2), and `> file` re-encoding stdout (section 12.3). It states that `$LASTEXITCODE` is the reliable signal. Fewer than three is a documentation defect, not a pass. | reviewer |
| 25 | Section 4.5's sentence "Loading the profile can only make the probe more conservative, never less" is struck or qualified, and its guard table no longer claims that `$st.Count -ne 1` rejects any command containing a newline (sections 12.6 and 12.7). | reviewer |
| 26 | In a real `PowerShell` tool call in the seeded replica, `git status \| Select-String "nothing added to commit"` returns zero results with exit 0, and the README predicted it. This is the section 12.1 case observed end to end; it is expected to fail, and the criterion is that the documentation said so in advance. | tech-lead, or `ac-cli-tester` |
