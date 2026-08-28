# Plan #1572: Coordinator to Orchestrator, phase 2 (internal identifiers, file names, SelectionCoordinator)

Status: READY_FOR_IMPLEMENTATION
Issue: #1572 (open, label `refactor`). Parent epic: #1570, phase 2 of 4. Phase 1 (#1571) closed
2026-08-27T16:26:04Z and landed.
Route: Full.
Author: ac-architect-v3. Consensus round 4. Supersedes the round-3 candidate
`08A46CDCAC4279F9BE08B8D98AE7A0FEBDC7325C7E36CB21F2243A2D31D2962A` (two `PLAN_APPROVED`, one
`CHANGES_REQUIRED`), before it the round-2 candidate
`A835750BC4EB5F364CB6BA7F0E6C0938A12536E2A0F99791B35206A14EBBFE9D` (two `PLAN_APPROVED`, one
`CHANGES_REQUIRED`), and before that the round-1 candidate
`09816BAF4995FCB4851F80265528FCEBB99B264C4E7871ACB2C805F303C6EAF7`, which three reviewers returned
`CHANGES_REQUIRED`. Section 15 maps every round-1, round-2 and round-3 finding to the section that
closes it.
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

**`147ad4ef` is the base against which every coordinate in this plan is measured.** Every file:line
citation, every count, every set and every digest below is read at `147ad4ef` (app) and `5ec1ad27`
(web); a line number here is meaningless against any other tree, and a reviewer who checks one
against a different tree is measuring something else. `origin/main` has since moved to `047248bc`
(the merge of #1601, `fix/1596`). That drift was classified and is inert for this plan: zero added
lines matching `coordinat` under `src-tauri/src` and `src`, zero changed paths under `src/`, and
`src-tauri/module-arcs.txt` byte-identical. It is recorded, not absorbed: section 13.5 stays binding
and the entry ritual of 1.2 re-runs the classification against the live target before the first
mutation.

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

### 3.1 Baseline, the unit of measure, and four corrections to the numbers in the issues

Every "contains the word coordinator" count in #1570 and #1572 counts prose, comments and string
literals along with identifiers. This phase renames identifiers, so the identifier count is the one
that predicts the diff. Both were measured; both are reported.

**The unit of measure is stated once, here, because round 1 produced three different file counts
from three correct measurements.** A file is counted in the **word** column if the case-insensitive
substring `coordinator` occurs anywhere in it, including prose and literals. A file is counted in
the **identifier** column if `coordinator` survives after every string literal, every character
literal and every line and block comment is blanked out, i.e. if the file carries a `coordinator`
*token in code*. A frozen wire-key member (`isCoordinator: true` in a fixture) is a token in code
and therefore counts in the identifier column even though Rule K leaves it alone; that is why the
identifier column is much larger than the set of files this plan edits. The blanking uses the same
left-to-right alternation as the criterion-6 comparator of section 9.3, so the two measurements
cannot disagree with each other.

| Measure | Word count (`coordinator`, ci) | Identifier count (token in code) | Files this plan edits |
| --- | --- | --- | --- |
| `src-tauri/src` | 66 | 55 | 55 (54 in 6.3 + the file R1 renames) |
| `src-tauri/tests` | 9 | 6 | 6 |
| `crates/` | 5 | **0** | **0** |
| `src/` (TS/TSX) | 89 | 73 | 34 (28 in 6.5 + the 6 files R2-R7 rename) |
| distinct Rust identifiers | n/a | 224 | |
| distinct TypeScript identifiers | n/a | 60 | |

Two derived facts, both of which are load bearing and both of which are asserted, not assumed:

- **The Rust file set of section 6.3 is provably complete.** The 55 files carrying a `coordinator`
  token in code under `src-tauri/src` are exactly the 54 rows of section 6.3 plus
  `config/coordinator_clocks.rs`, which appears in 6.2/6.1 instead because it is renamed. Set
  equality, not just cardinality.
- **The TypeScript file set of section 6.5 is provably complete.** Of the 73 TS/TSX files carrying a
  `coordinator` token in code, 34 are in the tables (the 28 rows of 6.5 plus the 6 files R2-R7
  rename). In the other 39 files the 130 surviving occurrences use **exactly 10 distinct
  identifiers, and all 10 are in Rule K's frozen enumeration**; none of the 130 stands in a
  declaration context. Section 5.5 gives the per-identifier counts and the sweep.
  Note the shape of that argument: what closes the 39 is the *identifier set*, not the occurrences'
  property shape. Property shape alone proves nothing, and this plan is its own counterexample: it
  renames `WorkgroupGroupRail.raise-hand.test.tsx:27` `coordinator?: boolean`, `sessions.ts:249`
  `let coordinator: Session | null`, and `sessionsStore.recordCoordinatorVisibleOrder`
  (`ProjectPanel.tsx:1911`, `sessions-helpers.test.ts:294`), all three of which are property-shaped.
  The inference "property-shaped, therefore a frozen wire key" is unsound and is not used here.

Corrections:

1. **`crates/` is untouched by this phase.** Every hit is a string literal, on **10 lines** (the
   round-1 candidate said "5 hits", which counted files, not lines): the two reason-detail
   strings at `crates/session-bridge/src/bin/agentscommander-api-helper.rs:681,684` and eight
   `"project:wg-1-team/coordinator"` test fixture FQNs in
   `crates/session-bridge/src/bin/agentscommander-api-helper.rs:2230`,
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
   This rule is also what reconciles the two file counts a reviewer will get: **69** files under
   `src-tauri/src` match `coordinat`, **66** match `coordinator`; the difference is the three files
   whose only hit is the geometry word. Likewise **91** TS/TSX files match `coordinat` and **89**
   match `coordinator`. Both numbers are right; only the substring differs.
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

Every serde-derived member whose name contains `coordinat`, with the wire key it produces today, the
owning type's serde derives, and **the existing test that reddens if that member's pin is omitted**.
The last column is the honest answer to "what regression does this leave untested"; it is why
section 9.1 adds a second test, and it is measured, not estimated.

| # | File:line | Type | Member | Wire key produced today | Derives | Existing tripwire for a missing pin |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `src-tauri/src/cli/team.rs:104` | `TeamListItem` | `coordinator` | `"coordinator"` | Ser | **none** |
| 2 | `src-tauri/src/cli/team.rs:115` | `TeamCreateResult` | `coordinator` | `"coordinator"` | Ser | **none** |
| 3 | `src-tauri/src/cli/team.rs:127` | `AddMemberResult` | `coordinator` | `"coordinator"` | Ser | **none** |
| 4 | `src-tauri/src/commands/ac_discovery.rs:84` | `AcTeam` | `coordinator` | `"coordinator"` | Ser | **none** |
| 5 | `src-tauri/src/commands/ac_discovery.rs:112` | `AcAgentReplica` | `is_coordinator` | `"isCoordinator"` | Ser | **none** |
| 6 | `src-tauri/src/commands/entity_creation.rs:59` | `TeamConfigResult` | `coordinator` | `"coordinator"` | Ser+De | `tests/cli_workgroup_team.rs:525`, `:1342` |
| 7 | `src-tauri/src/commands/loops.rs:30` | `LoopCreateRequest` | `busy_coordinator` | `"busyCoordinator"` | **De only** | **none** |
| 8 | `src-tauri/src/commands/loops.rs:45` | `LoopUpdateRequest` | `busy_coordinator` | `"busyCoordinator"` | **De only** | **none** |
| 9 | `src-tauri/src/config/agent_config.rs:95` | `AgentDarkFactory` | `is_coordinator_of` | `"isCoordinatorOf"` | Ser+De+Default, `skip_serializing_if` | **none**: the key occurs nowhere else in either repo |
| 10 | `src-tauri/src/config/loops.rs:25` | `LoopTargetKind` | `WorkgroupCoordinator` | `"workgroupCoordinator"` | Ser+De | **none** in Rust (the only occurrence is the TS type alias `types.ts:1320`) |
| 11 | `src-tauri/src/config/loops.rs:98` | `LoopPolicy` | `busy_coordinator` | `"busyCoordinator"` | Ser+De+Default | `config/loops.rs:753`, `tests/cli_loop.rs:181`, `:208` |
| 12 | `src-tauri/src/config/loops.rs:153` | `LoopAuditEntry` | `busy_coordinator_policy` | `"busyCoordinatorPolicy"` | Ser+De | **none**: the key occurs nowhere else in either repo |
| 13 | `src-tauri/src/config/loops.rs:169` | `AcLoopSummary` | `busy_coordinator` | `"busyCoordinator"` | Ser | `tests/cli_loop.rs:234` |
| 14 | `src-tauri/src/config/sessions_persistence.rs:377` | `PersistedSession` | `is_coordinator` | `"isCoordinator"` | Ser+De+Default | **none** (see the `#[serde(default)]` note below) |
| 15 | `src-tauri/src/config/settings.rs:294` | `AppSettings` | `restore_coordinator_wake_state` | `"restoreCoordinatorWakeState"` | Ser+De | `config/settings.rs:5374`, `:8282` |
| 16 | `src-tauri/src/config/settings.rs:310` | `AppSettings` | `legacy_start_only_coordinators` | `"startOnlyCoordinators"` (**already pinned**, no new pin) | Ser+De | `config/settings.rs:8281`, which **does not discriminate**: after `apply_issue_248_migration` the field is `None` and `skip_serializing_if = "Option::is_none"` elides it, so `!out.contains("startOnlyCoordinators")` passes with the pin present or absent. Inert here, because this row carries no new pin; criterion 8a covers the member regardless |
| 17 | `src-tauri/src/config/settings.rs:527` | `AppSettings` | `coordinator_idle_badge_yellow_minutes` | `"coordinatorIdleBadgeYellowMinutes"` | Ser+De | **none** (see below) |
| 18 | `src-tauri/src/config/settings.rs:529` | `AppSettings` | `coordinator_idle_badge_red_minutes` | `"coordinatorIdleBadgeRedMinutes"` | Ser+De | **none** (see below) |
| 19 | `src-tauri/src/config/settings.rs:532` | `AppSettings` | `coordinator_auto_close_enabled` | `"coordinatorAutoCloseEnabled"` | Ser+De | **none** (see below) |
| 20 | `src-tauri/src/config/settings.rs:534` | `AppSettings` | `coordinator_auto_close_minutes` | `"coordinatorAutoCloseMinutes"` | Ser+De | **none** (see below) |
| 21 | `src-tauri/src/config/settings.rs:538` | `AppSettings` | `coordinator_auto_close_skip_telegram_assigned` | `"coordinatorAutoCloseSkipTelegramAssigned"` | Ser+De | `config/settings.rs:7076` |
| 22 | `src-tauri/src/config/settings.rs:542` | `AppSettings` | `coordinator_cascade_close_enabled` | `"coordinatorCascadeCloseEnabled"` | Ser+De | **none**: the key occurs in no Rust file |
| 23 | `src-tauri/src/phone/types.rs:82` | `PtyInputReasonCode` | `SenderNotCoordinator` | `"sender_not_coordinator"` | Ser+De | **none**: the 4 occurrences are hand-written `match` arms that never reach serde |
| 24 | `src-tauri/src/phone/types.rs:85` | `PtyInputReasonCode` | `TargetIsCoordinator` | `"target_is_coordinator"` | Ser+De | **none**: same, hand-written arms only |
| 25 | `src-tauri/src/pty/git_watcher.rs:416` | `CoordinatorChangedPayload` | `is_coordinator` | `"isCoordinator"` | Ser, `pub(crate)` | **none** |
| 26 | `src-tauri/src/resource_monitor/watchdog.rs:35` | `QuarantineRetryPath` | `Coordinator` | `"coordinator"` | Ser, `pub(crate)` | **none** |
| 27 | `src-tauri/src/session/session.rs:122` | `Session` | `is_coordinator` | `"isCoordinator"` | Ser+De | **none** |
| 28 | `src-tauri/src/session/session.rs:298` | `SessionInfo` | `is_coordinator` | `"isCoordinator"` | Ser+De | **none** |

**6 of 28 members are covered; 22 are not.** Of the 27 members that need a new pin, 5 are covered
and 22 are not. Two facts make the gap worse than a bare count suggests:

1. **Every one of these members carries `#[serde(default)]` (or lives in a type whose fields all do).
   A missing pin is therefore silent in both directions**: serialisation writes the new key,
   deserialisation ignores the old one and takes the default. No panic, no compiler error, no
   deserialisation failure. The user simply loses the setting on upgrade.
2. **`coordinator_clock_settings_default_when_keys_absent` (`config/settings.rs:7044`) is not a
   tripwire, although the round-1 candidate named it as the primary one.** The test serialises
   `AppSettings::default()`, removes five `coordinator*` keys by literal, deserialises, and asserts
   the five fields hold their defaults. If a pin is omitted, the serialised object carries the new
   key, the `obj.remove(...)` calls silently match nothing, and deserialisation reads back **the
   default value that was serialised in the first place**. Every assertion still passes. The test
   cannot distinguish a present pin from an absent one for any of its five keys, whatever those
   defaults are, because the value it reads back is by construction the value it asserts. Members
   17 to 20 therefore have no tripwire at all, and member 21 is covered only by the separate
   round-trip test at `:7068`, which asserts key **presence** (`json.get(...)`) and does redden.

This is exactly the class of defect the compiler cannot see and criterion 6 cannot see (a missing
pin moves no literal). Section 9.1 closes it with a wire-key stability test that covers all 28
members, and section 9.5 criterion 8 replaces the round-1 claim with that test plus a diff-shape
gate. The `Derives` column above is what tells the implementer which direction to assert; `rename`
is bidirectional, so asserting one direction proves the pin is present and correct.

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

**This table freezes Rust identifiers, not TypeScript ones.** The `coordinator` payload *key* of
`create_team` and `update_team` is frozen on both sides of the boundary. The TypeScript
**parameters** that carry the value into that payload, `src/shared/ipc.ts:1114` and `:1134`, are a
different thing: they are arrow-function parameters, so they are neither a property name nor a
member nor a destructuring binding, Rule K does not reach them, and Rule A renames them. Section 5.5
decides both halves, with the shorthands at `:1122` and `:1142` expanding so the key does not move.

### 3.5 The literals the rename forces to change, and the sweeps that close the set

A string literal changes in this phase **only** when it embeds source text that a renamed identifier
owns. There are five such classes and no others. Round 1 enumerated three of them and missed two,
so each class below carries the sweep that closes it, run at `147ad4ef` over
`src-tauri/src`, `src-tauri/tests`, `crates` and `src`.

| Class | What forces the change | Sweep | Result |
| --- | --- | --- | --- |
| A. ES module specifiers | the file they name is renamed | the two badge/close roots over the whole tracked tree | **10 sites, 6 distinct literals** (allowlist L1-L6). Zero references outside `src/`: not in `test-debt.allowlist.json`, `vitest.config.ts`, `dependency-cruiser.config.mjs`, `tsconfig.json` or any workflow, and no `vi.mock` names them |
| B. Rust inline format captures | `format!("{ident}")` stops compiling when `ident` is renamed | `git grep -E '\{[A-Za-z0-9_]*[Cc]oordinat\|\{[A-Za-z0-9_]*COORDINAT'` | **exactly 2**: `config/instance_artifacts.rs:620`, `config/teams.rs:822` (allowlist L12, L14). All other hits are `use` import braces. `config/root_agent.rs:371` `{ROOT_COORDINATION_MESSAGING_PARAGRAPH}` is the near-miss: it is `COORDINATION`, excluded by correction 3, and **does not change** |
| C. TS template interpolations | `` `${ident}` `` stops typechecking when `ident` is renamed | `git grep -E '\$\{[^}]*[Cc]oordinat'` over `src` | **exactly 1**: `src/sidebar/components/loop-modal-helpers.ts:23` (allowlist L13) |
| D. Source-scanning tests | the test reads the tree back and pins the identifier as text | enumerate every cross-file `include_str!` / `normalized_production_source` | **exactly 2 coupled sites** (allowlist L7, L8), plus 5 verified-clean negative controls, listed below |
| E. `Debug` labels and the assertions that read them | the label names a renamed field, variant or struct | `git grep -E '(\.field\|\.debug_struct\|\.debug_tuple)\("[^"]*[Cc]oordinat'` and `git grep -E '\.(contains\|starts_with\|ends_with)\("[^"]*[Cc]oordinat'` | **exactly 5 labels** (`config/teams.rs:583,584,603`; `session/manager.rs:63`; `api/identity.rs:107`) and **exactly 2 assertions on `Debug` output** (`config/teams.rs:2328,2360`): allowlist L9, L10, L11. `stringify!` and `concat!` produce zero hits |

Classes B, C and E are what round 1 missed. B, C and the `"kind: Coordinator"` assertion in E are
**forced**: omitting them is a compile error or a red test, not a style choice. Everything else in
either repo is frozen by Rule P.

The class-D detail, unchanged from round 1 and reverified:

1. **`src-tauri/src/session/selection.rs`**, self-scanning sentinel over `include_str!("selection.rs")`
   at `:4034`. Coupled literals: `:3909` `"enum CoordinatorJob"`, `:3910` `"CoordinatorJob declaration"`,
   `:3912` `"CoordinatorJob opening brace"`, `:3926` `"CoordinatorJob closing brace"`,
   `:4049` `"enum CoordinatorJob {{ Rogue {{ value: {forbidden} }} }}"` (the mutation the sentinel must
   reject), `:4053` `"sentinel accepted forbidden CoordinatorJob field {forbidden}"`, and `:4037`
   `"CoordinatorJob contains a managed handle or arbitrary executable field: {:?}"`.
   Seven literals at seven line numbers (`:3909`, `:3910`, `:3912`, `:3926`, `:4037`, `:4049`, `:4053`).
   `:3909` and `:4049` are functionally load bearing; the other five name the renamed type.
2. **`src-tauri/tests/cli_workgroup_team.rs:1834-1843`**, which scrapes
   `src/commands/session.rs` through `normalized_production_source` and pins the call-site text
   `materialize_agent_context_file_with_filename_activated(&cwd,&target_filename,&managed_filenames,is_coordinator,auto_self_clear,container_repos.as_ref(),activation.as_ref()`.
   The argument name `is_coordinator` inside that literal is source text, not prose.
Source scans verified **clean** of any `coordinat` pin, so they are negative controls:
`src-tauri/src/agent_update.rs:2772`, `src-tauri/src/pty/watchers/mod.rs:2470`,
`src-tauri/tests/cli_project_registration.rs:563` and `:602`, plus the `entity_creation.rs` scrape at
`src-tauri/tests/cli_workgroup_team.rs:1854`. **There is no fourth source scan**; the enumeration
above is the complete cross-file set.

The class-E detail, which round 1 covered only in part:

- **The five manual `Debug` labels.** `config/teams.rs:583` `.field("sender_is_coordinator", ..)` and
  `:584` `.field("target_is_coordinator", ..)` in the `Debug` impl of `VerifiedTerminalSnapshotRoute`
  (`:578-587`); `:603` `.field("is_coordinator", ..)` in the `Debug` impl of
  `TerminalSnapshotTargetIdentity` (`:599-606`); `session/manager.rs:63` `.field("is_coordinator", ..)`;
  and `api/identity.rs:107` `.debug_struct("VerifiedBoundContainerCoordinator")`. Each names a field,
  struct or variant that Rule A renames, so each label follows it. Round 1 listed `:583` and omitted
  `:584`.
- **The two assertions that read `Debug` output.** `config/teams.rs:2360`
  `assert!(diagnostic.contains("is_coordinator: true"))` reads the `:603` label. `config/teams.rs:2328`
  `assert!(diagnostic.contains("kind: Coordinator"))` reads the derived `Debug` of
  `TerminalSnapshotAuthorityKind::Coordinator` (`teams.rs:565-569`), a variant Rule A renames to
  `Orchestrator`. That assertion is **forced**: leaving it turns the test red. Round 1 missed it
  entirely, and it is the only member of this class that no other rule would have caught.

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
| `coordinat` lines under `repo-agentscommander_webpage/src` | **26 case-sensitive, 32 case-insensitive** (round 1 said 31, which is neither) |
| of which carry the string `coordinator` | 8, all of them the i18n key `composer.coordinator` |
| `composer.coordinator` sites | `src/i18n/landing.ts:71,176,278,381,486,585` and `src/components/alternatives/TeamComposer.astro:24` (`data-i18n=`) and `:25` (`copy[...]`) |
| the six locales | `en` (block starts `:1`), `es` (`:106`), `pt` (`:208`), `fr` (`:310`), **`de`** (`:414`), `zh-CN` (`:519`), per `LandingLanguage` at `:104`. Line 486 falls in the **German** block, not an "en-alt" one; German's value is also `"ORCHESTRATOR"`, which is what made round 1 mislabel it. The six line numbers are right; only the label was wrong |
| `Coordination*` component files | `src/components/CoordinationDemo.tsx` (6796 B), `src/components/CoordinationDemo.css` (6452 B), `src/components/CoordinationProof.astro` (1434 B) |
| import / reference sites | `CoordinationDemo.tsx:3` (`import './CoordinationDemo.css'`), `:74`, `:196`; `CoordinationProof.astro:2`, `:23`; `README.md:37` |
| `CoordinationProof.astro` consumers | **none.** No page or layout imports it. |
| i18n mechanism | a generic `document.querySelectorAll('[data-i18n]')` switcher at `src/layouts/BaseLayout.astro:160-161` and `src/pages/alternatives/attention.astro:310-311`, typed `MessageKey` / `LandingMessageKey`. |
| type-safety of the key rename | **covers 7 of the 8 sites, not 8.** `LandingMessageKey = keyof typeof en` (`:103`) and the five non-`en` locales typed `Record<LandingMessageKey, string>` make a missed locale a type error, and `copy["composer.coordinator"]` at `TeamComposer.astro:25` is a typed indexed access. **`TeamComposer.astro:24` is not covered**: `data-i18n="composer.coordinator"` is a plain HTML attribute that `astro check` never reads, consumed at runtime by the generic switcher. A forgotten `:24` passes both W3 gates green and leaves that span untranslated. Section 12.2 adds a grep gate for exactly this. |
| CI | one workflow, `.github/workflows/deploy.yml`. Playwright specs live in `tests/`; only `tests/smoke.spec.ts:114` touches a `composer.*` key, and it is `composer.note`, not the renamed one. The repo's script is `npm run smoke` (`"smoke": "playwright test"`); `npm run check` is `astro check`. |

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
6. **Two new tests**: the sidebar filter token (issue item 3c, residual R11 of #1571) and the
   wire-key stability test that makes Rule S1 auditable (section 9.1).
7. **`src-tauri/module-arcs.txt` regenerated**, never hand-edited.
8. **Doc comments and code comments that name a renamed symbol**, updated with the symbol they name.
9. **The two `docs/` lines that name the file path R1 renames**: `docs/reference/architecture.md:777`
   and `docs/reference/directory-layout.md:76`, both `config/coordinator_clocks.rs`. These are not
   prose about the concept; they are a path that stops existing at C1. `docs/features/session-auto-close.md:151`
   is deliberately **not** included: it names `coordinator_clocks.json`, the on-disk file, which is
   frozen. This is the complete set: a sweep of the whole tracked tree for the seven renamed
   basenames returns only these three doc lines, the 14 `module-arcs.txt` lines, and `plans/` plus
   `CHANGELOG.md`, which epic decision 2 protects.

### Out of scope (binding)

Everything below stays byte-identical. Each has a named later owner.

| Preserved | Sites | Owner |
| --- | --- | --- |
| Every string literal not listed in section 5.4 | of the base tree's 28345 distinct literals, **379 match `coordinat` case-insensitively and 337 case-sensitively**; 23 of them change, the rest are frozen. The gate command uses PowerShell `-match`, which is case-insensitive, so 379 is the number a reviewer will see | phases 3 and 4 |
| Persisted and transported JSON keys | the 28 serde members of section 3.3, pinned by Rule S1 | phase 3 |
| The IPC command `close_coordinator` and the `coordinator` payload arg of `create_team` / `update_team` | section 3.4 | phase 3 |
| Event names | `"session_coordinator_changed"`, `"coordinator_clock_updated"`, `"coordinator_auto_close_changed"`, `"coordinator_manual_close_changed"` | phase 3 |
| `data-ac-testid` values | `ActionBar.tsx:301`; `ProjectPanel.tsx:4198,4213,4221`; `SettingsModal.tsx:1914,1932,1944,1963,1979,1998` (10 values) | phase 3 |
| Machine-readable error codes | `"selectionCoordinatorUnavailable"`, `"selectionCoordinatorBusy"`, `"selectionCoordinatorRecursiveSubmission"` | phase 3 (stated by #1572 itself) |
| On-disk file names | `"Context.coordinator.md"` (`session_context.rs:13`), `"coordinator_clocks.json"` and `"coordinator_clocks.json.*.tmp"` (`instance_artifacts.rs:129,134`), `"context:coordinator"` manifest scope | phase 3 |
| CLI flag names | `--coordinator`, `--busy-coordinator`. Two of the three `--coordinator` fields derive the flag from the identifier rather than from a literal, so Rule S2 pins them explicitly; see section 5.3 | phase 3 |
| Frozen historical template **content** | `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND`, `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION`, `_BEFORE_CROSS_WORKGROUP_RULE`, `_BEFORE_ORCHESTRATOR_RENAME` and every other frozen snapshot | never; frozen by design |
| Test-title strings, `expect()`/`panic!` prose, log format strings, comment prose that does not name a symbol | ~340 literals | phase 4 (residual elimination) |
| CSS class and id names (`.coordination-proof`, `#coordination-proof-title`, `.cdemo-*`, `COORD_IDLE_CLASS`) and every `coordinate` / `coordination` word | | not in the epic's grep domain |
| `CHANGELOG.md`, `plans/`, and all `docs/` **prose** | except the two `docs/` lines of in-scope item 9, which name a renamed file path rather than the concept | epic decision 2 and phase 4 |
| Any behavior change | | separate issue, per epic non-goals |

---

## 5. Decided solution

Five rules. Rule A is the default; Rules S, K and P beat it wherever they apply; Rule B replaces it
inside Concept B.

**On totality.** Round 1 asserted totality and five occurrences disproved it. Totality is not
something a plan can assert; it is something the closed enumerations have to earn. So each rule now
carries the sweep that closes it, and the five occurrences that were undecided are decided by name:

| Undecided in round 1 | Decided by |
| --- | --- |
| `coordinator_shutdown` (`commands/session.rs:1432`, used `:2573`), where Rule A would give it the wrong *concept* | the Concept B binding clause and the explicit row in 5.2 |
| the three clap fields whose `--coordinator` flag is derived, not literal (`cli/team.rs:53`, `:85`, `cli/workgroup.rs:49`) | **Rule S2**, section 5.3 |
| the object shorthand `src/shared/ipc.ts:1081`, where Rule K freezes a token Rule A deletes | the shorthand clause of Rule K, section 5.5 |
| `src/sidebar/components/ProjectPanel.raise-hand.test.tsx`, a whole file with in-scope locals and no table row | added to 6.5; the sweep in 5.5 proves it is the only one |
| `TeamConfigResult.coordinator` (`src/shared/types.ts:1505`), frozen by Rule K's text but named in neither operative list | named in the Rule K frozen enumeration and in the 6.5 row |

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
| `QuarantineRetryPath::Coordinator` | `QuarantineRetryPath::Arbiter` (+ Rule S1 pin) |
| local Rust bindings `coordinator`, `selection_coordinator`, `running_coordinator` in Concept B code | `arbiter`, `selection_arbiter`, `running_arbiter` |
| `coordinator_shutdown` (`commands/session.rs:1432`, consumed at `:2573`) | `arbiter_shutdown` |
| TypeScript `isSelectionCoordinatorBusyError` | `isSelectionArbiterBusyError` |

**Concept B binding clause, which generalises the row above.** Any local binding whose *value* comes
from a Concept B value (the `SelectionArbiter` handle, anything read off it, its error, its phase,
its job, its envelope) takes the `arbiter` name, keeping whatever suffix it carries. Rule A never
applies to such a binding, however it is spelled. `coordinator_shutdown` is the case that motivates
the clause: it is `coordinator.shutdown_token()` where `coordinator` is
`try_state::<SelectionCoordinator>()`, so a mechanical Rule A pass produces `orchestrator_shutdown`,
which names the wrong concept, compiles, and passes every gate in section 9. The surrounding code
confirms the concept: the `tokio::select!` arm it guards at `:2573-2575` returns the frozen error
string `"selectionCoordinatorUnavailable"`.

A sweep of every Rust `let` binding whose name contains `coordinator`
(`git grep -E 'let (mut )?[a-z_]*coordinator[a-z_]*[ :=]'`) confirms `coordinator_shutdown` is the
**only** compound Concept B binding in either repo: every other Concept B binding is a bare
`coordinator`, `selection_coordinator`, or a `.clone()` of one. Every compound binding that sweep
returns and that this clause does **not** claim (`coordinator_app` and `coordinator_session_mgr`
(`commands/ac_discovery.rs:2208-2209`), `coordinator_blockers` (`commands/entity_creation.rs:2200`),
`coordinator_matrix`, `coordinator_template`, `coordinator_bytes`, `coordinator_path`,
`coordinator_content`, `coordinator_ref`, `coordinator_name`, `coordinator_binding`,
`coordinator_id`, `coordinator_fqn`, `spawn_is_coordinator`, `host_coordinator`, `api_coordinator`)
is Concept A and takes Rule A unchanged.

The three error-code **strings** stay `selectionCoordinator*` (out of scope, phase 3). A function
named `isSelectionArbiterBusyError` that compares against the literal `"selectionCoordinatorBusy"`
is the intended, temporary state; section 10.2 records it as an accepted residual.

Concept B files, verified line by line: `session/selection.rs` (owner),
`commands/resource_monitor.rs`, `resource_monitor/watchdog.rs`, `commands/window.rs`,
`commands/session.rs`, `lib.rs`, `session/auto_close.rs`, `web/commands.rs`, `phone/mailbox.rs`,
`testability/ui_automation.rs`, `screenshot/windows.rs`, and the three integration tests
`tests/wake_consumption_measure.rs`, `tests/pty_powershell_managed_native.rs`,
`tests/pty_lifecycle_regression.rs`.

### 5.3 Rule S: pin every external name that a derive macro spells from the identifier

Rule S covers the whole hazard class, not just serde: **wherever a derive macro turns a Rust
identifier into an external name, renaming the identifier silently changes that external name, and
the compiler cannot see it.** There are exactly **three** such macros in this tree: `serde` (S1
below), clap's `#[arg(long)]` (S2 below), and `#[tauri::command]`, which spells both the IPC command
name and the payload keys from identifiers. The third needs no rule of its own because section 3.4
already enumerates its complete `coordinat` surface and **freezes** every site of it that reaches
the wire: the one command name (`close_coordinator`) and the two value args (`create_team`,
`update_team`) are not renamed at all, and the three remaining sites are `State<'_, T>` parameters,
which are injected from managed state and never appear in a payload. Rule S therefore has two parts,
one per macro this phase actually renames through. Round 1 had only S1, which is why the three clap
fields of S2 ended up undecided.

#### 5.3 S1: pin every serialised member

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
already had, and section 9 proves that by round-tripping the keys. `rename` applies to both
directions, so proving one direction per member proves the pin.

**27 new pins**, at exactly the 27 sites of section 3.3 rows 1-15 and 17-28.
`legacy_start_only_coordinators` (row 16) gains the identifier rename only; its existing
`rename = "startOnlyCoordinators"` at `settings.rs:308` is untouched.

#### 5.3 S2: pin every clap-derived flag name

`#[arg(long)]` with no explicit `long = "..."` makes clap derive the flag from the **field
identifier**. Section 4 freezes `--coordinator`, and three fields produce that flag by derivation,
not from any literal. Rename the field by Rule A **and** add an explicit `long = "coordinator"`
inside the existing `#[arg(..)]`, never as a second attribute:

| File:line | Struct | Attribute today | Attribute after |
| --- | --- | --- | --- |
| `src-tauri/src/cli/team.rs:49-53` | `TeamCreateArgs` | `#[arg(long, help = "Existing agent matrix name or _agent_<name> reference. Automatically included in the roster")]` | `#[arg(long = "coordinator", help = <unchanged>)]` |
| `src-tauri/src/cli/team.rs:84-85` | `TeamAddMemberArgs` | `#[arg(long)]` | `#[arg(long = "coordinator")]` |
| `src-tauri/src/cli/workgroup.rs:48-49` | `WorkgroupAddArgs` | `#[arg(long, hide = true)]` | `#[arg(long = "coordinator", hide = true)]` |

The precedent is in the same directory: `cli/loop_cmd.rs:82` and `:104` already write
`#[arg(long = "busy-coordinator", value_enum)]`, which is why renaming *their* `busy_coordinator`
field is safe with no S2 pin.

Why this matters concretely, and why no gate would have caught it: without the pin, `--coordinator`
becomes `--orchestrator`. For `team create` and `team add-member` that turns ~20 invocations in
`src-tauri/tests/cli_workgroup_team.rs` red, so the run at least stops. For `workgroup add` the flag
is `hide = true`, and its only test,
`workgroup_add_help_hides_team_definition_flags` (`cli_workgroup_team.rs:391`), asserts
`!help.contains("--coordinator")`, which passes whether the flag is `--coordinator` or
`--orchestrator`. That one would change a public CLI flag **in silence**.

A sweep of every `#[arg(..)]`, `#[value(..)]` and `#[command(..)]` attribute under `src-tauri/src/cli`
within two lines of a `coordinat` identifier returns exactly these three fields plus the two
already-explicit `busy-coordinator` fields. The S2 set is closed at three.

### 5.4 Rule P: literals are frozen, with an exact allowlist

**No string literal changes anywhere in either repo, except the 14 entries below, which are
23 distinct literals.** The allowlist is closed; anything else is a defect. Section 3.5 gives the
five sweeps that close it.

Count the unit carefully, because round 1 conflated three of them. An **entry** is a row here. A
**distinct literal** is a record in the comparator's set. A **site** is a file:line. The criterion-6
gate compares distinct-literal *sets*, so its unit is the middle one.

| # | Entry | Distinct literals | Literal today → after | Sites | Forced? |
| --- | --- | --- | --- | --- | --- |
| L1 | badge module specifier | 1 | `"./coordinator-badge"` → `"./orchestrator-badge"` | `src/shared/coordinator-badge.test.ts:2` | yes |
| L2 | badge module specifier | 1 | `"../../shared/coordinator-badge"` → `"../../shared/orchestrator-badge"` | `src/sidebar/components/coordinator-badge-class.ts:1`, `coordinator-badge-class.test.ts:3`, `ProjectPanel.tsx:52` | yes |
| L3 | badge-class specifier | 1 | `"./coordinator-badge-class"` → `"./orchestrator-badge-class"` | `src/sidebar/components/coordinator-badge-class.test.ts:4`, `ProjectPanel.tsx:53` | yes |
| L4 | close-store specifier | 1 | `"./coordinator-close"` → `"./orchestrator-close"` | `src/sidebar/stores/coordinator-close.test.ts:15` | yes |
| L5 | close-store specifier | 1 | `"../stores/coordinator-close"` → `"../stores/orchestrator-close"` | `src/sidebar/components/ProjectPanel.tsx:12`, `SessionItem.tsx:9` | yes |
| L6 | close-store specifier | 1 | `"../sidebar/stores/coordinator-close"` → `"../sidebar/stores/orchestrator-close"` | `src/shared/shortcuts.ts:3` | yes |
| L7 | the sentinel's `CoordinatorJob` text | **7** | `CoordinatorJob` → `ArbiterJob` in each | `src-tauri/src/session/selection.rs:3909, 3910, 3912, 3926, 4037, 4049, 4053` | yes |
| L8 | the scraped call-site text | 1 | `,is_coordinator,` → `,is_orchestrator,` inside it | `src-tauri/tests/cli_workgroup_team.rs:1836-1838` (one literal, written as a three-line continuation) | yes |
| L9 | `Debug` labels and the assertion that reads one | **4** | `"is_coordinator"` → `"is_orchestrator"`; `"sender_is_coordinator"` → `"sender_is_orchestrator"`; `"VerifiedBoundContainerCoordinator"` → `"VerifiedBoundContainerOrchestrator"`; `"is_coordinator: true"` → `"is_orchestrator: true"` | `config/teams.rs:603` and `session/manager.rs:63`; `config/teams.rs:583`; `api/identity.rs:107`; `config/teams.rs:2360` | consistency |
| **L10** | the fifth `Debug` label | 1 | `"target_is_coordinator"` → `"target_is_orchestrator"` | `src-tauri/src/config/teams.rs:584` | consistency |
| **L11** | the assertion on a derived `Debug` | 1 | `"kind: Coordinator"` → `"kind: Orchestrator"` | `src-tauri/src/config/teams.rs:2328` | **yes, red test otherwise** |
| **L12** | inline format capture | 1 | `"{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp"` → `"{ORCHESTRATOR_CLOCKS_FILE_NAME}.*.tmp"` | `src-tauri/src/config/instance_artifacts.rs:620` | **yes, compile error otherwise** |
| **L13** | template interpolation | 1 | `` `${wg.name} - ${coordinator.name}` `` → `` `${wg.name} - ${orchestrator.name}` `` | `src/sidebar/components/loop-modal-helpers.ts:23` | **yes, typecheck error otherwise** |
| **L14** | inline format capture | 1 | `"_agent_{coordinator}"` → `"_agent_{orchestrator}"` | `src-tauri/src/config/teams.rs:822` | **yes, compile error otherwise** |

L1 to L6 are ES module specifiers: they are file paths, not data, and they must move with the file.
L7 and L8 are source text that a test reads back out of the tree. L9 to L11 are `Debug` rendering and
the assertions that read it. L12 to L14 are the identifier itself, embedded in a format string;
their **rendered value does not change** (`_agent_<name>` renders identically before and after),
only the capture name does.

**L10 to L14 are new in round 2.** L11, L12, L13 and L14 were each a guaranteed red build or red
test on a correct implementation of round 1's plan.

#### 5.4a Which allowlist entries actually appear as a gate row

The comparator diffs **sets**, so a literal that this plan removes from one site but that survives at
another, frozen site produces no row at all. Exactly one entry is in that position:

- **L10 `"target_is_coordinator"` produces no `<=` row.** It occurs at six sites. Only
  `config/teams.rs:584` changes; the other five are frozen reason-code text
  (`api/handlers/pty_input.rs:292`, `cli/send.rs:755`, `config/teams.rs:1234`, `phone/types.rs:144`,
  and `crates/session-bridge/src/bin/agentscommander-api-helper.rs:684`), and Rule S1 re-pins the
  same spelling at `phone/types.rs:85`. The literal never leaves the set.

Every other entry's literals occur only at sites this plan changes, verified one by one:
`"is_coordinator"` occurs at exactly two sites (`config/teams.rs:603`, `session/manager.rs:63`) and
both change; `"sender_is_coordinator"`, `"VerifiedBoundContainerCoordinator"`,
`"is_coordinator: true"`, `"kind: Coordinator"`, the L12 and L14 format strings and the L13 template
occur once each; the six module specifiers occur only at the ten sites listed, and nothing outside
`src/` references those roots.

**Therefore the expected `<=` set is 23 minus 1 = 22 distinct literals.** Section 9.3 states the gate
in those terms.

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
`PersistedSession` and their fixtures; `coordinator` as a member of `AcTeam`, **as a member of
`TeamConfigResult` (`src/shared/types.ts:1505`)**, and of the `create_team` / `update_team` argument
objects; `busyCoordinator`; `restoreCoordinatorWakeState`;
`coordinatorIdleBadgeYellowMinutes`; `coordinatorIdleBadgeRedMinutes`; `coordinatorAutoCloseEnabled`;
`coordinatorAutoCloseMinutes`; `coordinatorAutoCloseSkipTelegramAssigned`;
`coordinatorCascadeCloseEnabled`; the `LoopTargetKind` value `"workgroupCoordinator"`.

`TeamConfigResult.coordinator` is called out explicitly because round 1 froze it by the rule's text
and named it in neither operative list, while renaming two sibling members 300 lines away in the
same file (`TeamSessionGroup.coordinator:1179`, `Team.coordinatorName:1193`). It is the TypeScript
mirror of the serde struct at `src-tauri/src/commands/entity_creation.rs:55-59` (section 3.3 row 6),
consumed by `normalizeTeamConfigResult` over `invoke("get_team_config")`. An implementer working
from the table rather than the rule text would rename it, `get_team_config` would keep returning
`coordinator`, the frontend would read `orchestrator`, and `EditTeamModal` would load with an empty
orchestrator selection, with no TypeScript error anywhere.

**Shorthand clause.** An object-literal shorthand `{ coordinator }` is two things at once: a property
name, which Rule K may freeze, and a reference to a binding, which Rule A may rename. Where the
property is frozen and the binding renames, **the shorthand is expanded** to
`<frozenKey>: <renamedBinding>`. Without this clause the stated precedence ("Rules S, K and P beat
Rule A") freezes a token that no longer exists and the tree does not typecheck.

**The shorthand census, closed line-independently.** A line-anchored regex will not close this set,
for two reasons: it misses a shorthand that shares its line with other properties, and on a Windows
worktree with `core.autocrlf=true` a `$` anchor matches nothing at all, because every line ends
`\r\n` (round 2 stated such a regex and it returns zero rows here). The sweep is therefore run over
the blanked tree, in the same pass section 9.3 defines: blank every string literal and comment, then
take every identifier containing `coordinator` that is preceded, skipping whitespace and newlines
alike, by `{` or `,` and followed by `,` or `}`. Over the whole of `src` that returns **37
shorthand-shaped occurrences**. 31 are import- or export-specifier list entries, which are not
object shorthands and rename with their declarations. The remaining **6 are real object-literal or
destructuring shorthands**, and each is decided here:

| Shorthand | Property half | Binding half | Verdict |
| --- | --- | --- | --- |
| `src/shared/ipc.ts:1081` | frozen wire key `coordinator` | the `const` at `:1037`, renamed | **conflict, expand** to `coordinator: orchestrator,` |
| `src/shared/ipc.ts:1122` | frozen `create_team` payload key `coordinator` | the parameter at `:1114`, renamed to `orchestrator` | **conflict, expand** to `coordinator: orchestrator,` |
| `src/shared/ipc.ts:1142` | frozen `update_team` payload key `coordinator` | the parameter at `:1134`, renamed to `orchestrator` | **conflict, expand** to `coordinator: orchestrator,` |
| `src/sidebar/components/ProjectPanel.raise-hand.test.tsx:39` | frozen `AcAgentReplica.isCoordinator` | the helper parameter at `:34`, renamed | **conflict, expand** to `isCoordinator: isOrchestrator,` |
| `src/sidebar/App.tsx:800` | frozen wire key `isCoordinator` | the destructuring binding *is* that frozen name | no conflict, **stays**: Rule K freezes both halves |
| `src/sidebar/stores/sessions.ts:279` `groups.push({ team, coordinator, members })` | `TeamSessionGroup.coordinator`, frontend-only, **renames** | the `let` at `:249`, **renames** | no conflict, **stays** a shorthand, as `{ team, orchestrator, members }` |

**Four conflicting shorthands, not two.** Round 2 said two because it treated the `ipc.ts:1114` and
`:1134` parameters as frozen. Rule K's "if and only if" does not reach a parameter; the register
rows below and decision 10.1.14 settle them as ordinary locals, and Rule K's own text is unchanged.

**Ambiguity register.** These are the complete set of sites where a frozen name also occurs as an
in-scope local. They are decided here so the implementer makes no judgment call.

| Site | What it is | Verdict |
| --- | --- | --- |
| `src/shared/ipc.ts:1037` | `const coordinator = value.coordinator;` | the `const` renames, `value.coordinator` does not |
| **`src/shared/ipc.ts:1081`** | the `return { agents, coordinator, repos, contextAlertPercentages }` of `normalizeTeamConfigResult`: a shorthand whose property is the frozen wire key and whose binding is the `const` renamed one row above | **expand**: the line becomes `coordinator: orchestrator,` |
| **`src/shared/ipc.ts:1114`, `:1134`** | the `coordinator: string` parameters of `EntityAPI.createTeam` and `EntityAPI.updateTeam` | arrow-function parameters: not a property name, not a member, not a destructuring binding, so Rule K does not reach them. Local bindings, **rename** to `orchestrator`. All six call sites pass positionally (`NewTeamModal.tsx:212`, `EditTeamModal.tsx:260`, `ipc.transport.test.ts:153`, `:161`, `:169`, `:177`), so no caller changes |
| **`src/shared/ipc.ts:1122`, `:1142`** | the shorthands `coordinator,` inside the `create_team` / `update_team` payload objects | property frozen, binding renamed: **expand**, each becomes `coordinator: orchestrator,`. The emitted payload key does not move, so `ipc.transport.test.ts:196`, `:207`, `:220` and `:231` stay green with their `coordinator:` assertions **unedited** |
| **`src/shared/types.ts:1505`** | `TeamConfigResult.coordinator: string` | wire key (mirrors `entity_creation.rs:59`), **frozen** |
| **`src/sidebar/components/ProjectPanel.raise-hand.test.tsx:34`, `:47`** | the parameter `isCoordinator = true` of the `replica` and `workgroup` test helpers | local bindings, **rename** to `isOrchestrator`, consistent with `AcDiscoveryPanel.tsx:43`, `EditTeamModal.tsx:362` and `NewTeamModal.tsx:309` |
| **`src/sidebar/components/ProjectPanel.raise-hand.test.tsx:39`** | shorthand `isCoordinator,` whose property is the frozen `AcAgentReplica.isCoordinator` | **expand**: `isCoordinator: isOrchestrator,` |
| **`src/sidebar/components/ProjectPanel.raise-hand.test.tsx:54`** | `replica(wgName, replicaName, isCoordinator)` | argument use of the renamed local, **renames**. `:67` and `:191` `isCoordinator: true/false` are object properties and stay frozen; `:106`, `:186`, `:202` are test titles and stay frozen (phase 4) |
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
| `src/sidebar/App.tsx:800` | `isCoordinator` bound by destructuring the `session_coordinator_changed` payload at `onSessionCoordinatorChanged(({ sessionId, isCoordinator }) => {` | frozen (a destructuring binding of a wire key); `setIsCoordinator` renames. The use is at `:801`; round 1 gave `:801` for the binding, one line off |
| `src/shared/ipc.ts:754`, `:756` | `{ sessionId: string; isCoordinator: boolean }` payload type | frozen |

`COORD_IDLE_CLASS` contains no `coordinator`.
`src/sidebar/styles/coord-quick-access-css.test.ts` **does** contain it, at `:4` (a comment) and
`:63` (a `describe` title); round 1 said it did not. Neither is an identifier, so the file is still
untouched, but by residual 10.2.4, not by the reason round 1 gave.

**Completeness of the TypeScript side.** Rule K's frozen set and Rule A's in-scope set together have
to cover every `coordinator` token in TypeScript code, and round 1's tables did not. The closing
measurement: blank every string literal and comment (the section 9.3 alternation), then look at what
survives. 73 TS/TSX files carry a `coordinator` token in code. 34 are in the tables of section 6
(the 28 rows of 6.5 plus the 6 files R2-R7 rename). In the remaining **39 files there are 130
occurrences, and they use exactly 10 distinct identifiers**:

| Identifier | Occurrences | Frozen by Rule K as |
| --- | --- | --- |
| `isCoordinator` | 50 | member of `Session`, `SessionInfo`, `AcAgentReplica`, `PersistedSession` and their fixtures |
| `coordinatorAutoCloseEnabled` | 15 | `AppSettings` key |
| `coordinator` | 12 | member of `AcTeam` / `TeamConfigResult` / the `create_team`-`update_team` argument objects |
| `busyCoordinator` | 9 | `LoopPolicy` and loop-request key |
| `coordinatorAutoCloseSkipTelegramAssigned` | 8 | `AppSettings` key |
| `coordinatorCascadeCloseEnabled` | 8 | `AppSettings` key |
| `coordinatorAutoCloseMinutes` | 7 | `AppSettings` key |
| `coordinatorIdleBadgeRedMinutes` | 7 | `AppSettings` key |
| `coordinatorIdleBadgeYellowMinutes` | 7 | `AppSettings` key |
| `restoreCoordinatorWakeState` | 7 | `AppSettings` key |

**All 10 are in the Rule K frozen enumeration above, and not one of the 130 occurrences stands in a
declaration context**: none is preceded by `const`, `let`, `var`, `function`, `class`, `interface`,
`type` or `enum`, and none is a parameter. That is the whole argument for the 39, and it is a
statement about a decidable identifier set rather than about occurrence shape. Round 2 argued this
from property shape instead, with a "four exceptions" clause that had already stopped being
arithmetically possible once `raise-hand.test.tsx` joined the tables: those four sites are among the
34, so they cannot also be among the 39. The measurement above replaces both. **No row of section
6.5 changes as a result**; the three reviewers' independent sweeps of these 39 files all returned
zero non-frozen occurrences, and so does this one.

**Exactly two of the 39 are non-test files**, and both were read individually:
`shared/testing/ui-harness.tsx:120,169-174,231` (seven frozen `AppSettings` keys plus a frozen
`isCoordinator` fixture member) and `sidebar/components/workgroup-session.ts:47` (the frozen
`replica.isCoordinator` read). The other 37 are `*.test.ts` / `*.test.tsx`.

Seven further non-test files carry a `coordinator` token, but **only inside a comment or a string
literal**, so the blanking pass erases every one of them and they are in neither the 73 nor the 39:
`main/listeners-home.ts:22`, `sidebar/components/RaiseHandIcon.tsx:5`,
`sidebar/components/RootAgentBanner.tsx:403`, `sidebar/stores/clock.ts:3` and
`sidebar/stores/sessions-helpers.ts:82` are comment prose;
`sidebar/components/ActionBar.tsx:301` is the frozen `data-ac-testid`
`"actionBar.sortCoordinators"`; `sidebar/stores/project-collapse.ts:8` is the frozen
`"coordinators"` collapse-key literal. They were read individually too. Round 2's prose called all
nine of these files members of the 39, which the census that defines the 39 cannot support; the
substance, that not one of them holds a non-frozen occurrence, is unchanged.

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

### 5.9 The wire-key stability test (the tripwire Rule S1 does not otherwise have)

Section 3.3 measures that 22 of the 28 serialised members have **no** existing test that reddens when
their pin is omitted, and that the test round 1 nominated as the primary tripwire cannot distinguish
a present pin from an absent one. A missing pin compiles, moves no literal, passes criteria 6a and
6b, passes the round trip (both sides change together), and passes the frontend suite (which mocks
the boundary). It is silent user-data loss on upgrade. This test is what closes that.

**What it asserts.** For each of the 28 members of section 3.3, that the member's wire key is exactly
the key in that table's fourth column. `#[serde(rename)]` is bidirectional, so **one direction per
member is sufficient** proof that the pin is present and spelled right.

**Which direction, per member.** Read it off the `Derives` column of section 3.3; there is no
judgment call:

- **Owning type derives `Deserialize`** (20 members: rows 6-12, 14-24, 27, 28): assert
  deserialisation. Feed a JSON object carrying the frozen key with a distinctive value, then assert
  the renamed field or variant holds it.
- **Owning type derives `Serialize` only** (8 members: rows 1-5, 13, 25, 26): construct a value with
  the field set to a distinctive value and assert the frozen key carries it.

**"A minimal object is enough" is false, and the implementer must not be told it.** Serde requires
every field that carries neither `#[serde(default)]` nor `#[serde(skip)]` **and is not an
`Option<T>`**. The `Option<T>` exemption is not spelled by an attribute: serde's derive answers an
absent key on an `Option<T>` field with `None` rather than with an error, so an attribute-less
`Option<T>` is optional on the wire exactly as if it carried `#[serde(default)]`. Every other
attribute-less field is required, so `from_value(json!({ "<key>": .. }))` fails with `missing field`
on 7 of the 12 owning types in the deserialise direction. Measured at `147ad4ef` by reading each
struct definition field by field and classifying each field by its declared type, the fields each
type requires **in addition to the key under test**, spelled as the wire key:

| Owning type | Section 3.3 rows | Additional fields serde requires |
| --- | --- | --- |
| `TeamConfigResult` | 6 | none, every field has `#[serde(default)]` |
| `AgentDarkFactory` | 9 | none |
| `LoopPolicy` | 11 | none |
| `LoopTargetKind`, `PtyInputReasonCode` | 10, 23, 24 | n/a: they are enums, and the value under test is the whole document |
| `LoopCreateRequest` | 7 | `projectPath`, `name`, `expr`, `workgroup`, `promptBody`. Not `id`: it is `Option<String>` with no attribute |
| `LoopUpdateRequest` | 8 | `projectPath`, `id`. Not `name`, `expr`, `workgroup` or `promptBody`: all four are `Option<String>` with no attribute |
| `LoopAuditEntry` | 12 | `runId`, `loopId`, `projectPath`, `kind`, `dueAt`, `startedAt`. Not `completedAt`, `target`, `sessionId`, `error` or `promptSnapshot`: all five are `Option<..>` with no attribute |
| `PersistedSession` | 14 | `name`, `shell`, `shellArgs`, `workingDirectory` |
| `AppSettings` | 15-22 | `defaultShell`, `defaultShellArgs`, `agents` |
| `Session` | 27 | `id`, `name`, `shell`, `shellArgs`, `createdAt`, `workingDirectory`, `status`, `waitingForInput`, `token`. Not `lastPrompt`: it is `Option<String>` with no attribute |
| `SessionInfo` | 28 | `id`, `name`, `shell`, `shellArgs`, `createdAt`, `workingDirectory`, `status`, `waitingForInput`, `token`. Not `lastPrompt`: it is `Option<String>` with no attribute |

The recipe for the largest of these already exists in the tree: the issue-#248 migration tests at
`config/settings.rs:8255-8312` carry exactly `defaultShell`, `defaultShellArgs` and `agents` plus the
field under test, and the comment at `:8257-8259` says so. Follow that shape. Where a required
field's value is awkward to spell (a `Uuid`, a `DateTime<Utc>`, a `SessionStatus`), copy it from the
nearest existing fixture for that type rather than inventing one.

`AgentDarkFactory` (row 9) is the one member where the serialise direction would not work at all:
`is_coordinator_of` carries `skip_serializing_if = "Vec::is_empty"`, so a default value emits no key.
It is a `Deserialize` member and is asserted in that direction, which sidesteps this entirely.

**Where it lives: two existing `#[cfg(test)]` modules, no new file, no new module.**

| Site | Members | Why there |
| --- | --- | --- |
| `src-tauri/src/lib.rs`, inside the existing `#[cfg(test)] mod tests` at `:3899-3900`, test named `wire_keys_are_stable_for_every_renamed_serialised_member` | 25 (rows 4-28) | every owning type is `pub` or `pub(crate)`, so all are reachable from the crate root by a fully-qualified `crate::` path |
| `src-tauri/src/cli/team.rs`, inside the existing `#[cfg(test)] mod tests` at `:579-580` (which opens with `use super::*;`, so the three private types are already in scope), test named `team_cli_wire_keys_are_stable` | 3 (rows 1-3) | `TeamListItem`, `TeamCreateResult` and `AddMemberResult` are private to `cli::team` and are visible nowhere else |

Two sites, not twelve, because splitting per owning module would force a new `#[cfg(test)]` module
into `commands/loops.rs` and `config/agent_config.rs`, which have none today.

**Constraints on the test, all binding.**

1. Every key is typed as a **literal** in the assertion. Do not build one from a constant, a
   `stringify!`, or the renamed identifier, or the test proves nothing about the wire.
2. The test contains **no string literal matching `coordinat` other than the wire keys themselves**.
   Those are 16 distinct literals (the fourth column of section 3.3, deduplicated); 14 already exist
   in the base tree and 2 do not, which is exactly the pair criterion 6b expects. Any other
   `coordinat` literal in this test would break 6b.
3. Reference every type by its fully-qualified `crate::` path inside the `#[cfg(test)]` block. Do not
   add a `use` at module scope. The levelization detector ignores `#[cfg(test)]`, so this adds no
   module arc and cannot move `module-arcs.txt`; a module-scope `use` would be a different question
   and is not needed.
4. No production code changes for this test. If a member cannot be asserted without widening a
   visibility or adding a `Default`, stop and report it rather than changing production code.
5. **Every deserialise assertion feeds a value that differs from the member's serde default, and the
   test states that default in a comment on the same assertion.** This is what stops an assertion
   from being vacuous. If the fed value equals the default, the test passes with the pin removed:
   deserialisation ignores the now-unknown frozen key and supplies exactly the value being asserted.
   The distinctive value, per default kind: `bool` defaulting `false` → feed `true`; `bool`
   defaulting `true` → feed `false`; `String` with `#[serde(default)]` → feed a non-empty
   distinctive string, never `""`; `Vec<T>` with a default → feed a non-empty array, never `[]`;
   `Option<T>` with `#[serde(default)]` → feed a present value, never `null`; an enum-valued field
   with `#[serde(default)]` → feed a variant other than that type's `Default`; a numeric field with
   `default = "<fn>"` → feed a value that function does not return. Two shapes cannot be vacuous
   whatever value is chosen, and are exempt: a member the type **requires** (row 12,
   `LoopAuditEntry::busy_coordinator_policy`, which carries no default) and an enum variant under
   test (rows 10, 23, 24), because with the pin gone deserialisation fails outright rather than
   defaulting. The serialise direction is likewise not vacuous, provided the assertion names the
   frozen key on the produced JSON: with the pin gone that key is absent and the assertion fails.
   Section 12.1 C6b samples this constraint, one experiment per default kind.

Shape, for the two directions:

```rust
// Deserialize direction, e.g. section 3.3 row 22. The three fields before the key under test are
// the ones AppSettings requires (table above), lifted from the issue-#248 tests at settings.rs:8255.
// coordinatorCascadeCloseEnabled defaults to true (default_true, settings.rs:541), so false is the
// distinctive value required by constraint 5.
let v: crate::config::settings::AppSettings = serde_json::from_value(serde_json::json!({
    "defaultShell": "bash",
    "defaultShellArgs": [],
    "agents": [],
    "coordinatorCascadeCloseEnabled": false
}))
.expect("settings from wire");
assert!(!v.orchestrator_cascade_close_enabled);

// Serialize direction, e.g. section 3.3 row 26
assert_eq!(
    serde_json::to_value(crate::resource_monitor::watchdog::QuarantineRetryPath::Arbiter)
        .expect("variant to wire"),
    serde_json::json!("coordinator")
);
```

**This test is the reason section 9.1 says two new tests, not one.** It adds no behavior and pins no
new contract: it states, executably, the contract the tree already has.

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

Rule A unless the row says otherwise. "S1-pin" means a serde pin (5.3 S1) is added in that file;
"S2-pin" means a clap `long = "coordinator"` pin (5.3 S2) is added.

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
| `src-tauri/src/cli/team.rs` | **S1-pin** on `TeamListItem::coordinator` (:104), `TeamCreateResult::coordinator` (:115), `AddMemberResult::coordinator` (:127); **S2-pin** `long = "coordinator"` on `TeamCreateArgs::coordinator` (:49-53) and `TeamAddMemberArgs::coordinator` (:84-85); plus the test helper `make_coordinator`; plus the new `team_cli_wire_keys_are_stable` test of section 5.9 in the existing `#[cfg(test)]` module at :403. |
| `src-tauri/src/cli/terminal_snapshot.rs` | `is_coordinator` read. |
| `src-tauri/src/cli/workgroup.rs` | `coordinator` locals (:193, :370) and `config::coordinator_clocks` path; **S2-pin** `long = "coordinator"` on `WorkgroupAddArgs::coordinator` (:48-49), whose flag is `hide = true` and whose only test passes either way. |
| `src-tauri/src/commands/ac_discovery.rs` | **S1-pin** on `AcTeam::coordinator` (:84) and `AcAgentReplica::is_coordinator` (:112); the `coordinator_clocks` State args at :1040 and :1826; 11 identifiers total. |
| `src-tauri/src/commands/entity_creation.rs` | **S1-pin** on `TeamConfigResult::coordinator` (:59); the `coordinator: String` value args of `create_team` (:2774) and `update_team` (:3348) stay frozen; `config::coordinator_clocks` paths; 12 identifiers total. |
| `src-tauri/src/commands/loops.rs` | **S1-pin** on `LoopCreateRequest::busy_coordinator` (:30) and `LoopUpdateRequest::busy_coordinator` (:45); `WorkgroupCoordinator` variant. |
| `src-tauri/src/commands/pty.rs` | `CoordinatorClocks`, `CoordinatorClocksState`, `coordinator_clocks`, `coordinator_cwd`. The four `[coordinator-clocks]` log prefixes are literals: frozen. |
| `src-tauri/src/commands/resource_monitor.rs` | Rule B: `SelectionCoordinator*` and `QuarantineRetryPath::Coordinator` at :1737 and :1827. |
| `src-tauri/src/commands/session.rs` | 16 identifiers. Rule A on `coordinator_clocks`, `coordinator_id`, `coordinator_matrix`, `coordinator_cascade_close_enabled`, `execute_manual_coordinator_destroy`, `is_coordinator`, `is_coordinator_for_cwd`, `CoordinatorCloseOutcome`; Rule B on `SelectionCoordinator`, on the `coordinator: State<..>` arg at :4582, and on **`coordinator_shutdown` at :1432, consumed at :2573, which becomes `arbiter_shutdown` and NOT `orchestrator_shutdown`** (5.2 binding clause). **The `close_coordinator` fn name at :3406 does not change.** The `is_coordinator` argument at the `materialize_agent_context_file_with_filename_activated` call site changes and drags allowlist entry L8. |
| `src-tauri/src/commands/window.rs` | Rule B: `SelectionCoordinator` and its local binding. |
| `src-tauri/src/config/activity_log.rs` | `is_coordinator` read. |
| `src-tauri/src/config/agent_config.rs` | **S1-pin** on `AgentDarkFactory::is_coordinator_of` (:95). |
| `src-tauri/src/config/instance_artifacts.rs` | `COORDINATOR_CLOCKS_FILE_NAME` (:129) and `COORDINATOR_CLOCKS_TMP_GLOB` (:134) identifiers; their **values** `"coordinator_clocks.json"` and `"coordinator_clocks.json.*.tmp"` are frozen, and so is the doc comment's reference at :133 except for the symbol name it cites. The test fn name at :617 renames, and **allowlist L12 applies at :620**: `format!("{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp")` is an inline capture of the renamed const and does not compile unless the literal follows. |
| `src-tauri/src/config/loops.rs` | **S1-pin** on `LoopTargetKind::WorkgroupCoordinator` (:25), `LoopPolicy::busy_coordinator` (:98), `LoopAuditEntry::busy_coordinator_policy` (:153), `AcLoopSummary::busy_coordinator` (:169); plus `BusyCoordinatorPolicy` (7 identifiers). |
| `src-tauri/src/config/mod.rs` | `pub mod coordinator_clocks;` at :12 becomes `pub mod orchestrator_clocks;`. |
| `src-tauri/src/config/projects.rs` | `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` use, `coordinator_bytes`, `coordinator_template`. |
| `src-tauri/src/config/root_agent.rs` | 5 identifiers. Every template string it holds is frozen. |
| `src-tauri/src/config/seed_manifest.rs` | `V1CoverageBoundary::CoordinatorStatelessV2ToV4`, `::CoordinatorStatelessV3ToV4`, `::CoordinatorSeededV3ToV4` at :3279-3281 and :5913-5915. The enum is `pub(crate)` and not serde-derived. |
| `src-tauri/src/config/seeded_context_templates.rs` | 22 identifiers, including the four frozen-snapshot constants (identifier only, bytes frozen) and `get_default_coordinator_template`, `is_known_generated_coordinator_template`, and the byte-exactness test fn names. |
| `src-tauri/src/config/session_context.rs` | 21 identifiers, including `COORDINATOR_CONTEXT_TEMPLATE_FILENAME` at :13 (its value `"Context.coordinator.md"` is frozen), `coordinator_path`, `coordinator_content`, `coordinator_template_path`. |
| `src-tauri/src/config/sessions_persistence.rs` | **S1-pin** on `PersistedSession::is_coordinator` (:377); one test fn name. |
| `src-tauri/src/config/settings.rs` | **S1-pin** on the 7 members at :294, :527, :529, :532, :534, :538, :542; identifier-only change on `legacy_start_only_coordinators` at :310, whose existing `rename = "startOnlyCoordinators"` is untouched. All `"startOnlyCoordinators"` / `"restoreCoordinatorWakeState"` / `"coordinator*"` literals in the migration and its tests are frozen. |
| `src-tauri/src/config/teams.rs` | The largest Rust file in the phase, 39 identifiers: `is_coordinator`, `is_coordinator_of` (:1619), `is_any_coordinator` (:1626), `is_coordinator_for_cwd` (:1636), `resolve_wg_coordinator_replica` (:442), `verified_wg_coordinator_target` (:493), `verify_pty_input_coordinator_root` (:1038), `DiscoveredTeam::coordinator_name` (:37) and `::coordinator_path` (:39) with **no pin** (not serde), `validate_coordinator_to_root_route`, `TerminalSnapshotAuthorityKind::Coordinator` (:567), `VerifiedPtyInputIdentity::is_coordinator` (:543, not serde, no pin), and the test fn names. Allowlist **L9** covers the `Debug` labels at :583 and :603 and the assertion at :2360; **L10** covers the `Debug` label at :584; **L11** covers the assertion at :2328, which reads the derived `Debug` of the renamed variant and goes red if left; **L14** covers `format!("_agent_{coordinator}")` at :822, an inline capture that does not compile unless the literal follows. The `format!("_agent_{member}")` at :809 is untouched. |
| `src-tauri/src/lib.rs` | 11 identifiers: `CoordinatorClocksState`, `coordinator_clocks`, `coordinator_clocks_for_exit`, `is_any_coordinator`, `is_coordinator`, `restore_coordinator_wake_state`, and Rule B on `SelectionCoordinator`, `selection_coordinator`, `selection_coordinator_for_exit`, `selection_coordinator_for_setup`. **The `close_coordinator` entry in the `generate_handler!` list, `lib.rs:3370`, does not change.** Also gains the new `wire_keys_are_stable_for_every_renamed_serialised_member` test of section 5.9, inside the existing `#[cfg(test)]` module. |
| `src-tauri/src/loops/delivery.rs` | 8 identifiers around the busy-orchestrator policy and target resolution. |
| `src-tauri/src/loops/scheduler.rs` | `BusyCoordinatorPolicy`, `WorkgroupCoordinator`, `busy_coordinator`, `busy_coordinator_policy`. |
| `src-tauri/src/phone/mailbox.rs` | 23 identifiers on the routing and authorization path. Every FQN fixture literal is frozen. |
| `src-tauri/src/phone/types.rs` | **S1-pin** on `PtyInputReasonCode::SenderNotCoordinator` (:82) and `::TargetIsCoordinator` (:85). |
| `src-tauri/src/pty/container_backend.rs` | One test fn name. |
| `src-tauri/src/pty/container_tokens.rs` | `verify_pty_input_coordinator_root` call. |
| `src-tauri/src/pty/git_watcher.rs` | `CoordinatorChangedPayload` (:414) type name renames freely (a struct type name is not serialised); **S1-pin** on its `is_coordinator` field (:416). |
| `src-tauri/src/pty/inject.rs` | `is_coordinator` read. |
| `src-tauri/src/pty/terminal_snapshot.rs` | `augment_coordinator_project`, `is_coordinator`, `verify_pty_input_coordinator_root`. |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | 6 identifiers; the FQN fixture literals are frozen. |
| `src-tauri/src/resource_monitor/watchdog.rs` | Rule B: `SelectionCoordinator`, `running_coordinator`, and **S1-pin** on `QuarantineRetryPath::Coordinator` (:35) which becomes `::Arbiter` with `#[serde(rename = "coordinator")]`. |
| `src-tauri/src/screenshot/windows.rs` | Rule B: one `SelectionCoordinator` reference. |
| `src-tauri/src/session/auto_close.rs` | 17 identifiers, Rule A on the clock and cascade path, Rule B on the arbiter handle. The three event-name literals are frozen. |
| `src-tauri/src/session/context_alerts.rs` | 6 identifiers. |
| `src-tauri/src/session/manager.rs` | 16 identifiers, including `coordinator_refs_by_team`, `coordinator_ids_by_team`, `coordinator_cwd`; allowlist L9 covers the `Debug` label at :63. |
| `src-tauri/src/session/selection.rs` | Concept B owner. All 8 Concept B symbols plus every local binding, and allowlist L7 for the 7 sentinel literals. The 6 Concept A lines the epic measured in this file take Rule A. |
| `src-tauri/src/session/session.rs` | **S1-pin** on `Session::is_coordinator` (:122) and `SessionInfo::is_coordinator` (:298). |
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

### 6.5 `repo-AgentsCommander`: MODIFIED, TypeScript (28)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src/shared/ipc.ts` | `CoordinatorCloseOutcome`, `closeCoordinator` (:213), `onSessionCoordinatorChanged` (:753), `onCoordinatorClockUpdated` (:779), `onCoordinatorAutoCloseChanged` (:788), `onCoordinatorManualCloseChanged` (:797), `isSelectionCoordinatorBusyError` (Rule B), and the local `const coordinator` at :1037. **The shorthand at :1081 expands to `coordinator: orchestrator,`** (Rule K shorthand clause): the property is the frozen wire key, the binding is the renamed `const`. Frozen: the `"close_coordinator"` invoke name at :214, the four event-name literals, the `{ isCoordinator: boolean }` payload types at :754/:756, and `busyCoordinator` at :971/:985. **The `coordinator: string` parameters at :1114 and :1134 rename to `orchestrator`** (Rule K does not reach a parameter; see the register rows in 5.5 and decision 10.1.14), **and the shorthands at :1122 and :1142 each expand to `coordinator: orchestrator,`**, which keeps the payload key byte-identical. All six call sites pass positionally, so no caller changes. |
| `src/shared/ipc.transport.test.ts` | `isSelectionCoordinatorBusyError` call at :134. Every `"selectionCoordinator*"` string and every `coordinator:` team-config fixture key is frozen. |
| `src/shared/shortcuts.ts` | `requestCoordinatorCloseById` import and call, plus allowlist L6 at :3. |
| `src/shared/types.ts` | `BusyCoordinatorPolicy` (:1322, type alias name; its three string values are unchanged), `CoordinatorCloseOutcome`, `TeamSessionGroup.coordinator` (:1179), `Team.coordinatorName` (:1193). Frozen: `isCoordinator` at :40 and :1243, `AcTeam.coordinator` at :1232, **`TeamConfigResult.coordinator` at :1505**, the settings keys, `busyCoordinator` at :1338, `LoopTargetKind = "workgroupCoordinator"` at :1320. |
| `src/sidebar/App.tsx` | `onSessionCoordinatorChanged`, `setIsCoordinator`, `isSelectionCoordinatorBusyError`. The destructured `isCoordinator` payload binding at **:800** is frozen; :801 is its use. |
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
| `src/sidebar/components/ProjectPanel.raise-hand.test.tsx` | **New in round 2.** The helper parameters `isCoordinator` at :34 and :47 and the argument use at :54 rename to `isOrchestrator`; the shorthand at :39 expands to `isCoordinator: isOrchestrator,`. Frozen: the object properties at :67 and :191, and the three test titles at :106, :186, :202. This is the only TypeScript file outside the tables that carried an in-scope local; section 5.5 gives the sweep that proves it. |
| `src/sidebar/components/ProjectPanel.regex-filter.test.tsx` | **One new `it(...)`** per section 5.8. No existing assertion changes; every `coordinator:` fixture key stays. |
| `src/sidebar/components/ProjectPanel.repo-browse.automation.test.tsx` | Local helper `coordinatorAgent` (:144). |
| `src/sidebar/components/ProjectPanel.repo-browse.test.tsx` | Local helper `coordinatorAgent`. |
| `src/sidebar/components/ProjectPanel.tsx` | The largest TypeScript file in the phase: `coordinatorItemKey` (:347), `runningCoordinatorPeers` (:355), `coordinatorsCollapsedKey` (:1871), `coordinatorPairCache` (:1879), `naturalCoordinatorItems` (:1880), `coordinatorItems` (:1908), `recordCoordinatorVisibleOrder` (:1911), `coordinatorVisibleOrder` (:1916), `selectedCoordinatorItem` (:1923), `filteredCoordinatorItems` (:1940), the six imports from the renamed stores and modules, `coordinatorIdleBadge`, the four `on*` listeners. Frozen: `replica.isCoordinator` reads, the `"coordinators"` collapse-key literal at :1871, the `"orchestrator"` filter tokens at :975 and :1016, the three `coordinatorClose.*` testids at :4198, :4213, :4221, and allowlist entries L2, L3, L5 for its three specifiers. |
| `src/sidebar/components/SessionItem.tsx` | `requestCoordinatorClose` import and call, plus allowlist L5 at :9. |
| `src/sidebar/components/SessionItem.test.tsx` | Locals `nonCoordinator` (:343) and `noRepoCoordinator` (:367). |
| `src/sidebar/components/SettingsModal.tsx` | Local `validateCoordinatorIdle` (:1652). All seven settings keys and the six `settings.general.coordinator*` testids are frozen. |
| `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx` | Test-helper option `coordinator?: boolean` (:27, :29, :145) and its use at :40. |
| `src/sidebar/components/loop-modal-helpers.ts` | `LoopCoordinatorOption` (:3), `coordinatorName` (:5, :22), `coordinatorOptionsFromWorkgroups` (:13), the local `coordinator` (:18, :19, :22, :23). **Allowlist L13 applies at :23**: the template literal `` `${wg.name} - ${coordinator.name}` `` interpolates the renamed local and does not typecheck unless it follows. `agent.isCoordinator` (:18) and `BusyCoordinatorPolicy`-typed members frozen except the type-alias name itself. |
| `src/sidebar/stores/sessions.ts` | `lastCoordinatorVisibleOrderByProject` and its setter (:14), `frozenCoordinatorVisibleOrderByProject` and its setter (:15), `recordCoordinatorVisibleOrder`, `coordinatorVisibleOrder`, `setIsCoordinator`, the local `coordinator` (:249, :279) and `team.coordinatorName` (:255, :269). |
| `src/sidebar/stores/sessions-helpers.test.ts` | `coordinatorVisibleOrder`, `recordCoordinatorVisibleOrder`. |
| `src/terminal/App.tsx` | `isSelectionCoordinatorBusyError` (Rule B). |

### 6.6 `repo-AgentsCommander`: MODIFIED, docs and data (3)

| Path completo archivo | Que se modifico |
| --- | --- |
| `src-tauri/module-arcs.txt` | Regenerated by the two-step pipeline of section 9.4. 14 lines move; the arc set is unchanged under the renaming bijection. Expected post SHA-256 `2EF5875ADE100F71B52E1D552755F11091E92D1A7EDA3A9351F00DFA6D9E92E6`, 1037 lines, 82163 bytes. |
| `docs/reference/architecture.md` | Line 777, one table cell: `config/coordinator_clocks.rs` becomes `config/orchestrator_clocks.rs`. The cell's description already says "Orchestrator idle clocks". Nothing else in the file changes. |
| `docs/reference/directory-layout.md` | Line 76, one table cell: the third column `config/coordinator_clocks.rs` becomes `config/orchestrator_clocks.rs`. **The first column, `coordinator_clocks.json`, does not change**: that is the on-disk artifact name, frozen for phase 3. Nothing else in the file changes. |

`docs/features/session-auto-close.md:151` is deliberately absent: it names `coordinator_clocks.json`,
the frozen artifact, not the renamed source file. These three doc lines are the complete set of
tracked-tree references to any of the seven renamed paths outside `src/`, `src-tauri/`,
`module-arcs.txt`, `plans/` and `CHANGELOG.md`.

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
   unchanged, and a file written by this build loads in the previous release. Rule S1 is what makes
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

**Two new tests, in three `it`/`fn` bodies.** Round 1 said "exactly one"; that was wrong, because the
one gate the phase actually lacked had no test at all.

| Test | Where | What it pins | Why it exists |
| --- | --- | --- | --- |
| `matches an orchestrator row by the synthetic filter token, ...` | `src/sidebar/components/ProjectPanel.regex-filter.test.tsx` (section 5.8) | the synthetic `"orchestrator"` filter token phase 1 introduced | issue item 3c, residual R11 of #1571 |
| `wire_keys_are_stable_for_every_renamed_serialised_member` | `src-tauri/src/lib.rs` `#[cfg(test)]` (section 5.9) | the exact wire key of 25 of the 28 members of section 3.3 | 22 of those members have no existing tripwire (measured, section 3.3); a missing Rule S1 pin is otherwise completely silent |
| `team_cli_wire_keys_are_stable` | `src-tauri/src/cli/team.rs` `#[cfg(test)]` (section 5.9) | the remaining 3 members, whose types are private to that module | same, plus module privacy |

This phase adds no behavior, so it adds no behavioral test beyond the first row; what it must prove
is that nothing moved. That is proved by the wire-key test above, the negative controls in 9.2, and
the two comparators in 9.3 and 9.4.

### 9.2 Negative controls: existing tests that must stay green with their assertions untouched

If any of these needs an assertion edited, the run has crossed a boundary and must stop, **with the
three declared exceptions listed after the table**, where the plan itself mandates the edit.

| Test | File | What it pins |
| --- | --- | --- |
| `coordinator_clock_settings_default_when_keys_absent` | `config/settings.rs:7044` | removes five `coordinator*` JSON keys from a serialised default and asserts the defaults come back. It must stay green with its five `obj.remove("coordinator...")` literals **frozen**. Note what it does **not** do: it is not a tripwire for a missing pin, because the value it reads back is by construction the value it asserts (section 3.3). Round 1 claimed it was the primary Rule S tripwire; it is not, and section 5.9 supplies the real one. |
| `coordinator_auto_close_skip_telegram_assigned_round_trips` | `config/settings.rs:7068` | asserts `json.get("coordinatorAutoCloseSkipTelegramAssigned")` is present after serialising. |
| the four issue-#248 migration tests | `config/settings.rs:8255-8347` | assert `!out.contains("startOnlyCoordinators")` and `out.contains("\"restoreCoordinatorWakeState\":true")`. The first of those is **not** a tripwire for row 16's existing pin: after the migration the field is `None` and `skip_serializing_if` elides it, so the negative assertion passes either way. They are also the plan's worked recipe for the required-field problem of section 5.9. |
| `exact_coordinator_error_strings_are_stable` | `session/selection.rs` | pins the three `selectionCoordinator*` error strings. Must stay green **unrenamed in its string content**. |
| `source_ownership_sentinel_rejects_each_one_line_mutation` and the `ArbiterJob` sentinel | `session/selection.rs:3961-4053` | reads `selection.rs` back and pins the enum declaration. Green only if allowlist L7 is applied completely. |
| `session_rs_threads_production_tokens_for_config_seed_and_context` | `tests/cli_workgroup_team.rs:1810` | pins the `is_coordinator` argument in the scraped call site. Green only if allowlist L8 is applied. |
| `coordinator_pre_token_minimization_snapshot_is_byte_exact`, `coordinator_pre_cross_workgroup_snapshot_is_byte_exact`, `coordinator_pre_orchestrator_rename_snapshot_is_byte_exact`, `old_coordinator_raise_hand_snapshot_is_byte_exact` | `config/seeded_context_templates.rs` | hash the frozen constants' bytes. Renaming the constants must not move a hash. |
| `coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name` | `config/instance_artifacts.rs:617` | pins that `COORDINATOR_CLOCKS_TMP_GLOB` is derivable from `COORDINATOR_CLOCKS_FILE_NAME`. **Declared exception: this test's body must change.** Its `format!("{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp")` at `:620` is an inline capture of a renamed const and will not compile otherwise (allowlist L12), and its own fn name renames by Rule A. The two **values** it compares, `"coordinator_clocks.json"` and `"coordinator_clocks.json.*.tmp"`, stay byte-identical, so the property it asserts is unchanged. Round 1 listed this test as "assertions untouched", which contradicted its own section 6.3. |
| `cli_workgroup_team.rs` `team_config["coordinator"]` assertions at :525 and :1342 | `tests/cli_workgroup_team.rs` | pin the team-config JSON key. |
| `cli_loop.rs` `list["loops"][0]["busyCoordinator"]` at :234, `busyCoordinator = "waitUntilIdle"` at :181, `"forceInject"` at :208 | `tests/cli_loop.rs` | pin the loop config key and its values. |
| `cli_project_registration.rs` `scope = "context:coordinator"` at :316, :489, :547 | `tests/cli_project_registration.rs` | pins the seed-manifest scope string. |
| `terminal-snapshot-portable` (both commands, 4 OSes) | `crates/` | pure negative control: `crates/` has zero identifiers in this phase, so these must be green and unchanged. |
| the 62 frontend test files' `isCoordinator` / `coordinator` / settings-key fixtures | `src/**/*.test.ts(x)` | pin every wire key on the TypeScript side. |

**The four declared assertion-text exceptions.** Every test whose assertion *text* changes in this
phase does so for a Rule P allowlist reason, and there are exactly four. Anything beyond this list is
a defect and stops the run.

| Test | File | Allowlist entry | Why the text must change |
| --- | --- | --- | --- |
| `coordinator_jobs_are_typed_data_without_managed_handles_or_futures` and `source_ownership_sentinel_rejects_each_one_line_mutation` | `session/selection.rs:3961-4056` | L7 | the sentinel reads `selection.rs` back and pins the enum by name |
| `session_rs_threads_production_tokens_for_config_seed_and_context` | `tests/cli_workgroup_team.rs:1810` | L8 | it scrapes `commands/session.rs` and pins the argument name in the call-site text |
| `terminal_snapshot_target_debug_omits_identity_and_path_text` (`teams.rs:2346`) and `terminal_snapshot_coordinator_policy_is_distinct_from_pty_input` (`teams.rs:2302`) | `config/teams.rs:2360`, `:2328` | L9, L11 | each asserts on `Debug` output whose text is a renamed field label or enum variant |
| `coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name` | `config/instance_artifacts.rs:617` | L12 | inline format capture of a renamed const; a compile error otherwise |

In all four, the **property** asserted is identical before and after. If any *other* test needs an
assertion edited, the run has changed behavior and must stop.

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

**Green iff both hold.** Round 1's formulation failed in both directions on a *correct*
implementation: 6a's allowlist was missing five literals the rename forces, and 6b was
unsatisfiable, because Rule S1 introduces key literals that did not exist before. Both halves below
are now closed, positive, and reproducible from the base tree.

- **6a. Every `<=` row matching `coordinat` is one of the 22 expected disappearances.** These are the
  23 distinct literals of the section 5.4 allowlist, minus L10 `"target_is_coordinator"`, which
  survives at five frozen sites and therefore never leaves the set (section 5.4a). Concretely, the
  22 are: the six module specifiers L1-L6; the seven `CoordinatorJob` literals of L7; the one
  scraped call-site literal of L8; the four labels and assertion of L9; `"kind: Coordinator"` (L11);
  `"{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp"` (L12); the L13 template; and `"_agent_{coordinator}"`
  (L14). **No other `coordinat` literal may disappear.** Fewer than 22 rows is also a failure unless
  the run can name which allowlist entry it did not need.

- **6b. The `=>` rows matching `coordinat` are exactly two: `"isCoordinatorOf"` and
  `"busyCoordinatorPolicy"`.** Not "none". Rule S1 writes 27 `#[serde(rename = "<key>")]` attributes,
  whose keys are 16 distinct literals; 14 of them already exist in the base tree, so pinning them
  adds no new set member, and **exactly two do not exist anywhere in either repo today**. Those two
  are the keys of `AgentDarkFactory::is_coordinator_of` (`config/agent_config.rs:95`) and
  `LoopAuditEntry::busy_coordinator_policy` (`config/loops.rs:153`), which, not coincidentally, are
  two of the members section 3.3 shows have no other coverage of any kind. So this half is a
  **positive** gate: if either row is missing, that pin was not written.

  Reproduce the "exactly two" from the base set before trusting it:

  ```powershell
  $before = [System.Collections.Generic.HashSet[string]]::new()
  foreach ($l in [System.IO.File]::ReadAllLines("$S\before.txt")) { [void]$before.Add($l) }
  foreach ($k in @('coordinator','isCoordinator','busyCoordinator','isCoordinatorOf',
                   'workgroupCoordinator','busyCoordinatorPolicy','restoreCoordinatorWakeState',
                   'coordinatorIdleBadgeYellowMinutes','coordinatorIdleBadgeRedMinutes',
                   'coordinatorAutoCloseEnabled','coordinatorAutoCloseMinutes',
                   'coordinatorAutoCloseSkipTelegramAssigned','coordinatorCascadeCloseEnabled',
                   'sender_not_coordinator','target_is_coordinator','startOnlyCoordinators')) {
    '{0,-45} {1}' -f $k, $(if ($before.Contains('"' + $k + '"')) { 'PRESENT' } else { 'ABSENT' })
  }
  # exactly two ABSENT: isCoordinatorOf, busyCoordinatorPolicy
  ```

  Rule S2 adds three `long = "coordinator"` literals; `"coordinator"` is PRESENT, so S2 contributes
  no row. The new tests of sections 5.8 and 5.9 contribute no row either: 5.8's only new literal is
  `orchestrator`, and 5.9 is constrained to contain no `coordinat` literal beyond the 16 wire keys.

Rows not matching `coordinat` are the renamed counterparts (`"./orchestrator-badge"`,
`"enum ArbiterJob"`, `"is_orchestrator"`, `"kind: Orchestrator"`, `"_agent_{orchestrator}"`, ...).
Inspect them, but they are not part of the gate: a serialised value containing the word
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
| 3 | `npm run check:frontend-dependencies` reports no cycle regression | `npm run check:frontend-dependencies` | **the measured triple is unchanged: `modules: 351, errors: 0, dependencies: 1535`.** Compare the triple, not the module list: the 6 TypeScript renames of R2-R7 change 6 node *names*, so an identical listing is the wrong expectation and "same module count" was the wrong phrasing. Baseline 351/0/1535 was run on the base tree with dependency-cruiser 18.0.0 | **implementer locally. No CI job runs this** (section 3.8). Run once on the base and once on the tip; both outputs in the PR body |
| 4 | Levelization gate passes with a regenerated `module-arcs.txt` | section 9.4 | all five conditions of 9.4 | **implementer locally. No CI job runs this** (section 3.8). Digest pasted in the PR body |
| 5 | `git log --follow` works on all 7 renamed files | `git log --follow --oneline -- <new path>` for each of the 7 | each listing reaches commits authored before C1, i.e. the file's pre-rename history | implementer at the branch tip, before opening the PR |
| 6 | No diff touches a serialised literal | section 9.3 | 6a and 6b both hold | implementer at the branch tip; reviewer re-runs the same command |
| 7 | The two concepts no longer share a word | `Select-String -Pattern 'Coordinator' src-tauri\src\session\selection.rs src-tauri\src\commands\resource_monitor.rs src-tauri\src\resource_monitor\watchdog.rs src-tauri\src\commands\window.rs` | the only remaining matches are the three phase-3 error-code strings `selectionCoordinatorUnavailable`, `selectionCoordinatorBusy`, `selectionCoordinatorRecursiveSubmission` and the test that pins them. Criterion 7 is certified in the form **"no identifier is shared between the two concepts"**, which is an accepted residual, not an open question: see 10.2.1 | implementer at the branch tip. See residual 10.2.1 |

Additional gate, not in the issue but required by section 5.3. Round 1's criterion 8 named a
tripwire that does not exist; this is its replacement, and it has three independent parts because a
missing pin is invisible to the compiler, to criterion 6 and to the frontend suite alike.

| # | Criterion | Command | Green means |
| --- | --- | --- | --- |
| 8a | Every serialised key is proved at runtime | `cargo test --lib wire_keys_are_stable_for_every_renamed_serialised_member team_cli_wire_keys_are_stable`, cwd `src-tauri`, stdout redirected | both tests green, 28 members asserted. This is the only gate that actually reddens on a missing pin for the 22 members that have no other coverage |
| 8b | The pin count and spelling match the plan | `git diff 147ad4ef..HEAD -- src-tauri/src \| Select-String -Pattern '^\+.*serde\(.*rename = '` | **exactly 27 added lines**, and their key strings are exactly the 27 of section 3.3 rows 1-15 and 17-28. Zero added lines that pin a key not in that table. This is section 14.1's manual count turned into a command |
| 8c | The clap flags did not move | `git diff 147ad4ef..HEAD -- src-tauri/src/cli \| Select-String -Pattern '^\+.*arg\(long'` includes the three S2 pins, and `cargo test --tests cli_workgroup_team` is green with its ~20 `--coordinator` invocations and `workgroup_add_help_hides_team_definition_flags` **unedited** | Rule S2 was applied at all three sites. The `workgroup add` flag is `hide = true`, so its test passes either way; 8c's diff half is what catches it |

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
5. **No string literal changes except the 14-entry allowlist of 5.4.** This is what makes criterion 6 a
   mechanical gate instead of a judgment call, and it is why test titles, `expect()` prose and log
   format strings are explicitly routed to phase 4.
6. **`CoordinatorChangedPayload` renames but its `is_coordinator` field is pinned.** A struct type
   name is never serialised by serde; a field name always is.
7. **One non-building commit per repo, containing only renames.** Section 7 item 6.
8. **Two pull requests, one plan.** The repos have independent CI and independent merge policy. The
   app PR is the one that carries acceptance criteria 1 to 8; the website PR carries `npm run check`
   and `npm run smoke`.
9. **The new test lives in `ProjectPanel.regex-filter.test.tsx`, not in a new file.** The behavior it
   pins is the regex filter's, its fixture already exists there, and a new file would need a new
   fixture for one assertion.
10. **Rule S covers clap as well as serde (S2), rather than freezing the three fields.** Freezing
    them would leave three Rust identifiers named `coordinator` after a phase whose whole purpose is
    to remove them, and would contradict the register's already-closed verdicts on locals. Pinning
    `long = "coordinator"` renames the identifier and keeps the public flag byte-identical, which is
    what section 4 requires. The precedent is `cli/loop_cmd.rs:82`/`:104`, in the same directory.
11. **The wire-key stability test is added, and 9.1 grows from one new test to two.** The alternative
    considered and rejected was leaving Rule S1 verified by the manual count in 14.1. Measurement,
    not preference, decided this: 22 of 28 members have no tripwire, and the test round 1 nominated
    cannot fail. A missing pin is silent user-data loss on upgrade, which is the one enhanced control
    section 13.1 declares applicable to this phase, so it needs executable evidence.
12. **Criterion 6b whitelists the two genuinely new key literals rather than making the comparator
    ignore `#[serde(..)]` strings.** Both would make 6b satisfiable. The whitelist was chosen because
    it turns 6b from a negative gate into a positive one that proves the two least-covered pins were
    written, and because it leaves the comparator a pure set-diff that a reviewer re-runs unmodified.
    No third option stays open.
13. **The two `docs/` path references are fixed in this phase (in-scope item 9), not deferred.** They
    name `config/coordinator_clocks.rs`, a path that stops existing at commit C1. A stale path in a
    reference table is a defect the rename creates, not the pre-existing prose that epic decision 2
    protects. The neighbouring line that names `coordinator_clocks.json` is left alone, because that
    artifact really does keep its name until phase 3.
14. **The two `ipc.ts` command parameters rename, and Rule K's "if and only if" is left exactly as it
    is.** `coordinator` at `src/shared/ipc.ts:1114` and `:1134` is an arrow-function parameter: not a
    property name, not an interface or type member, not a destructuring binding. Rule K's
    enumeration therefore does not reach it and Rule A applies, as it does to every other local in
    the register. The alternative considered was to widen Rule K with a fourth category covering
    "parameters whose value travels under a frozen key". It is rejected on three grounds. The
    parameter name crosses no boundary: `transport.invoke` takes `Record<string, unknown>`, and the
    payload key is fixed by the shorthand's property half, not by the parameter. All six call sites
    pass positionally, so no caller is affected. And freezing them would leave two TypeScript
    identifiers spelled `coordinator` alive after a phase whose stated purpose is to remove them,
    which is the same argument decision 10.1.10 uses to reject freezing the three clap fields.
    Widening the rule to protect a name that nothing reads would also make Rule K's boundary a
    judgment call again, which round 1 already showed is the expensive failure mode. Consequences,
    all written down: the shorthands at `:1122` and `:1142` expand, the conflicting-shorthand count
    goes from 2 to 4, and `ipc.transport.test.ts:196`, `:207`, `:220`, `:231` stay green **unedited**,
    because the emitted key does not move.
15. **C6b's discrimination proof is expanded to one experiment per default kind, and the general case
    is closed by a constraint rather than by more sampling.** The residual is real but narrow: only a
    *deserialise* assertion can be vacuous, and only when the fed value equals the member's serde
    default. Three experiments sampled the harness; they did not sample that hazard. The fix has two
    halves. Constraint 5 of section 5.9 makes "different from the member's default" a binding
    property of all 20 deserialise assertions and requires the default to be written beside each one,
    so a reviewer checks it by reading rather than by running. C6b then samples one member per
    default kind, which is exactly where an implementer's value choice can collide with a default,
    plus one serialise experiment per test site. The alternative considered and rejected was one
    experiment per pin: 27 pin-removal runs buy nothing over the constraint plus the per-kind sample,
    because members that share a default kind fail and pass together.

### 10.2 Accepted residuals, each owned by a later phase

1. **The three `selectionCoordinator*` error strings survive this phase, and criterion 7 is certified
   in a reformulated form that the tech lead has explicitly accepted.** Issue #1572 routes those
   strings to phase 3 in as many words, because they cross into the frontend. Criterion 7 read
   literally ("the two concepts no longer share a word") is therefore self-contradictory *inside the
   issue*: the same issue that demands it also defers the three strings that violate it. The only
   consistent reading is **"no identifier is shared between the two concepts"**, with the three
   strings, and the test `exact_coordinator_error_strings_are_stable` that pins them, as the issue's
   own declared exception.

   **This is a settled decision, not an open question.** `ac-tech-lead-v3` accepted this
   reformulation explicitly in the round-2 brief of 2026-08-27, on the record and by name, and will
   surface it to the user at Step 7.5. It is recorded here as an accepted residual with owner
   **#1573**, which is where the three strings are eliminated and criterion 7 becomes literally true.
   No reviewer needs to re-open it, and no implementer needs to decide it.
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

### 12.1 `repo-AgentsCommander`, 13 commits

| # | Commit | Content | Gate before moving on |
| --- | --- | --- | --- |
| C0 | `docs(1572): plan for orchestrator internal identifiers` | this file only | none |
| C1 | `refactor(1572): rename 7 coordinator files (pure git mv)` | the 7 `git mv` of section 5.6 and nothing else. **Does not build. This is the mandated split.** | `git show --stat C1` shows 7 renames, 0 insertions, 0 deletions |
| C2 | `refactor(1572): orchestrator identifiers in config/` | `config/orchestrator_clocks.rs`, `config/mod.rs`, `teams.rs` (allowlist **L9, L10, L11, L14**), `settings.rs` (Rule S1), `session_context.rs`, `seeded_context_templates.rs`, `instance_artifacts.rs` (allowlist **L12**), `projects.rs`, `seed_manifest.rs`, `root_agent.rs`, `loops.rs` (Rule S1), `agent_config.rs` (Rule S1), `sessions_persistence.rs` (Rule S1), `activity_log.rs`, plus the two `docs/` path cells of section 6.6 | `cargo check --all-targets` |
| C3 | `refactor(1572): orchestrator identifiers in commands/, lib.rs, web/` | `commands/*.rs` (Rule S1 on `ac_discovery`, `entity_creation`, `loops`), `lib.rs`, `web/commands.rs` | `cargo check --all-targets` |
| C4 | `refactor(1572): orchestrator identifiers in cli/, api/, phone/, loops/, pty/, session/` | the remaining Concept A files of section 6.3, with Rule S1 on `cli/team.rs`, `phone/types.rs`, `pty/git_watcher.rs`, `session/session.rs`, and **Rule S2 on `cli/team.rs:49-53`, `:84-85` and `cli/workgroup.rs:48-49`** | `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --tests cli_workgroup_team` green with its `--coordinator` invocations unedited (criterion 8c) |
| C5 | `refactor(1572): SelectionCoordinator becomes SelectionArbiter` | Rule B across the 12 production files plus allowlist L7, plus Rule S1 on `QuarantineRetryPath::Coordinator`, plus `coordinator_shutdown` becoming `arbiter_shutdown` at `commands/session.rs:1432`/`:2573` | `cargo check --all-targets` |
| C6 | `refactor(1572): orchestrator identifiers in src-tauri/tests` | the 6 files of section 6.4, including allowlist L8 | `cargo test --lib --bins --tests` (redirect stdout to a file), all green |
| C6b | `test(1572): pin the 28 serialised wire keys` | the two tests of section 5.9: `wire_keys_are_stable_for_every_renamed_serialised_member` in `lib.rs` and `team_cli_wire_keys_are_stable` in `cli/team.rs`. No production file changes in this commit | criterion 8a green; **and each test proved real**: see the note below |
| C7 | `refactor(1572): rewrite the 6 renamed TypeScript modules` | contents of the **6 TypeScript** files ADDED in 6.1 (**not** `config/orchestrator_clocks.rs`: that is the seventh renamed file, it is Rust, and 6.1 assigns its content commit to C2), plus the 10 module specifiers L1 to L6 in their importers and the renamed store and badge symbols at those importers' call sites. This closes `src/sidebar/components/SessionItem.tsx` and `src/shared/shortcuts.ts` **completely**, since the renamed-store import is their only in-scope content; `ProjectPanel.tsx` is only partly closed and returns in C8 | `npm run typecheck` |
| C8 | `refactor(1572): orchestrator identifiers in the sidebar and shared frontend` | the remaining **25** files of section 6.5, including `ProjectPanel.raise-hand.test.tsx`, the two `ipc.ts` parameter renames at `:1114`/`:1134`, and all four shorthand expansions (`ipc.ts:1081`, `:1122`, `:1142`, `raise-hand.test.tsx:39`), plus allowlist **L13** in `loop-modal-helpers.ts`. **Derivation of the 25**, written out because gate 5 of 13.2 counts this commit's paths against it: section 6.5 has **28** rows; subtract `ProjectPanel.regex-filter.test.tsx` (**1**), whose only change is the new `it(...)` and which is therefore C9; subtract `SessionItem.tsx` and `shared/shortcuts.ts` (**2**), which C7 closes completely. 28 − 1 − 2 = **25**. The 6 renamed TypeScript files are **not** subtracted: they live in 6.1/6.2, not 6.5, per the section 6 header. `ProjectPanel.tsx` is touched by both C7 and C8 and is counted here | `npm run typecheck`, `npm test` |
| C9 | `test(1572): pin the sidebar orchestrator filter token (R11)` | the one new `it(...)` of section 5.8 | `npm test -- ProjectPanel.regex-filter` green, and green only with the production token present |
| C10 | `chore(1572): regenerate module-arcs.txt` | `src-tauri/module-arcs.txt` only | section 9.4, all five conditions |
| C11 | `chore(1572): final gate evidence` | nothing, or the PR body only | sections 9.3, 9.5 criteria 1 to 8, in order |

**Both new tests must be proved to be real tests, by the same materialise-and-revert technique.**

- **C9.** Run it once with the production token at `ProjectPanel.tsx:975` temporarily reverted to
  `"coordinator"` and confirm it goes red, then restore.
- **C6b.** Nine experiments, **not** three, and they are named rather than left to a "pick any"
  (decision 10.1.15). Round 2's three sampled the harness; they did not sample the one way an
  assertion can be vacuous, which is feeding a deserialise member a value equal to its own serde
  default. The set below covers every default kind present in the deserialise direction plus one
  serialise experiment per test site. For each, temporarily delete that pin's `rename = "..."`,
  **one at a time**, run criterion 8a, confirm it goes **red**, then restore:

  | # | Member (section 3.3 row) | Direction, default kind | Distinctive value the test must feed | Test site |
  | --- | --- | --- | --- | --- |
  | 1 | `AppSettings::coordinator_auto_close_skip_telegram_assigned` (21) | De, `bool` defaulting **false** | `true` | `lib.rs` |
  | 2 | `AppSettings::coordinator_cascade_close_enabled` (22) | De, `bool` defaulting **true** | `false` | `lib.rs` |
  | 3 | `AppSettings::coordinator_idle_badge_yellow_minutes` (17) | De, numeric with `default = "<fn>"` | a value `default_coord_badge_yellow_minutes` does not return | `lib.rs` |
  | 4 | `TeamConfigResult::coordinator` (6) | De, `String` defaulting `""` | a non-empty distinctive string | `lib.rs` |
  | 5 | `LoopCreateRequest::busy_coordinator` (7) | De, `Option<T>` defaulting `None` | a present variant | `lib.rs` |
  | 6 | `AgentDarkFactory::is_coordinator_of` (9) | De, `Vec<T>` defaulting empty | a non-empty array | `lib.rs` |
  | 7 | `LoopPolicy::busy_coordinator` (11) | De, enum-valued field with `#[serde(default)]` | a variant other than that type's `Default` | `lib.rs` |
  | 8 | `QuarantineRetryPath::Coordinator` (26) | Ser, unit variant | n/a | `lib.rs` |
  | 9 | `TeamListItem::coordinator` (1) | Ser, plain field | a distinctive string | `cli/team.rs` |

  A wire-key test that passes with its pin removed is worthless, and this is the only way to know it
  does not. The 18 members not sampled here are covered by **constraint 5** of section 5.9, which is
  read, not run: members that share a default kind with a sampled member fail and pass together.

In both cases the revert must be verified by `git status --porcelain` being empty before the next
commit. Materialise-and-revert is the accepted technique for proving a gate discriminates.

### 12.2 `repo-agentscommander_webpage`, 4 commits

| # | Commit | Content | Gate |
| --- | --- | --- | --- |
| W1 | `refactor(1572): rename Coordination* components (pure git mv)` | the 3 `git mv` of section 5.7 and nothing else. Does not build. | `git show --stat W1` shows 3 renames, 0 insertions, 0 deletions |
| W2 | `refactor(1572): OrchestrationDemo identifiers and import sites` | `OrchestrationDemo.tsx`, `OrchestrationProof.astro`, `README.md` | `npm run check` |
| W3 | `refactor(1572): rename the composer.coordinator i18n key` | `src/i18n/landing.ts` (6 lines: 71, 176, 278, 381, 486, 585), `src/components/alternatives/TeamComposer.astro` (2 lines: 24, 25) | `npm run check`, `npm run smoke` (the repo's script name; there is no `playwright` script), **and the grep gate below** |
| W4 | `chore(1572): gate evidence` | PR body only | the diff command of section 9.3 |

**W3's grep gate, and why the two typed gates are not enough.** Seven of the eight i18n sites are
covered by TypeScript: the five non-`en` locales are `Record<LandingMessageKey, string>` and
`copy["composer.coordinator"]` at `TeamComposer.astro:25` is a typed indexed access. The eighth,
`data-i18n="composer.coordinator"` at `TeamComposer.astro:24`, is a plain HTML attribute that
`astro check` never reads and that only the runtime `[data-i18n]` switcher consumes. No Playwright
spec touches that key (`tests/smoke.spec.ts:114` uses `composer.note`), so a forgotten `:24` passes
`npm run check` and `npm run smoke` green and silently leaves that span untranslated on every
language switch. The gate that does catch it:

```powershell
cd D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-agentscommander_webpage
git grep -c 'composer\.orchestrator' -- src   # must total 8
git grep -c 'composer\.coordinator'  -- src   # must be 0 (no output)
```

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

1. **Rule S1 coverage, and the test that proves it.** Count the `#[serde(rename = "...")]` additions
   in the diff (criterion 8b): exactly **27** new pins, spelled exactly as section 3.3 column 5.
   Then check that C6b's two tests exist and cover all 28 members, and that the implementer recorded
   the three pin-removal experiments proving they discriminate. Do **not** accept
   `coordinator_clock_settings_default_when_keys_absent` as evidence: section 3.3 shows it passes
   whether the pin is there or not.
2. **Rule S2, the three clap fields.** `cli/team.rs:49-53`, `:84-85`, `cli/workgroup.rs:48-49` must
   each carry an explicit `long = "coordinator"`. The `workgroup add` one is the dangerous one: its
   flag is hidden and its only test asserts the flag is *absent* from `--help`, so it passes whether
   the flag is `--coordinator` or `--orchestrator`. Read the diff, not the test result.
3. **Criterion 6, both halves.** 6a: exactly the 22 expected `<=` rows, no others. 6b: exactly two
   `=>` rows, `"isCoordinatorOf"` and `"busyCoordinatorPolicy"`: **their absence is a failure**,
   because each is written by one of the two least-covered pins in the phase. A run that reports
   "no `=>` rows" did not apply those pins.
4. **The 23 allowlist literals** (section 5.4), and above all the five that are forced: L11, L12, L13
   and L14 are a red test or a compile error if skipped, and L10 changes a `Debug` label that no gate
   row will ever show. Each is also a literal a reviewer would otherwise flag as out-of-scope.
5. **Concept A versus Concept B in `commands/session.rs`, `session/auto_close.rs`, `phone/mailbox.rs`,
   `session/manager.rs`, `lib.rs` and `web/commands.rs`.** These are the six mixed files. A blind pass
   over them produces a compiling tree with the wrong names. Check `coordinator_shutdown`
   (`commands/session.rs:1432`, used `:2573`) by name: it must be `arbiter_shutdown`, and
   `orchestrator_shutdown` compiles and passes every gate while naming the wrong concept.
6. **The four shorthand expansions, and the two that must not move.** `src/shared/ipc.ts:1081`,
   `:1122` and `:1142` must each read `coordinator: orchestrator,`, and
   `ProjectPanel.raise-hand.test.tsx:39` must read `isCoordinator: isOrchestrator,`. A frozen
   shorthand left alone does not typecheck; a renamed one silently changes a wire key. The two that
   stay shorthands are `App.tsx:800` (both halves frozen) and `sessions.ts:279` (both halves rename).
   Then confirm `ipc.transport.test.ts:196`, `:207`, `:220` and `:231` still assert the key
   `coordinator`, **unedited**: that is the proof the expansions kept the payload key.
7. **`close_coordinator`.** Confirm the fn name at `commands/session.rs:3406`, its `generate_handler!`
   entry at `lib.rs:3370` and the invoke string at `src/shared/ipc.ts:214` are all still
   `close_coordinator`, while the return type became `OrchestratorCloseOutcome` on both sides. Then
   confirm `TeamConfigResult.coordinator` at `src/shared/types.ts:1505` was **not** renamed: if it
   was, `EditTeamModal` loads with an empty selection and TypeScript says nothing.
8. **The `module-arcs.txt` digest.** It is predicted exactly. A different digest means the diff
   changed the arc set, which this plan asserts is impossible.
9. **C1 and W1.** `git show --stat` must show renames only, zero insertions, zero deletions. If either
   carries a content hunk, `git log --follow` is at risk and criterion 5 is the test.

---

## 15. Round-1 findings, and where each one is closed

Round 1 (`09816BAF4995FCB4851F80265528FCEBB99B264C4E7871ACB2C805F303C6EAF7`) drew
`CHANGES_REQUIRED` from `ac-dev-rust-v3`, `ac-dev-rust-grinch-v3` and `ac-dev-webpage-ui-v3`. This
table is the closure record. Every row was re-measured independently at `147ad4ef` before being
acted on; where a reviewer's number and the round-1 number differed, the disagreement is resolved by
naming the unit rather than by picking a side.

### Blockers

| # | Finding | Closed in |
| --- | --- | --- |
| B1a | The Rule P allowlist was not closed: five literals the rename forces were missing (`teams.rs:584`, `teams.rs:2328`, `instance_artifacts.rs:620`, `loop-modal-helpers.ts:23`, `teams.rs:822`) | 5.4 entries **L10-L14**, plus the five sweeps in 3.5 that close each class. All five sites reverified in the source |
| B1a' | The allowlist's unit was ambiguous ("9 entries" compared against a gate that diffs distinct literals) | 5.4 states all three units and gives the counts: **14 entries, 23 distinct literals, 22 expected `<=` rows** |
| B1b | Criterion 6b was unsatisfiable: Rule S introduces key literals that do not exist in the before-set, so "no `=>` row" always reddens | 9.3 **6b** now expects **exactly two** rows, `"isCoordinatorOf"` and `"busyCoordinatorPolicy"`, and ships the command that re-derives that number from the base set. Decision 10.1.12 records why this option and not the comparator change |
| B2 | Criterion 8 claimed coverage it did not have; the two Rust reviewers disagreed on the size of the gap (~10 vs ~18) | 3.3 now carries a **member-by-member tripwire column**. The measured answer is **6 covered, 22 not**, worse than either estimate, because `coordinator_clock_settings_default_when_keys_absent` cannot fail: it asserts the same defaults it serialised. 5.9 adds the wire-key test, 9.1 grows to two new tests, 9.2 corrects the negative-control claim, and criterion 8 becomes **8a/8b/8c** |
| B3.1 | `coordinator_shutdown` undecided; Rule A would give it the wrong concept | 5.2 explicit row **and** a general Concept B binding clause, with the `let`-binding sweep proving it is the only compound case. 6.3 and C5 name it |
| B3.2 | Three clap fields derive `--coordinator` from the identifier, so Rule A silently changes a public flag | **Rule S2**, 5.3. Rule S is now "pin every external name a derive macro spells from the identifier", with serde as S1 and clap as S2. Criterion 8c gates it. Decision 10.1.10 records why pinning beats freezing |
| B3.3 | `src/shared/ipc.ts:1081`: a shorthand where Rule K and Rule A contradict, producing a tree that does not typecheck | Rule K **shorthand clause**, 5.5, plus a register row for it. Round 3 replaces round 2's line-anchored sweep with the shorthand census of 5.5, which raises the conflict count from 2 to 4 (finding BN1 below) |
| B3.4 | `ProjectPanel.raise-hand.test.tsx` absent from every table while holding in-scope locals | added to 6.5 (**27 → 28**), four register rows. Round 2's "C8 21 → 22" was wrong and round 3 re-derives it as **25** (finding BN2 below). The property-shape argument that round 2 used here is replaced by the identifier-set measurement of 5.5; the conclusion, that this is the **only** such file among the 39 untabled ones, is unchanged and was reconfirmed |
| B3.5 | `TeamConfigResult.coordinator` (`types.ts:1505`) frozen by Rule K's text but named in neither operative list | named in the Rule K frozen enumeration, in the register, in the 6.5 `types.ts` row, and in the reviewer checklist 14.7 |

### Factual corrections

| # | Correction | Closed in |
| --- | --- | --- |
| 1 | `crates/`: 10 literal lines, not 5 hits | 3.1 correction 1, with all 10 sites |
| 2 | File counts disagreed (66/69, 55/56, 73/80) | 3.1 **states the unit of measure once**, gives both columns, and adds the set-equality proofs for 6.3 and 6.5. Re-measured: 55 Rust identifier files (= 54 rows + the renamed file) and 73 TS, which reproduce the round-1 identifier numbers exactly; the reviewers' 69/80 are the `coordinat`-substring and comment-inclusive units |
| 3 | "379 lines match `coordinat`" is case-insensitive only | 4 out-of-scope row: **379 ci, 337 cs**, and the gate command is ci |
| 4 | Web repo `coordinat` line count | 3.7: **26 cs / 32 ci** (round 1 said 31, which is neither) |
| 5 | `src/sidebar/App.tsx:801` was one line off | register row now reads **`:800`**, with the use at `:801` noted |
| 6 | Rule K's closing sentence was false for `coord-quick-access-css.test.ts` | 5.5: the file **does** contain `coordinator` at `:4` and `:63`; it stays untouched by residual 10.2.4, not by the reason given |
| 7 | The sixth locale is German, not "en-alt" | 3.7 locale row, with all six block start lines |
| 8 | The i18n type-safety net covers 7 of 8 sites, not 8 | 3.7 has a dedicated row; 12.2 adds the `git grep -c` gate the reviewer proposed |
| 9 | Two `docs/` lines name the renamed path and had no owner | **in-scope item 9** and table **6.6**, with `session-auto-close.md:151` explicitly excluded and decision 10.1.13 recording why |
| 10 | `loop_cmd`'s `"busy-coordinator"` value does not exist | removed; the values are `wait-until-idle`, `force-inject`, `skip` |
| 11 | The web repo's script is `smoke`, not `npx playwright test` | 12.2 W3 gate and 3.7 CI row |
| 12 | Criterion 3 asked for "same module count" when 6 node names change | 9.5 criterion 3 compares the **triple 351 / 0 / 1535** |

### Round-2 findings, and where each one is closed

Round 2 (`A835750BC4EB5F364CB6BA7F0E6C0938A12536E2A0F99791B35206A14EBBFE9D`) drew `PLAN_APPROVED` from
`ac-dev-rust-v3` and `ac-dev-rust-grinch-v3`, and `CHANGES_REQUIRED` from `ac-dev-webpage-ui-v3`.
Every finding below was re-measured at `147ad4ef` before being acted on. Exactly two rows of
section 6.5 changed: the `ipc.ts` row, which is finding BN1, and the `App.tsx` row, whose frozen
binding reference goes from `:801` to `:800`, which is finding C-C. No pin, no digest, no gate
command and no other 6.5 row changed.

| # | Finding | Closed in |
| --- | --- | --- |
| BN1 | Rule K opens with "if and only if" over three categories, and the `ipc.ts:1114`/`:1134` parameters are in none of them, so the rule renames them while 6.5 and the 5.5 shorthand count froze them: a rule-versus-table contradiction that left the implementer choosing | **Decided: the parameters rename.** Register rows for `:1114`/`:1134` and `:1122`/`:1142` in 5.5, the rewritten shorthand census (4 conflicts among 6 real shorthands, not 2 among 8), the 3.4 note, the 6.5 `ipc.ts` row, C8 in 12.1, checklist item 14.6, and decision **10.1.14**. Rule K's text is untouched |
| BN2 | C8's "22 remaining files" is derivable from no reading of the plan: 6.5 has 28 rows, and only 3 are excludable. The likely origin, subtracting the 6 renamed TypeScript files, subtracts from the wrong section | 12.1 C8 reads **25**, with the subtraction written beside it, and 12.1 C7 is retitled to the **6** renamed TypeScript modules, the seventh being Rust with its content commit at C2 per 6.1. Section 15's B3.4 row is corrected |
| O-A | 5.9's worked example fails on the first run: "a minimal object is enough" is false for 7 of the 12 owning types, and `AppSettings` rejects the example JSON with `missing field `defaultShell`` | 5.9 replaces the premise with a measured required-field table for all 12 types and rebuilds the example on the issue-#248 recipe at `settings.rs:8255` |
| C-A | 5.5's "exactly four exceptions" stopped being arithmetically possible once `raise-hand.test.tsx` joined the 34, and the inference "property-shaped, therefore a frozen wire key" is unsound: this plan renames three property-shaped sites | 3.1 and 5.5 close the 39 on the **identifier set** instead: 130 occurrences, 10 distinct identifiers, all 10 in Rule K's frozen enumeration, zero in a declaration context, re-measured here. The conclusion and every 6.5 row are unchanged |
| C-B | The shorthand sweep anchored to a whole line, so it missed the inline `sessions.ts:279`, and on a CRLF worktree its `$` anchor matches nothing at all | 5.5 replaces it with a line-independent census over the blanked tree: 37 shorthand-shaped occurrences, 31 specifier entries, 6 real shorthands, each with a verdict |
| C-C | `App.tsx` reads `:800` in 5.5 and in section 15 but still `:801` in 6.5 | the 6.5 `App.tsx` row now reads `:800`, with `:801` named as the use |
| O-B | 5.3 says "exactly two such macros"; `#[tauri::command]` is a third that also spells external names from identifiers | 5.3 names all three and routes the third to 3.4, which enumerates its complete surface and freezes every site of it that reaches the wire. No gap, only the sentence |
| O-C | "Seven literals at six line numbers", followed by seven line numbers | 3.5 class-D item 1 reads **seven line numbers** |
| O-D | The two `#[cfg(test)]` citations name a `#[cfg(test)] fn` (`lib.rs:76`) and a helper child module (`cli/team.rs:403`) rather than the `mod tests` the test goes in | 5.9's site table cites `lib.rs:3899-3900` and `cli/team.rs:579-580` |
| O-E | The row-16 tripwire cannot detect a dropped pin: after the migration the field is `None` and is elided, so the negative assertion passes either way | 3.3 row 16 and the 9.2 issue-#248 row both say so |
| O-F | C6b's three pin-removal experiments leave one residual path to a vacuous test: a deserialise assertion fed a value equal to the member's own default | **Decided: expand, and close the general case by constraint.** 5.9 gains binding **constraint 5**; 12.1 C6b names **9** experiments, one per default kind plus one serialise experiment per test site; decision **10.1.15** records why sampling by default kind beats one run per pin |
| Base | The plan's coordinates needed to be pinned to a named base while `origin/main` moved to `047248bc` | 1.1 names `147ad4ef` as the coordinate base and records the drift with its classification (zero added `coordinat` lines, zero changed paths under `src/`, `module-arcs.txt` byte-identical). 13.5 stays binding and 1.2 re-runs the classification before the first mutation |

### Round-3 findings, and where each one is closed

Round 3 (`08A46CDCAC4279F9BE08B8D98AE7A0FEBDC7325C7E36CB21F2243A2D31D2962A`) drew `PLAN_APPROVED`
from `ac-dev-webpage-ui-v3` and `ac-dev-rust-grinch-v3`, and `CHANGES_REQUIRED` from
`ac-dev-rust-v3`. All three reviewers audited the round-3 delta hunk by hunk and found nothing moved
outside the frozen list. Round 4 changes prose only: no pin, no digest, no gate command, no row of
section 6.5, no rule and no criterion changed.

| # | Finding | Closed in |
| --- | --- | --- |
| DN1 | 5.9's required-field table over-states 5 of its 12 rows. Its premise, "serde requires every field carrying neither `#[serde(default)]` nor `#[serde(skip)]`", is false for `Option<T>`: an attribute-less `Option<T>` deserialises to `None` when its key is absent. Raised independently by both Rust reviewers, who reached the same five rows and the same corrections | 5.9's premise now carries the `Option<T>` exemption explicitly, and rows 7, 8, 12, 27 and 28 drop their `Option` fields, each with the reason spelled in the cell. Rows 6, 9, 11, 14, 15-22 and the enum row are unchanged, and the count "7 of the 12" is unaffected because the five corrected types all keep at least one required field. Re-verified twice: by reading all twelve struct definitions field by field at `147ad4ef`, and by running the four corrected shapes plus a `missing field` negative control each against serde 1.0.228 / serde_json 1.0.149, the versions `Cargo.lock` pins |
| DN2 | 5.5's "the nine non-test files among the 39" is arithmetically false: seven of the nine carry `coordinator` only in a comment or a literal, so the blanking pass that defines the census erases them and they are in neither the 73 nor the 39. Inherited round-2 prose, not a round-3 regression | 5.5 now states that **exactly two** of the 39 are non-test files, names them, and lists the seven comment-and-literal-only files separately with what each one carries. Re-measured: 73 files with a code token, 34 in section 6's tables, 39 remaining with 130 occurrences, of which 2 are non-test |
| DN3 | Section 15's round-2 header says no row of section 6.5 changed, contradicting its own C-C row, which changed the `App.tsx` reference from `:801` to `:800` | the header now names **both rows that changed**, `ipc.ts` (BN1) and `App.tsx` (C-C), and keeps the rest of the claim |
| DN4 | **Not a reviewer finding; caught while making the three above.** Decision 10.1.5 still called the Rule P allowlist "the nine-entry allowlist", the round-1 size. Finding B1a grew it to 14 in round 2 and 5.4 has said 14 ever since, so the decision contradicted the number it depends on | decision 10.1.5 reads **the 14-entry allowlist of 5.4**. The allowlist itself, its 14 entries, its 23 distinct literals and its 22 expected `<=` rows are untouched |

### Tech lead's ruling, recorded

**Criterion 7** is certified as "no identifier is shared between the two concepts", with the three
`selectionCoordinator*` strings as the issue's own declared exception. `ac-tech-lead-v3` accepted
this reformulation explicitly in the round-2 brief of 2026-08-27 and will surface it to the user at
Step 7.5. It is recorded as an accepted residual with owner **#1573** in 10.2.1, not as an open
question.

### Re-verified and deliberately unchanged

The reviewers independently confirmed, and this round did not touch: the exhaustiveness of the 28
serde members and the 27-pin count; the levelization prediction (1037 lines, 82163 bytes, digest
`2EF5875A...`, reproduced by two reviewers); `litset.mjs` and its `files=550 distinct=28345`
(reproduced again here); the absence of a fourth source scan; the IPC boundary; the Concept A/B
classifications apart from `coordinator_shutdown`; the 10 TypeScript module specifiers; the web repo
inventory; the feasibility of the 3c assertion; the commit order and the accepted non-building C1/W1;
and the CI inventory.
