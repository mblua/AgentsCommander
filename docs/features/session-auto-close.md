# Session auto-close

For developers running long-lived teams who want idle orchestrators and their agents to shut down on their own instead of lingering. After this page you know what AC auto-closes, when, how to read the idle badge, and how to change or turn off the timeout.

AgentsCommander watches every team for inactivity. When a team sits idle past a timeout (60 minutes by default), AC closes its sessions for you, so abandoned teams stop holding PTYs, processes, and memory. A per-orchestrator badge shows how long each team has been idle, so you can see a close coming.

## What gets auto-closed

Auto-close applies to **teams only**: orchestrator sessions and agent-owned member sessions. It never touches an ad-hoc shell you opened yourself.

| Session kind | Auto-closed? |
|---|---|
| Orchestrator session | Yes |
| Agent-owned member session (a replica running a coding agent) | Yes |
| Ad-hoc user shell (no coding agent, not an orchestrator) | Never |

A session counts as a team member only if it is an orchestrator or has a coding agent attached, and its working directory resolves to a team key `<project>:<room>`. Everything else is left alone.

Two more conditions gate an actual close:

- The session must have a **live PTY**. A deferred or not-yet-spawned session has nothing to terminate and is skipped.
- The team must be **established**: at least one member alive for 30 seconds or more. A team you just opened, or one that just woke on restore, is visible for at least that long before anything can close it.

## What happens to the selected session

If auto-close destroys the selected session, AC publishes an authoritative `none` selection and clears the central pane. It never selects a sibling, an exited Root session, or any other unrelated fallback. The pane stays neutral until a later authoritative selection fills it.

Closing a nonselected session leaves the current selection unchanged. This no-fallback rule is specific to automatic close; manual close keeps its eligible live-session fallback behavior.

## Two badges, two meanings

This page describes two separate sidebar badges. The **idle badge** (`Nm`) counts how long a team has been idle. The **AUTO-CLOSED badge** marks an orchestrator that auto-close has already shut down. The next two sections cover each one.

## The idle badge

Every orchestrator row in the sidebar shows an idle badge: the whole number of minutes since the team was last active, formatted `Nm`.

- `0m` right after activity.
- `45m` after 45 minutes of silence.
- It counts minutes only, with no hour or day rollover, so a team idle for three days reads `4320m`.

The badge changes color as idle time grows:

| Idle time | Color |
|---|---|
| Below the yellow threshold (default 30m) | Default, no warning color |
| At or above the yellow threshold (default 30m) | Yellow |
| At or above the red threshold (default 60m) | Red |

You set both thresholds yourself. `coordinatorIdleBadgeYellowMinutes` (default `30`) is the idle minute count at which the badge turns yellow, and `coordinatorIdleBadgeRedMinutes` (default `60`) is the count at which it turns red.

Both keys change **the badge color only**. They do not move the auto-close timeout: that stays `coordinatorAutoCloseMinutes`. Raising the red threshold to `120` while the timeout stays at `60` gives you a team that is closed while its badge is still yellow, so keep the red threshold at or below the timeout if you want the red badge to mean "about to close".

The badge is **informational and always on**. It shows even when auto-close is turned off, so you can monitor idle teams without letting AC close them.

## The AUTO-CLOSED badge

When auto-close fires and the session it destroys is the team's **orchestrator**, AC stamps that orchestrator row with an **AUTO-CLOSED** badge. This is your record that the team was closed by the timeout, not by you.

If only a non-orchestrator member is reaped while the orchestrator survives (the orchestrator was spared by a grace window or a late user message), the surviving orchestrator keeps its idle counter and is **not** stamped. The AUTO-CLOSED badge means specifically "this orchestrator's own session was auto-closed."

## Orchestrator cascade close

Cascade close is the other way a team's sessions go down together, and it is **the one you trigger**. When you close an orchestrator yourself, `coordinatorCascadeCloseEnabled` decides whether AC also closes that team's member sessions. It is `true` by default, so closing an orchestrator closes the team.

The cascade covers the members of that orchestrator's team that have a live PTY, and never the orchestrator's own siblings in other teams. Dormant and already-exited member rows are left alone: there is nothing to terminate.

If at least one live member is still working (it is not waiting for your input), AC does not close anything yet. It reports how many members are busy and asks you to confirm, so a cascade never silently kills an agent mid-task.

Set the key to `false` and only the orchestrator closes. Its members keep running as independent sessions, which is what you want when you are restarting an orchestrator and do not want its agents to lose their PTYs.

This key does **not** change what auto-close does. Auto-close closes orchestrators and agent-owned member sessions on the rules described above, whatever `coordinatorCascadeCloseEnabled` is set to.

## Settings

These keys live in `settings.json` (see the [settings reference](../reference/settings.md#session-auto-close)). Defaults shown.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `coordinatorAutoCloseEnabled` | bool | `true` | Master switch. When false, AC never auto-closes a team (the badge still shows). |
| `coordinatorAutoCloseMinutes` | number | `60` | Idle minutes before a team is closed. `0` also disables auto-close. |
| `coordinatorAutoCloseSkipTelegramAssigned` | bool | `false` | When true, auto-close skips sessions with Telegram assigned. Other sessions keep following the normal auto-close rules. |
| `coordinatorCascadeCloseEnabled` | bool | `true` | When true, closing an orchestrator yourself also closes its team's live member sessions. When false, only the orchestrator closes. It does not affect auto-close. |
| `coordinatorIdleBadgeYellowMinutes` | number | `30` | Idle minutes at which the badge turns yellow. Badge color only. |
| `coordinatorIdleBadgeRedMinutes` | number | `60` | Idle minutes at which the badge turns red. Badge color only. |

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

### Keeping Telegram-assigned sessions alive

Set:

```json
{
  "coordinatorAutoCloseSkipTelegramAssigned": true
}
```

When enabled, auto-close skips only sessions with Telegram assigned. Non-Telegram sessions in the same team still follow the configured timeout. A Telegram-protected member may remain as an orphan after a non-Telegram orchestrator is auto-closed, while the setting stays enabled and Telegram remains assigned.

## How idle is measured

Idle time is measured from a single anchor per team: **the more recent of two clocks**.

- **Last user message.** Updated when you send input to any session in the team: typing in the terminal, a web keystroke, or a Telegram message.
- **Last activity.** Updated from real PTY output (the agents doing work), advanced on the watcher's tick.

The anchor is `max(last user message, last activity)`, and idle time is `now - anchor`. Either real work or your input resets the clock, so a team is only "idle" when both the human and the agents have gone quiet.

Passive viewing is not activity. Keeping the terminal visible, clicking it, or focusing it does not update either clock. AC needs actual input or genuine PTY output to reset the idle anchor.

### Why a reopened team keeps its old idle time

Reopening does not reset an existing persisted idle anchor. AC seeds an anchor only when the team has none. A reopened team can therefore keep a large idle value instead of starting at `0m`.

The session may replay its scrollback for a second or two after it wakes. AC ignores output during the first 10 seconds so that repaint output does not look like fresh activity. Genuine PTY output after that repaint grace advances the anchor normally. Separately, the 30-second wake grace keeps the reopened team visible before auto-close can act.

### Why a close can lag the timeout by up to a minute

The watcher checks teams once every 60 seconds. A team that has crossed its timeout is closed on the next tick, so a close can trail the threshold by up to a minute. This is also why a team is not closed the instant you walk away.

## Internals: how the badge value reaches the UI

> **Skip this section** unless you are debugging the UI or building on AC's events. Everything above is all you need to use auto-close.

There is **no idle field on the session object**. The anchor reaches the frontend out of band, on a Tauri event:

- Event: `coordinator_clock_updated`
- Payload: `{ "replicaPath": "<orchestrator cwd>", "lastUserMessageAt": "<RFC 3339 timestamp>" }`

The frontend computes `Nm` from that timestamp. The clocks themselves persist per team in `coordinator_clocks.json`.

> **Naming caveat.** The payload field is named `lastUserMessageAt`, but it carries the **unified** `max(user message, activity)` anchor, not the user-message clock alone. The name is kept for backward compatibility; read it as "the idle anchor."

## Troubleshooting

**"My ad-hoc shell got closed."** It should not have. Auto-close only targets orchestrators and agent-owned sessions; a plain shell with no coding agent is never a candidate. If you saw a shell close, it was not auto-close. Check the row tooltip for the real exit reason.

**"My team did not close after the timeout."** Confirm `coordinatorAutoCloseEnabled` is true and `coordinatorAutoCloseMinutes` is non-zero. Then remember the team must be established (a member alive 30 seconds or more) and have a live PTY, and the close happens on the next 60-second tick. Reopening does not refresh the persisted anchor. A previously idle team can qualify after the 30-second wake grace, then close on a later tick unless qualifying activity updates the anchor.

**"The idle badge never reaches red."** Something is resetting the anchor. Genuine PTY output after the 10-second repaint grace, or actual input, counts as activity. Passive viewing and focus do not. Watch the `Nm` value: if it keeps dropping back to a low number, the team is not actually idle.

**"A member closed but the orchestrator stayed."** Expected. Only the orchestrator's own auto-close stamps the AUTO-CLOSED badge; a reaped sibling leaves a surviving orchestrator counting normally.

## See also

- [Settings reference](../reference/settings.md#session-auto-close) - session auto-close keys
- [Concepts: Session](../concepts.md#session) - session status dots and lifecycle
- [Glossary](../glossary.md) - session auto-close, idle badge
