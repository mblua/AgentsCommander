# Plan #1596: Route AgentsCommander CLI invocations through Git Bash and make `send` fail-loud with an enqueue receipt (silent message loss via PowerShell stdout capture)

Author: ac-architect-v3, workgroup wg-19-ac-dev-team-v3. Sole editor of this plan per round-1 consensus dispatch (`20260827-151844-wg19-ac-tech-lead-v3-to-wg19-ac-architect-v3-issue1596-cli-stdout-plan-r1.md`), amended by the owner directive (`20260827-152012-wg19-ac-tech-lead-v3-to-wg19-ac-architect-v3-issue1596-owner-directive-git-bash.md`) which reorders the design space: **Git Bash routing is PRIMARY**, the console-subsystem shim is demoted to a fallback that is analyzed and rejected below, and `send` fail-loud with a verifiable receipt is the shell-independent defense in depth.

Status: READY_FOR_IMPLEMENTATION

Revision: round 1 (2026-08-27 UTC). Plan-SHA256 of the certified bytes is reported to the tech-lead in the reply message, never embedded here (a file cannot contain its own hash; see `plans/1446-...` §Certification conventions).

Issue: [mblua/AgentsCommander#1596](https://github.com/mblua/AgentsCommander/issues/1596), "CLI stdout no capturado en PowerShell → pérdida silenciosa de mensajes" (OPEN, created by ac-tech-lead-v3).

Objective: eliminate the silent message-loss failure where an agent writes a `messaging/` file, `list-peers-lean` returns empty under PowerShell direct capture of the GUI-subsystem release binary, a guard aborts before `send` ever runs, and the agent still reports "enviado". Primary fix: make the harness route AC CLI invocations through Git Bash (`C:\Program Files\Git\bin\bash.exe`), whose console-subsystem stdout is capturable from any shell, and make the send recipe bash-native with mandatory receipt verification. Complementary (shell-independent): `send` prints a machine-readable `Queued: <message-id>` enqueue receipt that the role flow MUST verify before reporting success. The GUI binary behavior is unchanged; no new binary is introduced (shim rejected, §5.3).

---

## 1. Frozen authority and entry gate

Working tree: `repo-AgentsCommander`, branch `fix/1596-cli-stdout-powershell-capture` (created by the tech-lead from `main`). At authoring time (2026-08-27 UTC):

- `git status --porcelain` is empty; local `HEAD` == `origin/main` == `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e` (verified by `git log origin/main -1`).
- Codebase Memory gate: `ready` (project `D-0_repos-AgentsCommander_iac-.ac-wg-19-ac-dev-team-v3-repo-AgentsCommander`, 25133 nodes / 135247 edges, index at head `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e`). Graph operations used: `name` (attach_parent_console, spawn_record, spawn), `text` (bash.exe, spawn-record, windows_subsystem), `trace` (attach_parent_console). Direct reads were used for the exact bytes of the small anchor files (main.rs, Cargo.toml, send.rs, session_context.rs, config_seed.rs, cli_powershell_capture.rs, smoke scripts, docs, workflows) — the fallback is limited to exact-text anchors.

Root `.gitignore` line 11 ignores `/plans/`, so the implementer MUST force-add this plan file: `git add -f plans/1596-cli-stdout-powershell-capture.md`. Do not remove or weaken the ignore rule.

The implementers must repeat the authority ritual: fetch `origin/main` and stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. If a quoted line number no longer matches the quoted text, re-anchor on the text, never on the number. Target-branch drift after this round that does not touch the selected files (§6) does not reopen the design; it is recorded at the next bounded gate (skill: delivery-nonfunctional-invariants §Bounded target-branch drift).

## 2. Task class and threat model

Routine application change: one Rust CLI output line, one Rust context-template text change, one small Windows-only config-seed step, docs, and Windows test/CI coverage. No release, no signing, no packaging, no security-boundary change, no migration. Baseline gates apply; **no enhanced controls are applicable** (no hostile-host threat model is claimed; host executables are trusted per the repository contract — GitHub CI is the authoritative host evidence, §9.1).

## 3. Verified evidence (re-verified at `ecc6527`, not predicted)

### 3.1 The binary and its stdout path (confirms wg-17 / tech-lead Step 1)

1. `src-tauri/src/main.rs:1`: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` — the release binary is GUI-subsystem (PE SUBSYSTEM=2).
2. `src-tauri/Cargo.toml`: single package `agentscommander-new`, no `[[bin]]` sections, no console-subsystem shim. The deployed `agentscommander_ac2.exe` is a renamed copy of this one binary.
3. `src-tauri/src/cli/mod.rs:247-270` (`attach_parent_console`, Windows arm): attaches a console ONLY when both stdout and stderr handles are null/`FILE_TYPE_UNKNOWN`; valid inherited handles (pipes) are left untouched. This is the #129 fix. It cannot help the PS 5.1 interactive direct-capture case, where the parent gives the GUI-subsystem child no routable stdout to inherit (see §5.2).
4. All CLI output goes through `println!`/`cli_println!` (stdout) and `eprintln!` (stderr) on the CRT standard handles; there is no `WriteConsole`/`CONOUT$` path in `src-tauri/src`.
5. `src-tauri/tests/cli_powershell_capture.rs` (issue #129, `#[ignore]`, release-only) documents the sibling fact: PS-NonInteractive bare `&` does NOT propagate `$LASTEXITCODE` for GUI-subsystem children; the outer powershell.exe always exits 0. CI covers via `npm run smoke:cli-release-windows` → `scripts/smoke-cli-release-windows.ps1` → `scripts/smoke-cli-powershell.ps1`.
6. wg-17 telemetry (cited by the tech-lead, not re-measured here): PowerShell `$x = & $bin list-peers-lean ...; $null -eq $x` → True (empty); Git Bash `peers=$("$BIN" list-peers-lean ... 2>/dev/null)` → 10071 bytes, 29 peers parsed, FQN resolved. `2>&1 | Out-String` in PS recovers the JSON (10350 chars) — proving the JSON is emitted and only the PS 5.1 direct-capture channel drops it.

### 3.2 The harness shell mechanism (owner-directive item 1, verified in code/config)

The shell a coding agent uses for ITS tool-calls is fixed per runner:

- **pi**: the `bash` tool executes through `shellPath` from `.pi/settings.json` (project) merged over `~/.pi/agent/settings.json` (global); per the installed pi docs (`docs/settings.md`, "Project Overrides"): **project settings override global settings**. The AC RTK pi extension (`docs/integrations/rtk_pi/extensions/tool-hook.ts:245-263`, `configuredShell`) selects the log rule from the same two files and emits the `Bash:`/`PowerShell:` tool field — a `PowerShell:` line in `rtk-ignored-tools-pi.md` means the effective `shellPath` matched `/pwsh|powershell/i`, i.e. the agent's bash tool was running pwsh. AC seeds `<replica>/.pi/` (extensions + rtk launcher) at spawn from the workspace template via the generic config-seed machinery (`src-tauri/src/config/config_seed.rs`, `ConfigSeedConfig.dest`); the template folders (`<workspace>/.ac/default.pi/`) live OUTSIDE this repo. No repo code writes `.pi/settings.json` today (verified: no `shellPath`/`default.pi` hits under `src-tauri/src` outside the generic seed).
- **Claude Code**: exposes BOTH a `Bash` and a `PowerShell` tool; the AC RTK hooks register both matchers (`docs/integrations/rtk_claude/README.md`). The model chooses the tool; there is no settings switch to force Bash-only. The `PowerShell:`-prefixed lines in the Claude Code ignored-tools log are calls made through the PowerShell tool.
- **codex**: single Bash tool; no repo-side shell control point.

Conclusion: there is **no single harness switch** for all runners, but there IS a runner-independent lever — **wrap the AC CLI invocation in Git Bash** (`bash.exe` is console-subsystem; PowerShell waits for it, captures its stdout, and propagates its exit code). For pi there is an additional lever: a project-level `.pi/settings.json` `shellPath` (project overrides global) makes the whole bash tool run Git Bash.

### 3.3 The recipe location (owner-directive item 2, verified)

- The "## Inter-Agent Messaging / ### Send a message to another agent" recipe that lands in every agent's materialized CLAUDE.md/AGENTS.md is rendered by `src-tauri/src/config/session_context.rs`:
  - `render_inter_agent_messaging_block` (`:3436-3467`) — the common block ("Before every send, run `list-peers-lean` ...", "### List available peers");
  - `send_message_instructions` (`:3611-3641`) — the Workgroup and Root two-step `--send` arms;
  - tests asserting the recipe text and the profile byte budget (`:5255-5280` `default_context_embeds_filename_only_warning`, `:10289-10342` v3 byte-budget assertions: `full_wg.len() <= 8_313`, reduction `>= 757`).
- `docs/agents/inter-agent-messaging.md` (operator-facing) is already bash-native (`agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" ...`) and needs only the Windows-routing + receipt rules added.
- The harness role templates (tech-lead/dev `Role.md` in `.ac/_agent_*/`) are OUTSIDE this repo (Agent Matrix, user-owned harness state). They are handled as a handoff checklist (§7), not as repo edits. (Per the Golden Rule, their current contents were not read; the required replacement is fully specified below.)

### 3.4 `send` behavior today (owner-directive item 3, verified)

`src-tauri/src/cli/send.rs`:
- File path (`--send`/`--command`): validates routing BEFORE writing; writes `<msg_id>.json` to the outbox at `:1107` (`std::fs::write(&outbox_path, json)`); logs `[send] queued message` (`:1112`, log only — NO stdout line); then polls `delivered/`/`rejected/` artifacts and prints `Delivered: <msg_id> (mode=..., to=...)` on confirmation (`wait_for_delivery_confirmation`), or exits 1 on rejection/timeout.
- The pty-input path already prints `Operation ID:`/`Queued:` (`:682`) — the file path has no equivalent enqueue receipt. Exit codes: 0 success, 1 any error. In PS 5.1 direct capture neither stdout nor the exit code of the GUI child is observable — the only reliable signal for an agent is a receipt line on stdout captured through a console-subsystem carrier (bash.exe).

### 3.5 CI and deployment surface (constraint check, verified)

- `.github/workflows/pr-regression-gates.yml` jobs: `test-debt`, `rust-regression` (windows-latest: `cargo check`, `cargo clippy`, `cargo test --lib --bins --tests`), `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable`, `windows-release-cli-smoke` (windows-latest: `npm run build:prod:no-bundle` then `npm run smoke:cli-release-windows`), `frontend-regression`. Git Bash is preinstalled on GitHub `windows-latest` runners.
- Smoke binaries: `target/release/agentscommander.exe` + `agentscommander_testeable.exe` (copied by `scripts/copy-testable-binary.mjs`); `package.json` `smoke:cli-release-windows`. `smoke-cli-powershell.ps1` runs cases per (binary, shell) with `powershell.exe` + `pwsh.exe` and aggregates failures into `$failed`/exit code; the wrapper `smoke-cli-release-windows.ps1` only reads the helper's `$LASTEXITCODE`.
- Because the shim is rejected (§5.3), no binary names, deployment artifacts, or `0_AC` invocation paths change; only invocation shapes and templates are extended.

## 4. Root cause (single statement)

Agents run AC CLI discovery (`list-peers-lean`) through a PowerShell shell (pi `shellPath`=pwsh and/or Claude's PowerShell tool). The release AC binary is GUI-subsystem: PowerShell 5.1 direct capture yields empty stdout and no `$LASTEXITCODE`. The agent's guard (`$null -eq $peer → throw`) fires after the message file is written but before `send` enqueues it; nothing downstream verifies an enqueue receipt, so the agent reports success. The binary emits the JSON correctly (bash and `2>&1 | Out-String` prove it); the loss is entirely in the PowerShell→GUI-child capture channel.

## 5. Design decisions (decision-complete; no TBD)

### 5.1 D1 — PRIMARY: route AC CLI invocations through Git Bash

Adopted, with two layers:

- **L1 (mandatory, runner-independent, in-repo): bash-native invocation recipe.** Every AC CLI invocation in the materialized agent context and the operator docs runs through `C:\Program Files\Git\bin\bash.exe` on Windows (explicit `bash.exe -lc '...'` wrap or a runner whose bash tool is Git Bash), captured with `$(...)` and parsed with `python`/`jq`; the PowerShell `& $bin ... | ConvertFrom-Json` capture pattern is banned. This works from ANY shell (PowerShell 5.1 interactive, pwsh, cmd) because bash.exe is console-subsystem and the AC child inherits bash's valid pipe handles. Claude-Code runners are told to prefer the Bash tool for AC CLI invocations.
- **L2 (best-effort, in-repo, pi-specific): project-level `shellPath` seed.** On Windows, when the config-seed machinery publishes a `.pi` destination into a replica, the seed step writes `<replica>/.pi/settings.json` with `{"shellPath": "C:/Program Files/Git/bin/bash.exe"}` — ABSENT-ONLY (an operator-authored shellPath is never overwritten), best-effort (a write failure logs and never changes the seed outcome). Because pi merges project settings over global settings (verified §3.2), this flips the effective bash-tool shell to Git Bash even when `~/.pi/agent/settings.json` names pwsh. The write happens at spawn, before the pi process starts.

Feasibility of "force bash.exe for at least AC CLI invocations" is CONFIRMED (evidence §3.2/§3.4/§3.1.6), so per the directive the shim is NOT needed.

### 5.2 D2 — option (a) "write always to the inherited stdout handle": already implemented, insufficient — NOT the fix

The #129 `attach_parent_console` condition already writes to inherited handles whenever they are valid (cli/mod.rs:247-270). The PS 5.1 interactive direct-capture case provides NO routable stdout handle for the GUI child to inherit; no in-binary stdout strategy can conjure a pipe the parent never created. Recorded as rejected-with-evidence; no code change.

### 5.3 D3 — option (b) console-subsystem shim (`agentscommander_cli.exe`, SUBSYSTEM=3): REJECTED

Per the directive, the shim is only for the case where bash routing is infeasible. It is feasible (§5.1), so the shim is rejected, with these recorded reasons (kept for the issue thread):
- A second binary doubles the deployment surface (`0_AC` renames, smoke binary names, packaging), contradicting the minimal-footprint constraint;
- it does not fix Claude-Code PowerShell-tool usage (the tool would still invoke the exe — via pwsh — unless the recipe routes through bash), so the recipe change is required ANYWAY;
- PS 5.1 would still not propagate `$LASTEXITCODE` semantics to the harness in the same way bash does; the recipe wrap gives exit-code propagation for free (bash.exe is console-subsystem).
No shim code, no Cargo `[[bin]]`, no new binary names, no smoke-binary-list changes.

### 5.4 D4 — option (c) `send` fail-loud enqueue receipt: COMPLEMENTARY, adopted (shell-independent)

`send` prints `Queued: <message-id>` on stdout immediately after the outbox write succeeds (`send.rs:1107` success path, before the confirmation wait). The existing `Delivered: <message-id>` line on confirmed delivery is unchanged. The recipe (context + docs + role templates) mandates: **never report a message as sent without a captured `Queued:` line**; a missing receipt means NOT enqueued → report failure. This closes the "aborted before send / send failed / agent still says enviado" class regardless of shell, because the receipt is only observable when the process actually ran and enqueued.

### 5.5 D5 — in-repo vs out-of-repo split (owner-directive "marcá claramente")

| Change | Location | Owner |
|---|---|---|
| Context recipe (bash routing + receipt rule) | `session_context.rs` (repo) | implementer (this PR) |
| `send` `Queued:` receipt + `--help` sentence | `cli/send.rs` (repo) | implementer (this PR) |
| pi `shellPath` absent-only seed step + tests | `config_seed.rs` (repo) | implementer (this PR) |
| Operator doc recipe | `docs/agents/inter-agent-messaging.md` (repo) | implementer (this PR) |
| Windows capture tests | `tests/cli_powershell_capture.rs`, `scripts/smoke-cli-powershell.ps1` (repo) | implementer (this PR) |
| Role templates (tech-lead/dev `Role.md`) + workspace seeds (`default.pi`) + global pi settings + replica re-seed | Agent Matrix / `<workspace>/.ac/` / `~/.pi/` (OUTSIDE repo) | tech-lead (harness operator), after this PR merges, per §7 checklist |

### 5.6 D6 — budget constraint for the context text (MEASURED, not predicted)

The WG profile is byte-budgeted (`session_context.rs:10289-10342`: `full_wg.len() <= 8_313`; reduction vs v3 baseline `>= 757`). Measured on this tree by running `token_accounting::token_accounting_report` (debug build): `profile: WG replica` = **7998 chars** today (slack 315). The §6.1 text was sized against that measurement: net addition +242 (receipt rule +134 net, Windows paragraph +197, compensating trims −89 across the four edits), landing the profile at ≈8240 (slack 73, reduction 830 ≥ 757). The implementer MUST re-run the budget test (§8.1) and confirm the value; if the measured baseline differs (it should not — deterministic fixture), apply only the trims already specified in §6.1(a) and re-measure; do NOT change the budget constants. If the ceiling is still exceeded at that point, stop and report the measured `full_wg.len()` to the tech-lead.

## 6. In-repo change plan (file-by-file)

### 6.1 S1 — `src-tauri/src/config/session_context.rs` (recipe)

(a) In `render_inter_agent_messaging_block` (`:3436`), make these four exact text edits in the SAME block (the two additions are compensated by the three trims so the profile stays inside the byte budget — measured net effect in §5.6):

1. Replace

```
The recipient gets a file-path notification and reads the file. Do NOT use `--get-output`; it blocks and is only for non-interactive sessions. After sending, wait for the reply.
```

with

```
The recipient gets a file-path notification and reads the file. Do NOT use `--get-output` (blocks; non-interactive only). **Receipt required:** never report a message as sent without a captured `Queued: <message-id>` line; a missing receipt means NOT enqueued. After sending, wait for the reply.
```

2. Replace

```
Before every send, run `list-peers-lean` and use its exact JSON `name`; never guess.
```

with

```
Before every send, run `list-peers-lean` and use its exact JSON `name`.
```

3. Replace

```
If it returns an empty array, stop and report it; never scan sibling directories instead.
```

with

```
If it returns an empty array, stop and report it.
```

4. Replace

```
The recipient gets a file-path notification and reads the file.
```

with

```
The recipient reads the notified file path.
```

(b) In the same block, after the `### List available peers` code fence, append (as a new paragraph; Windows-only via a `#[cfg(target_os = "windows")]` constant so non-Windows builds render nothing — the whole paragraph is the constant, empty string elsewhere):

```
**Windows:** the release binary is GUI-subsystem; PowerShell direct capture is empty. Run AC CLI invocations via Git Bash (`C:\Program Files\Git\bin\bash.exe`); never `& $bin ... | ConvertFrom-Json`.
```

The full rationale and the `$(...)` capture / `python`/`jq` parse / Bash-tool preference guidance live in `docs/agents/inter-agent-messaging.md` (§6.4) — the context carries the compact operational rule to stay inside the byte budget.

(c) The Workgroup and Root arms of `send_message_instructions` (`:3611-3641`) are unchanged — the receipt rule and the shell rule live in the shared block, so both modes inherit them.

(d) Update tests:
- Extend `default_context_embeds_filename_only_warning` (`:5255`) or add a sibling test asserting the rendered WG context contains `Receipt required` and `Queued: <message-id>`.
- Add `#[cfg(windows)]` test: rendered context contains `**Windows:**` (the routing paragraph), `bash.exe`, and `ConvertFrom-Json`.
- The v3 byte-budget test (`:10289-10342`) must stay green (see D6); expected value after the change: `full_wg.len()` ≈ 8240 (measured baseline 7998 + net 242), well under the 8313 ceiling.
- The `{{`/`}}` no-placeholder assertion and the `--send <filename> --mode wake` assertions must stay green (no placeholder tokens introduced; the new text uses only literal backticks and `<...>` placeholders that are already legal in this block).

### 6.2 S2 — `src-tauri/src/cli/send.rs` (enqueue receipt)

(a) In `execute`, immediately after the successful `std::fs::write(&outbox_path, json)` (`:1107` success path, i.e. after the `if let Err(e) = ...` error arm, before `log::info!` at `:1112`), add one line:

```rust
crate::cli_println!("Queued: {msg_id}");
```

`msg_id` is the v4 UUID of the outbox file (`<msg_id>.json`) — the same id the `Delivered:` line reports. One emission site covers `--send`, `--command`, `--outbox`, and the root/master app-outbox paths (all share this write). The pty-input path is unchanged (already prints `Queued:` at `:682`).

(b) In `SendArgs` `after_help`, extend the `DELIVERY CONFIRMATION` paragraph with one sentence: "On enqueue, the CLI prints `Queued: <message-id>` to stdout; a missing `Queued:` line means the message was NOT enqueued." Add a unit test asserting the `after_help` contains that sentence (mirror `cli_after_help_documents_token_validation_model` in `cli/mod.rs`).

(c) Behavior contract (unchanged except the new stdout line): exit codes stay 0/1; `Delivered:` line unchanged; confirmation timeout still exits 1 with the message remaining durably queued — the `Queued:` line lets the caller distinguish "enqueued but unconfirmed" from "never enqueued".

### 6.3 S3 — `src-tauri/src/config/config_seed.rs` (pi shellPath seed)

(a) Add a small Windows-only helper (module-private, e.g. `ensure_pi_bash_shell_path(pi_dir: &Path) -> std::io::Result<()>`): if `pi_dir/settings.json` exists (no-follow check, mirroring `destination_absent_no_follow` semantics), do nothing; else atomically create it (write temp + rename in `pi_dir`, or plain `create_new` write) containing exactly:

```json
{"shellPath": "C:/Program Files/Git/bin/bash.exe"}
```

(b) In `perform_config_seed_with_clock_and_hooks` (`:469`), after the successful publish (after step 6 trash cleanup / step 7 log, immediately before the `ConfigSeedReport::Published(...)` return at `:691`), insert:

```rust
// #1596: pi's bash tool runs through `shellPath`; project `.pi/settings.json`
// overrides the global one, so an absent-only seed routes pi through Git Bash
// on Windows. Best-effort: failure logs and never changes the seed outcome.
#[cfg(windows)]
if dest_name == ".pi" {
    if let Err(error) = ensure_pi_bash_shell_path(&seed.dest) {
        log::warn!(
            "[config-seed] could not seed pi shellPath at {}: {}",
            seed.dest.display(),
            error
        );
    }
}
```

`dest_name` is already bound at `:547`. Gate: `cfg(windows)` + exact dest folder name `.pi` (only pi config seeds use a `.pi` dest). The write is inside the already-serialized seed section (the spawn chokepoint holds `ConfigSeedLockState`), so no new concurrency surface.

(c) Unit tests (all `#[cfg(windows)]`, in `config_seed.rs` test module):
- absent `settings.json` under a `.pi` dest → file created with exactly the JSON above, seed report still `Published`;
- existing `settings.json` with operator content → content untouched, no error;
- non-`.pi` dest (e.g. `.claude`) → no file written anywhere.
Reuse the existing fixture helpers (`write_file`, temp tree builders already used by the `default.claude` tests).

### 6.4 S4 — `docs/agents/inter-agent-messaging.md` (operator doc)

Add after the "Discovering peers" JSON example:
- a "## Shell routing on Windows (required)" subsection: GUI-subsystem explanation (one paragraph), the `bash.exe -lc '...'` wrap form for `list-peers-lean` and `send`, `$(...)` capture + `python`/`jq` parsing, the explicit ban on `$x = & $bin ... | ConvertFrom-Json`, and the Bash-tool preference for runners exposing Bash and PowerShell tools;
- in the "Sending a message" section, a receipt-verification step: after `send`, verify a `Queued: <message-id>` line (and, for confirmed delivery, `Delivered: <message-id>`) was captured before considering the message sent; missing receipt → report failure, never "enviado".

### 6.5 S5 — `src-tauri/tests/cli_powershell_capture.rs` (bash-routed coverage)

Add one `#[ignore]` release-mode test, same shape as the existing four: `list_peers_outputs_valid_json_under_powershell_noninteractive_via_git_bash`. Inner command under PS-NonInteractive: `& bash.exe -lc '<BIN> list-peers --token <token> --root <tmp>'` (single-quote escaping identical to the existing `ps_command`). Assertions: stdout parses as a JSON array; the OUTER run reports exit code 0 **and** — new capability — `$LASTEXITCODE` from bash.exe equals 0 (bash is console-subsystem, so PS propagates the AC binary's exit code through it; use the `run_ps` shape extended to `; exit $LASTEXITCODE`). Skip (return early with a note) when `bash.exe` is not on PATH. Keep the existing tests and their "no exit-code assertion" comment untouched — the comment gains one sentence pointing at the bash-routed test as the exit-code carrier.

### 6.6 S6 — `scripts/smoke-cli-powershell.ps1` (smoke coverage)

(a) Add `Invoke-BashRouted` (mirror of `Invoke-PSNonInteractiveDirect`): outer shell = `powershell.exe` (the caller shell), inner = `bash.exe -lc '<exe> <args...>'` via `ProcessStartInfo` with redirected stdout/stderr; exit code = bash's exit code (== AC binary's). Skip-case result when `bash.exe` is not found.
(b) Add one case after the existing `01-list-peers-direct`: `01-list-peers-via-git-bash` — asserts stdout parses as JSON, stderr empty, and `ExitCode -eq 0` (exit-code propagation through bash — intentionally NOT assertable in the direct GUI-child cases, per the file's own comment).
(c) `smoke-cli-release-windows.ps1` is UNCHANGED (it already aggregates the helper's `$failed`/exit code per binary; the new case runs for both smoke binaries under both shells).

## 7. Out-of-repo handoff checklist (owner: ac-tech-lead-v3 as harness operator; NOT implementable in this PR)

Executed after this PR is merged and the release binary is rebuilt, using the EXACT replacement text below:

1. **Role templates** — in `.ac/_agent_tech-lead/Role.md` and `.ac/_agent_dev-rust/Role.md` (and any other harness role that embeds the send recipe), replace any PowerShell capture pattern (`$x = & $bin list-peers-lean ... | ConvertFrom-Json` and variants) with the bash-native block: run `list-peers-lean` through Git Bash with `$(...)` capture, parse with `python`/`jq` (or pass the deterministic FQN directly to `--to`), and NEVER report a message as sent without a captured `Queued: <message-id>` receipt line from `send`.
2. **Workspace seed templates** — add `<workspace>/.ac/default.pi/settings.json` containing `{"shellPath": "C:/Program Files/Git/bin/bash.exe"}` so future replicas carry it at seed time (the in-repo step §6.3 is the runtime guarantee; the template is the seed-time belt).
3. **Global pi settings** — optional belt: `~/.pi/agent/settings.json` `shellPath` = `C:/Program Files/Git/bin/bash.exe`; the project-level seed overrides it either way (verified pi precedence §3.2).
4. **Existing replicas** — the §6.3 step applies absent-only at the next spawn of each pi session; replicas whose `.pi/settings.json` already names pwsh keep their operator setting (documented; not silently overwritten).
5. **RTK log-accuracy note (informational, not blocking)** — the pi extension's `configuredShell` (`tool-hook.ts:245-263`) uses last-wins (global over project), which can disagree with pi's actual project-overrides-global merge. With the §6.3 seed, the extension may label calls `PowerShell:` while pi actually runs bash. This is a log-field inaccuracy only (the routing is correct); fixing the extension is a separate out-of-repo seed change, tracked here as a note for the harness operator.
6. **Verification on this machine** — from the tech-lead replica: `peers=$("C:\Program Files\Git\bin\bash.exe" -lc '"<AGENTSCOMMANDER_BINARY_PATH>" list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"')`; expect non-empty JSON, 29 peers, FQN resolved; then a real `send` and confirm both `Queued:` and `Delivered:` lines.

## 8. Tests and acceptance evidence

### 8.1 Unit/CI tests (implementer-run; exact-head CI is authoritative per §9.1)

- `cargo test --locked --lib --bins --tests` (Windows) — includes the new/updated `session_context` recipe + budget tests, `send` after_help test, `config_seed` pi-shellPath tests. Expected: all green. Failure behavior: any failing new test blocks the PR.
- `cargo clippy --locked --all-targets` and `cargo check --locked` (Windows) — clean.
- `npm run smoke:cli-release-windows` locally (requires `npm run build:prod:no-bundle` first; Windows only). Expected: existing cases green + new `01-list-peers-via-git-bash` green for `agentscommander.exe` and `agentscommander_testeable.exe` under both shells. Skip cases (bash.exe missing) are accepted SKIPs, not failures.
- Manual release-mode run: `cargo test --release --test cli_powershell_capture -- --ignored` — all five tests green (incl. the new bash-routed one). This is a manual/`#[ignore]` check; CI's authoritative coverage is the smoke job (§9.1).
- The v3 byte-budget tests (§5.6/D6) MUST stay green; if not, apply the §6.1(a) trims and re-measure, and if still failing report the measured `full_wg.len()` to the tech-lead (do not change constants).

### 8.2 Manual behavioral verification (implementer, on this machine, release binary)

1. From PowerShell 5.1: `$x = & <release-exe> list-peers-lean --token <uuid> --root <replica>` → document empty (pre-existing bug shape, NOT a regression; no assertion).
2. Same via bash: `"C:\Program Files\Git\bin\bash.exe" -lc '"<release-exe>" list-peers-lean --token <uuid> --root <replica>'` → non-empty JSON; `$LASTEXITCODE` == 0.
3. `send --send` a scratch file against a scratch root → stdout contains a `Queued: <uuid>` line matching the outbox file name `<uuid>.json`.

### 8.3 Final Git/diff evidence (implementer, before PR)

- `git status --porcelain` shows ONLY: the plan file (untracked), `src-tauri/src/config/session_context.rs`, `src-tauri/src/cli/send.rs`, `src-tauri/src/config/config_seed.rs`, `docs/agents/inter-agent-messaging.md`, `src-tauri/tests/cli_powershell_capture.rs`, `scripts/smoke-cli-powershell.ps1`.
- `git diff` of the Rust changes is limited to the anchors in §6 (no budget constants, no seed-report shape, no `Cargo.*`, no workflow changes).
- Plan file force-added: `git add -f plans/1596-cli-stdout-powershell-capture.md`.

## 9. Delivery gates (baseline; evidence owner per gate)

Task class: routine (§2). No enhanced controls apply — recorded per the delivery-nonfunctional-invariants skill; host-tool provenance, signed-release, and hostile-host attestations are out of the accepted threat model and are NOT gates.

### 9.1 CI-to-plan parity

Source of truth: `.github/workflows/pr-regression-gates.yml` at `ecc6527` (job list §3.5). The PR head triggers: `test-debt`, `rust-regression` (windows: check/clippy/`cargo test --lib --bins --tests`), `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable`, `windows-release-cli-smoke` (`build:prod:no-bundle` + `smoke:cli-release-windows`), `frontend-regression`. The diff touches Rust (windows paths) + docs + a Windows-only test/script: every triggered and configured-required check must pass on the exact PR-head SHA. Evidence: PR status on the head SHA; local runs are complementary, never a substitute. Failure behavior: any red required check blocks delivery; an unexplained skip or waiver is a blocker. `rust-regression` on windows-latest is the authoritative run for the new `#[cfg(windows)]` tests; `windows-release-cli-smoke` (Git Bash preinstalled) is the authoritative run for the smoke case.

### 9.2 Deterministic toolchain and build

Repository contract: `Cargo.lock` committed; CI pins `npm@11.6.2` + `npm ci`; rust-toolchain stable via `dtolnay/rust-toolchain@stable`. Local: use `--locked` for cargo commands and `npm ci`; record `rustc --version` and `cargo --version` in the PR. Expected: `cargo test --locked` resolves from the lockfile. Failure behavior: resolution/build errors block.

### 9.3 Authorized, traceable Git

Issue #1596 OPEN; branch `fix/1596-...` created from `main` @ `ecc6527` (verified §1). State-changing Git runs only inside `repo-AgentsCommander`; delivery via PR (never direct push to `main`). Plan file force-added (§1). Preconditions: clean tree + pinned base before the first product mutation, re-fetched before PR creation (§1 ritual). Failure behavior: unknown/dirty base, missing issue linkage, or scope drift blocks readiness.

### 9.4 Process state, configuration, working directory

No inherited env/config materially changes the commands (cargo/npm standard). All mutating and cwd-sensitive commands run from the repo root with explicit paths. Expected: reproducible output. Failure behavior: cwd drift or ambient config interference is recorded and fixed before acceptance.

### 9.5 Validation and scope before acceptance

Frozen path set (§8.3) — the intended diff shape is exactly S1-S6 + the plan file. Postcondition: `git status --porcelain` matches §8.3. The new config_seed write is confined to `<replica>/.pi/settings.json` absent-only, covered by tests. Failure behavior: any file outside the set (or any budget constant / workflow / Cargo change) is scope drift → stop and report.

### 9.6 Mutation ownership and no-clobber recovery

The implementer is the only writer on this branch during implementation. Before writes: recheck frozen base + clean status. On failure: restore only paths this run changed, and only when their current state is demonstrably that run's output (`git diff`/`git restore -- <file>` on the specific file); never broad `git reset`/`git restore` of the tree; preserve and report any externally changed bytes. The plan file is added with `-f` only. Success: prove the §8.3 path set, index state (plan file staged), and ordinary-untracked state (nothing else).

### 9.7 Bounded execution and durable diagnostics

`cargo test`/`cargo clippy` run with a runner timeout (≥30 min for the cold `--release` build, ≥15 min for debug test builds) and non-interactive stdin. Smoke writes durable logs under `artifacts/cli-release-smoke/` (existing behavior); keep them until the outcome is reported. A timed-out run is never reported as success. Failure behavior: timeout/cancel → rerun with the recorded diagnostics; cleanup defects must not erase the primary failure.

### 9.8 Evidence discipline

Zero and absence are valid typed states: "no `.pi` settings.json present" → seed writes it; "existing settings.json" → untouched; "bash.exe missing" → SKIP case, not failure; "empty peer array" → the recipe says stop and report (unchanged). Each gate above states its expected result and failure behavior; remote-only evidence is owned by the exact-head CI checks (§9.1).

## 10. Dependency-cycle and layering statement (planning rule 8)

Enumerated arcs introduced by this plan:

- `cli/send.rs`: one new `crate::cli_println!` call — the macro is ALREADY imported/used by this module (macro lives in `cli/mod.rs`, same crate); NO new module arc.
- `config/config_seed.rs`: new private helper using only `std::fs`, `std::io`, `std::env`-free path logic — module-local; NO new module arc.
- `config/session_context.rs`: text constants + `#[cfg(windows)]` string gating — NO new module arc.
- Tests/scripts/docs: no Rust module arcs.

The plan adds **ZERO module-to-module arcs**; `cyclicSccs` and every SCC member set cannot change; no cross-boundary arc exists to classify. Role/layering hygiene: the config-seed helper stays in the config layer (file seeding is already its job); no lower layer gains a UI-transport/`AppHandle`/tauri dependency; `send` stays in the cli layer with its existing stdout macro.

Acceptance criterion for the implementer (base `ecc6527` vs final branch head, clean tree for both):

```
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<VAULT>/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green iff: (1) `cyclicSccs` equal pre/post; (2) cyclic SCC member sets identical set-to-set; (3) zero new `from -> to` pairs cross a previously-clean SCC boundary; (4) regenerated `module-arcs.txt` byte-identical (empty `git status` on it); (5) structural layering guards (e.g. `loops_layering`, `instance_gitignore_layering`) stay green. Exit code 1 from the detector is the normal gating outcome; only exit 3 means no graph.

## 11. Implementation order

1. Commit A — S1 `session_context.rs` recipe + its tests (context first: the behavioral instruction layer).
2. Commit B — S2 `send.rs` `Queued:` receipt + `after_help` sentence + test.
3. Commit C — S3 `config_seed.rs` pi shellPath step + tests.
4. Commit D — S4 docs, S5 `cli_powershell_capture.rs`, S6 smoke script (coverage of the recipe/send changes).
5. Force-add this plan file and run the full §8 acceptance battery: `cargo test --locked --lib --bins --tests`, clippy, check, budget tests, local smoke (release), manual §8.2 checks, §10 dependency-cycle gate.
6. Open/refresh the PR with the §9.3 evidence; report exact-head CI results.

## 12. Residual risks and accepted tradeoffs (decision-complete)

- **RTK log-field inaccuracy** (§7.5): informational only; routing correctness is unaffected.
- **Operator-set pwsh `shellPath` in an existing replica** is preserved (absent-only); such replicas keep relying on L1 (recipe wrap) until the operator reseeds. Accepted.
- **Claude Code PowerShell tool** cannot be disabled by config; the recipe's Bash-tool preference is instruction-level. Accepted (directive's "al menos las invocaciones al CLI" is satisfied by the wrap, which works from any tool/shell).
- **Context byte budget** (D6): the added text is compact; the trim fallback is specified; budget constants are frozen.
- **Non-Windows**: no behavior change (all new code paths are `#[cfg(windows)]` or doc-only); the receipt line is platform-neutral and harmless.
- **GUI behavior**: untouched — no `main.rs`, no Tauri surface, no new binary, no smoke binary-list change.
