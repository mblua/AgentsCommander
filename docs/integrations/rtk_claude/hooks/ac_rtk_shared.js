// The shell-independent half of the AgentsCommander RTK hooks, required by both
// `ac_rtk_claude_Bash.js` and `ac_rtk_claude_PowerShell.js`.
//
// Nothing shell-specific belongs here. Every question whose answer depends on how
// a particular shell parses a command lives in that shell's hook, so the two
// hooks never have to agree on anything except this module's contract.
//
// It reaches a replica the same way the hooks do. The seeder walks the source
// tree and copies every regular file in every subdirectory with no filter on name
// or extension (`src-tauri/src/config/config_seed.rs:1155-1201`), so a third file
// placed beside the hooks is installed with them, and `require("./ac_rtk_shared.js")`
// resolves from either one because Node resolves a relative `require` against the
// requiring module's own directory.

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

// The notice rtk writes to stderr on every filtered invocation. It is a known
// false positive -- rtk only looks for its hook in the global settings -- and no
// environment variable turns it off. Only `ac_rtk_claude_Bash.js` uses this, to
// build its stderr filter; the PowerShell hook deliberately filters nothing and
// its header says why. The string lives here because the notice is a property of
// rtk, not of a shell.
const NAG = "No hook installed";

// A command the caller already routed through rtk by hand.
const ALREADY_RTK = /^rtk\s/;

// Ask rtk what the command should become. rtk documents `rtk rewrite` as the
// single source of truth for hooks: it splits on `&&` / `;`, rewrites each
// segment that has an rtk equivalent and leaves the rest alone.
//
// The command goes in as a single argv entry with `shell: false`, so nothing
// re-parses it on the way. The `timeout` is required rather than optional: an rtk
// that hangs would otherwise hang the hook and with it the tool call, with
// nothing in any log. A killed call yields empty stdout, which both routing rules
// already handle.
function rtkRewrite(cmd) {
  const r = spawnSync("rtk", ["rewrite", cmd], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    shell: false,
    timeout: 5000,
  });
  return (r.stdout || "").trim();
}

// The log lives with the canonical agent state, not with the throwaway replica, so it
// survives across workgroups. Everything is derived, nothing hardcoded: a hook sits
// at <replica root>/.claude/hooks/, so two levels up from `hookDir` is the replica
// root and two more is `.ac`. From there the origin Matrix folder is the replica's
// own name with one leading underscore dropped (`__agent_foo` -> `_agent_foo`).
function ignoredLogPath(hookDir) {
  const replicaRoot = path.resolve(hookDir, "..", "..");
  const replicaName = path.basename(replicaRoot);
  if (!replicaName.startsWith("__")) return null; // not a Room replica: no Matrix above us
  return path.join(path.resolve(replicaRoot, "..", ".."), replicaName.slice(1), "rtk-ignored-tools-claude.md");
}

// One `YYYYMMDD_HHMMSS <Tool>: <command>` line per skip, local time. The tool field
// comes from the calling hook's own constant, not from the payload, because the
// payload may carry no `tool_name` at all and the line must still say which hook
// wrote it. Newlines are folded to spaces so a heredoc stays a single entry. Never
// touches stdout -- the hook protocol reads it -- and never throws: a missing Matrix
// or a locked file must not cost the user the command they asked for.
function logIgnored(hookDir, tool, cmd) {
  try {
    const target = ignoredLogPath(hookDir);
    if (!target) return;
    const d = new Date();
    const p = (n) => String(n).padStart(2, "0");
    const ts = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
    fs.appendFileSync(target, `${ts} ${tool}: ${cmd.replace(/\s+/g, " ").trim()}\n`, "utf8");
  } catch {
    // best effort
  }
}

// The hook protocol, shared by both hooks: read the payload, hand the command to
// the shell's own `decide`, and answer.
//
// `decide(body)` returns the complete replacement command, or `null` for "hand it
// back untouched and put it on the record". It never writes to stdout and never
// throws. Everything that decides which of those two happens is the shell's
// business and lives in the calling hook.
function runHook(hookDir, tool, decide) {
  const chunks = [];
  process.stdin.on("data", (c) => chunks.push(c));
  process.stdin.on("end", () => {
    let data;
    try {
      data = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    } catch {
      process.exit(0); // unreadable input: leave the command untouched
    }

    // Spread rather than rebuild, so every other field the tool carries
    // (`description`, `timeout`, `run_in_background`, ...) survives untouched.
    const ti = { ...(data.tool_input || {}) };
    const orig = typeof ti.command === "string" ? ti.command : "";
    const body = orig.replace(/^\s+/, "");
    if (!body) process.exit(0);

    const final = decide(body);
    if (final === null) {
      // Commands we hand back untouched leave no trace anywhere, which makes it
      // impossible to tell "rtk covered it" from "rtk never saw it". So every skip
      // is appended to `rtk-ignored-tools-claude.md` before we bow out.
      logIgnored(hookDir, tool, body);
      process.exit(0);
    }

    ti.command = final;

    process.stdout.write(
      JSON.stringify({
        hookSpecificOutput: {
          hookEventName: "PreToolUse",
          permissionDecision: "allow",
          permissionDecisionReason: `ac_rtk_claude_${tool}`,
          updatedInput: ti,
        },
      })
    );
  });
}

module.exports = { NAG, ALREADY_RTK, rtkRewrite, ignoredLogPath, logIgnored, runHook };
