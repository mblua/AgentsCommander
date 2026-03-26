# PLAN: Direct Messaging + --root Flag + Master Token

## Status: IN PROGRESS (branch: fix/mailbox-delivery-to-shipper)

## What's Already Done (committed + pushed)

### 1. `--root` param for send/list-peers CLI
**Files changed:** `src-tauri/src/cli/send.rs`, `src-tauri/src/cli/list_peers.rs`

- `--root <path>` explicitly sets the agent's root directory
- Derives sender name (`parent/folder`) from `--root` instead of CWD
- Derives outbox path as `<root>/.agentscommander/outbox/`
- Eliminates fragile CWD walk-up for agent-initiated sends
- Without `--root`, falls back to walk-up (manual human usage), but `execute()` prints error asking for `--root`

### 2. Init prompt uses `--root`
**File changed:** `src-tauri/src/commands/session.rs`

- Command templates in the init prompt now include `--root "{root}"` baked in
- No `cd` prefix needed — the command is CWD-agnostic

### 3. Anti-spoofing validation in mailbox
**File changed:** `src-tauri/src/phone/mailbox.rs`

- When a message has a token, the mailbox validates that `msg.from` matches the `working_directory` of the session that owns the token
- Mismatch → rejected with "Token-root mismatch" reason
- Added `agent_name_from_path()` helper to MailboxPoller

### 4. teams.json fixed (PROD config, not in repo)
**File:** `~/.agentscommander/teams.json`

- All member names updated to `parent/folder` format (e.g., `"agentscommander_2"` → `"0_repos/agentscommander_2"`)
- This was a data fix, not a code fix

---

## What Needs To Be Done

### 5. Master Token — bypass `can_reach()` sin flags extra

**Goal:** Whoever launches the app (mode app) gets a one-time master token printed to stdout. Using this token with the CLI `send` command bypasses team validation (`can_reach()`), allowing messages to any agent by exact name.

**Why:** The previous `--direct` flag approach was unnecessary complexity. The token itself IS the authorization. If you have the master token, you can talk to anyone. No new CLI flags needed — the existing `--token` param is sufficient.

**Design:**

1. **Generation**: On app startup (`lib.rs` → `run()`), generate a UUID v4 as the master token
2. **Storage**: In-memory only — stored as Tauri managed state (`MasterToken` struct). **Never persisted to disk.** Ephemeral: dies when the app closes, regenerated on next launch.
3. **Exposure**: Printed to stdout at startup. Only the person who launched the app sees it.
4. **Validation in mailbox** (`mailbox.rs` → `process_message()`):
   - Parse the message token
   - Check if it matches the master token → if yes:
     - Skip anti-spoofing (no session to validate against)
     - Skip `can_reach()` (bypass team membership)
     - Go directly to delivery (using standard mode logic)
   - If not master token → existing session token validation (anti-spoofing + `can_reach()`)

**Files to change:**

#### `src-tauri/src/lib.rs`
- Add `pub struct MasterToken(pub String);`
- In `run()`: generate UUID, wrap in `MasterToken`, `.manage()` it, print to stdout

#### `src-tauri/src/phone/mailbox.rs`
- In `process_message()`, before the existing token validation block:
  - `app.state::<MasterToken>()` to get the master token
  - If `msg.token == Some(master_token)` → skip everything, jump to delivery
- No changes to `OutboxMessage` struct
- No changes to `can_reach()` itself

#### No changes to `src-tauri/src/cli/send.rs`
- `--token` already exists and is sufficient
- No `--direct` flag needed

---

## How to Test in Dev

### Prerequisites
- Branch: `fix/mailbox-delivery-to-shipper`
- Dev app running: `npm run tauri dev`
- Copy the master token from the stdout output at startup
- At least one session open in the sidebar

### Test 1: Send message with master token (bypasses teams)
```bash
"./src-tauri/target/debug/agentscommander.exe" send \
  --token "<MASTER_TOKEN>" \
  --root "C:/Users/maria/0_repos/agentscommander_2" \
  --to "<session_agent_name>" \
  --message "Hello via master token" \
  --mode wake
```
- Verify: message delivered, NOT rejected
- Verify: no `can_reach()` check in logs

### Test 2: Send with invalid token (should be rejected)
```bash
"./src-tauri/target/debug/agentscommander.exe" send \
  --token "00000000-0000-0000-0000-000000000000" \
  --root "C:/Users/maria/0_repos/agentscommander_2" \
  --to "<session_agent_name>" \
  --message "Should fail" \
  --mode wake
```
- Verify: rejected (invalid session token, not master token)

### Test 3: Send without token (team-based, existing flow)
```bash
"./src-tauri/target/debug/agentscommander.exe" send \
  --root "C:/Users/maria/0_repos/agentscommander_2" \
  --to "Agents/Shipper" \
  --message "team test" \
  --mode wake
```
- Verify: goes through `can_reach()` as before

### Test 4: Session tokens still work with anti-spoofing
- An agent with a session token can still send, but is validated for token-root match
- Master token is the only way to bypass teams

---

## Architecture Notes

- **Master token replaces `--direct`** — no new CLI flags
- `--root` is still required for CLI sends (determines `from`)
- `--token` is dual-purpose: can be a session token (anti-spoofing + teams) or the master token (bypass all)
- The master token is ephemeral — only lives in process memory, never on disk
- Team-based routing via `can_reach()` is unchanged for non-master-token messages
