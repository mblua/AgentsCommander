# Lock order — first slice (#4457)

Scope: two subsystems with nested locks held across awaits. Other locks are not covered yet. Line numbers are not used; identifiers are.

Names:
- G1 = `tokio::sync::RwLock<SessionManager>` (outer, Tauri state).
- Inner = the `state` field of `SessionManager` (tokio RwLock, taken inside `SessionManager` methods).
- G2 = `TelegramBridgeState` (tokio Mutex).
- PTY = `Arc<std::sync::Mutex<PtyManager>>`.
- P = carrier mutation permit (`acquire_carrier_mutation_permit`).

## A. Telegram bridge
Order: G1 -> G2. Never await G1, or a SessionManager call that needs it, while holding G2.
- `attach_telegram_bot_by_id` holds G1 (read) and G2 together while it awaits `set_telegram_bot_id` and `persist_current_state_result`. That covers a sync settings file read and the wait for a spawned persist task.
- The spawned persist task (`run_owned_persist_core`) does not take G1 or G2. Its own order: P -> Inner (read, held while the carrier snapshot is taken and while the rename step runs) -> settings agents-layer publication state (`try_with_agents_layer_publication_state`) -> try snapshot leaf (`SAVE_SESSIONS_LOCK`) -> try publication slot. The last three are try-locks that return Retry.
- Attach also takes P directly while it holds G2: `set_telegram_bot_id` takes P, then the Inner write, and releases both when it returns. Effective order G1 -> G2 -> P -> Inner. P is nested in `set_telegram_bot_id`, and reached again by waiting in the persist task.
- Wait edge: after that, attach waits for the spawned persist task while still holding G1 and G2, and that task takes P again. Rules: never take or await G2 while holding P; never call `persist_current_state_result` or `persist_current_state` while holding P (the persist task would wait for the caller's own P). Checked holders today drop P before G2 or persist; no deadlock is shown.
- e.g. `teardown_legacy_destroy_runtime`, `detach_response_close_telegram` and `detach_restart_telegram` take G2 alone and release it before any G1 use. Only `teardown_legacy_destroy_runtime` then takes G1 (clone and release); the other two do not take G1.
- `raise_reader_demand_guarded` drops G2 before awaiting `session_is_live`.
- Known mismatch: the comment above `session_is_live` says the destroy path takes the session lock and then G2. The three Telegram-detach helpers above take G2 alone; no checked destroy path takes the session lock and then G2 (callers above `destroy_one_legacy_session` not traced). The rule is right; the stated reason is not confirmed.
- Known cost: attach holds G2 across a sync settings file read, the P wait in `set_telegram_bot_id`, and the persist-task wait. Every other G2 user waits that long, including app exit (`begin_exit` blocks the main thread on G2). G1 has no production writer today, so its read hold blocks nobody. Hypothetical: if a G1 writer is added, it and later readers would wait too (tokio RwLock is fair). No deadlock is shown.

## B. SessionManager and PTY
Two separate pairs, not one chain:
1. G1 -> Inner when both are held (Inner is released when the method returns; Inner can also be taken through a cloned `SessionManager` without G1).
2. G1 -> PTY. PTY is a std mutex: never hold it across an await, never await G1 while holding it. A std guard is not Send, so the compiler rejects holding it across an await in spawned tasks and Tauri command futures; that is partial enforcement only.
Sites: the live-members block in the session commands (G1, awaits `agent_team_members`, then PTY) and `find_root_session_candidate`. Inner is released before PTY at both. `destroy_one_legacy_session` takes PTY as a temporary without G1, then P.

## How this is checked
Review rule: a change that takes a lock named here in a new place, adds a lock while one of them is held, adds an await while one is held, or persists while holding P must update this file in the same PR and keep the order. The Rust reviewers (dev-rust-grinch, architect-grinch) check it.
Limit: human review only. No CI job fails on an inversion today. The existing order tests cover cross-process file locks only (stage_d / stage_e, seed-manifest) and are not run by default.

## Not covered (next slices)
Loops, idle detector (#9), watchers/quota, PTY kill, entity deletion, settings owner. Global state ownership: #3299, #3247.
