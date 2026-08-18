# Project Loops

For developers who want a coordinator prompted on a schedule instead of remembering to do it. After this page you can create a Loop that wakes a workgroup's coordinator on a cron expression, choose what happens when that coordinator is busy, and read the toast that tells you what the Loop did.

A Project Loop is a scheduled prompt. You give it a cron expression, a target workgroup, and the text to send; AC delivers that text to the workgroup's coordinator when the schedule comes due, waking or respawning the session if it is not running.

## What a Loop is

A Loop belongs to one registered project and targets one workgroup inside it. It carries four things: an id, a cron expression, the target workgroup whose **coordinator** receives the prompt, and the prompt text.

Two more properties control its behavior: whether it is enabled, and its busy policy, which decides what happens when the coordinator is mid-task at delivery time.

A Loop is not a background agent and does not run anything itself. All it does is put your prompt into a coordinator's terminal at the right moment. What happens next is whatever that coordinator does with it.

## Creating a Loop

**From the sidebar.** The project's Loops section carries the actions: create a Loop, run one now, edit it, enable or disable it, and delete it. Deleting asks for confirmation first.

The `New Loop` modal has four fields and two checkboxes:

| Control | What it takes |
|---|---|
| `Name` | A human-readable name, for example `Weekday standup`. |
| `Cron` | A five-field cron expression, for example `0 9 * * 1-5`. |
| `Workgroup Coordinator` | The target workgroup, chosen from a list. Starts on `Select a coordinator...`. |
| `Prompt` | The text to inject into the coordinator. |
| `Enabled` | On by default. Off creates the Loop without scheduling it. |
| `Force inject even if coordinator is busy` | Off by default. See [Busy sessions and respawn](#busy-sessions-and-respawn). |

The modal checks the cron expression while you type. It shows `Checking schedule...` while it asks the backend, then `Next run: <time>` when the expression parses. A wrong field count is rejected up front with `Cron expression must have exactly five fields`, and an expression the backend rejects shows `Invalid cron expression`. **`Create` stays disabled until the preview is ready**, so a Loop with an unparseable schedule cannot be created.

If the project has no workgroup with a verified coordinator, the picker is empty and the modal says `A workgroup with a verified coordinator is required.` Create the coordinator first.

`Ctrl+Enter` creates the Loop, `Escape` closes the modal.

**From the CLI.** Every operation is also a `loop` subcommand, which is what you want for scripting or for a machine with no GUI:

```bash
agentscommander loop create \
  --project MyProject \
  --name "Daily sync" \
  --cron "0 9 * * 1-5" \
  --workgroup wg-1-dev-team \
  --prompt "Check status and ask for blockers."
```

`list`, `update`, `enable`, `disable` and `remove` complete the set, and `list` also prints the scheduler state. See [`loop`](../reference/cli.md#loop) for every flag and its exact meaning; this page does not repeat them.

The CLI accepts one busy policy the modal does not expose: `--busy-coordinator skip`. The modal's checkbox chooses between waiting and force-injecting only.

## Scheduling

The cron expression has five fields: minute, hour, day-of-month, month, day-of-week.

AC scans each Loop on a schedule and compares the expression against the window since it last checked:

- A **disabled** Loop is skipped entirely.
- The **first** scan of a new Loop only writes a baseline. A Loop you create today does not fire for occurrences that fell before you created it.
- When several occurrences fall inside one window, AC takes **the latest one** and delivers once. A machine that was asleep through five daily runs gets one prompt, not five.
- A Loop that came due while **AgentsCommander was closed** is recorded as missed, not delivered late. You get the missed toast at startup and no surprise prompt.
- A delivery already **pending** on a busy coordinator is retried before any new occurrence is considered, and a second occurrence arriving while it is still pending is coalesced into it rather than queued.

## Delivery: what happens when a Loop fires

Delivery targets the coordinator of the Loop's workgroup, in this order:

1. AC resolves the target workgroup and its coordinator replica.
2. If a workgroup purge is destroying that agent right now, delivery is skipped with `purge-wg in progress for '<agent>'; loop delivery skipped`. A Loop tick never resurrects an agent a purge is removing.
3. If a live coordinator session exists, it is used. If not, AC clears any stale session records and **spawns the coordinator session**.
4. AC checks whether the coordinator is busy and applies the Loop's busy policy.
5. The prompt is injected into the session, exactly as if you had typed it, and becomes that session's last prompt.

Each outcome produces a toast in the sidebar:

| Outcome | Toast |
|---|---|
| Delivered | `Loop "<name>" delivered` |
| Coordinator busy, policy waits | `Loop "<name>" is pending until the coordinator is idle` |
| Coordinator busy, policy skips | `Loop "<name>" skipped because the coordinator is busy` |
| A second occurrence while one is pending | `Loop "<name>" coalesced into the pending delivery` |
| Came due while AC was closed | `Loop "<name>" was missed while AgentsCommander was closed` |
| Delivery failed | `Loop "<name>" failed` |

The first four and the last carry the backend's own message when it has one, so the text you see can be more specific than the table above.

## Busy sessions and respawn

**Busy.** AC checks whether the target coordinator is busy at the moment the Loop comes due, and the busy policy decides what to do about it:

- **Wait until idle** holds the delivery and marks the Loop pending. AC retries it on later scans and delivers when the coordinator goes idle. This is the default.
- **Force inject** delivers anyway, interrupting whatever the coordinator is doing. This is the `Force inject even if coordinator is busy` checkbox.
- **Skip** drops this occurrence and waits for the next scheduled one. CLI only.

**Respawn.** A Loop does not need its coordinator to be running. AC treats a session as live only when its status is active, running or idle **and** it still has a terminal attached. Anything else is respawned before delivery:

- A session that has **exited** is respawned.
- A session that is listed but has **no terminal** (dormant, never mounted, or left over from a previous run) is also respawned, and its stale record is cleared first.

The practical consequence: a Loop wakes a workgroup you closed yesterday. If you do not want that, disable the Loop rather than closing the session.

## Where the configuration lives

**There is no Loop key in `settings.json`.** Loops are per project, not a global preference.

Each Loop is a directory inside the project's AC root: `_loop_<id>/config.toml` holds the definition, and its scheduler state (last check, pending delivery) sits beside it. AC writes both, through the sidebar modals or the CLI; you do not need to edit them by hand.

Because the storage is on-disk project files, `loop` CLI commands need **no token**: any process that can already write to the project can write them.

See [`loop`](../reference/cli.md#loop) for the command surface.

## Troubleshooting

**"The Loop never fired."** Check three things in order: it is enabled; its cron expression is the five-field form you meant (`loop list` prints the scheduler state, including the next due time); and the target coordinator exists. A Loop created after today's occurrence does not fire retroactively, by design.

**"I got `Loop "<name>" was missed while AgentsCommander was closed`."** The occurrence fell while AC was not running. AC records it and does **not** deliver it late. Use `run now` from the Loops section if you want it delivered anyway.

**"I got `Loop "<name>" is pending until the coordinator is idle` and nothing since."** The coordinator has been busy ever since. The delivery is still queued and goes out when it goes idle. If you want the prompt to interrupt instead, switch the Loop to force inject.

**"A Loop opened a session I had closed."** Expected. An exited or terminal-less coordinator is respawned before delivery. Disable the Loop to stop that.

**"`Create` stays greyed out."** One of the required fields is empty, or the cron preview is not in its ready state. The preview must show `Next run: <time>` before `Create` enables.

**"The workgroup picker is empty."** The project has no workgroup with a verified coordinator, and the modal says so. Create the coordinator, then reopen the modal.

## See also

- [`loop` CLI reference](../reference/cli.md#loop) - every flag, and the JSON each subcommand prints
- [Non-stop mode](non-stop-mode.md) - the other way AC acts on a workgroup without you
- [Concepts](../concepts.md) - coordinator, workgroup, and session
