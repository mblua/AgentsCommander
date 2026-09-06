# Agents Commander v0.30.4

### Added

- **New `self-handoff-and-restart` CLI subcommand.** An agent can hand off through `SELF-HANDOFF.md` and come back on a genuinely new process running the **same configured coding agent** with the **same profile letter**, instead of an in-process `/clear` or a borrowed `self-handoff-and-switch`. Available to Room replicas, origin Agent Matrix agents, and the Root Agent; it does not change the replica's Selection-UI coding-agent or profile assignment, and adds no message-format field. ([#1632](https://github.com/mblua/AgentsCommander/issues/1632))

### Changed

- **Workgroups are now Rooms.** AgentsCommander creates `room-<N>-<team>` directories instead of `wg-<N>-<team>`, and every surface a person or an agent reads calls the concept a Room. **Nothing on disk is renamed, moved, converted or deleted:** every existing `wg-*` directory is still discovered, listed, addressed, operated and deleted exactly as before, and its inter-agent message filenames keep their `wg<N>` short token. Room and legacy Workgroup slot numbers are independent, so a project that already holds `wg-1-<team>` gets `room-1-<team>` next. The CLI gains the canonical names `room`, `purge-room` and `--room`; `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that parse to the identical value and produce identical side effects, exit codes and output, and a later release will remove them. Persisted config keys, event names, IPC command names, refresh reason codes, the outbox `action` value and the `%WORKGROUP%` injected-message token are unchanged, so nothing in flight breaks. ([#1614](https://github.com/mblua/AgentsCommander/issues/1614))
- **Startup coding-agent updates are cancellable, and the update overlay is all English.** Every unfinished row of the startup update timeline carries its own `Cancel` control, and a `Cancel all` control stops the whole pass; a row that is still `Verifying...` counts as unfinished and keeps both. Cancelling stops the step that is running, terminates its process tree, waits for those processes to be gone and prevents the steps after it; it leaves rows that already finished exactly as they are, and it does not reverse an updater command that had already completed. The prompt, the timeline and the cancel controls now share the card, so you can cancel an update while a first-time question is still open: Enter on a focused cancel control cancels without answering, while every other Enter and every Escape still answers `No`. Finished rows now state what actually happened instead of only success or failure: `Ready - <old> -> <new>`, `<version> (Nothing to update)`, `Update completed - Version could not be verified`, `Failed - <reason>` or `Cancelled`, and a cancelled row counts as completed rather than failed and raises no failure notification. The Settings > Coding Agents **Status** column is unchanged: it still reports `Updating...`, `Updated`, `Update failed`, or `-` when this AC start recorded no result for that command. ([#1672](https://github.com/mblua/AgentsCommander/issues/1672))

## Included scope

- feature: refactor(1614): create rooms as room-N-<team> and rename Workgroup to Room in GUI, CLI and docs ([#1614](https://github.com/mblua/AgentsCommander/issues/1614), [PR #1633](https://github.com/mblua/AgentsCommander/pull/1633))
- feature: feat(cli): add self-handoff-and-restart, refs #1632 ([#1632](https://github.com/mblua/AgentsCommander/issues/1632), [PR #1655](https://github.com/mblua/AgentsCommander/pull/1655))
- feature: feat(wake): honor dispatch agent+profile at spawn with delivered receipt (#1639) ([#1639](https://github.com/mblua/AgentsCommander/issues/1639), [PR #1645](https://github.com/mblua/AgentsCommander/pull/1645))
- docs: docs: document --agent per wake, --profile, dispatch ranking (#1640) ([#1640](https://github.com/mblua/AgentsCommander/issues/1640), [PR #1660](https://github.com/mblua/AgentsCommander/pull/1660))
- feature: feat(list-peers-lean): expose effective agent and profile (#1641) ([#1641](https://github.com/mblua/AgentsCommander/issues/1641), [PR #1657](https://github.com/mblua/AgentsCommander/pull/1657))
- feature: feat(pty): menu guard backend core (refs #1646) ([#1647](https://github.com/mblua/AgentsCommander/issues/1647), [PR #1653](https://github.com/mblua/AgentsCommander/pull/1653))
- feature: feat(pty): menu guard queue, settle holds and injection freeze (refs #1646) ([#1648](https://github.com/mblua/AgentsCommander/issues/1648), [PR #1659](https://github.com/mblua/AgentsCommander/pull/1659))
- feature: feat(ui): menu guard notification and universal sidebar indicator (refs #1646) ([#1649](https://github.com/mblua/AgentsCommander/issues/1649), [PR #1664](https://github.com/mblua/AgentsCommander/pull/1664))
- fix: fix(pty): use current-thread watcher runtimes (refs #1651) ([#1651](https://github.com/mblua/AgentsCommander/issues/1651), [PR #1683](https://github.com/mblua/AgentsCommander/pull/1683))
- fix: fix(#1652): renderer IPC black box and in-flight registry (phase 2 of 2) ([#1652](https://github.com/mblua/AgentsCommander/issues/1652), [PR #1701](https://github.com/mblua/AgentsCommander/pull/1701))
- fix: fix(sidebar): remove duplicate tile controls (refs #1654) ([#1654](https://github.com/mblua/AgentsCommander/issues/1654), [PR #1670](https://github.com/mblua/AgentsCommander/pull/1670))
- docs: docs: install AgentsCommander through a trusted coding agent ([#1662](https://github.com/mblua/AgentsCommander/issues/1662), [PR #1675](https://github.com/mblua/AgentsCommander/pull/1675))
- docs: docs: replace invented Fusion team copy with parallel Rooms positioning (refs #1667) ([#1667](https://github.com/mblua/AgentsCommander/issues/1667), [PR #1725](https://github.com/mblua/AgentsCommander/pull/1725))
- maintenance: chore: regenerate module-arcs record after #1646 (refs #1668) ([#1668](https://github.com/mblua/AgentsCommander/issues/1668), [PR #1671](https://github.com/mblua/AgentsCommander/pull/1671))
- feature: feat(sidebar): move session actions into context menu (#1673) ([#1673](https://github.com/mblua/AgentsCommander/issues/1673), [PR #1684](https://github.com/mblua/AgentsCommander/pull/1684))
- feature: feat(settings): add a Use button to each Coding Agent row ([#1674](https://github.com/mblua/AgentsCommander/issues/1674), [PR #1688](https://github.com/mblua/AgentsCommander/pull/1688))
- feature: feat(#1682): show the last coding-agent message timestamp in the terminal status strip ([#1682](https://github.com/mblua/AgentsCommander/issues/1682), [PR #1716](https://github.com/mblua/AgentsCommander/pull/1716))
- feature: feat(#1690): authoritative Rust cancellation and transport parity ([#1690](https://github.com/mblua/AgentsCommander/issues/1690), [PR #1712](https://github.com/mblua/AgentsCommander/pull/1712))
- feature: feat(#1691): monotonic frontend cancellation state and truthful English outcomes ([#1691](https://github.com/mblua/AgentsCommander/issues/1691), [PR #1718](https://github.com/mblua/AgentsCommander/pull/1718))
- docs: docs(#1692): synchronize the agent auto-update guide and changelog ([#1692](https://github.com/mblua/AgentsCommander/issues/1692), [PR #1728](https://github.com/mblua/AgentsCommander/pull/1728))
- maintenance: ui(settings): clarify configuration rail tooltip ([#1698](https://github.com/mblua/AgentsCommander/issues/1698), [PR #1699](https://github.com/mblua/AgentsCommander/pull/1699))
- fix: fix(#1702): discover room-* clones in reclaim-build-artifacts ([#1702](https://github.com/mblua/AgentsCommander/issues/1702), [PR #1703](https://github.com/mblua/AgentsCommander/pull/1703))
- maintenance: ui(settings): label rail action See profiles ([#1705](https://github.com/mblua/AgentsCommander/issues/1705), [PR #1706](https://github.com/mblua/AgentsCommander/pull/1706))
- maintenance: #1708 - sidebar context menu icons and the Detach session rename ([#1708](https://github.com/mblua/AgentsCommander/issues/1708), [PR #1711](https://github.com/mblua/AgentsCommander/pull/1711))
- maintenance: Show the Ctrl+Shift+W shortcut on the close-session controls (#1723) ([#1723](https://github.com/mblua/AgentsCommander/issues/1723), [PR #1736](https://github.com/mblua/AgentsCommander/pull/1736))
- feature: feat(#1730): Telegram glyph, agent-name chip and the target chip order on the sidebar rows ([#1730](https://github.com/mblua/AgentsCommander/issues/1730), [PR #1739](https://github.com/mblua/AgentsCommander/pull/1739))
- maintenance: ui(sidebar): swap the Add to Group and Create new group icons (refs #1731) ([#1731](https://github.com/mblua/AgentsCommander/issues/1731), [PR #1740](https://github.com/mblua/AgentsCommander/pull/1740))
- feature: feat(config): .local alter-ego override layer for context templates and instance settings (refs #1737) ([#1737](https://github.com/mblua/AgentsCommander/issues/1737), [PR #1744](https://github.com/mblua/AgentsCommander/pull/1744))
- fix: fix(agent-version): spawn cmd.exe instead of powershell.exe in the ANSI-skip probe test, refs #1741 ([#1741](https://github.com/mblua/AgentsCommander/issues/1741), [PR #1747](https://github.com/mblua/AgentsCommander/pull/1747))
- maintenance: ci(1742): connect release preparation to the deployed pipeline ([#1742](https://github.com/mblua/AgentsCommander/issues/1742), [PR #1806](https://github.com/mblua/AgentsCommander/pull/1806))
- fix: fix(sidebar): render RUNNING chips after the repo chips (#1745) ([#1745](https://github.com/mblua/AgentsCommander/issues/1745), [PR #1746](https://github.com/mblua/AgentsCommander/pull/1746))
- feature: feat(#1752): add See terminal action to the blocked-menu toast ([#1752](https://github.com/mblua/AgentsCommander/issues/1752), [PR #1766](https://github.com/mblua/AgentsCommander/pull/1766))
- maintenance: #1753: persist the blocked-menu communication on real transitions ([#1753](https://github.com/mblua/AgentsCommander/issues/1753), [PR #1759](https://github.com/mblua/AgentsCommander/pull/1759))
- feature: feat(#1754): surface blocked terminals in list-peers and list-peers-lean ([#1754](https://github.com/mblua/AgentsCommander/issues/1754), [PR #1772](https://github.com/mblua/AgentsCommander/pull/1772))
- feature: feat(sidebar): permanent working tint on rows and rooms, refs #1755 ([#1755](https://github.com/mblua/AgentsCommander/issues/1755), [PR #1781](https://github.com/mblua/AgentsCommander/pull/1781))
- feature: feat(#1757): detect the Codex "Hooks need review" blocking menu ([#1757](https://github.com/mblua/AgentsCommander/issues/1757), [PR #1782](https://github.com/mblua/AgentsCommander/pull/1782))
- docs: docs(#1758): document the menu guard (blockingMenus / menuGuardEnabled) ([#1758](https://github.com/mblua/AgentsCommander/issues/1758), [PR #1800](https://github.com/mblua/AgentsCommander/pull/1800))
- fix: fix(#1760): make the global context template distribution-owned (epic #1748 phase 01) ([#1760](https://github.com/mblua/AgentsCommander/issues/1760), [PR #1789](https://github.com/mblua/AgentsCommander/pull/1789))
- fix: fix(#1761): the global state entry stops recording a version ([#1761](https://github.com/mblua/AgentsCommander/issues/1761), [PR #1798](https://github.com/mblua/AgentsCommander/pull/1798))
- maintenance: test(#1767): pin the blocked-menu CLEAR path against a reintroduced rollback ([#1767](https://github.com/mblua/AgentsCommander/issues/1767), [PR #1787](https://github.com/mblua/AgentsCommander/pull/1787))
- docs: docs(glossary): name the two repo kinds and add 13 missing core terms, refs #1769 ([#1769](https://github.com/mblua/AgentsCommander/issues/1769), [PR #1771](https://github.com/mblua/AgentsCommander/pull/1771))
- docs: docs: adopt the work repo / Agents config repo vocabulary across docs, refs #1770 ([#1770](https://github.com/mblua/AgentsCommander/issues/1770), [PR #1776](https://github.com/mblua/AgentsCommander/pull/1776))
- docs: docs: retire the stale "container agents cannot reach their repos" claims (refs #1775) ([#1775](https://github.com/mblua/AgentsCommander/issues/1775), [PR #1792](https://github.com/mblua/AgentsCommander/pull/1792))
- feature: feat(sidebar): tint a quick-access orchestrator row when its room is working, refs #1783 ([#1783](https://github.com/mblua/AgentsCommander/issues/1783), [PR #1788](https://github.com/mblua/AgentsCommander/pull/1788))
- fix: fix(#1784): complete the room rename in the live session context ([#1784](https://github.com/mblua/AgentsCommander/issues/1784), [PR #1794](https://github.com/mblua/AgentsCommander/pull/1794))
- fix: fix(sidebar): drop the literal "RUNNING" word from the running-peer chip (#1790) ([#1790](https://github.com/mblua/AgentsCommander/issues/1790), [PR #1799](https://github.com/mblua/AgentsCommander/pull/1799))

## Install from npm

```text
npx @mblua/agentscommander@0.30.4
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/1807
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.30.3`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
