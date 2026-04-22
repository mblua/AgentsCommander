# Plan — fix: `update_team` / `sync_workgroup_repos` fail with "state not managed for field `gitWatcher`"

## 1. Requirement

When the user edits a team in the **Edit Team** modal (Step 3 of 3) and clicks **Save**, the backend panics/returns:

```
state not managed for field `gitWatcher` on command `update_team`.
You must call `.manage()` before using this command
```

Root cause: the `Arc<GitWatcher>` created at `src-tauri/src/lib.rs:269` is started (line 270) and then **moved** into `PtyManager::new()` at line 284 without ever being registered via `app.manage()`. Any Tauri command declaring `git_watcher: State<'_, Arc<GitWatcher>>` therefore fails at parameter resolution time.

Two commands are affected:

- `update_team` — `src-tauri/src/commands/entity_creation.rs:729` (param at line 732). Triggered by **EditTeamModal → Save** (`src/sidebar/components/EditTeamModal.tsx:199`).
- `sync_workgroup_repos` — `src-tauri/src/commands/entity_creation.rs:1064` (param at line 1067). Exposed via `EntityAPI.syncWorkgroupRepos` in `src/shared/ipc.ts:357`, **but no UI component currently invokes it**. Still reachable via devtools/console, and the same bug exists there.

Fix: add `app.manage(Arc::clone(&git_watcher));` in `lib.rs` between `.start()` (line 270) and the move into `PtyManager::new()` (line 281), so both Tauri state and `PtyManager` hold valid `Arc<GitWatcher>` handles.

---

## 2. Affected files

| # | File | Line(s) | Change type |
|---|------|---------|-------------|
| 1 | `src-tauri/src/lib.rs` | insert between current line 270 and 272 | Add one line |

No TypeScript changes. No new crates. No new events. No command signature changes.

---

## 3. Change description — `src-tauri/src/lib.rs`

### 3.1 Current state (lines 268-287, verified against `fix/update-team-gitwatcher-state` @ `ebf4dee`)

```rust
            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start(shutdown_for_setup.clone());

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            discovery_branch_watcher.start(shutdown_for_setup.clone());
            app.manage(discovery_branch_watcher);

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                git_watcher,
                Some(broadcaster_for_pty),
            )));
            app.manage(pty_mgr.clone());
```

### 3.2 Desired state (after the fix)

```rust
            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start(shutdown_for_setup.clone());
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
            let discovery_branch_watcher = DiscoveryBranchWatcher::new(
                app.handle().clone(),
                session_mgr_for_discovery,
            );
            discovery_branch_watcher.start(shutdown_for_setup.clone());
            app.manage(discovery_branch_watcher);

            // PtyManager needs GitWatcher for cleanup on session kill
            let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
                output_senders_for_pty,
                idle_detector_for_pty,
                git_watcher,
                Some(broadcaster_for_pty),
            )));
            app.manage(pty_mgr.clone());
```

### 3.3 The exact edit (for `feature-dev` / dev-rust to apply)

**In `src-tauri/src/lib.rs`, find:**

```rust
            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start(shutdown_for_setup.clone());

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
```

**Replace with:**

```rust
            // Git branch watcher: polls git branch for each session every 5s
            let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
            git_watcher.start(shutdown_for_setup.clone());
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
            app.manage(Arc::clone(&git_watcher));

            // Discovery branch watcher: polls git branch for discovered replicas every 15s
```

That is the entire code change: **one new statement plus a 3-line comment**, inserted right after the existing `git_watcher.start(...)` call.

### 3.4 Why this is ordered correctly

- `GitWatcher::new()` (`src-tauri/src/pty/git_watcher.rs:38-47`) returns `Arc<Self>`, so `git_watcher` is already `Arc<GitWatcher>`.
- `GitWatcher::start()` (line 49) takes `self: &Arc<Self>` — it borrows, does not consume. Inside `start()`, the worker thread captures its own `Arc::clone(self)` (line 50), so the background thread's lifetime is independent of the outer `git_watcher` binding.
- `app.manage(...)` takes ownership of its argument, stores it in the per-type state slot. Passing `Arc::clone(&git_watcher)` gives it one Arc handle; the local `git_watcher` binding keeps the original.
- `PtyManager::new(..., git_watcher, ...)` (line 281-286) consumes the local `git_watcher` by value. This move happens **after** the clone, so both destinations end up with valid `Arc<GitWatcher>` references to the same inner `GitWatcher`.

After the fix, the refcount progression inside `.setup(...)` is:

1. `GitWatcher::new()` returns → refcount = 1 (local `git_watcher`).
2. `git_watcher.start(...)` spawns thread with internal `Arc::clone` → refcount = 2 (local + worker thread).
3. `app.manage(Arc::clone(&git_watcher))` → refcount = 3 (local + worker + Tauri state).
4. `PtyManager::new(..., git_watcher, ...)` moves local in → refcount stays 3 (worker + Tauri state + PtyManager field).

No double-start: `start()` is called exactly once (line 270), before the clone and before the move.

### 3.5 Matching convention

The existing code already uses `Arc::clone(&...)` for shared references (e.g. `Arc::clone(&session_mgr)` at lines 224-227). Use `Arc::clone(&git_watcher)`, not `git_watcher.clone()`, to keep the idiom consistent and to make the intent (cheap refcount increment, not a deep clone) unambiguous.

---

## 4. Dependencies

- **None.** `Arc` and `Manager` are already imported in `lib.rs` (line 14 and line 24 respectively). `GitWatcher` already imported at line 19.
- No `Cargo.toml` change.
- No new TS types. No IPC signature changes.

---

## 5. Notes & constraints for the implementer

### 5.1 Do NOT

- Do **not** change `PtyManager::new()`'s signature. It must keep taking `Arc<GitWatcher>` by value so the existing call site stays correct.
- Do **not** refactor `GitWatcher::new()` to return `Self` instead of `Arc<Self>`. That would cascade into `start()`, the worker thread, and PtyManager — large blast radius for no gain.
- Do **not** move the `app.manage(Arc::clone(&git_watcher))` line **after** the `PtyManager::new(..., git_watcher, ...)` call. At that point the local binding has been moved and `&git_watcher` would fail to compile ("borrow of moved value"). Keep the line between `git_watcher.start(...)` and `let discovery_branch_watcher = ...`.
- Do **not** call `git_watcher.start(...)` a second time. Calling it once is enough; each call spawns a new worker thread.
- Do **not** touch `entity_creation.rs`. The commands are correct as-written; the fix is entirely in `lib.rs`.
- Do **not** add any frontend changes. No new types, no new handlers.

### 5.2 Must

- Run `cd src-tauri && cargo check` to confirm the edit compiles.
- Run `cd src-tauri && cargo clippy` to confirm no new warnings.
- Keep the comment above the new line explaining *why* the clone is needed, so a future reader does not assume it is dead code and delete it.

---

## 6. Test strategy

### 6.1 Sync with main before testing

```bash
cd repo-AgentsCommander
git fetch origin
git merge origin/main
```

### 6.2 Rebuild and run

```bash
cd repo-AgentsCommander
npm run kill-dev              # Kill any stale target\debug instance
npm run tauri dev             # Fresh build + run
```

### 6.3 Manual test — primary path (Edit Team Save)

1. Pick a project that has at least one team. If needed, create one via the Agent Creator UI.
2. In the sidebar, click the team's edit button (pencil) to open **Edit Team**.
3. Navigate to **Step 3 of 3** (Repo assignments).
4. **Case A — add a repo**: add a new repo, assign it to at least one agent, click **Save**. Expect: modal closes; no error toast; project reloads; new repo is visible in the team view.
5. **Case B — remove a repo**: remove an existing repo assignment, click **Save**. Expect: same success.
6. **Case C — no repo change**: open the modal, click through to Step 3, change nothing, click **Save**. Save succeeds; `sync_workgroup_repos_inner` emits one `session_git_repos` event per coordinator session for this team (even though repo assignments did not change), because the event currently carries `branch: None` which differs from the in-memory cached branch. Expect a ≤7s branch-badge flicker on affected sessions.
7. **Case D — team with existing workgroups**: ensure at least one workgroup exists for the team being edited. Immediately after Save, badges for affected sessions momentarily collapse to label-only (`branch: None` emit from the command). Within ~5-7s they restore to `label/branch` form via a `GitWatcher` poll/emit cycle. Both events are expected.
8. **Case E — team with no workgroups**: edit a team that has no workgroup yet. After Save: `sync_workgroup_repos_inner` finds no workgroups, returns `SyncResult { 0, 0, [] }`; no error.

### 6.4 Manual test — secondary path (`sync_workgroup_repos` directly)

No UI entry point currently, so test via devtools:

1. Open the sidebar window.
2. Right-click → Inspect → Console.
3. Run (replace paths/names with real values for your test project):
   ```js
   await window.__TAURI_INTERNALS__.invoke('sync_workgroup_repos', {
       projectPath: 'C:\\path\\to\\project',
       teamName: 'some-team'
   });
   ```
4. Expect: a `SyncResult` object back. No `state not managed` error.

*(If the exact devtools invoke handle is unavailable, `EntityAPI.syncWorkgroupRepos(projectPath, teamName)` can be called from the console after importing — but the test above is sufficient to prove state resolution works.)*

### 6.5 Negative regression checks

- **Session creation**: create a new session. Check the terminal spawns. Confirm the GitWatcher still emits `session_git_repos` for repos configured on that session (this exercises `PtyManager`'s `git_watcher` field being live). The branch should appear within ~5s.
- **Session destruction**: destroy a session. Check logs for any panic around `git_watcher.remove_session(...)` — there should be none; this path is unchanged.
- **App shutdown**: close the window. Confirm the process exits cleanly and the GitWatcher thread stops. The shutdown signal logic is unchanged; no behavioural regression expected.
- **Team creation** (`create_team`) — not affected (does not declare `git_watcher: State`), but verify it still works to confirm `emit_coordinator_refresh` wiring was not collaterally damaged.
- **Team deletion** (`delete_team`) — not affected, same verification.

### 6.6 Unit/integration tests

- No new unit test is strictly required for this one-line fix. The behaviour being restored is "Tauri state resolution succeeds", which is a runtime behaviour of the Tauri framework, not a business rule.
- A minimal integration test would have to spin up `tauri::App` and attempt to resolve `State<Arc<GitWatcher>>`, which adds complexity disproportionate to the fix. Skip unless dev-rust-grinch objects in review.
- The existing tests in `git_watcher.rs:202-273` (cache/gen CAS test) are unaffected and should still pass; `cd src-tauri && cargo test` should succeed with no new failures.

---

## 7. Regression risk analysis

| Area | Risk | Notes |
|------|------|-------|
| **Tauri state resolution** | ✅ Positive | `State<'_, Arc<GitWatcher>>` now resolves for `update_team` and `sync_workgroup_repos`. |
| **PtyManager ownership** | None | `PtyManager::new(..., git_watcher, ...)` still receives a valid Arc. Signature unchanged. |
| **GitWatcher background thread** | None | `start()` is still called exactly once on line 270. Worker thread already holds its own `Arc::clone`. No double-start. |
| **Cache / emission logic** | None | `GitWatcher::cache` is `Mutex<HashMap>` and shared by any holder of the Arc — adding one more holder is a refcount bump, not a semantic change. |
| **Other team-edit flows** | None | `create_team` and `delete_team` do not take `git_watcher: State` (verified via grep in `entity_creation.rs`). |
| **Multi-window event emissions** | None | No new emissions. The existing `session_git_repos` emit paths (`git_watcher.rs:132-138`, `entity_creation.rs:1049-1055`) are unchanged. |
| **`DiscoveryBranchWatcher`** | None | Unchanged. The fix introduces a parallel pattern to the one already in use for this type (lines 273-278). |
| **Web server path** | None | `pty_mgr` is still managed (`app.manage(pty_mgr.clone())`), and the web server uses `pty_mgr.clone()` for its own holder. Unchanged. |
| **Session restore** | None | The `spawn(async move { ... restore ... })` block accesses `Arc<Mutex<PtyManager>>` via `app.state::<Arc<Mutex<PtyManager>>>()`, which is unaffected. |
| **Memory** | Negligible | One additional Arc field in Tauri's state registry. ~8 bytes. No leak: drops when the app process exits. |

**Overall**: low-risk, surgical single-line fix. No architectural change.

---

## 8. Workflow hand-off

- **Plan status**: ready for dev-rust / dev-rust-grinch review.
- **Branch**: `fix/update-team-gitwatcher-state` (already on disk, clean, based on `origin/main`).
- **Plan file**: `repo-AgentsCommander/_plans/fix-update-team-gitwatcher-state.md` (this file).
- **Implementation size**: 1 inserted code line + 3 comment lines in a single file.
- **After impl**: run `/feature-dev` as required by CLAUDE.md change validation protocol, then hand off to shipper for build.
- **No need to**: touch frontend, add types, run `npm install`, update any TOML, or bump the version (per CLAUDE.md, patch bump happens at build time alongside other changes).

---

## 9. Dev-rust review (dev-rust, 2026-04-22)

### 9.1 Diagnosis verification — **confirmed**

I walked the architect's diagnosis against HEAD of `fix/update-team-gitwatcher-state`. Every reference is accurate; no surprises.

- `GitWatcher::new()` at `src-tauri/src/pty/git_watcher.rs:38-47` — `pub fn new(...) -> Arc<Self>`. Returns `Arc`, confirmed.
- `GitWatcher::start()` at `src-tauri/src/pty/git_watcher.rs:49-69` — `pub fn start(self: &Arc<Self>, shutdown: ...)`. Takes `&Arc<Self>` → borrows, does NOT consume. Inside, `let watcher = Arc::clone(self);` spawns the worker thread with its own refcount handle (line 50-51).
- `PtyManager::new(...)` at `src-tauri/src/pty/manager.rs:260-275` — takes `git_watcher: Arc<GitWatcher>` **by value** (line 263). This is the move site.
- `update_team` — `src-tauri/src/commands/entity_creation.rs:729-733` — declares `git_watcher: State<'_, Arc<GitWatcher>>` (line 732). Exactly as the plan states.
- `sync_workgroup_repos` — `src-tauri/src/commands/entity_creation.rs:1064-1068` — declares `git_watcher: State<'_, Arc<GitWatcher>>` (line 1067). Exactly as the plan states.
- `lib.rs:261-287` setup block — `GitWatcher::new()` at line 269, `.start()` at line 270, `PtyManager::new(..., git_watcher, ...)` at line 281-286 moves the binding, `app.manage(pty_mgr.clone())` at line 287 registers the PtyManager. Between line 270 and 281 there is **no** `app.manage(git_watcher)` — confirmed bug site.

### 9.2 Missing-state audit — scope is tight

I grepped for `State<'_, Arc<` across the entire backend to verify this is the only missing registration (not one of many). Result: every other `State<'_, Arc<T>>` in the codebase resolves to a type that IS managed in `lib.rs`:

| State type | Managed at |
|---|---|
| `Arc<tokio::sync::RwLock<SessionManager>>` | `lib.rs:252` |
| `Arc<DiscoveryBranchWatcher>` | `lib.rs:278` |
| `Arc<Mutex<PtyManager>>` | `lib.rs:287` |
| `Arc<WebAccessToken>` | `lib.rs:257` |
| **`Arc<GitWatcher>`** | **NEVER (the bug)** |

No other orphaned `State<Arc<X>>` exists. The scope of this fix is exactly one line. No hidden class-of-bug.

### 9.3 Semantic purpose of the fix

Worth documenting for grinch/reviewers: the reason `update_team` and `sync_workgroup_repos` need the `GitWatcher` state is to call `git_watcher.invalidate_session_cache(session_id)` at `entity_creation.rs:1048`. Without this, after writing new `repos` to disk, the next GitWatcher poll would short-circuit on the cached previous `Vec<SessionRepo>` equality and **silently skip the emit**, leaving the sidebar's branch badges stale until some unrelated change triggers a cache miss. So the fix isn't just "command stops crashing" — it's "repo changes actually propagate to the UI within ~5s". Grinch should validate Case D in §6.3 carefully against this.

### 9.4 Implementation ergonomics — the proposed idiom is correct

I considered three alternatives to the plan's `app.manage(Arc::clone(&git_watcher))`:

1. **Move `git_watcher` into `app.manage()` first, then retrieve via `app.state::<Arc<GitWatcher>>()` and pass to `PtyManager::new(...)`.** Rejected: `app.state()` returns `State<T>`, not `Arc<T>`; extracting the inner Arc requires `.inner().clone()` which is just `Arc::clone(&git_watcher)` with extra indirection, plus it inverts the natural construction order.
2. **Let bind a clone first, then use original for the move and clone for manage.** `let git_watcher_for_state = Arc::clone(&git_watcher); ...; app.manage(git_watcher_for_state);` — functionally identical, two lines instead of one. Strictly worse.
3. **Change `PtyManager::new()` to take `&Arc<GitWatcher>` and clone internally.** Rejected (and plan §5.1 already forbids this). Changes a signature for zero benefit.

The plan's choice (§3.2) — **`app.manage(Arc::clone(&git_watcher))` inserted between `.start()` at line 270 and the `discovery_branch_watcher` block** — is the smallest, most idiomatic, and most readable option. Approved as-is.

**Note on asymmetry with `DiscoveryBranchWatcher`**: at `lib.rs:278` the pattern is `app.manage(discovery_branch_watcher)` (direct move, no `Arc::clone`). This is NOT inconsistent — `DiscoveryBranchWatcher` has no second consumer (no equivalent of `PtyManager::new`) so the binding can be moved wholesale. `GitWatcher` has two consumers (Tauri state + PtyManager) so one consumer must take a clone. The plan's proposed comment already conveys this by mentioning "must happen BEFORE the `PtyManager::new(..., git_watcher, ...)` move"; that is sufficient, no further change requested.

### 9.5 Refcount / lifetime — verified

The plan's §3.4 refcount progression is correct. After the fix:

1. `GitWatcher::new()` → `Arc` with refcount 1 (local `git_watcher`).
2. `git_watcher.start(...)` spawns a `std::thread::spawn` that captures its own `Arc::clone(self)` → refcount 2.
3. `app.manage(Arc::clone(&git_watcher))` → refcount 3. Tauri's state registry is a `HashMap<TypeId, Box<dyn Any + Send + Sync>>` internally; it takes ownership of the Arc and retains it for the app process's lifetime.
4. `PtyManager::new(..., git_watcher, ...)` moves the local binding → refcount remains 3 (worker + Tauri state + PtyManager field; local binding dropped).

Drop ordering at shutdown: when the Tauri app terminates, the managed state map drops (→ refcount 2), the `Arc<Mutex<PtyManager>>` drops (→ refcount 1), and the worker thread either returns from its poll loop (via the `ShutdownSignal`) or the process image is torn down. Final refcount hits 0; `GitWatcher` is freed. **No leak.**

### 9.6 Lock-ordering and safety — no new hazards

The `GitWatcher::cache: Mutex<HashMap<Uuid, Vec<SessionRepo>>>` at `git_watcher.rs:20` is now reachable from:

- **(a)** the worker thread's `poll()` method (acquires briefly inside the poll loop)
- **(b)** `PtyManager` cleanup paths (`remove_session`)
- **(c)** command handlers via `git_watcher.invalidate_session_cache(session_id)` at `entity_creation.rs:1048`

All three holders call `self.cache.lock().unwrap()` and drop the guard within the same statement. No nested locking. No `.await` while holding the guard (the functions touching `cache` are synchronous). No deadlock risk. The fix adds path (c) but the Mutex's single-holder guarantee covers it.

**Windows specifics**: `Arc<T>` where `T: Send + Sync` is fully portable. `GitWatcher` fields are `Arc<tokio::sync::RwLock<SessionManager>>`, `AppHandle`, and `Mutex<HashMap<Uuid, Vec<SessionRepo>>>` — all `Send + Sync`. No Windows-specific concern.

### 9.7 Error handling / logging — explicitly NOT needed

`Manager::manage()` is infallible: it returns `bool` (false if the type was already managed, true otherwise). No error path to log. Adding a `log::info!` for "GitWatcher registered as managed state" would just be noise — it's a one-time setup step that either works or would fail to compile. **No log line warranted.** (Explicit answer to tech-lead Q3.)

If paranoia is warranted, one defensive option is `debug_assert!(app.manage(Arc::clone(&git_watcher)), "GitWatcher managed twice")`, but this is overkill for a setup function that runs exactly once. Skip.

### 9.8 Test strategy additions

The plan's §6.3 Cases A–E cover the primary UX paths. I'd add these to harden the suite:

- **Case F — Rapid repeated Save clicks**: open Edit Team, change a repo, click Save, then IMMEDIATELY re-open and Save again. Expect: both saves succeed; no race in the on-disk `config.json` write; final state reflects last click; no panic / lock poisoning. (This exercises re-entry of `sync_workgroup_repos_inner` and is orthogonal to the state fix, but it's cheap to verify while testing.)
- **Case G — Edit-Save while session is actively polling**: with a session whose repo has a long-running branch detection (slow git), click Save. Expect: the ongoing poll completes normally, the cache-invalidation from the command lands before the next poll tick, next tick emits fresh data. No deadlock, no lost emit.
- **Case H — Shutdown mid-save**: click Save, and within <500ms close the window. Expect: app exits cleanly; no panic in the async `sync_workgroup_repos_inner` being dropped; no orphan file writes.
- **Case I — Team with 0 repos (empty `repos` array)**: edit a team, remove ALL repos, Save. Expect: success, `config.json` written with `repos: []`, no emits (nothing to emit), no panic in `sync_workgroup_repos_inner` early-return path.
- **Case J — Team with no agents on any repo**: add a repo but assign it to zero agents. Expect: the replicas whose agent list is empty for that repo are untouched; no panic. (This mostly tests `sync_workgroup_repos_inner`'s filter logic, which is unchanged by this fix.)
- **Case K — Coordinator-only team**: team with only a coordinator, no other agents. Save. Expect: success; `sync_workgroup_repos_inner` either applies to the coordinator's replica or no-ops. Pre-existing behavior; verify no regression.

All of F–K exercise code paths that are UNCHANGED by this fix. I'd still recommend running at least F and G in the manual smoke test because they are the realistic footguns users hit first. H–K are nice-to-have.

### 9.9 Scope questions

- **No automated test for state resolution**: Plan §6.6 argues that spinning up a Tauri `App` just to prove state resolution is disproportionate to a one-line fix. I agree. The class of bug ("`State<Arc<T>>` declared but `T` never managed") is ALWAYS caught at first invocation — there is no silent failure mode to regress into. Clippy does not catch it; neither does `cargo check`. A generic audit script could flag it at CI time (grep `State<.*Arc<\w+>>` vs `app.manage(` for the same type), but that's a separate tooling initiative, not part of this fix. **Flagging as a follow-up scope question, NOT adding to this fix.**
- **UI entry point for `sync_workgroup_repos`**: Plan §1 notes that `sync_workgroup_repos` has no UI caller today. The fix repairs it regardless — reachable via devtools and via any future UI. Not our scope here to wire a UI.
- **No scope creep identified**: the plan stays exactly within "make the two commands work". Correct call.

### 9.10 Follow-up risk

- **Any future `State<Arc<GitWatcher>>` consumer** will Just Work after this fix — no need to revisit `lib.rs`. Good.
- **If `GitWatcher::new()` is ever refactored to return `Self` instead of `Arc<Self>`** (plan §5.1 forbids this), the `app.manage(Arc::clone(&git_watcher))` line would break the compile. Refactor would need to carry a matching update here. Acceptable — compile error makes the coupling visible.
- **If `PtyManager::new(...)` is ever refactored to take `&Arc<GitWatcher>`**, the `app.manage(Arc::clone(&git_watcher))` could be simplified to `app.manage(git_watcher)` (move), since the local binding would no longer be moved at line 281. Harmless future nit. Do not act on this now.

### 9.11 Design-review nits

- The 3-line comment above the new line (plan §3.2) is **mandatory** — without it, a future reader may `git blame` the single line, see "plumbing", and attempt to inline or remove it. Explicit mention of "(e.g. `update_team`, `sync_workgroup_repos`)" earns its keep.
- Alternative comment wording I considered: "needed by `update_team` and `sync_workgroup_repos` which declare `State<'_, Arc<GitWatcher>>`". The plan's current wording (`Register for Tauri commands that take State<'_, Arc<GitWatcher>>`) is slightly more general and ages better if more consumers appear. **No change requested**, plan's wording is fine.
- Consider using `#[allow(clippy::arc_with_non_send_sync)]` proactively? **No.** `GitWatcher` IS `Send + Sync` (all fields are). Clippy lint does not fire. No attribute needed.

### 9.12 Verdict

**APPROVE AS-IS.** The plan is correct, complete at the surgical-fix level, and well-justified.

- Diagnosis: verified against code, no discrepancies.
- Proposed edit: correct idiom, minimal blast radius, comment provides the rationale a future reader needs.
- Refcount / lifetime / lock-ordering: safe.
- Test strategy: Cases A–E are sufficient to merge; Cases F–G from §9.8 recommended as additional manual smoke tests (but not blocking).
- Scope: tight. No scope creep, no adjacent TODOs pulled in.
- Follow-up: file a separate ticket for a `State<Arc<T>>` / `app.manage()` CI audit script if anyone cares; do not bundle here.

Ready for grinch adversarial review. Implementation can proceed the moment grinch signs off.

---

## 10. Grinch adversarial review (dev-rust-grinch, 2026-04-22)

### 10.1 Verdict

**CONDITIONAL APPROVE — follow-ups required before implementation.**

The one-line fix is mechanically correct; I could not break it. However, the fix makes previously-unreachable code live for the first time, and that newly-reachable code has a real UX regression (branch-badge flicker) plus two test-plan claims that do not match what the code will actually do. None of this blocks the fix itself, but §6.3 needs a correctness pass before the tester runs manual smoke, and the flicker deserves a follow-up ticket. Details below.

### 10.2 Findings

#### Finding 1 — MEDIUM: Command emit publishes `branch: None` → transient badge flicker (newly reachable, pre-existing)

- **What.** At `src-tauri/src/commands/entity_creation.rs:1049-1055`, `sync_workgroup_repos_inner` emits `session_git_repos` with the `repos` value returned by `refresh_git_repos_for_sessions`. Tracing the source:
  - `sync_workgroup_repos_inner:1008-1011` builds each repo via `build_session_repo` (line 811-834), which **unconditionally sets `branch: None`** (line 832).
  - `SessionManager::refresh_git_repos_for_sessions` (`session/manager.rs:326-342`) clones the input `repos` into its return value (line 337 `changed.push((*id, repos.clone()))`).
  - Therefore every session in `changed` receives a `Vec<SessionRepo>` where every element has `branch: None`.
- **Why it matters.** The sidebar renders `{repo.label}{repo.branch ? "/${branch}" : ""}` (`src/sidebar/components/SessionItem.tsx:325`). Before save: badge shows `AgentsCommander/fix-foo`. After the command's emit lands: badge collapses to `AgentsCommander`. The next `GitWatcher` tick re-detects branches, calls `set_git_repos_if_gen`, emits with populated branches, badges restore. Worst-case user-visible flicker: up to ~7s (≤5s poll interval + ≤2s detection). This fires on **every Save**, including no-op saves (see Finding 2). Because the `state not managed` bug kept this path dormant, this UX regression has never been observed in production; the fix is what makes it live.
- **Evidence.**
  - `build_session_repo` sets `branch: None` — `entity_creation.rs:829-833`.
  - `refresh_git_repos_for_sessions` writes & returns input repos — `session/manager.rs:332-340`.
  - Command emit uses those repos unchanged — `entity_creation.rs:1049-1055`.
  - `GitWatcher` re-detects and re-emits via CAS — `pty/git_watcher.rs:117-147`.
- **Recommended action.** Do **not** block the fix on this. Plan §3 stays as-is. File a follow-up ticket: either (a) merge the new `assigned_repos` with existing in-memory branches in `refresh_git_repos_for_sessions` so the command's emit carries `branch: <Some>` when unchanged, (b) skip the command-level emit entirely and let the `invalidate_session_cache` + next `GitWatcher` tick do the work (latency ~5s instead of flicker), or (c) accept flicker deliberately. Record the chosen trade-off so a future reader does not re-discover this.

#### Finding 2 — LOW: Plan §6.3 Case C claim "may emit zero updates" is false

- **What.** §6.3 Case C says:
  > `sync_workgroup_repos_inner` may emit zero updates, which is fine
- **Why it matters.** The equality guard inside `refresh_git_repos_for_sessions` is `&s.git_repos != repos` (`session/manager.rs:334`). Because the in-memory `s.git_repos` carries detected branches (from prior `GitWatcher` CAS writes at `pty/git_watcher.rs:127`) while the command's computed `repos` always carries `branch: None` (Finding 1), this inequality **always triggers for any session that has been polled at least once**. Result: a no-op team edit produces a real emit *and* the Finding 1 flicker. The tester reading Case C will interpret any emitted event as unexpected and may file a phantom bug.
- **Recommended action.** Replace the Case C expectation with: "Save succeeds; `sync_workgroup_repos_inner` emits one `session_git_repos` event per coordinator session for this team (even though repo assignments did not change), because the event currently carries `branch: None` which differs from the in-memory cached branch. Expect a ≤7s branch-badge flicker on affected sessions." That is honest and is what the tester will observe.

#### Finding 3 — LOW: Plan §6.3 Case D mischaracterizes the timing

- **What.** §6.3 Case D says:
  > the affected sessions should receive a `session_git_repos` event and the branch badges should refresh within ~5s (one GitWatcher poll cycle)
- **Why it matters.** Two separate events reach the sidebar:
  1. **Immediate** (at command return): `session_git_repos` with `branch: None` from `entity_creation.rs:1049` — badges lose their branch suffix within a frame.
  2. **Deferred** (≤5s poll + ≤2s detection = ≤7s later): `session_git_repos` with populated branches from `GitWatcher::poll` at `git_watcher.rs:132` — badges restore.
  The tester is told to expect "refresh within ~5s", so if they see badges *drop* for 5s they might conclude the fix is broken. Case D wording should describe both emits.
- **Recommended action.** Rewrite Case D's expectation to: "Immediately after Save, badges for affected sessions momentarily collapse to label-only (`branch: None` emit from the command). Within ~5-7s they restore to `label/branch` form via a `GitWatcher` poll/emit cycle. Both events are expected."

#### Finding 4 — LOW: Plan §9.5 refcount drop-at-shutdown narrative is slightly optimistic

- **What.** §9.5 claims drop order at shutdown results in clean Arc release to 0. In reality, Rust `HashMap<TypeId, Box<dyn Any + Send + Sync>>` drop order is unspecified; Tauri's state map drop sequence is not guaranteed. If `ShutdownSignal` is dropped *before* `pty_mgr`, the worker thread's `tokio::select!` might pick the `sleep(POLL_INTERVAL)` branch for up to 5s before noticing cancellation, during which time it still holds its Arc clone. This delays final refcount-hits-0 by ≤5s but does **not** cause a leak — the process is exiting anyway, and `std::thread::spawn` is detached so no join blocks shutdown. Narrative is fine, numbers are fine; just flagging that "refcount 0" is not instant.
- **Why it matters.** If a future reader adds a `drop` assertion or a graceful-shutdown step that depends on the worker thread having already returned, they will be surprised. Pre-existing behavior; the fix does not change it.
- **Recommended action.** None required. If §9.5 is ever re-read, consider adding a one-liner: "worker thread releases its Arc clone asynchronously on the next `tokio::select` wake; for clean shutdown path this lands within ≤POLL_INTERVAL."

#### Finding 5 — NIT: Pre-existing `.unwrap()` on `self.cache.lock()` (not in fix scope)

- **What.** `GitWatcher::cache.lock().unwrap()` at `git_watcher.rs:72, 79, 92, 118, 139, 146` will panic if the mutex is poisoned (a holder panicked while locked). My role standard is "zero tolerance for `.unwrap()` on fallible ops", but the fix does not introduce new `.unwrap()` calls and there is no clean recovery path here — a poisoned state cache is a cold-start situation, not a recoverable error. Flagging for awareness only.
- **Why it matters.** Pre-existing hazard. Irrelevant to this fix's correctness.
- **Recommended action.** None. Not a scope-escalation request; if anyone cares, it belongs in a separate "audit `.unwrap()` in pty/git_watcher" ticket.

### 10.3 Attack scenarios tried

| # | Attack | Result |
|---|--------|--------|
| A1 | Refcount miscount: can `app.manage(Arc::clone(&git_watcher))` interact with the subsequent `PtyManager::new(..., git_watcher, ...)` move to leave `git_watcher` in an invalid state? | **Didn't break.** `Arc::clone` on `&git_watcher` borrows, returns new Arc; original binding remains valid for the move. Refcount progression 1→2→3→3 is correct. |
| A2 | Double-start: can the worker thread be spawned twice? | **Didn't break.** `start()` is called exactly once at `lib.rs:270`; the fix inserts `app.manage()` after it, not a second `start()`. |
| A3 | Drop-order: at shutdown, can the Tauri state map drop `Arc<GitWatcher>` while the worker thread still needs it? | **Didn't break.** Worker thread holds its own `Arc::clone(self)` captured in `start()` at `git_watcher.rs:50`; its Arc survives independent of Tauri state. See Finding 4 for narrative nit. |
| A4 | `Send + Sync` violation: is `Arc<GitWatcher>` safe to register with Tauri? | **Didn't break.** `GitWatcher` fields are `Arc<tokio::sync::RwLock<SessionManager>>` (Send+Sync), `AppHandle` (Send+Sync by Tauri contract), `Mutex<HashMap<Uuid, Vec<SessionRepo>>>` (Send+Sync with Send+Sync contents). `SessionRepo` (Clone, Serialize, PartialEq on owned String/Option<String>) is Send+Sync. |
| A5 | Stale-write race: poll captures gen=X, command bumps to X+1 mid-detection, poll CAS-writes at X. | **Didn't break.** `set_git_repos_if_gen` at `session/manager.rs:270-285` rejects the stale write; `poll()` at `git_watcher.rs:140-146` logs and clears cache; next tick re-evaluates at X+1. |
| A6 | Command emit vs. poll emit ordering: can the user see branches go to None and never come back? | **Didn't break.** Every CAS-rejected poll clears the cache slot, so the subsequent poll unconditionally re-evaluates, detects, CAS-writes at the current gen, and emits. Recovery is bounded by poll interval + detection timeout ≤7s. See Finding 1 for the flicker this produces. |
| A7 | Lock ordering across `GitWatcher`, `DiscoveryBranchWatcher`, and `SessionManager`. | **Didn't break.** Three mutex/lock acquisitions in the newly-reachable command: `session_mgr.write().await` → dropped, then `discovery_watcher` locks (`replicas` released before `discovery_cache`+`repos_cache` pair) → dropped, then `git_watcher.cache.lock()` (brief, single statement). No nested locking with await; no circular dependency. `DiscoveryBranchWatcher::invalidate_replicas` holds `discovery_cache` → `repos_cache` in that order; no code elsewhere acquires them in reverse. |
| A8 | Missed `State<Arc<T>>` with no `.manage()`: is `GitWatcher` the only one? | **Didn't break.** Grepped `State<'_, Arc<` vs `.manage(`/`app.manage(` across `src-tauri/src`. Every other managed type has a matching registration at `lib.rs:250-287`. Plan §9.2 was right. |
| A9 | Rapid repeated Save clicks (Case F from dev-rust): can concurrent `update_team` corrupt state or deadlock? | **Didn't break.** Tauri commands are async; two concurrent invocations serialize on `sessions.write().await`. Filesystem writes are last-writer-wins, which is acceptable for a Save button. Prolongs the Finding 1 flicker but no corruption. |
| A10 | `invalidate_replicas` without subsequent `discover_project` (e.g. Discovery panel closed): can DiscoveryBranchWatcher permanently lose replicas? | **Partially broke, but mitigated.** `invalidate_replicas` removes entries from `replicas`, `discovery_cache`, `repos_cache` (`ac_discovery.rs:348-367`). If no subsequent `discover_project` fires, those replicas stay out of the Discovery poller until next discovery. However: `EditTeamModal.handleSave` (`src/sidebar/components/EditTeamModal.tsx:206`) calls `projectStore.reloadProject()` which triggers `ProjectAPI.discover()` → `discover_ac_agents` → `update_replicas_for_project` → repopulates. Recovery is guaranteed for the Edit Team flow. For the devtools-only `sync_workgroup_repos` path (§6.4), no such guarantee — but that path is test-only and the user can trigger discover manually. Acceptable. |
| A11 | Windows-specific: path canonicalization + UNC prefix stripping in `build_session_repo`. | **Didn't break.** `build_session_repo` at `entity_creation.rs:811-834` strips `\\?\` prefix (line 816-818) and normalizes separators to `/` (line 820). Matches the shape produced by `DiscoveryBranchWatcher` so `Vec<SessionRepo>` equality comparisons hold. |
| A12 | Mutex poisoning on `GitWatcher::cache`. | **Pre-existing hazard, not broken by fix.** See Finding 5. |
| A13 | `app.manage()` double-registration returning `false` silently. | **Didn't break.** `app.manage()` returns `bool` (false on duplicate), but the fix only calls it once in the setup block. No path re-registers `Arc<GitWatcher>`. |
| A14 | `GitWatcher::new()` panic before `app.manage()` runs. | **Didn't break.** `GitWatcher::new` is infallible (`Arc::new(Self { ... })`); no I/O, no allocation-beyond-Arc. No panic path. |
| A15 | Worker thread panic (e.g., `tokio::runtime::Runtime::new()` fails at `git_watcher.rs:52-53`). | **Pre-existing.** Panic in the thread is isolated; main thread continues; `start()` has already returned by then. Worker is effectively dead, subsequent polls don't fire, but commands still work. Not introduced by this fix. |

### 10.4 Residual risk

- **Finding 1's flicker in the long tail.** Under heavy load (many simultaneous team edits, or large teams with many coordinator sessions), the flicker window could compound if `GitWatcher` falls behind. I have no evidence this will happen on typical desktop workloads (POLL_INTERVAL = 5s, DETECT_TIMEOUT = 2s, join_all parallelization — plenty of headroom) but did not stress-test. Low residual risk.
- **A10 corner case**: if a user invokes `sync_workgroup_repos` via devtools while *no* Discovery panel is open AND does not subsequently trigger `discover_ac_agents`, a subset of replicas will be missing from `DiscoveryBranchWatcher.replicas` until the next discover. Pre-existing in the newly-reachable code. Not a regression caused by this fix.
- **Serde field naming on the command emit.** The emit at `entity_creation.rs:1049-1055` uses `serde_json::json!` with camelCase string keys (`"sessionId"`, `"repos"`), and `SessionRepo` at `session/session.rs:29-39` carries `#[serde(rename_all = "camelCase")]` so `source_path` → `sourcePath` over the wire. Matches the TS `SessionRepoInput` shape at `src/shared/ipc.ts:18-21`. **No issue.** (Initially flagged during review; verified on re-read.)

### 10.5 Follow-ups to attach to this plan before implementation

1. Rewrite §6.3 Case C expectation per Finding 2.
2. Rewrite §6.3 Case D expectation per Finding 3.
3. File a **separate** ticket titled something like "fix: `sync_workgroup_repos_inner` emits `branch: None` causing badge flicker" for Finding 1. Do **not** bundle into this fix.
4. (Optional, not required) Trace the `source_path` vs `sourcePath` serde question from §10.4 residual risk #3 and document the outcome in a comment on the `SessionRepo` struct.

Ready for implementation after items 1-2 above are applied to §6.3. Item 3 can be filed in parallel. Item 4 is nice-to-have.
