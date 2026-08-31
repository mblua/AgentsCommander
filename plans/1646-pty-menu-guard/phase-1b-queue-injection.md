# Phase 1B: Queue, delivery settle, injection freeze, and dispatcher integration

Objective: Wire menu guard into programmatic injection chokepoints, wake settle hold and outbox retry preservation in mailbox, container PTY input retry with `C::MenuGuardBlocked`, DB queue lease release in message store with SQLite schema v3 migration, and dispatcher deferral handling.
Class: patterned
Owner: ac-dev-rust-v4

## 1. Exact Files and Symbols Modified

1. `src-tauri/src/phone/types.rs`:
   - `pub enum PtyInputReasonCode`: add variant `MenuGuardBlocked`.
   - `pub const fn pty_input_reason_code_name(code: PtyInputReasonCode) -> &'static str`:
     - `C::MenuGuardBlocked => "menu_guard_blocked",`
   - `pub const fn safe_detail(code: PtyInputReasonCode) -> &'static str`:
     - `C::MenuGuardBlocked => "The target session is blocked by an interactive menu.",`
   - `pub const fn pty_input_reason_allowed_for_status(status: PtyInputPublicStatus, reason: Option<PtyInputReasonCode>) -> bool`:
     - Update `S::Queued` branch to allow `C::MenuGuardBlocked`:
       `None | Some(C::RestoreInProgress | C::PurgeInProgress | C::SessionRace | C::LeaseLost | C::SpawnFailedSafe | C::StoreTransient | C::MenuGuardBlocked)`.

2. `src-tauri/src/api/error.rs`:
   - `fn pty_input_http_status(code: crate::phone::types::PtyInputReasonCode) -> StatusCode`:
     - `C::MenuGuardBlocked => StatusCode::SERVICE_UNAVAILABLE,`

3. `src-tauri/src/pty/inject.rs`:
   - In `inject_text_into_session_impl`: under the held `permit`, query `app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()`. If `guard.is_blocked(session_id)`, abort injection and return `Err(format!("{}: session {} is blocked by interactive menu", crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED, session_id))`.
   - Low-level writer `write_exact_agent_input_first` is preserved unchanged (PTY input guard check is enforced upstream in `mailbox.rs:wait_for_pty_input_ready` / `dispatch_pty_input_operation`).

4. `src-tauri/src/phone/mailbox.rs`:
   - In `finish_pty_input_before_boundary`: add `C::MenuGuardBlocked` to the retryable `matches!` list (`RestoreInProgress | PurgeInProgress | SessionRace | LeaseLost | SpawnFailedSafe | StoreTransient | MenuGuardBlocked`), delegating to `store.retry_pty_input_offloaded(injection_id.to_string(), lease_owner.to_string(), code, chrono::Utc::now()).await` to transition operation to `retry` status with scheduled backoff and lease release instead of rejecting.
   - In `wait_for_pty_input_ready`: read `menu_guard = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()`. If `menu_guard.as_ref().is_some_and(|g| g.is_blocked(session_id))`, return `Err(C::MenuGuardBlocked)`. This returns through `await_pty_input_before_deadline` to `finish_pty_input_before_boundary` before acquiring route guard or invoking `prepare_pty_input_boundary`, avoiding any terminalizing error path (`C::FinalRevalidationFailed`).
   - In `settle_live_before_inject`: query `menu_guard = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()`. In the established candidate loop, if `menu_guard.as_ref().is_some_and(|g| g.is_blocked(session_id))`:
     - If `start.elapsed() >= max_wait` (10s), return to allow injector to return `ERR_MENU_GUARD_DEFERRED`.
     - Otherwise, sleep for `poll` (500 ms) and `continue` looping.
   - In `settle_until_ready`: read `menu_blocked = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>().is_some_and(|g| g.is_blocked(session_id))`. Define readiness as `!menu_blocked && wake_settle_ready(waiting, rendered)`. If `max_wait` (90s) expires while `menu_blocked`, return `Err(format!("{}: session {} is blocked by interactive menu", crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED, session_id))` rather than forcing injection.
   - In `record_message_outcome`: if `crate::pty::menu_guard::is_menu_guard_deferred_error(&e)`, log debug and return immediately without incrementing `state.attempt_count` or moving message to `rejected/`.

5. `src-tauri/src/api/message_store.rs`:
   - Add `pub fn release_delivery_lease(&self, message_id: &str, reason: &str, now: DateTime<Utc>) -> Result<(), MessageStoreError>`:
     Executes:
     ```sql
     UPDATE messages
     SET status = ?1, next_attempt_at = ?2, lease_owner = NULL, lease_until = NULL, last_error = ?3
     WHERE message_id = ?4
     ```
     params: `[STATUS_QUEUED, &now_s, reason, message_id]`. Resets `status` to `STATUS_QUEUED` and `next_attempt_at` to `now_s`, clears `lease_owner` and `lease_until`, records `last_error`, preserves `attempt` count without advancing, and inserts audit record `insert_audit(&tx, message_id, "lease-released-deferred", Some(reason), &now_s)`.
   - Add `pub async fn release_delivery_lease_offloaded(&self, message_id: String, reason: String, now: DateTime<Utc>) -> Result<(), MessageStoreError>`.
   - Schema & DDL CHECK constraint updates:
     - Update SQLite table DDL in `schema_version < 2` for `pty_input_operations.reason_code` (line 957) and `pty_input_tombstones.reason_code` (line 1136) to include `'menu_guard_blocked'` in `reason_code IN (...)`.
     - Update `pty_input_operations` status/reason pair constraint (line 1018) for `status IN ('queued','preparing','retry')` to include `'menu_guard_blocked'` in the allowed reason list (`'restore_in_progress','purge_in_progress','session_race','lease_lost','spawn_failed_safe','store_transient','menu_guard_blocked'`).
   - SQLite Schema v3 Migration / Table Rebuild Facility:
     - In `migrate(&self)`:
       - Update schema corruption check to `if schema_version > 3 { return Err(MessageStoreError::StoreCorrupt); }`.
       - Disable foreign keys before migration: `conn.pragma_update(None, "foreign_keys", "OFF")?;` (restored to `"ON"` after transaction commit).
       - When `schema_version < 3`: execute version 3 migration step inside the transaction:
         1. Create new table `pty_input_operations_v3` with identical columns and updated CHECK constraints (including `'menu_guard_blocked'`).
         2. Copy data: `INSERT INTO pty_input_operations_v3 SELECT * FROM pty_input_operations;`.
         3. Drop old table: `DROP TABLE pty_input_operations;`.
         4. Rename table: `ALTER TABLE pty_input_operations_v3 RENAME TO pty_input_operations;`.
         5. Recreate index: `CREATE INDEX idx_pty_input_due ON pty_input_operations(source_plane, status, next_attempt_at, lease_until);`.
         6. Create new table `pty_input_tombstones_v3` with identical columns and updated CHECK constraints (including `'menu_guard_blocked'`).
         7. Copy data: `INSERT INTO pty_input_tombstones_v3 SELECT * FROM pty_input_tombstones;`.
         8. Drop old table: `DROP TABLE pty_input_tombstones;`.
         9. Rename table: `ALTER TABLE pty_input_tombstones_v3 RENAME TO pty_input_tombstones;`.
         10. Insert schema version record: `INSERT INTO api_message_schema(version, applied_at) VALUES(3, ?1)` with `canonical_pty_timestamp(Utc::now())`.
         11. Set `schema_version = 3;`.

6. `src-tauri/src/api/dispatcher.rs`:
   - In `dispatch_due_with`: match on `result`:
     - `Ok(_) => { store.mark_delivered_offloaded(...).await?; audit::record(..., "delivered"); }`
     - `Err(reason) if crate::pty::menu_guard::is_menu_guard_deferred_error(&reason) => { store.release_delivery_lease_offloaded(row.message_id.clone(), reason.clone(), chrono::Utc::now()).await?; audit::record(..., "menu-guard-deferred"); }`
     - `Err(reason) => { let status = store.mark_delivery_failed_offloaded(..., config.max_attempts).await?; audit::record(..., &status); }`

7. `src-tauri/src/cli/send.rs`:
   - In `fn reason_code_for_cli(code: crate::phone::types::PtyInputReasonCode) -> &'static str`: replace exhaustive match body with delegation to `crate::phone::types::pty_input_reason_code_name(code)`, eliminating enum match duplication and pattern drift with `phone::types`.

## 2. Inlined Decisions and Behavior

- **CLI Reason Code Mapping Delegation**: `reason_code_for_cli` in `src-tauri/src/cli/send.rs` delegates directly to `crate::phone::types::pty_input_reason_code_name(code)`, permanently unifying CLI reason formatting with canonical `phone::types` naming and eliminating drift/non-exhaustive compilation hazards on new `PtyInputReasonCode` variants.
- **Canonical Deferral Error String**: All programmatic deferral return sites use `format!("{}: session {} is blocked by interactive menu", ERR_MENU_GUARD_DEFERRED, session_id)`. Handlers match with `is_menu_guard_deferred_error(e)` (`e.starts_with(ERR_MENU_GUARD_DEFERRED)`), preventing mismatch and accidental outbox/DB retry burn.
- **DB Queue Lease Reset to Queued**: Releasing the delivery lease resets `status` to `STATUS_QUEUED` with `next_attempt_at` set to now and clears `lease_owner`/`lease_until`, without incrementing `attempt` or transitioning to `STATUS_POISONED`. `lease_due` picks up the message on subsequent dispatch ticks when the menu block clears.
- **PTY Input Pre-flight Retry and Public Validation**: Container PTY input operations check menu guard in `wait_for_pty_input_ready` before acquiring route guard and return `C::MenuGuardBlocked`. `finish_pty_input_before_boundary` handles `C::MenuGuardBlocked` through `store.retry_pty_input_offloaded`, scheduling retry with backoff and lease release without failing the operation as `Indeterminate` (`C::FinalRevalidationFailed`) or `Rejected`. `pty_input_reason_allowed_for_status` explicitly permits `C::MenuGuardBlocked` under `S::Queued`, satisfying `validate_enqueued_pty_input_result` and preventing `Err(C::StoreCorrupt)`.
- **SQLite Schema v3 Rebuild Migration**: The message store (`message_bus.db` in `config_dir`) persists across application upgrades. Because SQLite cannot alter existing `CHECK` constraints in place, `migrate()` increments `schema_version` to 3 and performs an atomic table rebuild for `pty_input_operations` and `pty_input_tombstones` under `foreign_keys = OFF`, preserving all existing data while ensuring existing databases accept `'menu_guard_blocked'` without `CHECK constraint failed` errors.
- **Established Live Settle Hold**: Established wake candidate settle loops wait while menu guard is active rather than immediately returning `InjectNow`, preventing poller thrashing.
- **Outbox Attempt Preservation**: When an outbox message delivery fails due to menu guard deferral (`is_menu_guard_deferred_error`), `record_message_outcome` does not advance `attempt_count` or move the message to `rejected/`.
- **Direct Keystroke Passthrough**: Direct user keystrokes from xterm.js bypass `inject_text_into_session_impl` and invoke `pty_write` directly, allowing the user to interact with and answer the menu.

## 3. Required Tests and Verification

1. Unit tests in `src-tauri/src/api/message_store.rs` and `src-tauri/src/api/dispatcher.rs`:
   - `test_release_delivery_lease_resets_status_queued_and_preserves_attempts`: verifies releasing delivery lease resets `status` to `STATUS_QUEUED`, sets `next_attempt_at`, clears lease fields, and preserves `attempt` count without setting `STATUS_POISONED`.
   - `test_dispatcher_menu_guard_deferred_releases_lease`: verifies `dispatch_due_with` releases lease when delivery returns `ERR_MENU_GUARD_DEFERRED`.
   - `test_retry_pty_input_menu_guard_blocked_persisted`: verifies `store.retry_pty_input` succeeds with `C::MenuGuardBlocked`, transitions to `retry`, and records `reason_code = 'menu_guard_blocked'` in `pty_input_operations` without CHECK constraint violation.
   - `test_schema_v3_migration_rebuilds_check_constraints`: creates a v2 schema database, applies `migrate()`, and verifies existing records are preserved while new operations with `reason_code = 'menu_guard_blocked'` can be stored and tombstoned.
   - `test_validate_enqueued_pty_input_result_allows_menu_guard_blocked_under_queued`: verifies `validate_enqueued_pty_input_result` accepts `C::MenuGuardBlocked` for `PtyInputPublicStatus::Queued`.
2. Integration tests in `src-tauri/src/pty/inject.rs` and `src-tauri/src/phone/mailbox.rs`:
   - `test_injection_blocked_when_menu_guard_active`: verifies `inject_text_into_session_impl` returns `ERR_MENU_GUARD_DEFERRED` and writes 0 bytes when menu guard is blocked.
   - `test_wake_settle_defers_and_does_not_burn_attempts`: verifies `settle_until_ready` returns deferred error on timeout and outbox retry tracker attempt count remains 0.
   - `test_settle_live_holds_during_menu_guard_block`: verifies `settle_live_before_inject` waits while menu guard is active.
   - `test_pty_input_operation_retried_on_menu_guard`: verifies `wait_for_pty_input_ready` returns `C::MenuGuardBlocked` and `finish_pty_input_before_boundary` delegates to `retry_pty_input_offloaded`, transitioning operation to `retry` rather than `rejected` or `indeterminate`.

Verification command:
```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

## 4. Objective Acceptance Criteria

1. Running `cargo test --manifest-path src-tauri/Cargo.toml` completes with 0 errors and all new/existing tests pass.
2. When a session matches a blocking menu pattern, `inject_text_into_session_impl` returns an error starting with `ERR_MENU_GUARD_DEFERRED` and writes 0 bytes to the PTY.
3. When a session is blocked, wake settle holds without injecting at 90s, outbox retry attempts do not increment, and DB queue dispatches release the lease with status reset to `STATUS_QUEUED` without advancing `attempt` or poisoning.
4. Container PTY input operations against a blocked session are retried with `C::MenuGuardBlocked` and never terminalized as `Indeterminate` or `Rejected`.
5. Existing SQLite databases migrate cleanly to schema v3 and accept `'menu_guard_blocked'` in `pty_input_operations` and `pty_input_tombstones`.

## 5. Preserve List

- Preserve `write_exact_agent_input_first` low-level contract.
- Preserve direct user keystrokes in `commands/pty.rs:pty_write`.
- Preserve standard delivery retry/poison failure behavior for non-deferred errors.
