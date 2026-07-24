# Settings reference

For developers editing `settings.json` by hand, or scripting AgentsCommander configuration. The full schema of the `settings.json` file AC reads at startup.

## File location

`settings.json` lives in the authoritative config directory:

| Binary | Config directory | Settings file |
|---|---|---|
| `/usr/bin/agentscommander` (canonical Linux DEB) | `$XDG_CONFIG_HOME/agentscommander`, or `$HOME/.config/agentscommander` | `<config-dir>/settings.json` |
| `C:\tools\agentscommander.exe` | `C:\tools\.agentscommander\` | `C:\tools\.agentscommander\settings.json` |
| `C:\work\agentscommander_team-a.exe` | `C:\work\.agentscommander_team-a\` | `C:\work\.agentscommander_team-a\settings.json` |
| (debug build) | adds `-dev` suffix | `…\.agentscommander-dev\settings.json` |

Raw Linux binaries and AppImages retain the same executable-relative portable
rule as other raw binaries. On Linux, startup requires the config root and
private directories to have mode `0700`; it creates or repairs
`settings.json` and other security-bearing regular files to mode `0600`.
Symlinks, hard-linked mutable files, special files, ownership mismatches, and
identity changes are rejected before settings are used. See
[Portable instances](../features/portable-instances.md) for the full rule.

## Editing rules

- The file is **JSON** (not JSONC, not YAML). Comments are not allowed.
- AC reads at startup and on `update_settings` IPC calls.
- If you edit `settings.json` **while the app is running**, your changes may be clobbered by the next in-memory save. For manual-only fields such as `specBoardEnabled`, edit while AC is closed, or reload settings before using any Settings save path.
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
  "specBoardEnabled": false
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

Besides the GUI Settings dialog and Onboarding, `agents[]` has a scriptable writer: the [`coding-agent`](cli.md#coding-agent) CLI verb (`list`/`show`/`catalog`/`add`/`update`/`remove`). It writes safely whether or not the GUI is running.

`AgentConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `id` | string | — | Stable internal id. Used by `create-agent --launch <id>`. |
| `label` | string | — | Display name in the launcher dropdown. |
| `command` | string | — | Binary to spawn. Resolved against PATH unless absolute. |
| `color` | string | — | CSS hex color for sidebar accent. |
| `configSeed` | `ConfigSeedConfig` \| absent | absent | Optional config-folder seed copied into each replica at spawn. Absent (the default) means no seeding. See [Config seed](../features/config-seed.md). |

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
| `themeLight` | bool | `true` | Light theme on; dark theme when false. |
| `specBoardEnabled` | bool | `false` | Shows the Spec Board toolbar button when true. This only controls the sidebar toolbar entrypoint; backend Spec Board commands remain callable and this is not an access-control or security boundary. |
| `sidebarStyle` | string | `"noir-minimal"` | Sidebar visual variant. Options: `noir-minimal`, `card-sections`, `command-center`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`. |
| `soundsEnabled` | bool | `true` | Master switch for all app-emitted sounds. |
| `teamIdleBeepEnabled` | bool | `true` | Beep when a team transitions from busy → all-idle. Gated by `soundsEnabled`. |
| `coordSortByActivity` | bool | `false` | Sort the coordinator quick-access list by most-recent activity. |

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

See [Telegram bridge setup](../integrations/telegram.md).

### Web server (opt-in)

| Field | Type | Default | Description |
|---|---|---|---|
| `webServerEnabled` | bool | `false` | Enable the embedded HTTP / WebSocket server. |
| `webServerPort` | u16 | platform-default per binary suffix | Listening port. |
| `webServerBind` | string | `"127.0.0.1"` | Bind address. Use `"0.0.0.0"` only if you understand the implications. |

### Brief auto-title

| Field | Type | Default | Description |
|---|---|---|---|
| `autoGenerateTaskTitle` | bool | `true` | When a coordinator session spawns and the brief has no `title:`, AC injects a prompt asking the agent to add one. |

### Templates

| Field | Type | Default | Description |
|---|---|---|---|
| `agentTemplatesPath` | string \| null | `null` | Local agent-templates root for the role-template picker. Empty/missing → default `<config-dir>/agent-templates/`. Relative → resolved against `<config-dir>/`. This does not control the Agency cache at `<config-dir>/agency-agents_templates`. |

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
- [`PRIVACY.md`](../../PRIVACY.md) — what credentials live here and how they are transmitted
