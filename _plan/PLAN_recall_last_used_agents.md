# PLAN: Recall Last Used Agents

## Goal

When the app starts, automatically re-create the terminal sessions that were open when it last closed. The user should see their agents back in the sidebar, each spawned in the same shell, same working directory, with the same name - ready to use.

PTY history (scrollback) is NOT restored. Only the session "recipe" (shell + args + cwd + name) is persisted and re-spawned.

---

## Current State

- Sessions live only in memory (`SessionManager` HashMap).
- On app close, everything is lost.
- Settings persist to `~/.agentscommander/settings.json` via `config::settings`.
- `Session` struct has: id, name, shell, shell_args, working_directory, status, created_at, last_prompt, waiting_for_input.

---

## Design Decisions

1. **Persist session list to `~/.agentscommander/sessions.json`** - same pattern as settings.json. JSON, not TOML, for consistency with existing persistence.

2. **Save on every mutation** (create, destroy, rename, reorder) - not just on app close. If the app crashes, state is still saved.

3. **New UUIDs on restore** - persisted sessions get fresh UUIDs when re-spawned. The old UUID is meaningless without its PTY.

4. **Restore order matters** - sessions are re-created in the same order they appeared in the sidebar. The last active session becomes active again.

5. **No setting to disable** - this is always-on. If the file is missing or corrupt, start with no sessions (current behavior).

---

## Data Model

### PersistedSession (new Rust struct)

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    pub working_directory: String,
    pub was_active: bool,  // true for the session that was active at close
}
```

### sessions.json (example)

```json
[
  {
    "name": "api-server",
    "shell": "powershell.exe",
    "shellArgs": ["-NoLogo"],
    "workingDirectory": "C:\\Users\\maria\\0_repos\\my-api",
    "wasActive": true
  },
  {
    "name": "frontend",
    "shell": "powershell.exe",
    "shellArgs": ["-NoLogo"],
    "workingDirectory": "C:\\Users\\maria\\0_repos\\my-frontend",
    "wasActive": false
  }
]
```

---

## Implementation Steps

### Step 1 - Persistence module (Rust)

**File:** `src-tauri/src/config/sessions_persistence.rs` (new)

- Define `PersistedSession` struct
- `save_sessions(sessions: &[PersistedSession])` - write to `~/.agentscommander/sessions.json`
- `load_sessions() -> Vec<PersistedSession>` - read from disk, return empty vec on error
- Helper: `snapshot_sessions(mgr: &SessionManager) -> Vec<PersistedSession>` - convert live sessions to persisted form

**File:** `src-tauri/src/config/mod.rs` - add `pub mod sessions_persistence;`

### Step 2 - Save on every session mutation (Rust)

**File:** `src-tauri/src/commands/session.rs`

After each of these commands, call `save_sessions(snapshot)`:

- `create_session` - after session is created and PTY spawned
- `destroy_session` - after session is removed
- `rename_session` - after name is updated
- `switch_session` - after active session changes (updates `was_active`)

Implementation: add a helper function `persist_current_state(session_mgr)` that:
1. Reads the session manager (already has the lock)
2. Builds `Vec<PersistedSession>` from live sessions
3. Calls `save_sessions()`

This runs on the async runtime, fire-and-forget. Persistence failure is logged but never blocks the UI.

### Step 3 - Restore on startup (Rust)

**File:** `src-tauri/src/lib.rs` - inside `setup()` closure, after creating windows:

```rust
// Restore last sessions
let persisted = sessions_persistence::load_sessions();
if !persisted.is_empty() {
    // Clone handles needed inside the async block
    let session_mgr = app.state::<...>().inner().clone();
    let pty_mgr = app.state::<...>().inner().clone();
    let app_handle = app.handle().clone();

    tauri::async_runtime::spawn(async move {
        let mut active_id = None;
        for ps in &persisted {
            // create session + spawn PTY (reuse logic from create_session command)
            // if ps.was_active, remember its id
        }
        // switch to the remembered active session
    });
}
```

Key: extract the create-session logic from the command handler into a shared function so both the command and the restore path can use it.

### Step 4 - Refactor create_session logic (Rust)

**File:** `src-tauri/src/commands/session.rs`

Extract the core of `create_session` into a standalone async function:

```rust
pub async fn create_session_inner(
    session_mgr: &Arc<RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    app_handle: &AppHandle,
    shell: String,
    shell_args: Vec<String>,
    cwd: String,
    session_name: Option<String>,
) -> Result<SessionInfo, String> { ... }
```

The Tauri command `create_session` becomes a thin wrapper that calls `create_session_inner` + persists.

The restore path calls `create_session_inner` in a loop + persists once at the end.

### Step 5 - Frontend: no changes needed

The frontend already listens for `session_created` events. When the backend restores sessions on startup, each `create_session_inner` emits `session_created`, so the sidebar populates automatically. The `session_switched` event handles activating the right session.

The only consideration: the frontend should not show a "loading" or empty state flash. Since restore happens in `setup()` before the webview fully loads, events should arrive before or shortly after the frontend mounts. If there's a race, the frontend already calls `list_sessions` on mount which will return the restored sessions.

---

## File Changes Summary

| File | Action |
|------|--------|
| `src-tauri/src/config/sessions_persistence.rs` | NEW - PersistedSession struct, save/load |
| `src-tauri/src/config/mod.rs` | EDIT - add module |
| `src-tauri/src/commands/session.rs` | EDIT - extract inner fn, add persist calls |
| `src-tauri/src/lib.rs` | EDIT - restore sessions in setup() |
| `src/shared/types.ts` | NO CHANGE - SessionInfo stays the same |
| `src/sidebar/App.tsx` | NO CHANGE - events handle everything |

---

## Edge Cases

- **CWD no longer exists**: Skip that session, log a warning. Do not crash.
- **Shell not found**: Skip that session, log a warning.
- **Corrupt JSON**: Log error, start with empty session list.
- **Empty file**: Start with no sessions (same as fresh install).
- **All sessions were closed before exit**: sessions.json is `[]`, app starts empty. Correct behavior.
- **Race condition on mount**: Frontend calls `list_sessions` after mount. If restore is still running, some sessions may not appear yet. The `session_created` events will fill them in. No action needed.

---

## What This Does NOT Do

- Restore PTY scrollback / history
- Restore terminal state (cursor position, colors, running processes)
- Remember window positions (separate feature)
- Auto-save periodically on a timer (mutations are enough)
- Limit the number of restored sessions

---

## Testing

1. Create 3 sessions with different names and cwds
2. Close the app
3. Reopen - verify all 3 appear in sidebar with correct names
4. Verify the previously-active session is active again
5. Verify each session's PTY is functional (can type commands)
6. Delete a session, close app, reopen - verify only 2 appear
7. Delete `sessions.json` manually - verify app starts empty
8. Put invalid JSON in `sessions.json` - verify app starts empty without crash
