# Epic Plan #1646: PTY menu guard (detect blocking coding-agent menus, freeze programmatic injection, notify the user)

Author: ac-architect-v4, room-3, 2026-08-30 UTC. Full `code-implementation-workflow` path, Round 6 candidate.
Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1646](https://github.com/mblua/AgentsCommander/issues/1646) — "PTY menu guard: detect blocking coding-agent menus, freeze programmatic injection, notify the user"
Delivery path: Full.
PARTITION: 3 phases (cut by Owner & Contract: Backend Core/Settings -> Queue/Injection Integration -> Frontend UI/Notifications).

## 1. Objective and Problem Statement

When AgentsCommander wakes a replica or delivers an inter-agent message, the target coding agent (e.g. `pi` on Windows or `codex` on Linux/container) may be parked on a blocking interactive menu (such as a folder-trust prompt) rather than its normal prompt.
Because `settle_until_ready` in `src-tauri/src/phone/mailbox.rs` previously only checked `waiting_for_input && has_rendered_visible_content`, a session parked on a trust menu satisfies both conditions. Furthermore, the 90-second settle max_wait cap injects anyway ("never drop a delivery"). When programmatic text and staggered carriage returns (`\r` at +1500 ms and +2000 ms) are injected via `src-tauri/src/pty/inject.rs`, the text is discarded and the first Enter silently selects the highlighted menu option (e.g. silently trusting an untrusted directory). Meanwhile, outbox deliveries and DB queue dispatches burn retry attempts (`MAX_DELIVERY_ATTEMPTS = 10` in outbox, `max_attempts = 5` in DB queue) until rejection / poisoning.

Objective:
1. Detect blocking interactive menus proactively via periodic scans against the de-ANSI'd logical screen rows from the `vt100` screen mirror.
2. Store user-editable per-agent regex patterns in `settings.json` under `agents[].blockingMenus`, with tolerant per-entry parsing and defaults materialized into `settings.json` when absent.
3. On menu match: freeze all programmatic PTY injection at both backend chokepoints (`inject_text_into_session_impl` and exact input pre-flight), hold outbox and DB deliveries without consuming retry attempts or poisoning, hold live settle loops, and notify the user via a Sidebar indicator across all replica rows and a notification with a "Resolved by user" action.
4. Release the hold automatically when the pattern no longer matches, or manually via "Resolved by user" for the current match episode. Direct user keystrokes from xterm.js (`pty_write`) remain unblocked at all times.

## 2. Pinned Base and Delivery Authority

- Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-3-ac-dev-team-v4\repo-AgentsCommander`
- Target branch: `feature/1646-pty-menu-guard` (branched from `main` at `39565fea25ae5721fa8bbe96868240191e564ab1` = `39565fe`)
- Clean tree precondition: `git status --porcelain` is empty.
- Pinned commit SHA: `39565fea25ae5721fa8bbe96868240191e564ab1`
- Threat Model: Routine enhancement and safety guard against unintended automated interaction with interactive CLI menus. Standard CI and toolchain trust apply.

## 3. Binding Scope Decisions

1. **Storage and Schema**:
   - `AppSettings.menu_guard_enabled: bool` (serialized as `menuGuardEnabled`, default `true`) acts as a global master kill switch.
   - `AgentConfig.blocking_menus: Option<Vec<BlockingMenuEntry>>` (serialized as `blockingMenus`).
   - `BlockingMenuEntry::Valid(BlockingMenuConfig)` / `Invalid(serde_json::Value)` untagged wrapper preserves hand-edited malformed entries without corrupting `settings.json` (mirroring `WatcherEntry` precedent `settings.rs:591-612`).
   - `BlockingMenuConfig`: `pattern: String` (required), `notification: String` (required), `enabled: bool` (default `true`), `captured_against: Option<String>` (serialized as `capturedAgainst`).
   - Defaults materialized on load/save when `blocking_menus` is `None`. Explicit `"blockingMenus": []` (`Some(vec![])`) disables the guard for that agent.
2. **Default Pattern Texts (Evidence-Backed Only)**:
   - `pi` (command stem `pi`): pattern `r"^\s*Trust project folder\?"`, notification `"pi is waiting for you to answer the folder-trust menu in this terminal"`, capturedAgainst `"pi 0.52 / Windows"`.
   - `codex` (command stem `codex`): pattern `r"^\s*Do you trust the contents of this directory\?"`, notification `"codex is waiting for you to answer the folder-trust menu in this terminal"`, capturedAgainst `"codex 0.x / Linux"`.
   - Other stems (`claude`, `agent`/Cursor, `antigravity`/`agy`): empty default list `[]` until evidence is captured in a follow-up task.
3. **Detection and Episode Lifecycle**:
   - Proactive periodic scan (250 ms tick) evaluates logical rows from `vt100` screen mirror (`pty::watchers::frame::logical_rows`).
   - Episode ID increments on every new match. Manual resolution records `suppressed_episode_id = Some(episode_id)`, releasing the hold for that episode only. A disappearance and reappearance starts a new episode and re-arms the block and notification.
4. **Canonical Deferral Prefix & Injection Chokepoint Enforcement**:
   - Canonical constant `crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED = "menu_guard_deferred"`.
   - Programmatic chokepoint `inject_text_into_session_impl` checks menu-guard block state under the held input permit before writing any byte; if blocked, returns `Err(format!("{}: session {} is blocked by interactive menu", ERR_MENU_GUARD_DEFERRED, session_id))`.
   - Container PTY input operations perform pre-flight check in `wait_for_pty_input_ready` before acquiring route guard and return `C::MenuGuardBlocked`; `finish_pty_input_before_boundary` routes `C::MenuGuardBlocked` through `store.retry_pty_input_offloaded`, scheduling retry with backoff and lease release without failing the operation as `Indeterminate` (`C::FinalRevalidationFailed`) or `Rejected`.
   - `pty_input_reason_allowed_for_status` in `src-tauri/src/phone/types.rs` explicitly allows `C::MenuGuardBlocked` under `S::Queued`, ensuring `validate_enqueued_pty_input_result` passes.
   - Direct user keystrokes (`pty_write`) bypass programmatic injection and are never blocked.
5. **Delivery Settle, Outbox Deferral, and DB Queue Lease Release & Schema v3**:
   - In `mailbox.rs`, `settle_live_before_inject` checks `menu_guard.is_blocked(session_id)`; when blocked, it loops/waits rather than immediately returning `InjectNow`, preventing poller thrashing.
   - In `mailbox.rs`, `settle_until_ready` checks menu-guard block; a blocked session is treated as not ready. If `max_wait` (90s) expires while blocked, it returns `Err(format!("{}: session {} is blocked by interactive menu", ERR_MENU_GUARD_DEFERRED, session_id))` rather than forcing injection.
   - In `record_message_outcome`, `is_menu_guard_deferred_error` (`e.starts_with(ERR_MENU_GUARD_DEFERRED)`) bypasses `state.attempt_count` increment, preserving outbox retry budget across indefinite menu dwell times.
   - In `api/dispatcher.rs`, `dispatch_due_with` checks `is_menu_guard_deferred_error(&reason)`: on deferral, calls `store.release_delivery_lease_offloaded(...)`, which resets `status` to `STATUS_QUEUED` with `next_attempt_at` set to now and clears `lease_owner` and `lease_until` without incrementing `attempt` or transitioning to `STATUS_POISONED`.
   - In `api/message_store.rs`, SQLite schema migration to version 3 rebuilds `pty_input_operations` and `pty_input_tombstones` tables with updated `CHECK` constraints including `'menu_guard_blocked'`, ensuring backward and forward compatibility on existing persistent installations.
6. **User Alerts & IPC**:
   - `SessionCommunicationKind::BlockedMenu` added to `SessionCommunication`.
   - Sidebar renders blocked-menu indicator in replica row communication slot across ALL agent replica rows (not gated by `isCoord()` or `taskTitle`).
   - Toast notification with `"Resolved by user"` action button shown to the user.
   - Tauri command `resolve_blocking_menu(id: String)` suppresses current episode and clears communication.

## 4. Phase Table

| Phase ID | Child Slug | Class | Owner | File Count | Depends On | Parallel With | Phase-SHA256 |
|---|---|---|---|---|---|---|---|
| Phase 1A | `phase-1a-backend-core` | `patterned` | `ac-dev-rust-v4` | 8 (+10 mechanical) | None | None | `FEAB11A1A633A1AD09610E351B455E8DEF2535F0C4C3E3E03BEC2489FFDF2D81` |
| Phase 1B | `phase-1b-queue-injection` | `patterned` | `ac-dev-rust-v4` | 7 | Phase 1A | None | `2335FCCC8F8187F8692E404E12441E0420C96ECA7DA0049D8647F72512D58832` |
| Phase 2 | `phase-2-frontend-ui` | `patterned` | `ac-dev-webpage-ui-v4` | 7 | Phase 1B | None | `A32026AD096C62ECB1E586F1D4019B8BA78ACF84751021F2EE1F10B06805879C` |

*Note on Phase 1A Scope and Cross-Phase Hand-Off:* Phase 1A includes 8 design-bearing production files (including `src-tauri/src/cli/coding_agent.rs` with `blocking_menus: None` literals in `blank_agent` and `definition_to_agent_seed`) plus 10 test/helper files updated under the plan-partitioning mechanical exception (`blocking_menus: None` and `message: None` across struct-literal sites to satisfy compiler `E0063`). Phase 1A touches `src-tauri/src/phone/mailbox.rs` ONLY at the 3 listed `cfg(test)` initializer lines; all production logic in `mailbox.rs` remains strictly owned by Phase 1B (both phases owned by `ac-dev-rust-v4`). Other test initializer sites belong to no other phase in the epic.

## 5. Dependency Cycle and Layering Statement

- Module arcs added:
  - `src-tauri/src/pty/menu_guard/mod.rs` references `crate::config::settings`, `crate::pty::watchers::frame`, `crate::pty::backend`, and `crate::session::session`.
  - `src-tauri/src/pty/inject.rs`, `src-tauri/src/phone/mailbox.rs`, `src-tauri/src/api/dispatcher.rs`, and `src-tauri/src/commands/session.rs` reference `crate::pty::menu_guard`.
  - `src-tauri/src/phone/mailbox.rs` and `src-tauri/src/api/error.rs` reference `crate::phone::types::PtyInputReasonCode::MenuGuardBlocked`.
  - `src-tauri/src/api/dispatcher.rs` references `crate::api::message_store` and `crate::pty::menu_guard::is_menu_guard_deferred_error`.
- Mechanical struct-literal initializers (`blocking_menus: None` and `message: None`) use Rust prelude `None` and add zero new module references or arcs.
- All added arcs are internal to pre-existing module SCCs or respect top-down dependency direction (`commands` -> `phone` -> `pty` / `session` -> `config` / `api`).
- Lower-layer modules retain pure predicates, error constants, and types; UI-transport and `AppHandle` interactions remain isolated at the commands/lifecycle boundary.
- Regenerated `src-tauri/module-arcs.txt` remains valid and acyclic with respect to SCC boundaries.

## 6. Delivery Nonfunctional Invariants

- Deterministic toolchain: Rust 1.80+ / Node 20+ pinned environment.
- Scoped Git: All mutations confined to `repo-AgentsCommander` on branch `feature/1646-pty-menu-guard`.
- Verification gates:
  - Phase 1A: `cargo test --manifest-path src-tauri/Cargo.toml` passes.
  - Phase 1B: `cargo test --manifest-path src-tauri/Cargo.toml` passes.
  - Phase 2: `npm test` and `npm run build` in repo root pass.
- Recovery protocol: In case of test failure, revert only files modified within the failing phase.
