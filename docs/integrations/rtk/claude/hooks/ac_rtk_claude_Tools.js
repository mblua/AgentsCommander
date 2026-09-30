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
