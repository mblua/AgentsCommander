# Config seed

For developers who want each replica to start with a ready-made tool config (a `.claude` folder, for example) instead of an empty one. After this page you can point a coding agent at a template config folder and have AC copy it into every replica at spawn, with AC path tokens already substituted.

When AgentsCommander spawns a replica for a coding agent, **config seed** copies a template config folder you maintain into that replica, so the agent starts with the settings, hooks, or files you want. The copy is atomic and best-effort: it never blocks or fails a launch.

## What it does

For a coding agent with config seed active, at spawn AC:

1. Picks the highest-precedence template folder that exists (see [Where the template comes from](#where-the-template-comes-from)).
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

## Where the template comes from

You do not name a source folder. AC derives candidate template locations by convention and uses the first one that exists, in this order, highest precedence first:

| Rank | Location | Folder name |
|---|---|---|
| 1 | Workspace, profile-lettered | `<workspace>/default_profile_<letter><dest>` |
| 2 | Workspace, base | `<workspace>/default<dest>` |
| 3 | Matrix, profile-lettered | `<matrix>/default_profile_<letter><dest>` |
| 4 | Matrix, base | `<matrix>/default<dest>` |

Two rules decide the winner:

- **Workspace beats matrix.** Every workspace-level candidate outranks every matrix-level candidate.
- **Profile beats base.** Within a location, the folder named for the session's resolved profile letter outranks the plain `default<dest>` folder.

`<letter>` is the session's resolved profile letter, lowercased, so profile `B` looks for `default_profile_b.claude`. `<workspace>` is the project's `.ac` root, and `<matrix>` is the agent's canonical `_agent_<name>` directory. See [Coding Agent Profiles](coding-agent-profiles.md) for how the profile letter is resolved.

> **The matrix's own `<dest>` is not a template.** A folder like `_agent_<name>/.claude` is the matrix agent's live config, not a seed source. AC deliberately never copies from it, so seeding cannot clobber the agent's real config. The matrix tiers use the `default<dest>` and `default_profile_<letter><dest>` naming instead.

**Example.** For a `claude` agent with `dest = ".claude"` resolving profile `A`, AC looks, in order, for `<workspace>/default_profile_a.claude`, `<workspace>/default.claude`, `<matrix>/default_profile_a.claude`, then `<matrix>/default.claude`. The first that exists is copied to `<replica>/.claude`.

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

The tier in the message (`WorkspaceProfile`, `WorkspaceBase`, `MatrixProfile`, or `MatrixBase`) tells you which template won. If you see no `[config-seed]` line at all, the seed did not run; see [Troubleshooting](#troubleshooting). See [Log filtering](../reference/log-filtering.md#where-logs-go) for where `app.log` lives and how to raise the log level.

## What it does not do

Config seed copies the template and, for `.claude`, re-applies the RTK hook. That is all. In particular:

- It does **not** write any claude.md exclude settings. The exclude subsystem was removed in [#590](https://github.com/mblua/AgentsCommander/issues/590); there is nothing to configure and nothing to document here.
- For any destination other than `.claude`, there is **no** post-copy re-apply step. The folder is copied as-is, with token substitution, and nothing further is stamped.

The only post-seed step is the RTK PreToolUse hook, and only for a `.claude` destination: AC re-applies it to match your global `injectRtkHook` setting, adding it when injection is on and removing it when off. See [RTK integration](rtk-integration.md).

## Troubleshooting

**"Nothing got seeded."** Config seed is active only when `enabled` is true and `dest` is non-empty. Then at least one of the four convention folders must actually exist on disk. If none exists, AC has nothing to copy and skips silently. Check the spawn logs for `[config-seed]` lines (see [Log filtering](../reference/log-filtering.md)).

**"My agent's config is overwritten on every launch."** That is expected: the destination is replaced atomically on every spawn. If `dest` matches the coding agent's actual config directory, the seed overwrites that live config each time. AC logs a heuristic warning when `dest` lines up with, or exactly matches, the agent's configured config-dir env. Point `dest` at a directory you intend AC to own, or turn seeding off for that agent.

**"`invalid dest '...'` in the logs."** `dest` must be a plain, safe folder name with no path separators or traversal. Fix the value in `settings.json`.

## See also

- [Settings reference](../reference/settings.md#coding-agents) - the `configSeed` field on a coding agent
- [Agent Matrix conventions, section 5](../agent-matrix-conventions.md#5-profile-path-placeholders) - the three AC path tokens
- [Coding Agent Profiles](coding-agent-profiles.md) - how the profile letter is resolved
- [RTK integration](rtk-integration.md) - the post-seed `.claude` hook
