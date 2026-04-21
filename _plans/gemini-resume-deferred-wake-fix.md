# Plan: Gemini Resume — Deferred-Wake Fix for `restart_session`

**Branch**: `feature/gemini-auto-resume` (stay on it; do NOT touch `main`; do NOT open PRs)
**Repo**: `repo-AgentsCommander`
**Base commit**: `34ce4ba`
**Type**: bug fix — follow-up to `_plans/gemini-auto-resume.md`

---

## 1. Requirement

The frontend overloads `restart_session` for two semantically different intents:

1. **True user-intent restart** — `SessionItem.tsx:188` context menu, `ProjectPanel.tsx:281` replica restart menu, `AcDiscoveryPanel.tsx:367` discovery restart menu. User WANTS a fresh conversation (no auto-resume).
2. **Wake a deferred session** — `ProjectPanel.tsx:107` (`handleReplicaClick` when `!isSessionLive(existing)`). User clicked a non-coordinator replica whose PTY was Exited(0) because `startOnlyCoordinators: true` skipped it at startup. User wants to CONTINUE the prior conversation.

Today `restart_session` hardcodes `skip_auto_resume: true` (`commands/session.rs:817`), so the wake path never injects `claude --continue`, `codex resume --last`, or `gemini --resume latest`. Runtime logs confirm: for gemini session `6c415634` (wg-5-dev-team/shipper) clicked at 09:37:21 today, `destroy_session_inner` + `create_session_inner` fire back-to-back with no `Auto-injected \`gemini --resume latest\`` log line anywhere.

Fix: let the caller opt out of `skip_auto_resume` per-call. Default preserves today's restart semantics.

**Approach**: Option A from the tech-lead message (add optional parameter). Chosen because:
- Single code path; no duplication of the destroy+create+re-attach+persist sequence.
- The frontend already knows the distinction (`isSessionLive` check), so pushing a boolean through is honest.
- Smallest blast radius — 1 Rust signature tweak, 1 TS interface tweak, 1 call-site change.
- Reversible: other three callers stay unchanged because `None` defaults to `true`.

---

## 2. Affected Files & Changes

### 2.1. `src-tauri/src/commands/session.rs`

#### Change 1 — `restart_session` signature (line 752-761)

Current:
```rust
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
) -> Result<SessionInfo, String> {
```

Replace with:
```rust
#[tauri::command]
pub async fn restart_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    tg_mgr: State<'_, TelegramBridgeState>,
    settings: State<'_, SettingsState>,
    id: String,
    agent_id: Option<String>,
    skip_auto_resume: Option<bool>,
) -> Result<SessionInfo, String> {
```

Tauri 2 default arg mapping makes this available on the JS side as `skipAutoResume`. No `#[serde(rename_all)]` needed on the command itself (argument mapping is automatic).

#### Change 2 — docstring (line 749-751)

Current:
```rust
/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY (no provider auto-resume). The restarted session is
/// automatically activated, Telegram bridges are re-attached, and state is persisted.
```

Replace with:
```rust
/// Restart a session: destroy the existing one and recreate it with the same
/// configuration but a fresh PTY. By default suppresses provider auto-resume
/// (true user-intent restart — fresh conversation). Callers that are instead
/// *waking* a previously-deferred session (e.g. a non-coordinator replica whose
/// PTY was Exited(0) at startup due to `startOnlyCoordinators: true`) pass
/// `skip_auto_resume = Some(false)` to allow `claude --continue`,
/// `codex resume --last`, or `gemini --resume latest` injection.
/// The restarted session is automatically activated, Telegram bridges are
/// re-attached, and state is persisted.
```

#### Change 3 — forward the value (line 817)

Current:
```rust
        git_repos,
        true, // skip_auto_resume — the whole point of restart
    )
    .await?;
```

Replace with:
```rust
        git_repos,
        effective_restart_skip_auto_resume(skip_auto_resume),
    )
    .await?;
```

#### Change 4 — add the helper

Add this free function **immediately above `restart_session`** (so above line 749, after the end of `destroy_session` at line 747):

```rust
/// Resolves the effective `skip_auto_resume` flag for `restart_session`.
/// Defaults to `true` (fresh conversation) to preserve existing restart-button semantics.
/// `Some(false)` is used by the deferred-wake path (ProjectPanel.handleReplicaClick)
/// to allow provider auto-resume and continue the prior conversation.
fn effective_restart_skip_auto_resume(requested: Option<bool>) -> bool {
    requested.unwrap_or(true)
}
```

This extraction exists **only** to give the change a unit-testable seam — `restart_session` itself is a `#[tauri::command]` bound to `State<'_, …>`, which cannot be constructed in a `#[test]`.

#### Change 5 — unit tests (inside the existing `#[cfg(test)] mod tests` block starting at line 1235)

Append these three tests alongside the existing injection tests. `super::effective_restart_skip_auto_resume` is the new helper:

```rust
#[test]
fn effective_restart_skip_auto_resume_defaults_to_true_for_none() {
    // No explicit value → preserve legacy "fresh conversation" semantics
    // used by SessionItem, ProjectPanel context menu, AcDiscoveryPanel.
    assert!(super::effective_restart_skip_auto_resume(None));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_false() {
    // Deferred-wake path (ProjectPanel.handleReplicaClick) MUST be able
    // to opt in to provider auto-resume; otherwise gemini/codex/claude
    // sessions re-open with a blank slate instead of continuing.
    assert!(!super::effective_restart_skip_auto_resume(Some(false)));
}

#[test]
fn effective_restart_skip_auto_resume_respects_explicit_true() {
    // Explicit true still works (future-proof against a caller that
    // wants to be explicit rather than rely on the default).
    assert!(super::effective_restart_skip_auto_resume(Some(true)));
}
```

End-to-end coverage of the gemini wake flow (that `inject_gemini_resume` actually fires for `restart_session(..., Some(false))` on a gemini session) is already indirectly provided by:
- Existing `inject_gemini_resume_*` tests (lines 1263-1289) — prove the injection function works.
- `create_session_inner` gating at `session.rs:392` — `if is_gemini && !skip_auto_resume { ... inject_gemini_resume(...) }`. Proven reachable once `effective_restart_skip_auto_resume(Some(false))` returns `false`.

No integration test exists for `restart_session` today; adding one would require standing up a full Tauri app handle and a spawned PTY. That is out of scope for this fix — the seam + unit tests pin the new contract.

---

### 2.2. `src/shared/ipc.ts`

#### Change 1 — `RestartSessionOptions` (line 35-37)

Current:
```ts
export interface RestartSessionOptions {
  agentId?: string;
}
```

Replace with:
```ts
export interface RestartSessionOptions {
  agentId?: string;
  /**
   * Forwarded to the backend `restart_session` command. Omit (or pass `true`)
   * for a true user-intent restart that starts a fresh conversation. Pass
   * `false` when waking a deferred session (PTY exited due to
   * `startOnlyCoordinators: true`) to allow provider auto-resume
   * (`claude --continue`, `codex resume --last`, `gemini --resume latest`).
   */
  skipAutoResume?: boolean;
}
```

#### Change 2 — `SessionAPI.restart` invoke payload (line 52-56)

Current:
```ts
  restart: (id: string, opts?: RestartSessionOptions): Promise<Session> =>
    transport.invoke<Session>("restart_session", {
      id,
      agentId: opts?.agentId ?? null,
    }),
```

Replace with:
```ts
  restart: (id: string, opts?: RestartSessionOptions): Promise<Session> =>
    transport.invoke<Session>("restart_session", {
      id,
      agentId: opts?.agentId ?? null,
      skipAutoResume: opts?.skipAutoResume ?? null,
    }),
```

`null` maps to Rust's `Option::None`, which `effective_restart_skip_auto_resume` resolves to `true` — preserving the three existing callers' behavior.

---

### 2.3. `src/sidebar/components/ProjectPanel.tsx`

#### Change — `handleReplicaClick` deferred-wake call (line 107)

Current:
```ts
      if (!isSessionLive(existing)) {
        // Session exists but PTY has exited — restart it
        try {
          await SessionAPI.restart(existing.id);
          if (isTauri) {
            await WindowAPI.ensureTerminal();
          }
        } catch (e) {
          console.error("Failed to restart session:", e);
        }
        return;
      }
```

Replace with:
```ts
      if (!isSessionLive(existing)) {
        // Session exists but PTY has exited (deferred at startup by
        // startOnlyCoordinators, or prior shutdown). Wake it with provider
        // auto-resume so the prior conversation continues — this is NOT a
        // user-intent "fresh conversation" restart.
        try {
          await SessionAPI.restart(existing.id, { skipAutoResume: false });
          if (isTauri) {
            await WindowAPI.ensureTerminal();
          }
        } catch (e) {
          console.error("Failed to wake session:", e);
        }
        return;
      }
```

The `console.error` label change is intentional — it reflects the actual semantics and makes the diagnostic distinguishable from true-restart failures at the other call sites.

---

### 2.4. No-change call sites (verify unchanged)

These callers **must NOT** be modified — they represent user-intent "fresh conversation" restart and rely on the `None → true` default:

| File | Line | Context |
|---|---|---|
| `src/sidebar/components/SessionItem.tsx` | 188 | Session context menu → "Restart Session" |
| `src/sidebar/components/ProjectPanel.tsx` | 281 | Replica context menu → `restartReplicaSession` |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | 367 | Discovery panel replica context menu → "Restart Session" |

Dev should **grep** these three lines after implementing and confirm no accidental changes crept in.

---

## 3. Ripple Effects Audit

Checked every caller of `create_session_inner` and every `skip_auto_resume` reference (`Grep` over `src-tauri/src`):

| File:line | Caller | Passes skip_auto_resume as | Relation to fix |
|---|---|---|---|
| `lib.rs:587` | startup restore (non-deferred) | `false` | Unrelated; already correct. |
| `commands/session.rs:621` | `create_session` command | `false` | Unrelated; user-new sessions resume naturally. |
| `commands/session.rs:817` | `restart_session` | **changes to `effective_restart_skip_auto_resume(...)`** | The fix itself. |
| `commands/session.rs:1189` | `create_root_agent_session` | `false` | Unrelated. |
| `phone/mailbox.rs:525` | phone wake-on-message spawn | `false` | Unrelated; agent must continue prior conversation. |
| `phone/mailbox.rs:1592` | session-requests mailbox | `false` | Unrelated. |
| `web/commands.rs:80` | WS `create_session` handler | `false` | Unrelated. WS has NO `restart_session` handler (grep-confirmed) — this fix is Tauri-only, as expected (the web transport isn't used by the frontend replica-click path). |

**Startup deferral path (`lib.rs` `startOnlyCoordinators`):** the deferred stubs are created without spawning a PTY, so they land in state as `Exited(0)` placeholders. No call site on the startup path calls `restart_session` — deferral is strictly a frontend concern until the user clicks the replica. Confirmed.

**Tauri command registration (`lib.rs:632`):** `commands::session::restart_session` is listed in the `invoke_handler!` macro. Adding an `Option<bool>` argument is transparent to the macro — it just expands to another deserialized field on the generated `Args` struct. No edit required.

**Telegram bridge re-attach (`commands/session.rs:835-867`):** unaffected. The `skip_auto_resume` decision happens before bridge attach and doesn't touch the bridge path.

**Persistence strip logic (`config/sessions_persistence::strip_auto_injected_args` at `commands/session.rs:789`):** already runs on the stored `shell_args` before the destroy+create. Unaffected. The new create (whether skipping or not) will re-inject on top of a clean recipe — exactly the existing contract.

**Existing tests at `session.rs:1263-1289`:** test `inject_gemini_resume` directly. Unaffected by the signature change because `restart_session` is not exercised in tests today.

**`_plans/gemini-auto-resume.md` §4 constraints** ("Do not add new crates. Maintain existing IPC patterns."): respected — no new crates; adding an `Option<bool>` to a `#[tauri::command]` and a matching optional field in an `invoke` payload is the existing IPC pattern (see `CreateSessionOptions` in `ipc.ts:26-33` which uses the same `?? null` pattern across many fields).

---

## 4. Validation Steps (for the implementing devs)

After dev-rust applies 2.1 and dev-webpage-ui applies 2.2–2.3:

1. **Rust typecheck**: `cd src-tauri && cargo check`
2. **Rust lint**: `cd src-tauri && cargo clippy` — must be clean.
3. **Rust tests**: `cd src-tauri && cargo test effective_restart_skip_auto_resume` (3 new tests pass) and `cargo test` (no regressions).
4. **TS typecheck**: `npx tsc --noEmit` — must be clean.
5. **Manual smoke (Windows, dev build)**:
   - Set `startOnlyCoordinators: true` in `~/.agentscommander/config.toml`.
   - Create a wg with a gemini-backed non-coordinator replica and have at least one prior conversation with that replica (so `~/.gemini/...latest` exists).
   - Close AC, reopen. Sidebar shows the replica with Exited state.
   - Click it. Observe in `~/.agentscommander_standalone/app.log` a line:
     `Auto-injected \`gemini --resume latest\` for agent '<agent_id>'`
   - Terminal window should open with gemini resuming the prior conversation, not starting blank.
   - Repeat with a codex and a claude replica — both must now log the corresponding auto-inject line on click-wake.
6. **Manual smoke — true restart unchanged**:
   - Right-click a LIVE replica → "Restart Session" (SessionItem context menu).
   - Confirm the log does NOT show an auto-inject line (the `None → true` default holds).
   - Session comes up with a fresh conversation, as today.

---

## 5. Constraints & Reminders

- Stay on branch `feature/gemini-auto-resume`. No PRs; local merge only, per CLAUDE.md.
- Do not add new crates. Do not refactor surrounding code in `restart_session` (bridge, persistence, telegram re-attach stay untouched).
- Do not touch the three "no-change" callers listed in §2.4.
- Bump patch version across the three version files (`tauri.conf.json`, `Cargo.toml`, `Titlebar.tsx`) per repo convention on the next compilable change set.
- Version strip logic (`strip_auto_injected_args`) is unaffected by this fix — the existing gemini strip from `_plans/gemini-auto-resume.md` §2 handles it.

---

## 6. Rollout Order Suggested for Tech-Lead

1. dev-rust: applies 2.1 (signature + helper + docstring + tests). `cargo check`, `cargo clippy`, `cargo test` all green.
2. dev-webpage-ui: applies 2.2 (ipc.ts) and 2.3 (ProjectPanel.tsx line 107). `npx tsc --noEmit` clean.
3. Tech-lead merges internally on the branch, runs the manual smoke in §4 step 5, confirms the three log lines appear.
4. If smoke passes, ship.

Single-round change — no protocol handshake needed across the two devs because the boundary (JSON field `skipAutoResume` ↔ Rust `Option<bool>`) is trivial and pinned by this plan.
