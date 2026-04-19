# Re-inject Credentials After `/clear`

**Branch**: `feature/reinject-credentials-after-clear`
**Repo**: `repo-AgentsCommander`
**Type**: feature (best-effort reliability improvement)

---

## 1. Requirement

`agentscommander send --command clear --to <agent>` writes `/clear\r` to the
agent's PTY (`src-tauri/src/phone/mailbox.rs:590-669`). Claude's `/clear`
wipes the context window — including the `# === Session Credentials ===`
block injected at session spawn (`src-tauri/src/commands/session.rs:374-478`).
After `/clear` the agent loses `Token`, `Root`, `Binary`, `BinaryPath`,
`LocalDir` and cannot talk to peers until a human repastes them.

After a successful `clear` remote command, automatically re-inject the SAME
credentials block the spawn path emits — only for agent sessions, only on
`clear` (never `compact`), only after idle returns.

---

## 2. Scope (hard constraints)

1. Trigger: ONLY `msg.command == Some("clear")`. `compact` path unchanged.
2. Gate: `session.agent_id.is_some()`. Plain shell sessions never receive
   a credentials block.
3. Wait for idle (`waiting_for_input == true`) via polling, max 30s, poll
   interval 500ms — same shape as `commands/session.rs:383-408` and
   `phone/mailbox.rs:749-776`. No fixed sleep.
4. DRY: extract the credentials-block builder currently inline at
   `commands/session.rs:410-453` into a helper and call it from BOTH
   spawn-path and new post-`/clear` path. Byte-for-byte identical output.
5. Injection uses the existing helper
   `crate::pty::inject::inject_text_into_session(app, session_id, &block, true)`
   with `submit = true` (Enter fired twice, same as spawn).
6. Token source: `Session.token` field (`Uuid`). Does NOT rotate on
   `/clear` — reuse as-is.
7. Hook point: inside `inject_into_pty` at
   `src-tauri/src/phone/mailbox.rs`, remote-command branch, immediately
   after the `message_delivered` emit and before the existing body-followup
   block (line 653). Background task via `tauri::async_runtime::spawn`. Do
   NOT block the delivery pipeline.
8. Best-effort: idle-poll timeout → `log::warn!` and abort; never `error!`,
   never retry.
9. No feature flags, no backwards-compat shims, no PRs, no merge to main.

---

## 3. Files touched

| File | Action | Lines |
|---|---|---|
| `src-tauri/src/pty/credentials.rs` | **CREATE** | new file |
| `src-tauri/src/pty/mod.rs` | modify | add `pub mod credentials;` |
| `src-tauri/src/commands/session.rs` | modify | replace inline block at 410-453 with helper call |
| `src-tauri/src/phone/mailbox.rs` | modify | add post-`/clear` re-inject task, add body chaining when `command == clear` |

---

## 4. New helper — `src-tauri/src/pty/credentials.rs`

Create this file with the exact contents below. It is a pure function: no
I/O except `std::env::current_exe()`, no session access, no lock.

```rust
//! Credential block builder.
//!
//! Produces the `# === Session Credentials ===` block injected into agent
//! sessions at spawn and after `/clear`. Output must stay byte-for-byte
//! identical across both call sites so agents parse consistently.

use uuid::Uuid;

/// Build the credentials block for a session.
///
/// The block is terminated by `\n` (no trailing Enter) — the caller is
/// responsible for flagging `submit=true` to `inject_text_into_session`
/// which adds the Enter keystrokes for agents that need them.
///
/// `token` is `Display`'d lowercase with dashes (standard `Uuid` format).
/// `cwd` is the session's working directory, verbatim.
///
/// `Binary`, `BinaryPath`, and `LocalDir` are derived from the current
/// process executable — the running `agentscommander*.exe`. This matches
/// the original inline behavior in `commands/session.rs` and is what
/// agents use to invoke back into the CLI.
pub fn build_credentials_block(token: &Uuid, cwd: &str) -> String {
    let exe = std::env::current_exe().ok();
    let binary_name = exe
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());
    let binary_path = {
        let raw = exe
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "agentscommander.exe".to_string());
        raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
    };
    let local_dir = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|parent| {
            parent
                .join(format!(".{}", &binary_name))
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| format!(".{}", &binary_name));
    let local_dir = local_dir
        .strip_prefix(r"\\?\")
        .unwrap_or(&local_dir)
        .to_string();

    format!(
        concat!(
            "\n",
            "# === Session Credentials ===\n",
            "# Token: {token}\n",
            "# Root: {root}\n",
            "# Binary: {binary}\n",
            "# BinaryPath: {binary_path}\n",
            "# LocalDir: {local_dir}\n",
            "# === End Credentials ===\n",
        ),
        token = token,
        root = cwd,
        binary = binary_name,
        binary_path = binary_path,
        local_dir = local_dir,
    )
}
```

### 4.1 Rationale for signature

- `token: &Uuid` — avoids `to_string()` roundtrip at spawn site (where we
  already have `Uuid`). At post-`/clear` site we have `Session.token: Uuid`
  from `SessionManager::get_session`. Both call sites pass `&session.token`
  directly.
- `cwd: &str` — matches `Session.working_directory: String`.
- Returns `String` — caller owns it and passes `&str` to
  `inject_text_into_session`.

---

## 5. `src-tauri/src/pty/mod.rs`

Current content (lines 1-4):

```rust
pub mod git_watcher;
pub mod idle_detector;
pub mod inject;
pub mod manager;
```

Add one line (kept alphabetic):

```rust
pub mod credentials;
pub mod git_watcher;
pub mod idle_detector;
pub mod inject;
pub mod manager;
```

---

## 6. `src-tauri/src/commands/session.rs` — replace inline block

### 6.1 Range to replace

Lines **410-453** inside the `tokio::spawn` auto-inject block. These lines
derive `exe`, `binary_name`, `binary_path`, `local_dir` and build
`cred_block`. The surrounding structure (idle-wait loop lines 382-408 and
the `inject_text_into_session` call lines 455-476) stays untouched.

### 6.2 Before (current, lines 410-453)

```rust
            let exe = std::env::current_exe().ok();
            let binary_name = exe
                .as_ref()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .unwrap_or_else(|| "agentscommander".to_string());
            let binary_path = {
                let raw = exe
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "agentscommander.exe".to_string());
                raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
            };
            let local_dir = exe
                .as_ref()
                .and_then(|p| p.parent())
                .map(|parent| {
                    parent
                        .join(format!(".{}", &binary_name))
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_else(|| format!(".{}", &binary_name));
            let local_dir = local_dir
                .strip_prefix(r"\\?\")
                .unwrap_or(&local_dir)
                .to_string();

            let cred_block = format!(
                concat!(
                    "\n",
                    "# === Session Credentials ===\n",
                    "# Token: {token}\n",
                    "# Root: {root}\n",
                    "# Binary: {binary}\n",
                    "# BinaryPath: {binary_path}\n",
                    "# LocalDir: {local_dir}\n",
                    "# === End Credentials ===\n",
                ),
                token = token,
                root = cwd_clone,
                binary = binary_name,
                binary_path = binary_path,
                local_dir = local_dir,
            );
```

### 6.3 After (single helper call)

```rust
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);
```

### 6.4 Context-preserved snippet for unambiguous placement

The dev should locate the block by the surrounding code:

```rust
                    None => {
                        log::warn!(
                            "[session] Session {} gone before credential injection",
                            session_id
                        );
                        return; // session destroyed, nothing to inject
                    }
                }
            }

            // <<< REPLACEMENT STARTS HERE (was lines 410-453) >>>
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);
            // <<< REPLACEMENT ENDS HERE >>>

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &cred_block,
                true,
            )
            .await
            {
```

### 6.5 Variable types note

At this call site:
- `token` is `Uuid` (from `session.token.clone()` at line 380).
- `cwd_clone` is `String` (from line 381, `cwd.clone()`).

`build_credentials_block(&token, &cwd_clone)` matches `(&Uuid, &str)` via
`String → &str` deref coercion. No new imports needed.

---

## 7. `src-tauri/src/phone/mailbox.rs` — post-`/clear` re-inject task

### 7.1 Hook point

Inside `inject_into_pty` remote-command branch. Insert a new spawned task
**between line 651 (end of `message_delivered` emit)** and **line 653 (the
existing body-followup `if` block)**.

### 7.2 Ordering semantics

When `command == "clear"`, we want:

1. `/clear` already written at line 629.
2. Wait for idle → inject credentials block.
3. If `msg.body` non-empty → wait for idle again → inject body.

When `command == "compact"`, the existing behavior is unchanged: skip the
new task; if body present the old follow-up task handles it.

So the new task **supersedes** the existing body-followup path for `clear`
only, to guarantee creds land before body. For `compact` the existing
follow-up runs as today.

### 7.3 Replacement shape

Replace the existing block at **lines 653-666**:

```rust
            // If body is also present, spawn follow-up as background task.
            // Command delivery is already complete — don't block the delivery pipeline.
            if !msg.body.is_empty() {
                let app_clone = app.clone();
                let msg_clone = msg.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!("Follow-up injection after remote command failed: {}", e);
                    }
                });
            }
```

With:

```rust
            // Post-command background work:
            //  - For `/clear` on an agent session: re-inject credentials (Claude wipes
            //    them with the context), then the body follow-up if present.
            //  - For `/compact` (or `/clear` on a plain shell): body follow-up only.
            // Never block the delivery pipeline — spawn as a detached task.
            let is_clear = command == "clear";
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            tauri::async_runtime::spawn(async move {
                if is_clear {
                    if let Err(e) = Self::reinject_credentials_after_clear_static(
                        &app_clone,
                        session_id,
                    )
                    .await
                    {
                        log::warn!(
                            "[mailbox] Credential re-inject after /clear skipped (session={}): {}",
                            session_id,
                            e
                        );
                    }
                }
                if !msg_clone.body.is_empty() {
                    if let Err(e) = Self::inject_followup_after_idle_static(
                        &app_clone,
                        session_id,
                        &msg_clone,
                    )
                    .await
                    {
                        log::warn!(
                            "[mailbox] Follow-up body injection after /{} failed (session={}): {}",
                            command_owned,
                            session_id,
                            e
                        );
                    }
                }
            });
```

Notes:
- `command_owned` exists only for the warn-log message; no behavior depends
  on it.
- Both sub-operations happen sequentially inside the spawned task — credit
  re-inject first, then body. The body follow-up still waits for idle
  internally, so ordering is preserved.
- If `is_clear` but session is not an agent, the helper returns early with
  a non-error warn message (see 7.4). No harm.

### 7.4 New static helper on `Mailbox` impl

Add after `inject_followup_after_idle_static` (insert immediately after
line 783, before `find_active_session`):

```rust
    /// Wait for agent to become idle after `/clear`, then re-inject the
    /// credentials block so the agent keeps its Token/Root/BinaryPath/
    /// LocalDir after its context window was wiped. Best-effort only.
    ///
    /// Gated to agent sessions (`session.agent_id.is_some()`). Plain shell
    /// sessions skip silently.
    ///
    /// Static — safe to spawn as a detached task without borrowing self.
    async fn reinject_credentials_after_clear_static(
        app: &tauri::AppHandle,
        session_id: Uuid,
    ) -> Result<(), String> {
        // Resolve the session once up front. We need `agent_id`, `token`,
        // and `working_directory`. We re-read inside the idle-poll loop
        // to detect destruction, but the fields we care about are
        // immutable for the lifetime of the session.
        let (token, cwd) = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            match mgr.get_session(session_id).await {
                Some(s) if s.agent_id.is_some() => {
                    (s.token, s.working_directory.clone())
                }
                Some(_) => {
                    // Not an agent session — nothing to re-inject.
                    return Ok(());
                }
                None => {
                    return Err(format!(
                        "Session {} gone before credential re-inject",
                        session_id
                    ));
                }
            }
        };

        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for idle before credential re-inject ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {}
                None => {
                    return Err(format!(
                        "Session {} destroyed during credential re-inject poll",
                        session_id
                    ));
                }
            }
        }

        // Build + inject. Same call shape as spawn path.
        let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd);
        crate::pty::inject::inject_text_into_session(app, session_id, &cred_block, true).await?;

        log::info!(
            "[mailbox] Credentials re-injected after /clear (session={})",
            session_id
        );
        Ok(())
    }
```

### 7.5 Imports

`mailbox.rs` already imports `Arc` (line near top). `SessionManager` is
already in scope (used by `inject_followup_after_idle_static`). `Uuid` is
in scope. No new imports needed — the helper uses
`crate::pty::credentials::build_credentials_block` and
`crate::pty::inject::inject_text_into_session` by fully-qualified paths.

---

## 8. Idle-poll loop parameters

| Param | Value | Rationale |
|---|---|---|
| `max_wait` | 30 s | Matches spawn path (`commands/session.rs:383`) and body follow-up (`mailbox.rs:749`). Consistent ceiling. |
| `poll` interval | 500 ms | Same. |
| On timeout | `log::warn!` + return `Err` (best-effort). No retry. | Spec §9: best-effort, no retry. |
| On session destroyed | `log::warn!` + return `Err`. | Nothing to inject into. |
| On `agent_id.is_none()` | Return `Ok(())` silently (no warn). | Not an error; feature doesn't apply. |

---

## 9. Logging

| Event | Level | Message pattern |
|---|---|---|
| Success | `info` | `[mailbox] Credentials re-injected after /clear (session={id})` |
| Idle timeout | `warn` | `[mailbox] Credential re-inject after /clear skipped (session={id}): Timeout waiting for idle before credential re-inject (30s)` |
| Session gone mid-poll | `warn` | `[mailbox] Credential re-inject after /clear skipped (session={id}): Session {id} destroyed during credential re-inject poll` |
| Non-agent session | (silent `Ok(())`) | no log — expected fast path |
| Body follow-up failure after clear | `warn` | `[mailbox] Follow-up body injection after /clear failed (session={id}): {e}` |

Never `log::error!` — feature is best-effort.

---

## 10. Race / TOCTOU analysis

Same class of race as the existing `/clear` write at
`mailbox.rs:619-623` (document lines, already commented in current source):

1. **Idle check vs inject.** Between the last `waiting_for_input == true`
   check and `inject_text_into_session`, the agent could become busy on an
   unrelated task (e.g. Telegram bridge, a separately-spawned follow-up,
   a user keystroke via xterm). If that happens, the credentials block is
   injected into a busy agent — the paste is queued by ConPTY and Claude
   will process it as soon as it returns to idle. This is the same
   behavior the spawn path accepts (lines 383-408 have the same race).
   **Acceptable**.

2. **Body follow-up interaction.** In the `clear` path, creds inject first,
   then body-followup runs. Because `inject_followup_after_idle_static`
   waits for idle again, it will see Claude busy while processing the
   credentials paste and keep polling. It lands after creds are processed.
   If the cred-inject task times out (30 s), the body-followup still
   attempts — it may succeed if the agent eventually idles — which is the
   pre-existing behavior for `/compact`. **Acceptable**.

3. **Session destruction during poll.** `get_session` is read once at the
   start; the subsequent `list_sessions().find(...)` detects destruction
   each tick. If the session is destroyed after the start snapshot but
   before the inject, the inject will fail with a PTY-write error (handled
   by `inject_text_into_session` return). **Acceptable** — warn-log.

4. **Token rotation.** `Session.token` is immutable post-creation. Snap-
   shotting `token` and `cwd` at task start is safe — no use of stale data.
   **No race**.

5. **Two `/clear`s back-to-back.** Each triggers its own task. Both tasks
   wait for idle; both inject. The second cred-inject is redundant but
   harmless — Claude simply has the cred block twice in context. Not worth
   a dedup lock; `/clear`-spam is not a realistic hot path. **Acceptable**.

---

## 11. Test plan (manual — Phase 1 MVP target)

Run on branch `feature/reinject-credentials-after-clear`.

### 11.1 Prereqs

1. Fresh dev build from branch: `cd repo-AgentsCommander && cargo check`
   then `npm run tauri dev`. Deploy path per global instructions:
   `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone.exe`.
2. Spawn an agent session (Claude) from any coordinator/replica. Confirm
   the initial `# === Session Credentials ===` block appears in the
   agent's context.

### 11.2 Happy path — clear on Claude agent

1. From a peer agent OR from a shell:
   `agentscommander_standalone.exe send --token <TOKEN> --root "<ROOT>" --to "<target-agent>" --command clear`
2. Observe: `/clear` fires in target; Claude clears its context.
3. Within ≤ 30 s of target returning to idle, a new
   `# === Session Credentials ===` block lands and is processed by Claude.
4. Ask the target to `send` back to the caller — it must succeed without
   the user repasting credentials.
5. Log check (app log):
   `[mailbox] Credentials re-injected after /clear (session=<uuid>)`.

### 11.3 Clear + body chain

1. `... --command clear --message "Hola, what is your role?"`
2. Expected ordering in target: `/clear` → idle → cred block → idle →
   `[Message from ...] Hola, what is your role?`.
3. Target responds normally; credentials are present.

### 11.4 Compact does NOT trigger re-inject

1. `... --command compact`
2. Observe: `/compact` fires. `compact` preserves a summary so credentials
   normally survive inside the summary.
3. Log check: NO `Credentials re-injected` line for this message id.
4. If body present, only the pre-existing follow-up body injection should
   log.

### 11.5 Plain shell session is NOT targeted

1. Spawn a plain `powershell` session (no `agent_id`).
2. `... --command clear --to <shell-session-agent-name>` (if addressable;
   otherwise target the session directly by its name).
3. Observe: `/clear` fires (powershell echoes `/clear` as an unknown
   command — expected). NO cred block is written. NO log line about
   credentials.

### 11.6 Busy agent idle-wait

1. Send the target a long-running prompt. While it is working, send
   `--command clear`.
2. `/clear` fires immediately per existing behavior (Claude processes
   `/clear` even mid-prompt).
3. Cred re-inject waits for `waiting_for_input == true` before firing. Log
   should show it landing after idle, not during busy.

### 11.7 Idle timeout

1. Simulate: modify the target agent to never go idle (e.g. run a tight
   loop via a tool), OR mock `waiting_for_input = false` locally for 30+ s.
2. Observe: a `warn` log is emitted with `Timeout waiting for idle before
   credential re-inject (30s)`. No crash. No retry.

---

## 12. Byte-for-byte parity check

After extracting the helper, capture a diff between pre- and post-change
`cred_block` strings at spawn time. They must match:

```
\n
# === Session Credentials ===\n
# Token: <uuid>\n
# Root: <cwd>\n
# Binary: <binary_name>\n
# BinaryPath: <binary_path>\n
# LocalDir: <local_dir>\n
# === End Credentials ===\n
```

Helper's derivation of `binary_name`, `binary_path`, `local_dir` is a
verbatim copy of the original inline code — same `\\?\` stripping, same
fallbacks. Token `Display` formatting via `format!("{token}", token =
token)` is identical to `format!("{token}", token = &token)` for `&Uuid`
(both dispatch to `Uuid`'s `Display`).

**Verification step for dev**: after the edit, temporarily `log::debug!`
the first 256 chars of `cred_block` at spawn site; spawn a new agent; copy
the block; revert to main; spawn an agent; copy the block; `diff` — must
be identical apart from the per-session UUID and cwd.

---

## 13. Build / check sequence for the dev

```bash
cd repo-AgentsCommander
git fetch origin
git checkout feature/reinject-credentials-after-clear
git merge origin/main                 # resolve any drift from main first
cd src-tauri && cargo check           # type-check
cd src-tauri && cargo clippy          # lint
# (no unit tests required — feature is manual-test only)
cd .. && npm run kill-dev
npm run tauri dev                     # run app
# then execute §11 test plan
```

---

## 14. Constraints recap (do NOT do)

- Do NOT alter `commands/session.rs:382-408` idle-poll loop — only
  replace the cred-block derivation (lines 410-453).
- Do NOT change the existing `compact` behavior.
- Do NOT merge to main, push to main, or open a PR. Branch-only.
- Do NOT add a retry loop on idle-timeout. Best-effort, one shot.
- Do NOT bump version for this change unless the dev ships a compilable
  release build — per CLAUDE.md versioning rule it's bumped only on a
  compilable change set, which the dev will do at ship time, not during
  plan application.
- Do NOT introduce `anyhow` — stay on `Result<T, String>` / `Result<T,
  AppError>` per existing patterns.
- Do NOT expose the helper to the frontend or Tauri commands. It is
  backend-internal.

---

## 15. Dev-rust review

**Reviewer**: wg-8-dev-team/dev-rust
**Date**: 2026-04-19
**Branch verified**: `feature/reinject-credentials-after-clear` (HEAD `39f8b7e`)
**Scope**: feasibility, correctness, completeness. No code written yet.

### 15.1 Code references — all verified

| Plan claim | Actual in branch | Status |
|---|---|---|
| `commands/session.rs:374-478` (agent auto-inject block) | Block runs 377-478; lines 410-453 are the inline cred derivation | OK |
| `commands/session.rs:410-453` (cred-block derivation to replace) | `let exe = std::env::current_exe().ok();` at 410; closing `);` at 453 | OK |
| `commands/session.rs:380` (`token = session.token.clone()`) | `let token = session.token.clone();` at line 380 | OK |
| `commands/session.rs:381` (`cwd_clone = cwd.clone()`) | `let cwd_clone = cwd.clone();` at 381 | OK |
| `phone/mailbox.rs:619-623` (race comment — "/clear write at…") | PTY write at **629**; 619-623 is the comment block immediately above | Citation drift, minor |
| `phone/mailbox.rs:651` (end of `message_delivered` emit) | Closing `);` at 651 | OK |
| `phone/mailbox.rs:653-666` (existing body-followup block) | Comments at 653-654, `if !msg.body.is_empty()` at 655, closing `}` at 666 | OK (replacement range inclusive) |
| `phone/mailbox.rs:749-776` (existing idle-poll shape) | `inject_followup_after_idle_static` at 744-783; poll loop at 754-776 | OK |
| `phone/mailbox.rs` insertion "after line 783, before `find_active_session`" | `inject_followup_after_idle_static` ends at 783; `find_active_session` at 787 | OK |
| `Session.agent_id`, `Session.token`, `Session.working_directory` | `session/session.rs:57`, `:76`, `:49` respectively | OK |
| `SessionManager::get_session -> Option<Session>` | `session/manager.rs:165` — returns cloned owned `Session`, NOT a guard | OK — safe to destructure without holding the outer lock into the inject await |
| `SessionManager::list_sessions -> Vec<SessionInfo>` | `session/manager.rs:151` — owned Vec, matches pattern at mailbox:765 | OK |
| Mailbox imports (`Arc`, `Uuid`, `SessionManager`) | `use std::sync::{Arc, Mutex}` at 3; `use uuid::Uuid` at 6; `use crate::session::manager::SessionManager` at 13 | OK — no new imports needed, §7.5 confirmed |
| `pty/mod.rs` (alphabetic mod list) | 4 entries; inserting `credentials` at top preserves order | OK |

### 15.2 Signature: `&Uuid` vs owned `Uuid`

`Uuid` is `Copy` (16 bytes). By-value vs by-reference has identical cost in release builds. Both paths dispatch to the same `Display` impl (`std` provides blanket `impl<T: Display + ?Sized> Display for &T`), so format output is byte-identical.

At both call sites (`commands/session.rs` spawn path, and the new mailbox helper) we already hold an owned `Uuid`, so `&token` is one borrow operator away. **Keep `&Uuid`** — no ergonomic loss, avoids any implicit `Copy` at call sites, keeps intent explicit.

Clippy: `&token` when helper takes `&Uuid` is NOT `needless_borrow` — the borrow is required. No lint risk.

### 15.3 `inject_text_into_session(submit=true)` side effects post-`/clear`

Per `pty/inject.rs:37-101`, `submit=true` for a Claude/Codex/Gemini shell performs:
1. PTY write of the full text block.
2. Sleep 1500 ms.
3. `\r` #1.
4. Sleep 500 ms.
5. `\r` #2 (logged as non-fatal on failure — `[inject] Enter (2/2) failed ... non-fatal`).

Total timeline: ~2000 ms of post-write PTY activity. Evaluated against post-`/clear` state:

- After `/clear`, Claude drops context and returns to an empty input prompt. Paste fills the input; Enter #1 submits it. Enter #2 hits empty input and is harmless per the existing inject.rs comment at lines 69-72. **Safe.**
- No residual `/clear` input-buffer state exists — `/clear` itself is processed and removed from the input buffer by Claude before returning to idle. **Safe.**
- Same sequence is already validated in production at session spawn (`commands/session.rs:455-476`). Reusing it here inherits the same reliability profile. **Safe.**

**Residual concern (accepted)**: false-positive idle between polls (§10.1). The 500 ms poll interval caps the exposure window; best-effort spec tolerates it.

### 15.4 Byte-for-byte parity — Display formatting

Original (line 448): `token = token` where `token: Uuid`.
Helper (line 128 in plan): `token = token` where `token: &Uuid`.

Both resolve through `Uuid: Display` via blanket `impl<T: Display + ?Sized> Display for &T`. Produces identical lowercase-hyphenated 36-char output for any given UUID value. **Parity confirmed by static reading.**

All other string derivations (`binary_name`, `binary_path`, `local_dir`, `\\?\` stripping, fallbacks for `current_exe()` failure) are verbatim copies. No semantic drift.

### 15.5 Clippy / compile

No new warnings expected:
- `concat!(...)` inside `format!(...)` is the identical pattern used at `commands/session.rs:437-453` today — clippy accepts it.
- `std::env::current_exe()` is `std`, no dep change.
- Helper is pure (one `current_exe()` call, no locks, no await, no panics). No async, no `Send`/`Sync` concerns.
- Possible clippy nit `clippy::uninlined_format_args` on `token = token` style — but **keep the named-arg form** to minimize diff vs the original and so the string-template layout remains self-documenting. Existing code uses the same form; clippy is quiet on it.

### 15.6 Error propagation in the new helper

`reinject_credentials_after_clear_static(...) -> Result<(), String>`:
- `inject_text_into_session(...).await?` returns `Result<(), String>` → `?` propagates `String` into outer `Result<(), String>`. Direct type match, no `From` conversion needed.
- Upstream caller (the spawned task in the plan's §7.3 replacement) `if let Err(e) = ...await` consumes the `String` for the `log::warn!`. Compiles cleanly.

### 15.7 Test coverage — RECOMMENDED addition

Plan §11 is manual-only. I am adding ONE unit test to the new `pty/credentials.rs` — worth it because the helper is the single source of truth for a string BOTH call sites depend on, and regressions would silently break agent auth.

Append at end of `src-tauri/src/pty/credentials.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_has_fixed_header_footer_and_embeds_token_and_root() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let block = build_credentials_block(&token, r"C:\example\root");

        assert!(
            block.starts_with("\n# === Session Credentials ===\n"),
            "header missing or altered: {:?}",
            &block[..block.len().min(64)]
        );
        assert!(
            block.contains("# Token: 00000000-0000-0000-0000-000000000001\n"),
            "token line missing/altered"
        );
        assert!(
            block.contains(r"# Root: C:\example\root"),
            "root line missing/altered"
        );
        assert!(
            block.ends_with("# === End Credentials ===\n"),
            "footer missing or altered: {:?}",
            &block[block.len().saturating_sub(64)..]
        );
    }
}
```

**Reasoning**: only `Token` and `Root` are test-deterministic; `Binary`, `BinaryPath`, `LocalDir` derive from `std::env::current_exe()` which varies per test host. The assertions above pin (a) header+footer spelling, (b) trailing `\n` on footer (agents rely on line-terminated parsing), and (c) verbatim pass-through of token + cwd. A regression that drops either line, rewords the delimiter, or swaps the UUID `Display` format will fail this test.

### 15.8 Plan-accuracy nits (non-blocking)

- §10 bullet 1 references "`mailbox.rs:619-623`" as the `/clear` write race site. Actual PTY write is at line **629**; 619-623 is the comment block above. Citation drift only — no behavior impact.
- §7.1 says "immediately after the `message_delivered` emit (line 651) and before the existing body-followup block (line 653)". Line 653 is the comment line; the `if` itself lives at line **655**. Same citation-only drift.

### 15.9 No-op confirmations

- `compact` path: plan §7.3 gates on `let is_clear = command == "clear"`. `compact` skips the cred re-inject branch and falls through to the existing body follow-up exactly as today. **Behavior preserved.**
- Plain shell (no `agent_id`): §7.4 early-returns `Ok(())` silently. **Behavior preserved.**
- Single vs double poll lock holds: the helper nests `session_mgr.read().await` → `mgr.list_sessions().await`, same pattern as `inject_followup_after_idle_static:763-765`. Outer `RwLock<SessionManager>` is never write-locked in practice (writes happen on inner `sessions: RwLock<HashMap>`), so nested reads don't block. **Matches existing precedent.**

### 15.10 Ready to implement

With §15.7 test added, plan is implementable as written. No blockers. No open questions.

**Delta vs architect's plan**: one unit test added in `pty/credentials.rs` per §15.7. Everything else is verification-only.

---

## 16. Grinch review

**Reviewer**: wg-8-dev-team/dev-rust-grinch
**Date**: 2026-04-19
**Branch verified**: `feature/reinject-credentials-after-clear` (HEAD `39f8b7e`)
**Scope**: adversarial plan review — design only, no code exists yet.
**Read**: plan §1–§15, `phone/mailbox.rs`, `commands/session.rs:370-485`, `pty/inject.rs`, `pty/idle_detector.rs`, `session/manager.rs`, `session/session.rs`, `telegram/bridge.rs:790-795`, `lib.rs:180-222`.

### 16.1 — Interleaving inside the 2 s `inject_text_into_session` window (BLOCKER-adjacent)

- **What**: `inject_text_into_session` (`pty/inject.rs:37-101`) acquires `Mutex<PtyManager>` ONLY briefly: once to write the text block, then releases the lock, sleeps 1500 ms, reacquires to write `\r` #1, releases, sleeps 500 ms, reacquires for `\r` #2. Between the text write and Enter #1 there is a **1500 ms window with no lock held**. Any other caller that acquires the PTY mutex during that window writes into the same un-submitted input buffer. At Enter #1 all queued bytes are submitted as one Claude input.
- **Why**: Concrete concurrent writers that go through this same helper: (a) `phone/mailbox.rs:711` standard message path (`[Message from X] body`), (b) `telegram/bridge.rs:792` Telegram user input, (c) the new post-`/clear` re-inject helper itself. Additionally, raw xterm.js keystrokes reach the PTY via the `pty_write` Tauri command which holds the same `Mutex<PtyManager>` — they bypass `inject_text_into_session` entirely and still interleave. Scenario: `/clear` delivered → Claude wipes context → re-inject fires cred block at T=0 → at T=500 ms user types `hola` in xterm (or Telegram user sends input, or another outbox message is picked up by the poller) → PTY input buffer is now `<cred_block>hola` → Enter #1 at T=1500 ms submits the entire concatenation as one prompt. Claude parses a mashed-up input; in best case, it ignores the comments and answers `hola` without creds installed (re-inject silently corrupted); in worst case, Claude interprets the fused block oddly and the session is wedged until the user intervenes.
- **Severity**: The race exists TODAY for the spawn-path cred inject and the Telegram bridge; this plan **adds a third recurring exposure window** (one per `/clear` per agent, potentially hourly in a Claude-heavy workflow).
- **Fix**: Plan must at minimum (a) document this race explicitly in §10 (currently §10 only lists idle/busy races, not same-window interleaving), and (b) call out that the 2 s post-cred-write window is where concurrent deliveries corrupt input. A real fix would hold the PTY mutex across text-write + both Enters, but that is an `inject_text_into_session` rewrite out of scope — the plan should at least name the risk and add a test (§16.10) that tries to reproduce interleaving.

### 16.2 — §12 verification step is a token-leak vector (BLOCKER)

- **What**: Plan §12 "Verification step for dev" instructs: "temporarily `log::debug!` the first 256 chars of `cred_block` at spawn site". The first 256 chars of the cred block are the header + `# Token: <uuid>\n` + `# Root: <cwd>\n`. That line **contains the full session token**.
- **Why**: `log::debug!` is typically wired to file logs (`env_logger`, Tauri app log dir). Logs are frequently copy-pasted into bug reports, shared in support threads, and sometimes shipped off-host. A developer following the plan verbatim, forgetting to revert, or committing the verification line by accident, leaks a valid session Token that grants full CLI access to that agent (send, list-peers, inject). The session token is a shared secret — `Session.token: Uuid` (`session/session.rs:76`) — and is the primary auth to the local daemon.
- **Severity**: High. The current codebase has **zero** cred-content log sites (verified via `grep log::.*cred|log::.*Token|log::.*credential` across `src-tauri/src` — only non-content messages such as `[session] Timeout waiting for idle before credential injection`). This plan introduces the first one, with no mechanism to ensure revert.
- **Fix**: Replace §12's verification recipe with a non-leaking alternative. Options, in order of preference:
  1. Unit test in `pty/credentials.rs` (already added as §15.7) that compares byte-for-byte against a golden fixture string with `{token}` / `{root}` placeholders — no runtime log needed.
  2. If runtime verification is required, log `sha256(cred_block)` (e.g. via `sha2` or a `DefaultHasher`) OR `cred_block.len() + first 32 chars ending before the `# Token` line` — never token bytes.
  3. Dev runs the check locally via a doc test or a `cargo run --example` that prints to stdout (not persisted) and never commits.
  Delete the "first 256 chars" wording from §12 entirely before the dev starts implementing.

### 16.3 — Body follow-up proceeds after cred re-inject fails (CORRECTNESS GAP)

- **What**: Plan §7.3 spawns one task that does (1) cred re-inject, then (2) body follow-up, with the latter gated only on `!msg_clone.body.is_empty()` — it does NOT check whether step (1) succeeded. If cred re-inject times out at 30 s (idle never returned, per §8 timeout matrix), the task still invokes `inject_followup_after_idle_static`, which may itself succeed (or also time out). If body wins, the agent receives `[Message from X] <body>` without credentials.
- **Why**: The feature's stated purpose (§1) is "agent cannot talk to peers until a human repastes them — this plan fixes that". But if a coordinator sends `... --command clear --message "do task Y"` and cred re-inject times out, the target Claude ends up with a task prompt AND no token. It cannot reply to the coordinator, cannot use `send`, and the coordinator is blocked waiting. Plan §10.2 hand-waves this as "same pre-existing behavior for /compact" — but `/compact` is a different contract (summary preserves creds inside the summary); `/clear` does not. The "matches existing behavior" argument does not hold.
- **Fix**: Treat the cred re-inject outcome as a precondition for body delivery on the `/clear` path. Plan §7.3 should branch:
  ```rust
  if is_clear {
      match reinject_credentials_after_clear_static(&app_clone, session_id).await {
          Ok(()) => { /* fall through to body */ }
          Err(e) => {
              log::warn!("[mailbox] Cred re-inject failed, skipping body to avoid leaving agent unable to reply: {}", e);
              return; // do NOT deliver body
          }
      }
  }
  if !msg_clone.body.is_empty() { /* body inject */ }
  ```
  Alternatively: inject body with a prepended warning marker `"[Message from X] WARNING: credentials were not re-injected after /clear; you cannot reply until a human repastes them.\n{body}"`. The current plan silently degrades; either path above is explicit. Document the chosen behavior in §7.2 ordering semantics.

### 16.4 — Duplicate creds from back-to-back `/clear`s: §10.5 reasoning is wrong, conclusion is right

- **What**: Plan §10 case 5 claims two concurrent `/clear`s produce "two cred blocks in context, harmless". Analysis of `mailbox.rs:597-617` precondition: `/clear` delivery REQUIRES `waiting_for_input == true`. While Clear #1 is being delivered / processed / cred-injected / ACK'd, `waiting_for_input` is false (Claude is busy). Clear #2 is REJECTED with `"agent is busy"` and retried by the outbox poller. By the time Clear #2 can land, Clear #1's cred block is already in-context or already timed out.
- **Why**: The harmful "two creds in one context" state is **structurally unreachable** because the precondition serializes `/clear` deliveries. No race. Plan §10.5 reasoning ("each triggers its own task... second is redundant but harmless") is factually incorrect — there is no second cred block in a single context window.
- **Fix**: No behavioral change needed. Rewrite §10.5 to: "Back-to-back `/clear`s are serialized by the idle precondition at `mailbox.rs:609`. Clear #2 cannot land until Claude is idle, which requires Clear #1's cred re-inject to have completed (or timed out). Only one cred block ever coexists in a given context window. No race."

### 16.5 — Idle-detection flapping vs 500 ms poll

- **What**: `IdleDetector` (`pty/idle_detector.rs:7-8`) uses `IDLE_THRESHOLD=2500ms` and `CHECK_INTERVAL=500ms`. `on_busy` fires on the first byte of activity; `on_idle` fires only after 2500 ms of PTY quiet. The re-inject helper polls `waiting_for_input` at 500 ms. In principle the 2500 ms quiet-threshold prevents narrow busy spikes from being missed, but there is a specific window worth naming.
- **Why**: Scenario: Claude finishes `/clear`, emits last byte at T=0. At T=2500ms idle_detector's callback fires → `tauri::async_runtime::spawn` → acquires outer `RwLock<SessionManager>.read()` → `mark_idle`. That inner write is an async `RwLock` acquisition which can be delayed under load. Re-inject task, on its 500 ms tick, reads `waiting_for_input` — if it polls during the scheduling gap between `on_idle` firing and `mark_idle` completing, it still sees `waiting_for_input == false`. Acceptable (extra poll cycle, 500 ms extra latency). Not a correctness issue — but plan §10 should mention this latency path so no one is confused if the log shows a 500 ms cred-inject lag after `[idle] IDLE …`.
- **Fix**: Non-blocking. Add a bullet to §10 acknowledging the `on_idle` callback → `mark_idle` scheduling path can delay the observable `waiting_for_input` transition by up to one poll tick.

### 16.6 — Memory / task-lifetime safety: no issue found

- **What**: Reviewed `tauri::async_runtime::spawn(async move { ... })` pattern in §7.3.
- **Why safe**: `AppHandle` is internally Arc-counted; cloning is cheap. The spawned task captures `app_clone`, `msg_clone` (`OutboxMessage` is `Clone`), `session_id` (`Copy`), `is_clear` (`bool`, `Copy`), `command_owned` (owned `String`) — all owned / cheaply cloned. No borrowed references escape the task. `tokio::sync::RwLock` does not poison on panic. `std::sync::Mutex<PtyManager>` poisons on panic while locked, but the new helper does not hold that lock itself — it only calls `inject_text_into_session` which briefly acquires it per write, and the helper's own code is panic-free (no `unwrap` on fallible ops, `std::env::current_exe()` uses `.ok()`, all formatting is total). JoinHandle is dropped (fire-and-forget) — panics inside the task would be silently swallowed, which matches existing `inject_followup_after_idle_static` behavior. No new leak, no poison risk, no stuck-reference. **No issue found.**

### 16.7 — Session resolution correctness: no material issue found

- **What**: Reviewed `get_session` (`session/manager.rs:165`) and `list_sessions` (`:151`) semantics.
- **Why safe**: `get_session` reads `self.sessions.read()` and clones — owned `Session` returned, no guard held. `list_sessions` reads `sessions` + `order` and filter_maps. Disagreement window: in `create_session` (`:62-63`) the session is inserted into `sessions` before being pushed to `order` — a new session exists in `get_session` but is absent from `list_sessions` for the duration of those two awaits. That window is microseconds and does not affect the re-inject flow (the cred re-inject task is spawned AFTER the `/clear` delivery, after the target session is fully constructed and in both maps). No other lock-upgrade. Field snapshot at task start (`token`, `cwd`) is safe — `Session.token` and `Session.working_directory` are immutable post-creation (verified: neither is mutated by any `SessionManager` method). **No issue found.**

### 16.8 — Windows `current_exe` edge cases

- **What**: Plan's helper (§4) uses `std::env::current_exe().ok()`, `to_string_lossy()`, and `strip_prefix(r"\\?\")`. Three concerns:
  1. **Non-UTF-8 exe path**: `to_string_lossy()` replaces invalid UTF-16 surrogates with U+FFFD. The agent receives a `BinaryPath: C:\…\u{FFFD}…\agentscommander_mb.exe` which is a broken path when the agent tries to invoke it. Silent corruption.
  2. **`current_exe()` returns Err**: Falls back to literal `"agentscommander"` / `"agentscommander.exe"` — almost certainly wrong for this user (real binary is `agentscommander_mb.exe` per Session Credentials block). Agent will be unable to invoke the CLI.
  3. **Symlinked or junction-pointed exe**: `current_exe()` resolves symlinks on most platforms. Not a bug, but the resolved path may differ from the path the user expects.
- **Why**: Case 1 is extremely rare on typical Windows setups (NTFS permits non-UTF-8 filenames but user directories are conventionally ASCII; localized paths are valid UTF-16 → valid UTF-8). Case 2 is exceptional (exe deleted / renamed at runtime). The existing spawn-path code at `commands/session.rs:410-435` has the SAME behavior today — this plan preserves parity, not a regression. BUT: this is the FIRST opportunity to improve observability.
- **Fix**: Low-priority, not a blocker. Recommend the helper log a single `log::warn!("[credentials] current_exe() unavailable, using fallback names")` when `current_exe()` returns Err, so operators can diagnose a mysterious "agent can't find its binary" bug. Do NOT log the path when it succeeds (not useful, no leak). Skip unless cheap.

### 16.9 — Versioning policy ambiguity

- **What**: `CLAUDE.md` says "Bump at minimum the patch version on every compilable change set" across 3 files. Plan §14 defers to "ship time" ("the dev will do at ship time, not during plan application"). Dev-rust §15 did not flag this.
- **Why**: Two readings of "change set" are possible: (a) per commit / per compilable state → plan violates by deferring; (b) per release → plan OK. CLAUDE.md's wording is not self-clarifying. Risk: whichever interpretation the dev picks may conflict with tech-lead / user expectation, creating rework.
- **Fix**: Pick one and stick to it for this branch. My recommendation: bump patch as part of the dev's first commit on this branch (atomic with the feature), since the branch is intended to produce a compilable change. Update CLAUDE.md (out of scope for THIS plan) to disambiguate the rule. For now, add one line to §14: "Dev: bump `tauri.conf.json`, `Cargo.toml`, `Titlebar.tsx` patch version atomically with the feature commit — per CLAUDE.md § Versioning."

### 16.10 — Test plan holes (§11)

Missing cases that should be added before sign-off:

1. **Concurrent Telegram input during cred re-inject window** — with a Telegram-wired agent session, send `/clear` AND simultaneously send a Telegram message. Expect: either both land cleanly (cred block submitted, user message treated separately) OR fail observably. Verify no token-leak in log and no hung session. Directly probes §16.1.
2. **User xterm keystrokes during cred re-inject window** — open the target session in the AC terminal window, send `/clear` from a peer, within 500 ms of the cred block appearing start typing in xterm. Probes `pty_write` interleaving.
3. **Concurrent `/clear` from two peers** — peer A and peer B each `send --command clear --to <target>` within 1 s. Verify precondition serializes: only one `/clear` lands at a time, each followed by its own cred re-inject. Only ONE cred block should ever be in context at once.
4. **Cred re-inject timeout path with body** — simulate an agent stuck busy (long-running tool call) for 30+ s, then `send --command clear --message "task"`. Verify per §16.3 chosen policy: either body is skipped (recommended) or body is delivered with a warning marker. Observe exact log sequence.
5. **Token-leak grep** — after ANY happy-path test, `grep <session-token-uuid>` against the app log. Expected: zero hits. Catches regressions that add cred content to logs (including any stray §12 debug line).
6. **Restored session from persistence** — restart AC between spawn and `/clear`. Verify that after restore, `/clear` + re-inject still produces a credentials block whose `Token:` matches `Session.token` in `sessions.toml` (NOT a rotated token).
7. **Session destruction mid-cred-inject** — hit `/clear`, and while the cred block is in the 2 s paste window (between text write and Enter #2), destroy the session from the sidebar. Verify: warn-log appears, no panic, no orphan task holds state after destruction. Probes §10.3 mid-`inject_text_into_session` destruction.
8. **Plain shell session with `agent_id: Some(...)`** — spawn a shell session that was wired to an agent id for some reason but is not a coding agent (edge case, unlikely). Verify §7.4 treats it per `agent_id.is_some()` gate, NOT by shell type. If this edge is impossible by construction, note that in the plan.

### 16.11 — Task cancellation safety (bonus, beyond tech-lead's 10)

- **What**: If the Tauri runtime is dropped (app quit) while the re-inject task is between `text write` and `Enter #2`, the task is dropped mid-await. The PTY input buffer is left with an uncommitted cred block. On next app launch the buffer is gone (fresh session); on the SAME session (if somehow retained), the next `\r` submits the leftover cred block.
- **Why**: Practically rare — app quit mid-`/clear` is uncommon. Observable impact: if user restarts the app fast enough to retain the PTY, the NEXT keypress in xterm could submit a stale cred block with an outdated token. Logs show no trace because the write happened.
- **Fix**: Non-blocking. Document in §10 that cred re-inject is best-effort and not cancellation-safe: on app shutdown mid-inject, the paste may remain uncommitted in the PTY input buffer. If the dev wants to harden, wrap the helper in a `tokio::select!` against the existing `ShutdownSignal` (see `lib.rs:222`, pattern used by `MailboxPoller`). Out of scope for this plan unless the user requests it.

### 16.12 — §15.7 unit test is under-specified (nit)

- **What**: Dev-rust's added test (§15.7) only asserts presence of header, `# Token:`, `# Root:`, and footer. It does NOT pin:
  - The **leading `\n`** before the header (agents depend on this to ensure the block starts on a fresh line — without it, a trailing character from a previous output can glue the `#` onto the prior line and break parsing).
  - The **order of lines** (if `# Root:` is moved before `# Token:`, the test still passes).
  - The **absence of extra lines** (if a future regression adds `# Debug: ...` between Token and Root, the test still passes).
- **Why**: The whole point of extracting the helper is byte-for-byte parity. A regression that subtly reorders or extends the block would pass the test while potentially breaking agent parsing on the other side.
- **Fix**: Strengthen §15.7's test to use a golden-template comparison:
  ```rust
  let expected = format!(
      "\n# === Session Credentials ===\n# Token: {token}\n# Root: {root}\n# Binary: {binary_name}\n# BinaryPath: {binary_path}\n# LocalDir: {local_dir}\n# === End Credentials ===\n",
      token = "00000000-0000-0000-0000-000000000001",
      root = r"C:\example\root",
      // binary_name / binary_path / local_dir are derived — extract them from the actual block and splice in
  );
  ```
  Or simpler: split `block` by `\n`, assert the exact sequence of 8 expected line prefixes in order (`""`, `"# === Session Credentials ==="`, `"# Token: "`, `"# Root: "`, `"# Binary: "`, `"# BinaryPath: "`, `"# LocalDir: "`, `"# === End Credentials ==="`, `""`). This pins order and line count.

---

### Verdict summary

**Blockers (must be addressed before implementation):**

- **§16.2** — delete the §12 "log first 256 chars" verification recipe. It leaks tokens.
- **§16.3** — specify whether body follow-up aborts on cred re-inject failure (strongly recommend abort). Silent body-without-creds delivery partially defeats the feature.

**Should-fix (recommended before implementation, not strict blockers):**

- **§16.1** — at minimum document the 2 s cred-paste / concurrent-writer interleaving race in §10. Without it, plan §10's race inventory is incomplete.
- **§16.4** — rewrite §10.5 reasoning. Conclusion stands, justification is wrong.
- **§16.10** — add test cases #1, #4, and #5 (Telegram interleave, cred-timeout body policy, token-leak grep). #5 is cheap and would catch §16.2-class regressions automatically.

**Nits (safe to apply as polish):**

- **§16.5** — annotate §10 with the `on_idle` → `mark_idle` scheduling gap.
- **§16.8** — optional warn-log on `current_exe()` Err fallback.
- **§16.9** — pin a versioning stance for this branch (inline bump or deferred, pick one).
- **§16.11** — note cancellation-safety caveat in §10.
- **§16.12** — strengthen §15.7 test to pin line order and count.

**Approved vectors (no issue found):**

- **§16.6** — memory / task lifetime safety.
- **§16.7** — session resolution correctness.
- **§16.4** — duplicate creds in practice (conclusion only; reasoning needs rewrite).

**Overall**: Plan is NOT yet ready to hand to a dev. Two blockers (§16.2, §16.3) require plan edits — both small textual changes, no redesign. Once those land, plan is implementable.

---

## 17. Round 2 — architect response

**Author**: wg-8-dev-team/architect
**Date**: 2026-04-19
**Status**: Addresses grinch §16 findings. Preserves §1–§16 verbatim. Amendments in this section **supersede** the referenced earlier-section text for the dev's purposes — read §17 as authoritative where it overlaps.

### 17.1 Decisions

| ID | Pick | Rationale |
|---|---|---|
| **B1** (§16.2 token-leak) | **Option C** — no runtime cred-content logging. Rely on §15.7 unit test (strengthened per 17.7) as sole verification. | Cheapest, zero leak risk, test-gated regressions. Matches tech-lead lean. |
| **B2** (§16.3 body w/o creds) | **Policy A** — abort body follow-up when cred re-inject fails on the `clear` path. Warn-log and return. | Best-effort spirit. A body without creds defeats the feature's stated purpose (§1): agent gets a task it cannot reply to. |

### 17.2 Amendment to §7.3 — control-flow update (B2)

The spawned task body in §7.3 is replaced with:

```rust
            let is_clear = command == "clear";
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            tauri::async_runtime::spawn(async move {
                if is_clear {
                    match Self::reinject_credentials_after_clear_static(
                        &app_clone,
                        session_id,
                    )
                    .await
                    {
                        Ok(()) => { /* creds in — fall through to body */ }
                        Err(e) => {
                            log::warn!(
                                "[mailbox] Cred re-inject after /clear failed (session={}): {} \
                                 — skipping body follow-up to avoid delivering a message the \
                                 agent cannot reply to",
                                session_id,
                                e
                            );
                            return;
                        }
                    }
                }
                if !msg_clone.body.is_empty() {
                    if let Err(e) = Self::inject_followup_after_idle_static(
                        &app_clone,
                        session_id,
                        &msg_clone,
                    )
                    .await
                    {
                        log::warn!(
                            "[mailbox] Follow-up body injection after /{} failed (session={}): {}",
                            command_owned,
                            session_id,
                            e
                        );
                    }
                }
            });
```

**Behavioral contract (updates §7.2)**:

- `/clear` path: body is delivered **iff** cred re-inject succeeds. On cred timeout or error, body is dropped. Sender is NOT notified of the drop (best-effort) but the warn-log identifies the dropped message by `session_id`. Senders relying on body delivery should avoid chaining `--command clear --message ...` when re-inject reliability matters — they can send the body as a separate `send` call after confirming the target responded.
- `/compact` path: unchanged. Body follow-up fires regardless (compact preserves creds in the summary, so there is no preconditioning).
- Non-`clear` commands never enter the re-inject branch.

### 17.3 Amendment to §10 — race inventory additions

Append these cases to the race/TOCTOU inventory. They **supersede** §10 case 5 (SF2) and extend the list (SF1, nits 16.5 + 16.11).

**§10 case 5 (rewrite per SF2 / §16.4)**:

> 5. **Two `/clear`s back-to-back.** Structurally serialized by the idle precondition at `mailbox.rs:609`. Clear #2 requires `waiting_for_input == true`; while Clear #1 is being delivered / processed / cred-re-injected, Claude is busy and `waiting_for_input == false`. Clear #2 is rejected with `"agent is busy (not idle)"` and retried by the outbox poller. By the time Clear #2 can land, Clear #1's cred block is already in-context or has timed out. **Only one cred block ever coexists in a single context window. No race.**

**New §10 case 6 (SF1 / §16.1)** — `inject_text_into_session` interleave window:

> 6. **2 s interleave window inside `inject_text_into_session`.** Per `pty/inject.rs:37-101`, the helper writes the text block, **releases** `Mutex<PtyManager>`, sleeps 1500 ms, reacquires for Enter #1, releases, sleeps 500 ms, reacquires for Enter #2. Any other writer that acquires the PTY mutex between the text write and Enter #1 — e.g. the standard message path (`mailbox.rs:711`), Telegram input (`telegram/bridge.rs:792`), or raw xterm keystrokes via `pty_write` — writes into the same un-submitted input buffer. Enter #1 then submits the concatenation as one Claude input. This race exists **today** for the spawn-path cred inject and all other `submit=true` paths; this plan adds a third recurring exposure window (one per `/clear` on an agent). Fixing the race is out of scope — it requires holding the PTY mutex across all three writes, an `inject_text_into_session` rewrite. **Documented, not fixed.** Mitigation: the interleave usually produces a malformed Claude prompt rather than a silent cred substitution; the test plan §11.9 probes it. **Acceptable (parity with existing risk).**

**New §10 case 7 (nit 16.5)** — `on_idle` → `mark_idle` scheduling gap:

> 7. **Idle-detector → `mark_idle` scheduling latency.** `IdleDetector` fires `on_idle` after 2500 ms of PTY quiet; the callback spawns a task that takes the outer `SessionManager` read lock to call `mark_idle`. Under load this completion can lag the callback by up to one poll tick. The re-inject poll at 500 ms may therefore see `waiting_for_input == false` for one extra tick after the detector fired. Net effect: up to 500 ms additional latency before cred re-inject starts. **Not a correctness issue.** Logs may show `[idle] IDLE ...` followed by a ≤500 ms gap before `[mailbox] Credentials re-injected ...` — this is expected.

**New §10 case 8 (nit 16.11)** — cancellation safety on app shutdown:

> 8. **Cancellation-safety on app shutdown.** The re-inject task is not cancellation-wrapped against the global shutdown signal (see `lib.rs:222` / `MailboxPoller` pattern). If the Tauri runtime is dropped between the text write and Enter #2, the task is dropped mid-await. The PTY input buffer is left with an uncommitted cred block; on normal restart the buffer is gone (new session / new PTY). If the same PTY is somehow retained, the next `\r` submits the stale cred block. **Rare and best-effort acceptable**; not fixed. A future hardening can wrap the helper in `tokio::select!` against the shutdown signal.

### 17.4 Amendment to §11 — test plan additions (SF3 + nit 16.10)

Append these cases to §11 after §11.7. They supersede §11's "this is the complete test list".

**§11.8 — Telegram interleave during cred-inject window (probes §10.6)**

1. Target session is Telegram-wired (Telegram bridge attached).
2. From a peer: `send --command clear --to <target>`.
3. Within 500 ms of `/clear` landing, send a Telegram user message to the agent.
4. Expected: either (a) both land cleanly — cred block submitted, Telegram message submitted separately on its own line — or (b) the interleaved input is visibly malformed in Claude's transcript but neither hangs the session nor corrupts the token.
5. Verify no session hang: target returns to idle within 30 s.
6. Verify `§11.10` token-leak grep is clean.

**§11.9 — xterm keystroke interleave during cred-inject window**

1. Open the target session in the AC terminal window.
2. From a peer: `send --command clear --to <target>`.
3. Within 500 ms of the cred block appearing, type a short string in xterm.
4. Expected: same as §11.8 — no hang, no token-leak, session recoverable.

**§11.10 — Cred-timeout with body (probes B2 Policy A)**

1. Keep the target agent busy with a long-running tool call (>35 s).
2. Another peer: `send --command clear --message "task Y" --to <target>`.
3. `/clear` is rejected upfront by the idle precondition at `mailbox.rs:609` → reset. Retry once the agent idles? **Revised setup**: trigger `/clear` while agent is idle (accepts), then induce busy state within 100 ms (e.g. spawn a heavy second prompt into the target via xterm). Cred re-inject poll will time out after 30 s.
4. Expected per B2 Policy A: warn-log `"Cred re-inject after /clear failed ... skipping body follow-up"`. Target does NOT receive `[Message from X] task Y`.
5. Verify: no body marker in agent context.

**§11.11 — Token-leak regression grep (nit 16.10 #5, protects §16.2)**

Run after every test case in §11.1 through §11.10:

1. Capture the full app log for the test window (Tauri app log dir, usually `%LOCALAPPDATA%\Agents Commander\logs\` — adapt to deploy path).
2. Grep the log for the target session's `Token` UUID value (pull from the initial spawn cred-block visible in the agent's context).
3. Expected hit count: **0**. Any match is a regression and MUST be triaged before ship.
4. Also grep for the literal strings `# Token:` and `# === Session Credentials ===` — expected 0 hits for each.

**§11.12 — Restored session across restart**

1. Spawn an agent session. Note the `Token` value.
2. Quit AC; relaunch.
3. Session restores from `sessions.toml`. Verify the agent context still has the original cred block.
4. `send --command clear --to <restored-target>`.
5. Verify the NEW cred block emitted post-`/clear` has the **same** `Token` (no rotation — §6.6 invariant).

**§11.13 — Session destruction mid cred-inject (probes §10.3 + §10.8)**

1. `send --command clear --to <target>`.
2. Within the 2 s paste window (after the text-write, before Enter #2 is confirmed in logs), destroy the target session from the sidebar.
3. Expected: warn-log `"Session ... destroyed during credential re-inject poll"` OR a PTY-write error from `inject_text_into_session`. No panic. No orphan task state. No token in log.

**§11.14 — Plain shell session is NEVER re-injected (§7.4 gate)**

Re-run §11.5 additionally asserting: no `[mailbox] Credentials re-injected ...` line in app log for the `clear` send targeting a plain shell session (even one unusually configured with `agent_id: Some(...)` — the gate is on `agent_id.is_some()`, not on shell type).

### 17.5 Amendment to §12 — strike runtime verification (B1 Option C)

Replace the entirety of §12 with:

> ## 12. Byte-for-byte parity check (Option C — test-gated only)
>
> Parity is enforced by the `pty/credentials.rs` unit test introduced in §15.7 and **strengthened per §17.7 below**. Any regression that reorders lines, reworks delimiters, drops the leading `\n`, or changes `Uuid`/`cwd` pass-through fails the test at `cargo test`.
>
> **No runtime verification recipe is permitted.** Do NOT add `log::debug!`, `log::info!`, `println!`, or any other sink that emits cred-block content at runtime — the first 256 chars contain the session `Token`. The helper's output must never be logged, persisted, or displayed outside the target agent's PTY.
>
> Dev confirmation step: run `cargo test -p agentscommander --lib build_credentials_block` (or equivalent `cargo test pty::credentials`) and observe the unit test passes. That is the complete parity check.

### 17.6 Amendment to §4 — `current_exe()` Err observability (nit 16.8, cheap)

Append to the helper body in §4:

```rust
pub fn build_credentials_block(token: &Uuid, cwd: &str) -> String {
    let exe = std::env::current_exe().ok();
    if exe.is_none() {
        log::warn!(
            "[credentials] current_exe() unavailable — cred block will use fallback \
             binary name. Agent may be unable to invoke the CLI."
        );
    }
    // ... (rest of helper unchanged from §4)
```

Single `log::warn!`, no path or token content. Safe for logs. Useful for diagnosing the rare "agent can't find its binary" failure mode.

### 17.7 Amendment to §15.7 — strengthen unit test (nit 16.12)

Replace the test body in §15.7 with one that pins **line order**, **line count**, and **leading newline**:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_structure_is_byte_stable() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let block = build_credentials_block(&token, r"C:\example\root");

        // Split on '\n' (keep empty trailing element → final "" after last \n).
        let lines: Vec<&str> = block.split('\n').collect();

        // Expected structure, in order (9 elements: 8 \n-terminated lines + trailing empty):
        //  0: ""                                   (leading \n)
        //  1: "# === Session Credentials ==="
        //  2: "# Token: 00000000-0000-0000-0000-000000000001"
        //  3: "# Root: C:\example\root"
        //  4: "# Binary: <runtime-derived>"
        //  5: "# BinaryPath: <runtime-derived>"
        //  6: "# LocalDir: <runtime-derived>"
        //  7: "# === End Credentials ==="
        //  8: ""                                   (trailing \n)
        assert_eq!(lines.len(), 9, "line count drift: {}", lines.len());
        assert_eq!(lines[0], "", "missing leading newline");
        assert_eq!(lines[1], "# === Session Credentials ===");
        assert_eq!(lines[2], "# Token: 00000000-0000-0000-0000-000000000001");
        assert_eq!(lines[3], r"# Root: C:\example\root");
        assert!(lines[4].starts_with("# Binary: "), "Binary line prefix");
        assert!(lines[5].starts_with("# BinaryPath: "), "BinaryPath prefix");
        assert!(lines[6].starts_with("# LocalDir: "), "LocalDir prefix");
        assert_eq!(lines[7], "# === End Credentials ===");
        assert_eq!(lines[8], "", "missing trailing newline");
    }
}
```

**Dev-rust note**: this supersedes the presence-only assertions in §15.7. The new test fails on: line reorder, missing leading `\n`, missing trailing `\n`, extra or missing lines, rewording of any header/footer/field prefix, or any change in token/cwd rendering.

### 17.8 §16.9 — versioning policy: deferred to user

Per tech-lead direction, this nit is not resolved in this plan round. Dev should follow CLAUDE.md literally on first commit (bump patch) **unless** user or tech-lead instructs otherwise before the dev's first commit lands. No plan change here.

### 17.9 Summary of changes in Round 2

| Finding | Action | Where |
|---|---|---|
| B1 — token leak (§16.2) | Delete runtime-verification recipe. Test-gated only. | §17.5 (replaces §12) |
| B2 — body without creds (§16.3) | Policy A: abort body when cred re-inject errs. | §17.2 (replaces §7.3 task body) + §17.1 |
| SF1 — 2 s interleave (§16.1) | Document in §10 as case 6. | §17.3 |
| SF2 — §10.5 reasoning (§16.4) | Rewrite case 5. | §17.3 |
| SF3 — test plan additions (§16.10) | Add §11.8–§11.14. | §17.4 |
| Nit 16.5 | Document `on_idle`→`mark_idle` gap as §10 case 7. | §17.3 |
| Nit 16.8 | Warn-log on `current_exe()` Err. | §17.6 |
| Nit 16.9 | Deferred to user. | §17.8 |
| Nit 16.11 | Document cancellation-safety as §10 case 8. | §17.3 |
| Nit 16.12 | Strengthen §15.7 test (order + count + \n). | §17.7 |

### 17.10 Plan-edit hygiene note

§§1–16 intentionally left untouched per tech-lead instruction. The dev MUST consult §17 **alongside** §§1–16 — where §17 supersedes, §17 wins. A future Round 3 (if any) should either merge §17 back into §§1–16 as a clean revision or continue the numbered-response pattern.

---

## 18. Round 2 — dev-rust sign-off

**Reviewer**: wg-8-dev-team/dev-rust
**Date**: 2026-04-19
**Branch verified**: `feature/reinject-credentials-after-clear` (HEAD `39f8b7e`)
**Scope**: verify §17 amendments compile, match Rust semantics, preserve §15 findings, and are implementable as-written.

### 18.1 §17.2 control-flow — PASS (compiles, minimal diff, semantics correct)

Walked through the replacement task body line-by-line against Rust semantics:

- **Captures**: `app_clone`, `msg_clone`, `session_id` (Copy), `is_clear` (Copy `bool`), `command_owned` — all owned or Copy; `async move` is legal. No borrow escapes the closure.
- **`match ... { Ok(()) => { /* comment */ } Err(e) => { ...; return; } }`** — the `Ok(())` arm is an empty block evaluating to `()`; the `Err` arm ends in `return;` which is `!` and unifies with `()`. The whole `match` is a unit-typed statement. **Compiles.**
- **`return;` inside `async move { ... }`** — exits the spawned future early with `()` (the async block's natural return). It does NOT bypass the spawn; the JoinHandle resolves normally. **Semantically correct** — exactly the "skip body follow-up on cred-fail" contract stated in §17.1 B2 Policy A.
- **`log::warn!` format string with `\<newline>` continuation** — Rust string literals do support `\<newline>` to escape both the newline and any leading whitespace on the next line. The message collapses to a single line at compile time. **Compiles.**
- **Borrow check for `command_owned`**: captured by move into `async move`; referenced only inside the body-path `log::warn!` via `{}` (which borrows). Lives for the closure's lifetime. No move-after-use. If `is_clear && body.is_empty()`, `command_owned` is never read — but the reference exists syntactically, so the compiler does not flag "unused". **No warnings.**
- **`&app_clone` passed to both `reinject_credentials_after_clear_static` and `inject_followup_after_idle_static`** — both take `&AppHandle`, which is `Clone` (Arc-backed) and not consumed by the call. Sequential usage safe. **No lifetime issue.**
- **Flow equivalence**: when `is_clear == false`, the whole first `if` is skipped (compact / non-command paths), and the body branch runs unchanged from the pre-§17 behavior. **Compact path preserved** exactly.

The switch from `if let Err(...)` (original §7.3) to `match` (new §17.2) is the minimal-diff way to express the early-return semantics. An `if let Err(e) = ... { log::warn!(...); return; }` form would also compile, but `match` makes the Ok-then-continue branch structurally explicit. I prefer `match` here. No alternative worth proposing.

**Verdict**: §17.2 is correct and minimal. No change requested.

### 18.2 §17.7 unit test — PASS (implementable as-written, count verified)

Verified the element count by walking the format string in §4:

```
 "\n",                                   ← 1 \n (leading)
 "# === Session Credentials ===\n",      ← 1 \n
 "# Token: {token}\n",                   ← 1 \n
 "# Root: {root}\n",                     ← 1 \n
 "# Binary: {binary}\n",                 ← 1 \n
 "# BinaryPath: {binary_path}\n",        ← 1 \n
 "# LocalDir: {local_dir}\n",            ← 1 \n
 "# === End Credentials ===\n",          ← 1 \n (trailing)
```

Total `\n` = 8. `str::split('\n')` returns `count('\n') + 1 = 9` elements. Expected layout:

| idx | content |
|---|---|
| 0 | `""` (before leading `\n`) |
| 1 | `# === Session Credentials ===` |
| 2 | `# Token: 00000000-0000-0000-0000-000000000001` |
| 3 | `# Root: C:\example\root` |
| 4 | `# Binary: <runtime-derived>` |
| 5 | `# BinaryPath: <runtime-derived>` |
| 6 | `# LocalDir: <runtime-derived>` |
| 7 | `# === End Credentials ===` |
| 8 | `""` (after trailing `\n`) |

Matches §17.7 assertions exactly. **Counts check out.**

Additional checks:

- **Windows path**: `r"C:\example\root"` is a raw string — backslashes are literal. `lines[3]` comparison with `r"# Root: C:\example\root"` also raw → matches byte-for-byte. No escape glitch.
- **`Uuid::parse_str` import**: already reachable via `super::*` (imports `build_credentials_block` + `uuid::Uuid`). No extra `use` statement required beyond `use super::*;`.
- **No extra deps**: `uuid` is already a workspace dep; `Uuid::parse_str` is `const`-unstable but usable at runtime. `#[test]` fns are fine. **No `Cargo.toml` edit needed.**
- **Prefix-only assertions** on `lines[4..=6]` correctly accommodate the runtime-variable `Binary`/`BinaryPath`/`LocalDir` fields without sacrificing the leading-colon contract.
- **Failure modes covered**: line reorder (full-match fails), missing leading `\n` (`lines[0] != ""`), missing trailing `\n` (`lines[8] != ""`), extra line (`lines.len() != 9`), missing line (also `len != 9`), any header/footer/field prefix drift (direct `assert_eq!`).

**Zero friction to implement.** No change requested.

### 18.3 §17 vs §15 compatibility — PASS (no contradictions)

Cross-checked each §15 finding against §17 amendments:

| §15 finding | §17 impact | Status |
|---|---|---|
| §15.1 code-reference table | Unchanged. §17 does not rewrite the cited line ranges except §7.3 (now superseded by §17.2 with its own code block). | Still valid |
| §15.2 `&Uuid` verdict | §17 preserves `&Uuid` signature (§4 unchanged, §17.6 only appends a `log::warn!` inside the helper). | Still valid |
| §15.3 `submit=true` safety | §17.3 adds §10 case 6 documenting the 2 s interleave window — complements §15.3 rather than contradicting it. Grinch's broader concurrent-writer race is now acknowledged in plan. | Strengthened |
| §15.4 parity | §17.5 strikes the runtime log recipe; parity enforced by strengthened §17.7 test. Parity claim survives in stronger form. | Strengthened |
| §15.5 clippy | §17.6's single `log::warn!` adds no lint risk. No new warnings expected. | Still valid |
| §15.6 error propagation | §17.2 replaces `if let Err` with `match` — same `?`-free, String-typed error flow at the leaves (`inject_text_into_session` still propagates `String`). | Still valid |
| §15.7 unit test | Superseded by §17.7 (strict superset of §15.7's assertions). | Replaced, consistently |
| §15.8 citation nits | §17.3 rewrites §10 case 5 only; §10 case 1's "619-623" reference is NOT rewritten by §17. Still a citation drift, still non-blocking. | Still valid (unchanged) |
| §15.9 no-op confirmations (compact, plain-shell, lock pattern) | §17.2 preserves compact path (gated on `is_clear`); §7.4 plain-shell gate unchanged; lock pattern unchanged. | Still valid |

**Nothing §17 adds contradicts §15.** The two rounds compose cleanly.

### 18.4 §17.5 (B1 Option C — no runtime cred logging) — ACCEPT

Zero objection. Strongly agree. From the implementer seat:

- The §15.7/§17.7 unit test is strictly more reliable than any `log::debug!` recipe: it runs on every `cargo test` and catches regressions automatically, without any "remember to revert" burden.
- The §17.5 prohibition ("do NOT add `log::debug!`, `log::info!`, `println!`, or any other sink that emits cred-block content at runtime") matches existing codebase hygiene — `grep -R "log::.*Token\|log::.*cred\|log::.*credential" src-tauri/src` at HEAD `39f8b7e` shows ZERO cred-content log sites today. Plan preserves that invariant. ✓
- §11.11 token-leak grep (in §17.4) turns the invariant into a test assertion. Good defense-in-depth.

**Accept verbatim.**

### 18.5 §17.8 (versioning deferred) — ACCEPT with one concrete concern

From the implementer seat, I accept the deferral. But **flagging one actionable concern before the first commit**:

CLAUDE.md §Versioning requires syncing THREE files atomically on every compilable change:
1. `src-tauri/tauri.conf.json` → `"version"` ← dev-rust can edit (backend)
2. `src-tauri/Cargo.toml` → `version` ← dev-rust can edit (backend)
3. `src/sidebar/components/Titlebar.tsx` → `APP_VERSION` ← **dev-rust MUST NOT edit** per my role (`Role.md`: "Modify frontend code (TypeScript, CSS, HTML) — that's dev-webpage-ui's domain").

So my first commit on this branch cannot satisfy CLAUDE.md's literal 3-file sync without crossing role boundaries. Options:

- **A** (recommended): dev-rust bumps `tauri.conf.json` + `Cargo.toml` atomically with the feature commit; tech-lead delegates the `Titlebar.tsx` bump to `dev-webpage-ui` as a sibling commit on the same branch (one-line change, trivial).
- **B**: tech-lead grants dev-rust a one-line exception for `Titlebar.tsx` version bump only. Documented in the commit message ("Role boundary crossed by tech-lead exception for atomic version sync").
- **C**: defer all three bumps to `shipper`'s ship commit; dev-rust's feature commit ships the code without a version bump. Strictly a CLAUDE.md deviation but pragmatic.

**Request from tech-lead**: pick A, B, or C before I start the work commit. If no reply by the time I finish `cargo check` + `cargo clippy`, my default is **Option A**: bump the two backend files only, and send a one-message delegation to `dev-webpage-ui` for the frontend constant bump.

This is the one round-2 item I cannot resolve without coordination.

### 18.6 Remaining plan-accuracy nits (non-blocking)

- §10 case 1 still cites `mailbox.rs:619-623` for the `/clear` write; actual PTY write is line 629 (the 619-623 range is the preceding comment block). §17.3 rewrites cases 5, 6, 7, 8 but leaves case 1's bullet untouched. **Still informational only** — no code behavior depends on the citation accuracy.
- §7.1 still says "line 653 (the existing body-followup `if` block)"; actual `if` is at line 655, 653 is a comment. §17.2 replaces the §7.3 task body but §7.1's surrounding narrative is untouched. **Still informational only.**

I will not update these during implementation unless tech-lead says so.

### 18.7 Verdict

**APPROVE** with one coordination item: **§18.5 versioning policy** needs a call from tech-lead before my first commit. All §17 content is implementable as-written and byte-consistent with §15.

No other concerns. Ready to execute on tech-lead's "go" + §18.5 pick.

---

## 19. Round 2 — grinch sign-off

**Reviewer**: wg-8-dev-team/dev-rust-grinch
**Date**: 2026-04-19
**Scope**: verify §17 resolves §16 findings; check §17 for new races, leaks, or edge cases.

### 19.1 Verdict: **APPROVE**

All §16 blockers resolved. All §16 should-fixes applied. Plan is implementable as the union of §§1–18. Three minor follow-ups noted as nits below — none block the dev.

### 19.2 §17.2 (B2 — abort body when cred re-inject errs): RESOLVES §16.3

Walked the new control flow at §17.2 lines 967-1010 against `reinject_credentials_after_clear_static` semantics in §7.4:

- `is_clear == false` (compact / non-command): outer `if` skipped → body fires unchanged. Compact preserves creds in summary. ✓
- `is_clear == true, Ok(())`: cred re-inject succeeded → falls through to body. Body lands with creds in context. ✓
- `is_clear == true, Err(_)`: warn-log + **explicit `return`** from spawned task. Body never invoked. ✓
- Helper `Err` cases that correctly suppress body:
  - Idle-poll timeout (no cred write attempted).
  - Session destroyed mid-poll (no cred write).
  - `inject_text_into_session` PTY-write error (cred block partially in PTY).
- Non-agent session (`agent_id.is_none()`): helper returns `Ok(())` silently → body fires. Correct — plain shells receive body as inert text. ✓

**No bypass path found.** §17.2 fully addresses §16.3.

**Residual subtlety (NOT a gap, naming for the record)**: `inject_text_into_session` returns `Ok` even if Enter #2's PTY-write fails (logged "non-fatal", `pty/inject.rs:95`). So helper `Ok` confirms text + Enter #1 went out, not necessarily Enter #2. Enter #1 is the submit; Enter #2 is the safety net. Body-followup polls for idle anyway, so even if Enter #2 was swallowed, body waits for Claude to settle. Correct semantics. No action needed.

### 19.3 §17.5 + grep audit (B1 — token leak): RESOLVES §16.2

Re-grepped `src-tauri/src` for any sink that could emit cred-block content:

| Site | Logs payload? | Does cred path route here? |
|---|---|---|
| `pty/inject.rs:53` (`[inject] session=... submit=... shell=...`) | metadata only — no text | YES (helper) — **safe** |
| `pty/inject.rs:66` (`bytes={text.len()}`) | length only | YES — **safe** |
| `phone/mailbox.rs:705-709` (`first_100={:?}`) | **logs first 100 chars of payload** | **NO** — cred re-inject calls `inject_text_into_session` directly via §7.4 helper; bypasses `inject_into_pty`'s standard branch entirely |
| `commands/session.rs:464,471` cred spawn-path | "auto-injected" / failure code only — no content | YES — **safe** |
| `phone/mailbox.rs` new helper (§7.4) warn lines | log message text only | YES — **safe** (no content interpolation) |
| §17.6 `current_exe()` warn (in `pty/credentials.rs`) | static string literal | n/a (only on `is_none`) — **safe** |

Conclusion: NO runtime sink emits cred content. **§16.2 fully addressed.**

**Maintenance nit (NOT a blocker)**: `mailbox.rs:705-709` logs `first_100` chars of any payload routed through the standard inject path. Cred-block bypasses today, but a future refactor that consolidates "all PTY injects flow through `inject_into_pty`'s standard branch" would re-expose the leak. Recommend a one-line SECURITY comment near `mailbox.rs:705`:
```rust
// SECURITY: this `first_100` log MUST NOT see credential blocks.
// Cred re-inject (§7.4) and spawn-path cred inject call inject_text_into_session
// directly, bypassing this branch. Do not refactor without re-verifying.
```
Out of scope for this plan; can land during implementation or in a separate hygiene PR.

### 19.4 §17.6 (`current_exe()` warn-log): SAFE

Re-read §17.6 helper amendment (lines 1112-1117). The `log::warn!` argument is a static string literal — no `{path}`, no token, no cred content. Fires only when `exe.is_none()`. **Confirmed clean.**

**Docstring nit (NOT a blocker)**: Plan §4 docstring says "It is a pure function: no I/O except `std::env::current_exe()`". After §17.6 the helper also performs I/O via `log::warn!` on the rare-Err branch. Update §4 docstring to: "no I/O except `current_exe()` and a single `log::warn!` when `current_exe()` returns Err". Cosmetic; dev can fix in passing.

### 19.5 §11.8–§11.14 vs §16.10 coverage

| §16.10 case | Plan placement | Status |
|---|---|---|
| #1 Telegram interleave | §11.8 | ✓ |
| #2 xterm keystroke interleave | §11.9 | ✓ |
| #3 Two concurrent `/clear`s from peers | **NOT COVERED** | gap (nit) |
| #4 Cred-timeout body policy | §11.10 | ✓ |
| #5 Token-leak grep | §11.11 | ✓ — runs after every test, strong hygiene gate |
| #6 Restored session post-restart | §11.12 | ✓ |
| #7 Destruction mid-cred-inject | §11.13 | ✓ |
| #8 Plain shell with `agent_id: Some` | §11.14 | ✓ |

**Gap (nit)**: §16.10 #3 not added. The behavior is structurally guaranteed by the precondition at `mailbox.rs:609` (now documented as §10 case 5 in §17.3), but a manual test would confirm the documented invariant. Recommend adding §11.15:

> **§11.15 — Concurrent `/clear` from two peers (probes §10 case 5)**
> 1. Two peers each `send --command clear --to <target>` within ≤1 s while target idle.
> 2. Expected: only one `/clear` lands; the other returns `"agent is busy (not idle)"` and is retried by the outbox poller.
> 3. After Clear #1's cred re-inject completes (or times out), Clear #2 lands on next retry.
> 4. Verify only ONE cred block coexists in Claude's context at any moment; total of two cred blocks (one per `/clear`).

Nit; defer to architect or dev's discretion.

### 19.6 New issues introduced by §17 — none material

1. **`command_owned` capture on cred-error early return** — captured by `move`, only referenced in body-followup branch. On early return, dropped without use. Rust handles cleanly; no `unused_variables` warning since the variable IS read in the other branch. No bug.
2. **Helper purity violation** — §17.6 adds `log::warn!` to a previously-pure helper. Cosmetic only (see §19.4 docstring nit).
3. **§17.7 unit test mechanics** — verified `String::split('\n')` on a string ending in `\n` produces a trailing empty element. The 9-element layout in §17.7 is correct (1 leading empty + 7 content + 1 trailing empty). ✓ Independent confirmation matches dev-rust §18.2.
4. **§17.10 hygiene** — architect already acknowledges the §§1–16 + §17 split. Round 3 (if any) should consolidate. Not new; flagged only.
5. **Sender unaware of dropped body on cred-error** — §17.2 contract names this. Correct trade-off for a best-effort feature; not a new issue.
6. **§18.5 versioning role-boundary issue** — dev-rust correctly flags that the 3-file version sync requires a frontend edit dev-rust cannot make. Out of grinch scope (it's a coordination question for tech-lead), but I concur that **Option A** (split commits across roles) is the cleanest of the three; it preserves both CLAUDE.md atomicity (same branch) and role boundaries.

No new races. No new leaks. No new edge cases.

### 19.7 Distinguishing residual §16 items from new §19 nits

**Residual from §16**: none. All blockers + should-fixes resolved. §16.9 (versioning) was deferred per tech-lead instruction — not my call; dev-rust §18.5 has now escalated it appropriately.

**New nits from §19** (all non-blocking, listed by impact):
- §19.3 — add a SECURITY comment near `mailbox.rs:705` to prevent future refactor from accidentally routing cred-block through the `first_100` log.
- §19.5 — add §11.15 (concurrent-`/clear` test) to round out §16.10 coverage.
- §19.4 — update §4 docstring to acknowledge the §17.6 warn-log side-effect.

### 19.8 Final verdict

**APPROVE.** Plan is ready for the dev to implement. The three §19 nits can be applied in-line during implementation or rolled into a Round 3 consolidation pass — none block coding from starting.

**Round-2 delta vs Round-1 §16**: 2 blockers resolved, 3 should-fixes applied, 4 of 5 nits applied (16.9 deferred to user / dev-rust §18.5). 7 of 8 §16.10 test cases added. Three new minor nits surfaced in §19, all flagged non-blocking.

