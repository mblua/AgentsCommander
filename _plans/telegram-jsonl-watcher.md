# Plan: Telegram JSONL Watcher for Claude Code Sessions

**Branch:** `feature/telegram-jsonl-watcher`
**Status:** AWAITING ARCHITECT REVIEW

---

## Problem

The current Telegram bridge captures PTY output via a 6-phase pipeline:

```
PTY bytes -> vt100::Parser -> 800ms stabilization -> AgentFilter -> Buffer -> Telegram
```

This is extremely complex and still produces noisy output because the source is raw terminal bytes full of ANSI escapes, cursor movement, spinner animations, TUI chrome, and box-drawing characters.

## Discovery

Claude Code writes **clean structured session logs** as JSONL files:

- **Location:** `~/.claude/projects/{mangled-cwd}/<session-id>.jsonl`
- **Path mangling:** Replace non-alphanumeric, non-hyphen chars with `-` (already implemented in `commands/session.rs:72-74`)
- **Format:** One JSON object per line with structured message data:

```json
{"type": "permission-mode", "permissionMode": "bypassPermissions", "sessionId": "..."}
{"type": "user", "message": {"role": "user", "content": "..."}}
{"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "clean model output"}, {"type": "tool_use", "name": "Bash"}]}}
{"type": "summary", ...}
```

The `message.content` field for assistant messages contains **clean text without any ANSI codes or terminal artifacts** -- exactly what we want to send to Telegram.

## Requirement

Build a **JSONL file watcher** as an **alternative output source** for the Telegram bridge, specifically for Claude Code sessions. When a session is detected as Claude Code, the bridge should watch the JSONL session file instead of using the PTY-based pipeline.

### Must Have

1. **JSONL Watcher module** (`src-tauri/src/telegram/jsonl_watcher.rs` or similar):
   - Watch the session's JSONL file for new lines appended
   - Parse each new line, extract `role: "assistant"` messages
   - Extract text content from `message.content` (handle both `string` and `[{type: "text", text: "..."}]` formats)
   - Skip tool_use blocks, tool_result blocks, system messages, permission-mode entries
   - Feed extracted text to the existing buffer/send pipeline (Phase 5-6 of current bridge)

2. **Session JSONL path resolution:**
   - Reuse the existing mangling logic from `commands/session.rs:72-74`
   - Find the most recently modified `.jsonl` file in the project directory
   - The session file changes when Claude Code restarts, so must handle file rotation

3. **Integration with existing bridge:**
   - When `spawn_bridge()` is called and the session is detected as Claude Code:
     - Spawn the JSONL watcher task **instead of** the PTY output_task
     - The poll_task (Telegram -> PTY input) remains unchanged
   - When the session is NOT Claude Code, fall back to the existing PTY pipeline
   - The `BridgeHandle`, `BridgeInfo`, `TelegramBridgeManager` interfaces should NOT change

4. **Detection of Claude Code session:**
   - The `is_claude` flag already exists in `commands/session.rs:65`
   - Need to propagate this flag to the session metadata so the bridge can check it at attach time
   - Alternatively, check if the JSONL project dir exists at attach time

5. **File watching strategy:**
   - Use `notify` crate (already common in Rust ecosystem) OR simple polling (e.g., poll every 500ms, check file size, read new bytes)
   - Given that Claude Code writes infrequently (only on message completion), **polling is acceptable and simpler**
   - Track file position (byte offset) to only read new content

### Nice to Have (NOT in scope for v1)

- Watching multiple JSONL files if Claude starts a new session (file rotation)
- Sending user messages to Telegram (only assistant for now)
- Support for other coding agents (Codex, Cursor, etc.) -- this is explicitly deferred

### Architecture Constraint

The JSONL watcher must be **well-isolated** as a self-contained module. The intent is to later add similar watchers for other coding agents that have their own log formats. Think of it as:

```
telegram/
  bridge.rs          -- existing, orchestrates which output source to use
  jsonl_watcher.rs   -- NEW: Claude Code JSONL file watcher
  manager.rs         -- existing, unchanged interface
  ...
```

Future agents would add their own watcher modules (e.g., `codex_watcher.rs`) with the same output interface.

## Existing Code References

- **Path mangling:** `src-tauri/src/commands/session.rs:72-74`
- **Claude detection:** `src-tauri/src/commands/session.rs:65`
- **Bridge spawn:** `src-tauri/src/telegram/bridge.rs:413-452`
- **Output task (to be replaced for Claude):** `src-tauri/src/telegram/bridge.rs:470-499`
- **Buffer + send phases:** reuse from existing bridge.rs (Phase 5-6)
- **AgentFilter trait:** already exists in bridge.rs, may not be needed for JSONL (text is already clean)
- **Telegram API send:** `src-tauri/src/telegram/api.rs`

## JSONL Parsing Details

Each line is independent JSON. Relevant message types:

| type | role | action |
|------|------|--------|
| `"user"` | `"user"` | SKIP (v1) |
| `"assistant"` | `"assistant"` | EXTRACT text, send to Telegram |
| `"permission-mode"` | - | SKIP |
| `"summary"` | - | SKIP |

For `message.content`:
- If `string`: use directly
- If `array`: iterate, collect all `{type: "text", text: "..."}` blocks, join with newline
- Skip `{type: "tool_use", ...}` and `{type: "tool_result", ...}` blocks

## Open Questions for Architect

1. Should the watcher trait be generic from the start (e.g., `trait SessionLogWatcher`) or just a concrete struct for now?
2. Should we add a `notify` crate dependency or use simple polling? The JSONL is written infrequently.
3. Where should the `is_claude` flag be stored in session metadata? Currently it's a local variable in `create_session`. Options: add to `Session` struct, or re-derive at attach time from the session's CWD.
