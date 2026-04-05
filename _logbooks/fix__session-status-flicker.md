# fix/session-status-flicker — Session status flickering to blue (busy)

## Problem Statement

Sessions spontaneously flip to blue (running/busy) status without any user interaction or agent activity. The idle detector race condition was partially fixed previously (`checked_duration_since`), but something still triggers spurious busy transitions.

## Root Cause

In `src-tauri/src/pty/manager.rs`, the PTY read loop called `idle_detector.record_activity_with_bytes(id, n)` on **every** PTY read unconditionally. This meant terminal escape sequences — cursor repositioning, title bar updates (OSC), color resets, prompt redraws — were counted as "activity" and flipped the session from idle to busy.

Common sources of escape-only PTY output that triggered the bug:
- Cursor position/blink sequences (CSI)
- Terminal title updates (OSC `\x1b]0;...\x07`)
- Color/attribute resets (SGR)
- Shell environment updates

## Fix Applied

**Content-aware activity filtering** in the PTY read loop:

1. Moved the `String::from_utf8_lossy` conversion earlier in the loop (was already needed for ACRC/marker scanning)
2. Before recording activity, strip ANSI escape sequences using the existing `strip_ansi_csi()` function
3. Only record activity if stripped output contains printable characters above ASCII space (excluding U+FFFD replacement chars)
4. Added fast-path optimization: skip `strip_ansi_csi` allocation when no `\x1b` is present in the chunk

**File changed**: `src-tauri/src/pty/manager.rs` (PTY read loop, ~lines 284-308)

## What This Does NOT Fix

Shell prompt redraws that contain visible text (e.g., `user@host:~$`) will still pass the filter. However, these only occur in response to user commands (which IS activity), not spontaneously. If periodic RPROMPT timer updates prove problematic, a "post-command grace period" in the idle detector would be the correct follow-up fix.

## Validation

- `cargo check` — compiles clean
- Code review by feature-dev:code-reviewer — passed with performance optimization applied
