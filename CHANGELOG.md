# Changelog

All notable user-facing changes are tracked here and in [GitHub Releases](https://github.com/mblua/AgentsCommander/releases).

This file follows a lightweight [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape: one section per release in reverse-chronological order. Each entry groups changes under **Added / Changed / Removed / Fixed / Security** where useful.

## 0.8.43 - 2026-05-27

Public-push release: repo cleanup, documentation rewrite scaffolding, and factual corrections to public copy. See umbrella issue [#313](https://github.com/mblua/AgentsCommander/issues/313).

### Added

- `ROADMAP.md` at repo root: Shipped / Planned / Considering tracked publicly, with links to GitHub issues.
- `SECURITY.md` at repo root: vulnerability reporting policy + supported versions + 90-day coordinated disclosure.
- `CHANGELOG.md` at repo root: this file.
- `.github/ISSUE_TEMPLATE/`: `bug_report.yml`, `feature_request.yml`, and `config.yml` (directs Q&A to GitHub Discussions).
- `.github/PULL_REQUEST_TEMPLATE.md`: short checklist for contributors.

### Changed

- Documentation: `ROLE_AC_BUILDER.md` moved to `docs/agent-matrix-conventions.md`.
- `.gitignore`: added `_logbooks/`; replaced the hand-listed `.ac-new/wg-{1,2,3}-ac-devs/` entries with the single glob `.ac-new/wg-*/`.
- README factual fixes:
  - Supported coding agents corrected to **Claude Code · Codex · Gemini** (was: Claude Code · Codex · OpenCode; OpenCode is not yet supported, tracked in [#315](https://github.com/mblua/AgentsCommander/issues/315)).
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
