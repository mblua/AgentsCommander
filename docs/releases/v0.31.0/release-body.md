# Agents Commander v0.31.0

### Changed

- **Workgroups are now Rooms.** AgentsCommander creates `room-<N>-<team>` directories instead of `wg-<N>-<team>`, and every surface a person or an agent reads calls the concept a Room. **Nothing on disk is renamed, moved, converted or deleted:** every existing `wg-*` directory is still discovered, listed, addressed, operated and deleted exactly as before, and its inter-agent message filenames keep their `wg<N>` short token. Room and legacy Workgroup slot numbers are independent, so a project that already holds `wg-1-<team>` gets `room-1-<team>` next. The CLI gains the canonical names `room`, `purge-room` and `--room`; `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that parse to the identical value and produce identical side effects, exit codes and output, and a later release will remove them. Persisted config keys, event names, IPC command names, refresh reason codes, the outbox `action` value and the `%WORKGROUP%` injected-message token are unchanged, so nothing in flight breaks. ([#1614](https://github.com/mblua/AgentsCommander/issues/1614))

## Included scope

- feature: Rename Workgroups to Rooms while preserving legacy behavior ([#1614](https://github.com/mblua/AgentsCommander/issues/1614), [PR #1633](https://github.com/mblua/AgentsCommander/pull/1633))
- fix: Preserve release-path compatibility required by the Room change ([#1625](https://github.com/mblua/AgentsCommander/issues/1625), [PR #1631](https://github.com/mblua/AgentsCommander/pull/1631))
- fix: Include the protected-main release correction ([#1627](https://github.com/mblua/AgentsCommander/issues/1627), [PR #1627](https://github.com/mblua/AgentsCommander/pull/1627))

## Install from npm

```text
npx @mblua/agentscommander@0.31.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/1621
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.30.3`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
