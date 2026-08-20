// Pi port of the AgentsCommander RTK hooks (Claude Code PreToolUse pair:
// ac_rtk_claude_Bash.js + ac_rtk_claude_PowerShell.js + ac_rtk_shared.js).
//
// pi exposes a single shell tool, `bash` (Git Bash on Windows), so there is no
// separate PowerShell matcher. This extension picks the rule by the shell pi
// will run: the Claude Bash rule by default, the Claude PowerShell rule
// (probe-first, ask the safety question before rtk) when settings.json
// `shellPath` points at pwsh/powershell.
//
// The ignored-tools log is written by this extension (never by the agent's
// own tools) to the origin Agent Matrix root as `rtk-ignored-tools-pi.md`,
// next to the Claude hooks' `rtk-ignored-tools-claude.md` and the RTK
// history DB, so every session of this agent reports into the same place.
//
// Every command routed through rtk feeds the RTK history database
// (`RTK_DB_PATH` of the session); every command handed back untouched is
// appended to the ignored log, so "rtk covered it" and "rtk never saw it" stay
// distinguishable. pi's native `read` tool (not a shell tool, so unwrappable)
// is registered in the same database by running `rtk read` alongside it, so
// file reads are tracked too; `write`/`edit` calls append a `Write:`/`Edit:`
// line with the target path to the ignored-tools log for traceability. rtk invocations go through
// `.pi/hooks/ac-rtk.sh`, which strips the banner at the source so a caller's
// `2>&1` cannot leak it into stdout.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// --- shared (port of ac_rtk_shared.js) ---

// The notice rtk writes to stderr on every filtered invocation; a known false
// positive with no config to turn it off. Only the Bash rule filters it.
const NAG = "No hook installed";

// A command the caller already routed through rtk by hand.
const ALREADY_RTK = /^rtk\s/;

// rtk documents `rtk rewrite` as the single source of truth for hooks: it
// splits on `&&` / `;`, rewrites each segment that has an rtk equivalent and
// leaves the rest alone. Keyed on non-empty stdout, not the exit code.
function rtkRewrite(cmd: string): string {
  const r = spawnSync("rtk", ["rewrite", cmd], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    shell: false,
    timeout: 5000,
  });
  return (r.stdout || "").trim();
}

// Resolved from the replica layout (`__agent_<name>` replica under
// `.ac/wg-<N>-*/` maps to `.ac/_agent_<name>`), no hardcoded absolute path.
const LOG_FILE = (() => {
  const replicaRoot = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
  const agentName = path.basename(replicaRoot).replace(/^__/, "");
  return path.join(replicaRoot, "..", "..", `_${agentName}`, "rtk-ignored-tools-pi.md");
})();

// rtk launcher: strips the banner at rtk's own stderr, so shell-level `2>&1`
// merges cannot leak it (the FILTER below only owns fd2). Falls back to the
// bare binary if the wrapper is missing, so a missing file never breaks a
// command.
const WRAPPER = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "rtk",
  "ac-rtk.sh",
).replace(/\\/g, "/");
const RTK_CMD = fs.existsSync(WRAPPER) ? WRAPPER : "rtk";

// `rtk rewrite` emits one `rtk <sub> ...` per rewritten segment; route every
// segment through the wrapper. Consumes whole separator runs (`&&`, `||`,
// `;`, `|`) so compound syntax survives, and masks quoted regions so `rtk`
// inside a string literal is never rewritten.
const SEGMENT_RTK = /(^|[;&|]+)\s*rtk\s+/g;
const routeRtk = (cmd: string): string => {
  const quoted = new Set<number>();
  let inS = false;
  let inD = false;
  let esc = false;
  for (let i = 0; i < cmd.length; i++) {
    const ch = cmd[i];
    if (esc) {
      esc = false;
      quoted.add(i);
      continue;
    }
    if (ch === "\\" && inD) {
      esc = true;
      quoted.add(i);
      continue;
    }
    if (ch === "'" && !inD) {
      inS = !inS;
      quoted.add(i);
      continue;
    }
    if (ch === '"' && !inS) {
      inD = !inD;
      quoted.add(i);
      continue;
    }
    if (inS || inD) quoted.add(i);
  }
  let out = "";
  let last = 0;
  for (const m of cmd.matchAll(SEGMENT_RTK)) {
    const i = m.index;
    if (i === undefined) break;
    let overlap = false;
    for (let k = i; k < i + m[0].length; k++) {
      if (quoted.has(k)) {
        overlap = true;
        break;
      }
    }
    if (overlap) continue;
    out += cmd.slice(last, i) + m[1] + (m[1] ? " " : "") + RTK_CMD + " ";
    last = i + m[0].length;
  }
  return out + cmd.slice(last);
};

// One `YYYYMMDD_HHMMSS <Tool>: <command>` line per skip, local time. Never
// throws: a locked file must not cost the user the command they asked for.
function logIgnored(tool: string, cmd: string): void {
  try {
    fs.mkdirSync(path.dirname(LOG_FILE), { recursive: true });
    const d = new Date();
    const p = (n: number) => String(n).padStart(2, "0");
    const ts = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
    fs.appendFileSync(LOG_FILE, `${ts} ${tool}: ${cmd.replace(/\s+/g, " ").trim()}\n`, "utf8");
  } catch {
    // best effort
  }
}

// --- Bash rule (port of ac_rtk_claude_Bash.js) ---
// Covers Git Bash, the shell pi's bash tool runs on Windows.

// Silences the rtk "No hook installed" notice. Prepend as its own statement so
// heredocs keep working; exit codes and the rest of stderr are preserved.
const FILTER = `exec 2> >(grep --line-buffered -v '${NAG}' >&2)\n`;

// True when the command is a single plain invocation whose head the shell does
// not resolve to a builtin or keyword. Asked of the shell (`type -t`), never a
// kept list, so the two cannot drift.
function prefixable(cmd: string): boolean {
  if (/[\n;|&<>(){}`]/.test(cmd)) return false;
  const head = cmd.split(/\s+/)[0];
  if (!head || head.includes("=")) return false; // FOO=bar cmd
  const t = spawnSync("bash", ["-c", 'type -t -- "$1"', "bash", head], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    shell: false,
    timeout: 5000,
  });
  return !["builtin", "keyword", "function", "alias"].includes((t.stdout || "").trim());
}

function decideBash(body: string): string | null {
  if (ALREADY_RTK.test(body)) return FILTER + body.replace(/^rtk\s/, RTK_CMD + " ");
  const rewritten = rtkRewrite(body);
  if (rewritten) return FILTER + routeRtk(rewritten);
  if (prefixable(body)) return FILTER + RTK_CMD + " " + body; // unknown binary: wrap so rtk still reports stats
  return null; // builtins / shell syntax: leave alone, but on the record
}

// pi's native `read` tool is not a shell tool, so rtk cannot wrap it. Register
// the usage by running `rtk read` alongside: best effort, never blocks the
// read, never emits output. pi's own read result is untouched.
function registerRead(filePath: string): void {
  if (!filePath) return;
  try {
    const p = spawn("rtk", ["read", filePath], { stdio: "ignore", shell: false });
    p.unref();
  } catch {
    // best effort
  }
}

// --- PowerShell rule (port of ac_rtk_claude_PowerShell.js) ---
// Used only when pi's bash tool runs pwsh via settings `shellPath`.

// The safety question is asked of the PowerShell parser and of `Get-Command`,
// never of a character class kept here. The verdict is fenced with a sentinel
// because the probe loads the user profile, which prints banners.
const PROBE = [
  "$ErrorActionPreference = 'SilentlyContinue'",
  "$t = $null; $e = $null",
  "$ast = [System.Management.Automation.Language.Parser]::ParseInput($env:AC_RTK_CMD, [ref]$t, [ref]$e)",
  "if ($e.Count) { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "$st = $ast.EndBlock.Statements",
  "if ($st.Count -ne 1) { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "$p = $st[0] -as [System.Management.Automation.Language.PipelineAst]",
  "if (-not $p) { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "$c0 = $p.PipelineElements[0] -as [System.Management.Automation.Language.CommandAst]",
  "if (-not $c0) { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "if ($c0.InvocationOperator -ne 'Unknown') { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "$n = $c0.GetCommandName()",
  "if (-not $n) { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "if ($n -like '*=*') { 'AC_RTK_VERDICT:SHELL'; exit 0 }",
  "$g = Get-Command -Name $n -ErrorAction SilentlyContinue | Select-Object -First 1",
  "if (-not $g) { 'AC_RTK_VERDICT:APP'; exit 0 }",
  "if ($g.CommandType -eq 'Application') { 'AC_RTK_VERDICT:APP' } else { 'AC_RTK_VERDICT:SHELL' }",
].join("\n");

// True when the command is a single statement, that statement is a single
// pipeline, the pipeline's first element is a plain command invocation with no
// call operator, its name is a bare name containing no `=`, and that name
// resolves to `Application` or does not resolve at all. Anything that goes
// wrong reads as false: fail closed, the cost is one statistic.
function headIsExternal(cmd: string): boolean {
  const r = spawnSync("pwsh", ["-NonInteractive", "-NoLogo", "-Command", PROBE], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    shell: false,
    timeout: 5000,
    env: { ...process.env, AC_RTK_CMD: cmd },
  });
  const marked = (r.stdout || "").split(/\r?\n/).filter((l) => l.startsWith("AC_RTK_VERDICT:"));
  return marked.length > 0 && marked[marked.length - 1].trim() === "AC_RTK_VERDICT:APP";
}

// The order is inverted vs the Bash rule on purpose: under PowerShell the
// rewrite path is the dangerous one (`ls` is an alias for Get-ChildItem, so a
// rewrite swaps an object stream for a text stream). Nothing is prepended, so
// `$?`, `$LASTEXITCODE` and redirected bytes behave natively.
function decidePwsh(body: string): string | null {
  if (ALREADY_RTK.test(body)) return body;
  if (!headIsExternal(body)) return null;
  const rewritten = rtkRewrite(body);
  if (rewritten) return rewritten;
  return "rtk " + body;
}

// --- which rule applies ---

// project `.pi/settings.json` then global `~/.pi/agent/settings.json`; the
// winner is the last `shellPath` found that names a shell. Default: bash.
function configuredShell(): "bash" | "powershell" {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const candidates = [
    path.join(here, "..", "..", ".pi", "settings.json"),
    path.join(os.homedir(), ".pi", "agent", "settings.json"),
  ];
  let shell: "bash" | "powershell" = "bash";
  for (const f of candidates) {
    try {
      const s = JSON.parse(fs.readFileSync(f, "utf8"));
      if (typeof s.shellPath === "string" && /pwsh|powershell/i.test(s.shellPath)) shell = "powershell";
      else if (typeof s.shellPath === "string" && /bash|sh\.exe/i.test(s.shellPath)) shell = "bash";
    } catch {
      // missing or unreadable: try the next candidate
    }
  }
  return shell;
}

const RULE = configuredShell() === "powershell"
  ? { decide: decidePwsh, tool: "PowerShell" }
  : { decide: decideBash, tool: "Bash" };

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", (event) => {
    if (event.toolName === "read") {
      const p = (event.input as { path?: unknown }).path;
      registerRead(typeof p === "string" ? p : "");
      return;
    }
    if (event.toolName === "write" || event.toolName === "edit") {
      const p = (event.input as { path?: unknown }).path;
      if (typeof p === "string" && p) {
        logIgnored(event.toolName === "write" ? "Write" : "Edit", p);
      }
      return;
    }
    if (event.toolName !== "bash") return;
    const orig = typeof event.input.command === "string" ? event.input.command : "";
    const body = orig.replace(/^\s+/, "");
    if (!body) return;

    const final = RULE.decide(body);
    if (final === null) {
      logIgnored(RULE.tool, body); // handed back untouched: on the record
      return;
    }
    event.input.command = final; // the mutation pi applies before execution
  });
}
