# Fix issue #69 — backend-authoritative `isCoordinator` on `AcAgentReplica`

- **Issue**: https://github.com/mblua/AgentsCommander/issues/69
- **Decision comment** (approach chosen over suffix-fallback patch): https://github.com/mblua/AgentsCommander/issues/69#issuecomment-4297014974
- **Branch**: `fix/issue-69-coordinator-detection`
- **Scope**: backend-authoritative coordinator flag on `AcAgentReplica`; frontend helper deletions; orthogonal observability on dead identity links.

Line numbers in this plan were verified against the current tip of `fix/issue-69-coordinator-detection` (which is aligned with `origin/main`). The diff against `origin/main` is empty on the affected files — dev can apply offsets 1:1.

---

## 1. Overview

Problem (issue #69): the frontend re-derives whether a replica is a coordinator from team config (`ProjectPanel.tsx:isReplicaCoordinator`, `AcDiscoveryPanel.tsx:isReplicaCoord`). Both helpers use a narrow `originProject/name` match that misses cross-project WG replicas (the same pattern `config::teams::is_coordinator` already solves correctly on the backend via WG-aware suffix matching). The visible symptom: in `AcDiscoveryPanel`, replicas never render the "C" badge even when they ARE coordinators of their team's WG.

Decision: make the backend the single source of truth. Compute `is_coordinator` once during `discover_ac_agents` / `discover_project`, serialize it on `AcAgentReplica` as `isCoordinator`, delete both frontend helpers, and inline the field read. Also add a "C" badge on replica rows in `AcDiscoveryPanel` (currently only origin agents get it).

Orthogonal: when `canonicalize` of a replica's identity path fails, log a warning with the replica path and the dead target. Keep the existing fallback behavior — observability only.

---

## 2. Files to touch

| File | Change |
|---|---|
| `src-tauri/src/commands/ac_discovery.rs` | Add `is_coordinator: bool` to `AcAgentReplica` struct; populate at both construction sites; add `log::warn!` on identity-path canonicalize failure (both fns) |
| `src/shared/types.ts` | Add `isCoordinator: boolean` to `AcAgentReplica` interface |
| `src/sidebar/components/ProjectPanel.tsx` | Delete `isReplicaCoordinator`; inline `replica.isCoordinator` at 3 call sites |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | Delete `isReplicaCoord`; inline `replica.isCoordinator` at 1 call site; **add** "C" badge inside the replica badges block |

No changes to `config/teams.rs` (the backend function already correct). No changes to session-level `is_coordinator` pipeline (`Session.is_coordinator`, `refresh_coordinator_flags`, `session_coordinator_changed` emit). That pipeline remains separate and authoritative for LIVE sessions — this plan only addresses the DISCOVERY-level view used to render replica rows before a session is instantiated.

---

## 3. Backend: `src-tauri/src/commands/ac_discovery.rs`

### 3.1 `AcAgentReplica` struct — add `is_coordinator` field

**Location**: definition at lines **68-85**. Struct already derives `Debug, Clone, Serialize` with `#[serde(rename_all = "camelCase")]` (line 69), so the new field will serialize as `isCoordinator` with no extra attributes.

**Insert after line 84** (after `repo_branch: Option<String>`):

```rust
    /// True if this replica is a coordinator of any discovered team.
    /// Computed at construction against a fresh `config::teams` snapshot;
    /// covers WG-aware suffix matching that simple `originProject/name`
    /// comparison on the frontend misses. See issue #69.
    pub is_coordinator: bool,
```

Result: new field is the 8th (last) field. No reordering of existing fields.

### 3.2 Compute the team snapshot ONCE per discovery call

Both `discover_ac_agents` and `discover_project` already call `crate::config::teams::discover_teams()` near the end (lines **850** and **1201**) to feed `refresh_coordinator_flags`. Move that call to the TOP of each function and reuse the same snapshot for both per-replica `is_coordinator` computation and the existing refresh call.

#### 3.2.a `discover_ac_agents` (line 554) — hoist snapshot

**Current layout**:

```rust
// line 559  Result<AcDiscoveryResult, String> {
// line 560      let cfg = settings.read().await;
// line 561      let mut agents: Vec<AcAgentMatrix> = Vec::new();
// …
// line 849      // Recompute coordinator flags on every live session against the fresh team snapshot.
// line 850      let teams_snapshot = crate::config::teams::discover_teams();
```

**Change**:

1. Insert the snapshot binding **immediately after line 560** (`let cfg = settings.read().await;`). Add a blank comment-bearing line so the purpose is obvious:

```rust
    let cfg = settings.read().await;
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    let teams_snapshot = crate::config::teams::discover_teams();
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
```

2. **Delete** the re-computation at line **850** (`let teams_snapshot = crate::config::teams::discover_teams();`). The existing usage at line 853 (`mgr.refresh_coordinator_flags(&teams_snapshot).await`) already consumes by reference, so the hoisted binding is a drop-in.

#### 3.2.b `discover_project` (line 946) — hoist snapshot

**Current layout**:

```rust
// line 958      let cfg = settings.read().await;
// …
// line 1200     // Recompute coordinator flags on every live session against the fresh team snapshot.
// line 1201     let teams_snapshot = crate::config::teams::discover_teams();
```

**Change**:

1. Insert the snapshot binding **immediately after line 958** (`let cfg = settings.read().await;`):

```rust
    let cfg = settings.read().await;
    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    let teams_snapshot = crate::config::teams::discover_teams();
```

2. **Delete** the re-computation at line **1201**.

### 3.3 Populate `is_coordinator` at both construction sites

`is_any_coordinator` is in `crate::config::teams` and has signature:

```rust
pub fn is_any_coordinator(agent_name: &str, teams: &[DiscoveredTeam]) -> bool
```

It requires the `agent_name` in the WG-qualified form `"wg-<team>/<replica>"` — that form triggers the WG-aware branch of `is_coordinator` (teams.rs:182-186), which is exactly the matching logic the frontend helper is missing.

#### 3.3.a Site 1 — `discover_ac_agents` (lines 727-735)

**Current code**:

```rust
// line 727                                wg_agents.push(AcAgentReplica {
// line 728                                    name: replica_name,
// line 729                                    path: wg_path.to_string_lossy().to_string(),
// line 730                                    identity_path,
// line 731                                    origin_project,
// line 732                                    preferred_agent_id,
// line 733                                    repo_paths,
// line 734                                    repo_branch,
// line 735                                });
```

At this point in the code, the outer-loop variable `dir_name` (declared at line **621**, not shadowed since the `wg_dir_name` at line **671** is a separate binding inside the inner `wg_entries` loop) still refers to the WG directory name (e.g. `"wg-4-dev-team"`). The replica's suffix is `replica_name` (declared at line 676).

**Change**: immediately BEFORE the `wg_agents.push(AcAgentReplica {` line (line 727), insert:

```rust
                                let is_coordinator = crate::config::teams::is_any_coordinator(
                                    &format!("{}/{}", dir_name, replica_name),
                                    &teams_snapshot,
                                );
```

Then add the field to the struct literal (as the new last field, after `repo_branch,`). Final block:

```rust
                                wg_agents.push(AcAgentReplica {
                                    name: replica_name,
                                    path: wg_path.to_string_lossy().to_string(),
                                    identity_path,
                                    origin_project,
                                    preferred_agent_id,
                                    repo_paths,
                                    repo_branch,
                                    is_coordinator,
                                });
```

Indentation: 32 spaces (8 levels × 4), matching the existing block.

#### 3.3.b Site 2 — `discover_project` (lines 1091-1099)

Same pattern. Outer-loop variable is also `dir_name` (line **993**); the WG branch is entered at line **1014** (`if dir_name.starts_with("wg-")`). `replica_name` is declared at line **1044**.

**Current code**:

```rust
// line 1091                        wg_agents.push(AcAgentReplica {
// line 1092                            name: replica_name,
// line 1093                            path: wg_path.to_string_lossy().to_string(),
// line 1094                            identity_path,
// line 1095                            origin_project,
// line 1096                            preferred_agent_id,
// line 1097                            repo_paths,
// line 1098                            repo_branch,
// line 1099                        });
```

**Change**: immediately BEFORE the `wg_agents.push(AcAgentReplica {` line (line 1091), insert:

```rust
                        let is_coordinator = crate::config::teams::is_any_coordinator(
                            &format!("{}/{}", dir_name, replica_name),
                            &teams_snapshot,
                        );
```

Then add `is_coordinator,` as the new last field of the struct literal:

```rust
                        wg_agents.push(AcAgentReplica {
                            name: replica_name,
                            path: wg_path.to_string_lossy().to_string(),
                            identity_path,
                            origin_project,
                            preferred_agent_id,
                            repo_paths,
                            repo_branch,
                            is_coordinator,
                        });
```

Indentation: 24 spaces (6 levels × 4).

### 3.4 Orthogonal: `log::warn!` on dead identity target

The logger facility used throughout this file is `log::warn!` / `log::info!` / `log::debug!` (see line 35, 264, 317, 516, 813, 827 for examples). Import is already in scope (the file uses `log::` unqualified — `log` is a crate-level dep).

#### 3.4.a Site 1 — `discover_ac_agents` (lines 690-696)

**Current code**:

```rust
// line 690                                // Resolve identity to determine origin project
// line 691                                let origin_project = identity_path.as_ref()
// line 692                                    .and_then(|rel| {
// line 693                                        std::fs::canonicalize(wg_path.join(rel)).ok()
// line 694                                            .and_then(|abs| extract_origin_project(&abs))
// line 695                                    })
// line 696                                    .or_else(|| Some(project_folder.clone()));
```

**Replace** lines **691-696** with:

```rust
                                let origin_project = identity_path.as_ref()
                                    .and_then(|rel| {
                                        let target = wg_path.join(rel);
                                        std::fs::canonicalize(&target)
                                            .map_err(|e| {
                                                log::warn!(
                                                    "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                                    wg_path.display(),
                                                    target.display(),
                                                    e
                                                );
                                                e
                                            })
                                            .ok()
                                            .and_then(|abs| extract_origin_project(&abs))
                                    })
                                    .or_else(|| Some(project_folder.clone()));
```

Control flow preserved: the `.ok()` still collapses the `Result` into `Option`, and the `.or_else` fallback to `project_folder` still fires.

#### 3.4.b Site 2 — `discover_project` (lines 1058-1064)

**Current code**:

```rust
// line 1058                        // Resolve identity to determine origin project
// line 1059                        let origin_project = identity_path.as_ref()
// line 1060                            .and_then(|rel| {
// line 1061                                std::fs::canonicalize(wg_path.join(rel)).ok()
// line 1062                                    .and_then(|abs| extract_origin_project(&abs))
// line 1063                            })
// line 1064                            .or_else(|| Some(project_folder.clone()));
```

**Replace** lines **1059-1064** with the same structure (adjusted indentation):

```rust
                        let origin_project = identity_path.as_ref()
                            .and_then(|rel| {
                                let target = wg_path.join(rel);
                                std::fs::canonicalize(&target)
                                    .map_err(|e| {
                                        log::warn!(
                                            "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                            wg_path.display(),
                                            target.display(),
                                            e
                                        );
                                        e
                                    })
                                    .ok()
                                    .and_then(|abs| extract_origin_project(&abs))
                            })
                            .or_else(|| Some(project_folder.clone()));
```

Note: the warning message format is identical at both sites to keep log grep patterns stable. Only the `wg_path` values differ per call.

### 3.5 Repo-path canonicalize — DO NOT TOUCH

For clarity: the other `canonicalize` calls in this file at lines **711** and **1077** (inside the `repo_paths` filter_map chain) are unrelated. They canonicalize the replica's declared repo paths, not the identity. Tech-lead scoped this observability change to the identity path specifically. Leave those untouched.

---

## 4. Shared types: `src/shared/types.ts`

### 4.1 Add `isCoordinator: boolean` to `AcAgentReplica`

**Current interface** (lines **225-233**):

```ts
export interface AcAgentReplica {
  name: string;
  path: string;
  identityPath?: string;
  originProject?: string;
  preferredAgentId?: string;
  repoPaths: string[];
  repoBranch?: string;
}
```

**Change**: add `isCoordinator: boolean;` as the new last field, immediately before the closing `}` on line **233**. The field is REQUIRED (not optional) because Rust serializes it unconditionally for every replica.

```ts
export interface AcAgentReplica {
  name: string;
  path: string;
  identityPath?: string;
  originProject?: string;
  preferredAgentId?: string;
  repoPaths: string[];
  repoBranch?: string;
  isCoordinator: boolean;
}
```

Neighbors: `Session` (line 7) already has `isCoordinator: boolean;` at line **22**. No name collision since the interfaces are distinct; the IDE may surface it as a shared property name across unrelated shapes, which is fine.

---

## 5. Frontend: `src/sidebar/components/ProjectPanel.tsx`

### 5.1 Delete `isReplicaCoordinator` helper

**Current definition** at lines **58-65**:

```ts
/** Check if a replica is the coordinator of its workgroup's team */
function isReplicaCoordinator(replica: AcAgentReplica, projectFolder: string, teams: AcTeam[], teamName?: string): boolean {
  const project = replica.originProject || projectFolder;
  const fullRef = `${project}/${replica.name}`;
  if (!teamName) return false;
  const team = teams.find((t) => t.name === teamName);
  return team ? team.coordinator === fullRef : false;
}
```

**Change**: delete lines **58-65** entirely, including the JSDoc line. No other file imports this helper (it's a module-local `function` declaration, not exported).

### 5.2 Call sites (3) — inline `replica.isCoordinator`

#### 5.2.a Call site at line **403** (inside `renderReplicaItem` closure body)

**Current code**:

```ts
// line 402           const dotClass = () => replicaDotClass(wg, replica);
// line 403           const isCoord = () => isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName);
// line 404           const rn = () => replicaRepoName(replica) || …
```

**Change**: replace the RHS of line 403 so `isCoord` reads the field directly:

```ts
          const isCoord = () => replica.isCoordinator;
```

Rationale for keeping `isCoord` as an accessor (rather than deleting it and inlining everywhere): the two consumers at lines **498** and **523** already call `isCoord()`. Keeping it as an accessor is the smallest diff and preserves SolidJS tracking semantics (JSX `<Show when={…}>` re-evaluates the accessor). `replica` is stable inside the closure scope, so `replica.isCoordinator` is equivalent to the old accessor in every way that matters.

No changes to lines 498 or 523 — they continue to call `isCoord()`.

#### 5.2.b Call site at line **660** (inside coordinator-Quick-Access memo)

**Current code**:

```ts
// line 656              const coordinators = createMemo(() => {
// line 657                const result: { replica: AcAgentReplica; wg: AcWorkgroup }[] = [];
// line 658                for (const wg of proj.workgroups) {
// line 659                  for (const replica of wg.agents) {
// line 660                    if (isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName)) {
// line 661                      result.push({ replica, wg });
// line 662                    }
// line 663                  }
// line 664                }
// line 665                return result;
// line 666              });
```

**Change**: replace line **660** with the direct field read:

```ts
                    if (replica.isCoordinator) {
```

No other call sites of `isReplicaCoordinator` exist in this file or anywhere in `src/` (verified via grep).

### 5.3 Call-site sweep — verification grep

After edits, dev should run:

```bash
rtk grep -rn "isReplicaCoordinator" src/
```

Expected output: empty (function definition and all 3 call sites removed).

---

## 6. Frontend: `src/sidebar/components/AcDiscoveryPanel.tsx`

### 6.1 Delete `isReplicaCoord` helper

**Current definition** at lines **46-64**:

```ts
/** Check if a replica is the coordinator of any team. Two-pass: exact match first,
 * suffix fallback for missing originProject (mirrors backend suffix rule at
 * ac_discovery.rs:666-684). */
const isReplicaCoord = (replica: AcAgentReplica): boolean => {
  if (replica.originProject) {
    const ref = `${replica.originProject}/${replica.name}`;
    if (teams().some((t) => t.coordinator === ref)) return true;
  }
  const suffixHit = teams().some(
    (t) => t.coordinator?.split("/").pop() === replica.name
  );
  if (suffixHit && !replica.originProject) {
    console.warn(
      "[AcDiscoveryPanel] replica treated as coordinator via suffix fallback; originProject missing",
      replica.path
    );
  }
  return suffixHit;
};
```

**Change**: delete lines **46-64** entirely (JSDoc + const body). This helper is a closure-scoped `const` inside the component, not exported.

### 6.2 Delete the local accessor at line **308**

**Current code**:

```ts
// line 306                    <For each={wg.agents}>
// line 307                      {(replica) => {
// line 308                        const coord = () => isReplicaCoord(replica);
// line 309                        return (
```

**Change**: delete line **308** entirely. The replacement at the call site (next step) reads `replica.isCoordinator` directly, so the accessor is unnecessary.

### 6.3 Update the call site at line **319**, and ADD the "C" badge

Current block (lines **318-331**) — the replica's `.ac-discovery-badges` div:

```tsx
// line 318                              <div class="ac-discovery-badges">
// line 319                                <Show when={coord()}>
// line 320                                  <Show when={replica.repoPaths.length === 1 && replica.repoBranch}>
// line 321                                    <span class="ac-discovery-badge branch">
// line 322                                      {(() => {
// line 323                                        const dir = replica.repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
// line 324                                        const label = dir.startsWith("repo-") ? dir.slice(5) : dir;
// line 325                                        return `${label}/${replica.repoBranch}`;
// line 326                                      })()}
// line 327                                    </span>
// line 328                                  </Show>
// line 329                                </Show>
// line 330                                <span class="ac-discovery-badge team">replica</span>
// line 331                              </div>
```

**Change**: two edits to this block.

1. **Line 319**: replace `<Show when={coord()}>` with `<Show when={replica.isCoordinator}>`. This preserves the existing gate on the branch badge (branch only shown for coordinator replicas, per current behavior).

2. **Insert a new "C" badge as the first child of `.ac-discovery-badges`** (immediately after the `<div class="ac-discovery-badges">` opening tag at line 318, before the existing `<Show when={coord()}>` now at 319). This mirrors the pattern already used for origin agents at lines **265-267**:

```tsx
                                <Show when={replica.isCoordinator}>
                                  <span class="ac-discovery-badge coord">C</span>
                                </Show>
```

Rationale for inserting BEFORE the branch-badge `<Show>`: matches the origin-agent badge order (C badge is the first entry in the flex row). No CSS change needed — the existing `.ac-discovery-badge.coord` class is already defined and used by the origin-agent path.

**Final block** (after edits):

```tsx
                              <div class="ac-discovery-badges">
                                <Show when={replica.isCoordinator}>
                                  <span class="ac-discovery-badge coord">C</span>
                                </Show>
                                <Show when={replica.isCoordinator}>
                                  <Show when={replica.repoPaths.length === 1 && replica.repoBranch}>
                                    <span class="ac-discovery-badge branch">
                                      {(() => {
                                        const dir = replica.repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
                                        const label = dir.startsWith("repo-") ? dir.slice(5) : dir;
                                        return `${label}/${replica.repoBranch}`;
                                      })()}
                                    </span>
                                  </Show>
                                </Show>
                                <span class="ac-discovery-badge team">replica</span>
                              </div>
```

The two consecutive `<Show when={replica.isCoordinator}>` gates are intentionally separate (each gates a distinct badge — C badge vs branch badge). Do NOT merge them into a single `<Show>` wrapping both, since that would change the DOM shape — branch badge is currently inside its own `<Show>`-nested hierarchy for clarity around the `repoPaths.length === 1 && repoBranch` sub-gate.

### 6.4 DO NOT touch `isCoordinator(agent.name)` at line 40-42 or 265

**Current code at lines 39-42**:

```ts
  /** Check if agent is a coordinator of any team */
  const isCoordinator = (agentName: string): boolean => {
    return teams().some((t) => t.coordinator === agentName);
  };
```

**And its call site at line 265** (inside the AGENTS list, for `AcAgentMatrix`):

```tsx
// line 250                    const coord = () => isCoordinator(agent.name);
// …
// line 265                        <Show when={coord()}>
// line 266                          <span class="ac-discovery-badge coord">C</span>
// line 267                        </Show>
```

This is a DIFFERENT helper — it operates on `AcAgentMatrix` (origin agents), not replicas. Leave it alone. Issue #69 is exclusively about the replica side.

### 6.5 Call-site sweep — verification grep

After edits:

```bash
rtk grep -rn "isReplicaCoord\b" src/
```

Expected: empty.

```bash
rtk grep -rn "isCoordinator" src/
```

Expected: only `src/shared/types.ts` (Session + AcAgentReplica fields), the inlined `replica.isCoordinator` reads, and the `AcDiscoveryPanel.tsx` local `isCoordinator(agentName)` for `AcAgentMatrix` at lines 40-42/250. No other matches.

---

## 7. Data flow (end-to-end)

```
discover_teams() ──▶ Vec<DiscoveredTeam>
                            │
                            ▼
     ┌─────────────────────────────────────────────────────┐
     │  per replica in wg.agents:                          │
     │    wg_qualified = format!("{wg}/{replica_name}")    │
     │    is_coordinator = is_any_coordinator(&wg_q, &ts)  │
     └─────────────────────────────────────────────────────┘
                            │
                            ▼
              AcAgentReplica { …, is_coordinator: bool }
                            │
         serde rename_all = "camelCase"
                            │
                            ▼
                    JSON → frontend
                            │
                            ▼
     AcAgentReplica { …, isCoordinator: boolean } (types.ts)
                            │
          ┌─────────────────┴──────────────────┐
          ▼                                    ▼
 ProjectPanel.tsx                    AcDiscoveryPanel.tsx
  replica.isCoordinator              replica.isCoordinator
  gates coord badge                  gates new "C" badge
  + branch-badge visibility          + branch-badge visibility
  + coord-quick-access filter        (unchanged agent-matrix path
                                      still uses local helper)
```

---

## 8. Edge cases & constraints

| Case | Expected behavior |
|---|---|
| Replica has no `origin_project` (canonicalize failed) | `is_any_coordinator` still works — WG-aware suffix match keys off `dir_name` (the WG name), not `origin_project`. Warn log fires per §3.4. |
| Replica is in a WG whose team has NO coordinator | `coordinator_name` is `None` → `is_coordinator` returns `false` → `isCoordinator = false`. No badge. |
| Team has a coordinator defined in a sibling project (cross-project WG) | Exact string match on `origin_project/name` may fail, but WG-aware suffix match catches it. `isCoordinator = true`. This is the primary fix for #69. |
| Live session for the replica changes its coordinator state | Session-level `Session.is_coordinator` is a separate pipeline (`refresh_coordinator_flags` + `session_coordinator_changed` event). That remains authoritative for LIVE session rendering. `AcAgentReplica.isCoordinator` is a DISCOVERY-time snapshot and will refresh on the next discovery call. Two truths for two rendering surfaces — no merge. |
| Backwards compatibility | None needed. `AcAgentReplica` is only serialized by discovery commands (`discover_ac_agents`, `discover_project`); no persisted copy, no external consumer. New field is additive on the wire; old TS code ignoring it would still work, but we update TS in the same PR to require it. |
| Empty `teams_snapshot` (no teams discovered) | `is_any_coordinator` returns `false` for every replica. No badges render. Matches today's behavior on a greenfield install. |

---

## 9. Testing checklist (for dev-rust / dev-webpage-ui)

1. `cargo check` + `cargo clippy -- -D warnings` clean.
2. `npx tsc --noEmit` clean.
3. `npm run tauri dev`, open sidebar.
4. In a project whose team defines a coordinator via absolute cross-project path: the coordinator's replica row in `AcDiscoveryPanel` (Workgroups section) now renders a "C" badge AND the `repo-…/branch` badge (if single-repo). **Before the fix, neither rendered.**
5. Non-coordinator replica rows in the same WG: NO "C" badge, NO branch badge. Only the `replica` badge.
6. In `ProjectPanel` (Coordinator Quick-Access) with a cross-project coordinator: the coord row appears in the Quick-Access list (it did not under the old `isReplicaCoordinator` exact-match).
7. Same non-coordinator replicas no longer appear in the Quick-Access list.
8. Running-peers badges (feature-coord-running-peers-badges) remain unaffected — that plan's memo reads `replicaDotClass`, not `isReplicaCoordinator`, so the per-peer filter is untouched.
9. Kill a replica's identity target (rename the `_agent_*` dir its `config.json` points at), re-run discovery: `log::warn!` appears in `tauri dev` console including both the replica path and the dead target path. `origin_project` still falls back to the current project folder (no control-flow change).
10. Session-level coordinator highlighting on LIVE sessions (the `Session.is_coordinator` pipeline) behaves exactly as before — not touched by this PR.

---

## 10. Things the dev MUST NOT do

- Do NOT extend `is_any_coordinator` or `is_coordinator` in `teams.rs`. They are already correct; issue #69 is frontend-side divergence from them.
- Do NOT reimplement WG-aware suffix logic in TypeScript. The whole point is to delete that code, not port it.
- Do NOT change the existing session-level `Session.is_coordinator` / `refresh_coordinator_flags` / `session_coordinator_changed` machinery. That handles LIVE sessions and is orthogonal.
- Do NOT change the `.ok()` → error-handling behavior of the identity `canonicalize` call. The warning is an additional side channel, not a control-flow change.
- Do NOT touch the repo-path `canonicalize` calls at lines 711 and 1077. They canonicalize repo paths, not identity.
- Do NOT rename `is_coordinator` → `isCoordinator` manually on the Rust side. Serde rename takes care of the wire format.
- Do NOT merge the two `<Show when={replica.isCoordinator}>` gates in `AcDiscoveryPanel.tsx` (§6.3) into one wrapping `<Show>`. The inner gate still needs the `repoPaths.length === 1 && repoBranch` sub-gate in its own `<Show>`.
- Do NOT drop the `console.warn` in `isReplicaCoord` by repurposing it — the whole helper is deleted. The equivalent observability now lives in the Rust `log::warn!` on canonicalize failure.
- Do NOT mark `isCoordinator` as optional in TypeScript. Required, matches the Rust `bool`.
- Do NOT pre-compute `is_coordinator` inside the `wg_entries` scan by some other means (e.g. resolving origin_project first and comparing names manually). The whole purpose is to delegate to `is_any_coordinator`.

---

## 11. Dependencies

None. No new crates, no new imports. `crate::config::teams::is_any_coordinator` is already reachable from `ac_discovery.rs` via `crate::config::teams::discover_teams` call at the current line 850 / 1201.

---

## 12. Rollout / migration notes

- No config migration.
- No TOML schema change.
- No persisted-session impact.
- First discovery call after the update immediately populates `isCoordinator` on every replica; the frontend receives the enriched shape on the next `discover_ac_agents` / `discover_project` round-trip. No staged deployment needed.

---

## 13. Dev-rust enrichment — round 1

Reviewed against the codebase tip of `fix/issue-69-coordinator-detection` (matches `origin/main`). Every line number / code snippet in §§1–12 checks out. Toolchain is `rustc 1.93.1` — which matters for one of the points below. Additions are append-only; nothing in §§1–12 is rewritten.

### 13.1 Clippy will reject the `.map_err(|e| { …; e }).ok()` pattern — use `.inspect_err` instead

**Reasoning**: The toolchain is 1.93.1, and `clippy::manual_inspect` (stable since 1.81, default-warn) specifically targets the `.map_err(|e| { side-effect; e })` shape. The plan's §3.4.a and §3.4.b use exactly that shape. With `cargo clippy -- -D warnings` (per §9 step 1), the build will fail. `.inspect_err` has been stable since Rust 1.76, is idiomatic, and is a drop-in here because the plan is only using the closure for logging (never mutating the error).

**Recommended rewrite of §3.4.a** (replace the whole `.and_then(|rel| { … })` block at the new code):

```rust
                                let origin_project = identity_path.as_ref()
                                    .and_then(|rel| {
                                        let target = wg_path.join(rel);
                                        std::fs::canonicalize(&target)
                                            .inspect_err(|e| {
                                                log::warn!(
                                                    "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                                    wg_path.display(),
                                                    target.display(),
                                                    e
                                                );
                                            })
                                            .ok()
                                            .and_then(|abs| extract_origin_project(&abs))
                                    })
                                    .or_else(|| Some(project_folder.clone()));
```

**§3.4.b** takes the same substitution (indentation adjusted). Control-flow and the "warning is side-channel, not control-flow" invariant from §10 are preserved — `.inspect_err` leaves the `Result` shape unchanged before `.ok()` collapses it.

Note that the codebase currently contains no `.inspect_err` call sites; this PR would introduce the idiom. That's fine — it's stdlib, no dep cost, and is the clippy-preferred form. If tech-lead prefers to keep the current idiom for symmetry with existing code, the alternative is to add a local `#[allow(clippy::manual_inspect)]` on each of the two identity blocks, but I'd flag that as strictly worse (suppresses a valid lint and introduces an opaque attribute).

### 13.2 `teams_snapshot` hoist is lock-safe — no contention with `settings.read().await`

**Reasoning**: Tech-lead asked whether the hoisted `teams_snapshot` could collide with the `cfg = settings.read().await` guard held above it. Verified:

- `crate::config::teams::discover_teams()` calls `crate::config::settings::load_settings()` (`config/settings.rs:299`), which reads the settings file from disk directly via `std::fs::read_to_string` — **it never acquires the `SettingsState` `RwLock`**.
- So the sequence `cfg = settings.read().await;  teams_snapshot = discover_teams();` holds one read guard and performs an independent synchronous disk read. No deadlock path, no lock re-entry.
- Writer-side (anything that does `settings.write().await + save_settings`) is blocked by our outstanding read guard for the duration of the discovery call, so disk/memory can't split mid-call. The snapshot is internally consistent for the life of the call.

**Suggested addition to the snapshot comment** in §3.2.a and §3.2.b (append one line so the invariant is discoverable):

```rust
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    let teams_snapshot = crate::config::teams::discover_teams();
```

Nothing functional changes — this is a docstring pinning the invariant so a future refactor of `load_settings` can't silently introduce a deadlock.

### 13.3 WG-qualified format is correct as written — traced end-to-end

**Reasoning**: Tech-lead asked whether `format!("{}/{}", dir_name, replica_name)` keys off the `wg-` prefix or a stripped form. Traced through `teams.rs`:

- Plan produces: `"wg-4-dev-team/dev-rust"` (from `dir_name` at `ac_discovery.rs:621`, which is the raw `.ac-new` entry filename — NOT stripped).
- `is_any_coordinator` → `is_coordinator` → hits the WG branch at `teams.rs:182-186`:
  - `extract_wg_team("wg-4-dev-team/dev-rust")` returns `Some("dev-team")` (strips `wg-`, then `split_once('-')` on `"4-dev-team"` discarding `"4"`).
  - `team.name` comes from `_team_dev-team` dir (prefix stripped at `teams.rs:346-349`) → `"dev-team"`.
  - `wg_team == team.name` → match. Suffix compare `agent_suffix(agent_name) == agent_suffix(coord_name)` decides the verdict.
- `replica_name` at `ac_discovery.rs:676` is already prefix-stripped (`__agent_` removed) so `agent_suffix` of the WG-qualified name returns the clean replica suffix — matches how the coord side is computed.

**Conclusion**: format string in §3.3.a and §3.3.b is correct. **Do not** change it. (Flagging explicitly because the architect left this as a pressure-test bait in comments and a hasty re-read might "fix" it to `strip_prefix("wg-")` + `dir_name`, which would break matching.)

Edge cases for non-conforming WG names (`wg-4` numeric-only, `wg-alpha` no hyphen): `extract_wg_team` returns `None` → suffix branch is skipped → replica gets `is_coordinator = false`. This is pre-existing behavior; not a regression.

### 13.4 Unit-test coverage is already adequate — do NOT add a redundant test in this PR

**Reasoning**: Tech-lead asked whether a new `#[cfg(test)]` is warranted for the WG-qualified suffix path. Verified:

- `teams.rs:248-272` already has `is_coordinator_for_cwd_matches_wg_replica`, which constructs a `DiscoveredTeam { coordinator_name: Some("wg-1-dev-team/tech-lead"), … }` and asserts that a coord replica in a DIFFERENT WG (`wg-4-dev-team`) with matching suffix resolves `true`. That exercises `is_any_coordinator` through the `is_coordinator_for_cwd` wrapper.
- `is_coordinator_for_cwd` (`teams.rs:205-208`) just does `agent_name_from_path(cwd) → is_any_coordinator(&name, teams)`. Given `cwd = "C:/…/.ac-new/wg-4-dev-team/__agent_tech-lead"`, `agent_name_from_path` produces `"wg-4-dev-team/tech-lead"` — **exactly** the string this plan feeds to `is_any_coordinator` via `format!`. Same input, same code path.
- Adding a new test that directly asserts `is_any_coordinator("wg-4-dev-team/tech-lead", &teams) == true` would be strict duplicate coverage.

**Recommendation**: No new test. Instead, add a one-liner comment above the new `let is_coordinator = …` calls in §3.3.a and §3.3.b pointing at the existing guard — so the review trail is obvious:

```rust
                                // WG-aware suffix match — covered by
                                // teams::tests::is_coordinator_for_cwd_matches_wg_replica.
                                let is_coordinator = crate::config::teams::is_any_coordinator(
                                    &format!("{}/{}", dir_name, replica_name),
                                    &teams_snapshot,
                                );
```

If feature-dev reviewers flag "needs a test for the new `is_coordinator` field", the response is the comment above.

### 13.5 `log::warn!` level for dead identity is correct — do NOT downgrade to `debug`

**Reasoning**: Tech-lead flagged concern that 20 stale replicas = 20 warnings per scan. A dead identity link is a real integrity problem: the replica references an `_agent_*` matrix dir that no longer exists. The user should see that, not have it tucked into `debug`. The warning pressure is *actionable* — it tells the user "clean up these replicas or restore the matrix dir." Volume is capped at whatever stale replicas exist, and discovery is not a hot path (app start, manual refresh, project add).

Keep `warn`. No dedup layer needed (would be over-engineering for a self-limiting problem). If the issue repeats across many discoveries, it's exactly the kind of repetition that motivates user cleanup — *that's the signal working*.

### 13.6 TypeScript mocks/fixtures — none exist; plan is complete for TS

**Reasoning**: Tech-lead asked to grep for fixtures/factories constructing `AcAgentReplica`. Ran:

- `Grep "AcAgentReplica" src/` → 3 files only: `types.ts`, `ProjectPanel.tsx`, `AcDiscoveryPanel.tsx`. No test/fixture file.
- `Glob "**/*.{test,spec}.{ts,tsx}"` inside `src/` → empty. (Matches are only inside `node_modules/entities`.)
- `Grep "AcAgentReplica\s*=|: AcAgentReplica\s*=|AcAgentReplica>\s*="` → no matches. No object literals, no partial-type constructors.

The frontend has zero test harness for these discovery types. Adding `isCoordinator: boolean` (required) to the interface only affects the three files already listed in §2. No fixture updates are needed.

### 13.7 SolidJS accessor retention at `ProjectPanel.tsx:403` — plan rationale is correct but optional

**Reasoning**: The plan keeps `const isCoord = () => replica.isCoordinator;` as an accessor to preserve "SolidJS tracking semantics." This is correct in the sense that `<Show when={isCoord()}>` works identically with accessor or direct value. But note:

- Under the OLD code, `isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName)` closed over `proj.teams`. In SolidJS, if `proj.teams` were a reactive store slice, the accessor would re-track it. Fine.
- Under the NEW code, `replica.isCoordinator` is a plain property on a per-render value — it does not change after render. So the accessor form is reactively inert and the `<Show>` would re-render only when `replica` itself gets a new reference.

**Effect**: no behavior change. The plan is correct to keep the accessor (smallest diff). If a subsequent cleanup wants to drop it and inline `replica.isCoordinator` at lines 498 and 523, that's also correct — just out of scope for #69.

Flagging this so we don't waste a review round on "why keep the accessor if it's no longer tracking anything?" — the answer is "it's a no-op diff minimizer; not a bug, not worth changing."

### 13.8 Minor clarity nit — §5.2 header count

**Reasoning**: §5 header says `### 5.2 Call sites (3) — inline` but the sub-sections (§5.2.a, §5.2.b) document only 2 direct call sites of `isReplicaCoordinator` (lines 403 and 660). Lines 498 and 523 in the plan's text refer to indirect consumers of the LOCAL `isCoord()` accessor — not `isReplicaCoordinator` itself. Either:

- (a) Change header to `Call sites (2) — inline` and keep the §5.2.a / §5.2.b structure, or
- (b) Keep the "(3)" wording as "3 consumers of the derived value" and add a one-liner under the header clarifying that two of those consumers remain unchanged because the local `isCoord()` accessor is preserved.

Non-blocking. I'd pick (a) — cleaner. No code impact either way.

### 13.9 Pre-empt for `/feature-dev` review (§6b)

**Reasoning**: The /feature-dev code-reviewer typically flags bugs, DRY, convention violations, missing tests, and logic errors. Likely hits and pre-emptive answers:

| Likely flag | Pre-emptive mitigation already in plan / enrichment |
|---|---|
| `clippy::manual_inspect` on the `map_err` chain | §13.1 addresses. Use `.inspect_err`. |
| "Why are there two near-identical `is_coordinator` compute blocks (`ac_discovery.rs:~727` and `~1091`)?" — DRY concern | **Scope-bound**. `discover_ac_agents` and `discover_project` already contain duplicate replica-construction blocks — this PR adds 3 lines to each. Extracting a helper would balloon scope and fight current structure. Call this out in the PR description: "pre-existing duplication; #69 preserves it intentionally." |
| "Is there a test for `AcAgentReplica.isCoordinator`?" | §13.4 — existing `is_coordinator_for_cwd_matches_wg_replica` exercises the same code path. Add the pointer-comment from §13.4 so the answer is inline. |
| "Serde rename correctness — does `is_coordinator` serialize as `isCoordinator`?" | Yes — struct already has `#[serde(rename_all = "camelCase")]` at `ac_discovery.rs:69`. Existing fields (`originProject`, `repoPaths`, etc.) confirm the rename works. No attribute needed on the new field. Mention this in the PR description pre-emptively. |
| "Log volume on warn" | §13.5 — justified retention of `warn` level. |
| "Should `isCoordinator` be optional (`?`) in TS for forward-compat?" | §4.1 + §10 already say REQUIRED. Plan explicit; pre-empted. |
| "`.ok()` after `inspect_err` hides the typed error — is that intentional?" | Yes — §3.4 specifies "observability only, preserve existing fallback behavior." Documented in §8 and §10. |

Nothing in the above requires additional edits to the plan beyond what §13.1 already adds.

### 13.10 Summary of proposed changes to the plan

1. **§3.4.a / §3.4.b**: swap `.map_err(|e| { log::warn!; e }).ok()` for `.inspect_err(|e| { log::warn!; }).ok()`. Rationale: clippy `manual_inspect` on toolchain 1.93.1.
2. **§3.2.a / §3.2.b**: add a 2-line lock-safety docstring on the hoisted snapshot. Rationale: lock-invariant discoverability — shields against future refactor regressions.
3. **§3.3.a / §3.3.b**: add a one-line test-coverage pointer comment above each `is_any_coordinator` call. Rationale: self-documenting test trail for reviewers.
4. **§5.2 header**: optional count fix (`(3)` → `(2)`). Non-blocking cosmetic.

All other sections pass review as-is. Plan is implementation-ready after changes 1–3 land; change 4 is cosmetic.

---

## 14. Grinch adversarial review — round 1

Reviewed against the tip of `fix/issue-69-coordinator-detection` (clean against `origin/main`). Toolchain `rustc 1.93.1`, edition 2021. Findings organized by the angles the tech-lead enumerated, followed by one additional concern I found off-list. Approvals are stated explicitly where I could not break the plan.

### Per-angle verdicts

**A1. Race / consistency (`teams_snapshot` hoist)** — **No issue.** The hoisted snapshot is captured ONCE under the held `settings.read().await` guard; all subsequent per-replica `is_any_coordinator` calls and the terminal `refresh_coordinator_flags` read the same snapshot, so a single discovery pass is internally consistent. This is a *net improvement* over pre-plan: before, the terminal refresh could see a different team config than the per-replica state would have seen (if per-replica had existed). The snapshot ages across the pass (vs. fresh at refresh time pre-plan), but within-pass inconsistency is eliminated. Dev-rust §13.2 lock-safety note is correct.

**A2. Partial-failure surfaces** — See **Finding F3 (LOW)** below.

**A3. `is_any_coordinator` invariants on pathological inputs** — **No issue found.**
- Empty `agent_name` `""`: `extract_wg_team("")` splits → `Some("")` → `starts_with("wg-")` false → returns `None`. WG branch skipped. Safe.
- Multi-slash `"wg-4-dev-team/foo/bar"`: `extract_wg_team` takes `.split('/').next()` = `"wg-4-dev-team"` (OK). `agent_suffix` takes `.last()` = `"bar"` (not `"foo"`). If a team's coord suffix is `"bar"`, returns true — but real replica names cannot contain `/` because they come from `strip_prefix("__agent_")` of a filesystem dir name. Effectively unreachable.
- `agent_name` NOT starting with `wg-`: `extract_wg_team` → `None`. Exact-match branch returns false. Safe.
- Team with coordinator referencing a non-existent WG dir: `coord_name` is a plain `String` at this point (already resolved by `resolve_agent_ref`); the WG-aware branch compares strings, not filesystem state. Safe.
- Double-digit WG index `"wg-10-dev-team"`: `strip_prefix("wg-")` → `"10-dev-team"` → `split_once('-')` → `("10", "dev-team")` → returns `Some("dev-team")`. Correct.
- Degenerate `"wg-"` or `"wg-only"`: `split_once('-')` on `""`/`"only"` returns `None`. WG branch skipped. Safe.

No panics, no infinite loops, no unexpected matches on tested inputs.

**A4. `dir_name` scope confusion** — **No issue, verified line-by-line.**
- `discover_ac_agents`: `dir_name` declared at `ac_discovery.rs:621`. Between 621 and 727, the only other nearby binding is `wg_dir_name` at line 671 (distinct identifier, not a shadow). `display_name`, `wg_path`, `replica_name`, `replica_config`, etc. are all distinct names. At line 727, `dir_name` resolves to the WG dir name (e.g., `"wg-4-dev-team"`). ✓
- `discover_project`: `dir_name` declared at line 993. Between 993 and 1091, `wg_dir_name` at line 1039 is the only nearby binding (distinct identifier). At line 1091, `dir_name` resolves to the WG dir name. ✓

The `format!("{}/{}", dir_name, replica_name)` produces the intended WG-qualified key at both sites.

**A5. `.inspect_err` FnOnce/Fn compile check** — **No issue.** `Result::inspect_err<F: FnOnce(&E)>(self, f: F) -> Self` (stable since 1.76, on toolchain 1.93.1). The proposed closure captures `wg_path` and `target` immutably via `.display()` (which takes `&self`), takes `&e`, and is called at most once. Compiles as-is. Return type is unchanged (`Result<PathBuf, io::Error>`), so the downstream `.ok().and_then(...)` chain is a drop-in replacement.

**A6. Badge render order regression** — **No issue.** The new block places the "C" badge as the first child of `.ac-discovery-badges`, mirroring the origin-agent precedent at `AcDiscoveryPanel.tsx:264-267`. For coord-with-branch replicas, the order becomes `[C] [repo/branch] [replica]`, which matches the origin-agent convention of `[C] [teams…] [no-role?]`. CSS class `.ac-discovery-badge.coord` is shared between origin agents and replicas (see `sidebar.css:2331`), so rendering characteristics are consistent. The two consecutive `<Show when={replica.isCoordinator}>` gates are semantically clean — one per badge — and the plan's §10 explicitly forbids merging them. ✓

**A7. Hidden call sites** — **No issue, verified via grep.**
- `rg "isReplicaCoordinator" src/` → only `ProjectPanel.tsx` lines 59 (def), 403 (call), 660 (call). 2 direct callers + 1 definition.
- `rg "isReplicaCoord\b" src/` → only `AcDiscoveryPanel.tsx` lines 49 (def), 308 (call). 1 direct caller + 1 definition.
- No `.test.*`, `.spec.*`, `.stories.*` files inside `src/` reference these (dev-rust §13.6 already confirmed the absence of a test harness).

**A8. `AcAgentReplica` construction outside `ac_discovery.rs`** — **No issue, verified.**
`rg "AcAgentReplica\s*\{" src-tauri/src/` returns exactly 3 matches: line 70 (struct def), line 727 (site 1), line 1091 (site 2). No hidden constructor, no test fixture. The plan's 2-site count is correct.

**A9. Serde default on old clients (stale dev webview bundle)** — **Low risk, no change needed.**
`AcAgentReplica` is transient (no persistence, no external consumer). TypeScript enforces the required field at build time only; runtime access of `undefined` via `<Show when={replica.isCoordinator}>` evaluates falsy and hides the badge. Rust always emits the field. Worst case on a stale bundle: "C" badge doesn't render until the frontend bundle is rebuilt — no crash, no data loss. The plan's §8 "Backwards compatibility: None needed" stands.

**A10. `is_any_coordinator` signature sanity** — **No issue.**
`pub fn is_any_coordinator(agent_name: &str, teams: &[DiscoveredTeam]) -> bool` (teams.rs:199). Plan passes `&format!(...)` which is `&String` deref-coercing to `&str`, and `&teams_snapshot` which is `&Vec<DiscoveredTeam>` deref-coercing to `&[DiscoveredTeam]`. Both coercions are idiomatic Rust — no `.as_slice()` required, no ergonomic trap.

**A11. Clippy beyond `manual_inspect`** — **See Finding F1 (LOW) below for a related but non-clippy concern.**
- `unused_variables` on hoisted `teams_snapshot`: **Not triggered.** Both discovery fns consume `teams_snapshot` on at least one code path (in `discover_ac_agents`, unconditionally at line 853; in `discover_project`, at line 1204 for refresh AND at the per-replica call at line 1091). Rustc's `unused_variables` reports bindings that are NEVER read on ANY path — so no warning, even with the early-return at `discover_project:962-967`. However, this same early-return creates a **perf regression** — see F1.
- `redundant_clone` on `&teams_snapshot`: **Not triggered.** Plan passes by reference throughout; no implicit `.clone()` involved.

### Findings (off-list or refining an angle)

**F1 — MEDIUM · `teams_snapshot` hoist in `discover_project` wastes a filesystem scan on the common `.ac-new`-missing early return**
- **What**: §3.2.b instructs inserting `let teams_snapshot = ...` immediately after `let cfg = settings.read().await;` at line 958. The very next block (lines 961-967) is `if !ac_new_dir.is_dir() { return Ok(empty) }`. When that fires, `teams_snapshot` is computed and then discarded — but `discover_teams()` is itself a full scan of `settings.project_paths` (and their immediate children) looking for `.ac-new/_team_*/config.json` files. O(projects × child_dirs) disk calls per early return.
- **Why**: `discover_project` is called per user-selected folder (e.g., when opening a project / adding a project to the sidebar / refreshing a single project). Folders without a `.ac-new/` subdirectory are a routine early-return case — any plain repo the user opens before enabling AC triggers it. Pre-plan, `teams_snapshot` was computed at line 1201, AFTER this guard; the early return avoided it entirely. Post-plan, every such call pays the scan cost for zero benefit.
- **Fix**: In §3.2.b, move the `teams_snapshot` binding AFTER the early-return block — insert it at line ~970 (e.g., between `let _ = ensure_ac_new_gitignore(&ac_new_dir);` and `let project_folder = ...`). Note: `discover_ac_agents` does NOT have this issue (its inner `if !base.is_dir() { continue; }` is a `continue`, not a function return), so §3.2.a can remain at line 561 as written.

**F2 — LOW · Behavior change not documented in §8: replica flagged `isCoordinator` even when its WG has no team assignment locally**
- **What**: Old `isReplicaCoordinator` (ProjectPanel.tsx:59-65) gated on `teamName` — if `wg.teamName` was undefined (no team matched to this WG during the association pass at lines 799-835 / 1158-1194), the helper short-circuited to `false`. New backend `is_any_coordinator` does NOT gate on WG→team association — it iterates `teams_snapshot` (which spans ALL project paths) and returns true on WG-aware suffix match against ANY team whose name equals `extract_wg_team(wg_name)`.
- **Why**: A WG whose local team file was deleted (or whose team lives in a different project that wasn't scanned during this discovery's WG-team association pass) can now flip a replica to `isCoordinator = true` if a matching team exists anywhere in `settings.project_paths`. Visible effect: a replica row in `AcDiscoveryPanel` renders `[C] [replica]` with no team context nearby, which can read as an orphan. The `ProjectPanel` Quick-Access list may also include such replicas, surfacing them as coordinators in a project that has no record of their team. Arguably this IS the intended #69 outcome (cross-project coordinator recognition), but it's a NEW visible case not present in the old behavior.
- **Fix**: Add a row to the §8 edge-cases table — e.g., *"WG has no assigned team locally (`wg.team_name = None`) but replica suffix matches a coordinator in another project's team with matching `extract_wg_team(wg.name)`"* → *"`isCoordinator = true`. C badge renders without a nearby team badge. Intentional per #69 cross-project recognition, but differs from old helper's `teamName`-gated behavior."* No code change recommended; tightening the check would re-introduce the narrowness #69 removed.

**F3 — LOW · No observability when `discover_teams()` transiently returns a short/empty list (silent "C" badge loss)**
- **What**: §3.4 adds `log::warn!` only on identity-path `canonicalize` failures. `discover_teams()` itself (teams.rs:285-315 / 318-402) swallows all errors — `.ok()` on `std::fs::read_to_string`, `.and_then(|c| serde_json::from_str(&c).ok())` on parse, `continue` on bad entries. If a `_team_*/config.json` is mid-write, locked, or truncated during discovery, that team silently disappears from `teams_snapshot`, and every replica that depended on it silently flips to `isCoordinator = false`. No log, no signal to the user or developer. On the next successful discovery, the badges return — confusing intermittent behavior.
- **Why**: The plan adds observability for dead identity paths but not for the broader "teams config failed to load" case, which is the more common cause of a coordinator-flag mis-classification. The warning justification in §13.5 ("a dead identity link is a real integrity problem") applies equally here, arguably more so (a team config mid-write affects every member of that team).
- **Fix**: Either (a) non-invasive: add a `log::debug!("[teams] discovered {} teams across {} project paths", teams.len(), project_paths_count)` at the end of `discover_teams()` so diagnostics show the count; a sudden drop is then visible in logs. Or (b) document in §8 as a known observability gap and defer. Non-blocking for #69; acceptable to defer if tech-lead prefers a minimal PR.

### Nits (already identified by dev-rust, re-affirmed)

- **N1**: §5.2 header "(3)" should be "(2)" — dev-rust §13.8 already covers. I concur with option (a) (change to "(2)"). Non-blocking.

### Summary verdict

Plan is **implementable with one MEDIUM fix (F1) and two LOW doc/observability additions (F2, F3)**. None of the findings is a correctness bug; F1 is a perf regression on a common path, F2 is a documentation gap for a behavior change, F3 is an observability gap. Dev-rust §13.1–§13.10 stands. Probed angles A1, A3–A10 are clean. A2 and A11 are covered by F3 and F1 respectively.

If F1 is addressed (move the `discover_project` snapshot past the early return) and F2/F3 are either patched or explicitly deferred, the plan is green for implementation.

---

## 15. Architect round 2 — adjudication

Append-only verdicts on findings in §§13–14 per tech-lead protocol. Dev-rust applies the **Final deltas to apply** list (§15.5) when implementing; §§1–14 remain untouched textually and are to be read together with §15 as the authoritative spec.

### 15.1 Verdicts — dev-rust §13

| Ref | Finding | Verdict | Reasoning |
|---|---|---|---|
| §13.1 | `.map_err(\|e\| …; e).ok()` trips `clippy::manual_inspect` under `-D warnings` | **ACCEPT** | Would break §9 step 1 on toolchain 1.93.1. `.inspect_err` is stdlib (stable 1.76), same semantics, drop-in. See Delta 1 + Delta 2. |
| §13.2 | Lock-safety docstring on hoisted `teams_snapshot` | **ACCEPT** | 2 comment lines are free; they pin an invariant that a future `load_settings` refactor could silently break. See Delta 3 + Delta 4b. |
| §13.3 | Test-coverage pointer comment above each `is_any_coordinator` call | **ACCEPT** | Self-documents the review trail; pre-empts a "needs a test" round from /feature-dev. See Delta 5 + Delta 6. |
| §13.8 | §5.2 header "(3)" → "(2)" | **ACCEPT** (option a) | Accurate count — 2 direct callers (lines 403 and 660). Consumers of the local `isCoord()` accessor at lines 498/523 remain by design. See Delta 7. |

§13.4, §13.5, §13.6, §13.7, §13.9, §13.10 did not propose code deltas — they are standing rationales that inform the PR description and reviewer responses, and require no plan edit.

### 15.2 Verdicts — grinch §14

| Ref | Severity | Finding | Verdict | Reasoning |
|---|---|---|---|---|
| F1 | MEDIUM | `teams_snapshot` hoist in §3.2.b wastes a scan on the `.ac-new`-missing early return | **ACCEPT** | Real perf regression on a common call site. The fix is a pure relocation — no logic change, no scope expansion. `discover_ac_agents` (§3.2.a) is unaffected because its equivalent guard is `continue` inside a loop, not a function return. See Delta 4a. |
| F2 | LOW | Behavior change not documented: WG with no local team but foreign-team suffix match now flags `isCoordinator = true` | **ACCEPT** (doc only) | This IS the intended #69 consequence of cross-project coordinator recognition. Narrowing the check would re-introduce the divergence that motivated #69. Adding a §8 edge-case row is the honest disclosure without compromising the fix. See Delta 8. |
| F3 | LOW | `discover_teams()` silent short-return → replicas silently flip to `isCoordinator = false` | **ACCEPT** (option a, integrate now — not deferred) | Single-line `log::debug!` at the tail of `discover_teams()`. Thematically cohesive with §3.4's observability work; deferring would orphan the matching diagnostic of a transient parse/IO failure. The §10 "DO NOT DO" guard forbidding extensions of `is_any_coordinator` / `is_coordinator` remains intact — only `discover_teams` gets diagnostics. See Delta 9 + Delta 10. |
| N1 | NIT | Duplicate of §13.8 | covered by Delta 7 | — |

### 15.3 Angles grinch probed clean (A1, A3–A10; A2/A11 absorbed by F3/F1)

Acknowledged without deltas. These are the load-bearing correctness guarantees:

- **A3 — `is_any_coordinator` invariants on pathological inputs**: empty/degenerate agent names and WG names all route safely to `false`. No panic path.
- **A4 — `dir_name` scope**: confirmed line-by-line at both construction sites; no shadow, no alias collision.
- **A5 — `.inspect_err` compile check**: `FnOnce(&E)` bound satisfied by the proposed closure (captures by reference, called at most once).
- **A6 — Badge render order**: C badge as first child mirrors the origin-agent precedent; the two consecutive `<Show>` gates stay separate per §10.
- **A7 — Hidden call sites**: grep-verified, no test/spec/stories references.
- **A8 — `AcAgentReplica` construction**: exactly 2 sites + 1 definition.
- **A9 — Stale-client serde tolerance**: `<Show when={undefined}>` is falsy; no crash on a stale webview bundle.
- **A10 — `is_any_coordinator` signature coercion**: `&String` / `&Vec<T>` deref-coerce cleanly.

These are preserved by the deltas below — no delta must disturb any of these invariants.

### 15.4 Scope note for §2

Delta 9 adds one new row to the §2 "Files to touch" table: `src-tauri/src/config/teams.rs` gets a single `log::debug!` line at the tail of `discover_teams()`. The prior §2 wording ("No changes to `config/teams.rs`") referred to the **coordinator LOGIC** — `is_any_coordinator`, `is_coordinator`, `is_coordinator_for_cwd`, `can_communicate`, `is_in_team` — which still DO NOT change. §10 bullet 1 ("Do NOT extend `is_any_coordinator` or `is_coordinator` in `teams.rs`") remains verbatim and unambiguous: the forbidden surface is the logic functions, not `discover_teams`. The logic invariant holds; observability improves by exactly one line.

### 15.5 Final deltas to apply

Dev-rust applies these in order. Each delta is self-contained; no cross-delta dependency. Every delta specifies the plan section it modifies AND the concrete source change the dev should make during implementation.

#### Delta 1 — §3.4.a, identity-path `canonicalize` block in `discover_ac_agents`

Replace the proposed `.map_err(|e| { log::warn!(…); e })` chain with `.inspect_err(|e| { log::warn!(…); })`. Same log format string, same arguments, same line count for the closure body minus the trailing `e` return. `.inspect_err` returns the original `Result` unchanged, so the downstream `.ok().and_then(|abs| extract_origin_project(&abs))` is unchanged.

Final substitution (replaces the §3.4.a "**Replace** lines 691-696 with:" code block):

```rust
                                let origin_project = identity_path.as_ref()
                                    .and_then(|rel| {
                                        let target = wg_path.join(rel);
                                        std::fs::canonicalize(&target)
                                            .inspect_err(|e| {
                                                log::warn!(
                                                    "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                                    wg_path.display(),
                                                    target.display(),
                                                    e
                                                );
                                            })
                                            .ok()
                                            .and_then(|abs| extract_origin_project(&abs))
                                    })
                                    .or_else(|| Some(project_folder.clone()));
```

**Motivation**: `clippy::manual_inspect` fires under `cargo clippy -- -D warnings` (per §9 step 1) on toolchain 1.93.1.

#### Delta 2 — §3.4.b, identity-path `canonicalize` block in `discover_project`

Same substitution as Delta 1, with indentation reduced to match `discover_project`'s shallower nesting. Log format string and arguments are identical to Delta 1 (so grep patterns stay stable across both sites).

Final substitution (replaces the §3.4.b "**Replace** lines 1059-1064 with:" code block):

```rust
                        let origin_project = identity_path.as_ref()
                            .and_then(|rel| {
                                let target = wg_path.join(rel);
                                std::fs::canonicalize(&target)
                                    .inspect_err(|e| {
                                        log::warn!(
                                            "[ac-discovery] identity canonicalize failed — replica='{}' target='{}' err={}",
                                            wg_path.display(),
                                            target.display(),
                                            e
                                        );
                                    })
                                    .ok()
                                    .and_then(|abs| extract_origin_project(&abs))
                            })
                            .or_else(|| Some(project_folder.clone()));
```

#### Delta 3 — §3.2.a, hoisted-snapshot docstring in `discover_ac_agents`

Extend the 3-line comment on the hoisted `teams_snapshot` to 5 lines (adds 2 lines pinning lock-safety):

```rust
    let cfg = settings.read().await;
    // Discovery-wide team snapshot — used per-replica for is_coordinator
    // and at the end for refresh_coordinator_flags. Computed once so a
    // single discovery pass presents a coherent coordinator view.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    let teams_snapshot = crate::config::teams::discover_teams();
    let mut agents: Vec<AcAgentMatrix> = Vec::new();
```

No code change beyond the 2 new comment lines. Insertion point and binding identity are unchanged from original §3.2.a.

#### Delta 4 — §3.2.b, hoisted-snapshot relocation AND docstring in `discover_project`

Two coordinated sub-changes.

**4a (relocation — F1 fix)**: move the hoisted binding from its original §3.2.b insertion point (immediately after `let cfg = settings.read().await;` at line ~959) to **after the `.ac-new`-missing early return** (lines 961-967) AND after `let _ = ensure_ac_new_gitignore(&ac_new_dir);` at line 970. Concretely, insert the binding between line 970 (`let _ = ensure_ac_new_gitignore(&ac_new_dir);`) and line 972 (`let project_folder = base`).

**4b (docstring)**: apply a 4-line comment adapted from Delta 3, adding an F1 reference so the placement rationale is obvious. Final form:

```rust
    let _ = ensure_ac_new_gitignore(&ac_new_dir);

    // Discovery-wide team snapshot — see discover_ac_agents for rationale.
    // Lock-safe: discover_teams() reads settings from disk via load_settings()
    // and does NOT acquire SettingsState; the read guard above stays valid.
    // Placed AFTER the .ac-new-missing early return so non-AC folders don't
    // pay a wasted filesystem scan (§15 Finding F1).
    let teams_snapshot = crate::config::teams::discover_teams();

    let project_folder = base
```

The deletion at line 1201 (original §3.2.b step 2) is unchanged — the hoisted binding at the new location still serves the `refresh_coordinator_flags(&teams_snapshot)` call at line 1204.

**Motivation**: F1 MEDIUM — folders without `.ac-new/` are a routine `discover_project` call path (any non-AC folder the user opens). The original hoist made every such call run a full `settings.project_paths`-wide `discover_teams()` scan for a `teams_snapshot` that is immediately discarded by the early return.

#### Delta 5 — §3.3.a, test-coverage pointer in `discover_ac_agents`

Prepend a 2-line comment immediately above the `let is_coordinator = …` insertion specified in the original §3.3.a:

```rust
                                // WG-aware suffix match — covered by
                                // teams::tests::is_coordinator_for_cwd_matches_wg_replica.
                                let is_coordinator = crate::config::teams::is_any_coordinator(
                                    &format!("{}/{}", dir_name, replica_name),
                                    &teams_snapshot,
                                );
```

Indentation (32 spaces) and call-site unchanged. The comment points at `teams.rs:248-272`, which exercises the exact WG-qualified key format this plan feeds to `is_any_coordinator`.

#### Delta 6 — §3.3.b, test-coverage pointer in `discover_project`

Same 2-line comment as Delta 5, indentation matched to `discover_project`'s nesting (24 spaces):

```rust
                        // WG-aware suffix match — covered by
                        // teams::tests::is_coordinator_for_cwd_matches_wg_replica.
                        let is_coordinator = crate::config::teams::is_any_coordinator(
                            &format!("{}/{}", dir_name, replica_name),
                            &teams_snapshot,
                        );
```

#### Delta 7 — §5.2 header text

Rename the section header:

- **From**: `### 5.2 Call sites (3) — inline `replica.isCoordinator``
- **To**: `### 5.2 Call sites (2) — inline `replica.isCoordinator``

Cosmetic only. The sub-sections §5.2.a (line 403) and §5.2.b (line 660) cover the two direct `isReplicaCoordinator` callers accurately; lines 498/523 are consumers of the local `isCoord()` accessor that the plan keeps in place by design, not direct callers.

#### Delta 8 — §8 edge-case table, new row

Append one row to the existing §8 edge-cases table, after the "Empty `teams_snapshot`" row:

| Case | Expected behavior |
|---|---|
| WG has no assigned team locally (`wg.team_name = None`) but replica suffix matches a coordinator in another project's team with matching `extract_wg_team(wg.name)` | `isCoordinator = true`. "C" badge renders without a nearby team badge. **This is an intentional consequence of #69's cross-project coordinator recognition** and differs from the old helper's `teamName`-gated behavior. Do NOT narrow the check — that would re-introduce the divergence issue #69 fixes. |

**Motivation**: F2 LOW — documents a new visible case (replica flagged coordinator despite no local team assignment) as an intentional trade-off rather than a surprise. Matches the spirit of the grinch F2 proposal exactly.

#### Delta 9 — NEW §3.6, `discover_teams()` count diagnostic

Insert a new subsection inside §3, immediately after §3.5 ("Repo-path canonicalize — DO NOT TOUCH"):

> #### 3.6 Observability on `discover_teams()` count
>
> **Location**: `src-tauri/src/config/teams.rs`, `pub fn discover_teams()` at line 285. The function currently returns `teams` at line 315 with no diagnostic output, so a transient short/empty return (config file locked mid-write, partial IO, bad JSON) silently flips every dependent replica to `isCoordinator = false`.
>
> **Change**: immediately before the `teams` return at line 315, add a single `log::debug!` line exposing the count:
>
> ```rust
>     log::debug!(
>         "[teams] discovered {} team(s) across {} project path(s)",
>         teams.len(),
>         settings.project_paths.len()
>     );
>     teams
> ```
>
> `settings` (`AppSettings`, bound at line 286 via `crate::config::settings::load_settings()`) remains in scope at the return point. No clone, no borrow-checker conflict.
>
> **Scope discipline**: this is the ONLY edit to `teams.rs` in this PR. The logic functions (`is_any_coordinator`, `is_coordinator`, `is_coordinator_for_cwd`, `can_communicate`, `is_in_team`) remain untouched per §10 bullet 1. The §2 table is updated (see Delta 9b) to reflect the added file.

**Delta 9b — §2 table, new row**

Append one row to the §2 "Files to touch" table, after the existing `src/sidebar/components/AcDiscoveryPanel.tsx` row:

| File | Change |
|---|---|
| `src-tauri/src/config/teams.rs` | Add 1-line `log::debug!` at the tail of `discover_teams()` — observability only, no logic change |

Also update the existing trailing paragraph in §2: change "No changes to `config/teams.rs` (the backend function already correct)." to "The coordinator LOGIC in `config/teams.rs` (`is_any_coordinator`, `is_coordinator`, `is_coordinator_for_cwd`, `can_communicate`, `is_in_team`) stays untouched — Delta 9 adds one diagnostic-only log line at the tail of `discover_teams()`."

**Motivation**: F3 LOW — without this diagnostic, a transient parse/IO failure in team config reads is invisible, producing intermittent "coordinator flag disappeared" behavior that is hard to triage.

#### Delta 10 — §9 testing checklist, new step 11

Append one step to the end of §9:

> 11. **Observability check (Delta 9)**: after a fresh discovery, search the `tauri dev` console for `[teams] discovered N team(s) across M project path(s)`. Expected `N` = number of `_team_*/config.json` files across all `.ac-new/` dirs under `settings.project_paths`. Expected `M` = `settings.project_paths.len()`. If `N` drops between consecutive discovery calls without a legitimate config edit, that is the F3 signal firing — investigate whether a team config file is being written while being read.

**Motivation**: closes the observability loop — Delta 9 adds the emit, Delta 10 documents how to consume it.

### 15.6 Expected result after deltas land

- Clippy-clean under `cargo clippy -- -D warnings` on toolchain 1.93.1 (Delta 1, Delta 2).
- Lock-safety invariant documented in code (Delta 3, Delta 4b).
- Wasted `discover_teams()` scan on `.ac-new`-missing early return eliminated (Delta 4a).
- Test-coverage trail inlined in both construction sites (Delta 5, Delta 6).
- §5.2 header count corrected (Delta 7).
- F2 cross-project edge case disclosed in the edge-case table (Delta 8).
- F3 short-return observability closed by a single `log::debug!` and a testing-step pointer (Delta 9, Delta 10).

No correctness change. No logic drift. No new crate dependencies. The plan's original thesis (backend-authoritative `isCoordinator`, frontend helper deletions, identity-path warn, new "C" badge) is untouched and remains the implementation contract.

Ready for grinch round-2 re-review against this delta set. If faithful, dev-rust proceeds to implementation directly from §§1–14 + §15.5 deltas.

---

## 16. Grinch round 2 — re-review

Re-reviewed §15 against §§1–14, the current branch tip, and the concrete source (ac_discovery.rs, teams.rs, lib.rs, Cargo.toml). Append-only; §§1–15 untouched.

### 16.1 Per-delta verdict table

| Delta | Target | Verdict | One-line justification |
|---|---|---|---|
| 1 | §3.4.a `inspect_err` rewrite in `discover_ac_agents` | **FAITHFUL** | Log format string identical to my A5-validated version; closure satisfies `FnOnce(&E)`; `.ok().and_then(...)` chain unchanged. |
| 2 | §3.4.b `inspect_err` rewrite in `discover_project` | **FAITHFUL** | Exact twin of Delta 1 with 8-space reduction; format string deliberately identical so log greps stay stable across both sites. |
| 3 | §3.2.a 5-line docstring on hoisted snapshot in `discover_ac_agents` | **FAITHFUL** | Insertion point `ac_discovery.rs:560` confirmed; binding identity unchanged; lock-safety invariant faithfully encoded. |
| 4a | §3.2.b F1 relocation in `discover_project` (line ~970 not ~959) | **FAITHFUL** | Verified layout: early-return block spans 961-967, `ensure_ac_new_gitignore` at 970, `let project_folder = base` at 972. Insertion between 970 and 972 is AFTER the early return AND before construction (1091) and refresh (1204). No user of `teams_snapshot` exists between 958 and 971, so no order violation. |
| 4b | §3.2.b 6-line docstring (lock-safety + F1 rationale) | **FAITHFUL** | F1 rationale citation is accurate; placement comment ("AFTER the .ac-new-missing early return") matches Delta 4a. |
| 5 | §3.3.a test-coverage pointer in `discover_ac_agents` | **FAITHFUL** | Verified `teams::tests::is_coordinator_for_cwd_matches_wg_replica` exists at `teams.rs:252`. Indentation 32 spaces matches the nesting of the insertion point. |
| 6 | §3.3.b test-coverage pointer in `discover_project` | **FAITHFUL** | Same test reference as Delta 5; indentation 24 spaces matches shallower nesting. |
| 7 | §5.2 header "(3)" → "(2)" | **FAITHFUL** | Matches §13.8 / §14 N1; direct `isReplicaCoordinator` callers are exactly lines 403 and 660. |
| 8 | §8 new edge-case row | **FAITHFUL** | Row wording matches F2 intent: documents the behavior without narrowing the check; architect explicitly calls out "Do NOT narrow the check — that would re-introduce the divergence issue #69 fixes." |
| 9 | New §3.6 + §2 row — `log::debug!` in `teams.rs:discover_teams()` | **FAITHFUL with two NITs** | See 16.2 NIT-A (line-number off-by-one) and NIT-B (default-filter suppression). |
| 9b | §2 table new row + paragraph rewrite | **FAITHFUL** | Scope note preserves §10 bullet 1 verbatim; the new paragraph is stricter than the original (lists 5 logic functions as untouched vs. the original 2). |
| 10 | §9 step 11 — observability test step | **FAITHFUL with one NIT** | See 16.2 NIT-B (reviewer will see blank output under default log config). |

### 16.2 NITs (no DRIFT, no NEW-ISSUE of substance — but worth addressing before implementation)

**NIT-A — Delta 9 line-number off-by-one** — *LOW severity*

§15 Delta 9 asserts: *"The function currently returns `teams` at line 315 with no diagnostic output"* and *"immediately before the `teams` return at line 315, add a single `log::debug!` line"*.

Verified on branch tip: `teams.rs:314` is the bare `teams` return expression; `teams.rs:315` is the closing `}`. So the `log::debug!` should go immediately before line 314, not line 315. Non-blocking — dev-rust will locate the insertion by semantic context ("before `teams` return") regardless. Just a cosmetic documentation fix: change "line 315" → "line 314" in §15 Delta 9 prose.

**NIT-B — Delta 9/10 interaction: `debug!` is suppressed under the project's default env_logger config** — *LOW severity*

Verified in `src-tauri/src/lib.rs:102-104`:

```rust
env_logger::Builder::from_env(
    env_logger::Env::default().default_filter_or("agentscommander=info"),
)
```

The crate is `agentscommander_lib` (Cargo.toml line 40), which matches the filter prefix `agentscommander`, so the default level for every `log::*!` in this codebase is **info**. A `log::debug!` call is **suppressed by default** unless the developer sets `RUST_LOG=agentscommander=debug` (or equivalent).

Consequence: Delta 10 step 11 instructs the reviewer to *"search the `tauri dev` console for `[teams] discovered N team(s) across M project path(s)`"* — but under default config, there will be **zero output**. A reviewer not familiar with the env_logger filter will either (a) conclude F3 was not integrated / the diagnostic is broken, or (b) waste time debugging a non-issue.

This is a NEW interaction created by the adjudication: F3 and Delta 9 together make the observability hinge on an env var that §9 does not currently set. Two acceptable fixes, architect chooses:

- **Option 1 (recommended)**: Change Delta 9 from `log::debug!` to `log::info!`. Volume concern is negligible — discovery runs a handful of times per session (app start, project add, manual refresh), so the emit rate is well under a line per second. Matches the `log::info!` idiom already used throughout `ac_discovery.rs` (lines 633, 813, 832, 1172, 1191). The diagnostic becomes visible out-of-the-box without breaking anything.
- **Option 2**: Keep `log::debug!` but amend Delta 10 to instruct: *"Before running `tauri dev`, set `RUST_LOG=agentscommander=debug` (or start `tauri dev` with that env var). Otherwise the diagnostic is suppressed by the project's default log filter (`lib.rs:103`)."*

Option 1 is strictly less fragile and preserves the diagnostic as a living signal rather than a "turn on when investigating" switch. I recommend Option 1.

### 16.3 Cross-delta conflict check

None.

- Delta 3 (comment in `discover_ac_agents`) and Delta 4 (relocation + comment in `discover_project`) are in separate functions of the same file. No textual overlap.
- Delta 4a (relocation) + Delta 4b (docstring at new location) are explicitly presented as coordinated sub-changes. Applying 4a without 4b would leave the §3.2.b rationale stale; applying both together produces the intended final form.
- Delta 5/6 comments prepend the `is_coordinator` call sites established by §3.3.a/§3.3.b. They do not touch the same lines as Delta 1/2 (identity canonicalize) or Delta 3/4 (snapshot hoist).
- Delta 9 edits `teams.rs` alone; Delta 5/6 reference `teams::tests::...` but do NOT modify `teams.rs` tests. No file-level conflict, no function-level conflict.
- Delta 10 edits §9 only (test plan prose), no code change, no conflict.

All 10 deltas are applyable in a single mechanical pass by dev-rust.

### 16.4 Angle-absorption verification (§15.3 claims)

- **A2 absorbed by Delta 9/10**: **Legitimate.** My §14 A2 asked "what happens if `discover_teams()` fails or returns empty when it shouldn't?" F3 narrowed this to the specific `discover_teams` short-return case; Delta 9 adds the diagnostic that catches it; Delta 10 documents how to consume the diagnostic. A2 is closed (modulo NIT-B).
- **A11 absorbed by Delta 4a**: **Legitimate.** My §14 A11 flagged no clippy trigger but called out the perf regression (the "related but non-clippy concern" of F1). Delta 4a relocates the snapshot past the early return, eliminating the wasted scan. A11 is closed.

Both absorptions are substantive (address the root), not decorative (acknowledgment only).

### 16.5 Forbidden-surface contract (§10 vs §15.4)

§10 bullet 1 verbatim on branch tip: *"Do NOT extend `is_any_coordinator` or `is_coordinator` in `teams.rs`. They are already correct; issue #69 is frontend-side divergence from them."*

§15.4 does NOT modify §10; it clarifies the broader contract: *"the logic functions (`is_any_coordinator`, `is_coordinator`, `is_coordinator_for_cwd`, `can_communicate`, `is_in_team`) remain untouched."* This is strictly MORE restrictive than §10 bullet 1 (five named functions vs. two). Delta 9 modifies ONLY `discover_teams()`, which is not in either list. Contract preserved, in fact tightened.

Additionally verified: `teams.rs` currently contains zero `log::` calls, so Delta 9 introduces the first `log::` usage in that file. The `log` crate is a workspace dep (Cargo.toml line 15), and Rust 2021 edition resolves `log::debug!` via absolute path with no `use` import required. Compiles as-written.

### 16.6 Cumulative risk / implementability

With all 10 deltas on top of §§1–12:

- Source files touched: `ac_discovery.rs` (4 touch points: two canonicalize blocks + two snapshot hoists + two construction sites, localized to lines 560-561, 691-696, 727-735, 970-972, 1059-1064, 1091-1099), `types.ts` (1 line), `ProjectPanel.tsx` (3 edits), `AcDiscoveryPanel.tsx` (3 edits), `teams.rs` (1 line, new). Total: 5 files, ~15 localized edits.
- Plan-text edits: header rename (§5.2), one new row (§2), one new row (§8), one new subsection (§3.6), one new test-plan step (§9.11).
- No cross-file ordering dependency. No reuse of intermediate state between deltas.
- Clippy-clean with Delta 1+2 on toolchain 1.93.1.
- TypeScript build-clean with the `isCoordinator: boolean` required-field addition (only 3 consumers, all updated by §§5-6).

Dev-rust can land the full delta set in a single sitting without partial-apply risk.

### 16.7 No new correctness issues introduced

I specifically looked for:
- New unused-variable warnings from the relocation (Delta 4a): **none** — `teams_snapshot` is still consumed unconditionally at line 1204 and conditionally at line 1091.
- Borrow-checker conflicts from `settings.project_paths.len()` at end of `discover_teams` (Delta 9): **none** — `settings` is bound at line 286 (owned `AppSettings`), and no mutable borrow exists at the insertion point.
- Scope leak from the relocated hoist (Delta 4a): **none** — `teams_snapshot` stays in the outer function scope; the construction sites inside nested loops (line 1091) capture it by immutable reference via `&teams_snapshot`, which is valid for the entire function body.
- Async/await subtleties from moving the snapshot past `ensure_ac_new_gitignore(&ac_new_dir)`: **none** — `ensure_ac_new_gitignore` is sync (no `.await`), so no interleaving.

### 16.8 Overall verdict — **APPROVE** (conditional on NIT-B fix)

Green for Step 6 implementation, with one caveat:

- **NIT-A (line-number)**: optional cosmetic fix to §15 prose; can be deferred to a follow-up or just tolerated.
- **NIT-B (debug level suppression)**: **apply before implementation**. Recommended Option 1: change Delta 9 from `log::debug!` → `log::info!`. One-character swap in §15; eliminates the "test step produces no output" failure mode that a reviewer would otherwise hit on first run.

If architect agrees with Option 1 (quick patch in §15), grinch approval stands and dev-rust can proceed. If architect prefers Option 2 (env-var instruction in Delta 10), also fine — just updates Delta 10 prose. Either fix closes NIT-B.

No round 3 needed. No correctness bug found. No logic drift. No scope creep. Plan is implementable as-is with one trivial `debug!` → `info!` swap in Delta 9.
