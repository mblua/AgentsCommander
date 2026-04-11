# Plan: Save last_ac_context.md to Agent CWD

**Branch:** `feature/save-last-ac-context`
**Status:** READY FOR IMPLEMENTATION (v2 — corrected location)

---

## Requirement

After the combined context file is resolved for a Claude session (whether from replica context[] or global fallback), save a copy to `{cwd}/last_ac_context.md` for debugging/inspection. Must work in ALL cases, not just when `build_replica_context()` succeeds.

## Problem with v1 plan

The v1 plan placed the write inside `build_replica_context()` in `session_context.rs`. That function has early returns (no config.json → `Ok(None)`, no context[] → `Ok(None)`), which means sessions falling back to the global context would NEVER get `last_ac_context.md`. The user wants it saved always.

## Change (v2)

**Two files:**

### 1. REMOVE from `src-tauri/src/config/session_context.rs`

Delete lines 347-351 in `build_replica_context()` (the 3 lines added by v1 that are now in the wrong place):

```rust
// REMOVE THESE LINES:
    // Also save a copy in the agent's working directory for inspection
    let local_copy = cwd_path.join("last_ac_context.md");
    if let Err(e) = std::fs::write(&local_copy, &combined) {
        log::warn!("Failed to write last_ac_context.md to {}: {}", local_copy.display(), e);
    }
```

### 2. ADD to `src-tauri/src/commands/session.rs`

In `create_session_inner()`, at line 148 where `context_path` is already resolved (either replica or global fallback), insert at the top of the `if let Some(context_path)` block:

```rust
if let Some(context_path) = context_path {
    // Save a copy of the resolved context to the agent's cwd for inspection
    let local_copy = std::path::Path::new(&cwd).join("last_ac_context.md");
    if let Err(e) = std::fs::copy(&context_path, &local_copy) {
        log::warn!("Failed to copy context to {}: {}", local_copy.display(), e);
    }

    // ... existing --append-system-prompt-file injection follows ...
```

Uses `std::fs::copy` (not `write`) since `context_path` is already a file on disk. `cwd` is available as a `String` from the function parameters. Warn-only on failure.

## Why this works

At line 148, `context_path` holds the final resolved path regardless of how it was obtained:
- Replica context[] → `build_replica_context()` returned `Ok(Some(path))`
- Global fallback → `ensure_global_context()` returned the path
- Both paths converge into the same `context_path` variable before line 148

## Notes

- No new dependencies
- No struct changes, no API changes
- `std::fs::copy` is in std — no imports needed
- If cwd is read-only or missing, the warning is the only effect
