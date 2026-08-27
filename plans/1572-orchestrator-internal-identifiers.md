# Plan #1572: Coordinator to Orchestrator, phase 2 (internal identifiers, file names, SelectionCoordinator)

Status: READY_FOR_IMPLEMENTATION
Issue: #1572 (open, label `refactor`). Parent epic: #1570, phase 2 of 4. Phase 1 (#1571) closed
2026-08-27T16:26:04Z and landed.
Route: Full.
Author: ac-architect-v3. Consensus round 1.
Repos: `repo-AgentsCommander` and `repo-agentscommander_webpage`. One plan, two pull requests.

---

## 1. Frozen authority and entry gate

### 1.1 Frozen base

| Repo | Branch | Base SHA (== `origin/main` at authoring) |
| --- | --- | --- |
| `repo-AgentsCommander` | `refactor/1572-orchestrator-internal-identifiers` | `147ad4efa537f3ae5386c6949fa039dfa7e6735a` |
| `repo-agentscommander_webpage` | `refactor/1572-orchestrator-internal-identifiers` | `5ec1ad27e1fed2970a83c191a5d4e33993a5436f` |

Both working trees were clean (`git status --porcelain` empty) when every number below was
measured. Every count, line number, path and digest in this plan is a measurement at those two
SHAs, not an estimate. Where a number in issue #1572 or epic #1570 disagrees with a measurement
here, the measurement is authoritative and section 3.1 says why.

Codebase Memory gate for `repo-AgentsCommander`: `status: ready`,
project `D-0_repos-AgentsCommander_iac-.ac-wg-13-ac-dev-team-v3-repo-AgentsCommander`,
`head_sha 147ad4efa537f3ae5386c6949fa039dfa7e6735a`, 25292 nodes, 139043 edges.

### 1.2 Entry ritual for the implementer

Run this before the first mutation, in both repos. Any failure stops the run.

```powershell
cd D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-AgentsCommander
git status --porcelain              # must be empty
git rev-parse HEAD                  # must be 147ad4efa537f3ae5386c6949fa039dfa7e6735a
git branch --show-current           # must be refactor/1572-orchestrator-internal-identifiers
git fetch origin main --quiet ; git rev-parse origin/main
```

If `origin/main` has moved off `147ad4ef`, classify the drift by changed paths before continuing
(section 13.5). Drift that does not touch `src-tauri/`, `src/`, `scripts/02-module-arc-record.mjs`,
`.github/workflows/` or `.gitattributes` is recorded and synchronised at the next bounded gate; it
does not reopen this plan.

Baseline digest that must hold before the first mutation:

```powershell
(Get-FileHash -Algorithm SHA256 src-tauri\module-arcs.txt).Hash
# A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E
```

---

## 2. Issue and objective

After phase 1 the product says "Orchestrator" everywhere a human or an agent reads text, but the
source still names the agent role `coordinator` in Rust and TypeScript identifiers and in 7 file
names. A second, unrelated internal type, `SelectionCoordinator`, shares the word and blocks the
epic's zero-occurrence goal.

Phase 2 makes the identifiers match the product term, without crossing a single serialization
boundary. Compiled behavior must be byte-for-byte equivalent: same JSON keys on disk, same IPC
command names and payload keys, same event names, same `data-ac-testid` values, same
machine-readable error codes, same frozen template bytes.

Two concepts, kept apart on purpose:

- **Concept A**, the agent role (`is_coordinator`, the idle badge, auto-close, the team
  `coordinator` key, `Context.coordinator.md`): becomes **Orchestrator**.
- **Concept B**, `SelectionCoordinator`, the single-threaded arbiter that serialises session
  transitions: becomes **SelectionArbiter**, deliberately NOT "Orchestrator", so the two concepts
  can never be confused again.

---

## 3. Evidence, measured at `147ad4ef` / `5ec1ad27`

### 3.1 Baseline, and four corrections to the numbers in the issues

Every "contains the word coordinator" count in #1570 and #1572 counts prose, comments and string
literals along with identifiers. This phase renames identifiers, so the identifier count is the one
that predicts the diff. Both were measured; both are reported.

| Measure | Word count (any context) | Identifier count (literals and comments stripped) |
| --- | --- | --- |
| `src-tauri/src` files with a hit | 66 | 56 |
| `src-tauri/tests` files with a hit | 9 | 6 |
| `crates/` files with a hit | 5 | **0** |
| `src/` (TS/TSX) files with a hit | 89 | 73 |
| distinct Rust identifiers | n/a | 224 |
| distinct TypeScript identifiers | n/a | 60 |

Corrections:

1. **`crates/` is untouched by this phase.** All 5 hits are string literals: the two reason-detail
   strings at `crates/session-bridge/src/bin/agentscommander-api-helper.rs:681,684` and four
   `"project:wg-1-team/coordinator"` test fixture FQNs in
   `crates/session-bridge/tests/docker_bridge_e2e.rs:294`,
   `crates/session-bridge/tests/terminal_snapshot_helper_process.rs:19`,
   `crates/terminal-snapshot-renderer/src/json.rs:1053,1262,1292,1322` and
   `crates/terminal-snapshot-renderer/tests/goldens.rs:95`. Zero identifiers. The epic's note that
   "any phase that renames those strings must change both copies" is therefore **not** binding on
   phase 2, because phase 2 renames no string. `terminal-snapshot-portable` is a pure negative
   control for this PR.
2. **"62 affected frontend test files" is right, but only after one subtraction.** 63 files matching
   `*.test.ts`/`*.test.tsx` under `src/` contain the substring `coordinat`. One of them,
   `src/screenshot-overlay/App.test.tsx`, matches only on the geometry word `coordinates` and is not
   a Coordinator file at all. 63 minus 1 = 62.
3. **`coordinat` is not `coordinator`.** `coordinate`, `coordinates`, `coordination`, `coordinating`
   and `coordinates` do not contain the string `coordinator` and are therefore outside both this
   phase and the epic's zero-occurrence gate. Two identifiers are excluded by this rule alone:
   `src-tauri/src/testability/window_placement.rs::env_json_parses_negative_coordinates` (the file's
   only hit, so the file does not change) and `src/screenshot-overlay/App.test.tsx`. The web repo's
   `CoordinationDemo` / `CoordinationProof` are in scope by the issue's explicit enumeration, not by
   this rule.
4. **The issue's `module-arcs.txt` line list is correct but incomplete as a description.** The 14
   lines are 9, 327, 337, 398, 420, 465, 579-584, 900, 1024. The file is fully sorted, so renaming
   the module does not edit 14 lines in place: it moves them. Section 3.6 gives the exact post-state.

### 3.2 Concept B, the complete symbol set (two symbols more than the issue's table)

`src-tauri/src/session/selection.rs` is 4000+ lines and owns the type. Declarations:

| Symbol | Declared at | Occurrences (identifier) | Files |
| --- | --- | --- | --- |
| `SelectionCoordinator` | `session/selection.rs:765` (struct), `:775` (impl) | 94 | 14 |
| `SelectionCoordinatorError` | `session/selection.rs:354` (enum) | 100 | 2 |
| `CoordinatorPhase` | `session/selection.rs:422` (enum), `:429` (impl) | 47 | 1 |
| `CoordinatorJob` | `session/selection.rs:685` (enum) | 42 | 1 |
| `CoordinatorEnvelope` | `session/selection.rs:738` (struct) | 26 | 1 |
| `CoordinatorInner` | `session/selection.rs:745` (struct) | 11 | 1 |
| `COORDINATOR_QUEUE_CAPACITY` | `session/selection.rs:26` | 8 | 1 |
| `COORDINATOR_ADMISSION_CAPACITY` | `session/selection.rs:27` | 4 | 1 |
| `QuarantineRetryPath::Coordinator` | `resource_monitor/watchdog.rs:35` | 3 | 2 |
| `isSelectionCoordinatorBusyError` (TS) | `src/shared/ipc.ts` | 7 | 4 |

`SelectionCoordinatorError` (100 occurrences, the largest single symbol in the phase) and
`QuarantineRetryPath::Coordinator` are **not** in the issue's rename table. They are Concept B by
construction: `SelectionCoordinatorError` is the error type of `SelectionCoordinator::*`, and
`QuarantineRetryPath::Coordinator` names the watchdog's arbiter path, not an agent role. Leaving
either behind would leave Concept B sharing the word and fail acceptance criterion 7. They are
added to the rename table in section 5.2.

`BoundContainerCoordinatorError` (`api/identity.rs:87`, 31 occurrences, 2 files) and
`VerifiedBoundContainerCoordinator` (`api/identity.rs:95`) look like Concept B but are **Concept A**:
they verify a container-bound workgroup orchestrator identity. They take the Orchestrator name.

### 3.3 The serialization boundary, exhaustively

This is the single most consequential measurement in the plan. `AppSettings` and 20 other types
carry `#[serde(rename_all = "camelCase")]` (or `"snake_case"`), so the persisted or transported key
is **derived from the Rust identifier**. Renaming any of these members without an explicit pin
silently changes a persisted JSON key, an IPC payload key or a stored enum value, which is exactly
what the issue puts out of scope and routes to phase 3.

Every serde-derived member whose name contains `coordinat`, with the wire key it produces today:

| File:line | Type | Member | Wire key produced today |
| --- | --- | --- | --- |
| `src-tauri/src/cli/team.rs:104` | `TeamListItem` | `coordinator` | `"coordinator"` |
| `src-tauri/src/cli/team.rs:115` | `TeamCreateResult` | `coordinator` | `"coordinator"` |
| `src-tauri/src/cli/team.rs:127` | `AddMemberResult` | `coordinator` | `"coordinator"` |
| `src-tauri/src/commands/ac_discovery.rs:84` | `AcTeam` | `coordinator` | `"coordinator"` |
| `src-tauri/src/commands/ac_discovery.rs:112` | `AcAgentReplica` | `is_coordinator` | `"isCoordinator"` |
| `src-tauri/src/commands/entity_creation.rs:59` | `TeamConfigResult` | `coordinator` | `"coordinator"` |
| `src-tauri/src/commands/loops.rs:30` | `LoopCreateRequest` | `busy_coordinator` | `"busyCoordinator"` |
| `src-tauri/src/commands/loops.rs:45` | `LoopUpdateRequest` | `busy_coordinator` | `"busyCoordinator"` |
| `src-tauri/src/config/agent_config.rs:95` | `AgentDarkFactory` | `is_coordinator_of` | `"isCoordinatorOf"` |
| `src-tauri/src/config/loops.rs:25` | `LoopTargetKind` | `WorkgroupCoordinator` | `"workgroupCoordinator"` |
| `src-tauri/src/config/loops.rs:98` | `LoopPolicy` | `busy_coordinator` | `"busyCoordinator"` |
| `src-tauri/src/config/loops.rs:153` | `LoopAuditEntry` | `busy_coordinator_policy` | `"busyCoordinatorPolicy"` |
| `src-tauri/src/config/loops.rs:169` | `AcLoopSummary` | `busy_coordinator` | `"busyCoordinator"` |
| `src-tauri/src/config/sessions_persistence.rs:377` | `PersistedSession` | `is_coordinator` | `"isCoordinator"` |
| `src-tauri/src/config/settings.rs:294` | `AppSettings` | `restore_coordinator_wake_state` | `"restoreCoordinatorWakeState"` |
| `src-tauri/src/config/settings.rs:310` | `AppSettings` | `legacy_start_only_coordinators` | `"startOnlyCoordinators"` (already pinned) |
| `src-tauri/src/config/settings.rs:527` | `AppSettings` | `coordinator_idle_badge_yellow_minutes` | `"coordinatorIdleBadgeYellowMinutes"` |
| `src-tauri/src/config/settings.rs:529` | `AppSettings` | `coordinator_idle_badge_red_minutes` | `"coordinatorIdleBadgeRedMinutes"` |
| `src-tauri/src/config/settings.rs:532` | `AppSettings` | `coordinator_auto_close_enabled` | `"coordinatorAutoCloseEnabled"` |
| `src-tauri/src/config/settings.rs:534` | `AppSettings` | `coordinator_auto_close_minutes` | `"coordinatorAutoCloseMinutes"` |
| `src-tauri/src/config/settings.rs:538` | `AppSettings` | `coordinator_auto_close_skip_telegram_assigned` | `"coordinatorAutoCloseSkipTelegramAssigned"` |
| `src-tauri/src/config/settings.rs:542` | `AppSettings` | `coordinator_cascade_close_enabled` | `"coordinatorCascadeCloseEnabled"` |
| `src-tauri/src/phone/types.rs:82` | `PtyInputReasonCode` | `SenderNotCoordinator` | `"sender_not_coordinator"` |
| `src-tauri/src/phone/types.rs:85` | `PtyInputReasonCode` | `TargetIsCoordinator` | `"target_is_coordinator"` |
| `src-tauri/src/pty/git_watcher.rs:416` | `CoordinatorChangedPayload` | `is_coordinator` | `"isCoordinator"` |
| `src-tauri/src/resource_monitor/watchdog.rs:35` | `QuarantineRetryPath` | `Coordinator` | `"coordinator"` |
| `src-tauri/src/session/session.rs:122` | `Session` | `is_coordinator` | `"isCoordinator"` |
| `src-tauri/src/session/session.rs:298` | `SessionInfo` | `is_coordinator` | `"isCoordinator"` |

The precedent for the fix already exists in the same file: `settings.rs:305-310` renames the Rust
identifier `legacy_start_only_coordinators` while pinning its key with
`#[serde(rename = "startOnlyCoordinators")]`.

**Not serde**, therefore free to rename with no pin: `src-tauri/src/config/teams.rs:37`
`DiscoveredTeam::coordinator_name` and `:39` `DiscoveredTeam::coordinator_path`. `DiscoveredTeam`
carries no `Serialize`/`Deserialize` derive. This also settles the TypeScript side: no Rust type
emits a `coordinatorName` key, so `Team.coordinatorName` and `LoopCoordinatorOption.coordinatorName`
in TypeScript are frontend-only and are in scope.

### 3.4 The Tauri IPC boundary

`#[tauri::command]` derives the IPC command name from the function name, and the payload keys from
the **value** parameters. `State<'_, T>` parameters are injected from managed state and never appear
in the payload.

| Site | Kind | Verdict |
| --- | --- | --- |
| `src-tauri/src/commands/session.rs:3406` `pub async fn close_coordinator` | command name, registered in `lib.rs`, invoked from `src/shared/ipc.ts:214` as `"close_coordinator"` | **Do not rename.** Phase 3. |
| `src-tauri/src/commands/entity_creation.rs:2768` `create_team`, arg at `:2774` | value arg, payload key `coordinator` | **Do not rename.** Phase 3. |
| `src-tauri/src/commands/entity_creation.rs:3340` `update_team`, arg at `:3348` | value arg, payload key `coordinator` | **Do not rename.** Phase 3. |
| `src-tauri/src/commands/ac_discovery.rs:1040`, `discover_ac_agents` arg `coordinator_clocks: State<..>` | State arg, not in the payload | Free to rename. |
| `src-tauri/src/commands/ac_discovery.rs:1826`, `discover_project` arg `coordinator_clocks: State<..>` | State arg, not in the payload | Free to rename. |
| `src-tauri/src/commands/session.rs:4582`, `get_active_session` arg `coordinator: State<'_, SelectionCoordinator>` | State arg, not in the payload | Free to rename (Concept B). |

### 3.5 Source-text-coupled literals: the only Rust literals that change

Three tests read source text and pin it by literal. If the identifier moves and the literal does
not, the tree does not build green. These are the complete set; every other Rust literal is frozen.

1. **`src-tauri/src/session/selection.rs`**, self-scanning sentinel over `include_str!("selection.rs")`
   at `:4034`. Coupled literals: `:3909` `"enum CoordinatorJob"`, `:3910` `"CoordinatorJob declaration"`,
   `:3912` `"CoordinatorJob opening brace"`, `:3926` `"CoordinatorJob closing brace"`,
   `:4049` `"enum CoordinatorJob {{ Rogue {{ value: {forbidden} }} }}"` (the mutation the sentinel must
   reject), `:4053` `"sentinel accepted forbidden CoordinatorJob field {forbidden}"`, and the message
   `"CoordinatorJob contains a managed handle or arbitrary executable field: {:?}"`.
   `:3909` and `:4049` are functionally load bearing; the other five name the renamed type.
2. **`src-tauri/tests/cli_workgroup_team.rs:1834-1843`**, which scrapes
   `src/commands/session.rs` through `normalized_production_source` and pins the call-site text
   `materialize_agent_context_file_with_filename_activated(&cwd,&target_filename,&managed_filenames,is_coordinator,auto_self_clear,container_repos.as_ref(),activation.as_ref()`.
   The argument name `is_coordinator` inside that literal is source text, not prose.
3. **`src-tauri/src/config/teams.rs:2360`**, `assert!(diagnostic.contains("is_coordinator: true"))`,
   which asserts on the output of the manual `Debug` impl at `teams.rs:603`
   (`.field("is_coordinator", &self.is_coordinator)`). Its siblings are `teams.rs:583`
   (`.field("sender_is_coordinator", ..)`) and `session/manager.rs:63`
   (`.field("is_coordinator", ..)`). `api/identity.rs:107` `.debug_struct("VerifiedBoundContainerCoordinator")`
   is the same class.

Source scans verified **clean** of any `coordinat` pin, so they are negative controls:
`src-tauri/src/agent_update.rs:2772`, `src-tauri/src/pty/watchers/mod.rs:2470`,
`src-tauri/tests/cli_project_registration.rs:563` and `:602`.

### 3.6 `module-arcs.txt`: base state and the exact predicted post state

| Property | Base (`147ad4ef`) | Predicted post-rename |
| --- | --- | --- |
| arc lines | 1037 | 1037 |
| bytes | 82149 | 82163 |
| line endings | LF only, one trailing LF, fully sorted | same |
| SHA-256 (uppercase) | `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E` | `2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6` |

The post-state was computed by applying the single substitution
`agentscommander_lib::config::coordinator_clocks` to `agentscommander_lib::config::orchestrator_clocks`
over the 1037 base lines and re-sorting. The 14 affected lines land at 12, 327, 339, 399, 420, 465,
587-592, 900 and 1025. `src-tauri/module-arcs.txt` is pinned `text eol=lf` in `.gitattributes`, so
the worktree digest is reproducible on Windows with `core.autocrlf=true`.

### 3.7 Website inventory

| Item | Measurement |
| --- | --- |
| `coordinat*` lines under `repo-agentscommander_webpage/src` | 31 |
| of which carry the string `coordinator` | 8, all of them the i18n key `composer.coordinator` |
| `composer.coordinator` sites | `src/i18n/landing.ts:71,176,278,381,486,585` (six locales: en, es, pt, fr, en-alt, zh) and `src/components/alternatives/TeamComposer.astro:24` (`data-i18n=`) and `:25` (`copy[...]`) |
| `Coordination*` component files | `src/components/CoordinationDemo.tsx` (6796 B), `src/components/CoordinationDemo.css` (6452 B), `src/components/CoordinationProof.astro` (1434 B) |
| import / reference sites | `CoordinationDemo.tsx:3` (`import './CoordinationDemo.css'`), `:74`, `:196`; `CoordinationProof.astro:2`, `:23`; `README.md:37` |
| `CoordinationProof.astro` consumers | **none.** No page or layout imports it. |
| i18n mechanism | a generic `document.querySelectorAll('[data-i18n]')` switcher at `src/layouts/BaseLayout.astro:160-161` and `src/pages/alternatives/attention.astro:310-311`, typed `MessageKey` / `LandingMessageKey`. A key rename that misses a site is a **type error**, caught by `npm run check`. |
| CI | one workflow, `.github/workflows/deploy.yml`. Playwright specs live in `tests/`; only `tests/smoke.spec.ts:114` touches a `composer.*` key, and it is `composer.note`, not the renamed one. |

The issue's table names only the `.tsx` and the `.astro`. `CoordinationDemo.css` is added to the
rename set: it is imported by name at `CoordinationDemo.tsx:3`, so renaming the component without it
leaves a file whose name contradicts its only importer. That is decided here, not left open.

### 3.8 CI and local gate inventory, derived from the target-base workflows

`repo-AgentsCommander/.github/workflows/pr-regression-gates.yml` (`on: pull_request` and `on: push`
to any non-main branch, no path filter) runs 7 jobs:

| Job | Runner | Commands |
| --- | --- | --- |
| `test-debt` | ubuntu | `npm run test:debt`, `npm run test:classify:self`, `npm run test:report:self` |
| `rust-regression` | windows | `npm ci`, `npm run build`, `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests` (cwd `src-tauri`) |
| `rust-regression-linux` | ubuntu | `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`. **No `cargo test`.** |
| `rust-regression-macos` | macos | `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`. **No `cargo test`.** |
| `terminal-snapshot-portable` | windows, ubuntu, macos-15, macos-15-intel | `cargo test --locked -p terminal-snapshot-renderer`, `cargo test --locked -p session-bridge --bin agentscommander-api-helper terminal_snapshot` |
| `windows-release-cli-smoke` | windows | `npm run build:prod:no-bundle`, `npm run smoke:cli-release-windows` |
| `frontend-regression` | ubuntu | `npm run typecheck`, `npm test` behind the #480 known-debt guard |

`validate-branch-name.yml` triggers on push and accepts `refactor/1572-...`.
`bundle-validation.yml`, `lockfile-check.yml` and `version-sync-check.yml` are path-filtered on
`package.json` / `package-lock.json` / `Cargo.lock` / `src-tauri/tauri*.conf.json` / `packaging/**` /
`src-tauri/nsis/**` / `src-tauri/icons/**`. This diff touches none of those paths, so they do not
trigger. `cache-warm.yml` is main-only and scheduled.

**Two acceptance criteria of the issue have no CI job.** Neither
`npm run check:frontend-dependencies` (criterion 3) nor the levelization gate (criterion 4) is wired
into any workflow; `scripts/02-module-arc-record.mjs:27` states the levelization wiring is manual
"by design". Both are local-only evidence with a named owner and time in section 13.

### 3.9 Dependency-cycle and layering baseline

The change adds and removes **zero** module-to-module references. Every edit is a rename of an
existing symbol at an existing site; no `use`, no `mod`, no call site is added, removed, or
retargeted. The only structural fact that moves is the module **identifier**
`agentscommander_lib::config::coordinator_clocks`, whose 14 arcs are preserved one-for-one under the
renaming bijection (section 3.6). `src-tauri/tests/` contains no layering guard that names
`config::coordinator_clocks`; the two guards that pin dependency sets by equality,
`instance_gitignore_layering.rs` and `claude_watcher_layering.rs`, name `config`,
`config::instance_artifacts` and constant-leaf modules, and no `coordinator` identifier appears in
either file.

---

## 4. Scope

### In scope (binding)

1. **Concept A**: every Rust and TypeScript **identifier** whose text contains `Coordinator` /
   `coordinator` / `COORDINATOR` and denotes the agent role, subject to Rules S and K.
2. **Concept B**: the 10 symbols of section 3.2.
3. **7 file renames** in `repo-AgentsCommander`, each split into a pure `git mv` commit and a
   content commit.
4. **3 file renames** in `repo-agentscommander_webpage` plus their 6 reference sites.
5. **The i18n key `composer.coordinator`** in `repo-agentscommander_webpage`, 8 lines.
6. **One new test** for the sidebar filter token (issue item 3c, residual R11 of #1571).
7. **`src-tauri/module-arcs.txt` regenerated**, never hand-edited.
8. **Doc comments and code comments that name a renamed symbol**, updated with the symbol they name.

### Out of scope (binding)

Everything below stays byte-identical. Each has a named later owner.

| Preserved | Sites | Owner |
| --- | --- | --- |
| Every string literal not listed in section 5.4 | 379 distinct `coordinat` literals across `src-tauri/src`, `src-tauri/tests`, `crates`, `src` | phases 3 and 4 |
| Persisted and transported JSON keys | the 28 serde members of section 3.3, pinned by Rule S | phase 3 |
| The IPC command `close_coordinator` and the `coordinator` payload arg of `create_team` / `update_team` | section 3.4 | phase 3 |
| Event names | `"session_coordinator_changed"`, `"coordinator_clock_updated"`, `"coordinator_auto_close_changed"`, `"coordinator_manual_close_changed"` | phase 3 |
| `data-ac-testid` values | `ActionBar.tsx:301`; `ProjectPanel.tsx:4198,4213,4221`; `SettingsModal.tsx:1914,1932,1944,1963,1979,1998` (10 values) | phase 3 |
| Machine-readable error codes | `"selectionCoordinatorUnavailable"`, `"selectionCoordinatorBusy"`, `"selectionCoordinatorRecursiveSubmission"` | phase 3 (stated by #1572 itself) |
| On-disk file names | `"Context.coordinator.md"` (`session_context.rs:13`), `"coordinator_clocks.json"` and `"coordinator_clocks.json.*.tmp"` (`instance_artifacts.rs:129,134`), `"context:coordinator"` manifest scope | phase 3 |
| CLI flag names | `--coordinator`, `--busy-coordinator` | phase 3 |
| Frozen historical template **content** | `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND`, `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION`, `_BEFORE_CROSS_WORKGROUP_RULE`, `_BEFORE_ORCHESTRATOR_RENAME` and every other frozen snapshot | never; frozen by design |
| Test-title strings, `expect()`/`panic!` prose, log format strings, comment prose that does not name a symbol | ~340 literals | phase 4 (residual elimination) |
| CSS class and id names (`.coordination-proof`, `#coordination-proof-title`, `.cdemo-*`, `COORD_IDLE_CLASS`) and every `coordinate` / `coordination` word | | not in the epic's grep domain |
| `CHANGELOG.md`, `plans/`, `docs/` | | epic decision 2 and phase 4 |
| Any behavior change | | separate issue, per epic non-goals |

---

## 5. Decided solution

Five rules. They are total: every occurrence in the tree is decided by exactly one of them, and
Rules S, K and P beat Rule A wherever they apply.

### 5.1 Rule A: Concept A substitution

Inside an **identifier** (a Rust or TypeScript name, never inside a string literal), replace the
whole word, preserving the case shape and touching nothing else in the identifier:

| Found | Becomes |
| --- | --- |
| `coordinator` | `orchestrator` |
| `Coordinator` | `Orchestrator` |
| `COORDINATOR` | `ORCHESTRATOR` |
| `coordinators` | `orchestrators` |
| `Coordinators` | `Orchestrators` |

Worked examples, all measured sites: `is_coordinator` becomes `is_orchestrator`;
`resolve_wg_coordinator_replica` becomes `resolve_wg_orchestrator_replica`;
`COORDINATOR_CONTEXT_TEMPLATE_FILENAME` becomes `ORCHESTRATOR_CONTEXT_TEMPLATE_FILENAME`;
`CoordinatorClocksState` becomes `OrchestratorClocksState`; `BusyCoordinatorPolicy` becomes
`BusyOrchestratorPolicy`; `LoopTargetKind::WorkgroupCoordinator` becomes
`LoopTargetKind::WorkgroupOrchestrator`; `V1CoverageBoundary::CoordinatorStatelessV2ToV4` becomes
`V1CoverageBoundary::OrchestratorStatelessV2ToV4`; `pendingCoordinatorClose` becomes
`pendingOrchestratorClose`; `__resetCoordinatorCloseModalHostForTests` becomes
`__resetOrchestratorCloseModalHostForTests`.

Rule A applies to test function names, test-local helpers, fixture builders and local bindings
exactly as it applies to production symbols. It does **not** apply to `coordinate`, `coordinates`,
`coordination`, `coordinating` (correction 3 of section 3.1).

The four frozen-template constants keep their frozen bytes and take the new identifier
(`ORCHESTRATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME` and its three siblings). This is
deliberate and is not the epic's decision 2: decision 2 protects historical **records**
(`CHANGELOG.md`, `plans/`), and the constants' historical payload is their string value, which does
not change by a single byte. The identifier names which template slot the snapshot belongs to, and
that slot is now the Orchestrator context template. The doc comment above each constant states which
release it froze.

### 5.2 Rule B: Concept B substitution

| Current | Becomes |
| --- | --- |
| `SelectionCoordinator` | `SelectionArbiter` |
| `SelectionCoordinatorError` | `SelectionArbiterError` |
| `CoordinatorInner` | `ArbiterInner` |
| `CoordinatorJob` | `ArbiterJob` |
| `CoordinatorEnvelope` | `ArbiterEnvelope` |
| `CoordinatorPhase` | `ArbiterPhase` |
| `COORDINATOR_QUEUE_CAPACITY` | `ARBITER_QUEUE_CAPACITY` |
| `COORDINATOR_ADMISSION_CAPACITY` | `ARBITER_ADMISSION_CAPACITY` |
| `QuarantineRetryPath::Coordinator` | `QuarantineRetryPath::Arbiter` (+ Rule S pin) |
| local Rust bindings `coordinator`, `selection_coordinator`, `running_coordinator` in Concept B code | `arbiter`, `selection_arbiter`, `running_arbiter` |
| TypeScript `isSelectionCoordinatorBusyError` | `isSelectionArbiterBusyError` |

The three error-code **strings** stay `selectionCoordinator*` (out of scope, phase 3). A function
named `isSelectionArbiterBusyError` that compares against the literal `"selectionCoordinatorBusy"`
is the intended, temporary state; section 10.2 records it as an accepted residual.

Concept B files, verified line by line: `session/selection.rs` (owner),
`commands/resource_monitor.rs`, `resource_monitor/watchdog.rs`, `commands/window.rs`,
`commands/session.rs`, `lib.rs`, `session/auto_close.rs`, `web/commands.rs`, `phone/mailbox.rs`,
`testability/ui_automation.rs`, `screenshot/windows.rs`, and the three integration tests
`tests/wake_consumption_measure.rs`, `tests/pty_powershell_managed_native.rs`,
`tests/pty_lifecycle_regression.rs`.

### 5.3 Rule S: pin every serialised member

For each of the 28 members in section 3.3, rename the Rust identifier by Rule A or Rule B **and**
add, on the same member, an explicit serde rename to the key it produces today. Where a `#[serde(..)]`
attribute already exists, the rename is added inside it, never as a second attribute.

```rust
// before
#[serde(default = "default_true")]
pub coordinator_auto_close_enabled: bool,

// after
#[serde(default = "default_true", rename = "coordinatorAutoCloseEnabled")]
pub orchestrator_auto_close_enabled: bool,
```

```rust
// before
#[serde(rename_all = "snake_case")]
pub enum PtyInputReasonCode { .. SenderNotCoordinator, .. }

// after
#[serde(rename_all = "snake_case")]
pub enum PtyInputReasonCode { .. #[serde(rename = "sender_not_coordinator")] SenderNotOrchestrator, .. }
```

`rename` overrides `rename_all`, so the emitted and accepted key is unchanged in both directions.
`legacy_start_only_coordinators` at `settings.rs:310` is already pinned; it gains the identifier
rename only, its existing `rename = "startOnlyCoordinators"` is untouched.

Adding a pin is not a behavior change: it makes the wire format explicit at exactly the value it
already had, and section 9 proves that by round-tripping the keys.

### 5.4 Rule P: literals are frozen, with an exact allowlist

**No string literal changes anywhere in either repo, except these nine.** The allowlist is closed;
anything else is a defect.

| # | Literal today | Becomes | Site |
| --- | --- | --- | --- |
| L1 | `"./coordinator-badge"` | `"./orchestrator-badge"` | `src/shared/coordinator-badge.test.ts:2` |
| L2 | `"../../shared/coordinator-badge"` | `"../../shared/orchestrator-badge"` | `src/sidebar/components/coordinator-badge-class.ts:1`, `coordinator-badge-class.test.ts:3`, `ProjectPanel.tsx:52` |
| L3 | `"./coordinator-badge-class"` | `"./orchestrator-badge-class"` | `src/sidebar/components/coordinator-badge-class.test.ts:4`, `ProjectPanel.tsx:53` |
| L4 | `"./coordinator-close"` | `"./orchestrator-close"` | `src/sidebar/stores/coordinator-close.test.ts:15` |
| L5 | `"../stores/coordinator-close"` | `"../stores/orchestrator-close"` | `src/sidebar/components/ProjectPanel.tsx:12`, `SessionItem.tsx:9` |
| L6 | `"../sidebar/stores/coordinator-close"` | `"../sidebar/stores/orchestrator-close"` | `src/shared/shortcuts.ts:3` |
| L7 | the 7 `CoordinatorJob` literals of section 3.5 item 1 | `ArbiterJob` in each | `src-tauri/src/session/selection.rs:3909,3910,3912,3926,4049,4053` and the managed-handle message |
| L8 | the pinned call-site text containing `,is_coordinator,` | `,is_orchestrator,` | `src-tauri/tests/cli_workgroup_team.rs:1836-1838` |
| L9 | the `Debug` field labels `"is_coordinator"`, `"sender_is_coordinator"`, `"VerifiedBoundContainerCoordinator"` and the assertion `"is_coordinator: true"` | `"is_orchestrator"`, `"sender_is_orchestrator"`, `"VerifiedBoundContainerOrchestrator"`, `"is_orchestrator: true"` | `src-tauri/src/config/teams.rs:583,603,2360`; `src-tauri/src/session/manager.rs:63`; `src-tauri/src/api/identity.rs:107` |

L1 to L6 are ES module specifiers: they are file paths, not data, and they must move with the file.
L7 to L9 are source text that a test reads back out of the tree; they are the identifier, quoted.

The website repo's allowlist is separate and equally closed: the i18n key `"composer.coordinator"`
becomes `"composer.orchestrator"` at its 8 sites, and `'./CoordinationDemo.css'` becomes
`'./OrchestrationDemo.css'` at `CoordinationDemo.tsx:3`, `'./CoordinationDemo.tsx'` becomes
`'./OrchestrationDemo.tsx'` at `CoordinationProof.astro:2`. No locale **value** changes: phase 1
already set them to ORCHESTRATOR / ORQUESTADOR / ORQUESTRADOR / ORCHESTRATEUR / 编排者.

### 5.5 Rule K: TypeScript members that mirror a wire key are frozen

A TypeScript identifier is **out of scope** if and only if it is a property name, an interface or
type member name, or a destructuring binding whose text is a key produced by a Rust serde member of
section 3.3 or by a Tauri command argument of section 3.4. Everywhere else, Rule A applies, including
to local variables and functions that happen to be spelled like a wire key.

Frozen by Rule K: `isCoordinator` as a member of `Session`, `SessionInfo`, `AcAgentReplica`,
`PersistedSession` and their fixtures; `coordinator` as a member of `AcTeam` and of the
`create_team` / `update_team` argument objects; `busyCoordinator`; `restoreCoordinatorWakeState`;
`coordinatorIdleBadgeYellowMinutes`; `coordinatorIdleBadgeRedMinutes`; `coordinatorAutoCloseEnabled`;
`coordinatorAutoCloseMinutes`; `coordinatorAutoCloseSkipTelegramAssigned`;
`coordinatorCascadeCloseEnabled`; the `LoopTargetKind` value `"workgroupCoordinator"`.

**Ambiguity register.** These are the complete set of sites where a frozen name also occurs as an
in-scope local. They are decided here so the implementer makes no judgment call.

| Site | What it is | Verdict |
| --- | --- | --- |
| `src/shared/ipc.ts:1037` | `const coordinator = value.coordinator;` | the `const` renames, `value.coordinator` does not |
| `src/shared/types.ts:1179` | `TeamSessionGroup.coordinator: Session \| null` | frontend-only aggregate, **renames** |
| `src/shared/types.ts:1193` | `Team.coordinatorName?: string` | frontend-only, **renames** |
| `src/shared/types.ts:1232` | `AcTeam.coordinator: string \| null` | wire key, frozen |
| `src/shared/types.ts:40`, `:1243` | `isCoordinator: boolean` | wire key, frozen |
| `src/sidebar/components/AcDiscoveryPanel.tsx:43` | `const isCoordinator = (agentName: string): boolean =>` | local function, **renames**; `t.coordinator` at `:44` frozen |
| `src/sidebar/components/EditTeamModal.tsx:32`, `:362` | signal `coordinator`/`setCoordinator`, local memo `isCoordinator` | **rename**; `teamConfig.coordinator` and the IPC arg frozen |
| `src/sidebar/components/NewTeamModal.tsx:36`, `:309` | signal `coordinator`/`setCoordinator`, local memo `isCoordinator` | **rename**; `coordinator:` at `:201`, `request.coordinator` at `:216`, `name="coordinator"` at `:324` frozen |
| `src/sidebar/components/loop-modal-helpers.ts:18`, `:22` | `const coordinator = ...`, `coordinatorName:` | **rename**; `agent.isCoordinator` frozen |
| `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx:27,29,145` | test-helper option `coordinator?: boolean` | **renames**; `isCoordinator: coordinator` at `:40` becomes `isCoordinator: orchestrator` |
| `src/sidebar/stores/sessions.ts:249`, `:279` | `let coordinator: Session \| null` and the `TeamSessionGroup` field | **rename** |
| `src/sidebar/App.tsx:801` | `isCoordinator` bound by destructuring the `session_coordinator_changed` payload | frozen (a destructuring binding of a wire key); `setIsCoordinator` renames |
| `src/shared/ipc.ts:754`, `:756` | `{ sessionId: string; isCoordinator: boolean }` payload type | frozen |

`src/sidebar/styles/coord-quick-access-css.test.ts` and `COORD_IDLE_CLASS` contain no `coordinator`
and are untouched.

### 5.6 The 7 file renames in `repo-AgentsCommander`

| # | From | To |
| --- | --- | --- |
| R1 | `src-tauri/src/config/coordinator_clocks.rs` | `src-tauri/src/config/orchestrator_clocks.rs` |
| R2 | `src/shared/coordinator-badge.ts` | `src/shared/orchestrator-badge.ts` |
| R3 | `src/shared/coordinator-badge.test.ts` | `src/shared/orchestrator-badge.test.ts` |
| R4 | `src/sidebar/components/coordinator-badge-class.ts` | `src/sidebar/components/orchestrator-badge-class.ts` |
| R5 | `src/sidebar/components/coordinator-badge-class.test.ts` | `src/sidebar/components/orchestrator-badge-class.test.ts` |
| R6 | `src/sidebar/stores/coordinator-close.ts` | `src/sidebar/stores/orchestrator-close.ts` |
| R7 | `src/sidebar/stores/coordinator-close.test.ts` | `src/sidebar/stores/orchestrator-close.test.ts` |

R1 also requires `src-tauri/src/config/mod.rs:12` to become `pub mod orchestrator_clocks;` and every
`config::coordinator_clocks::` path in 12 other files to follow. R2 to R7 require the 10 module
specifiers of Rule P allowlist entries L1 to L6.

### 5.7 The 3 file renames in `repo-agentscommander_webpage`

| # | From | To |
| --- | --- | --- |
| W1 | `src/components/CoordinationDemo.tsx` | `src/components/OrchestrationDemo.tsx` |
| W2 | `src/components/CoordinationDemo.css` | `src/components/OrchestrationDemo.css` |
| W3 | `src/components/CoordinationProof.astro` | `src/components/OrchestrationProof.astro` |

Content follow-ups: the component identifier `CoordinationDemo` at `OrchestrationDemo.tsx:74`, `:196`
and `OrchestrationProof.astro:2`, `:23` becomes `OrchestrationDemo`; the two import specifiers of
section 5.4; and `README.md:37` `CoordinationDemo (Solid island)` becomes `OrchestrationDemo (Solid island)`.
The CSS selectors `.coordination-proof` and `#coordination-proof-title` inside
`OrchestrationProof.astro:6,7,12,29` are **not** renamed: they carry the word "coordination", which
is outside the epic's grep domain.

### 5.8 The new test for the sidebar filter token (issue item 3c)

`src/sidebar/components/ProjectPanel.tsx:975` and `:1016` inject the synthetic filter token
`"orchestrator"` for an orchestrator row (`replica.isCoordinator ? "orchestrator" : null` and
`agentName === team.coordinator ? "orchestrator" : null`). No test in
`ProjectPanel.regex-filter.test.tsx` types that literal, so the behavior phase 1 changed is unpinned.

Add exactly one `it(...)` to `src/sidebar/components/ProjectPanel.regex-filter.test.tsx`, modelled on
the existing paired-row test at `:493`:

- title: `it("matches an orchestrator row by the synthetic filter token, and a member row not at all (#1572 / R11)")`
- body: render the existing project fixture that already carries one orchestrator replica and one
  non-orchestrator member, open the filter, type the literal `orchestrator`, assert the orchestrator
  row is still present and the member row is not.
- It must type the literal `orchestrator`. Do not build it from a constant, or the test proves
  nothing about the token.

One assertion pair, no new fixture, no new helper.

---

## 6. Affected surfaces: ADDED, REMOVED, MODIFIED

Every planned path appears in exactly one table of its repo. A renamed path appears in REMOVED under
its old name and in ADDED under its new name; the old path never appears in MODIFIED.

### 6.1 `repo-AgentsCommander`: ADDED (8)

| Path completo archivo | Que se modifico |
| --- | --- |
| `plans/1572-orchestrator-internal-identifiers.md` | This plan. |
| `src-tauri/src/config/orchestrator_clocks.rs` | `git mv` of `config/coordinator_clocks.rs` (commit C1), then Rule A over `CoordinatorClocks`, `CoordinatorClocksState`, `COORDINATOR_CLOCKS_FILE_NAME` and the module's own doc header (commit C2). File contents otherwise identical. |
| `src/shared/orchestrator-badge.ts` | `git mv` of `shared/coordinator-badge.ts`, then Rule A over `CoordinatorBadge`, `CoordinatorIdleLevel`, `coordinatorIdleBadge`; the two settings keys it reads stay frozen by Rule K. |
| `src/shared/orchestrator-badge.test.ts` | `git mv` of `shared/coordinator-badge.test.ts`, then Rule A plus the L1 specifier. |
| `src/sidebar/components/orchestrator-badge-class.ts` | `git mv`, then Rule A over `CoordinatorIdleLevel` plus the L2 specifier and the file-name reference in its own comment at line 6. |
| `src/sidebar/components/orchestrator-badge-class.test.ts` | `git mv`, then Rule A plus the L2 and L3 specifiers. |
| `src/sidebar/stores/orchestrator-close.ts` | `git mv`, then Rule A over the 11 store symbols (`PendingCoordinatorClose`, `pendingCoordinatorClose`, `setPendingCoordinatorClose`, `confirmPendingCoordinatorClose`, `requestCoordinatorClose`, `requestCoordinatorCloseById`, `registerCoordinatorCloseModalHost`, `__resetCoordinatorCloseModalHostForTests`, `coordinatorCloseModalHostAvailable`, `closeCoordinator`, and the local `isCoordinator` uses that are not wire members). |
| `src/sidebar/stores/orchestrator-close.test.ts` | `git mv`, then Rule A plus the L4 specifier. Its `describe("coordinator-close helper (#588)")` title is a literal and stays frozen (phase 4). |

### 6.2 `repo-AgentsCommander`: REMOVED (7)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src-tauri/src/config/coordinator_clocks.rs` | Renamed to `src-tauri/src/config/orchestrator_clocks.rs` by a pure `git mv` in commit C1. |
| `src/shared/coordinator-badge.ts` | Renamed to `src/shared/orchestrator-badge.ts`, pure `git mv`, C1. |
| `src/shared/coordinator-badge.test.ts` | Renamed to `src/shared/orchestrator-badge.test.ts`, pure `git mv`, C1. |
| `src/sidebar/components/coordinator-badge-class.ts` | Renamed to `.../orchestrator-badge-class.ts`, pure `git mv`, C1. |
| `src/sidebar/components/coordinator-badge-class.test.ts` | Renamed to `.../orchestrator-badge-class.test.ts`, pure `git mv`, C1. |
| `src/sidebar/stores/coordinator-close.ts` | Renamed to `.../orchestrator-close.ts`, pure `git mv`, C1. |
| `src/sidebar/stores/coordinator-close.test.ts` | Renamed to `.../orchestrator-close.test.ts`, pure `git mv`, C1. |

No file is deleted outright.

### 6.3 `repo-AgentsCommander`: MODIFIED, Rust production (54)

Rule A unless the row says otherwise. "S-pin" means Rule S applies in that file.

| Path completo archivo | Que se modifico |
| --- | --- |
| `src-tauri/src/api/error.rs` | `SenderNotCoordinator`, `TargetIsCoordinator` variant references. |
| `src-tauri/src/api/handlers/pty_input.rs` | Same two variant references. |
| `src-tauri/src/api/handlers/terminal_snapshot.rs` | `BoundContainerCoordinatorError`, `NotCoordinator`, `verify_final_bound_container_coordinator`, `verify_live_bound_container_coordinator`. |
| `src-tauri/src/api/identity.rs` | `BoundContainerCoordinatorError` (enum at :87), `VerifiedBoundContainerCoordinator` (struct at :95), `verify_live_bound_container_coordinator` (:112), `verify_final_bound_container_coordinator` (:190), and the `debug_struct` label at :107 (allowlist L9). |
| `src-tauri/src/cli/close_session.rs` | `is_coordinator_of` call. |
| `src-tauri/src/cli/list_peers.rs` | 18 identifiers: `verified_wg_coordinator_target`, `discover_root_coordinator_peers*`, `coordinator_name`, the peer test helpers. Its `--help` const already says "orchestrator" (phase 1) and is a literal: frozen. |
| `src-tauri/src/cli/list_sessions.rs` | `is_coordinator` field read. |
| `src-tauri/src/cli/loop_cmd.rs` | `BusyCoordinatorCli`, `BusyCoordinatorPolicy`, `WorkgroupCoordinator`, `busy_coordinator`. The flag name `--busy-coordinator` and the value `"busy-coordinator"` are literals: frozen. |
| `src-tauri/src/cli/send.rs` | 10 identifiers around `is_coordinator*` and the send authorization path. |
| `src-tauri/src/cli/task_append_body.rs` | `is_any_coordinator` and one test fn name. |
| `src-tauri/src/cli/task_set_title.rs` | `is_any_coordinator` and one test fn name. |
| `src-tauri/src/cli/team.rs` | **S-pin** on `TeamListItem::coordinator` (:104), `TeamCreateResult::coordinator` (:115), `AddMemberResult::coordinator` (:127); plus the test helper `make_coordinator`. |
| `src-tauri/src/cli/terminal_snapshot.rs` | `is_coordinator` read. |
| `src-tauri/src/cli/workgroup.rs` | `coordinator` local and `config::coordinator_clocks` path. |
| `src-tauri/src/commands/ac_discovery.rs` | **S-pin** on `AcTeam::coordinator` (:84) and `AcAgentReplica::is_coordinator` (:112); the `coordinator_clocks` State args at :1040 and :1826; 11 identifiers total. |
| `src-tauri/src/commands/entity_creation.rs` | **S-pin** on `TeamConfigResult::coordinator` (:59); the `coordinator: String` value args of `create_team` (:2774) and `update_team` (:3348) stay frozen; `config::coordinator_clocks` paths; 12 identifiers total. |
| `src-tauri/src/commands/loops.rs` | **S-pin** on `LoopCreateRequest::busy_coordinator` (:30) and `LoopUpdateRequest::busy_coordinator` (:45); `WorkgroupCoordinator` variant. |
| `src-tauri/src/commands/pty.rs` | `CoordinatorClocks`, `CoordinatorClocksState`, `coordinator_clocks`, `coordinator_cwd`. The four `[coordinator-clocks]` log prefixes are literals: frozen. |
| `src-tauri/src/commands/resource_monitor.rs` | Rule B: `SelectionCoordinator*` and `QuarantineRetryPath::Coordinator` at :1737 and :1827. |
| `src-tauri/src/commands/session.rs` | 16 identifiers. Rule A on `coordinator_clocks`, `coordinator_id`, `coordinator_matrix`, `coordinator_cascade_close_enabled`, `execute_manual_coordinator_destroy`, `is_coordinator`, `is_coordinator_for_cwd`, `CoordinatorCloseOutcome`; Rule B on `SelectionCoordinator` and the `coordinator: State<..>` arg at :4582. **The `close_coordinator` fn name at :3406 does not change.** The `is_coordinator` argument at the `materialize_agent_context_file_with_filename_activated` call site changes and drags allowlist entry L8. |
| `src-tauri/src/commands/window.rs` | Rule B: `SelectionCoordinator` and its local binding. |
| `src-tauri/src/config/activity_log.rs` | `is_coordinator` read. |
| `src-tauri/src/config/agent_config.rs` | **S-pin** on `AgentDarkFactory::is_coordinator_of` (:95). |
| `src-tauri/src/config/instance_artifacts.rs` | `COORDINATOR_CLOCKS_FILE_NAME` (:129) and `COORDINATOR_CLOCKS_TMP_GLOB` (:134) identifiers; their **values** `"coordinator_clocks.json"` and `"coordinator_clocks.json.*.tmp"` are frozen. One test fn name. |
| `src-tauri/src/config/loops.rs` | **S-pin** on `LoopTargetKind::WorkgroupCoordinator` (:25), `LoopPolicy::busy_coordinator` (:98), `LoopAuditEntry::busy_coordinator_policy` (:153), `AcLoopSummary::busy_coordinator` (:169); plus `BusyCoordinatorPolicy` (7 identifiers). |
| `src-tauri/src/config/mod.rs` | `pub mod coordinator_clocks;` at :12 becomes `pub mod orchestrator_clocks;`. |
| `src-tauri/src/config/projects.rs` | `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` use, `coordinator_bytes`, `coordinator_template`. |
| `src-tauri/src/config/root_agent.rs` | 5 identifiers. Every template string it holds is frozen. |
| `src-tauri/src/config/seed_manifest.rs` | `V1CoverageBoundary::CoordinatorStatelessV2ToV4`, `::CoordinatorStatelessV3ToV4`, `::CoordinatorSeededV3ToV4` at :3279-3281 and :5913-5915. The enum is `pub(crate)` and not serde-derived. |
| `src-tauri/src/config/seeded_context_templates.rs` | 22 identifiers, including the four frozen-snapshot constants (identifier only, bytes frozen) and `get_default_coordinator_template`, `is_known_generated_coordinator_template`, and the byte-exactness test fn names. |
| `src-tauri/src/config/session_context.rs` | 21 identifiers, including `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` at :13 (its value `"Context.coordinator.md"` is frozen), `coordinator_path`, `coordinator_content`, `coordinator_template_path`. |
| `src-tauri/src/config/sessions_persistence.rs` | **S-pin** on `PersistedSession::is_coordinator` (:377); one test fn name. |
| `src-tauri/src/config/settings.rs` | **S-pin** on the 7 members at :294, :527, :529, :532, :534, :538, :542; identifier-only change on `legacy_start_only_coordinators` at :310, whose existing `rename = "startOnlyCoordinators"` is untouched. All `"startOnlyCoordinators"` / `"restoreCoordinatorWakeState"` / `"coordinator*"` literals in the migration and its tests are frozen. |
| `src-tauri/src/config/teams.rs` | The largest Rust file in the phase, 39 identifiers: `is_coordinator`, `is_coordinator_of` (:1619), `is_any_coordinator` (:1626), `is_coordinator_for_cwd` (:1636), `resolve_wg_coordinator_replica` (:442), `verified_wg_coordinator_target` (:493), `verify_pty_input_coordinator_root` (:1038), `DiscoveredTeam::coordinator_name` (:37) and `::coordinator_path` (:39) with **no pin** (not serde), `validate_coordinator_to_root_route`, and the test fn names. Allowlist L9 covers the `Debug` labels at :583 and :603 and the assertion at :2360. |
| `src-tauri/src/lib.rs` | 11 identifiers: `CoordinatorClocksState`, `coordinator_clocks`, `coordinator_clocks_for_exit`, `is_any_coordinator`, `is_coordinator`, `restore_coordinator_wake_state`, and Rule B on `SelectionCoordinator`, `selection_coordinator`, `selection_coordinator_for_exit`, `selection_coordinator_for_setup`. **The `close_coordinator` entry in the `generate_handler!` list, `lib.rs:3370`, does not change.** |
| `src-tauri/src/loops/delivery.rs` | 8 identifiers around the busy-orchestrator policy and target resolution. |
| `src-tauri/src/loops/scheduler.rs` | `BusyCoordinatorPolicy`, `WorkgroupCoordinator`, `busy_coordinator`, `busy_coordinator_policy`. |
| `src-tauri/src/phone/mailbox.rs` | 23 identifiers on the routing and authorization path. Every FQN fixture literal is frozen. |
| `src-tauri/src/phone/types.rs` | **S-pin** on `PtyInputReasonCode::SenderNotCoordinator` (:82) and `::TargetIsCoordinator` (:85). |
| `src-tauri/src/pty/container_backend.rs` | One test fn name. |
| `src-tauri/src/pty/container_tokens.rs` | `verify_pty_input_coordinator_root` call. |
| `src-tauri/src/pty/git_watcher.rs` | `CoordinatorChangedPayload` (:414) type name renames freely (a struct type name is not serialised); **S-pin** on its `is_coordinator` field (:416). |
| `src-tauri/src/pty/inject.rs` | `is_coordinator` read. |
| `src-tauri/src/pty/terminal_snapshot.rs` | `augment_coordinator_project`, `is_coordinator`, `verify_pty_input_coordinator_root`. |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | 6 identifiers; the FQN fixture literals are frozen. |
| `src-tauri/src/resource_monitor/watchdog.rs` | Rule B: `SelectionCoordinator`, `running_coordinator`, and **S-pin** on `QuarantineRetryPath::Coordinator` (:35) which becomes `::Arbiter` with `#[serde(rename = "coordinator")]`. |
| `src-tauri/src/screenshot/windows.rs` | Rule B: one `SelectionCoordinator` reference. |
| `src-tauri/src/session/auto_close.rs` | 17 identifiers, Rule A on the clock and cascade path, Rule B on the arbiter handle. The three event-name literals are frozen. |
| `src-tauri/src/session/context_alerts.rs` | 6 identifiers. |
| `src-tauri/src/session/manager.rs` | 16 identifiers, including `coordinator_refs_by_team`, `coordinator_ids_by_team`, `coordinator_cwd`; allowlist L9 covers the `Debug` label at :63. |
| `src-tauri/src/session/selection.rs` | Concept B owner. All 8 Concept B symbols plus every local binding, and allowlist L7 for the 7 sentinel literals. The 6 Concept A lines the epic measured in this file take Rule A. |
| `src-tauri/src/session/session.rs` | **S-pin** on `Session::is_coordinator` (:122) and `SessionInfo::is_coordinator` (:298). |
| `src-tauri/src/testability/ui_automation.rs` | Rule B on `SelectionCoordinator`; Rule A on `automation_app_with_coordinator` and the local `coordinator`. |
| `src-tauri/src/web/commands.rs` | `CoordinatorClocksState`, `coordinator_clocks`, Rule B on `SelectionCoordinator` and its local. |

`src-tauri/src/testability/window_placement.rs` is deliberately **absent**: its only hit,
`env_json_parses_negative_coordinates`, is the geometry word.

### 6.4 `repo-AgentsCommander`: MODIFIED, Rust integration tests (6)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src-tauri/tests/cli_loop.rs` | Test helper `project_with_verified_coordinator`. The `--busy-coordinator` flag strings and the `"busyCoordinator"` JSON assertions are frozen. |
| `src-tauri/tests/cli_project_registration.rs` | `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` use at :514. The `"context:coordinator"` manifest assertions are frozen. |
| `src-tauri/tests/cli_workgroup_team.rs` | Test fn `team_create_outputs_coordinator_first_roster`, plus allowlist L8 at :1836-1838. The `--coordinator` flag strings and `config["coordinator"]` assertions are frozen. |
| `src-tauri/tests/pty_lifecycle_regression.rs` | Rule B: `SelectionCoordinator` import at :25 and `selection_coordinator` local. |
| `src-tauri/tests/pty_powershell_managed_native.rs` | Rule B: import at :50 and local. |
| `src-tauri/tests/wake_consumption_measure.rs` | Rule B: import at :65 and local. |

### 6.5 `repo-AgentsCommander`: MODIFIED, TypeScript (27)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src/shared/ipc.ts` | `CoordinatorCloseOutcome`, `closeCoordinator` (:213), `onSessionCoordinatorChanged` (:753), `onCoordinatorClockUpdated` (:779), `onCoordinatorAutoCloseChanged` (:788), `onCoordinatorManualCloseChanged` (:797), `isSelectionCoordinatorBusyError` (Rule B), and the local `const coordinator` at :1037. The `"close_coordinator"` invoke name at :214, the four event-name literals, the `{ isCoordinator: boolean }` payload types at :754/:756, `busyCoordinator` at :971/:985 and the `coordinator: string` command args at :1114/:1134 are all frozen. |
| `src/shared/ipc.transport.test.ts` | `isSelectionCoordinatorBusyError` call at :134. Every `"selectionCoordinator*"` string and every `coordinator:` team-config fixture key is frozen. |
| `src/shared/shortcuts.ts` | `requestCoordinatorCloseById` import and call, plus allowlist L6 at :3. |
| `src/shared/types.ts` | `BusyCoordinatorPolicy` (:1322, type alias name; its three string values are unchanged), `CoordinatorCloseOutcome`, `TeamSessionGroup.coordinator` (:1179), `Team.coordinatorName` (:1193). Frozen: `isCoordinator` at :40 and :1243, `AcTeam.coordinator` at :1232, the settings keys, `busyCoordinator` at :1338, `LoopTargetKind = "workgroupCoordinator"` at :1320. |
| `src/sidebar/App.tsx` | `onSessionCoordinatorChanged`, `setIsCoordinator`, `isSelectionCoordinatorBusyError`. The destructured `isCoordinator` payload binding at :801 is frozen. |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | The local function `isCoordinator` at :43 and the comment at :42. `t.coordinator` at :44 frozen. |
| `src/sidebar/components/EditLoopModal.tsx` | `coordinatorOptions` (:30), `coordinatorOptionsFromWorkgroups` import. `busyCoordinator` frozen. |
| `src/sidebar/components/EditTeamModal.tsx` | Signal `coordinator`/`setCoordinator` (:32), `coordinatorEntry` (:140), `coordinatorRef` (:139), `nextCoordinator` (:137), local memo `isCoordinator` (:362). |
| `src/sidebar/components/NewLoopModal.tsx` | `coordinatorOptions`, `coordinatorOptionsFromWorkgroups`. |
| `src/sidebar/components/NewLoopModal.test.ts` | `coordinatorName` fixture field (:89) and `coordinatorOptionsFromWorkgroups`. `busyCoordinator` and `isCoordinator` frozen. |
| `src/sidebar/components/NewTeamModal.tsx` | Signal `coordinator`/`setCoordinator` (:36), local memo `isCoordinator` (:309). Frozen: the IPC arg at :201, `request.coordinator` at :216, `name="coordinator"` at :324. |
| `src/sidebar/components/NewTeamModal.context-alerts.test.tsx` | Local `coordinatorRadio` (:67). The `coordinator:` team-config fixture keys are frozen. |
| `src/sidebar/components/ProjectPanel.context-menu-hover.test.tsx` | Local `openCoordinatorMenu` (:187). |
| `src/sidebar/components/ProjectPanel.context-menu.test.tsx` | Local helper `projectDiscoveryWithCoordinatorRepos` (:81). |
| `src/sidebar/components/ProjectPanel.groups-filter.test.tsx` | Local `openCoordinatorMenu`. |
| `src/sidebar/components/ProjectPanel.regex-filter.test.tsx` | **One new `it(...)`** per section 5.8. No existing assertion changes; every `coordinator:` fixture key stays. |
| `src/sidebar/components/ProjectPanel.repo-browse.automation.test.tsx` | Local helper `coordinatorAgent` (:144). |
| `src/sidebar/components/ProjectPanel.repo-browse.test.tsx` | Local helper `coordinatorAgent`. |
| `src/sidebar/components/ProjectPanel.tsx` | The largest TypeScript file in the phase: `coordinatorItemKey` (:347), `runningCoordinatorPeers` (:355), `coordinatorsCollapsedKey` (:1871), `coordinatorPairCache` (:1879), `naturalCoordinatorItems` (:1880), `coordinatorItems` (:1908), `recordCoordinatorVisibleOrder` (:1911), `coordinatorVisibleOrder` (:1916), `selectedCoordinatorItem` (:1923), `filteredCoordinatorItems` (:1940), the six imports from the renamed stores and modules, `coordinatorIdleBadge`, the four `on*` listeners. Frozen: `replica.isCoordinator` reads, the `"coordinators"` collapse-key literal at :1871, the `"orchestrator"` filter tokens at :975 and :1016, the three `coordinatorClose.*` testids at :4198, :4213, :4221, and allowlist entries L2, L3, L5 for its three specifiers. |
| `src/sidebar/components/SessionItem.tsx` | `requestCoordinatorClose` import and call, plus allowlist L5 at :9. |
| `src/sidebar/components/SessionItem.test.tsx` | Locals `nonCoordinator` (:343) and `noRepoCoordinator` (:367). |
| `src/sidebar/components/SettingsModal.tsx` | Local `validateCoordinatorIdle` (:1652). All seven settings keys and the six `settings.general.coordinator*` testids are frozen. |
| `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx` | Test-helper option `coordinator?: boolean` (:27, :29, :145) and its use at :40. |
| `src/sidebar/components/loop-modal-helpers.ts` | `LoopCoordinatorOption` (:3), `coordinatorName` (:5, :22), `coordinatorOptionsFromWorkgroups`, the local `coordinator` (:18). `agent.isCoordinator` and `BusyCoordinatorPolicy`-typed members frozen except the type-alias name itself. |
| `src/sidebar/stores/sessions.ts` | `lastCoordinatorVisibleOrderByProject` and its setter (:14), `frozenCoordinatorVisibleOrderByProject` and its setter (:15), `recordCoordinatorVisibleOrder`, `coordinatorVisibleOrder`, `setIsCoordinator`, the local `coordinator` (:249, :279) and `team.coordinatorName` (:255, :269). |
| `src/sidebar/stores/sessions-helpers.test.ts` | `coordinatorVisibleOrder`, `recordCoordinatorVisibleOrder`. |
| `src/terminal/App.tsx` | `isSelectionCoordinatorBusyError` (Rule B). |

### 6.6 `repo-AgentsCommander`: MODIFIED, data (1)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src-tauri/module-arcs.txt` | Regenerated by the two-step pipeline of section 9.4. 14 lines move; the arc set is unchanged under the renaming bijection. Expected post SHA-256 `2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6`, 1037 lines, 82163 bytes. |

### 6.7 `repo-agentscommander_webpage`: ADDED (3)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src/components/OrchestrationDemo.tsx` | `git mv` of `CoordinationDemo.tsx` (commit W1), then the component identifier at :74 and :196 and the CSS import specifier at :3 (commit W2). The `<span class="cdemo-title">workgroup · live coordination</span>` copy at :120 is unchanged. |
| `src/components/OrchestrationDemo.css` | `git mv` of `CoordinationDemo.css`. No content change: its selectors carry `cdemo-`, not `coordinator`. |
| `src/components/OrchestrationProof.astro` | `git mv` of `CoordinationProof.astro`, then the import at :2 and the element at :23. The `.coordination-proof` selectors at :6, :7, :12, :29 and the eyebrow copy at :11 are unchanged. |

### 6.8 `repo-agentscommander_webpage`: REMOVED (3)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src/components/CoordinationDemo.tsx` | Renamed to `src/components/OrchestrationDemo.tsx`, pure `git mv`, W1. |
| `src/components/CoordinationDemo.css` | Renamed to `src/components/OrchestrationDemo.css`, pure `git mv`, W1. |
| `src/components/CoordinationProof.astro` | Renamed to `src/components/OrchestrationProof.astro`, pure `git mv`, W1. |

### 6.9 `repo-agentscommander_webpage`: MODIFIED (3)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src/i18n/landing.ts` | The key `"composer.coordinator"` becomes `"composer.orchestrator"` on lines 71, 176, 278, 381, 486, 585. The six values are untouched. |
| `src/components/alternatives/TeamComposer.astro` | `data-i18n="composer.coordinator"` at :24 and `copy["composer.coordinator"]` at :25 follow the key. |
| `README.md` | Line 37, `CoordinationDemo (Solid island)` becomes `OrchestrationDemo (Solid island)`. |

No page or layout imports `CoordinationProof.astro`, so no consumer site exists to update. No
Playwright spec references the renamed key or the renamed components.

---

## 7. Required behavior, edge cases, failure behavior

1. **Behavior is identical.** No control flow, no condition, no default, no ordering, no timing, no
   allocation site changes. The only semantic construct added anywhere is `#[serde(rename = "...")]`,
   which restates the key serde already derives.
2. **On-disk compatibility is total in both directions.** A `settings.json`, `sessions.json`,
   `coordinator_clocks.json`, team config or loop config written by the previous release loads
   unchanged, and a file written by this build loads in the previous release. Rule S is what makes
   that true; section 9.2 proves it per key.
3. **The IPC contract is unchanged.** Command names, payload keys, event names and error-code strings
   are byte-identical, so a frontend built from this branch and a backend built from `main` remain
   interoperable in both directions.
4. **`ui-query` and the UI automation bridge are unchanged**, because no `data-ac-testid` value moves.
5. **The frozen template snapshots stay byte-exact.** Their four byte-exactness tests hash the
   constant's value, not its name, so renaming the constant cannot move the hash. If one of those
   tests reddens, the run edited a frozen string and must stop.
6. **The pure-`git mv` commit C1 does not build.** This is the direct, unavoidable consequence of the
   commit discipline the issue mandates (a combined rename-plus-rewrite commit lands near the 50
   percent similarity threshold and makes `git log --follow` unreliable). It is accepted, bounded and
   declared: exactly one commit in each repo is not independently buildable, it contains only
   renames, and the branch is validated at its tip. No CI job runs per-commit.
7. **Failure behavior of the run.** Any red gate stops the run at that commit. Recovery is
   `git restore --source=HEAD --staged --worktree -- <the exact paths this commit wrote>`, never a
   repository-wide `git reset --hard` or `git clean`. If a path's current bytes are not this run's
   output, the conflict is reported and the path is left alone (section 13.4).
8. **Edge case, a name that is both.** `commands/session.rs` holds both concepts. Concept B occurrences
   are those reaching `SelectionCoordinator`/`selection::*`; everything else is Concept A. The two
   substitutions must not be run as one blind pass over that file.
9. **Edge case, `close_coordinator` and its outcome type.** The command name stays; its return type
   `CoordinatorCloseOutcome` renames on both sides of the boundary, because a Rust type name is not
   serialised and the TypeScript type is a local alias. `src/shared/ipc.ts:214`
   `transport.invoke<CoordinatorCloseOutcome>("close_coordinator", ...)` becomes
   `transport.invoke<OrchestratorCloseOutcome>("close_coordinator", ...)`.
10. **Edge case, `git log --follow` across a rename plus a same-branch content commit.** The pure
    `git mv` gives 100 percent similarity, so `--follow` picks the rename up at C1 regardless of how
    large the later content commit is. Criterion 5 tests exactly this.

---

## 8. Compatibility and security

**Compatibility.** Zero break, by construction: the diff changes no byte that is written, read,
transported or displayed. The upgrade and downgrade paths both work with no migration, which is why
epic #1570 marks phase 2 "Breaks on-disk compatibility: No". The migration work belongs to phase 3,
and this phase deliberately makes phase 3's job smaller by making every serialised key explicit at
its declaration site instead of implicit in `rename_all`.

**Security.** The authorization surface is renamed, not changed. `is_coordinator`,
`is_coordinator_of`, `is_any_coordinator`, `is_coordinator_for_cwd`, `resolve_wg_coordinator_replica`,
`verified_wg_coordinator_target`, `verify_pty_input_coordinator_root`,
`validate_coordinator_to_root_route` and the two `verify_*_bound_container_coordinator` functions all
keep their exact predicates, their exact call sites and their exact failure values. The reason codes
`sender_not_coordinator` and `target_is_coordinator` keep their wire spelling, so an external
consumer of the API helper's reason detail is unaffected. The existing negative tests
(`is_coordinator_rejects_legacy_unqualified_from`, `is_any_coordinator_requires_qualified_fqn`,
`resolve_wg_coordinator_replica_rejects_spoofed_name`, `..._rejects_spoofed_stale_identity`,
`verified_wg_coordinator_target_rejects_origin_coordinator`, `root_agent_claim_rejects_spoofed_wg_coordinator_dir_name`,
`invalid identity must not grant coordinator/root authority`, `coordinator_route_rejects_symlink_*`
and `coordinator_route_rejects_reparse_*`) are renamed and must stay green with their assertions
untouched. If any of them needs an assertion edited, the run has changed behavior and must stop.

Threat model: routine refactor on a trusted developer host with a repository-pinned toolchain. No
enhanced provenance control applies (section 13.2).

---

## 9. Tests and objective acceptance criteria

### 9.1 New tests

Exactly one new test in the whole phase, the sidebar filter token test of section 5.8. This phase
adds no behavior, so it adds no other test; what it must prove is that nothing moved, and that is
proved by the negative controls in 9.2 and the two comparators in 9.3 and 9.4.

### 9.2 Negative controls: existing tests that must stay green with their assertions untouched

If any of these needs an assertion edited, the run has crossed a boundary and must stop.

| Test | File | What it pins |
| --- | --- | --- |
| `coordinator_clock_settings_default_when_keys_absent` | `config/settings.rs:7044` | removes the five `coordinator*` JSON keys and asserts the defaults come back. Fails immediately if Rule S is skipped on any of them. |
| `coordinator_auto_close_skip_telegram_assigned_round_trips` | `config/settings.rs:7068` | asserts `json.get("coordinatorAutoCloseSkipTelegramAssigned")` is present after serialising. |
| the four issue-#248 migration tests | `config/settings.rs:8255-8347` | assert `!out.contains("startOnlyCoordinators")` and `out.contains("\"restoreCoordinatorWakeState\":true")`. |
| `exact_coordinator_error_strings_are_stable` | `session/selection.rs` | pins the three `selectionCoordinator*` error strings. Must stay green **unrenamed in its string content**. |
| `source_ownership_sentinel_rejects_each_one_line_mutation` and the `ArbiterJob` sentinel | `session/selection.rs:3961-4053` | reads `selection.rs` back and pins the enum declaration. Green only if allowlist L7 is applied completely. |
| `session_rs_threads_production_tokens_for_config_seed_and_context` | `tests/cli_workgroup_team.rs:1810` | pins the `is_coordinator` argument in the scraped call site. Green only if allowlist L8 is applied. |
| `coordinator_pre_token_minimization_snapshot_is_byte_exact`, `coordinator_pre_cross_workgroup_snapshot_is_byte_exact`, `coordinator_pre_orchestrator_rename_snapshot_is_byte_exact`, `old_coordinator_raise_hand_snapshot_is_byte_exact` | `config/seeded_context_templates.rs` | hash the frozen constants' bytes. Renaming the constants must not move a hash. |
| `coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name` | `config/instance_artifacts.rs` | pins `"coordinator_clocks.json"` and its temp glob. |
| `cli_workgroup_team.rs` `team_config["coordinator"]` assertions at :525 and :1342 | `tests/cli_workgroup_team.rs` | pin the team-config JSON key. |
| `cli_loop.rs` `list["loops"][0]["busyCoordinator"]` at :234, `busyCoordinator = "waitUntilIdle"` at :181, `"forceInject"` at :208 | `tests/cli_loop.rs` | pin the loop config key and its values. |
| `cli_project_registration.rs` `scope = "context:coordinator"` at :316, :489, :547 | `tests/cli_project_registration.rs` | pins the seed-manifest scope string. |
| `terminal-snapshot-portable` (both commands, 4 OSes) | `crates/` | pure negative control: `crates/` has zero identifiers in this phase, so these must be green and unchanged. |
| the 62 frontend test files' `isCoordinator` / `coordinator` / settings-key fixtures | `src/**/*.test.ts(x)` | pin every wire key on the TypeScript side. |

### 9.3 Acceptance criterion 6: the exact literal-set extraction and comparison command

The issue requires the plan to specify this command rather than leave it to the reviewer. Here it is,
in full. Write the tool to a scratch path outside both repos; it is not committed.

```javascript
#!/usr/bin/env node
// litset.mjs -- quoted-literal set extractor. Issue #1572, acceptance criterion 6.
// Usage:  node litset.mjs <root> [<root> ...]
// stdout: one line per DISTINCT quoted string literal under the roots, sorted ascending by the
// whole line, LF only, one trailing LF. A newline inside a literal is rendered as the two
// characters \n so every record stays one line. A literal longer than 400 characters is
// truncated and suffixed with <TRUNC>. stderr: "files=<n> distinct=<n>".
// File domain: *.rs, *.ts, *.tsx. Skipped dirs: target, node_modules, .git, dist.
// The scanner is one left-to-right alternation, so a construct that only LOOKS like a string
// start inside another construct cannot desynchronise it:
//   Rust: raw string | string | char literal | line comment | block comment
//   TS  : template | double-quoted | single-quoted | line comment | block comment
// Only the string/template alternatives are recorded. Consuming Rust char literals is load
// bearing: '"' would otherwise open a phantom string and pair with the next unrelated quote.
// The Rust raw-string hash run is captured and back-referenced so r#"a "b" c"# terminates at
// its own "# and not at the first interior quote. Comments are discarded: a doc comment is
// prose, not a literal, and criterion 6 is about literals.
// This is a COMPARATOR, not an inventory: it is run over two trees and the outputs are diffed,
// so any residual imprecision is identical on both sides and cancels.
import fs from 'node:fs';
import path from 'node:path';

const roots = process.argv.slice(2);
if (roots.length === 0) { console.error('usage: node litset.mjs <root> [<root> ...]'); process.exit(2); }

const files = [];
function walk(d) {
  for (const e of fs.readdirSync(d, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const p = path.join(d, e.name);
    if (e.isDirectory()) { if (['target', 'node_modules', '.git', 'dist'].includes(e.name)) continue; walk(p); }
    else if (/\.(rs|ts|tsx)$/.test(e.name)) files.push(p);
  }
}
for (const r of roots) { if (fs.existsSync(r)) walk(r); }

const RS = /(r(#*)"[\s\S]*?"\2)|("(?:[^"\\]|\\[\s\S])*")|'(?:[^'\\]|\\[\s\S])'|\/\/[^\n]*|\/\*[\s\S]*?\*\//g;
const TS = /(`(?:[^`\\]|\\[\s\S])*`)|()("(?:[^"\\\n]|\\.)*")|('(?:[^'\\\n]|\\.)*')|\/\/[^\n]*|\/\*[\s\S]*?\*\//g;

const set = new Set();
for (const f of files) {
  const s = fs.readFileSync(f, 'utf8');
  const re = new RegExp((f.endsWith('.rs') ? RS : TS).source, 'g');
  let m;
  while ((m = re.exec(s))) {
    const lit = m[1] ?? m[3] ?? m[4];
    if (lit === undefined) continue;
    const rendered = (lit.length > 400 ? lit.slice(0, 400) + '<TRUNC>' : lit).replace(/\r?\n/g, '\\n');
    set.add(rendered);
  }
}
const out = [...set].sort();
process.stdout.write(out.join('\n') + (out.length ? '\n' : ''));
process.stderr.write(`files=${files.length} distinct=${out.length}\n`);
```

Run it over a materialised base tree and the branch tip:

```powershell
$S = "$env:TEMP\1572-litset"                      # scratch, outside both repos
New-Item -ItemType Directory -Force $S | Out-Null
# ... write litset.mjs into $S ...

cd D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-AgentsCommander
# BASE tree, materialised so the shared workgroup checkout is never moved off the branch tip
New-Item -ItemType Directory -Force "$S\base" | Out-Null
git archive 147ad4efa537f3ae5386c6949fa039dfa7e6735a | tar -x -C "$S\base"
node "$S\litset.mjs" "$S\base\src-tauri\src" "$S\base\src-tauri\tests" "$S\base\crates" "$S\base\src" > "$S\before.txt"
# HEAD tree, in place
node "$S\litset.mjs" src-tauri\src src-tauri\tests crates src > "$S\after.txt"

# The two binding comparisons
Compare-Object (Get-Content "$S\before.txt") (Get-Content "$S\after.txt") |
  Where-Object { $_.InputObject -match 'coordinat' } |
  Format-Table SideIndicator, InputObject -AutoSize -Wrap
```

The base run must report `files=550 distinct=28345`, of which 379 lines match `coordinat`.
`*.ts` and `*.tsx` are not pinned in `.gitattributes`, so the archived base can be LF while the
worktree is CRLF under `core.autocrlf=true`. The comparator renders every newline inside a literal
as the two characters `\n`, so the two sides are directly comparable and no line-ending
normalisation step is needed.

**Green iff both hold:**

- **6a.** Every `<=` row (present in `before`, absent in `after`) is one of the nine allowlist entries
  of section 5.4: the six module specifiers L1 to L6, the seven `CoordinatorJob` literals of L7, the
  one scraped call-site literal of L8, and the four `Debug`/assertion labels of L9. **No other
  `coordinat` literal may disappear.**
- **6b.** There is **no** `=>` row matching `coordinat`. No new literal containing `coordinator` may
  appear anywhere. This is the half that proves nothing was accidentally rewritten into a new
  serialised spelling.

Rows not matching `coordinat` are the renamed counterparts (`"./orchestrator-badge"`,
`"enum ArbiterJob"`, `"is_orchestrator"`, ...) plus the literals introduced by the single new test of
section 5.8. Inspect them, but they are not part of the gate: a serialised value containing the word
`coordinator` cannot change without appearing in 6a or 6b.

Website repo, same idea, one command, no tool needed because the surface is 8 lines:

```powershell
cd D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-agentscommander_webpage
git diff 5ec1ad27e1fed2970a83c191a5d4e33993a5436f..HEAD -- src README.md |
  Select-String -Pattern '^[+-].*coordinat' -CaseSensitive:$false
```

Green iff every `-` line is one of the 8 `composer.coordinator` sites, the two import specifiers, the
`CoordinationDemo` identifier sites and `README.md:37`, and every `+` line is its renamed counterpart.

### 9.4 Acceptance criterion 4: the levelization gate

Run from the repo root on a clean tree, with
`VAULT = D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust`:

```powershell
cd D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-AgentsCommander
node "$VAULT\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph graph.json --quiet
npm run record:arcs -- --graph graph.json
Remove-Item graph.json                      # gitignored; it carries an absolute path, never commit it
(Get-FileHash -Algorithm SHA256 src-tauri\module-arcs.txt).Hash
```

The detector **exits 1 when gating cycles exist and still writes the graph**; that is the normal
outcome on this repository. Only exit 3 means no graph was written. Never conflate them.

**Green iff all five hold:**

1. The regenerated `src-tauri/module-arcs.txt` hashes to
   `2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6`, is 1037 lines and 82163 bytes.
2. **The bijection check.** Applying the single substitution
   `agentscommander_lib::config::coordinator_clocks` to `agentscommander_lib::config::orchestrator_clocks`
   over the base file and re-sorting reproduces the regenerated file byte for byte:
   Run it in Node, not in PowerShell: `Sort-Object` is culture-aware even with `-CaseSensitive`,
   while `scripts/02-module-arc-record.mjs` sorts with JavaScript's `Array.prototype.sort`, which is
   UTF-16 code-unit ordinal. A culture-aware sort produces a different file and a false failure.
   ```powershell
   git show 147ad4efa537f3ae5386c6949fa039dfa7e6735a:src-tauri/module-arcs.txt > "$S\base-arcs.txt"
   node "$S\bijection.mjs" "$S\base-arcs.txt" src-tauri\module-arcs.txt
   ```
   with `bijection.mjs`:
   ```javascript
   import fs from 'node:fs'; import crypto from 'node:crypto';
   const [basePath, actualPath] = process.argv.slice(2);
   const base = fs.readFileSync(basePath, 'utf8').replace(/\r\n/g, '\n');
   const arcs = base.split('\n').filter(Boolean);
   const predicted = arcs
     .map(l => l.split('agentscommander_lib::config::coordinator_clocks')
               .join('agentscommander_lib::config::orchestrator_clocks'))
     .sort()                       // same ordinal sort the record script uses
     .join('\n') + '\n';
   const pb = Buffer.from(predicted, 'utf8');
   const actual = fs.readFileSync(actualPath);
   console.log('lines', arcs.length, 'predicted', pb.length, 'actual', actual.length,
               'equal', pb.equals(actual));
   console.log('sha256', crypto.createHash('sha256').update(pb).digest('hex').toUpperCase());
   ```
   Green iff it prints `lines 1037 predicted 82163 actual 82163 equal true` and
   `sha256 2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6`.
   This is the adapted form of the skill's byte-identity criterion. A literal byte-identity against
   the base is impossible here (the module id itself is renamed), and identity **under the renaming
   bijection** is the exactly equivalent statement that the arc SET did not change.
3. A second regeneration after committing leaves `git status --porcelain src-tauri/module-arcs.txt`
   empty. That is the skill's byte-identity criterion in its literal form, applied to the final tree.
4. `coverage.graphShape.cyclicSccs` is unchanged pre versus post, and every cyclic SCC member set is
   identical set to set after applying the same renaming bijection to the member names.
5. The structural layering guards stay green: `src-tauri/tests/instance_gitignore_layering.rs`,
   `src-tauri/tests/claude_watcher_layering.rs`, and every other `*_layering.rs` in
   `src-tauri/tests/`, with no assertion edited.

### 9.5 Objective acceptance criteria, mapped to the issue's seven

| # | Issue criterion | Command | Green means | Owner and time |
| --- | --- | --- | --- | --- |
| 1 | `cargo build` and `cargo test` pass with no new warning | `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests`, cwd `src-tauri`, from PowerShell, stdout redirected to a file | exit 0 on all three; zero new warnings under `-D warnings` | implementer locally per content commit; CI job `rust-regression` on the exact PR head |
| 2 | `vitest` passes across the 62 affected frontend test files | `npm run typecheck` then `npm test` | exit 0; the #480 known-debt set is unchanged | implementer locally; CI job `frontend-regression` on the exact PR head |
| 3 | `npm run check:frontend-dependencies` reports no cycle regression | `npm run check:frontend-dependencies` | same module count and 0 errors as on the base (baseline recorded in the PR body before the first content commit) | **implementer locally. No CI job runs this** (section 3.8). Run once on the base and once on the tip; both outputs in the PR body |
| 4 | Levelization gate passes with a regenerated `module-arcs.txt` | section 9.4 | all five conditions of 9.4 | **implementer locally. No CI job runs this** (section 3.8). Digest pasted in the PR body |
| 5 | `git log --follow` works on all 7 renamed files | `git log --follow --oneline -- <new path>` for each of the 7 | each listing reaches commits authored before C1, i.e. the file's pre-rename history | implementer at the branch tip, before opening the PR |
| 6 | No diff touches a serialised literal | section 9.3 | 6a and 6b both hold | implementer at the branch tip; reviewer re-runs the same command |
| 7 | The two concepts no longer share a word | `Select-String -Pattern 'Coordinator' src-tauri\src\session\selection.rs src-tauri\src\commands\resource_monitor.rs src-tauri\src\resource_monitor\watchdog.rs src-tauri\src\commands\window.rs` | the only remaining matches are the three phase-3 error-code strings `selectionCoordinatorUnavailable`, `selectionCoordinatorBusy`, `selectionCoordinatorRecursiveSubmission` and the test that pins them | implementer at the branch tip. See residual 10.2.1 |

Additional gate, not in the issue but required by section 5.3:

| # | Criterion | Command | Green means |
| --- | --- | --- | --- |
| 8 | Every serialised key is byte-identical | `cargo test --lib settings::` plus the round-trip and migration tests of section 9.2, with **no assertion edited** | Rule S was applied to all 28 members. A single missed pin reddens `coordinator_clock_settings_default_when_keys_absent` or one of the four #248 migration tests |

---

## 10. Explicit decisions and accepted residuals

### 10.1 Decisions

1. **Rename the serialised members and pin the key, rather than defer them to phase 3.** The issue
   names the settings `coordinator_*` fields and `is_coordinator` explicitly, and the out-of-scope
   list forbids changing anything serialised. Both are satisfiable only by renaming the identifier
   and pinning the key. The alternative, deferring 28 members to phase 3, contradicts the issue's own
   enumeration and would leave phase 2 unable to reach its stated scope. Precedent exists in the same
   file (`settings.rs:305-310`).
2. **`SelectionCoordinatorError` and `QuarantineRetryPath::Coordinator` join the Concept B table.**
   Section 3.2 gives the evidence. Without them, criterion 7 cannot pass.
3. **`CoordinationDemo.css` joins the website rename set.** Section 3.7 gives the reason.
4. **The frozen-snapshot constants take the new identifier, their bytes stay frozen.** Section 5.1
   gives the reason and distinguishes this from epic decision 2.
5. **No string literal changes except the nine-entry allowlist.** This is what makes criterion 6 a
   mechanical gate instead of a judgment call, and it is why test titles, `expect()` prose and log
   format strings are explicitly routed to phase 4.
6. **`CoordinatorChangedPayload` renames but its `is_coordinator` field is pinned.** A struct type
   name is never serialised by serde; a field name always is.
7. **One non-building commit per repo, containing only renames.** Section 7 item 6.
8. **Two pull requests, one plan.** The repos have independent CI and independent merge policy. The
   app PR is the one that carries acceptance criteria 1 to 8; the website PR carries `npm run check`
   and `npx playwright test`.
9. **The new test lives in `ProjectPanel.regex-filter.test.tsx`, not in a new file.** The behavior it
   pins is the regex filter's, its fixture already exists there, and a new file would need a new
   fixture for one assertion.

### 10.2 Accepted residuals, each owned by a later phase

1. **The three `selectionCoordinator*` error strings survive this phase.** Issue #1572 routes them to
   phase 3 in as many words, because they cross into the frontend. The consequence is that criterion
   7, read literally as "no shared word anywhere", cannot be fully true at the end of phase 2: the
   two concepts still share the word inside those three string constants and inside
   `exact_coordinator_error_strings_are_stable`. Criterion 7 is therefore certified in the form
   stated in the 9.5 table: **no shared identifier**, with the three strings as the issue's own
   declared exception. Owner: #1573.
2. **A function named `isSelectionArbiterBusyError` compares against `"selectionCoordinatorBusy"`.**
   Direct consequence of residual 1. Owner: #1573.
3. **Rust says `session.is_orchestrator` while TypeScript says `session.isCoordinator`.** Direct
   consequence of Rules S and K: the Rust identifier is in scope, the wire key it produces is not, and
   the TypeScript member is that key. This asymmetry exists for exactly one phase. Owner: #1573.
4. **~340 prose literals keep the word**: test titles, `expect()`/`panic!` messages, log format
   strings, `[coordinator-clocks]` log prefixes, agent-facing template bodies, and comment prose that
   names no symbol. Owner: #1574, the residual-elimination phase.
5. **10 `data-ac-testid` values, 4 event names, the `close_coordinator` command, the `--coordinator`
   and `--busy-coordinator` flags, `Context.coordinator.md`, `coordinator_clocks.json` and the
   `context:coordinator` manifest scope** all keep the word. Owner: #1573.
6. **CSS selectors `.coordination-proof` and `#coordination-proof-title`** keep the word
   "coordination", which the epic's grep does not match. No owner needed.
7. **Acceptance criteria 3 and 4 have no CI job.** Section 3.8. They are local-only with a named owner
   and their outputs go in the PR body. Wiring them into CI is out of scope here; if the team wants
   it, it is a separate issue.

---

## 11. Dependency-cycle and layering statement

Applying `verify-no-dependency-cycles` to this plan:

1. **Enumerated new arcs: zero. Enumerated removed arcs: zero.** Every edit renames an existing
   symbol at an existing site. The plan adds no `use`, no `mod`, no `impl`, no call, and retargets
   none. The one module identifier that changes,
   `agentscommander_lib::config::coordinator_clocks` to `agentscommander_lib::config::orchestrator_clocks`,
   carries its 14 arcs unchanged: 6 incoming (from `agentscommander_lib`, `cli::workgroup`,
   `commands::ac_discovery`, `commands::entity_creation`, `commands::pty`, `commands::session`),
   2 more incoming (`session::auto_close`, `web::commands`), and 6 outgoing (to `config`,
   `config::ac_root`, `config::instance_artifacts`, `config::projects`,
   `config::sessions_persistence`, `config::teams`).
2. **Per-arc verdict: not applicable, because there is no new arc.** For completeness, every one of
   the 14 preserved arcs has an identical source and target module after the rename, so none can
   cross an SCC boundary it did not already cross, and none can join two SCCs that were separate.
3. **Measurement.** The instrument is present and runnable from this workgroup
   (`repo-personal/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs`,
   172938 bytes). The pre-state is measured: 1037 arcs, SHA-256
   `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`. The post-state is **predicted
   exactly**: 1037 arcs, SHA-256 `2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6`.
   The full pre/post run, including `cyclicSccs` and SCC member sets, is the implementer's step and is
   written into the plan as acceptance criterion 4 (section 9.4), with the bijection form of the
   byte-identity criterion. This plan does not claim the run happened; it claims the arc set is
   provably invariant and gives the executable proof.
4. **Role and layering hygiene: unchanged.** No module gains an `AppHandle`, a `tauri` dependency, or
   any transport it did not already have. `config::orchestrator_clocks` keeps exactly the six outgoing
   edges `config::coordinator_clocks` had, all of them into `config::*`, so the persistence layer
   gains nothing upward. No function moves between modules.
5. **Gate: passes.** The plan adds no cycle, no SCC and no cross-boundary arc.

Frontend: `npm run check:frontend-dependencies` is a cycle gate over the TypeScript module graph. The
7 file renames move 6 nodes and rewrite 10 edge labels; no edge is added or removed, so the graph is
isomorphic and the cycle count cannot change. Criterion 3 measures it anyway, base and tip.

---

## 12. Implementation order and ownership

Route: Full. The implementer owns every commit; the reviewer owns section 9's gates; the tech lead
owns the merge.

### 12.1 `repo-AgentsCommander`, 12 commits

| # | Commit | Content | Gate before moving on |
| --- | --- | --- | --- |
| C0 | `docs(1572): plan for orchestrator internal identifiers` | this file only | none |
| C1 | `refactor(1572): rename 7 coordinator files (pure git mv)` | the 7 `git mv` of section 5.6 and nothing else. **Does not build. This is the mandated split.** | `git show --stat C1` shows 7 renames, 0 insertions, 0 deletions |
| C2 | `refactor(1572): orchestrator identifiers in config/` | `config/orchestrator_clocks.rs`, `config/mod.rs`, `teams.rs`, `settings.rs` (with Rule S), `session_context.rs`, `seeded_context_templates.rs`, `instance_artifacts.rs`, `projects.rs`, `seed_manifest.rs`, `root_agent.rs`, `loops.rs`, `agent_config.rs`, `sessions_persistence.rs`, `activity_log.rs` | `cargo check --all-targets` |
| C3 | `refactor(1572): orchestrator identifiers in commands/, lib.rs, web/` | `commands/*.rs` (Rule S on `ac_discovery`, `entity_creation`, `loops`), `lib.rs`, `web/commands.rs` | `cargo check --all-targets` |
| C4 | `refactor(1572): orchestrator identifiers in cli/, api/, phone/, loops/, pty/, session/` | the remaining Concept A files of section 6.3, with Rule S on `cli/team.rs`, `phone/types.rs`, `pty/git_watcher.rs`, `session/session.rs` | `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings` |
| C5 | `refactor(1572): SelectionCoordinator becomes SelectionArbiter` | Rule B across the 12 production files plus allowlist L7, plus Rule S on `QuarantineRetryPath::Coordinator` | `cargo check --all-targets` |
| C6 | `refactor(1572): orchestrator identifiers in src-tauri/tests` | the 6 files of section 6.4, including allowlist L8 | `cargo test --lib --bins --tests` (redirect stdout to a file), all green |
| C7 | `refactor(1572): rewrite the 7 renamed modules` | contents of the 7 ADDED files plus the 10 module specifiers L1 to L6 in their importers | `npm run typecheck` |
| C8 | `refactor(1572): orchestrator identifiers in the sidebar and shared frontend` | the remaining 21 files of section 6.5 | `npm run typecheck`, `npm test` |
| C9 | `test(1572): pin the sidebar orchestrator filter token (R11)` | the one new `it(...)` of section 5.8 | `npm test -- ProjectPanel.regex-filter` green, and green only with the production token present |
| C10 | `chore(1572): regenerate module-arcs.txt` | `src-tauri/module-arcs.txt` only | section 9.4, all five conditions |
| C11 | `chore(1572): final gate evidence` | nothing, or the PR body only | sections 9.3, 9.5 criteria 1 to 8, in order |

C9 must be proved to be a real test: run it once with the production token at `ProjectPanel.tsx:975`
temporarily reverted to `"coordinator"` and confirm it goes red, then restore. Materialise-and-revert
is the accepted technique; the revert must be verified by `git status --porcelain` before C10.

### 12.2 `repo-agentscommander_webpage`, 4 commits

| # | Commit | Content | Gate |
| --- | --- | --- | --- |
| W1 | `refactor(1572): rename Coordination* components (pure git mv)` | the 3 `git mv` of section 5.7 and nothing else. Does not build. | `git show --stat W1` shows 3 renames, 0 insertions, 0 deletions |
| W2 | `refactor(1572): OrchestrationDemo identifiers and import sites` | `OrchestrationDemo.tsx`, `OrchestrationProof.astro`, `README.md` | `npm run check` |
| W3 | `refactor(1572): rename the composer.coordinator i18n key` | `src/i18n/landing.ts` (6 lines), `src/components/alternatives/TeamComposer.astro` (2 lines) | `npm run check` (the key is typed `LandingMessageKey`, so a missed site is a type error), `npx playwright test` |
| W4 | `chore(1572): gate evidence` | PR body only | the diff command of section 9.3 |

### 12.3 Delivery

Two pull requests against `main`, each linked to #1572, neither pushed directly to `main`. The app PR
lands first; the website PR has no dependency on it but is merged in the same session so the epic's
phase-2 checkbox flips once.

---

## 13. Delivery nonfunctional invariants

### 13.1 Accepted task class and threat model

**Routine refactor**, no behavior change, no persisted-format change, no packaging or release step,
on a trusted developer host with a repository-defined toolchain. No enhanced provenance control is
applicable: no release, no signing, no packaging, no untrusted build host, no security-boundary
change (section 8), no destructive or irreversible migration, no demonstrated concurrent mutation, no
custom execution infrastructure. Independently anchored executable hashes, DLL closure inventories,
poisoned-`PATH` tests and SDK manifests are therefore **not applicable** and any finding in that class
is advisory, not a blocker.

One enhanced control **is** applicable and is declared: the diff must not move a serialised byte
(section 3.3). Its hazard is concrete (a silent JSON key change loses user settings on upgrade), the
requirement comes from the issue's own out-of-scope list, and the baseline control is insufficient
because `rename_all` derives the key from the identifier, so the compiler cannot see the break. Its
trust anchor is the existing round-trip and migration tests plus the literal-set comparator; its
scope is the 28 members of section 3.3; its evidence is criteria 6 and 8; its recovery is to revert
the offending commit; its owner is the implementer at each content commit.

### 13.2 Baseline gate map

| Gate | Source of truth | Executable evidence | Expected result | Failure behavior | Owner and time |
| --- | --- | --- | --- | --- | --- |
| 1. CI-to-plan parity | `.github/workflows/*.yml` at the base | section 3.8 job table; the exact-head rule of 13.4 | 7 PR jobs plus `validate-branch-name` green; 3 path-filtered workflows do not trigger | any red job blocks the merge; an unexplained skip does not satisfy the gate | CI on the PR head; reviewer reads the checks |
| 2. Deterministic toolchain | `.github/workflows/pr-regression-gates.yml`, `package.json`, `.gitattributes` | Rust `stable` via `dtolnay/rust-toolchain@stable`, Node 22, npm pinned to 11.6.2 in CI; locally the resolved `cargo`/`node` with versions recorded in the PR body; `src-tauri/module-arcs.txt` pinned `text eol=lf` | reproducible arc-record digest on Windows with `core.autocrlf=true` | a digest mismatch is a real failure, never normalised away | implementer at C10; CI at the PR head |
| 3. Authorized, traceable Git | issue #1572 open; branch `refactor/1572-orchestrator-internal-identifiers`; base `147ad4ef` / `5ec1ad27` | section 1.2 entry ritual; `git status --porcelain` empty before every commit | clean base, issue-numbered branch, delivery by PR only | a dirty or unknown base, or a direct push to `main`, blocks | implementer before the first mutation |
| 4. Process state, configuration, cwd | `scripts/02-module-arc-record.mjs:27-35` | every `cargo` command with `working-directory: src-tauri`; `record:arcs` from the repo root; `graph.json` deleted after use (gitignored, carries an absolute path) | no task-created file outside the intended path set | a stray `graph.json` in `git status` blocks C10 | implementer |
| 5. Validation and scope before acceptance | this plan's section 6 tables | `git status --porcelain` and `git diff --stat` after each commit, compared against the table for that commit | only the listed paths changed | any path outside the tables is reported, not absorbed | implementer per commit; reviewer at the tip |
| 6. Mutation ownership and no-clobber recovery | section 7 item 7 | per-commit `git status`; recovery scoped to the exact paths the commit wrote | recovery restores only this run's output | a path whose bytes are not this run's output is left alone and the conflict reported | implementer |
| 7. Bounded execution and durable diagnostics | CI job timeouts; local redirection | `cargo test` stdout redirected to a file (it is swallowed otherwise, and panic detail with it); CI retains job logs | a failing test's output survives to the report | a timed-out or cancelled command is never reported as success | implementer locally; CI remotely |
| 8. Evidence discipline | this section | every number in sections 3, 6 and 9 is a measurement at `147ad4ef` / `5ec1ad27`; zero and absence are typed (`crates/` has **zero** identifiers, and that is asserted, not assumed) | no assumed evidence | a gate whose evidence could not be produced is named with its owner, not silently dropped | architect at authoring; implementer at execution |

### 13.3 Local versus CI evidence ownership

| Evidence | Local | CI | Note |
| --- | --- | --- | --- |
| `cargo check` / `clippy` / `cargo test` | yes, per content commit | `rust-regression` (Windows) is the only job that runs `cargo test`; the Linux and macOS jobs are check plus clippy only | a `#[cfg(unix)]` test would compile everywhere and run nowhere; this phase adds none |
| `npm run typecheck`, `npm test` | yes | `frontend-regression` | |
| `npm run check:frontend-dependencies` | **yes, only here** | **no job exists** | criterion 3; base and tip outputs in the PR body |
| levelization gate | **yes, only here** | **no job exists** | criterion 4; digest in the PR body |
| literal-set comparator | yes | no job exists | criterion 6; the reviewer re-runs the same command |
| `git log --follow` | yes | no | criterion 5 |
| `crates/` portability | yes if desired | `terminal-snapshot-portable` on 4 OSes | negative control |
| Windows release CLI smoke | no | `windows-release-cli-smoke` | host-dependent; CI owns it |

### 13.4 Exact-head acceptance rule

Delivery requires every triggered and configured-required check to be green for the **exact PR-head
SHA**. Evidence from another SHA, an unexplained skip, a waiver or a bypass does not satisfy the gate.
A rerun that erases a prior failure does not erase it: walk the per-attempt job payloads if a check
was rerun. If any workflow, required-check configuration, base or diff drifts, re-derive section 3.8
before merging.

### 13.5 Bounded target-branch drift

The base is pinned at `147ad4ef` / `5ec1ad27` for this round. Later movement of `origin/main` alone
does not invalidate this plan, produce `CHANGES_REQUIRED`, or require the target to stay motionless.
Before the first product mutation and again before opening each PR, fetch the live target and
classify the drift by changed paths:

- Drift touching `src-tauri/src/**`, `src-tauri/tests/**`, `src/**`, `src-tauri/module-arcs.txt`,
  `scripts/02-module-arc-record.mjs`, `.gitattributes` or `.github/workflows/**` requires refreshing
  only the affected evidence: re-measure the touched file's identifier set, re-derive section 3.8, and
  recompute the `module-arcs.txt` base and predicted digests of section 3.6.
- Any other drift is recorded and synchronised at the next bounded gate. It does not reopen the design.

Once a PR exists, exact PR-head checks and the repository merge policy are authoritative. Continuous
pre-PR attestation that the target never moved is forbidden.

---

## 14. What a reviewer should attack first

In this order, because this is where the phase can actually break:

1. **Rule S coverage.** Count the `#[serde(rename = "...")]` additions in the diff. It must be **27**
   new pins (28 members minus `legacy_start_only_coordinators`, already pinned). A missing pin is a
   silent data-loss bug on upgrade that no compiler catches. `coordinator_clock_settings_default_when_keys_absent`
   and the four #248 migration tests are the tripwires; verify they were not edited.
2. **Criterion 6, part 6b.** No new literal containing `coordinator` may appear. That is the half a
   careless run fails, by rewriting a wire key into a "consistent" new spelling.
3. **The three source-text-coupled literals** (section 3.5). Each one is a green test that a partial
   rename turns red in a confusing way, and each one is a literal the reviewer would otherwise flag as
   an out-of-scope change.
4. **Concept A versus Concept B in `commands/session.rs`, `session/auto_close.rs`, `phone/mailbox.rs`,
   `session/manager.rs`, `lib.rs` and `web/commands.rs`.** These are the six mixed files. A blind pass
   over them produces a compiling tree with the wrong names.
5. **`close_coordinator`.** Confirm the fn name at `commands/session.rs:3406`, its `generate_handler!`
   entry at `lib.rs:3370` and the invoke string at `src/shared/ipc.ts:214` are all still
   `close_coordinator`, while the return type became `OrchestratorCloseOutcome` on both sides.
6. **The `module-arcs.txt` digest.** It is predicted exactly. A different digest means the diff
   changed the arc set, which this plan asserts is impossible.
7. **C1 and W1.** `git show --stat` must show renames only, zero insertions, zero deletions. If either
   carries a content hunk, `git log --follow` is at risk and criterion 5 is the test.
