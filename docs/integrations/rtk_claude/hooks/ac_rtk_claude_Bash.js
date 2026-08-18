// PreToolUse/Bash hook: routes the command through rtk so its output is compacted.
//
// The rewrite decision is delegated to `rtk rewrite`, which rtk documents as the
// single source of truth for hooks. It splits on `&&` / `;`, rewrites each segment
// that has an rtk equivalent and leaves the rest alone -- notably shell builtins
// like `cd` and `export`, which a blind "rtk " prefix would break (rtk tries to
// resolve them via PATH and exits 127).
//
// We key on non-empty stdout rather than the exit code: `rtk rewrite "cd /tmp && ls"`
// returns a perfectly good rewrite with exit 3, so rtk's own documented idiom
// (`REWRITTEN=$(rtk rewrite "$CMD") || exit 0`) would discard compound commands.
//
// Empty stdout from `rtk rewrite` means two different things: the command has no rtk
// equivalent (`hola`, `node --version`) *and* the command is pure shell builtins
// (`cd /tmp && export X=1`, `echo hola`). We still want stats on the first group, so we
// fall back to prefixing `rtk `, but only when the command is a single plain invocation
// whose head the shell does not resolve to a builtin or keyword. Prefixing those is the
// old bug (`rtk cd x && ls` -> exit 127, "Binary 'cd' not found on PATH").
//
// When a rewrite happens we also silence the "No hook installed" notice rtk writes
// to stderr on every filtered invocation. It is a known false positive (rtk only
// looks for its hook in the global settings) with no config to turn it off. The
// filter is prepended as its own `exec` statement instead of wrapping the command,
// so heredocs keep working; exit codes and the rest of stderr are preserved.
//
// Commands we hand back untouched leave no trace anywhere, which makes it impossible to
// tell "rtk covered it" from "rtk never saw it". So every skip is appended to
// `rtk_ignored_tools.md` in the origin Agent Matrix before we bow out.

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const NAG = "No hook installed";
const FILTER = `exec 2> >(grep --line-buffered -v '${NAG}' >&2)\n`;

// The log lives with the canonical agent state, not with the throwaway replica, so it
// survives across workgroups. Everything is derived, nothing hardcoded: this file sits
// at <replica root>/.claude/hooks/, so two levels up is the replica root and two more
// is `.ac`. From there the origin Matrix folder is the replica's own name with one
// leading underscore dropped (`__agent_foo` -> `_agent_foo`).
function ignoredLogPath() {
  const replicaRoot = path.resolve(__dirname, "..", "..");
  const replicaName = path.basename(replicaRoot);
  if (!replicaName.startsWith("__")) return null; // not a WG replica: no Matrix above us
  return path.join(path.resolve(replicaRoot, "..", ".."), replicaName.slice(1), "rtk_ignored_tools.md");
}

// One `YYYYMMDD_HHMMSS: <command>` line per skip, local time. Newlines are folded to
// spaces so a heredoc stays a single entry. Never touches stdout -- the hook protocol
// reads it -- and never throws: a missing Matrix or a locked file must not cost the
// user the command they asked for.
function logIgnored(cmd) {
  try {
    const target = ignoredLogPath();
    if (!target) return;
    const d = new Date();
    const p = (n) => String(n).padStart(2, "0");
    const ts = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
    fs.appendFileSync(target, `${ts}: ${cmd.replace(/\s+/g, " ").trim()}\n`, "utf8");
  } catch {
    // best effort
  }
}

// Only a bare `cmd arg arg` line is safe to prefix. Anything with shell syntax --
// operators, redirects, subshells, heredocs, command substitution, newlines -- would
// either attach the prefix to the wrong segment or change what the shell does with it.
// Whether the head is something rtk can exec is asked of the shell (`type -t`) rather
// than kept as a builtin list here, so there is no second copy to drift: builtin /
// keyword / function / alias stay untouched, `file` and not-found get wrapped.
function prefixable(cmd) {
  if (/[\n;|&<>(){}`]/.test(cmd)) return false; // `(` and backtick already cover $( )
  const head = cmd.split(/\s+/)[0];
  if (!head || head.includes("=")) return false; // FOO=bar cmd
  const t = spawnSync("bash", ["-c", 'type -t -- "$1"', "bash", head], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
    shell: false,
  });
  return !["builtin", "keyword", "function", "alias"].includes((t.stdout || "").trim());
}

const chunks = [];
process.stdin.on("data", (c) => chunks.push(c));
process.stdin.on("end", () => {
  let data;
  try {
    data = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  } catch {
    process.exit(0); // unreadable input: leave the command untouched
  }

  const ti = { ...(data.tool_input || {}) };
  const orig = typeof ti.command === "string" ? ti.command : "";
  const body = orig.replace(/^\s+/, "");
  if (!body) process.exit(0);

  // Already routed through rtk (e.g. a command we wrote by hand): only silence the notice.
  let rewritten;
  if (/^rtk\s/.test(body)) {
    rewritten = body;
  } else {
    // Passed as a single argv entry, so no shell re-parsing happens here.
    const r = spawnSync("rtk", ["rewrite", body], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      shell: false,
    });
    rewritten = (r.stdout || "").trim();
    if (!rewritten) {
      if (!prefixable(body)) {
        logIgnored(body); // builtins / shell syntax: leave it alone, but on the record
        process.exit(0);
      }
      rewritten = "rtk " + body; // unknown binary: wrap it so rtk still reports stats
    }
  }

  ti.command = FILTER + rewritten;

  process.stdout.write(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "allow",
        permissionDecisionReason: "ac_rtk_claude",
        updatedInput: ti,
      },
    })
  );
});
