# Sidebar guide

For developers who want to read the sidebar rather than click around it. After this page you know what each rail entry, row, badge and indicator means, and which page owns the feature behind it.

The sidebar has three parts: a rail of group entries down one edge, the project panel that lists projects, workgroups, replicas and sessions, and an action bar of toggles across the top. This page is the map. Where a badge belongs to a feature with its own page, this page says what the badge means and sends you there.

Four of the action bar's toggles change what the panel shows, and each tooltip tells you what pressing it will do rather than what is on:

- `Show Home` and `Hide Home` swap the central pane between Home and the terminal.
- `Show recent coordinators first` and `Show coordinators in default order` reorder the coordinator list. It persists as [`coordSortByActivity`](../reference/settings.md#window--ui).
- `Show category sections` and `Hide category sections` toggle the panel's category grouping.
- `Always keep selected workgroup visible` pins the selected workgroup in view. It persists as [`alwaysShowSelectedWorkgroup`](../reference/settings.md#window--ui).

## The workgroup rail

The rail shows one entry per group. An entry stands for a **set of workgroups**, not a single one: selecting it shows the workgroups that belong to that group.

Each entry has two lines. The first carries the group's name, preceded by a raise-hand indicator when one applies. The second carries a counter, preceded by a running dot when something in the group is working. Hovering an entry shows a tooltip naming the project folder and the workgroups the entry covers.

Entries you created are draggable, so you can reorder them; the built-in ones are not. Right-clicking an entry opens its context menu, and the selected entry is marked as pressed for assistive technology.

The rail collapses by section. Clicking a project section header collapses it, and AC remembers which ones you collapsed in `railCollapsedProjects`. The cross-project Favorites section has its own collapsed state, `railFavoritesCollapsed`. Both are written only by the dedicated rail collapse action, so an unrelated settings save cannot lose them.

## Favorites and groups

A **group** is a named set of workgroups. You edit groups in the `Edit groups` modal, where each group has two fields: `Group name`, and `Group regex`, the pattern that decides which workgroups belong to it. A new group starts named `Group 1`, `Group 2` and so on. A group can also carry a sound alert, whose length is set in seconds.

Because membership is a pattern rather than a list, a workgroup created later joins the group on its own if its name matches. That is the point of the regex, and it is also the thing to check when a workgroup shows up in a group you did not expect.

**Favorites** is the rail's own cross-project section, listed above the project sections and collapsible independently. Its collapsed state is `railFavoritesCollapsed`.

You mark one from the rail itself: right-click a rail entry and the menu offers `Edit`, which opens the groups editor, and `Favorite`. The same entry reads `Unfavorite` once the group is marked, so the label always tells you what pressing it will do. Only a group entry and the built-in non-stop entry can be favorited; the menu shows that second item for nothing else.

One group is built in and cannot be removed: the non-stop group, described in [Non-stop mode](non-stop-mode.md).

## Raise hand

A raised hand means **a coordinator is asking for your attention**. The indicator appears on the rail entry for the group the coordinator is in, with the tooltip and accessible label `A coordinator raised its hand`, and on the replica row in the project panel.

An agent raises its own hand; you never raise it for it. It calls the CLI from inside its session:

```bash
agentscommander raise-hand --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

The daemon accepts it only when the caller's token belongs to a **live coordinator session with a visible `TASK.md` title slot**. Stdout is exactly `true` or `false`.

**It is cleared by real user input to that session, and by nothing else.** It survives an app restart. Clicking the row, selecting it, or looking at it does not lower the hand: type something to the session.

## The project panel

The panel is a tree. Projects contain workgroups and teams; those contain agents and replicas; a replica contains its sessions.

A replica row shows the replica's path as its tooltip and carries the indicators and controls for that replica: the idle badge, the repo badges from branch discovery, and the per-session controls for voice, detaching and Telegram. The Telegram control opens its own bot menu.

Right-clicking a row opens a context menu, and AC builds a different one depending on whether the row has an active session or is an inactive replica.

**With an active session**, the menu offers `Restart Session`, `Coding Agent`, `Open Replica's Folder` (tooltipped with the path it will open), one entry per repository AC discovered in the replica, `Open Matrix folder` when the replica has an Agent Matrix behind it, then `Open in new window`, which reads `Re-attach to main` when the session is already detached, an entry to add the replica to a group, and `Clear task title`.

**On an inactive replica** the same menu appears without the two entries that need a running session: no `Restart Session` and no detach toggle.

`Clear task title` is disabled when there is nothing to clear, and says which case you are in through its tooltip: `Clear task title`, or `Nothing to clear`.

Right-clicking a project row instead gives you the project's own menu, which is where `Archive Project` lives. See [Project archiving](project-archiving.md).

## What a session row shows

A session row packs several indicators. Each belongs to a feature documented elsewhere; this is the key:

| Indicator | What it means | Page |
|---|---|---|
| Status dot | The session's lifecycle state | [Concepts: Session](../concepts.md#session) |
| Profile drift badge | The session's coding-agent profile no longer matches its definition | [Coding Agent Profiles](coding-agent-profiles.md#drift-the-outdated-badge) |
| Context badge | How much of the agent's context window is used | [Context tracking](context-tracking.md) |
| Idle badge and AUTO-CLOSED badge | How long the team has been idle, and that auto-close already closed it | [Session auto-close](session-auto-close.md) |
| Mic | Voice capture for that session | [Voice-to-text](voice-to-text.md) |
| Telegram | A bridge is attached to that session | [Telegram bridge](telegram-bridge.md) |
| Git branch | The session's current branch and whether its repository is dirty | The next section |

The row also carries the actions for the session: close, detach, Telegram and the file explorer.

## The git branch badge

AC polls each session's repository in the background and shows the branch on the row.

**Dirty means one of two things**, and the distinction matters: either the working tree has uncommitted changes, or `HEAD` is not contained by the cached origin tracking, which is what you see when you have committed locally and not pushed. A clean tree with unpushed commits still reads as dirty, on purpose.

A repository in a state with no branch name, such as a detached `HEAD` or a rebase in progress, shows no branch rather than a guess.

Two settings shape the polling. `gitSweepConcurrency` is how many repositories the sweeper inspects at once, clamped to 1 through 4; `1`, the default, is strictly sequential and is what bounds concurrent `git.exe` processes. `gitSweepMinIntervalSecs` is a lower bound on one sweeper round, clamped to 1 through 3600. The effective period is the larger of that bound and the round's own duration, so on a large set of workgroups the round duration dominates and the setting never fires.

Both are manual-only, with no UI, and take effect on the next restart.

## The agent picker

The picker opens for one target and is titled `Assign profile for <target>`, naming the replica or session it will act on. Despite the name, it is the coding-agent **profile assignment** dialog: it lists your configured coding agents, sorted by label, and assigns a profile letter to the target.

The apply scope decides how far the assignment reaches: this replica, every replica of the same kind, or the whole workgroup. A wider scope shows a preview of how many targets it would overwrite and requires you to confirm in as many words, for example `I understand this overwrites 4 replicas of this kind`. You can also ask AC to restart the matching sessions after writing the selection.

See [Coding Agent Profiles](coding-agent-profiles.md) for what a profile is and how the letters resolve.

## Quick coding-agent configuration

This inline panel configures a coding agent without opening Settings. It offers the known coding agents as one-tap presets, each button labelled `Select <agent>` for assistive technology, plus a custom entry where you supply the label and command yourself. When it succeeds it confirms with the agent's name, for example `claude configured!`.

It writes the same coding agent catalog as the Settings `Coding Agents` tab; the tab is where you edit an existing agent's environment rows, backend, profiles and seeds. See [Coding agents](../integrations/coding-agents.md) for the full configuration surface.

## Branch and repo discovery

For a workgroup replica, AC discovers the repositories inside it and shows one badge per repository. The badge text is the repository's label, and its branch when one is known, as `label/branch`.

The badge's tooltip carries the repository's source path plus its status. When the repository is dirty it reads `<path> (local work not confirmed by cached origin tracking)`, and when AC could not determine the status it reads `<path> (status unknown)`.

The panel updates from the `ac_discovery_branch_updated` event, so a branch you switch in a terminal appears here without a refresh.

## Zoom

The zoom control sits in the titlebar as a group labelled `UI zoom`: a `Zoom out` button, the current percentage, and a `Zoom in` button. Steps are 10 percentage points, and the range runs from **50% to 300%**. Each button disables at its end of the range.

The value persists per window. `mainZoom`, `terminalZoom`, `sidebarZoom` and `guideZoom` each default to `1.0`, which is 100%.

## Settings

| Key | What it controls |
|---|---|
| `gitSweepConcurrency` | How many repositories the git sweeper inspects at once. `1` by default, clamped to 1 through 4. |
| `gitSweepMinIntervalSecs` | Lower bound in seconds on one sweeper round. `10` by default, clamped to 1 through 3600. |

See [Settings reference](../reference/settings.md#git-status-sweeper) for both, including why raising the concurrency is rarely the fix.

## Troubleshooting

**"A workgroup appeared in a group I did not put it in."** Group membership is a regex over workgroup names, not a list. Check the group's `Group regex` in the `Edit groups` modal against the new workgroup's name.

**"The raise-hand indicator will not go away."** It is cleared by real user input to that session and survives restarts. Clicking or selecting the row is not input; send the session something.

**"A branch badge says dirty and `git status` says the tree is clean."** Dirty also covers `HEAD` not being contained by the cached origin tracking, which is the ordinary state after committing without pushing.

**"A session row shows no branch at all."** The repository is in a state with no branch name, such as a detached `HEAD` or a rebase in progress. AC shows nothing rather than guessing.

**"My rail sections keep collapsing themselves."** They are remembered on purpose, in `railCollapsedProjects` and `railFavoritesCollapsed`. Expanding a section rewrites the setting.

**"The zoom buttons are greyed out."** You are at an end of the range: zoom stops at 50% and at 300%.

## See also

- [App windows](app-windows.md) - the windows the sidebar opens beside itself
- [Concepts](../concepts.md) - project, workgroup, replica, session and coordinator
- [Coding agents](../integrations/coding-agents.md) - the catalog the picker and the quick panel write
