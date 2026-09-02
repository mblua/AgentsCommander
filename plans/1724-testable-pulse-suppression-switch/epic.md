# Epic Plan #1724: Testability-gated switch to suppress the sidebar layout pulse

Author: ac-architect-v4, room-4-ac-dev-team-v4, 2026-09-02 UTC. Full `code-implementation-workflow` path, Round 2 candidate.
Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1724](https://github.com/mblua/AgentsCommander/issues/1724), "Add a testability-gated switch to suppress the sidebar layout pulse (unblocks #1656)"
Delivery path: Full.
PARTITION: 3 phases (cut by Owner: Rust backend, then frontend, then docs).

## 1. Objective and problem statement

`runPulse` in `src/main/App.tsx` nudges the main splitter by 16 px on every terminal attach that carries history, forcing a reflow so the TUI repaints. It is a workaround for an unfixed attach-seed fidelity gap tracked in #1656. There is no way to turn it off, which blocks #1656 twice: its discriminating experiment cannot be observed while the pulse masks the result, and its final validation needs the same suppression. The switch was originally inside #1656, which made that issue circular, so it was extracted here.

Objective: deliver a switch that suppresses the pulse, gated so it can never affect production behavior, and whose suppressed state is directly observable through the `ui-terminal` automation query.

Observability is the hard part and is not negotiable. The pulse already fails to move the divider on five pre-existing silent paths: `clamped`, `persistence_owned`, `busy`, `dragging`, and the case where no request is ever made because `snapshot.data.length === 0`. A switch whose only evidence is "the divider did not move" cannot be told apart from any of those, so it would prove nothing. This plan therefore makes suppression produce a distinct, first-class pulse trace that reaches `ui-terminal`.

## 2. Pinned base and delivery authority

- Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-AgentsCommander`
- Target branch: `feature/1724-testable-pulse-suppression-switch`, branched from `main` at `1e57aa581de4c4fd18590cdf0652d8bf60b18a4f` (`1e57aa5`).
- Pinned base SHA: `1e57aa581de4c4fd18590cdf0652d8bf60b18a4f`. Every line number in this epic and in the phase files was re-read against that SHA for Round 2, and every anchor that had drifted in the Round 1 draft was corrected. Line numbers remain a reading aid, not the contract: where an anchor and the quoted code disagree, the quoted code governs and the implementer locates it by content.
- Clean tree precondition: `git status --porcelain` empty before the first product mutation of each phase.
- Accepted task class: **routine application-code change**. Accepted threat model: repository-pinned toolchain, standard resolved tool invocation, scoped local validation, and all triggered GitHub checks green on the exact PR-head SHA. No release, signing, packaging-provenance, untrusted-host, destructive-migration, or security-boundary change is involved, so every enhanced control in `delivery-nonfunctional-invariants` is explicitly non-applicable (see section 8).

## 3. Decided solution

**A process-lifetime boolean, resolved once at startup behind the existing two-part UI-automation gate, read once by the main window, and checked at the top of `runPulse`, where it completes the pulse as `status: "skipped"`, `reason: "suppressed"`.**

The switch is on when all three hold:

1. UI automation is enabled for this process (`--ui-automation` or `AC_UI_AUTOMATION=1`, already resolved by `resolve_enabled_from_cli_or_env`, `src-tauri/src/testability/ui_automation.rs:1467`);
2. the running binary is named `agentscommander_testeable.exe` (`TESTABLE_EXE_NAME`, `src-tauri/src/testability/window_placement.rs:4`);
3. the environment variable `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE` is exactly `1`.

Conditions 1 and 2 are the same gate the UI automation bridge uses. Condition 3 is the opt-in. Any other combination resolves to `false` silently; the value is then immutable for the life of the process.

### 3.1 Why this shape, and why the alternatives are rejected

**Candidate 1, a bridge operation toggling suppression at runtime: rejected.**
A runtime toggle can land at any point of a pulse's lifecycle, and `runPulse` is a multi-leg state machine that owns a temporary sidebar width across two awaited legs plus a 200 ms dwell (`src/main/App.tsx:486-681`). A toggle arriving after `setSidebarWidth(nudgedWidth)` (`App.tsx:554`) and before the restore leg would have to either abandon the pulse mid-flight, leaving the divider at the nudged width and the sidebar in a state the plan would then have to define and test, or be deferred to the next request, in which case it is not a runtime toggle at all. It also costs a new `UiAutomationAction` variant, which is compile-forced through `next_variant` and `action_wire_name` and pinned by a Rust/TypeScript parity test (`ui_automation.rs:176-209`), plus a new selector surface. That is a materially larger blast radius for a capability nobody needs: the consumer is a test harness that decides suppression before it launches the app. The chosen design has no toggle, so no in-flight pulse can ever observe a change of the flag; that is the specified behavior for the race, and it is guaranteed structurally rather than by ordering care.

**Candidate 2's CLI-flag half, a new `--suppress-layout-pulse` clap flag: rejected.**
It would widen `Cli` (`src-tauri/src/cli/mod.rs:97-127`), `main.rs`'s GUI arm (`src-tauri/src/main.rs:114-143`), and the public `run(placement, ui_automation_enabled)` signature (`src-tauri/src/lib.rs:2376`) to carry a third startup parameter across a boundary that carries no other per-run automation configuration. `AC_UI_AUTOMATION` is already a first-class peer of `--ui-automation`, so an environment variable is the idiomatic and strictly smaller expression of the same input, and it reaches `UiAutomationState` without touching any of those three files.

**Candidate 3, a field on the UI automation session state: adopted, in combination with candidate 2's environment-variable input.**
The resolved boolean is stored as an immutable field on `UiAutomationInner` and read through an accessor that ANDs `self.enabled()`. That makes "suppression without UI automation" structurally unrepresentable rather than merely unreached, which is what criterion 2 asks for. Note that the on-disk `UiAutomationSession` (`ui_automation.rs:139-147`) is deliberately **not** touched: that record exists so a separate CLI process can find and address the running GUI, and suppression is not addressed by the CLI.

### 3.2 Where the check sits, and why it cannot race

`runPulse` has exactly two call sites: `App.tsx:728` inside `onMainTerminalLayoutPulseRequest`, guarded by `if (sidebarInitializationSettled)`, and `App.tsx:910` in the `finally` of `onMount`, immediately after `sidebarInitializationSettled = true`. The flag is resolved as the **first statement of `onMount`**, in its own `try`/`catch`, before the outer `try` that the `finally` belongs to. Therefore the flag is final on every path, including the path where the outer `try` throws, before either call site can run. A pulse request that arrives before resolution is parked by the pre-existing mechanism, not evaluated early.

The check is placed after `runPulse`'s existing first guard and before `owner.started = true`, which is before the first width write at `App.tsx:554`. Criterion 3 holds: the divider never moves and no sidebar width is written.

### 3.3 What the suppressed pulse looks like

`finishPulse(owner, "skipped", "suppressed")` produces a complete, well-formed trace: `version: 1`, the request's `requestId`, `sessionId` and `attachGeneration`, `status: "skipped"`, `reason: "suppressed"`, all three phase traces at `emptyPulsePhase()` (every field `null`), `dwellMs: 0`, `settingsWritesDelta: 0`. `finishPulse`'s settings-write override only rewrites a `requestedStatus === "completed"`, so the status and reason survive unchanged (`App.tsx:236-245`).

`requestMainLayoutPulse` never falls back to its local `"unhandled"` result, because that fallback is guarded by `if (!request.accepted)` (`TerminalView.tsx:986-996`) and `onMainTerminalLayoutPulseRequest` sets `request.accepted = true` synchronously at accept time (`App.tsx:692`), before `runPulse` is reached at all. The guard is therefore already closed when `dispatchEvent` returns, on the synchronous path and on the parked path alike; whether `finishPulse` has completed the request by then does not matter to it, and `complete` is idempotent in any case (`TerminalView.tsx:951-957`).

### 3.4 Why the trace reaches `ui-terminal`

`settleAttachViewport` stores the trace only when `resizeOutcome === "sent"` (`TerminalView.tsx:1346-1353`). On the pulse branch the outcome comes from `confirmFinalPtyResize`, whose return type is `Promise<"sent" | "failed">`: it always starts or joins a real attempt and can never return `"deduplicated"` (`TerminalView.tsx:510-523`). Suppression does not change the grid, so the final fit is the same as today and the resize still goes out. The stored trace is then surfaced on the `ui-terminal` target whenever its `attachGeneration` matches the current attach (`TerminalView.tsx:1112-1131`), as `target.layoutPulse` (`src/shared/types.ts:1165`). A harness therefore reads `layoutPulse.reason === "suppressed"` and has proof, not an inference.

The CLI boundary preserves it: `sanitize_response_for_cli` (`ui_automation.rs:1698-1717`) rewrites only `response.available`, the near-miss list, and never touches `response.target`, so `target.layoutPulse` reaches the `ui-terminal` stdout JSON intact.

### 3.5 Reason precedence, stated so nobody is surprised

Suppression is decided in `runPulse`, which runs after `onMainTerminalLayoutPulseRequest`'s pre-existing `busy`, `dragging` and `persistence_owned` rejections (`App.tsx:702-723`). A request rejected for one of those reasons keeps that reason and never reaches the suppression check. This is correct and is not a hole: in every such case the pulse also did not run and no width was written, and the harness sees the true reason rather than a synthesized one. What criterion 4 requires is that a genuinely suppressed pulse be distinguishable, and `"suppressed"` is emitted by exactly one call site.

Under suppression, `busy` becomes structurally near-unreachable in steady state, because a suppressed owner completes synchronously inside `runPulse` and clears `pulseOwner`, so no live owner survives to block the next request. The one residual window is the pre-existing startup window before `sidebarInitializationSettled`, where a second request can still be rejected as `busy` while the first is parked. Harnesses observing suppression should read the trace of a single attach, which is what the #1656 experiment does.

## 4. Scope

### In scope

- Rust: an environment-driven, gate-checked resolver; an immutable field and accessor on `UiAutomationState`; the third constructor parameter that carries it, together with the mechanical update of the six `#[cfg(test)]` call sites of `UiAutomationState::new` that live in the same file; a Tauri command exposing it; its registration.
- Frontend: a new `"suppressed"` member on `MainTerminalLayoutPulseReason`; an `AutomationAPI` method; the resolution and the single guard in `src/main/App.tsx`; one new test file.
- Docs: the switch documented in the "Semantic UI Automation" section of `docs/testing/README.md`, where the bridge is documented.

### Out of scope

Removing the pulse; changing pulse behavior when the switch is off; any user-facing setting; any `settings.json` schema field; any new CLI flag or subcommand; any change to the `UiAutomationAction` set, to `UiAutomationSession`, or to the on-disk automation protocol; bumping `MainTerminalLayoutPulseTrace.version`; any change to the five pre-existing skip paths.

## 5. Evidence inventory (read at `1e57aa5`)

| Fact | Location |
|---|---|
| Two-part gate: testable exe name | `src-tauri/src/testability/ui_automation.rs:1624-1632`, `window_placement.rs:4` |
| Two-part gate: `--ui-automation` / `AC_UI_AUTOMATION` | `ui_automation.rs:21`, `ui_automation.rs:1467-1484` |
| Refusal code `refusing_non_testeable_binary` | `ui_automation.rs:1475-1482`, `ui_automation.rs:1612-1622` |
| Named precedent for a gated startup resolver with a pure, unit-tested core | `window_placement.rs:18-80` (`resolve_from_cli_or_env` delegating to `resolve_from_cli_or_env_for_exe`) |
| `UiAutomationState` / `UiAutomationInner` / `new` / `enabled()` | `ui_automation.rs:245-294` |
| `UiAutomationState::new` call sites: seven, one in production and six in the same file's `#[cfg(test)] mod tests` | `src-tauri/src/lib.rs:2427-2430`; `ui_automation.rs:2919`, `:2938`, `:2960`, `:2979`, `:3025`, `:3072` |
| Existing command triple and its registration | `src-tauri/src/commands/testability.rs:3-26`, `lib.rs:3573-3575` |
| `runPulse`, first guard, first width write | `src/main/App.tsx:486-489`, `:554` |
| `runPulse`'s only two call sites | `App.tsx:728`, `App.tsx:910` |
| `finishPulse` | `App.tsx:203-268` |
| `createPulseOwner`, seeded trace | `App.tsx:280-319` |
| Pre-existing skip reasons | `App.tsx:527` (`clamped`), `:712` (`busy`), `:718` (`dragging`), `:722` (`persistence_owned`), `TerminalView.tsx:993` (`unhandled`) |
| Request listener registered at setup, before `onMount` | `App.tsx:732-735` |
| `onMount` order and the `finally` that settles initialization | `App.tsx:883-912` |
| Reason union (15 members, additive) | `src/shared/types.ts:1101-1116` |
| Trace shape, `version: 1` | `types.ts:1127-1139` |
| `UiTerminalAutomationTarget.layoutPulse` | `types.ts:1165` |
| Trace stored only when `resizeOutcome === "sent"` | `TerminalView.tsx:1346-1353` |
| `confirmFinalPtyResize` cannot return `"deduplicated"` | `TerminalView.tsx:510-523` |
| `layoutPulse` projected onto the `ui-terminal` target | `TerminalView.tsx:1112-1131` |
| `AutomationAPI` surface | `src/shared/ipc.ts:591-600` |
| Existing pulse test file, and the two mocks that make it immune | `src/main/App.sidebar-width.test.tsx:44` (`vi.mock("../shared/ipc", ...)` replaces the whole module) and `:50` (`vi.mock("../shared/platform", () => ({ isTauri: false }))`) |
| Precedent for `isTauri: true` plus a `@tauri-apps/api/window` mock | `src/terminal/App.workflow.test.tsx`, `src/sidebar/components/Titlebar.zoom.test.tsx` |
| No exhaustive switch over the reason union anywhere | grep `MainTerminalLayoutPulseReason`: 8 hits, all type positions |
| Rust never models the reason vocabulary (opaque `Value`) | `ui_automation.rs:213-235` (`target: Option<Value>`); zero Rust hits for `persistence_owned` / `layoutPulse` |
| Bridge documentation site | `docs/testing/README.md:79-83` |

Codebase Memory gate: project `D-0_repos-AgentsCommander_iac-.ac-room-4-ac-dev-team-v4-repo-AgentsCommander`, `head_sha 1e57aa581de4c4fd18590cdf0652d8bf60b18a4f`, 26748 nodes, 144611 edges, status `ready`.

## 6. Phase table

| Phase ID | Child slug | Class | Owner | Files | Depends on | Parallel with | Phase-SHA256 |
|---|---|---|---|---|---|---|---|
| Phase 1 | `phase-1-rust-gate-and-command` | `patterned` | `ac-dev-rust-v4` | 3 | None | None | `A2BDFDF40409B1DFAE98C84A1834621D5D0E9A4AB95081E2EA374C1CD8DEC7A1` |
| Phase 2 | `phase-2-frontend-suppression` | `design-bearing` | `ac-dev-webpage-ui-v4` | 4 | Phase 1 | Phase 3 | `85C35A854546A5C4AB202B129E7C8798ABEF85EBA5FFCB34D52353D2B0202877` |
| Phase 3 | `phase-3-docs` | `patterned` | `ac-technical-writer-v4` | 1 | Phase 1 | Phase 2 | `A930CB752B3EA20A61F68BDADAD022873C4A3DF343F8AFCBE7A75591EE007280` |

Cut justification, in the skill's order:

1. **Owner.** Rust, frontend and docs never share a phase, and this change has all three. That alone forces three phases and is the operative rule here; the file counts (3, 4, 1) are far below the ten-file budget and are not the reason for the cut.
2. **Contract.** Phase 1 changes exactly one contract, the Tauri IPC surface (one added command). Phase 2 changes exactly one contract, the frontend pulse-reason vocabulary, and is the *consumer* side of Phase 1's IPC change, which is precisely where the rule says the consumer belongs. Phase 3 changes no contract. No phase touches CLI, persistence, or settings schema.
3. **Green tree.** Phase 1 ends green with no dead code: every symbol it adds has a live caller inside the same phase (`ENV_SUPPRESS_LAYOUT_PULSE` from the resolver, the private pure core from the public resolver, the public resolver from `lib.rs`, `current_exe_name` from `current_exe_is_testable` and the resolver, the accessor from the command, the command from `generate_handler!`). `cargo clippy --workspace --all-targets -- -D warnings` therefore passes at the Phase 1 boundary. Phase 2 ends green with `npm run typecheck`, `npm test` and `npm run build`. Phase 3 touches only Markdown.
4. **Budget.** Not reached.

Phase 3 depends only on Phase 1 (it documents the environment-variable name and the gate) and shares no file with Phase 2, so the two are parallelizable.

## 7. Dependency-cycle and layering statement

**This plan adds zero module arcs, in Rust and in TypeScript.**

Enumerated new module-to-module references and their verdicts:

| New reference | Arc | Verdict |
|---|---|---|
| `lib.rs` calls `crate::testability::ui_automation::resolve_layout_pulse_suppression` | `agentscommander_lib -> agentscommander_lib::testability::ui_automation` | Already recorded, `src-tauri/module-arcs.txt:43`. No new arc. |
| `commands/testability.rs` new command references `UiAutomationState` | `agentscommander_lib::commands::testability -> agentscommander_lib::testability::ui_automation` | Already recorded, `module-arcs.txt:532`. No new arc. |
| `ui_automation.rs` resolver compares against `TESTABLE_EXE_NAME` | `agentscommander_lib::testability::ui_automation -> agentscommander_lib::testability::window_placement` | Already recorded, `module-arcs.txt:1038`, and the `use` at `ui_automation.rs:15` already exists. No new arc, no new import. |
| `lib.rs` adds one entry to `tauri::generate_handler!` | `agentscommander_lib -> agentscommander_lib::commands::testability` | Not recorded today (the detector does not resolve references inside that macro; there is no such line in `module-arcs.txt`). Adding a sibling entry therefore adds no recorded arc either. |
| `src/main/App.tsx` imports `AutomationAPI` from `../shared/ipc` | `src/main/App -> src/shared/ipc` | Already exists: `App.tsx:16` already imports `SettingsAPI` from the same module. No new arc. |
| `src/main/App.tsx` imports the reason type from `../shared/types` | `src/main/App -> src/shared/types` | Already exists, `App.tsx:3-15`. No new arc. |

Removed arcs: none.

Per-arc classification: every reference above is either already present in the committed arc record or invisible to the detector for a reason that is unchanged by this plan. No reference joins two previously distinct SCCs and none crosses a previously clean SCC boundary, because no new pair is introduced at all.

Role and layering hygiene: the resolver added to `ui_automation.rs` is a pure function over a boolean, a string and an `Option<&str>`, plus one thin wrapper that reads `std::env` and `current_exe`. No lower layer gains an `AppHandle`, a `tauri` type, or any UI transport; the only `tauri` contact is the new `#[tauri::command]` in `commands/testability.rs`, which is the layer that already owns that surface. No co-location fix is required.

Acceptance criterion the implementation reviewer must run (base SHA versus final branch head, clean tree for both), from the repository root:

```
node "D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "D:\0_repos\AgentsCommander_iac\.ac\room-4-ac-dev-team-v4\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green if and only if: `coverage.graphShape.cyclicSccs` is equal pre and post; every cyclic SCC member set is identical set to set; zero new `from -> to` pairs cross a previously clean SCC boundary; the regenerated `src-tauri/module-arcs.txt` is byte-identical, so `git status --porcelain src-tauri/module-arcs.txt` is empty; and the structural layering guard tests stay green. Detector exit code 1 means gating cycles exist and the graph was still written, which is the normal outcome on this repository; only exit 3 means no graph. Emit the graphs outside the repository root or delete them before committing, because they are not gitignored.

The frontend cycle gate `npm run check:frontend-dependencies` is not wired into CI and must be run locally in Phase 2. Expected result: pass, since Phase 2 adds no import edge that does not already exist.

## 8. Delivery nonfunctional invariants

**CI-to-plan parity**, derived from `.github/workflows/` at the pinned base and the planned diff:

| Check | Triggered? | Owner of the evidence |
|---|---|---|
| `test-debt` | Yes (`pull_request`, no path filter) | CI. Phase 2 adds a real test file and no ignored or placeholder test, so the gate is unaffected. |
| `rust-regression` (windows: `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`) | Yes | CI is authoritative. Phase 1 runs the same three commands locally. |
| `rust-regression-linux`, `rust-regression-macos` (check + clippy) | Yes | CI only. Not reproducible locally on this Windows host; recorded as remote-owned. |
| `rust-fmt` (`cargo fmt --all -- --check`) | Yes | Phase 1 must run `cargo fmt --all` before committing and must not hand-format the added code. All Rust shown in the phase file is illustrative of semantics, not of formatting. |
| `terminal-snapshot-portable` | Yes | CI. No file in scope belongs to `terminal-snapshot-renderer` or `session-bridge`; expected unchanged pass. |
| `windows-release-cli-smoke` | Yes | CI. The plan adds no CLI verb and no clap flag, so the smoke surface is unchanged. |
| `frontend-regression` (`npm run typecheck`, `npm test`) | Yes | Phase 2 runs both locally; CI is authoritative. |
| `lockfile-check` | Yes (`pull_request`) | No dependency is added; `package.json` and `package-lock.json` are untouched. Expected pass. |
| `bundle-validation` | **No.** Path-filtered to tauri configs, icons, nsis, packaging, `package*.json`, `Cargo.lock`. None is in scope. | Non-applicable, proven by the path filter. |
| `version-sync-check` | **No.** Path-filtered to `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `Cargo.lock`, `src-tauri/tauri.conf.json`, the two version scripts. None is in scope. | Non-applicable, proven by the path filter. |
| `validate-branch-name` | Yes | The branch `feature/1724-testable-pulse-suppression-switch` is already issue-numbered and was created by the tech lead. |

Delivery requires every triggered and configured-required check to be green **on the exact PR-head SHA**. Evidence from another SHA, an unexplained skip, a waiver, or a bypass does not satisfy the gate. If workflows, required-check configuration, base, or diff drift, re-derive this table.

**Deterministic toolchain and build.** Rust stable as pinned by `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c`; Node 22 with npm pinned to 11.6.2; `npm ci` against the committed lockfile. Cargo commands run with `working-directory: src-tauri`; the cargo target directory is at the repository root, not under `src-tauri`. Record the resolved `cargo --version`, `rustc --version` and `node --version` once per phase in the phase report.

**Authorized, traceable Git.** Open issue #1724 exists; branch is issue-numbered; the base is synchronized and pinned above. All state-changing Git runs inside `repo-AgentsCommander`. Delivery is by pull request; direct push to `main` is forbidden. Before the first product mutation of a phase and again before PR creation or update, fetch the live target and classify pinned-base drift by changed paths. Drift touching any file in section 6's file lists, the formatter configuration, toolchain or dependency resolution, or the workflows above requires refreshing only the affected evidence. Drift proven unrelated is recorded and synchronized at the next gate; it does not reopen accepted design.

**Process state and working directory.** Two environment variables materially change behavior under test and must be explicitly cleared or set: `AC_UI_AUTOMATION` and the new `AC_UI_AUTOMATION_SUPPRESS_LAYOUT_PULSE`. Rust unit tests must not depend on either; the pure resolver takes its inputs as parameters precisely so no test reads process state. Use an explicit working directory for every cargo and npm command. Emit no scratch file into the repository; if the cycle detector is run, write `pre.json` and `post.json` outside the repository root.

**Validation and scope before acceptance.** Each phase freezes its exact path set (listed in its phase file) before mutation and proves afterwards that `git status --porcelain` lists only those paths. `src-tauri/module-arcs.txt` must remain byte-identical; a non-empty status on that file is a failure, not a regeneration to commit.

**Mutation ownership and recovery.** Immediately before writing, recheck branch, base and the affected paths. On failure, restore only the paths that phase actually changed and only while their current content is still that phase's own output; preserve externally changed bytes and report the conflict. No repository-wide `git reset`, `git restore`, or `git clean` as recovery.

**Bounded execution and diagnostics.** `cargo test` and `npm test` run under the runner's timeout with stdin closed. Capture stdout and stderr to a file and report the exit code; `cargo test` stdout is swallowed when piped from some shells, so redirect to a file and read it back. A timed-out or failed command is never reported as success.

**Evidence discipline.** Zero and absence are valid states: "no new module arc" and "two non-applicable path-filtered workflows" are asserted positively above with their proofs. Every claim in each phase file's acceptance criteria is an executable command with a stated expected result.

**Enhanced controls: all explicitly non-applicable.** No independently anchored executable hashes, DLL or helper closure inventories, poisoned-`PATH` tests, SDK manifests, hostile-parent environment maps, exclusive cooperation locks, or mutation ledgers are required, because the accepted task class is a routine application-code change on a trusted developer host with no release, signing, packaging, migration, or security-boundary component. Any finding that presumes a hostile host, tool tampering, or supply-chain provenance is advisory and is not a readiness blocker.

## 9. Compatibility impact

- **Non-testable binaries are unaffected in every reachable state.** The resolver's exe-name condition fails, so the field is `false`, the command returns `false`, and `runPulse` takes the same path it takes today. Setting the environment variable on a release, stage, or room binary does nothing and does not fail startup, which is deliberate: a stray variable must never prevent the product from launching.
- **Browser (non-Tauri) builds** never read the flag at all, because the read is `isTauri`-gated; the switch is unreachable there by construction.
- **`UiAutomationState::new` gains a third parameter, and that is contained.** The constructor is `pub`, but `agentscommander_lib` is this application's own library: a repo-wide grep at the pinned base finds exactly seven call sites, one in `src-tauri/src/lib.rs` and six in `ui_automation.rs`'s own `#[cfg(test)] mod tests`, and none in `src-tauri/src/main.rs` or under `src-tauri/tests/`. All seven are updated inside Phase 1's frozen three-file set, the six test ones with `false`, so no consumer outside that set exists to break.
- **No persisted state changes.** `settings.json` is untouched; the on-disk `UiAutomationSession` record is untouched; no migration is required and no downgrade is affected.
- **`MainTerminalLayoutPulseTrace.version` stays `1`.** The version pins the trace's field set, and the field set is unchanged. `"suppressed"` is a new value of an existing string field, not a new field, and Rust carries the trace as an opaque `serde_json::Value`, so no Rust type, wire key, or parity test moves. A consumer that switches on `reason` and does not know `"suppressed"` sees an unrecognized skip reason, which is the same class of outcome it already gets for any of the fifteen existing reasons it does not handle.
- **Existing pulse tests are untouched and must stay untouched.** `src/main/App.sidebar-width.test.tsx` replaces the whole `../shared/ipc` module with a two-method `SettingsAPI` stub and mocks `isTauri` to `false`, so the new `isTauri`-gated `AutomationAPI` read is never executed in that file and no added `await` enters its startup path. If that file needs any edit, the frontend design is wrong; stop and escalate rather than editing it.
- **`ui-terminal` consumers** gain one new possible value of `target.layoutPulse.reason`. No existing selector, action, or response field changes.

## 10. Acceptance criteria, mapped to the issue

| Issue criterion | Where it is satisfied | Objective evidence |
|---|---|---|
| 1. Switch off is the default and the only reachable state in a non-testable binary; pulse behavior byte-identical; existing pulse tests pass untouched | Phase 1 resolver returns `false` unless all three conditions hold; Phase 2 guard is a single `if` before `owner.started = true` | `npm test` green with `src/main/App.sidebar-width.test.tsx` unmodified (`git diff --stat` must not list it); Rust unit test `layout_pulse_suppression_defaults_off` |
| 2. A non-testable binary cannot enable the switch, enforced by the same gate; a test proves the refusal | Phase 1: `resolve_layout_pulse_suppression_for_exe` requires `exe_name == TESTABLE_EXE_NAME`; the accessor additionally ANDs `enabled()` | Rust unit tests `layout_pulse_suppression_refuses_non_testeable_exe` and `layout_pulse_suppression_refuses_without_automation` |
| 3. Switch on suppresses the pulse before any width write; divider never moves; no sidebar width written | Phase 2: guard at the top of `runPulse`, before `owner.started = true` and 60+ lines before the first `setSidebarWidth` | Frontend test asserts the sidebar pane's inline width is unchanged across the whole request and that `SettingsAPI.update` was never called |
| 4. The suppressed state is observable through the `ui-terminal` query | Phase 2: `finishPulse(owner, "skipped", "suppressed")` yields a trace that `settleAttachViewport` stores and `ui-terminal` projects as `target.layoutPulse` | Frontend test asserts the completed result is exactly `{status: "skipped", reason: "suppressed"}` with `version: 1`, empty phase traces, `dwellMs: 0`; section 3.4 proves the transport to `ui-terminal` from committed code |
| 5. Frontend and Rust tests cover on, off, and the refusal path | Phase 1 Rust tests (5); Phase 2 frontend tests (4) | `cargo test --lib` and `npm test` |
| 6. The switch is documented wherever the UI automation bridge is documented | Phase 3 | `docs/testing/README.md` "Semantic UI Automation" section names the variable, the gate, and the observable reason |

## 11. Certification

Status: **READY_FOR_IMPLEMENTATION**.

Every applicable baseline gate in `delivery-nonfunctional-invariants` has named executable evidence, an expected result, an owner and a failure behavior in sections 7 and 8. Every enhanced control is explicitly marked non-applicable with its reason. The dependency-cycle gate is satisfied by a zero-new-arc proof against the committed `src-tauri/module-arcs.txt`, with the byte-identity re-run written into the acceptance criteria. No `TBD`, no open alternative, and no choice is left to the implementer.
