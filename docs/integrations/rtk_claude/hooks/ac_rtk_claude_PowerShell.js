// PreToolUse/PowerShell hook: routes the command through rtk so its output is compacted.
//
// This hook covers the `PowerShell` tool, PowerShell 7 or newer, on any operating
// system. `ac_rtk_claude_Bash.js` is the sibling that covers the other shell tool,
// and `ac_rtk_shared.js` holds the half that does not depend on a shell.
//
// It asks the safety question before it asks rtk, which is the opposite of the
// Bash hook's order, because under PowerShell the rewrite path is the dangerous
// one. `rtk rewrite` decides on the head of each segment and does not know which
// shell will run the result, so it turns `ls` into `rtk ls`. Under bash that is
// close to behaviour-preserving. Under PowerShell `ls` is an alias for
// `Get-ChildItem`, so the rewrite swaps an object stream for a text stream: a tail
// such as `| Where-Object { $_.Name }` then matches nothing and the pipeline
// returns empty with exit 0. `rtk ls` does not even run on Windows outside Git
// Bash. So anything the probe does not clear goes to the ignored log: a missed
// rewrite costs one statistic, a wrong rewrite silently breaks the command.
//
// This hook prepends nothing to the routed command, and that is a decision rather
// than an omission. It means rtk's "No hook installed" notice is not filtered and
// appears on stderr once per routed command. PowerShell has no statement that
// reassigns the rest of a script's error stream, so every construct that filters
// the notice has to turn rtk from an external binary into a shell function.
// Measured, that wrapper leaves `$?` reporting True after a failing command, drops
// every record from a `2>&1` capture, defeats `2>` and `2>$null`, rewrites every
// LF to CRLF in redirected stdout and corrupts redirected binary output beyond
// recovery, and splits an unquoted comma in an argument into two argv entries.
// Four broken channels, three of them silent and one of them destroying files,
// against one cosmetic stderr line. The notice is not worth it, so a routed
// command here behaves natively in every channel.
//
// The probe spawns `pwsh`, and the hook cannot verify that the harness's
// `PowerShell` tool is the same PowerShell. Where the tool runs Windows PowerShell
// 5.1, `pwsh` is either absent, in which case every command fails closed to the
// ignored log, or present and answering for a different command table than the one
// that will run.
//
// Clearing the probe does not make a routed command correct, only unambiguous to
// PowerShell. Two classes still break silently and neither has an in-hook fix.
// Names Windows owns with different semantics from the POSIX tool rtk assumes
// resolve as `Application` and pass the gate: `tree` loses its listing at exit 0
// and `find "x" f.js` loses its matches at exit 0. And any pipeline tail that
// parses the head's text breaks when rtk reformats that text, whatever the head
// is: `git status | Select-String "nothing added to commit"` returns zero results
// where the command the agent typed returns one. Both classes exist today under
// bash too; this hook neither creates nor widens them.
//
// Commands we hand back untouched leave no trace anywhere, which makes it impossible
// to tell "rtk covered it" from "rtk never saw it". So every skip is appended to
// `rtk-ignored-tools-claude.md` in the origin Agent Matrix before we bow out.

const { spawnSync } = require("node:child_process");
const { ALREADY_RTK, rtkRewrite, runHook } = require("./ac_rtk_shared.js");

// The safety question is asked of the PowerShell parser and of `Get-Command`,
// never of a character class or a keyword list kept here, so there is no second
// copy of the shell's syntax to drift. This is the same principle the Bash hook
// states for `type -t`, expressed in the other shell.
//
// The verdict is fenced with a sentinel because the probe loads the user profile,
// and `oh-my-posh`, `starship`, `conda init` and corporate banners all print on
// profile load. Comparing the whole buffer against the verdict would fail for
// every command, sending every PowerShell call to the ignored log with nothing
// reporting why.
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
// call operator, its command name is a bare name containing no `=`, and that name
// either resolves to `CommandType = Application` or does not resolve at all.
//
// The command travels in the environment rather than on the command line, so there
// is no quoting boundary to get wrong. The profile is loaded on purpose: the probe
// must answer for the command table the real session will use, and a profile that
// defines a function or an alias would otherwise be invisible to it.
//
// Anything that goes wrong here -- `pwsh` absent, the probe killed by its
// `timeout`, no verdict line in the output -- reads as false and the command is
// handed back untouched and logged. Fail closed: the cost is one statistic, and the
// alternative is rewriting a command whose shape was never verified.
function headIsExternal(cmd) {
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

// Once the probe clears the command, accepting `rtk rewrite`'s output is safe
// against the hazard above: only one segment exists, so rtk can only replace the
// one head the probe just cleared, and rtk never touches an element after a pipe.
// When rtk declines, the head was still an external binary, so prefixing is what
// puts an unknown binary into the statistics.
function decide(body) {
  if (ALREADY_RTK.test(body)) return body; // already routed: nothing to do
  if (!headIsExternal(body)) return null; // PowerShell owns the head, or the shape is not a plain pipeline
  const rewritten = rtkRewrite(body);
  if (rewritten) return rewritten;
  return "rtk " + body;
}

runHook(__dirname, "PowerShell", decide);
