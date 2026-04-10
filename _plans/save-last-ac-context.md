# Plan: Save last_ac_context.md to Agent CWD

**Branch:** `feature/save-last-ac-context`
**Status:** READY FOR IMPLEMENTATION

---

## Requirement

After `build_replica_context()` writes the combined context to the cache directory, also save a copy to `{cwd}/last_ac_context.md` for debugging/inspection purposes.

## Change

**Single file:** `src-tauri/src/config/session_context.rs`

**Location:** `build_replica_context()`, after line 345 (`std::fs::write(&file_path, &combined)`), before the `log::info!` on line 347.

**Insert:**

```rust
// Also save a copy in the agent's working directory for inspection
let local_copy = cwd_path.join("last_ac_context.md");
if let Err(e) = std::fs::write(&local_copy, &combined) {
    log::warn!("Failed to write last_ac_context.md to {}: {}", local_copy.display(), e);
}
```

That's it. 3 lines. Uses `cwd_path` (already a `&Path` from line 250) and `combined` (already a `String` from line 319). Failure is a warning, not an error — session creation proceeds normally.

## Notes

- No new dependencies
- No struct changes
- No API changes
- If the cwd doesn't exist or is read-only, the warning log is the only effect
