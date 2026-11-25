# Agents Commander v0.49.0

### Added

- Settings > Coding Agents has **Export** and **Import** buttons. Export saves every coding agent and its A/B/C profiles to `ac-coding-agents-exports_YYYYMMDD_HHMMSS.json`; when the file contains environment variable values, a red sticky warning says so. Import adds the file's agents with new IDs (a duplicated label gets ` (N)`) to the tab draft, kept only when you press Save; an invalid file changes nothing. (#5071, #5129, #5130, #5131)
- On Linux, when sound alerts cannot play because the GStreamer element `autoaudiosink` (gst-plugins-good) is missing, the sidebar sound icon shows a warning badge; pressing it shows the install command for Arch, Debian/Ubuntu and Fedora. (#4843, #4853)
- Settings > General > System > Logging has **Record IPC black box (renderer freeze diagnostics)** (`ipcBlackBoxEnabled`, off by default). A change applies after a restart. (#1707, #5166)
- Hovering the **Preflight command** box lists `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%` and `%AC_MATRIX_ROOT%`, what each resolves to, and that other `%VAR%` markers block launch and `$VAR` is not expanded. (#5139, #5143)

### Changed

- A confirmed screenshot now copies the PNG image to the clipboard. The new setting **Paste image path instead of image** (`screenshotCopyPath`, off by default) keeps the old behavior of copying the file path. The PNG is always saved, and the toast reads `Screenshot saved: <path>`. (#5178, #5199)
- Session warning toasts name the agent and offer **See terminal** when its session is live. (#5146, #5150)
- `engines.node` is `>=22` for development and CI. (#4896, #4897)

### Fixed

- Quitting the app no longer hangs forever on the final session save. (#4929, #4934)
- The team idle clock ignores PTY resize repaints and the first repaint after a restore, so they no longer count as activity. (#2158, #4957)
- In the Open Agent dialog, choosing a repository now opens the agent picker. (#5103, #5213)
- Closing a session can recover a stuck response-close, but only when all close work is proven idle. (#3317, #4854)
- CLI outbox messages are written atomically, so a reader never sees a half-written message. (#5081, #5082)
- Select dropdowns are readable on Linux (WebKitGTK), and the Context History select has enough contrast in the light theme. (#5140, #5200)
- Sidebar, main window and resource monitor controls use native elements and correct roles for screen readers. (#4924, #4962, #5001, #4945, #5183, #5061, #5220)

### Security

- Rust dependency updates for RUSTSEC-2026-0592 (`url` 2.5.9) and RUSTSEC-2026-0594 (`cookie` 0.18.3). (#4863, #4864, #4987, #4988)

### Notes

- About 160 behavior-preserving refactor, test, documentation and CI PRs: issue references removed from code comments and test titles, module docs for every Rust file, reasons for every lint `allow`, `unwrap`/`expect` lints in production code, module-cycle cuts, Sonar findings, less PTY output copying, smaller `tokio` and `reqwest` feature sets, and fewer flaky tests. (#4860, #4868, #4878, #4900, #4949, #4965, #4981, #5181, #5182, #5188, #5198)

## Included scope

- maintenance: Sanitize: drop issue refs from Rust test comments, lote13+14 (#137) ([#137](https://github.com/mblua/AgentsCommander/issues/137), [PR #5047](https://github.com/mblua/AgentsCommander/pull/5047))
- fix: fix(selection): read shutdown deadlines from the Tokio clock (#1234) ([#1234](https://github.com/mblua/AgentsCommander/issues/1234), [PR #5217](https://github.com/mblua/AgentsCommander/pull/5217))
- feature: feat(settings): IPC black box toggle, off by default, applies on restart (refs #1707) ([#1707](https://github.com/mblua/AgentsCommander/issues/1707), [PR #5166](https://github.com/mblua/AgentsCommander/pull/5166))
- fix: fix(idle): team idle clock ignores PTY resize and restore repaint (closes #2158) ([#2158](https://github.com/mblua/AgentsCommander/issues/2158), [PR #4957](https://github.com/mblua/AgentsCommander/pull/4957))
- fix: fix(session): recover a quiescent stuck response-close Closing on explicit discard (refs #3317) ([#3317](https://github.com/mblua/AgentsCommander/issues/3317), [PR #4854](https://github.com/mblua/AgentsCommander/pull/4854))
- feature: feat: warn when Linux sound alerts are silent due to missing GStreamer sink (refs #4843) ([#4843](https://github.com/mblua/AgentsCommander/issues/4843), [PR #4853](https://github.com/mblua/AgentsCommander/pull/4853))
- docs: docs(readme): replace hero screenshot with presentation video (refs #4855) ([#4855](https://github.com/mblua/AgentsCommander/issues/4855), [PR #4856](https://github.com/mblua/AgentsCommander/pull/4856))
- maintenance: test(pty): wait for shutdown worker threads to end before reaping (#4857) ([#4857](https://github.com/mblua/AgentsCommander/issues/4857), [PR #5125](https://github.com/mblua/AgentsCommander/pull/5125))
- maintenance: refactor: split the Linux/macOS-only too_many_lines baseline functions (closes #4859) ([#4859](https://github.com/mblua/AgentsCommander/issues/4859), [PR #4860](https://github.com/mblua/AgentsCommander/pull/4860))
- docs: docs: module docs for the remaining top-level Rust files (closes #4861) ([#4861](https://github.com/mblua/AgentsCommander/issues/4861), [PR #4865](https://github.com/mblua/AgentsCommander/pull/4865))
- maintenance: refactor: add a reason to every allow(clippy::...) (refs #4862) ([#4862](https://github.com/mblua/AgentsCommander/issues/4862), [PR #4868](https://github.com/mblua/AgentsCommander/pull/4868))
- security: fix(deps): update url to 2.5.9 for RUSTSEC-2026-0592 (closes #4863) ([#4863](https://github.com/mblua/AgentsCommander/issues/4863), [PR #4864](https://github.com/mblua/AgentsCommander/pull/4864))
- maintenance: refactor: remove or justify allow(dead_code) in 30 files (closes #4866) ([#4866](https://github.com/mblua/AgentsCommander/issues/4866), [PR #4878](https://github.com/mblua/AgentsCommander/pull/4878))
- maintenance: chore(lint): deny todo, unimplemented and dbg_macro (closes #4867) ([#4867](https://github.com/mblua/AgentsCommander/issues/4867), [PR #4870](https://github.com/mblua/AgentsCommander/pull/4870))
- maintenance: refactor: remove 50 clippy allows that no longer fire (closes #4869) ([#4869](https://github.com/mblua/AgentsCommander/issues/4869), [PR #4900](https://github.com/mblua/AgentsCommander/pull/4900))
- maintenance: ci: pin Node 22 (engines + .nvmrc); confirm --locked and npm ci (closes #4896) ([#4896](https://github.com/mblua/AgentsCommander/issues/4896), [PR #4897](https://github.com/mblua/AgentsCommander/pull/4897))
- fix: fix: resolve 7 Sonar accessibility and nested-ternary findings in sidebar components (refs #4924) ([#4924](https://github.com/mblua/AgentsCommander/issues/4924), [PR #4962](https://github.com/mblua/AgentsCommander/pull/4962))
- fix: fix: bound final session persistence at exit (refs #4929) ([#4929](https://github.com/mblua/AgentsCommander/issues/4929), [PR #4934](https://github.com/mblua/AgentsCommander/pull/4934))
- maintenance: ci: restore the PR APT cache through one local composite action (P01, Refs #4944) ([#4944](https://github.com/mblua/AgentsCommander/issues/4944), [PR #5225](https://github.com/mblua/AgentsCommander/pull/5225))
- fix: fix(sidebar): resolve #4945 batch-2 Sonar accessibility findings ([#4945](https://github.com/mblua/AgentsCommander/issues/4945), [PR #5183](https://github.com/mblua/AgentsCommander/pull/5183))
- maintenance: refactor(pty): break pty::output module cycle (#4963) ([#4963](https://github.com/mblua/AgentsCommander/issues/4963), [PR #4965](https://github.com/mblua/AgentsCommander/pull/4965))
- maintenance: refactor(sanitize): cut session::context_alerts SCC via leaf model.rs (#4976) ([#4976](https://github.com/mblua/AgentsCommander/issues/4976), [PR #4981](https://github.com/mblua/AgentsCommander/pull/4981))
- security: fix(deps): bump cookie to 0.18.3 (RUSTSEC-2026-0594) ([#4987](https://github.com/mblua/AgentsCommander/issues/4987), [PR #4988](https://github.com/mblua/AgentsCommander/pull/4988))
- fix: fix(a11y): S6819 native elements in sidebar, main window and resource monitor (#5061) ([#5061](https://github.com/mblua/AgentsCommander/issues/5061), [PR #5220](https://github.com/mblua/AgentsCommander/pull/5220))
- feature: feat: coding agents export/import buttons (refs #5071) ([#5071](https://github.com/mblua/AgentsCommander/issues/5071), [PR #5129](https://github.com/mblua/AgentsCommander/pull/5129))
- fix: fix: write CLI outbox messages atomically (#5081) ([#5081](https://github.com/mblua/AgentsCommander/issues/5081), [PR #5082](https://github.com/mblua/AgentsCommander/pull/5082))
- fix: fix(sidebar): switch Open Agent from repo list to agent picker (Closes #5103) ([#5103](https://github.com/mblua/AgentsCommander/issues/5103), [PR #5213](https://github.com/mblua/AgentsCommander/pull/5213))
- fix: fix(settings): show export env-values warning toast in red (refs #5130) ([#5130](https://github.com/mblua/AgentsCommander/issues/5130), [PR #5131](https://github.com/mblua/AgentsCommander/pull/5131))
- feature: feat(settings): preflight command tooltip lists supported variables (refs #5139) ([#5139](https://github.com/mblua/AgentsCommander/issues/5139), [PR #5143](https://github.com/mblua/AgentsCommander/pull/5143))
- fix: fix(ui): readable select dropdowns on Linux WebKitGTK (refs #5140) ([#5140](https://github.com/mblua/AgentsCommander/issues/5140), [PR #5200](https://github.com/mblua/AgentsCommander/pull/5200))
- fix: fix(sidebar): name the agent in session warning toasts, refs #5146 ([#5146](https://github.com/mblua/AgentsCommander/issues/5146), [PR #5150](https://github.com/mblua/AgentsCommander/pull/5150))
- maintenance: perf(pty): pass PTY output to the fan-out as a byte slice (#5176) ([#5176](https://github.com/mblua/AgentsCommander/issues/5176), [PR #5181](https://github.com/mblua/AgentsCommander/pull/5181))
- feature: feat(screenshot): copy the captured image to the clipboard by default (refs #5178) ([#5178](https://github.com/mblua/AgentsCommander/issues/5178), [PR #5199](https://github.com/mblua/AgentsCommander/pull/5199))
- maintenance: perf(pty): scan for printable output without building a stripped copy (#5179) ([#5179](https://github.com/mblua/AgentsCommander/issues/5179), [PR #5182](https://github.com/mblua/AgentsCommander/pull/5182))
- maintenance: deps: minimal tokio features instead of full (#5187) ([#5187](https://github.com/mblua/AgentsCommander/issues/5187), [PR #5188](https://github.com/mblua/AgentsCommander/pull/5188))
- maintenance: deps: drop unused reqwest blocking feature (#5194) ([#5194](https://github.com/mblua/AgentsCommander/issues/5194), [PR #5198](https://github.com/mblua/AgentsCommander/pull/5198))
- docs: docs(readme): replace the 60-second presentation video with the 108-second cut (refs #5207) ([#5207](https://github.com/mblua/AgentsCommander/issues/5207), [PR #5208](https://github.com/mblua/AgentsCommander/pull/5208))
- docs: docs(readme): replace the presentation video with the cut that names Linux, Windows and macOS (refs #5222) ([#5222](https://github.com/mblua/AgentsCommander/issues/5222), [PR #5227](https://github.com/mblua/AgentsCommander/pull/5227))

## Install from npm

```text
npx --allow-scripts=@mblua/agentscommander @mblua/agentscommander@0.49.0
```

## Verification identity

- Release issue: https://github.com/mblua/AgentsCommander/issues/5242
- Reviewed evidence set: `bound by the exact release-authority-v1 review-set-sha256 field`
- Candidate assets: 17
- Predecessor: `v0.48.0`

The workflow must publish this body verbatim, make the GitHub Release public and immutable before npm, and attach provenance for the exact assets and package.
