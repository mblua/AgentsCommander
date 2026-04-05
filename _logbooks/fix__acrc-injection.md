# Bug Fix: %%ACRC%% credential injection reliability

## Problem Statement
Agents output `%%ACRC%%` to request session credentials, but the marker is sometimes not detected by the PTY read loop. Agents fall back to `--token dummy` which now fails due to the token validation fix, making them unable to communicate.

## Root Causes Found

### 1. UTF-8 buffer boundary silently kills detection (CRITICAL)

**File**: `src-tauri/src/pty/manager.rs`, line 271 (before fix)

The PTY read loop uses a 4096-byte buffer. When a multi-byte UTF-8 character (emoji, accented char, etc.) gets split at the buffer boundary:

```
Chunk 1: ...text\xC3       (incomplete 2-byte sequence at end)
Chunk 2: \xA9 text with %%ACRC%%\n   (orphaned continuation byte at start)
```

`std::str::from_utf8(&data)` rejects the ENTIRE chunk. Since all marker scanning (ACRC + response markers) happens inside `if let Ok(text) = from_utf8(...)`, both chunks are silently skipped. The `acrc_tail` buffer also doesn't get updated, breaking cross-buffer detection for subsequent reads.

This is particularly problematic because:
- Claude Code's TUI renders lots of Unicode (emoji, box-drawing chars, etc.)
- A single split char causes TWO consecutive chunks to fail
- There is zero logging when this happens — completely invisible

**Fix**: Replaced `from_utf8()` with `String::from_utf8_lossy()`. Invalid bytes become U+FFFD replacement characters, but since ACRC markers are pure ASCII, detection is unaffected. Added logging when replacement chars are produced.

### 2. Cooldown persists after failed injection (MEDIUM)

**File**: `src-tauri/src/pty/manager.rs`, lines 309-316 (before fix)

The 10-second cooldown timestamp was set BEFORE `inject_credentials` ran:

```
Detect marker → Check cooldown → Set cooldown → Spawn injection task
                                  ^^^^^^^^^^^^^
                                  Set here, BEFORE injection
```

If injection failed (session not found, PTY write error), the cooldown still blocked retries for 10 seconds. The agent would output `%%ACRC%%` again but get silently ignored.

**Fix**: Moved cooldown to AFTER successful injection inside the async task. Changed `inject_credentials` to return `bool` so the caller knows whether to set cooldown.

### 3. No logging for UTF-8 failures (LOW)

When `from_utf8` failed, there was zero indication in logs. Added diagnostic logging when lossy conversion produces replacement characters.

## Files Changed

- `src-tauri/src/pty/manager.rs`:
  - Line 275: `from_utf8()` → `String::from_utf8_lossy()`
  - Lines 276-281: Added UTF-8 replacement char diagnostic logging
  - Lines 318-322: Removed premature cooldown set
  - Lines 333-347: Moved cooldown to async task, only on success
  - Line 644: `inject_credentials` now returns `bool`
  - Lines 697-709: Return `false` on failure, `true` on success

## Testing

- `cargo check` passes cleanly
- `cargo clippy` passes
