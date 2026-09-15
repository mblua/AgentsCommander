# Agents Commander v0.33.0

### Added

- **Coding-agent selections for replicas can be locked.** Assignments offer ordinary and assign-and-lock scopes with reviewed conflict decisions, independent lock removal, KEEP badges and an explicit Save default for future replicas. Replica creation preserves existing selections and applies Matrix defaults only when a replica config is first created. Selection ownership is retained through restart completion, and re-applying the same locked pair is an unchanged write. ([#1939](https://github.com/mblua/AgentsCommander/issues/1939), [#1940](https://github.com/mblua/AgentsCommander/issues/1940), [#1941](https://github.com/mblua/AgentsCommander/issues/1941), [#1942](https://github.com/mblua/AgentsCommander/issues/1942), [#1943](https://github.com/mblua/AgentsCommander/issues/1943), [#2010](https://github.com/mblua/AgentsCommander/issues/2010))
- **The managed coding-agent catalog is persisted with local overrides.** User edits in `agents.local.json` win and survive restart; the catalog is seeded on first run even when no project is registered, and legacy, edited, foreign or corrupt files are never rewritten. ([#1968](https://github.com/mblua/AgentsCommander/issues/1968), [#1969](https://github.com/mblua/AgentsCommander/issues/1969), [#2021](https://github.com/mblua/AgentsCommander/issues/2021))
- **Grok Build (`grok`) is available in the coding-agent catalog.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))

### Changed

- **The Coding Agent assignment modal has a new layout**: three columns, a selection lock bar, Matrix default and Apply to controls, and profile comparison rows that show the launch line. ([#2014](https://github.com/mblua/AgentsCommander/issues/2014))
- **Suffixed executables refuse to start when their adjacent `.agentscommander_<suffix>` configuration directory is not writable**, before any durable write and without a HOME fallback. Unsuffixed executables keep `HOME/.agentscommander`, and explicit overrides are retained. ([#1935](https://github.com/mblua/AgentsCommander/issues/1935))
- **Muse is disabled in the coding-agent catalog.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))
- **The npm package README recommends global installation.** ([#1955](https://github.com/mblua/AgentsCommander/issues/1955))

### Removed

- **The legacy pre-v2 `codingAgentProfiles` settings migration was removed.** It ran on every load and dropped profile labels. ([#2018](https://github.com/mblua/AgentsCommander/issues/2018))

### Fixed

- **The npm launcher starts the app on macOS** by resolving the executable inside the installed `.app` bundle. ([#2016](https://github.com/mblua/AgentsCommander/issues/2016))
- **Profile labels survive restart.** ([#2018](https://github.com/mblua/AgentsCommander/issues/2018))
- **Current Codex final answers are forwarded to Telegram.** ([#1997](https://github.com/mblua/AgentsCommander/issues/1997))
- **Injected messages are submitted in Hermes, OpenCode and Grok.** ([#1999](https://github.com/mblua/AgentsCommander/issues/1999))
- **npm release verification installs the Linux runtime before running the CLI.** ([#1988](https://github.com/mblua/AgentsCommander/issues/1988))
- Several intermittent test fixtures are now deterministic. ([#1466](https://github.com/mblua/AgentsCommander/issues/1466), [#1984](https://github.com/mblua/AgentsCommander/issues/1984), [#1996](https://github.com/mblua/AgentsCommander/issues/1996), [#1998](https://github.com/mblua/AgentsCommander/issues/1998))

## Included scope

- maintenance: fix(#1466): make context-alert retry ordering test deterministic ([#1466](https://github.com/mblua/AgentsCommander/issues/1466), [PR #1993](https://github.com/mblua/AgentsCommander/pull/1993))
- fix: fix(#1935): refuse suffixed executables without a writable adjacent config dir ([#1935](https://github.com/mblua/AgentsCommander/issues/1935), [PR #2020](https://github.com/mblua/AgentsCommander/pull/2020))
- feature: feat(config): preserve replica selections and creation defaults (refs #1939) ([#1939](https://github.com/mblua/AgentsCommander/issues/1939), [PR #1959](https://github.com/mblua/AgentsCommander/pull/1959))
- feature: feat(1940): retain selection ownership through restart completion ([#1940](https://github.com/mblua/AgentsCommander/issues/1940), [PR #1990](https://github.com/mblua/AgentsCommander/pull/1990))
- feature: feat(1941): expose scoped selection locks and defaults ([#1941](https://github.com/mblua/AgentsCommander/issues/1941), [PR #1992](https://github.com/mblua/AgentsCommander/pull/1992))
- feature: feat(1942): add typed selection-lock client and completion handling ([#1942](https://github.com/mblua/AgentsCommander/issues/1942), [PR #1994](https://github.com/mblua/AgentsCommander/pull/1994))
- feature: feat(1943): add selection locks, KEEP badges and explicit defaults ([#1943](https://github.com/mblua/AgentsCommander/issues/1943), [PR #2004](https://github.com/mblua/AgentsCommander/pull/2004))
- docs: docs: highlight global npm installation (refs #1955) ([#1955](https://github.com/mblua/AgentsCommander/issues/1955), [PR #1956](https://github.com/mblua/AgentsCommander/pull/1956))
- feature: feat(1968): persist managed coding-agent catalog ([#1968](https://github.com/mblua/AgentsCommander/issues/1968), [PR #1991](https://github.com/mblua/AgentsCommander/pull/1991))
- docs: docs: document managed coding-agent catalog (refs #1969) ([#1969](https://github.com/mblua/AgentsCommander/issues/1969), [PR #2008](https://github.com/mblua/AgentsCommander/pull/2008))
- maintenance: test: make session NotFound retry fixture deterministic (#1984) ([#1984](https://github.com/mblua/AgentsCommander/issues/1984), [PR #2005](https://github.com/mblua/AgentsCommander/pull/2005))
- fix: fix(1988): install Linux runtime before npm release verification ([#1988](https://github.com/mblua/AgentsCommander/issues/1988), [PR #1989](https://github.com/mblua/AgentsCommander/pull/1989))
- maintenance: test: stabilize capped shutdown ownership fixture (#1996) ([#1996](https://github.com/mblua/AgentsCommander/issues/1996), [PR #2000](https://github.com/mblua/AgentsCommander/pull/2000))
- fix: fix: forward current Codex final answers to Telegram, refs #1997 ([#1997](https://github.com/mblua/AgentsCommander/issues/1997), [PR #2007](https://github.com/mblua/AgentsCommander/pull/2007))
- maintenance: fix: synchronize critical-key cleanup assertions (refs #1998) ([#1998](https://github.com/mblua/AgentsCommander/issues/1998), [PR #2006](https://github.com/mblua/AgentsCommander/pull/2006))
- fix: fix(coding-agents): submit Hermes/OpenCode/Grok and disable Muse (#1999) ([#1999](https://github.com/mblua/AgentsCommander/issues/1999), [PR #2003](https://github.com/mblua/AgentsCommander/pull/2003))
- fix: fix(#2010): keep the same locked pair as an unchanged selection write ([#2010](https://github.com/mblua/AgentsCommander/issues/2010), [PR #2012](https://github.com/mblua/AgentsCommander/pull/2012))
- feature: feat(#2014): apply prototype v4 to the Coding Agent modal ([#2014](https://github.com/mblua/AgentsCommander/issues/2014), [PR #2027](https://github.com/mblua/AgentsCommander/pull/2027))
- fix: fix(npm): resolve the macOS .app bundle executable in the launcher (refs #2016) ([#2016](https://github.com/mblua/AgentsCommander/issues/2016), [PR #2025](https://github.com/mblua/AgentsCommander/pull/2025))
- fix: fix(2018): drop legacy codingAgentProfiles migration so profile labels survive restart ([#2018](https://github.com/mblua/AgentsCommander/issues/2018), [PR #2026](https://github.com/mblua/AgentsCommander/pull/2026))
- fix: fix(#2021): seed the instance coding-agent catalog when no project is registered ([#2021](https://github.com/mblua/AgentsCommander/issues/2021), [PR #2022](https://github.com/mblua/AgentsCommander/pull/2022))

## Install from npm

```text
npx @mblua/agentscommander@0.33.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/2033
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.32.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
