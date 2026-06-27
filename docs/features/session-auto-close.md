# Session auto-close

For developers running long-lived teams who want idle coordinators and their agents to shut down on their own instead of lingering. After this page you know what AC auto-closes, when, how to read the idle badge, and how to change or turn off the timeout.

AgentsCommander watches every team for inactivity. When a team sits idle past a timeout (60 minutes by default), AC closes its sessions for you, so abandoned teams stop holding PTYs, processes, and memory. A per-coordinator badge shows how long each team has been idle, so you can see a close coming.

## What gets auto-closed

Auto-close applies to **teams only**: coordinator sessions and agent-owned member sessions. It never touches an ad-hoc shell you opened yourself.

| Session kind | Auto-closed? |
|---|---|
| Coordinator session | Yes |
| Agent-owned member session (a replica running a coding agent) | Yes |
| Ad-hoc user shell (no coding agent, not a coordinator) | Never |

A session counts as a team member only if it is a coordinator or has a coding agent attached, and its working directory resolves to a team key `<project>:<wg>`. Everything else is left alone.

Two more conditions gate an actual close:

- The session must have a **live PTY**. A deferred or not-yet-spawned session has nothing to terminate and is skipped.
- The team must be **established**: at least one member alive for more than 30 seconds. A team you just opened, or one that just woke on restore, is visible for at least that long before anything can close it.

## The idle badge

Every coordinator row in the sidebar shows an idle badge: the whole number of minutes since the team was last active, formatted `Nm`.

- `0m` right after activity.
- `45m` after 45 minutes of silence.
- It counts minutes only, with no hour or day rollover, so a team idle for three days reads `4320m`.

The badge changes color as idle time grows:

| Idle time | Color |
|---|---|
| Below the yellow threshold (default 30m) | Default, no warning color |
| At or above the yellow threshold (default 30m) | Yellow |
| At or above the red threshold (default 60m) | Red |

The badge is **informational and always on**. It shows even when auto-close is turned off, so you can monitor idle teams without letting AC close them.

## The AUTO-CLOSED badge

When auto-close fires and the session it destroys is the team's **coordinator**, AC stamps that coordinator row with an **AUTO-CLOSED** badge. This is your record that the team was closed by the timeout, not by you.

If only a non-coordinator member is reaped while the coordinator survives (the coordinator was spared by a grace window or a late user message), the surviving coordinator keeps its idle counter and is **not** stamped. The AUTO-CLOSED badge means specifically "this coordinator's own session was auto-closed."

## Settings

All four keys live in `settings.json` (see the [settings reference](../reference/settings.md#session-auto-close)). Defaults shown.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `coordinatorAutoCloseEnabled` | bool | `true` | Master switch. When false, AC never auto-closes a team (the badge still shows). |
| `coordinatorAutoCloseMinutes` | number | `60` | Idle minutes before a team is closed. `0` also disables auto-close. |
| `coordinatorIdleBadgeYellowMinutes` | number | `30` | Idle minutes at which the badge turns yellow. |
| `coordinatorIdleBadgeRedMinutes` | number | `60` | Idle minutes at which the badge turns red. |

Out of the box (`coordinatorAutoCloseEnabled` true, `coordinatorAutoCloseMinutes` 60), a team that sits idle for more than an hour is closed automatically.

### Turning auto-close off

Set either field:

```json
{
  "coordinatorAutoCloseEnabled": false
}
```

or:

```json
{
  "coordinatorAutoCloseMinutes": 0
}
```

Either one stops every close. The idle badge keeps counting, so you still see idle time at a glance.

## How idle is measured

Idle time is measured from a single anchor per team: **the more recent of two clocks**.

- **Last user message.** Updated when you send input to any session in the team: typing in the terminal, a web keystroke, or a Telegram message.
- **Last activity.** Updated from real PTY output (the agents doing work), advanced on the watcher's tick.

The anchor is `max(last user message, last activity)`, and idle time is `now - anchor`. Either real work or your input resets the clock, so a team is only "idle" when both the human and the agents have gone quiet.

### Why a reopened team keeps its old idle time

When you reopen an abandoned team, the session replays its scrollback for a second or two. AC ignores output in the first 10 seconds of a woken session so that scrollback replay does not look like fresh activity and reset the idle clock. Genuine activity after that window advances the anchor normally.

### Why a close can lag the timeout by up to a minute

The watcher checks teams once every 60 seconds. A team that has crossed its timeout is closed on the next tick, so a close can trail the threshold by up to a minute. This is also why a team is not closed the instant you walk away.

## How the badge value reaches the UI (backend contract)

There is **no idle field on the session object**. The anchor reaches the frontend out of band, on a Tauri event:

- Event: `coordinator_clock_updated`
- Payload: `{ "replicaPath": "<coordinator cwd>", "lastUserMessageAt": "<RFC 3339 timestamp>" }`

The frontend computes `Nm` from that timestamp. The clocks themselves persist per team in `coordinator_clocks.json`.

> **Naming caveat.** The payload field is named `lastUserMessageAt`, but it carries the **unified** `max(user message, activity)` anchor, not the user-message clock alone. The name is kept for backward compatibility; read it as "the idle anchor."

## Troubleshooting

**"My ad-hoc shell got closed."** It should not have. Auto-close only targets coordinators and agent-owned sessions; a plain shell with no coding agent is never a candidate. If you saw a shell close, it was not auto-close. Check the row tooltip for the real exit reason.

**"My team did not close after the timeout."** Confirm `coordinatorAutoCloseEnabled` is true and `coordinatorAutoCloseMinutes` is non-zero. Then remember the team must be established (a member alive more than 30 seconds) and have a live PTY, and the close happens on the next 60-second tick. A team you just reopened restarts from fresh activity.

**"The idle badge never reaches red."** Something is resetting the anchor. Any PTY output (an agent still working, a watchdog, a stray log) or any input counts as activity. Watch the `Nm` value: if it keeps dropping back to a low number, the team is not actually idle.

**"A member closed but the coordinator stayed."** Expected. Only the coordinator's own auto-close stamps the AUTO-CLOSED badge; a reaped sibling leaves a surviving coordinator counting normally.

## See also

- [Settings reference](../reference/settings.md#session-auto-close) - the four auto-close keys
- [Concepts: Session](../concepts.md#session) - session status dots and lifecycle
- [Glossary](../glossary.md) - session auto-close, idle badge
