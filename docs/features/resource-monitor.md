# Resource monitor

For developers running enough agents at once to worry about memory. After this page you can read what each agent group is consuming, set the thresholds that count as too much, and decide whether AC warns you or kills the group.

The Resource Monitor samples every agent session and its child processes, shows what each one is using, and compares the numbers against thresholds you set. A watchdog acts on those thresholds: it either surfaces the state and leaves the processes alone, or it terminates the offending session's whole process group.

## What it measures

The unit of measurement is the **agent group**: one agent session plus the processes it spawned. AC reports per group:

- Its identity: the session name, and the project, workgroup and agent role it belongs to.
- Its state, and the last error if the group has one.
- How many processes it contains, and whether AC could observe the descendants at all.
- Private bytes, working set bytes and CPU percent.
- Its network state and a one-line network summary.

Above the groups, AC reports application-wide figures: the app's own private and working-set bytes, how many agent groups are active against the configured limit, and an overall state that folds in the network state.

Private bytes is the number the thresholds compare against, for both the group total and each individual process.

## Turning it on

`resourceMonitorEnabled` is the master switch and is **on by default**. With it off, the window still opens but reports nothing and shows the banner `Resource monitoring is disabled.`

The switch does more than blank the display. The watchdog reads the same key and stops on every tick while it is false, so turning the monitor off also turns the watchdog off. There is no way to run the watchdog without the monitor.

The monitor also needs the platform to support process-tree enforcement. Where that support is missing, the watchdog does nothing at all whatever action you configure, even though the readings still appear.

## The Resource Monitor window

The window is titled `Resource Monitor` and has three controls in its header: `Refresh` takes a fresh sample immediately, `Settings` opens the `Resources` tab of the Settings dialog, and `Detach` (see the next section) appears only when the monitor is embedded in the main window.

A strip of four tiles summarizes the whole app:

| Tile | What it shows |
|---|---|
| `State` | The overall state, folding in the network state. |
| `Active Agents` | Active agent groups against the configured maximum. |
| `App Private` | AC's own private bytes, with the working set alongside. |
| `Network` | The network state and its summary line. |

Below that, the `Agents` section lists the groups. A filter bar narrows the list by status (all, active or inactive), by project, by workgroup and by agent role; while any filter is on, the header reads `Showing <n> of <total>`. The header also carries `Last update <time>`. Selecting a group row expands it to the processes inside that group, and a group whose kill is allowed can be terminated from here after a confirmation.

The list refreshes on a timer: every 2 seconds while active and every 10 seconds when idle, dropping to 15 seconds when `resourceBackoffPolling` is on. When a sample fails, the window shows `Snapshot failed: <error>`; with `resourceKeepLastSnapshot` on it keeps the previous reading on screen under `Showing last snapshot from <time>.` instead of blanking.

## Attaching it to the main window

The monitor renders in either of two places: its own window, or the central pane of the main window in place of the terminal. `mainResourceMonitorAttached` records which, defaults to `false`, and is restored at startup, so the layout you left is the layout you come back to.

When it is embedded, the header gains a `Detach` button, tooltipped `Detach to a separate window`. Pressing it opens the standalone window and returns the main window's central pane to the terminal.

## The watchdog: thresholds and actions

Keep two ideas apart. **The thresholds decide whether a group is over the line. `resourceWatchdogAction` decides what AC does about it.** They are not a mapping: every threshold is evaluated on every tick, whatever the action is set to.

Three thresholds are evaluated, against groups in the running state only:

| Threshold key | Compared against |
|---|---|
| `agentGroupWarnPrivateBytes` | the group's total private bytes |
| `agentGroupKillPrivateBytes` | the group's total private bytes |
| `agentProcessKillPrivateBytes` | each individual process's private bytes |

A group needs killing when it crosses either kill threshold, the group one or the per-process one. It needs warning when it crosses the warn threshold or already needs killing.

`resourceWatchdogAction` has exactly two values:

| Value | What AC does |
|---|---|
| `"warn"` | Computes and surfaces the threshold state. It terminates nothing. This is the default. |
| `"killGroup"` | Does the same, and additionally terminates the process group of every session that needs killing. |

Two consequences that surprise people:

**There is no per-process kill.** Crossing `agentProcessKillPrivateBytes` marks the group, and the whole session's process group is terminated. AC identifies the offending processes but does not terminate them individually, so one runaway child takes its session down with it.

**`"warn"` is not "AC never touches my processes".** Groups that were quarantined by an earlier kill are still reclaimed under `"warn"`, on every tick. That cleanup is deliberately outside the action gate, because a leaked slot has to be reclaimed even on a default install. What `"warn"` guarantees is that no *threshold crossing* terminates anything.

## Settings

| Key | What it controls |
|---|---|
| `resourceMonitorEnabled` | Master switch for the monitor and the watchdog. `true` by default. |
| `maxConcurrentAgentProcesses` | The cap the `Active Agents` tile counts against. `32` by default. |
| `resourceWatchdogAction` | `"warn"` or `"killGroup"`. `"warn"` by default. |
| `agentGroupWarnPrivateBytes` | Group private bytes at which the group warns. 8 GiB by default. |
| `agentGroupKillPrivateBytes` | Group private bytes at which the group needs killing. 12 GiB by default. |
| `agentProcessKillPrivateBytes` | Private bytes of a single process at which its whole group needs killing. 12 GiB by default. |
| `resourceKeepLastSnapshot` | Whether a failed sample keeps the previous reading on screen. `true` by default. |
| `resourceBackoffPolling` | Whether the sampling interval backs off while idle. `true` by default. |
| `mainResourceMonitorAttached` | Whether the monitor occupies the main window's central pane. `false` by default. |

See [Settings reference](../reference/settings.md#resource-monitor) for types and exact defaults.

## Troubleshooting

**"The window says `Resource monitoring is disabled.`"** `resourceMonitorEnabled` is `false`. Turn it back on; note that the watchdog was off too while it was.

**"Nothing is ever killed, and the action is `killGroup`."** Two checks. First, the platform must support process-tree enforcement; without it the watchdog returns on every tick and no action value changes that. Second, `resourceMonitorEnabled` must be true, because the watchdog rides on it.

**"A session died and it was nowhere near the group threshold."** Look at `agentProcessKillPrivateBytes`. One process over that limit takes down the whole group, and the group total can be well under `agentGroupKillPrivateBytes` when that happens.

**"I set `warn` and AC still terminated something."** Quarantine reclamation runs under `warn` as well. It only touches groups a previous kill already left behind, never a group that has merely crossed a threshold now.

**"The numbers stopped moving and the window says `Showing last snapshot from <time>.`"** Sampling failed and `resourceKeepLastSnapshot` is holding the previous reading so the panel does not blank. `Snapshot failed: <error>` names the cause; press `Refresh` after fixing it.

## See also

- [App windows](app-windows.md) - the monitor as one of AC's separate windows
- [Settings reference](../reference/settings.md#resource-monitor) - the resource keys and their types
- [Session auto-close](session-auto-close.md) - the other way a session shuts down without you
