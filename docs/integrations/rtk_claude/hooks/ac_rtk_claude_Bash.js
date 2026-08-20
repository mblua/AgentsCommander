// PreToolUse/Bash hook: routes the command through rtk so its output is compacted.
//
// This hook covers the `Bash` tool, which means Windows under Git Bash, Linux and
// macOS. Those are the three environments its shell probe and its character class
// are written for; no other shell reaches this file.
//
// `ac_rtk_claude_PowerShell.js` is the sibling that covers the other shell tool,
// and `ac_rtk_shared.js` holds the half that does not depend on a shell.
//
// This hook filters rtk's "No hook installed" notice off stderr. The PowerShell
// hook deliberately does not, and that asymmetry is a decision rather than an
// oversight: PowerShell has no statement that reassigns the rest of a script's
// error stream, and every construct that emulates one turns rtk from an external
// binary into a shell function, which breaks `$?`, stderr redirection, the bytes
// of redirected stdout and comma-bearing arguments. That file's header carries the
// measurements.
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
// `rtk-ignored-tools-claude.md` in the origin Agent Matrix before we bow out.

const { spawnSync } = require("node:child_process");
const { NAG, ALREADY_RTK, rtkRewrite, runHook } = require("./ac_rtk_shared.js");

const FILTER = `exec 2> >(grep --line-buffered -v '${NAG}' >&2)\n`;

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
    timeout: 5000,
  });
  return !["builtin", "keyword", "function", "alias"].includes((t.stdout || "").trim());
}

// Ask rtk first, and only ask the shell when rtk declined. Under bash the rewrite
// path is the safe one: every head `rtk rewrite` replaces is a real binary emitting
// text, so the substitution is close to behaviour-preserving whatever the shape of
// the command. The PowerShell hook inverts this order, for the reason its header
// gives.
function decide(body) {
  // Already routed through rtk (e.g. a command we wrote by hand): only silence the notice.
  if (ALREADY_RTK.test(body)) return FILTER + body;
  const rewritten = rtkRewrite(body);
  if (rewritten) return FILTER + rewritten;
  if (prefixable(body)) return FILTER + "rtk " + body; // unknown binary: wrap it so rtk still reports stats
  return null; // builtins / shell syntax: leave it alone, but on the record
}

runHook(__dirname, "Bash", decide);
