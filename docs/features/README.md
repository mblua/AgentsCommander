# Features

For developers looking for the page that covers one AgentsCommander feature. Every feature page lives in this directory; this index is the map.

## Agents and sessions

| Page | What it covers |
|---|---|
| [Coding Agent Profiles](coding-agent-profiles.md) | Define several launch variants of one coding agent and switch a single session between them. |
| [Container coding agents](container-coding-agents.md) | Run a coding agent under AC's Container runtime, what it does with your host credentials, and what does not work yet. |
| [Session auto-close](session-auto-close.md) | Close idle teams on a timeout, read the idle badge, and change or turn the timeout off. |
| [Coding agent auto-update](agent-auto-update.md) | Answer the startup update prompt once per coding-agent command and have AC remember it. |
| [Voice-to-text](voice-to-text.md) | Dictate prompts to a coding agent instead of typing them. |

## Automation

| Page | What it covers |
|---|---|
| [Non-stop mode](non-stop-mode.md) | Watch a group of workgroups and get a Telegram message or a sound when one stops working. |
| [Project Loops](project-loops.md) | Send a scheduled prompt to a workgroup coordinator on a cron expression, waking or respawning the session. |
| [Spec Board](spec-board.md) | Edit a Mermaid file in its own window with a live preview, snapshots, and an agent handoff. |
| [Watchers](watchers.md) | Match a pattern against every agent terminal at once and read the hits in the activity window. |

## Monitoring

| Page | What it covers |
|---|---|
| [Resource monitor](resource-monitor.md) | Watch what each agent group is using and set the thresholds at which AC warns you or kills it. |
| [Context tracking](context-tracking.md) | Read how much of an agent context window is used and alert a coordinator when a member crosses a threshold. |
| [Terminal snapshots](terminal-snapshots.md) | Read one live backend terminal viewport as versioned JSON or a PNG without changing the session. |
| [Window capture](window-capture.md) | Capture one live native window as a PNG from the CLI or the control-plane API. Windows only. |
| [Screenshot capture](screenshot-capture.md) | Press a global hotkey, drag a rectangle, and save a PNG inside the replica that owns your session. Windows only. |

## Remote access

| Page | What it covers |
|---|---|
| [Telegram bridge](telegram-bridge.md) | Attach a Telegram bot to one session so PTY output reaches your phone and your replies reach the agent. |

## Configuration and packaging

| Page | What it covers |
|---|---|
| [Config seed](config-seed.md) | Copy a template config folder into every replica at spawn, with AC path tokens already substituted. |
| [Seed manifest](seed-manifest.md) | Read the Git-diffable inventory of the project-scoped files AC published into your project's `.ac` folder. |
| [Portable instances](portable-instances.md) | Run isolated AgentsCommander instances side by side from separate copies of the binary. |
