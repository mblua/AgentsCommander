# Watchers

For developers who want to know when a pattern appears in an agent's terminal, across every agent rather than one at a time. After this page you can configure a watcher, choose the mode and the deduplication that fit what you are matching, read the activity window, and work out why a watcher you configured is not running.

A watcher is a pattern AC matches against the terminal output of your agent sessions. Watchers live at the root of `settings.json`, keyed by watcher id, so one pattern can apply to every agent at once. Matches land in the Watcher Activity window.

## What a watcher is

A watcher is a context-scrape pattern with an id, a mode, and rules about which agents it reaches and what counts as a repeat.

Two properties are worth understanding before anything else:

- **A watcher reaches agents, not sessions you open by hand.** Only agent sessions are registered with the watcher engine, exactly as with the context scraper. A plain shell has a terminal but never a watcher.
- **A watcher is root-level.** The per-agent `contextRegex` can only describe one agent's reading. A watcher can apply to every configured agent, which is the shape `contextRegex` cannot express.

A malformed entry does not take the rest down with it. AC skips that one watcher, writes one log line, and leaves every other watcher and every other setting working.

## The two modes: state and occurrence

`mode` is required and has two values, and picking the wrong one is the most common way to get surprising activity.

| Mode | What it means |
|---|---|
| `state` | A **reading**. It is idempotent and gated: the same state observed again is the same state, not a new event. |
| `occurrence` | An **event**. Every match that the frame diff declares evaluable counts as its own occurrence. |

Use `state` for something that is true or false at a moment, such as a status line. Use `occurrence` for something that happens, such as a warning being printed. Deduplication, below, applies to occurrence matching.

## Deduplication

Two settings decide when two matches are "the same one":

- `dedupe` picks what is compared: `row` (the default), `capture`, or `none`.
- `dedupeWindowMs` is how long that comparison holds, in milliseconds. The default is `2000`.

The window is capped at **60000 ms**, and an oversized value is **clamped, not rejected**. Configure `dedupeWindowMs: 600000` and the watcher still runs, with a one-minute window and this log line:

```text
[watchers] watcher '<id>' asks for a 600000 ms dedupe window; clamping to 60000 ms
```

That is worth knowing before you conclude that a long window is not working: it is working, at 60 seconds.

## Commands a watcher can run

The `commands` field does not tell a watcher to run a command. It is a **selector**: it decides which coding-agent commands the watcher reaches. A watcher matches patterns; it never executes anything.

| `commands` value | Which agents the watcher reaches |
|---|---|
| Absent or `null` (the default) | Every configured agent. |
| A list of commands | Only agents whose `command` executable stem matches an entry exactly. |
| An empty list | No agent at all. |

Matching is on the executable stem, so `claude` matches an agent launched as `claude --effort max`.

If any entry in the list is not a command, AC **skips the whole watcher**:

```text
[watchers] watcher '<id>' is being skipped: its commands selector entry '<token>' is not a command. A watcher with an unreadable selector reaches nobody, never everybody
```

The refusal to guess is deliberate. A typo in one selector entry must never silently widen a pattern to every agent you run.

## The watcher budget

Each agent runs at most **eight** watchers. The number is a compile-time constant with no settings key behind it, and it is **per agent, not global**: configure thirty watchers if you want, and each individual agent runs at most eight of the ones that reach it.

At the limit nothing is rejected, dropped or evicted. The overflow watchers stay configured, stay valid, and do not run on that agent. The same watcher can be over budget on one agent and running happily on another.

**Which eight win: watcher id order, ascending.** Watchers are resolved in key order, and the first eight that reach an agent are the ones that run. That has a consequence worth stating plainly: **renaming a watcher can change which watchers run**, because the id is the sort key.

Disabled watchers never consume budget. A watcher with `enabled: false` is skipped before the candidate list is built, silently, because that is a state you chose rather than a problem.

You can observe the limit in exactly two places:

1. **One log line per agent**, naming every displaced id in a single line:

   ```text
   [watchers] agent '<agent id>' is over the 8-watcher budget; these are configured but not running on it: <id>,<id>,...
   ```

2. **A notice in Settings**, appended to the watcher's reach description: ` Not running on <agent labels> (budget).` It is empty when the watcher is disabled or when nothing was displaced.

There is **nothing on the terminal**: no toast, no modal. If you are waiting for a popup to tell you a watcher is not running, you will wait forever.

## The Watchers window

Matches land in a separate window titled `Watcher Activity`.

The window is scoped: a selector at the top chooses one agent session or `All sessions`. The scope decides both what is fetched and how much is kept: **500 rows for a single session, 100 for all sessions**.

Each row carries the watcher id, the agent it fired on, the workgroup, and the matched text; selecting a row expands it. Four filters narrow the list, by watcher, by agent, by workgroup, and by free text, and they combine. Alongside the rows the window reports which watchers are currently active, which are degraded, whether the activity was truncated, and how many frames may have been missed.

The window remembers its own position and size. That geometry is what `watchersGeometry` holds, which is why the window reopens where you left it.

## Settings

| Key | What it controls |
|---|---|
| `watchers` | The watcher patterns, keyed by watcher id. `{}` by default. Resolved in key order against the eight-watcher budget. |
| `watchersGeometry` | Position and size of the Watcher Activity window. `null` by default. |

See [Settings reference](../reference/settings.md#watchers) for the full `WatcherConfig` shape, field by field. This page explains the behavior rather than repeating the schema.

## Troubleshooting

**"I configured a watcher and it never fires on one agent."** Check the budget first. Look for `[watchers] agent '<agent id>' is over the 8-watcher budget` in the log, and for ` Not running on <agent labels> (budget).` on the watcher in Settings. Watcher ids sort ascending and the first eight win, so an id late in the alphabet is the one that loses.

**"A watcher stopped reaching every agent after I edited it."** Its `commands` selector no longer tokenizes. The log line names the offending entry: `[watchers] watcher '<id>' is being skipped: its commands selector entry '<token>' is not a command`. The whole watcher is skipped, so the symptom is "nothing anywhere", not "nothing on one agent".

**"One watcher broke and now nothing works."** That is not what happens, and the log says so: `[watchers] watcher '<id>' is not a valid watcher and is being skipped; every other watcher and every other setting is unaffected: <detail>`. If everything really did stop, the cause is elsewhere.

**"My long dedupe window is being ignored."** It is being clamped to 60000 ms, and the log line names the value you asked for. Anything above one minute behaves as one minute.

**"The watcher works but I see nothing in the window."** Check the scope selector and the four filters: a filter that hides everything looks exactly like a watcher that never fired. Also check that the session is an agent session; a plain shell is never watched.

## See also

- [Context tracking](context-tracking.md) - the per-agent `contextRegex` reading and its badge
- [Settings reference](../reference/settings.md#watchers) - the `WatcherConfig` schema
- [Concepts](../concepts.md) - session, workgroup and agent vocabulary
