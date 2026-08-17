# Settings reference

For developers editing `settings.json` by hand, or scripting AgentsCommander configuration. The full schema of the `settings.json` file AC reads at startup.

## File location

`settings.json` lives **next to the binary** in the per-instance config directory:

| Binary | Config directory | Settings file |
|---|---|---|
| `C:\tools\agentscommander.exe` | `C:\tools\.agentscommander\` | `C:\tools\.agentscommander\settings.json` |
| `C:\work\agentscommander_team-a.exe` | `C:\work\.agentscommander_team-a\` | `C:\work\.agentscommander_team-a\settings.json` |
| (debug build) | adds `-dev` suffix | `…\.agentscommander-dev\settings.json` |

See [Portable instances](../features/portable-instances.md) for the rule.

## Editing rules

- The file is **JSON** (not JSONC, not YAML). Comments are not allowed.
- AC reads at startup and on `update_settings` IPC calls.
- If you edit `settings.json` **while the app is running**, your changes may be clobbered by the next in-memory save. For manual-only fields such as `specBoardEnabled`, edit while AC is closed, or reload settings before using any Settings save path.
- `terminalSnapshotsEnabled` is security-sensitive. AgentsCommander's own writers serialize through a file lock and only the dedicated Settings compare-and-set action can change it. An out-of-process editor that ignores that lock remains last-writer authority.
- AC tolerates unknown fields (`serde` skips them) so adding a field will not break an older binary, but the older binary will not honor it.

## Example

A minimal `settings.json`:

```json
{
  "defaultShell": "powershell.exe",
  "defaultShellArgs": ["-NoLogo"],
  "agents": [
    {
      "id": "claude",
      "label": "Claude Code",
      "command": "claude",
      "color": "#E87B35"
    },
    {
      "id": "codex",
      "label": "Codex",
      "command": "codex",
      "color": "#10A37F"
    },
    {
      "id": "gemini",
      "label": "Gemini",
      "command": "gemini",
      "color": "#4285F4"
    }
  ],
  "telegramBots": [],
  "raiseTerminalOnClick": true,
  "voiceToTextEnabled": false,
  "geminiApiKey": "",
  "geminiModel": "gemini-2.5-flash",
  "voiceAutoExecute": true,
  "voiceAutoExecuteDelay": 15,
  "themeLight": false,
  "specBoardEnabled": false,
  "terminalSnapshotsEnabled": false
}
```

## Top-level fields

### Shell

| Field | Type | Default | Description |
|---|---|---|---|
| `defaultShell` | string | `powershell.exe` (Win) / `/bin/bash` (Unix) | The shell binary AC spawns for plain sessions. |
| `defaultShellArgs` | string[] | `["-NoLogo"]` (Win) / `[]` (Unix) | Args passed to `defaultShell`. |

### Coding agents

| Field | Type | Default | Description |
|---|---|---|---|
| `agents` | `AgentConfig[]` | See example | The dropdown of available coding agents. |
| `agentAutoUpdateByCommand` | object | `{}` | Per-coding-agent-command answer to the startup update prompt. Keys are coding-agent commands (for example `claude`, `codex`); `true` means AC updates that command at startup without asking again, `false` means it never asks again and never updates. An absent key means AC asks on the next startup. See [Coding agent auto-update](../features/agent-auto-update.md). |

Besides the GUI Settings dialog and Onboarding, `agents[]` has a scriptable writer: the [`coding-agent`](cli.md#coding-agent) CLI verb (`list`/`show`/`catalog`/`add`/`update`/`remove`). It writes safely whether or not the GUI is running.

`AgentConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `id` | string | — | Stable internal id. Used by `create-agent --launch <id>`. |
| `label` | string | — | Display name in the launcher dropdown. |
| `command` | string | — | Binary to spawn. Resolved against PATH unless absolute. |
| `color` | string | — | CSS hex color for sidebar accent. |
| `envs` | `CodingAgentEnv[]` | `[]` | Environment rows applied at spawn. See below. |
| `isolatedHome` | bool | `false` | Provide an isolated `CODEX_HOME` at spawn (Codex). |
| `instructionsFilename` | string \| null | `null` | Bare `.md` filename AC writes into the agent root at launch. |
| `contextRegex` | string \| null | `null` | Regex pattern for the per-agent context scraper reading. Absent or blank disables the reading; the value is used byte-for-byte (never trimmed). |
| `backend` | `AgentBackendConfig` | `{ "kind": "local" }` | Runtime backend. See below. |
| `configSeed` | `ConfigSeedConfig` \| absent | absent | Optional config-folder seed copied into each replica at spawn. Absent (the default) means no seeding. See [Config seed](../features/config-seed.md). |

`CodingAgentEnv`:

| Field | Type | Default | Description |
|---|---|---|---|
| `key` | string | — | Environment variable name. |
| `value` | string | — | Environment variable value. |
| `source` | `"user" \| "system"` | `"user"` | Origin of the row. `system` marks AC-managed rows. |
| `enabled` | bool | `true` | Whether the row is applied. |

`AgentBackendConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `kind` | `"local" \| "container"` | `"local"` | `container` uses the Docker container transport. |
| `image` | string \| null | `null` | Per-agent Docker image override for the container runtime. Falls back to `AGENTSCOMMANDER_CONTAINER_IMAGE` at launch. |

`ConfigSeedConfig` (one optional object on a coding agent):

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Whether seeding runs for this agent. |
| `dest` | string | `""` | Destination folder name under the replica root (for example `.claude`). Validated as a safe name, no path separators or traversal. |

Seeding is active only when `enabled` is true and `dest` is non-empty. See [Config seed](../features/config-seed.md) for template precedence and token substitution.

### Coding agent profiles

Lettered launch variants (`A`, `B`, `C`, ...) per coding agent. See [Coding Agent Profiles](../features/coding-agent-profiles.md) for the feature; this is the `settings.json` schema.

| Field | Type | Default | Description |
|---|---|---|---|
| `codingAgentProfiles` | `CodingAgentProfilesConfig` | See below | The profile matrix and its defaults. |

`CodingAgentProfilesConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `schemaVersion` | number | `2` | Schema version. A version-1 object is upgraded and persisted on load, after a one-time v1 backup. |
| `profileSlots` | `{ <LETTER>: { label: string } }` | `{ "A": { "label": "" } }` | The defined profile letters. Alias: `letters`. |
| `defaultProfileByAgent` | `{ <agent>: <LETTER> }` | `{}` | Tier-4 fallback letter per agent matrix. Alias: `agentDefaults`. Rarely set by hand. |
| `profilesByAgent` | `{ <coding-agent-id>: { <LETTER>: ProfileCellConfig } }` | `{}` | The matrix: per coding agent, the cell for each letter. Alias: `matrix`. |
| `profileLabelsByAgent` | `{ <coding-agent-id>: { <LETTER>: string } }` | `{}` | Optional per-(agent, letter) label override. Empty inherits. |

`ProfileCellConfig` (one cell of `profilesByAgent`):

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Whether the cell participates in resolution. |
| `command` | string | `""` | Parameters appended to the agent's base command (not the binary). |
| `env` | `{ string: string }` | `{}` | Per-cell env, overlaid on the agent env (profile wins on a key clash). |
| `notes` | string | `""` | Free text. |

The per-agent and per-replica assignments do **not** live here. They are stored in each agent's `config.json` under `tooling`: the origin default (`tooling.defaultProfile`), the instance override (`tooling.profile`, legacy `tooling.instanceProfileOverride`), and the drift fingerprint (`tooling.profileContentHash`).

### Container coding agents

Host login reuse for coding agents running under the Container runtime. **This feature is in progress**: container agents cannot reach their `repo-*` directories yet. See [Container coding agents](../features/container-coding-agents.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `containerCredentialsFromHost` | bool | `true` | When a coding agent runs under the Container runtime, copy the host user's credential file for that agent (Claude: `~/.claude/.credentials.json`) into the replica config dir at spawn, set `CLAUDE_CONFIG_DIR` to it, stamp the container's first-run state (onboarding complete, `/workspace` trusted), and delete the copy on teardown. When false, AC copies, injects, and stamps nothing, and you supply credentials yourself (for example a `CLAUDE_CODE_OAUTH_TOKEN` env row). Claude Code only today. |

The copied file is a full-account credential (access token plus long-lived refresh token) in plaintext. Read [Security model → Container coding agents](../security.md#container-coding-agents-copied-host-credentials) before you leave this on.

### Terminal snapshots

| Field | Type | Default | Description |
|---|---|---|---|
| `terminalSnapshotsEnabled` | bool | `false` | Permit identity-authorized Root Agents and same-workgroup Coordinators to read a live backend terminal viewport as JSON or PNG. |

This is a disclosure gate, not a display preference. Terminal screens can contain passwords, tokens, source code, prompts, and personal data. AgentsCommander performs no automatic redaction.

Use **Settings > General > Terminal snapshots > Allow authorized terminal snapshots** to change it. The UI calls a dedicated idempotent compare-and-set operation with the value that was current when the modal opened. If another window or process changed the gate, a stale save conflicts and reloads the authoritative value instead of re-enabling it.

Every whole-settings writer preserves the current gate and cannot opt in. Old settings files deserialize the absent field as `false`, but the snapshot service is stricter: the on-disk key and managed in-memory value must both be exactly `true` at initial and final authorization. A missing key, duplicate key, malformed JSON, wrong type, unreadable file, or linked file fails closed as `terminal_snapshots_disabled`.

Direct out-of-process edits that ignore AgentsCommander's settings lock remain last-writer authority. If you edit this field by hand, stop the app first and keep the value a JSON boolean.

See [Terminal snapshots](../features/terminal-snapshots.md) for authorization, content, output, and cleanup behavior.

### Projects

Each registered project is stored in two forms: a canonical absolute path (the existing fields) and a portable path relative to the folder holding the running binary (the companion fields, added for [portable instances](../features/portable-instances.md#portable-project-paths)). The three absolute fields and their three companions are index-aligned.

| Field | Type | Default | Description |
|---|---|---|---|
| `projectPath` | string \| null | `null` | Legacy single-project field: the first active registration's absolute path. Kept for backward compat. |
| `projectPathRelativeToInstance` | string \| null | `null` | Companion of `projectPath`. Portable form relative to the binary's directory, or `null` when there is no portable form. |
| `projectPaths` | string[] | `[]` | All active projects registered in the sidebar. New entries appended by `new-project` / `open-project`. |
| `projectPathsRelativeToInstance` | (string \| null)[] | `[]` | Companion array of `projectPaths`: one slot per entry, same length and order. `null` where an entry has no portable form. |
| `archivedProjectPaths` | string[] | `[]` | Absolute paths of archived (registered but hidden) projects. |
| `archivedProjectPathsRelativeToInstance` | (string \| null)[] | `[]` | Companion array of `archivedProjectPaths`: same length and order. |

**Companion format.** A companion string is relative to the directory of the running executable (see [Portable instances](../features/portable-instances.md#portable-project-paths)), always written with `/` separators on every OS. `.` means the instance folder itself; `..` is allowed as long as it does not climb above the filesystem root. A project on a different Windows drive or UNC share than the binary has no relative form, so its companion slot is `null` and it stays absolute-only.

**Array alignment.** Each plural companion array has exactly the same length and index meaning as its absolute array: slot `i` in `projectPathsRelativeToInstance` is the portable form of `projectPaths[i]`, or `null`. A length mismatch, an orphan companion (a companion present while its absolute field is absent), a wrong-typed field, or a non-null companion beside a `null` primary is structural corruption (see below).

**Legacy migration.** A `settings.json` written by an older build has the three absolute fields and no companions. AC loads it unchanged (absolute-only) and adds a companion only after that project successfully validates, at the first reconciliation boundary or an explicit register/archive operation. Absent companions are valid legacy metadata, never corruption.

**Resolution at load (fail-closed).** On every load AC resolves and validates both candidates for each registration. Validation canonicalizes on the filesystem and requires an existing directory that is either a project containing `.ac/` or a legacy collection root with a project child. The per-registration outcome:

| Absolute side | Relative side | Result |
|---|---|---|
| valid | absent, invalid, or unavailable | select the absolute path; add or repair the companion after validation |
| invalid or missing | valid | select the relative path; refresh the stale absolute side |
| valid | valid, same directory | select one absolute path (prefers the absolute spelling) |
| valid | valid, different directory | conflict: select neither, mutate nothing, raise one sticky red toast with both paths |
| invalid | invalid | load issue: select neither, preserve both raw values |
| valid | present, but no instance base available | evaluate the absolute side only; keep the relative value for a later normal launch |

"Same" and "different" are decided by filesystem identity, not string comparison, so symlinks and Windows case/alias spellings collapse to one directory. Only validated canonical absolute paths reach startup restoration, team discovery, archive/session gates, and the sidebar. Unresolved or conflicting entries are filtered from the runtime lists but preserved on disk.

**Atomic reconciliation.** When a load selects a path whose companion must be added, repaired, or normalized, AC rewrites only the affected field group (active or archived) through the existing atomic writer (temp file plus rename with retry). Writes are atomic and never torn, but there is no `fsync`, so this is not a power-loss durability guarantee. Generic settings saves use a preserve mode that copies the six raw project fields from disk verbatim rather than rebuilding them, so an unrelated save can never re-pair, reorder, or drop project metadata. A structurally malformed project field blocks all project-list reconciliation and mutation while unrelated settings saves still succeed; the malformed bytes are retained and reported, not normalized.

**Archive pairing.** Archiving, unarchiving, and removing a project move or delete the whole pair (absolute plus companion) together, preserving array order. Archived entries carry their own companion array with the same alignment rules as the active one.

**Downgrade.** An older AgentsCommander build ignores the companion fields and reads only the absolute fields, so a downgraded install still opens your projects. The caveat: if a dual-path conflict exists, the old build does not see it (it reads only the absolute side) and therefore loses the newer build's fail-closed protection for that registration. Resolve conflicts before downgrading.

### Resource monitor

| Field | Type | Default | Description |
|---|---|---|---|
| `resourceMonitorEnabled` | bool | `true` | Master switch for the resource monitor. |
| `maxConcurrentAgentProcesses` | u32 | `32` | Cap on concurrently running agent processes. |
| `resourceWatchdogAction` | `"warn" \| "killGroup"` | `"warn"` | Action when a threshold trips. |
| `agentGroupWarnPrivateBytes` | u64 | `8589934592` (8 GiB) | Private bytes at which the agent group warns. |
| `agentGroupKillPrivateBytes` | u64 | `12884901888` (12 GiB) | Private bytes at which the agent group is killed. |
| `agentProcessKillPrivateBytes` | u64 | `12884901888` (12 GiB) | Private bytes at which a single agent process is killed. |
| `resourceKeepLastSnapshot` | bool | `true` | Keep the last snapshot. |
| `resourceBackoffPolling` | bool | `true` | Use backoff polling. |

### Git status sweeper

| Field | Type | Default | Description |
|---|---|---|---|
| `gitSweepConcurrency` | number | `1` | How many repositories the global git sweeper inspects at once. Clamped to `1..=4` when read. `1` is strictly sequential, which is what bounds concurrent `git.exe`; raise it to `2` only if one slow repository is delaying the others. |
| `gitSweepMinIntervalSecs` | number | `10` | Lower bound, in seconds, on one sweeper round. Clamped to `1..=3600` when read; `0` is raised to `1`. The effective period is `max(this, round duration)`, so on a large workgroup set the round duration dominates and this never fires. |

Both are manual-only (no UI) and are read from the in-memory settings, so an edit takes effect on the next **restart**.

### Window & UI

| Field | Type | Default | Description |
|---|---|---|---|
| `sidebarAlwaysOnTop` | bool | `false` | Pin sidebar above other windows. |
| `mainAlwaysOnTop` | bool | `false` | Pin the unified main window. |
| `raiseTerminalOnClick` | bool | `true` | Raise the terminal window when clicking a session. |
| `mainSidebarWidth` | number | platform-default | Sidebar pane width inside the main window. Clamped to `[200, 600]`. |
| `mainSidebarSide` | `"left" \| "right"` | `"right"` | Side of the main window where the sidebar lives. |
| `mainZoom` / `terminalZoom` / `sidebarZoom` / `guideZoom` | number | `1.0` | Per-window zoom (1.0 = 100%). |
| `mainGeometry` / `sidebarGeometry` / `terminalGeometry` | object \| null | `null` | Persisted window geometry. AC writes these on close. |
| `themeLight` | bool | `false` | Light theme on; dark theme when false. Fresh and missing values default to dark. |
| `specBoardEnabled` | bool | `false` | Shows the Spec Board toolbar button when true. This only controls the sidebar toolbar entrypoint; backend Spec Board commands remain callable and this is not an access-control or security boundary. |
| `sidebarStyle` | string | `"noir-minimal"` | Sidebar visual variant. Options: `noir-minimal`, `card-sections`, `command-center`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`. |
| `soundsEnabled` | bool | `true` | Master switch for all app-emitted sounds. |
| `teamIdleBeepEnabled` | bool | `true` | Beep when a team transitions from busy → all-idle. Gated by `soundsEnabled`. |
| `coordSortByActivity` | bool | `false` | Sort the coordinator quick-access list by most-recent activity. |
| `screenshotCaptureHotkey` | string | `"Ctrl+Q"` | Native global hotkey for screenshot capture. One modifier plus one key; only `Ctrl` (or `Control`) and a single letter or digit are accepted. Windows-only. See [Screenshot capture](../features/screenshot-capture.md). |
| `mainResourceMonitorAttached` | bool | `false` | Whether the Resource Monitor occupies the main central pane instead of the terminal. Restored on startup. |
| `alwaysShowSelectedWorkgroup` | bool | `true` | Keep the selected workgroup visible in the sidebar. |
| `railCollapsedProjects` | string[] | `[]` | Rail project sections the user collapsed by clicking their header. Entries are frontend-normalized project paths (lowercase, forward slashes, no trailing slash). Written only by the dedicated rail collapse action; whole-settings writers restore it from live memory. |
| `railFavoritesCollapsed` | bool | `false` | Collapsed state of the rail's cross-project Favorites section. Same protection as `railCollapsedProjects`. |

### Coordinator wake state

| Field | Type | Default | Description |
|---|---|---|---|
| `restoreCoordinatorWakeState` | bool | `false` | On app start, wake coordinators whose PTY was awake at shutdown. Non-coordinators always stay asleep until clicked. |

### Session auto-close

Idle teams (coordinators plus agent-owned sessions) close themselves after a timeout. Ad-hoc shells are never auto-closed. See [Session auto-close](../features/session-auto-close.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `coordinatorAutoCloseEnabled` | bool | `true` | Master switch for auto-close. When false, idle teams are never closed (the idle badge still shows). |
| `coordinatorAutoCloseMinutes` | u32 | `60` | Idle minutes before a team is auto-closed. `0` also disables auto-close. |
| `coordinatorAutoCloseSkipTelegramAssigned` | bool | `false` | When true, auto-close skips sessions with Telegram assigned. Other sessions keep following the normal auto-close rules. |
| `coordinatorCascadeCloseEnabled` | bool | `true` | When true, manually closing a coordinator also closes its team agents (cascade). When false, only the coordinator closes. |
| `coordinatorIdleBadgeYellowMinutes` | u32 | `30` | Idle minutes at which the coordinator idle badge turns yellow. |
| `coordinatorIdleBadgeRedMinutes` | u32 | `60` | Idle minutes at which the coordinator idle badge turns red. |

### Voice-to-text

| Field | Type | Default | Description |
|---|---|---|---|
| `voiceToTextEnabled` | bool | `false` | Master switch for the mic button. |
| `geminiApiKey` | string | `""` | Gemini API key. Plaintext — protect your account. |
| `geminiModel` | string | `gemini-2.5-flash` | Transcription model. |
| `voiceAutoExecute` | bool | `true` | Press Enter automatically after transcription. |
| `voiceAutoExecuteDelay` | u32 | `15` | Seconds to wait before pressing Enter. |

See [Voice-to-text setup](../integrations/voice.md).

### Telegram

| Field | Type | Default | Description |
|---|---|---|---|
| `telegramBots` | object[] | `[]` | List of configured bots. Each has `id`, `label`, `token`, `chatId`. |
| `telegramNetworkPollErrorLogging` | object | See below | Log severity for transient and sustained Telegram `getUpdates` network failures. Non-network poll failures still log at `error`. |

`TelegramNetworkPollErrorLogging`:

| Field | Type | Default | Description |
|---|---|---|---|
| `firstFailureLevel` | `"debug" \| "warn" \| "error"` | `"warn"` | Level for the first failure of a sequence. |
| `transientRepeatLevel` | `"debug" \| "warn" \| "error"` | `"debug"` | Level for repeating failures inside the transient window. |
| `sustainedLevel` | `"debug" \| "warn" \| "error"` | `"error"` | Level once the failure is sustained. |
| `sustainedAfterSeconds` | u64 | `60` | Seconds of failure before the sustained level applies. |
| `sustainedRepeatSeconds` | u64 | `60` | Repeat interval at which the sustained level is re-emitted. |

See [Telegram bridge setup](../integrations/telegram.md).

### Web server (opt-in)

| Field | Type | Default | Description |
|---|---|---|---|
| `webServerEnabled` | bool | `false` | Enable the embedded HTTP / WebSocket server. |
| `webServerPort` | u16 | platform-default per binary suffix | Listening port. |
| `webServerBind` | string | `"127.0.0.1"` | Bind address. Use `"0.0.0.0"` only if you understand the implications. |

#### Web Remote Access on a trusted LAN

Web Remote Access is the embedded HTTP/WebSocket listener controlled by the
`webServer*` settings. It is not the Control-plane API for Docker or
distributed agents. The API's IP/ADDRESS display and `apiServerEnabled`,
`apiServerBind`, and `apiServerPort` do not configure the web listener.

The safe defaults remain `webServerEnabled: false` and
`webServerBind: "127.0.0.1"`. External access is an explicit opt-in, and it
exposes live terminal content to every party that can reach and authenticate to
the listener.

To configure Web Remote Access for a trusted LAN:

1. Close AC. Use the [File location](#file-location) section above to find the
   per-instance `settings.json` next to the binary. Do not edit a global or
   shared `.ac/` file.
2. Change only the existing `webServerBind` and `webServerPort` keys. Do not
   replace the whole JSON document. Substitute the host's real private LAN
   IPv4 address and the listening port you chose:

   ```json
   {
     "webServerBind": "192.168.1.42",
     "webServerPort": 9877
   }
   ```

   A concrete private LAN address is preferred because it limits the listener
   to the intended network adapter. `0.0.0.0` listens on every available
   interface and is not the recommended LAN configuration. If an experienced
   operator deliberately uses it, make the firewall scope even more
   restrictive. `9877` is only an example: named binary instances can have
   different profile-aware defaults.
3. Restart AC so the manual values take effect. Then use the existing Web
   Remote Access enable/start control to turn on the web listener. This does
   not add a bind or port UI, and the adjacent Control-plane API controls do
   not configure Web Remote Access.

If the host firewall blocks the trusted LAN client, create an inbound rule for
the selected TCP `webServerPort` only. Limit it to the Private profile, the
selected local LAN address and port, and the intended client IP address or
subnet as the remote scope. Do not use an Any-profile, Any-remote-address,
public-network, or internet-facing rule. A firewall rule permits network
reachability; it does not authenticate a user.

The per-instance `web-token.txt` file is the Web Remote Access credential. It
is separate from the CLI `master-token.txt` file and from Control-plane API
client tokens. Treat it, and any URL or browser state that carries it, as a
password: use it only with a trusted client, never commit it, paste it into
tickets, chat, logs, or screenshots, and never use it as a firewall substitute.
Use the existing local Web Remote Access flow to obtain and authenticate with
the token. Do not invent a URL parameter or token-rotation procedure.

After starting Web Remote Access, verify on the host that a listener exists on
the chosen `<LAN-IP>:<webServerPort>`. From a second trusted device on the
allowed LAN, browse to that host and port, complete the normal web-token
authentication, and confirm that the expected terminal session is visible. If
the remote connection fails while the local listener is correct, re-check the
selected private address, port, network profile, firewall remote scope, and
that both devices are on the same trusted LAN.

Terminal content can contain passwords, tokens, source code, prompts, and
personal data. Web Remote Access performs no automatic redaction. Leave the
listener off when you do not need it, and remove or disable the narrowly scoped
firewall allowance when finished.

### Control-plane API server (opt-in)

In-daemon control-plane API server for Docker/distributed agents. Default off: no new listening socket unless the operator opts in.

| Field | Type | Default | Description |
|---|---|---|---|
| `apiServerEnabled` | bool | `false` | Enable the control-plane API server. |
| `apiServerPort` | u16 | profile-aware default per binary suffix | Listening port. |
| `apiServerBind` | string | `"127.0.0.1"` | Bind address. Any non-loopback bind logs a loud startup warning. |

See [`api-client`](cli.md#api-client) for minting and revoking control-plane client tokens.

### Brief auto-title

| Field | Type | Default | Description |
|---|---|---|---|
| `autoGenerateTaskTitle` | bool | `true` | When a coordinator session spawns and the brief has no `title:`, AC injects a prompt asking the agent to add one. |

### Templates

| Field | Type | Default | Description |
|---|---|---|---|
| `agentTemplatesPath` | string \| null | `null` | Local agent-templates root for the role-template picker. Empty/missing → default `<config-dir>/agent-templates/`. Relative → resolved against `<config-dir>/`. This does not control the Agency cache at `<config-dir>/agency-agents_templates`. |

### Self-handoff

| Field | Type | Default | Description |
|---|---|---|---|
| `autoSelfClearEnabled` | bool | `true` | Global master for auto self-handoff-and-clear. `false` turns it off for every agent. When `true`, the class-aware default applies (ON for coordinator/Root, OFF for specialists), subject to the per-agent override below. |
| `autoSelfClearByAgent` | `{ <agent-name>: bool }` | `{}` | Per-agent override of the class default, keyed by agent name (same key as `defaultProfileByAgent`). Applies only while the global master is on; absent = use the class default. |

### Watchers

Root-level context-scrape watcher patterns, keyed by watcher id. A pattern can apply to every agent, which the per-agent `contextRegex` shape cannot express. A malformed entry is skipped (one log line) instead of invalidating the whole settings file.

| Field | Type | Default | Description |
|---|---|---|---|
| `watchers` | `{ <id>: WatcherEntry }` | `{}` | Watcher patterns, resolved in key order against an 8-watcher budget. |
| `watchersGeometry` | object \| null | `null` | Geometry of the watcher activity window. |

`WatcherConfig` (one valid entry):

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Whether this watcher runs. |
| `mode` | `"state" \| "occurrence"` | — (required) | `state` is a reading, idempotent and gated; `occurrence` is an event, every match the frame diff declares evaluable counts. |
| `pattern` | string | — (required) | Match pattern. |
| `commands` | string[] \| null | `null` | Absent or null: reaches every configured agent. Present: only entries whose `command` executable stem matches exactly. Present and empty: reaches none. |
| `dedupe` | `"row" \| "capture" \| "none"` | `"row"` | What makes two occurrence matches "the same one" inside the dedupe window. |
| `dedupeWindowMs` | u64 | `2000` | Dedupe window in milliseconds. |
| `capturedAgainst` | string \| null | `null` | Free text (e.g. "claude 2.1.212"). Never validated, never parsed. |

### Update notifications

| Field | Type | Default | Description |
|---|---|---|---|
| `npmUpdateNotificationsEnabled` | bool | `true` | Check npm on startup (at most once per 24h) and notify in-app when a newer published version is available. |

### Tokens

| Field | Type | Default | Description |
|---|---|---|---|
| `rootToken` | string \| null | `null` | Root token that bypasses routing checks in `send`. Treat as a master credential. |

### Onboarding

| Field | Type | Default | Description |
|---|---|---|---|
| `onboardingDismissed` | bool | `false` | Whether the first-run wizard was dismissed. |

### Logging

| Field | Type | Default | Description |
|---|---|---|---|
| `logLevel` | string \| null | `null` | One of `error`, `warn`, `info`, `debug`, `trace`. Applied live, no restart. An invalid value, a legacy filter string, or `null` falls back to `info`. The `RUST_LOG` env var, if set, overrides this and freezes the live selector until restart. |
| `activityLogEnabled` | bool | `false` | Enable the activity log. |

See [Log filtering](log-filtering.md).

## Migration carriers

| Field | Type | Description |
|---|---|---|
| `startOnlyCoordinators` | bool \| null | Legacy name for `restoreCoordinatorWakeState`. Read on deserialize, dropped on next save. |
| `darkfactoryZoom` | number | Legacy zoom for the removed Dark Factory window. Retained for backwards-compat reads only. |

These will silently disappear from your `settings.json` on the next save after AC reads them.

## Validating a file

Use any JSON validator. AC will refuse to start if the file is not valid JSON and fall back to defaults; it does not silently overwrite a broken file. If you suspect corruption, rename to `settings.bad.json` and let AC regenerate a fresh default.

## See also

- [Portable instances](../features/portable-instances.md) — per-instance config rules
- [CLI reference](cli.md) — verbs that read/write this file
- [Terminal snapshots](../features/terminal-snapshots.md) - the default-off screen-content read capability
- [`PRIVACY.md`](../../PRIVACY.md) — what credentials live here and how they are transmitted
