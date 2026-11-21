# Agents Commander v0.48.0

### Added

- **Temporary Rooms**: a new sidebar category below the Root Agent creates disposable `temp-room-trNN` folders in the app config directory and runs a coding agent there (**Create new Temp Room**). Temporary Rooms are not restored when the app restarts. (#4779, #4782)
- A temp-room agent starts limited to its own folder, receives AgentsCommander CLI credentials, and can message verified room orchestrators and the Root Agent in both directions. `%AC_REPLICA_ROOT%` resolves to the temp room's folder; `%AC_WORKSPACE_ROOT%` and `%AC_MATRIX_ROOT%` fail with a clear error, and the panel warns about them before launch. (#4780, #4809, #4786, #4793)
- Temp-room agents support groups, Non-stop, favorites and **Edit TASK title** / **Clear task title**, like room replicas. Deleting or re-creating a `trNN` folder clears its group membership. (#4805, #4822)
- **Unleash** / **Leash** in the replica menu (above Close Session) and on Temporary Room rows lifts or restores the workspace restrictions of a local replica after a confirmation, and restarts the session fresh. Unleashed rows show a breathing red halo. The Root Agent never shows the item. The flag is removed when its room or replica is deleted. (#4752, #4761, #4762, #4763, #4764, #4765, #4768, #4772, #4826, #4828)
- Coding agents installed through mise are detected and skipped by the startup auto-update, which used to fail on every start. Settings > Auto-update shows "Managed by mise - update with `mise upgrade <command>`". (#4804, #4808)
- Testable Linux builds can save a PNG of the main window with `ui-backend --selector window.snapshot --value <path>`, without moving or focusing the window. (#4248, #4848)
- Sidebar menu controls have unique `data-ac-testid` values, keyed by path, for `ui-*` GUI tests. (#4825, #4829, #4827, #4836, #4841, #4847)

### Changed

- The sidebar has one regex filter. It sits in the group header stuck at the top of the sidebar ("Temporary Rooms" at scroll top, then the stuck project header) and filters temporary rooms and projects. (#4785, #4792, #4810, #4814)
- The **WWW** button is hidden unless `webBrowserEnabled` is `true` in settings.json (off by default). The `web-browser` CLI commands are unchanged. (#4751, #4753)
- The context-history card opens after the mouse stays 2 s on the `CTX %` badge (focus, click and touch still open it at once). All context-history text is now English. (#4796, #4797)
- A raised hand is lowered only by Enter in that session. Scrolling, typing or arrow keys no longer lower it. (#4778, #4781)
- npm 12 and newer skip this package's install script unless allowed. Install docs, the in-app upgrade command and the missing-executable hint now use `--allow-scripts=@mblua/agentscommander`. (#4837, #4840, #4842, #4844)
- Releases bundle `THIRD_PARTY_NOTICES.md` and `LICENSE`, generated from the Rust and npm dependency graphs. (#4491, #4528)
- Semver-compatible npm and Cargo dependency updates. Rust is pinned to 1.98.1 through `rust-toolchain.toml`. (#4769, #4773, #4473, #4530)

### Fixed

- On Linux only one GUI per executable name runs at a time, the same rule as on Windows. (#4788, #4791)
- On Linux, Spec Board accepts spec files inside a repo instead of rejecting them as "outside every valid repo root". (#4604, #4631)
- A render error shows a recovery card with **Try again** and **Reload** instead of a blank window. (#4461, #4479)
- Saving Settings no longer rolls back keys written elsewhere while the modal was open, such as the temp-room agent, sidebar width or window geometry. (#4783, #4789)
- A stuck wake no longer blocks message delivery to an agent forever. It ends with a logged, counted error, and each claimed delivery has a 300 s deadline. (#3263, #4816)
- A poisoned internal lock no longer cascades into panics. (#98, #4730)
- Container cleanup no longer waits 10 s and logs a false timeout when nothing is shutting down. (#4643, #4688)
- The sticky project header is opaque in the deep-space, arctic-ops, obsidian-mesh and neon-circuit sidebar styles. Hovering a sidebar TASK title shows the full title. (#4821, #4830, #4838, #4839)
- Session selection timing is logged off the async worker, and a create ticket is freed before the response, removing two delays and races. (#4813, #4815, #4807, #4812)

### Security

- Each window gets only the Tauri permissions it uses. The screenshot overlay keeps event IPC only. (#4330, #4469)
- External Telegram text, peer messages, handoff prompts and loop prompts pass the PTY text policy before delivery. CR, ESC and other forbidden controls are rejected, never rewritten. (#4309, #4522)
- Debug output redacts bot tokens, session tokens, API keys and other settings secrets. (#4475, #4487, #4637, #4660, #4638, #4745)
- The RTK hooks for Claude Code no longer auto-approve the commands they rewrite; Claude Code's normal permission flow decides. (#4583, #4586)

### Notes

- About 105 behavior-preserving refactor and test PRs. CI adds CodeQL, cargo deny, ESLint, Prettier, an npm audit gate, a PR net-line budget, per-test flaky rates and SHA-pinned actions; TypeScript enables `noImplicitOverride`, `noUnusedLocals` and `noUncheckedIndexedAccess`. (#4315, #4337, #4591, #4633, #4460, #4537, #4596, #4615, #4490, #4738, #4715, #4736, #4527, #4718, #4690, #4700, #4571, #4585, #4587, #4612, #4652, #4671)
- Security threat model, module map, frontend state map and SAFETY notes for every `unsafe` block. (#4493, #4509, #4526, #4532, #4580, #4594, #4653, #4662, #4743)

## Included scope

- fix: fix(#98): recover poisoned std locks and forbid panicking lock acquisition ([#98](https://github.com/mblua/AgentsCommander/issues/98), [PR #4730](https://github.com/mblua/AgentsCommander/pull/4730))
- fix: fix(mailbox): end stuck wakes instead of blocking delivery forever (refs #3263) ([#3263](https://github.com/mblua/AgentsCommander/issues/3263), [PR #4816](https://github.com/mblua/AgentsCommander/pull/4816))
- feature: feat(#4248): Linux window snapshot via ui-backend window.snapshot ([#4248](https://github.com/mblua/AgentsCommander/issues/4248), [PR #4848](https://github.com/mblua/AgentsCommander/pull/4848))
- security: fix: validate external PTY text before delivery (#4309) ([#4309](https://github.com/mblua/AgentsCommander/issues/4309), [PR #4522](https://github.com/mblua/AgentsCommander/pull/4522))
- maintenance: ci: run cargo deny on every PR (#4315) ([#4315](https://github.com/mblua/AgentsCommander/issues/4315), [PR #4337](https://github.com/mblua/AgentsCommander/pull/4337))
- security: fix(#4330): restrict Tauri capabilities per window ([#4330](https://github.com/mblua/AgentsCommander/issues/4330), [PR #4469](https://github.com/mblua/AgentsCommander/pull/4469))
- maintenance: ci: add ESLint first slice and PR lint job (#4460) ([#4460](https://github.com/mblua/AgentsCommander/issues/4460), [PR #4537](https://github.com/mblua/AgentsCommander/pull/4537))
- fix: fix: recover from window render errors (#4461) ([#4461](https://github.com/mblua/AgentsCommander/issues/4461), [PR #4479](https://github.com/mblua/AgentsCommander/pull/4479))
- maintenance: build: pin Rust 1.98.1 via rust-toolchain.toml (refs #4473) ([#4473](https://github.com/mblua/AgentsCommander/issues/4473), [PR #4530](https://github.com/mblua/AgentsCommander/pull/4530))
- security: fix(telegram): redact bot token in watcher Debug and match redact contexts in any case (#4475) ([#4475](https://github.com/mblua/AgentsCommander/issues/4475), [PR #4487](https://github.com/mblua/AgentsCommander/pull/4487))
- maintenance: ci: npm audit gate with a committed baseline (closes #4490) ([#4490](https://github.com/mblua/AgentsCommander/issues/4490), [PR #4738](https://github.com/mblua/AgentsCommander/pull/4738))
- maintenance: chore: generate and bundle third-party notices (#4491) ([#4491](https://github.com/mblua/AgentsCommander/issues/4491), [PR #4528](https://github.com/mblua/AgentsCommander/pull/4528))
- docs: docs(security): add evidence-backed threat model (#4493) ([#4493](https://github.com/mblua/AgentsCommander/issues/4493), [PR #4509](https://github.com/mblua/AgentsCommander/pull/4509))
- docs: docs(architecture): verify module map and record checked commit (#4526) ([#4526](https://github.com/mblua/AgentsCommander/issues/4526), [PR #4532](https://github.com/mblua/AgentsCommander/pull/4532))
- maintenance: feat(ci): publish per-test flaky rate and advisory flaky-test issues (closes #4527) ([#4527](https://github.com/mblua/AgentsCommander/issues/4527), [PR #4718](https://github.com/mblua/AgentsCommander/pull/4718))
- maintenance: chore(tsconfig): enable noImplicitOverride (#4571 P01) ([#4571](https://github.com/mblua/AgentsCommander/issues/4571), [PR #4585](https://github.com/mblua/AgentsCommander/pull/4585))
- docs: docs(state): map frontend state owners and guard the map (#4580 P01) ([#4580](https://github.com/mblua/AgentsCommander/issues/4580), [PR #4594](https://github.com/mblua/AgentsCommander/pull/4594))
- security: fix(rtk): Claude hooks rewrite without auto-approving (refs #4583) ([#4583](https://github.com/mblua/AgentsCommander/issues/4583), [PR #4586](https://github.com/mblua/AgentsCommander/pull/4586))
- maintenance: ci(codeql): add CodeQL security analysis for TypeScript and Rust (#4591) ([#4591](https://github.com/mblua/AgentsCommander/issues/4591), [PR #4633](https://github.com/mblua/AgentsCommander/pull/4633))
- maintenance: build(frontend): add pinned Prettier config and format scripts (#4596 P01) ([#4596](https://github.com/mblua/AgentsCommander/issues/4596), [PR #4615](https://github.com/mblua/AgentsCommander/pull/4615))
- fix: fix(spec_board): path_is_under uses the key separator on every OS (#4604) ([#4604](https://github.com/mblua/AgentsCommander/issues/4604), [PR #4631](https://github.com/mblua/AgentsCommander/pull/4631))
- security: fix(security): redact tokens and API keys in remaining Debug impls (#4637) ([#4637](https://github.com/mblua/AgentsCommander/issues/4637), [PR #4660](https://github.com/mblua/AgentsCommander/pull/4660))
- security: fix(security): redact secrets in AppSettings and LocalSettingsOverlay Debug (#4638) ([#4638](https://github.com/mblua/AgentsCommander/issues/4638), [PR #4745](https://github.com/mblua/AgentsCommander/pull/4745))
- fix: fix(pty): skip 10s cleanup wait when no shutdown producer exists (#4643) ([#4643](https://github.com/mblua/AgentsCommander/issues/4643), [PR #4688](https://github.com/mblua/AgentsCommander/pull/4688))
- docs: docs: add module docs to top-level Rust modules (refs #4653) ([#4653](https://github.com/mblua/AgentsCommander/issues/4653), [PR #4662](https://github.com/mblua/AgentsCommander/pull/4662))
- maintenance: ci: pin GitHub Actions to commit SHAs and block tag pins (#4690) ([#4690](https://github.com/mblua/AgentsCommander/issues/4690), [PR #4700](https://github.com/mblua/AgentsCommander/pull/4700))
- maintenance: ci: PR net-line budget check with written justification (closes #4715) ([#4715](https://github.com/mblua/AgentsCommander/issues/4715), [PR #4736](https://github.com/mblua/AgentsCommander/pull/4736))
- feature: feat: hide the sidebar WWW button behind webBrowserEnabled (#4751) ([#4751](https://github.com/mblua/AgentsCommander/issues/4751), [PR #4753](https://github.com/mblua/AgentsCommander/pull/4753))
- feature: feat(#4752): P01 move integrity rules into their own context block ([#4754](https://github.com/mblua/AgentsCommander/issues/4754), [PR #4762](https://github.com/mblua/AgentsCommander/pull/4762))
- feature: feat(#4752): P02 persist the Unleash flag and skip Git brakes for unleashed replicas ([#4755](https://github.com/mblua/AgentsCommander/issues/4755), [PR #4761](https://github.com/mblua/AgentsCommander/pull/4761))
- feature: feat(#4752): P03 omit workspace restrictions for unleashed replicas ([#4756](https://github.com/mblua/AgentsCommander/issues/4756), [PR #4764](https://github.com/mblua/AgentsCommander/pull/4764))
- feature: feat(#4752): P04 add the Unleash command and discovery field ([#4757](https://github.com/mblua/AgentsCommander/issues/4757), [PR #4765](https://github.com/mblua/AgentsCommander/pull/4765))
- feature: feat(#4752): P05 add the Unleash/Leash menu item and confirmation ([#4758](https://github.com/mblua/AgentsCommander/issues/4758), [PR #4768](https://github.com/mblua/AgentsCommander/pull/4768))
- feature: feat(#4752): P06 show a breathing red halo on unleashed tiles ([#4759](https://github.com/mblua/AgentsCommander/issues/4759), [PR #4772](https://github.com/mblua/AgentsCommander/pull/4772))
- feature: feat(#4752): P07 drop Unleash flags when a room or replica is deleted ([#4760](https://github.com/mblua/AgentsCommander/issues/4760), [PR #4763](https://github.com/mblua/AgentsCommander/pull/4763))
- maintenance: chore(deps): apply semver-compatible npm and Cargo updates (refs #4769) ([#4769](https://github.com/mblua/AgentsCommander/issues/4769), [PR #4773](https://github.com/mblua/AgentsCommander/pull/4773))
- fix: fix: persist user-input session state only on Enter (#4778) ([#4778](https://github.com/mblua/AgentsCommander/issues/4778), [PR #4781](https://github.com/mblua/AgentsCommander/pull/4781))
- feature: feat(temp-rooms): Temporary Rooms with a bare agent, stage 1 (refs #4779) ([#4779](https://github.com/mblua/AgentsCommander/issues/4779), [PR #4782](https://github.com/mblua/AgentsCommander/pull/4782))
- feature: feat: temporary rooms stage 2 — room-limited context, AC CLI and messaging, Unleash (refs #4780) ([#4780](https://github.com/mblua/AgentsCommander/issues/4780), [PR #4809](https://github.com/mblua/AgentsCommander/pull/4809))
- fix: fix(settings): keep outside-written keys on Settings save (refs #4783) ([#4783](https://github.com/mblua/AgentsCommander/issues/4783), [PR #4789](https://github.com/mblua/AgentsCommander/pull/4789))
- fix: fix(sidebar): one regex filter on the Temporary Rooms header (refs #4785) ([#4785](https://github.com/mblua/AgentsCommander/issues/4785), [PR #4792](https://github.com/mblua/AgentsCommander/pull/4792))
- fix: fix(temp-rooms): resolve %AC_REPLICA_ROOT% and warn on unavailable placeholders (refs #4786) ([#4786](https://github.com/mblua/AgentsCommander/issues/4786), [PR #4793](https://github.com/mblua/AgentsCommander/pull/4793))
- fix: fix: enforce Linux GUI single-instance per executable name (refs #4788) ([#4788](https://github.com/mblua/AgentsCommander/issues/4788), [PR #4791](https://github.com/mblua/AgentsCommander/pull/4791))
- fix: fix: open CTX history card after 2 s hover and translate context history UI to English (refs #4796) ([#4796](https://github.com/mblua/AgentsCommander/issues/4796), [PR #4797](https://github.com/mblua/AgentsCommander/pull/4797))
- feature: feat(agent-update): handle mise-managed coding agents (refs #4804) ([#4804](https://github.com/mblua/AgentsCommander/issues/4804), [PR #4808](https://github.com/mblua/AgentsCommander/pull/4808))
- feature: feat(temp-rooms): groups, Non-stop, favorites and TASK title for temp-room agents (refs #4805) ([#4805](https://github.com/mblua/AgentsCommander/issues/4805), [PR #4822](https://github.com/mblua/AgentsCommander/pull/4822))
- fix: fix(session): free create ticket before finalize response refs #4807 ([#4807](https://github.com/mblua/AgentsCommander/issues/4807), [PR #4812](https://github.com/mblua/AgentsCommander/pull/4812))
- feature: feat(sidebar): filter follows the stuck group header (refs #4810) ([#4810](https://github.com/mblua/AgentsCommander/issues/4810), [PR #4814](https://github.com/mblua/AgentsCommander/pull/4814))
- fix: fix(selection): record selection timing off the async worker (refs #4813) ([#4813](https://github.com/mblua/AgentsCommander/issues/4813), [PR #4815](https://github.com/mblua/AgentsCommander/pull/4815))
- fix: fix(sidebar): keep sticky project header opaque in 4 styles, refs #4821 ([#4821](https://github.com/mblua/AgentsCommander/issues/4821), [PR #4830](https://github.com/mblua/AgentsCommander/pull/4830))
- feature: feat: add data-ac-testid to sidebar menu controls (refs #4825) ([#4825](https://github.com/mblua/AgentsCommander/issues/4825), [PR #4829](https://github.com/mblua/AgentsCommander/pull/4829))
- fix: fix(sidebar): show unleash halo on Temporary Room rows (refs #4826) ([#4826](https://github.com/mblua/AgentsCommander/issues/4826), [PR #4828](https://github.com/mblua/AgentsCommander/pull/4828))
- fix: fix(sidebar): key replica menu and row testids by path (refs #4827) ([#4827](https://github.com/mblua/AgentsCommander/issues/4827), [PR #4836](https://github.com/mblua/AgentsCommander/pull/4836))
- fix: fix(npm): npm 12 allow-scripts opt-in for install, upgrade and npx (refs #4837) ([#4837](https://github.com/mblua/AgentsCommander/issues/4837), [PR #4840](https://github.com/mblua/AgentsCommander/pull/4840))
- fix: fix: show the full TASK title as the sidebar task-title tooltip (refs #4838) ([#4838](https://github.com/mblua/AgentsCommander/issues/4838), [PR #4839](https://github.com/mblua/AgentsCommander/pull/4839))
- fix: fix(sidebar): key row decoration and header blockedMenu testids by path (refs #4841) ([#4841](https://github.com/mblua/AgentsCommander/issues/4841), [PR #4847](https://github.com/mblua/AgentsCommander/pull/4847))
- maintenance: ci(npm): run runtime smoke on npm 11 and 12 with allowScripts opt-in (refs #4842) ([#4842](https://github.com/mblua/AgentsCommander/issues/4842), [PR #4844](https://github.com/mblua/AgentsCommander/pull/4844))

## Install from npm

```text
npx --allow-scripts=@mblua/agentscommander @mblua/agentscommander@0.48.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/4850
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.47.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
