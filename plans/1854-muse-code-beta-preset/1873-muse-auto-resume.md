# Phase #1873 — add first-class Muse workspace-latest automatic resume
Class: design-bearing
Owner: AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-v4
Child issue: [#1873 — Rust: add first-class Muse workspace-latest automatic resume](https://github.com/mblua/AgentsCommander/issues/1873)
Parent epic: [#1854 — Add a beta Muse Code catalog preset](https://github.com/mblua/AgentsCommander/issues/1854)
Repository: /home/mblua/0_repos/AgentsCommander_iac/.ac/room-9-ac-dev-team-v4/repo-AgentsCommander
Branch: feature/1873-muse-auto-resume
Branch base: then-current main, recorded when the coordinator creates this branch
Design evidence base: 5c7dd08841a5846f483ba59a0d0dd94ea6410c4d
Depends on: #1860 landed on main and green, including the five frozen plans and Rust catalog
Total phase files: 10
## Objective and cause
Resume eligible restored/reopened Muse into its newest retained launch-cwd session:
~~~text
muse resume --last
~~~
Linux Muse 1.0.3 (1.0.3-R2198.1), 2026-09-08, has parser/no-history narrative only; successful retained-session proof is unavailable. Vendor continuation/live resume remain PENDING; workspace-latest resume is required, not observed. AC owns no Muse UUID. Preserve fresh/non-Muse behavior and never retry fresh. Rust `CodingAgentKind::Muse` and TypeScript `"muse"` land atomically; no one-sided handoff/merge boundary.
## Exact file set
Only these ten files may differ from the phase branch base:
1. src-tauri/src/session/profile.rs
2. src-tauri/src/commands/session.rs
3. src-tauri/src/config/settings.rs
4. src-tauri/src/config/sessions_persistence.rs
5. src-tauri/src/loops/delivery.rs
6. src-tauri/src/commands/telegram.rs
7. src-tauri/src/config/agent_command.rs
8. src-tauri/src/config/config_seed.rs
9. src-tauri/src/pty/spawn_diagnostics.rs
10. src/shared/types.ts
The five frozen plans and all #1860 bytes are tracked base content: verify, never edit/stage/recommit them. Also keep lib.rs, phone/mailbox.rs, session_context.rs, agent_version.rs, session/manager.rs, pty/inject.rs, telegram/bridge.rs, other TypeScript, Cargo metadata, locks, workflows, module-arcs.txt, generated files, and docs unchanged.
## Pre-mutation and environment gate
1. Synchronize byte-identical open/linked #1854/#1873 bodies and immutable hashes.
2. After green #1860, branch from updated main; record `PHASE_BASE_SHA`, clean root/index/worktree, issue sync, frozen plans and its nine-file/two-commit handoff.
3. Fetch/classify relevant main drift; refresh affected toolchain/arc/workflow evidence.
4. Pre-mutation provisioning/checks are feedback only in a separate shell; end it before the product commit. Use only a proven-below-root `AGENTSCOMMANDER_ROOT/.evidence/ac-1873.*` temp; record OS/UTC, tools, ten file hashes, probe limits and `ARC_RECORD_SHA_BEFORE`.
Any failed precondition blocks writes. This phase authorizes no Muse install/authentication/update, interactive input, or live prompt.
### Replica-local Node prerequisite

Host Node 18.19.1/npm 9.2.0 is bootstrap-only and may not run repository validation. Before mutation, provision Node 22/npm 11.6.2 below AGENTSCOMMANDER_ROOT using body 1 in a separate feedback shell; that shell must exit before any product write/commit. After committing the ten-file candidate, producer feedback and independent lead acceptance each execute bodies 1–3 together in a fresh noninteractive shell at that clean candidate. Never resume body 3 from a precommit shell, rebind its anchors, or reuse its logs. Every blocking command retains combined output and start/end/elapsed/command-exit/tee-exit metadata:

~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
if test -z "${LEAD_CHECKOUT_SHA256:-}"; then LEAD_CANDIDATE_SHA="$(git rev-parse HEAD)" || exit 1; bind_checkout; fi
assert_checkout; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1
command -v timeout rg npm sha256sum find >/dev/null
test -n "${AGENTSCOMMANDER_ROOT:-}"
mkdir -p "$AGENTSCOMMANDER_ROOT/.evidence"
RUNTIME_LOG_DIR="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1873.XXXXXX")"
case "$RUNTIME_LOG_DIR" in "$AGENTSCOMMANDER_ROOT"/*) ;; *) exit 1;; esac
run_logged_to() {
  local log_dir="$1" label="$2" limit="$3"; shift 3; local start_utc start_s end_utc end_s rc tee_rc meta
  case "$log_dir" in "$RUNTIME_LOG_DIR"|"$RUNTIME_LOG_DIR"/*) ;; *) return 1;; esac; test -d "$log_dir"
  assert_checkout
  start_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; start_s="$(date +%s)"
  set +e; timeout "$limit" "$@" </dev/null 2>&1 | tee "$log_dir/$label.log"
  local -a status=("${PIPESTATUS[@]}"); set -e; rc="${status[0]}"; tee_rc="${status[1]}"
  assert_checkout
  end_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; end_s="$(date +%s)"
  meta="$log_dir/$label.meta"
  { printf 'timeout_limit=%s\nstdin=/dev/null\nstart_utc=%s\nend_utc=%s\nelapsed_seconds=%s\ncommand_exit=%s\ntee_exit=%s\nargv=' "$limit" "$start_utc" "$end_utc" "$((end_s-start_s))" "$rc" "$tee_rc"; printf '%q ' "$@"; printf '\n'; } | tee "$meta"
  local -a meta_status=("${PIPESTATUS[@]}"); for value in "${meta_status[@]}" "$rc" "$tee_rc"; do test "$value" -eq 0 || exit 1; done
  validate_runtime_meta "$meta" "$limit" "$@"; test "$rc" -eq 0; test "$tee_rc" -eq 0
}
run_logged() { run_logged_to "$RUNTIME_LOG_DIR" "$@"; }
run_gate_logged() { local label="$1"; test -n "${RUNTIME_GATE_DIR:-}"; run_logged_to "$RUNTIME_GATE_DIR" "$@"; assert_log_destination "$RUNTIME_GATE_DIR" "$label"; }
assert_log_destination() { local d="$1" label="$2" ext expected f; local -a found; for ext in log meta; do test -f "$d/$label.$ext"; mapfile -t found < <(find "$RUNTIME_LOG_DIR" -type f -name "$label.$ext" -print); expected=1; if test "$d" != "$RUNTIME_LOG_DIR" && test -n "${RUNTIME_PRECOMMIT_GATE_DIR:-}" && test "$d" != "$RUNTIME_PRECOMMIT_GATE_DIR"; then expected=2; fi; test "${#found[@]}" -eq "$expected"; for f in "${found[@]}"; do test "$f" = "$d/$label.$ext" || { test -n "${RUNTIME_PRECOMMIT_GATE_DIR:-}"; test "$f" = "$RUNTIME_PRECOMMIT_GATE_DIR/$label.$ext"; }; done; done; }
validate_runtime_meta() { local meta="$1" limit="$2" argv start end; shift 2; local -a row; printf -v argv '%q ' "$@"; mapfile -t row <"$meta"; test "${#row[@]}" -eq 8; cmp -s "$meta" <(printf '%s\n' "${row[@]}"); test "${row[0]}" = "timeout_limit=$limit"; test "${row[1]}" = stdin=/dev/null; printf '%s\n' "${row[2]}" | rg -x -- 'start_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z' >/dev/null; printf '%s\n' "${row[3]}" | rg -x -- 'end_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z' >/dev/null; start="${row[2]#start_utc=}"; end="${row[3]#end_utc=}"; [[ "$end" == "$start" || "$end" > "$start" ]]; printf '%s\n' "${row[4]}" | rg -x 'elapsed_seconds=(0|[1-9][0-9]*)' >/dev/null; test "${row[5]}" = command_exit=0; test "${row[6]}" = tee_exit=0; test "${row[7]}" = "argv=$argv"; }
TOOL_ROOT="$AGENTSCOMMANDER_ROOT/.toolchains/ac-1854-node22"
mkdir -p "$TOOL_ROOT"/{cache,tmp,install}
: >"$TOOL_ROOT/npmrc"
BOOTSTRAP_NPM="$(command -v npm)"
NODE_BIN="$TOOL_ROOT/install/node_modules/node/bin"
run_logged node-provision 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" \
  "$BOOTSTRAP_NPM" install --prefix "$TOOL_ROOT/install" --no-save --no-package-lock node@22 npm@11.6.2
NODE22="$NODE_BIN/node"
NPM11="$TOOL_ROOT/install/node_modules/npm/bin/npm-cli.js"
export PATH="$NODE_BIN:$PATH"; hash -r
test "$(command -v node)" = "$NODE22"
run_logged node-direct-version 1m "$NODE22" -p process.versions.node
run_logged node-path-version 1m node -p process.versions.node
cmp -s "$RUNTIME_LOG_DIR/node-direct-version.log" "$RUNTIME_LOG_DIR/node-path-version.log"
rg -x -- '22\.[0-9]+\.[0-9]+' "$RUNTIME_LOG_DIR/node-path-version.log"
run_logged npm-version 1m "$NODE22" "$NPM11" --version
rg -x -- '11\.6\.2' "$RUNTIME_LOG_DIR/npm-version.log"
run_logged npm-ci 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" ci
ARC_RECORD_SHA_BEFORE="$(sha256sum src-tauri/module-arcs.txt | cut -d' ' -f1)"
printf 'arc_record_sha256_before=%s\n' "$ARC_RECORD_SHA_BEFORE" | tee "$RUNTIME_LOG_DIR/arc-record-before.meta"
~~~

The PATH prefix is present during provisioning and exported before every npm ci/run child; direct and PATH-resolved guards must agree on major 22. Cache/tmp/user config stays below AGENTSCOMMANDER_ROOT, with no global/HOME write. Any command/tee failure, including timeout 124, blocks.

## Settled continuity and eligibility contract

Add CodingAgentKind::Muse with serde/as_str value muse. Detect only a direct shell with exact stem muse, including an absolute path ending `/muse`; reject prefix names and argument-only identity inside env/shell/PowerShell/cmd/compound wrappers.

Add MUSE_PROFILE with:

- IdleTuning::DEFAULT.
- resume_tokens exactly ["resume", "--last"].
- container_credential: None.
- auto_self_clear_supported: false.

In the same product commit, add only `"muse"` to `src/shared/types.ts::CodingAgentKind`. `SessionInfo::from` already copies the Rust value to `agentKind`, so both sides land together. Change no wire/serde/interface/IPC shape. The successful-create test serializes `SessionInfo` and asserts `agentKind == "muse"`; typecheck/build prove the consumer.

In commands/session.rs add pure pub(crate) `trusted_muse_auto_resume_spawn`, true only when every condition holds:

- The resolved AgentSpawnCommand is present, proving configured-spawn provenance.
- The actual shell and args exactly equal that resolved recipe.
- The resolved backend is LocalProcess.
- The compiled host is macOS or Linux.
- Path::file_name of the direct shell is the exact case-sensitive string muse.
- Configured args are empty.

Add private `maybe_inject_muse_resume`: require detected Muse plus `skip_auto_resume == false`, slice-destructure the profile tokens, append them to only the local effective vector, and return true. Wrong arity or a second call fails closed without mutation.

Empty argv is the precedence rule. Any configured argument wins and suppresses AC injection, including resume, --last, UUID, prompt, --no-session-log, workspace/root/help/version, or future flags; AC neither validates nor rewrites them.

Pre-spawn settings validation honors the same identity. After normalization, `validate_agent_command_text` computes `CodingAgentKind::detect` once and returns before legacy scans for exactly Pi or Muse. Direct `muse --workspace /tmp/codex resume --last` is valid despite a Codex-looking value; preserve its argv and suppress injection. An `env` wrapper around the same token sequence is not detected as Muse and retains the existing Codex manual-resume rejection.

Ad-hoc launches, prefix names, wrappers, recipe mismatches, Windows, and containers remain unchanged.

## Lifecycle state table

| Material path | Existing state/intent | Required Muse process argv |
|---|---|---|
| Ordinary create | No prior state; omitted/default skip | configured plain muse |
| Explicit Restart Session | User chose a fresh boundary | configured plain muse |
| Startup restore | known persisted row, fresh marker false | muse resume --last |
| Startup restore | start_fresh_on_restore true or durable mirror forces fresh | configured plain muse |
| Closed coordinator reopen through ProjectPanel | close marker requests resume | muse resume --last |
| First coordinator launch | no close marker | configured plain muse |
| Root selection with no prior Root | fresh create | configured plain muse |
| Root agent-picker selection, live Root record | restart flag omitted; backend defaults fresh | tear down old runtime; exactly one replacement spawn with configured plain muse |
| Dormant Root record | wake existing state | muse resume --last |
| Root record missing its PTY but still non-dormant | same omitted restart flag/fresh replacement | configured plain muse |
| Mailbox cold creation | spawn_with_resume false | configured plain muse |
| Mailbox known-state wake | spawn_with_resume true | muse resume --last |
| Loop cold creation | had_existing_match false | configured plain muse |
| Loop exited/missing-PTY wake | had_existing_match true | muse resume --last |
| Loop live reuse | no spawn | no process argv |

Existing provider-neutral skip_auto_resume and durable start_fresh_on_restore remain authoritative. Add no Muse branch to lib.rs or phone/mailbox.rs. Root picker is not live-PTY reuse: `rootAgentCodingAgentAction` omits the flag, `SessionAPI.restart` sends null, `effective_restart_skip_auto_resume(None)` is true, and `execute_restart_transaction` tears down then replaces.

## Failure, event, and persistence contract

Spawn-time and post-spawn failures are distinct and fail closed:

- If `PtyBackend::spawn` returns `Err`, create returns that one diagnostic, publishes no created event, reverts/removes the pending row, records exactly one resume attempt, and never starts plain Muse. No row remains to restart: the user must fix the cause and launch/create again.
- If spawn succeeds and the child later exits nonzero, create has published its one `session_created` event and retained row. The local monitor surfaces one child-initiated `[pty] child-exit` diagnostic with exact code; that row's `Restart Session` is available as deliberate fresh recovery, with no second error/event/replacement/plain retry.

No-history, corrupt/unreadable state, permissions, invalid selectors, authentication, parser, and other Muse exits follow whichever timing occurs. Each path makes exactly one process attempt, surfaces one diagnostic, and never automatically retries or falls back to plain Muse; AC inspects no vendor text/code.
Automatic retry is forbidden because exit 1 also covers unknown UUID and picker failure, not only no-history.

Session.shell_args stays configured; only the local spawn vector and Session.effective_shell_args receive AC tokens. Muse takes Pi's preserve path in strip_auto_injected_args: never strip textual resume --last. Snapshots/restart preserve selectors, prompts, root flags, and --no-session-log byte-for-byte.

Continuity is workspace-latest for launch cwd; no store/lifetime, tie-break, cwd normalization, symlink/case/repository/worktree grouping, --workspace effect, UUID discovery, or container-durability claim is made.

## Exact changes by file and symbol

### session/profile.rs

- Extend CodingAgentKind, detect/as_str/profile, comments, and all-kind tests; add exact MUSE_PROFILE. `muse_serde_profile_detection_and_submission_boundary_are_stable` covers direct/absolute positives, prefix/argument/wrapper negatives, serde/profile, default idle, and no PtySubmissionAgent.

### commands/session.rs

- Add the predicate/injector at the settled seam, a test-only Muse settings row,
  and explicit `Some(Muse) => None` ManagedContextTarget.
- muse_resume_eligibility_is_exact_and_fail_closed covers all positives/vetoes; muse_fresh_and_resume_intents_share_one_launch_seam covers true, false, and durable-fresh intent. muse_manual_collision_args_pass_validation_and_reach_backend_unchanged proves the accepted collision argv suppresses injection end to end.
- trusted_muse_resume_reaches_backend_effective_argv_only uses CapturingSpawnBackend: macOS/Linux spec/effective args are the tokens, configured args stay empty, generic agent/profile env reaches configured/child env unchanged, and kind/wire value is Muse/muse; unsupported hosts stay plain.
- muse_live_root_selection_is_one_fresh_replacement routes omitted intent through execute_restart_transaction and requires one teardown, one empty-arg replacement, new id, old-row removal, and no resume token.
- muse_spawn_error_is_not_retried_fresh uses FailingSpawnBackend: one resume attempt/Err diagnostic, no created event/pending row/retry; only a later user launch/create can retry after the cause is fixed.
- macOS/Linux-only `muse_post_spawn_nonzero_exit_is_reported_once_without_fresh_fallback` runs a temp executable named `muse` that journals argv then exits 41. Through real LocalProcessBackend require successful create, one retained row/created event, wire muse, `[muse-path,"resume","--last"]`, bounded Exited(41,false), one ChildInitiated report with duplicate suppression, one journal entry, no other event/plain fallback; the retained row permits user-selected fresh Restart Session. FailingSpawnBackend is separate.
- Preserve every prior-provider result.

### config/settings.rs and config/sessions_persistence.rs

- Extend `validate_agent_command_text` from Pi to Pi-or-Muse; change no scanner. `validate_agent_commands_allows_direct_muse_arguments_with_legacy_provider_collision` accepts the collision and `validate_agent_commands_rejects_wrapped_muse_collision_before_spawn` pins the wrapper error.
- `muse_recipes_and_snapshots_preserve_configured_args_not_effective_resume` covers empty/manual/resume/UUID/no-log/prompt/workspace/root/wrapper/mixed-case recipes and snapshot round-trip; effective tokens never persist.

### loops/delivery.rs

- Pass the resolved command to loop_spawn_skip_auto_resume before moving it; return true only for cold eligible Muse. The named replacement test pins cold/known Muse and every prior provider.

### commands/telegram.rs

- `derive_reader` returns exactly `Telegram bridge does not support Muse sessions`
  for both backends. `derive_reader_rejects_muse_for_all_backends_before_bridge_creation` asserts full `Err(String)` equality, not a substring.
- Do not edit telegram/bridge.rs. `attach_telegram_bot_by_id` returns before `TelegramBridgeManager::attach`, so no Muse bridge/filter is created.

### config/agent_command.rs and config/config_seed.rs

- Map direct/absolute Muse to AGENTS.md while explicit filenames win; prove it in `muse_default_instructions_filename_is_agents_md` and `muse_explicit_instructions_filename_wins`.
- Map its config-dir warning to None in `muse_has_no_config_dir_warning_or_seed_contract`; add no Muse built-in/factory seed, automatic Muse credential flow, Muse-specific env key, isolation, or validation rule.
  The same test proves an active user `configSeed` still resolves provider-neutrally; the backend test above proves generic agent/profile env still flows without a Muse/AgentKind gate.

### pty/spawn_diagnostics.rs and shared/types.ts

- Add cfg(test)-only SpawnRecord exit-reported/cause accessors; change no production behavior. Add only `"muse"` to the TS union in the Rust contract commit; change no field/transport shape.

`session/manager.rs` and `pty/inject.rs` stay byte-identical; profile coverage keeps Muse outside logical/privileged PTY and self-maintenance behavior.

## Verification and positive control

Run from the repository root with noninteractive stdin, pipefail, retained stdout/stderr/exit/timing, and 30-minute bounds. List mode owns enumeration without executing test bodies; sixteen full-name `--exact` invocations independently own execution:

~~~bash
set -euo pipefail
command -v rg bash cmp sha256sum sort sed tr wc find tail >/dev/null
declare -F run_logged >/dev/null; test -n "${RUNTIME_LOG_DIR:-}"; test -n "${NODE_BIN:-}"
RUNTIME_REPO_ROOT="$(pwd)"; export RUNTIME_REPO_ROOT
case "$(uname -s)" in Linux|Darwin) ;; *) printf 'supported-host evidence required\n' >&2; exit 1;; esac
RUNTIME_PATHS=(src-tauri/src/commands/session.rs src-tauri/src/commands/telegram.rs \
  src-tauri/src/config/agent_command.rs src-tauri/src/config/config_seed.rs src-tauri/src/config/sessions_persistence.rs src-tauri/src/config/settings.rs \
  src-tauri/src/loops/delivery.rs src-tauri/src/pty/spawn_diagnostics.rs src-tauri/src/session/profile.rs src/shared/types.ts)
RUNTIME_REQUIRED_ROOT_LABELS=(rustfmt rust-lib layering cargo-check clippy npm-typecheck npm-build arc-recorder-self module-reference-diff diff-check protected-byte-check)
RUNTIME_FINAL_ROOT_LABELS=(exact-head-scope exact-head-scope-equal exact-head-status exact-head-clean evidence-sha evidence-date evidence-os evidence-sha-after evidence-sha-stable exact-head-status-after exact-head-clean-after)
RUNTIME_REQUIRED_NEXT=0; RUNTIME_FINAL_NEXT=0; RUNTIME_GATE_LEDGER="$RUNTIME_LOG_DIR/muse-gates.tsv"; RUNTIME_REQUIRED_LEDGER="$RUNTIME_LOG_DIR/required-root.tsv"; RUNTIME_FINAL_LEDGER="$RUNTIME_LOG_DIR/final-root.tsv"; test ! -e "$RUNTIME_GATE_LEDGER"; test ! -e "$RUNTIME_REQUIRED_LEDGER"; test ! -e "$RUNTIME_FINAL_LEDGER"; : >"$RUNTIME_GATE_LEDGER"; : >"$RUNTIME_REQUIRED_LEDGER"; : >"$RUNTIME_FINAL_LEDGER"
muse_workflow_exit() { local rc=$?; trap - EXIT; test "${RUNTIME_WORKFLOW_STAGE:-}" = complete || rc=1; exit "$rc"; }
run_required_root() { local label="$1"; test "$label" = "${RUNTIME_REQUIRED_ROOT_LABELS[$RUNTIME_REQUIRED_NEXT]:-}"; if test "$RUNTIME_REQUIRED_NEXT" -eq 0; then test "$RUNTIME_WORKFLOW_STAGE" = ready; else test "$RUNTIME_WORKFLOW_STAGE" = precommit-finalized; fi; run_logged "$@"; assert_log_destination "$RUNTIME_LOG_DIR" "$label"; printf '%s\n' "$label" >>"$RUNTIME_REQUIRED_LEDGER"; RUNTIME_REQUIRED_NEXT=$((RUNTIME_REQUIRED_NEXT+1)); }
run_final_root() { local label="$1"; test "$label" = "${RUNTIME_FINAL_ROOT_LABELS[$RUNTIME_FINAL_NEXT]:-}"; if test "$RUNTIME_FINAL_NEXT" -lt 7; then test "$RUNTIME_WORKFLOW_STAGE" = precommit-finalized; test "$RUNTIME_REQUIRED_NEXT" -eq "${#RUNTIME_REQUIRED_ROOT_LABELS[@]}"; else test "$RUNTIME_WORKFLOW_STAGE" = exact-head-finalized; fi; run_logged "$@"; assert_log_destination "$RUNTIME_LOG_DIR" "$label"; printf '%s\n' "$label" >>"$RUNTIME_FINAL_LEDGER"; RUNTIME_FINAL_NEXT=$((RUNTIME_FINAL_NEXT+1)); }
arm_exact_head() { local h="$1"; test "$RUNTIME_WORKFLOW_STAGE" = precommit-finalized; test "$RUNTIME_REQUIRED_NEXT" -eq "${#RUNTIME_REQUIRED_ROOT_LABELS[@]}"; cmp -s "$RUNTIME_REQUIRED_LEDGER" <(printf '%s\n' "${RUNTIME_REQUIRED_ROOT_LABELS[@]}"); test "$RUNTIME_FINAL_NEXT" -eq 7; test "$(git -C "$RUNTIME_REPO_ROOT" rev-parse HEAD)" = "$h"; test ! -s "$RUNTIME_LOG_DIR/exact-head-status.log"; RUNTIME_EXACT_HEAD="$h"; RUNTIME_WORKFLOW_STAGE=exact-head-ready; }
complete_muse_workflow() { local h="$1"; test "$RUNTIME_WORKFLOW_STAGE" = exact-head-finalized; test "$h" = "$RUNTIME_EXACT_HEAD"; test "$RUNTIME_FINAL_NEXT" -eq "${#RUNTIME_FINAL_ROOT_LABELS[@]}"; cmp -s "$RUNTIME_FINAL_LEDGER" <(printf '%s\n' "${RUNTIME_FINAL_ROOT_LABELS[@]}"); mapfile -t gates <"$RUNTIME_GATE_LEDGER"; test "${#gates[@]}" -eq 2; printf '%s\n' "${gates[0]}" | rg -x $'precommit\t'"$h"$'\t[0-9A-F]{64}' >/dev/null; printf '%s\n' "${gates[1]}" | rg -x -- $'exact-head\t'"$h"$'\t[0-9A-F]{64}' >/dev/null; test "$(git -C "$RUNTIME_REPO_ROOT" rev-parse HEAD)" = "$h"; test ! -s "$RUNTIME_LOG_DIR/exact-head-status-after.log"; RUNTIME_WORKFLOW_STAGE=complete; printf 'runtime_workflow=PASS head=%s gates=precommit,exact-head required_root=11 final_root=11\n' "$h"; }
RUNTIME_WORKFLOW_STAGE=ready; trap muse_workflow_exit EXIT
run_required_root rustfmt 30m cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
RUST_TESTS=(
  'session::profile::tests::muse_serde_profile_detection_and_submission_boundary_are_stable' 'commands::session::tests::muse_resume_eligibility_is_exact_and_fail_closed'
  'commands::session::tests::muse_fresh_and_resume_intents_share_one_launch_seam' 'commands::session::tests::muse_manual_collision_args_pass_validation_and_reach_backend_unchanged'
  'commands::session::tests::trusted_muse_resume_reaches_backend_effective_argv_only' 'commands::session::tests::muse_live_root_selection_is_one_fresh_replacement' 'commands::session::tests::muse_spawn_error_is_not_retried_fresh'
  'config::settings::tests::validate_agent_commands_allows_direct_muse_arguments_with_legacy_provider_collision' 'config::settings::tests::validate_agent_commands_rejects_wrapped_muse_collision_before_spawn'
  'config::sessions_persistence::tests::muse_recipes_and_snapshots_preserve_configured_args_not_effective_resume' 'loops::delivery::tests::muse_loop_cold_is_fresh_and_known_state_resumes_without_provider_drift'
  'commands::telegram::tests::derive_reader_rejects_muse_for_all_backends_before_bridge_creation' 'config::agent_command::tests::muse_default_instructions_filename_is_agents_md' 'config::agent_command::tests::muse_explicit_instructions_filename_wins'
  'config::config_seed::tests::muse_has_no_config_dir_warning_or_seed_contract' 'commands::session::tests::muse_post_spawn_nonzero_exit_is_reported_once_without_fresh_fallback'
)
assert_muse_16() {
  local label="$1"; test "$RUNTIME_GATE_STAGE" = target
  MUSE_ASSERT_BODY='
    set -euo pipefail; d="$RUNTIME_GATE_DIR"; label="$1"; head="$2"; shift 2; tests=("$@"); fixed=40DC53784EF025D6336196F95F3565F6F363B10B69165D02B18B4D4F127A2873
    exact8(){ local f="$1" limit="$2" argv start end; shift 2; local -a r; printf -v argv "%q " "$@"; mapfile -t r <"$f"; test "${#r[@]}" -eq 8; cmp -s "$f" <(printf "%s\n" "${r[@]}"); test "${r[0]}" = "timeout_limit=$limit"; test "${r[1]}" = stdin=/dev/null; printf "%s\n" "${r[2]}" | rg -x "start_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z" >/dev/null; printf "%s\n" "${r[3]}" | rg -x "end_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z" >/dev/null; start="${r[2]#start_utc=}"; end="${r[3]#end_utc=}"; [[ "$end" == "$start" || "$end" > "$start" ]]; printf "%s\n" "${r[4]}" | rg -x "elapsed_seconds=(0|[1-9][0-9]*)" >/dev/null; test "${r[5]}" = command_exit=0; test "${r[6]}" = tee_exit=0; test "${r[7]}" = "argv=$argv"; }
    test "${#tests[@]}" -eq 16; test "$(git -C "$RUNTIME_REPO_ROOT" rev-parse HEAD)" = "$head"; list_cmd=(cargo test --manifest-path src-tauri/Cargo.toml --locked --lib muse -- --list --format pretty); exact8 "$d/$label-list.meta" 30m "${list_cmd[@]}"; if rg -q -- "test result:" "$d/$label-list.log"; then exit 1; fi; mapfile -t list_totals < <(rg -x -- "16 tests, 0 benchmarks" "$d/$label-list.log" || true); test "${#list_totals[@]}" -eq 1
    printf "%s\n" "$@" | LC_ALL=C sort >"$d/$label.expected"
    sed -n "s/^\\(.*\\): test$/\\1/p" "$d/$label-list.log" | LC_ALL=C sort >"$d/$label.actual"
    expected_sha="$(sha256sum "$d/$label.expected" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"
    actual_sha="$(sha256sum "$d/$label.actual" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"
    test "$expected_sha" = "$fixed"; test "$(wc -l <"$d/$label.actual")" -eq 16; test "$actual_sha" = "$fixed"; cmp -s "$d/$label.expected" "$d/$label.actual"; : >"$d/$label.executed"; : >"$d/$label.summaries"
    for i in "${!tests[@]}"; do printf -v run "muse-exact-%02d" "$((i+1))"; t="${tests[$i]}"; exact_cmd=(cargo test --manifest-path src-tauri/Cargo.toml --locked --lib "$t" -- --exact --test-threads=1); exact8 "$d/$run.meta" 30m "${exact_cmd[@]}"; mapfile -t markers < <(rg -o -- "test result:" "$d/$run.log" || true); test "${#markers[@]}" -eq 1; mapfile -t rows < <(rg -x -- "test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in .+" "$d/$run.log" || true); test "${#rows[@]}" -eq 1; printf "%s\n" "$t" >>"$d/$label.executed"; printf "%s\t%s\n" "$run" "${rows[0]}" >>"$d/$label.summaries"; done
    LC_ALL=C sort -o "$d/$label.executed" "$d/$label.executed"; executed_sha="$(sha256sum "$d/$label.executed" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"; test "$(wc -l <"$d/$label.executed")" -eq 16; test "$executed_sha" = "$fixed"; cmp -s "$d/$label.expected" "$d/$label.executed"; list_sha="$(sha256sum "$d/$label-list.log" | cut -d" " -f1)"; list_meta_sha="$(sha256sum "$d/$label-list.meta" | cut -d" " -f1)"; summaries_sha="$(sha256sum "$d/$label.summaries" | cut -d" " -f1)"
    printf "assertion=PASS\nhead=%s\nenumeration_log_sha256=%s\nenumeration_meta_sha256=%s\nexpected_count=16\nenumerated_count=16\nexecuted_count=16\nexpected_sha256=%s\nenumerated_sha256=%s\nexecuted_sha256=%s\nsummaries_sha256=%s\n" "$head" "$list_sha" "$list_meta_sha" "$expected_sha" "$actual_sha" "$executed_sha" "$summaries_sha"
  '
  export MUSE_ASSERT_BODY; run_gate_logged "$label-assert" 1m bash -c "$MUSE_ASSERT_BODY" bash "$label" "$RUNTIME_GATE_HEAD" "${RUST_TESTS[@]}"
  RUNTIME_GATE_STAGE=asserted
}
begin_muse_16_gate() { local gate="$1" seq; test -z "${RUNTIME_GATE_STAGE:-}" || test "$RUNTIME_GATE_STAGE" = finalized; case "$gate:$RUNTIME_WORKFLOW_STAGE" in precommit:ready) test "$RUNTIME_REQUIRED_NEXT" -eq 1; cmp -s "$RUNTIME_REQUIRED_LEDGER" <(printf 'rustfmt\n'); seq=1;; exact-head:exact-head-ready) seq=2;; *) return 1;; esac; RUNTIME_GATE_HEAD="$(git rev-parse HEAD)"; RUNTIME_GATE_DIR="$RUNTIME_LOG_DIR/$gate-$RUNTIME_GATE_HEAD"; test ! -e "$RUNTIME_GATE_DIR"; mkdir "$RUNTIME_GATE_DIR"; printf 'version=2\ngate=%s\nsequence=%s\nhead=%s\n' "$gate" "$seq" "$RUNTIME_GATE_HEAD" >"$RUNTIME_GATE_DIR/gate.identity"; export RUNTIME_GATE_DIR RUNTIME_GATE_HEAD; RUNTIME_GATE_STAGE=begun; }
run_muse_16_target() { test "$RUNTIME_GATE_STAGE" = begun; run_gate_logged muse-targeted-list 30m cargo test --manifest-path src-tauri/Cargo.toml --locked --lib muse -- --list --format pretty; local i run; for i in "${!RUST_TESTS[@]}"; do printf -v run "muse-exact-%02d" "$((i+1))"; run_gate_logged "$run" 30m cargo test --manifest-path src-tauri/Cargo.toml --locked --lib "${RUST_TESTS[$i]}" -- --exact --test-threads=1; done; RUNTIME_GATE_STAGE=target; }
seal_muse_16() { local label="$1"
  test "$RUNTIME_GATE_STAGE" = asserted
  MUSE_SEAL_BODY='
    set -euo pipefail; mode="$1"; label="$2"; head="$3"; shift 3; tests=("$@"); d="$RUNTIME_GATE_DIR"; tmp="$(mktemp -d "$d/.seal.XXXXXX")"; fixed=40DC53784EF025D6336196F95F3565F6F363B10B69165D02B18B4D4F127A2873
    exact8(){ local f="$1" limit="$2" argv start end; shift 2; local -a r; printf -v argv "%q " "$@"; mapfile -t r <"$f"; test "${#r[@]}" -eq 8; cmp -s "$f" <(printf "%s\n" "${r[@]}"); test "${r[0]}" = "timeout_limit=$limit"; test "${r[1]}" = stdin=/dev/null; printf "%s\n" "${r[2]}" | rg -x "start_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z" >/dev/null; printf "%s\n" "${r[3]}" | rg -x "end_utc=[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z" >/dev/null; start="${r[2]#start_utc=}"; end="${r[3]#end_utc=}"; [[ "$end" == "$start" || "$end" > "$start" ]]; printf "%s\n" "${r[4]}" | rg -x "elapsed_seconds=(0|[1-9][0-9]*)" >/dev/null; test "${r[5]}" = command_exit=0; test "${r[6]}" = tee_exit=0; test "${r[7]}" = "argv=$argv"; }
    test "${#tests[@]}" -eq 16; test "$(git -C "$RUNTIME_REPO_ROOT" rev-parse HEAD)" = "$head"; mapfile -t identity <"$d/gate.identity"; test "${#identity[@]}" -eq 4; cmp -s "$d/gate.identity" <(printf "%s\n" "${identity[@]}"); test "${identity[0]}" = version=2; case "${identity[1]}:${identity[2]}" in gate=precommit:sequence=1|gate=exact-head:sequence=2) ;; *) exit 1;; esac; test "${identity[3]}" = "head=$head"
    list_cmd=(cargo test --manifest-path src-tauri/Cargo.toml --locked --lib muse -- --list --format pretty); assert_cmd=(bash -c "$MUSE_ASSERT_BODY" bash "$label" "$head" "${tests[@]}"); seal_cmd=(bash -c "$MUSE_SEAL_BODY" bash seal "$label" "$head" "${tests[@]}"); exact8 "$d/$label-list.meta" 30m "${list_cmd[@]}"; exact8 "$d/$label-assert.meta" 1m "${assert_cmd[@]}"; if rg -q -- "test result:" "$d/$label-list.log"; then exit 1; fi; mapfile -t list_totals < <(rg -x -- "16 tests, 0 benchmarks" "$d/$label-list.log" || true); test "${#list_totals[@]}" -eq 1
    printf "%s\n" "${tests[@]}" | LC_ALL=C sort >"$tmp/expected"; sed -n "s/^\\(.*\\): test$/\\1/p" "$d/$label-list.log" | LC_ALL=C sort >"$tmp/actual"; test "$(wc -l <"$tmp/expected")" -eq 16; test "$(wc -l <"$tmp/actual")" -eq 16; expected_sha="$(sha256sum "$tmp/expected" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"; actual_sha="$(sha256sum "$tmp/actual" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"; test "$expected_sha" = "$fixed"; test "$actual_sha" = "$fixed"; cmp -s "$tmp/expected" "$tmp/actual"; : >"$tmp/executed"; : >"$tmp/summaries"; names=(gate.identity "$label-list.log" "$label-list.meta")
    for i in "${!tests[@]}"; do printf -v run "muse-exact-%02d" "$((i+1))"; t="${tests[$i]}"; exact_cmd=(cargo test --manifest-path src-tauri/Cargo.toml --locked --lib "$t" -- --exact --test-threads=1); exact8 "$d/$run.meta" 30m "${exact_cmd[@]}"; mapfile -t markers < <(rg -o -- "test result:" "$d/$run.log" || true); test "${#markers[@]}" -eq 1; mapfile -t rows < <(rg -x -- "test result: ok\\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9]+ filtered out; finished in .+" "$d/$run.log" || true); test "${#rows[@]}" -eq 1; printf "%s\n" "$t" >>"$tmp/executed"; printf "%s\t%s\n" "$run" "${rows[0]}" >>"$tmp/summaries"; names+=("$run.log" "$run.meta"); done
    LC_ALL=C sort -o "$tmp/executed" "$tmp/executed"; executed_sha="$(sha256sum "$tmp/executed" | cut -d" " -f1 | tr "[:lower:]" "[:upper:]")"; test "$(wc -l <"$tmp/executed")" -eq 16; test "$executed_sha" = "$fixed"; cmp -s "$tmp/expected" "$tmp/executed"; list_sha="$(sha256sum "$d/$label-list.log" | cut -d" " -f1)"; list_meta_sha="$(sha256sum "$d/$label-list.meta" | cut -d" " -f1)"; summaries_sha="$(sha256sum "$tmp/summaries" | cut -d" " -f1)"; printf "assertion=PASS\nhead=%s\nenumeration_log_sha256=%s\nenumeration_meta_sha256=%s\nexpected_count=16\nenumerated_count=16\nexecuted_count=16\nexpected_sha256=%s\nenumerated_sha256=%s\nexecuted_sha256=%s\nsummaries_sha256=%s\n" "$head" "$list_sha" "$list_meta_sha" "$expected_sha" "$actual_sha" "$executed_sha" "$summaries_sha" >"$tmp/assert.log"
    cmp -s "$d/$label.expected" "$tmp/expected"; cmp -s "$d/$label.actual" "$tmp/actual"; cmp -s "$d/$label.executed" "$tmp/executed"; cmp -s "$d/$label.summaries" "$tmp/summaries"; cmp -s "$d/$label-assert.log" "$tmp/assert.log"; names+=("$label.expected" "$label.actual" "$label.executed" "$label.summaries" "$label-assert.log" "$label-assert.meta"); test "${#names[@]}" -eq 41; { printf "version=2\nhead=%s\nenumeration_log_sha256=%s\nenumeration_meta_sha256=%s\n" "$head" "$list_sha" "$list_meta_sha"; (cd "$d" && sha256sum "${names[@]}"); } >"$tmp/manifest"
    if test "$mode" = seal; then test ! -e "$d/$label-assertion.manifest"; mv "$tmp/manifest" "$d/$label-assertion.manifest"; printf "seal=PASS\nhead=%s\nenumeration_log_sha256=%s\nmanifest_sha256=%s\n" "$head" "$list_sha" "$(sha256sum "$d/$label-assertion.manifest" | cut -d" " -f1)"; else test "$mode" = verify; cmp -s "$d/$label-assertion.manifest" "$tmp/manifest"; exact8 "$d/$label-seal.meta" 1m "${seal_cmd[@]}"; printf "seal=PASS\nhead=%s\nenumeration_log_sha256=%s\nmanifest_sha256=%s\n" "$head" "$list_sha" "$(sha256sum "$tmp/manifest" | cut -d" " -f1)" >"$tmp/seal.log"; cmp -s "$d/$label-seal.log" "$tmp/seal.log"; { printf "version=2\nhead=%s\n" "$head"; (cd "$d" && sha256sum "$label-assertion.manifest" "$label-seal.log" "$label-seal.meta"); } >"$tmp/final.manifest"; if test -e "$d/$label-final.manifest"; then cmp -s "$d/$label-final.manifest" "$tmp/final.manifest"; else mv "$tmp/final.manifest" "$d/$label-final.manifest"; fi; printf "finalizer=PASS head=%s target_tests=16 manifest_sha256=%s\n" "$head" "$(sha256sum "$d/$label-final.manifest" | cut -d" " -f1)"; fi
  '
  export MUSE_SEAL_BODY; run_gate_logged "$label-seal" 1m bash -c "$MUSE_SEAL_BODY" bash seal "$label" "$RUNTIME_GATE_HEAD" "${RUST_TESTS[@]}"; RUNTIME_GATE_STAGE=sealed
}
finalize_muse_16_gate() { test "$RUNTIME_GATE_STAGE" = sealed; local d="$RUNTIME_GATE_DIR" h="$RUNTIME_GATE_HEAD" b="$RUNTIME_GATE_DIR/muse-16-evidence.tar.gz" x pre post gate i run; timeout 1m bash -c "$MUSE_SEAL_BODY" bash verify muse-targeted "$h" "${RUST_TESTS[@]}" >"$d/muse-targeted-finalizer.log"; local -a names=(gate.identity muse-targeted-list.log muse-targeted-list.meta); for i in "${!RUST_TESTS[@]}"; do printf -v run "muse-exact-%02d" "$((i+1))"; names+=("$run.log" "$run.meta"); done; names+=(muse-targeted.expected muse-targeted.actual muse-targeted.executed muse-targeted.summaries muse-targeted-assert.log muse-targeted-assert.meta muse-targeted-assertion.manifest muse-targeted-seal.log muse-targeted-seal.meta muse-targeted-final.manifest); test "${#names[@]}" -eq 45; tar -C "$d" -czf "$b" "${names[@]}"; pre="$(sha256sum "$b" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')"; cmp -s <(printf '%s\n' "${names[@]}") <(tar -tzf "$b"); LC_ALL=C tar --numeric-owner -tvzf "$b" >"$d/members.types"; test "$(wc -l <"$d/members.types")" -eq 45; awk 'substr($1,1,1)!="-"{exit 1}' "$d/members.types"; timeout 1m bash -c "$MUSE_SEAL_BODY" bash verify muse-targeted "$h" "${RUST_TESTS[@]}" >/dev/null; x="$(mktemp -d "$AGENTSCOMMANDER_ROOT/.evidence/ac-1873-receiver.XXXXXX")"; case "$x" in "$AGENTSCOMMANDER_ROOT"/*) ;; *) return 1;; esac; tar --no-same-owner --no-same-permissions -xzf "$b" -C "$x"; post="$(sha256sum "$b" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')"; test "$post" = "$pre"; (export RUNTIME_GATE_DIR="$x"; timeout 1m bash -c "$MUSE_SEAL_BODY" bash verify muse-targeted "$h" "${RUST_TESTS[@]}") >"$d/muse-targeted-receiver.log"; rg -x 'finalizer=PASS head=[0-9a-f]{40} target_tests=16 manifest_sha256=[0-9a-f]{64}' "$d/muse-targeted-receiver.log"; test "$(git -C "$RUNTIME_REPO_ROOT" rev-parse HEAD)" = "$h"; gate="$(sed -n '2s/^gate=//p' "$d/gate.identity")"; case "$gate:$RUNTIME_WORKFLOW_STAGE" in precommit:ready) RUNTIME_PRECOMMIT_GATE_DIR="$d"; RUNTIME_WORKFLOW_STAGE=precommit-finalized;; exact-head:exact-head-ready) RUNTIME_EXACT_HEAD_GATE_DIR="$d"; RUNTIME_WORKFLOW_STAGE=exact-head-finalized;; *) return 1;; esac; printf '%s\t%s\t%s\n' "$gate" "$h" "$post" >>"$RUNTIME_GATE_LEDGER"; RUNTIME_GATE_BUNDLE_SHA256="$post"; RUNTIME_GATE_STAGE=finalized; unset RUNTIME_GATE_DIR RUNTIME_GATE_HEAD; }
begin_muse_16_gate precommit
run_muse_16_target
assert_muse_16 muse-targeted
seal_muse_16 muse-targeted; finalize_muse_16_gate
run_required_root rust-lib 30m cargo test --manifest-path src-tauri/Cargo.toml --locked --lib -- --nocapture
rg -n -- '^test result: ok\. [1-9][0-9]* passed; 0 failed' "$RUNTIME_LOG_DIR/rust-lib.log"
run_required_root layering 30m cargo test --manifest-path src-tauri/Cargo.toml --locked --test loops_layering --test instance_gitignore_layering --test project_settings_layering -- --nocapture
test "$(rg -c -- '^test result: ok\. [1-9][0-9]* passed; 0 failed' "$RUNTIME_LOG_DIR/layering.log")" -eq 3
run_required_root cargo-check 30m cargo check --manifest-path src-tauri/Cargo.toml --locked --all-targets
run_required_root clippy 30m cargo clippy --manifest-path src-tauri/Cargo.toml --locked --workspace --all-targets -- -D warnings
for task in typecheck build; do
  run_required_root "npm-$task" 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" run "$task"
done
run_required_root arc-recorder-self 15m env PATH="$NODE_BIN:$PATH" TMPDIR="$TOOL_ROOT/tmp" NPM_CONFIG_CACHE="$TOOL_ROOT/cache" NPM_CONFIG_USERCONFIG="$TOOL_ROOT/npmrc" "$NODE22" "$NPM11" run record:arcs:self
rg -n -- '^\[arc-record\] self-test passed: [1-9][0-9]* cases$' "$RUNTIME_LOG_DIR/arc-recorder-self.log"
run_required_root module-reference-diff 1m git diff --unified=0 "$PHASE_BASE_SHA" -- "${RUNTIME_PATHS[@]}"
ARC_RECORD_SHA_AFTER="$(sha256sum src-tauri/module-arcs.txt | cut -d' ' -f1)"
printf 'before=%s\nafter=%s\n' "$ARC_RECORD_SHA_BEFORE" "$ARC_RECORD_SHA_AFTER" | tee "$RUNTIME_LOG_DIR/arc-record-after.meta"
test "$ARC_RECORD_SHA_AFTER" = "$ARC_RECORD_SHA_BEFORE"
run_required_root diff-check 1m git diff --check
run_required_root protected-byte-check 1m git diff --exit-code -- src-tauri/module-arcs.txt package.json package-lock.json Cargo.lock src-tauri/Cargo.toml .github/workflows
~~~

The one postcommit EXIT-enforced workflow is root rustfmt→precommit target/assert/seal/finalize→ten more required root checks→seven exact-head checks→exact-head target/assert/seal/finalize→four final checks→completion. The legacy `precommit` gate name denotes the first candidate pass: both gates bind the same committed HEAD. Each gate requires a test-body-free `--format pretty` list with the exact 16-name multiset and fixed C-sort SHA `40DC53784EF025D6336196F95F3565F6F363B10B69165D02B18B4D4F127A2873`, then 16 full-name `--exact` logs/metas with exactly one total `test result:` occurrence and one 1-pass/0-fail/0-ignore/0-measure row apiece. Every extra/missing/duplicate identity, unexpected summary, zero selection, ambiguous command, forged row, omission/reorder/reuse/swap, metadata/log leak, incomplete precommit-only execution, or missing final HEAD/status check returns nonzero. Assertion, seal, finalizer, extracted receiver, and lead execution rederive the exact 45-regular-member evidence.

Producer bundles are diagnostic, never an execution root. Feed them through #1860's apply-patch materialization and exact single-receipt parser; the tech lead independently runs the frozen executor below, then emits the canonical notification. The receiver requires that lead-owned executor/log attestation, both ordered gate bundles, and stable final HEAD/status; producer-only hashes, PASS rows, or private paths cannot authorize handoff.

The synthetic harness has one complete two-gate positive; fresh clones must return nonzero for omitted target/assertion/seal/gate/final check, gate swap/reuse, reduced/extra/duplicate sets, zero-selection, PASS→FAIL, forged counts/digests/summary or metadata, swapped UTC rows, wrong log destination, fake/no Cargo invocation, precommit-only exit, and post-assert/seal/bundle/shard/HEAD mutation. Its named stdout-forgery cold negative defines only the real effective-argv test, which emits the other 15 identities and a fake 16-pass summary before Cargo's real 1-pass summary; material assertion, seal, finalizer, extracted receiver, and full lead-executor variants must each return nonzero.

The arc recorder self-tests its CLI only. Exact diff plus equal arc SHA is the fast path; any other reference/pair, zero/failing test, timeout, debt, or drift blocks.

## Grinch proof

Owner: AgentsCommander_iac:room-9-ac-dev-team-v4/ac-dev-rust-grinch-v4 after a green candidate and before acceptance.

1. Record missing successful vendor evidence without running Muse; review the effective-argv positive control and successful-spawn/nonzero-exit test log. Synthetic argv never proves vendor continuation.
2. Hash session/profile.rs; change only MUSE_PROFILE's --last token to --first and hash the mutant.
3. In its own below-root log dir, use the same 30-minute runner contract for only trusted_muse_resume_reaches_backend_effective_argv_only; require a retained nonzero failure naming it, never zero selection/unrelated build failure.
4. Require the mutant hash before recovery; restore only that token, require the candidate hash, rerun the same bounded/logged exact test, and require one pass.

This mutation proves the positive control detects a wrong effective selector; it does not replace the full lifecycle and boundary suite.

## Dependency-cycle and layering gate

Planned new module arcs: zero. Planned removed module arcs: zero.

The one new cross-module call is:

~~~text
src-tauri/src/loops/delivery.rs -> crate::commands::session::trusted_muse_auto_resume_spawn
~~~

The base arc record already contains agentscommander_lib::loops::delivery -> agentscommander_lib::commands::session because spawn_coordinator_session calls create_session_inner, so the new call stays inside an existing module arc. `commands::session` tests already call `pty::spawn_diagnostics::record_for`; new assertions reuse that test dependency. The TS union adds no import; other edits are same-file or reuse relationships; settings.rs already imports CodingAgentKind. No lower layer gains Tauri, AppHandle, or UI transport.

The arc record retains its exact pre-edit SHA and all three layering targets pass. Bare `record:arcs` is forbidden because it requires `--graph`; `record:arcs:self` only self-tests the recorder and neither accepts a graph nor regenerates the record. The fast path also requires the retained diff to enumerate zero new module pairs. Any new import/use/mod/re-export/declaration or unclassified qualified call triggers the slow path.

On that trigger, stop without hiding the candidate. The coordinator sends PHASE_BASE_SHA, clean candidate SHA, exact diff, and trigger output to ac-tech-lead-v4, the authorized `rust-levelization-run` owner. Its immutable clean-base/candidate record contains `coverage.graphShape.cyclicSccs`, sorted cyclic-SCC member sets, every added/removed and cross-boundary pair, regenerated arc SHA/byte comparison, and three layering exits. Green requires unchanged cyclicSccs, identical member sets, zero new cross-boundary pairs, byte-identical arc record, and all guards passing. Missing output, exit 3, dirty inputs, or mismatch blocks; the architect approves before continuation.

## Commit, CI, scope, and recovery

Commit only the ten owned files and require a clean final tree; against PHASE_BASE_SHA the branch delta is exactly those paths. Plans, #1860, other frontend, docs, manifests, locks, workflows, generated files, module-arcs.txt, version 0.30.5, and release state remain unchanged. The commit contains profile.rs and types.ts together; never hand off/merge a one-sided wire head.

Commit after the separate feedback shell exits. Then start bodies 1–3 from body 1 in a fresh noninteractive shell; this body 3 continues that same clean-candidate execution without an intervening commit. Prior feedback evidence is retired, never carried forward. Producer output stays diagnostic; only independent lead execution is authoritative:

~~~bash
printf '%s\n' "${RUNTIME_PATHS[@]}" >"$RUNTIME_LOG_DIR/exact-head-scope.expected"
run_final_root exact-head-scope 1m git diff --name-only "$PHASE_BASE_SHA" HEAD --
run_final_root exact-head-scope-equal 1m cmp -s "$RUNTIME_LOG_DIR/exact-head-scope.expected" "$RUNTIME_LOG_DIR/exact-head-scope.log"
run_final_root exact-head-status 1m git status --porcelain --untracked-files=normal
run_final_root exact-head-clean 1m test ! -s "$RUNTIME_LOG_DIR/exact-head-status.log"
run_final_root evidence-sha 1m git rev-parse HEAD
run_final_root evidence-date 1m date -u +%Y-%m-%d
run_final_root evidence-os 1m uname -srm
IFS= read -r RUNTIME_EVIDENCE_SHA <"$RUNTIME_LOG_DIR/evidence-sha.log"
IFS= read -r RUNTIME_EVIDENCE_DATE_UTC <"$RUNTIME_LOG_DIR/evidence-date.log"
IFS= read -r RUNTIME_EVIDENCE_OS <"$RUNTIME_LOG_DIR/evidence-os.log"
case "$RUNTIME_EVIDENCE_OS" in Linux*|Darwin*) ;; *) exit 1;; esac
arm_exact_head "$RUNTIME_EVIDENCE_SHA"
begin_muse_16_gate exact-head; test "$RUNTIME_GATE_HEAD" = "$RUNTIME_EVIDENCE_SHA"
run_muse_16_target
assert_muse_16 muse-targeted
seal_muse_16 muse-targeted; finalize_muse_16_gate
run_final_root evidence-sha-after 1m git rev-parse HEAD
run_final_root evidence-sha-stable 1m cmp -s "$RUNTIME_LOG_DIR/evidence-sha.log" "$RUNTIME_LOG_DIR/evidence-sha-after.log"
run_final_root exact-head-status-after 1m git status --porcelain --untracked-files=normal
run_final_root exact-head-clean-after 1m test ! -s "$RUNTIME_LOG_DIR/exact-head-status-after.log"
complete_muse_workflow "$RUNTIME_EVIDENCE_SHA"
~~~

### Lead-owned execution root

At the exact clean candidate HEAD, the tech lead—not the producer—extracts the first three frozen Bash bodies, requires their combined SHA, and executes them from a fresh noninteractive shell. Its canonical attestation includes the executor/log hashes, both 16-list/16-exact gates, and the one exact workflow PASS row; producer bundles without this separate execution are untrusted.

~~~bash
set -euo pipefail
checkout_fingerprint() { local h b cfg rc digest; git reflog exists HEAD || return 1; git reflog exists "$LEAD_BRANCH_REF" || return 1; if cfg="$(git config --get core.logAllRefUpdates)"; then test "$cfg" != false || return 1; else rc=$?; test "$rc" -eq 1 || return 1; fi; h="$(git reflog show --format='%H %gD %gs' HEAD)" || return 1; b="$(git reflog show --format='%H %gD %gs' "$LEAD_BRANCH_REF")" || return 1; digest="$(set -o pipefail; printf '%s\n%s\n' "$h" "$b" | sha256sum)" || return 1; [[ "$digest" =~ ^[0-9a-f]{64}'  -'$ ]] || return 1; printf '%s\n' "${digest%% *}"; }
assert_checkout() { local h ref b fingerprint; h="$(git rev-parse --verify 'HEAD^{commit}')" || exit 1; ref="$(git symbolic-ref -q HEAD)" || exit 1; b="$(git rev-parse --verify "$LEAD_BRANCH_REF^{commit}")" || exit 1; fingerprint="$(checkout_fingerprint)" || exit 1; test "$h" = "$LEAD_CANDIDATE_SHA" && test "$ref" = "$LEAD_BRANCH_REF" && test "$b" = "$LEAD_CANDIDATE_SHA" && test "$fingerprint" = "$LEAD_CHECKOUT_SHA256" || exit 1; }
bind_checkout() { : "${LEAD_CANDIDATE_SHA:?}"; LEAD_BRANCH_REF="$(git symbolic-ref -q HEAD)" || exit 1; LEAD_CHECKOUT_SHA256="$(checkout_fingerprint)" || exit 1; export LEAD_CANDIDATE_SHA LEAD_BRANCH_REF LEAD_CHECKOUT_SHA256; assert_checkout; }
LEAD_CANDIDATE_SHA="${EXPECTED_RUNTIME_HEAD:?}"; bind_checkout
test -n "${LEAD_DIR:-}"; case "$LEAD_DIR" in "$AGENTSCOMMANDER_ROOT"/*) ;; *) exit 1;; esac; mkdir "$LEAD_DIR/runtime"; plan=plans/1854-muse-code-beta-preset/1873-muse-auto-resume.md; executor="$LEAD_DIR/runtime/executor.sh"; awk '/^~~~bash$/{b++; inside=(b<=3); next} /^~~~$/{inside=0; next} inside{print}' "$plan" >"$executor"; executor_sha="$(sha256sum "$executor"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"; test "$executor_sha" = E5E8E53306470DC4DBBC46DE73C86CD3B2B01A22DB7183A2C67B6EEA8102BFA9; test "$(git rev-parse HEAD)" = "$EXPECTED_RUNTIME_HEAD"; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; command -v cargo bash timeout >/dev/null; set +e; PHASE_BASE_SHA="$EXPECTED_PHASE_BASE_SHA" env -u BASH_ENV -u ENV bash --noprofile --norc "$executor" </dev/null 2>&1 | tee "$LEAD_DIR/runtime/executor.log"; outer_status=("${PIPESTATUS[@]}"); set -e; for value in "${outer_status[@]}"; do test "$value" -eq 0 || exit 1; done; assert_checkout; rg -x -- "runtime_workflow=PASS head=$EXPECTED_RUNTIME_HEAD gates=precommit,exact-head required_root=11 final_root=11" "$LEAD_DIR/runtime/executor.log"; test "$(git rev-parse HEAD)" = "$EXPECTED_RUNTIME_HEAD"; status_output="$(git status --porcelain --untracked-files=normal)" || exit 1; test -z "$status_output" || exit 1; assert_checkout; printf 'runtime_executor=PASS head=%s executor_sha256=%s log_sha256=%s\n' "$EXPECTED_RUNTIME_HEAD" "$executor_sha" "$(sha256sum "$LEAD_DIR/runtime/executor.log"|cut -d' ' -f1|tr '[:lower:]' '[:upper:]')"
~~~

The exact-head gate rebuilds and binds its 45-member list/execution evidence to `RUNTIME_EVIDENCE_SHA`, reports 16 independently executed passes, then checks stable SHA/clean status. Missing gate/assert/seal, artifact, verifier, or link blocks; the #1861 handoff embeds every value/hash, including the cfg nonzero test.

Retain lead candidate/ref/complete HEAD-and-branch-history anchors through final state, before/after CI lookup and delivery; assert_checkout guards each boundary. CI/delivery equal LEAD_CANDIDATE_SHA; new candidates discard all evidence. Persistent movement, checkout/detach H1→H2→H1 during any child, and outer-tee failure must reject.
On the exact #1873 PR-head SHA require every triggered/configured-required check: test-debt; Windows Rust check/clippy/full tests; Linux Rust check/clippy/configured test; macOS Rust check/clippy; rust-fmt; four portable terminal legs; Windows release CLI smoke; frontend regression; validate-branch-name. Lockfile-drift passes its detector and skips regeneration because package inputs do not change; bundle-validation/version-sync are path-inapplicable. Re-derive after relevant base/workflow/diff drift. Another SHA, waiver, bypass, or unexplained skip fails.

Recovery is path-scoped and compare-before-restore: restore phase bytes only while current hashes equal recorded output; preserve external edits and report conflicts. Broad reset/checkout/restore/clean is forbidden. If arc-record bytes change, retain diagnostics and stop for the cycle gate.

## Acceptance criteria

1. Muse is a serde-stable first-class kind with exact profile/default idle; Rust serialization and TypeScript union land atomically.
2. Only a trusted, exact, empty-argv, local macOS/Linux Muse recipe receives resume --last, only for resume intent.
3. Every lifecycle row matches; other providers are unchanged and live reuse never spawns.
4. The backend positive proves effective resume argv while configured/persisted argv stays unchanged.
5. Manual forms round-trip byte-for-byte; a spawn `Err` removes its row and requires a later user launch/create, while a retained post-spawn failure permits user-selected fresh Restart Session; each surfaces once and never retries automatically.
6. AGENTS.md is the instruction target; the built-in adds no seed/factory or automatic Muse credential flow, while user-configured `configSeed` and generic agent/profile env remain provider-neutral; context, Telegram, logical/privileged PTY, self-clear, Windows/container resume, version, install/auth/update remain unsupported.
7. Harness-owned exact enumeration, 16 independent exact passes, full/layering tests, format/check/clippy, Grinch mutation, byte-identical arc record, exact ten-file scope, and exact-head CI pass.

## Preserve list

- Claude, Codex, Antigravity, and Pi detection, resume, stripping, idle, Telegram, context, logical-command, and fresh-boundary behavior.
- Configured shell/args bytes, manual Muse arguments, durable fresh intent, selection/event order, rollback, and diagnostics.
- Byte-identical session/manager.rs and pty/inject.rs; Muse stays outside privileged/logical PTY through unchanged PtySubmissionAgent.
- No Muse UUID persistence, history preflight, error-text classifier, automatic retry, context watcher, built-in seed/factory, automatic Muse credential flow, container, version probe, install/auth/update, logical command, privileged PTY, or Telegram support beyond exact pre-attach rejection; preserve provider-neutral configured seed/env behavior.
- All files outside the exact ten-file set.

Status: READY_FOR_IMPLEMENTATION
