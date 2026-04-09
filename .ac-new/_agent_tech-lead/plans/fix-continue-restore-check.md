# Bug: `--continue` not injected on session restore for most agent sessions

## Location

`src-tauri/src/commands/session.rs` — lines 66-89

## Problem

When the app restarts and restores persisted sessions, the logic to auto-inject `--continue` for Claude agent sessions checks for `.claude/` **in the session's CWD**:

```rust
let claude_dir_exists = std::path::Path::new(&cwd).join(".claude").is_dir();
if is_claude && is_restore && claude_dir_exists {
```

This check is **wrong**. The `.claude/` directory in a project folder only contains local settings (`settings.json`, `settings.local.json`). It is NOT where Claude Code stores conversation state.

Claude Code stores conversation history in:

```
~/.claude/projects/{mangled-cwd}/
```

Where `{mangled-cwd}` replaces path separators with `--` and removes the drive colon. For example:

```
CWD:     C:\Users\maria\0_repos\agentscommander\.ac-new\_agent_tech-lead
Mangled: C--Users-maria-0-repos-agentscommander--ac-new--agent-tech-lead
State:   ~/.claude/projects/C--Users-maria-0-repos-agentscommander--ac-new--agent-tech-lead/
```

## Impact

Any agent session whose CWD does **not** have a local `.claude/` folder (which is most of them — e.g. all agents under `.ac-new/`) will NOT get `--continue` on app restart. This means they start a fresh conversation instead of resuming, losing all prior context.

## Evidence

- Tech-lead agent CWD (`\.ac-new\_agent_tech-lead\`) has **no** `.claude/` directory
- But `~/.claude/projects/C--Users-maria-0-repos-agentscommander--ac-new--agent-tech-lead/` **exists** with 3 conversation files (984KB of conversation state)
- On next app restart, this session will NOT get `--continue`

## Fix

Replace the check on line 68:

```rust
// BEFORE (wrong — checks for local settings dir)
let claude_dir_exists = std::path::Path::new(&cwd).join(".claude").is_dir();

// AFTER (correct — checks for conversation state in Claude's global projects dir)
let claude_project_exists = {
    if let Some(home) = dirs::home_dir() {
        let mangled = cwd
            .replace(':', "")
            .replace('\\', "-")
            .replace('/', "-");
        home.join(".claude").join("projects").join(&mangled).is_dir()
    } else {
        false
    }
};
```

Then update line 69:

```rust
if is_claude && is_restore && claude_project_exists {
```

## Important: verify the mangling algorithm

The mangling shown above is approximate. **Before implementing**, verify exactly how Claude Code mangles paths by comparing a few known CWDs against their directory names in `~/.claude/projects/`. The pattern observed so far:

| CWD | Directory name |
|-----|---------------|
| `C:\Users\maria` | `C--Users-maria` |
| `C:\Users\maria\0_repos\agentscommander` | `C--Users-maria-0-repos-agentscommander` |
| `C:\Users\maria\0_repos\agentscommander\.ac-new\_agent_tech-lead` | `C--Users-maria-0-repos-agentscommander--ac-new--agent-tech-lead` |

Note: `\` becomes `-`, but `.` is preserved as-is (e.g. `.ac-new` → `-ac-new` not `--ac-new`... wait, it's `--ac-new`). Double-check the exact rules.

## Also update: `strip_auto_injected_continue`

In `src-tauri/src/config/sessions_persistence.rs` line 234, the `strip_auto_injected_continue` function should remain unchanged — it strips based on shell args, not filesystem state. But verify it still works correctly after the fix.

## Validation

After the fix:
1. Start the app, create a Claude agent session with a CWD that has NO `.claude/` folder
2. Have a conversation (creates state in `~/.claude/projects/`)
3. Restart the app
4. Verify the restored session gets `--continue` injected (check logs for "Auto-injected --continue")
