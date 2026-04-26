# Fix issue #84 — Wire `exclude_global_claude_md` on Agent Matrix and Workgroup creation

- **Issue**: https://github.com/mblua/AgentsCommander/issues/84 — "Exclude global CLAUDE.md flag ignored when creating agents via Agent Matrix"
- **Branch**: `bug/84-wire-exclude-claude-md-on-agent-creation` (already created from `origin/main`, checked out)
- **Scope**: `repo-AgentsCommander` only. Two function bodies in `src-tauri/src/commands/entity_creation.rs`. No UI, no schema, no IPC, no helper changes.

Line numbers in this plan were verified against the current tip of `bug/84-wire-exclude-claude-md-on-agent-creation` (aligned with `origin/main`). Dev can apply offsets 1:1.

---

## 1. Bug summary

The "Exclude global CLAUDE.md on agent creation" flag (`AgentConfig.exclude_global_claude_md`) only takes effect for two legacy code paths:

- `NewAgentModal.tsx` (parent-folder + name + select-coding-agent flow), via Tauri command `write_claude_settings_local`.
- CLI `create-agent --launch <id>` in `src-tauri/src/cli/create_agent.rs`.

The two code paths the user actually exercises in the current UI — **`create_agent_matrix`** (creating an agent inside a loaded project) and **`create_workgroup`** (instantiating workgroup replicas from a team) — never call the helper that writes `.claude/settings.local.json`. The flag is silently ignored for those flows.

## 2. Root cause

`config::claude_settings::ensure_claude_md_excludes(dir)` (in `src-tauri/src/config/claude_settings.rs:9`) is correct and idempotent — it merges into an existing `<dir>/.claude/settings.local.json` and adds `claudeMdExcludes` pointing at `<HOME>/.claude/CLAUDE.md` (forward-slash path).

The flag is also correctly stored on `AgentConfig.exclude_global_claude_md` (`src-tauri/src/config/settings.rs:20`) and surfaced in the UI (`SettingsModal.tsx:461-471`, defaulted to `true` for the Claude Code preset in `agent-presets.ts:22` and `OnboardingModal.tsx:18,73`).

What is missing is the **invocation** of the helper from the two Tauri commands that own Agent Matrix and Workgroup creation:

- `commands/entity_creation.rs::create_agent_matrix` (line 178) — creates `.ac-new/_agent_<name>/` with `Role.md`, `config.json`, and the `memory/plans/skills/inbox/outbox/` subdirs. Never reads `AppSettings`.
- `commands/entity_creation.rs::create_workgroup` (line 431) — creates `wg-N-<team>/__agent_<name>/` replica dirs with `inbox/`, `outbox/`, and `config.json`. Never reads `AppSettings`.

The reference pattern already exists in `cli/create_agent.rs:122-140`: load settings, find a matching agent, if `agent.exclude_global_claude_md` is true → call `ensure_claude_md_excludes(&agent_dir)`, log a warning on error, do **not** propagate.

## 3. Approach

Add invocation of `ensure_claude_md_excludes` to both `create_agent_matrix` and `create_workgroup`, **trigger-by-fleet**: if **any** `AgentConfig` in the in-memory `AppSettings` has `exclude_global_claude_md: true`, write the file. Settings are read via Tauri DI (`State<'_, SettingsState>`), not via `load_settings()` — see §13 Round 2 for the rationale (canonical source-of-truth, no race with `save_settings` torn writes, no sync I/O in async runtime). Rationale for the gate itself:

- Agent Matrix and Workgroup replicas are launched against any coding agent at runtime — we don't know which one when creating the dir. We can't gate per-agent.
- The generated `.claude/settings.local.json` is inert for Codex and Gemini (they don't read it). For Claude Code, it does the work the user asked for.
- Idempotent. Safe to run on dirs that already have the file (helper merges).
- Conservative: if **no** agent has the flag, the file is not written. Honors the user's choice when they unticked it for every coding agent.

### 3.1 Known asymmetry vs legacy paths

The two existing call-sites (`NewAgentModal.tsx:75` and `cli/create_agent.rs:136`) gate **per-agent** because they know which coding agent is being launched at the moment of creation. The two new call-sites (`create_agent_matrix`, `create_workgroup`) gate **fleet-wide (`any()`)** because the binding to a coding agent is deferred to runtime.

Concretely: with two Claude entries (A flag=true, B flag=false), launching B via the legacy modal does NOT write the file, but creating a matrix or workgroup does. This is structurally unavoidable without a UX change ("which coding agent will you use for this matrix?"), which is out of scope for #84. The divergence is documented here, in §6 (trigger table includes a mixed-flags row), in §8.1 #5 (test row exercising the asymmetry), and **must be mentioned in the commit message** for traceability.

## 4. Files to touch

| File | Change |
|---|---|
| `src-tauri/src/commands/entity_creation.rs` | (a) One new `use` line for `SettingsState` at top of file (`tauri::State` is already imported on line 6). (b) Add `settings: State<'_, SettingsState>` parameter to `create_agent_matrix` signature; insert helper invocation block at end of body. (c) Add the same parameter to `create_workgroup` signature (alongside the existing `State<>`s); compute the gate once before the replica loop; invoke the helper inside the loop. |
| `src-tauri/src/config/claude_settings.rs` | (d) Add a 5-line comment block at the top of the file listing the four callers (anti-regression paper trail per §12.5 mitigation (a)). No code change. |

No changes to:

- `src-tauri/src/config/claude_settings.rs` (helper logic) — helper is correct and tested via existing manual flows. Only a leading comment block is added per (d) above; no code change.
- `src-tauri/src/config/settings.rs` — `AgentConfig.exclude_global_claude_md` already defined and persisted. `SettingsState` type and `lib.rs:238` registration are reused as-is.
- `src-tauri/src/cli/create_agent.rs` — already cabled correctly (lines 134-140).
- `src-tauri/src/commands/agent_creator.rs` — `write_claude_settings_local` Tauri command still in use by `NewAgentModal.tsx`.
- `src/sidebar/components/NewAgentModal.tsx` — already cables the helper via `AgentCreatorAPI.writeClaudeSettingsLocal` (lines 73-81).
- `src/sidebar/components/SettingsModal.tsx`, `OnboardingModal.tsx`, `agent-presets.ts`, `types.ts` — UI/schema is correct.
- `src/guide/components/HintsTab.tsx` — copy is accurate; the key works, the bug was only that we never wrote it for these two flows.
- Any environment-variable mechanism (`CLAUDE_CONFIG_DIR`, `HOME`, `USERPROFILE`) — out of scope.

## 5. Exact code changes

### 5.1 Imports

Add at the top of `src-tauri/src/commands/entity_creation.rs`, alongside the existing `use crate::commands::ac_discovery::DiscoveryBranchWatcher;` block (around line 8):

```rust
use crate::config::claude_settings::ensure_claude_md_excludes;
use crate::config::settings::SettingsState;
```

Place these together with the other `use crate::...` lines for readability. Order is not load-bearing.

`tauri::State` is already imported on line 6 (`use tauri::{AppHandle, Emitter, State};`) — no change needed there.

### 5.2 `create_agent_matrix` — function at line 178

**Current signature** (lines 178-182):

```rust
#[tauri::command]
pub async fn create_agent_matrix(
    project_path: String,
    name: String,
    description: String,
) -> Result<CreatedEntityResult, String> {
```

**After the change** — add `settings: State<'_, SettingsState>` as the first parameter. Tauri's command macro auto-injects `State<>` parameters; the frontend `invoke()` call is unchanged (frontend still passes `{ projectPath, name, description }`). Pattern matches `commands/config.rs:24,33,75,123` and `commands/ac_discovery.rs:562,981`.

```rust
#[tauri::command]
pub async fn create_agent_matrix(
    settings: State<'_, SettingsState>,
    project_path: String,
    name: String,
    description: String,
) -> Result<CreatedEntityResult, String> {
```

**Current end of function** (lines 213-220):

```rust
    // config.json
    std::fs::write(agent_dir.join("config.json"), "{\n  \"tooling\": {}\n}\n")
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    let result_path = agent_dir.to_string_lossy().to_string();
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}
```

**After the change** — insert the invocation block between writing `config.json` and the final `let result_path = ...`. Pseudo-code:

```rust
    // config.json
    std::fs::write(agent_dir.join("config.json"), "{\n  \"tooling\": {}\n}\n")
        .map_err(|e| format!("Failed to write config.json: {}", e))?;

    // Issue #84 — auto-generate .claude/settings.local.json if any configured
    // coding agent has `exclude_global_claude_md`. Inert for Codex/Gemini.
    // Reads from in-memory SettingsState (kept in sync by `update_settings` in
    // commands/config.rs:32-44). Avoids the disk-read race that load_settings()
    // would have against a concurrent save_settings() (see §13.2).
    let exclude_claude_md = {
        let s = settings.read().await;
        s.agents.iter().any(|a| a.exclude_global_claude_md)
    };
    if exclude_claude_md {
        if let Err(e) = ensure_claude_md_excludes(&agent_dir) {
            log::warn!(
                "[entity_creation] Failed to write .claude/settings.local.json for {}: {}",
                agent_dir.display(),
                e
            );
        }
    }

    let result_path = agent_dir.to_string_lossy().to_string();
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}
```

**Notes**:

- Position: after `Role.md` and `config.json` are written, before constructing the result. The helper requires the dir to exist (it does — created at line 195) and creates `.claude/` itself. Sub-directory creation order with the `memory/plans/...` block at lines 198-201 is irrelevant: `.claude/` is independent.
- Error handling: `log::warn!`, do not propagate. Matches `cli/create_agent.rs:137-139` semantics: agent creation succeeds even if the settings-local write fails. The user can re-run via the legacy "New Agent" flow or fix the file manually if needed.
- The read guard scope (`{ let s = settings.read().await; ... }`) is intentionally narrow so the `RwLock` is dropped before the (sync) helper invocation. No deadlock risk even if the helper grew an async path later.

### 5.3 `create_workgroup` — function at line 431

**Current signature** (lines 431-436):

```rust
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<WorkgroupCloneResult, String> {
```

**After the change** — add `settings: State<'_, SettingsState>` alongside the existing `session_mgr` State. Frontend `invoke()` call is unchanged.

```rust
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    project_path: String,
    team_name: String,
) -> Result<WorkgroupCloneResult, String> {
```

**Current loop creating replica dirs** (lines 515-573):

```rust
    // Create __agent_*/ replica dirs
    for agent_path in &team_agents {
        let agent_dir_name = Path::new(agent_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(agent_path);

        // Extract the clean agent name (strip _agent_ prefix)
        let agent_name = agent_dir_name
            .strip_prefix("_agent_")
            .unwrap_or(agent_dir_name);

        let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
        std::fs::create_dir_all(&replica_dir)
            .map_err(|e| format!("Failed to create replica dir for {}: {}", agent_name, e))?;

        // inbox/ and outbox/
        for sub in &["inbox", "outbox"] {
            std::fs::create_dir_all(replica_dir.join(sub))
                .map_err(|e| format!("Failed to create {} for {}: {}", sub, agent_name, e))?;
        }

        // ...config.json computation and write...
    }
```

**After the change** — snapshot the gate **once before the loop** (deliberate: every replica in the same workgroup must get consistent treatment, even if the user toggles the flag mid-creation), then invoke the helper inside the loop after `replica_dir` is created:

```rust
    // Issue #84 — snapshot gate ONCE before the loop. Deliberate: all replicas
    // in this workgroup creation must use the same gate value. Mid-loop
    // toggles via update_settings are intentionally ignored — half-applied
    // workgroups would be worse than a stale snapshot.
    let exclude_claude_md = {
        let s = settings.read().await;
        s.agents.iter().any(|a| a.exclude_global_claude_md)
    };

    // Create __agent_*/ replica dirs
    for agent_path in &team_agents {
        let agent_dir_name = Path::new(agent_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(agent_path);

        let agent_name = agent_dir_name
            .strip_prefix("_agent_")
            .unwrap_or(agent_dir_name);

        let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
        std::fs::create_dir_all(&replica_dir)
            .map_err(|e| format!("Failed to create replica dir for {}: {}", agent_name, e))?;

        // inbox/ and outbox/
        for sub in &["inbox", "outbox"] {
            std::fs::create_dir_all(replica_dir.join(sub))
                .map_err(|e| format!("Failed to create {} for {}: {}", sub, agent_name, e))?;
        }

        // Issue #84 — write .claude/settings.local.json if any agent has the flag.
        if exclude_claude_md {
            if let Err(e) = ensure_claude_md_excludes(&replica_dir) {
                log::warn!(
                    "[entity_creation] Failed to write .claude/settings.local.json for replica {}: {}",
                    replica_dir.display(),
                    e
                );
            }
        }

        // ...config.json computation and write... (unchanged)
    }
```

**Notes**:

- Position: after `replica_dir` and its `inbox`/`outbox` subdirs exist, before the `config.json` write. The helper does not depend on `config.json` being present (and the helper's own `.claude/` subdir doesn't collide with anything in the replica).
- Snapshot semantics documented in the inline comment per §12.7. The read guard is dropped before the loop starts, so the `RwLock` is never held across the loop iterations or the (sync) helper invocations.
- Error handling: same as 5.2 — `log::warn!`, do not propagate. Workgroup creation continues even if one replica's settings file fails.

### 5.4 `claude_settings.rs` — anti-regression caller comment

Add the following 6-line comment block at the very top of `src-tauri/src/config/claude_settings.rs` (above `use std::path::Path;` on line 1). Per §12.5 mitigation (a). No code change.

```rust
// Callers of `ensure_claude_md_excludes` (must be kept in sync with any new
// agent-creation flow — see issue #84 for the original miss):
//   - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd, NewAgentModal.tsx)
//   - cli/create_agent.rs (CLI `create-agent --launch <id>`)
//   - commands/entity_creation.rs::create_agent_matrix
//   - commands/entity_creation.rs::create_workgroup (per-replica)
```

This is a grep-able paper trail. If a future dev adds a fifth agent-creation path and forgets the helper, anyone touching `claude_settings.rs` will see the list and notice the omission. The cost is 6 lines; the alternative (integration tests requiring `tempfile` in `[dev-dependencies]` plus a fresh `mod tests` block) was deemed disproportionate per §11.9.

## 6. Trigger logic — full spec

**Condition**: `settings.read().await.agents.iter().any(|a| a.exclude_global_claude_md)`.

| Scenario | `agents` length | Any flag true? | Helper invoked? |
|---|---|---|---|
| Fresh install, no settings.json | n/a (`AppSettings::default()` → `agents: vec![]`) | no | **no** |
| User dismissed onboarding without adding agents | 0 | no | **no** |
| User added Claude Code via preset (default) | ≥1 | yes (preset = true) | **yes** |
| User added only Codex/Gemini (default flag false) | ≥1 | no | **no** |
| User added Claude Code but unticked the flag | ≥1 | no | **no** |
| Mix of Claude with flag + Codex without | ≥2 | yes | **yes** |
| **Mixed flags within one type**: Claude A (flag=true) + Claude B (flag=false) — the asymmetry case from §3.1 | ≥2 | yes | **yes** for matrix/workgroup; legacy `NewAgentModal` invokes only when the user picks A |

Edge cases to keep in mind for dev:

- `SettingsState` is initialized at app start by `load_settings()` in `lib.rs:238`. By the time any `#[tauri::command]` fires, the state is populated (or holds an `AppSettings::default()` if startup load failed). The `read().await` never blocks indefinitely — `update_settings` only holds the write lock for the duration of `*s = to_save`.
- The flag is per `AgentConfig`, not per coding-agent-type. If the user has two Claude entries with different values, `any()` returns `true` and the file is written. Acceptable: the flag's user-facing meaning is "create the exclude file when *any* of my coding agents wants this", and any positive intent fires the wiring. See §3.1 for the legacy-vs-new asymmetry implication.

## 7. Idempotency and ordering

- `ensure_claude_md_excludes` already handles the case where `.claude/settings.local.json` exists with other keys (`claude_settings.rs:25-50`): parses the JSON, treats parse errors / non-objects as empty `{}`, preserves existing keys, appends to `claudeMdExcludes` array if not already present. Re-running is safe.
- Both invocations sit **after** the dir's mandatory contents (`Role.md` + `config.json` for matrix; subdirs for replica) are created. If the helper fails, the agent/replica is still functionally complete.
- `create_workgroup` calls `ensure_ac_new_gitignore(&base)` early (line 444). The new `.claude/` we create inside `__agent_*` is not a separate gitignore concern — `.ac-new/` is already gitignored at the parent level.
- **Known limitation (per §12.6)**: `ensure_claude_md_excludes` calls `create_dir_all(&claude_dir)` before the JSON write. If the `create_dir_all` succeeds but the subsequent `std::fs::write` fails (disk full, AV interception, transient permission flip), the agent/replica dir is left with an empty `.claude/` subdir. This is cosmetic — a re-run is idempotent (the helper finds the empty dir and proceeds normally). Not transactional, not worth fixing for #84.

## 8. Test plan

Test plan consolidated per §11.9, §11.10, §12.4, §12.5. The original §8.1 #5 (merge-with-existing-file) is **dropped** because, as §12.4 documented, both `create_agent_matrix` and `create_workgroup` early-return if the target dir already exists — the path is structurally unreachable from the new wiring. The merge logic is exercised by the legacy `NewAgentModal` path (regression-checked in §8.2 below), and the helper itself is unchanged in this fix.

### 8.1 Manual functional tests (build a debug binary, click through the UI)

1. **Agent Matrix with flag = true** (Claude preset default).
   - Open AgentsCommander against a project that has a `.ac-new/`.
   - Create a new Agent Matrix from the sidebar, name it `t1`.
   - Verify `<project>/.ac-new/_agent_t1/.claude/settings.local.json` exists with:
     ```json
     {
       "claudeMdExcludes": [
         "<absolute home path>/.claude/CLAUDE.md"
       ]
     }
     ```
     (forward-slash path).

2. **Agent Matrix flag exclusion smoke test**.
   - With the agent created in 8.1, launch a Claude Code session at `<project>/.ac-new/_agent_t1/` (cwd = that dir).
   - In the session, ask "what do you have in your context?" or check if the `# claudeMd` block referencing `~/.claude/CLAUDE.md` appears.
   - Expected: the global CLAUDE.md content does NOT appear.
   - This is the user-facing pass/fail. Empirically validated by tech-lead — the key works when applied at the cwd.

3. **Workgroup replicas**.
   - Create a team `t2` referencing existing agent matrix `t1`.
   - Create a workgroup from team `t2` (UI button or `create_workgroup` IPC).
   - Verify `<project>/.ac-new/wg-1-t2/__agent_t1/.claude/settings.local.json` exists with the same shape.
   - Repeat with multiple agents in the team — each replica must have the file.

4. **Flag = false everywhere**.
   - In Settings → Coding Agents, untick "Exclude global CLAUDE.md on agent creation" for every configured coding agent. Save settings.
   - Create a new Agent Matrix `t3`.
   - Verify `<project>/.ac-new/_agent_t3/.claude/` does NOT exist (helper not invoked → dir not created). The agent is still created with `Role.md` and `config.json`.
   - Same for a new workgroup: replicas have `inbox/`, `outbox/`, `config.json`, but NO `.claude/`.

5. **Mixed-flag asymmetry vs legacy** (per §12.1 / §3.1).
   - Configure two Claude Code entries: "Claude A" (flag=true) and "Claude B" (flag=false). Save settings.
   - **New path**: create a new Agent Matrix `t5a`. Verify `<project>/.ac-new/_agent_t5a/.claude/settings.local.json` **exists** (because `any()` is true).
   - **Legacy path**: open the legacy "New Agent" modal (`NewAgentModal`), pick parent + name `t5b`, then in the launch step pick "Claude B". Verify the resulting agent dir does **NOT** have `.claude/settings.local.json` (per-agent gate; B's flag is false).
   - **Legacy path again**: same modal, new agent `t5c`, this time pick "Claude A". Verify `.claude/settings.local.json` **exists**.
   - This documents the structural asymmetry. Pass condition: both observations match the table above. The asymmetry is documented in §3.1 and the commit message; the test ensures it stays observable, not that it's hidden.

6. **No settings.json**.
   - Backup `<config_dir>/settings.json` and delete it.
   - Restart AgentsCommander. UI will run onboarding.
   - Without completing onboarding, attempt to trigger matrix/workgroup creation through the IPC (or just complete onboarding skipping agent setup).
   - Verify no `.claude/` directory is created. No crash.
   - With `SettingsState` (DI), this case is handled by `lib.rs:238`'s startup `load_settings()` returning `AppSettings::default()` with empty `agents` — the `any()` check returns `false`. No `load_settings()` call from inside the new wiring.

### 8.2 Regression checks (legacy paths must remain functional)

- `cli/create_agent.rs --launch <id>` path — already cabled (lines 134-140), must still work. Smoke test by running the CLI command with a Claude agent id and verifying the resulting `.claude/settings.local.json` shape.
- `NewAgentModal.tsx` legacy "New Agent" path — already cabled via Tauri `write_claude_settings_local`. Smoke test by triggering the modal, picking a Claude agent with `excludeGlobalClaudeMd: true`, and verifying the settings file is still written. This also exercises the helper's merge logic against any pre-existing `.claude/settings.local.json` keys (e.g., user-local `permissions`) — covering the scenario the dropped §8.1 #5 originally targeted.

### 8.3 Automated tests — none added

Per §11.9 and §12.5, no new automated tests are added in this fix:

- **Unit tests** in `entity_creation.rs` would require creating a fresh `mod tests`, refactoring the gate block into an inner helper, and adding `tempfile` to `[dev-dependencies]`. ~30+ lines of infrastructure for a ~30-line fix. Disproportionate.
- **Helper-level tests** in `claude_settings.rs` (the cheaper alternative from §11.9) would still require a fresh `mod tests` block plus a `tempfile`-backed harness. The helper has been in production since the legacy path shipped without regressions; deferring its tests until someone needs them for another feature is the pragmatic call.
- **Anti-regression mitigation**: §5.4 adds a 6-line caller comment to `claude_settings.rs` listing the four agent-creation flows. A future dev who adds a fifth flow without grepping for `ensure_claude_md_excludes` will at least see the list when they read the helper (and most will, because the helper is small and focused). It's a paper trail, not a guarantee.

If grinch insists on automated coverage in Round 3, the cheapest add is the helper-level test (~15 lines + `tempfile` dev-dep). Architect's position: defer to a separate issue tracking integration-test infrastructure for `entity_creation.rs` broadly (team CRUD, workgroup CRUD, etc.), since that file currently has zero tests and #84 is not the right vehicle to bootstrap them.

## 9. No-go scope

To prevent scope creep, the following are explicitly **not** part of this fix:

- **UI changes**: `SettingsModal.tsx`, `OnboardingModal.tsx`, `NewAgentModal.tsx`, `HintsTab.tsx` are correct and remain as-is.
- **Schema / types**: `AgentConfig.exclude_global_claude_md`, `AppSettings`, `src/shared/types.ts` are unchanged.
- **IPC**: no new Tauri commands. `write_claude_settings_local` and `pick_folder` / `create_agent_folder` in `commands/agent_creator.rs` are unchanged.
- **Helper logic**: `config/claude_settings.rs` is correct (path normalization, merge behavior). Do not refactor.
- **CLI**: `cli/create_agent.rs` is already cabled correctly. Do not touch.
- **PTY env vars**: `pty/manager.rs` (line 324 onward) does not need to set `CLAUDE_CONFIG_DIR`, `HOME`, or `USERPROFILE`. The fix is purely file-based at the cwd.
- **`claude-mb` wrapper**: out of scope. The wrapper sets `CLAUDE_CONFIG_DIR` but that does not affect `~/.claude/CLAUDE.md` discovery; the file-based exclude at the cwd works under both `claude` direct and `claude-mb`.
- **Existing replicas / agents**: this fix only takes effect for **newly created** matrices and replicas. Existing dirs without `.claude/settings.local.json` are not retroactively patched. If the user wants to fix old agents, they can recreate them or run the helper manually. Backfill is a separate feature — not part of #84.
- **`save_settings` atomicity** (per §12.3): pre-existing NIT — `save_settings` truncate-and-write is non-atomic. Adopting `SettingsState`-based reads in this fix removes the symptom for the new call-sites (in-memory state has no torn-read window). The underlying disk-write atomicity issue remains for any code path still calling `load_settings()` directly, but that's pre-existing and out of scope. **Action**: file a follow-up issue for `save_settings` to use write-to-tmp + fsync + rename, separate from #84.
- **Branch / PR**: no merge to `main`, no push to `origin/main` from dev-rust. Scope ends at "dev-rust commits + pushes to `bug/84-wire-exclude-claude-md-on-agent-creation`, shipper builds for test". Merge decision is the user's.

## 10. Rough size estimate

- **Lines of code**: ~35 lines added — imports (2 lines), two signature changes (1 line each), gate-snapshot blocks in both functions (~6 lines each), invocation blocks (~10 lines each), plus the 6-line caller comment in `claude_settings.rs`. Net: still small.
- **Risk**: low. No control-flow changes for failure paths, no public API changes (Tauri command frontend invoke unchanged — `State<>` params are auto-injected), no new types. The new code only adds a write under a guard that defaults to "do nothing" if no agent has the flag.
- **Resolved review concerns** (Round 1 → Round 2 outcomes):
  - (a) `load_settings()` vs DI — Round 1 architect said "DI not necessary, disk read is cheap". **Round 2 outcome (§13.2)**: grinch correctly observed that Tauri DI is free (frontend pays nothing); benefits include canonical source-of-truth, no race with `save_settings` torn-write, no sync I/O in async runtime. **Adopted: switched to `State<'_, SettingsState>` + `settings.read().await`**.
  - (b) Per-agent gate vs `any()` — `any()` is correct because matrix/replica is bound to a coding agent only at runtime. **Round 2 outcome (§13.1)**: grinch noted asymmetry with legacy paths (per-agent there). Architect accepts asymmetry as structurally unavoidable; documented in §3.1 + commit message + §8.1 #5 test row.
  - (c) `log::warn!` vs propagating `Err` — parity with `cli/create_agent.rs:137-139`. Agent creation is the primary success criterion; settings file is a nice-to-have. **Unchanged**.

---

## 11. Dev-rust additions (verification + enrichment)

This section was added by **dev-rust** during the review/enrichment step (Step 3 of the role workflow). All claims below were verified against the working tree at `bug/84-wire-exclude-claude-md-on-agent-creation` (HEAD = `1bc94d8`, in sync with `origin/main`). The plan above is accurate; the additions here close ambiguities and pre-empt grinch's review.

### 11.1 Line/path verification — confirmed

| Plan claim | Verified location | Status |
|---|---|---|
| `entity_creation.rs::create_agent_matrix` at line 178 | line 178 (`pub async fn create_agent_matrix(`) | ✓ |
| End-of-`create_agent_matrix` block (config.json write → `Ok(...)`) at lines 213-220 | lines 213-219 + closing `}` on 220 | ✓ |
| `agent_dir` mkdir at line 195 | line 195 (`std::fs::create_dir_all(&agent_dir)`) | ✓ |
| `memory/plans/skills/inbox/outbox` loop at lines 198-201 | lines 198-201 | ✓ |
| `entity_creation.rs::create_workgroup` at line 431 | line 431 (`pub async fn create_workgroup(`) | ✓ |
| Replica creation loop at lines 515-573 | line 515 (`for agent_path in &team_agents {`) → line 573 (`std::fs::write(replica_dir.join("config.json")...)`) | ✓ |
| `replica_dir` mkdir at line 527 | line 527 (`std::fs::create_dir_all(&replica_dir)`) | ✓ |
| `inbox`/`outbox` at lines 530-534 | lines 531-534 (off-by-one in plan; `for sub in &["inbox", "outbox"] {` is on 531, not 530) | ✓ (cosmetic) |
| `ensure_ac_new_gitignore(&base)` invocation at line 444 | line 444 | ✓ |
| `use crate::commands::ac_discovery::DiscoveryBranchWatcher;` at line 8 (insertion target for new `use`s) | line 8 | ✓ |
| `claude_settings::ensure_claude_md_excludes` at `claude_settings.rs:9` | line 9 (`pub fn ensure_claude_md_excludes(dir: &Path) -> Result<(), String>`) | ✓ |
| Merge logic in helper at lines 25-50 | lines 24-54 (close enough for the plan's purpose) | ✓ |
| `AgentConfig.exclude_global_claude_md` at `settings.rs:20` | line 20 (`pub exclude_global_claude_md: bool,`) | ✓ |
| `cli/create_agent.rs:137-139` warn-and-continue pattern | lines 137-139 (`eprintln!("Warning: ...")`, not `log::warn!`, but the *semantic* pattern matches) | ✓ — see §11.6 below |

No line drift. Dev can apply the patch 1:1 using the line numbers in §5.

### 11.2 Imports — visibility and module path verified — UPDATED for Round 2

The `use` lines proposed in §5.1 (now post-§13.2) compile as written:

```rust
use crate::config::claude_settings::ensure_claude_md_excludes;
use crate::config::settings::SettingsState;
```

Why:
- `src-tauri/src/config/mod.rs:2` has `pub mod claude_settings;`.
- `claude_settings.rs:9` exposes `pub fn ensure_claude_md_excludes(dir: &Path) -> Result<(), String>`.
- `src-tauri/src/config/mod.rs:7` has `pub mod settings;`.
- `settings.rs:360` exposes `pub type SettingsState = Arc<RwLock<AppSettings>>` (re-used here; `lib.rs:238` registers the singleton).
- `tauri::State` is already imported in `entity_creation.rs:6` — no additional import line needed.

No additional imports are needed beyond these two. The `Path` type used by the helper is already in scope via `use std::path::{Path, PathBuf};` at `entity_creation.rs:3`.

> Round 1 originally proposed `use crate::config::settings::load_settings;`. Round 2 §13.2 replaced it with `SettingsState`. `load_settings()` is no longer called from the new wiring.

### 11.3 `log` crate availability — no feature flag needed

`src-tauri/Cargo.toml:15` has `log = "0.4"` as a regular (non-optional) dependency. `log::warn!` is already used throughout `entity_creation.rs` (e.g., lines 218, 423, 445, 581, 667, 797). The plan's pseudo-code in §5.2 / §5.3 will compile and emit at runtime exactly as the existing logs do.

### 11.4 `dirs` crate availability — confirmed

The helper depends on `dirs::home_dir()` (`claude_settings.rs:18`). `Cargo.toml:19` has `dirs = "6"`. No change needed in this fix.

### 11.5 Error type — `Result<(), String>` shape — UPDATED for Round 2

`ensure_claude_md_excludes` returns `Result<(), String>`. Both `create_agent_matrix` and `create_workgroup` already return `Result<T, String>` to Tauri (lines 182 and 436). The pseudo-code in §5.2 and §5.3 never propagates the helper's `Err` (only logs it via `log::warn!`), so no new `?` operators are introduced.

> Round 2 §13.2: function **signatures DO change** in this fix — both functions gain a `settings: State<'_, SettingsState>` parameter. The return types are unchanged; the auto-injected `State<>` does not affect the frontend `invoke()` shape. Round 1 dev-rust said "function signatures don't change" — that referred to return types and pre-existing parameters. The new `State<>` param is additive, frontend-invisible, and idiomatic in this codebase.

### 11.6 `cli/create_agent.rs` parity — clarification

The plan refers to `cli/create_agent.rs:137-139` as the parity reference. **Important**: that file uses `eprintln!`, not `log::warn!`, because the CLI subcommand runs without the Tauri logging subsystem fully initialized in the same way. Inside `entity_creation.rs` (which runs as Tauri `#[tauri::command]` handlers, with `env_logger` set up at app start), `log::warn!` is the right macro and matches the rest of the file. So the *behavior* matches (warn and continue, do not propagate) — the *macro* differs by context. No ambiguity once stated.

### 11.7 `load_settings()` side effect — OBSOLETED by Round 2 §13.2

> **Note (Round 2)**: this section described the `root_token` auto-gen side effect of `load_settings()`. Architect adopted §12.2 (use `State<'_, SettingsState>` instead of `load_settings()`), so this side effect no longer applies to the new call-sites. The startup `load_settings()` call in `lib.rs:238` still triggers the side effect once at app launch, which is unchanged from before this fix.
>
> Original section preserved below for context only.

~~At `settings.rs:331-337`, `load_settings()` auto-generates and **persists** a `root_token` to `<config_dir>/settings.json` if missing. This is idempotent (subsequent calls find the token and skip the write), but it's a write side effect on the *first* invocation against a wiped/fresh config. In practice this never bites us: after first launch, `root_token` is always set; the worst case is one extra disk write on a brand-new install.~~

### 11.8 No event emit needed — IPC surface unchanged

- `create_agent_matrix` does NOT take `AppHandle` (signature at `entity_creation.rs:178-182`). It cannot emit a Tauri event even if we wanted to. Frontend already refreshes the agent list via the `list_all_agents` command (sidebar polls / refreshes on user action). No new event required.
- `create_workgroup` already emits `emit_coordinator_refresh(&app, ...)` at line 599 after the loop. The newly-written `.claude/settings.local.json` does not change coordinator state, so no additional emit is needed. Frontend listeners are unchanged.

The fix is purely file-system, no IPC surface changes — matching the plan's §9 ("IPC: no new Tauri commands").

### 11.9 Test infrastructure — recommendation: skip §8.2 unit tests

`entity_creation.rs` has **no `#[cfg(test)] mod tests` block** (verified by grep). Adding §8.2's optional unit test would require:
1. Creating a fresh `mod tests { ... }` at the bottom of the file.
2. Refactoring the `if exclude_claude_md { ... }` block into a private `fn maybe_write_claude_settings(dir: &Path, settings: &AppSettings)` so it can be called in isolation.
3. Adding `tempfile` (or similar) to `[dev-dependencies]` — currently absent.

That's 30+ lines of test infrastructure for a 30-line fix. **Recommendation: drop §8.2.** The change is too small to justify the infra cost. The helper itself (`claude_settings.rs`) is unchanged and its merge logic already battle-tested via the cli/create_agent.rs `--launch` path that ships in production.

If grinch insists on a unit test, the cheaper alternative is to add a test in `claude_settings.rs` (which would have a `mod tests` of its own, also currently absent — but the surface is much smaller: pre-create a JSON file with a `permissions` key, call `ensure_claude_md_excludes`, assert both keys present). That isolates the test to the helper without touching `entity_creation.rs`.

### 11.10 §8 item 5 (merge with existing file) — clarification for executor

The plan's manual test #5 ("Merge with existing file") tests the **helper's** merge behavior, not our wiring. This fix doesn't change the helper. Two acceptable resolutions:

- **Option A (recommended)**: Demote item 5 from §8.1 to §8.3 (regression check) and rephrase as "Verify the helper's merge logic still works by manually invoking it via the legacy `New Agent` flow with a pre-existing `.claude/settings.local.json` containing a `permissions` key". This keeps the test in scope for a smoke-check without forcing dev to set up an awkward race with matrix-creation.
- **Option B**: Add a small unit test inside `claude_settings.rs` (per §11.9) that pre-creates a JSON object with `permissions.allow`, calls `ensure_claude_md_excludes`, and asserts both keys present. ~15 lines.

Either is fine. Dev-rust will default to **Option A** unless tech-lead/grinch asks for Option B in the consensus round.

### 11.11 Verification commands for dev (Step 4)

After implementation, dev-rust must run from `repo-AgentsCommander/`:

```bash
cargo check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

If `cargo test --manifest-path src-tauri/Cargo.toml` is part of CI, run it too — but no new tests are added in the recommended path (§11.9), so this should be a no-op aside from existing tests in `settings.rs` and elsewhere.

No new dependencies, no new feature flags, no new `[dev-dependencies]` — Cargo.toml is unchanged.

### 11.12 Minor plan nit

§10 mentions "settings.json is small and read is fast" and elsewhere mentions "TOML/JSON read". The settings file is JSON-only (verified at `settings.rs:313`: `serde_json::from_str::<AppSettings>(&contents)`). Cosmetic — doesn't affect implementation.

### 11.13 Dev-rust position on grinch's anticipated concerns (§10) — UPDATED for Round 2

| Concern | Round 1 architect's answer | Dev-rust Round 1 position | Round 2 outcome |
|---|---|---|---|
| (a) DI / inject `AppSettings` instead of calling `load_settings()` | Not necessary — settings.json read is cheap | Agreed: plumbing seemed costly | **Reversed**: grinch (§12.2) correctly noted that Tauri `State<T>` is auto-injected; the frontend `invoke()` is unchanged and bubbling cost is zero. Plan now uses `State<'_, SettingsState>` + `settings.read().await`. The §11.7 root_token concern becomes moot. |
| (b) Per-agent gate vs `any()` | `any()` is correct because matrix/replica is bound to a coding agent only at runtime | Agree | **Confirmed**: kept `any()`. Asymmetry with legacy paths documented in §3.1 + §8.1 #5 + commit message per §13.1. |
| (c) `log::warn!` instead of propagating `Err` | Parity with `cli/create_agent.rs:137-139`; agent creation is the primary success criterion | Agree | **Unchanged**. |

The Round 1 dev-rust position on (a) was based on a correct cost estimate of "DI in general", but underestimated how cheap Tauri's specific DI is. Lesson noted; updated.

---

## 12. Grinch adversarial review

Added by **dev-rust-grinch** during Step 4. All claims verified against HEAD = `1bc94d8`. Findings numbered; each has Issue / Severity / Reasoning / Proposed mitigation.

### 12.0 Top-line

Tras review adversarial: **2 MEDIUM, 5 NIT, 0 BLOCKER**. Las dos MEDIUM merecen respuesta del architect/dev antes de Step 6 — ambas tienen mitigations baratas y mejoran consistencia/robustez sin engordar scope.

| # | Finding | Severity |
|---|---|---|
| 12.1 | Gate-semantic inconsistency: nuevos paths usan `any()`, legacy + CLI usan per-agent | **MEDIUM** |
| 12.2 | `load_settings()` bypassa el `SettingsState` in-memory canónico; preferir `tauri::State<SettingsState>` (DI Tauri = gratis) | **MEDIUM** |
| 12.3 | `save_settings` no es atómico (truncate + write); race con `load_settings()` paralelo → silent miss | NIT (pre-existente) |
| 12.4 | §8.1 #5 (merge con archivo existente) testea un path estructuralmente inalcanzable para los paths nuevos | NIT |
| 12.5 | Sin tests automatizados que prevengan re-aparición del patrón bug original (otro path de creación de agentes que olvide cablear el helper) | NIT |
| 12.6 | Stray `.claude/` si el helper falla a medias | NIT (cosmético) |
| 12.7 | Snapshot del gate en `create_workgroup`: cambios mid-loop son ignorados (by design, falta doc) | NIT |
| 12.8 | Tech-lead handoff hint: `SETTINGS_LOCK` referenciado no existe en el codebase | Informational |

### 12.1 Gate-semantic inconsistency: new paths use `any()`, legacy + CLI use per-agent

**Issue**: Los dos paths existentes que cablean el helper usan **per-agent gate**:
- `NewAgentModal.tsx:75` → `if (agent.excludeGlobalClaudeMd && createdPath()) { ... }` — flag del coding agent que el usuario picked.
- `cli/create_agent.rs:136` → `if agent.exclude_global_claude_md { ... }` — flag del coding agent identificado por `--launch <id>`.

El plan §3 propone `any()` para los dos paths nuevos. Resultado: dos paths existentes son per-agent; los dos paths nuevos son fleet-wide. Para el mismo setup de usuario, comportamiento divergente:

| Setup | Path | Resultado |
|---|---|---|
| Claude A (flag=true) + Claude B (flag=false). User picks B vía NewAgentModal | legacy | NO se escribe el archivo |
| Mismo setup. User crea matrix vía sidebar (`create_agent_matrix`) | nuevo | SÍ se escribe el archivo |

**Severity**: MEDIUM.

**Reasoning**: Funcionalmente no rompe nada (archivo inerte para Codex/Gemini, idempotente para Claude). Pero conceptualmente es una sorpresa: el usuario que customizó dos entries con flags distintos espera comportamiento consistente entre paths. El test plan (§8) no cubre este escenario. Si recrea un matrix después de tickar/destikar el flag, no entiende por qué a veces sí y a veces no. La asimetría es estructural (matrix/replica se launchan against un coding agent solo at runtime, como bien documenta §3) — pero esa razón estructural no llega al usuario sin documentación.

**Proposed mitigation** (cualquiera o combo):
- (a) Aceptar la divergencia + documentarla en `HintsTab.tsx` (actualizar copy del hint para distinguir "creación de matrix/replica" vs "lanzamiento legacy/CLI") y en commit message.
- (b) Cambiar a `all()` (escribir solo si TODOS los coding agents tienen flag). Más conservador, pero rompe el caso "user solo agregó Claude con flag=true y un placeholder Codex sin tickar".
- (c) Mantener `any()` + agregar a §8 un caso explícito (mixed flags) para que un futuro dev no lo "arregle" pensando que es bug.

**Recomendación grinch**: (a) + (c). Divergencia estructural; documentarla es barato.

### 12.2 `load_settings()` bypassa `SettingsState`; preferir DI Tauri por consistencia

**Issue**: El plan §5 invoca `load_settings()` en cada nueva ubicación. `load_settings()` lee disco cada vez. Pero el app **ya tiene** un `SettingsState = Arc<tokio::sync::RwLock<AppSettings>>` registrado como Tauri state en `lib.rs:255` y usado por toda la superficie de comandos (`config.rs`, `ac_discovery.rs`, `repos.rs`, `session.rs`, `web/`, `phone/`, `telegram/`). El patrón canónico para leer settings desde un comando Tauri es:

```rust
pub async fn create_agent_matrix(
    settings: State<'_, SettingsState>,  // Tauri DI — frontend NO pasa nada
    project_path: String,
    name: String,
    description: String,
) -> Result<CreatedEntityResult, String> {
    let exclude_claude_md = {
        let s = settings.read().await;
        s.agents.iter().any(|a| a.exclude_global_claude_md)
    };
    ...
}
```

La afirmación de §11.13 fila (a), "plumbing tauri::State<SettingsState> would force a parameter on both commands and bubble up to every caller — high cost", **es incorrecta**: Tauri inyecta `State<T>` automáticamente como parámetro del handler; el frontend no pasa nada por `invoke()`. **Cero bubbling, una línea más en la firma**. `create_workgroup` ya recibe `State<'_, Arc<tokio::sync::RwLock<SessionManager>>>` en línea 433 — el patrón ya está. Para `create_agent_matrix` (línea 178) sería el primer `State<>` pero el cambio es trivial.

**Severity**: MEDIUM.

**Reasoning**: Tres beneficios concretos:
1. **Source-of-truth canónico**: `update_settings` (commands/config.rs:32-44) actualiza la in-memory state DESPUÉS de escribir disco. Leer in-memory garantiza consistencia con el resto del app. `load_settings()` lee disco — diverge si el caller corre en paralelo con `update_settings`.
2. **Sin race con write parcial**: ver §12.3. La in-memory state no sufre torn reads.
3. **Sin disk I/O bloqueante en runtime tokio**: `load_settings()` es sync I/O dentro de `async fn`. Microsegundos en práctica, pero el patrón "blocking I/O en async" es justo lo que se evita en Rust async. `settings.read().await` es no-bloqueante.

**Proposed mitigation**: Reescribir §5.2 y §5.3 para usar `State<'_, SettingsState>`. Cero impacto frontend, una línea más en cada firma. Costo: ~10 minutos. §11.7 (root_token side-effect) deja de aplicar — `load_settings()` desaparece de los call-sites nuevos. La sección §11.7 se puede borrar.

Si architect rechaza §12.2 (queda con `load_settings()` por simetría con `cli/create_agent.rs:123` que corre fuera del runtime Tauri), grinch concede — pero pide que se agregue log explícito que distinga "ningún agent tiene el flag" de "load_settings() falló silenciosamente y devolvió defaults". Hoy no se distinguen y eso es exactamente el silent-miss de §12.3.

### 12.3 `save_settings` no es atómico — race con `load_settings()` mid-write

**Issue**: `save_settings` (settings.rs:354) usa `std::fs::write(&path, json)`, que trunca el archivo a 0 bytes y luego escribe. No hay write-to-tmp + rename atómico. Si `load_settings()` lee el archivo entre el truncate y el write completo, lee `""` → `serde_json::from_str("")` falla → retorna `AppSettings::default()` con `agents: vec![]` → `any()` = false → archivo NO escrito.

**Severity**: NIT (pre-existente; no introducido por este fix, pero los 2 nuevos call-sites de `load_settings()` amplifican la superficie).

**Reasoning**: Probabilidad real es baja en disco local rápido (microsegundos), pero NO baja con:
- Antivirus interceptando writes en Windows (Defender, MDE) — agrega ms.
- Drives de red.
- Sistemas con disk pressure / antivirus paranoico.

El silent-miss es **diagnóstico imposible**: usuario tikó flag, creó matrix, no se escribió, no hay error en UI. Reportará como "el flag no funciona" → duplicado de #84. Solo un `log::warn!` en log file lo delata.

**Proposed mitigation**: Out-of-scope para este fix. Crear issue separada para que `save_settings` haga atomic-write (write a `<path>.tmp`, fsync, rename). Si se adopta §12.2, este finding desaparece para los paths nuevos (in-memory state no sufre torn reads).

### 12.4 §8.1 #5 testea un path estructuralmente inalcanzable

**Issue**: §8.1 #5 instruye pre-crear `<project>/.ac-new/_agent_t4/.claude/settings.local.json` antes de invocar `create_agent_matrix`. Pero `create_agent_matrix` falla en línea 190-192 si `agent_dir` ya existe:

```rust
let agent_dir = base.join(format!("_agent_{}", safe_name));
if agent_dir.exists() {
    return Err(format!("Agent '{}' already exists", safe_name));
}
```

Para que `agent_dir/.claude/settings.local.json` exista, `agent_dir` tiene que existir, lo que aborta `create_agent_matrix` antes de llegar al helper. **El path no se ejecuta nunca**. Lo mismo aplica a `create_workgroup` (línea 465-467 chequea `wg_dir.exists()`).

**Severity**: NIT.

**Reasoning**: El test, tal como está escrito, valida la lógica de merge del helper, NO el wiring nuevo. El helper ya está battle-tested por la flow legacy. Es redundante y confuso.

**Proposed mitigation**: Concuerda con §11.10 Option A — demote a §8.3 y reformular como "valida que el helper sigue mergeando vía la flow legacy de NewAgentModal". O directamente borrarlo (la flow legacy no cambia). Grinch prefiere borrarlo.

### 12.5 Sin tests automatizados — el patrón bug original puede re-emerger

**Issue**: §11.9 recomienda dropear los unit tests de §8.2 por costo de infra. Net: cero coverage automatizado del wiring nuevo. La razón por la cual #84 existió es que no había ningún test que enforce el invariante "todo path de creación de agente debe cablear `ensure_claude_md_excludes`". Sin tests, el invariante sigue dependiendo de la atención del próximo dev.

**Severity**: NIT (info / risk-flagging).

**Reasoning**: La inversión de §11.9 (30+ líneas de infra para 30 líneas de fix) es razonable mirando solo el costo inmediato. Pero el costo *real* incluye la próxima vez que este bug aparezca. La base de código tiene 4 paths de creación de agentes (legacy NewAgentModal, CLI `--launch`, `create_agent_matrix`, `create_workgroup`); si alguien agrega un quinto y olvida el helper, otra issue #84.

**Proposed mitigation** (no bloqueante):
- (a) Agregar un comment en `claude_settings.rs` listando los callers — un grep-able paper trail:
  ```rust
  // Callers (must be kept in sync with any new agent-creation flow):
  // - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd, NewAgentModal.tsx)
  // - cli/create_agent.rs (--launch)
  // - commands/entity_creation.rs::create_agent_matrix
  // - commands/entity_creation.rs::create_workgroup (per-replica)
  ```
- (b) Crear issue separada para test integración cuando alguien agregue tempfile a dev-deps.

**Recomendación grinch**: (a). Costo: 5 líneas de comment, sin overhead de testing infra.

### 12.6 Stray `.claude/` si el helper falla a medias

**Issue**: `ensure_claude_md_excludes` (claude_settings.rs:14-16) llama `create_dir_all(&claude_dir)` ANTES de leer/escribir `settings.local.json`. Si el create_dir tiene éxito pero el `std::fs::write` final falla (disk full, antivirus, permisos cambiando), el agent dir queda con un `.claude/` vacío. El caller `log::warn!`s y sigue.

**Severity**: NIT (cosmético).

**Reasoning**: Un `.claude/` vacío es inofensivo. La próxima ejecución del helper lo encuentra y procede normalmente. No corrompe nada. Pero teóricamente la operación NO es transaccional como sugiere §7 ("idempotency and ordering").

**Proposed mitigation**: Ninguna. Documentar como known limitation o ignorar.

### 12.7 Snapshot del gate en `create_workgroup` — cambios mid-loop son ignorados

**Issue**: §5.3 computa `exclude_claude_md` UNA VEZ antes del loop. Si el usuario togglea el flag mientras `create_workgroup` está procesando replicas, el snapshot inicial vence; cambios al toggle son ignorados.

**Severity**: NIT (by design).

**Reasoning**: Snapshot-at-start es la decisión correcta — todas las replicas del mismo workgroup deben tener comportamiento consistente. Half-applied changes son peor que un snapshot. Pero la decisión no está documentada en §5.3.

**Proposed mitigation**: Agregar un line comment en el código del §5.3 explicando "snapshot deliberado: todas las replicas usan el mismo gate". Una línea.

### 12.8 Tech-lead handoff hint: `SETTINGS_LOCK` no existe

**Issue**: El handoff message del tech-lead menciona "load_settings() toma un lock mutable (SETTINGS_LOCK en settings.rs:331)". Verificado: **no existe ningún `SETTINGS_LOCK` en el codebase** (grep recursivo en `src-tauri/`). settings.rs:331 es `if settings.root_token.is_none() {` — la línea de root-token auto-gen, no un lock.

**Severity**: Informational (no afecta el plan).

**Reasoning**: El concern de concurrencia que el tech-lead anticipaba está cubierto por §12.2 y §12.3 desde un ángulo diferente: la sincronización canónica es el `Arc<tokio::sync::RwLock<>>` de `SettingsState` (que `load_settings()` NO usa). Apunto esto solo para que el tech-lead actualice su mental model.

---

### Bottom line (Grinch)

**Veredicto**: Plan necesita una ronda más con architect/dev-rust antes de Step 6. Dos findings MEDIUM (§12.1, §12.2) merecen respuesta:

- **§12.2** es un cambio arquitectural barato (1 param de `State<>` por función) que cierra §12.3 (NIT) en el camino. Si se adopta, `load_settings()` desaparece de los call-sites nuevos y §11.7 ya no aplica.
- **§12.1** es decisión de UX: aceptar divergencia con doc (recomendado) o cambiar a `all()` (más conservador). Grinch prefiere accept-with-doc + test row en §8.

Resto (§12.3-§12.7) son NIT que no bloquean — fold-in donde tenga sentido o ignore con justificación corta.

**Concesiones aceptables**:
- Si architect rechaza §12.2 (queda con `load_settings()`): grinch concede, pero pide log explícito que distinga "no agents have flag" de "load_settings() failed silently".
- Si architect rechaza §12.1 (mantiene `any()` sin doc): grinch concede como NIT, pero el commit message debe mencionar la divergencia.

Si ambas se aceptan o se conceden con justificación clara: **APROBADO** para Step 6. Si una se rechaza sin justificación: 3a ronda.

---

## 13. Round 2 — Architect responses

This section logs the architect's disposition of every finding from §11 (dev-rust) and §12 (grinch). Every change in §1-§10 above flows from a decision recorded here. Severity tags follow grinch's classification.

### 13.1 §12.1 — Gate-semantic asymmetry (MEDIUM): **accepted (partial)**

- **Decision**: keep `any()` for the new paths. Document the asymmetry. Add an explicit test row.
- **What changed in the plan**:
  - §3.1 added — explicit prose explaining the structural reason and naming the legacy/new divergence.
  - §6 trigger table — extended with the mixed-flags row.
  - §8.1 #5 — replaced with a mixed-flag test scenario that exercises both legacy (per-agent) and new (`any()`) paths against the same settings, verifying both observable outcomes.
  - Commit message requirement: dev-rust must mention the divergence in the commit body. Suggested wording: *"Note: the new wiring uses a fleet-wide `any()` gate because matrix/replica creation is decoupled from coding-agent selection. Legacy NewAgentModal and CLI `--launch` paths still use a per-agent gate. The asymmetry is structural — see plan §3.1."*
- **Rejected**: editing `HintsTab.tsx` to reflect the asymmetry. Rationale: the hint is user-facing copy aimed at "what does this option do"; surfacing internal flow asymmetry would confuse more than clarify. The asymmetry is invisible to users who use a single coding agent (the overwhelmingly common case). Documenting it in the plan + commit message + test row gives future devs the trail without burdening end users.

### 13.2 §12.2 — `load_settings()` vs `tauri::State<SettingsState>` (MEDIUM): **accepted in full**

- **Decision**: switch to `State<'_, SettingsState>` + `settings.read().await` for both `create_agent_matrix` and `create_workgroup`. Drop `load_settings()` from the new wiring.
- **Reasoning**: grinch's argument is correct on every point. Round 1 architect (myself) underestimated the simplicity of Tauri's DI — the frontend `invoke()` is unchanged because Tauri auto-injects `State<>` parameters at the handler level. The benefits enumerated in §12.2 (canonical source-of-truth synced by `update_settings`, no torn-read race with `save_settings`, no sync I/O in async runtime) all materialize at zero plumbing cost. This is a strict improvement over Round 1.
- **What changed in the plan**:
  - §3 — clarified that settings are read via `State<'_, SettingsState>`, not `load_settings()`.
  - §4 — file-touch table updated to mention signature changes.
  - §5.1 — imports updated (`SettingsState` instead of `load_settings`).
  - §5.2 — `create_agent_matrix` signature gains a `settings: State<'_, SettingsState>` parameter; gate read uses `settings.read().await`.
  - §5.3 — `create_workgroup` signature gains the same parameter; gate snapshot uses `settings.read().await`. The §12.7 snapshot-deliberate comment is now in the code block.
  - §6 trigger condition restated as `settings.read().await.agents.iter().any(...)`. Edge cases reframed around `SettingsState` initialization.
  - §11.7 marked OBSOLETED.
  - §11.13 row (a) updated.
- **Side effect for §12.3**: closed for the new call-sites — in-memory `SettingsState` has no torn-read window. The underlying `save_settings` non-atomicity remains for any code path still calling `load_settings()` directly, but that's pre-existing and out-of-scope. §9 logs a follow-up issue.

### 13.3 §12.3 — `save_settings` non-atomic (NIT, pre-existing): **acknowledged, out of scope**

- **Decision**: do not address in this fix. Effectively closed for the new call-sites by §13.2. File a follow-up issue for `save_settings` to use atomic write-to-tmp + rename.
- **What changed in the plan**: §9 No-go scope adds the explicit note + follow-up requirement.

### 13.4 §12.4 — §8.1 #5 unreachable test (NIT): **accepted (drop)**

- **Decision**: the original §8.1 #5 (merge with existing file via matrix-creation) is structurally unreachable per `entity_creation.rs:190-192` and `:465-467` early-returns. Dropped.
- **What changed in the plan**: §8 rewritten. Original #5 removed; replaced with the §13.1 mixed-flag asymmetry test. Helper merge logic is now exercised via §8.2 (legacy NewAgentModal regression check), where the dir doesn't pre-exist and the merge path is reachable when the user has pre-set local settings.

### 13.5 §12.5 — No automated tests (NIT): **accepted (mitigation a)**

- **Decision**: adopt grinch's mitigation (a) — add a 6-line caller-list comment to `claude_settings.rs`. Skip integration tests for #84 per §11.9 cost analysis.
- **What changed in the plan**: §4 file-touch row (d) added. §5.4 added with the exact comment block. §8.3 explains why no automated tests are added and what the comment substitutes for.

### 13.6 §12.6 — Stray `.claude/` on partial helper failure (NIT cosmetic): **acknowledged, no action**

- **Decision**: document as known limitation. No code change.
- **What changed in the plan**: §7 gains a final bullet citing §12.6 and explaining the fallback (idempotent re-run finds the empty dir, proceeds normally).

### 13.7 §12.7 — Mid-loop snapshot semantics (NIT by design): **accepted (comment)**

- **Decision**: snapshot-at-start is the right call; document it inline.
- **What changed in the plan**: §5.3's pseudo-code now includes the deliberate-snapshot comment block. The notes paragraph after the block reaffirms it.

### 13.8 §12.8 — `SETTINGS_LOCK` non-existent (Informational): **acknowledged**

- **Decision**: no plan change. The concurrency concern grinch raised in §12.2 / §12.3 captures the real synchronization story. Tech-lead's mental model: the canonical sync is the `Arc<tokio::sync::RwLock<>>` of `SettingsState`, which `load_settings()` does NOT participate in (it reads disk).

### 13.9 Round 1 dev-rust additions (§11.x): all preserved

- §11.1 (line verification): unchanged — line numbers verified pre-edit.
- §11.2-§11.6: unchanged — module paths, log/dirs availability, `Result<(), String>` shape, CLI parity clarification all still apply.
- §11.7: marked OBSOLETED per §13.2.
- §11.8 (no event emit): unchanged.
- §11.9 (no §8.2 unit tests): incorporated into the new §8.3.
- §11.10 (§8 #5 demote/drop): superseded — the original #5 is deleted, replaced with the mixed-flag test row (§13.1).
- §11.11 (verification commands): unchanged — `cargo check` and `cargo clippy` still apply with the new signatures.
- §11.12 (settings.json TOML/JSON nit): unchanged — cosmetic, no impact.
- §11.13: updated per §13.2.

### 13.10 Bottom line (Architect, Round 2)

- 2 MEDIUMs: §12.1 partial-accepted (asymmetry kept, fully documented) + §12.2 fully-accepted (DI adopted).
- 5 NITs: 4 accepted with concrete plan changes (§12.4, §12.5, §12.7 → code/test/comment; §12.3 → out-of-scope follow-up issue). 1 acknowledged without action (§12.6 → known limitation).
- 1 Informational: acknowledged (§12.8).

The plan is now self-contained: dev-rust can implement §5.1-§5.4 1:1, then run §11.11's verification commands, commit on `bug/84-wire-exclude-claude-md-on-agent-creation` with a body that mentions the §13.1 asymmetry, and hand off to grinch for re-review.

I am asking for grinch re-review before Step 6. If grinch finds residual concerns on the new wiring (e.g., RwLock guard scope, snapshot-at-start in `create_workgroup`, the §5.4 caller comment list), let's iterate. Otherwise: green-light to Step 6.

---

## 14. Dev-rust Round 2 ack

Re-read all sections amended by architect in Round 2 (§3, §3.1, §4, §5.1-§5.4, §6, §7, §8, §9, §10, §11.2, §11.5, §11.7 OBSOLETED, §11.13 UPDATED, new §13). Spot-checked claims against current source. Tech-lead's six ack questions are answered below.

### 14.1 New signatures with `State<'_, SettingsState>` — ✅ ack

Verified against the codebase:

- **`SettingsState` is exported correctly**: `src-tauri/src/config/settings.rs:360` defines `pub type SettingsState = Arc<RwLock<AppSettings>>;` where `RwLock` is `tokio::sync::RwLock` (alias on `settings.rs:4`). Module is `pub mod settings;` in `config/mod.rs:7`. The plan's `use crate::config::settings::SettingsState;` compiles 1:1.
- **State is registered at app start**: `src-tauri/src/lib.rs:238` does `let settings: SettingsState = ...`, then `lib.rs:255` does `.manage(settings)`. By the time `create_agent_matrix` or `create_workgroup` run as `#[tauri::command]` handlers, the state is guaranteed populated. Plan's claim in §13.2 is correct.
- **Position convention**: surveyed `commands/session.rs:616-619` (4 `State<>`s, settings last), `commands/session.rs:820-823` (same pattern), `commands/telegram.rs:15-17` (settings last). The convention is: `AppHandle` first, then all `State<>` params (settings typically last among them), then regular IPC params. The plan's chosen positions match:
  - `create_agent_matrix(settings: State<'_, SettingsState>, project_path, name, description)` — no `AppHandle`, no other `State<>`, so `settings` first is correct.
  - `create_workgroup(app, session_mgr, settings: State<'_, SettingsState>, project_path, team_name)` — `settings` after `session_mgr` and before regular params. Consistent with the convention; the alternative (placing `settings` after `team_name`) would violate "all `State<>`s before regular params". ✓
- **Frontend `invoke()` truly unchanged**: verified `src/shared/ipc.ts:311-312` — `transport.invoke<void>("create_agent_matrix", { projectPath, name, description })` — and `:347-352` — `transport.invoke<void>("create_workgroup", { ... })` — neither passes any state-shaped key. Tauri's `#[tauri::command]` macro injects `State<T>` parameters from the app's managed state, NOT from the JSON payload, so the frontend is invariant. Architect's claim in §13.2 is correct.

### 14.2 Read pattern with narrow guard scope — ✅ ack

Verified:

- **`SettingsState` IS `RwLock` (not `Mutex`)**: `pub type SettingsState = Arc<RwLock<AppSettings>>` with `tokio::sync::RwLock`. Method is `read().await` returning `RwLockReadGuard`. Plan's pattern is correct.
- **Helper is sync**: `pub fn ensure_claude_md_excludes(dir: &Path) -> Result<(), String>` (no `async`). The narrow guard scope `{ let s = settings.read().await; ... }` releases the lock at the closing brace, **before** the sync helper is called. This is the right shape:
  - We don't hold the read lock across an await point (none after the read inside the helper).
  - We don't hold the read lock across the (sync) helper, which would only matter if a writer (e.g., `update_settings`) was waiting — even then, an immediate release is best practice.
- **Canonical reference**: `commands/config.rs:24-29` uses the identical pattern for `get_settings` (`let s = settings.read().await; ... result.clone()`). The plan's read shape is consistent with the rest of the codebase.

No deadlock or hold-across-await risk in the proposed code.

### 14.3 §5.4 caller comment in `claude_settings.rs` — ✅ ack (useful, not redundant)

The 6-line caller list is worth the 6 lines:
- `claude_settings.rs` is currently 62 lines and tightly focused; the comment doesn't bury anything.
- It's grep-able (`rg "ensure_claude_md_excludes" src-tauri/src` already finds the callers, but the comment puts them in the dev's face when they open the helper file — different kind of discoverability).
- The original bug (#84) is exactly the failure mode this comment guards against: a new agent-creation flow added without grepping for the helper. A 6-line list isn't a hard guarantee, but it's the cheapest possible nudge.
- Maintenance burden is minimal: the list grows by one line per new flow; outdated entries become visible on the next read (the file is small).

Not redundant. Approved.

### 14.4 §11.7 OBSOLETED — ✅ ack

Confirmed obsolete. With `State<'_, SettingsState>`, the new wiring no longer calls `load_settings()`, so:
- The `root_token` auto-gen side effect at `settings.rs:331-337` is not triggered by our new code paths.
- The startup `load_settings()` call in `lib.rs:238` still triggers it once at app launch (unchanged from before this fix).
- §11.7's §11.13 row (a) lesson is preserved in §11.13 UPDATED.

The OBSOLETED marker + struck-through original text in §11.7 is the right way to preserve the trail without misleading future readers. No action.

### 14.5 §11.13 UPDATED — ✅ ack

Agree with the row (a) reversal. The original Round 1 dev-rust position ("DI plumbing is intrusive") was correct *in general* but wrong *for Tauri's specific DI model*. Tauri auto-injects `State<T>` at the handler layer; the frontend `invoke()` payload is unchanged, and there's no bubbling cost up the call chain because handlers are leaf nodes. The §11.13 table now reflects this clearly with the "Round 2 outcome" column.

I'd note for future Round 1 reviews: when assessing DI cost in this codebase, the right comparison is "does this handler already take any `State<>`?" If yes (as `create_workgroup` did), adding one more is trivial. If no (as `create_agent_matrix` didn't), the cost is one parameter line. Both cases are well below the threshold of "intrusive". Mental model corrected.

### 14.6 §13.3 atomic `save_settings` as follow-up — ✅ ack (deferral acceptable, not blocking)

Acceptable to defer for #84:
- The new call-sites are now closed for the torn-read race because they read in-memory via `State<>`, not from disk via `load_settings()`.
- The pre-existing `save_settings` non-atomicity remains a real (low-probability) bug for any code path still calling `load_settings()` directly. Notable existing call-sites: `cli/create_agent.rs:123`, `commands/config.rs:48` (in `open_web_remote`), `lib.rs:238` (startup). None of these are introduced or amplified by this fix.
- The fix shape (write-to-tmp + fsync + rename) is small but touches a function used everywhere — proper scope is its own issue with its own review pass.

Recommend the follow-up issue title: *"`save_settings` should be atomic (write-to-tmp + rename)"* with a one-line repro pointer to §12.3. Dev-rust will not file the issue from this branch (it's tech-lead's call when to open it).

### 14.7 No new issues found in the Round 2 amendments

- Imports (§5.1): correct, compile 1:1.
- Function bodies (§5.2 / §5.3): correct, including the snapshot-deliberate comment in §5.3.
- §5.4 comment block: correct file (`claude_settings.rs`), correct position (top of file, above `use std::path::Path;`).
- §6 trigger condition restated correctly with `settings.read().await`.
- §8 test plan: rewritten coherently. Mixed-flag test row in §8.1 #5 is the right replacement for the dropped unreachable-path test.
- §9 follow-up note for `save_settings` atomicity: present.
- §10 size/risk estimate updated to reflect the additional signature changes and §5.4 comment block.
- §11.x sections: all preserved, OBSOLETED/UPDATED markers applied correctly.
- §13 Round 2 disposition log: complete, every grinch finding has a recorded decision.

### 14.8 Bottom line (Dev-rust, Round 2)

**APROBADO as-amended-by-architect.** No ajustes necesarios antes del re-review de grinch. El plan es implementable 1:1 desde §5.1-§5.4, con verificación vía §11.11. Si grinch aprueba sin nuevos findings, listo para Step 6 (implementación) en mi próximo turno.

---

## 15. Grinch Round 2 re-review

Re-review por **dev-rust-grinch** post-§13 (architect Round 2) y §14 (dev-rust ack). Verificado contra HEAD = `1bc94d8` (sin commits aún en la branch).

### 15.0 Disposition de findings de Round 1

| # | Finding | Severity (R1) | Disposition (R2) | Veredicto grinch |
|---|---|---|---|---|
| 12.1 | Gate-semantic asymmetry (any vs per-agent) | MEDIUM | Partial-accept: kept `any()`, doc en §3.1 + §6 trigger row + §8.1 #5 mixed-flag test + commit-msg requirement; rechazado update de `HintsTab.tsx` | ✅ **Resuelto** |
| 12.2 | `load_settings()` vs `State<SettingsState>` | MEDIUM | Full-accept: ambas firmas reciben `State<'_, SettingsState>`, narrow guard scope, snapshot-once en workgroup; §11.7 OBSOLETED | ✅ **Resuelto** |
| 12.3 | `save_settings` non-atomic | NIT | Out-of-scope (cerrado para call-sites nuevos vía §13.2; follow-up issue para call-sites legacy) | ✅ **Resuelto** |
| 12.4 | §8.1 #5 unreachable | NIT | Dropped, reemplazado por §8.1 #5 mixed-flag test (que sí ejercita los nuevos paths) | ✅ **Resuelto** |
| 12.5 | Sin tests automatizados | NIT | Mitigation (a) adoptada — caller-list comment de 6 líneas en `claude_settings.rs` (§5.4) | ✅ **Resuelto** |
| 12.6 | Stray `.claude/` en falla parcial | NIT | Documentado como known limitation en §7 | ✅ **Resuelto** |
| 12.7 | Snapshot mid-loop sin doc | NIT | Comment inline agregado en §5.3 | ✅ **Resuelto** |
| 12.8 | `SETTINGS_LOCK` no existe | Informational | Ack | ✅ **Resuelto** |

**Concesión §12.1 acceptada**: el rechazo de tocar `HintsTab.tsx` está bien razonado — la copy del hint apunta al modelo mental del usuario que tiene UN coding agent (caso normal); la asimetría es invisible para ellos. El usuario que configura múltiples Claude entries con flags distintos es advanced setup, y para ese caso el commit message + §3.1 + §8.1 #5 test row dejan trail suficiente. No insisto.

### 15.1 Verificación del nuevo wiring (post-§13.2)

#### 15.1.1 RwLock guard scope — sin hold-across-await ✅

Patrón propuesto en §5.2 / §5.3:

```rust
let exclude_claude_md = {
    let s = settings.read().await;
    s.agents.iter().any(|a| a.exclude_global_claude_md)
};
```

- Único `.await` está en `settings.read().await` mismo (acquisition). Al completar, el guard `s` está bound y NO hay más awaits hasta el cierre del block.
- `s.agents.iter().any(...)` es 100% sync (Vec iter + closure que evalúa un bool field).
- El block expression returnea `bool` (Copy); el guard `s` se dropea en el `};`. La `bool` capturada vive afuera del lock.
- Helper subsiguiente (`ensure_claude_md_excludes`) es `pub fn` (sync, ver `claude_settings.rs:9`) — no introduce awaits ni mientras el lock está held ni después.

Sin hold-across-await. Sin riesgo de deadlock con `update_settings` (único writer; ver §15.1.4).

#### 15.1.2 Snapshot semantics en `create_workgroup` ✅

§5.3 computa `exclude_claude_md` UNA VEZ antes del loop. Dentro del loop, el `if exclude_claude_md` referencia la `bool` capturada — cero re-acquisition del lock por iteración. Coherente con el §5.3 inline comment ("snapshot gate ONCE before the loop. Deliberate: all replicas... must use the same gate value").

Verificado además que `team_agents` es local al `create_workgroup` (parsed de `team_config` líneas 480-484) y no muta entre el snapshot y el loop. La `bool` snapshot es la única información settings-derived usada dentro del loop. ✓

#### 15.1.3 Caller comment §5.4 — cubre los 4 sitios Rust correctos ✅ (con NIT)

Verificado vía `rg "ensure_claude_md_excludes" src-tauri/src` (tres matches actuales: la def + dos callers existentes). Post-fix los callers serán exactamente:

1. `commands/agent_creator.rs:59` (`write_claude_settings_local` Tauri cmd) — ✓ listado
2. `cli/create_agent.rs:137` (`--launch`) — ✓ listado
3. `commands/entity_creation.rs::create_agent_matrix` — ✓ listado (se agrega en este fix)
4. `commands/entity_creation.rs::create_workgroup` (per-replica) — ✓ listado (se agrega en este fix)

**Lista correcta y completa**. Ver §15.2 (NIT) para una observación cosmética sobre la anotación parentética.

#### 15.1.4 Edge cases sobre la firma cambiada ✅

- **Concurrencia con `update_settings`**: único writer es `commands/config.rs::update_settings:32-44`. Adquiere `write().await` brevemente (un assign). El nuevo wiring adquiere `read().await` brevemente (iter + any). Tokio `RwLock` es fair y no-bloqueante (futures yield); no hay deadlock posible — `update_settings` no llama a otras Tauri commands desde dentro del write lock (no reentrancy).
- **Inicialización del state**: `lib.rs:238-239` hace `Arc::new(tokio::sync::RwLock::new(load_settings()))` ANTES de `tauri::Builder::default().manage(settings)` (línea 255). Por construcción, ningún `#[tauri::command]` puede correr antes de que el state esté managed. La primera llamada a `create_agent_matrix` o `create_workgroup` siempre encuentra `SettingsState` populated. ✓
- **Frontend invariante**: `ipc.ts:311-312` (`createAgentMatrix`) y `ipc.ts:347-352` (`createWorkgroup`) NO pasan ningún campo `settings` en el payload JSON. Tauri auto-inyecta `State<T>` desde managed state, no desde el payload. Frontend invariante confirmado. ✓
- **Position convention**: `create_agent_matrix(settings, project_path, name, description)` y `create_workgroup(app, session_mgr, settings, project_path, team_name)` matchean la convención del codebase (e.g. `commands/session.rs::create_session:614-619`). ✓
- **Handler registration**: `lib.rs:683` y `:690` listan `commands::entity_creation::create_agent_matrix` y `create_workgroup` por nombre. La macro `#[tauri::command]` auto-detecta `State<T>` params; la entrada en `generate_handler![]` no cambia. ✓

Sin edge cases nuevos.

### 15.2 Findings residuales

#### 15.2.1 NIT (cosmético, no bloqueante): anotación incompleta en el caller comment §5.4

**Issue**: El §5.4 caller comment escribe:

```rust
//   - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd, NewAgentModal.tsx)
```

La anotación parentética solo menciona `NewAgentModal.tsx`. Pero `write_claude_settings_local` también lo invoca `SessionItem.tsx:212` (acción de context menu "Exclude Claude.md", manual + sin per-agent gate — lo hace solo si el usuario clickea explícitamente). Verificado vía `rg "writeClaudeSettingsLocal" src` → 2 invocations frontend más la def en `ipc.ts:371`.

**Severity**: NIT (cosmético).

**Reasoning**: La lista de callers Rust es correcta (4 entries, los 4 callers post-fix). La anotación es informativa, no contractual — no afecta correctness. Pero un futuro dev que lee el comment y quiere mapear cada Rust caller a su origen frontend puede asumir que NewAgentModal es el único trigger. Para el propósito anti-regresión (detectar nuevos paths de creación que olviden el helper), es irrelevante: el dev futuro busca "qué funciones Rust llaman a `ensure_claude_md_excludes`", no "qué caminos de UI lo disparan".

**Proposed mitigation**: Cambiar la línea a:

```rust
//   - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd; frontend: NewAgentModal.tsx + SessionItem.tsx ctx-menu)
```

Una palabra más, completitud del paper trail. Opcional — si dev-rust prefiere no tocar el wording, lo deja como está y queda como recordatorio para cuando alguien edite §5.4 en el futuro.

### 15.3 Bottom line (Grinch Round 2)

**APROBADO** para Step 6.

- 2 MEDIUM de Round 1 → ambos resueltos (§12.1 partial con concesión razonada, §12.2 full-adopt).
- 5 NIT de Round 1 → todos cerrados (4 con cambios concretos, 1 documentado como known limitation).
- 1 Informational de Round 1 → ack.
- 1 NIT nuevo (§15.2.1) → cosmético, no bloqueante; dev-rust puede ignorar o aplicar el wording sugerido al implementar §5.4.

El nuevo wiring con `State<'_, SettingsState>` está correctamente shapeado: guard scope estrecho, sin hold-across-await, snapshot consistente en `create_workgroup`, frontend invariante, registration unchanged, position matches convention. No encontré formas de romperlo.

Dev-rust puede proceder a Step 6 (implementación) cuando tech-lead lo habilite. Después de la implementación, vuelve a grinch para Step 7 (review del diff).
