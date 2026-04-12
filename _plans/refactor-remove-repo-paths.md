# Plan: Remove `repo_paths` and replace with `project_paths`

**Branch:** `feature/start-only-coordinators`
**Status:** READY FOR IMPLEMENTATION

---

## Requirement

`settings.repo_paths` (JSON: `repoPaths`) is a dead field. In ALL production instances (standalone, mb, phidimensions), `repoPaths` is `[]`. All scanning code that reads `repo_paths` produces empty results. The actual paths live in `settings.project_paths` (JSON: `projectPaths`).

Remove `repo_paths` from the Rust struct and TypeScript interface. Replace every backend usage with `project_paths`. Backward compatible: serde silently ignores unknown JSON fields (no `deny_unknown_fields`), so existing `settings.json` files with a `repoPaths` key are fine.

**DO NOT TOUCH** `agent.repo_paths` / `AcAgentReplica.repoPaths` — that is a per-agent concept from `config.json`, completely unrelated.

---

## Changes (8 files)

### 1. `src-tauri/src/config/settings.rs` — Remove field + Default

**Remove struct field.** Delete lines 37–38:

```rust
    /// Agent folders and parent folders to scan for potential agents
    pub repo_paths: Vec<String>,
```

**Simplify Default impl.** Replace lines 143–155 (the destructuring tuple):

```rust
        let (default_shell, default_shell_args, repo_paths) = if cfg!(target_os = "windows") {
            (
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string()],
                vec![],
            )
        } else {
            (
                "/bin/bash".to_string(),
                vec![],
                vec![format!("{}/repos", dirs::home_dir().unwrap_or_default().display())],
            )
        };
```

With:

```rust
        let (default_shell, default_shell_args) = if cfg!(target_os = "windows") {
            (
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string()],
            )
        } else {
            (
                "/bin/bash".to_string(),
                vec![],
            )
        };
```

**Remove from Self block.** Delete line 160:

```rust
            repo_paths,
```

---

### 2. `src/shared/types.ts` — Remove from TS interface

**Delete line 118:**

```typescript
  repoPaths: string[];
```

---

### 3. `src-tauri/src/config/teams.rs` — Replace in `discover_teams()`

**Line 238** — update doc comment:

```rust
/// Scans settings.repo_paths (and immediate children) for `.ac-new/_team_*/config.json`.
```
→
```rust
/// Scans settings.project_paths (and immediate children) for `.ac-new/_team_*/config.json`.
```

**Line 243** — replace field access:

```rust
    for repo_path in &settings.repo_paths {
```
→
```rust
    for repo_path in &settings.project_paths {
```

---

### 4. `src-tauri/src/cli/list_peers.rs` — Replace in WG replica discovery

**Line 497** — update comment:

```rust
    // Scan repo_paths for .ac-new/wg-*/__agent_* replicas
```
→
```rust
    // Scan project_paths for .ac-new/wg-*/__agent_* replicas
```

**Line 499** — replace field access:

```rust
    for base_path in &settings.repo_paths {
```
→
```rust
    for base_path in &settings.project_paths {
```

---

### 5. `src-tauri/src/commands/repos.rs` — Replace in `search_repos`

**Line 98** — replace field access:

```rust
    for base_path in &cfg.repo_paths {
```
→
```rust
    for base_path in &cfg.project_paths {
```

---

### 6. `src-tauri/src/commands/ac_discovery.rs` — Replace in `discover_ac_agents()`

**Line 424** — replace field access:

```rust
    for base_path in &cfg.repo_paths {
```
→
```rust
    for base_path in &cfg.project_paths {
```

**DO NOT TOUCH** any other `repo_paths` in this file (lines 80, 240, 242, 256, 556, 573, 574, 585, 868, 883, 884, 895). Those are all `agent.repo_paths` / `AcAgentReplica.repo_paths` — a different concept (per-agent repo list from agent `config.json`).

---

### 7. `src-tauri/src/web/commands.rs` — Replace in web server handler

**Line 227** — replace field access:

```rust
            let repo_paths = cfg.repo_paths.clone();
```
→
```rust
            let repo_paths = cfg.project_paths.clone();
```

(Keeping the local variable name `repo_paths` to minimize diff in the surrounding code at line 235. A rename to `project_paths` is optional.)

---

### 8. `src-tauri/src/phone/mailbox.rs` — Replace in `poll()` and `resolve_target_root()`

**Line 70** — replace in `poll()`:

```rust
            cfg.repo_paths.clone()
```
→
```rust
            cfg.project_paths.clone()
```

**Line 1084** — update comment:

```rust
        // Check settings repo_paths
```
→
```rust
        // Check settings project_paths
```

**Line 1087** — replace in `resolve_target_root()`:

```rust
        for rp in &cfg.repo_paths {
```
→
```rust
        for rp in &cfg.project_paths {
```

**Line 1112** — update comment:

```rust
        // Check WG replicas: "wg-name/agent" → scan repo_paths for .ac-new/wg-name/__agent_agent/
```
→
```rust
        // Check WG replicas: "wg-name/agent" → scan project_paths for .ac-new/wg-name/__agent_agent/
```

**Line 1116** — replace in WG replica scan:

```rust
                for rp in &cfg.repo_paths {
```
→
```rust
                for rp in &cfg.project_paths {
```

---

### Optional: `src/sidebar/components/SettingsModal.tsx` — Update comment

**Line 172** — cosmetic:

```typescript
    // Refresh repos (repo_paths may have changed)
```
→
```typescript
    // Refresh repos (project_paths may have changed)
```

---

## `project_path` (singular) — DO NOT REMOVE

The tech-lead asked to check if the backend uses `project_path` (singular, `settings.rs:96-98`). Findings:

- **Backend:** Nothing reads `settings.project_path` directly. No Rust code ever accesses this field.
- **Frontend (via IPC):** Used actively:
  - `App.tsx:102` — passes `appSettings.projectPath` to `projectStore.initFromSettings()` as the legacy migration path
  - `project.ts:66-75` — `initFromSettings(projectPaths, legacyPath)` merges the singular path into the array (deduped)
  - `project.ts:148` — `persistProjectPaths()` writes `projectPath: paths[0]` for backward compat

**Risk of removing:** Users who only have `projectPath` set in `settings.json` (never used multi-project) would lose their loaded project. The field exists in the Rust struct solely to pass it through IPC to the frontend. Removing it from Rust = removing it from the `get_settings` response = breaking the migration path.

**Recommendation:** KEEP `project_path` in the Rust struct. It's not dead — it's a backward-compat IPC bridge. Remove it only after a migration cycle where all instances have saved at least once (which writes `projectPaths` from the frontend).

---

## What does NOT change

- `AcAgentReplica.repo_paths` (Rust) / `AcAgentReplica.repoPaths` (TS) — per-agent concept, untouched
- `settings.project_paths` / `settings.project_path` — kept as-is
- `AppSettings` serialization — `rename_all = "camelCase"` handles naming automatically
- Backward compat — existing `settings.json` with `repoPaths` key: serde ignores unknown fields

---

## Dependencies

None.

---

## Notes

1. **Immediate functional impact:** After this change, features that were silently broken (producing empty results due to `repo_paths: []`) will start working because they'll read `project_paths` which contains the user's actual project paths. This includes:
   - `discover_teams()` — will find teams in loaded projects
   - `discover_ac_agents()` — will find agents in loaded projects
   - `list-peers` CLI — will discover WG replicas
   - `search_repos` — will return results
   - `mailbox.poll()` — will scan loaded project outboxes
   - `resolve_target_root()` — will resolve agent paths from project_paths
   - Web server repo search — will return results

2. **Double scan in `mailbox.poll()`:** Line 80 initializes `all_paths` from `repo_paths` (now `project_paths`), then lines 81-84 add session CWDs. After this change, `project_paths` will have actual content, so the CWD fallback becomes a supplement rather than the sole data source. No logic change needed — deduplication at line 82 prevents double-processing.

3. **`discover_teams()` called from `lib.rs` restore loop:** The `start_only_coordinators` feature (same branch) calls `discover_teams()` during session restore. After this change, `discover_teams` will actually find teams (instead of scanning empty `repo_paths`). This makes the coordinator-only-start feature functional.

---

## Dev-Rust Review

**Reviewer:** dev-rust | **Date:** 2026-04-12

### Verification results

| Item | Status |
|---|---|
| settings.rs lines 37-38, 143-155, 160 | CORRECT |
| types.ts line 118 | CORRECT |
| teams.rs lines 238, 243 | CORRECT |
| list_peers.rs lines 497, 499 | CORRECT |
| repos.rs line 98 | CORRECT |
| ac_discovery.rs line 424 | CORRECT |
| web/commands.rs line 227 | CORRECT |
| mailbox.rs lines 70, 1084, 1087, 1112, 1116 | CORRECT (all 5 sites) |
| SettingsModal.tsx line 172 (optional) | CORRECT |
| `AcAgentReplica.repo_paths` NOT affected | CONFIRMED — separate struct in ac_discovery.rs:80 and types.ts:224, all usages are `agent.repo_paths` / `replica.repoPaths`, untouched |

### Exhaustive grep — all `repo_paths` / `repoPaths` occurrences categorized

**Rust (`repo_paths`) — 30 occurrences:**
- 3 in settings.rs → **field removal** (covered by plan)
- 2 in teams.rs → **replace** (covered)
- 2 in list_peers.rs → **replace** (covered)
- 1 in repos.rs → **replace** (covered)
- 2 in ac_discovery.rs (settings access, lines 424 + 743 doc comment) → line 424 covered, **line 743 MISSED** (see below)
- 11 in ac_discovery.rs (agent/replica `repo_paths`) → DO NOT TOUCH ✓
- 1 in web/commands.rs → **replace** (covered)
- 5 in mailbox.rs → **replace** (covered)
- 3 local variables (mailbox.rs:68/80, web/commands.rs:235) → fed by the replaced field, no change needed

**TypeScript (`repoPaths`) — 15 occurrences:**
- 1 in types.ts:118 (`AppSettings`) → **remove** (covered)
- 1 in types.ts:224 (`AcAgentReplica`) → DO NOT TOUCH ✓
- 6 in AcDiscoveryPanel.tsx → all `replica.repoPaths` → DO NOT TOUCH ✓
- 7 in ProjectPanel.tsx → all `replica.repoPaths` → DO NOT TOUCH ✓

### Missed reference

**ac_discovery.rs line 743** — doc comment on `discover_project`:
```rust
/// Unlike discover_ac_agents which scans repo_paths from settings,
```
Should be updated to:
```rust
/// Unlike discover_ac_agents which scans project_paths from settings,
```
This is cosmetic (doc comment only) but inconsistent if left. Recommend adding to the plan as a 9th change or folding it into the ac_discovery.rs change (file 6).

### mailbox.rs analysis (3 replacement sites + fallback logic)

Verified all 3 replacement sites:
1. **Line 70** (`poll()`) — `cfg.repo_paths.clone()` → feeds `all_paths` at line 80. Session CWDs are appended as supplementary paths (lines 81-84). After this change, `project_paths` provides actual content, CWD fallback becomes a supplement. Dedup at line 82 prevents double-processing. **Safe.**
2. **Line 1087** (`resolve_target_root()`, first scan) — iterates settings paths looking for agent name match. After change, will scan actual project dirs. **Safe.**
3. **Line 1116** (`resolve_target_root()`, WG replica scan) — iterates settings paths looking for `.ac-new/wg-name/__agent_X/` subdirs. After change, will scan actual project dirs. **Safe.**

The fallback chain in `resolve_target_root()` is: session CWDs → settings paths → discovered teams → WG replica scan. All 4 levels work independently. Replacing `repo_paths` with `project_paths` strengthens the second level (was always empty, now has real data). No logic change needed.

### Summary

- **ALL line numbers verified CORRECT** against current codebase (post start-only-coordinators commit)
- **AcAgentReplica.repo_paths confirmed NOT affected** — 18 occurrences across 4 files, all untouched
- **1 missed doc comment** at ac_discovery.rs:743 (low severity, cosmetic)
- **No edge cases found** — the replacement is purely mechanical (field rename from dead source to live source)
- **APPROVED for implementation**
