# #1938 — serialize local config writes across processes

Status: architect candidate for review. Child #1938. Parent #1937. Class design-bearing. Owner ac-dev-rust-v4. Depends on no earlier phase. Contract: persistence IO only. Three modified files; zero added/deleted files:
- src-tauri/src/config/local_config_io.rs
- src-tauri/src/commands/ac_discovery.rs
- .github/workflows/pr-regression-gates.yml

## Objective and implementation

Prevent concurrent app and CLI read-modify-publish from losing unrelated JSON fields. Existing update_config_json_object is the single JSON-object mutation funnel. Keep its public signature and preserve existing atomic publish/retry semantics. No selection UI/API or new on-disk selection fields in this phase.

In local_config_io add private ConfigFileWriteLock holding std::fs::File. Acquire after process-local write mutex and parent creation, before file existence/read/parse; retain through closure, temp sync and atomic publish. Canonicalize existing parent, append `.<filename>.lock`; config.json -> .config.json.lock; project-settings.json -> .project-settings.json.lock. Use read/write/create without truncate, regular non-symlink validation, try_lock polling every 50ms, deadline 5s. On WouldBlock poll; on other errors stop; timeout returns distinct configLockTimeout diagnostic with path and elapsed limit. Never delete sidecar after unlock, including error recovery; dropping its handle releases OS lock and process exit does likewise. Never lock replaceable config inode. Reuse existing PID-specific temporary publication path and Windows ReplaceFileW/Unix rename retries.

All readers/writers inside update_config_json_object occur under the guard. Do not move read before it. Whole-value write callers still use the funnel; #1939 replaces destructive replica behavior. write_file_atomic currently writes non-JSON artifacts; retain existing behavior and prove no replica-config callers bypass the JSON funnel. Expand existing agent_replica_root_config_writes_go_through_shared_helper guard coverage to CLI/phone/web roots only for actual replica/local-config writers; report unexpected matches, never blanket whitelist them. Static scan supplements documented call-site audit, not exhaustive proof.

In ensure_ac_root_gitignore add exact narrow lines `**/.config.json.lock` and `/.project-settings.json.lock`, preserving original bytes and idempotent newline behavior per existing team-config-lock tests. Matrix config.json remains trackable. Runtime sidecars are ignored; temporary file patterns remain unchanged. Do not add lower-layer imports: local_config_io keeps only std/serde/existing dependencies, no path_identity/commands/AppHandle.

Regular-path validation is proportionate, not hostile-host attestation. Directory aliases canonicalize to one physical sidecar. Never truncate or write lock contents. A malicious external process replacing/unlinking the sidecar or older binaries ignoring it are outside cooperative guarantee; document this boundary. Network filesystems without compatible lock support fail visibly, never silently fall back to process-only exclusion.

## Required tests and CI

Add in-file tests named issue_1937_config_lock_process_exclusion, issue_1937_config_lock_process_release, issue_1937_config_lock_timeout, plus other issue_1937_config_lock-prefixed cases: process exclusion across separate processes, process-death release, same-process separate handles, canonical/raw/verbatim/Windows case aliases, 5s timeout using injectable shorter test duration, malformed JSON preserved, closure failure preserved, atomic publish failure preserves prior bytes, no deletion of sidecar, concurrent disjoint JSON updates both survive. Use re-executed Rust test binary with explicit test-only child environment and a temporary directory inside test artifact root; never app session/token credentials. Parent must bound child lifetime and assert named child/parent success, not merely exit zero. Add exact narrow gitignore assertions and idempotence tests alongside existing ac_discovery tests.

Workflow edit: in rust-regression-linux and rust-regression-macos add one shell:bash step, working-directory:src-tauri, timeout-minutes:10, after cargo check/clippy:
```bash
set -euo pipefail
cargo test --locked --lib issue_1937_config_lock -- --test-threads=1 --nocapture 2>&1 | tee issue-1937-config-lock.log
grep -qE "^test result: ok\. [1-9][0-9]* passed; 0 failed" issue-1937-config-lock.log
```
After the result check, require grep -qF for each exact identifier issue_1937_config_lock_process_exclusion, issue_1937_config_lock_process_release and issue_1937_config_lock_timeout; a missing match fails the step. Windows full `cargo test --lib --bins --tests` already executes these; add explicit diagnostic assertions locally. Existing platform/job behavior and #1850 controls remain intact.

From src-tauri: `cargo test --locked --lib issue_1937_config_lock -- --test-threads=1 --nocapture`; `cargo test --locked --lib ensure_ac_root_gitignore`; `cargo fmt --all -- --check`. All new tests positive count, no unexplained failure. Full check/clippy and exact-head platform CI required before merge.

Green boundary: all old config writes behave as before except bounded concurrent exclusion/errors; no lock UX exists. New module arcs zero; generated module-arcs remains byte-identical. Do not implement #1939/#1940 in this phase. Owner hands local_config_io behavior and ac_discovery file back only after this phase lands.

## Per-phase delivery contract (binding)

Planning reference base 7aef1d14e6bb0255b1430614c078145335dd54a4, original issue branch feature/1937-selection-locks. This phase implements only after listed earlier phases have landed. Coordinator pins synchronized current main as actual phase base and creates `feature/1938-config-write-locks` in authorized repo-AgentsCommander; record exact SHA/branch/index/tracked/untracked before edits. Never assume a predecessor branch is landed. Coordinator classifies fetched drift before first edit and before PR create/update; refresh only semantically affected source/toolchain/format/workflow evidence, not unrelated movement. Open real child issue, matching frozen digest votes, and coordinator implementation gate are preconditions.

Threat model routine application feature. Concurrency/persistence/lifecycle controls are task-required; executable/DLL attestation, hostile-host hardening, release signing, and arbitrary install changes are not applicable. Never edit another agent memory/root or task state. Work from verified authorized repo cwd; Bash through configured Windows Git Bash as session requires. Keep build outputs in repository ignored target/dist/node_modules, diagnostics in room-shared/issue-1937. Record meaningful versions and relevant inherited CARGO_TARGET_DIR/RUSTFLAGS/BUILD_PROFILE; no credential dumps. CI uses Node22/npm11.6.2/stable Rust, local observed Node24/npm11.6.2/Rust1.97.1. Use npm ci/cargo --locked when resolving; do not change lockfiles/toolchain to suppress failures.

Immediately before mutation compare selected paths/head/index with frozen record. Preserve scoped originals. On failure restore only this run output still equal to recorded written bytes; preserve external edits and report conflict. No broad reset/restore/cleanup. End with `git diff --check`, `git diff --name-only`, `git diff --cached --name-only`, `git status --short --untracked-files=normal` and explicit lockfile/config/workflow diff. Unexpected scope fails; update plan/review before continuing. Failed, timed-out, cancelled or zero-selected tests never pass. Use shell timeout (local focused tests600s, check/clippy1800s; stop and report timeout) and retain stdout/stderr/exit. No live session/screenshot/input tests without user authorization.

Rust phases: on clean actual phase base and clean final candidate, from repo root run the exact instrument with distinct base/post artifact names:
```bash
node "../repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph "../room-shared/issue-1937/phase-post.json" --json > "../room-shared/issue-1937/phase-post-report.json"
node "../repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/Levelization/02-levelize.mjs" rank "../room-shared/issue-1937/phase-post.json" > "../room-shared/issue-1937/phase-post-levels.json"
node scripts/02-module-arc-record.mjs --graph "../room-shared/issue-1937/phase-post.json" --out "../room-shared/issue-1937/phase-post-arcs.txt"
cmp src-tauri/module-arcs.txt "../room-shared/issue-1937/phase-post-arcs.txt"
```
Use child-number-specific artifact subdirectory for each phase; never overwrite prior baseline. Detector exit1 with valid graph means existing cycle; exit2/3 fails measurement. Compare cyclicSccs and exact sorted SCC member sets; compare unique from/to arc differences (not findings); each new arc must be inside a pre-existing cyclic SCC. #1939 alone permits entity_creation->coding_agent_profiles; all other phases zero added/removed Rust arcs. Require arc-record bytes exactly equal generated final graph and layering tests green: from src-tauri `cargo test --locked --test loops_layering --test instance_gitignore_layering --test project_settings_layering`. Actual post code measurement belongs implementation/reviewer gate; a planning projection is not post proof.

Coordinator/shipper refresh configured rules and require all triggered jobs plus13 configured checks on exact PR head: validate-branch-name, lockfile-drift, rust-regression, frontend-regression, rust-regression-linux, rust-regression-macos, terminal-snapshot-portable (windows-latest/ubuntu-latest/macos-15/macos-15-intel), windows-release-cli-smoke, test-debt, rust-fmt. Regression workflow applies every PR; additional Linux release parity and #1850 platform jobs also must pass. Conditional version/bundle jobs apply if their configured paths change; inspect actual final diff. Existing #480 frontend allowance only exact committed classifier result. Any unexplained red/skipped required check blocks merge; no bypass/direct main push. Final merged-main build after last phase, no install. Child acceptance includes behavior tests plus these delivery conditions, not just compilation.
