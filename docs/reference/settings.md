# Settings reference

For developers editing `settings.json` by hand, or scripting AgentsCommander configuration. The full schema of the `settings.json` file AC reads at startup.

## File location

`settings.json` lives in the configuration directory selected once at runtime. Selection depends on the exact binary version:

| Verified version and selected case | Config directory | Settings file |
|---|---|---|
| `v0.30.3`, executable parent and stem available for `C:\tools\agentscommander.exe` | `C:\tools\.agentscommander\` | `C:\tools\.agentscommander\settings.json` |
| `v0.30.3`, executable parent or stem unavailable, normal production identity | `$HOME/.agentscommander-new` | `$HOME/.agentscommander-new/settings.json` |
| `v0.33.0` and `main`, nonblank `AGENTSCOMMANDER_CONFIG_DIR` | Override value, verbatim | `<override>/settings.json` |
| `v0.33.0` and `main`, executable without an underscore suffix (for example `agentscommander.exe`), no override | `$HOME/.agentscommander` | `$HOME/.agentscommander/settings.json` |
| `v0.33.0` and `main`, `agentscommander_<suffix>.exe`, no override, adjacent folder writable | `<executable folder>/.agentscommander_<suffix>` | `<executable folder>/.agentscommander_<suffix>/settings.json` |

Published `v0.30.3` has no public override or writability probe; it does not fall back because a derivable adjacent path is read-only. The public override and writability-probe behavior are in `v0.33.0` and `main`; before relying on them for any other release, verify that exact release tag. See [Portable instances](../features/portable-instances.md#config-directory-rule) for the complete versioned contract.

On `v0.33.0` and `main`, `agentscommander_<suffix>.exe` never uses `$HOME`. If its adjacent folder cannot be written, it does not start. A conclusively unwritable folder gives a message that tells you to move the executable to a writable folder or set `AGENTSCOMMANDER_CONFIG_DIR`; when the write result is indeterminate, the message tells you to set `AGENTSCOMMANDER_CONFIG_DIR`. Neither a `v0.33.0` nor a `main` build moves, copies or merges settings from an older folder; each reads a folder only when its own rule selects it. To find and reuse them, see [Settings left by published releases](../features/portable-instances.md#settings-left-by-published-releases).

A future release renames `settings.json` to `settings.30.instance.no-git.json`. This is not implemented yet; see [File naming convention](file-naming.md).

## Editing rules

- The file is **JSON** (not JSONC, not YAML). Comments are not allowed.
- AC reads at startup and on `update_settings` IPC calls.
- If you edit `settings.json` **while the app is running**, your changes may be clobbered by the next in-memory save. For manual-only fields such as `specBoardEnabled`, edit while AC is closed, or reload settings before using any Settings save path.
- `terminalSnapshotsEnabled` is security-sensitive and defaults to `true`. AgentsCommander's own writers serialize through a file lock, and only the dedicated Settings compare-and-set action can change an explicit value. One exception follows from the default: in a legacy file with no `terminalSnapshotsEnabled` key, any unrelated whole-settings save materializes `true`, because an absent key already means enabled. Write an explicit `false` if you want the capability off. An out-of-process editor that ignores that lock remains last-writer authority.
- AC tolerates unknown fields (`serde` skips them) so adding a field will not break an older binary, but the older binary will not honor it.

## Recovering a previous version

AgentsCommander keeps a bounded history of previous `settings.json` versions beside the live file: `settings.backup.1.json` through `settings.backup.5.json`. Slot 1 is the version the most recent save replaced; slot 5 is the oldest kept. A save that produces bytes identical to the file already on disk does not rotate, so repeated no-op saves do not evict real history.

Recovery is a manual copy: while AgentsCommander is closed, copy the slot you want over `settings.json`. The slots hold the same secrets as `settings.json`, so treat them with the same care.

> A slot is written without a temp-and-rename, so a crash during rotation can leave
> `settings.backup.1.json` truncated. AgentsCommander does not report a truncated
> `settings.json` as an error: it logs the parse failure and starts from default
> settings, so a bad copy looks like a silently reset configuration, not a failure.
> Before starting AgentsCommander, confirm the file you copied is complete and valid
> JSON. If slot 1 is short or does not parse, use `settings.backup.2.json`, which holds
> the generation before it.

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
      "id": "antigravity",
      "label": "Antigravity",
      "command": "agy",
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

The `terminalSnapshotsEnabled: false` line above is an explicit opt-out, not the default. The default is `true`, and so is an absent key.

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
| `agentAutoUpdateByCommand` | object | `{}` | Per-coding-agent-command answer to the startup update prompt. Keys are coding-agent commands (for example `claude`, `codex`); `true` means AC updates that command at startup without asking again, `false` means it never asks again and never updates. An absent key means AC asks on the next startup. See [Coding agent auto-update](../features/agent-auto-update.md). Settings > Coding Agents shows the current value per update-capable agent in the read-only Auto-update table. The startup question writes this map once per coding agent per start: the first answer, from any window, is the one stored. |

Besides the GUI Settings dialog and Onboarding, `agents[]` has a scriptable writer: the [`coding-agent`](cli.md#coding-agent) CLI verb (`list`/`show`/`catalog`/`add`/`update`/`remove`). It writes safely whether or not the GUI is running.

The catalog entry's own `autoUpdate` field is inert: only `agentAutoUpdateByCommand` authorizes an update, keyed by the exact command string, and a changed command never inherits another command's answer. `updateCommands` likewise resolve only from the persisted catalog (project `.ac/coding-agents/agents.json` layered with `agents.local.json`); see [Coding agents § Managed catalog](../integrations/coding-agents.md#managed-catalog-base-local-overrides-and-migration).

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
| `blockingMenus` | `BlockingMenuEntry[]` \| absent | absent | Legacy. Moved to `settings-blocking-menus.local.json` on the first start after upgrade and then absent, unless the migration could not run (see [Menu guard](#menu-guard)); while present it applies as before. |
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

See [Context tracking](../features/context-tracking.md).

### Coding agent profiles

Lettered launch variants (`A`, `B`, `C`, ...) per coding agent. See [Coding Agent Profiles](../features/coding-agent-profiles.md) for the feature; this is the `settings.json` schema.

| Field | Type | Default | Description |
|---|---|---|---|
| `codingAgentProfiles` | `CodingAgentProfilesConfig` | See below | The profile matrix and its defaults. |

`CodingAgentProfilesConfig`:

| Field | Type | Default | Description |
|---|---|---|---|
| `schemaVersion` | number | `2` | Schema version. Older pre-v2 profile fields are no longer read: they are ignored. Before any save drops them, AC keeps the original once as `settings.pre-384-v1.json` and writes settings without them; the `open-project` and `new-project` CLI commands keep them and write no backup. |
| `profileSlots` | `{ <LETTER>: { label: string } }` | `{ "A": { "label": "" } }` | The defined profile letters. |
| `defaultProfileByAgent` | `{ <agent>: <LETTER> }` | `{}` | Tier-4 fallback letter per agent matrix. Rarely set by hand. |
| `profilesByAgent` | `{ <coding-agent-id>: { <LETTER>: ProfileCellConfig } }` | `{}` | The matrix: per coding agent, the cell for each letter. |
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

Host login reuse for coding agents running under the Container runtime. The container receives that agent's admissible repo mounts under `/repos`, and sees host credentials when enabled. See [Container coding agents](../features/container-coding-agents.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `containerCredentialsFromHost` | bool | `true` | When a coding agent runs under the Container runtime, copy the host user's credential file for that agent (Claude: `~/.claude/.credentials.json`) into the replica config dir at spawn, set `CLAUDE_CONFIG_DIR` to it, stamp the container's first-run state (onboarding complete, `/workspace` trusted), and delete the copy on teardown. When false, AC copies, injects, and stamps nothing, and you supply credentials yourself (for example a `CLAUDE_CODE_OAUTH_TOKEN` env row). Claude Code only today. |

The copied file is a full-account credential (access token plus long-lived refresh token) in plaintext. Read [Security model → Container coding agents](../security.md#container-coding-agents-copied-host-credentials) before you leave this on.

### Terminal snapshots

| Field | Type | Default | Description |
|---|---|---|---|
| `terminalSnapshotsEnabled` | bool | `true` | Permit identity-authorized Root Agents and same-room Orchestrators to read a live backend terminal viewport as JSON or PNG. On by default; set an explicit `false` to deny. |

This is a disclosure gate, not a display preference. Terminal screens can contain passwords, tokens, source code, prompts, and personal data. AgentsCommander performs no automatic redaction.

Use **Settings > General > Terminal snapshots > Allow authorized terminal snapshots** to change it. The UI calls a dedicated idempotent compare-and-set operation with the value that was current when the modal opened. If another window or process changed the gate, a stale save conflicts and reloads the authoritative value instead of re-enabling it.

A whole-settings writer preserves an explicit value and cannot flip `false` to `true`. It does write `true` into a legacy file whose key is absent, because both a fresh installation and an absent key already mean enabled.

The snapshot service reads the gate strictly at initial and final authorization, from the on-disk key and the managed in-memory value. An absent key passes as enabled; an explicit `false` denies. A duplicate key, malformed JSON, wrong type, unreadable file, or linked file fails closed as `terminal_snapshots_disabled`.

Direct out-of-process edits that ignore AgentsCommander's settings lock remain last-writer authority. If you edit this field by hand, stop the app first and keep the value a JSON boolean.

See [Terminal snapshots](../features/terminal-snapshots.md) for authorization, content, output, and cleanup behavior.

### Projects

Each registered project is stored as a canonical absolute path and may also have a portable companion relative to the selected instance base. For an adjacent configuration that base is the native executable's directory. Under the `v0.33.0` and `main` resolver, an absolute public config override uses the override directory's parent; published `v0.30.3` has no such override. A home fallback or `main` relative override has no base. The three absolute fields and their three companions are index-aligned.

| Field | Type | Default | Description |
|---|---|---|---|
| `projectPath` | string \| null | `null` | Legacy single-project field: the first active registration's absolute path. Kept for backward compat. |
| `projectPathRelativeToInstance` | string \| null | `null` | Companion of `projectPath`. Portable form relative to the selected instance base, or `null` when there is no portable form. |
| `projectPaths` | string[] | `[]` | All active projects registered in the sidebar. New entries appended by `new-project` / `open-project`. |
| `projectPathsRelativeToInstance` | (string \| null)[] | `[]` | Companion array of `projectPaths`: one slot per entry, same length and order. `null` where an entry has no portable form. |
| `archivedProjectPaths` | string[] | `[]` | Absolute paths of archived (registered but hidden) projects. |
| `archivedProjectPathsRelativeToInstance` | (string \| null)[] | `[]` | Companion array of `archivedProjectPaths`: same length and order. |

See [Project archiving](../features/project-archiving.md).

**Companion format.** A companion string is relative to the [selected instance base](../features/portable-instances.md#portable-project-paths), always written with `/` separators on every OS. `.` means the base itself; `..` is allowed as long as it does not climb above the filesystem root. A project on a different Windows drive or UNC share than the base has no relative form, so its companion slot is `null` and it stays absolute-only.

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

See [Resource monitor](../features/resource-monitor.md).

### Git status sweeper

| Field | Type | Default | Description |
|---|---|---|---|
| `gitSweepConcurrency` | number | `1` | How many repositories the global git sweeper inspects at once. Clamped to `1..=4` when read. `1` is strictly sequential, which is what bounds concurrent `git.exe`; raise it to `2` only if one slow repository is delaying the others. |
| `gitSweepMinIntervalSecs` | number | `10` | Lower bound, in seconds, on one sweeper round. Clamped to `1..=3600` when read; `0` is raised to `1`. The effective period is `max(this, round duration)`, so on a large room set the round duration dominates and this never fires. |
| `ciActivityEnabled` | bool | `true` | Whether the remote-activity sweeper asks GitHub whether a run for each room repository's current branch at its exact `HEAD` is unfinished. A resolved default branch always reports no CI activity and injects no `ci-started` or `ci-finished` notice into the orchestrator, as does a non-default branch that points at the default branch's tip; an unresolved default branch keeps the ordinary behaviour. No clamp. Read at startup to decide whether the sweeper thread exists at all, and once per round otherwise. Needs a **restart**. |
| `ciActivityNotifyOrchestrator` | bool | `true` | Whether a CI state change may inject a notice into the room's orchestrator. No clamp. Needs a **restart**. |
| `branchStalenessEnabled` | bool | `true` | Whether the sweeper asks whether the repository's default branch holds commits this checkout does not. No clamp. Read once per round. Needs a **restart**. |
| `branchStalenessNotifyOrchestrator` | bool | `true` | Whether a staleness answer may inject a notice into the room's orchestrator. No clamp. Needs a **restart**. |
| `ciSweepMinIntervalSecs` | number | `30` | Seconds between CI questions for a key that is not running. Clamped to `10..=3600` when read. A key whose last confirmed state is `Running` uses a fixed `10`-second cadence instead, because `Finished` is the transition that unblocks an agent. Needs a **restart**. |
| `branchStalenessIntervalSecs` | number | `260` | Seconds between staleness questions. Clamped to `10..=3600` when read. Needs a **restart**. |

See [Sidebar guide](../features/sidebar-guide.md).

The remote-activity feature ships on. With no `gh` on `PATH` it costs one `PATH` read at startup and nothing else: no `gh` process, no network, and a sidebar identical to before.

The chip colour and the notice have separate switches, because they have different costs: the colour is passive, while the notice is injected into a working agent's terminal and consumes its context.

`branchStalenessEnabled` is a sibling of `ciActivityEnabled`, not a child, so you can ask either question without the other. With both `enabled` dials off, no sweeper thread starts at all.

The two `gitSweep*` dials are manual-only (no UI) and are read from the in-memory settings, so an edit takes effect on the next **restart**.

### Window & UI

| Field | Type | Default | Description |
|---|---|---|---|
| `sidebarAlwaysOnTop` | bool | `false` | Pin sidebar above other windows. |
| `mainAlwaysOnTop` | bool | `false` | Pin the unified main window. |
| `raiseTerminalOnClick` | bool | `true` | Raise the terminal window when clicking a session. |
| `mainSidebarWidth` | number | platform-default | Sidebar pane width inside the main window. Clamped to `[200, 600]`. |
| `mainSidebarSide` | `"left" \| "right"` | `"right"` | Side of the main window where the sidebar lives. |
| `mainZoom` / `terminalZoom` / `sidebarZoom` | number | `1.0` | Per-window zoom (1.0 = 100%). |
| `mainGeometry` | object \| null | `null` | Saved normal bounds of the unified main window: `x`, `y`, `width` and `height` of the outer window, in physical pixels. AC seeds and updates it only from a persisted value or a normal (not maximized, fullscreen or minimized) observation. See [Main window placement](#main-window-placement). |
| `mainWindowDisplayState` | `"normal"` \| `"maximized"` | `"normal"` | Saved display state of the unified main window. The key is optional; a missing value reads as `normal`. Fullscreen and minimized are never saved. |
| `sidebarGeometry` / `terminalGeometry` | object \| null | `null` | Legacy keys from the two-window layout. AC reads them at load; when the file has no `mainGeometry` and no local overlay pins it, `terminalGeometry` seeds `mainGeometry`. |
| `themeLight` | bool | `false` | Light theme on; dark theme when false. Fresh and missing values default to dark. |
| `specBoardEnabled` | bool | `false` | Shows the Spec Board toolbar button when true. This only controls the sidebar toolbar entrypoint; backend Spec Board commands remain callable and this is not an access-control or security boundary. |
| `sidebarStyle` | string | `"noir-minimal"` | Sidebar visual variant. Options: `noir-minimal`, `card-sections`, `command-center`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`. |
| `soundsEnabled` | bool | `true` | Master switch for all app-emitted sounds. |
| `teamIdleBeepEnabled` | bool | `true` | Beep when a team transitions from busy → all-idle. Gated by `soundsEnabled`. |
| `coordSortByActivity` | bool | `false` | Sort the orchestrator quick-access list by most-recent activity. |
| `screenshotCaptureHotkey` | string | `"Ctrl+Q"` | Native global hotkey for screenshot capture. One modifier plus one key; only `Ctrl` (or `Control`) and a single letter or digit are accepted. Windows, macOS and Linux/X11. See [Screenshot capture](../features/screenshot-capture.md). |
| `sidebarCompactHotkey` | string | `"Ctrl+Shift+E"` | Hotkey that toggles the compact sidebar. Accepted range `Ctrl+Shift+<A-Z>` (parts are case-insensitive; the first part may be `Ctrl` or `Control`), excluding the reserved letters `W`, `R`, `C`, `V`. An invalid value blocks the save with an error naming the field; it is not repaired. |
| `mainResourceMonitorAttached` | bool | `false` | Whether the Resource Monitor occupies the main central pane instead of the terminal. Restored on startup. |
| `alwaysShowSelectedWorkgroup` | bool | `true` | Keep the selected room visible in the sidebar. |
| `railCollapsedProjects` | string[] | `[]` | Rail project sections the user collapsed by clicking their header. Entries are frontend-normalized project paths (lowercase, forward slashes, no trailing slash). Written only by the dedicated rail collapse action; whole-settings writers restore it from live memory. |
| `railFavoritesCollapsed` | bool | `false` | Collapsed state of the rail's cross-project Favorites section. Same protection as `railCollapsedProjects`. |

#### Main window placement

The main window's placement is the pair `mainGeometry` + `mainWindowDisplayState`. `mainGeometry` holds the last normal rectangle: a maximized window records `"maximized"` in `mainWindowDisplayState` but keeps the previous rectangle, while a fullscreen or minimized observation changes nothing. On a first run with nothing saved yet, AC samples the window once at startup and keeps that rectangle as the normal one, so launch, maximize and close does persist the placement; a saved rectangle always wins over that sample, and if the startup sample is itself maximized, fullscreen or minimized AC keeps no rectangle and saves nothing until the window is next seen normal. That pairing is what returns the window to its pre-maximized size and position later.

While the app runs, AC coalesces moves and resizes for 500 ms and then saves both keys. On every accepted quit route it awaits one flush of the latest placement for at most 2 seconds before quitting; a flush that times out or fails logs the failure and the quit continues, so the next start uses the placement already on disk.

At startup AC converts the saved physical rectangle to logical pixels and restores it when it is still visible on a connected monitor (more than a 50 px overlap in both axes). An off-screen rectangle falls back to the centered default: no larger than 1400x900, centered on the primary monitor (with no monitor reported, AC assumes a 1920x1080 screen at the origin). A saved `"maximized"` state is requested after the window opens; if maximizing fails, AC logs a warning and leaves the window normal. On a testable build, a launch placement (`AC_TEST_WINDOW_PLACEMENT` or its CLI flags) overrides both saved keys for that launch. See [Windowing and multimonitor tests](../testing/10-windowing-and-multimonitor.md).

Every writer that saves the whole settings object keeps a present, non-`null` on-disk value of each key, so unrelated settings saves do not clobber a hand-edited placement; a missing key or an explicit `null` counts as absent and can be filled from the caller's value. The narrow placement command is the only writer that changes the values deliberately. A window move or an accepted quit rewrites them, so edit these keys while AC is closed.

If `settings.local.json` pins either key, the placement command refuses with `main_window_placement_overlay_pinned` and changes neither the file nor memory. On quit, AC shows one alert per accepted close round: `Window placement is pinned by the local settings overlay and was not saved.` Quitting continues after the alert.

### On app restart

| Field | Type | Default | Description |
|---|---|---|---|
| `restoreCoordinatorWakeState` | bool | `false` | On app start, wake orchestrators whose PTY was awake at shutdown. Non-orchestrators stay asleep until clicked, unless `restartResumeWakeWorkingAgents` is also on. |
| `restartResumeWakeWorkingAgents` | bool | `false` | On app start, also wake non-orchestrator replicas whose last recorded state was working (`status` Running or Active with `waitingForInput` false in `sessions.json`). Off by default, so the standing policy that non-orchestrators stay asleep until clicked is unchanged unless you opt in. |
| `restartResumeOrchestratorPrompt` | string | `AgentsCommander was restarted. Continue with the work that was in flight.` | Typed into an orchestrator that was working at shutdown, once its PTY is back and its prompt is up. Empty means type nothing. Has no effect unless `restoreCoordinatorWakeState` is on, because a sleeping orchestrator has no prompt to type into. |
| `restartResumeAgentPrompt` | string | `.` | Typed into a non-orchestrator replica that was working at shutdown, once its PTY is back and its prompt is up. Empty means type nothing. Has no effect unless `restartResumeWakeWorkingAgents` is on. |

Each checkbox tries to bring back one class of session, and the matching text is what AgentsCommander tries to type into the ones that were mid-task. A text setting does nothing on its own: if the checkbox for that class is off, nothing in that class is woken and nothing is typed. A session that was idle at shutdown is never typed into: nudging an agent that was not mid-task starts work nobody asked for. A session you restarted, or whose conversation you cleared from the phone, is never typed into, even when it comes back. Text is typed at most once per app start, and only once the agent is back at its prompt and has actually printed something; an agent that never gets there within the time AgentsCommander allows is left alone rather than typed into blind.

### Session auto-close

Idle teams (orchestrators plus agent-owned sessions) close themselves after a timeout. Ad-hoc shells are never auto-closed. See [Session auto-close](../features/session-auto-close.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `coordinatorAutoCloseEnabled` | bool | `true` | Master switch for auto-close. When false, idle teams are never closed (the idle badge still shows). |
| `coordinatorAutoCloseMinutes` | u32 | `60` | Idle minutes before a team is auto-closed. `0` also disables auto-close. |
| `coordinatorAutoCloseSkipTelegramAssigned` | bool | `false` | When true, auto-close skips sessions with Telegram assigned. Other sessions keep following the normal auto-close rules. |
| `coordinatorCascadeCloseEnabled` | bool | `true` | When true, manually closing an orchestrator also closes its team agents (cascade). When false, only the orchestrator closes. |
| `coordinatorIdleBadgeYellowMinutes` | u32 | `30` | Idle minutes at which the orchestrator idle badge turns yellow. |
| `coordinatorIdleBadgeRedMinutes` | u32 | `60` | Idle minutes at which the orchestrator idle badge turns red. |

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

### Co-managed (Jev)

Six top-level keys, all in `settings.json`, all global. They configure the classifier; the per-room on/off flag is **not** here, it lives under the room root. See [Co-managed rooms](../features/co-managed-rooms.md).

| Field | Type | Default | Description |
|---|---|---|---|
| `jevApiKey` | string | `""` | Jev API key. Plaintext — protect your account. **Empty means the feature is inert**, in every room, whatever the room flags say. |
| `jevModel` | string | `jev-1.13.0` | Classification model. A **pinned** version, not a floating tag. |
| `jevEndpoint` | string | `https://api.typesafe.ai/v1/systemone` | The Typesafe System One endpoint the candidate text and catalog questions are sent to. |
| `jevTimeoutSecs` | u64 | `20` | Request timeout, in seconds. A timeout is an abstention. |
| `jevThreshold` | f32 | `0.70` | Minimum absolute score for a category to win. |
| `jevMargin` | f32 | `0.15` | Minimum margin the winner must hold over the runner-up. |

**Why the model default is pinned.** The `0.70` threshold and the `0.15` margin were **measured on `jev-1.13.0`**. That is why the default is a pinned version rather than a floating tag: changing the model leaves those two numbers unmeasured, and AC cannot tell you what they should be instead.

#### The room file: `<room-root>/.co-managed/config.json`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | The room's Co-managed flag. Written by the **Co-managed** toggle on the orchestrator row. |
| `catalogPath` | string or null | `null` | Path to the category catalog. Relative paths resolve against the room root. No UI field yet; edit the file. |

**Both keys absent means off.** A malformed file is **repaired to defaults on load**, never fatally rejected, and unknown keys survive a rewrite so a newer build's key is not lost by an older one. The directory also holds `state.json`, the `queue/` directory and one advisory `lock` file governing config and state. `room-*/` is gitignored, so none of it enters your repository.

#### The category catalog file

`catalogPath` points at a JSON document of the shape:

```json
{
  "categories": {
    "<your-category-id>": {
      "question": "A yes/no question that decides this category.",
      "destination": "user | orchestrator | root | default_reply",
      "peer": "project:room-N-team/agent",
      "reply": "A fixed sentence, for default_reply only."
    }
  }
}
```

Three things you need to know and cannot guess:

1. **Each category maps to exactly one of the four destinations**: `user`, `orchestrator`, `root`, `default_reply`. Anything else is an abstention with a visible reason naming the category. Validation is per category, so one broken entry does not disable its valid siblings.
2. **A fixed reply that expresses approval is rejected when the catalog loads**, with the offending category named. Denied phrases are matched case-insensitively: `approved`, `go ahead`, `the user agrees`, `authorised`, `authorized`, `lgtm`, `ship it`.
3. **The number of categories affects the classification.** The classifier's measured behaviour is order- and composition-sensitive: a faithful repeat of one measured run flipped 3.5% of bands, and changing only the question order collapsed the ranking (Spearman 0.349538 against 0.967303). AC therefore emits the questions in a fixed byte-sorted order by category id and requires both an absolute score and a margin over the runner-up, abstaining otherwise. **Adding or removing a category changes the call, so it can change outcomes** for categories you did not edit.

See [Co-managed rooms](../features/co-managed-rooms.md).

### Web server (opt-in)

| Field | Type | Default | Description |
|---|---|---|---|
| `webServerEnabled` | bool | `false` | Enable the embedded HTTP / WebSocket server. |
| `webServerPort` | u16 | platform-default per binary suffix | Listening port. |
| `webServerBind` | string | `"127.0.0.1"` | Bind address. Use `"0.0.0.0"` only if you understand the implications. |

See [Remote web UI](../features/remote-web-ui.md).

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
   active selected `settings.json`. Do not edit a guessed adjacent, global, or
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

See [Control-plane API](../features/control-plane-api.md).

See [`api-client`](cli.md#api-client) for minting and revoking control-plane client tokens.

### Brief auto-title

| Field | Type | Default | Description |
|---|---|---|---|
| `autoGenerateTaskTitle` | bool | `true` | When an orchestrator session spawns and the brief has no `title:`, AC injects a prompt asking the agent to add one. |

### Templates

| Field | Type | Default | Description |
|---|---|---|---|
| `agentTemplatesPath` | string \| null | `null` | Local agent-templates root for the role-template picker. Empty/missing → default `<config-dir>/agent-templates/`. Relative → resolved against `<config-dir>/`. This does not control the Agency cache at `<config-dir>/agency-agents_templates`. |

### Self-handoff

| Field | Type | Default | Description |
|---|---|---|---|
| `autoSelfClearEnabled` | bool | `true` | Global master for auto self-handoff-and-clear. `false` turns it off for every agent. When `true`, the class-aware default applies (ON for orchestrator/Root, OFF for specialists), subject to the per-agent override below. |
| `autoSelfClearByAgent` | `{ <agent-name>: bool }` | `{}` | Per-agent override of the class default, keyed by agent name (same key as `defaultProfileByAgent`). Applies only while the global master is on; absent = use the class default. |

### Watchers

Root-level context-scrape watcher patterns, keyed by watcher id. A pattern can apply to every agent, which the per-agent `contextRegex` shape cannot express. A malformed entry is skipped (one log line) instead of invalidating the whole settings file.

| Field | Type | Default | Description |
|---|---|---|---|
| `watchers` | `{ <id>: WatcherEntry }` | `{}` | Watcher patterns, resolved in key order against an 8-watcher budget. |
| `watchersGeometry` | object \| null | `null` | Geometry of the watcher activity window. |

See [Watchers](../features/watchers.md).

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

### Menu guard

Proactive detection of terminal blocking menus, such as a folder-trust prompt an agent will not move past. One root switch, plus three blocking-menus files next to `settings.json`. The startup download has a Settings checkbox, **Download blocking-menu pattern updates from GitHub**; there is no CLI verb, and hand-editing `settings-blocking-menus.local.json` is still the only way to add your own patterns. That file is read at the next start.

| Field | Type | Default | Description |
|---|---|---|---|
| `menuGuardEnabled` | bool | `true` | Root switch for the whole feature. With `false`, each 250 ms tick clears any session the guard was holding and evaluates nothing. |
| `remoteBlockingMenusEnabled` | bool | `true` | Download the published blocking-menu patterns at startup, at most once per 24 h. They apply at the next start. `false` stops the download only; a file already downloaded keeps applying. |

| File | Who writes it | When |
|---|---|---|
| `settings-blocking-menus.json` | AC | Rewritten at start whenever its content differs from the running version's embedded content; edits there are lost. |
| `settings-blocking-menus.local.json` | You | Read at start. AC writes it only when the #1905 migration succeeds, and that write inserts `byAgent` rows without overwriting an existing one. |
| `settings-blocking-menus.remote.json` | AC | Written only by the startup download, and only after the whole file passes validation. Read and validated again at every start. |

`BlockingMenusFile` (all three files share this shape; the remote file must also have an empty `byAgent`):

| Field | Type | Default | Description |
|---|---|---|---|
| `schemaVersion` | number | `1` | Must be `1`. Any other value rejects the whole file. |
| `note` | string \| absent | absent | Free text AC never parses. |
| `byCommand` | object | `{}` | Keys are the lowercase executable stem, exact match; values are entry arrays. |
| `byAgent` | object | `{}` | Keys are agent ids; values are entry arrays. The migration writes the exported legacy array here. |

Precedence, first present wins and replaces the layers below it whole: (1) an array still on the agent (a legacy `blockingMenus` array the migration could not move, or one inside an `agents` array owned by `settings.local.json`); (2) `byAgent[id]` in `.local`; (3) `byCommand[stem]` in `.local`; (4) `byCommand[stem]` in `settings-blocking-menus.remote.json`; (5) `byCommand[stem]` in the shipped file; (6) nothing. A remote array for a stem replaces the shipped array for that stem whole and can be `[]`; to override a remote entry, put that stem in `.local` `byCommand`. `byAgent` is never read from the remote file.

Use `byCommand` when the pattern should follow the command, `byAgent` when it should follow one agent id; a `byAgent` row always wins.

`BlockingMenuConfig` (one valid entry):

| Field | Type | Default | Description |
|---|---|---|---|
| `pattern` | string | — (required) | Rust `regex` expression, matched unanchored against one logical screen row at a time. Wrapped physical rows are joined first. An invalid pattern is logged once and skipped. |
| `notification` | string | — (required) | The text shown on the toast and returned as `blockedMenuMessage` by `list-peers`. |
| `enabled` | bool | `true` | Whether this entry is evaluated. `false` is the durable way to switch off a shipped default. |
| `capturedAgainst` | string \| null | `null` | Free text (e.g. "codex 0.153.2 / Windows"). Never validated, never parsed. Omitted from the file when absent. |

The first settings load after an upgrade moves every legacy `blockingMenus` array from `settings.json` into `.local` under `byAgent.<id>`, dropping only arrays equal to the shipped set (a `[]` on a stem that ships nothing counts as equal); an id already present in `.local` is kept and the legacy copy in `settings.json` is discarded, not merged, and the `.local` file is written before `settings.json` is touched. Before the compare, a non-empty codex array missing the hooks-review pattern gets it back-filled once. If the migration cannot run, the arrays stay in place and apply as before: a `.local` that cannot be read, a `.local` that does not parse or has the wrong shape, a `.local` that cannot be written, two agents sharing an id with different arrays or commands, or an `agents` array owned by `settings.local.json`. Each settings load retries and logs one line per attempt while the cause stands; for an overlay-owned `agents` array, move the entries into `.local` by hand and delete those agents' legacy arrays from the overlay.

An entry AC cannot read as a `BlockingMenuConfig` is kept verbatim, skipped at evaluation, and left in place; it never invalidates the file. A `.local` file with the wrong shape — not an object, another `schemaVersion`, or the wrong type for `note`, `byCommand` or `byAgent` — is ignored whole, with one error line in the log, and the layers below it still apply. The shipped file is never parsed at runtime: AC rewrites it from the binary's embedded copy at start and evaluates that embedded copy.

The remote file is validated whole at every start and ignored whole, with one warning line in the log, when any check fails; the shipped patterns then apply.

See [Menu guard](../features/menu-guard.md).

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

See [Activity log](../features/activity-log.md).

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
- [Terminal snapshots](../features/terminal-snapshots.md) - the default-on screen-content read capability
- [Menu guard](../features/menu-guard.md) - `menuGuardEnabled` and the three `settings-blocking-menus` files in use
- [`PRIVACY.md`](../../PRIVACY.md) — what credentials live here and how they are transmitted
