# Coding Agent Profiles

For developers who want more than one way to launch the same coding agent: a cheap variant, a max-effort variant, an isolated-config variant, switchable per agent and per session.

After this page you can define a `claude` "max effort" variant and a "cheap" variant, set one agent to default to one of them, and switch a single session to the other without editing any command by hand.

> **Upgrading from a pre-#597 config?** See [the breaking change](#the-effective-launch-command) before you launch: a profile cell now holds parameters, not a full command.

## What a profile is

A **profile** is a lettered launch variant (`A`, `B`, `C`, ...) of a coding agent. Each profile holds extra command parameters plus environment variables. You assign a profile to an agent or to a single session; at launch AC composes the agent's base command with the profile's parameters and starts the session with the result.

The whole feature is the **profile matrix**: a grid of these variants, one row per coding agent, one column per letter.

## Three things called "profile"

The word "profile" appears in three unrelated places in AgentsCommander. This page owns the third one. Keep them separate:

| Term | What it means | Where it lives |
|---|---|---|
| **Path placeholder** | A `%AC_REPLICA_ROOT%`-style token you can use *inside* a profile cell's command or env. Not a kind of profile. | [Agent Matrix conventions §5](../agent-matrix-conventions.md#5-profile-path-placeholders) |
| **Tuned coding-agent integration** | A first-class `CodingAgentKind` (resume tokens, idle tuning) for one CLI. Independent of A/B/C. | [Coding agents](../integrations/coding-agents.md), `session/profile.rs::CodingAgentKind` |
| **Profile (this page)** | A lettered launch variant of a coding agent, resolved from the profile matrix. | This page |

When this page says "profile" with no qualifier, it always means the third one.

## The profile matrix

The matrix is a grid. Each **row** is a coding agent (`claude`, `codex`, `gemini`, or a custom one). Each **column** is a profile letter. Each **cell** is one launch variant for that coding agent and letter:

| | A | B | C |
|---|---|---|---|
| **claude** | (params, env) | (params, env) | (params, env) |
| **codex** | (params, env) | (params, env) | (params, env) |

A cell holds four fields:

| Field | Meaning |
|---|---|
| `enabled` | Whether this cell participates in resolution. A disabled cell is skipped (except `A`, which is always available). |
| `command` | The extra parameters appended to the agent's base command. Params only, not the binary. See [The effective launch command](#the-effective-launch-command). |
| `env` | Environment variables for this variant. Overlaid on the agent's base env, profile wins on a key clash. |
| `notes` | Free text for your own reference. |

The matrix is keyed by **coding-agent id** (the row). Which *letter* a given agent or session uses is a separate assignment, resolved by the ranking below.

> **Out of the box only `A` exists.** You add `B`, `C`, and so on in **Settings -> Coding Agents**. Letters are single characters `A` through `Z`. The `A`/`B`/`C` examples on this page are illustrative, not a fixed set.

## Quick walkthrough

This runs the scenario from the top of the page end to end. It assumes a `claude` Coding Agent is already configured.

1. Open **Settings -> Coding Agents** and find the `claude` row.
2. Add profile **B** and set its params to `--effort max` (your "max effort" variant).
3. Add profile **C** and set its params to a cheaper variant, for example `--model <small-model-id>`.
4. On an agent, use **Set default** and pick **B**. New sessions for that agent now start on B.
5. Launch a session for that agent. It runs `claude --effort max`.
6. To switch only that session to C, open the launch picker, pick profile **C**, and assign it to the replica. The session relaunches as the cheaper variant. You never edited a command by hand.

## How a profile is chosen

Resolution runs in two distinct steps.

### Step 1: pick the requested letter (4-tier ranking)

AC picks the requested letter from the first source that has one, highest priority first:

| Tier | Source | Set by |
|---|---|---|
| 1. Instance override | The replica's `tooling.profile` in its own `config.json` | Assigned to the replica; persists across future launches |
| 2. Explicit request | The letter you pick for this one launch | The launch picker, for this launch only |
| 3. Origin default | The agent matrix's `tooling.defaultProfile` | The "Set default" action (per agent) |
| 4. Agent default | `codingAgentProfiles.defaultProfileByAgent[<agent>]` in `settings.json` | Rarely set by hand; see note below |
| Floor | `A` | Always available when nothing else resolves |

Tier 1 outranks tier 2 on purpose: a profile you deliberately assigned to a replica should survive future launches, so it beats an ephemeral letter picked for a single launch.

> **Naming trap:** the "Set default" action writes the **origin default** (tier 3), the per-agent-matrix `tooling.defaultProfile`. It does **not** write tier 4. Tier 4 (`defaultProfileByAgent`) has no dedicated button; in practice it is populated only by an inherited/migrated config or by hand-editing `settings.json`.

### Step 2: walk down to the nearest enabled cell

Once Step 1 picks a letter, AC looks up the cell for that coding agent and letter. If the cell is missing or disabled, AC walks **down** the alphabet toward `A` and uses the first enabled cell it finds (`D` -> `C` -> `B` -> `A`). `A` is always materialized, as an empty cell if you never defined one, so resolution always terminates.

When the cell that wins is not the letter that was requested, AC marks the result `fallbackApplied`. That is the signal behind "I asked for D but it launched with C."

**Worked example.** You request `D` for `codex`, but only `codex` cells `A` and `C` are enabled:

1. Step 1 picks `D` (say, from an explicit request).
2. Step 2 walks `D` -> `C`. `C` is enabled, so the effective profile is `C`.
3. The session launches with `codex`'s `C` params, and `fallbackApplied` is true.

## Assigning and defaulting a profile

**Set a default profile for an agent (tier 3).** Use the "Set default" action on the agent. This writes the origin matrix's `tooling.defaultProfile`, so every new session for that agent starts on that letter unless a higher tier overrides it.

**Override the profile for one replica/session (tier 1).** In the launch picker, pick the Coding Agent and Profile, then assign it to the replica. This writes the replica's `tooling.profile` (the instance override), which beats the agent default. The picker previews the exact composed command before you launch.

**Pick a profile for a single launch (tier 2).** Choosing a different letter at launch time, without assigning it to the replica, applies only to that launch.

## The effective launch command

The command that actually starts is the agent's base command followed by the profile cell's parameters, joined with a single space:

```
effective = "<agent base command> <profile cell params>"
```

The base command (the binary, plus any fixed args) comes from the Coding Agent entry in `settings.json`. The cell holds only the extra params. An empty side contributes nothing, and if both are empty the launch fails with `agent command is empty`.

**Example.** Base command `claude-amp`, profile `B` params `--effort max --model <model-id>`:

```
claude-amp --effort max --model <model-id>
```

Environment variables merge in layers: the agent's base env first, then the profile cell's env on top (profile wins on a clashing key), then any AC-generated env (such as an isolated `CODEX_HOME`). Path placeholders like `%AC_REPLICA_ROOT%` are allowed in either the base command or the cell and expand at launch; see [Agent Matrix conventions §5](../agent-matrix-conventions.md#5-profile-path-placeholders).

> **Breaking change: a profile cell holds parameters, not a full command.** AC composes the cell after the base command. If a cell still contains the whole command (for example `claude --foo` on a `claude` base), it composes to `claude claude --foo`, which launches wrong or fails. There is **no automatic migration**. Move the binary into the Coding Agent command and keep only parameters in each cell. Re-check every existing profile cell after upgrading.

## Drift: the "outdated" badge

A profile's identity is positional (coding-agent id plus letter), so editing a cell's command or env does not, by itself, change which cell a running session is on. To catch the case where a running session's profile no longer matches its configuration, AC fingerprints the effective command and merged env at launch and compares it on demand.

When the configuration drifts from what a session launched with, that session shows a clickable badge in the sidebar:

- **Badge:** `⟳ outdated`
- **Tooltip:** "Loaded profile no longer matches its configuration. Reload to relaunch with the current profile."
- **Click it** to relaunch the session with the current configuration. The relaunch clears the badge.

Drift is **manual**: AC never auto-reloads. The check covers an edit to the base command, the cell params, the base env, or the cell env. It survives an AC restart (the fingerprint is persisted per replica). Plain-shell sessions (sessions with no coding agent) never drift. In a directory that is not an agent replica the fingerprint is not persisted, so any drift is not retained across an AC restart.

## Where profiles are stored

Profiles live in two places: the global `settings.json` (the matrix and defaults) and per-agent `config.json` files (the per-agent and per-replica assignments).

| Datum | Location | Key |
|---|---|---|
| Matrix cells (params, env, notes) | `settings.json` | `codingAgentProfiles.profilesByAgent[<coding-agent-id>][<letter>]` |
| Profile letters and labels | `settings.json` | `codingAgentProfiles.profileSlots`, `codingAgentProfiles.profileLabelsByAgent` |
| Agent default letter (tier 4) | `settings.json` | `codingAgentProfiles.defaultProfileByAgent[<agent>]` |
| Origin default (tier 3) | agent matrix `_agent_<name>/config.json` | `tooling.defaultProfile` |
| Instance override (tier 1) | replica `__agent_<name>/config.json` | `tooling.profile` (legacy `tooling.instanceProfileOverride`) |
| Drift fingerprint | replica/matrix `config.json` | `tooling.profileContentHash` |

The full `codingAgentProfiles` schema (including `schemaVersion`) is in the [settings reference](../reference/settings.md#coding-agent-profiles). The matrix uses schema version 2; a version-1 config is upgraded and persisted on load, after a one-time v1 backup.

## There is no CLI for profiles

You configure profiles in **Settings** or by editing `settings.json` and the per-agent `config.json` files. The `agentscommander` CLI has no `profile` subcommand. Resolution happens internally when a session spawns.

## Troubleshooting

**"My session launched with A even though I asked for D."** Letter fallback (Step 2). There is no enabled `D`, `C`, or `B` cell for that coding agent, so resolution walked down to `A`. Enable the cell you want, or define its params.

**"Ignoring invalid profile letter '...'."** A profile letter must be a single character `A` through `Z`. AC logs this and ignores the bad value rather than failing the launch.

**"I edited a profile and the running session did not change."** Drift is manual. Click the `⟳ outdated` badge on the session (or restart it) to relaunch with the new configuration.

**"After upgrading, my agent launches the binary twice, or fails with `agent command is empty`."** The profile cell still holds a full command. Move the binary into the Coding Agent command and keep only parameters in the cell. See [the breaking-change note](#the-effective-launch-command).

## See also

- [Settings reference](../reference/settings.md#coding-agent-profiles) - the full `codingAgentProfiles` schema
- [Coding agents](../integrations/coding-agents.md) - the coding-agent catalog and tuned `CodingAgentKind` integrations
- [Agent Matrix conventions §5](../agent-matrix-conventions.md#5-profile-path-placeholders) - path placeholders usable inside a profile cell
- [Glossary](../glossary.md) - profile, profile matrix, profile cell
