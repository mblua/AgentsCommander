# Issue #1027: Auto-close terminal hijack full-path plan

Status: READY_FOR_IMPLEMENTATION

Issue: `mblua/AgentsCommander#1027`

Related invariant: `mblua/AgentsCommander#889`

Repository: `repo-AgentsCommander`

Branch: `fix/1027-auto-close-terminal-hijack`

Verified base: `main`, `origin/main`, and branch HEAD all at `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe` on 2026-07-16.

## 1. Objective

Fix the backend selection defect and every consumer race that lets destruction of one session replace the central terminal with an unrelated exited, detached, or PTY-less record. The completed change must provide one canonical, cause-aware selection transition path across native Tauri and browser/WebSocket clients; make restart and multi-member destruction selection-atomic; make persistence reads coherent with selection commits; and make SolidJS consumers process-epoch and revision-aware.

This issue does not redefine inactivity. A visible terminal that shows `1265m` and receives no terminal input or genuine post-repaint output remains eligible for auto-close under the current product rules. Passive viewing and focus remain non-activity. The defect is the invalid selection and terminal behavior after an eligible close.

## 2. Verified evidence and cause

### 2.1 Incident evidence

- On 2026-07-15, Root session `ea134552-d0c1-4528-bbd9-67fc47ea18a4` had already exited and had no retained PTY screen snapshot.
- Auto-close later destroyed selected WG-14 session `4835810b-ad00-4804-92b5-c279e17e976b`.
- `SessionManager::destroy_session` removed WG-14, chose `order.first()`, made the exited Root record canonical, and directly wrote `SessionStatus::Active` without checking status, PTY presence, or detached state.
- The command layer emitted `session_switched` for Root. The terminal followed that ID, requested a screen snapshot that could not exist, and displayed the unrecoverable resize/repaint message. The sidebar correctly retained Root's exited status, producing the visible split.
- A later explicit user switch logged Root being demoted from backend `Active`, proving this was a canonical backend selection change rather than stale terminal text.
- The July 16 user comment confirms the affected terminal was visible at `1265m`, the user was looking but not typing, and auto-close then led to the hijacked/black screens. It does not establish a timer or focus-activity defect.

### 2.2 Current code path

- `src-tauri/src/session/manager.rs::destroy_session` couples record removal to an unfiltered `order.first()` fallback and directly writes `Active`.
- `SessionManager::switch_session` independently performs the same unconditional `Active` write. This is the writer invariant in #889.
- `src-tauri/src/commands/session.rs::destroy_session_inner_with_options` uses the manager result, filters only runtime-detached IDs in one branch, and emits no authoritative null selection when no replacement exists.
- The retained-Root branch filters exited and detached rows but still does not prove PTY presence.
- Auto-close calls the generic single-session destroy path once per member. A selected member can therefore be replaced by a sibling that is about to be destroyed, producing intermediate selection churn.
- Restart destroys the old record, exposes any destroy fallback, creates the replacement, and only then selects it. The fallback is observable even with perfectly ordered events.
- `src-tauri/src/web/commands.rs` independently re-derives active selection and broadcasts a second set of events after calling the shared destroy path. Tauri and browser clients do not consume one publication.
- `src/terminal/App.tsx` has independent asynchronous `getActive()`/`list()` work from initial load, destroy, and switch listeners. It has no revision guard, does not clear when a selected ID is absent from `list()`, and will bind an exited or PTY-less record.
- `src/terminal/components/TerminalView.tsx` correctly reports a missing snapshot for a mounted terminal, but a dormant record should never have been mounted. Resize cannot recreate a missing PTY. The snapshot implementation exposed the bad selection; it did not cause it.
- `src/sidebar/stores/sessions.ts::setActiveId` deliberately preserves an exited status. That safeguard explains the sidebar/terminal disagreement and must remain.

### 2.3 Inactivity finding

- The idle badge and auto-close use the same persisted anchor, `max(last_user_message_at, last_activity_at)`.
- Reopening uses `seed_if_absent` and deliberately keeps an existing old anchor. The existing frontend regression test pins that behavior.
- PTY input, genuine PTY output, Telegram/web user input, and successful inter-agent delivery are activity sources. `terminal.focus()` is not.
- `REPAINT_GRACE = 10s`, `WAKE_GRACE = 30s`, and the 60-second tick work as currently documented in code and tests.
- Therefore `1265m` followed by close after wake grace is expected current eligibility. No inactivity code changes are authorized by this plan.

## 3. Scope

### 3.1 In scope

- Policy-free manager removal.
- A single canonical selection coordinator and a single internal manager commit path.
- Live versus explicitly dormant selection semantics.
- PTY and runtime-detached eligibility.
- Typed destruction and selection sources.
- A process epoch plus process-monotonic selection revision, structured transition logging, and reconnect-safe hydration.
- Cause-specific policies for session creation, auto-close, manual close, restart, restore, detach, attach, spawn rollback, resource-monitor kill, runtime liveness loss, explicit user switch, background cleanup, and multi-member batches.
- Selection-atomic restart and batch destruction.
- One authoritative Tauri/WebSocket event payload and one hydration payload from the same revision state.
- Runtime validation of the untrusted IPC/WebSocket payload at one shared TypeScript boundary.
- Epoch/revision-aware terminal and sidebar reconciliation.
- Aggregate manager snapshots so persistence cannot combine records and selection from different commits.
- Neutral no-selection UI and dormant wake guidance with all PTY operations gated off.
- #889's unconditional `Active` writer invariant.
- Contract documentation and the inaccurate auto-close troubleshooting sentence discovered during verification.

### 3.2 Out of scope

- Counting passive focus, window visibility, mouse movement, or viewing as activity.
- Resetting the persisted idle anchor on reopen.
- Changing the 10-second repaint grace, 30-second wake grace, 60-second tick, timeout comparison, Telegram protection, or team membership rules.
- Retrying or redesigning native screen snapshots, xterm caching, or PTY resize recovery.
- Adding a separate high-retention selection log sink or splitting PTY TRACE retention. The narrow structured log lines in this change are sufficient for #1027; retention is a follow-up.
- Preferring same-project or same-workgroup fallbacks. Stable manager order preserves current explicit-close UX without adding a new ranking product feature.
- A general rewrite of `SessionStatus` or persistence field names such as `was_active`.
- New crates, a feature/domain/ports/adapters frontend reorganization, or unrelated refactors.

## 4. Decided solution

### 4.1 Canonical selection model

Rename the manager concept from an unqualified active ID to canonical selection state. Selection has three modes:

1. `None`: no central session is selected.
2. `Live`: the selected record exists, is not `Exited`, has a live PTY route and backend handle, and is not present in runtime `DetachedSessionsState`. Only this mode is displayable and only this mode may carry `SessionStatus::Active`.
3. `Dormant`: the selected record exists with `SessionStatus::Exited(code)`, is not runtime-detached, and remains exited. It is a selection for sidebar continuity and wake guidance, not a live terminal.

An automatic fallback target may only become `Live`; it must never choose some other `Dormant` record. A dormant target is allowed only when the exact target was selected explicitly by the user, when an exited detached session is explicitly attached back to main, when restore reinstates the exact persisted selection, or when the exact currently selected live record becomes exited through `livenessReconcile`. A non-exited record with no PTY is inconsistent, not dormant; selection rejects it and leaves the previous canonical selection unchanged unless the record being torn down was already selected, in which case the safe result is `None`.

Runtime detached state is `DetachedSessionsState`, because it represents an actual detached window. `Session::was_detached` remains persistence intent and is not a displayability gate: restore intentionally leaves it true when a detached-window rebuild fails so the next launch retries. Do not add `hasPty` to every `SessionInfo` or treat the serialized `wasDetached` field as live runtime truth. The authoritative selection payload below carries the runtime snapshot consumers need.

### 4.2 Exact selection payload

Retain the event name `session_switched` for compatibility, but replace every ad hoc payload with this single Rust/TypeScript contract:

```ts
export type SessionSelectionMode = "none" | "live" | "dormant";

export type SessionSelectionCause =
  | { source: "initialHydration"; userInitiated: false; mode: "none" }
  | { source: "sessionCreated"; userInitiated: boolean; mode: "live" }
  | { source: "userSwitch"; userInitiated: true; mode: "live" | "dormant" }
  | { source: "manualClose"; userInitiated: true; mode: "live" | "none" }
  | { source: "autoClose"; userInitiated: false; mode: "none" }
  | { source: "restart"; userInitiated: boolean; mode: "live" | "none" }
  | { source: "restore"; userInitiated: false; mode: "live" | "dormant" | "none" }
  | { source: "detach"; userInitiated: true; mode: "live" | "none" }
  | { source: "attach"; userInitiated: true; mode: "live" | "dormant" }
  | { source: "spawnRollback"; userInitiated: false; mode: "none" }
  | { source: "resourceMonitor"; userInitiated: boolean; mode: "none" }
  | { source: "backgroundCleanup"; userInitiated: false; mode: "none" }
  | { source: "livenessReconcile"; userInitiated: false; mode: "dormant" | "none" };

export type SessionSelectionSource = SessionSelectionCause["source"];

interface SessionSelectionOrder {
  epoch: string;
  revision: number;
}

type SessionSelectionBase = SessionSelectionOrder & SessionSelectionCause;

export type SessionSelection =
  | (SessionSelectionBase & {
      mode: "none";
      id: null;
      status: null;
      hasPty: false;
      detached: false;
      displayable: false;
    })
  | (SessionSelectionBase & {
      mode: "live";
      id: string;
      status: "active";
      hasPty: true;
      detached: false;
      displayable: true;
    })
  | (SessionSelectionBase & {
      mode: "dormant";
      id: string;
      status: { exited: number };
      hasPty: boolean;
      detached: false;
      displayable: false;
    });
```

Rust uses matching `Serialize` types with `#[serde(rename_all = "camelCase")]`, a process-generated UUID serialized as the `epoch` string, and `u64` for `revision`; TypeScript uses `number`. Rust represents mode data as private enum variants and exposes only invariant-preserving constructors used by the manager commit. A process cannot approach JavaScript's exact-integer limit through UI selection transitions. Increment uses `checked_add`; overflow rejects the transition, logs an error, and preserves the prior selection rather than wrapping. The TypeScript union deliberately makes invalid mode/status/liveness combinations unrepresentable after decoding.

Payload invariants are exact:

| Mode | `id` | `status` | `hasPty` | `detached` | `displayable` |
|---|---|---|---|---|---|
| `none` | `null` | `null` | literal `false` | literal `false` | literal `false` |
| `live` | record ID | literal `"active"` | literal `true` | literal `false` | literal `true` |
| `dormant` | record ID | `{ exited: code }` | actual snapshot, normally `false` | literal `false` | literal `false` |

Every payload also carries a nonempty UUID `epoch`. One epoch is generated inside `SessionManagerState::new` with its initial revision-0 `None` payload and remains stable for the lifetime of that managed backend state. The coordinator reads it from manager state; no caller supplies or mutates it. A new backend process constructs new managed state and therefore a new epoch, so a surviving browser client can distinguish a reset revision domain from a stale event.

Source semantics are part of the decoded contract rather than comments. The decoder and Rust constructors enforce these allowed combinations:

| Source | Allowed modes | Allowed `userInitiated` |
|---|---|---|
| `initialHydration` | `none` at revision 0 only | literal `false` |
| `sessionCreated` | `live` | trusted create intent |
| `userSwitch` | `live` or `dormant` | literal `true` |
| `manualClose` | `live` or `none` | literal `true` |
| `autoClose` | `none` | literal `false` |
| `restart` | `live` or `none` | trusted restart intent |
| `restore` | `live`, `dormant`, or `none` | literal `false` |
| `detach` | `live` or `none` | literal `true` |
| `attach` | `live` or `dormant` | literal `true` |
| `spawnRollback` | `none` | literal `false` |
| `resourceMonitor` | `none` | derived only from `User`/watchdog reason; AppShutdown emits no selection |
| `backgroundCleanup` | `none` | literal `false` |
| `livenessReconcile` | `dormant` or `none` | literal `false` |

The TypeScript base is therefore a source/flag/**allowed-mode** union intersected with the invariant-bearing mode-data union, rather than a free source, boolean, or cross-product. An impossible source/mode/flag tuple reduces to `never` for trusted TypeScript construction. The runtime decoder still constructs a fresh normalized object and rejects that tuple in untrusted JSON, plus non-plain nested status data, inherited accessors, and missing or extra contract keys. It never returns the original untrusted object by assertion.

The initial process state is:

```json
{
  "epoch": "<process-uuid>",
  "id": null,
  "source": "initialHydration",
  "userInitiated": false,
  "revision": 0,
  "mode": "none",
  "status": null,
  "hasPty": false,
  "detached": false,
  "displayable": false
}
```

Every material selection change increments the process-local revision exactly once for the committed and published final state. A no-op request does not increment or emit. Candidate validation and retry happen before commit, so rejected or provisional candidates consume no revision and no unpublished manager state.

The existing backend command name `get_active_session` remains to avoid needless protocol-name churn, but its return type becomes `SessionSelection`. The frontend wrapper is renamed to `SessionAPI.getSelection()` so bundled code does not mistake the response for an ID. It invokes as `unknown` and passes the result through the single shared `decodeSessionSelection` boundary before returning. Malformed/failed hydration is logged and leaves a safe neutral binding only when the request's captured connection/binding generation is still current; it cannot clear a newer valid event that arrived while the request was pending. `onSessionSwitched` likewise decodes an `unknown` event payload before any store mutation; malformed events are logged and dropped without invoking consumer callbacks. Tauri invoke and WebSocket dispatch serialize the same stored payload; neither transport owns an epoch or counter.

### 4.3 Single authoritative transition API

Add `src-tauri/src/session/selection.rs` because selection policy, source types, revision state, runtime eligibility, and transaction gating are one cohesive cross-command concept. Keeping this in the already large command module would preserve the current duplication. No crate is added.

The module owns:

- `SelectionSource`, `SelectionMode`, `SessionSelection`, `RuntimeSelectionSnapshot`, and `SelectionRequest`.
- A sealed internal `SelectionCause` enum that is the only input from which published `source` and `userInitiated` are derived. Fixed causes do not accept a separate boolean: `UserSwitch`, `ManualClose`, `Detach`, and `Attach` derive `true`; `AutoClose`, `Restore`, `SpawnRollback`, `BackgroundCleanup`, and `LivenessReconcile` derive `false`; create/restart variants carry trusted Rust intent, while resource-monitor accepts only trusted `User` or `Watchdog` (never `AppShutdown`) and derives the flag. `commit_selection_transition` never accepts an independently supplied source/flag pair.
- `SelectionCoordinator`, managed once by Tauri, containing the sender for one bounded 64-entry Tokio MPSC queue, an exact 65-permit general admission semaphore covering the one running job plus queued/reserved work, and an exact 16-permit create-ticket semaphore.
- A private `CoordinatorJob` enum is the only MPSC item. Each variant carries plain owned request data and its typed oneshot sender. No queued variant may contain or capture an `AppHandle`, `SelectionCoordinator`, `SessionManager`, `PtyManager`, container backend, or other managed-state handle. The worker owns one `WorkerContext` containing the publication handle and resolves managed state at execution time. This is a compile-reviewable ownership rule that prevents `channel -> queued closure -> AppHandle/state -> coordinator sender -> channel` and `channel -> queued closure -> PtyManager -> container sender -> channel` retain cycles; arbitrary caller-supplied boxed futures are not part of this design.
- One FIFO coordinator worker spawned at setup. It executes exactly one `CoordinatorJob` at a time, including async restart and multi-member teardown work, without holding a lock guard. The job's owned general-admission permit is released only after its typed result is sent or its receiver is known dropped.
- `SelectionTransaction`, an internal worker-owned context used by restart, multi-member destruction, and other lifecycle jobs. It is a capability object, not a lock guard.
- Pure candidate classification and rejection reasons.
- Private variant-specific `submit_*` methods construct `CoordinatorJob` values. External Tauri/WebSocket commands and hydration first use `try_acquire_owned` on the general semaphore and then nonblocking queue admission; capacity pressure returns a typed busy/unavailable error rather than creating futures waiting on `Sender::send`. Destructive user commands, including manual destroy/restart, detach/attach, and user resource kill, gain admission before their first side effect. Because the one running job retains one permit, the remaining 64 permits correspond exactly to the queue's maximum queued or physically reserved work. A startup race in which no job has started may make the sixty-fifth non-running submit see the full physical queue; it releases its permit and returns `Busy`, preserving the same upper bound.
- Critical internal producers use one deduplicated fair waiter per `CriticalAdmissionKey { session_id, kind }`, where kind is exactly `RouteLoss`, `WatchdogKill`, or `BackgroundCleanup`. A key is registered only after an aggregate manager snapshot proves the ID is public or pending. Heterogeneous operations for one ID never coalesce because their final policies differ. Watchdog/background cleanup wait before side effects, while unsolicited route/process loss necessarily waits after the external loss but before manager reconciliation. A duplicate returns `CriticalAdmissionOutcome::AlreadyPending` immediately and allocates no task. The first waiter awaits the fair general semaphore and then one queue slot with shutdown cancellation; while it owns the logical permit, ordinary `try_acquire` submissions cannot steal the slot it is waiting to reserve. The key is removed on missing-ID rejection, admission failure, job completion, and shutdown. Thus at most one waiter exists for each of three kinds per extant public/pending ID, with no tombstone entry.
- `SelectionCoordinator::reserve_create` acquires, in this exact fail-fast order, one of 16 create permits, one of 65 general permits, and one actual MPSC slot with `try_reserve_owned`; failure at any later acquisition releases every earlier permit. The returned opaque `CreateFinalizationTicket` stores the current manager epoch/revision and an **optional** auto-select precondition: `Some(epoch, revision)` only when that snapshot is mode `None`, otherwise `None`. A create is allowed while another session is selected, but it can never auto-select from that state. The ticket is bound to its pending ID by the same no-await insertion operation, and every pending-record setter requires that matching live ticket capability. Pre-insertion failure calls `OwnedPermit::release` and queues nothing. After insertion, ordinary success or failure consumes the owned slot on exactly one typed success/rollback finalizer. Unfinished-ticket `Drop` uses `OwnedPermit::send` (never `send().await` or racy `try_send`) for the same idempotent rollback. Tickets count against the 65 admitted-work budget and physically reduce the 64-slot queue; the 16-ticket cap leaves at least 48 slots that slow spawns cannot park. The seventeenth concurrent create returns `Busy` before any create side effect.
- Every non-suppressed create races its complete post-ticket body against the coordinator shutdown token. Cancellation of the caller, panic unwind, or shutdown drops that body, which drops the PTY/backend cancellation guard and ticket and therefore enqueues rollback through the reserved slot. Ticket/pending mutation APIs reject work after shutdown closure. Receiver shutdown uses `Receiver::close`, so Tokio 1.51 `OwnedPermit::send` finalizers are still accepted and drained. A create that has not inserted a pending row simply releases its permits. These rules, plus backend cancellation guards, prevent a ticketed create from mutating manager selection/lifecycle state or leaving a process after final shutdown persistence.
- `SelectionCoordinator::transition(request)`, the only public production entry point for a simple canonical selection change; it submits the typed transition variant without accepting an `AppHandle` or caller future.
- `SelectionCoordinator::snapshot()`, the hydration read, submitted as its typed variant so it is ordered after prior transitions.
- An internal `SelectionTransaction::transition` used only inside a worker job. A job must never call a public coordinator submit method recursively, which would wait behind itself; restart/create suppression and batch helpers receive the transaction context explicitly.
- `SelectionCoordinatorError::{Busy, Unavailable, RecursiveSubmission}` with exact strings `selectionCoordinatorBusy`, `selectionCoordinatorUnavailable`, and `selectionCoordinatorRecursiveSubmission`. Native commands and the WebSocket command envelope propagate strings unchanged. TypeScript narrows `catch (error: unknown)` only by exact equality; only `Busy` is retryable. `RecursiveSubmission` is an invariant failure, not overload. A task-local worker marker is checked by **every** external and critical enqueue method, including the container's narrow sender, so an accidental re-entry returns it immediately rather than waiting behind itself. Helpers that legitimately run inside a job receive `SelectionTransaction` and do not enqueue.
- A worker lifecycle guard is tied to the existing shutdown signal. Shutdown closes both semaphores and new queue admission first, wakes/removes critical waiters, cancels ticketed create bodies, returns `Unavailable` for ordinary queued jobs that have not started, and lets the current transaction reach its one consistency finalizer once destructive external work has begun. The receiver remains open for already-owned create permits and executes their rollback finalizers. After triggering producers, `RunEvent::Exit` closes/drains and joins the coordinator **before** the existing global PTY/job/resource kill sequence and before the final aggregate persistence snapshot. Coordinator drain has its own exact five-second cap using `SHUTDOWN_CLEANUP_BUDGET_SECS`; this adds at most five seconds to today's worst-case exit, while later existing caps remain unchanged. On cap expiry, abort and await the worker `JoinHandle`, drop queued oneshots so callers resolve unavailable, log the current source/session/phase and outstanding ticket/key counts, then continue global cleanup and persist the last complete manager snapshot. A commit cannot be aborted while its no-await manager critical section is executing. No coordinator or ticket capability can mutate selection/lifecycle manager state after the worker is joined/aborted; cancellation-safe local and container spawn guards clean any blocking spawn that completes later. The worker owns the sole publication `AppHandle` clone; joining drops it. Container callbacks hold only narrow sender/dedup handles, and queued messages contain no managed handles, so shutdown has no retain cycle or orphaned coordinator task.

`SessionManager` consolidates records, stable order, pending-create IDs, next session number, selected ID, process epoch, last payload, and revision into one `Arc<RwLock<SessionManagerState>>`. This replaces the current independent `sessions`, `order`, `active_session`, and `next_number` locks, whose separately awaited reads cannot form an atomic cross-field snapshot. Pending is manager lifecycle metadata, not a new serialized `SessionStatus`: normal `get_session`/list/persistence/candidate snapshots omit pending records. Only `get_pending_session(id, &CreateFinalizationTicket)`, ticket-bound pending setters, and transaction-scoped restore/restart accessors can address one, so an old caller cannot accidentally bypass the exclusion. The manager exposes one restricted mutation primitive, `commit_selection_transition`, callable only with an unforgeable commit capability constructed by `SelectionTransaction`. That primitive receives an already-decided target (`Keep`, `Clear`, `Live(id, runtime witness)`, or `Dormant(id, runtime witness)`), the sealed cause, and an optional typed lifecycle mutation set containing record removals, `FinalizeCreate(id)`, `MarkExited(id, code)`, and/or `SetDetachedIntent(id, value)`. It does not rank or discover a target, and no caller can replace the manager-owned epoch. Live/dormant runtime witnesses also have private constructors, so another crate module cannot manufacture `has_pty=true` or `detached=false` and bypass the coordinator.

`commit_selection_transition` performs record removals, order removals, exited-record updates including raise-hand clearing, old-live demotion, target validation, target status mutation, selected-ID mutation, payload construction, and revision increment under one `SessionManagerState` write guard, with no await while the guard is held. It returns removed records, changed row snapshots, and cleared-raise-hand IDs needed by callers. It validates these invariants even though the coordinator already checked them:

- `Live` target exists, is not exited, has a `has_pty=true` witness, and has `detached=false`; then and only then it writes `SessionStatus::Active`.
- `Dormant` target exists and is exited; its status is preserved byte-for-byte.
- A `Keep` decision is invalid when the removal set contains the selected ID.
- A `Keep` decision is invalid when `MarkExited` changes the selected live record; the same atomic commit must select that exact dormant record or clear it according to source policy.
- Lifecycle mutation sets reject duplicate IDs, conflicting exit codes, an ID present in both remove and `MarkExited`, a selected target that is also removed, and removal/exit of a pending ID without its owning rollback/finalization capability. `MarkExited` is idempotent: once a public record is `Exited(first_code)`, a duplicate or late exit callback preserves `first_code`, produces no row update, and consumes no selection revision.
- `FinalizeCreate(id, final_state)` requires an existing pending record and a private runtime witness. `Live` finalization requires a non-exited record plus live PTY and records the actual attached/detached snapshot; `Dormant(code)` finalization is restore-only, writes/preserves that exited code, and requires no live PTY. It removes the pending marker in the same commit that may select an attached finalized target and returns the now-public row for persistence/event publication. It conflicts with remove or a separate `MarkExited` for the same ID; restore represents a pending exited record through the dormant finalization variant instead of two mutations. No target other than the matching finalizer may select a pending ID. Rollback removes both record and pending marker atomically.
- The previous selected record is changed from `Active` to `Running` only when it still exists, differs from the new ID, and was actually `Active`.
- No removal path chooses `order.first()` or writes another record's status on its own.

Every other manager method acquires at most one state read or write guard and performs its direct map/order/number mutation before releasing it. It must not call another async manager method while holding that guard. This preserves short critical sections and prevents self-deadlock after state consolidation.

Remove standalone production exit marking entirely. Restore records use restore-only dormant `FinalizeCreate`; every already-public exit uses the coordinator's atomic `MarkExited` lifecycle mutation. A `#[cfg(test)]` manager-only fixture may set up exited rows, but no production visibility remains. The commit capability and source-ownership test make this enforceable rather than relying on comments.

Remove production uses of `SessionManager::switch_session`, `set_active_only`, `clear_active`, `clear_active_if`, and standalone `get_active()`. Their semantics are subsumed by the coordinator and manager commit primitive. Add aggregate manager snapshot methods that copy the required records, order, and selection from one `SessionManagerState` read guard and release it before returning. `sessions_persistence::snapshot_sessions` consumes that aggregate snapshot rather than combining `list_sessions()` with a later active-ID read. No standalone selected-ID projection remains in production; exact-ID checks use the selection field already present in the aggregate snapshot or the coordinator payload.

### 4.4 Manager removal is policy-free

Replace `SessionManager::destroy_session -> Option<Uuid>` with lifecycle mutation data that reports exactly what happened, never a replacement choice. In production, removal and finalized exit marking are supplied to `commit_selection_transition` as one typed mutation set so persistence readers cannot observe a removed selected record with an interim fallback or an exited selected record with a stale live payload.

The removal outcome contains:

- removed IDs and removed records;
- missing IDs;
- whether the selected ID was among the removed IDs;
- the pre-transition selection snapshot;
- the optional final selection commit.

The outcome is built while the same manager guard is held; it is not assembled from separate async reads. Persistence cannot observe a removed selected record, a new selected ID with an old record status, or an incorrect `was_active` bit.

The manager never scans stable order to decide policy. `order` is only used by `SelectionCoordinator` to build a stable candidate list before it passes an explicit decision back. Tests use one exact `#[cfg(test)] finalize_pending_for_test(id, TestFinalState)` fixture plus the real commit primitive with a test-only capability; there is no second removal/selection policy seam. The fixture may expose an explicitly requested live/dormant row but cannot rank another ID, publish, or exist in a production build.

`SessionManager::create_session` also stops auto-selecting its first inserted record. It atomically inserts a `Running` pre-spawn record plus its pending-create marker and binds that ID to the owning ticket/transaction. Add trusted Rust `CreateSelectionIntent::{User, Background, Suppress}` to `create_session_inner` and the shared Root helper because the same code serves UI, startup, loops/mailbox, restore, and restart.

For a non-suppressed top-level non-Root create, argument normalization and provably read-only validation may run first; `reserve_create` must then succeed **before** `mark_spawning`, archive unarchive, coordinator-clock mutation/event, resource-slot reservation, config/credential write, manager insertion, or PTY/container side effect. This is the first create-side-effect gate. The optional auto-select precondition from the ticket is `Some` only for a starting `None`; a create begun under any non-null selection still runs and publishes its row but can never select itself. Every metadata setter between insertion and finalization takes the matching ticket, and public projections, persistence, automatic candidates, explicit switch, detach/attach, and screenshot omit the pending row even after its PTY appears.

After successful spawn and all best-effort post-spawn config work, the ticket spends its owned queue permit on one typed finalizer. That job revalidates record, backend kind, route, PTY handle, and runtime detached state, then applies `FinalizeCreate`. It selects the same ID only if its stored `Some(epoch, revision)` still matches the current `None`. It commits, persists once, publishes dual-transport `session_created`, then publishes the optional `sessionCreated` selection payload and session-scoped warnings. No outer create caller performs a second persist or lifecycle emit. A missing row, lost PTY, canceled backend, or failed validation invokes backend-kind-aware residual cleanup, removes pending state, and publishes no row or selection. The compare-and-set prevents a create begun under another selection from adopting a later auto-close/cleanup null, and pending exclusion prevents an unannounced PTY becoming fallback or crash persistence.

Root uniqueness is different: every top-level Root reuse/create/wake is one coordinator lifecycle job, while restore/restart Root helpers receive the existing transaction. Remove `ROOT_AGENT_SESSION_LOCK` rather than nesting it with finalization. `Suppress` is legal only inside one of those already-running restore/restart transactions and obtains no independent ticket; `User` publishes `userInitiated=true`, and `Background` publishes `false`. Restore live/dormant placeholders, Root startup auto-create, and restart replacements use `Suppress`; native/WebSocket create and Root commands use `User`; loop delivery, phone/mailbox wake, and CLI/GUI session-request ingestion use `Background`. Exact `Busy` behavior remains: filesystem delivery is retryable/not delivered, inline API wake is rejected, loop delivery records no success and waits for its normal next schedule, and only an otherwise valid GUI session-request JSON remains for the next poll; other request failures retain current deletion behavior. These rules remove the pre-PTY `Active` writer and keep normal spawn rollback selection-free.

### 4.5 Runtime eligibility and async lock safety

`PtyManager::has_session` remains the PTY liveness source and its liveness algorithm is unchanged. `DetachedSessionsState` remains actual detached-window truth. `PtyManager` still receives the constructor/sender wiring and cancellation cleanup changes listed in section 5; no PTY selection policy moves into it.

The selection coordinator follows this sequencing and lock order:

1. The applicable typed submit method obtains bounded admission without holding a state guard. External requests fail fast when the admission budget is exhausted; a deduplicated critical lifecycle waiter waits fairly. The FIFO worker begins the job only after every earlier admitted job completes.
2. Clone `SessionManager` from the outer `Arc<RwLock<_>>`, then release the outer guard.
3. Copy record/order/selection data from one `SessionManagerState` read guard; release it when the async snapshot method returns.
4. Snapshot `DetachedSessionsState` in a short standard-mutex block and release it.
5. Snapshot `PtyManager::has_session` in a separate short standard-mutex block and release it.
6. Immediately before final commit, rebuild any fallback candidate list from a fresh aggregate manager/order snapshot, then re-snapshot each decided target's record, detached membership, and PTY presence in the same separated order. A non-Root create that finishes spawning during teardown remains pending because its reserved finalizer is queued behind the running transaction, so it is deliberately excluded and can become public only afterward. The scan excludes every planned batch member and never trusts a list captured before teardown. A rejected automatic candidate consumes no manager mutation or revision; classify the next stable candidate. A rejected explicit live target preserves the prior selection only after separately revalidating it. If the rejected target is the now-invalid current selection, the same worker transaction performs the policy's sealed `LivenessReconcile` repair rather than emitting an impossible source/mode pair.
7. Call `commit_selection_transition` once for the final decision. Inside the manager it mutates one `SessionManagerState` write guard and releases the guard before returning.
8. For a material selection/lifecycle mutation, persist a coherent aggregate snapshot without holding the manager guard. Persistence failure is logged and does not roll back committed or external state.
9. Publish synchronously to Tauri and the managed WebSocket broadcaster, resolve the job's oneshot, and only then let the worker start the next job. This preserves commit/persistence/event order across coordinator jobs.

No synchronous mutex/RwLock guard, manager internal guard, outer `Arc<RwLock<SessionManager>>` guard, filesystem guard, Telegram guard, or resource-monitor guard is held across an await. Every changed command clones `SessionManager` through the outer read guard, releases that guard, then awaits methods on the clone; persistence receives that clone rather than a live outer guard. The coordinator transaction replaces the current Tokio Root-uniqueness mutex, eliminating a Root-lock/coordinator lock-order cycle instead of documenting one. The coordinator worker provides async transaction ownership without a state lock guard. There is no committed-then-rolled-back provisional selection: such state could leak to persistence even if publication were suppressed. A PTY route can still disappear after any precommit witness; that is a real later lifecycle transition and must enqueue the `livenessReconcile` policy below. Restart, startup restore, detach/attach, Root reuse/create/wake, user/watchdog resource kill, and destruction batches execute as worker lifecycle jobs; helpers inside them receive `SelectionTransaction` and never re-enqueue behind themselves. User selection and hydration queue behind those transactions without blocking an executor thread or holding a synchronous lock.

The current container synchronous failure path needs an additional lock-order correction. `PtyManager::write`/`resize`/`kill` are called while the outer `Arc<Mutex<PtyManager>>` guard is held. `ContainerPtyBackend::close_transport_from_sync` currently calls the route-remover callback synchronously, and that callback tries to lock the same outer mutex, so an outbound queue-full/closed failure can self-deadlock. The synchronous path must mark/remove the backend-internal route state once, clone the cleanup data, and spawn a single future that removes the outer `PtyManager` route and submits lifecycle reconciliation only after the original method has returned and the outer guard can be released. Async disconnect/exit/reaper paths may remove the outer route before awaiting the handler because they do not hold that guard. No new callback may synchronously re-enter `PtyManager`.

Ticket cancellation also exposes a current container-spawn gap that local-process spawn already solves. Dropping `spawn_runtime_backed` while its `spawn_blocking(runtime.start)` is running currently detaches that blocking task; it can create a container after the caller's rollback removed the manager row, while no runtime handle was installed for `PtyManager::kill` to stop. Add a container spawn-cancellation guard from pending-state insertion through successful handshake. Dropping it marks a shared cancellation flag, removes backend pending/attaching state and credentials/token/logical slot once, and schedules cleanup of any installed handle. The blocking start wrapper checks the same flag after `runtime.start`; if canceled, it stops the returned handle before discarding it and never installs or announces the route. Cancellation during handshake removes/stops the already-installed handle. Disarm only after the backend is live and `PtyManager::spawn` can record its route. Rollback is backend-kind-aware even before route registration. Local spawn retains its existing equivalent cancellation guard. This is required for caller cancellation, ticket shutdown cancellation, panic unwind, and the five-second shutdown-cap path.

User detach/attach obtains fail-fast coordinator admission before any window side effect and prevalidates the exact record/window label. Detach creates the window first; a build failure changes nothing. It then adds runtime-detached membership, rechecks that the window still exists and the record still has a live PTY, and commits `SetDetachedIntent(true)` plus the final fallback/keep decision together. Lost window or liveness before commit destroys any just-created window, removes runtime membership, performs the required liveness reconciliation if applicable, and publishes no detach transition. Attach destroys the exact existing window first; a destroy failure changes nothing. On success it removes runtime membership and commits `SetDetachedIntent(false)` with live/dormant selection in one manager mutation. The existing detached geometry remains preserved for a future detach; the dedicated geometry command remains its owner. PTY loss discovered after successful window destruction is classified inside that same job as dormant/none liveness reconciliation rather than returning with a stale live payload. Persistence/event failure never rolls back a committed window state, but is logged. Restore's `skip_switch` path receives its existing transaction instead of reacquiring admission. `WindowEvent::Destroyed` remains an idempotent observation that may clear runtime membership and emit `terminal_attached`, but cannot mutate persisted intent or canonical selection. This prevents busy-after-window partial state, a user switch between membership and selection, and a vanished-window stale-detached record.

Startup restore is admitted as one worker lifecycle job before normal selection-affecting commands. Every restore create helper receives its transaction and remains selection-suppressed; ready live/dormant rows accumulate pending while failed rows are rolled back. Before detached-window reconstruction, one manager commit finalizes all ready rows without selecting, then persists and publishes their `session_created` rows in stable order. This ordering is required because a newly opened locked detached `TerminalApp` immediately calls the normal `list()` projection and does not subscribe to created rows; keeping the row pending through window creation would strand that window blank. The transaction then reconstructs detached windows against public finalized rows, updates actual detached state/`was_detached`, and performs exactly one final restore selection commit/persistence/event from the post-reconstruction eligible set. An early user switch/hydration is queued and therefore observes the final restore state, and a queued explicit user switch may then override it in FIFO order. The existing `RestoreInProgress` behavior for mailbox cleanup remains; it is not used as a second selection lock.

Boot ordering is explicit. The coordinator begins in `CoordinatorPhase::Bootstrapping`; every submit method except `submit_restore_first` returns `Busy` without side effects in that phase. Setup constructs/manages the worker, `PtyManager`, and narrow container lifecycle sender, builds the main window, and then `submit_restore_first` atomically enqueues the restore job and changes the phase to `Running`. That transition occurs before starting the container pending reaper, resource watchdog, web server, control-plane API server, mailbox poller, loop scheduler, auto-close, non-stop watchdog, UI automation, screenshot hotkey, or any other lifecycle producer. Idle/git/discovery workers are constructed but also started only after the restore job has queue position one. Native invoke handling cannot overtake setup; any early frontend hydration that nevertheless observes `Bootstrapping` follows the existing exact-`Busy` retry. Restore is submitted even with zero persisted rows, so there is no no-restore alternate ordering.

### 4.6 Source and policy matrix

Use a separate internal `DestructionSource` enum so callers cannot silently fall back to a generic cause. Source values are assigned by trusted Rust call sites, never accepted from a client-supplied string. `DestructionSource` maps to the published `SelectionSource` when selection changes.

| Flow | Exact policy | Published source and user flag |
|---|---|---|
| Session create | Reserve bounded finalization capacity before the first create side effect and capture `Some(epoch, revision)` only when selection is `None`; capture no auto-select precondition otherwise. After successful PTY spawn, the one reserved finalizer persists/publishes the row and selects it only when that exact `Some` precondition still matches. Cancellation uses the guaranteed rollback slot. Root reuse/create/wake instead runs wholly inside its existing coordinator transaction. Trusted `CreateSelectionIntent::User` publishes `true`, `Background` publishes `false`, and transaction-only `Suppress` performs no transition. A create begun under a non-null selection, or whose `None` changed and later returned, never selects. | `sessionCreated`; flag derived from trusted intent |
| Auto-close | A tick uses fail-fast admission before rechecks/teardown; `Busy` logs one deferred tick and performs no side effect, so the next 60-second tick retries without an accumulating waiter. Once admitted, preserve every current anchor, grace, Telegram, and late-activity recheck. Destroy all confirmed IDs in one batch. If the selected ID is successfully torn down, select `None`. Never choose a fallback. If selection is outside the batch, keep it. | `autoClose`, `false` |
| Manual single close | If the selected session is closed, choose the first remaining eligible live attached PTY-backed record in stable manager order. If none exists, select `None`. Closing a nonselected session preserves selection. | `manualClose`, `true` |
| Manual coordinator cascade | Treat members plus coordinator as one planned batch. Exclude the full planned batch from fallback ranking, suppress all intermediate changes, and apply the same manual fallback once after finalization. A failed selected member that remains live keeps its existing selection; it is not a fallback candidate. | `manualClose`, `true` |
| Explicit restart or dormant wake | Keep the old selection externally stable while restart prevalidation runs. For a live old session, tear down resources only after every pre-teardown validation succeeds; on ready replacement, atomically remove old and select new. For an already-dormant old record, retain that exact record and selection until a replacement is ready, because there is no live PTY to tear down; a failed wake leaves the dormant record/exit code available for retry. On replacement failure after live-old teardown, remove stale records and select `None` if old was selected. Never expose a sibling/root fallback. | `restart`; explicit command/wake `true` |
| Background/bulk restart | A nonselected restarted record does not move selection. If the old record was selected, its ready replacement inherits selection even when the caller's old `activate_after` behavior was false. Agent self-clear may still request activation, but its user flag is false. | `restart`, `false` |
| Startup restore | Suppress all per-create selection. After live/deferred creation and detached-window reconstruction, restore the exact persisted selected record as `Live` when eligible or `Dormant` when exited and attached. A missing, inconsistent, archived, or runtime-detached persisted target falls back only to the first eligible live attached PTY-backed record. With no persisted target, choose the first eligible live attached record. Otherwise choose `None`. Publish at most once. | `restore`, `false` |
| Detach | After successful window creation and detached-state update, change canonical selection only when the detached ID was selected. Choose the first remaining eligible live attached PTY-backed record, or `None`. Detaching a nonselected record preserves selection. Restore's `skip_switch=true` path performs no transition. | `detach`; command `true`, restore suppressed |
| Attach | Missing record remains the current silent cleanup/no-op. A live attached target is selected `Live`. An exited target explicitly attached by the user is selected `Dormant` and shows wake guidance. A non-exited PTY-less target is rejected without manufacturing `Active`; the prior selection remains. | `attach`, `true` |
| Spawn rollback | Normal pre-spawn records cannot be selected because create auto-selection occurs only after successful PTY spawn. Remove the record with no selection event. Defensive cleanup of an unexpectedly selected record selects `None`, never a fallback. | `spawnRollback`, `false` |
| Resource-monitor kill | The user command obtains fail-fast coordinator admission before `kill_group`; `Busy` means no kill side effect. The watchdog obtains deduplicated critical admission before killing, so pressure enforcement is deferred rather than dropped. Each runs kill, verified-result classification, PTY teardown, `MarkExited`, persistence, and publication inside that one worker lifecycle job; no post-kill job is recursively submitted. Only a verified `Terminated` result atomically marks exited and selects `None` when the ID was selected; it never chooses a fallback. Nonfinalized/quarantined/terminating/error results do not change session or selection state. Concurrent user/watchdog requests serialize: the first changed result publishes, the later already-terminated/exited result is a no-op. Low-level `SessionDestroy` and `SpawnRollback` reasons remain inside their already-admitted outer destruction/create transactions and never construct a second resource selection cause. `AppShutdown` is deliberately excluded: global shutdown runs after coordinator join and preserves public rows for restore rather than publishing selection/liveness events during exit. | `resourceMonitor`; `true` for `User`, `false` for watchdog; no other `ResourceKillReason` publishes this source |
| Runtime liveness loss | When a selected live record's route is removed and the record first becomes `Exited(code)`, preserve that exact selected ID as `Dormant`; this is a downgrade of the current selection, not automatic dormant fallback. If the selected record becomes non-exited and PTY-less, clear to `None`. A nonselected first exit changes row/persistence only and consumes no selection revision. Duplicate/late exit callbacks preserve the first exit code and publish nothing. | `livenessReconcile`, `false` |
| Explicit user switch | Runtime-detached target focuses its detached window and does not change canonical selection. Eligible live target becomes `Live`. Exited target becomes `Dormant` with status preserved. A non-exited PTY-less target returns an error and preserves an unrelated still-valid selection. If that target is the currently selected ID whose route was just lost, the same worker job instead performs the required safe null repair under sealed `LivenessReconcile`, then returns the switch error; `userSwitch` itself never publishes `none`. | successful switch: `userSwitch`, `true`; selected-route repair: `livenessReconcile`, `false` |
| Delivery/mailbox/root stale cleanup | Obtain one deduplicated internal admission before destructive cleanup (or use the caller's existing transaction); do not remove first and enqueue later. Treat it as background cleanup. If cleanup removes the selected record, select `None`; never select an unrelated fallback. | `backgroundCleanup`, `false` |

Manual close therefore retains fallback UX. This is the least-surprising policy because current explicit close and detach flows already move to an ordered remaining session. The correction narrows the candidate set and centralizes the change; it does not replace an explicit-close workflow with an unexpected blank pane when a healthy session is available. Background flows remain categorically different and always clear.

Restore normalizes persisted selection intent before creating anything. Exactly one `was_active=true` row defines the exact persisted target. Zero flags means no target. More than one is corrupt/inconsistent input: log the conflicting stable row identities once, treat the exact target as absent, and use the documented first eligible live attached fallback after reconstruction; do not let loop iteration order silently pick the last flag. Archived/missing targets remain ineligible as already specified.

Every lifecycle flow determines “selected” from the coordinator/manager selection snapshot, never by testing whether a record's status is `Active`. In particular, retained-Root close must replace `matches!(existing.status, SessionStatus::Active)` with exact selected-ID membership, because a legitimately selected dormant Root remains `Exited(code)`.

The production container transport-loss paths are hidden selection writers because they remove a route and then call `SessionManager::mark_exited`: `ContainerPtyBackend::close_transport`, `close_transport_from_sync`, and `reap_expired_pending_sessions`. Replace the backend's direct `SessionManager` field with an injected narrow lifecycle sender installed from `lib.rs`; it holds no `AppHandle` or manager and submits typed `(session_id, exit_code)` critical messages to the coordinator worker. Async close/reaper paths await the acknowledgement after route removal, while the synchronous close path defers both outer-route removal and submission as described in the lock-order rule above. The worker submits `MarkExited` plus the decided selected-target downgrade/clear in one `commit_selection_transition`; it does not call public `mark_exited` and then reconcile. A route loss for a still-pending create records no public exit row or selection; its owning finalizer observes the missing PTY and performs rollback. The commit preserves the existing raise-hand cleanup and returns whether its clear event is needed. After the atomic manager result, the same job persists the coherent snapshot and emits/broadcasts the row and optional selection updates through shared transport publication before the next job runs. Resource-monitor command finalization, resource-watchdog finalization, and retained-Root teardown use the same atomic exit-mutation rule with their own causes. Restore-time pending records use restore-only dormant `FinalizeCreate`, not direct exit marking; test-only direct fixtures are updated mechanically.

The enabled production testability surface is also a real caller: `testability/ui_automation.rs::handle_resource_watchdog_backend_request` directly calls `kill_group(Watchdog)` today. Convert backend request processing for this selector to an async task that awaits the same deduplicated whole watchdog coordinator job and writes its response only after finalization; do not block the UI-automation poll loop or call `block_on` from it. Sample/warn-only modes remain read-only. This keeps automation diagnostics deterministic without leaving manager/selection state stale.

A retained Root that is already dormant is not exited a second time merely because the user closes it. Its original `Exited(code)` is preserved byte-for-byte, its terminal cache is disposed, and the manual fallback/null transition is computed from canonical selected-ID membership. This pins dormant close behavior and prevents a repeated `MarkExited(id, 0)` from erasing a real nonzero exit code.

### 4.7 Multi-member destruction transaction

Refactor the shared destroy implementation into a typed single/batch transaction instead of repeated calls that publish independently.

- One single-close or batch-close operation executes as one coordinator worker job; no nested destroy helper re-enqueues behind its own job.
- A batch records the complete planned ID set before teardown.
- Auto-close continues to run its current per-member late anchor, repaint-silence, and Telegram rechecks immediately before asking the open batch to tear down that member. This preserves current inactivity semantics and per-member protection while deferring selection publication.
- Resource/Telegram/PTY teardown is performed per member. Successful teardown is queued for atomic record removal; a failure with a still-live PTY leaves the record and existing selection intact. A teardown that reports an error but leaves no PTY removes an ordinary session. The one retained-Root exception is atomically marked `Exited(observed_code)` when an exit code exists, preserves an existing first exit code, and otherwise uses the existing deliberate-teardown code `0`; it is never retained as a live-looking phantom. There is no caller choice between remove and exit marking.
- `session_destroyed` is emitted once per record actually removed, or once for a retained Root whose terminal cache must be discarded. Failed live records do not emit it.
- The manager applies all queued removals plus one final caller-decided selection in a single commit.
- The batch emits zero or one `session_switched` payload after every member has reached a final state.
- Every error path calls batch finalization before returning. Dropping a partially used batch without finalization is a logged invariant violation covered by failure-path tests.

### 4.8 Restart atomicity and failure behavior

Replace destroy-then-create publication with a selection transaction:

1. Snapshot old configuration, old selection membership, source/user intent, and carry-over fields, including the agent self-clear communication value that currently gets restored by `phone/mailbox.rs` after restart returns.
2. Enqueue the whole restart as one coordinator worker job before old PTY teardown. Hydration and other selection jobs remain queued behind it.
3. Complete every fallible configuration/profile/archive prevalidation that does not require releasing the old process's resource slot before touching the old Telegram bridge, PTY, manager record, selection, or events. For a live old session, then tear down old external resources without removing its manager record or publishing `session_destroyed`/selection yet; failures that can occur only after slot release are post-teardown failures. For an already-dormant old record, skip teardown and keep it retryable until success.
4. Create the replacement with create auto-selection suppressed and restart-only lifecycle publication deferred. The helper returns pending `SessionInfo` plus warnings to the transaction; it does not emit `session_created` or other session-scoped events until final validation succeeds.
5. Verify the replacement has a live PTY and is attached.
6. Before finalization, copy any self-clear communication into the pending replacement, then atomically `FinalizeCreate(new)`, remove the old record, and select the replacement when `activate_after` is true or the old record was selected. Otherwise finalize the replacement while preserving the unrelated current selection.
7. Persist the coherent final snapshot, then publish `session_created(new)`, `session_destroyed(old)`, and the one final selection payload in that order when selection changed. The created row already carries transferred self-clear communication, so that birth does **not** also emit `session_communication_changed`; that event remains for mutations of an already-public row. Publish deferred warnings only for the successful replacement. Then reattach optional Telegram state; a Telegram reattach failure is logged but does not roll back the live replacement.

Failure rules are fixed:

- Failure before the old live PTY is lost leaves its record, selection, Telegram attachment, and PTY unchanged and emits no create/destroy/selection event.
- Failure to wake an already-dormant old record removes any partial replacement but retains the old dormant record, exact exit code, and selection; it emits no create/destroy/selection event.
- Failure after old teardown removes the old record and any unannounced partial replacement. If old was selected, publish one `restart` null selection. If old was not selected, preserve the unrelated selection and emit no selection event.
- A replacement that spawned but fails final liveness/displayability validation is treated as restart failure, not selected.
- Because restart publication is deferred, cleanup of a partial replacement never leaves a `session_created` ghost row and does not need a compensating destroyed event. Early replacement publication is not permitted.
- No restart failure chooses a fallback.
- Event publication failure does not roll back committed backend state. It logs source/revision/error; the next `get_active_session` hydration self-heals the client.

### 4.9 Tauri and WebSocket parity

- Add one `publish_selection` implementation used by the coordinator. It emits the typed payload to Tauri windows and the managed `WsBroadcaster` exactly once.
- Use the same dual-transport publication for `session_destroyed` from the shared destruction transaction so browser clients also dispose caches for background auto-close.
- Use shared dual-transport publication for every `session_created`/row refresh and `session_communication_changed` produced by a changed lifecycle path, including normal/background create, retained Root refresh, resource finalization, and container liveness loss. The current shared create helper emits only to Tauri while the WebSocket command does not publish `session_created`; without this correction a browser sidebar can receive a live selection for a row it never receives. Web command routes do not compensate or duplicate these events.
- Every first transition of a public live row to retained `Exited(code)`, whether from retained Root close, verified resource kill, or container liveness loss, publishes dual-transport `session_destroyed(id)` for xterm-cache disposal and locked-detached-window closure, then `session_created(exited row)` as the established row-refresh event, then the optional authoritative dormant/null selection. A duplicate `MarkExited` publishes none of the three. Without the destroy signal, a detached `TerminalApp` (which intentionally ignores central selection and created-row events) would remain mounted on a dead route indefinitely.
- `src-tauri/src/web/commands.rs` stops broadcasting selection and destroy events independently after shared calls. It does not re-query `get_active`, clear detached state, or create a second revision.
- Native and web `switch_session` routes call the same source-aware inner command.
- Native and web `get_active_session` routes return the same `SessionSelection` snapshot from the same manager revision.
- Restrict the WebSocket `broadcast_event` command to the exact client-originated UI allowlist currently required by the wrapper: `theme_changed` accepts an exact `{ light: boolean }` object, `resource_monitor_attach` accepts an exact empty object, and `open_settings` accepts an exact `{ section?: string }` object. Rust uses three `#[serde(deny_unknown_fields)]` payload structs and rejects nonobjects, extra keys, wrong field types, `null`, and every other event name. `session_switched`, `session_created`, `session_destroyed`, and every backend-owned lifecycle event are denied. The current arbitrary event string lets an authenticated browser read the epoch and forge a valid high-revision selection despite a perfect decoder.
- Extend `Transport` with a synchronous local `connectionState(): { state: "connected" | "disconnected", generation }` snapshot plus an optional lifecycle subscription carrying the same value. `WsTransport` increments generation exactly once on each accepted `onopen`, reports `disconnected` for that generation on close, and ignores callbacks/messages from superseded socket objects. The snapshot closes the missed-open race where the singleton socket connects before an app subscribes. These notifications are local and are never sent through `broadcast_event`. The IPC wrapper exposes `getTransportConnectionState` and `onTransportConnectionState`; Tauri always snapshots `{ state:"connected", generation:0 }` and its subscription returns a no-op unlisten.
- `TerminalApp` and `SidebarApp` register the lifecycle listener, synchronously read the current snapshot, reconcile the greater/current generation if an event raced the read, and only then start initial hydration when connected. If initially disconnected they remain neutral until an accepted connected event. On WebSocket disconnect, each immediately invalidates live routing/display state and revokes in-flight live input. On initial connect or reconnect, each records that connection generation as awaiting hydration, then calls `getSelection()` and applies the result only if the request generation is still current **before** considering epoch/revision. An exact `selectionCoordinatorBusy` response schedules one coalesced, generation-owned retry timer with capped backoff (50, 100, 250, 500, then 1000 ms); there is never more than one pending request or timer per app, and disconnect, unmount, an accepted selection event, or a new generation cancels it. Non-busy errors are logged once and keep the safe neutral state. Normal equal revisions remain stale. The explicit reconnect-hydration path may rebind an equal epoch/revision exactly once when the store is still awaiting that same generation and no newer logical selection has arrived; any newer event/new epoch clears the awaiting marker so the late equal snapshot cannot rebind. A backend restart supplies a new epoch and replaces the old revision domain.
- Every hydration result, including the initial request, carries its request-start connection generation into reconciliation. `onSessionSwitched` synchronously tags each decoded event with `getTransportConnectionState().generation` inside the transport callback before invoking app consumers; Tauri tags 0. A late result from an older generation is rejected even when its epoch was never previously applied and therefore is not yet in `retiredEpochs`. Event callbacks from a superseded socket are filtered at `WsTransport`; tests also inject a replay on the new socket to prove a retired/old epoch cannot return merely because its JSON is otherwise valid.
- Slow WebSocket clients may still be dropped by the existing bounded broadcaster. Reconnect hydration restores the current snapshot; no mutation is rolled back for a delivery failure.

### 4.10 Frontend reconciliation

#### Terminal

The unlocked central `TerminalApp` branch owns one `reconcileSelection(payload)` path for both decoded hydration and decoded events. A `lockedSessionId` detached window never follows canonical central selection: it retains its exact-ID list binding, is created only after backend live/detached validation, and closes on a matching destroyed event. The refactor keeps that branch conditional so a global null/dormant/fallback event cannot blank or retarget an unrelated detached window.

1. Register `onSessionSwitched` before starting hydration.
2. Initialize applied revision to `-1` with no epoch. For the same epoch, normally accept only a strictly greater revision. For a new nonretired epoch, retire the previous epoch and accept regardless of numeric revision. Reject every payload from a retired epoch so a late old hydration cannot replace a newer process. The sole exception is an equal epoch/revision reconnect snapshot tagged with the exact currently awaited connection generation; it may rebind cleared live state but does not represent a new logical selection.
3. Check the captured connection generation first, then reserve the accepted epoch, revision, selection ID/mode, and connection generation in `terminalStore` before any `SessionAPI.list()` await. Accepting a newer logical event clears any reconnect-await marker.
4. For `none`, clear selection ID, live active ID, title, shell/args, cwd, task, Root flag, and prompt-driving state immediately.
5. For `dormant`, retain only selection ID/mode/revision, clear the live active ID and live metadata, and show `Session exited. Wake it from the sidebar.` Do not mount `TerminalView`.
6. For `live`, atomically suspend the previous binding before calling `list()`: set `activeSessionId=null`, clear all old routing metadata/prompt/task data, and revoke selection-owned voice/automation input. This is mandatory even though the accepted payload is live; leaving live A writable while B's list request is pending lets keystrokes continue to route through stale A after the backend has selected B. Preserve the accepted selection mode and keep `TerminalView` mounted in a no-active-route pending state so its existing per-session cache is not destroyed on every live-to-live switch; its current reactive null guard hides all entries and disables input/snapshot/resize until binding succeeds. Capture ID, epoch, revision, and connection generation. On completion, discard the result unless all four still match the store's current authoritative selection/binding generation.
7. If the live ID is absent from `list()`, the matching row is `Exited`, or `list()` fails, clear all displayed/live state while retaining the accepted revision so an older completion cannot restore the previous terminal. A `Running`/`Idle` row may be an older row snapshot and the authoritative live payload promotes it; an `Exited` row for the same immutable session ID can never be a valid predecessor of a new live incarnation and must not bind.
8. Bind metadata only after the revision and ID recheck.

For the unlocked central branch, `terminalStore` distinguishes `selectionId` from `activeSessionId` and owns the current epoch, applied revision, retired epochs, current connection generation, optional awaiting-hydration generation, and live binding state (`pending`, `bound`, or `unavailable`). `activeSessionId` is non-null only for a bound `live`/displayable selection. Existing `StatusBar`, `WorkgroupTask`, `LastPrompt`, voice, clear-input, and central `TerminalView` operations continue to use that live ID, so a dormant, missing, pending, disconnected, or unavailable central selection cannot call `get_screen_snapshot`, `pty_resize`, `pty_write`, task commands, or voice input. The unchanged detached `TerminalView` may use its locked ID only while its validated window exists; matching destruction closes the window and disposes the route/cache.

`session_destroyed` no longer calls `get_active_session` or `list()` in the main terminal and never derives a replacement. It closes a matching locked detached window and disposes the matching cache. If its ID equals the current `activeSessionId`, `TerminalApp` also synchronously safety-suspends that live binding (active ID and writable metadata become null while the accepted epoch/revision/selection ID are retained) before returning from the event callback. Destruction is published before the authoritative final selection, so treating it as cache-only would leave a dead selected route writable during the inter-event window. The later selection event or reconnect hydration alone decides `none`, fallback, dormant, or replacement. Remove `TerminalApp`'s first-`session_created` auto-selection heuristic. Selection events remain the sole selection authority.

`TerminalView`'s snapshot, retry, resize, and cache code remains unchanged. In the unlocked central branch it is mounted for an accepted live selection even while binding is briefly pending, but its reactive active ID is null during that interval, so all existing input/snapshot/resize guards stay closed while hidden caches survive a live-to-live metadata lookup. It is unmounted for central `none`, `dormant`, disconnect, or an unavailable live row, making its existing unavailable-buffer guidance live-PTY-specific by construction. Its established locked-detached ID path is preserved and is not fed through central selection reconciliation.

`LastPrompt` removes its `session_switched -> list()` listener. It loads initial prompt data, updates on `session_created` and `last_prompt`, and derives the displayed prompt from the terminal store. It must not own a second selection reconciliation path.

#### Sidebar

- Store the last `SessionSelection`, current epoch, applied revision, retired epochs, and reconnect hydration generation.
- Replace `setActiveId` with `applySelection(payload)`, which applies the same epoch/revision retirement and exact-generation reconnect-rebind rule as the terminal, sets the canonical highlight, preserves exited status for `dormant`, marks only a `live` target active, demotes any prior frontend `active` string, and clears pending review on the selected record.
- A non-null payload whose record is absent clears the visible active ID while retaining the revision. A stored/reapplied `live` payload must also refuse to promote a row whose current status is `Exited`; session IDs are not reused, so an exited upsert is newer liveness evidence, not a stale pre-live row. This prevents retained Root's `session_destroyed` -> exited `session_created` -> selection event sequence from briefly manufacturing frontend `active` from the old stored live payload.
- Register the selection listener before initial `list()`/`get_active_session` work. Apply the list, then reapply the newest stored selection; an older hydration payload is ignored by revision.
- `setSessions` and `addSession` reapply the stored selection state after merging so a late list or created event cannot overwrite a newer selection.
- `session_created` adds/updates the row only. It no longer auto-selects the first row.
- `session_destroyed` removes the row and detached UI flag only. It never derives selection.

#### Other consumers

- `listeners-home.ts` and `listeners-central-view.ts` continue to react only when the now-required `userInitiated` field is true and `id` is non-null. Update comments that currently mention automatic destroy promotion.
- `shortcuts.ts` reads the decoded selection once per shortcut. Ctrl+Shift+W may use a non-null live or dormant selected ID for the existing close workflow. Ctrl+Shift+R calls voice only when the union mode is `live`; `none` and `dormant` are no-ops. No component invokes transport directly.
- Existing sidebar click/restart UX remains. A persisted dormant Root selection now produces central wake guidance until the user uses the existing wake action.

`src/shared/voice-recorder.ts` gains an operation-generation lease and `revokeSession(id)`/`revokeLiveBinding()` cleanup. Destroy, dormant/null selection, live-to-live suspension, disconnect, and an exited row revoke the affected lease, stop MediaRecorder and media tracks, clear backend recording state, and cancel auto-execute timers. Every continuation after `getUserMedia`, `voice_transcribe`, `hadTyping`, or settings lookup rechecks the lease before `PtyAPI.write` or scheduling Enter, so a recording started while live cannot write text or `\r` after the session becomes dormant/destroyed. Cancel cleanup may send the one `voice_mark_recording(false)` bookkeeping call; it sends no PTY input.

Audit all sidebar voice launch surfaces, not only the central status bar. `SessionItem.tsx` must use its existing `sessionHasLivePty` predicate for rendering and in the click handler instead of the current `!isInactive()` gate, while `RootAgentBanner.tsx` and the ProjectPanel quick row retain/add the same defensive handler check. A dormant row exposes close/wake behavior but no mic, detach, Telegram-input, or other PTY-dependent action. In particular, Root's close button moves outside the current `hasLivePty()` render block because close is a lifecycle command, not PTY input; its handler accepts an existing dormant record, awaits/catches the destroy promise under the existing busy gate, and relies on the backend's exact-code-preserving retained-Root policy. Repeated close while already unselected is a backend no-op selection transition. The shared coordinator-close helper catches direct noncoordinator destroy failures internally before UI callers intentionally discard its promise, and keyboard shortcut handlers catch failed hydration/close dispatch; no modified `invoke()` rejection becomes an unhandled promise. `session_destroyed` and exited row upserts revoke an in-flight per-row voice lease even when the destroyed/exited session was not canonically selected.

Both `TerminalApp` and `SidebarApp` use a disposed flag/registration helper: if unmount occurs while an async `listen` or connection subscription is resolving, the late unlisten is invoked immediately; pending hydration/list completions check disposal before store mutation. This prevents the added reconnect listeners and reconciliation promises from surviving a destroyed window/component.

`src/shared/session-selection.ts` is the sole trust boundary for this contract. `decodeSessionSelection(value: unknown): SessionSelection` validates a plain own-property record, the exact key set, canonical UUID epoch/ID strings, nonnegative safe-integer revision, the source/mode/user-intent table, signed 32-bit exited code, and every literal in the mode invariant table. It returns a newly constructed value and rejects missing, extra, inherited, accessor-backed, or invalid combinations with a diagnostic error before a component/store can mutate. Transport generics are compile-time hints, not runtime validation; components never cast raw JSON into this union. Test fixtures therefore use valid UUIDs rather than placeholders such as `session-1`.

## 5. Exact files and symbols

### 5.1 Rust production changes

| File | Exact changes |
|---|---|
| `src-tauri/src/session/selection.rs` (new) | Add payload/source/mode/runtime types, sealed cause-to-source/user mapping, managed-handle-free typed `CoordinatorJob` enum, exact 65/64/16 admission machinery, three-kind critical dedup, boot/running/closing phases, recursive-submit error, five-second joined shutdown, create-ticket/capability lifecycle, worker context, transaction/commit capability, transition/snapshot API, pure eligibility helpers, structured logging, dual-transport publication, and source-ownership sentinels. |
| `src-tauri/src/session/mod.rs` | Export the new `selection` module. |
| `src-tauri/src/session/manager.rs` | Consolidate records/order/pending-create IDs/next-number/selection/process-epoch/payload/revision into one `SessionManagerState` lock; generate the epoch with the initial revision-0 payload; make `create_session` policy-free and pending-by-default; require ticket/transaction capabilities for pending setters; exclude pending rows from normal list/persistence/candidate snapshots; add atomic `FinalizeCreate` and `SetDetachedIntent`; replace `destroy_session`, `switch_session`, `set_active_only`, `clear_active`, `clear_active_if`, and standalone `get_active` production paths with `commit_selection_transition`/aggregate snapshots; add the exact test-only pending finalizer, stable candidate/removal outcomes, and tests. |
| `src-tauri/src/commands/session.rs` | Add typed `DestructionSource` and `CreateSelectionIntent`; make ticket reservation the first non-Root create side-effect gate; use optional null preconditions, ticket-bound pending metadata, one finalizer-owned persist/event sequence, and backend-kind-aware cancellation rollback; remove outer duplicate create persistence/emits. Add whole-transaction Root reuse/create/wake with removal of `ROOT_AGENT_SESSION_LOCK`, shared single/batch destruction, source-aware switch, final-state manual fallback, canonical-ID retained Root handling, exact dormant exit-code preservation, atomic restart with deferred events/failure cleanup and in-transaction self-clear communication transfer, typed `get_active_session`, and removal of direct selection emits. Thread trusted intent/transaction through every helper. |
| `src-tauri/src/session/auto_close.rs` | Keep all clocks/predicates/constants; fail-fast admit the whole tick before side effects and defer cleanly on `Busy`; open one `AutoClose` batch, perform current rechecks per member immediately before deferred teardown, finalize once, and derive coordinator badge results from actual successful teardown outcomes. |
| `src-tauri/src/commands/window.rs` | Admit before window effects; route detach/attach through one transaction; commit `SetDetachedIntent` with selection; preserve geometry; compensate a lost new window/live route; classify post-attach route loss safely; only switch away when detaching the selected ID; remove direct switch/clear/selection emits. |
| `src-tauri/src/lib.rs` | Start/manage the phased `SelectionCoordinator`, worker, and shutdown guard; install the narrow container lifecycle sender; enqueue the mandatory restore job at FIFO position one before starting every lifecycle producer/server/watcher/hotkey listed in section 4.5; pass narrow handles to watchdog/automation; run restore as one transaction with `Suppress`, no Root mutex/direct exit/clear calls, and one final selection after detach reconstruction. On `RunEvent::Exit`, trigger producers, close/drain-or-abort-and-join with the exact five-second cap, then run unchanged global PTY/resource cleanup and final aggregate persistence; keep `get_active_session` registration. |
| `src-tauri/src/web/commands.rs` | Return typed hydration; call shared source-aware manual destroy/switch paths; delete active re-derivation and duplicate WebSocket emits; deny backend lifecycle names in client `broadcast_event`; decode only the three exact `deny_unknown_fields` UI payload structs; add parity/event-count/forgery/payload-negative tests. |
| `src-tauri/src/commands/resource_monitor.rs` | Obtain fail-fast coordinator admission before user `kill_group`, then run kill verification/finalization in that transaction; on verified first exit submit `MarkExited` plus selected-target null, publish destroyed-for-cache, exited row/raise-hand, and optional selection updates to both transports in order; `Busy` has no kill side effect and nonfinalized paths remain unchanged. |
| `src-tauri/src/resource_monitor/watchdog.rs` | Replace fire-and-forget ignored kill results with a deduplicated critical coordinator job that gains admission before `kill_group`, then uses the same verified idempotent finalizer as the command without recursive submission; log errors, keep quarantined/terminating state unchanged, and classify watchdog transitions as non-user initiated. App-shutdown bulk kill remains outside selection finalization after coordinator join. |
| `src-tauri/src/testability/ui_automation.rs` | Route the real backend watchdog kill selector through the shared async whole-watchdog coordinator job and complete its response file after finalization; spawn/await asynchronously from the poller without runtime `block_on`. Keep sample/warn modes read-only and preserve diagnostics schema. |
| `src-tauri/src/pty/container_backend.rs` | Remove the direct `SessionManager` field and replace three direct exit/persist paths with the narrow coordinator sender; await acknowledgement in async close/reaper and defer outer-route removal/submission in synchronous close. Add the shared-flag container spawn-cancellation guard that cleans pending/attaching state and stops a late `runtime.start` handle before install. The worker performs atomic exit/selection mutation and first-exit destroy/row publication without an AppHandle/channel retain cycle. |
| `src-tauri/src/pty/manager.rs` | Remove the manager constructor argument, install the narrow lifecycle sender before producers start, retain the existing `has_session` algorithm, and add backend-kind-aware pending-spawn cleanup before registry route installation. No selection policy moves here. |
| `src-tauri/src/config/sessions_persistence.rs` | Replace separate `list_sessions()` plus `get_active()` reads with one aggregate manager snapshot so records, selected ID, and `was_active` are from one committed state. |
| `src-tauri/src/screenshot/windows.rs` | Replace raw selected-ID lookup with a narrow `resolve_live_capture_session` helper over the typed coordinator snapshot; capture only for `Live`/displayable selection and return the existing no-active error for `None` or `Dormant`. |
| `src-tauri/src/loops/delivery.rs` | Pass `BackgroundCleanup` for stale-session cleanup and `Background` create intent; coordinator Busy performs no spawn and remains a failed iteration handled by the existing later schedule, with no delivery-success audit. |
| `src-tauri/src/phone/mailbox.rs` | Pass typed background/restart sources; move self-clear communication capture into the restart request and remove the post-return direct restore/emit; preserve filesystem/API/loop Busy behavior, but keep valid GUI request JSON for next-poll retry only on exact Busy; log deletion failures; update test fixtures to the manager test seam. |
| `src-tauri/src/commands/config.rs` | Replace boolean-only bulk restart activation with the typed restart selection intent: background user flag false, replacement inherits selection only when old was selected. |

`src-tauri/src/pty/manager.rs` receives only constructor/lifecycle-sender and backend-kind-aware cancellation cleanup; its verified `has_session` algorithm already supplies the route/backend liveness check and remains unchanged. `src-tauri/src/session/session.rs` is not changed: pending-create state and the selection contract belong in manager/coordinator state, and no stale per-record `hasPty` field or new status variant is introduced.

### 5.2 TypeScript/TSX production changes

| File | Exact changes |
|---|---|
| `src/shared/types.ts` | Add the named invariant-bearing mode union intersected with a source/user-intent/allowed-mode union so impossible policy tuples reduce to `never`; extend state typing only for stored epoch/revision/binding data. Do not add list-derived PTY liveness. |
| `src/shared/session-selection.ts` (new) | Add the sole fresh-object `unknown`-to-`SessionSelection` runtime decoder, exact-key/plain-record checks, exhaustive mode/source/intent validation, UUID/safe-integer checks, and decoder tests. |
| `src/shared/transport.ts` | Add synchronous local connection-state/generation snapshot and optional connected/disconnected subscription to the transport interface; Tauri is connected generation 0. |
| `src/shared/transport-ws.ts` | Snapshot and publish local lifecycle generations on accepted `onopen`/`onclose`, reject superseded socket callbacks, preserve listeners across reconnect, and add pre-subscription-open/disconnect/reconnect/replay-generation tests. Do not send lifecycle notifications to the server. |
| `src/shared/ipc.ts` | Import/export the typed payload; replace `SessionAPI.getActive` with decoded `getSelection` over an `unknown` `get_active_session` result; decode `onSessionSwitched` unknown payloads and synchronously tag callbacks with the delivery generation; expose connection snapshot plus `onTransportConnectionState`; classify only the exact `selectionCoordinatorBusy` string from `catch (error: unknown)` for the coalesced hydration retry. Keep all invokes/listens behind this wrapper. |
| `src/shared/voice-recorder.ts` | Add per-operation generation leases and session/live-binding revocation; fence every async continuation before PTY write/auto-execute and fully release MediaRecorder, stream, AudioContext, timers, and backend recording tracking on revocation. |
| `src/terminal/stores/terminal.ts` | Add selection ID/mode/epoch/revision/retired epochs, connection/awaited-hydration generations, binding state, and a live-only `activeSessionId`; add ordering-guarded suspend/begin/bind/clear methods that reset all stale metadata atomically; preserve rename partial-update behavior through a dedicated method. |
| `src/terminal/App.tsx` | In the unlocked central branch, register selection and connection-lifecycle listeners before hydration with disposal-safe registration; implement the one generation-first epoch/revision reconciler; suspend live routing before every live metadata lookup, matching destroy event, and disconnect without deriving selection; reject exited rows; remove destroy requery and first-created auto-selection; render neutral, dormant, pending-live, or bound-live content from authoritative state. Preserve the conditional exact-ID detached branch and its matching-destroy window close; never apply central selection to it. |
| `src/terminal/components/LastPrompt.tsx` | Remove selection listener and selection-triggered relists; keep initial/create/prompt data ownership only. |
| `src/sidebar/stores/sessions-helpers.ts` | Add a pure payload-to-session-list reducer that preserves exited records, refuses stale-live promotion of an exited row, and applies valid live status consistently. |
| `src/sidebar/stores/sessions.ts` | Store/apply epoch/revision selection order plus reconnect-hydration generation, replace `setActiveId`, reject retired epochs, and reapply newest selection after list/upsert mutations. |
| `src/sidebar/App.tsx` | Register authoritative selection and connection-lifecycle listeners first with disposal-safe registration; hydrate list plus decoded selection generation-safely; clear stale live highlight/voice lease on disconnect; revoke destroyed/exited-row voice; remove first-created auto-selection; keep destroyed event row/cache-only. |
| `src/sidebar/components/SessionItem.tsx` | Gate mic, detach, Telegram-input, and other PTY-dependent controls and handlers on the existing live-record predicate; preserve close/wake behavior for dormant rows. |
| `src/sidebar/components/RootAgentBanner.tsx` | Keep mic/detach/Telegram and their handlers live-gated, move close outside the live-only render block so a selected dormant Root can be manually cleared/fallback-selected, preserve wake, and add voice revocation coverage. |
| `src/sidebar/components/ProjectPanel.tsx` | Add defensive live checks to quick-row voice handlers and revoke a row's in-flight voice operation when it exits. |
| `src/sidebar/stores/coordinator-close.ts` | Put coordinator and direct noncoordinator destroy branches under one internal error boundary so existing `void requestCoordinatorClose(...)` UI dispatches cannot produce unhandled rejections; retain confirmation behavior. |
| `src/shared/shortcuts.ts` | Replace `getActive` calls with decoded selection: close may use any non-null selected ID, while voice requires the `live` variant; catch/log rejected hydration or close dispatch inside the async shortcut invoked by the DOM handler. |
| `src/main/listeners-home.ts` | Consume required typed payload and correct obsolete destroy-fallback comments; behavior stays user-initiated-only. |
| `src/main/listeners-central-view.ts` | Consume required typed payload and correct obsolete destroy/detach promotion comments; behavior stays user-initiated-only. |
| `src/main/stores/home.ts` | Replace the obsolete `SessionAPI.getActive` comment with selection-mode terminology; no store behavior change. |

`src/terminal/components/TerminalView.tsx`, `StatusBar.tsx`, and `WorkgroupTask.tsx` are verified but intentionally unchanged. Their existing `activeSessionId` gates become safe because the store exposes that ID only for a bound displayable live selection. `TerminalView` may remain mounted with a null reactive ID during a pending live lookup so its existing cache survives; it performs no PTY operation in that state. The current snapshot message is not rewritten or retried.

### 5.3 Documentation changes

| File | Exact changes |
|---|---|
| `docs/reference/architecture.md` | Replace `getActive` with decoded selection hydration; document the full `session_switched` payload, process epoch/revision ownership, reconnect hydration, and `session_destroyed` cache/safety-suspension-only responsibility without replacement derivation. |
| `docs/features/session-auto-close.md` | State that automatically closing the selected session clears the central pane and never chooses an unrelated fallback; explicitly state passive viewing/focus is not activity; correct the troubleshooting claim that reopen starts from fresh activity, while retaining repaint/wake grace semantics. |

## 6. Required behavior and edge cases

### 6.1 Selection and status invariants

- At most one canonical selected ID exists.
- At most one record is `SessionStatus::Active`, and it is the canonical `Live` selection.
- Published source, mode, and user-intent fields always satisfy the source contract table and are derived from one sealed Rust cause.
- `Exited(code)` never becomes `Active` without a successful new PTY spawn represented by a new live session record.
- A non-exited record without a PTY is never selected automatically or marked `Active` by selection bookkeeping.
- A pending-create record is not a public session and cannot be selected, listed, persisted, detached/attached, captured, or ranked until its owning finalizer atomically removes the pending marker.
- A runtime-detached session is never canonical in the central pane.
- Dormant selection preserves its exit code and never enables terminal input, resize, snapshot, voice, or task actions.
- Candidate ranking is stable manager order after exclusions. Rejection continues to the next candidate and logs the exact reason.
- Closing/destroying a nonselected session does not generate a selection revision.
- Manager record/order/selection snapshots are coherent: persistence never serializes a missing selected record or assigns `was_active` from a different generation than the rows.
- Natural route loss of the selected live record publishes a later dormant downgrade or safe null through `livenessReconcile`; it never leaves the stored payload claiming `Live` after the exit handler completes.
- A duplicate resource/transport exit cannot overwrite the first exit code, republish a row, or consume another selection revision.
- A create can auto-select only against the unchanged `None` epoch/revision it observed before insertion; it cannot adopt a later `None` created by destruction.

### 6.2 Event ordering

- New-session auto-selection: the same dual-transport `session_created(new)` precedes final `session_switched(new)`.
- Destruction: each successful `session_destroyed(id)` precedes the one final selection event.
- Restart success: `session_created(new)`, `session_destroyed(old)`, then the one final `session_switched(new)`.
- Restart failure after teardown: `session_destroyed(old)`, then null selection only when old was selected.
- Restart wake failure for an already-dormant old record emits no create/destroy/selection event and leaves that record selected dormant with its original exit code.
- A retained Root close emits `session_destroyed(root)` for terminal-cache disposal, then `session_created(root exited)` for row refresh, then the final fallback/null selection when Root was selected.
- Resource/container first-exit finalization uses that same destroyed-for-cache, exited-row refresh, optional selection order; a nonselected exit emits the first two only, and a duplicate emits nothing.
- No event source omits `epoch`, `source`, `userInitiated`, `revision`, mode, status, PTY snapshot, detached snapshot, or displayability.
- A client-originated `broadcast_event` cannot publish any backend lifecycle event. Changed `session_created`, `session_destroyed`, row-refresh, communication, and selection attempts reach Tauri and WebSocket through one backend publisher in the same order.

### 6.3 Partial destruction failure

- A target whose cleanup fails while its PTY is still live remains a record and remains selectable. It receives no destroyed event.
- A target whose PTY is confirmed gone is never left selected as live merely because later cleanup reported an error.
- Manual fallback excludes the planned batch so a sibling awaiting teardown cannot be chosen. A selected target that itself failed and remains live is preserved as the existing selection, not rediscovered as fallback.
- Auto-close never falls back, including partial batches.
- Badge markers use actual coordinator teardown success, preserving current #589 semantics.
- Fallback candidates are scanned from final manager order after teardown. A member created, detached, exited, removed, or added to the planned set during a barrier-controlled batch is either correctly included or excluded by that final state; a stale pre-teardown candidate list is never committed.

### 6.4 Stale and missing frontend data

- Within one epoch, revisions lower than or equal to the applied revision are ignored. A new nonretired epoch supersedes any numeric revision; a retired epoch can never return.
- Connection generation is checked before epoch. WebSocket disconnect clears stale live routing immediately; a hydration result from an older connection generation is ignored even if its epoch was never applied. Equal epoch/revision is allowed only for the one still-awaited current-generation reconnect snapshot so a transient reconnect can safely rebind, and a newer event cancels that exception.
- A stale `list()` completion cannot update ID, mode, metadata, xterm, or input after a newer epoch/revision was reserved.
- Accepting live B suspends live A and all input before B's `list()` await. While B is pending, neither A nor B receives snapshot, resize, write, voice, task, or automation IPC.
- A selected ID missing from `list()` clears the terminal store instead of retaining the prior session.
- A matching list row already marked `Exited` cannot be bound from an older live payload, and a stored live sidebar payload cannot promote a later exited upsert.
- A failed `list()` for a new selection clears the previous live surface; it does not leave input routed to the old ID.
- Null clears title, command, cwd, task, Root flag, last-prompt selection, terminal mount, and automation input.
- Dormant renders wake guidance and no generic resize recovery promise.
- Destroy/exit/disconnect revokes in-flight MediaRecorder/transcription/auto-execute continuations; no delayed transcript or Enter reaches a dormant, removed, or superseded session.

### 6.5 Logging

Every transition attempt logs one narrow structured line containing:

- epoch and revision;
- old and new IDs;
- source and user flag;
- old and new modes;
- old and new statuses;
- target PTY presence;
- target runtime-detached state;
- result (`committed`, `noop`, or `rejected`).

Every rejected automatic fallback logs candidate ID, status, PTY presence, detached state, exclusion membership, and one stable reason (`missingRecord`, `pendingCreate`, `exited`, `missingPty`, `detached`, `plannedForDestruction`, or `lostEligibility`). Do not add PTY output to these lines and do not add a new log crate.

Every changed production path handles fallibility explicitly. Queue admission/closure, oneshot cancellation, worker shutdown, lock poisoning, event serialization/publication, spawned-task join, persistence, window destruction, and lifecycle callback failure carry the source and session ID in their diagnostic. Do not introduce production `.unwrap()` or a recoverable `expect()`, and do not use `let _ = ...` for a selection/lifecycle event, persistence result, worker send, or teardown result. A committed state is never rolled back because one transport publication failed, but that failure is logged with epoch/revision and the other transport is still attempted.

## 7. Compatibility and security impact

### 7.1 Compatibility

- `session_switched` retains its event name and `id` field. Existing consumers that only read `id` remain structurally compatible; new bundled consumers require all fields.
- `userInitiated` changes from optional to always present.
- `get_active_session` keeps its command name but returns the full payload instead of `string | null`. This is an intentional internal/WebSocket contract change required for revision-safe hydration. Bundled Tauri and browser clients ship with the matching wrapper in the same change.
- Selection revision is process-local and resets with a new backend process. The process UUID epoch identifies that revision domain, and all windows and WebSocket clients attached to one process share it. Epoch is runtime-only and is not persisted.
- WebSocket reconnect adds a local transport lifecycle callback but no server event. Surviving browser clients clear stale live routing on disconnect and hydrate on reconnect; a new epoch supersedes an old process's higher revision. The existing generic web `broadcast_event` command is intentionally narrowed to the three client-owned UI events.
- Persistence continues to derive `was_active` from the selected ID, now from the same aggregate manager snapshot as the rows. Dormant persisted selection remains supported.
- No config migration or new persisted file is introduced.
- No new dependency is introduced.
- Coordinator overload/shutdown errors are deliberate internal command-contract changes: only exact `selectionCoordinatorBusy` is retryable, while `selectionCoordinatorUnavailable` and `selectionCoordinatorRecursiveSubmission` are terminal for that request. Bootstrapping can return Busy only until the mandatory restore job is first in the FIFO.

### 7.2 Security and safety

- The fix removes the path that can silently route subsequent keyboard input to an unrelated agent, which is the primary safety improvement.
- Event source and user intent are derived from sealed backend causes. Browser clients cannot submit a source string or use generic event broadcast to make a background action look user initiated or forge a higher selection revision.
- The three remaining client-owned broadcast events accept only their exact deny-unknown-fields payloads; an allowlisted name is not permission to forward arbitrary JSON.
- Tauri and WebSocket JSON is decoded from `unknown` once at the shared frontend boundary. Malformed mode/status/liveness combinations are rejected before reaching writable state.
- The new payload reveals only session ID, status, and runtime booleans already inferable from existing session/window behavior. It does not expose tokens, prompts, cwd, command lines, or credentials.
- No filesystem, network, shell, PTY-input, or Tauri permission surface is added.

## 8. TypeScript rule scope and adoption finding

The complete `apply-typescript-best-practices` skill and all 636 lines of its `TypeScript_best_rules.md` reference were read before any TypeScript/TSX inspection.

The reference's 15-rule features/domain/application/ports/adapters/ui architecture profile is optional and applies only when an applicable ADR or `AGENTS.md` explicitly adopts it. No repository-scoped `AGENTS.md`, ADR, profile adoption manifest, dependency-cruiser configuration, architectural ESLint configuration, or feature-layer registry adopts that profile. The workgroup `AGENTS.md` governs agent conduct and the Dev-Rust role but does not adopt the optional frontend profile. The current frontend is the flat `src/{shared,sidebar,terminal,main,...}` structure, `tsconfig.json` has `strict: true`, and `package.json` provides `typecheck` and Vitest but no ESLint/dependency-cruiser gate.

Decision:

- Do not claim optional architectural-profile conformance.
- Do not reorganize this fix into feature/domain/ports/adapters folders or add the profile toolchain.
- Apply the relevant repository-compatible TypeScript obligations: strict mode-and-cause discriminated types, one fresh-object runtime decoder for untrusted selection IPC/WebSocket JSON, matching Rust/TypeScript camelCase fields, named exports for the new reusable contract/decoder APIs while preserving established component defaults, no unsafe double assertions, typed IPC wrappers instead of component-level transport calls, exhaustive mode/source handling, connection-generation-first stale-async guards, revocable async voice continuations, disposal-safe listeners, and objective `tsc` plus Vitest verification.
- Do not claim an ESLint or dependency-cruiser result that the repository has not adopted. Do not add those tools in this issue.

## 9. Implementation order

All phases belong to one complete #1027 delivery. Do not merge or close #1027 after only the manager invariant.

### Phase 1: MVP foundation

1. Consolidate `SessionManagerState`; add the Rust selection contract/coordinator, sealed cause and commit capabilities, managed-handle-free typed jobs, exact 65/64/16 admission, three typed critical waiter kinds, Bootstrapping/Running/Closing phases, exact recursive/overload errors, five-second joined shutdown, process epoch/revision, aggregate snapshots, logging, and atomic `commit_selection_transition` primitive.
2. Make manager create/removal policy-free; add pending-create state, reserved finalization tickets, atomic `FinalizeCreate`, trusted create intent, and selection precondition; remove unconditional `Active` writers; migrate persistence to the aggregate public snapshot; add manager/selection/persistence/source-sentinel unit tests, including #889 cases.
3. Add the matching TypeScript discriminated payload, runtime decoder, and decoded IPC hydration/listener contract so the branch continues to typecheck as backend emitters migrate.
4. Convert manual single destroy and explicit switch to the coordinator. Pin the decided manual eligible fallback.

### Phase 2: Full features

5. Refactor shared destroy into typed single/batch transactions and migrate retained Root, coordinator cascade, auto-close, delivery, mailbox, and spawn rollback sources.
6. Make restart selection-atomic with deferred replacement events and distinct dormant-wake failure behavior, including background/bulk behavior. Update config/mailbox callers with explicit user intent.
7. Enqueue restore first and only then start every lifecycle producer listed in section 4.5. Migrate restore as one startup transaction, detach/attach with window/runtime/persisted-intent compensation, command/watchdog resource finalization, container transport-loss reconciliation including synchronous outer-lock deferral, the container spawn-cancellation guard, and the screenshot live-selection consumer.
8. Remove web re-derivation/double broadcast; centralize create/destroy/row publication; deny forged lifecycle broadcasts; add connection disconnect/generations and reconnect hydration; prove native/WebSocket parity.
9. Implement terminal-store and TerminalApp generation/epoch/revision reconciliation with pre-list binding suspension, then sidebar exited-row-safe reconciliation. Add revocable voice leases and all row gates. Remove secondary selection owners in `LastPrompt`, created, and destroyed listeners.

### Phase 3: Polish

10. Update Home/central listener comments and typed fixtures, architecture docs, and auto-close docs.
11. Add structured transition/rejection logging assertions and failure-path diagnostics.
12. Run focused tests, then the full Rust/frontend regression gates and formatting checks.

### Phase 4: Extras

No extras are authorized in this issue. Log-retention separation and focus-as-activity remain follow-ups.

## 10. Tests and objective verification

### 10.1 Rust unit and integration tests

Add or update tests in these exact areas:

- `session/manager.rs`
  - Inserted records are atomically pending, do not appear in normal list/persistence/candidate snapshots, and do not auto-select or become `Active` before successful `FinalizeCreate`.
  - Live `FinalizeCreate` requires the matching pending non-exited PTY-backed record, records actual detached state, and removes pending state atomically with optional attached selection. Restore-only dormant finalization preserves its exact exit code without PTY. Missing, already-finalized, live-PTY-less, remove+finalize, separate-exit+finalize, and selection-of-detached-finalized cases reject without partial mutation.
  - Ordered `[exited_root, active_live]`, automatic removal of active: selected ID becomes null, Root remains `Exited`, and Root is never written `Active`.
  - Commit live rejects exited, missing-record, missing-PTY witness, and detached witness.
  - Explicit dormant commit preserves exact exit code.
  - Previous live selection demotes to `Running` only through the central commit.
  - `MarkExited` plus selected dormant/clear decision updates record, raise-hand state, stored payload, and revision in one snapshot; selected `Keep` is rejected.
  - Duplicate/out-of-order `MarkExited(id, later_code)` preserves the first exit code, returns unchanged row/selection outcomes, and emits no revision; remove+exit overlap, duplicate removals, conflicting exit codes, and removed target mutations reject atomically.
  - The manager's direct exit fixture is compiled only under `#[cfg(test)]`; production restore uses dormant `FinalizeCreate`, and the ownership sentinel finds no callable production exit writer.
  - Revisions strictly increase on changes, no-op does not increment, overflow does not wrap.
  - Aggregate record/order/selection snapshots cannot observe half of a concurrent commit.
  - `SetDetachedIntent` changes only persisted detached intent, never geometry; detach/attach combines it atomically with the selection decision, and an idempotent repeat is a no-op revision.
- `session/selection.rs`
  - In `Bootstrapping`, every public, critical, and create-ticket submit except the single `submit_restore_first` returns exact `Busy` without allocating a waiter, ticket, permit, or queue slot. `submit_restore_first` is accepted exactly once at FIFO position one, changes the phase to `Running`, and later producer submissions follow it.
  - FIFO worker serializes a blocked lifecycle job, a user switch, and hydration in submission order without any state lock guard spanning an await; graceful shutdown rejects not-started ordinary work, cancels ticketed create bodies, drains their reserved rollback finalizers, lets a destructive running transaction reach its consistency finalizer, resolves every oneshot/ticket, and leaves no worker task.
  - A barrier-held transaction that exceeds the shutdown budget is aborted and joined before aggregate persistence; every caller receives unavailable, no post-persist mutation occurs, and the persisted manager snapshot is internally coherent even though the log records the interrupted external phase.
  - Filling the 65-job running/queued/reserved admission budget makes an external request fail immediately with `Busy` and creates no sender-admission waiter; reserved create tickets reduce available queue capacity, and one typed critical waiter is admitted fairly after capacity returns. Flooding the same `(id, kind)` returns typed `AlreadyPending` without allocating waiters, while route-loss/watchdog/background-cleanup keys for the same ID remain distinct and execute their own idempotent policies. A missing manager ID creates no waiter. Completion, missing-ID rejection, admission failure, and shutdown remove each key so a later genuine exit is admissible.
  - Hold 16 create tickets without finalizing: a seventeenth create fails before side effects, while a normal transition and critical reconciliation can still use unparked queue capacity. Ticket completion/drop releases both sub-budget and general admission exactly once.
  - From the worker task-local context, every public/critical enqueue method, create reservation, and narrow container lifecycle sender returns exact `selectionCoordinatorRecursiveSubmission`; no path allocates, waits, or deadlocks behind itself.
  - A source-ownership sentinel parses the private `CoordinatorJob` declaration and rejects any managed-state handle or arbitrary closure/future field. A drop test retains only weak references to coordinator, PTY/container state, and the publication handle, closes/joins the worker, and proves all become collectable.
  - Stable candidate order skips pending-create, excluded, exited, PTY-less, and runtime-detached records.
  - Payload invariant table for none/live/dormant, complete cause/source/mode/user table, stable UUID epoch, and exact camelCase source serialization. Each fixed source/user mapping has a mutation case with the boolean or source changed one step.
  - Barrier-controlled PTY loss after initial eligibility but before final commit rejects the candidate without manager mutation/revision and tries the next permitted candidate. PTY loss immediately after a valid commit queues exactly one later `livenessReconcile` revision in event order.
  - A user switch whose target loses its PTY at the final barrier preserves an unrelated still-live selection. If the target was the stale current selection, it clears exactly once with `livenessReconcile`/false and returns an error; no `userSwitch`/none payload is constructible or decodable.
  - Structured rejection reasons are stable.
- `commands/session.rs`
  - User create under unchanged `None` reserves before side effects and publishes `sessionCreated` with `userInitiated=true`; loop/mailbox/CLI-request create uses `false`; restore-time Root auto-create and restart suppression publish nothing. A create started under `Live` receives no auto-select precondition, still finalizes/publishes its public row, and cannot select itself after that selection clears. Capacity exhaustion fails before `mark_spawning`, archive/coordinator-clock/resource/config/credential mutation, manager insertion, or local/container spawn.
  - The reserved finalizer is the sole persistence and create/selection publisher: success persists once then emits one dual-transport created row and optional selection in order; each outer caller emits/persists zero duplicates.
  - A non-Root create captures Live A/revision N, blocks in spawn with one reserved ticket, auto-close clears A at N+1, then create completes: its stale precondition cannot select the new record. A `None` changed away and back with a higher revision is likewise rejected.
  - Hold a create after its PTY appears but before its reserved finalizer, then manually close the selected session and take a persistence/list snapshot: the pending ID is neither fallback nor visible/persisted. Release finalization and prove `session_created` precedes any later eligibility/selection of that row.
  - Fail before pending insertion and prove the reserved slot is released without queuing a finalizer. Cancel, panic, or trigger shutdown after record insertion and after PTY spawn; the ticket's reserved nonblocking rollback removes the record/route, emits no ghost row or selection, releases capacity once, and does not wait on a full sender. Finalization that discovers a removed record or lost route performs the same cleanup.
  - Concurrent top-level Root calls plus startup restore serialize through the coordinator without the removed Root mutex, create/reuse at most one live Root, and contain no recursive `run` call.
  - Exact Busy propagation: filesystem wake is left retryable/not delivered, inline wake is rejected, loop delivery has no success audit and retries only on its normal schedule, and GUI session-request JSON survives for the next poll. A later admitted retry creates once; non-Busy session-request failure retains current delete behavior and every failed delete logs its path.
  - Manual close retains first eligible live attached fallback and skips dormant/no-PTY/detached records.
  - Background cleanup of selected record returns null with no fallback.
  - Delivery/mailbox/stale-Root cleanup gains or reuses coordinator admission before teardown; a barrier proves no selected record/PTY disappears while its background cleanup job is merely waiting.
  - Retained Root close keeps exited record and selects only eligible manual fallback.
  - Closing a canonically selected dormant Root detects selection by ID, not `Active` status, preserves a nonzero exit code, exposes close/fallback behavior, and publishes the decided fallback/null exactly once without PTY input.
  - Multi-member manual batch emits destroyed events per success and one final selection event. Barrier cases finish a pending create's PTY while the batch is held, remove its route before finalization, lose/detach an existing public candidate, and fail a planned selected member. The pending create is never ranked or announced early, its finalizer runs only after the batch, and the final fresh scan never selects a stale/planned member.
  - Restart success captures events and observes no intermediate fallback; only ready replacement is selected.
  - Restart static/pre-teardown failure leaves the live old record, PTY, Telegram state, selection, and event stream unchanged. Failure after teardown ends with one old destroy plus null when old was selected and preserves unrelated selection otherwise.
  - Dormant wake failure retains the selected old dormant Root/agent and exact exit code with no create/destroy/selection event. A spawned replacement that fails final validation was never announced and leaves no sidebar ghost; success publishes created-new, destroyed-old, selection exactly in order.
  - Agent self-clear restart transfers the prior communication value into the still-pending replacement before `FinalizeCreate`; the one created-row payload already contains it, and no separate `session_communication_changed` event or post-return direct manager mutation occurs.
  - Spawn rollback of the first precreated record emits no selection in the normal path.
- `session/auto_close.rs`
  - Reproduce old persisted anchor, member below then above 30-second wake grace, dormant Root first in order, and selected live WG member.
  - Run current late rechecks through the real deferred-destruction batch seam.
  - Assert exactly one full `{ id:null, source:autoClose, userInitiated:false, ... }` payload and no Root/sibling selection.
  - Multiple stale members produce no intermediate selection events.
  - A full coordinator defers one tick before any member recheck/teardown, creates no waiting task, and the next tick can admit and close normally.
  - Existing 25 anchor/wake-grace tests remain unchanged and green.
- `commands/window.rs`
  - Detaching a nonselected session preserves selection.
  - Detaching selected session skips exited/no-PTY/detached fallbacks.
  - Attaching live selects live; attaching exited selects dormant; non-exited/no-PTY rejects without `Active`.
  - Coordinator `Busy` occurs before create/destroy-window, runtime detached-set, or `was_detached` mutation; a window failure inside an admitted job likewise leaves all four state surfaces unchanged.
  - Detach with a window that vanishes, or a PTY that disappears, between creation and commit destroys the new window, removes runtime membership, leaves persisted detached intent false, and performs only the required liveness repair. Attach with post-destroy PTY loss removes runtime membership, clears persisted intent, and commits dormant/null liveness state without a stale live payload. Successful attach preserves the stored detached geometry byte-for-byte.
- `commands/resource_monitor.rs`
  - Verified selected kill publishes destroyed-for-cache, exited row, then null with the correct user flag; a locked detached window closes on the first event.
  - Nonfinalized kill does not change selection or revision.
  - A full coordinator returns `selectionCoordinatorBusy` before invoking user `kill_group`, PTY teardown, or manager mutation.
- `resource_monitor/watchdog.rs`
  - Verified watchdog kill uses `userInitiated=false`, tears down/marks exited, and clears a selected ID once; quarantined/terminating/error outcomes do not mutate it.
  - Concurrent user and watchdog finalizers for one ID produce one row mutation/selection revision and preserve the first exit code regardless of completion order.
  - A full queue creates one deduplicated watchdog waiter before kill; duplicate ticks allocate no waiter and call no kill, then capacity release runs exactly one whole kill/finalize job. AppShutdown cleanup after coordinator join emits no selection lifecycle event.
- `testability/ui_automation.rs`
  - Backend `resourceMonitor.watchdog` kill/tick mode leaves the response pending while the coordinator job is barrier-held, then writes the unchanged diagnostics shape only after the same destroyed/row/selection finalization; it never calls direct `kill_group` or runtime `block_on`.
  - Sample/warn modes enqueue no lifecycle job, and shutdown/disposal completes the response with a diagnostic unavailable error rather than leaking an automation task or inflight file.
- `pty/container_backend.rs`
  - Selected async transport close and pending-session reap remove the route, atomically mark exited, then publish destroyed-for-cache, exited row, and one `livenessReconcile` dormant payload with the exact ID and exit code.
  - Selected non-exited/no-route inconsistency clears to `None`; nonselected transport loss emits destroyed-for-cache plus exited row but no selection revision. Duplicate exit emits no lifecycle event and cannot close/recreate the row twice.
  - With a real `Arc<Mutex<PtyManager>>` guard held as production holds it, forced synchronous queue-full/closed teardown returns under a timeout, then removes the outer route and runs the lifecycle handler exactly once after guard release. This test must deadlock if outer-route removal is moved back into the synchronous callback.
  - Hold `runtime.start` behind a barrier, cancel the ticketed create, then return a late container handle: the shared cancellation guard stops that handle, removes pending/attaching state, token, credentials, and logical slot exactly once, installs no route, publishes no row/selection, and leaves no process after coordinator join. Repeat cancellation during handshake after handle installation.
- `config/sessions_persistence.rs`
  - A barrier-controlled persistence snapshot racing a selection/removal commit sees either complete old state or complete new state, never a missing selected ID or mismatched `was_active`.
- `screenshot/windows.rs`
  - Unit-test `resolve_live_capture_session`: `None` and `Dormant` fail before OS capture with the existing no-active error; `Live` resolves the matching record. Existing capture tests retain current behavior.
- `web/commands.rs`
  - Tauri event and WebSocket receiver observe identical payload bytes/fields and revision for the same transition.
  - Web destroy/switch emits selection once, not twice.
  - Web hydration returns the same revision snapshot as native manager state.
  - A background/native create and retained-Root row refresh reach WebSocket `session_created` exactly once; browser state is not dependent on the command response.
  - Client `broadcast_event` accepts only `theme_changed`, `resource_monitor_attach`, and `open_settings`, each through its exact `#[serde(deny_unknown_fields)]` payload struct. For each name, wrong type, nonobject, null, missing field, and extra field fail. A valid-looking forged `session_switched` using the real epoch/high revision, plus forged created/destroyed variants, returns an error and reaches neither Tauri nor WebSocket listeners.
- `lib.rs` restore tests
  - Persisted dormant target restores dormant and never `Active`.
  - Persisted detached target falls back only to eligible live attached candidate.
  - Restore publishes at most one selection transition and shares its revision with subsequent hydration.
  - Archived persisted target is never adopted.
  - A user switch and hydration submitted while restore is blocked remain queued; hydration sees final restore state and the later user switch wins in FIFO order. No restore helper recursively enqueues.
  - Zero, one, and multiple persisted `was_active` flags are normalized deterministically: exactly one valid flag is the exact target; multiple flags are logged as inconsistent and use the documented eligible-live fallback rather than loop-order last-wins.
  - Restore finalizes/persists/publishes rows before creating each locked detached window; that window's immediate `list()` contains its exact row, while canonical restore selection remains suppressed until post-reconstruction finalization.
  - Barrier each container reaper, resource watchdog, web/API create request, mailbox/loop request, auto-close/non-stop watchdog, automation request, and screenshot hotkey at its first possible lifecycle action; none can submit or mutate before `submit_restore_first`, and each admitted action is ordered behind restore. Early native/browser hydration receives exact Busy and its single capped retry succeeds after Running.

Run a compile-caller audit with `rg -n "create_session\(|destroy_session\(|switch_session\(|set_active_only\(|clear_active(_if)?\(|get_active\(|mark_exited\(" src-tauri/src src-tauri/tests`. Update test-only manager fixtures in manager, persistence, auto-close, discovery, entity/config/PTY/loops/phone/container/git-watcher/workgroup diagnostics, `src-tauri/src/api/handlers/session_transport.rs`, `src-tauri/tests/pty_lifecycle_regression.rs`, and `src-tauri/tests/wake_consumption_measure.rs` for the policy-free manager, new coordinator/PtyManager constructors, and typed source/intent signatures. Every production exit must be coordinator lifecycle-reconciled; restore uses dormant finalization and direct exit calls are test-only before `cargo check --all-targets` is accepted.

Also record `rg -n "kill_group\(|kill_all_owned_groups\(" src-tauri/src --glob '*.rs'`. Production `User`, `Watchdog`, `SessionDestroy`, `SpawnRollback`, and `AppShutdown` callers must each match the policy matrix: user/watchdog whole coordinator jobs, destroy/rollback inside an existing transaction, and app shutdown only after coordinator join. A direct automation/watchdog kill outside those exact symbols fails the caller audit.

Add a mutation-sensitive source-ownership sentinel that scans production Rust sources and fails if (a) an assignment `= SessionStatus::Active` exists anywhere except the single manager commit site, (b) a production `session_switched` emit/broadcast exists outside the authoritative selection publisher, (c) `session_created`/`session_destroyed` from a migrated lifecycle path exists outside the shared lifecycle publisher, (d) `session_communication_changed` from a migrated path exists outside the shared row publisher or the exact named preexisting nonselection producer allowlist, (e) removed manager mutators are compiled outside `#[cfg(test)]`, (f) any direct production `mark_exited` remains, or (g) the web client-broadcast allowlist admits a backend lifecycle name. The test allowlists file plus enclosing symbol, not line number or count, and its mutation fixture injects one direct emitter for each lifecycle name into a migrated command and requires failure. The final review records the exact allowed hits from `rg -n 'SessionStatus::Active|session_(switched|created|destroyed|communication_changed)|mark_exited|broadcast_event' src-tauri/src --glob '*.rs'`; a count-only assertion is insufficient.

### 10.2 Frontend tests

- `src/terminal/App.workflow.test.tsx`
  - Hydrate from full selection payload.
  - Null selection clears header, shell, task, prompt selection, xterm, and input.
  - Dormant Root renders exact wake guidance and issues no snapshot, resize, write, voice, or task IPC.
  - Emit live B while A is bound and hold B's `list()`: A's metadata/input clears synchronously and neither A nor B receives write, resize, snapshot, voice, task, or automation IPC while pending. Binding B restores operations only after the guarded result.
  - Emit live A revision 1 then live B revision 2 in one epoch; resolve B's list first and A's last; B remains bound. Existing A/B/A xterm-cache/snapshot-dedup behavior stays green despite the pending null route.
  - A live payload whose matching list row is exited never binds. A running/idle row for the matching ID can bind because the newer selection payload is authoritative.
  - Disconnect clears live binding before reconnect. Transient reconnect accepts the exact same epoch/revision once for the awaited generation and restores the same live session. A duplicate equal snapshot, an older-generation completion, and an equal snapshot after a newer event are ignored.
  - Fill the coordinator queue, return typed busy from reconnect hydration, and prove only one capped-backoff retry request/timer exists. An accepted event, disconnect, generation change, and unmount each cancel it; after capacity returns, the current-generation retry restores the snapshot.
  - Apply old epoch revision 500, reconnect, then hydrate new epoch revision 0; stale routing clears immediately and the new process wins. Also hold the first hydration so its old epoch was never applied, reconnect/apply the new epoch, then resolve the older-generation hydration: generation-first rejection still wins.
  - Unmount while selection/connection listener registration and `list()` are unresolved; every late unlisten runs and no completion mutates the terminal store.
  - Non-null ID absent from list clears the old terminal.
  - List rejection for a newer selection clears the old terminal.
  - While live A is bound, deliver `session_destroyed(A)` and hold the later authoritative selection event: A's cache and writable binding clear synchronously, no input/snapshot/resize/voice/task call can target A in the gap, and no selection hydration/list query or fallback derivation occurs. The final selection payload alone binds or clears.
  - A locked detached window binds only its exact live row, ignores central null/dormant/fallback events, and still closes/disposes on matching `session_destroyed`; the central refactor does not route it through `selectionId`.
  - `session_created` alone never selects.
  - `LastPrompt` does not relist on selection.
- `src/sidebar/stores/sessions-helpers.test.ts`
  - Live reducer promotes only live target and demotes prior active.
  - Dormant reducer preserves exited status and exit code.
  - Null clears highlight.
  - Older/equal revision is ignored.
  - New epoch revision 0 supersedes old epoch revision 500, and the retired old epoch cannot return.
  - Same-process reconnect may reapply an equal selection only for the currently awaited generation; normal equal payloads remain ignored.
  - Late list/upsert reapplies the newest stored selection, except an exited upsert for the stored live ID remains exited and clears visible active state.
- Add `src/sidebar/App.selection.workflow.test.tsx`
  - Listener-before-hydration race: event revision 2 wins over hydration revision 1.
  - Created-first-row heuristic is gone.
  - Destroy removes row only; subsequent authoritative null drives selection.
  - Retained Root event order (`destroyed`, exited `created`, final selection) never shows Root as frontend active between events, even when the stored payload was previously live.
  - Resource/container first-exit uses the same destroyed/exited-row ordering, closes a locked detached window, and never re-promotes the exited row; a duplicate exit produces no second remove/upsert cycle.
  - Browser background `session_created` adds the row before its selection event; a missing-row live selection becomes highlighted when the later upsert arrives.
  - Disconnect, destroyed event, and exited upsert revoke the matching voice operation. Unmount-before-listener-resolution leaves no connection/selection listener.
- Add `src/shared/session-selection.test.ts`
  - Decode one valid payload for each allowed mode/source/user combination.
  - Reject missing/extra/inherited/accessor keys, non-plain records, unsafe/negative revisions, invalid IDs/exit codes, and every false-positive or false-negative mode/status/hasPty/detached/displayable combination.
  - Table-drive a one-mutation-deep negative for every fixed source: flip only mode, source, or `userInitiated` and require the decoder to reject in the target file/diagnostic path.
  - Add `satisfies SessionSelection` compile fixtures for valid variants and targeted `@ts-expect-error` fixtures proving wrong source/mode/user combinations are rejected without a cast.
- Add `src/shared/transport-ws.test.ts`
  - Each accepted `onopen` increments local connection generation exactly once, `onclose` reports disconnected once, superseded socket messages/callbacks are ignored, reconnect keeps existing subscriptions, explicit close stops notifications, and no lifecycle notification is sent over the socket.
  - Open generation 1 before registering an app listener; the synchronous snapshot still starts initial hydration at generation 1. Race an open/close between listener registration and snapshot read and prove the greater current generation/state wins without duplicate hydration.
  - Replay an old-epoch valid selection on the new socket generation and prove transport delivery cannot bypass store epoch/generation rejection.
- Add IPC wrapper tests proving `getSelection` and `onSessionSwitched` both decode `unknown`, reject malformed payloads before callback/store mutation, expose connection snapshots plus connected/disconnected generations without affecting Tauri fakes, and retry only the exact `selectionCoordinatorBusy` rejection (not unavailable, arbitrary strings, objects, or `Error` instances).
- Add `src/shared/voice-recorder.test.ts` plus sidebar component workflow cases: destroy/dormant/disconnect at each await boundary (`getUserMedia`, transcription, typing check, settings delay) cancels resources and causes zero delayed `pty_write`/Enter calls; dormant `SessionItem` and Root rows expose close/wake but no mic/detach/Telegram control, direct PTY-dependent handler invocation is also gated, and closing an already-dormant selected Root preserves its exit code while applying exactly one manual fallback/null transition.
- Add `src/shared/shortcuts.test.ts`: live selection enables close and voice; dormant selection enables close but never voice; null selection enables neither; rejected selection hydration/close is caught once with no unhandled rejection.
- Add coordinator-close/component rejection cases proving noncoordinator destroy, Root dormant destroy, and coordinator cascade failures are caught/logged once, leave the busy/confirmation UI consistent, and do not dispatch a second lifecycle call.
- Update `src/main/listeners-home.test.ts` and `listeners-central-view.test.ts` with complete mandatory payloads and retain user-initiated-only behavior.
- Audit `rg -n "get_active_session|session_switched" src --glob "*.test.ts" --glob "*.test.tsx"`. Update full decoded payload fixtures in browser `App.workflow`, sidebar messaging/profile-drift/context-template/session-env-warning/raise-hand workflows, terminal App/spawn-size/render-gate workflows, main listener tests, shared shortcuts tests, and shared fake-transport tests. Event fakes must include epoch and the complete discriminated variant; no `as unknown as` escape is permitted.
- Keep all existing live snapshot reconciliation, resize retry/dedup, Root TASK visibility, Home, central resource-monitor, and sidebar exited-status tests green.

### 10.3 Commands

Run focused tests during development, then all final gates:

```powershell
npm run typecheck
npm test -- src/shared/session-selection.test.ts src/shared/transport-ws.test.ts src/shared/voice-recorder.test.ts src/shared/shortcuts.test.ts src/terminal/App.workflow.test.tsx src/sidebar/App.selection.workflow.test.tsx src/sidebar/stores/sessions-helpers.test.ts src/main/listeners-home.test.ts src/main/listeners-central-view.test.ts
npm test
npm run test:debt
npm run build
```

From `src-tauri`:

```powershell
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib --bins --tests
```

No ESLint or dependency-cruiser command is part of acceptance because the repository does not configure or adopt either gate.

## 11. Objective acceptance criteria

The implementation is accepted only when all of the following are objectively true:

1. Auto-close destroying the selected session publishes exactly one authoritative null selection with the process epoch, `source=autoClose`, `userInitiated=false`, and a newer revision; it never selects an unrelated record.
2. Pending-create, exited, non-exited/no-PTY, planned-for-destruction, and runtime-detached records are never automatic fallback candidates.
3. Manager removal never chooses a replacement and never writes another record `Active`.
4. The source-ownership sentinel proves the only production `Active` assignment and exit mutation are inside the manager commit and the only `session_switched` publisher is authoritative; no direct callable production exit writer remains. Sealed cause/commit/witness types prevent a caller from forging source, user intent, or PTY eligibility.
5. Explicit dormant selection and dormant Root close preserve `Exited(code)` and produce a non-displayable dormant/final payload as applicable. Duplicate or out-of-order exit callbacks preserve the first code and emit nothing twice.
6. Manual close selects the first remaining eligible live attached PTY-backed record in **final** stable manager order, or null when none exists; barrier-controlled batch churn cannot commit a stale/planned candidate.
7. Multi-member destruction produces no intermediate sibling/root selections and at most one final selection event.
8. A create ticket stores `Some(epoch, revision)` if and only if selection is `None` at reservation and otherwise stores no auto-select precondition. The former auto-selects only against that unchanged `None`; the latter still creates/publishes its row but never selects it, including after later auto-close/cleanup null. Its pending row cannot be listed, persisted, explicitly selected, detached/attached, captured, or used as fallback before atomic finalization and `session_created` publication, and the finalizer is the sole persistence/event owner.
9. Restart success exposes no fallback and publishes deferred created-new, destroyed-old, then only the ready replacement when selection should move. Pre-teardown failure leaves the live old session/resources untouched; post-teardown failure publishes null only when the removed old record was selected; dormant-wake failure retains the old dormant record and emits no ghost row.
10. Session create, startup restore, detach, attach, spawn rollback, command/watchdog/automation resource kill, container liveness loss, explicit switch, and background cleanup follow the policy matrix and use the central coordinator. Restore is FIFO job one before any lifecycle producer starts. SessionDestroy/SpawnRollback remain in their outer transaction, and AppShutdown runs only after coordinator join with no selection event. Detach/attach compensates window/liveness races without changing geometry. The container synchronous queue-full/closed path cannot re-enter the caller-held `PtyManager` mutex, and canceled blocking container spawn/handshake cannot install or leak a late handle.
11. The coordinator admits at most 65 running/queued/reserved jobs while the MPSC remains exactly 64; at most 16 slots may be parked by slow create tickets, leaving at least 48 physical slots unparked. Create capacity and every destructive command admission precede side effects; external overload fails fast; typed critical route/watchdog/cleanup work is same-kind deduplicated, fairly admitted, and not dropped; recursive submission fails rather than hangs; and five-second capped shutdown closes, drains or aborts, and joins with no worker, ticket, critical key, or unresolved oneshot. Queued jobs are private typed data/oneshot variants with no arbitrary future, closure, or managed-state handle, so queue ownership cannot retain the coordinator.
12. Tauri and WebSocket clients receive each changed selection/create/destroy/row event once in the specified order and hydrate from the same epoch/revision state. Client `broadcast_event` cannot forge a backend lifecycle event, and each of its three UI event names rejects null, nonobject, wrong, missing, or extra payload fields.
13. Transport JSON is accepted only through the fresh-object exhaustive runtime decoder; one-mutation-deep source/mode/user/liveness negatives all fail before store callbacks.
14. Connection generation is checked before epoch/revision. Disconnect clears routing, older-generation initial/reconnect hydration cannot apply even when its epoch was never seen, a new epoch revision 0 supersedes an old high revision, and retired/replayed epochs cannot return on a new socket.
15. Accepting live B suspends live A before metadata lookup, and a matching destroy event safety-suspends A before the subsequent final selection event without deriving a replacement. Pending, destroyed, missing, failed, or exited state leaves no writable binding. Stale async list/hydration completions and disposed component listeners cannot mutate state.
16. Null selection clears the central terminal and all routing metadata; dormant selection shows wake guidance. A sidebar live payload never promotes a later exited row.
17. Dormant/missing/pending/disconnected selection causes no screen snapshot, PTY resize, PTY write, voice, automation input, or task IPC. Destroy/exit/rebind revokes delayed transcription and auto-execute writes, and every sidebar PTY-dependent control is live-gated while dormant close/wake remains available.
18. Existing live PTY snapshot/resize/cache behavior and detached-terminal cache disposal remain green.
19. Existing inactivity, badge, repaint-grace, wake-grace, timeout, and Telegram-protection tests remain green; passive focus/view remains non-activity.
20. Selection logs contain epoch, old/new IDs, cause/source, revision, statuses, PTY presence, detached state, result, and stable rejection reason; no changed fallible lifecycle operation is silently ignored or uses a new production `.unwrap()`.
21. One manager snapshot supplies finalized persistence rows plus selected ID, excludes pending creates, and concurrency tests prove no torn `was_active` state or crash ghost. Shutdown persists only after the coordinator is joined/aborted, so no ticket or late blocking spawn can publish or mutate afterward. Restore deterministically handles corrupt multiple active flags. Screenshot capture rejects dormant/null selection and accepts live selection only.
22. The same implementation subsumes #889's unconditional-writer invariant. #1027 is not considered resolved until the backend, transport, frontend, restart, security, and UX criteria above all pass.

## 12. Step 5 developer validation record (historical input)

The Step 5 code audit retained the architect's core decisions: full-path scope, passive focus/view as non-activity, automatic-close null policy, manual eligible fallback, explicit dormant mode, one backend coordinator, dual-transport publication, and no optional frontend architecture-profile adoption. It added these implementation-critical corrections:

- Process-local revision alone was unsafe for a browser surviving backend restart. The payload now has a process UUID epoch, and WebSocket connection generations trigger safety-clear plus hydration.
- The flat TypeScript interface admitted impossible states, and transport generics did not validate JSON. The contract is now a discriminated union decoded once from `unknown`.
- Separate manager locks and persistence reads could expose torn selection/record state. Manager cross-fields now share one state lock and one aggregate persistence snapshot.
- A Tokio mutex held across async teardown conflicted with the repository lock rule. One FIFO coordinator worker now serializes whole async jobs without any lock guard spanning an await.
- Committed-then-rolled-back provisional selection could leak to persistence. Eligibility retry now precedes the one final commit; later route loss is a distinct `livenessReconcile` transition.
- `sessionCreated` lacked caller intent. Trusted Rust `CreateSelectionIntent` now distinguishes user, background, and suppressed creation.
- Container transport-loss `mark_exited` paths and screenshot capture's raw `get_active` assumption were hidden production consumers. Both have explicit coordinator-aware behavior and tests.
- The TypeScript reference currently contains 636 lines, correcting the earlier 597-line statement.

No feature/domain/ports/adapters reorganization, new dependency, inactivity semantic change, or certification metadata is introduced by this enrichment. No implementation choice remains unresolved.

## 13. Step 6 adversarial validation record

The Step 6 audit read the full plan, both issue contracts, and the current branch callers/events. It retained Step 5's central state/coordinator/epoch decisions but corrected these concrete failure paths:

- The bounded MPSC did not bound tasks waiting on `send().await`; bounded admission, critical per-session coalescing, shutdown, and recursive-call failure semantics are now explicit.
- Container synchronous close could call a route remover that re-locks the already-held outer `PtyManager` mutex. Outer route removal and reconciliation are now deferred on that path and pinned by a real-lock timeout test.
- Resource-watchdog and enabled UI-automation watchdog `kill_group` calls were omitted production callers. Both now gain coordinator admission before kill and use the same whole idempotent resource finalizer as the user command; app-shutdown kill remains intentionally selection-silent after coordinator join.
- Web `broadcast_event` accepted arbitrary event names, so a browser could forge selection cause/epoch/revision. It is now an exact three-event UI allowlist, and lifecycle forgery is a required negative test.
- `session_created` was Tauri-only in the shared create path. Changed create/row/communication events now use the same dual publication as selection/destruction so browser sidebars cannot miss the selected row.
- A create that began under a live selection could finish after auto-close and adopt the new null. Create auto-selection now stores a compare-and-set precondition only when reservation observes `None`; a create begun under any non-null selection stores no auto-select precondition.
- Long teardown used a stale fallback list. Finalization now re-scans final manager order and has barrier tests for member creation/removal/exit/detach and selected-member failure.
- Restart announced a replacement before final liveness validation and treated an already-dormant wake like a destroyed live PTY. Restart-only events are deferred; dormant wake failure retains the old record/code; pre/post-teardown failures have distinct evidence.
- The terminal could leave live A writable while awaiting metadata for live B and could bind an exited row from a stale live payload. Accepted live transitions now suspend routing first and reject exited rows without destroying the existing xterm cache on every successful switch.
- Voice recording/transcription/auto-execute could outlive selection or session destruction, and `SessionItem` gated PTY controls only on synthetic inactive IDs. Voice operations now carry revocable leases and every row surface has handler-time/live rendering gates.
- Sidebar upsert reapplication could promote an exited retained Root from an older stored live payload before the final selection event. An exited row now wins safely for an immutable session ID.
- Epoch retirement alone did not reject an older-generation hydration whose epoch had never been applied, and async listener registration could leak after unmount. Connection generation is checked first, disconnect clears immediately, newer events cancel equal-rebind permission, and registration/completions are disposal-safe.
- Separate source/user inputs, duplicate exit callbacks, and manual grep-only ownership left mutation holes. Sealed causes/capabilities, first-exit idempotence, exact source-mode decoder negatives, and a production-source sentinel now enforce them.
- A non-Root create could perform row/PTY side effects and then lose fail-fast admission for its selection/rollback finalizer, while nesting the current Root mutex with the coordinator would deadlock restore against a concurrent Root command. Create now reserves bounded finalization capacity before side effects, cancellation spends that slot on rollback, and all Root reuse/create/wake work is serialized by the coordinator with the old Root mutex removed.
- A spawned create was still a normal `Running` manager row before finalization. Another manual fallback or persistence snapshot could adopt/persist that unannounced PTY before `session_created`. Manager-owned pending-create IDs now hide it from every public/candidate/persistence projection until atomic `FinalizeCreate`.
- `session_destroyed` was still described as cache-only even though destruction publication precedes the final selection event. A matching destroyed event now safety-suspends the writable terminal binding immediately without deriving selection, closing the inter-event stale-input window.
- Container/resource `MarkExited` only refreshed the row/selection. A locked detached window intentionally ignores central selection and created-row events, so it would stay mounted on a dead route. Every first public live-to-exited transition now emits destroyed-for-cache/window-close before the exited row and optional selection.
- Fail-fast busy hydration had no recovery path. Reconnect/initial hydration now has one generation-owned capped-backoff retry that is canceled by events, disconnect, replacement generation, or unmount.

No optional feature/domain/ports/adapters profile, dependency, inactivity semantic, focus-as-activity behavior, or unrelated refactor is introduced. The required TypeScript skill and all 636 reference lines were applied to the TS/TSX surfaces without claiming adoption of that optional profile.

## 14. Blocker disposition

There is no user-preference blocker. Manual-close fallback, event schema, source policy, restart failure behavior, revision ownership, and frontend dormant UX are all decided above.

Developer and adversarial findings are incorporated. Step 7 adjudication below resolves the remaining architecture questions; no participant decision is outstanding.

## Grinch Review

1. **What:** The worker queue was nominally bounded but `send().await` admitted unbounded waiting caller futures. **Why:** A command flood during a long restore/teardown could exhaust memory while the 64-slot queue itself stayed within capacity; lifecycle callbacks could also be dropped or hang at shutdown. **Fix:** Added a running+queue admission budget, fail-fast external overload, deduplicated critical waiters, shutdown ownership, and recursive-run/overload tests.
2. **What:** `close_transport_from_sync` could synchronously re-enter `Arc<Mutex<PtyManager>>`. **Why:** production `write`/`resize`/`kill` holds that mutex while the route-remover callback locks it again, deadlocking on queue-full/closed. **Fix:** Deferred outer-route removal and lifecycle submission until after the caller returns; added a real-lock timeout regression.
3. **What:** Resource-watchdog kills were absent from the finalization inventory. **Why:** `watchdog.rs` discards `kill_group` results, and the enabled UI-automation backend invokes the same kill directly, so a verified dead selected process could remain canonically live. **Fix:** Added both callers to the file matrix, admitted them before destructive work, and routed them through the shared idempotent whole-job finalizer with race/async-response tests; app shutdown is explicitly excluded.
4. **What:** Browser clients could forge authoritative lifecycle events. **Why:** the generic `broadcast_event` command accepted `session_switched`; a client can hydrate the real epoch, send a higher valid revision, and bind another live ID. **Fix:** Restricted client broadcast to three typed UI events and added high-revision/source forgery negatives.
5. **What:** Browser row events were incomplete. **Why:** shared `session_created` was Tauri-only, so a browser sidebar could accept selection but never receive its row; retained Root/resource liveness refresh had the same risk. **Fix:** Required one dual publisher for changed create/destroy/row/communication events and parity/event-count tests.
6. **What:** Create and batch finalization had stale-selection windows. **Why:** a create begun under Live A could adopt a later auto-close null, while a long manual teardown could commit a candidate list captured before member churn. **Fix:** Added an optional create compare-and-set present only for a starting `None`, plus a final fresh fallback scan with barrier tests.
7. **What:** Restart failure ordering leaked ghost rows and discarded retryable dormant state. **Why:** `session_created(new)` could precede failed final validation, and a dormant wake has no old live PTY whose loss justifies deleting the old record. **Fix:** Deferred restart replacement publication and separated live pre/post-teardown failures from dormant-wake failure.
8. **What:** The proposed live reconciler did not close the old input route before awaiting `list()`. **Why:** after backend switch A→B, A remained writable until B metadata returned; an exited B row could also bind from the stale live payload. **Fix:** Added synchronous binding suspension, exited-row rejection, pending-state gates, and inter-await no-IPC tests while preserving xterm caches.
9. **What:** Voice and sidebar PTY controls were not actually live-only. **Why:** transcription/auto-execute continuations can write after destroy/dormancy, and `SessionItem` renders controls for real exited IDs. **Fix:** Added revocable operation leases, await-boundary tests, destroyed/exited revocation, and rendering plus handler-time row gates while preserving dormant close/wake.
10. **What:** Sidebar selection reapplication could manufacture frontend `active` on an exited Root. **Why:** retained Root publishes an exited upsert before final selection; reapplying the old live payload would overwrite that row. **Fix:** Made exited status win for immutable IDs and added assertions between each event, not only after the sequence.
11. **What:** Epoch handling and listener cleanup missed generation/disposal races. **Why:** an old hydration epoch never previously applied was not retired, disconnect left routing live, equal reconnect permission could survive a newer event, and late listener promises could leak. **Fix:** Made generation the first guard, added disconnect invalidation and permission cancellation, and required disposal-safe subscription/completion tests.
12. **What:** Source/user pairing, duplicate exit codes, and hidden writers remained convention-based. **Why:** independent flags can be mismatched, a late exit can overwrite the first code, and a new direct writer/emitter can bypass functional tests. **Fix:** Added sealed causes/commit witnesses, first-exit idempotence, one-negation-deep decoder/policy tests, and an exact production-source ownership sentinel.
13. **What:** Create finalization and Root uniqueness were not cancellation-, starvation-, or deadlock-safe. **Why:** a create could insert/spawn before discovering a full worker queue, 64 slow ticket reservations could park every queue slot, and a Root caller holding `ROOT_AGENT_SESSION_LOCK` while waiting behind a restore job that needed the same lock would deadlock. **Fix:** Reserved cancellation-safe finalization before non-Root side effects under a 16-ticket sub-budget, and replaced the Root mutex with whole coordinator transactions plus concurrency tests.
14. **What:** Destroy-first event ordering left a stale writable binding. **Why:** `session_destroyed(old)` precedes the final selection payload, but the terminal's cache-only handler left `activeSessionId=old` in that observable gap. **Fix:** Added synchronous matching-ID safety suspension with no fallback derivation and a held-final-event mutation test.
15. **What:** Fail-fast hydration overload could strand a connected browser in neutral state. **Why:** a full coordinator queue returned `Busy`, but no later event is guaranteed to repair the missed reconnect snapshot. **Fix:** Added one bounded, generation-owned hydration retry loop and cancellation/resource tests.
16. **What:** A spawned but unannounced create was eligible and persistable before finalization. **Why:** the manager already held a `Running` row and the PTY route could appear while another close transaction scanned fallback candidates, allowing selection to precede `session_created`; concurrent persistence could retain a crash ghost. Conversely, leaving a restored row pending through detached-window creation would make that window's one-shot `list()` miss it forever. **Fix:** Added manager-owned pending-create state, atomic `FinalizeCreate`, projection/candidate exclusion, barrier tests, and row finalization/publication before detached-window reconstruction.
17. **What:** Retained non-Root exits leaked detached windows and terminal caches. **Why:** resource/container finalization published an exited upsert and optional central selection, but a locked detached `TerminalApp` listens only for matching `session_destroyed`; it would remain writable-looking on a missing route. **Fix:** Standardized first live-to-exited publication as destroyed-for-cache/window-close, exited row, optional selection, with duplicate suppression and locked-window tests.
18. **What:** Several destructive producers could act before coordinator admission. **Why:** a full queue after resource kill or window detach would leave backend/runtime state changed with no ordered manager finalizer; same-ID dedup also incorrectly merged route loss, watchdog, and cleanup policies. **Fix:** Required admission before user/watchdog/detach/cleanup side effects, made auto-close defer before teardown, and keyed critical waiters by operation kind with mutation tests.

## 15. Step 7 consensus adjudication

The current branch, issue contracts, complete plan, and both enrichment reports support one implementable design with no unresolved alternative:

1. **Admission and shutdown:** Keep 65 logical permits because they cover exactly one running job plus the 64 physical queue positions. Keep the queue at 64 and the create sub-budget at exactly 16; each ticket owns one logical permit and one physical `OwnedPermit`, so 16 slow spawns leave at least 48 positions for ordinary or critical work. Critical admission is fair and keyed by exact `(session_id, RouteLoss|WatchdogKill|BackgroundCleanup)`, with one typed waiter per same-kind key. Private data-only job variants, task-local recursive rejection on every enqueue path, receiver-close ticket draining, and close/drain-or-abort-and-join under the existing five-second constant make the design bounded, retain-cycle-free, and objectively testable.
2. **Pending create and publication:** A create ticket is the first side-effect gate. Every inserted precreated row is manager-owned pending and absent from normal list, persistence, candidate, switch, window, and capture projections until the reserved finalizer atomically publishes it. Starting `None` yields an optional compare-and-set; starting non-null yields none. Restore is the explicit transaction-scoped exception: it finalizes and publishes rows before locked detached-window reconstruction, then makes one restore selection. Success has one persistence/event owner; pre-insertion failure queues nothing; post-insertion cancellation, panic, failed validation, or shutdown spends the reserved slot on rollback.
3. **Caller and lifecycle closure:** Bootstrapping places restore first before container, resource, automation, web/API, mailbox/loop, auto-close, watchdog, hotkey, or discovery producers start. Root/restart/batch/detach/attach and low-level destroy/rollback helpers receive the existing transaction rather than recursively enqueueing. User and watchdog resource kills obtain whole-job admission before `kill_group`; there is no post-kill finalizer window. Container route loss uses a narrow managed-handle-free sender, synchronous close defers outer-manager removal until its caller-held lock is released, and blocking spawn/handshake has a late-handle cancellation guard. Shutdown joins the coordinator before global cleanup and final aggregate persistence.
4. **Step 5 versus Step 6:** Retain Step 5's consolidated manager state, sealed source/intent contract, process epoch and revision, one FIFO coordinator, dual-transport publication, discriminated TypeScript union, runtime decoder, and generation-first frontend reconciliation. Step 6 prevails on its four explicit dissents: a raw bounded queue is insufficient without logical admission and shutdown ownership; manager-visible precreated rows are replaced by pending projections; destroy is cache disposal plus immediate matching-binding safety suspension, never cache-only; and resource kill/finalization is one pre-admitted transaction, never kill first and enqueue later. These are complementary corrections rather than competing implementations.
5. **Scope and contract:** Passive focus/view remains nonactivity. Auto-close and background destruction clear a removed selected ID and never pick a fallback. Manual close alone chooses the first final-state eligible live, attached, PTY-backed record in stable order. Dormant and non-exited/no-PTY selections never mount or route a terminal. The file/symbol matrix, phased order, failure behavior, compatibility/security impact, regression commands, mutation sentinels, and numbered acceptance criteria leave no unresolved marker, competing option, or implementer policy choice.

All architecture and certification questions are resolved. Implementation must satisfy the complete plan as one delivery; partial manager-only completion does not close #1027.
