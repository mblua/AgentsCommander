# Coding agent auto-update

For developers who want their coding agents updated at startup without being asked every time. After this page you know what the startup prompt asks, what each answer commits you to, and how to change an answer you already gave.

When AC starts, it can update the coding agents you use before you launch a session with them. The first time it meets a coding agent it has not asked about, it shows one prompt per agent. Your answer is remembered per coding-agent command, and AC never asks about that command again.

## What it does

At startup AC runs an update pass over your coding agents. While that pass is running, an overlay covers the sidebar and reads `Actualizando coding agents...`.

The pass is keyed by **command**, not by agent id. Several coding agent profiles can share one binary (a "max effort" `claude` and a "cheap" `claude`, for example), and the binary is what gets updated, so AC asks once for `claude` rather than once per profile.

An agent AC has never asked about is not updated silently. It gets the prompt below, and the default answer is No.

## The startup prompt

The prompt appears inside the same overlay, one at a time, and names the coding agent by its label:

> ¿Querés que al arranque se intente actualizar automáticamente el coding agent `<label>`?

Two buttons answer it: `Sí` and `No`. `No` holds the focus when the prompt opens.

Pressing **Enter or Escape answers No**. Both keys take the safe answer rather than the focused-button answer, so dismissing the prompt out of reflex never signs you up for automatic updates. While an answer is in flight the buttons are disabled and the keys are ignored, so you cannot answer twice.

## How your answer is remembered

Your answer is written into `agentAutoUpdateByCommand` in `settings.json`, a map from coding-agent command to your answer:

- `true` means AC updates that command at every startup and does not ask again.
- `false` means AC never updates that command and does not ask again.
- **An absent key means AC has never asked**, so it asks on the next startup.

AC persists your answer **before** it tries to act on it. The point is that a failure while starting the update cannot cost you the answer: whatever happens next, that command is not asked about again.

To change your mind later, edit the entry in `settings.json`. Setting a command back to an absent key (removing it from the map) makes AC ask you again on the next startup.

## Settings

| Key | What it controls |
|---|---|
| `agentAutoUpdateByCommand` | Your remembered answer per coding-agent command. `{}` by default, which means AC has asked about nothing yet. |

See [Settings reference](../reference/settings.md#coding-agents) for the field's type and defaults.

## Troubleshooting

**"I answered and got `Se actualizará en el próximo arranque.`"** Your answer arrived after the prompt had already closed on the backend, so nothing was updated this time. The answer is saved: the update runs at the next startup, and you are not asked again.

**"I answered No and got `No se volverá a preguntar.`"** Same race, harmless outcome. The prompt had already closed, and since a No means "never update", nothing was pending anyway. The answer is stored.

**"The prompt shows an error toast and stays open."** The answer did not reach the backend at all. The overlay keeps the prompt open on purpose so you can press the button again; the toast stays until you dismiss it because a silent failure here would leave you thinking you had answered.

**"AC updated an agent I never approved."** Check `agentAutoUpdateByCommand` for the agent's **command**, not its label or id. Two profiles sharing one command share one answer, so approving the update for one profile approves it for the binary they both use.

**"My agents never update although I answered Yes."** The catalog entry has no `updateCommands`: the catalog was seeded before update commands shipped (or the agent is `cursor`, which ships none by design). AC now backfills the built-in defaults at read time, so this is usually already resolved; to force a command, edit `agents.json` directly (the CLI only exposes the catalog read-only). The preference control remains `agentAutoUpdateByCommand`. With the new shipping commands, users who registered `hermes`, `opencode`, or `agy` and were never asked may see ONE first prompt at the next startup (default No) — answering it is how the per-command preference is set.

**"I want to be asked again."** Remove that command's key from `agentAutoUpdateByCommand`. An absent key is what makes AC ask.

## See also

- [Settings reference](../reference/settings.md#coding-agents) - `agentAutoUpdateByCommand` and the rest of the coding agent configuration
- [Coding agents](../integrations/coding-agents.md) - configuring the agents this prompt asks about
- [Coding Agent Profiles](coding-agent-profiles.md) - why several profiles can share one command
