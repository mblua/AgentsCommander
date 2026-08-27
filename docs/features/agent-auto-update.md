# Coding agent auto-update

For developers who want their coding agents updated at startup without being asked every time. After this page you know what the startup prompt asks, what each answer commits you to, and how to change an answer you already gave.

When AC starts, it can update the coding agents you use before you launch a session with them. The first time it meets a coding agent it has not asked about, it shows one prompt per agent. Your answer is remembered per coding-agent command, and AC never asks about that command again.

## What it does

At startup AC runs an update pass over your coding agents. While that pass is running, an overlay covers the sidebar and reads `Actualizando coding agents...`.

While the pass runs, the overlay shows every coding agent of the pass as a step of a timeline: `Pendiente` until its update starts, `Actualizando...` with the exact update command while it runs, then `Listo` or `Falló` with the reason; a counter (`2 de 3 completados · 1 falló`) and a progress bar sit above the list. When AC could read the agent's version before and after the update, the finished step shows the transition (`2.1.34 → 2.1.35`; `2.1.34 → no instalada` when the update left the CLI broken). When everything finished the card stays on screen as a summary until you press `Cerrar` (or Enter/Escape); failures are also shown as notifications after you close it. The startup question is one question across every window: answering it on the desktop or in a browser remote tab closes it everywhere, the first answer wins, and a later answer from another window changes nothing.

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

## The Auto-update list in Settings

Settings > Coding Agents shows a read-only **Auto-update** table with one row per catalog entry that ships `updateCommands` (Cursor ships none, so it is not listed). Per row: **Auto-update** shows your remembered answer (`Yes`, `No`, or `Will ask at startup` when AC has never asked), **Installed** shows the detected version, `Installed` when the command resolves but AC does not run a version probe for it, `Not installed` when the command is not found or its version check fails (hover for the reason and the resolved path), and `Checking...` until the first check completes, and **Status** shows `Updating...` while the startup pass is updating that command and `Updated` or `Update failed` for the outcome of this AC start. Rows marked `(not registered)` are supported but not registered in `agents[]`, so they are never updated at startup. The table never changes settings: use the Auto-update dropdown of the corresponding agent to change an answer. Version checks run `<command> --version` for the built-in coding agents only, in the background, with a 15 second bound, and read the version from the first non-empty line of the output. AC asks again each time you open the Coding Agents screen: within 10 minutes of the last check it answers from its cache without running anything, after that it checks again. A version check never overlaps an update of the same agent: before each startup update AC reads that agent's current version (built-in agents only, 15 second bound), the update starts only after that reading, the Settings checks wait until the pass is over, and right after the pass AC re-checks the agents it updated; the reading before and the one after are the transition the overlay shows.

## Troubleshooting

**"The list says `Not installed` but I can run the agent from my shell."** AC resolves the command with the PATH of the AC process (started from Explorer or the Start menu), which can differ from your shell's PATH; a CLI installed after AC started, or into a user-only directory, reads as not installed until AC restarts. Hover the cell: the reason says whether the command was not found or its `--version` failed.

**"`Not installed` right after an update."** An interrupted update can leave a broken install (for example an npm shim whose package is gone); the version check then fails and the row reports `Not installed` with the exit code. Re-run the vendor's install command.

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
