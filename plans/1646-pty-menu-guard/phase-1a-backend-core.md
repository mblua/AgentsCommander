# Phase 1A: Backend core, settings schema, detection engine, and IPC resolve command

Objective: Implement settings schema with defaults materialization and tolerant parsing, session communication domain updates, PTY menu guard proactive detection subsystem with frame scanning, `resolve_blocking_menu` IPC command, production CLI initializer updates, and workspace-wide mechanical struct-initializer sweep.
Class: patterned
Owner: ac-dev-rust-v4

## 1. Exact Files and Symbols Modified (Design Scope: 8 files)

1. `src-tauri/src/config/settings.rs`:
   - `pub struct AppSettings`: add field `#[serde(default = "default_true")] pub menu_guard_enabled: bool`.
   - `pub struct AgentConfig`: add field `#[serde(default, skip_serializing_if = "Option::is_none")] pub blocking_menus: Option<Vec<BlockingMenuEntry>>`.
   - Define `BlockingMenuEntry` enum: untagged `Valid(BlockingMenuConfig)` / `Invalid(serde_json::Value)` with helper method `pub fn valid(&self) -> Option<&BlockingMenuConfig>`.
   - Define `BlockingMenuConfig` struct: `pub pattern: String`, `pub notification: String`, `#[serde(default = "default_true")] pub enabled: bool`, `#[serde(default, skip_serializing_if = "Option::is_none")] pub captured_against: Option<String>`.
   - Function `pub fn default_blocking_menus_for_command(command: &str) -> Vec<BlockingMenuEntry>`:
     - If command stem is `"pi"`: single `Valid` entry with pattern `r"^\s*Trust project folder\?"`, notification `"pi is waiting for you to answer the folder-trust menu in this terminal"`, enabled `true`, capturedAgainst `"pi 0.52 / Windows"`.
     - If command stem is `"codex"`: single `Valid` entry with pattern `r"^\s*Do you trust the contents of this directory\?"`, notification `"codex is waiting for you to answer the folder-trust menu in this terminal"`, enabled `true`, capturedAgainst `"codex 0.x / Linux"`.
     - All other commands (e.g. claude, agent/Cursor, antigravity): return `vec![]`.
   - Function `pub fn materialize_blocking_menus(agents: &mut [AgentConfig]) -> bool`: populates `agent.blocking_menus` with `Some(default_blocking_menus_for_command(&agent.command))` when `agent.blocking_menus.is_none()`. Returns `true` if any agent was updated.
   - In `load_settings_from_path`: invoke `materialize_blocking_menus(&mut settings.agents)`; if `true`, set `needs_save = true`.
   - In `AppSettings::default()`: initialize `menu_guard_enabled: true`.

2. `src-tauri/src/session/session.rs`:
   - `SessionCommunicationKind`: add variant `BlockedMenu`.
   - `SessionCommunication`: add field `#[serde(default, skip_serializing_if = "Option::is_none")] pub message: Option<String>`.

3. `src-tauri/src/session/manager.rs`:
   - Add `pub async fn set_blocked_menu(&self, id: Uuid, message: String, updated_at: chrono::DateTime<chrono::Utc>) -> Option<(bool, SessionCommunication)>`:
     Sets `session.communication` to `Some(SessionCommunication { kind: SessionCommunicationKind::BlockedMenu, visible: true, updated_at: updated_at.to_rfc3339(), message: Some(message) })`. Returns `(changed, communication)`.
   - Add `pub async fn clear_blocked_menu(&self, id: Uuid) -> bool`:
     Clears `session.communication` if currently `Some` with `kind == SessionCommunicationKind::BlockedMenu` and `visible == true`. Returns `true` if cleared.
   - In `set_communication` (~line 925): initialize `message: None` in the `SessionCommunication` struct literal.

4. `src-tauri/src/pty/menu_guard/mod.rs` (New Module):
   - `pub const ERR_MENU_GUARD_DEFERRED: &str = "menu_guard_deferred";`
   - `pub fn is_menu_guard_deferred_error(e: &str) -> bool { e.starts_with(ERR_MENU_GUARD_DEFERRED) }`
   - Declare `pub struct MenuGuard` holding:
     - `sessions: std::sync::Mutex<std::collections::HashMap<Uuid, MenuGuardSessionState>>`
     - `compiled_patterns: std::sync::Mutex<std::collections::HashMap<String, Result<regex::Regex, String>>>`
     - `next_episode_id: std::sync::atomic::AtomicU64`
   - State struct `MenuGuardSessionState`:
     - `episode_id: u64`
     - `suppressed_episode_id: Option<u64>`
     - `matched_pattern: Option<String>`
     - `matched_notification: Option<String>`
     - `is_blocked: bool`
     - `last_seen_stamp: Option<crate::pty::watchers::FrameStamp>`
   - `pub fn evaluate_logical_rows(&self, session_id: Uuid, logical_rows: &[crate::pty::watchers::frame::LogicalRow], entries: &[BlockingMenuEntry]) -> MenuGuardEvaluation`:
     - Finds first enabled `Valid(config)` whose compiled regex matches any logical row.
     - If regex fails to compile, logs error once and skips entry.
     - On match: if `suppressed_episode_id == Some(current_episode_id)` -> `is_blocked = false`, `should_notify = false`. Else -> `is_blocked = true`, `should_notify = true`, sets `matched_notification`. If starting new match from clean state, increments `episode_id`.
     - On no match: if previously matching, resets `suppressed_episode_id = None`, `matched_pattern = None`, `matched_notification = None`, `is_blocked = false`, `should_clear_notification = true`.
   - `pub fn resolve_current_episode(&self, session_id: Uuid) -> bool`: sets `suppressed_episode_id = Some(episode_id)` and `is_blocked = false`.
   - `pub fn is_blocked(&self, session_id: Uuid) -> bool`: returns `state.is_blocked`.
   - `pub async fn scan_tick<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>)`:
     - If `!settings.menu_guard_enabled`, unblocks all sessions and clears communication.
     - For each live session, queries `screen_rows_since(session_id, last_seen_stamp)`.
     - If `Frame(frame)`, runs `logical_rows(&frame)` and `evaluate_logical_rows`.
     - Synchronizes state changes with `SessionManager` and emits `session_communication_changed` via `crate::session::selection::publish_session_communication`.
   - `pub fn start(self: &Arc<Self>, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal)`: runs scan tick every 250 ms.

5. `src-tauri/src/pty/mod.rs`:
   - Add `pub mod menu_guard;`.

6. `src-tauri/src/commands/session.rs`:
   - Add `#[tauri::command] pub async fn resolve_blocking_menu(app: AppHandle, session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>, menu_guard: State<'_, Arc<crate::pty::menu_guard::MenuGuard>>, id: String) -> Result<(), String>`:
     Parses UUID, invokes `menu_guard.resolve_current_episode(uuid)`, calls `session_mgr.clear_blocked_menu(uuid)`, and emits `publish_session_communication(&app, uuid, None)`.

7. `src-tauri/src/lib.rs`:
   - Register `Arc<MenuGuard>` in `app.manage(...)`.
   - Spawn `menu_guard.start(...)` with app shutdown token.
   - Add `resolve_blocking_menu` to `generate_handler![...]`.

8. `src-tauri/src/cli/coding_agent.rs` (Production CLI):
   - In `fn blank_agent() -> AgentConfig` (~line 519): add `blocking_menus: None`.
   - In `fn definition_to_agent_seed(def: &CodingAgentDefinition) -> AgentConfig` (~line 537): add `blocking_menus: None`.

## 2. Mechanical Initializer Sweep (Compiler-Forced Fallout & Mechanical Exception)

Adding `pub blocking_menus: Option<Vec<BlockingMenuEntry>>` to `AgentConfig` and `pub message: Option<String>` to `SessionCommunication` triggers `E0063: missing field in initializer` at every struct-literal site in the workspace not listing the new field.

### Pinned Mechanism & Precedent
- Mechanism: Literal `None` at each site (`blocking_menus: None` and `message: None`), exactly matching the precedent documented in `src-tauri/src/config/settings.rs:575-577` ("the 20-plus struct-construction sites that already had to write `context_regex: None`").
- Anti-overengineering constraint: Do NOT introduce `Default` implementations or struct-update refactors on existing structs (no new design).

### Partition Rule Mechanical Exception
Under the plan-partitioning rule, these initializer additions represent a single mechanical rule applied across test and helper sites with zero design freedom, validated by the single phase command `cargo test --manifest-path src-tauri/Cargo.toml`. The design-bearing scope remains the 8 files in Section 1.

### Cross-Phase Hand-Off & Ownership
- `src-tauri/src/phone/mailbox.rs`: Phase 1A touches `mailbox.rs` ONLY at the 3 listed `cfg(test)` initializer lines (~12904, ~15492, ~23749). ALL production changes to `mailbox.rs` (settle checks, outbox retry preservation, error conversions) remain strictly in Phase 1B (`phase-1b-queue-injection.md`). Both Phase 1A and Phase 1B share the same single owner (`ac-dev-rust-v4`).
- Test-only initializer sites in `web/commands.rs`, `commands/config.rs`, `agent_update.rs`, `config/agent_command.rs`, `config/coding_agent_profiles.rs`, `config/coding_agent_mutations.rs`, `config/sessions_persistence.rs`, `cli/create_agent.rs`, `cli/self_switch.rs`, and `tests/pty_powershell_managed_native.rs` belong to NO OTHER phase in the epic.

### Complete Enumeration of Struct-Literal Sites

#### A. `AgentConfig` Literal Sites (add `blocking_menus: None`)
1. `src-tauri/src/cli/coding_agent.rs` (~line 519): `fn blank_agent() -> AgentConfig` (production)
2. `src-tauri/src/cli/coding_agent.rs` (~line 537): `fn definition_to_agent_seed(def: &CodingAgentDefinition) -> AgentConfig` (production)
3. `src-tauri/src/config/settings.rs` (~line 5969): `settings_with_agents` in `mod tests`
4. `src-tauri/src/config/settings.rs` (~line 6049): `config_seed_serde_round_trips_camel_case_and_omits_when_absent` in `mod tests`
5. `src-tauri/src/config/agent_command.rs` (~line 1166): `fn agent(id: &str, command: &str) -> AgentConfig` in `mod tests`
6. `src-tauri/src/config/coding_agent_profiles.rs` (~line 796): `settings_with_project` in `mod tests`
7. `src-tauri/src/config/coding_agent_mutations.rs` (~line 575): `fn agent(id: &str, label: &str, command: &str) -> AgentConfig` in `mod tests`
8. `src-tauri/src/cli/create_agent.rs` (~line 273): `fn agent(id: &str, label: &str, command: &str) -> AgentConfig` in `mod tests`
9. `src-tauri/src/cli/self_switch.rs` (~line 494): `fn agent(id: &str, label: &str, command: &str) -> crate::config::settings::AgentConfig` in `mod tests`
10. `src-tauri/src/commands/config.rs` (~line 2906): `settings_with_single_agent` in `mod tests`
11. `src-tauri/src/commands/session.rs` (~line 4956): `test_settings` (first agent) in `mod tests`
12. `src-tauri/src/commands/session.rs` (~line 4968): `test_settings` (second agent) in `mod tests`
13. `src-tauri/src/commands/session.rs` (~line 5032): `configured_pi_materialization_wires_auto_self_clear_to_agents_md` in `mod tests`
14. `src-tauri/src/commands/session.rs` (~line 5087): `inert_pi_spawn` in `mod tests`
15. `src-tauri/src/commands/session.rs` (~line 9014): `heuristic_agent_metadata_cannot_authorize_pi_mutation` in `mod tests`
16. `src-tauri/src/commands/session.rs` (~line 9303): `resolve_actual_agent_keeps_requested_agent_when_normalized_command_matches` in `mod tests`
17. `src-tauri/src/commands/session.rs` (~line 9382): `resolve_agent_from_shell_skips_invalid_configured_command` in `mod tests`
18. `src-tauri/src/agent_update.rs` (~line 3353): `run_startup_updates_emits_started_with_nodes_then_command_events_then_finished_then_post_probe` in `mod tests`
19. `src-tauri/src/web/commands.rs` (~line 1342): `fn test_agent(id: &str) -> crate::config::settings::AgentConfig` in `mod tests`
20. `src-tauri/src/lib.rs` (~line 4416): `settings_with_agent` in `mod tests`
21. `src-tauri/src/phone/mailbox.rs` (~line 12904): `fn wake_agent(id: &str, label: &str, command: &str) -> AgentConfig` in `mod tests`
22. `src-tauri/tests/pty_powershell_managed_native.rs` (~line 373): `fn agent_config(id: &str, command: &str) -> agentscommander_lib::config::settings::AgentConfig` in integration tests

#### B. `SessionCommunication` Literal Sites (add `message: None` or `message: Some(...)`)
1. `src-tauri/src/session/session.rs` (~line 497): `session_communication_round_trips` in `mod tests` (`message: None`)
2. `src-tauri/src/session/session.rs` (~line 510): `session_communication_round_trips` in `mod tests` (`message: None`)
3. `src-tauri/src/session/manager.rs` (~line 925): `set_communication` in `SessionManager` (production, `message: None`)
4. `src-tauri/src/session/manager.rs` (~line 959): `set_blocked_menu` in `SessionManager` (production, `message: Some(message)`)
5. `src-tauri/src/session/manager.rs` (~line 3000): `restore_communication_accepts_dormant_coordinator_and_preserves_original_raise_time` in `mod tests` (`message: None`)
6. `src-tauri/src/session/manager.rs` (~line 3028): `restore_communication_rejects_non_coordinator_hidden_payload_and_unknown_id` in `mod tests` (`message: None`)
7. `src-tauri/src/commands/session.rs` (~line 6548): `restart_session_retains_communication` in `mod tests` (`message: None`)
8. `src-tauri/src/commands/session.rs` (~line 9465): `visible_hand` closure in `carry_communication_for_restart_matrix` in `mod tests` (`message: None`)
9. `src-tauri/src/commands/session.rs` (~line 9472): `hidden_hand` closure in `carry_communication_for_restart_matrix` in `mod tests` (`message: None`)
10. `src-tauri/src/config/sessions_persistence.rs` (~line 2401): `failed_recoverable_sanitization_clears_stale_runtime_fields` in `mod tests` (`message: None`)
11. `src-tauri/src/config/sessions_persistence.rs` (~line 3866): `purge_unretained_sessions_respects_session_save_lock` in `mod tests` (`message: None`)
12. `src-tauri/src/config/sessions_persistence.rs` (~line 4477): `communication_round_trips_when_present` in `mod tests` (`message: None`)
13. `src-tauri/src/phone/mailbox.rs` (~line 15492): `test_record_message_outcome_clears_exited_coordinator_communication` in `mod tests` (`message: None`)
14. `src-tauri/src/phone/mailbox.rs` (~line 23749): `dormant_session_wake_failure_restores_pending_coordinator_hand` in `mod tests` (`message: None`)

## 3. Inlined Decisions and Behavior

- **Tolerant Parsing**: Malformed entries in `settings.json` deserialize to `BlockingMenuEntry::Invalid(Value)` and are skipped during evaluation without causing overall deserialization failure or dropping user bytes on save.
- **Default Materialization**: Absent `blockingMenus` on an `AgentConfig` is populated with evidence-backed defaults on load and saved back to `settings.json`. An explicit empty array `[]` (`Some(vec![])`) is preserved and disables the guard for that agent.
- **Master Kill Switch**: `menu_guard_enabled: false` immediately unblocks all sessions and skips all matching.
- **Episode Re-arming**: Resolving an episode suppresses only that specific `episode_id`. When the menu disappears and reappears, a new episode is created and the block re-arms.
- **Canonical Prefix and Helper Location**: `ERR_MENU_GUARD_DEFERRED = "menu_guard_deferred"` and predicate `is_menu_guard_deferred_error(e)` reside in `pty::menu_guard` as the authoritative source of truth.

## 4. Required Tests and Verification

1. Unit tests in `src-tauri/src/config/settings.rs`:
   - `test_blocking_menus_defaults_materialization`: verifies `pi` and `codex` receive correct default patterns when absent, while `claude` receives `[]`.
   - `test_blocking_menus_tolerant_parsing`: verifies invalid JSON types (e.g. integer or invalid object) deserialize to `Invalid` and round-trip without corruption.
   - `test_blocking_menus_explicit_empty_array`: verifies `[]` deserializes as `Some(vec![])` and is not overwritten by defaults.
   - `test_menu_guard_master_switch`: verifies `menuGuardEnabled` defaults to `true` and serializes/deserializes correctly.
2. Unit tests in `src-tauri/src/pty/menu_guard/mod.rs`:
   - `test_menu_guard_pattern_matching`: tests logical rows matching pi and codex patterns.
   - `test_menu_guard_episode_suppression_and_rearm`: tests manual resolution suppressing the active episode, clearing on menu disappearance, and re-arming on menu reappearance.
   - `test_menu_guard_invalid_regex_tolerance`: tests invalid regex pattern logging once and skipping without panic.
3. Unit tests in `src-tauri/src/commands/session.rs`:
   - `test_resolve_blocking_menu_command`: verifies `resolve_blocking_menu` resolves episode, clears session manager communication, and emits `publish_session_communication`.

Verification command:
```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 5. Objective Acceptance Criteria

1. Running `cargo test --manifest-path src-tauri/Cargo.toml` completes with 0 errors and all new/existing tests and mechanical initializer sites pass.
2. An absent `blockingMenus` field in `settings.json` materializes evidence-backed defaults for `pi` and `codex`.
3. When a session matches a blocking menu pattern, `MenuGuard` marks the session blocked and emits `session_communication_changed`.
4. Invoking `resolve_blocking_menu` unblocks the session for the current episode; a menu disappearance and reappearance re-blocks the session.

## 6. Preserve List

- Preserve `WatcherEntry` and user watcher engine capability boundary (`pty/watchers/mod.rs:8-12`).
- Preserve existing `RaiseHand` behavior and lifecycle in `SessionCommunication`.
- Preserve unknown keys in `settings.json` via the #1077 write gate.
- Preserve all production code in `src-tauri/src/phone/mailbox.rs` for Phase 1B.

