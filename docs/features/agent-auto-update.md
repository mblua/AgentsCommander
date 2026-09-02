# Coding agent auto-update

For developers who want their coding agents updated at startup without being asked every time. After this page you know what the startup prompt asks, what each answer commits you to, how to stop an update pass that is already running, and how to change an answer you already gave.

When AC starts, it can update the coding agents you use before you launch a session with them. The first time it meets a coding agent it has not asked about, it shows one prompt per agent. Your answer is remembered per coding-agent command, and AC never asks about that command again.

## What it does

At startup AC runs an update pass over your coding agents. While that pass is running, an overlay covers the sidebar and reads `Updating coding agents...`.

The overlay shows every coding agent of the pass as a row of a timeline, in pass order. A row shows exactly one line of text: while it is unfinished, one state word; once it finishes, its whole outcome as a single string. A counter (`2 of 3 completed`, plus `, 1 failed` only when a row actually failed) and a progress bar sit above the list.

The four unfinished states are:

| Row reads | Meaning |
|---|---|
| `Pending` | the row is in the pass and its update has not started |
| `Updating...` | an updater command is running; the row also lists the exact commands and shows an activity bar |
| `Verifying...` | the updater commands finished and AC is reading the agent's version again |
| `Cancelling...` | AC accepted a cancellation for this row and is stopping the work |

A `Verifying...` row is **not** finished. It does not count towards `completed`, it is not a failure, and it can still be cancelled.

When the pass ends and this window saw at least one result, the card becomes the final summary: a green check, the title `Coding agent updates complete`, and the timeline as it ended. It stays on screen until you press `Close` (Enter and Escape also close it on that surface). Failed rows are shown as notifications reading `Auto-update failed for <label> (<command>): <reason>` after you close the summary.

The pass is keyed by **command**, not by agent id. Several coding agent profiles can share one binary (a "max effort" `claude` and a "cheap" `claude`, for example), and the binary is what gets updated, so AC asks once for `claude` rather than once per profile.

An agent AC has never asked about is not updated silently. It gets the prompt below, and the default answer is No.

## What each finished row means

A finished row is terminal. Its text is built from that row's own result and never changes afterwards.

| Row reads | What happened |
|---|---|
| `Ready - <old> -> <new>` | the update ran and AC read a different version before and after |
| `<version> (Nothing to update)` | the update ran and the version did not change; there was nothing to install |
| `Update completed - Version could not be verified` | the update ran without error, but AC could not compare a version before and after. It is a success and is **not** counted as failed |
| `Failed - <reason>` | the update failed. `<reason>` is the failure AC recorded |
| `Cancelled` | you cancelled this row. It counts as completed, it is **not** a failure, and it raises no failure notification |

Two failures are worth naming because they look like cancellations and are not:

- `Failed - Cancellation cleanup did not complete.` means AC stopped the update and confirmed the step's process tree was gone, but the cleanup itself was defective: a kill or a bookkeeping call returned an error, a settlement deadline expired and forced AC to kill again, or an output reader did not settle cleanly. AC reports that truthfully as a failed row rather than claiming a clean cancellation.
- `Failed - Updater process-tree containment unavailable; update stopped.` (Windows only) means AC could not put the updater into the job object it uses to contain and stop it, so it did not run the updater at all.

## Cancelling an update pass

Every unfinished row carries its own `Cancel` button, whose accessible name is `Cancel <label> update`. `Verifying...` rows keep it. A row loses the button only when it reaches a finished state.

Below the timeline, `Cancel all` (accessible name `Cancel all coding agent updates`) cancels the whole pass. It is shown while any row is unfinished, so a pass whose only remaining row is `Verifying...` still offers it.

What cancelling does:

- it stops the step that is running now and terminates that step's whole process tree. The control reports back as soon as the backend accepts the cancellation; the row itself stays unfinished until AC has proven those processes gone;
- it prevents the later updater steps of that row, and any row that had not started yet;
- for a row cancelled during `Verifying...`, it stops and settles the post-install version probe and the row ends as `Cancelled`;
- `Cancel all` leaves rows that already finished exactly as they are. It fabricates no result for them and does not turn a finished row into a cancelled one.

What cancelling does **not** do: it does not undo an updater command that already ran to completion. If the vendor's installer finished before you cancelled, that install stands. AC reports `Cancelled` for the row because you stopped the pass, not because it rolled anything back.

Once you cancel a row, its button stays visible and disabled until the row finishes, so a lost event or a reopened window cannot re-enable it. `Cancel all` disables both controls for the rest of the pass.

Keyboard and pointer:

- clicking either control cancels;
- **Space** on a focused cancel control activates it natively;
- **Enter** on a focused cancel control cancels and does **not** answer a prompt that happens to be open;
- every other **Enter**, and every **Escape**, answers an open prompt with No (see below).

If the cancellation request fails, AC shows a red notification that stays until you dismiss it: `Could not cancel the coding agent update.` for a row, `Could not cancel coding agent updates.` for `Cancel all`. Those two sentences are the whole message. AC never appends a backend diagnostic to them or to the overlay; diagnostics go to the browser console only.

## The startup prompt

The prompt appears inside the same overlay, one at a time, and names the coding agent by its label:

> Automatically update the `<label>` coding agent at startup?

Two buttons answer it: `Yes` and `No`. `No` holds the focus when the prompt opens.

Pressing **Enter or Escape answers No**. Both keys take the safe answer rather than the focused-button answer, so dismissing the prompt out of reflex never signs you up for automatic updates. The one exception is Enter on a focused cancel control, which cancels that update and leaves the question unanswered. While an answer is in flight the buttons are disabled and the keys are ignored, so you cannot answer twice.

The prompt does not replace the timeline. The question sits between the progress bar and the row list, so the row buttons and `Cancel all` stay reachable while it is open. Cancelling the row that is being asked about closes its question without recording any answer: nothing is written to `settings.json`, and AC asks about that command again at the next startup.

Answering **No** removes that agent's row from the timeline. The counter's total drops with it, so a pass of three agents where you decline one continues as `<n> of 2 completed`.

AC waits 60 seconds for an answer. After that the prompt expires, and an answer that arrives late is stored for future startups without acting on this one:

- if you answered Yes, AC shows `This coding agent will be updated at the next startup.`
- if you answered No, AC shows `You will not be asked again.`

The startup question is one question across every window: answering it on the desktop or in a browser remote tab closes it everywhere, the first answer wins, and a later answer from another window changes nothing.

## Every window shows the same pass

Desktop windows and browser remote tabs converge on the same state. AC broadcasts each pass event to every surface, and every surface also reads an authoritative status snapshot that carries the running rows, the verifying rows, the results, and which cancellations have been accepted. A window that opens mid-pass therefore shows the pass already in progress, with the cancelled rows already disabled.

Closing a window, or navigating away from it, does not cancel anything. The pass runs in the AC backend; only the cancel controls stop it.

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

Settings > Coding Agents shows a read-only **Auto-update** table with one row per catalog entry that ships `updateCommands` (Cursor ships none, so it is not listed). Per row: **Auto-update** shows your remembered answer (`Yes`, `No`, or `Will ask at startup` when AC has never asked), **Installed** shows the detected version, `Installed` when the command resolves but AC does not run a version probe for it, `Not installed` when the command is not found or its version check fails (hover for the reason and the resolved path), and `Checking...` until the first check completes, and **Status** shows `Updating...` while the startup pass is updating that command, `Updated` or `Update failed` for the outcome of this AC start, and `-` when this AC start recorded no result for that command.

This **Status** column has only those four values, so it is coarser than the overlay's timeline. A row you cancelled reads `Update failed` here, and hovering it reads `unknown error`, because the column classifies on success alone and a cancelled update is not a success. The overlay's timeline is the surface that distinguishes cancelled, unchanged and unverified outcomes; read it, not this column, for what actually happened.

Rows marked `(not registered)` are supported but not registered in `agents[]`, so they are never updated at startup. The table never changes settings: use the Auto-update dropdown of the corresponding agent to change an answer. Version checks run `<command> --version` for the built-in coding agents only, in the background, with a 15 second bound, and read the version from the first non-empty line of the output. AC asks again each time you open the Coding Agents screen: within 10 minutes of the last check it answers from its cache without running anything, after that it checks again. A version check never overlaps an update of the same agent: before each startup update AC reads that agent's current version (built-in agents only, 15 second bound), the update starts only after that reading, the Settings checks wait until the pass is over, and right after the pass AC re-checks the agents it updated; the reading before and the one after are the transition the overlay shows.

## Troubleshooting

**"The list says `Not installed` but I can run the agent from my shell."** AC resolves the command with the PATH of the AC process (started from Explorer or the Start menu), which can differ from your shell's PATH; a CLI installed after AC started, or into a user-only directory, reads as not installed until AC restarts. Hover the cell: the reason says whether the command was not found or its `--version` failed.

**"`Not installed` right after an update."** An interrupted update can leave a broken install (for example an npm shim whose package is gone); the version check then fails and the row reports `Not installed` with the exit code. Re-run the vendor's install command.

**"A row says `Update completed - Version could not be verified`."** The updater exited without error but AC could not read a comparable version on both sides of it, so it refuses to invent one. Check the agent's version yourself with `<command> --version`. This is not a failed update and does not raise a failure notification.

**"I cancelled and the agent still got updated."** Cancellation stops the step that is running and everything after it; it does not reverse an updater command that had already finished. The row reads `Cancelled` because you stopped the pass.

**"A row I cancelled reads `Failed - Cancellation cleanup did not complete.`"** The update stopped and its process tree is gone: AC proves that before it reports this row at all. What failed is the cleanup around it, so AC will not claim a clean cancellation. Nothing is left for you to hunt down; to see which part failed, search `app.log` for `defective` ([Log filtering](../reference/log-filtering.md#where-logs-go) says where that file lives).

**"I answered and got `This coding agent will be updated at the next startup.`"** Your answer arrived after the prompt had already closed on the backend, so nothing was updated this time. The answer is saved: the update runs at the next startup, and you are not asked again.

**"I answered No and got `You will not be asked again.`"** Same race, harmless outcome. The prompt had already closed, and since a No means "never update", nothing was pending anyway. The answer is stored.

**"The prompt shows an error notification and stays open."** The answer request failed, so AC cannot confirm your answer was recorded. The overlay keeps the prompt open on purpose so you can press the button again; the notification stays until you dismiss it because a silent failure here would leave you thinking you had answered.

**"AC updated an agent I never approved."** Check `agentAutoUpdateByCommand` for the agent's **command**, not its label or id. Two profiles sharing one command share one answer, so approving the update for one profile approves it for the binary they both use.

**"My agents never update although I answered Yes."** The catalog entry has no `updateCommands`: the catalog was seeded before update commands shipped (or the agent is `cursor`, which ships none by design). AC now backfills the built-in defaults at read time, so this is usually already resolved; to force a command, edit `agents.json` directly (the CLI only exposes the catalog read-only). The preference control remains `agentAutoUpdateByCommand`. With the new shipping commands, users who registered `hermes`, `opencode`, or `agy` and were never asked may see ONE first prompt at the next startup (default No) - answering it is how the per-command preference is set.

**"I want to be asked again."** Remove that command's key from `agentAutoUpdateByCommand`. An absent key is what makes AC ask.

## Where the strings on this page come from

For contributors keeping this page in sync. Every UI string quoted above - the overlay, the prompt, the toasts and notifications, and the Settings **Auto-update** table - is defined at one of these locations. Coding-agent command names, settings keys and values, file names, log messages and commands you run yourself are not listed.

| Strings | Source |
|---|---|
| `Updating coding agents...`, `Coding agent updates complete` | `src/sidebar/components/AgentUpdateOverlay.tsx:113` |
| `Automatically update the <label> coding agent at startup?` | `src/sidebar/components/AgentUpdateOverlay.tsx:276` |
| `Yes`, `No` (and the autofocus on `No`) | `src/sidebar/components/AgentUpdateOverlay.tsx:286`, `:291`, `:296` |
| Enter/Escape answer No; Enter on a cancel control does not | `src/sidebar/components/AgentUpdateOverlay.tsx:183`, `:190` |
| `Cancel` row button, accessible name `Cancel <label> update` | `src/sidebar/components/AgentUpdateOverlay.tsx:374`, `:378` |
| `Cancel all`, accessible name `Cancel all coding agent updates`, shown while any row is unfinished | `src/sidebar/components/AgentUpdateOverlay.tsx:399`, `:406`, `:410` |
| `Close` | `src/sidebar/components/AgentUpdateOverlay.tsx:395` |
| `This coding agent will be updated at the next startup.`, `You will not be asked again.` | `src/sidebar/components/AgentUpdateOverlay.tsx:139`, `:143` |
| `Pending`, `Updating...`, `Verifying...`, `Cancelling...` | `src/sidebar/agent-update-status.ts:73-77` |
| `(Nothing to update)`, `Update completed - Version could not be verified`, `Cancelled`, `Failed` | `src/sidebar/agent-update-status.ts:82-85` |
| `Ready - <old> -> <new>` and the rest of the outcome text | `src/sidebar/agent-update-status.ts:175`, `:188` |
| `<n> of <N> completed`, `, <n> failed` | `src/sidebar/agent-update-status.ts:287` |
| **Auto-update** column `Yes`, `No`, `Will ask at startup` | `src/sidebar/agent-update-status.ts:70` |
| **Installed** column `Checking...`, `Installed`, `Not installed` | `src/sidebar/agent-update-status.ts:80`, `:98`, `:102`, `:112` |
| Settings **Status** column values `Updating...`, `Updated`, `Update failed`, and `-` for a command with no result (a cancelled row reads `Update failed` here) | `src/sidebar/agent-update-status.ts:71`, `:125-127` |
| `unknown error`, the hover text of an `Update failed` cell whose result carries no reason | `src/sidebar/agent-update-status.ts:79`, used at `:127` |
| `(not registered)` | `src/sidebar/components/AgentAutoUpdateStatusList.tsx:128` |
| `Could not cancel the coding agent update.`, `Could not cancel coding agent updates.` | `src/sidebar/agent-update.ts:84-85` |
| `Auto-update failed for <label> (<command>): <reason>` | `src/sidebar/agent-update.ts:139` |
| Answering No removes the row from the pass | `src-tauri/src/agent_update.rs:582`, `src/sidebar/agent-update.ts:523` |
| 60 second prompt wait | `src-tauri/src/agent_update.rs:56` |
| `Cancellation cleanup did not complete.` | `src-tauri/src/agent_update.rs:64` |
| `Updater process-tree containment unavailable; update stopped.` | `src-tauri/src/agent_update.rs:3017` |
| Cancellation terminates and reaps the step's process tree | `src-tauri/src/agent_update.rs:1987` |
| Cancelling during verification yields `Cancelled` | `src-tauri/src/agent_update.rs:3028` |

## See also

- [Settings reference](../reference/settings.md#coding-agents) - `agentAutoUpdateByCommand` and the rest of the coding agent configuration
- [Coding agents](../integrations/coding-agents.md) - configuring the agents this prompt asks about
- [Coding Agent Profiles](coding-agent-profiles.md) - why several profiles can share one command
