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
- If you edit `settings.json` **while the app is running**, your changes may be clobbered by the next in-memory save. Edit while AC is closed, or use the **Settings** UI.
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
      "color": "#E87B35",
      "gitPullBefore": false
    },
    {
      "id": "codex",
      "label": "Codex",
      "command": "codex",
      "color": "#10A37F",
      "gitPullBefore": false
    },
    {
      "id": "gemini",
      "label": "Gemini",
      "command": "gemini",
      "color": "#4285F4",
      "gitPullBefore": false
    }
  ],
  "telegramBots": [],
  "raiseTerminalOnClick": true,
  "voiceToTextEnabled": false,
  "geminiApiKey": "",
  "geminiModel": "gemini-2.5-flash",
  "voiceAutoExecute": true,
  "voiceAutoExecuteDelay": 15,
  "themeLight": false
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

`AgentConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `id` | string | — | Stable internal id. Used by `create-agent --launch <id>`. |
| `label` | string | — | Display name in the launcher dropdown. |
| `command` | string | — | Binary to spawn. Resolved against PATH unless absolute. |
| `color` | string | — | CSS hex color for sidebar accent. |
| `gitPullBefore` | bool | `false` | Run `git pull` in the working directory before launching. |
| `excludeGlobalClaudeMd` | bool | `false` | Claude-specific: auto-write `.claude/settings.local.json` with `claudeMdExcludes` on agent creation. |

### Projects

| Field | Type | Default | Description |
|---|---|---|---|
| `projectPath` | string \| null | `null` | Legacy single-project field. Kept for backward compat. |
| `projectPaths` | string[] | `[]` | All projects registered in the sidebar. New entries appended by `new-project` / `open-project`. |

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
| `sidebarStyle` | string | `"noir-minimal"` | Sidebar visual variant. Options: `noir-minimal`, `card-sections`, `command-center`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`. |
| `soundsEnabled` | bool | `true` | Master switch for all app-emitted sounds. |
| `teamIdleBeepEnabled` | bool | `true` | Beep when a team transitions from busy → all-idle. Gated by `soundsEnabled`. |
| `coordSortByActivity` | bool | `false` | Sort the coordinator quick-access list by most-recent activity. |

### Coordinator wake state

| Field | Type | Default | Description |
|---|---|---|---|
| `restoreCoordinatorWakeState` | bool | `false` | On app start, wake coordinators whose PTY was awake at shutdown. Non-coordinators always stay asleep until clicked. |

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

### RTK integration

| Field | Type | Default | Description |
|---|---|---|---|
| `injectRtkHook` | bool | `false` | Inject the RTK `PreToolUse` hook into every managed agent's `.claude/settings.local.json` at startup. |
| `rtkPromptDismissed` | bool | `false` | Suppress the "RTK detected — enable hook injection?" banner for the lifetime of this settings file. |

See [RTK integration](../features/rtk-integration.md).

### Brief auto-title

| Field | Type | Default | Description |
|---|---|---|---|
| `autoGenerateTaskTitle` | bool | `true` | When a coordinator session spawns and the brief has no `title:`, AC injects a prompt asking the agent to add one. |

### Templates

| Field | Type | Default | Description |
|---|---|---|---|
| `agentTemplatesPath` | string \| null | `null` | Local agent-templates root for the role-template picker. Empty/missing → default `<config-dir>/agent-templates/`. Relative → resolved against `<config-dir>/`. |

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
| `logLevel` | string \| null | `null` | Filter expression. Applied at startup if `RUST_LOG` is unset. Standard `env_logger` syntax (e.g. `info,agentscommander_lib::config::teams=trace`). |

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
