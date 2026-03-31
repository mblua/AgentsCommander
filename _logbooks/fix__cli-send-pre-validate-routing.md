# Bug Fix: CLI send returns success on messages that will be rejected

## Problem Statement
The CLI `send` command is fire-and-forget: it writes to outbox/ and exits with success. The actual routing validation happens asynchronously in the MailboxPoller. If the message is rejected, the sender never learns about it.

**User requirement**: No more queued messages. All messages must be delivered and confirmed immediately, or rejected immediately.

## Root Cause
- `send.rs` writes to outbox, prints "Message queued", exits 0
- `mailbox.rs` MailboxPoller picks up the file 0-3s later, validates, delivers or rejects
- No feedback loop to the CLI process

## Plan
1. **Pre-validate routing in CLI**: Load `teams.json`, run `can_communicate()` before writing to outbox. Fail immediately if routing would reject.
2. **Remove queue mode entirely**: No more passive inbox writes. All messages must be PTY-injected.
3. **Add delivery confirmation polling**: After writing to outbox, CLI polls for file to appear in `delivered/` or `rejected/`. Reports actual outcome.
4. **Remove queue fallbacks in mailbox**: `active-only`, `wake`, `wake-and-sleep` modes reject instead of falling back to queue when conditions aren't met.

## Changes

### send.rs
- Import `load_dark_factory` + `can_communicate`
- Pre-validate routing before outbox write
- Remove "queue" from valid modes, default to "wake"
- Poll delivered/rejected after outbox write
- Restructure --get-output to happen after delivery confirmation

### mailbox.rs
- Remove `deliver_queue` method
- Remove `resolve_inbox_dir` method (only used by deliver_queue)
- `deliver_active_only`: reject instead of queue fallback
- `deliver_wake`: reject instead of queue fallback
- `deliver_wake_and_sleep`: reject instead of queue fallback
- `process_message`: remove "queue" mode handler

---

## Log

### 2026-03-30 — Implementation start
- Branch: `fix/cli-send-pre-validate-routing`
- Reading all key files to understand the full flow
