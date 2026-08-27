# Context tracking

For developers who want to see how much of a coding agent's context window is gone before it runs out. After this page you can turn the reading on for an agent, read the badge on a session row, set per-team alert thresholds, and know exactly when AC notifies an orchestrator and when it does not.

AC scrapes the context percentage out of an agent's own terminal output using a pattern you configure, shows it as a badge on the session row, and can notify a workgroup's orchestrator when a member crosses a threshold you set. It never acts on the reading itself.

## What it tracks

The reading comes from the agent, not from AC. Each coding agent in your catalog can carry a `contextRegex`: a regex pattern AC matches against that agent's terminal to pull out a percentage.

Two consequences follow from that:

- **No pattern, no reading.** An absent or blank `contextRegex` disables the reading for that agent. The value is used byte-for-byte and is never trimmed, so a pattern with a stray space is a different pattern.
- **The reading is best-effort.** It is whatever the agent printed and the pattern matched. Treat it as an indicator, not as an accounting of the context window.

The reading is per session, because the pattern belongs to the coding agent that session is running.

## The context badge

Each session row carries a badge with the current reading:

| Badge | Meaning |
|---|---|
| `CTX 42%` | The last successful reading for that session. |
| `CTX N/A` | The badge is configured, but there is no reading yet. |

The badge is exposed as a meter running from 0 to 100, labelled `Context window used`, with the value read out as `Context <n>% used` for assistive technology.

The badge appears only when the session's coding agent has a non-blank `contextRegex`. A session whose agent has no pattern shows no badge at all, which is different from `CTX N/A`: the first means "not configured", the second means "configured, nothing read yet".

## Context alerts

A team can carry up to **three** context alert thresholds. When a member session crosses one, AC sends that workgroup's orchestrator an informational message. **No automatic action is taken**: no session is cleared, closed, restarted, handed off, or reassigned.

The thresholds are validated when you save them:

- Each is a whole percentage from **1 through 100**.
- They must be **distinct**; two equal thresholds are rejected.
- At most **three** per team.
- They are stored **sorted ascending**, whatever order you typed them in.

**Alerts are off by default.** A newly built team has no thresholds at all, so nothing fires until you add one.

**The firing rule: once per crossing.** Each threshold has its own latch. When a reading reaches or passes a threshold whose latch is armed, AC queues the alert and latches that threshold. While the session stays above it, later readings are skipped, so a session parked at 92 percent does not alert every time it is sampled. Thresholds crossed in the same reading are queued together and delivered as one message.

**A session that drops below and climbs back alerts again.** That is the behavior most likely to surprise you, and it is the point of the latch: it suppresses repetition, not recurrence.

Four things re-arm a latch:

1. the reading drops back below that threshold, the normal case and the only one you control directly;
2. the session ends;
3. the session is reported unavailable and confirmed no longer live, or its member policy resolves to disabled or permanently ineligible;
4. the session's identity fingerprint changes.

The last three drop the session's whole alert state, latches included.

## Setting thresholds for a team

Thresholds are edited in the team modals, in a section headed `Context usage alerts (optional)`. They apply to **every workgroup of that team**.

The editor works one threshold at a time:

- `Add threshold` appends a row, and a counter beside it reads `<n> of 3 thresholds`. The button disables at three.
- Each row is labelled `Threshold <n> percentage` and takes a number, with a `%` suffix shown beside the input.
- `Remove` deletes that row.
- With none configured, the section reads `No context alerts configured.`

**These thresholds are not a `settings.json` key.** They are stored in the team's own configuration, alongside the rest of what the team defines. There is no global context-alert setting, and nothing you can add to `settings.json` will turn alerts on for every team at once.

## The injected alert message

The message AC injects is a template with the id `context-alert`, and it ships pinned to this text:

```text
[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No action taken; you decide any follow-up.
```

Three tokens are substituted: `%MEMBER%` for the member that crossed, `%WORKGROUP%` for the workgroup it belongs to, and `%THRESHOLDS%` for the threshold or thresholds crossed in that reading.

The template is operator-editable. If you have changed it and want the shipped default back:

```bash
agentscommander injected-messages reseed --id context-alert
```

That rewrites only the `context-alert` entry, after writing a timestamped `.bak-` copy, and leaves your comments, entry order and every other message untouched. See [`injected-messages`](../reference/cli.md#injected-messages) for the full command.

## Settings

| Key | What it controls |
|---|---|
| `contextRegex` | The per-agent regex that produces the reading. `null` by default; absent or blank disables the reading for that agent. |

See [Settings reference](../reference/settings.md#coding-agents) for the field in context; it lives on each entry of the coding agent catalog.

## Troubleshooting

**"The session row shows no badge."** The coding agent that session runs has no `contextRegex`, or it is blank. Set the pattern on the agent, not on the session.

**"The badge is stuck on `CTX N/A`."** The pattern is configured but has matched nothing yet. Check that the agent actually prints a context percentage, and that the pattern matches its exact output; the value is used byte-for-byte, so a trailing space in the pattern is a real difference.

**"A member is clearly over the threshold and no alert arrived."** Most often the alert already fired for that crossing and the threshold is latched. It re-arms when the reading drops back below. If the reading has never been under it, check the log: **a reading above 100 percent is rejected outright, with an error line and no alert**, so an agent printing a nonsense percentage produces silence rather than a warning.

**"The team modal rejected my thresholds."** The validation is strict and says which rule you hit: at most three values, each a whole percentage from 1 through 100, and all distinct.

**"I edited the alert text and want the original back."** Run `agentscommander injected-messages reseed --id context-alert`. The previous version is preserved in a timestamped `.bak-` file next to the config.

## See also

- [Watchers](watchers.md) - root-level patterns that reach every agent, where `contextRegex` reaches one
- [`injected-messages` CLI reference](../reference/cli.md#injected-messages) - resetting the `context-alert` template
- [Settings reference](../reference/settings.md#coding-agents) - `contextRegex` and the coding agent catalog
