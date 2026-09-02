# Changelog

All notable user-facing changes are tracked here and in [GitHub Releases](https://github.com/mblua/AgentsCommander/releases).

This file follows a lightweight [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape: one section per release in reverse-chronological order. Each entry groups changes under **Added / Changed / Removed / Fixed / Security** where useful.

## Unreleased

### Added

- **New `self-handoff-and-restart` CLI subcommand.** An agent can hand off through `SELF-HANDOFF.md` and come back on a genuinely new process running the **same configured coding agent** with the **same profile letter**, instead of an in-process `/clear` or a borrowed `self-handoff-and-switch`. Available to Room replicas, origin Agent Matrix agents, and the Root Agent; it does not change the replica's Selection-UI coding-agent or profile assignment, and adds no message-format field. ([#1632](https://github.com/mblua/AgentsCommander/issues/1632))

### Changed

- **Workgroups are now Rooms.** AgentsCommander creates `room-<N>-<team>` directories instead of `wg-<N>-<team>`, and every surface a person or an agent reads calls the concept a Room. **Nothing on disk is renamed, moved, converted or deleted:** every existing `wg-*` directory is still discovered, listed, addressed, operated and deleted exactly as before, and its inter-agent message filenames keep their `wg<N>` short token. Room and legacy Workgroup slot numbers are independent, so a project that already holds `wg-1-<team>` gets `room-1-<team>` next. The CLI gains the canonical names `room`, `purge-room` and `--room`; `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that parse to the identical value and produce identical side effects, exit codes and output, and a later release will remove them. Persisted config keys, event names, IPC command names, refresh reason codes, the outbox `action` value and the `%WORKGROUP%` injected-message token are unchanged, so nothing in flight breaks. ([#1614](https://github.com/mblua/AgentsCommander/issues/1614))
- **Startup coding-agent updates are cancellable, and the update overlay is all English.** Every unfinished row of the startup update timeline carries its own `Cancel` control, and a `Cancel all` control stops the whole pass; a row that is still `Verifying...` counts as unfinished and keeps both. Cancelling stops the step that is running, terminates its process tree, waits for those processes to be gone and prevents the steps after it; it leaves rows that already finished exactly as they are, and it does not reverse an updater command that had already completed. The prompt, the timeline and the cancel controls now share the card, so you can cancel an update while a first-time question is still open: Enter on a focused cancel control cancels without answering, while every other Enter and every Escape still answers `No`. Finished rows now state what actually happened instead of only success or failure: `Ready - <old> -> <new>`, `<version> (Nothing to update)`, `Update completed - Version could not be verified`, `Failed - <reason>` or `Cancelled`, and a cancelled row counts as completed rather than failed and raises no failure notification. The Settings > Coding Agents **Status** column is unchanged and still reports only `Updating...`, `Updated` or `Update failed`. ([#1672](https://github.com/mblua/AgentsCommander/issues/1672))

## 0.30.3

### Changed

- **Releases now publish to npm via OIDC Trusted Publishing.** `publish-npm` pins npm to 11.6.2, because OIDC trusted publishing requires npm >= 11.5.1 and the Node 22 runner ships 10.9.8. The `NPM_AGENTSCOMMANDER` token is retained as a fallback for this release only; npm prefers OIDC and falls back to a token, so the migration carries no risk to the release itself. No application change: this version is functionally identical to 0.30.2. ([#1563](https://github.com/mblua/AgentsCommander/issues/1563))

## 0.30.2

### Added

- **Web server: the bind address is now editable from the titlebar popover.** The PORT row grew into a BIND section (ADDR + PORT). The address chooser offers `Localhost only (127.0.0.1)`, `All interfaces (0.0.0.0)` (which discloses that any device on your network can reach the server), every detected IPv4 with its adapter name (virtual and tunnel adapters grouped and collapsed), and a validated manual entry. A stored address that is no longer on the machine stays visible as a disabled `Unavailable` row. Applying an address restarts a running server, starts a stopped-but-enabled one, and only saves otherwise. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))

### Changed

- **The context-alert message injected into a coordinator's terminal is now operator-editable**, and its visible prefix changes from `[AgentsCommander context alert]` to `[AC context alert]`. The wording lives in `injected-messages.toml`, next to the executable in the config directory, alongside a read-only `injected-messages.default.toml` reference. Markdown is preserved byte for byte, the placeholders are `%MEMBER%`, `%WORKGROUP%`, `%THRESHOLDS%` and the optional `%OBSERVED%`, and an entry you have edited is never overwritten by an upgrade. `injected-messages reseed --id <id>` (or `--all`) restores a shipped default, taking a timestamped backup first. ([#1157](https://github.com/mblua/AgentsCommander/issues/1157))

### Fixed

- **`close-session` no longer reports a false timeout on graceful closes.** The CLI's delivery-confirmation wait was hardcoded to 30s while a graceful close takes ~30s per session, so 97.9% of graceful closes exited 1 as "delivery confirmation timeout" despite succeeding moments later. The CLI now runs a single wait for the daemon's response (still fast-failing on rejection), budgets `--timeout` + 60 seconds (default 90s, matching `send --confirm-timeout`), and when the wait expires it exits 2 ("outcome unknown": the close keeps running server-side) instead of a fabricated exit 1. Timeout messages now print both UUIDs with correct labels (`request` vs `message`). ([#1440](https://github.com/mblua/AgentsCommander/issues/1440))
- **Web server bind failures are no longer invisible.** The status payload now carries the failed address, port and verbatim OS error; the popover explains the failure in plain language (`Stopped · bind failed`, amber dot) instead of a bare `Stopped`. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))
- **The titlebar web server toggle now follows runtime state.** A failed bind no longer shows a contradictory `Stop Server`, and starting the server only persists the "enable web server" setting once the server has actually started, so a failed attempt no longer turns it on for every future launch. The `Enable web server` checkbox in Settings remains the way to change that setting directly. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))
- **Settings no longer reports a web server that failed to start as `Running`.** The Start button in Settings now reflects the actual result of the start attempt. ([#1453](https://github.com/mblua/AgentsCommander/issues/1453))

## 0.30.1

### Changed

- Advanced all desktop, Cargo, root/npm wrapper, lockfile, Tauri, and installer Release references to 0.30.1.
- Routed v0.30.1 through the repository's enabled immutable-Release path.
- Prepared `@mblua/agentscommander@0.30.1` for a separate protected OIDC publication after immutable Release verification.

## 0.30.0

### Added

- Activated v1 seed-manifest emission for project, team, and workgroup flows ([#1109](https://github.com/mblua/AgentsCommander/pull/1109)).
- Added per-agent activity intervals and application lifecycle records in `activity.jsonl` ([#1152](https://github.com/mblua/AgentsCommander/pull/1152)).
- Seeded a per-instance `.gitignore` for generated instance files ([#1165](https://github.com/mblua/AgentsCommander/pull/1165)).
- Added configurable regex watchers over PTY output with an activity window ([#1174](https://github.com/mblua/AgentsCommander/pull/1174)).
- Added rotation of an origin Agent Matrix `memory/` directory when a fresh session spawns ([#1181](https://github.com/mblua/AgentsCommander/pull/1181)).
- Added a user-editable registry for PTY-injected message templates ([#1203](https://github.com/mblua/AgentsCommander/pull/1203)).
- Added a cross-platform checker for `SKILL.md` structure ([#1222](https://github.com/mblua/AgentsCommander/pull/1222)).
- Added authorized terminal snapshots in JSON and PNG through the API and CLI ([#1238](https://github.com/mblua/AgentsCommander/pull/1238)).
- Made the Non-stop pseudo-group favoritable from the groups rail ([#1260](https://github.com/mblua/AgentsCommander/pull/1260)).
- Displayed the active screenshot shortcut in the sidebar ([#1275](https://github.com/mblua/AgentsCommander/pull/1275)).
- Added a setting to disable `activity.jsonl` generation, with generation off by default ([#1300](https://github.com/mblua/AgentsCommander/pull/1300)).
- Added a warning when Default Shell is not configured as a complete executable path ([#1314](https://github.com/mblua/AgentsCommander/pull/1314)).
- Added an authenticated API endpoint for native-window screenshots ([#1316](https://github.com/mblua/AgentsCommander/pull/1316)).
- Added ordered per-agent update commands and the startup auto-update flow ([#1324](https://github.com/mblua/AgentsCommander/pull/1324), [#1326](https://github.com/mblua/AgentsCommander/pull/1326), [#1328](https://github.com/mblua/AgentsCommander/pull/1328)).
- Added `window-list` and `window-screenshot` CLI verbs for native window capture ([#1333](https://github.com/mblua/AgentsCommander/pull/1333)).
- Added a per-agent auto-update dropdown to Coding Agent profiles ([#1345](https://github.com/mblua/AgentsCommander/pull/1345)).
- Added a dedicated collapsible Coordinator Quick-Access section ([#1354](https://github.com/mblua/AgentsCommander/pull/1354)).
- Added shell-specific RTK hooks, including PowerShell tool coverage ([#1426](https://github.com/mblua/AgentsCommander/pull/1426)).
- Added a statusline branch to Claude context suggestions ([#1435](https://github.com/mblua/AgentsCommander/pull/1435)).
- Added native-tool usage records to the RTK savings database ([#1465](https://github.com/mblua/AgentsCommander/pull/1465)).
- Added an artifact registry as the source for per-instance `.gitignore` generation ([#1470](https://github.com/mblua/AgentsCommander/pull/1470)).
- Exposed the web server bind address and startup failures in the UI ([#1475](https://github.com/mblua/AgentsCommander/pull/1475)).

### Changed

- Unified coding-agent badge styling across sidebar rows ([#1168](https://github.com/mblua/AgentsCommander/pull/1168)).
- Removed the unused phone feature and unreachable `sync_workgroup_repos` command ([#1201](https://github.com/mblua/AgentsCommander/pull/1201)).
- Renamed watcher toolbar controls to match the product vocabulary ([#1207](https://github.com/mblua/AgentsCommander/pull/1207)).
- Moved the activity log checkbox below the Log level hint in settings ([#1308](https://github.com/mblua/AgentsCommander/pull/1308)).
- Bounded terminal output admission to prevent renderer saturation ([#1312](https://github.com/mblua/AgentsCommander/pull/1312)).
- Moved the coding-agent catalog into project `.ac` data and recorded per-agent auto-update metadata ([#1322](https://github.com/mblua/AgentsCommander/pull/1322)).
- Added the `{{AGENT_REPOS}}` and `# Agent Repos` context vocabulary while retaining the frozen alias ([#1430](https://github.com/mblua/AgentsCommander/pull/1430)).

### Fixed

- Made Coding Agents row clicks honor 1-rail mode ([#1099](https://github.com/mblua/AgentsCommander/pull/1099)).
- Prevented phantom resource-monitor cap exhaustion on non-Windows platforms ([#1145](https://github.com/mblua/AgentsCommander/pull/1145)).
- Gated the file-in-use classifier by platform ([#1150](https://github.com/mblua/AgentsCommander/pull/1150)).
- Recovered orphaned quarantined Windows resource groups ([#1159](https://github.com/mblua/AgentsCommander/pull/1159)).
- Kept the watcher activity polling chain running after refreshes ([#1212](https://github.com/mblua/AgentsCommander/pull/1212)).
- Bounded the watcher mount chain and armed polling unconditionally ([#1226](https://github.com/mblua/AgentsCommander/pull/1226)).
- Accepted the coordinator entry in `team_members` instead of rejecting the team configuration ([#1247](https://github.com/mblua/AgentsCommander/pull/1247)).
- Excluded Alert me sessions from the Ungrouped sidebar group ([#1278](https://github.com/mblua/AgentsCommander/pull/1278)).
- Matched the Codex profile to the context-first Coding Agent row layout ([#1288](https://github.com/mblua/AgentsCommander/pull/1288)).
- Stopped the orphan-session warning loop in session persistence ([#1296](https://github.com/mblua/AgentsCommander/pull/1296)).
- Moved Git status polling off the async runtime and deduplicated requests ([#1303](https://github.com/mblua/AgentsCommander/pull/1303)).
- Launched Windows agent commands through the configured Default Shell ([#1311](https://github.com/mblua/AgentsCommander/pull/1311)).
- Limited automatic updates to registered coding agents ([#1336](https://github.com/mblua/AgentsCommander/pull/1336)).
- Moved startup restore work off the main thread so the auto-update prompt can render ([#1344](https://github.com/mblua/AgentsCommander/pull/1344)).
- Replayed bounded PTY output history when terminal views are rebuilt ([#1376](https://github.com/mblua/AgentsCommander/pull/1376)).
- Required rendered content before a cold-spawn wake is considered settled ([#1390](https://github.com/mblua/AgentsCommander/pull/1390)).
- Registered the screenshot global hotkey at the start of setup to avoid the startup race ([#1402](https://github.com/mblua/AgentsCommander/pull/1402)).
- Retained diagnostics when saving settings fails ([#1400](https://github.com/mblua/AgentsCommander/pull/1400)).
- Restored PTY broadcast push delivery ([#1432](https://github.com/mblua/AgentsCommander/pull/1432)).
- Made `close-session` wait once for the daemon response ([#1447](https://github.com/mblua/AgentsCommander/pull/1447)).
- Reconciled the screen-parser grid at attach and healed the embedded viewport ([#1450](https://github.com/mblua/AgentsCommander/pull/1450)).
- Made kill verification corpse-aware so terminated processes reach the Terminated state ([#1449](https://github.com/mblua/AgentsCommander/pull/1449)).
- Guaranteed that the PTY attach seed starts on a line boundary ([#1464](https://github.com/mblua/AgentsCommander/pull/1464)).
- Ignored RTK runtime artifacts under Agent Matrix directories ([#1473](https://github.com/mblua/AgentsCommander/pull/1473)).
- Sequenced local TASK writes against session snapshots by owning workgroup ([#1477](https://github.com/mblua/AgentsCommander/pull/1477)).
- Used loopback URLs when opening a browser for wildcard web-server binds ([#1485](https://github.com/mblua/AgentsCommander/pull/1485)).
- Logged a warning when seed rendering panics during terminal-output activation ([#1460](https://github.com/mblua/AgentsCommander/pull/1460)).
- Stopped replica config seeding from persisting `replica_config_file` rows in `seed-manifest.toml`, while retaining support for reading and later pruning legacy rows ([#1487](https://github.com/mblua/AgentsCommander/pull/1487)).

### Security

- Warned in settings that API keys and bot tokens are stored in plaintext ([#1353](https://github.com/mblua/AgentsCommander/pull/1353)).

## 0.20.0 – 2026-07-23

Large release covering everything merged since `0.10.0` (616 commits across ~110 PRs). Headlines: **containerized coding agents** (Docker / "Camino 2" backend), an **in-daemon Control Plane API**, a **coding-agent catalog overhaul** (Hermes / Cursor CLI / Pi), **live context-usage visibility** (CTX badges + alerts), **workgroup UI Groups** (Telegram-style sidebar rail), a **single self-contained web-server executable**, plus a deep PTY/terminal reliability and settings-persistence hardening pass.

### Added

- **Containerized coding agents (Docker / "Camino 2" backend)**: run coding agents inside Docker containers. New PTY session-backend refactor, async spawn routing, container transport backend + session-transport endpoint, a `session-bridge` crate + Docker runtime, and a DB-backed `MessageStore` + dispatcher. Adds an `ac-claude-ready` prebuilt image, a UI+CLI container runtime selector per agent, read-write mounting of enabled repos into container sessions, and container auth via copied host credentials. ([#819](https://github.com/mblua/AgentsCommander/issues/819): #823, #826, #829, #832, #834; #865, #868, #930, #935)
- **In-daemon Control Plane API server**: an HTTP control-plane API hosted in the daemon (starting with `send` + `list-peers-lean`), an enable/disable toggle, a bind/port editor, an in-app API-client mint command with Settings UI, and backend hardening. ([#791](https://github.com/mblua/AgentsCommander/issues/791), #838, #846, #853, #872)
- **Coding-agent catalog overhaul**: removed the Gemini CLI preset; added **Hermes**, **Cursor CLI**, and **Pi**; externalized the catalog to a seeded, editable backend JSON (2 phases); and added scriptable coding-agent config management via CLI. ([#766](https://github.com/mblua/AgentsCommander/issues/766), #769, #786)
- **Pi Coding Agent support**: auto-resume, use as a self-handoff-and-switch source, logical-clear → `/new` mapping, and a suggested context-badge pattern. (#1069, #1081, #1059, #1054)
- **Live context-usage visibility (CTX)**: a per-session context-usage scrape off the vt100 mirror, a sidebar **CTX badge** with a configurable Settings pattern, team context-usage alerts, and per-peer CTX percent exposed in `list-peers` / `list-peers-lean`. (#1032, #1033, #1056, #1088)
- **Workgroup UI Groups**: a Telegram-style sidebar rail that filters projects into groups, with reorder, auto-focus, edit-in-context-menu, live web↔desktop sync, and a raise-hand indicator on the group tab. ([#737](https://github.com/mblua/AgentsCommander/issues/737), #808, #810, #851, #822, #763)
- **Project seed manifest & config seeding**: a staged project seed-manifest system (core, plumbing, lifecycle-removal outcomes, conformance/scale harness), context-publication outcomes, `%USER_HOME%` expansion in seeded content, and plaintext env values (masking only `PASSWORD*` keys). (#1038, #1060, #1062, #1063, #1064, #1061, #924, #1052)
- **Single self-contained web-server executable**: the frontend `dist` is embedded into the binary. (#796)
- **Sidebar & workflow tools**: sidebar rail favorites + collapsible categories, archive/unarchive projects, a coordinator repo "Browse Main / Browse Branch" submenu, a delete-agent context-menu action, a reusable CodingAgentQuickConfiguration modal, a titlebar zoom (%) stepper, a browser web-server titlebar menu, and a Sidebar left/right option. (#965, #881, #943, #843, #975, #863, #835, #840)
- **Active-agent screenshot capture.** (#714)
- **"Non-stop" / "Alert me!" watchdog group** with Telegram / sound alerts on a working-vs-total disparity. (#777, #799)
- **CLI**: `purge-wg` one-shot workgroup purge; coordinator-scoped exact PTY text injection (including Root→coordinator); `send --confirm-timeout` with the default raised to 90s. (#885, #1057, #782)
- **Persisted raise-hand indicator** across app restarts, plus raise-hand group rules. (#747, #775)

### Changed

- **Agent template / context-lifecycle minimization** ([#1005](https://github.com/mblua/AgentsCommander/issues/1005) S1–S6): trimmed the messaging, GOLDEN RULE / write-restriction, skills-intro, and root/coordinator templates; added coordinator self-clear, durable fresh-conversation intent, and a settled live-wake path before injection.
- **Agent boundary hardening**: restrict agent reads to allowed zones (GOLDEN RULE), move the cross-workgroup boundary into the coordinator context, stop the Root Agent from consuming the global context template, and protect user-set task titles. (#923, #1030, #979, #738)
- **Coding-agent UX**: rail selection by row click, a 1/2-rail toggle for the Coding Agents screen, profile cards expanded by default, last Coding Agent + Profile shown on powered-off tiles, and onboarding that fits all options without scrolling. (#895, #1095, #790, #733, #768)
- **Removed the RTK (Rust Token Killer) integration.** (#928)
- **Docs**: document the factory-default seed tier, expose code-signing and privacy links on releases, and correct the Windows signing status. (#876, #754, #719)
- **Repo hygiene**: stop tracking `_logbooks/`, gitignore/untrack `plans/` and remove `_prototypes/`, purge explanatory comments from 56 TS/TSX files, and add a workgroup build-artifact reclaim script. (#990, #1048, #1046, #932)

### Fixed

- **Terminal reliability (the black/blank terminal)**: never gate live PTY output behind the snapshot round-trip; spawn the PTY at real size and never resize an unrendered child; PTY spawn diagnostics; cancel-safe local PTY spawn; and PTY spawn offloaded off the async runtime. (#955, #973, #942, #847, #839)
- **Settings persistence**: disk-authoritative project paths (stop the GUI clobbering CLI writes), a unique per-writer temp filename, keeping project lists disk-authoritative to stop silent session deletion, and atomic + retried git-guard writes. (#778, #774, #888, #836)
- **Container transport**: surface real startup failures, require an explicit image, redact secrets from diagnostics, translate host↔container env paths, guard the bind mount, strip the Windows verbatim prefix from mount sources, stop the stray Docker console window on Windows startup, Camino 2 hardening, and WSS TLS. (#892, #894, #993, #992, #831, #1017)
- **Sidebar / UI**: fixed first-click loss from sidebar DOM recreation, keep sidebar modals open across refresh, reset scroll to the selected project, refresh the agent list on partial-delete failure, align context-menu glyphs, close the replica context menu on pointer leave, highlight only the active project's group, align project-header search controls, keep the group rail drag alive after pointer-capture loss, and correct APPLY-TO scope counters. (#748, #710, #941, #856, #987, #977, #860, #816, #815, #800)
- **Titlebar zoom**: handle fire-and-forget zoom-apply rejections and harden against a leaked `initZoom` document listener. (#1083, #1093)
- **API server**: fix the `0.0.0.0`-bind status false-negative + readiness await, and use distinct newtypes for Web/Api handles (startup panic). (#878, #794)
- **Coordinator / CLI robustness**: prevent auto-close terminal hijack, preserve Restart-Session fresh intent against non-substantive PTY writes, ignore the team-config coordination lock, compact Git scope warnings, persist absolute + instance-relative project paths, hide the TASK section for the Root Agent, read the on-disk `--get-output` response before checking timeout, normalize Windows verbatim cwd, add a Telegram auto-close exemption, and skip the create-gate for the restart replacement create. (#1027, #871, #1070, #1072, #1077, #771, #729, #730, #817, #1101)
- **Web/desktop parity**: route web coding-agent profile commands and sync project group changes live between web and desktop. (#859, #822)
- **Repaired the agency-agents-roles skill** the indexer was silently dropping. (#909)
- **Hardened the send/messaging slice** and extracted a shared WG-replica walk-up helper. (#724, #726)
- **CI**: always report the lockfile-drift check and make test-debt comment-masking string-aware. (#1022, #801)

### Security

- **Restrict agent reads to allowed zones (GOLDEN RULE enforcement).** (#923)
- **Redact secrets from container transport diagnostics.** (#904)
- **Decouple domain logic from UI presentation values** to reduce accidental exposure surface. (#882)

## 0.9.0 – 2026-06-15

Feature release: **Project Loops** (scheduled, recurring agent runs), a public marketing-copy overhaul around the "compound your coding agents" message, OpenCode documented as usable (provider-agnostic), and a large test/CI regression-hardening pass.

### Added

- **Project Loops**: scheduled, recurring agent runs. Define a cron-style schedule that re-triggers a coordinator, workgroup, or agent; ships the scheduler backend, the sidebar UI, and scheduler safety guards. ([#354](https://github.com/mblua/AgentsCommander/issues/354))
- **Agency template install in the New Agent modal**: browse and install [@msitarzewski/agency-agents](https://github.com/msitarzewski/agency-agents) role templates directly from the create-agent flow, wired over the embedded WebSocket. ([#465](https://github.com/mblua/AgentsCommander/issues/465))
- **Semantic GUI automation bridge**: a CLI and UI context-click automation surface for deterministic, scriptable GUI interactions. ([#499](https://github.com/mblua/AgentsCommander/issues/499))
- **External link confirmation for terminal links**: clicking an external link in an xterm session now asks for confirmation before opening, in both desktop and browser modes. ([#474](https://github.com/mblua/AgentsCommander/issues/474))
- **Deterministic GUI test mode**: a fake-transport mode that makes browser and GUI flows reproducible in CI.

### Changed

- **Public marketing copy overhaul**: README and docs now lead with the "compound your coding agents" message: bring any coding agent at full power, put a Fusion team on a Loop, and let scheduled runs compound toward the best answer. Positioned around "only adds, never subtracts." ([#520](https://github.com/mblua/AgentsCommander/issues/520))
- **OpenCode documented as supported (provider-agnostic)**: OpenCode can be run today via the custom coding-agent path and pointed at any provider or model. A first-class tuned profile (its own `CodingAgentKind`, resume tokens, idle tuning) remains planned. ([#315](https://github.com/mblua/AgentsCommander/issues/315))
- **Profile modal layout**: enlarged desktop layout and fixed intermediate stacking. ([#471](https://github.com/mblua/AgentsCommander/issues/471))
- **Test and CI regression-hardening pass**: PR regression gates, GUI regression suites, PTY lifecycle coverage, mailbox wake-routing tests, close-session integration fixtures, CLI behavior contract tests, and a Windows release CLI smoke test. ([#475](https://github.com/mblua/AgentsCommander/issues/475), [#479](https://github.com/mblua/AgentsCommander/issues/479), [#485](https://github.com/mblua/AgentsCommander/issues/485))

### Fixed

- **Coordinator repo badges for discovered workgroups**: workgroups discovered on disk now show their repo badge correctly. ([#500](https://github.com/mblua/AgentsCommander/issues/500))
- **Outbound network port exhaustion**: hardened outbound network resource handling and fixed bridge shutdown and poll backoff so repeated outbound requests no longer exhaust ports. ([#501](https://github.com/mblua/AgentsCommander/issues/501), [#502](https://github.com/mblua/AgentsCommander/issues/502))
- **Frontend websocket rejections**: fixed spurious rejection of valid frontend WebSocket connections. ([#480](https://github.com/mblua/AgentsCommander/issues/480), [#491](https://github.com/mblua/AgentsCommander/issues/491))
- **Project Loops scheduler races**: scheduler safety hardening, stale-prompt pre-write race, stale-delivery revalidation, enable and disable UI refresh, and delete-modal concurrency.

### Security

- **Hardened destructive filesystem delete paths**: tightened guards around destructive delete operations and closed review gaps found during the hardening pass. ([#512](https://github.com/mblua/AgentsCommander/issues/512))

## 0.8.43 — 2026-05-27

Public-push release: repo cleanup, documentation rewrite scaffolding, and factual corrections to public copy. See umbrella issue [#313](https://github.com/mblua/AgentsCommander/issues/313).

### Added

- `ROADMAP.md` at repo root — Shipped / Planned / Considering tracked publicly, with links to GitHub issues.
- `SECURITY.md` at repo root — vulnerability reporting policy + supported versions + 90-day coordinated disclosure.
- `CHANGELOG.md` at repo root — this file.
- `.github/ISSUE_TEMPLATE/` — `bug_report.yml`, `feature_request.yml`, and `config.yml` (directs Q&A to GitHub Discussions).
- `.github/PULL_REQUEST_TEMPLATE.md` — short checklist for contributors.

### Changed

- Documentation: `ROLE_AC_BUILDER.md` moved to `docs/agent-matrix-conventions.md`.
- `.gitignore`: added `_logbooks/`; replaced hand-listed workgroup entries with a single workspace workgroup glob.
- README factual fixes:
  - Supported coding agents corrected to **Claude Code · Codex · Gemini** (was: Claude Code · Codex · OpenCode — OpenCode is not yet supported; tracked in [#315](https://github.com/mblua/AgentsCommander/issues/315)).
  - Window-model description updated to reflect the unified main window (the old "Sidebar and Terminal are independent windows" description was pre-unification).
  - Settings tab name "Dark Factory" referenced as **"Teams"** in public copy. Internal code rename is tracked in [#314](https://github.com/mblua/AgentsCommander/issues/314).
  - Release-tag example updated from the stale `v0.4.9` to the current `0.8.x` line.

### Removed

- Obsolete artifact files at repo root: `DIAG-telegram-emission.md`, `FIXES_CODEX.md`, `PLAN-telegram-bridge.md`, `agentscommander-prompt.md`, and the `_test_dark_factory/` stub directory.
- Spanish-language and obsolete plan files from `docs/`: `Descripcion.md`, `home-es.md`, `PLAN_dark_factory.md`, `PLAN_OrganigramaDF.md`, `PROMPT_Etapa2_OrganigramaDF.md`.

### Notes

- Full README rewrite, new docs prose (quickstart, concepts, comparison, troubleshooting, faq, glossary, style-guide, use-cases, integrations, agents, features, reference), Acknowledgments section, and visual assets ship in follow-up commits on the same `chore/313-public-push` branch.

## Earlier releases

For all releases before `0.8.43`, see the auto-generated changelog on the [GitHub Releases](https://github.com/mblua/AgentsCommander/releases) page.
