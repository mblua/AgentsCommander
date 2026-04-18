# Plan — Delete `active-only` + `wake-and-sleep`, remove idle-gate from `wake`

**Branch**: `feature/messages-always-by-files` (extend, do NOT branch). Stack on HEAD `4d0d215`.
**Repo**: `repo-AgentsCommander`
**Date**: 2026-04-18
**Requirement source**: `__agent_tech-lead/_scratch/requirement-delete-modes.md`
**Predecessor plans**: `_plans/messages-always-by-files.md`, `_plans/trim-pty-overhead.md` (shipped as commits `03141b9`, `07b4360`, `24b160f`, `ea46672`, `4d0d215`).

---

## 1. Summary

Three collapsing changes on top of HEAD `4d0d215`:

1. Delete mode `active-only` entirely (dead code — zero internal callers).
2. Delete mode `wake-and-sleep` entirely (plus its temp-session spawn machinery).
3. Remove the busy-gate from `deliver_wake` — always inject when a non-Exited session exists.

Result: one delivery path (`wake`) that accepts any non-Exited recipient state. Respawn-on-Exited kept. `--command` idle-gate untouched. Version bump `0.6.1 → 0.7.0` (SemVer minor — breaking CLI).

Commit strategy: **3 incremental commits**, bisect-friendly (see §9).

---

## 2. Audit — symbols affected

Verified against HEAD `4d0d215` with `grep -nE 'active_only|wake_and_sleep|active-only|wake-and-sleep|resolve_agent_command|preferred_agent|TEMP_SESSION_PREFIX|--agent'`.

### 2.1 Will be DELETED (clear-cut dead)

| Symbol | File | Lines | Reason |
|---|---|---|---|
| `async fn deliver_active_only` | `phone/mailbox.rs` | 398-430 | Removed mode. |
| `async fn deliver_wake_and_sleep` | `phone/mailbox.rs` | 583-750 | Removed mode. |
| match arm `"active-only" => …` | `phone/mailbox.rs` | 382 | Mode gone. |
| match arm `"wake-and-sleep" => …` | `phone/mailbox.rs` | 384 | Mode gone. |
| valid-modes element `"active-only"` | `cli/send.rs` | 110 | CLI refuses input. |
| valid-modes element `"wake-and-sleep"` | `cli/send.rs` | 110 | CLI refuses input. |
| `DELIVERY MODES` doc line for `active-only` | `cli/send.rs` | 12 | Help text. |
| `DELIVERY MODES` doc line for `wake-and-sleep` | `cli/send.rs` | 13 | Help text. |
| README delivery-modes bullet `active-only` | `README.md` | 229 | Docs. |
| README delivery-modes bullet `wake-and-sleep` | `README.md` | 230 | Docs. |

### 2.2 Will be KEPT — not actually orphaned by these deletes

| Symbol | Reason for keeping |
|---|---|
| `async fn resolve_agent_command` (`mailbox.rs:1394-1456`) | Still called by `deliver_wake` at L492 (persistent-respawn spawn path). The wake-and-sleep call at L636 goes away; the wake call stays. Function keeps one live caller. |
| `OutboxMessage.preferred_agent: String` (`phone/types.rs:23`) | Read by `resolve_agent_command` when `!= "auto"`. Written by `cli/send.rs:238` (`args.agent`) and `cli/close_session.rs:100` (`""`). Still useful for spawn-path selection under `wake`. |
| `cli/send.rs --agent` flag (L51-53) | Populates `preferred_agent`. Per requirement §Out-of-scope: "Touching `--agent` (unless `--agent` is dead code post-wake-and-sleep deletion)". It is NOT dead — `deliver_wake` spawn path consumes it. Keep. |
| `TEMP_SESSION_PREFIX` (`session/session.rs:22`) | Referenced by `config/sessions_persistence.rs:6,158,163,232` for defensive purge of legacy temp sessions on load, and by `phone/mailbox.rs:1013` as a sort-key tiebreaker in `find_active_session` (non-temp preferred). Neither consumer is removed by this PR. Keep. |
| `OutboxMessage.get_output` / `request_id` / `sender_agent` / `priority` / `action` / `target` / `force` / `timeout_secs` | Close-session uses `action/target/force/timeout_secs/request_id`. `sender_agent` is a fallback in `resolve_agent_command`. `get_output` is vestigial (see §2.4). Keep all. |

### 2.3 Comments that need updating (stale references, not deletions)

| File | Line | Current text | New text |
|---|---|---|---|
| `phone/mailbox.rs` | 389 | `&format!("Unsupported delivery mode '{}'. Valid: active-only, wake, wake-and-sleep", mode)` | `&format!("Unsupported delivery mode '{}'. Valid: wake", mode)` |
| `phone/mailbox.rs` | 754-756 | doc-comment on `inject_into_pty`: `interactive=true` ≈ "wake/active-only"; `interactive=false` ≈ "wake-and-sleep". | Rewrite to reflect only `wake` live + the now-unreachable marker branch. See §5.3 note. |
| `session/session.rs` | 20 | `/// Prefix used for temporary sessions spawned by wake-and-sleep delivery.` | `/// Prefix used historically by wake-and-sleep delivery (removed in 0.7.0). Retained for defensive purge of legacy temp sessions persisted under older versions.` |

### 2.4 Semantic-dead code surfaced but NOT deleted in this PR (§11 risks)

| Item | Where | Why it's semantic-dead | Decision |
|---|---|---|---|
| `inject_into_pty::interactive` param | `mailbox.rs:753-764` | After deletes, every remaining caller passes `interactive=true`. | **Keep for now** — removing it also forces the `use_markers=true` branch at L855-863 to be deleted (it's guarded by `!interactive`). Per requirement §Out-of-scope: "Touching `--get-output`". Leave as dead code. Flag follow-up. |
| `use_markers=true` branch at L855-863 | `mailbox.rs` | Unreachable — always `false` post-deletes (only wake-and-sleep set `interactive=false`). | **Keep** per above. Document unreachability in a doc-comment so future readers know. |
| `register_response_watcher` call path inside `use_markers` | `mailbox.rs:876-891` | Only reachable via the dead branch above. | **Keep**. Follow-up cleanup. |
| `cli/send.rs --get-output` flag | `send.rs:44-46` | Still exists; always times out post-deletes (response watcher never fires under `wake` interactive=true). | **Keep** per requirement §Out-of-scope. Flag as documentation follow-up (users should stop passing it). |
| `cli/send.rs --timeout` | `send.rs:56-58` | Only consumed by `--get-output`. Semantic-dead by transitivity. | **Keep** per out-of-scope. |

No types, fields, imports, or helpers become **symbol-dead** after the deletes in §2.1 — every remaining reference has a live caller or a defensive-use justification listed above.

---

## 3. Decisions (explicit)

| Question | Decision | Reasoning |
|---|---|---|
| Delete `resolve_agent_command`? | **No, keep.** | Still used by `deliver_wake` persistent-respawn path at `mailbox.rs:492`. |
| Delete `--agent` CLI flag? | **No, keep.** | Populates `preferred_agent`, consumed by the kept `resolve_agent_command`. |
| Delete any `OutboxMessage` fields? | **No.** | Every field has a surviving consumer (close-session, spawn-path, or defensive). |
| Delete `TEMP_SESSION_PREFIX` const? | **No, keep.** | Defensive purge in sessions-persistence; harmless sort key in `find_active_session`. |
| Collapse match in `mailbox.rs:380-385`? | **Collapse to defensive single-arm.** See §5.2. | Poller processes outbox files written by any token path (CLI-validated modes + potential raw outbox files from root-token scripts). Keep a defensive reject for malformed mode values instead of `unreachable!()`. |
| Remove `interactive` param from `inject_into_pty`? | **No, keep.** | Per requirement §Out-of-scope for `--get-output`. Removing would force cascading deletion of the markers branch. |
| Commit strategy? | **3 commits** (per §9). | Bisect-friendly per requirement recommendation. |
| `--get-output` follow-up? | **Flag-only in this PR; no code change.** | Requirement explicit out-of-scope. |

---

## 4. Files to MODIFY

### 4.1 `phone/mailbox.rs`

#### 4.1.1 Delete `deliver_active_only` entirely

**Lines 398-430** — delete whole function including leading doc-comment `/// Deliver mode: active-only …`.

#### 4.1.2 Delete `deliver_wake_and_sleep` entirely

**Lines 583-750** — delete whole function including leading doc-comment.

(Line 751 closing brace of the impl block is unaffected. Empty line handling: leave a single blank line between the function above and below per rustfmt.)

#### 4.1.3 Collapse the mode match

**Lines 375-392** — current:
```rust
// Deliver based on mode — all modes require immediate delivery or rejection
let mode = if msg.mode.is_empty() {
    "wake"
} else {
    msg.mode.as_str()
};
match mode {
    "active-only" => self.deliver_active_only(app, &msg).await?,
    "wake" => self.deliver_wake(app, &msg).await?,
    "wake-and-sleep" => self.deliver_wake_and_sleep(app, &msg).await?,
    _ => {
        return self.reject_message(
            path,
            &msg,
            &format!("Unsupported delivery mode '{}'. Valid: active-only, wake, wake-and-sleep", mode),
        ).await;
    }
}
```

Replace with:
```rust
// Deliver — only `wake` is supported. Defensive check for malformed outbox files.
let mode = if msg.mode.is_empty() {
    "wake"
} else {
    msg.mode.as_str()
};
if mode != "wake" {
    return self
        .reject_message(
            path,
            &msg,
            &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
        )
        .await;
}
self.deliver_wake(app, &msg).await?;
```

#### 4.1.4 Rewrite `deliver_wake` — remove idle-gate

**Lines 432-581** (`async fn deliver_wake`) — the only change is removing the busy-reject branch. The rest of the function body (find_active_session, Exited respawn, spawn-persistent, boot-wait, inject) is unchanged.

New body (showing only the changed if-let block at L439-484; everything before and after is byte-identical):

```rust
async fn deliver_wake(
    &self,
    app: &tauri::AppHandle,
    msg: &OutboxMessage,
) -> Result<(), String> {
    if let Some(session_id) = self.find_active_session(app, &msg.to).await {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        let session = sessions.iter().find(|s| s.id == session_id.to_string());

        if let Some(s) = session {
            log::info!(
                "[mailbox] wake: session {} status={:?} waiting_for_input={}",
                session_id, s.status, s.waiting_for_input
            );
            if !matches!(s.status, SessionStatus::Exited(_)) {
                // Always inject — PTY stdin buffer holds the input until the
                // agent finishes the current turn. No busy-gate.
                drop(mgr);
                return self.inject_into_pty(app, session_id, msg, true).await;
            }
            log::info!(
                "[mailbox] wake: session {} is Exited, destroying before respawn",
                session_id
            );
            drop(mgr);
            if let Err(e) =
                crate::commands::session::destroy_session_inner(app, session_id).await
            {
                log::error!(
                    "[mailbox] wake: failed to destroy exited session {}: {}",
                    session_id, e
                );
            }
        } else {
            log::warn!(
                "[mailbox] wake: session {} not in list_sessions",
                session_id
            );
            drop(mgr);
        }
    }

    // ── No active session (or only Exited) — spawn a persistent one ──
    // <<< unchanged from current L486-581 >>>
    // (spawn + boot-wait + inject, exactly as today)
}
```

Diff against current:
- Lines 452-455 (the `if s.waiting_for_input { drop(mgr); return self.inject_into_pty(...).await; }` block) — **removed**.
- Lines 456-461 (the `if !matches!(s.status, SessionStatus::Exited(_)) { return Err("Destination agent session is active but not idle..."); }` block) — **replaced** by the new "always inject non-Exited" block shown above.
- Lines 462-476 (Exited destroy) — unchanged.
- Lines 477-483 (session-not-in-list warn) — unchanged.
- Lines 486-581 (spawn-persistent tail) — unchanged.

#### 4.1.5 Fix stale comments

**Line 389** — error format string (covered in §4.1.3 replacement already).

**Lines 753-764** (`inject_into_pty` doc-comment) — current text names the dead modes. Rewrite:
```rust
/// Inject a message into a session's PTY stdin.
/// `interactive` = true: the session is a live interactive agent under `wake`
///   delivery — plain message only, no response markers, no watcher.
/// `interactive` = false: the non-interactive one-shot path. Currently
///   unreachable (only `wake-and-sleep` set this, and that mode was removed in
///   0.7.0). Retained alongside the `use_markers` branch for future
///   non-interactive consumers; see _plans/delete-modes.md §2.4.
```

### 4.2 `cli/send.rs`

#### 4.2.1 Help text

**Lines 11-13** — current block:
```
DELIVERY MODES:\n  \
  wake            Inject into PTY if the destination agent is idle (waiting for input). Reject otherwise.\n  \
  active-only     Inject into PTY if the destination agent is actively running (not idle). Reject otherwise.\n  \
  wake-and-sleep  Spawn a temporary session for the destination agent, inject the message, destroy when done.\n\n\
```

Replace with:
```
DELIVERY MODES:\n  \
  wake            Inject into PTY. If no session exists, spawn a persistent one; if Exited, respawn. Always delivers.\n\n\
```

#### 4.2.2 `valid_modes` array

**Line 110** — current:
```rust
let valid_modes = ["active-only", "wake", "wake-and-sleep"];
```

Replace with:
```rust
let valid_modes = ["wake"];
```

(Keep the array form so the validator code below remains unchanged and future modes can be appended trivially.)

### 4.3 `README.md`

#### 4.3.1 Mode table entry

**Line 217** — current:
```markdown
| `--mode` | No | Delivery mode: `wake` (default), `active-only`, `wake-and-sleep` |
```

Replace with:
```markdown
| `--mode` | No | Delivery mode: `wake` (default and currently the only supported value; reserved for future modes) |
```

#### 4.3.2 Mode description bullets

**Lines 227-232** — current block:
```markdown
**Delivery modes:**
- `wake` — Inject into PTY if the destination agent is idle (waiting for input). Reject otherwise.
- `active-only` — Inject into PTY if the destination agent is actively running (not idle). Reject otherwise.
- `wake-and-sleep` — Spawn a temporary session for the destination agent, inject the message, and destroy the session when done. Reject if the agent cannot be spawned.
```
(The exact line numbers around 229-230 are the two bullets to delete; L228 and L227 contain the heading/`wake` bullet which needs its reject clause removed.)

Replace the whole `**Delivery modes:**` block with:
```markdown
**Delivery modes:**
- `wake` — Inject into the recipient's PTY. If an active session exists, the message is written to stdin regardless of whether the agent is mid-turn (the agent's stdin buffer absorbs it; the agent reads on the next idle). If the session is Exited it is destroyed and a fresh persistent one is spawned. If no session exists one is spawned.
```

### 4.4 `session/session.rs`

**Line 20** — update doc-comment on `TEMP_SESSION_PREFIX` (text in §2.3).

### 4.5 Files NOT modified (verified)

- `CLAUDE.md` — grep for `active-only|wake-and-sleep|--mode` shows L51, L58, L76 referring only to `--mode wake`. No stale mode references.
- `ROLE_AC_BUILDER.md` — L418, L421 reference only `--mode wake`. No change.
- `session_context.rs` — L428 `default_context` template already uses `--mode wake` only.
- `phone/types.rs` — `OutboxMessage.mode: String` stays. No field deletions.
- `phone/messaging.rs` — not touched by this feature.
- Frontend (`src/**`) — search for `"active-only"` / `"wake-and-sleep"` in `.ts`/`.tsx`/`.css`: zero matches for the mode strings. No UI references to clean.
- `commands/session.rs::resolve_agent_command` (L934) — separate function, different signature, unrelated to mailbox's `resolve_agent_command`. Not touched.

### 4.6 Version bump

- `src-tauri/tauri.conf.json` → `"version": "0.7.0"`.
- `src-tauri/Cargo.toml` → `version = "0.7.0"`.
- `package.json` → `"version": "0.7.0"`.
- `package-lock.json` → top-level + `packages."".version` (npm writes both).
- `src/sidebar/components/Titlebar.tsx` → `APP_VERSION` constant.

---

## 5. Exact new text — consolidated reference

### 5.1 `cli/send.rs` after_help DELIVERY MODES block

```
DELIVERY MODES:
  wake            Inject into PTY. If no session exists, spawn a persistent one; if Exited, respawn. Always delivers.
```

### 5.2 `phone/mailbox.rs` mode dispatch (replacing L375-392)

```rust
// Deliver — only `wake` is supported. Defensive check for malformed outbox files.
let mode = if msg.mode.is_empty() {
    "wake"
} else {
    msg.mode.as_str()
};
if mode != "wake" {
    return self
        .reject_message(
            path,
            &msg,
            &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
        )
        .await;
}
self.deliver_wake(app, &msg).await?;
```

### 5.3 `phone/mailbox.rs` `deliver_wake` (full new body)

Given the size of the function, the canonical new body is: the current body **minus the lines 452-461 block** replaced with a single non-Exited "always inject" branch. Dev implements as a targeted edit, NOT a full-function rewrite, to keep the diff reviewable. Exact replacement region is lines 452-461; replacement text is:

```rust
                if !matches!(s.status, SessionStatus::Exited(_)) {
                    // Always inject — PTY stdin buffer holds the input until the
                    // agent finishes the current turn. No busy-gate.
                    drop(mgr);
                    return self.inject_into_pty(app, session_id, msg, true).await;
                }
```

Everything before (L432-451) and after (L462-581) is byte-identical to HEAD.

---

## 6. Test plan

### 6.1 Deletions

No test file in `src-tauri/src/**/*.rs` references `active_only`, `wake_and_sleep`, `active-only`, or `wake-and-sleep` (grep verified). **Nothing to delete.**

### 6.2 Rewrites

No existing test asserts busy-gate rejection on `deliver_wake`. **Nothing to rewrite.**

### 6.3 New test — assert always-inject on busy

File: `src-tauri/src/phone/mailbox.rs` — add a `#[cfg(test)] mod tests` block at the end of the file (there is none today).

Scope challenge: `deliver_wake` touches `tauri::AppHandle`, `SessionManager`, `PtyManager`, `SessionStatus`, and a bunch of state. A full unit test would require mocking all of this. Lightweight alternative: a **pure logic test** over the same branching structure, verifying the decision that used to reject now injects.

Proposed test — isolates the busy-vs-Exited decision (the only branch that changed) as a helper fn and tests it:

```rust
#[cfg(test)]
mod tests {
    use crate::session::session::SessionStatus;

    /// Returns `Some(true)` when `deliver_wake` would inject into the existing
    /// session, `Some(false)` when it would Exit-destroy + fall through to
    /// spawn, `None` when no session exists. Mirrors the decision at
    /// mailbox.rs L445-484 after the idle-gate removal. Kept as a pure fn so
    /// the logic is testable without a tauri runtime.
    fn wake_decision(status: SessionStatus) -> bool {
        !matches!(status, SessionStatus::Exited(_))
    }

    #[test]
    fn wake_always_injects_when_busy() {
        assert!(wake_decision(SessionStatus::Running));
        assert!(wake_decision(SessionStatus::Active));
    }

    #[test]
    fn wake_always_injects_when_idle() {
        assert!(wake_decision(SessionStatus::Idle));
    }

    #[test]
    fn wake_falls_through_to_respawn_when_exited() {
        assert!(!wake_decision(SessionStatus::Exited(0)));
    }
}
```

This pins the decision table. The decision fn itself is an inline expression inside `deliver_wake`; the test is a structural mirror. If the dev refactors `deliver_wake` to restore a busy-gate, the structural mirror no longer reflects the code — caught in review rather than by the test. Acceptable given no tauri-mock infrastructure exists.

Alternative (higher-value but larger scope): extract the decision into a real helper fn (`fn wake_action_for(status: &SessionStatus) -> WakeAction`) and wire `deliver_wake` to call it. Then the test exercises the actual production path. Proposed as follow-up; out of scope for bisect-friendliness of this PR.

### 6.4 Full suite

- `cargo test --lib` — must pass. The messaging.rs tests (21 of them) are unaffected.
- `cargo clippy -- -D warnings` — must pass. Removing `deliver_active_only` and `deliver_wake_and_sleep` deletes unused code; clippy will not flag deletions. Confirm `resolve_agent_command` still has at least one live caller — if dev accidentally deletes it, clippy will flag dead code (because `close_session` reads `preferred_agent` but never via this path). Double-check at `cargo check` time.

### 6.5 Manual smoke

- `npm run kill-dev` then `npm run tauri dev`.
- From agent A under `wg-7-dev-team`, `send --send <filename> --mode wake` to agent B while B is mid-turn (busy). Expect: B receives the message in its PTY without sender-side error.
- From agent A to a non-running peer C (no active session). Expect: persistent session spawns for C; once idle, message is injected.
- From agent A to a session with `SessionStatus::Exited`. Expect: exited session destroyed, new one spawned, message delivered after boot.
- Try `send --mode active-only ...` → CLI error "invalid mode 'active-only'. Valid: wake".
- Try `send --mode wake-and-sleep ...` → same CLI error.
- Send a `send --command clear` → unchanged behaviour (idle-gate still enforced per requirement §5).

---

## 7. Docs migration

Enumerated fully in §4.3 (`README.md`) and §2.3 (comments). No other doc or agent-facing context file mentions the removed modes (grep-verified).

---

## 8. Risks

1. **External scripts / user automation using `--mode active-only` or `--mode wake-and-sleep`.** Post-merge these hard-fail with a clear CLI error. Breaking change — justified SemVer minor bump. No internal caller audited ⇒ no internal regression.
2. **Telegram bridge.** Grep for `"active-only"` / `"wake-and-sleep"` in `src-tauri/src/telegram/**` returns zero. The telegram manager constructs `OutboxMessage` via different paths (mailbox ingestion from its own inbox) and does not set mode to the deleted values. No action.
3. **UI dashboards / sidebar.** Frontend grep for the mode strings returns zero. Sidebar displays `waiting_for_input` as a badge — unaffected. No action.
4. **`--get-output` becomes universally non-functional.** Already semantic-dead outside wake-and-sleep; with wake-and-sleep gone it is non-functional for all callers. Keep the flag per requirement §Out-of-scope. Flag as doc follow-up (README should warn "`--get-output` is currently non-functional with `--mode wake`; reserved for a future reimplementation").
5. **Response markers (`%%AC_RESPONSE::…%%`) dead branch.** Leaving `use_markers=true` branch unreachable is a code smell but matches requirement. Future readers may be confused; mitigated by the doc-comment update in §2.3 and §4.1.5.
6. **Race: message injected mid-turn.** Per requirement's own analysis and per user instruction, the PTY stdin buffer of Claude/Codex/Gemini accepts input while the agent generates. If a specific coding-agent CLI flushes stdin on turn-boundary (unlikely but possible for some patched versions), the injection is lost silently. Mitigation: the Option B (queue + watcher) fallback is explicitly staged as a follow-up in requirement §Rationale; accept the risk for Option A.
7. **Concurrent busy injection.** A sender hitting a busy agent previously received an explicit "not idle" error, giving it feedback to retry. Post-change, the message lands in the stdin buffer; no feedback to sender either way. The CLI still reports `Message delivered: <id>` — the semantics shift silently from "delivered and read" to "delivered to stdin". Callers that relied on the rejection as a coordination signal (none identified in-tree) would need a different strategy. Document in commit message.
8. **Multiple senders hitting the same busy agent.** Two simultaneous messages concatenate in stdin buffer; the agent reads them back-to-back on the next idle. Semantically acceptable for file-based messaging (each message is a bounded notification with an absolute path). No mitigation needed.
9. **Exited respawn race.** Unchanged — pre-existing behaviour kept. Two senders hitting an Exited session within the same poll cycle could both trigger destroy + spawn; the second `destroy_session_inner` returns error ("session not found") and the second spawn creates a duplicate. Pre-existing; not this PR's scope.

---

## 9. Commit strategy (3 commits, bisect-friendly)

### Commit 1 — `remove active-only mode`
- Delete `deliver_active_only` (`mailbox.rs:398-430`).
- Delete match arm `"active-only"` (`mailbox.rs:382`).
- Remove `"active-only"` from `valid_modes` array (`send.rs:110`).
- Remove `DELIVERY MODES` help line (`send.rs:12`).
- Remove README delivery-modes bullet (L229).
- Update error message in match-default arm (`mailbox.rs:389`) to drop `active-only` from the Valid list.

### Commit 2 — `remove wake-and-sleep mode`
- Delete `deliver_wake_and_sleep` (`mailbox.rs:583-750`).
- Delete match arm `"wake-and-sleep"` (`mailbox.rs:384`).
- Collapse the match block to the single-arm defensive form (§5.2).
- Remove `"wake-and-sleep"` from `valid_modes` array.
- Remove `DELIVERY MODES` help line for wake-and-sleep (`send.rs:13`).
- Remove README delivery-modes bullet (L230).
- Update README `--mode` table entry (L217) to `wake` only.
- Update doc-comment on `TEMP_SESSION_PREFIX` (`session/session.rs:20`).
- Update doc-comment on `inject_into_pty` (`mailbox.rs:753-764`).

### Commit 3 — `remove idle-gate from wake + bump 0.7.0`
- Edit `deliver_wake` to always-inject non-Exited sessions (mailbox.rs replacement at L452-461 per §5.3).
- Update the README `wake` description to say "always delivers".
- Update help-text line for `wake` (`send.rs:11`) to say "always delivers".
- Add unit tests per §6.3 (`#[cfg(test)] mod tests` at end of `mailbox.rs`).
- Bump version in 5 files (§4.6).

Each commit compiles, passes tests, and has coherent behavior. Commit 1 and 2 are pure deletes (mode surface shrinks). Commit 3 is the behavior change.

---

## 10. Sequence for the implementing dev

1. Verify HEAD is `4d0d215` on branch `feature/messages-always-by-files`. Do not branch.
2. **Commit 1**: perform edits in §9 Commit 1; `cargo check && cargo clippy -- -D warnings && cargo test --lib`; commit with message referencing `_plans/delete-modes.md`.
3. **Commit 2**: perform edits in §9 Commit 2; rebuild + retest; commit.
4. **Commit 3**: perform edits in §9 Commit 3 (including tests + version bump); rebuild + retest; commit.
5. Run manual smoke per §6.5.
6. Do NOT push to main. Stack on `feature/messages-always-by-files`.
7. Report back to tech-lead with the 3 commit SHAs.

---

## 11. NOT-scope (explicit)

- `--command` idle-gate: untouched per user instruction.
- `--get-output` / `--timeout` flags: kept despite becoming semantic-dead. Follow-up.
- `use_markers=true` branch in `inject_into_pty`: kept as unreachable code. Follow-up.
- `interactive` param on `inject_into_pty`: kept despite every caller passing `true`. Follow-up.
- Removing `--mode` entirely: future PR.
- Option B (queue + watcher) for busy-injection reliability: re-evaluate post-smoke-test.
- `commands/session.rs::resolve_agent_command` (separate function from `mailbox.rs::resolve_agent_command`): unrelated, untouched.
- Merge / push to `main`.

---

## 12. dev-rust additions — 2026-04-18

Reviewer: dev-rust. Incremental against HEAD `4d0d215`. Verdict: **ACK with §12 deltas.** No P0/P1. One OPEN on test strategy; three small corrections.

### 12.1 Line-ref verification against HEAD

Every claim in plan §2-5 and §9 confirmed against the working tree:

| Plan claim | Verified | Note |
|---|---|---|
| `mailbox.rs:375-392` match block | ✅ L375-392 exact | match at L381, `"active-only"` at L382, `"wake"` at L383, `"wake-and-sleep"` at L384, error msg at L389 |
| `mailbox.rs:398-430` `deliver_active_only` | ✅ L398-430 exact (doc on L398, fn sig L399-403, body ends L430) | — |
| `mailbox.rs:432-581` `deliver_wake` | ✅ doc at L432-433, fn sig L434-438, body spans to L581 | — |
| `mailbox.rs:452-461` idle-gate block (target of §5.3) | ✅ `if s.waiting_for_input { … }` at L452-455, `if !matches!(s.status, SessionStatus::Exited(_))` at L456-461 | Byte-exact replacement region confirmed |
| `mailbox.rs:492` `resolve_agent_command` call in `deliver_wake` | ✅ `let agent_command = self.resolve_agent_command(app, msg).await;` | Survives delete |
| `mailbox.rs:583-750` `deliver_wake_and_sleep` | ✅ doc L583, fn sig L584-590, body ends ~L750 | — |
| `mailbox.rs:636` `resolve_agent_command` call in `deliver_wake_and_sleep` | ✅ exact | Removed with the fn |
| `mailbox.rs:753-764` `inject_into_pty` doc-comment | ✅ L753 `/// Inject…`, L754 mentions `wake/active-only`, L756 mentions `wake-and-sleep` | Rewrite target confirmed |
| `mailbox.rs:855-863` `use_markers=true` branch | ✅ (already the collapsed match from commit `ea46672`; the marker arm lives at L857-860 of the post-trim match) | See §12.4 below — collapsed match changes the line numbers slightly |
| `mailbox.rs:1013` `TEMP_SESSION_PREFIX` in `find_active_session` | ✅ `.starts_with(crate::session::session::TEMP_SESSION_PREFIX)` | — |
| `mailbox.rs:1394-1395` `resolve_agent_command` fn def | ✅ doc L1394, fn sig L1395 | — |
| `mailbox.rs:1404-1405` reads `msg.preferred_agent` | ✅ `if msg.preferred_agent != "auto"` at L1404, `cfg.agents.iter().find(…)` at L1405 | — |
| `cli/send.rs:11-13` help text for the three modes | ✅ exact | — |
| `cli/send.rs:51-53` `--agent` flag | ✅ doc "Agent CLI to use for wake-and-sleep mode" at L51 (stale wording; plan keeps fn but should refresh the doc — see §12.3) | — |
| `cli/send.rs:110` `valid_modes` | ✅ `["active-only", "wake", "wake-and-sleep"]` | — |
| `cli/send.rs:238` `preferred_agent: args.agent` | ✅ exact | — |
| `cli/close_session.rs:100` `preferred_agent: String::new()` | ✅ exact | — |
| `phone/types.rs:23` `preferred_agent: String` | ✅ exact | — |
| `session/session.rs:20-22` `TEMP_SESSION_PREFIX` doc + const | ✅ doc L20, blank L21, `pub const` L22 | — |
| `README.md:217` mode table row | ✅ exact | — |
| `README.md:227-230` mode bullets block | ✅ heading `**Delivery modes:**` at L227, bullets L228-230 (note: plan §4.3.2 says "L229-230 are the two bullets to delete; L228 and L227 contain the heading/wake bullet" — L228 is the `wake` bullet, L227 is the heading) | Plan wording correct |
| `sessions_persistence.rs` `TEMP_SESSION_PREFIX` usage | ✅ 4 refs at L6, L158, L163, L232 | — |

Audit query `grep -E 'active-only|wake-and-sleep|active_only|wake_and_sleep'` across `src-tauri/src`: **25 occurrences in 3 files** (mailbox.rs 20, send.rs 4, session.rs 1). All accounted for in §2 + §4. Zero in `#[test]` or `#[cfg(test)]` blocks.

Frontend (`src/`): zero matches. Docs (CLAUDE.md, ROLE_AC_BUILDER.md, session_context.rs): all reference only `--mode wake`, zero stale mode references — matches plan §4.5.

### 12.2 Symbol audit — one addition beyond architect's enumeration

Architect's enumeration (§2.1-§2.4) is complete. Adding one item:

- **`cli/send.rs:51` doc-comment on the `--agent` field.** Plan keeps the flag (correct — consumed by `resolve_agent_command`) but doesn't refresh the doc, which currently reads `"Agent CLI to use for wake-and-sleep mode"`. After the delete, the wake-and-sleep mode is gone but `--agent` still feeds `preferred_agent` → `deliver_wake`'s persistent-respawn spawn. Stale doc. Trivial fix:

  ```rust
  /// Agent CLI to use when `wake` spawns a new persistent session.
  #[arg(long, default_value = "auto")]
  pub agent: String,
  ```

- **`mailbox.rs:1394` doc-comment on `resolve_agent_command`.** Same issue — `/// Resolve which agent CLI to use for wake-and-sleep mode.` Should be:

  ```rust
  /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
  /// persistent session for the destination agent.
  ```

- **`phone/types.rs:23` `OutboxMessage.preferred_agent` field.** Check current doc-comment; if it references the dead mode, refresh.

These 3 edits belong in Commit 2 alongside the other stale-comment updates.

### 12.3 Version bump — Titlebar.tsx is a no-op

Plan §4.6 lists `src/sidebar/components/Titlebar.tsx → APP_VERSION` as a bump target. Verified: that file reads `__APP_VERSION__` via vite's `define` in `vite.config.ts` (which pulls `tauriConf.version` at build time). There is no hardcoded version string to edit. Bumping `tauri.conf.json` propagates automatically to both Titlebar.tsx sites (sidebar and terminal).

**Correction**: drop Titlebar.tsx from the bump list. Five files to edit:
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock` (auto-syncs on next `cargo check`; commit it)
- `package.json`
- `package-lock.json` (top-level + `packages.""`)

This matches what was actually done in prior version bumps (`03141b9` 0.5.4→0.6.0 and `ea46672` 0.6.0→0.6.1).

### 12.4 Collapsed-match line-number drift (non-actionable)

Plan §2.4 and §4.1.5 reference `mailbox.rs:855-863` as the `use_markers=true` branch. After `ea46672` the if/else was collapsed into a `match (use_markers, msg.request_id.as_ref())`. Current layout:

- L855 `let use_markers = msg.get_output && !interactive;`
- L856-862 `let payload = match (...) { (true, Some(rid)) => format!(...), _ => format_pty_wrap(...) };`

The marker-arm is L857-860 inside the match. After the delete (§Scope), the marker arm becomes **structurally unreachable** because `interactive=true` for every caller → `use_markers = false` always. Plan §2.4 flags this correctly. Implementation does not need a new edit — the dead branch simply stops receiving traffic. Flagged here purely for clarity: the plan's "L855-863" range refers to the logical block, not a contiguous line range.

### 12.5 OPEN-1 — test strategy for `deliver_wake` decision

Plan §6.3 proposes a "pure logic mirror" test: a `wake_decision(status) -> bool` helper defined inside `#[cfg(test)] mod tests`, not called from production code. Architect's own comment: _"If the dev refactors `deliver_wake` to restore a busy-gate, the structural mirror no longer reflects the code — caught in review rather than by the test."_

This is the same class of drift-detection fragility grinch P1-A flagged in round 2 (reply-hint macro). Previous plans learned: when the test duplicates logic, the test doesn't guard the logic — it guards its own copy.

**Alternative** (architect mentions, proposes as follow-up): extract the decision into a real helper fn, wire `deliver_wake` to call it, test the real thing.

Concrete proposal (additive to plan §4.1.4 and §6.3):

```rust
// Near the top of mailbox.rs's impl block, or co-located with deliver_wake.

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the
    /// agent is waiting for input or mid-turn.
    Inject,
    /// Session is Exited — destroy it, fall through to spawn-persistent.
    RespawnExited,
}

/// Pure decision given a session's status. Used by `deliver_wake`.
/// Extracted so the decision table can be unit-tested without a tauri
/// runtime.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}
```

`deliver_wake` rewrites the target region (plan §5.3) to:

```rust
if let Some(s) = session {
    log::info!(
        "[mailbox] wake: session {} status={:?} waiting_for_input={}",
        session_id, s.status, s.waiting_for_input
    );
    match wake_action_for(&s.status) {
        WakeAction::Inject => {
            drop(mgr);
            return self.inject_into_pty(app, session_id, msg, true).await;
        }
        WakeAction::RespawnExited => {
            log::info!(
                "[mailbox] wake: session {} is Exited, destroying before respawn",
                session_id
            );
            drop(mgr);
            // existing destroy_session_inner call …
        }
    }
}
```

Tests exercise the REAL production fn:

```rust
#[test] fn wake_injects_when_running()  { assert_eq!(wake_action_for(&SessionStatus::Running), WakeAction::Inject); }
#[test] fn wake_injects_when_active()   { assert_eq!(wake_action_for(&SessionStatus::Active),  WakeAction::Inject); }
#[test] fn wake_injects_when_idle()     { assert_eq!(wake_action_for(&SessionStatus::Idle),    WakeAction::Inject); }
#[test] fn wake_respawns_when_exited()  { assert_eq!(wake_action_for(&SessionStatus::Exited(0)), WakeAction::RespawnExited); }
```

Cost: +1 enum + 1 pub(crate) fn + 4 short tests + 1 call site swap = ~25 lines. `SessionStatus` already derives `PartialEq` (verified at `session/session.rs:60`) so `assert_eq!` on the new `WakeAction` works cleanly.

Benefit: if any future dev restores a busy-gate inside `deliver_wake` by adding a conditional AROUND `wake_action_for(&s.status)`, the tests don't catch that — same residual fragility as before. But if they edit `wake_action_for` itself (the isolated decision primitive), tests catch it. The helper shrinks the "trust-the-reviewer" surface from "entire `deliver_wake` body" to "the 3-line `wake_action_for` fn".

Architect deferred this as "out of scope for bisect-friendliness of this PR." Bisect-friendliness argument is weak: the helper lives in the same Commit 3 as the decision change; any bisect step lands on a commit with both the helper and its tests. Same signal strength.

**OPEN-1**: (a) plan as-written (mirror test) OR (b) extract real helper fn + tests-against-production? I lean (b). Round-1 and round-2 P1s both re-taught the "test the real thing, not a copy" lesson; repeating the pattern here costs nothing (commit size stays bisect-clean) and closes the residual fragility. Tech-lead's call.

### 12.6 Semantic-dead flags UX note (not actionable in this PR)

Plan §2.4 correctly keeps `--get-output`, `--timeout`, `--agent`-via-`--mode`-only-supports-wake. After the delete, `--get-output` becomes universally non-functional. Per requirement §Out-of-scope: keep them.

**Recommendation** (doc-only, out of this PR or as a single-line README note): add a footnote to the README flag table: `--get-output`: "Currently non-functional under the only supported mode (`wake`); reserved for future reimplementation." Without that, users will pass `--get-output` and hit silent timeouts.

Not blocking. Flag for a follow-up or include in the commit message as a known-issue call-out.

### 12.7 Summary

- **ACK plan with §12 deltas.**
- **OPEN-1** (§12.5): mirror test vs extract real helper fn — my lean is (b) real helper; defer to tech-lead.
- **Corrections** (§12.2, §12.3): 3 stale doc-comments to refresh (`--agent`, `resolve_agent_command`, optionally `OutboxMessage.preferred_agent`); Titlebar.tsx not needed in version bump (propagates via vite).
- All 25 `active-only`/`wake-and-sleep` refs accounted for. Zero in tests. Zero in `session_context.rs` / CLAUDE.md / ROLE_AC_BUILDER.md. Zero in frontend. Doc-migration scope is closed.

— dev-rust (Step 3 review, 2026-04-18)

---

## 13. grinch adversarial review — Step 4 (2026-04-18)

Reviewer: dev-rust-grinch. Reviewed against HEAD `4d0d215`. Premise: tech-lead ratified OPEN-1 = (b) `wake_action_for` helper + `WakeAction { Inject, RespawnExited }` enum, §12.2 stale-doc scrub in commit 2, §12.3 Titlebar.tsx dropped, `--get-output` README footnote in commit 3.

### 13.1 Verdict

**APPROVED.** No P0, no P1. 5 P2 observations (4 pre-existing risks not in §8; 1 implementation nit for dev-rust). OPEN-1 vote: **CONFIRM (b)**.

### 13.2 What I verified

- **Line-ref drift against HEAD**: reconfirmed dev-rust §12.1. All ranges match: mailbox.rs 375-392 mode dispatch, 398-430 deliver_active_only, 432-581 deliver_wake, 452-461 idle-gate replacement region, 492 & 636 `resolve_agent_command` calls, 583-750 deliver_wake_and_sleep, 753-764 `inject_into_pty` doc, 855-862 `use_markers` collapsed match (post-`ea46672`), 1013 TEMP_SESSION_PREFIX sort key, 1394-1395 `resolve_agent_command` fn, 1404-1405 `preferred_agent` reads. send.rs 11-13 help, 51-53 `--agent`, 110 valid_modes. types.rs 23 `preferred_agent`. session.rs 20-22 TEMP_SESSION_PREFIX. README 217 + 227-230. ✓
- **Match collapse safety (tech-lead Q1)**: `msg.mode` has `#[serde(default)]` → missing field deserializes to `""`. Plan normalizes `""` → `"wake"`. Malformed JSON fails serde parse entirely → handled by pre-existing `reject_raw` path. Unicode/case-variant modes (`"WAKE"`, `"wAkE"`, `"wake\0"`) go through bytewise `!= "wake"` → rejected via `reject_message`. Sender gets the reject reason in delivered/ flow. CLI-side validation at send.rs:110 prevents most malformed modes reaching outbox anyway. Defense-in-depth adequate. ✓
- **Idle-gate removal (tech-lead Q2)**: new body (`if !matches!(Exited)` → inject else destroy-and-fall-through) is byte-identical to current flow EXCEPT: the L452-455 `if s.waiting_for_input { inject }` block disappears and the L456-461 reject-if-not-Exited block flips to inject-if-not-Exited. Lock ordering preserved (`drop(mgr)` before `.await`). Exited respawn path untouched. ✓ Race between `find_active_session` and `inject_into_pty` same as before: session could die in between; `inject_into_pty` fails cleanly. Pre-existing, not aggravated.
- **WakeAction helper shape (tech-lead Q4)**: `wake_action_for(status: &SessionStatus) -> WakeAction`. Two variants `Inject | RespawnExited` capture the domain. `waiting_for_input` correctly falls out of the decision post-trim (always-inject for any non-Exited). "Session does not exist" case doesn't enter the helper — outer `if let Some(s) = session` guards. No third `Spawn` variant needed. ✓ Dev-rust §12.5 sketch is complete as proposed.
- **Dead symbol audit (tech-lead Q5)**:
  - `resolve_agent_command` (mailbox.rs:1395): 2 callers today (L492 deliver_wake, L636 deliver_wake_and_sleep). Post-commit-2: L492 remains. ✓ Alive.
  - `preferred_agent` (types.rs:23): written by send.rs:238 + close_session.rs:100, read by mailbox.rs:1404. Survives. Note: `preferred_agent_id` in `ac_discovery.rs` is a DIFFERENT field (suffix `_id`); unrelated.
  - `TEMP_SESSION_PREFIX` (session.rs:22): 6 refs. Post-commit-2 removes `mailbox.rs:654` (inside deleted fn). Remaining: session.rs:22 def + sessions_persistence.rs L6/L158/L163/L232 (defensive purge) + mailbox.rs:1013 (sort tiebreaker). 5 live refs. ✓
  - No helpers inside `deliver_wake_and_sleep` are referenced from elsewhere (body is self-contained). Clean delete.
  - `commands/session.rs::resolve_agent_command` at L934 is a SEPARATE private fn (different signature, different callers at L548/L759). Unrelated. Plan §4.5 notes this correctly.
- **Semantic-dead code (tech-lead Q6)**: `inject_into_pty::interactive` param with all callers passing `true` — Rust lint `unused_variables` only fires for unread params; this one is still read inside the fn body, so no warning. `use_markers=true` arm at mailbox.rs:857-860 becomes runtime-unreachable — compile-time reachable, no clippy warn. `register_response_watcher` at L864-878 becomes runtime-unreachable — same. Clippy with `-D warnings` stays clean. ✓ Pre-existing-dead risk accumulates, but not this PR's fault.
- **Commit bisect (tech-lead Q7)**: each commit compiles. Commit 1 (remove active-only) leaves deliver_wake + deliver_wake_and_sleep + match arms for wake/wake-and-sleep + default. Commit 2 (remove wake-and-sleep) leaves deliver_wake (with busy-gate) + match collapsed. Commit 3 (idle-gate removal + 0.7.0) finalizes. Reverting commit 2 works cleanly; reverting commit 1 works cleanly. ✓
- **Frontend/docs scan**: grepped `active-only|wake-and-sleep|active_only|wake_and_sleep` across `repo-AgentsCommander/src` (frontend) — zero hits. CLAUDE.md / ROLE_AC_BUILDER.md / session_context.rs — zero stale mode refs. ✓
- **Version bump (tech-lead Q10)**: CLI contract breaks (2 modes gone, 1 reject class gone). Pre-1.0 SemVer: minor bump = breaking change. 0.6.1 → 0.7.0 correct. 5 files per §12.3 correction (Titlebar.tsx auto-propagated via vite). ✓

### 13.3 P2 findings (not blocking)

#### P2-1 — PTY stdin buffer overflow on persistently busy agent (missing from §8 risks)

Plan §8.6 notes "if a specific coding-agent CLI flushes stdin on turn-boundary, the injection is lost silently" — but doesn't address what happens if the agent DOESN'T flush and the buffer fills.

**Scenario**: Agent A is busy for 10+ minutes (long LLM turn). 30 senders each inject ~250-char messages. Cumulative buffered input ≈ 7.5 KB. Windows ConPTY pipe buffer depends on configuration; typical default is 4-64 KB. At or near buffer capacity, `WriteFile` (underlying `inject_text_into_session`) can **block or fail with `ERROR_BROKEN_PIPE`** depending on pipe state.

If `inject_text_into_session` blocks synchronously, the MailboxPoller stalls — subsequent messages to other agents pile up in outbox. If it fails, the message is rejected and the sender sees a delivery timeout.

**Mitigation**: out of scope for this PR (Option B queue+watcher re-addresses via deferred injection). Just add as an explicit §8 risk so future triage doesn't have to rediscover.

#### P2-2 — Mid-turn injection may corrupt line-oriented CLI UIs (missing from §8)

Plan §8.6 notes injection-during-generation concerns but doesn't enumerate the UI-corruption class.

**Scenario**: Agent uses a readline-style prompt (Claude Code's interactive prompt, some shell wrappers). When mid-turn, the cursor is typically past a prompt string like `> `. Injecting text mid-turn puts it in readline's input buffer, NOT as a new line. When the turn finishes and control returns to readline, the buffer shows the injected text AT THE CURRENT CURSOR POSITION. User sees a half-drafted message pre-filled with someone else's content.

**Severity**: cosmetic for humans (confusing but non-destructive); functional for AI agents that auto-submit queued input (they'd read the previous sender's message as user intent).

**Mitigation**: Option B; or per-agent-CLI heuristic to buffer at mailbox level and inject only at idle. Out of scope.

Plan should call this out in §8 so post-deploy triage recognizes the pattern if it shows up.

#### P2-3 — `--get-output` + `--timeout` become silent-fail flags (partially in §8.4)

Plan §8.4 flags `--get-output` as universally non-functional; tech-lead's commit 3 README footnote documents this. But the CLI itself still accepts the flag without warning. A user passing `--send foo.md --get-output` waits up to `--timeout` seconds for nothing.

**Mitigation options** (out of scope for this PR):
- (a) Emit a `log::warn!` at CLI parse time if `--get-output` is set and `--mode == "wake"` — silent safety net.
- (b) Add a one-line stderr banner before the wait loop: `"Note: --get-output is currently non-functional; waiting will time out."`
- (c) Leave as-is; rely on README footnote.

Plan picks (c) implicitly. Defensible given out-of-scope constraints; flag for follow-up.

#### P2-4 — `register_response_watcher` dead path accumulates

Pre-existing concern flagged in trim review (Step 7). After delete-modes, the `use_markers=true` match arm becomes TRULY unreachable (previously it had a theoretical `--get-output` path; now the `interactive` param is always `true` and `use_markers = msg.get_output && !interactive = false`). `register_response_watcher` is fully dead code.

Not this PR's cleanup scope (requirement §Out-of-scope: "Touching `--get-output`"). Track as follow-up: delete the markers branch + `register_response_watcher` path + `interactive` param + `--get-output`/`--timeout` flags as a single "response-watcher retirement" PR after empirical Option A validation.

#### P2-5 — Implementation nit for dev-rust: `WakeAction` derives

The §12.5 sketch shows `#[derive(Debug, PartialEq)]`. Tech-lead's resolution said "enum `WakeAction { Inject, RespawnExited }`" without specifying derives. Implementer MUST include both:
- `PartialEq` — required for `assert_eq!` in the 4 unit tests.
- `Debug` — required for `assert_eq!` failure messages to print the variant name.

Trivial, but flagging so it doesn't land in the first impl pass with test compile errors.

### 13.4 OPEN-1 confirmation

**CONFIRM tech-lead ratification: (b) `wake_action_for` helper + `WakeAction` enum + 4 tests against production.**

Reasoning:
- Mirror test has the same P1-A drift-class fragility that round 2 proved to fix via real-helper pattern. Repeating the fragile pattern here would be a step back.
- Cost: ~15 lines total (enum + pub(crate) helper + 4 short asserts). Bisect size argument (architect's rationale for mirror) is weak — helper lives in the same commit as the behaviour change, same signal strength.
- Benefit: future dev restoring a busy-gate inside `wake_action_for` itself gets caught by tests. Restoring a busy-gate AROUND the helper (in deliver_wake) is caught in code review — same surface as the mirror test anyway, but now the primitive decision table is independently exercised.

Done right. No pushback.

### 13.5 No new OPENs

### 13.6 Summary table

| Finding | Severity | Action |
|---|---|---|
| Stdin buffer overflow risk | P2 | Add as §8 risk; track as Option B follow-up |
| Mid-turn UI corruption | P2 | Add as §8 risk; empirical smoke will surface |
| `--get-output` silent-fail | P2 | README footnote (in plan); optional parse-time warn (follow-up) |
| Response watcher dead path | P2 | Follow-up cleanup PR |
| `WakeAction` derives | P2 | Dev-rust adds `Debug, PartialEq` at impl time |
| OPEN-1 | — | CONFIRM (b) |
| Line-ref drift | — | Zero drift against HEAD |
| Bisect safety | — | Each commit compiles + tests |

### 13.7 Approval

**APPROVED.** Ready for Step 5 (quick ACK) or direct to Step 6 (dev-rust implementation).

No round-2-style architectural surprises. Plan + tech-lead decisions are internally coherent. The 5 P2s are either pre-existing risks to document or implementation nits that naturally surface during impl.

— dev-rust-grinch (Step 4 delete-modes, 2026-04-18)
