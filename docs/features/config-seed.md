# Config seed

For developers who want each replica to start with a ready-made tool config (a `.claude` folder, for example) instead of an empty one. After this page you can point a coding agent at a template config folder and have AC copy it into every replica at spawn, with AC path tokens already substituted.

When AgentsCommander spawns a replica for a coding agent, **config seed** copies a template config folder you maintain into that replica, so the agent starts with the settings, hooks, or files you want. The copy is atomic and best-effort: it never blocks or fails a launch.

## What it does

For a coding agent with config seed active, at spawn AC:

1. Picks the first source that qualifies, checking the [seed tiers](#seed-tiers-where-the-config-comes-from) from highest precedence to lowest.
2. Copies it to `<replica root>/<dest>`, substituting AC path tokens in text files.
3. For a `.claude` destination only, re-applies the RTK PreToolUse hook to match your global `injectRtkHook` setting.

The destination is replaced atomically. AC renames any existing folder to a trash name first, then moves the new copy into place, so the destination is always either fully old or fully new, never half-written. If anything goes wrong, AC logs it and the spawn continues. A seed failure never stops your session from launching.

## Enabling it

Config seed is configured **per coding agent**, on the agent's entry in `settings.json` under `configSeed`:

```json
{
  "agents": [
    {
      "id": "claude",
      "label": "Claude Code",
      "command": "claude",
      "configSeed": {
        "enabled": true,
        "dest": ".claude"
      }
    }
  ]
}
```

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Whether seeding runs for this agent. |
| `dest` | string | `""` | Destination folder name under the replica root (for example `.claude`). |

Config seed is **active only when `enabled` is true and `dest` is non-empty**. Omit `configSeed` entirely (the default) and no seeding happens. `dest` is a single folder name, validated as a safe name with no path separators or traversal, both at save time and again at spawn.

## Seed tiers: where the config comes from

You never name a source folder directly. `<dest>` is exactly the **Config folder** value you set for the coding agent in **Settings -> Coding Agents** (the `configSeed.dest` field), for example `.claude`. From that single value AC derives a fixed list of candidate sources, the **seed tiers**, and uses the **first tier that qualifies**, highest precedence first:

| Rank | Tier | Folder |
|---|---|---|
| 1 | Workspace, profile-lettered | `<workspace>/default_profile_<letter><dest>` |
| 2 | Workspace, base | `<workspace>/default<dest>` |
| 3 | Matrix, profile-lettered | `<matrix>/default_profile_<letter><dest>` |
| 4 | Matrix, base | `<matrix>/default<dest>` |
| 5 | Factory default (AC catalog) | `<config_dir>/coding-agents/_seed/<dest>` |

**The first matching tier wins.** AC checks the tiers top to bottom and stops at the first one that qualifies; lower tiers are not consulted for that spawn.

The path roots:

- `<workspace>` is the project's `.ac` root.
- `<matrix>` is the agent's canonical `_agent_<name>` directory.
- `<config_dir>` is the per-binary config directory next to the AC executable (see [Portable instances](portable-instances.md#config-directory-rule)).
- `<letter>` is the session's resolved profile letter, lowercased, so profile `B` looks for `default_profile_b<dest>`. See [Coding Agent Profiles](coding-agent-profiles.md) for how the letter is resolved.

### The two kinds of tier

The five tiers split into two groups with **different ownership and different overwrite behavior**.

**Tiers 1-4 are your templates (user-owned).** These are workspace and matrix folders you create and maintain. When one qualifies (the folder exists and is readable) AC copies it over `<replica>/<dest>` **on every spawn**, atomically replacing whatever was there. Two rules order these four:

- **Workspace beats matrix.** Every workspace-level candidate outranks every matrix-level candidate.
- **Profile beats base.** Within a location, the folder named for the session's resolved profile letter outranks the plain `default<dest>` folder.

**Tier 5 is the AC factory default (catalog-owned).** This is the config folder AC ships for a built-in coding agent, at `<config_dir>/coding-agents/_seed/<dest>/`. It has the **lowest precedence** and behaves differently from tiers 1-4: it is **absent-only**. AC fills `<replica>/<dest>` from it **only when the destination does not already exist** (and the factory folder holds at least one file). It **never overwrites** an existing config folder, so a replica's accumulated config, credentials, and session state are safe. If the destination is already present, tier 5 is skipped and the spawn is byte-for-byte unchanged. Tier 5 is, in effect, a one-time bootstrap for a brand-new replica when you have supplied no template of your own.

> **The matrix's own `<dest>` is not a template.** A folder like `_agent_<name>/.claude` is the matrix agent's live config, not a seed source. AC deliberately never copies from it, so seeding cannot clobber the agent's real config. The matrix tiers use the `default<dest>` and `default_profile_<letter><dest>` naming instead.

### Example: Claude, `Config folder` = `.claude`, profile `A`

AC looks, in order, for:

1. `<workspace>/default_profile_a.claude`
2. `<workspace>/default.claude` - a template you own; if it exists it wins, and it is re-copied on every spawn.
3. `<matrix>/default_profile_a.claude`
4. `<matrix>/default.claude`
5. `<config_dir>/coding-agents/_seed/.claude` - the AC factory fallback, used **only** when none of the above exists **and** the replica has no `.claude` yet.

The first that qualifies is copied to `<replica>/.claude`. So `<workspace>/default.claude`, the user template, always wins ahead of the factory seed; `<config_dir>/coding-agents/_seed/.claude` is only the factory fallback that bootstraps a fresh replica.

## The factory default and the "Re-seed" button

Only the built-in **Claude**, **Codex**, and **OpenCode** agents ship a factory default (tier 5); other agents ship none, so for them tier 5 is simply absent. AC writes each shipped master into `<config_dir>/coding-agents/_seed/<dest>/` once, on first launch, then leaves it alone. The master is **yours to edit** afterward: change `_seed/.claude/settings.json` and every future absent-only bootstrap uses your edited copy.

**Settings -> Coding Agents** shows a **Re-seed default configuration** button on each built-in that ships a master. It restores that master to the version AC shipped:

- It first backs up your current master to `<dest>.bak-<timestamp>` (your edits are never lost), then atomically swaps AC's shipped default into place.
- It changes **only** the tier-5 master under `<config_dir>/coding-agents/_seed/`. It does **not** touch any running session, any replica's live `<dest>`, or your workspace/matrix templates (tiers 1-4).
- Because tier 5 is absent-only, re-seeding affects only future replicas that still have no `<dest>` and no higher-tier template; existing replicas keep their config.

Use it when you have edited a factory master and want AC's original default back.

## Token substitution

AC substitutes the three AC path tokens inside the **content** of seeded files, so a template can refer to its own replica, workspace, or matrix path and have it resolve correctly per replica:

- `%AC_REPLICA_ROOT%`
- `%AC_WORKSPACE_ROOT%`
- `%AC_MATRIX_ROOT%`

These are the same tokens documented in [Agent Matrix conventions, section 5](../agent-matrix-conventions.md#5-profile-path-placeholders). The substitution rules:

- Text files **5 MiB or smaller** are read and have the tokens substituted.
- Files **larger than 5 MiB** are streamed and copied verbatim, never read whole into memory.
- **Binary files** (a NUL byte in the first 8 KB) and non-UTF-8 files are copied verbatim.
- CRLF line endings and a UTF-8 byte-order mark survive the copy; only the known token substrings change.
- Symlinks and Windows junctions in the template are skipped.

## Verifying it worked

A successful seed logs one `info`-level line. With `logLevel` at `info` (the default) or lower, look in `<config-dir>/app.log` for a line like:

```text
[config-seed] seeded 'C:\tools\.ac\wg-1-team\__agent_claude\.claude' into replica from WorkspaceBase source 'C:\tools\.ac\default.claude'
```

The tier in the message (`WorkspaceProfile`, `WorkspaceBase`, `MatrixProfile`, `MatrixBase`, or `CatalogDefault` for the factory default) tells you which source won. If you see no `[config-seed]` line at all, the seed did not run; see [Troubleshooting](#troubleshooting). See [Log filtering](../reference/log-filtering.md#where-logs-go) for where `app.log` lives and how to raise the log level.

## What it does not do

Config seed copies the template and, for `.claude`, re-applies the RTK hook. That is all. In particular:

- It does **not** write any claude.md exclude settings. The exclude subsystem was removed in [#590](https://github.com/mblua/AgentsCommander/issues/590); there is nothing to configure and nothing to document here.
- For any destination other than `.claude`, there is **no** post-copy re-apply step. The folder is copied as-is, with token substitution, and nothing further is stamped.

The only post-seed step is the RTK PreToolUse hook, and only for a `.claude` destination: AC re-applies it to match your global `injectRtkHook` setting, adding it when injection is on and removing it when off. See [RTK integration](rtk-integration.md).

## Troubleshooting

**"Nothing got seeded."** Config seed is active only when `enabled` is true and `dest` is non-empty. Then a source must qualify: one of the four workspace/matrix template folders, or, for a built-in that ships one, the tier-5 factory default (used only when the replica has no `<dest>` yet). If nothing qualifies, AC has nothing to copy and skips silently, logging that it found no source at any tier. Check the spawn logs for `[config-seed]` lines (see [Log filtering](../reference/log-filtering.md)).

**"My agent's config is overwritten on every launch."** That is expected: the destination is replaced atomically on every spawn. If `dest` matches the coding agent's actual config directory, the seed overwrites that live config each time. AC logs a heuristic warning when `dest` lines up with, or exactly matches, the agent's configured config-dir env. Point `dest` at a directory you intend AC to own, or turn seeding off for that agent.

**"`invalid dest '...'` in the logs."** `dest` must be a plain, safe folder name with no path separators or traversal. Fix the value in `settings.json`.

## See also

- [Settings reference](../reference/settings.md#coding-agents) - the `configSeed` field on a coding agent
- [Coding Agent Profiles](coding-agent-profiles.md) - how the profile letter is resolved
- [Portable instances](portable-instances.md#config-directory-rule) - where `<config_dir>` lives
- [Agent Matrix conventions, section 5](../agent-matrix-conventions.md#5-profile-path-placeholders) - the three AC path tokens
- [RTK integration](rtk-integration.md) - the post-seed `.claude` hook
