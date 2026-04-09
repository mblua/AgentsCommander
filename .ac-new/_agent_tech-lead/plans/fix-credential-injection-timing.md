# Fix: Credential injection timing for --continue sessions

## Problem

When a session restores with `--continue`, Claude Code takes 5-10+ seconds loading the previous conversation. The credential block is injected after a fixed 2s delay (`session.rs:182`), arriving before Claude is ready. The token gets lost and the agent can't communicate.

## Root Cause

`session.rs:182` uses `tokio::time::sleep(2000ms)` — a naive fixed delay. Insufficient for `--continue` sessions with large conversations.

## Fix

Replace the fixed 2s sleep at `session.rs:182` with idle-polling, reusing the exact pattern from `mailbox.rs:654-686` (`inject_followup_after_idle_static`):

1. Poll `waiting_for_input` every 500ms
2. Max timeout 30s
3. On idle detected → inject credentials
4. On timeout → inject anyway as fallback (better to try late than never)

### Before (session.rs:181-182)
```rust
tokio::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
```

### After
```rust
tokio::spawn(async move {
    // Wait for Claude to become idle (ready for input) instead of fixed delay.
    // Mirrors the pattern in mailbox.rs inject_followup_after_idle_static.
    let max_wait = std::time::Duration::from_secs(30);
    let poll = std::time::Duration::from_millis(500);
    let start = std::time::Instant::now();

    loop {
        if start.elapsed() >= max_wait {
            log::warn!("[session] Timeout waiting for idle before credential injection for session {}", session_id);
            break; // inject anyway as fallback
        }
        tokio::time::sleep(poll).await;

        let session_mgr = app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        match sessions.iter().find(|s| s.id == session_id.to_string()) {
            Some(s) if s.waiting_for_input => break, // ready
            Some(_) => {} // still busy, keep polling
            None => {
                log::warn!("[session] Session {} gone before credential injection", session_id);
                return; // session destroyed, nothing to inject
            }
        }
    }
```

### Key difference from mailbox pattern
- On timeout: **inject anyway** (break, don't return Err). A late token is better than no token.
- On session gone: **return early** (don't inject into nothing).

## Scope
- Only `session.rs:181-182` changes (replace the sleep line with the polling loop)
- Everything else (credential block formatting, inject call) stays identical
- This improves timing for ALL Claude sessions, not just --continue

## Validation
- `cargo check` must pass
- No other files modified
