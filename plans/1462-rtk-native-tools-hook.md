# Plan #1462: Record native-tool usage in the RTK savings database

Author: architect, wg-4-community. Authored 2026-08-20 UTC on the Lite delivery path (authored and certified in one pass).

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1462](https://github.com/mblua/AgentsCommander/issues/1462), `Record Read/Grep/Glob/Edit/Write tool usage in the RTK savings DB via Claude Code hooks`.

This change adds two files and edits five under `docs/integrations/rtk_claude/`, and rewords one sentence of `docs/integrations/rtk.md`. It touches no Rust, no TypeScript, no CSS, no build script, no CI workflow and nothing under `src-tauri/` or `src/`. It adds no crate, no npm dependency, no Tauri command, no IPC surface, no event and no migration.

---

## 1. Frozen authority and entry gate

The implementation working tree is `repo-AgentsCommander`, branch `feature/1462-rtk-native-tools-hook`, targeting `main`.

At authoring time all three of the following resolved exactly to `1376c2b84a23125624e919c9af7e65813d624241`:

- committed `HEAD` of `feature/1462-rtk-native-tools-hook`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

Every line number in this plan refers to that SHA. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if any of the three no longer equals the frozen SHA. Do not rebase, merge a moved base, or silently substitute a newer commit.

Repository mechanics verified at the frozen SHA:

- Root `.gitignore:11` is `/plans/`. This plan file must be force-added: `git add -f plans/1462-rtk-native-tools-hook.md`. Do not remove or weaken that ignore rule.
- `scripts/validate-branch-name.mjs:15` accepts type `feature`. `feature/1462-rtk-native-tools-hook` parses as type `feature`, number `1462`, slug `rtk-native-tools-hook` (21 characters, under the 50-character `MAX_SLUG` cap at `:16`). The `validate-branch-name` check passes.
- `git status --porcelain=v1 --untracked-files=all` at freeze time reported **zero** entries. The tree is clean.
- `.gitattributes:9` is `docs/integrations/rtk_claude/hooks/*.js text eol=lf` and `.gitattributes:2` is `*.sh text eol=lf`. Both files this plan adds are covered already. **Do not edit `.gitattributes`.**
- `core.autocrlf` is `true` in this clone, so a working-tree digest of a `.md` file is not reproducible across a checkout. Any digest quoted for a Markdown file must be taken from the blob: `git show <commit>:<path> | sha256sum`.

### 1.1 Working-copy rule for anyone who runs the hook

A replica's `.claude/` directory is destroyed and rebuilt on every spawn: `perform_config_seed_with_clock_and_hooks` stages a fresh tree, renames the existing `.claude` aside and installs atomically (`src-tauri/src/config/config_seed.rs:463-687`).

**Keep every scratch copy and test harness at your replica root, never inside `.claude/`.** Work left in `.claude/` is gone at the next spawn.

---

## 2. Objective

Close the blind spot the issue names: `Read`, `Grep`, `Glob`, `Edit`, `Write` and `NotebookEdit` run inside Claude Code, never reach a shell, and therefore never reach `rtk`. The RTK database is the operator's record of what an agent consumed, and today the largest share of what an agent reads and writes is absent from it with nothing marking the gap.

After this change, every successful call to one of those six tools adds one row to the `commands` table of the database `RTK_DB_PATH` names, and `rtk gain`, `rtk gain -H` and `rtk gain -f json|csv` count it alongside the shell commands.

The hook is **observability only**. It does not filter, rewrite, truncate, deny or measurably delay any tool call.

---

## 3. Evidence and current-state gap

### 3.1 What the mirror holds today

`docs/integrations/rtk_claude/` holds five tracked entries at the frozen SHA:

```text
docs/integrations/rtk_claude/README.md                          30079 bytes
docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js         4784 bytes,  83 lines
docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js   7366 bytes, 123 lines
docs/integrations/rtk_claude/hooks/ac_rtk_shared.js              5844 bytes, 129 lines
docs/integrations/rtk_claude/settings.local.json                  636 bytes,  31 lines
```

Those are **blob** byte counts. `core.autocrlf` is `true` and `.gitattributes` does not pin `*.md`, so `README.md` checks out CRLF and measures 30437 bytes in the working tree. Compare working-tree file against working-tree file, or blob against blob, never one against the other.

`settings.local.json:9-30` declares one `PreToolUse` array with two entries, matchers `Bash` and `PowerShell`. There is no `PostToolUse` key and no other matcher.

**The old single `ac_rtk_claude.js` is already gone from the mirror at this SHA**, and so is the split that #1416 performed. The dispatch statement that the mirror still holds a 132-line `ac_rtk_claude.js` was measured against a stale clone (`90c429f8`) and is withdrawn by the coordinator. **This plan deletes no hook file.**

### 3.2 The mirror is a mirror, and it is nearly in sync already

The seed AC installs from is `<workspace>/.ac/default.claude/`, an untracked on-disk folder outside every repository (`resolve_config_seed`, `config_seed.rs:151-211`, selects it as tier `ConfigSeedTier::WorkspaceBase` at `:187`; `copy_tree_internal`, `config_seed.rs:1149-1203`, copies every regular file in every subdirectory with no filter on name or extension). Nothing in Rust generates the hooks or the matcher block: the only Rust occurrence of `PreToolUse` in the whole repository is a test fixture at `config_seed.rs:2119-2127`.

Compared byte for byte on 2026-08-20 against `D:\0_repos\AgentsCommander_iac\.ac\default.claude\`:

| File | Mirror vs seed |
|---|---|
| `hooks/ac_rtk_claude_Bash.js` | identical, SHA-256 `5D7B292F89865A00...` |
| `hooks/ac_rtk_claude_PowerShell.js` | identical, SHA-256 `261FECC8F2272F9F...` |
| `hooks/ac_rtk_shared.js` | identical, SHA-256 `A929BCBA9D507D41...` |
| `README.md` | identical, SHA-256 `0595F49CBB6B8248...` |
| `settings.local.json` | mirror 636 bytes, seed 755 bytes. The seed adds a trailing 4-line `statusLine` block and nothing else. |
| `statusline.sh` | **absent from the mirror**, present in the seed, 968 bytes, 17 lines |

So the mirror has exactly two gaps against the seed, and both are additive in the safe direction.

### 3.3 What the hook payload carries, and what it does not

Verified against the Claude Code hooks reference and the installed harness (`claude 2.1.237`):

- Common fields on every hook payload: `session_id`, `prompt_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`.
- `PreToolUse` adds `tool_name`, `tool_input`, `tool_use_id`.
- `PostToolUse` adds `tool_response` on top of those.
- **No payload carries a timing field.** This is why `PreToolUse` has to leave a start mark for `PostToolUse` to read.
- **`PostToolUse` fires only when the tool succeeded.** A failed call raises the separate `PostToolUseFailure` event, which this issue does not register. Failed tool calls are therefore not recorded, by design.
- **A matcher made only of letters, digits, `_`, `-`, spaces, `,` and `|` is an exact list, not a regular expression.** `Read|Grep|Glob|Edit|Write|NotebookEdit` matches those six names and nothing else, and matching is case-sensitive. A tool that is not on the list cannot reach the hook.
- On exit 0, hook stdout and stderr go to the Claude Code debug log only. Neither reaches the model and neither is shown to the user.
- Exit 2 on `PreToolUse` blocks the tool call. Any other nonzero exit is a non-blocking error whose first stderr line is shown to the user in a notice. **This hook must therefore always exit 0.**
- All matching hooks for one event run in parallel, in the session's current working directory, which is the `cwd` field of the payload.

Tool input shapes, confirmed against the installed tool schemas:

| Tool | `tool_input` keys this hook reads | Other keys present |
|---|---|---|
| `Read` | `file_path` | `offset`, `limit`, `pages` |
| `Grep` | `pattern`, `path` | `glob`, `type`, `output_mode`, `-i`, `-n`, `head_limit`, ... |
| `Glob` | `pattern`, `path` | none |
| `Edit` | `file_path` | `old_string`, `new_string`, `replace_all` |
| `Write` | `file_path` | `content` |
| `NotebookEdit` | `notebook_path` | `cell_id`, `new_source`, `cell_type`, `edit_mode` |

**The `tool_response` shapes are not documented per tool and must not be assumed.** The token estimate serializes whatever arrives (section 4.4).

### 3.4 The database, measured

`RTK_DB_PATH` is an operator-managed ENVIRONMENT row on the coding agent, currently `%AC_MATRIX_ROOT%\rtk-matrix-history-claude.db` for Claude sessions. Measured against rtk 0.42.4 on the authoring workstation:

- The schema is exactly the one `docs/integrations/rtk.md:204-223` already documents. `commands` is `(id, timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path)`, `journal_mode` is `wal`, `user_version` is 0.
- A database rtk has never written does not exist as a file. rtk creates it on its first write.
- `timestamp` is RFC 3339 with nine fractional digits and a `+00:00` offset, for example `2026-08-20T05:52:22.150418200+00:00`.
- `project_path` on Windows is the canonicalized current directory carrying the verbatim `\\?\` extended-length prefix. `rtk gain -p` filters on that exact string, so a row storing the plain path is silently excluded from the project-scoped report.
- `rtk gain` accepts rows it did not write. Rows labelled `tool:read`, `tool:grep`, `tool:glob`, `tool:edit`, `tool:write` and `tool:notebookedit` appear under **By Command** and in **Recent Commands** with `-H`, and are counted in the totals and in `-f json`.
- `rusqlite`, which rtk uses, sets `sqlite3_busy_timeout` to 5000 ms by default (`rusqlite-0.32.1/src/inner_connection.rs:119`). rtk therefore waits up to five seconds for a competing writer, which is far longer than this hook ever holds the lock.

### 3.5 Node on the box

`node --version` is `v24.13.0`, so `node:sqlite` and its `DatabaseSync` class are available with no dependency and no `npm install`. Two measured properties drive section 4:

- `new DatabaseSync(path)` **creates the file** when it does not exist. The hook must test for existence first, or a mistyped `RTK_DB_PATH` silently produces an empty database that rtk never reads.
- `node:sqlite` is still flagged experimental, so requiring it prints `ExperimentalWarning: SQLite is an experimental feature` to stderr. `process.removeAllListeners("warning")` before the require suppresses it. A `--disable-warning=ExperimentalWarning` flag on the registered command would do the same on Node 21.3 and later, but on an older Node an unknown flag makes the process exit 9 before the script starts, which is the loud failure this hook must never produce. The in-code form is therefore the decided one.

### 3.6 The mirror cannot be executed in place

`package.json:5` is `"type": "module"`, so Node treats every `.js` under this repository as an ES module and the CommonJS hooks fail with:

```text
ReferenceError: require is not defined in ES module scope, you can use import instead
```

The hooks are correct; the repository is the wrong place to run them from. Copy them to a scratch directory at your **replica root** (never inside `.claude/`, per section 1.1) and run them there. Do not "fix" this with a `.cjs` extension or a `package.json` beside the hooks: both change what the seeder installs.

---

## 4. The decided solution

### 4.1 One new file, registered for two events

Add `docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js`, CommonJS, and register it in `docs/integrations/rtk_claude/settings.local.json` under both `PreToolUse` and `PostToolUse` with the matcher `Read|Grep|Glob|Edit|Write|NotebookEdit`. The existing `Bash` and `PowerShell` `PreToolUse` entries stay exactly as they are.

One script serves both events and switches on `hook_event_name`. Two scripts would duplicate the tool table, the mark path and the label map for no gain.

### 4.2 It requires nothing from `ac_rtk_shared.js`, and that is deliberate

`ac_rtk_shared.js` exports `{ NAG, ALREADY_RTK, rtkRewrite, ignoredLogPath, logIgnored, runHook }`. Every one of them is wrong for this hook:

- `runHook(hookDir, tool, decide)` is a `PreToolUse`-only contract. It reads `tool_input.command`, exits early when that field is absent (`ac_rtk_shared.js:101-103`), and ends in either an ignored-log line or a `permissionDecision: "allow"` JSON document. This hook has no `command` field, must produce no stdout at all, and must handle a second event.
- `logIgnored` and `ignoredLogPath` write the ignored-tools log, which requirement 5 of the issue forbids for this hook.
- `NAG`, `ALREADY_RTK` and `rtkRewrite` are about rewriting shell commands, which this hook never does.

The only genuinely shared behaviour is "concatenate stdin and `JSON.parse` it", five lines. Extracting it would mean editing a file that both live hooks depend on and that is currently byte-identical to the seed, forcing a re-verification of the whole Bash and PowerShell routing suite to save five lines. **The new hook imports nothing from `ac_rtk_shared.js`, and `ac_rtk_shared.js` gains no export.** Its only edit in this issue is the ignored-log rename of section 4.6.

### 4.3 The timing mark

`PreToolUse` writes `String(Date.now())` into `<os.tmpdir()>/ac-rtk-claude-tools/<tool_use_id>`, with every character outside `[A-Za-z0-9_-]` replaced by `_` so a hostile or unusual id cannot escape the directory. It writes nothing to stdout, so the tool proceeds through the normal permission flow untouched.

`PostToolUse` reads the mark, deletes it, and uses `now - start` as `exec_time_ms`. A missing, unreadable or future-dated mark yields **0**, which is also what `rtk` stores for commands it did not time.

**Stale-file policy:** a mark outlives its call only when `PostToolUse` never ran, which happens when the tool failed (the failure raises `PostToolUseFailure` instead) or the call was interrupted. Every `PreToolUse` sweeps the mark directory and unlinks anything whose mtime is older than one hour. The directory is this hook's own, so the sweep never lists an unrelated file, and the deletion in `PostToolUse` keeps it near-empty in steady state.

`exec_time_ms` measured this way includes the `PostToolUse` hook's own Node start-up, about 40 ms on the authoring workstation, and therefore about 70 to 80 ms for a fast tool call. It measures volume and frequency, not tool latency. The README must say so.

### 4.4 The row

One `INSERT` per successful call, into the existing `commands` table. No `CREATE TABLE`, no `ALTER TABLE`, no migration: the schema belongs to rtk.

```sql
INSERT INTO commands
  (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
```

| Column | Value |
|---|---|
| `timestamp` | `new Date(now).toISOString().replace("Z", "000000+00:00")`, which turns `2026-08-20T06:55:55.062Z` into `2026-08-20T06:55:55.062000000+00:00`, rtk's nine-digit RFC 3339 UTC shape |
| `original_cmd` | the tool name, then whichever of `file_path`, `notebook_path`, `pattern`, `path` are present as non-empty strings, joined by single spaces. For example `Read D:/x/a.rs`, `Grep fn main D:/x`, `NotebookEdit D:/x/n.ipynb` |
| `rtk_cmd` | `tool:read`, `tool:grep`, `tool:glob`, `tool:edit`, `tool:write`, `tool:notebookedit` |
| `input_tokens` | the estimate below |
| `output_tokens` | the same estimate, so `saved_tokens` and `savings_pct` are consistent with it |
| `saved_tokens` | `0` |
| `savings_pct` | `0.0` |
| `exec_time_ms` | section 4.3 |
| `project_path` | `cwd` from the payload, resolved with `fs.realpathSync.native`, prefixed with `\\?\` on Windows when it starts with a drive letter, so it matches the exact string rtk stores and `rtk gain -p` groups the rows together |

**Token estimate, pinned:** `Math.ceil((utf8Bytes(JSON.stringify(tool_input)) + utf8Bytes(JSON.stringify(tool_response))) / 4)`. A part that is absent or not serializable counts as zero.

Both halves are counted because the volume lives on a different side per tool: a `Read` puts it in `tool_response`, a `Write` puts it in `tool_input`. Counting only the response would score a 50 KB `Write` at about ten tokens. Serializing whatever arrives, rather than reaching for named fields, is what makes the estimate survive the undocumented and version-dependent `tool_response` shapes of section 3.3.

### 4.5 Failure behaviour is total

Every failure ends in **exit 0 with empty stdout**. At most one line reaches stderr, which on exit 0 is debug-log only. Nothing is ever written to the ignored-tools log.

Covered and measured: `RTK_DB_PATH` unset; `RTK_DB_PATH` naming a file that does not exist (no row, and **the file is not created**); a file that is not a SQLite database; a database with no `commands` table; a database another writer holds past the busy timeout; unparseable stdin; empty stdin; a JSON array or `null` instead of an object; a `tool_input` of `null`; a tool name not on the list; an event that is neither `PreToolUse` nor `PostToolUse`; a Node too old for `node:sqlite`, which fails on the lazy require inside the same `try`.

A `process.on("uncaughtException", () => process.exit(0))` line is the backstop for what no `try` can reach, such as an error event on stdin. It is what makes "never a nonzero exit" true by construction rather than by inspection.

### 4.6 The ignored-log rename, and why the mirror deliberately differs from the seed

The operator renamed the ignored-tools log from `rtk_ignored_tools.md` to `rtk-ignored-tools-claude.md`. Running replica hooks already use the new name; the seed does not. The mirror must carry the **new** name, in `ac_rtk_shared.js:58` and in the comment lines at `ac_rtk_shared.js:109`, `ac_rtk_claude_Bash.js:43` and `ac_rtk_claude_PowerShell.js:49`, and throughout the README.

This makes those three files differ from today's seed on purpose. The operator then copies the mirror into the seed, which is how the rename lands there (section 8.5). **State this in the plan, in the README and in the pull request, so nobody "fixes" the mirror back to the old name.**

### 4.7 What this deliberately does not do

Recording zero-savings rows lowers the global percentage `rtk gain` reports. That is the intended visibility and not a regression: the denominator was always wrong, and now it is honest. The README must say so where it explains the new rows.

---

## 5. Affected surfaces

### 5.1 `docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js` (new)

This is the specification. Create the file with exactly this content, LF line endings, trailing newline, no BOM. A working copy that produced every measurement in section 9 is at `<architect replica root>/1462-proto/ac_rtk_claude_Tools.js`; it is identical to this listing and is evidence, not a second source.

```javascript
// PreToolUse + PostToolUse hook for Claude Code's native file tools: `Read`, `Grep`,
// `Glob`, `Edit`, `Write` and `NotebookEdit`. Observability only.
//
// Those six tools run inside Claude Code and never reach a shell, so rtk never sees
// them and `rtk gain` under-reports what an agent actually consumed. This hook does
// not filter, rewrite, deny or slow the call; it records one row per successful call
// in the same `commands` table rtk writes, in the database `RTK_DB_PATH` names, so
// `rtk gain`, `rtk gain -H` and `rtk gain -f json|csv` count them next to the shell
// commands. The rows carry zero savings on purpose: they are volume, not savings.
//
// One script serves both events and switches on `hook_event_name`. `PreToolUse`
// stores the start time in a small mark file keyed by `tool_use_id` and prints
// nothing, so the call proceeds through the normal permission flow untouched.
// `PostToolUse` reads the mark back, estimates tokens from the payload and inserts
// the row. There is no timing field in any hook payload, which is why the mark exists.
//
// Every failure is swallowed: no `RTK_DB_PATH`, a database that does not exist yet
// (rtk creates it on its first write, and this hook never creates it, because the
// schema belongs to rtk), a locked database past the busy timeout, a schema this
// insert does not fit, an unreadable payload. Each ends in exit 0 with empty stdout
// and at most one line on stderr, which Claude Code keeps in its debug log. Nothing
// here writes to the ignored-tools log: that file records shell commands the sibling
// hooks declined to rewrite, and these tools are never rewritten.
//
// Only `node:` builtins are used. `node:sqlite` ships with Node 22.5 and later and is
// required lazily, so an older Node ends in the same silent exit 0. It is still flagged
// experimental, so requiring it prints a warning to stderr; the require is preceded by
// `process.removeAllListeners("warning")` rather than by a `--disable-warning` flag on
// the registered command, because an unknown flag makes Node exit 9 before the script
// starts, which is exactly the loud failure this hook must never produce.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

// `rtk_cmd` per tool. Distinctive non-rtk labels, so `rtk gain`'s "By Command" table
// groups them apart from real rtk invocations. The key set is also the tool allow-list.
const TOOLS = {
  Read: "tool:read",
  Grep: "tool:grep",
  Glob: "tool:glob",
  Edit: "tool:edit",
  Write: "tool:write",
  NotebookEdit: "tool:notebookedit",
};

// Start marks live in their own directory under the OS temp dir, so the stale sweep
// only ever lists this hook's files. A mark outlives its call only when PostToolUse
// never ran (the tool failed, or the user interrupted), so anything older than an
// hour is garbage.
const MARK_DIR = path.join(os.tmpdir(), "ac-rtk-claude-tools");
const STALE_MS = 60 * 60 * 1000;

// How long an insert waits for a busy database before giving up. Writers hold the
// lock for about a millisecond, so this is only ever reached when something else
// holds a long write transaction, and then losing one row beats stalling the agent.
const BUSY_MS = 500;

function markPath(id) {
  if (typeof id !== "string" || !id) return null;
  return path.join(MARK_DIR, id.replace(/[^A-Za-z0-9_-]/g, "_"));
}

function sweep(now) {
  let names;
  try {
    names = fs.readdirSync(MARK_DIR);
  } catch {
    return;
  }
  for (const n of names) {
    const p = path.join(MARK_DIR, n);
    try {
      if (now - fs.statSync(p).mtimeMs > STALE_MS) fs.unlinkSync(p);
    } catch {
      // best effort
    }
  }
}

function onPre(data, now) {
  const p = markPath(data.tool_use_id);
  if (!p) return;
  fs.mkdirSync(MARK_DIR, { recursive: true });
  fs.writeFileSync(p, String(now), "utf8");
  sweep(now);
}

// Milliseconds since the matching PreToolUse, 0 when there is no usable mark. The
// mark is removed either way so it cannot be counted twice.
function elapsed(data, now) {
  const p = markPath(data.tool_use_id);
  if (!p) return 0;
  let start;
  try {
    start = Number(fs.readFileSync(p, "utf8"));
    fs.unlinkSync(p);
  } catch {
    return 0;
  }
  return Number.isFinite(start) && start <= now ? Math.round(now - start) : 0;
}

// `original_cmd`: the tool name followed by the path-like arguments the call named,
// never by content. `Read`, `Edit` and `Write` carry `file_path`, `NotebookEdit`
// carries `notebook_path`, and `Grep` and `Glob` carry `pattern` plus an optional
// `path`. Patterns are search strings the agent typed, the same thing rtk already
// stores for a shell `grep`. A key that is absent contributes nothing, so a payload
// shape this list does not know still produces a row naming the tool.
function originalCmd(data) {
  const ti = data.tool_input && typeof data.tool_input === "object" ? data.tool_input : {};
  const parts = [data.tool_name];
  for (const k of ["file_path", "notebook_path", "pattern", "path"]) {
    if (typeof ti[k] === "string" && ti[k]) parts.push(ti[k]);
  }
  return parts.join(" ");
}

// Token estimate: UTF-8 bytes of the serialized `tool_input` plus `tool_response`,
// divided by four and rounded up. The shapes of `tool_response` are not documented
// per tool, so whatever arrives is serialized as is; a missing or unserializable
// part counts as zero.
function jsonBytes(v) {
  if (v === undefined) return 0;
  try {
    return Buffer.byteLength(JSON.stringify(v), "utf8");
  } catch {
    return 0;
  }
}

// rtk stores `std::fs::canonicalize(current_dir)`, which on Windows carries the
// `\\?\` extended-length prefix. Matching that form is what lets `rtk gain -p` put
// these rows in the same project as rtk's own.
function projectPath(data) {
  const cwd = typeof data.cwd === "string" && data.cwd ? data.cwd : process.cwd();
  let real = cwd;
  try {
    real = fs.realpathSync.native(cwd);
  } catch {
    // keep the payload's cwd
  }
  return process.platform === "win32" && /^[A-Za-z]:\\/.test(real) ? "\\\\?\\" + real : real;
}

function onPost(data, now) {
  const exec = elapsed(data, now); // always, so the mark is consumed even when no row is written
  const dbPath = process.env.RTK_DB_PATH;
  if (!dbPath || !fs.existsSync(dbPath)) return;
  process.removeAllListeners("warning"); // silence node:sqlite's ExperimentalWarning
  const { DatabaseSync } = require("node:sqlite");
  const tokens = Math.ceil((jsonBytes(data.tool_input) + jsonBytes(data.tool_response)) / 4);
  const timestamp = new Date(now).toISOString().replace("Z", "000000+00:00"); // rtk's RFC 3339 shape
  const db = new DatabaseSync(dbPath, { timeout: BUSY_MS });
  try {
    db.prepare(
      "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
    ).run(timestamp, originalCmd(data), TOOLS[data.tool_name], tokens, tokens, 0, 0.0, exec, projectPath(data));
  } finally {
    db.close();
  }
}

// Last line of defence for the "never a nonzero exit" requirement. Everything below
// already runs inside a `try`, so this only catches what no `try` can reach, such as
// an error event on stdin. A nonzero exit would put a notice in front of the user.
process.on("uncaughtException", () => process.exit(0));

const chunks = [];
process.stdin.on("data", (c) => chunks.push(c));
process.stdin.on("end", () => {
  try {
    const data = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    if (!data || typeof data !== "object" || !TOOLS[data.tool_name]) return;
    const now = Date.now();
    if (data.hook_event_name === "PreToolUse") onPre(data, now);
    else if (data.hook_event_name === "PostToolUse") onPost(data, now);
  } catch (e) {
    process.stderr.write(`ac_rtk_claude_Tools: ${e && e.message ? e.message : e}\n`);
  }
});
```

**Binding constraint:** none of the four seed placeholders `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%`, `%AC_MATRIX_ROOT%`, `%USER_HOME%` may appear in this file (`copy_file_substituted`, `config_seed.rs:1205-1245`, would rewrite them at seed time). The content above contains none.

### 5.2 `docs/integrations/rtk_claude/settings.local.json`

Becomes 55 lines and 1240 bytes. Lines 2-8 (`includeCoAuthoredBy`, `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `mcpServers`, `claudeMdExcludes`) are unchanged, the two existing `PreToolUse` entries are unchanged, a third `PreToolUse` entry and a new `PostToolUse` key are added, and the seed's `statusLine` block is brought in. Two-space indentation, trailing newline.

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
      },
      {
        "matcher": "Read|Grep|Glob|Edit|Write|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "node .claude/hooks/ac_rtk_claude_Tools.js"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Read|Grep|Glob|Edit|Write|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "node .claude/hooks/ac_rtk_claude_Tools.js"
          }
        ]
      }
    ]
  },
  "statusLine": {
    "type": "command",
    "command": "bash \"${CLAUDE_PROJECT_DIR:-.}/.claude/statusline.sh\""
  }
```

**No `timeout` field is set.** The default for a command hook is 600 seconds, and this hook finishes in under 200 ms on every measured path including the busy-timeout path. A lower value would add a knob with nothing to tune.

### 5.3 `docs/integrations/rtk_claude/statusline.sh` (new)

A byte-for-byte copy of `<workspace>/.ac/default.claude/statusline.sh` (968 bytes, 17 lines, LF, SHA-256 `4997A3970E088628...`). It is copied, not authored: the mirror exists so a reviewer sees what the seed installs, and this file is currently missing from it. Do not modify its content. `.gitattributes:2` already pins `*.sh` to `eol=lf`.

### 5.4 `docs/integrations/rtk_claude/hooks/ac_rtk_shared.js`

One functional edit and one comment edit, both the section 4.6 rename. Nothing else in the file changes, and no export is added or removed.

| Line | Change |
|---|---|
| 58 | `"rtk_ignored_tools.md"` becomes `"rtk-ignored-tools-claude.md"` |
| 109 | the comment `is appended to \`rtk_ignored_tools.md\` before we bow out.` takes the new name |

### 5.5 `docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js` and `ac_rtk_claude_PowerShell.js`

Comment-only edits, the same rename: `ac_rtk_claude_Bash.js:43` and `ac_rtk_claude_PowerShell.js:49` each name `rtk_ignored_tools.md` in their closing header paragraph and take the new name. **No code in either file changes**, and their routing behaviour must be observably identical to the frozen files.

### 5.6 `docs/integrations/rtk_claude/README.md`

`docs/style-guide.md` binds: lead with a concrete outcome, second person present tense active voice, one concept per H2, show the exact command and its expected first lines, name the exact error string, no banned words, every snippet must run. Line numbers are at the frozen SHA.

| Lines | Required change |
|---|---|
| 3 | The promise sentence covers three hooks and both artifacts. It must now promise that the reader can say which shell command landed in which file **and** which native-tool call is in the database. |
| 5-10 | The copied-file list gains `hooks/ac_rtk_claude_Tools.js` and `statusline.sh`. State each file's real line count **as landed**; do not carry a number from this plan. |
| 12 | "All four" becomes the new count. The mirror statement stays, and gains the section 4.6 sentence: the ignored-log name in this directory is `rtk-ignored-tools-claude.md`, which is deliberately ahead of the seed, and the operator copies this directory into the seed. |
| 14-23 | The replica tree listing gains `.claude/hooks/ac_rtk_claude_Tools.js` and `.claude/statusline.sh`. |
| 25-40 | The registration excerpt becomes the block of section 5.2, including the `PostToolUse` key. |
| 42 | "Before every `Bash` tool call and every `PowerShell` tool call" gains the native tools and both events, and states that the tools hook answers with **nothing** rather than with `permissionDecision`. |
| 44-51 | "Two files come out of that decision" is now wrong in two ways: the database row for a native tool is written by a hook, not by `rtk`, and the sentence must say which hook writes what. Update the table's `rtk-matrix-history*.db` row to name both writers. The anchor at line 51 points at the H2 at line 69; change both together or the link breaks. |
| 49, 63, 65, 90, 137, 246, 256, 262, 283, 327 | Every occurrence of `rtk_ignored_tools.md`, including the H2 at 256, becomes `rtk-ignored-tools-claude.md`. |
| 57-63 | The comparison table's "Matchers registered" row becomes `Bash`, `PowerShell`, and the six native tools across two events. |
| 69-83 | The H2 "What they cover: the two shell tools, and nothing else" is now false. Rewrite the heading and the section: covered are the two shell tools plus `Read`, `Grep`, `Glob`, `Edit`, `Write`, `NotebookEdit`; still uncovered are `WebFetch`, `WebSearch`, `Task`, `Bash` output beyond what rtk filters, every MCP tool, and any other tool the harness exposes. Add the two new gaps: a **failed** native-tool call raises `PostToolUseFailure` and is not recorded, and a native-tool call made when `RTK_DB_PATH` is unset or names a missing file is not recorded either. Keep the existing "close to complete but not total" list, which is still correct for the shell hooks. |
| 220-246 | "Reproduce any row" gains a native-tools example: the exact `PreToolUse` and `PostToolUse` payloads piped into `node ac_rtk_claude_Tools.js`, the expected empty stdout and exit 0, and the row that appears. Keep the section 3.6 warning about running the mirror in place. |
| 289-307 | "What reaches the database" opens with "Neither hook touches the database", which is now false. Rewrite: the two shell hooks still touch nothing and `rtk` writes their rows; `ac_rtk_claude_Tools.js` writes its own row directly with `node:sqlite`. Add the new-row table (the six `rtk_cmd` labels, `original_cmd` shape, zero savings, `exec_time_ms` including hook start-up per section 4.3), the `RTK_DB_PATH` dependency, and one sentence of section 4.7 stating that zero-savings rows lower the reported percentage on purpose. |
| 309-352 | Failure modes gain one H3 for the tools hook listing the section 4.5 set with the exact strings a reader will see in the debug log (`file is not a database`, `no such table: commands`, `database is locked`), and stating that all of them end in exit 0 with the tool call unaffected. Update "Neither hook ever blocks a tool" at 349 to cover three hooks and both events, naming that this hook writes no stdout at all. |
| 353-358 | "See also" is unchanged. |

Add one short H2 or a paragraph in the database section showing how to read the new rows, with real output:

```bash
rtk gain -H
```

```text
  7.  tool:read                     3      0    0.0%    24ms  ░░░░░░░░░░
```

### 5.7 `docs/integrations/rtk.md`

Line 91 says AgentsCommander seeds `PreToolUse` hooks "one for the `Bash` tool and one for the `PowerShell` tool, and those also write a log of the commands they declined to rewrite". After this change a third hook exists and it writes database rows rather than log lines. Reword that one sentence and keep the link to `rtk_claude/README.md` intact.

Out of scope, recorded so it is not lost: line 93's claim that the `rtk init` excerpt is installed "once per shell tool" is about **RTK's** hook, not AC's, and #1416 already deferred it to its own issue. **Do not change it here.**

---

## 6. Required behaviour, edge cases and failure behaviour

### 6.1 `PreToolUse`

1. Concatenate stdin, `JSON.parse(text || "{}")`.
2. If the parse throws, if the result is not an object, or if `tool_name` is not a key of `TOOLS`, return with no output.
3. Write `String(Date.now())` to the mark path. Create the mark directory if needed.
4. Sweep marks older than one hour.
5. Exit 0. **Never write to stdout**, so the tool proceeds through the normal permission flow with the input the model wrote.

### 6.2 `PostToolUse`

1. Steps 1 and 2 of 6.1.
2. Compute `exec_time_ms` from the mark and delete the mark. Do this **before** any database work, so a call that writes no row still consumes its mark and cannot double-count later.
3. If `RTK_DB_PATH` is unset or empty, or names a path that does not exist, return.
4. Require `node:sqlite` lazily, open with `{ timeout: 500 }`, insert one row per section 4.4, close in a `finally`.
5. Exit 0 with empty stdout.

### 6.3 Edge cases, each measured

| Input | Result |
|---|---|
| `PostToolUse` with no preceding `PreToolUse` | one row, `exec_time_ms` = 0 |
| `tool_input` is `null` or absent | one row, `original_cmd` is the bare tool name |
| `tool_response` absent | one row, the estimate counts `tool_input` only |
| `tool_use_id` absent or not a string | no mark is written or read, `exec_time_ms` = 0, the row is still written |
| `tool_name` not in the list (`WebFetch`, `Bash`, ...) | no row, exit 0, empty stdout |
| `hook_event_name` is `PostToolUseFailure` or anything else | no row, exit 0, empty stdout |
| stdin is empty, `{not json`, `[1,2]`, or `null` | no row, exit 0, empty stdout |
| two tool calls in one assistant turn | both rows land; hooks run in parallel and SQLite serializes the writers |

### 6.4 Failure behaviour

Every row of section 4.5 ends in exit 0 with empty stdout. Measured wall time per invocation on the authoring workstation, including Node start-up:

| Path | Time |
|---|---|
| `PreToolUse`, mark written | 43 to 52 ms |
| `PostToolUse`, row inserted | 47 to 58 ms |
| any early return (no env, no database, wrong tool, bad stdin) | 41 to 47 ms |
| `PostToolUse` against a database another writer holds | about 660 ms, then exit 0 with no row |
| eight `PostToolUse` calls in parallel | all eight rows land, slowest 184 ms |

Baseline `node -e 0` on the same box is about 86 ms cold and about 40 ms warm, so essentially the whole cost is Node start-up and there is no cheaper version of this hook that still uses Node. Against tool calls the model already waits on, and given that hooks for one event run in parallel, this is not a noticeable delay. **`PreToolUse` writes no stdout, so it can never change or block the call whatever it costs.**

---

## 7. Compatibility, security and cost

### 7.1 Compatibility

- **Nothing in the product reads these files.** No Rust, no TypeScript, no test and no script references `ac_rtk_claude_Tools.js` or the row labels. Adding the file cannot break a build.
- **The schema belongs to rtk.** The hook only ever `INSERT`s into `commands` with an explicit column list, so a future rtk migration that **adds** a column keeps working. A migration that renames or drops one of the nine columns makes the insert throw, which is caught, so the failure is a missing row and never a broken tool call. The README and this section are the record of that dependency.
- **Concurrent writers.** The database is in WAL mode. rtk's `rusqlite` waits up to 5000 ms for a busy database, the hook waits 500 ms, and each write is held for about a millisecond. rtk therefore never loses a row to this hook; the hook may lose one to a long external transaction, and drops it silently rather than stalling the agent.
- **Replicas already on disk** keep their current hooks until their next spawn, when the seeder replaces the `.claude` tree wholesale. During the gap they behave exactly as today.
- **`rtk gain` percentages move.** Zero-savings rows lower the reported average. That is section 4.7's intended visibility.
- **The mirror deliberately differs from the seed** on the ignored-log name until the operator applies section 8.5. Section 4.6 is the record.

### 7.2 Security

- **No secrets enter the database.** `original_cmd` carries the tool name plus `file_path`, `notebook_path`, `pattern` and `path` only. File content never reaches a row: `Write.content`, `Edit.old_string`, `Edit.new_string`, `NotebookEdit.new_source` and every `tool_response` body are measured for their **byte length** and then discarded. A `Grep` pattern is a search string the agent typed, which is exactly what rtk already stores for a shell `grep`.
- **No new attack surface.** The hook spawns no process, opens no socket, and reads no file except its own mark. The only path it writes outside the mark directory is `RTK_DB_PATH`, which the operator sets.
- **Path handling.** `tool_use_id` is sanitized to `[A-Za-z0-9_-]` before it becomes a filename, so a crafted id cannot traverse out of the mark directory.
- **No permission widening.** The hook has no `deny` path and no `permissionDecision` output at all. It cannot allow anything the tool call would not otherwise have got, and it cannot block anything.
- **The mark directory is world-writable on a multi-user box**, being under the OS temp dir. The worst a hostile mark achieves is a wrong `exec_time_ms` on one row, which is why `elapsed` rejects a non-finite or future value and falls back to 0.

### 7.3 Dependency-cycle gate

No Rust or TypeScript module arc is added, removed or moved. Nothing under `src-tauri/`, `src/` or `scripts/` is touched, so no SCC can change, no `cyclicSccs` count can move, and no arc can cross a previously clean SCC boundary. No lower-layer module gains an `AppHandle` or `tauri` dependency, because no Rust module is edited at all.

The only module graph in scope is the seeded JavaScript tree. Today it is `ac_rtk_claude_Bash.js -> ac_rtk_shared.js` and `ac_rtk_claude_PowerShell.js -> ac_rtk_shared.js`, two arcs into one sink. Section 4.2 decides that `ac_rtk_claude_Tools.js` imports nothing from `ac_rtk_shared.js`, so **this change adds zero module arcs**: the new file is an isolated node requiring only `node:fs`, `node:os`, `node:path` and, lazily, `node:sqlite`. The graph stays acyclic by construction.

**Binding acceptance criterion, carried into section 9:** `ac_rtk_shared.js` must not require any hook file, and no hook file may require another hook file. A reviewer checks it with `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`, which must show only `node:` specifiers plus the two existing `./ac_rtk_shared.js` lines in the Bash and PowerShell hooks.

---

## 8. Implementation order

### 8.1 Step 1: the hook

Create `docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js` exactly as section 5.1.

Gate before continuing: `node --check` on the file passes, and the section 9.2 suite passes from a scratch copy at your replica root.

### 8.2 Step 2: the registration and the missing seed file

1. Rewrite `settings.local.json` per section 5.2.
2. Copy `statusline.sh` from the seed per section 5.3.

Gate before continuing: `settings.local.json` parses, and criterion 3 of section 9.1 reports the four expected matcher-to-command pairs across the two events.

### 8.3 Step 3: the ignored-log rename

Apply the four edits of sections 5.4 and 5.5. Nothing else in those three files changes.

Gate before continuing: `git diff --stat` shows exactly two changed lines in `ac_rtk_shared.js` and one in each of the other two hooks, and `git grep -n "rtk_ignored_tools" -- docs/integrations/rtk_claude/hooks/` returns nothing.

### 8.4 Step 4: the documentation

Rewrite `README.md` per section 5.6 and touch the one sentence of `docs/integrations/rtk.md` per section 5.7. Do this **after** the hook and the registration are final, so every line count, every quoted snippet and every table row is copied from what actually landed rather than from this plan.

### 8.5 Step 5: the runtime half, which is not ours

The files that take effect live in the workspace seed, outside every repository and outside every agent's write zone. **No agent may write them.** State this verbatim in the pull request description, naming the exact operation:

The operator copies, from the landed branch to the workspace tree, replacing what is there:

```text
docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js       ->  <workspace>/.ac/default.claude/hooks/ac_rtk_claude_Tools.js
docs/integrations/rtk_claude/hooks/ac_rtk_shared.js             ->  <workspace>/.ac/default.claude/hooks/ac_rtk_shared.js
docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js        ->  <workspace>/.ac/default.claude/hooks/ac_rtk_claude_Bash.js
docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js  ->  <workspace>/.ac/default.claude/hooks/ac_rtk_claude_PowerShell.js
docs/integrations/rtk_claude/settings.local.json                ->  <workspace>/.ac/default.claude/settings.local.json
docs/integrations/rtk_claude/README.md                          ->  <workspace>/.ac/default.claude/README.md
```

For this workspace, `<workspace>` is `D:\0_repos\AgentsCommander_iac`. Nothing is deleted from the seed. `statusline.sh` is not copied: the mirror's copy came **from** the seed and is already identical there.

`settings.local.json` hot-reloads, so an already-running session picks up the new registration on its next tool call. The hook **file** must exist before the registration names it, or every matching call silently runs nothing. Copy the hooks first, `settings.local.json` last.

---

## 9. Tests and acceptance criteria

No automated test is added. There is no JavaScript test runner wired to `docs/`, and `package.json:5` is `"type": "module"` while these hooks are CommonJS, so adding one would mean new tooling for six mirror files that no build consumes.

Criteria 1 to 17 are provable from the branch. 18 to 21 require the section 8.5 copies, which the operator applies by hand.

### 9.1 Branch-only, static. Owner: reviewer

| # | Criterion | How |
|---|---|---|
| 1 | `docs/integrations/rtk_claude/` holds `README.md`, `settings.local.json`, `statusline.sh` and `hooks/` with exactly `ac_rtk_claude_Bash.js`, `ac_rtk_claude_PowerShell.js`, `ac_rtk_claude_Tools.js`, `ac_rtk_shared.js`. | `git ls-tree -r --name-only HEAD -- docs/integrations/rtk_claude/` |
| 2 | `settings.local.json` parses and declares three `PreToolUse` entries and one `PostToolUse` entry, with the two tools entries carrying matcher `Read\|Grep\|Glob\|Edit\|Write\|NotebookEdit` and command `node .claude/hooks/ac_rtk_claude_Tools.js`. | `node -e "const h=JSON.parse(require('fs').readFileSync('docs/integrations/rtk_claude/settings.local.json','utf8')).hooks; for (const k of Object.keys(h)) for (const e of h[k]) console.log(k, e.matcher, '->', e.hooks[0].command)"` |
| 3 | The matcher names every tool literally. There is no regular-expression metacharacter in it, because a letters-and-pipes matcher is an exact list (section 3.3). | read the matcher string |
| 4 | None of the four seed placeholders appears anywhere in the directory. | `git grep -n -e '%AC_REPLICA_ROOT%' -e '%AC_WORKSPACE_ROOT%' -e '%AC_MATRIX_ROOT%' -e '%USER_HOME%' -- docs/integrations/rtk_claude/` returns nothing |
| 5 | The JavaScript module graph gains no arc (section 7.3). | `grep -n "require(" docs/integrations/rtk_claude/hooks/*.js`: `ac_rtk_claude_Tools.js` requires only `node:` specifiers, `ac_rtk_shared.js` requires only `node:` specifiers, and no hook requires another hook |
| 6 | `ac_rtk_claude_Tools.js` never writes to stdout on any path, and never touches the ignored log. | `grep -n "stdout\|logIgnored\|ignored" docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js` returns nothing |
| 7 | The hook contains no `CREATE TABLE`, no `ALTER TABLE` and no `DROP`. | `grep -niE "create table\|alter table\|drop " docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js` returns nothing |
| 8 | No file content reaches a row. `content`, `old_string`, `new_string` and `new_source` appear in the hook only inside comments, never as a key it reads into `original_cmd`. | read `originalCmd` |
| 9 | The ignored-log rename is complete inside the directory, and the code edit is one line. | `git grep -n "rtk_ignored_tools" -- docs/integrations/rtk_claude/` returns nothing; `git diff --stat` shows 2 changed lines in `ac_rtk_shared.js`, 1 in `ac_rtk_claude_Bash.js`, 1 in `ac_rtk_claude_PowerShell.js` |
| 10 | The Bash and PowerShell hooks are otherwise untouched. | `git diff docs/integrations/rtk_claude/hooks/ac_rtk_claude_Bash.js docs/integrations/rtk_claude/hooks/ac_rtk_claude_PowerShell.js` shows comment lines only |
| 11 | `statusline.sh` is byte-identical to the seed's copy, and `settings.local.json`'s `statusLine` block matches the seed's. | `diff docs/integrations/rtk_claude/statusline.sh <workspace>/.ac/default.claude/statusline.sh` (read-only against the seed) |
| 12 | The mirror matches the seed byte for byte **except** the decided deltas: the new `ac_rtk_claude_Tools.js`, the new `settings.local.json`, the ignored-log rename in three files, and the README. Nothing else differs. | diff each pair and account for every difference |
| 13 | The README names every mirror file with its real landed line count, shows the section 5.2 registration block including `PostToolUse`, no longer claims the hooks cover the two shell tools and nothing else, no longer says "Neither hook touches the database", documents the `RTK_DB_PATH` dependency, shows `rtk gain -H` output for a tool row, and states that the ignored-log name here is deliberately ahead of the seed. | read the README against section 5.6 |
| 14 | The internal anchor at README line 51 resolves to the rewritten H2. | follow the link |
| 15 | `docs/integrations/rtk.md:91` no longer implies two hooks, and its link to `rtk_claude/README.md` still resolves. | read the line |
| 16 | Nothing outside `docs/integrations/rtk_claude/` and `docs/integrations/rtk.md` is modified. | `git diff --name-only origin/main...HEAD` |
| 17 | `git status --porcelain=v1 --untracked-files=all` is empty after the commit, and `plans/1462-rtk-native-tools-hook.md` is committed (force-added past `.gitignore:11`). | `git status`, `git ls-files plans/` |

### 9.2 Branch-only, behavioural. Owner: any reviewer with `node`, `rtk` and `python3`

The hook **cannot** be run in place (section 3.6). Copy it to a scratch directory at your **replica root**, never inside `.claude/`.

```bash
mkdir -p "$AGENTSCOMMANDER_ROOT/1462-check"
cp docs/integrations/rtk_claude/hooks/ac_rtk_claude_Tools.js "$AGENTSCOMMANDER_ROOT/1462-check/"
cd "$AGENTSCOMMANDER_ROOT/1462-check"
echo '{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"D:/x/a.rs"},"tool_use_id":"toolu_01DEMO","cwd":"'"$PWD"'"}' | node ac_rtk_claude_Tools.js
```

Expected: no output at all, exit 0.

A ready-made harness that automates criteria 18 to 20 below is at `<architect replica root>/1462-proto/check_1462.py`. Copy it beside the hook and run `python check_1462.py`; it prints one `PASS`/`FAIL` line per check and exits nonzero if any fails. It is a convenience, not the specification: the criteria are.

| # | Criterion | Expected |
|---|---|---|
| 18 | A `PreToolUse` then `PostToolUse` pair for each of `Read`, `Grep`, `Glob`, `Edit`, `Write`, `NotebookEdit`, with `RTK_DB_PATH` pointing at a scratch database **rtk created**, yields exactly one row per tool. | six rows, `rtk_cmd` in `tool:read`, `tool:grep`, `tool:glob`, `tool:edit`, `tool:write`, `tool:notebookedit`; `original_cmd` = `Read D:/x/a.rs`, `Grep fn main D:/x`, `Glob **/*.rs`, `Edit D:/x/a.rs`, `Write D:/x/b.rs`, `NotebookEdit D:/x/n.ipynb`; `input_tokens` = `output_tokens` = `ceil((bytes(tool_input) + bytes(tool_response)) / 4)`; `saved_tokens` = 0; `savings_pct` = 0.0; `exec_time_ms` > 0; `timestamp` ends `+00:00`; `project_path` starts `\\?\` on Windows; every mark file consumed |
| 19 | Each of these yields exit 0, empty stdout and **no** row: no `RTK_DB_PATH`; `RTK_DB_PATH` naming a missing file; a file that is not a database; a database with no `commands` table; garbage stdin; empty stdin; a tool not on the list; `hook_event_name` of `PostToolUseFailure`. And the missing file is **not created**. | as stated; the debug-log stderr lines are `file is not a database` and `no such table: commands` |
| 20 | A `PostToolUse` against a database another writer holds in a write transaction gives up inside the busy timeout, adds no row and exits 0. Eight parallel `PostToolUse` calls all land. | busy path under 1500 ms wall; eight rows |
| 21 | No run in the scratch directory writes any ignored-tools log. | `<matrix>/rtk-ignored-tools-claude.md` line count is unchanged across the whole suite |

### 9.3 After the operator applies section 8.5

These cannot be proved from the branch, because the files that take effect are outside every repository.

| # | Criterion | Owner |
|---|---|---|
| 22 | The six files of section 8.5 are in `<workspace>/.ac/default.claude/`. | **the operator** |
| 23 | In a live session (running, since `settings.local.json` hot-reloads, or freshly spawned), one `Read`, one `Grep`, one `Glob`, one `Edit` and one `Write` each add exactly one row to that agent's `RTK_DB_PATH` database, and `rtk gain -H` lists them under **By Command** and in **Recent Commands**. | tech-lead, or `dev-rust` |
| 24 | In the same session, the tool calls behave exactly as before: none is blocked, denied, altered or visibly delayed, and the ignored-tools log gains no line from a native tool. | tech-lead, or `dev-rust` |
| 25 | A `Bash` call and a `PowerShell` call in the same session still route as they do today, and their ignored-log lines land in `rtk-ignored-tools-claude.md`. | tech-lead, or `dev-rust` |

Criterion 23 is the one that proves the issue is closed. Until it is reported, this change is landed but unproven, and the pull request description must say so rather than implying the gap is measured shut.

---

## 10. Non-goals, binding on the implementer

- **No Rust, no TypeScript, no `src-tauri/`, no `src/`, no `scripts/`, no `.github/`, no `.gitattributes`.** The seeder needs no change; it already copies whatever is in the tree.
- **Do not change `rtk`, and do not file an upstream rtk issue as part of this work.**
- **Do not filter, truncate, rewrite or deny any tool call.** Denying `Read` in particular would break `Edit`, which requires a prior `Read`.
- **Do not register `PostToolUseFailure`.** Failed tool calls are out of scope for this issue.
- **Do not create the database or any table.** The schema belongs to rtk; a missing `RTK_DB_PATH` target is a silent no-op.
- **Do not write to `rtk-ignored-tools-claude.md` from the new hook.** That file records shell commands a hook declined to rewrite.
- **Do not add an export to `ac_rtk_shared.js`, and do not make the new hook require it** (section 4.2).
- **Do not touch the repository's own `.claude/settings.json`.** Its inline marker at `:3-9` has its own Bash-only blind spot and is filed separately.
- **Do not write anything under `<workspace>/.ac/default.claude/`.** It is outside every agent's write zone. Section 8.5 is the operator's step.
- **Do not rename the hooks to `.cjs`** and **do not add a `package.json`** to the hooks directory to work around section 3.6. Both change what the seeder installs.
- **Do not add a `timeout` field** to the hook registration (section 5.2).
- **Do not add `--disable-warning` or any other flag** to the registered command (section 3.5).
- **Do not "correct" the mirror's ignored-log name back to `rtk_ignored_tools.md`** to match today's seed (section 4.6).
- **Do not correct `docs/integrations/rtk.md:93`** (section 5.7).
- **Do not add a test runner, a markdown linter or a link checker.** None exists in `.github/workflows/`.
