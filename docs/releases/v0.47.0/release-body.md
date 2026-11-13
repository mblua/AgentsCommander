# Agents Commander v0.47.0

### Added

- Coding Agents and Profiles can run a **Preflight** command before the CLI starts: first the Coding Agent's command, then the effective Profile's. The CLI starts only if every configured command exits with code 0 within its timeout (default 60 s). Container agents with a saved preflight do not launch and show an error. (#3516, #3523)
- **Test preflights** in Settings runs the saved preflight chain for a stopped target without starting the CLI, with live progress and cancel. An empty chain shows "Nothing to test". (#3832, #3909)
- Context heat map: each agent's context usage is recorded locally per project and shown in a "Contexto por agente" view, a coloured sidebar badge and a card. The Root Agent is excluded. (#3532, #3921)
- Settings > **Context Heat Map** (desktop only) turns recording on, sets how many days to keep (1 to 365, default 30) and the session windows used for colours (default 25, 100 and 300 sessions). Recording is off by default, and the sidebar CTX button is hidden until it is on. (#4049, #4078, #4091, #4097)
- Screenshot capture works on Wayland-only Linux sessions (for example Omarchy / Hyprland) through the desktop portal. Settings > Hotkeys > **Add the screenshot shortcut to Hyprland for me** writes AgentsCommander's own key binding into the Hyprland config. (#2774, #4249)
- The titlebar always shows a camera chip. Clicking it takes a screenshot; it shows the key only while the shortcut is active. (#2774, #4249)

### Changed

- The context heat map groups history by session count instead of day, week and month buckets. The old `.ac/heatmap.json` file is ignored, not migrated or deleted. (#4049, #4078, #4091, #4097)
- Windows builds set the static VC runtime explicitly and no longer print the `STATIC_VCRUNTIME is deprecated` warning. Linking is unchanged. (#4047, #4057)

### Fixed

- On Linux the terminal uses a font that really renders monospace, instead of a proportional substitute with wide letter gaps. Windows is unchanged. (#4244, #4247)
- `ui-wait` keeps waiting through app startup instead of failing at once with `automation_not_enabled` or `automation_session_missing`. On timeout it prints the last diagnostic. (#4255, #4287)
- A container global sweep keeps its last real error when later attempts only hit the deadline. (#4151, #4156)

### Security

- The embedded web browser refuses to load the app's own origin, where app commands would bypass the remote access checks. External pages work as before. (#4289, #4322)
- Frontend sources are checked for raw HTML sinks, and branch-name validator logs strip line breaks. (#4300, #4333, #4171, #4173, #4182, #4183)

### Notes

- About 315 behavior-preserving refactor PRs resolve SonarCloud findings (nested ternaries, regex, accessibility labels, duplicated code), split complex functions and move code to single owners. (#4170, #4172, #4297, #4326, #4351, #4411)
- CI runs the frontend regression on every PR, skips npm install scripts in workflow installs, fails on stale complexity allowlist entries and enforces gate 04 on PR trees. Text files use LF through `.gitattributes`. (#4291, #4328, #2076, #2077, #4180, #4181, #4184, #4166, #4167, #4098, #4122, #4243, #4245)
- The testable build can be produced on Linux (`agentscommander_testeable.exe` copy of the Linux binary). (#4248, #4257)

## Included scope

- maintenance: ci: skip npm install scripts in regression-gate installs, Sonar G8 (closes #2076, #2077) ([#2077](https://github.com/mblua/AgentsCommander/issues/2077), [PR #4180](https://github.com/mblua/AgentsCommander/pull/4180))
- feature: feat(screenshot): Wayland/Hyprland capture and clickable camera chip (refs #2774) ([#2774](https://github.com/mblua/AgentsCommander/issues/2774), [PR #4249](https://github.com/mblua/AgentsCommander/pull/4249))
- feature: feat: local Coding Agent and Profile preflights (refs #3516) ([#3516](https://github.com/mblua/AgentsCommander/issues/3516), [PR #3523](https://github.com/mblua/AgentsCommander/pull/3523))
- feature: feat(context-history): context heatmap (refs #3532) ([#3532](https://github.com/mblua/AgentsCommander/issues/3532), [PR #3921](https://github.com/mblua/AgentsCommander/pull/3921))
- feature: feat(preflight): Test preflights UX (refs #3832) ([#3832](https://github.com/mblua/AgentsCommander/issues/3832), [PR #3909](https://github.com/mblua/AgentsCommander/pull/3909))
- maintenance: chore(build): set static VC runtime explicitly, drop deprecated STATIC_VCRUNTIME (refs #4047) ([#4047](https://github.com/mblua/AgentsCommander/issues/4047), [PR #4057](https://github.com/mblua/AgentsCommander/pull/4057))
- feature: feat(settings): context heat map toggle in Settings, hide CTX when off (refs #4049) ([#4049](https://github.com/mblua/AgentsCommander/issues/4049), [PR #4078](https://github.com/mblua/AgentsCommander/pull/4078))
- feature: feat(context-history): heat map by session count (#4091) ([#4091](https://github.com/mblua/AgentsCommander/issues/4091), [PR #4097](https://github.com/mblua/AgentsCommander/pull/4097))
- maintenance: ci: enforce gate 04 on PR trees with relocation-safe debt (#4098) ([#4098](https://github.com/mblua/AgentsCommander/issues/4098), [PR #4122](https://github.com/mblua/AgentsCommander/pull/4122))
- fix: fix(container): keep last real global-sweep error on deadline-only attempts (#4151) ([#4151](https://github.com/mblua/AgentsCommander/issues/4151), [PR #4156](https://github.com/mblua/AgentsCommander/pull/4156))
- maintenance: ci: fail the complexity gate on unused cognitive-complexity allowlist entries ([#4166](https://github.com/mblua/AgentsCommander/issues/4166), [PR #4167](https://github.com/mblua/AgentsCommander/pull/4167))
- maintenance: fix: resolve Sonar new-code reliability issues, lot L1 (sort compare and floating promises) ([#4170](https://github.com/mblua/AgentsCommander/issues/4170), [PR #4172](https://github.com/mblua/AgentsCommander/pull/4172))
- security: fix: resolve Sonar new-code issues, lot L2 (floating promises and log injection) ([#4171](https://github.com/mblua/AgentsCommander/issues/4171), [PR #4173](https://github.com/mblua/AgentsCommander/pull/4173))
- maintenance: ci: add --ignore-scripts to npm installs in bundle-validation and cache-warm (#4181) ([#4181](https://github.com/mblua/AgentsCommander/issues/4181), [PR #4184](https://github.com/mblua/AgentsCommander/pull/4184))
- security: fix(scripts): sanitize data.state in branch validator log (Sonar S5145) (#4182) ([#4182](https://github.com/mblua/AgentsCommander/issues/4182), [PR #4183](https://github.com/mblua/AgentsCommander/pull/4183))
- maintenance: chore(git): enforce LF for all text files via .gitattributes (refs #4243) ([#4243](https://github.com/mblua/AgentsCommander/issues/4243), [PR #4245](https://github.com/mblua/AgentsCommander/pull/4245))
- fix: fix(terminal): pick a font that really renders monospace on Linux (refs #4244) ([#4244](https://github.com/mblua/AgentsCommander/issues/4244), [PR #4247](https://github.com/mblua/AgentsCommander/pull/4247))
- maintenance: Testable mode on Linux (#4248) ([#4248](https://github.com/mblua/AgentsCommander/issues/4248), [PR #4257](https://github.com/mblua/AgentsCommander/pull/4257))
- fix: fix(ui-automation): ui-wait retries startup preflight errors (refs #4255) ([#4255](https://github.com/mblua/AgentsCommander/issues/4255), [PR #4287](https://github.com/mblua/AgentsCommander/pull/4287))
- security: fix(web-browser): refuse the app origin and prove IPC isolation (#4289) ([#4289](https://github.com/mblua/AgentsCommander/issues/4289), [PR #4322](https://github.com/mblua/AgentsCommander/pull/4322))
- maintenance: ci: run frontend regression on every PR validation (#4291) ([#4291](https://github.com/mblua/AgentsCommander/issues/4291), [PR #4328](https://github.com/mblua/AgentsCommander/pull/4328))
- maintenance: refactor: simplify sidebar nested conditionals (#4297 lot B) ([#4297](https://github.com/mblua/AgentsCommander/issues/4297), [PR #4323](https://github.com/mblua/AgentsCommander/pull/4323))
- security: test(security): guard raw HTML sinks in frontend sources (#4300) ([#4300](https://github.com/mblua/AgentsCommander/issues/4300), [PR #4333](https://github.com/mblua/AgentsCommander/pull/4333))
- maintenance: refactor(tests): group similar tests into parameterized tests (S5976) (#4326) ([#4326](https://github.com/mblua/AgentsCommander/issues/4326), [PR #4327](https://github.com/mblua/AgentsCommander/pull/4327))
- maintenance: refactor(ci): name positional parameters in release.yml shell helpers (#4351) ([#4351](https://github.com/mblua/AgentsCommander/issues/4351), [PR #4352](https://github.com/mblua/AgentsCommander/pull/4352))
- maintenance: refactor: Sonar L1 part A in sanitizing_containers and ipc (#4410) ([#4410](https://github.com/mblua/AgentsCommander/issues/4410), [PR #4411](https://github.com/mblua/AgentsCommander/pull/4411))

## Install from npm

```text
npx @mblua/agentscommander@0.47.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/4433
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.46.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
