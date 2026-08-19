# Non-stop mode

For developers running several workgroups at once who cannot watch all of them. After this page you can put a workgroup under non-stop watch, set how long a stall may last before AC tells you, and diagnose a group that looks configured but never alerts.

Non-stop mode watches a group of workgroups and tells you when one of them stops working. You choose the workgroups, how long a stall is allowed to last, and how AC reaches you: a Telegram message, a sound, or both. AC checks once a second and alerts once per stall, so a workgroup that stalls for an hour does not produce an hour of alerts.

## What it does

Each project has one built-in non-stop group. Its default name is `Alert me!`, and it sits pinned above the groups you create yourself.

While that group has at least one member workgroup and at least one alert measure turned on, the sidebar reports the group's state to the backend: how many members are working, how many there are in total, and the names of the members that are not working. AC calls "not every member is working" a **disparity**.

A disparity on its own is not an alert. Workgroups pause between turns all the time, and a pause of a few seconds is normal. What AC acts on is a disparity that **lasts**, which is what the group's tolerance sets.

If neither Telegram nor sound is enabled for the group, the sidebar does not report it at all. That is deliberate: a group with no way to reach you has nothing to fire, so AC stays quiet rather than tracking a state nobody will see.

## Turning it on

Two controls do the same thing, and either one is enough.

- **The workgroup rail.** The rail shows one entry per group, and the non-stop entry carries the group's display name (`Alert me!` unless you renamed it). Selecting it shows the workgroups currently in the group.
- **The project panel.** Each workgroup row in the project tree has a non-stop checkbox in the built-in slot pinned above your own groups. Its tooltip reads `Watch this workgroup in the Alert me! group`. Tick it and that workgroup joins the group; untick it and it leaves.

Turning the group on is not the whole setup. Enable Telegram, sound, or both for the group, otherwise nothing is watched. See [Troubleshooting](#troubleshooting).

## What the watchdog does

The watchdog is a backend loop that ticks **once a second** for as long as AC runs. Nothing you configure stops it: turning non-stop off for a workgroup changes what a tick finds, not whether ticks happen.

On each tick, for each project's non-stop group:

- If there is no disparity, there is nothing to arm.
- When a disparity begins, AC records when it started.
- When that disparity has lasted the tolerance configured for the group, the episode **fires once**. It does not fire again while the same disparity continues.
- If the disparity ends, the episode re-arms and can fire again on the next one.

The watchdog also protects itself against a frontend that has gone away. An armed episode whose last report is older than **180 seconds** is disarmed instead of fired, and AC logs:

```text
[non-stop] '<project path>' disarmed: no frontend report for >180s (frontend gone)
```

That is why closing the window that reports stops alerts rather than triggering them.

## Scope: per workgroup

Non-stop is **per workgroup within one project**, never a global switch. Each project keeps its own non-stop group, and a workgroup is watched only if it is a member of its own project's group.

The group's display name is derived, not stored blindly:

- The name you set is trimmed of surrounding whitespace.
- An empty name falls back to `Alert me!`.
- The legacy name `Non-stop` also falls back to `Alert me!`, so instances configured before the rename show the current name.
- A longer name is capped at the group-name limit.

The rail entry shows that derived name. The project panel does not: its non-stop slot and the slot's tooltip always render the built-in `Alert me!`. Rename the group to anything that does not fall back to that name and the two surfaces disagree until you rename it back.

## Where the configuration lives

**There is no non-stop key in `settings.json`.** Non-stop is not a global preference, so it is not in the global settings file and there is no key to add there.

The configuration is per project, in the project's `.ac/project-settings.json`, alongside the rest of the project's group configuration. AC writes that file for you when you use the rail or the project panel; you do not need to edit it by hand.

What that file holds for the group: its name, its member workgroups, its tolerance in seconds, and the two measures (Telegram, with its bot, and sound, with its duration).

## Troubleshooting

**"The group has workgroups in it and nothing ever alerts."** Check that Telegram or sound is enabled for the group. With neither enabled, the sidebar stops reporting the group and the watchdog never sees it. This is the most common cause, and it is silent by design: there is no error and no toast.

**"The group is on, a workgroup is clearly stalled, and nothing happened."** Two checks. First, the group must have at least one member: an empty group produces no report, because no disparity is possible. Second, the stall must outlast the group's tolerance; a stall shorter than that never arms an episode long enough to fire.

**"Alerts stopped after I closed a window."** Look for `[non-stop] '<project path>' disarmed: no frontend report for >180s (frontend gone)` in the log. The backend disarms an episode when the sidebar stops reporting for more than three minutes, so a closed or unloaded frontend silences the group instead of firing it.

**"I renamed the group and the rail still says `Alert me!`."** The name `Non-stop` is treated as the legacy name and falls back to `Alert me!`. Pick any other name.

## See also

- [Concepts](../concepts.md) - workgroup, session, and what "working" means for a session
- [Project Loops](project-loops.md) - the other way AC acts on a workgroup without you
- [Telegram bridge](telegram-bridge.md) - the bridge that delivers a Telegram alert
