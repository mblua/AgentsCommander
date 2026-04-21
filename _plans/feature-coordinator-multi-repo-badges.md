# Plan: Coordinator-only Multi-Repo Badges

**Branch:** `feature/coordinator-multi-repo-badges`
**Scope:** backend session data model, git watcher, session persistence, teams/coordinator plumbing, shared `Session` type, sidebar renderers, sidebar CSS
**Status:** Ready for implementation — reconciled round 1 (dev-rust §12 + Grinch findings folded in)
**Current HEAD when line numbers were re-anchored:** `313b71e fix(context): unblock messaging dir writes + de-prioritize --help hint`

---

## Requirement

Two user-visible changes bundled as one feature:

1. **Replace `"multi-repo"` static badge with N per-repo badges.**
   For coordinator sessions whose workgroup has N repos, render N badges stacked vertically. Each badge shows `<repo-label>/<branch>`. Repo label = repo dir name with leading `repo-` stripped. Branch is always shown, even when it is the default branch.
2. **Hide the repo/branch badge on every non-coordinator session.**
   Standalone sessions, mono-repo non-coordinator replicas, and multi-repo non-coordinator replicas must render no repo badge at all. Only coordinators display repo badges.

Both rules also apply to the "Coordinator Quick-Access" row in `ProjectPanel` and to the static `AcDiscoveryPanel` list.

---

## Design Summary

Replace the single-branch data model (`git_branch`, `git_branch_source`, `git_branch_prefix`) with a structured per-session list of repos: `git_repos: Vec<SessionRepo>` where `SessionRepo { label, source_path, branch }`. Store an `is_coordinator: bool` on `Session`/`SessionInfo`, computed at session-creation time from the current `discover_teams()` snapshot and recomputed after every discovery/team-CRUD call. Change `GitWatcher` to iterate the session's `git_repos` per tick (each `detect_branch` call wrapped in a 2s timeout) and emit an updated `Vec<SessionRepo>` (with branches filled) as a single `session_git_repos` event. Migrate old `sessions.json` entries on load: single-repo legacy → single `SessionRepo`; `"multi-repo"` legacy → empty list (`DiscoveryBranchWatcher` backfills it on next discovery tick by calling a new `SessionManager::set_git_repos(session_id, Vec<SessionRepo>)`).

The frontend's session creation call sites (`ProjectPanel.tsx`, `AcDiscoveryPanel.tsx`) already compute `repoPaths` per replica — they will now pass the full list (with labels) instead of the single source/prefix pair. `SessionItem.tsx` maps the `Vec` to N stacked badges, gated by `session.isCoordinator`. `ProjectPanel.renderReplicaItem` gates its own `branchLabel()` block on the existing `isCoord()` helper.

This design is strictly additive to the existing coordinator detection already present in `src-tauri/src/config/teams.rs` (`is_any_coordinator`, `is_in_team`, `discover_teams`). It reuses that logic rather than introducing a parallel one.

---

## 1. Data model

### 1.1 `src-tauri/src/session/session.rs`

**Anchors (post-merge HEAD `313b71e`)**
- `TEMP_SESSION_PREFIX` const: line 24
- `Session` struct: lines 28-62 (legacy branch fields at 45, 49, 53)
- `SessionInfo` struct: lines 74-98 (legacy branch fields at 90, 92, 94)
- `impl From<&Session> for SessionInfo`: lines 100-122 (legacy copies at 115-117)

**Add a new public type at the top of the file (after `TEMP_SESSION_PREFIX`, before `Session`):**

```rust
/// One repo watched inside a session, rendered as a single sidebar badge "<label>/<branch>".
/// Populated at session creation time from the replica's `repoPaths`; `branch` is filled
/// and refreshed by `GitWatcher` on each poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRepo {
    /// Repo dir name with leading "repo-" stripped (e.g. "AgentsCommander").
    pub label: String,
    /// Absolute path to the repo root. Branch detection runs `git rev-parse` in this dir.
    pub source_path: String,
    /// Current branch. `None` until first watcher tick, or when detection fails.
    #[serde(default)]
    pub branch: Option<String>,
}
```

**Inside `Session`, REMOVE these three fields (currently lines 45, 49, 53):**

```rust
pub git_branch: Option<String>,
#[serde(default)]
pub git_branch_source: Option<String>,
#[serde(default)]
pub git_branch_prefix: Option<String>,
```

**Add in their place:**

```rust
/// Repos watched by this session. Empty = no repo badge rendered.
#[serde(default)]
pub git_repos: Vec<SessionRepo>,
/// Whether this session's agent is a coordinator of any discovered team.
/// Controls repo-badge visibility on the sidebar. Recomputed after every discovery.
#[serde(default)]
pub is_coordinator: bool,
```

**Do the exact same removal + addition inside `SessionInfo` (currently lines 90, 92, 94).**

**Update `impl From<&Session> for SessionInfo`** to copy `git_repos` and `is_coordinator` in place of the three removed fields.

**Why remove instead of keep-alongside:** the legacy triplet encoded two different things (one repo's source + prefix; a static label for multi-repo). Neither survives the new model. Keeping them as unused shadow fields invites drift. Migration for on-disk TOML/JSON is handled in §6.

### 1.2 `src-tauri/src/session/manager.rs`

**Anchors (post-merge HEAD `313b71e`)**
- `SessionManager::create_session(...)`: lines 26-76
- `set_git_branch(...)`: line 243
- `get_sessions_directories(...)`: line 250

**Changes**

- Replace the `git_branch_source: Option<String>, git_branch_prefix: Option<String>` parameters of `create_session(...)` with `git_repos: Vec<SessionRepo>, is_coordinator: bool`. Write them straight into the constructed `Session`. `git_branch` initial value is no longer set; the `git_repos` list starts with each entry's `branch = None`. Final signature:

  ```rust
  pub async fn create_session(
      &self,
      shell: String,
      shell_args: Vec<String>,
      working_directory: String,
      agent_id: Option<String>,
      agent_label: Option<String>,
      git_repos: Vec<SessionRepo>,
      is_coordinator: bool,
  ) -> Result<Session, AppError>
  ```

- Replace `set_git_branch(id, branch)` with `set_git_repos(id, repos: Vec<SessionRepo>)`. Overwrites the whole list atomically.
- Add a helper `set_is_coordinator(id, is_coordinator: bool)` to be called after a team-config refresh.
- Rename `get_sessions_directories(...)` → `get_sessions_repos(...)` and change its return type to:
  ```rust
  Vec<(Uuid, Vec<SessionRepo>, u64 /* git_repos_gen snapshot */)>
  ```
  (no `working_dir` — `source_path` on each `SessionRepo` is the only thing the watcher consumes). The generation counter is used by `set_git_repos_if_gen` (see §2.1.d) to prevent a stale-snapshot watcher tick from overwriting a refresh. The new watcher iterates each repo's `source_path`; `working_directory` plays no role in branch detection.

- Add the refresh helper described in §2 (`refresh_coordinator_flags`) AND the two helpers described in §2.1.d (`refresh_git_repos_for_sessions`, `set_git_repos_if_gen`).

**Runtime-only field on `Session`**: add `#[serde(skip)] pub git_repos_gen: u64` (initialized to 0 in `SessionManager::create_session`). Do NOT add it to `SessionInfo` (frontend does not need it) or to `PersistedSession` (gen resets on app restart is fine — the whole point is to catch races inside one poll cycle).

**`impl From<&Session> for SessionInfo` field delta** (to pair with §1.1): remove `git_branch`, `git_branch_source`, `git_branch_prefix` copies (3); add `git_repos: s.git_repos.clone()` and `is_coordinator: s.is_coordinator` (2). Net: −1 copy. Both `Session` and `SessionInfo` must declare `git_repos` with the same `#[serde(default)]` attribute so JSON round-trips agree.

---

## 2. Coordinator-flag plumbing

**Decision**: compute `is_coordinator` at session-creation time and recompute whenever team membership can change. **Do not** recompute on every `list_sessions()` call — that would hit disk on every sidebar render.

**Agent name resolution**: reuse `crate::config::teams::agent_name_from_path(&working_directory)`. It already strips the `__agent_`/`_agent_` prefixes and returns `parent/agent`. This is the same function used by the deferred-restore path in `lib.rs:543`, so behavior stays consistent.

**Evaluation helper** — add to `src-tauri/src/config/teams.rs` (near `is_any_coordinator`):

```rust
/// Resolve whether the agent running at `working_directory` is a coordinator of any discovered team.
/// Thin wrapper so call sites don't have to duplicate the `agent_name_from_path` + `is_any_coordinator` pair.
pub fn is_coordinator_for_cwd(working_directory: &str, teams: &[DiscoveredTeam]) -> bool {
    let agent_name = agent_name_from_path(working_directory);
    is_any_coordinator(&agent_name, teams)
}
```

**Refresh helper** — add to `src-tauri/src/session/manager.rs`:

```rust
/// Recompute `is_coordinator` for every session using the current team snapshot.
/// Returns the list of (session_id, new_value) pairs whose flag actually changed,
/// so callers can emit a single event batch.
pub async fn refresh_coordinator_flags(&self, teams: &[crate::config::teams::DiscoveredTeam]) -> Vec<(Uuid, bool)> {
    let mut sessions = self.sessions.write().await;
    let mut changes = Vec::new();
    for (id, s) in sessions.iter_mut() {
        let new_val = crate::config::teams::is_coordinator_for_cwd(&s.working_directory, teams);
        if s.is_coordinator != new_val {
            s.is_coordinator = new_val;
            changes.push((*id, new_val));
        }
    }
    changes
}
```

**Coordinator-changed event payload** — define a typed struct in `src-tauri/src/pty/git_watcher.rs` (or a shared `events` module) so every emit site serializes identically:

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatorChangedPayload {
    pub session_id: String,
    pub is_coordinator: bool,
}
```

All refresh-emit sites use `app.emit("session_coordinator_changed", CoordinatorChangedPayload { session_id: id.to_string(), is_coordinator: new_val })`. Do NOT hand-roll `serde_json::json!(...)` blobs.

**Where to call `refresh_coordinator_flags` from** (scope-refined — only sites where coordinator-ness can actually change):

1. `commands/ac_discovery.rs::discover_ac_agents(...)` — end of the fn, after `branch_watcher.update_replicas(...)`. Resolve `let teams = crate::config::teams::discover_teams();` then call the refresh. For each changed session, emit `session_coordinator_changed`. **MANDATORY.**
2. `commands/ac_discovery.rs::discover_project(...)` — same pattern at end. **MANDATORY.**
3. `commands/entity_creation.rs` — after `create_team`, `update_team`, `delete_team`, `create_workgroup`, `delete_workgroup`. **MANDATORY.**
4. **Do NOT** add the refresh to `sync_workgroup_repos` (line 943 Tauri command) or `sync_workgroup_repos_inner` (line 772) — neither mutates team membership. That path is handled by the separate `git_repos` refresh below.
5. **Do NOT** add the refresh to `delete_agent_matrix` / `create_agent_matrix` — those are guarded by referential integrity and cannot change coordinator-ness of any active session.

**Where to set `is_coordinator` on initial creation:**

1. `commands/session.rs::create_session_inner(...)` — compute once from `discover_teams()` just before calling `mgr.create_session(...)`.
2. `lib.rs` deferred-restore path (lines 540-572, `mgr.create_session(...)` call at 543-551) — the `in_team && !is_coord` branch already has `is_coord` in scope (line 538). Pass it into the new `create_session(...)` signature.
3. `lib.rs` live-restore path (lines 575-600, `create_session_inner(...)` call at 575-589) — `create_session_inner` computes its own `is_coordinator` internally from `discover_teams()`, so no extra arg is needed here. The `teams` vec at line 518 stays scoped to the `start_only_coords` branch — do not expand its scope for this feature.

### 2.1 `git_repos` refresh on workgroup repo mutations (Grinch #4)

`update_team` (entity_creation.rs:709) and the standalone `sync_workgroup_repos` Tauri command (entity_creation.rs:943) both call `sync_workgroup_repos_inner` (line 772), which rewrites every replica's `config.json` `repos` field. The live session's `git_repos` is untouched — the sidebar shows a stale repo count until `DiscoveryBranchWatcher` next polls (≤15s).

**2.1.a `sync_workgroup_repos_inner` signature change** (§12.13.A). Currently synchronous 3-arg. New signature:

```rust
async fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    git_watcher: &Arc<GitWatcher>,
    app: &AppHandle,
) -> Result<SyncResult, String>
```

Both Tauri command callers (`update_team` line 709, `sync_workgroup_repos` line 943) must extend their signatures with `State<'_, Arc<tokio::sync::RwLock<SessionManager>>>`, `State<'_, Arc<GitWatcher>>`, `AppHandle` and forward them. Tauri injects `State` transparently — no frontend `invoke` change is needed.

**2.1.b Per-replica `SessionRepo` production — canonicalization mandate** (§12.13.B). Inside `sync_workgroup_repos_inner`, the current assigned-repo list is built as `Vec<String>` of RELATIVE paths (entity_creation.rs:831-838, e.g. `"../repo-X"`). To keep `Vec<SessionRepo>` PartialEq stable between this writer and `DiscoveryBranchWatcher`, apply the SAME canonicalization `ac_discovery.rs:562-569` uses (canonicalize + strip `\\?\` UNC prefix). Note that the resolution context here is the REPLICA dir, not `wg_path`:

```rust
.filter_map(|rel| {
    let resolved = replica_dir.join(rel);
    std::fs::canonicalize(&resolved).ok()
        .map(|p| {
            let s = p.to_string_lossy();
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
        })
})
.map(|source_path| {
    let dir = source_path.replace('\\', "/").split('/').last().unwrap_or("").to_string();
    let label = if dir.starts_with("repo-") { dir[5..].to_string() } else { dir };
    SessionRepo { label, source_path, branch: None }
})
```

Without this, `source_path` comparisons between the two writers never match and `session_git_repos` re-emits every 5s for every coordinator — silent UI flash.

**2.1.c Partial-failure filter** (Grinch #15). `sync_workgroup_repos_inner` rewrites each replica's `config.json` independently; failures push `SyncError` onto `result.errors` (entity_creation.rs:902-917) and the loop continues. **The refresh list MUST include only replicas whose `config.json` write succeeded.** Collect successful replicas as you go:

```rust
let mut updates: Vec<(String /* session_name */, Vec<SessionRepo>)> = Vec::new();
for replica in replicas_to_update {
    // ... build assigned_repos Vec<SessionRepo> per 2.1.b ...
    match std::fs::write(&replica.config_path, serialized) {
        Ok(()) => {
            result.replicas_updated += 1;
            updates.push((format!("{}/{}", wg.name, replica.name), assigned_repos));
        }
        Err(e) => {
            result.errors.push(SyncError { replica: replica.name.clone(), error: e.to_string() });
            // do NOT push onto `updates` — in-memory state must match on-disk
        }
    }
}
// ... then call refresh + emit using the filtered `updates` list
```

If the in-memory push happened regardless of disk-write outcome, `sessions.json` persistence would carry the NEW list while on-disk `config.json` holds the OLD list. Next discovery tick re-pushes OLD via `set_git_repos`, silently reverting the update across a restart. Non-negotiable.

**2.1.d Helper on `SessionManager` — generation-counter CAS to prevent watcher-refresh races** (Grinch #14):

```rust
// Session struct (add runtime-only field, NOT persisted, NOT sent to frontend):
//
//     #[serde(skip)]  // stays inside the backend; frontend does not see it
//     pub git_repos_gen: u64,
//
// PersistedSession does NOT get this field. SessionInfo does NOT get this field.

/// Replace git_repos for sessions whose name matches. Bumps `git_repos_gen` on every write
/// so an in-flight GitWatcher::poll that loaded the pre-refresh snapshot cannot overwrite us.
/// Returns the list of (session_id, new_repos) pairs where a write actually happened.
pub async fn refresh_git_repos_for_sessions(
    &self,
    updates: &[(String /* session_name */, Vec<SessionRepo>)],
) -> Vec<(Uuid, Vec<SessionRepo>)> {
    let mut sessions = self.sessions.write().await;
    let mut changed = Vec::new();
    for (name, repos) in updates {
        if let Some((id, s)) = sessions.iter_mut().find(|(_, s)| &s.name == name) {
            if &s.git_repos != repos {
                s.git_repos = repos.clone();
                s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
                changed.push((*id, repos.clone()));
            }
        }
    }
    changed
}

/// Compare-and-swap variant used by the watcher. Only writes if the generation recorded
/// at snapshot-time still matches — if a refresh bumped the gen in between, the watcher's
/// stale detection is discarded. Returns true on successful write.
pub async fn set_git_repos_if_gen(&self, id: Uuid, repos: Vec<SessionRepo>, expected_gen: u64) -> bool {
    let mut sessions = self.sessions.write().await;
    if let Some(s) = sessions.get_mut(&id) {
        if s.git_repos_gen == expected_gen {
            s.git_repos = repos;
            s.git_repos_gen = s.git_repos_gen.wrapping_add(1);
            return true;
        }
    }
    false
}
```

The companion change in §1.2 `get_sessions_repos` now returns `Vec<(Uuid, Vec<SessionRepo>, u64 /* gen */)>` so the watcher captures the snapshot-time generation. `GitWatcher::poll` uses `set_git_repos_if_gen` instead of the unchecked `set_git_repos` (details in §3.1 / §3.2.4). On gen mismatch: log `debug!`, skip both the write AND the emit — a refresh just landed and emitted its own event.

**Why generation counter over Vec-CAS**: a Vec-equality CAS (option c in Grinch #14) would fire false-positive "changed" when the freshly-detected branches match the pre-refresh list verbatim. A monotonic gen captures the intent ("a refresh happened since your snapshot") without equality ambiguity. One `u64` field; wrapping add handles any lifetime.

**2.1.e Callers and post-refresh flow**:

- After 2.1.c filtering and the `refresh_git_repos_for_sessions` call, iterate the returned `changed` list. For each entry:
  - Call `git_watcher.invalidate_session_cache(session_id)` so the next `GitWatcher` tick does not spuriously re-fire on a stale cache hit.
  - `app.emit("session_git_repos", SessionGitReposPayload { session_id, repos })`.
- **Also call `discovery_branch_watcher.invalidate_replicas(&affected_replica_paths)`** — where `affected_replica_paths` is the list of `replica_path`s (absolute paths to the `__agent_*` dirs) whose `config.json` was just rewritten. This scrubs the matching entries from the `DiscoveryBranchWatcher`'s own `replicas` / `discovery_cache` / `repos_cache` so the next watcher tick does NOT iterate stale `source_path`s and write OLD repos back via a valid-gen CAS (Grinch #17). The entries are re-registered by the next `discover_project` call — the frontend already triggers one via `reloadProject` after `updateTeam` returns (EditTeamModal.tsx:206). See §3.2.5 for the helper.
- Both callers of `sync_workgroup_repos_inner` (`update_team` line 755 and the standalone `sync_workgroup_repos` command line 987) inherit this behavior via the new inner signature — no separate wiring.

**Resolution of §12.7 vs Grinch #4 disagreement**: both are needed and non-overlapping. `refresh_coordinator_flags` handles flag changes on team CRUD. `refresh_git_repos_for_sessions` handles repo-list changes on `sync_workgroup_repos_inner`. `update_team` (which internally triggers both) is covered by the separate call sites.

---

## 3. `git_watcher` fan-out

**Decision**: one polling task that iterates **(session, repo)** pairs. Emit one event per session when *any* of its repos changed. This keeps one task per app (matches today), avoids a task-per-repo proliferation, and keeps the watcher simple.

### 3.1 `src-tauri/src/pty/git_watcher.rs`

**Anchors**
- `GitWatcher` struct + cache: lines 12-16
- `GitBranchPayload`: lines 18-23
- `poll(...)`: lines 62-108
- `detect_branch(...)`: lines 110-134

**Replace the cache** from `HashMap<Uuid, Option<String>>` to:

```rust
cache: Mutex<HashMap<Uuid, Vec<SessionRepo>>>,
```

Value is the last-emitted per-repo state for a session; `changed` is evaluated by `Vec` equality (`PartialEq` derived on `SessionRepo`).

**Replace the emitted payload** with:

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GitReposPayload {
    session_id: String,
    repos: Vec<SessionRepo>,
}
```

Emit event name `"session_git_repos"` (new). Deprecate the old `"session_git_branch"` event — no frontend listener remains once §7 lands.

**Replace `poll(...)` body** (keep the outer select! loop, POLL_INTERVAL, and thread bootstrap unchanged):

```rust
async fn poll(&self) {
    let sessions: Vec<(Uuid, Vec<SessionRepo>, u64)> = {
        let mgr = self.session_manager.read().await;
        mgr.get_sessions_repos().await
    };

    for (id, repos, gen_snapshot) in sessions {
        if repos.is_empty() {
            // Nothing to watch. If cache still has this id, clear it so a later
            // "repos appeared" transition re-emits.
            let mut cache = self.cache.lock().unwrap();
            cache.remove(&id);
            continue;
        }

        // Parallelize per-repo detection (Grinch #16). Each call is individually bounded
        // by `detect_branch_with_timeout` (2s, §3.1.1). Without join_all, worst-case per
        // poll is M×N×2s under simultaneous stalls.
        let branches: Vec<Option<String>> = futures::future::join_all(
            repos.iter().map(|r| Self::detect_branch_with_timeout(&r.source_path))
        ).await;

        let refreshed: Vec<SessionRepo> = repos.iter().zip(branches.into_iter())
            .map(|(r, branch)| SessionRepo {
                label: r.label.clone(),
                source_path: r.source_path.clone(),
                branch,
            })
            .collect();

        let changed = {
            let cache = self.cache.lock().unwrap();
            cache.get(&id) != Some(&refreshed)
        };

        if changed {
            // CAS write — if a refresh bumped the gen between our snapshot and now,
            // the write + emit are skipped. Prevents the stale-overwrite race (Grinch #14).
            let wrote = {
                let mgr = self.session_manager.read().await;
                mgr.set_git_repos_if_gen(id, refreshed.clone(), gen_snapshot).await
            };

            if wrote {
                let _ = self.app_handle.emit(
                    "session_git_repos",
                    GitReposPayload { session_id: id.to_string(), repos: refreshed.clone() },
                );
                self.cache.lock().unwrap().insert(id, refreshed);
            } else {
                log::debug!(
                    "[GitWatcher] gen mismatch on session {} — refresh landed during poll; skipping stale emit",
                    id
                );
                // Invalidate our cache so the next tick re-evaluates against the refreshed list.
                self.cache.lock().unwrap().remove(&id);
            }
        }
    }
}
```

**Dependencies**: `futures` is already in-tree (used elsewhere). Verify with `cargo tree | grep futures` — if absent add `futures = "0.3"` to `Cargo.toml`.

**`remove_session(id)` stays.** The new cache type still supports `.remove(&id)`.

**Add `invalidate_session_cache(&self, id: Uuid)`** (tiny helper next to `remove_session`, same body: `self.cache.lock().unwrap().remove(&id);`). Used by `refresh_git_repos_for_sessions` callers in §2.1 to force a re-emit on the next tick.

### 3.1.1 `detect_branch_with_timeout` — new helper (replaces the "keep `detect_branch` unchanged" note)

Wrap every `git rev-parse` call in `tokio::time::timeout(Duration::from_secs(2), ...)`. On timeout treat as `None` (same as failure) and log a `warn!` with the repo path. One bad repo cannot stall others.

**Change required in `detect_branch` itself** (Grinch #13): the current implementation at git_watcher.rs:114-121 spawns `tokio::process::Command::output()` without `.kill_on_drop(true)`. When the outer `tokio::time::timeout` fires, tokio drops the pending future; the `Child` handle is dropped WITHOUT killing the child. `git.exe` keeps running to completion in the OS. On a sustained stall, every poll tick spawns a fresh git.exe that never returns — thousands of zombie processes per hour.

**Fix**: add `.kill_on_drop(true)` to the `Command` builder in the EXISTING `detect_branch` before `cmd.output().await`. Apply to BOTH `git_watcher.rs:114-121` AND `ac_discovery.rs:392-399`.

```rust
async fn detect_branch(working_dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(working_dir)
        .kill_on_drop(true);  // <-- ADDED. Terminates git.exe when the future is dropped by timeout.

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    // ... rest unchanged ...
}

async fn detect_branch_with_timeout(working_dir: &str) -> Option<String> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Self::detect_branch(working_dir),
    ).await {
        Ok(result) => result,
        Err(_) => {
            log::warn!(
                "[GitWatcher] detect_branch timed out for {} (>2s); treating as no-branch",
                working_dir
            );
            None
        }
    }
}
```

**On Windows specifics**: `kill_on_drop(true)` on `tokio::process::Command` calls `TerminateProcess` via the Child's Drop impl. This has been stable on Windows since tokio 1.0. If a future tokio release breaks it, the fallback is explicit `child.start_kill()` inside a `tokio::select!` block (see Grinch #13 for the exact alternative).

The new helper composes around `detect_branch`. Apply the SAME treatment (both `kill_on_drop` and the timeout wrapper) in `DiscoveryBranchWatcher` — mirror the helper there.

### 3.1.2 `git_repos` ordering invariant (Grinch #7)

The `changed` gate relies on `Vec<SessionRepo>` equality, which is order-sensitive. Document and enforce:

> **Invariant**: `git_repos` order is the order of the replica's `config.json` `repos` array. Never sort, never dedupe via `HashMap`, never rebuild from a set. Writers preserve insertion order: `update_replicas` in §3.2, `refresh_git_repos_for_sessions` in §2.1, and the frontend `gitRepos.map(...)` in §4.4 all inherit this order directly from `replica.repoPaths`.

Add a one-line comment to this effect above `set_git_repos` and `update_replicas`.

### 3.2 `src-tauri/src/commands/ac_discovery.rs` — `DiscoveryBranchWatcher`

The discovery watcher (lines 213-411) updates **un-instantiated** replicas (for the non-session replica list UI) AND pushes to matching sessions on lines 364-383. It currently only handles single-repo replicas (line 240 condition `repo_paths.len() == 1`).

**3.2.1 Widen `ReplicaBranchEntry`** to carry the full repo list:

```rust
#[derive(Clone)]
struct ReplicaBranchEntry {
    replica_path: String,
    /// (label, absolute repo path) pairs. Order = replica config.json `repos` array order.
    repos: Vec<(String, String)>,
    /// Session name format: "wg_name/replica_name"
    session_name: String,
}
```

**3.2.2 Widen both caches** to detect per-repo change independently (§12.2, Grinch #3):

```rust
pub struct DiscoveryBranchWatcher {
    app_handle: AppHandle,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    /// Keyed by the **project directory that contains `.ac-new/`** — NOT by the user-configured
    /// `settings.project_paths` entry. A `project_paths` entry can be a parent that holds multiple
    /// projects (e.g. `"C:/repos"` with `C:/repos/proj-A/.ac-new` and `C:/repos/proj-B/.ac-new`).
    /// Keying by base_path would bucket all children under one entry AND allow the same replica
    /// to land under two keys if both a parent and a child path appear in settings (Grinch #12).
    /// Invariant: every key directly contains a `.ac-new/` subdir.
    replicas: Mutex<HashMap<String /* ac_new_parent_dir */, Vec<ReplicaBranchEntry>>>,
    /// Single-repo branch cache — gates `ac_discovery_branch_updated` emission for the panel UI.
    discovery_cache: Mutex<HashMap<String /* replica_path */, Option<String>>>,
    /// Full per-repo state cache — gates `session_git_repos` emission. Independent from discovery_cache
    /// so multi-repo replicas can re-emit on per-repo branch drift even when the single-branch view is None.
    repos_cache: Mutex<HashMap<String /* replica_path */, Vec<SessionRepo>>>,
}
```

The old unified `cache: Mutex<HashMap<String, Option<String>>>` is replaced.

**3.2.3 Fix multi-project overwrite (Grinch #1 + #12) — switch `update_replicas` to per-ac_new-dir keyed merge:**

Replace the current signature `update_replicas(&self, workgroups: &[AcWorkgroup])` with:

```rust
/// Update this project's replicas in the watcher. `ac_new_parent_dir` is the directory that
/// directly contains `.ac-new/` — NOT a grand-parent from `settings.project_paths`. See the
/// invariant comment on the `replicas` field.
pub fn update_replicas_for_project(&self, ac_new_parent_dir: &str, workgroups: &[AcWorkgroup]) {
    debug_assert!(
        std::path::Path::new(ac_new_parent_dir).join(".ac-new").is_dir(),
        "update_replicas_for_project: {} does not contain .ac-new/",
        ac_new_parent_dir
    );

    // Invariant: git_repos order = replica.repo_paths order (which follows config.json `repos`).
    // Never sort or dedupe here. See §3.1.2.
    let mut entries = Vec::new();
    for wg in workgroups {
        for agent in &wg.agents {
            if agent.repo_paths.is_empty() { continue; }
            let repos: Vec<(String, String)> = agent.repo_paths.iter()
                .map(|rp| {
                    let dir = rp.replace('\\', "/").split('/').last().unwrap_or("").to_string();
                    let label = if dir.starts_with("repo-") { dir[5..].to_string() } else { dir };
                    (label, rp.clone())
                })
                .collect();
            entries.push(ReplicaBranchEntry {
                replica_path: agent.path.clone(),
                repos,
                session_name: format!("{}/{}", wg.name, agent.name),
            });
        }
    }

    // Swap in this project's entries; leave other projects alone.
    let mut map = self.replicas.lock().unwrap();
    map.insert(ac_new_parent_dir.to_string(), entries);

    // Prune cache entries that no longer belong to ANY project.
    let valid: std::collections::HashSet<String> = map.values()
        .flatten()
        .map(|e| e.replica_path.clone())
        .collect();
    drop(map);
    self.discovery_cache.lock().unwrap().retain(|k, _| valid.contains(k));
    self.repos_cache.lock().unwrap().retain(|k, _| valid.contains(k));
}
```

**Callers adjusted** — both must pass the `.ac-new/`-containing dir, never a `settings.project_paths` entry:

- `discover_ac_agents` (line 688) walks `cfg.project_paths` + immediate children (ac_discovery.rs:431-444) and builds workgroups for all discovered project dirs. Inside the walk, the variable `repo_dir` (line 447) IS the correct key — it is the directory where `.ac-new/` was found. Group workgroups by that during the walk: `HashMap<String /* repo_dir */, Vec<AcWorkgroup>>`. After the outer loop, iterate the map and call `update_replicas_for_project(&repo_dir_str, &wgs)` per entry. This prevents the ambiguity where `base_path = "C:/repos"` with children `proj-A` and `proj-B` would bucket all their workgroups under one key AND prevents the double-registration that occurs when `settings.project_paths` contains both a parent and a child (Grinch #12).
- `discover_project(path, ...)` (line 1020) already targets a single project — `path` IS the project dir containing `.ac-new/`. Call `update_replicas_for_project(&path, &workgroups)`.

**Invariant enforcement**: the `debug_assert!` above catches mistaken call-site passes (e.g. a `base_path` parent) in dev builds. In release builds the assert is a no-op and the caller-side discipline carries the contract — add an `if !Path::new(...).join(".ac-new").is_dir() { log::warn!(...); return; }` guard at the top of the fn to prevent silent corruption if a future caller slips.

**3.2.4 `poll(...)` — parallel detection, independent change gates, CAS-gen on session emit**

```rust
async fn poll(&self) {
    // Flatten per-project entries.
    let entries: Vec<ReplicaBranchEntry> = {
        let map = self.replicas.lock().unwrap();
        map.values().flatten().cloned().collect()
    };
    if entries.is_empty() { return; }

    for entry in &entries {
        // Capture the session's git_repos_gen (if a session exists) BEFORE running detections.
        // Used for CAS on set_git_repos_if_gen (Grinch #14).
        let (session_id_opt, gen_snapshot) = {
            let mgr = self.session_manager.read().await;
            match mgr.find_by_name(&entry.session_name).await {
                Some(id) => {
                    let gen = mgr.get_git_repos_gen(id).await.unwrap_or(0);
                    (Some(id), gen)
                }
                None => (None, 0),
            }
        };

        // Parallelize per-repo detection (Grinch #16). Each call individually bounded by
        // detect_branch_with_timeout (2s). Without join_all this was M×N×2s worst case.
        let branches: Vec<Option<String>> = futures::future::join_all(
            entry.repos.iter().map(|(_, path)| Self::detect_branch_with_timeout(path))
        ).await;

        let refreshed: Vec<SessionRepo> = entry.repos.iter().zip(branches.into_iter())
            .map(|((label, path), branch)| SessionRepo {
                label: label.clone(),
                source_path: path.clone(),
                branch,
            })
            .collect();

        // --- Gate A: emit ac_discovery_branch_updated (single-branch UI for AcDiscoveryPanel) ---
        // Only single-repo replicas surface a branch here; multi-repo = None so the panel hides it.
        let discovery_branch: Option<String> = if entry.repos.len() == 1 {
            refreshed[0].branch.clone()
        } else {
            None
        };
        let discovery_changed = {
            let mut cache = self.discovery_cache.lock().unwrap();
            let prev = cache.get(&entry.replica_path).cloned();
            if prev.as_ref() != Some(&discovery_branch) {
                cache.insert(entry.replica_path.clone(), discovery_branch.clone());
                true
            } else { false }
        };
        if discovery_changed {
            let _ = self.app_handle.emit(
                "ac_discovery_branch_updated",
                DiscoveryBranchPayload {
                    replica_path: entry.replica_path.clone(),
                    branch: discovery_branch,
                },
            );
        }

        // --- Gate B: emit session_git_repos (full per-repo state for SessionItem) ---
        // Independent cache so multi-repo replicas re-emit on per-repo drift even when Gate A stays None.
        let repos_changed = {
            let mut cache = self.repos_cache.lock().unwrap();
            let prev = cache.get(&entry.replica_path);
            if prev != Some(&refreshed) {
                cache.insert(entry.replica_path.clone(), refreshed.clone());
                true
            } else { false }
        };
        if repos_changed {
            if let Some(session_id) = session_id_opt {
                // CAS write: skip if a refresh bumped gen during our detection window (Grinch #14).
                let wrote = {
                    let mgr = self.session_manager.read().await;
                    mgr.set_git_repos_if_gen(session_id, refreshed.clone(), gen_snapshot).await
                };
                if wrote {
                    let _ = self.app_handle.emit(
                        "session_git_repos",
                        SessionGitReposPayload {
                            session_id: session_id.to_string(),
                            repos: refreshed.clone(),
                        },
                    );
                } else {
                    log::debug!(
                        "[DiscoveryBranchWatcher] gen mismatch for {} — refresh landed during poll; skipping stale emit",
                        entry.replica_path
                    );
                    // Clear our own cache entry so next tick re-evaluates against the fresh list.
                    self.repos_cache.lock().unwrap().remove(&entry.replica_path);
                }
            }
            // If no session exists yet (un-instantiated replica), Gate A has already covered
            // the display surface — no session to push git_repos into.
        }
    }
}
```

Rename `SessionGitBranchPayload` (line 207-211) → `SessionGitReposPayload` with `repos: Vec<SessionRepo>` instead of `branch: Option<String>`.

**New `SessionManager::get_git_repos_gen(id)` helper** — returns `Option<u64>`. Trivial read-only wrapper; add next to `get_session` in manager.rs. Required so both watchers can snapshot the gen without holding a write lock during detection.

### 3.2.5 `invalidate_replicas` — scrub stale discovery entries post-refresh (Grinch #17)

`gen`-CAS (§2.1.d) guards against writers whose snapshot PREDATES a refresh. It does NOT guard against `DiscoveryBranchWatcher::poll` running AFTER a refresh but iterating a STALE `self.replicas` map — that map is populated only by `update_replicas_for_project` from `discover_*`, and `refresh_git_repos_for_sessions` does not refresh it. A poll in the ~500ms window between the refresh and the frontend's follow-up `reloadProject` detects branches on OLD `source_path`s, captures the post-refresh gen, and its CAS write SUCCEEDS — regressing the session back to the OLD list (NEW → OLD → NEW flicker; `sessions.json` may persist OLD if `snapshot_sessions` lands in the window).

**Add to `DiscoveryBranchWatcher`** (place alongside `update_replicas_for_project` in §3.2.3):

```rust
/// Remove the specified replicas from `replicas`, `discovery_cache`, and `repos_cache`.
/// Called by `refresh_git_repos_for_sessions` callers (§2.1.e) so the next watcher tick
/// does not iterate stale `source_path`s between a session-level refresh and the
/// follow-up `discover_project` call that re-registers the replicas with NEW paths.
///
/// Paths absent from `replicas` after this call are silently re-inserted on the next
/// `update_replicas_for_project` for their project — no caller coordination needed beyond
/// the existing frontend `reloadProject` flow.
pub fn invalidate_replicas(&self, replica_paths: &[String]) {
    {
        let mut map = self.replicas.lock().unwrap();
        for entries in map.values_mut() {
            entries.retain(|e| !replica_paths.iter().any(|p| p == &e.replica_path));
        }
    }
    {
        let mut dc = self.discovery_cache.lock().unwrap();
        let mut rc = self.repos_cache.lock().unwrap();
        for p in replica_paths {
            dc.remove(p);
            rc.remove(p);
        }
    }
    log::debug!(
        "[DiscoveryBranchWatcher] invalidated {} replica(s); awaiting next discover_project re-registration",
        replica_paths.len()
    );
}
```

**Lock ordering**: `replicas` lock taken first, released before `discovery_cache` + `repos_cache` are taken. Matches `poll()`'s own lock discipline (poll does `map.lock → .cloned().flatten()` then drops before touching caches). No deadlock.

**Gap handling**: between `invalidate_replicas` and the next `discover_project`, `DiscoveryBranchWatcher::poll` does nothing for the invalidated replicas. `GitWatcher` (the per-session watcher that iterates `SessionManager.sessions[*].git_repos` — which now hold the NEW repo list after `refresh_git_repos_for_sessions`) continues polling them correctly, so branches still update. If the frontend's `reloadProject` never fires (edge case), branches still resolve via `GitWatcher`; only the un-instantiated-replica panel display of `AcDiscoveryPanel` pauses updates for those replicas — acceptable.

**Why this doesn't re-introduce #14**: `GitWatcher`'s CAS path remains intact. Gen-CAS covered the `GitWatcher` race (Grinch #14). `invalidate_replicas` covers the orthogonal `DiscoveryBranchWatcher` race (Grinch #17). Both guards coexist.

**Why option (a) over (b)/(c)**: option (b) — synchronously call `discover_project` from inside `sync_workgroup_repos_inner` — couples the mutation path to discovery infrastructure, grows lock contention, and requires threading another `State<>` through. Option (c) — per-replica source-epoch on `ReplicaBranchEntry` plus matching CAS arg — doubles the CAS complexity for a bug that option (a) fixes in ~15 lines. Option (a) is localized, already-modified code, and the `invalidate_replicas` API is reusable for any future mutation path that needs the same guarantee.

**Why two gates**: §12.2 and Grinch #3 both flagged that the old single-`Option<String>` cache cannot detect per-repo drift. The fix is to track both: the single-branch view (for the legacy `ac_discovery_branch_updated` event consumed by `AcDiscoveryPanel`) AND the full-repo view (for the new `session_git_repos` event consumed by `SessionItem`). Gate A guards the panel emission; Gate B guards the session emission; neither starves the other.

**Why both events survive:** `AcDiscoveryPanel.tsx` listens for `ac_discovery_branch_updated` to update its un-instantiated replica-list display. That panel is still the replica picker shown when a workgroup has no session yet — keep it working. `SessionItem.tsx` subscribes to the new `session_git_repos` event.

---

## 4. Session creation call sites

### 4.1 `src-tauri/src/commands/session.rs`

**Anchors**
- `create_session_inner(...)`: lines 194-519
- `create_session(...)` Tauri command: lines 523-627
- `restart_session(...)` Tauri command: lines 715-841
- `create_root_agent_session(...)`: lines 1063-1200

**New inner signature** (replace two params with one list + coordinator flag):

```rust
pub async fn create_session_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    skip_tooling_save: bool,
    git_repos: Vec<SessionRepo>,          // replaces git_branch_source + git_branch_prefix
    skip_auto_resume: bool,
) -> Result<SessionInfo, String>
```

`is_coordinator` is computed **inside** `create_session_inner` from a fresh `discover_teams()` call, not taken as a parameter. Rationale: there's exactly one correct source for this — the on-disk teams config — and having every caller compute it independently invites drift.

```rust
// Near the top of create_session_inner, after resolving agent identity:
let teams = crate::config::teams::discover_teams();
let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);
```

Pass `git_repos` and `is_coordinator` into `SessionManager::create_session(...)`.

**New Tauri command signature** for `create_session`:

```rust
#[tauri::command]
pub async fn create_session(
    /* ...existing state params... */
    shell: Option<String>,
    shell_args: Option<Vec<String>>,
    cwd: Option<String>,
    session_name: Option<String>,
    agent_id: Option<String>,
    git_repos: Option<Vec<SessionRepo>>,   // replaces git_branch_source + git_branch_prefix
) -> Result<SessionInfo, String>
```

Default when `None` is `Vec::new()`.

**`restart_session`**: read the existing session's `git_repos` before destruction (current reads are on lines 728-750) and pass them back into `create_session_inner`. The coordinator flag is recomputed fresh inside `create_session_inner`, so the restarted session picks up any team-config change that happened since.

**`create_root_agent_session`**: pass `Vec::new()` for `git_repos`. Root-agent path has no repos to watch.

### 4.2 Other `create_session_inner` callers (pass empty vec)

- `src-tauri/src/web/commands.rs` lines 79-80: replace `None, None` with `vec![]`.
- `src-tauri/src/phone/mailbox.rs` — **two** call sites (not three): lines 524-525 and 1481-1482. Replace `None, None` with `vec![]` at each.

These are web-remote and mailbox-driven paths that do not know the replica's repo layout. Leaving `git_repos` empty produces the correct "no badge" behavior.

### 4.3 `src-tauri/src/lib.rs` — restore paths

**Anchors (post-merge HEAD `313b71e`)**
- `teams` loaded for `start_only_coords`: line 518
- Deferred restore branch (`in_team && !is_coord`): lines 540-572, with `mgr.create_session(...)` call at 543-551 (legacy branch args at 549-550)
- Live restore: lines 575-600, with `create_session_inner(...)` call at 575-589 (legacy branch args at 586-587)

**Deferred restore** currently calls `mgr.create_session(...)` directly. Update it to build `Vec<SessionRepo>` from the persisted session (see §6 migration) and pass `is_coord` (already computed on line 538 as `crate::config::teams::is_any_coordinator(&agent_name, &teams)`) as the `is_coordinator` argument. Note: `is_coord` in this branch is always `false` because the branch condition is `in_team && !is_coord` — so pass `false` literally, or pass the variable for clarity.

**Live restore** calls `create_session_inner(...)`. Replace the two positional legacy args (lines 586-587) with a single `ps.git_repos.clone()` arg. The coordinator flag is recomputed inside `create_session_inner` via `discover_teams()` — no extra work here. Do NOT expand the `teams` variable's scope out of the `start_only_coords` branch.

### 4.4 Frontend create sites

- `src/sidebar/components/ProjectPanel.tsx` — two sites:
  - `handleReplicaClick` lines 121-149 (direct session creation). `gitBranchSource` at 122, `gitBranchPrefix` at 123, computed at 126-131, passed at 137-138 and 147-148.
  - Pending-launch modal callback lines 1306-1318 (`gitBranchSource`/`gitBranchPrefix` at 1316-1317).

  Replace the `gitBranchSource` + `gitBranchPrefix` computation with:

  ```ts
  const gitRepos = (replica.repoPaths ?? []).map((p) => {
    const dir = p.replace(/\\/g, "/").split("/").pop() ?? "";
    const label = dir.startsWith("repo-") ? dir.slice(5) : dir;
    return { label, sourcePath: p };
  });
  ```

  Pass `gitRepos` through `SessionAPI.create({ ..., gitRepos })`. Update the `PendingLaunch` interface (lines 18-23) and the pending-launch payload accordingly.

- `src/sidebar/components/AcDiscoveryPanel.tsx` lines 49-79, 372-380 — mirror the exact same change.

- `src/shared/ipc.ts` `CreateSessionOptions` (lines 20-28): replace `gitBranchSource?: string; gitBranchPrefix?: string;` with `gitRepos?: SessionRepoInput[];` where `SessionRepoInput = { label: string; sourcePath: string }`. Update the body of `SessionAPI.create` to send `gitRepos: opts?.gitRepos ?? null`.

- `src/shared/types.ts`: add

  ```ts
  export interface SessionRepo {
    label: string;
    sourcePath: string;
    branch: string | null;
  }
  ```

  Add `gitRepos: SessionRepo[]` and `isCoordinator: boolean` to `Session`. REMOVE `gitBranch`, `gitBranchSource`, `gitBranchPrefix`.

- `src/sidebar/stores/sessions.ts` `makeInactiveEntry` (lines 31-49): initialize `gitRepos: []` and `isCoordinator: false`. Remove the three legacy-field initializations.

- `src/sidebar/stores/sessions.ts` `setGitBranch` (lines 302-304): replace with `setGitRepos(sessionId: string, repos: SessionRepo[])` using the same `setState(...)` pattern but on the new `gitRepos` field. Add `setIsCoordinator(sessionId: string, value: boolean)` mirror.

---

## 5. Frontend rendering

### 5.1 `src/sidebar/components/SessionItem.tsx`

**Anchor**: lines 300-320 (the `session-item-meta` block).

**Rules**
- Do NOT derive anything from `props.session.gitBranch`, `gitBranchSource`, `gitBranchPrefix`. Those fields no longer exist.
- Render the repo-badge block only when `props.session.isCoordinator === true`.
- For non-coordinator sessions, keep the `agent-badge` row intact — only the repo badges disappear.

**Replacement block** (in place of lines 300-320):

```tsx
<Show when={!isRecording() && !isProcessing() && !isAutoExecuting() && !isTypingWarning() && !voiceRecorder.micError()}>
  <Show when={sessionAgentLabel() || (props.session.isCoordinator && !isInactive() && props.session.gitRepos.length > 0)}>
    <div class="session-item-meta">
      <Show when={sessionAgentLabel()}>
        {(agentLabel) => (
          <span class={`agent-badge ${sessionHasLivePty() ? "running" : ""}`} data-agent={agentLabel()}>
            {agentLabel()}
          </span>
        )}
      </Show>
      <Show when={props.session.isCoordinator && !isInactive() && props.session.gitRepos.length > 0}>
        <div class="session-item-branches">
          <For each={props.session.gitRepos}>
            {(repo) => (
              <div class="session-item-branch" title={`${repo.label}${repo.branch ? `/${repo.branch}` : ""}`}>
                {repo.label}{repo.branch ? `/${repo.branch}` : ""}
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  </Show>
</Show>
```

### 5.2 `src/sidebar/components/ProjectPanel.tsx` — `renderReplicaItem`

**Anchor**: `renderReplicaItem` starts at line 396. Specifically: `isCoord()` at line 402, `rn()` at line 403, `branchLabel()` helper at lines 404-412, `.ac-discovery-badges` block at lines 489-502 (`branchLabel` rendered at 490-491). Coordinator Quick-Access reuse at lines 646 and 698.

**Changes**
- Gate the branch badge block on `isCoord()` (already defined on line 402). When `!isCoord()`, render NO branch badge.
- When the session exists AND `s.gitRepos.length > 0`, render one `ac-discovery-badge branch` per repo (vertically stacked via the new CSS from §5.4).
- When the session does NOT exist yet (unopened replica), keep the existing discovery-based branch fallback but ONLY show it when:
  - `isCoord()` AND `replica.repoPaths.length === 1` AND `replica.repoBranch` is set. Format: `<repoLabel>/<repoBranch>`.
  - For multi-repo coordinator replicas with no session, render nothing. The discovery pass already filled `replica.repoBranch` only for single-repo cases (see `detect_git_branch_sync` call site); replicating a full per-repo discovery just for un-instantiated multi-repo replicas is out of scope.
  - For non-coordinator replicas (any repo count), render nothing.

**Replacement sketch for the badge block inside `renderReplicaItem`:**

```tsx
<div class="ac-discovery-badges">
  <Show when={isCoord()}>
    <Show
      when={session() && session()!.gitRepos.length > 0}
      fallback={
        <Show when={replica.repoPaths.length === 1 && replica.repoBranch}>
          <span class="ac-discovery-badge branch">
            {rn()}/{replica.repoBranch}
          </span>
        </Show>
      }
    >
      <For each={session()!.gitRepos}>
        {(repo) => (
          <span class="ac-discovery-badge branch">
            {repo.label}{repo.branch ? `/${repo.branch}` : ""}
          </span>
        )}
      </For>
    </Show>
  </Show>
  <Show when={liveAgentLabel()}>
    <span class="ac-discovery-badge agent">{liveAgentLabel()}</span>
  </Show>
  <Show when={isCoord()}>
    <span class="ac-discovery-badge coord">coordinator</span>
  </Show>
  <Show when={extraBadge}>
    <span class="ac-discovery-badge team">{extraBadge}</span>
  </Show>
</div>
```

**Note**: the `rn()` helper (line 403) already computes the repo label the same way (`stripRepoPrefix`). Keep it.

**Coordinator Quick-Access** (lines 630-651) reuses `renderReplicaItem`, so no separate change is needed. It gets the new gating for free.

### 5.3 `src/sidebar/components/AcDiscoveryPanel.tsx`

**Anchor**: replica render inside workgroup — `branchLabel` at line 292-296, badge rendering at lines 307-308 (inside the `For` on workgroup agents around line 290-316). `handleReplicaClick` at line 49. `isCoordinator(agent.name)` helper at line 31 (currently applies to agent-matrix list only, not replicas).

**Changes**
- Apply the same coordinator gate. A replica is a coordinator if its `{originProject}/{replica.name}` matches any team's `coordinator` field. Use a TWO-pass helper that mirrors the backend suffix fallback (ac_discovery.rs:666-684) to handle missing/transient `originProject` (Grinch #9):

  ```ts
  const isReplicaCoord = (replica: AcAgentReplica): boolean => {
    // Pass 1: exact match when originProject is known.
    if (replica.originProject) {
      const ref = `${replica.originProject}/${replica.name}`;
      if (teams().some((t) => t.coordinator === ref)) return true;
    }
    // Pass 2: suffix fallback — covers canonicalize failures, missing identity,
    // or absolute-path coordinator refs from another project. Mirrors the
    // backend's is_coordinator WG-aware suffix rule.
    const suffixHit = teams().some(
      (t) => t.coordinator?.split("/").pop() === replica.name
    );
    if (suffixHit && !replica.originProject) {
      // Log once so users with transient identity resolution failures can trace
      // why a coordinator badge still renders despite missing originProject.
      console.warn(
        "[AcDiscoveryPanel] replica treated as coordinator via suffix fallback; originProject missing",
        replica.path
      );
    }
    return suffixHit;
  };
  ```

- Replace the `branchLabel()` computation (lines 292-296) with a per-repo `For`, gated on `isReplicaCoord(replica)`:

  ```tsx
  <Show when={isReplicaCoord(replica)}>
    <Show when={replica.repoPaths.length === 1 && replica.repoBranch}>
      <span class="ac-discovery-badge branch">
        {(() => {
          const dir = replica.repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
          const label = dir.startsWith("repo-") ? dir.slice(5) : dir;
          return `${label}/${replica.repoBranch}`;
        })()}
      </span>
    </Show>
    {/* Multi-repo in this panel intentionally omitted — see §5.2 rationale */}
  </Show>
  ```

- The `session_git_repos` event is NOT consumed here. This panel only shows un-instantiated replicas based on discovery data; the sidebar `SessionItem` is the canonical live view.

### 5.4 `src/sidebar/styles/sidebar.css`

**Anchor**: `.session-item-meta` rule at line 440-443, `.session-item-branch` rule at lines 430-438.

**Add** a vertical stack container for multi-repo badges in `SessionItem`:

```css
.session-item-branches {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
```

**Change** `.session-item-meta`:

```css
.session-item-meta {
  display: flex;
  align-items: flex-start;   /* was: center — needed now that the branches column can be taller */
  gap: 6px;
  flex-wrap: wrap;
  min-width: 0;
}
```

**Leave `.session-item-branch` unchanged** — its existing `overflow: hidden; text-overflow: ellipsis` is what we want for each badge.

For `ProjectPanel`'s `.ac-discovery-badges`: NO structural change is required — that container already wraps on multiple lines (`flex-wrap: wrap` in the existing rule). Each `<span class="ac-discovery-badge branch">` becomes its own flex child, producing the vertical stack effect when badges can't fit on one line. If reviewers want explicit stacking, add:

```css
.ac-discovery-badges .ac-discovery-badge.branch + .ac-discovery-badge.branch {
  margin-left: 0;
  margin-top: 2px;
}
```

Only if validation shows horizontal flow. Defer until manual QA.

### 5.5 Event subscription

Add a new listener in `src/shared/ipc.ts`:

```ts
export function onSessionGitRepos(
  callback: (data: { sessionId: string; repos: SessionRepo[] }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ sessionId: string; repos: SessionRepo[] }>(
    "session_git_repos",
    callback
  );
}
```

REMOVE `onSessionGitBranch` (lines 201-208). Search callers: it is wired in `src/sidebar/stores/sessions.ts`. Replace the listener there to call `sessionsStore.setGitRepos(sessionId, repos)`.

Also add `onSessionCoordinatorChanged(...)` listener mirror for `session_coordinator_changed { sessionId, isCoordinator }` emitted by §2. Wire it to `sessionsStore.setIsCoordinator(sessionId, value)`.

---

## 6. Migration of stored sessions

### 6.1 `src-tauri/src/config/sessions_persistence.rs`

**Anchor**: `PersistedSession` struct, lines 16-48. `load_sessions()` lines 139-198. `snapshot_sessions(...)` legacy mapping at lines 248-249.

**Remove** the legacy-field mapping from `snapshot_sessions(...)` (lines 248-249), but **keep** the two fields on `PersistedSession` itself as READ-ONLY, skip-on-serialize:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    // ...existing fields (name, shell, shell_args, working_directory, was_active, agent_id, agent_label, ...)...

    /// NEW: authoritative repo list.
    #[serde(default)]
    pub git_repos: Vec<crate::session::session::SessionRepo>,
    /// NEW: recomputed on restore; present for forward-compat only.
    #[serde(default)]
    pub is_coordinator: bool,

    /// LEGACY — read-only. `#[serde(skip_serializing_if)]` means the next save drops them.
    /// Consumed by the upgrade pass below; `.take()` after migrating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,
}
```

This resolves §12.1 / Grinch #6: no secondary parse, no zip-by-index fragility. Each row carries its own legacy values through `deduplicate()` naturally — any dropped row takes its legacy payload with it.

**Upgrade pass** — run it AFTER temp-filtering AND `deduplicate()`, just before returning `deduped`, mutating each entry in place with `.take()` so the fields become `None` and will be elided on the next save:

```rust
// Inside load_sessions(), right before `log::info!("Loaded {} persisted sessions ...", deduped.len(), path);`:
for ps in deduped.iter_mut() {
    if !ps.git_repos.is_empty() { continue; } // already new-schema
    match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
        (Some(source), Some(prefix)) if prefix != "multi-repo" => {
            log::info!(
                "[sessions] Upgrading legacy single-repo session '{}' → git_repos[1]={{label:{}, source:{}}}",
                ps.name, prefix, source
            );
            ps.git_repos.push(crate::session::session::SessionRepo {
                label: prefix,
                source_path: source,
                branch: None,
            });
        }
        (Some(source), None) => {
            // Shouldn't happen in data produced by this codebase, but the fields are
            // `#[serde(default)]` — a hand-edited or crash-truncated file could hit it.
            // Synthesize label from the dir name so the badge renders instead of silently dropping.
            let dir = source.replace('\\', "/").split('/').last().unwrap_or("").to_string();
            let label = if dir.starts_with("repo-") { dir[5..].to_string() } else { dir };
            log::warn!(
                "[sessions] Upgrading legacy session '{}' with source but no prefix; synthesized label '{}'",
                ps.name, label
            );
            ps.git_repos.push(crate::session::session::SessionRepo {
                label,
                source_path: source,
                branch: None,
            });
        }
        (None, Some(prefix)) if prefix == "multi-repo" => {
            log::info!(
                "[sessions] Legacy multi-repo session '{}' → git_repos left empty; DiscoveryBranchWatcher will backfill",
                ps.name
            );
        }
        (None, Some(other)) => {
            log::warn!(
                "[sessions] Legacy session '{}' has unknown prefix '{}' without source; dropping",
                ps.name, other
            );
        }
        (None, None) => {}
    }
}
```

**Guarantees**:
- Order-correct: upgrade runs AFTER `deduplicate()`, so the legacy payload always travels with its own row.
- Logged: every migrated row emits an info/warn line so upgrades are observable.
- One-shot: `.take()` clears the Options; `skip_serializing_if = "Option::is_none"` elides them on save.
- Forward-compat: `load_sessions_raw()` at line 123 (used by the CLI for live snapshots) keeps reading the legacy fields until the first save, without crashing.

**Do NOT** emit the legacy fields in `snapshot_sessions(...)` — remove lines 248-249 entirely.

**The `is_coordinator` field** is NOT critical to migrate — the live-restore path recomputes it via `create_session_inner` (§4.3), and the deferred-restore path passes `false` (dormant sessions are by definition non-coordinator). Leave `is_coordinator` at its `serde(default)` value; first restore corrects it.

### 6.2 Order of operations on first launch after upgrade

1. `load_sessions()` parses, upgrades legacy fields.
2. Normal/deferred restore runs, computes fresh `is_coordinator`, builds correct `git_repos`.
3. `GitWatcher.poll()` fires after 5s, fills `branch` on every `SessionRepo`.
4. `DiscoveryBranchWatcher.poll()` fires after 15s, overwrites `git_repos` for sessions whose name matches a known replica (this handles the "legacy multi-repo → empty → backfill" case).

This order ensures no stale `"multi-repo"` text ever reaches the UI.

---

## 7. Edge cases

1. **Repo path missing on disk**
   `detect_branch(source_path)` already returns `None` when `git rev-parse` fails (wrong dir, not a repo). The badge renders as `<label>/` — acceptable when `branch` is `None`? **No.** Per the rendering rule in §5.1 and §5.2, when `repo.branch` is `None` render only the label (no trailing slash). This is already encoded in the JSX `{repo.branch ? `/${repo.branch}` : ""}`. Verify during validation.

2. **Repo exists but is not a git repo (never initialized)**
   Same as (1): `detect_branch` returns `None`, badge renders `<label>` alone. Acceptable.

3. **Repo deleted between session creation and watcher tick**
   `detect_branch` returns `None`, badge drops to label-only on next tick. No panic. No crash.

4. **Team has zero repos but agent is coordinator**
   `git_repos` stays empty. `SessionItem` renders no repo badge (the `Show when=... .length > 0` guard handles this). Coordinator-ness is still true, but it has nothing to show.

5. **User demotes a coordinator via team edit**
   `entity_creation::update_team(...)` → `refresh_coordinator_flags(...)` (§2) → emits `session_coordinator_changed` → frontend flips `session.isCoordinator` to `false` → badges disappear without a restart.

6. **User adds a new repo to a workgroup's `config.json` on disk**
   The next discovery tick calls `DiscoveryBranchWatcher.update_replicas`, which builds the new full repo list. The next `poll()` tick pushes it via `set_git_repos`. No session restart required.

7. **Coordinator Quick-Access row**
   Reuses `renderReplicaItem` → gets the new behavior for free. No separate code path.

8. **Inactive team members (`makeInactiveEntry`)**
   Initialized with `gitRepos: []`, `isCoordinator: false`. These placeholders already render no branch badge (guard `!isInactive()` on line 313) — behavior preserved.

9. **Web-view `Session` type** (`src-tauri/src/web/mod.rs` line 129 for the struct field; the mapping is an inline closure in `api_sessions_handler` at lines 166-177, NOT a `From` impl — verified on current HEAD)
   The web REST view currently exposes `git_branch: Option<String>`. Two options:

   **A** (chosen): keep the field, populate it by joining `git_repos` with `", "` as the separator (not newline). Minimal break for existing web clients.

   **B**: replace with `git_repos: Vec<SessionRepo>` and update the browser/remote client. Cleaner, but touches the web client.

   Pick **A** for this feature; file a follow-up issue for **B** if desired. Implementation (inside the inline closure at web/mod.rs:166-177):

   ```rust
   git_branch: if s.git_repos.is_empty() {
       None
   } else {
       Some(s.git_repos.iter()
           .map(|r| match &r.branch {
               Some(b) => format!("{}/{}", r.label, b),
               None => r.label.clone(),
           })
           .collect::<Vec<_>>()
           .join(", "))
   },
   ```

   **Why `", "` not `"\n"`**: newline-separated strings get truncated at the first `\n` in single-line UI contexts (HTTP JSON clients, status bars, notification titles). Comma is safe everywhere. This addresses §12.9 / Grinch #10. `s` here is `SessionInfo` — §1.1 guarantees `SessionInfo` carries `git_repos`.

10. **Restart preserves repo list**
    `restart_session(...)` reads the stored `git_repos` before destruction and passes them back into `create_session_inner`. Confirmed on every live restart.

11. **Legacy sessions.json read by an older binary after upgrade**
    After a save under the new schema, the file contains `gitRepos` (new) and no longer `gitBranchSource`/`gitBranchPrefix` (old). An older binary deserializing this file would see the three legacy fields as `None`/empty and lose the badge until next run — acceptable, since forward compatibility is not a requirement for this feature.

12. **Restart right after a team-repo edit**
    User adds a repo to a team, then restarts the session BEFORE the next 15s discovery poll. `restart_session` reads `git_repos` from the OLD session (session.rs:728-750) and passes it back. The restarted session inherits the stale list; next discovery poll corrects it via `DiscoveryBranchWatcher`. Acceptable; no code fix needed.

13. **Out-of-band team config edit**
    User hand-edits `_team_*/config.json` on disk (no app CRUD). `refresh_coordinator_flags` never fires until the next `discover_*` call or subsequent team CRUD. Badges stay stale until then. Documented limitation; relies on existing discovery cadence.

14. **Repo clone still in progress during first session open**
    `create_workgroup` clones asynchronously. A coordinator session created immediately after may see `repo_paths` pointing at a dir without `.git`. `detect_branch_with_timeout` returns `None` → badge renders label-only. Self-heals once the clone completes and the next watcher tick runs.

15. **Coordinator path renamed between restarts**
    `agent_name_from_path(working_directory)` computes a different name after a rename; existing team config no longer matches → session's `is_coordinator` becomes `false` on first restore. Badges hide. User must edit the team to re-add the renamed agent path. Documented limitation.

---

## 8. Touched files list

### Backend (dev-rust territory)

| File | Reason |
|---|---|
| `src-tauri/src/session/session.rs` | Replace legacy branch fields with `git_repos: Vec<SessionRepo>` + `is_coordinator: bool`; add `SessionRepo` type; add runtime-only `#[serde(skip)] git_repos_gen: u64` field on `Session`. |
| `src-tauri/src/session/manager.rs` | Update `create_session(...)` signature; rename `set_git_branch` → `set_git_repos`; add `set_is_coordinator`, `refresh_coordinator_flags`, `refresh_git_repos_for_sessions`, `set_git_repos_if_gen`, `get_git_repos_gen`; rename `get_sessions_directories` → `get_sessions_repos` (returns `(Uuid, Vec<SessionRepo>, u64 gen)`). |
| `src-tauri/src/commands/session.rs` | Rework `create_session_inner(...)` signature; compute coordinator flag from `discover_teams()`; update `create_session` Tauri command and `restart_session` to use `git_repos`; update `create_root_agent_session`. |
| `src-tauri/src/commands/ac_discovery.rs` | Rework `DiscoveryBranchWatcher`: map keyed by `.ac-new/`-containing dir (Grinch #12, not base_path), two change-gate caches (§12.2, Grinch #3), drop single-repo filter, mirror 2s timeout helper with `.kill_on_drop(true)`, parallelize detection with `join_all`, use `set_git_repos_if_gen` CAS (Grinch #14), add `invalidate_replicas` helper (Grinch #17). `discover_ac_agents` / `discover_project` call `update_replicas_for_project` + `refresh_coordinator_flags` with the correct key. |
| `src-tauri/src/commands/entity_creation.rs` | Call `refresh_coordinator_flags` after `create_team`, `update_team`, `delete_team`, `create_workgroup`, `delete_workgroup`. Convert `sync_workgroup_repos_inner` to `async` + extend its signature (§2.1.a); apply replica-dir canonicalization (§2.1.b); filter `updates` by successful writes only (§2.1.c, Grinch #15); call `refresh_git_repos_for_sessions` + `git_watcher.invalidate_session_cache` + `discovery_branch_watcher.invalidate_replicas` + emit `session_git_repos` for each changed session (Grinch #4, #17). |
| `src-tauri/src/config/teams.rs` | Add `is_coordinator_for_cwd(working_directory, teams)` helper. |
| `src-tauri/src/config/sessions_persistence.rs` | Replace legacy fields on `PersistedSession` with `git_repos` + `is_coordinator`; add legacy-upgrade pass in `load_sessions`; update `snapshot_sessions` mapping. |
| `src-tauri/src/pty/git_watcher.rs` | Rework cache to `HashMap<Uuid, Vec<SessionRepo>>`; parallelize per-repo detection via `futures::future::join_all`; use `set_git_repos_if_gen` CAS for the write; add `detect_branch_with_timeout` (2s); add `.kill_on_drop(true)` on the `Command` builder; add `invalidate_session_cache`; define `GitReposPayload` and `CoordinatorChangedPayload` structs. |
| `src-tauri/src/lib.rs` | Update deferred and live restore paths to pass `git_repos` (+ `is_coordinator` where applicable). |
| `src-tauri/src/web/commands.rs` | Replace `None, None` branch args with `vec![]`. |
| `src-tauri/src/web/mod.rs` | Populate exposed `git_branch` by joining `git_repos` (option A in §7.9). |
| `src-tauri/src/phone/mailbox.rs` | Replace `None, None` branch args with `vec![]` in three call sites. |

### Frontend (dev-rust-grinch territory)

| File | Reason |
|---|---|
| `src/shared/types.ts` | Add `SessionRepo`; replace `gitBranch`/`gitBranchSource`/`gitBranchPrefix` with `gitRepos` + `isCoordinator` on `Session`. |
| `src/shared/ipc.ts` | Update `CreateSessionOptions` to use `gitRepos`; replace `onSessionGitBranch` with `onSessionGitRepos`; add `onSessionCoordinatorChanged`. |
| `src/sidebar/stores/sessions.ts` | Update `makeInactiveEntry`; replace `setGitBranch` with `setGitRepos`; add `setIsCoordinator`; rewire listeners. |
| `src/sidebar/components/SessionItem.tsx` | Map `gitRepos` to N stacked badges; gate visibility on `isCoordinator`. |
| `src/sidebar/components/ProjectPanel.tsx` | Compute and pass `gitRepos` at session creation (both direct and pending-launch paths); gate badge block on `isCoord()` in `renderReplicaItem`; render per-repo badges. |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | Compute and pass `gitRepos` at session creation; gate branch badge on per-replica coordinator check. |
| `src/sidebar/styles/sidebar.css` | Add `.session-item-branches` stack; adjust `.session-item-meta` alignment. |

---

## 9. Validation

1. `cd src-tauri && cargo check` — compile.
2. `cd src-tauri && cargo test` — existing tests still pass (`strip_auto_injected_args` and friends are unaffected).
3. `npx tsc --noEmit` — frontend types compile.
4. Manual: start with NO `sessions.json` in `~/.agentscommander/`. Create a coordinator session in a single-repo workgroup. Verify one `<repo>/<branch>` badge renders.
5. Manual: add a second repo to the workgroup (edit replica `config.json`, re-run discovery). Verify the coordinator session sprouts a second badge without a restart, stacked vertically.
6. Manual: create a non-coordinator replica in the same workgroup. Verify NO repo badge renders on it.
7. Manual: demote the coordinator via Edit Team. Verify the badges disappear live (no restart).
8. Manual: restart the app with a coordinator multi-repo session running. Verify badges repopulate after the first `DiscoveryBranchWatcher` tick.
9. Manual: legacy `sessions.json` with `"gitBranchPrefix": "multi-repo"` saved by the previous binary. Launch. Verify no `"multi-repo"` string ever renders; badges backfill from discovery.
10. Manual: legacy `sessions.json` with single-repo `gitBranchSource`+`gitBranchPrefix`. Launch. Verify the session shows one correct badge immediately (from the upgrade pass).
11. Manual: create a coordinator session in a workgroup whose `repo-*` dir has been deleted. Verify the badge renders as the repo label alone (no `/`), no panic.
12. Manual: **multi-project with overlapping paths** (guards Grinch #1 + #12). Configure `settings.project_paths` with THREE entries: (a) a parent path `"C:/repos"`, (b) a child path `"C:/repos/proj-A"` (so proj-A appears via both the parent walk AND its own entry), and (c) an unrelated sibling `"C:/other-repos"`. Each project has a coordinator workgroup with sessions running. Verify:
    - Every coordinator's badges populate from first poll.
    - No session receives duplicate `session_git_repos` events per tick (inspect logs / event counts).
    - Removing proj-A from disk and re-running discovery prunes proj-A's cache entries (no zombie polling logged).
    - `update_replicas_for_project` receives the `.ac-new/`-containing dir, never the `base_path` parent — add a `debug_assert` that fires if violated.
13. Manual: **team edit adds a repo**. Edit Team modal → add a third repo → save. Verify the coordinator session sprouts the third badge within ~5s. Expected sequence: (a) `refresh_git_repos_for_sessions` fires immediately with the new list + `branch: None`, sidebar shows label-only badges for the new repos; (b) within one `GitWatcher.POLL_INTERVAL` (5s), branches fill in. The label-only transient is NOT a bug (§12.13.C). Also verify: if ONE replica's `config.json` write fails (simulate with a read-only file), that replica's session does NOT receive the refresh — guards Grinch #15 partial-failure contract.
14. Manual: **hung repo**. Point a coordinator replica at a repo path that blocks `git rev-parse` (network share, huge repo with AV scan). Verify: (a) OTHER sessions' badges still update on the next poll — the hung repo falls back to `None` after 2s and does not starve siblings; (b) no leaked `git.exe` processes accumulate in Task Manager across 20+ poll cycles — guards Grinch #13 `kill_on_drop`; (c) poll wall-clock stays near 2s even with multiple stalled repos — guards Grinch #16 `join_all` parallelization.
15. Manual: **refresh/watcher race** (guards Grinch #14). Script sequence: start a coordinator session with a single repo; put a breakpoint (or add a sleep) in `GitWatcher::poll` between `get_sessions_repos` and `set_git_repos_if_gen`; while suspended, fire a team edit that adds a second repo (triggers `refresh_git_repos_for_sessions`); resume the watcher. Verify the watcher's CAS write fails, the debug log "gen mismatch" fires, and the sidebar ends with the NEW two-repo list (not regressed to one). Persistence snapshot taken inside the window must also carry the NEW list.
16. Unit test: `is_coordinator_for_cwd` — craft a `DiscoveredTeam` slice and a working_directory string, assert the helper returns the expected bool. Fixture-free test locking in the contract that refresh logic depends on. Place in `src-tauri/src/config/teams.rs` under `#[cfg(test)]`.
17. Unit test: legacy-migration — build a `PersistedSession` with `git_branch_source=Some(...)`, `git_branch_prefix=Some("agentscommander")`, run the upgrade pass, assert `git_repos` has one entry with matching label/source and both legacy fields are `None`. Same test with `prefix="multi-repo"` — `git_repos` stays empty, legacy fields cleared. Place in `sessions_persistence.rs` under `#[cfg(test)]`.
18. Unit test: `set_git_repos_if_gen` CAS semantics — create a `Session`, call `refresh_git_repos_for_sessions` to bump gen, then call `set_git_repos_if_gen` with an outdated `expected_gen`, assert `false` return and `git_repos` unchanged. Call again with the current gen, assert `true` and new value visible. Place in `src-tauri/src/session/manager.rs` under `#[cfg(test)]`.
19. Manual: **discovery-stale race** (guards Grinch #17). Script sequence: start a coordinator session; suspend `DiscoveryBranchWatcher::poll` with a breakpoint BEFORE `get_git_repos_gen`; fire a team edit that adds a repo (triggers `refresh_git_repos_for_sessions` + `invalidate_replicas`); **simulate the frontend dropping `reloadProject`** (close the Edit Team modal abruptly or mock the follow-up IPC); resume the suspended poll. Assert: the watcher's iteration finds NO entries for the invalidated replicas (they were scrubbed), no CAS write fires, the sidebar stays on NEW. Re-open the project panel (triggers a fresh `discover_project`) and confirm replicas are re-registered with NEW `source_path`s and subsequent polls update branches correctly. If `invalidate_replicas` is not wired, this test sees the NEW → OLD regression #17 describes.
20. Unit test: `DiscoveryBranchWatcher::invalidate_replicas` — pre-populate `replicas` / `discovery_cache` / `repos_cache` with two projects × two replicas each; call `invalidate_replicas` with one replica path from project A; assert that replica is absent from all three maps, the OTHER replica in project A is untouched, and both of project B's replicas are untouched. Place in `src-tauri/src/commands/ac_discovery.rs` under `#[cfg(test)]`.

---

## 10. Dependencies

No new Rust crates. No new npm packages.

---

## 11. Notes for devs

- Do NOT add a parallel "coordinator detector" in the frontend. The backend is the single source of truth via `session.isCoordinator`.
- Do NOT special-case the `"multi-repo"` string anywhere. It should vanish from the codebase entirely after this feature.
- Keep `DiscoveryBranchWatcher`'s event `ac_discovery_branch_updated` alive for `AcDiscoveryPanel` single-repo fallback. Do NOT repurpose or rename it.
- When you emit `session_coordinator_changed`, batch one event per changed session — do not emit for unchanged flags (the `refresh_coordinator_flags` helper already returns only changes).
- The `agent-badge` row in `SessionItem` (the coding-agent pill from the previous feature) is INDEPENDENT of repo badges. Do not conflate the two `Show` blocks.
- Windows only: `detect_branch` uses `CREATE_NO_WINDOW` to suppress the console flash. Preserve that flag on any new invocation, AND pair it with `.kill_on_drop(true)` on every `tokio::process::Command` builder that drives `git.exe` (prevents zombie processes when `tokio::time::timeout` fires).
- Do NOT re-serialize the per-repo detection loop inside either watcher's `poll()` — it MUST stay `futures::future::join_all(...)` or an equivalent concurrent primitive. Serial iteration turns poll wall-clock into M×N×2s under partial stalls. If you simplify this loop, re-verify validation #14 passes.
- The `git_repos_gen` field on `Session` is runtime-only (`#[serde(skip)]`). Never expose it in `SessionInfo` or `PersistedSession` — races are intra-process and must not leak through persistence or IPC.
- **Two independent race guards** protect `session.git_repos` writes. Both must stay wired:
  - **`set_git_repos_if_gen` (gen-CAS)** covers `GitWatcher` — where the watcher reads directly from `SessionManager.sessions` and the snapshot might predate a refresh.
  - **`invalidate_replicas`** covers `DiscoveryBranchWatcher` — where the watcher iterates its OWN `replicas` map, which is orthogonal to session state and stays stale until `discover_project` re-registers. Without this, a post-refresh poll with fresh gen but stale source_paths will CAS-succeed and regress `git_repos` back to the OLD list.

  If a future mutation path starts writing `session.git_repos` without going through `refresh_git_repos_for_sessions`, it MUST call both `git_watcher.invalidate_session_cache` AND `discovery_branch_watcher.invalidate_replicas` to preserve these guarantees.

---

## Grinch Review

Adversarial pass against current HEAD on `feature/coordinator-multi-repo-badges`. Numbered by severity (1 = highest). **Reconciliation markers added inline — see each finding.**

### 1. `DiscoveryBranchWatcher::update_replicas` OVERWRITES across projects — plan amplifies an existing bug into a functional hole

**(absorbed into §3.2.3 — `update_replicas_for_project` with per-project-keyed `HashMap<project_path, Vec<ReplicaBranchEntry>>`. Callers in `discover_ac_agents` and `discover_project` call the new API per-project. Validation #12 in §9 guards it.)**

- **What.** `update_replicas` ends with `*self.replicas.lock().unwrap() = entries;` (ac_discovery.rs:288). Each call from `discover_ac_agents` (line 688) or `discover_project` (line 1020) REPLACES the full watch list. When `projectStore.initFromSettings` loops over multiple saved `project_paths` and fires `discover_project` per path (project.ts:72-74), the LAST call wins — all other projects' replicas are dropped from the watcher.
- **Why it breaks.** This plan makes `DiscoveryBranchWatcher` the authoritative backfill path for legacy "multi-repo" sessions (§6.2 step 4) and for multi-repo `git_repos` on live coordinator sessions (§3.2). With >1 project loaded, the first project's coordinator sessions either (a) never get `git_repos` populated (legacy migration branch left empty per §6.1) or (b) never see updates after initial creation. Badges stay empty indefinitely for the "losing" project.
- **Fix.** Change `update_replicas` to MERGE rather than replace, keyed by `replica_path`. Either maintain a per-project entry map (`HashMap<project_path, Vec<ReplicaBranchEntry>>`) and flatten for polling, or change the signature so the caller passes a project identifier and only that project's entries are replaced. Add a test that covers two `discover_project` calls back-to-back and asserts both projects' replicas are polled.

### 2. `git_watcher::detect_branch` has no timeout — one stalled repo halts all polling

**(absorbed into §3.1.1 — new `detect_branch_with_timeout` helper wraps every call in `tokio::time::timeout(Duration::from_secs(2), ...)`. Same treatment mirrored in `DiscoveryBranchWatcher`. Validation #14 in §9 guards it.)**

- **What.** `detect_branch` awaits `cmd.output()` unboundedly (git_watcher.rs:121, ac_discovery.rs:399). Plan §3.1 keeps the function unchanged and calls it sequentially in a per-session loop (`for r in &repos { let branch = Self::detect_branch(...).await; ... }`).
- **Why it breaks.** If a single repo's `.git` is on a slow network drive, locked by a long-running `git fetch`, held by antivirus, or the repo is a huge submodule, `git rev-parse --abbrev-ref HEAD` can block for minutes. Because the loop is sequential across BOTH repos-in-a-session AND sessions, ONE bad repo stalls every session that polls after it. The 5-second `POLL_INTERVAL` becomes meaningless; watcher threads pile up. Plan's high-suspicion list called this scenario out explicitly — no mitigation was added.
- **Fix.** Wrap each call: `tokio::time::timeout(Duration::from_secs(3), Self::detect_branch(&r.source_path)).await.ok().flatten()`. Log the path on timeout. Same treatment for `DiscoveryBranchWatcher::detect_branch`.

### 3. `DiscoveryBranchWatcher::poll` change-detection will NEVER fire for multi-repo replicas — session `git_repos` stay stale after the first tick

**(absorbed into §3.2.2 and §3.2.4 — two independent caches (`discovery_cache: HashMap<String, Option<String>>` and `repos_cache: HashMap<String, Vec<SessionRepo>>`) with two separate gates inside `poll()`. Multi-repo replicas re-emit on per-repo drift even when the single-branch view stays None.)**

- **What.** Plan §3.2: "For multi-repo replicas, set `branch = None` so the UI hides the badge." The `changed` gate in `poll` (ac_discovery.rs:327-343) compares the single-branch payload (`Option<String>`) against the cache. For a multi-repo replica, that payload is `None` forever; after the first tick seeds the cache with `None`, `changed` is always `false` on subsequent ticks. Plan puts the `mgr.set_git_repos(...)` + `session_git_repos` emit INSIDE the `if changed` block. Net: once the cache is seeded, multi-repo sessions never receive an updated repo list from this watcher — even when branches actually change on disk.
- **Why it breaks.** This is the path the plan relies on for legacy-multi-repo backfill AND for live updates when a user adds/removes a repo from a workgroup. Broken here = badges permanently stale on disk edits. Plan §6.2 step 4 assumes this watcher fills `git_repos`; it only fires on the first tick.
- **Fix.** Give `DiscoveryBranchWatcher` a SECOND cache keyed by replica_path whose value is `Vec<SessionRepo>` (the full per-repo state). Gate the session-side push (`set_git_repos` + `session_git_repos` emit) on a change in THAT vec, independent of the single-branch `ac_discovery_branch_updated` change gate. Plan §3.2 already implies this (widen `ReplicaBranchEntry`), but it omits the cache-type change, leaving a gap.

### 4. `update_team` and `sync_workgroup_repos` mutate replica `repos` arrays but never refresh live sessions' `git_repos`

**(absorbed into §2.1 — new `SessionManager::refresh_git_repos_for_sessions` called from inside `sync_workgroup_repos_inner` after the per-replica `config.json` rewrite loop. Covers BOTH callers of the inner fn (`update_team` at line 755 and the standalone `sync_workgroup_repos` Tauri command at line 987). Pairs with `git_watcher.invalidate_session_cache` so the next watcher tick fills branches. Resolution of §12.7 vs #4 disagreement: both refresh paths are needed and non-overlapping — coordinator-flag refresh on team CRUD (§2), git_repos refresh on sync (§2.1). Validation #13 in §9 guards it.)**

- **What.** `update_team` (entity_creation.rs:709) writes the team config THEN calls `sync_workgroup_repos_inner` which rewrites every replica's `config.json` `repos` array (entity_creation.rs:862-863). Plan §2 hooks `refresh_coordinator_flags` into `update_team` — that only flips `is_coordinator`. The session's stored `git_repos` is untouched. Same story for the standalone `sync_workgroup_repos` Tauri command (entity_creation.rs:943) — not listed in plan §2 at all.
- **Why it breaks.** After a team edit reassigns which repos a coordinator replica watches, the live session still shows the OLD repo list. Staleness persists until the user manually re-triggers discovery (open AcDiscoveryPanel, refresh a project) — and then only fixes on the next `DiscoveryBranchWatcher` tick (15s), assuming finding #1 doesn't block it.
- **Fix.** After `sync_workgroup_repos_inner` succeeds, do one of:
  - (a) Call `discover_project` internally (which updates the watcher + pushes on next tick), OR
  - (b) For each replica whose `repos` array changed, resolve its active session (if any) via `find_by_name` and call `set_git_repos` directly with the freshly-resolved list. This is the low-latency option.
  List `sync_workgroup_repos` explicitly in plan §2 so the dev doesn't miss it.

### 5. Plan line numbers drift vs. current HEAD — plan was authored against an older snapshot

**(absorbed throughout the plan — every anchor re-anchored against HEAD `313b71e`. Notably: §1.1 session.rs, §1.2 manager.rs, §4.2 mailbox.rs TWO sites, §4.3 lib.rs, §4.4 ProjectPanel.tsx, §5.2 ProjectPanel.tsx, §5.3 AcDiscoveryPanel.tsx, §6.1 sessions_persistence.rs, §7.9 web/mod.rs inline closure (not From impl).)**

- **What.** Specific mismatches found:
  - §4.2 references `phone/mailbox.rs` lines `532-533, 660-661, 1677-1678`. Actual file has 1588 lines; `git_branch_source/_prefix` call sites live at 524-525 and 1481-1482. Line 1677-1678 does not exist; line 660-661 is unrelated (`inject_followup_after_idle_static` spawn).
  - §7.9 says "inside the From impl" for `web/mod.rs`. There is no `From<SessionInfo> for ApiSessionView` impl; the mapping is an inline closure in `api_sessions_handler` (web/mod.rs:166-177). The code snippet is correct, the location description is not.
  - §5.2 references `ProjectPanel.tsx` lines 396-542 / 489-502 for the badge block — verified roughly accurate (renderReplicaItem at 396, badge block at 489-502), but the surrounding `handleReplicaClick` call sites at 121-149 are correct only for one of the two paths (the other is inside the pending-launch modal callback at lines 373-380, not 1312-1318).
- **Why it matters.** A dev following §4.2 verbatim will either miss one call site (only 2 exist, not 3) or apply edits at phantom line numbers. A reviewer looking for "§7.9 From impl" wastes time. Low functional risk once caught, but erodes trust in the plan's completeness audit.
- **Fix.** Re-grep with the branch's current HEAD before implementation; update numbers. Explicitly state the phone/mailbox call-site count is TWO, not three.

### 6. Migration upgrade pass — order vs. dedupe, and zip-by-index fragility

**(absorbed into §6.1 — dropped the secondary parse + zip approach entirely. Legacy fields now live on `PersistedSession` itself with `skip_serializing_if = "Option::is_none"`, consumed via `.take()` in the upgrade loop AFTER `deduplicate()`. No index coupling. Validation #16 in §9 guards it.)**

- **What.** §6.1 says "Inside `load_sessions()`, after `serde_json::from_str::<Vec<PersistedSession>>` succeeds" upgrade via a secondary parse + zip. Current `load_sessions` flow is: parse → filter temps → `deduplicate(filtered)`. If the upgrade runs AFTER `deduplicate`, the zip between `sessions` and `legacy` iterates different-length/reordered vecs (`deduplicate` removes duplicates by name/cwd, not by index). Mismatched indices → wrong repo attributed to wrong session OR silently dropped via `.zip` truncation.
- **Why it breaks.** Either a post-dedupe session has its repo data paired with the WRONG session's legacy fields, or a post-filter legacy entry gets zipped against a post-dedupe new entry that came from a different source row.
- **Fix.** Run the upgrade pass IMMEDIATELY after the primary `from_str`, BEFORE temp filtering and deduplication. Add an `assert_eq!(sessions.len(), legacy.len())` (or guard with `if` + log) to catch parse divergence — the two parses should produce the same length because they deserialize the same array.

### 7. `SessionRepo` ordering stability — `Vec` equality changes spuriously when source is `read_dir`

**(absorbed into §3.1.2 — hard rule documented: `git_repos` order is the replica's `config.json` `repos` array order. Never sorted, never HashMap-derived. Comment added above `set_git_repos` and `update_replicas_for_project`.)**

- **What.** §3.1 uses `cache.get(&id) != Some(&refreshed)` as the changed-detection primitive. `Vec<SessionRepo>` PartialEq is order-sensitive. Plan §3.2 says "In `update_replicas(...)`, for every replica register ALL its `repo_paths`". `AcAgentReplica.repo_paths` is built from `replica_config.get("repos")` (ac_discovery.rs:556-570) — that order is authoritative (it follows the config.json array). Good. But the assembly inside `update_replicas` uses that same order, so we're fine there. The trap is elsewhere: in `git_watcher::poll`, the list comes from `SessionManager::get_sessions_repos()` which reads the Session's persisted `git_repos` vec. As long as every writer preserves insertion order from the config, equality is stable. If ANY future writer sorts, dedupes, or rebuilds from a `HashMap`, equality will flip every tick.
- **Why it matters.** A spurious "changed" verdict on every tick burns events (trivial cost) and, more importantly, bypasses the optimization. The real harm would be the inverse: two writers interleaving different orderings on the same Vec causing `changed` to oscillate every 5s and spamming the frontend.
- **Fix.** Document a hard rule in the plan: `git_repos` order IS the replica config `repos` array order — never sorted, never re-derived from a map. Add a one-line invariant comment near `set_git_repos` and `update_replicas`. Optionally replace the Vec equality with a per-element set-equality on `(source_path, branch)` pairs as a defensive measure.

### 8. Legacy `(Some(source), None)` shape silently loses the repo in the migration

**(absorbed into §6.1 — explicit match arm for `(Some(source), None)` synthesizes the label from the source-path dir name. Every match arm logs info/warn for observability.)**

- **What.** §6.1 match arms: `(Some, Some) => push repo`, `(None, Some("multi-repo")) => leave empty`, `_ => {}`. The third arm swallows `(Some(source), None)` — a legacy session with only `git_branch_source` set but no `git_branch_prefix`. Current writers always set both together (ProjectPanel.tsx:125-128, AcDiscoveryPanel.tsx:54-57), so this shouldn't occur in data produced by this codebase. BUT: the fields are `#[serde(default)]`, so an externally hand-edited `sessions.json` or a partial write from a previous crash could produce this shape.
- **Why it matters.** Low probability, but silent loss with no log line is the worst failure mode when it does occur.
- **Fix.** Extend the match: `(Some(source), None) => push SessionRepo { label: <derive from dir name>, source_path: source.to_string(), branch: None }`. Log a `warn!` for each arm that executes so upgrades are observable.

### 9. Race window: `AcDiscoveryPanel.isReplicaCoord` silently false when `originProject` is `undefined`

**(absorbed into §5.3 — two-pass `isReplicaCoord`: exact match when `originProject` present, suffix fallback otherwise (mirrors backend ac_discovery.rs:666-684). `console.warn` when the fallback fires.)**

- **What.** §5.3 proposed helper: `const ref = \`${replica.originProject ?? ""}/${replica.name}\`; return teams().some((t) => t.coordinator === ref);`. When `originProject` is missing (canonicalize failure in `extract_origin_project`, missing `identity` in config.json, or fresh workgroup before identity resolution), `ref` becomes `"/name"` — it will never match any team's `coordinator` (which is always `"project/name"` per `resolve_agent_ref`). Badge is silently hidden.
- **Why it matters.** A real coordinator replica with a transient identity-resolution failure appears non-coordinator in AcDiscoveryPanel. Users may think their team config is broken.
- **Fix.** Add a fallback: if `originProject` is missing, match by suffix (`teams().some((t) => t.coordinator?.split('/').pop() === replica.name)`). Mirror the "Pass 2 suffix fallback" already present in the backend workgroup-to-team association (ac_discovery.rs:666-684). Log a `console.warn` when the fallback fires.

### 10. `web/mod.rs` `git_branch` joined-newline — unverified client impact

**(absorbed into §7.9 — separator switched to `", "` as recommended. Follow-up issue for option B (`git_repos` structured field) deferred.)**

- **What.** §7.9 option A joins repo labels with `"\n"` and puts the multi-line string in the public `git_branch: Option<String>` field.
- **Why it might break.** Grep confirms no in-repo consumer of `/api/sessions` beyond the web client inside this monorepo, so forward risk is unknown. External consumers (custom web UIs, monitoring scripts) that render `git_branch` in a single-line widget (status bar, grid cell, notification title) will see either a truncated first line or a literal `\n` in the display. Telegram bridge formatting is worth an audit too.
- **Fix.** Two actions: (a) grep the broader workspace (`mailbox.rs`, any `status_line`/`summary` formatter) for any site that consumes `ApiSessionView.git_branch` as a single-line string before merging; (b) if any exist, switch to option B (expose `git_repos` as its own field) or use `" | "` as the separator instead of `"\n"`. The newline is the highest-blast-radius choice.

### 11. Plan §2 `get_sessions_repos` returns a dead-weight `working_dir`

**(absorbed into §1.2 — return type now `Vec<(Uuid, Vec<SessionRepo>)>`. No `working_dir`.)**

- **What.** Plan §1.2 defines the return type as `Vec<(Uuid, String /* working_dir */, Vec<SessionRepo>)>` and the caller in §3.1 immediately discards it: `.map(|(id, _wd, repos)| (id, repos))`. Plan justifies it as "returned for future flexibility".
- **Why it matters.** YAGNI. Dead fields drift out of date — some future caller will think `working_dir` is authoritative for git detection and bypass `source_path`. Role.md explicitly prohibits "features that weren't asked for".
- **Fix.** Drop `working_dir` from the return type. Re-add if a concrete need arises.

---

**Summary.** Findings #1–#4 are functional holes that will produce observable staleness or lockups in real usage. #1 and #3 are the ones most likely to fail silently (no panic, no error — just wrong/empty badges). #6 is subtle but can corrupt sessions across restart under the right legacy-data shape. The remaining are plan-accuracy or edge-case fixes. Plan is NOT ready for implementation without at least #1–#4 addressed.

## 12. Dev-rust enrichments (review pass)

Added by dev-rust after reading the plan against current code. Each item includes reasoning. **Reconciliation markers added inline.**

### 12.1 Legacy migration in `load_sessions()` — avoid secondary parse (§6.1)

**(absorbed into §6.1 — in-place legacy fields with `.take()` after `deduplicate()`.)**

**Issue**: the plan's `LegacyPersistedSession` secondary parse + `sessions.iter_mut().zip(legacy.iter())` in `load_sessions()` is **index-fragile**. `load_sessions()` (sessions_persistence.rs:152-197) filters out TEMP sessions and runs `deduplicate()` AFTER the initial `serde_json::from_str`. By the time the upgrade loop runs, `sessions` is a subset of what `legacy` parsed, and indices no longer align. The wrong session would receive the wrong legacy triplet.

**Reasoning**: a single parse that preserves legacy fields inline is both simpler and correctness-proof — you carry the legacy values through filter+dedup as part of the same struct.

**Revised approach**: keep `git_branch_source` and `git_branch_prefix` as read-only optional fields on `PersistedSession` itself (in addition to the new `git_repos` + `is_coordinator`), tagged with `#[serde(default, skip_serializing_if = "Option::is_none")]` so they are read on load but NOT emitted on save. Run the upgrade pass AFTER `deduplicate()` (right before returning `deduped`), mutating each entry in place:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    // ...existing new fields...
    #[serde(default)]
    pub git_repos: Vec<crate::session::session::SessionRepo>,
    #[serde(default)]
    pub is_coordinator: bool,

    // Legacy: read-only, dropped on write
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_prefix: Option<String>,
}

// Inside load_sessions(), after deduplicate():
for ps in deduped.iter_mut() {
    if !ps.git_repos.is_empty() { continue; }
    match (ps.git_branch_source.take(), ps.git_branch_prefix.take()) {
        (Some(source), Some(prefix)) if prefix != "multi-repo" => {
            ps.git_repos.push(crate::session::session::SessionRepo {
                label: prefix,
                source_path: source,
                branch: None,
            });
        }
        _ => {
            // Empty "multi-repo" or unknown combo → leave empty; discovery backfills.
        }
    }
}
```

`snapshot_sessions` ignores the legacy fields (they're `None` after `.take()` and `skip_serializing_if` elides them), so the first save retires them. `load_sessions_raw()` at line 123 (used by the CLI) also benefits — it keeps the legacy fields for one read cycle without crashing.

### 12.2 `DiscoveryBranchWatcher` cache type change (§3.2)

**(absorbed into §3.2.2 — two independent caches: `discovery_cache: HashMap<String, Option<String>>` and `repos_cache: HashMap<String, Vec<SessionRepo>>`.)**

**Issue**: the plan widens `ReplicaBranchEntry` but doesn't explicitly rename the watcher's `cache: Mutex<HashMap<String, Option<String>>>` (ac_discovery.rs:217). That cache is keyed by `replica_path` and stores a single branch. With multi-repo entries the single-`Option<String>` value is insufficient to detect per-repo changes.

**Reasoning**: without a multi-repo-aware cache, the watcher either re-emits on every tick (no cache hit) or never detects repo-specific branch drift for multi-repo replicas.

**Add to §3.2**:

> Change `cache` to `Mutex<HashMap<String, Vec<SessionRepo>>>`. On poll, build the refreshed `Vec<SessionRepo>` for the entry, compare to `cache.get(&entry.replica_path)` via `Vec` equality, update cache on change.
>
> The `ac_discovery_branch_updated` event still carries `branch: Option<String>` for backwards-compat with `AcDiscoveryPanel`. For single-repo entries it's the detected branch; for multi-repo entries it's `None` so the panel hides the label. The per-repo detail lives in `session_git_repos` (dispatched from the same poll iteration when `find_by_name` resolves).
>
> Drop the `known_branches` pre-seed map (ac_discovery.rs:237) for multi-repo entries — the cache pruning on line 284-285 already handles stale replica-paths correctly by `retain`.

### 12.3 `CoordinatorChangedPayload` struct (§2, §5.5)

**(absorbed into §2 — typed struct defined alongside `GitReposPayload`. All emit sites use it.)**

**Issue**: plan mentions event name `session_coordinator_changed { sessionId, isCoordinator }` but does not define the serializable Rust struct. Without one, the emit sites in `ac_discovery.rs` and `entity_creation.rs` each invent their own `serde_json::json!(...)` blob and payload drift across call sites.

**Add** next to `GitReposPayload` in `src-tauri/src/pty/git_watcher.rs` (or a new shared module):

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CoordinatorChangedPayload {
    pub session_id: String,
    pub is_coordinator: bool,
}
```

**Reasoning**: a single typed payload prevents subtle serde-rename mistakes (`isCoordinator` vs `is_coordinator`) and matches the convention used by `GitBranchPayload` today.

All `refresh_coordinator_flags` callers emit via `app.emit("session_coordinator_changed", CoordinatorChangedPayload { session_id: id.to_string(), is_coordinator: new_val })`.

### 12.4 `SessionManager::create_session` signature — add `is_coordinator` explicitly

**(absorbed into §1.2 — final signature spelled out with `git_repos` and `is_coordinator` args.)**

**Issue**: §1.2 describes the signature change but leaves `is_coordinator` as a separate pass-through. Confirm the final signature is:

```rust
pub async fn create_session(
    &self,
    shell: String,
    shell_args: Vec<String>,
    working_directory: String,
    agent_id: Option<String>,
    agent_label: Option<String>,
    git_repos: Vec<SessionRepo>,
    is_coordinator: bool,
) -> Result<Session, AppError>
```

**Reasoning**: `create_session_inner` (§4.1) computes `is_coordinator` from `discover_teams()` and passes it in. The deferred-restore path in `lib.rs:549` also has `is_coord` in scope and passes it. Keeping the arg explicit at the manager layer avoids a second compute inside the manager and keeps responsibility layered: commands compute, manager stores.

Inside the function, write `is_coordinator` straight into the constructed `Session`; `git_branch` (the old single-Option field) is gone entirely.

### 12.5 `detect_branch` timeout for hung paths

**(absorbed into §3.1.1 — `detect_branch_with_timeout` helper with 2s cap.)**

**Issue**: plan keeps `detect_branch(...)` unchanged (§3.1). Current implementation at git_watcher.rs:110-134 awaits `cmd.output().await` with no timeout. If a repo's `source_path` is on a network drive or a stale symlink, the tokio task blocks until OS-level timeout — other sessions in the same poll wait too.

**Reasoning**: the new multi-repo world multiplies the blocking exposure by N repos per session. One hung repo in a 4-repo workgroup stalls branch detection for all 4 until the OS gives up.

**Recommendation**: wrap the git spawn in `tokio::time::timeout(Duration::from_secs(2), cmd.output()).await`. On timeout, treat as `None` (same as failure). 2s is generous for a local `git rev-parse`; on network drives it fails gracefully instead of stalling the poll.

Non-blocking for the feature — can be a follow-up if the tech-lead prefers minimal diff. Flag so the risk is documented.

### 12.6 `sync_workgroup_repos_inner` post-update staleness (§7)

**(superseded — §2.1 now actively refreshes `git_repos` instead of relying on the 15s discovery cadence. Validation #13 in §9 replaces §12.6's proposed validation #12.)**

**Issue**: `update_team` (entity_creation.rs:709) calls `sync_workgroup_repos_inner` which rewrites every replica's `config.json` `repos` field. Existing sessions hold their own `git_repos` snapshot from creation time. They do NOT pick up the new list until `DiscoveryBranchWatcher` next runs (≤15s). During that window the sidebar shows a stale repo count.

**Reasoning**: the plan assumes the frontend calls `discover_*` after team edits (it does, in `ProjectPanel` after `update_team`), so the staleness window collapses to a few seconds. Acceptable for this feature. Call it out so the QA pass knows to check it.

**Add to §9 validation**:

> 12. Manual: edit a team (Edit Team modal) to add a third repo. Verify that within ~15s the coordinator session sprouts the third badge. The frontend discovery refresh that fires after `update_team` should collapse this to a few seconds.

No code change required — plan is correct, just adds documentation.

### 12.7 `refresh_coordinator_flags` call-site scope refinement (§2)

**(absorbed into §2 — refresh call-site scope limited to team/workgroup CRUD; `sync_workgroup_repos` explicitly excluded per this finding. Grinch #4's orthogonal concern — `git_repos` refresh on sync — handled separately in §2.1. Both reviewers were correct for different fields; plan now splits the refresh concerns along the same split.)**

**Issue**: plan lists `create_team`, `update_team`, `delete_team`, `create_workgroup`, `delete_workgroup` as refresh sites. For the feature to work correctly:

- **`update_team`, `delete_team`**: MANDATORY — coordinator can change or vanish. Keep.
- **`create_team`, `create_workgroup`**: FOR COMPLETENESS — creating a new team cannot change coordinator-ness of existing sessions (those sessions' agents aren't in the new team yet), but the refresh is cheap and guards against race conditions. Keep.
- **`delete_workgroup`**: the deleted workgroup's sessions are being destroyed externally anyway. The refresh targets surviving sessions whose agent paths were inside the wg — they'll be orphaned. Refresh is a no-op for deleted sessions, a sanity check for others. Keep.
- **`sync_workgroup_repos` (standalone Tauri command)**: does NOT change coordinator or team membership. DO NOT add a refresh call here — only `git_repos` drifts, and that's a discovery-tick problem.
- **`delete_agent_matrix`, `create_agent_matrix`**: guarded by referential-integrity check (only deletes agents no team references). Cannot change coordinator of any active session. Skip.

**Reasoning**: adding refresh to only the sites where coordinator-ness can actually change keeps the emission traffic minimal and the coupling narrow. Sites that are no-ops still get the refresh so a future team-edit operation can't be silently missed by an out-of-date call path.

### 12.8 `SessionInfo::from(&Session)` migration checklist

**(absorbed into §1.2 — explicit field delta documented at the bottom of §1.2.)**

Current impl (session.rs:98-120) copies all 12 fields. After the change:
- REMOVE: `git_branch`, `git_branch_source`, `git_branch_prefix` → 3 fewer copies.
- ADD: `git_repos: s.git_repos.clone()`, `is_coordinator: s.is_coordinator` → 2 more copies.

Net: 2 fewer field copies. Verify both `Session` and `SessionInfo` declare `git_repos` with the SAME serde attributes (`#[serde(default)]`) so JSON round-trips agree.

### 12.9 Web REST view — separator choice (§7.9)

**(absorbed into §7.9 — separator is `", "`.)**

Plan picks newline-joining for the back-compat `git_branch: Option<String>` in `web/mod.rs:174`. Newline is problematic in single-line UI contexts (HTTP JSON clients that truncate at `\n` when rendering).

**Recommendation**: use `", "` instead of `"\n"` — same readability, no truncation risk:

```rust
git_branch: if s.git_repos.is_empty() {
    None
} else {
    Some(s.git_repos.iter()
        .map(|r| match &r.branch {
            Some(b) => format!("{}/{}", r.label, b),
            None => r.label.clone(),
        })
        .collect::<Vec<_>>()
        .join(", "))
},
```

**Reasoning**: existing web clients format branch as inline text; comma separation is a strictly better fallback with no downside. Follow-up to replace with structured `git_repos` field (option B) remains desirable.

Note: this iterates `s.git_repos` where `s` is `SessionInfo` (from `list_sessions().await`), so `SessionInfo` MUST carry `git_repos` (confirmed in §12.8).

### 12.10 Additional edge cases to record in §7

**(absorbed into §7 items 12-15.)**

**12.10.a** — **`restart_session` right after a team-repo edit**: user adds a repo to team, then restarts the session BEFORE the next 15s discovery poll. Plan says `restart_session` reads `git_repos` from the OLD session (session.rs:728-750) and passes it back. The new session inherits the stale list; next discovery poll corrects it. Acceptable; no code fix needed.

**12.10.b** — **Out-of-band team config edit**: user hand-edits `_team_*/config.json` on disk (no app CRUD). `refresh_coordinator_flags` never fires until next `discover_*` tick or subsequent team CRUD. Badges stay stale until then. Document as a known limitation; rely on existing discovery cadence.

**12.10.c** — **Repo clone still in progress during first session open**: `create_workgroup` clones asynchronously; a coordinator session created immediately after may see `repo_paths` pointing at a dir without `.git`. `detect_branch` returns `None` → badge renders as label alone. Self-heals once the clone completes and the next watcher tick runs. Acceptable.

**12.10.d** — **Coordinator path renamed between restarts**: `agent_name_from_path(working_directory)` computes a different name after a rename; existing team config no longer matches → session's `is_coordinator` becomes `false` on first restore. Badges hide. User must edit the team to re-add the renamed agent path. Document.

### 12.11 `pty::manager::PtyManager::kill` interaction (no change)

**(no-op verification — kept for audit trail.)**

`pty/manager.rs:367` calls `self.git_watcher.remove_session(id)`. The new `GitWatcher::remove_session(&self, id: Uuid)` signature is unchanged (still `self.cache.lock().unwrap().remove(&id)`), just with a different cache value type. Confirm by inspection — no touched-files addition needed.

### 12.12 Test assertions to add

**(absorbed into §9 validation #15 (`is_coordinator_for_cwd` unit test) and #16 (legacy-migration unit test).)**

Plan's validation list (§9) is manual-QA heavy. Add two cheap unit tests:

1. `strip_auto_injected_args` tests remain unaffected — the session migration doesn't touch the arg stripping logic. Confirm no regressions by running `cargo test` after the change.
2. Add a unit test for `is_coordinator_for_cwd`: given a crafted `DiscoveredTeam` slice and a `working_directory` string, assert the helper returns the expected bool. Fixture-free test that locks in the contract the refresh logic depends on.

Place in `src-tauri/src/config/teams.rs` under `#[cfg(test)]`.

---

**Status after this pass**: plan is implementable as-written once §12.1, §12.2, and §12.3 are folded into their respective sections. §12.5, §12.9, §12.12 are quality upgrades the tech-lead can accept or defer. The touched-files list is complete — no additions needed.

---

## 12.13 `sync_workgroup_repos_inner` signature + path canonicalization (round 2)

Round-2 check against architect's §2.1 design. Two implementation details the plan omits that a dev will hit on day 1. **Reconciliation markers added inline.**

### A. Signature change — currently missing from the plan body

**(absorbed into §2.1.a — async signature with `session_mgr`, `git_watcher`, `app` added; both Tauri command callers updated.)**

`sync_workgroup_repos_inner` at `entity_creation.rs:772` is today:

```rust
fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
) -> Result<SyncResult, String>
```

Synchronous, three args. To do `mgr.refresh_git_repos_for_sessions(...).await`, `git_watcher.invalidate_session_cache(...)`, and `app.emit("session_git_repos", ...)`, the signature must become:

```rust
async fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    git_watcher: &Arc<GitWatcher>,
    app: &AppHandle,
) -> Result<SyncResult, String>
```

BOTH callers (`update_team` line 709, `sync_workgroup_repos` standalone Tauri command line 943) must be extended to take `State<'_, Arc<tokio::sync::RwLock<SessionManager>>>`, `State<'_, Arc<GitWatcher>>`, `AppHandle` and pass them through. Tauri injects State transparently — no frontend `invoke` signature change. Spell this out in §2.1 so the dev doesn't need to reverse-engineer it.

### B. Path canonicalization — equality stability of `Vec<SessionRepo>`

**(absorbed into §2.1.b — mandate to apply ac_discovery's canonicalize-and-strip-UNC pattern inside `sync_workgroup_repos_inner`, with replica_dir as the resolution context.)**

The plan's §2.1 says "build the `(session_name, Vec<SessionRepo>)` list from the updated replicas" but doesn't specify how `source_path` is produced. Inside `sync_workgroup_repos_inner` the assigned repo list (entity_creation.rs:831-838) is built as `Vec<String>` of RELATIVE strings like `"../repo-X"`. These need to be resolved to ABSOLUTE canonical paths matching exactly what `ac_discovery.rs:562-569` produces — otherwise `source_path` differs between the two writers (sync-path vs discovery-path) and `PartialEq` on `Vec<SessionRepo>` flips every tick, triggering gratuitous re-emits in both §3.2.4 Gate B and `refresh_git_repos_for_sessions`.

**Mandate in §2.1**: use the same canonicalize-and-strip-UNC pattern ac_discovery already uses:

```rust
.filter_map(|rel| {
    let resolved = replica_dir.join(rel);
    std::fs::canonicalize(&resolved).ok()
        .map(|p| {
            let s = p.to_string_lossy();
            s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
        })
})
.map(|source_path| {
    let dir = source_path.replace('\\', "/").split('/').last().unwrap_or("").to_string();
    let label = if dir.starts_with("repo-") { dir[5..].to_string() } else { dir };
    SessionRepo { label, source_path, branch: None }
})
```

**Reasoning**: without this, the bug is silent — everything "works" but the watcher fires `session_git_repos` every 5s unnecessarily. Waste of events, frontend re-renders every tick, sidebar flashes. Easy to get wrong if the dev copies only ac_discovery's `repo_paths` producer but forgets this helper runs in a different context (replica dir, not wg_path).

### C. Post-sync UI flash — document as known transient

**(absorbed into §9 validation #13 — label-only transient for ≤5s is expected behavior, not a bug.)**

After `refresh_git_repos_for_sessions` fires, `git_repos` is pushed with `branch: None` for every repo (no `git rev-parse` in the sync path). The sidebar renders label-only badges for up to 5s until the next `GitWatcher.poll()` fills branches. Acceptable for MVP; note in §9 validation #13 so QA doesn't flag it as a bug.

---

**Round 2 verdict**: items §12.13.A and §12.13.B are implementation-ready clarifications, not design gaps. Fold them into §2.1 as sub-bullets "signature" and "path canonicalization". §12.13.C is a one-line addition to validation #13. After that, **approved for round 3** from the dev-rust side.

---

## Grinch Review — Round 2

Adversarial pass against the revised body (sections 1–9). Five new findings, numbered continuing from round 1 (#12–#16). Round-1 findings #1–#11 remain accurately absorbed per the inline reconciliation markers. **Round-2 reconciliation markers added inline below.**

### 12. Per-project map key is ambiguous in `discover_ac_agents` — same replica can end up in two entries

**(absorbed into §3.2.2 + §3.2.3 — map re-keyed by the `.ac-new/`-containing dir (not `base_path`). `discover_ac_agents` groups workgroups by `repo_dir` during its walk; `discover_project` passes its own `path`. `debug_assert!` + runtime warn guard enforce the invariant. Validation #12 in §9 rewritten to cover the parent+child `project_paths` configuration.)**

- **What.** §3.2.3 proposes keying `replicas: Mutex<HashMap<String /* project_path */, Vec<ReplicaBranchEntry>>>` and for `discover_ac_agents`: "push workgroups into a `HashMap<String /* base_path */, Vec<AcWorkgroup>>` during the walk, then iterate that map and call the new API per project". The problem: `base_path` (a user-configured `settings.project_paths` entry) is NOT at the same granularity as the ACTUAL project dir (the one containing `.ac-new/`). `discover_ac_agents` walks base + immediate children (ac_discovery.rs:431-444); a single `base_path = "C:/repos"` can yield workgroups from `C:/repos/proj-A/.ac-new`, `C:/repos/proj-B/.ac-new`, etc. All those workgroups would be bucketed under key `"C:/repos"`. Meanwhile `discover_project` keys by the actual project dir (`"C:/repos/proj-A"`). If the user has both `"C:/repos"` AND `"C:/repos/proj-A"` in `settings.project_paths` (I see no dedupe in the settings persistence path), proj-A's replicas end up registered under TWO keys simultaneously.
- **Why it breaks.** `poll()` does `map.values().flatten().cloned().collect()` and iterates the flat list. A duplicate-registered replica gets detected twice per tick. The two caches (`discovery_cache`, `repos_cache`) are keyed by `replica_path`, not by `(project_path, replica_path)` — so both iterations share one cache slot. First iteration: cache empty → writes cache, emits `session_git_repos`. Second iteration: cache hit → no emit. Fine in steady state, but:
  - On cache pruning (§3.2.3 end): `update_replicas_for_project` recomputes `valid` as the flat set of `replica_path`s. A call for `base_path = "C:/repos"` that drops proj-A (e.g., proj-A was removed from disk) would leave proj-A absent from the `C:/repos` vec, but the `C:/repos/proj-A` vec stays untouched (different key). The cache `retain` keeps proj-A entries because they still appear under the OTHER key. Zombie replica keeps getting polled.
  - Worse: a refresh call for `base_path = "C:/repos"` that REBUILDS proj-A's entries with a new repo list races against an in-flight `discover_project("C:/repos/proj-A")` call. Last-writer-wins per key, and the two keys hold DIFFERENT repo lists for the same replica. `poll()` iterates both → emits two different `session_git_repos` events per tick for the same session.
- **Fix.** Key the map by the **actual project dir** (the dir containing `.ac-new/`), not by `base_path`. In `discover_ac_agents`, the walk already computes `repo_dir` (ac_discovery.rs:447) — that IS the correct key. In `discover_project`, `path` already is the project dir — also correct. Document the invariant on `update_replicas_for_project`: the `project_path` argument is the directory that contains `.ac-new/`, never a parent of multiple projects. Add a `debug_assert!` or log-warn if the passed path does not contain `.ac-new/`.

### 13. `tokio::time::timeout` over `Command::output()` leaks git.exe processes

**(absorbed into §3.1.1 — `.kill_on_drop(true)` added to the `Command` builder in `detect_branch` in BOTH `git_watcher.rs` AND `ac_discovery.rs`. Validation #14 in §9 checks Task Manager for zombie accumulation across 20+ poll cycles.)**

- **What.** §3.1.1 wraps `detect_branch` in `tokio::time::timeout(Duration::from_secs(2), ...)`. `detect_branch` uses `tokio::process::Command::output()` without `.kill_on_drop(true)` (git_watcher.rs:114-121 and ac_discovery.rs:392-399 in current HEAD; preserved unchanged by plan). When the outer timeout fires, tokio drops the pending future — the Child handle is dropped WITHOUT killing the child. The `git.exe` process keeps running to completion inside the OS.
- **Why it breaks.** On a sustained stall (locked `.git/index`, unresponsive network share, AV scan hanging onto the repo), every `POLL_INTERVAL` tick (5s `GitWatcher`, 15s `DiscoveryBranchWatcher`) spawns a fresh git.exe that never returns. After an hour of a stuck repo: 720+ orphaned git.exe for one repo × one watcher. With modest concurrency (10 sessions × 3 repos each stalled) thousands of zombie processes per hour. Eventually Tauri itself slows, Windows Explorer slows, users complain. Plan's 2s timeout correctly bounds the WAIT but not the resource cost.
- **Fix.** Add `.kill_on_drop(true)` to the `tokio::process::Command` builder in `detect_branch` BEFORE spawning. Dropping the Child (which happens when `timeout` cancels the outer future) then issues TerminateProcess. If `kill_on_drop` misbehaves on Windows: split the flow explicitly — `let mut child = cmd.spawn()?; tokio::select! { out = child.wait_with_output() => ..., _ = tokio::time::sleep(TIMEOUT) => { child.start_kill().ok(); None } }`.

### 14. `refresh_git_repos_for_sessions` + `invalidate_session_cache` still has a race — in-flight `GitWatcher::poll` can overwrite the refresh's emit with stale repos

**(absorbed into §2.1.d + §3.1 + §3.2.4 — option (a) generation counter chosen. `Session.git_repos_gen: u64` (runtime-only, `#[serde(skip)]`). `refresh_git_repos_for_sessions` bumps gen on every write. Both watchers capture the gen at snapshot time via `get_sessions_repos` (returns `(Uuid, Vec<SessionRepo>, u64)`) and write via `set_git_repos_if_gen(id, repos, expected_gen)` — CAS skips the write+emit on mismatch. Validation #15 in §9 scripts the exact race. Justification for option (a) over (c): Vec-equality CAS would false-positive when detected branches happen to match the pre-refresh list.)**

- **What.** §2.1 sequence on `sync_workgroup_repos_inner` success:
  1. Rewrite each replica `config.json` on disk.
  2. Call `refresh_git_repos_for_sessions(&updates)` — acquires `sessions.write().await`, overwrites in-memory `s.git_repos`.
  3. Call `git_watcher.invalidate_session_cache(session_id)` — clears the GitWatcher cache slot.
  4. Emit `session_git_repos` with the new list (branches all `None`, to be filled on next watcher tick).

  Meanwhile, `GitWatcher::poll()` at some point T0 before step 2 called `get_sessions_repos().await` — acquired `sessions.read().await`, got the OLD repo list, released the lock. It is now running `detect_branch_with_timeout` serially on each OLD `source_path`. The reads complete after step 3. Then `GitWatcher::poll` does:

  ```rust
  let changed = { let cache = self.cache.lock().unwrap(); cache.get(&id) != Some(&refreshed) };
  ```

  `refreshed` was built from OLD `source_path`s with freshly-detected branches. `cache` was invalidated at step 3 (`None`). Result: `changed = true`. Then `mgr.set_git_repos(id, refreshed.clone()).await` — this writes the OLD list back over the NEW one that `refresh_git_repos_for_sessions` just wrote. The emit that follows sends the OLD list.
- **Why it breaks.** User-visible sequence: fresh repos briefly appear (step 4 emit) → revert to old repos (`GitWatcher` re-emit with OLD `source_path`s + old-path branches) → fresh repos re-appear on NEXT `GitWatcher` poll (+5s). In the middle, the in-memory session's `git_repos` has REGRESSED to the old list — if `snapshot_sessions` runs in that window, persistence saves the OLD list, so a restart loses the update.
- **Fix.** Any of:
  - (a) **Generation counter** on `Session`: `git_repos_gen: u64`. `refresh_git_repos_for_sessions` bumps it. `GitWatcher::poll` captures the gen at `get_sessions_repos` time and re-reads before `set_git_repos` — if gen changed, skip the write + emit.
  - (b) **Re-read check**: `GitWatcher::poll` re-reads the session's current `git_repos` just before `set_git_repos`; if the `source_path` set differs from the list it detected branches for, skip this session's emit. Simpler but more lock traffic.
  - (c) **Compare-and-swap `set_git_repos`**: add an `expected_prev` arg and no-op when the current value doesn't match. `GitWatcher` passes the pre-detection snapshot.
  (a) or (c) cleanest. Plan currently ships none.

### 15. `sync_workgroup_repos_inner` partial failures can desync in-memory `git_repos` from on-disk `config.json`

**(absorbed into §2.1.c — filter the `updates` list to include only replicas whose `config.json` write succeeded. Failed replicas stay out of the refresh call. Validation #13 in §9 includes a simulated partial-failure case.)**

- **What.** `sync_workgroup_repos_inner` (entity_creation.rs:802-922) iterates replicas and writes each `config.json` independently. On write failure it pushes a `SyncError` into `result.errors` and continues (lines 902-917). Plan §2.1 says: "build the `(session_name, Vec<SessionRepo>)` list from the updated replicas (one entry per replica)" — does NOT specify filtering by success.
- **Why it breaks.** A dev who follows the plan literally will include ALL replicas in the `refresh_git_repos_for_sessions` call. In-memory `git_repos` updates for replicas whose `config.json` write FAILED. In-memory = NEW list; on-disk = OLD list. The sidebar shows new badges. On next restart, `sessions.json` persistence carries the NEW list, but the replica's `config.json` (canonical source for `DiscoveryBranchWatcher`) carries the OLD list — next discovery tick reads OLD and pushes it back via `set_git_repos`, silently reverting the update. Users trusting the badge during the window could run commands against the "wrong" repo view.
- **Fix.** Filter the updates list to exclude replicas whose `config.json` write failed. `SyncResult.errors` has `replica: String` (the dir name); collect only the SUCCESSFUL replicas and include only THOSE in the refresh call. Make this explicit in §2.1 so the dev doesn't miss the filter — the current phrasing "from the updated replicas" is ambiguous on whether "updated" means "attempted" or "succeeded".

### 16. Serial per-repo detection degrades poll cadence catastrophically under stalls (responsiveness, not correctness)

**(absorbed into §3.1 + §3.2.4 — `futures::future::join_all` replaces the sequential per-repo loop in both watchers. §11 "Notes for devs" records the anti-regression rule. Validation #14 in §9 confirms wall-clock stays near 2s under multiple stalls.)**

- **What.** §3.1 and §3.2.4 both iterate `for r in &repos { detect_branch_with_timeout(...).await; }` sequentially, and the outer loop iterates sessions sequentially. With M sessions × N repos, worst-case poll wall-clock = M × N × 2s. For a modest 10 sessions × 3 repos all simultaneously stalled: 60s. `GitWatcher::POLL_INTERVAL = 5s` means healthy sessions that would normally see branch updates every 5s now wait 60s. `DiscoveryBranchWatcher::BRANCH_POLL_INTERVAL = 15s` similarly stretches.
- **Why it matters.** Not a correctness bug — each individual repo is bounded at 2s by the timeout. But overall watcher responsiveness collapses, and the fix is trivial: parallelize detection via `futures::future::join_all` or `FuturesUnordered`. With 10 concurrent detections the worst case drops from 60s back to ~2s. Since the fan-out structure is being rewritten NOW, fixing in this feature costs nothing; re-serializing later is a bigger change.
- **Fix.** Inside each `for entry in &entries` (§3.2.4) and `for (id, repos) in sessions` (§3.1), run detections concurrently: `let branches = futures::future::join_all(repos.iter().map(|r| Self::detect_branch_with_timeout(&r.source_path))).await;` then zip `repos` with `branches` to build `refreshed`. `futures` is already in-tree. Add a note in §11 so future edits don't re-serialize the loop.

---

**Round 2 summary.** Findings #12–#15 are real holes introduced by, or left uncovered in, the round-1 reconciliation; #16 is a perf/responsiveness concern worth fixing now because the fan-out structure is being rewritten anyway. Most visible in normal usage: #12 (multi-project users) and #14 (any user editing a team). #13 is a slow-burning leak on stalled infrastructure. #15 masks a partial-failure bug across restarts.

**Answers to tech-lead's five round-2 questions, for the record:**

1. **Per-project keying prevents cross-project overwrite?** Partially. The rename moved the bug to "same replica in two keys" under realistic `project_paths` configurations where a parent and a child dir are both saved. See #12.
2. **Does §2.1 cover every mutation that changes a session's repo set?** §2.1 covers both callers of `sync_workgroup_repos_inner` (`update_team`, `sync_workgroup_repos`). `create_workgroup` writes replica `config.json` once but no session yet exists — N/A. `delete_team` / `delete_workgroup` destroy the whole context — N/A. `delete_agent_matrix` has referential-integrity guards that correctly prevent coordinator-affecting deletions. **However**, partial-failure handling inside the inner is underspecified — see #15.
3. **2s timeout budget — can N stalled repos blow past N×2s?** Per-repo: NO, the budget is respected. Total wall-clock per poll: NOT bounded — M×N×2s worst case, see #16. And the timeout leaks git.exe processes, see #13.
4. **Refresh + invalidate interaction — any race where cache is invalidated, session re-read with stale `git_repos`, watcher re-emits stale?** YES. In-flight `GitWatcher::poll` can overwrite the refresh's write. See #14.
5. **`.take()` clearing on first save?** YES. `skip_serializing_if` on `PersistedSession` + no snapshot mapping for legacy fields = one-shot clear on first save. `load_sessions_raw` still reads legacy if they're on-disk, but CLI consumers don't use those fields. No re-serialize path found.

---

## Grinch Review — Round 3 (FINAL)

One new finding against the revised body (sections 2.1, 3.1, 3.1.1, 3.2.2, 3.2.3, 3.2.4 + validation #12–#15, #18). Numbered continuing from round 2. **Reconciliation marker added inline.**

### 17. Gen-counter CAS does NOT protect `DiscoveryBranchWatcher` from writing stale detections when its own `replicas` map is still stale post-refresh

**(absorbed into §3.2.5 + §2.1.e — option (a) chosen. New `DiscoveryBranchWatcher::invalidate_replicas(&[String])` helper scrubs entries from `replicas`, `discovery_cache`, `repos_cache`. Called from §2.1.e alongside `git_watcher.invalidate_session_cache` after `refresh_git_repos_for_sessions`. Next `DiscoveryBranchWatcher::poll` finds no entries for invalidated replicas → no stale detection, no CAS regression. Re-registration happens on next `discover_project` (existing frontend `reloadProject` flow). `GitWatcher` continues polling via `SessionManager.sessions[*].git_repos` which already hold the NEW list — no branch-update gap on session-bound paths. Validation #19 in §9 scripts the exact race with simulated `reloadProject` drop; Validation #20 is a unit test for `invalidate_replicas`. §11 "Notes for devs" records the two-guard invariant so future mutation paths cannot regress.)**

- **What.** §2.1.d's `git_repos_gen` counter tracks one thing: "who wrote `session.git_repos` last". It does NOT track "whether the writer's INPUT (its detection source) was fresh". `DiscoveryBranchWatcher::poll` iterates `self.replicas`, which is populated by `update_replicas_for_project` — called only from `discover_ac_agents` / `discover_project`. `sync_workgroup_repos_inner` (§2.1) writes replica `config.json` and bumps `session.git_repos_gen` via `refresh_git_repos_for_sessions`, but it does NOT refresh `DiscoveryBranchWatcher.replicas`. Discovery-map refresh depends on the frontend calling `reloadProject` → `discover_project` AFTER `updateTeam` returns (EditTeamModal.tsx:206). There is a real ordering window where `DiscoveryBranchWatcher::poll` runs AFTER the refresh but BEFORE discovery re-runs.
- **Why it breaks.** Concretely, possibility Y:
  1. `update_team` backend completes: writes replica `config.json` (= NEW), calls `refresh_git_repos_for_sessions` (bumps `gen` G → G+1, session.git_repos = NEW), emits `session_git_repos` with NEW. UI shows NEW. `update_team` returns.
  2. Network / Tauri-IPC latency between frontend receiving the response and firing `reloadProject`: ~50–500ms typical.
  3. Inside that window, `DiscoveryBranchWatcher::poll()` fires (every 15s). It iterates `self.replicas` — still STALE (OLD source_paths). For the entry, captures `gen_snapshot = G+1` (the POST-refresh value — the refresh already landed). Detects branches on OLD source_paths, builds `refreshed = [OLD + fresh-branches]`.
  4. `repos_cache` comparison: previous cached state was whatever `DiscoveryBranchWatcher` wrote LAST before refresh, e.g. `[OLD + old-branches]`. New `refreshed` is `[OLD + fresh-branches]`. If any branch changed since last poll, `repos_changed = true`.
  5. CAS: `set_git_repos_if_gen(id, [OLD + fresh-branches], expected_gen = G+1)`. Current `gen = G+1`. CAS SUCCEEDS. Writes OLD list back, bumps `gen = G+2`. Emits `session_git_repos` with OLD.
  6. Frontend finally calls `reloadProject` → `discover_project` → `update_replicas_for_project` → `self.replicas` now NEW. Next poll (+15s) detects NEW, CAS (gen = G+2 == G+2) writes NEW. UI shows NEW again.

  User-visible sequence: **NEW → OLD → NEW (up to 15s gap between the middle two)**. Gen-CAS did not help because the gen was captured AFTER the refresh landed; CAS only guards against writers whose snapshot predates the refresh, not against writers whose snapshot postdates the refresh but whose SOURCE-OF-TRUTH is stale.

  **Persistence hazard**: if `snapshot_sessions` runs during the OLD-write window (e.g. on any session-state change, which emits and triggers `persist_current_state`), the saved `sessions.json` carries the OLD repo list. A crash or restart in that window loses the update permanently, even though the UI briefly showed NEW.

  **Probability**: roughly 500ms/15000ms ≈ 3% per `update_team` invocation when branches have also changed since the last `DiscoveryBranchWatcher` tick. Higher under rapid back-to-back edits. Not a corner case.

- **Why validation #15 does not catch this.** Validation #15 scripts the "refresh-during-detection" race (possibility X): pause `GitWatcher::poll` between `get_sessions_repos` and `set_git_repos_if_gen`, fire refresh, resume, observe CAS failure. That correctly tests gen captured BEFORE refresh. It does NOT test gen captured AFTER refresh while `DiscoveryBranchWatcher.replicas` is still stale. To cover it: add a validation step that suspends `DiscoveryBranchWatcher::poll` BEFORE it reaches `get_git_repos_gen`, fires a team edit that adds a repo, SKIPS the follow-up `reloadProject` (simulate frontend dropping the call — close the modal window abruptly, kill the webview, or mock the IPC), resumes the poll, observes that the watcher's CAS write SUCCEEDS and incorrectly reverts the repo list. Plan currently asserts the opposite.

- **Fix.** Three viable options; pick one:
  - (a) **Invalidate replica entries in `DiscoveryBranchWatcher` from the refresh caller**. Add a helper:
    ```rust
    impl DiscoveryBranchWatcher {
        /// Remove the specified replicas from `replicas`, `discovery_cache`, and `repos_cache`.
        /// Used by refresh_git_repos_for_sessions callers to prevent stale detection emits
        /// before the follow-up discover_project runs.
        pub fn invalidate_replicas(&self, replica_paths: &[String]) {
            let mut map = self.replicas.lock().unwrap();
            for entries in map.values_mut() {
                entries.retain(|e| !replica_paths.contains(&e.replica_path));
            }
            drop(map);
            let mut dc = self.discovery_cache.lock().unwrap();
            let mut rc = self.repos_cache.lock().unwrap();
            for p in replica_paths {
                dc.remove(p);
                rc.remove(p);
            }
        }
    }
    ```
    In §2.1.e, after `refresh_git_repos_for_sessions`, call `discovery_branch_watcher.invalidate_replicas(&affected_replica_paths)`. The next `DiscoveryBranchWatcher::poll` sees no entries for those replicas and does nothing until `discover_project` re-registers them with NEW source_paths.
  - (b) **Synchronously call `discover_project` from inside `sync_workgroup_repos_inner`** after the refresh. Guarantees watcher.replicas is NEW before the function returns. Downside: couples `sync_workgroup_repos_inner` to discovery infrastructure; slower response; may need State<> for the watcher handle in yet another place.
  - (c) **Add a per-replica source-epoch** on `ReplicaBranchEntry`. `update_replicas_for_project` bumps it when swapping entries. CAS writer passes its captured epoch alongside gen. Plan becomes more complex; refuses the simpler approach.

  Option (a) is minimal-diff and localised. Recommended.

- **Why I'm pushing back on round 3**. Tech-lead's instruction: "any push-back must be substantive enough to justify locking the design AGAINST you." This is substantive because:
  1. The race is reproducible on current-code assumptions (no additional file edits needed to hit it — just normal `update_team` through the UI).
  2. The failure mode is both visual (NEW→OLD→NEW flicker users will see) AND persistent (if `snapshot_sessions` runs in the window, restart permanently loses the update).
  3. Gen-CAS by design CANNOT fix it — the gen captures AFTER the refresh, so by the CAS's own semantics the write is "valid".
  4. The mitigation is ~15 lines of Rust in one place. No design overhaul, no new primitives beyond what already exists in the watcher.
  5. The existing architectural choice (gen over vec-CAS) stands; this finding does not reopen that decision. It adds a sibling guard.

---

**Round 3 answers to tech-lead's five questions, for the record:**

1. **Gen-counter closes the refresh-during-detection window?** YES for possibility X (gen captured before refresh). NO for possibility Y (gen captured after refresh, watcher's replicas source is stale) — see #17. Architect's choice of (a) over vec-CAS is fine on its own merits; I would have picked the same. Neither fixes #17.
2. **`debug_assert!` + runtime warn for #12 in release builds?** Yes. §3.2.3 pairs `debug_assert!` (dev-only) with a runtime early-return guard (`if !...is_dir() { log::warn!(...); return; }`). Release builds still short-circuit and skip the corrupt update. User sees no visible signal (log-only), but the data stays consistent. Acceptable.
3. **`.kill_on_drop(true)` on Windows?** Stable since tokio 1.0. Child's Drop calls TerminateProcess directly via the OS HANDLE — does not require the tokio reactor to be alive, so it works during watcher shutdown (thread exits, Runtime drops, any in-flight Child drops and kills). Plan's Windows-specific fallback note for a future tokio regression is appropriate defense. No leak path found.
4. **`join_all` vs `try_join_all` / `FuturesUnordered`?** `join_all` is correct here. Each inner future is individually bounded at 2s by `tokio::time::timeout`, so "waits for ALL" means waits up to ~2s regardless of count. `try_join_all` doesn't apply (returns Err on first failure; detection returns `Option<String>`, not Result). `FuturesUnordered` lets you process results as they arrive, but the emission contract is "one `session_git_repos` event per session with the FULL refreshed list", which needs every detection to complete before building the payload. No gain.
5. **fsync on `config.json` write for power-loss safety?** Pre-existing weakness of ALL `config.json` writes in this codebase (not introduced by this feature; `sync_workgroup_repos_inner` followed the same pattern as `set_replica_context_files` and `create_workgroup`). In scope for this feature: no. Worth filing as a separate hardening issue. Mitigation (atomic tmp + rename + sync_all) is pattern-established in `sessions_persistence::save_sessions` at line 201-221; the fix can copy that template in a follow-up.

---

**Round 3 verdict.** Four of the five tech-lead questions are closed cleanly. #17 is the one remaining hole. If the fix option (a) above is adopted and validation updated to cover possibility Y, plan is **ready for implementation**. Without #17 addressed, ship-quality under normal use will show the NEW→OLD→NEW flicker on team edits, and edge-case restart loss of the update is possible.

