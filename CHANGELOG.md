# Changelog

All notable user-facing changes are tracked here and in [GitHub Releases](https://github.com/mblua/AgentsCommander/releases).

This file follows a lightweight [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape: one section per release in reverse-chronological order. Each entry groups changes under **Added / Changed / Removed / Fixed / Security** where useful.

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
