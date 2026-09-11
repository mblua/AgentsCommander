# Epic #1925: Remote blocking-menu patterns, a fetched `remote` precedence layer

Author: ac-architect-v4, room-5, 2026-09-10 UTC. `code-implementation-workflow` Full path, round 3 candidate. Round 1 (`Plan-SHA256: 1E05B6DFC871FD0ADB8A55C9D898C24899DD869DB0684FE0666605363633E308`) and round 2 (`Plan-SHA256: 4F92477E7065F19DB52ED65B6F5BD81F63B2C04C3E35A61CC84583D55AD48DD2`) were rejected; sections 14 and 15 map each finding to its fix.
Status: READY_FOR_IMPLEMENTATION
PARTITION: 6 phases

- Issue: https://github.com/mblua/AgentsCommander/issues/1925 (OPEN). Score comment: https://github.com/mblua/AgentsCommander/issues/1925#issuecomment-5626282269 (score 50, raised to Full; vetoes: verification 13, blast radius 15, state 12, environment 9).
- Repository: `D:\0_repos\AgentsCommander_iac\.ac\room-5-ac-dev-team-v4\repo-AgentsCommander`
- Planning checkout: `feature/1925-remote-blocking-menu-patterns` at `main` = `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`, shallow clone. It is used for planning only and is never delivered.
- Drift baseline: `b4f7c9de7f15b75f8f12c034c8356f73babd0a8d`. Every line number in this plan is pinned to it. It is not any phase's branch point.
- Delivery shape: after consensus `ac-tech-lead-v4` creates one child issue per phase. Each phase is implemented at its own Step 8 on `feature/<child-issue>-<slug>`, cut from the then-current `origin/main`, and lands by its own PR in dependency order, depending only on landed phases. `ac-tech-lead-v4` opens and lands every PR and collects the exact-head CI evidence. `ac-shipper-v4` builds once, from `origin/main`, after phase 6 lands; it does not push and owns no PR.

## 1. Objective

New blocking-menu patterns reach installed clients without a release. At startup, AC downloads one published file from `main`, validates it whole, caches it, and applies it at the next start as a new layer between the user's `.local` file and the shipped patterns. A settings flag, on by default and exposed as a checkbox, turns the download off. A CI-checked inventory records every path that released binaries fetch.

## 2. Task class and threat model

- Class: routine application change with security-relevant validation of remote input. No release, signing, packaging, dependency or migration change.
- Accepted threat model, from the issue: a GitHub or repo-write compromise can publish any bytes to the served path. The validator bounds what that gains to "some sessions falsely report blocked, with bounded text". No signature check (explicitly out). The local user can edit the cache, so it is re-validated at every read; the local account is otherwise trusted.
- Hazards the checks answer, verified at the drift baseline: a blocked session refuses injection (`src-tauri/src/pty/inject.rs:359-365`, `ERR_MENU_GUARD_DEFERRED`), so a catch-all pattern silently stops message delivery; `notification` text reaches persisted session state through `set_blocked_menu_and_persist` (`src-tauri/src/config/sessions_persistence.rs:2243`) and `list-peers` output. `regex` is the backtracking-free crate (`Cargo.lock`: `regex` 1.12.3), so match time is not the hazard.
- Memory bound: each accepted pattern compiles within `size_limit(256 * 1024)` and the menu guard keeps one compiled copy per pattern, so the remote layer adds at most 200 x 256 KiB = 50 MiB of compiled programs, plus each regex's lazy-DFA cache at the `regex` crate default. Only a repo-write attacker reaches that ceiling; the real file compiles to a few KiB.
- Enhanced controls: none applicable. No release or signing, no untrusted build host, no destructive migration, no custom execution infrastructure.

## 3. Evidence (read at the drift baseline)

- Graph index: `index_status` is `ready` but indexed 2026-09-09T15:32:29Z, and `check_index_coverage` reports `metadata_changed` for all 10 cited source files, so source was read directly.
- `src-tauri/src/config/settings.rs`: `npm_update_notifications_enabled` `:621-624`, default `:1018`; `EMBEDDED_BLOCKING_MENUS_JSON` `:1048`; `BLOCKING_MENUS_SCHEMA_VERSION` `:1051`; `parse_blocking_menus_file` `:1084-1099`; `shipped_blocking_menus` `:1102`; `default_blocking_menus_for_command` `:1113`; `load_local_blocking_menus_file` `:1165-1182`; `BlockingMenusStore` `:1184-1243` (`resolve` `:1217`, `resolve_for` `:1237`); private `settings_path` `:2393`; `SettingsState = Arc<RwLock<AppSettings>>` `:5408`; S6 golden fixture `EXPECTED_NON_PROJECT_SETTINGS_JSON` `:10244-10392` (key slot after `"raiseTerminalOnClick": true,` `:10341`), normalizer `:10394-10417`, test `a_no_overlay_save_writes_the_control_captured_on_the_pinned_base` `:10421`; tests `mod blocking_menus_1905` `:11461`.
- `src-tauri/src/update_check.rs`, a `pub mod` (`lib.rs:22`): `should_check` `:84-92`; `run_startup_check` `:186-253`, reading its flag from `SettingsState` at `:188-192`; `fetch_latest` `:261-307`, taking `OutboundNetwork` from app state at `:262`; tests `:355-490` (`mod tests {` `:356`, closing `}` `:490`, the last line).
- `src-tauri/src/lib.rs`: store built once at `:2969-2971`; update-check spawn block `:3064-3072`.
- `src-tauri/src/network/mod.rs`: `#[cfg(test)] new_for_tests` `:35`, `acquire` `:53`, `general` `:84`, `acquired_labels_for_tests` `:94`.
- Mock-app harness: tauri `test` feature in dev-dependencies (`src-tauri/Cargo.toml:78`); `settings_app` builds `tauri::test::mock_builder()` with a managed `SettingsState` (`src-tauri/src/commands/pty.rs:1091-1097`) and is used inside `#[tokio::test]` (`:1255-1257`).
- `src-tauri/src/pty/menu_guard/mod.rs`: `store` `:52`; `entries_for` `:80`; `Regex::new` `:92`; `evaluate_logical_rows` `:105`.
- `src-tauri/src/config/instance_artifacts.rs`: rows byte-sorted by name; `CODEX_HOME_DIR_NAME` = `"codex-home"` `:123`; `BLOCKING_MENUS_LOCAL_FILE_NAME` `:157`, row `:431-436`. `instance_gitignore.rs` fixture neighbours `"app.log.5",` `:999` and `"codex-home/agent-1/config.toml",` `:1000`, and `"settings-blocking-menus.local.json",` `:1029`.
- Fetch literals: the only `raw.githubusercontent.com` literal under `src-tauri/` and `src/` is `HOME_MARKDOWN_URL`, `src-tauri/src/commands/config.rs:36-37`. Row-1 version: `v0.5.0` is the tag directly before `v0.8.43`; `gh api .../contents/src-tauri/src/commands/config.rs?ref=<tag>` gives 0 literals at `v0.5.0` and 1 at `v0.8.43`; `npm view @mblua/agentscommander versions` lists no version between 0.5.0 and 0.8.50.
- Served-path regex simulation (the phase 3 regex and owner/repo filter, run by script): the round-1 phase-2 pin literal `format!("https://raw.githubusercontent.com/mblua/AgentsCommander/{REMOTE_BLOCKING_MENUS_SOURCE_REF}/...")` is counted, with ref `{REMOTE_BLOCKING_MENUS_SOURCE_REF}`; the round-2 segment assertion is not counted; the URL const is counted once, with ref `main`; the loopback URL is not counted; an `MBLUA/agentscommander` literal is counted.
- Line endings: `git config --show-origin core.autocrlf` gives `true` from `C:/Program Files/Git/etc/gitconfig`. `.gitattributes` pins `*.rs`, `*.json`, `*.toml` and `*.sh` to LF and has no `*.md` rule. `git ls-files --eol` gives `i/lf w/crlf` for `docs/home-en.md`, `PRIVACY.md`, `docs/features/menu-guard.md`, `docs/reference/settings.md`, `docs/reference/directory-layout.md` and `src/sidebar/components/SettingsModal.tsx`, and `w/lf` for `src-tauri/src/commands/config.rs` and `package.json`. A script run of phase 3's parse over its section 5.1 inventory: LF gives 2 rows and no error; CRLF with the `\r` kept gives 4 `malformed inventory row` errors plus `inventory has no rows`; CRLF with one trailing `\r` removed gives 2 rows and no error. The literal scan finds the same 2 literals, same paths and lines, on CRLF source.
- Phase 6 criterion 3, run on scratch copies of `update_check.rs` with `git diff --no-index -U0` and the criterion's `sed`, `cmp` and `awk`: the section 5.3 edit passes (`wc -l` 134, `cmp` exit 0, `awk` empty). Each control fails: an edited existing assertion (`cmp` exit 1), the submodule placed mid-module (`cmp` exit 1), one deleted `run_startup_check` line (`awk` prints `@@ -190 +196,0 @@`). `interval_elapsed` placed above `should_check` makes `awk` print `@@ -82,4 +84,3 @@`, so phase 6 pins the placement.
- Docs wording: `grep -n -i -E '(two|both) (blocking-menus )?files'` prints 6 lines at base, `docs/features/menu-guard.md:129,131,138,205` and `docs/reference/settings.md:462,473`; the round-2 case-sensitive `two files` grep saw only `menu-guard.md:129` and `:131`.
- Home renderer, `src/main/components/HomeView.tsx:6-11`, `MarkdownIt({ html: false, ... })`: `md.render('<!-- x -->')` returns `<p>&lt;!-- x --&gt;</p>`, so an HTML comment is visible.
- Frontend: `src/shared/types.ts:685`; npm checkbox block `SettingsModal.tsx:2102-2113`; round-trip test model `SettingsModal.automation.test.ts:2018-2048`; full `AppSettings` fixtures at `ui-harness.tsx:181`, `AgentPickerModal.test.tsx:185`, `CodingAgentQuickConfiguration.test.ts:124`, `OnboardingModal.test.ts:125`, `SettingsModal.test.ts:86`, `settings-save.test.ts:100`, `SettingsModal.automation.test.ts:207`.
- Docs: `docs/reference/settings.md:462` and `:548` say two blocking-menus files; `docs/features/menu-guard.md:129` and `:131` too.
- CI: ruleset 15279066 requires `validate-branch-name`, `lockfile-drift`, `rust-regression`, `frontend-regression`, `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable` (4 legs), `windows-release-cli-smoke`, `test-debt` and `rust-fmt`, with `require_code_owner_review: false`. `validate-branch-name` runs `scripts/validate-branch-name.mjs --check-issue` with pattern `^(bug|chore|ci|docs|feat|feature|fix|refactor|style|test)\/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$` (`:15`). `pr-regression-gates.yml` has no path filter; `test-debt` runs Node 22 with no `npm ci` (`:22-44`). `package.json` `record:arcs:self` `:29`.
- Absent at base: `remote-resources/`, `.github/CODEOWNERS`, `CODEOWNERS`, `docs/CODEOWNERS`.
- Room-rename allowlist keys `docs/reference/directory-layout.md:51` (`scripts/room-rename-allowlist.mjs:243`); no CI or test runs it.

## 4. Decisions

Binding owner decisions from the issue: (O1) serve from `main`; (O2) the opt-out flag defaults to `true` and has a Settings checkbox; (O3) the path is versioned per resource, `blocking-menus/v1/`.

Architect decisions:

- A1. Precedence, first hit wins and replaces the layers below it whole: legacy array on the agent; `local.byAgent`; `local.byCommand`; `remote.byCommand` (new); `shipped.byCommand`. `default_blocking_menus_for_command` and the #1905 migration stay shipped-only. No superset rule, so a remote `[]` can retract a shipped pattern.
- A2. No new module. The cycle gate (section 8) forbids one, so synchronous logic goes into `config::settings` and the async shell into `update_check.rs`.
- A3. One validator, `validate_remote_blocking_menus_file`, used on arrival, at every cache read, and in CI through a `cargo test` that `include_str!`s the published bytes (it runs in `rust-regression` on every PR).
- A4. Limits: 64 KB body, read by chunks and stopped past the cap; one 10 s timeout for send and body; HTTP 200 only; at most 200 entries; pattern at most 512 bytes; notification at most 200 bytes with no control characters; `RegexBuilder` `size_limit(256 * 1024)`; no empty-string match; no match on a 13-row embedded corpus; `byAgent` empty; unreadable entries reject. Memory ceiling in section 2.
- A5. Throttle stamp `blocking-menus-remote-check.json`, written after every attempt that passed the due check. The cache `settings-blocking-menus.remote.json` is written only on acceptance, by `write_file_atomic`.
- A6. The served-path check is a Node script with a self-test, run as a new step in the existing required job `test-debt`, so the ruleset needs no change.
- A7. No HTML comment in `docs/home-en.md`. The issue conditioned it on the renderer dropping comments; the renderer shows them (section 3), on every install.
- A8. The first published file equals the shipped file with only its `note` changed. It adds no pattern: the issue does not carry the text of the six further patterns.
- A9. Partition and landing order. Six phases, each with one owner and at most one contract. The Rust phase that starts the request (phase 6) lands last, after the served-path check that knows its literal (phase 3), the switch (phase 4) and the disclosure (phase 5). The flag field (phase 2) is split from the download so the checkbox can land against a persisted field before any request exists. Section 6 shows every landed state.
- A10. B1 treatment. The served-path scan keeps counting every literal, templated ones included, with no exemption and no allowlist; self-test case 11 pins that. Phase 6's pin test compares split URL segments, so it writes no counted byte run, and phase 6 must print `served-paths: 2 rows, 2 literals, consistent` with exactly one added `raw.githubusercontent.com/` line. Rejected: exempting `{`/`}` literals would let a templated production fetch escape the inventory; an allowlist weakens the every-literal rule.
- A11. B2 treatment. The flag read lives in exactly one function, `run_remote_blocking_menus_for_app<R: tauri::Runtime>`, which production reaches through `run_remote_blocking_menus_startup` and phase 6 test 9 calls through a mock app. Test 9 has an off case (npm flag on) and an on control (npm flag off), both against loopback and a tempdir, and phase 6 section 7 requires a three-mutation proof: `true`, `false`, and the npm flag.
- A12. Each phase's cycle gate compares its own pre and post trees, because the branch point moves with `origin/main`; the drift-baseline values are the reference.
- A13. Line endings in the served-path check. A Windows checkout holds the inventory in CRLF (section 3), and phase 6 runs the check on Windows. The script removes one trailing `\r` from each inventory line before parsing; phase 3 self-test case 12 fails a parser that keeps it, and phase 3 control C runs the real inventory as CRLF. Rejected: a `.gitattributes` pin, which adds a sixth phase 3 file and still does not cover a CRLF `--inventory` copy.
- A14. Existing-test protection in phase 6. A whole-file "removes no line" check contradicts the mandated `should_check` extraction. Phase 6 criterion 3 instead requires the base `mod tests` body to be a byte-identical prefix of the new one (the new submodule goes last), and every removed line to be `use std::path::PathBuf;` (`:10`) or inside `should_check` (`:84-92`), with `interval_elapsed` placed directly after `should_check`. It still catches an edited or deleted existing test and any other removed production line (section 3).

For the human gate. These are not silently decided; each follows the issue text:

- H1. The flag gates the download only, since the issue says it is "read once in the startup task". A cache already downloaded keeps applying while the flag is off, and the docs say so. Alternative, if the owner prefers it: skip the cache at store build when the flag is off (a settings read at `lib.rs:2970`).
- H2. `CODEOWNERS` requests review but does not block, because the ruleset has `require_code_owner_review: false`. Enforcement is a repository setting outside this diff.
- H3. A failed or offline attempt also waits 24 h. This differs from the npm check, and `PRIVACY.md` states the difference.
- H4. Additions (`claude` x3, `codex` x2, `grok` x1) need captured pattern text, and land as a later content PR on `main`.
- H5. One window cannot be avoided: after phase 4 or 5 lands and before phase 6, `main` has a working checkbox and a `PRIVACY.md` entry for a download that does not run yet. No request is made and no data leaves. It follows from the three landing rules plus "Rust, frontend and docs never share a phase": the request's Rust phase must land strictly after both the frontend switch and the docs disclosure. Recommendation: land phases 4, 5 and 6 back to back with no release cut in between; if a release is cut there, it ships only that conservative state.

## 5. Compatibility and state

- `AppSettings.remote_blocking_menus_enabled`: `#[serde(default = "default_true")]`, so existing settings files load `true`. Unverified risk: a downgrade to a binary without the field may drop a stored `false`.
- New instance files `settings-blocking-menus.remote.json` (phase 1) and `blocking-menus-remote-check.json` (phase 6) get registry rows. Instance `.gitignore` reconciliation is append-only, so existing installs gain both rules at the next start.
- Path contract: a binary pins `v1` = `BLOCKING_MENUS_SCHEMA_VERSION` forever. A future schema v2 publishes under `blocking-menus/v2/`, and `v1` stays served (inventory row).
- A later binary with a stricter validator rejects an older cache at read and falls back to shipped until its next accepted download.
- No migration, no hot swap, no file watcher: the store is still built once per process.

## 6. Every landed state of `main`

| After landing | Startup request | Settings switch | `PRIVACY.md` | Served-path check | A release cut here ships |
|---|---|---|---|---|---|
| base | none | not needed | accurate | absent; only the Home literal, as today | today's product |
| phase 1 | none | not needed | accurate | absent or present; only the Home literal | an empty remote layer (no install has the cache) |
| phase 2 | none | not needed | accurate | absent or present; only the Home literal | one settings key, default `true`, that nothing reads |
| phase 3 | none | not needed | accurate | present: 2 rows, 1 literal, consistent | a CI step and the inventory; row 2's version wording waits for phase 6 |
| phase 4 | none | present, persists | accurate | absent or present; only the Home literal | a switch for a download that does not run yet (H5) |
| phase 5 | none | present | discloses the download early | absent or present; only the Home literal | the same, plus an early disclosure (H5) |
| phase 6 | yes, default on | present | discloses it | present: 2 rows, 2 literals, consistent | the finished feature |

None of the three forbidden states occurs: the only state with a request is phase 6, and phase 6 section 2 step 3 stops unless phases 1 to 5 have landed and `npm run check:served-paths` passes. Phase 6 section 7 then requires `2 rows, 2 literals, consistent`.

## 7. Scope map

| Issue section | Phase |
|---|---|
| 1 fifth precedence layer | 1 |
| 2 source path, version pin | 1 (published file), 6 (URL const and pin test) |
| 3 fetch | 6 |
| 4 cache | 1 (reader), 6 (writer) |
| 5 validation on arrival and read, CI over published bytes | 1 (validator, read, CI test), 6 (HTTP status and size) |
| 6 settings flag and UI | 2 (Rust field), 4 (checkbox), 6 (the read) |
| 7 inventory, gates, CODEOWNERS, Home comment | 3 (Home comment dropped per A7) |
| 8 privacy and docs | 5 |

## 8. Dependency-cycle and layering statement

- Module arcs added: zero. Removed: zero. References the plan adds, all to pairs that already have an arc:
  - phase 1: `config::settings` to `config::instance_artifacts` (import list `settings.rs:10-12` grows) and to `config::local_config_io` (existing, `settings.rs:1149`); `regex` is external; the `pty::menu_guard` addition is test code.
  - phase 2: none (a field, a default, a fixture line, a test).
  - phase 6: `config::settings` to `config::instance_artifacts` and `config::local_config_io` (existing); `update_check` to `config::settings` (existing, `update_check.rs:189`) and to `network` (existing, `:262`); `lib` to `update_check` (existing, `lib.rs:3070`). `tauri::Runtime` and `chrono` are external. Test code is outside the measurement (`includeTests: false`).
- Verdict per reference: internal to the existing graph; none crosses an SCC boundary, so none can create or grow a cyclic SCC. The one cyclic SCC includes the crate root, `update_check`, `config::settings`, `commands::config` and `pty::menu_guard`; `config`, `network`, `config::local_config_io` and `config::instance_artifacts` are trivial SCCs whose incoming arcs already exist.
- Measured this round at the drift baseline (detector `01-rust_module-dependency-cycles.mjs`, exit 1 as normal): `199 modules, 3934 edges`; `[arc-record] ... 1091 arcs`; `cmp` against `src-tauri/module-arcs.txt` exit 0; `modules=199 cyclicSccs=1 sizes=86`; `scc0-sha256=5572EC179B7E7DAA97E5572FB2CD7D9337F7CA1BA645DB0C3CAD24CC2259FAB5`. Round 1's `ac-dev-rust-v4` and `ac-dev-rust-grinch-v4` reproduced the same values.
- Why no new module: one called from the crate root and calling `config::settings` would join the 86-member SCC and change its member set.
- Layering: `config::settings` gains no `tauri` or `AppHandle` use; `AppHandle<R>` stays in `update_check`.
- Post gate, in phases 1, 2 and 6 section 7: pre and post arc records `cmp`-identical to `src-tauri/module-arcs.txt`, pre and post SCC digests `cmp`-identical, and `instance_gitignore_layering` green (phases 1 and 6).

## 9. Delivery invariants

| Gate | Evidence, owner, time | Failure behavior |
|---|---|---|
| 1 CI parity | Per phase PR: every required check in section 3 plus every other triggered check (`rust-linux-release-parity`, `issue-1850-windows-profile` debug and release) green on the exact PR head SHA. `ac-tech-lead-v4` collects it at PR time. New evidence runs in `rust-regression` and its Linux and macOS legs plus `rust-fmt` and clippy (phases 1, 2, 6), `test-debt` (phases 3, 6), `frontend-regression` (phase 4). Phase 5 changes docs only, and the required checks still run on it. | Any red, skipped or other-SHA result blocks that phase's merge. |
| 2 Toolchain | Workflow-pinned stable Rust, `--locked`, Node 22 in CI; `Cargo.toml`, `Cargo.lock` and `package-lock.json` untouched in every phase; owners are the phase implementers. | A lockfile or manifest diff is a scope failure. |
| 3 Git | Parent #1925 open; one child issue per phase created by `ac-tech-lead-v4` after consensus; branch `feature/<child-issue>-<slug>` with the slug from section 12, matching the `validate-branch-name` pattern, cut from `origin/main` at that phase's Step 8; state-changing Git only in `repo-AgentsCommander`; one PR per phase, opened and landed by `ac-tech-lead-v4` in dependency order; never a direct push to `main`. Each phase's section 2 checks branch name, clean status, branch point and landed-dependency markers. | Wrong branch, dirty tree, unlanded dependency: stop and report. |
| 4 Process state | `cargo` from `src-tauri`, `npm` and `node` from the repo root; proxy variables for loopback tests (phase 6 section 2); graph files in scratch outside the repo. | An unrecorded environment deviation is reported before code. |
| 5 Scope | Per-phase exact path sets, checked by `git diff --name-only "$(git merge-base HEAD origin/main)" HEAD`; 25 unique paths (section 10). | An extra path fails the phase. |
| 6 Mutation and recovery | Per-phase section 10: restore only this run's own paths; no `reset` or `clean`; phase 6 reverts its mutation-proof edits. | External change: preserve it and report. |
| 7 Bounded execution | Existing CI job timeouts; the product fetch bounded at 10 s; the loopback helper serves one connection and ignores socket errors. | A timed-out step is a failure. |
| 8 Evidence | Commands, expected lines and failure behavior in each phase's section 7. Remote-only evidence, owner `ac-tech-lead-v4`: the `test-debt` log on the phase 3 and phase 6 PR heads, the CODEOWNERS errors API on the phase 3 branch, and `curl -sS -o /dev/null -w '%{http_code}' <endpoint>` = `200` after phase 1 lands. | Missing evidence means not done. |
| Build | `ac-shipper-v4` builds once, from `origin/main`, after phase 6 lands. It does not push and owns no PR. | A build from any other ref does not count. |

Drift: every phase's section 2 classifies `git diff --name-only b4f7c9de... <branch point>`. Files of already-landed #1925 phases are expected; only other drift in that phase's files, toolchain or workflows reopens that phase's evidence. Once a PR exists, exact-head checks are authoritative.

## 10. File-impact table

| Path | Phase | Kind | Reach | Risk note |
|---|---|---|---|---|
| `src-tauri/src/config/settings.rs` | 1, 2, 6 | modify | runtime + tests | store resolve order; validator; flag field; S6 fixture; URL const |
| `src-tauri/src/config/instance_artifacts.rs` | 1, 6 | modify | runtime | 2 registry rows; leaf property preserved |
| `src-tauri/src/config/instance_gitignore.rs` | 1, 6 | test only | tests | 2 fixture paths |
| `src-tauri/src/pty/menu_guard/mod.rs` | 1 | test only | tests | evaluator proof |
| `remote-resources/blocking-menus/v1/settings-blocking-menus.json` | 1 | new | every installed client once phase 6 ships | served from `main` with no staging (O1) |
| `src-tauri/src/update_check.rs` | 6 | modify | startup network | new detached request; `should_check` refactor |
| `src-tauri/src/lib.rs` | 6 | modify | startup | one spawn block |
| `src/shared/types.ts` | 4 | modify | IPC type | one field |
| `src/sidebar/components/SettingsModal.tsx` | 4 | modify | UI | one checkbox |
| `src/shared/testing/ui-harness.tsx` | 4 | modify | tests | fixture key |
| `src/sidebar/components/AgentPickerModal.test.tsx` | 4 | test only | tests | fixture key |
| `src/sidebar/components/CodingAgentQuickConfiguration.test.ts` | 4 | test only | tests | fixture key |
| `src/sidebar/components/OnboardingModal.test.ts` | 4 | test only | tests | fixture key |
| `src/sidebar/components/SettingsModal.test.ts` | 4 | test only | tests | fixture key |
| `src/sidebar/components/settings-save.test.ts` | 4 | test only | tests | fixture key |
| `src/sidebar/components/SettingsModal.automation.test.ts` | 4 | test only | tests | fixture key + round-trip test |
| `remote-resources/SERVED-PATHS.md` | 3 | new | repo metadata | append-only by rule |
| `.github/CODEOWNERS` | 3 | new | review routing | non-blocking (H2) |
| `scripts/check-served-paths.mjs` | 3 | new | CI | both directions + 12-case self-test |
| `package.json` | 3 | modify | scripts | 2 entries |
| `.github/workflows/pr-regression-gates.yml` | 3 | modify | required job `test-debt` | one step |
| `PRIVACY.md` | 5 | modify | public policy | must not contradict #1924 |
| `docs/reference/settings.md` | 5 | modify | docs | menu guard section; related-links line |
| `docs/features/menu-guard.md` | 5 | modify | docs | three files, precedence |
| `docs/reference/directory-layout.md` | 5 | modify | docs | 2 rows |

## 11. Acceptance map (issue acceptance to proof)

| # | Proof |
|---|---|
| 1 | Phase 6 test `an_accepted_download_is_cached_and_applies_at_the_next_start` and phase 1 test `a_remote_pattern_in_the_cache_reaches_the_evaluator_1925`. Real endpoint reachability: the `curl` in section 9 gate 8. |
| 2 | Phase 6 test `the_startup_path_reads_the_remote_flag_not_the_npm_flag` through the production flag read, with its three-mutation proof, and test `disabled_makes_no_request_and_writes_nothing`. |
| 3 | Phase 6 test `every_failure_keeps_the_previous_cache` (7 cases, cache byte-equal); phase 1 test `every_rejection_names_its_check`; the phase 6 no-emit grep. |
| 4 | Phase 1 test `precedence_is_local_then_remote_then_shipped`. |
| 5 | Phase 1 test `default_blocking_menus_for_command_ignores_the_remote_cache`. |
| 6 | Phase 4 round-trip test; phase 2 test `remote_flag_defaults_true_and_round_trips`. |
| 7 | Phase 3 self-test (12 cases), controls A, B and C, and the real run; the phase 6 real run with 2 literals. |
| 8 | Phase 5 sections 7 and 8 checks. |

## 12. Phase table

| Id | Child issue | Branch slug | Class | Owner | Files | Depends on | Parallel with | Phase-SHA256 |
|---|---|---|---|---|---|---|---|---|
| `phase-1-rust-remote-layer` | tech-lead creates | `rust-remote-layer` | design-bearing | `ac-dev-rust-v4` | 5 | none | none | `CBA6F650FA9827EB73206EE4E9FC219235A5F090AB35434C466BB3DC0E21A91E` |
| `phase-2-rust-settings-flag` | tech-lead creates | `rust-settings-flag` | patterned | `ac-dev-rust-v4` | 1 | phase 1 | phase 3 | `476033BB04CA3E937F11AAD5C612F7364C07816FC7832952E35A9C27EBB55831` |
| `phase-3-ci-served-paths` | tech-lead creates | `ci-served-paths` | design-bearing | `ac-dev-webpage-ui-v4` | 5 | phase 1 | phases 2, 4, 5 | `BF71B59001716479877141D751BD1F7D74CB10816BBCE2B764DDC38569DDEBDF` |
| `phase-4-frontend-checkbox` | tech-lead creates | `frontend-checkbox` | patterned | `ac-dev-webpage-ui-v4` | 9 | phase 2 | phase 3 | `92DD4E579C8AAEE9BC1CDD62B57A81B149B948104B2B7A6FD7C4220887B68983` |
| `phase-5-docs` | tech-lead creates | `docs` | patterned | `ac-technical-writer-v4` | 4 | phases 2, 4 | phase 3 | `F286D5FE27BFAA37AC571DD437FAF8B19B3B26F04C9C811BEDFE4A8F8AE243B8` |
| `phase-6-rust-startup-download` | tech-lead creates | `rust-startup-download` | design-bearing | `ac-dev-rust-v4` | 5 | phases 1, 2, 3, 4, 5 | none | `017DB55E0AF66B06916930F0142830FFBEC5220D18BF514B7311CEAE66932DEB` |

Digests are SHA-256 over each file's working-tree bytes (LF, no CR). Each phase file carries its own `Status: READY_FOR_IMPLEMENTATION`.

## 13. Out of scope and residuals

- Out of scope, per the issue: applying without restart; a "refresh now" action; a Settings surface for `.local`; moving `docs/home-en.md`; any change to `menuGuardEnabled`.
- Residuals, none blocking: H5's window. Append-only is enforced by review, not CI. A published array for a stem can hide a newer shipped pattern for that stem until the file is regenerated from `main`'s shipped file; that is replace-not-merge by design. A8's first published file adds no pattern, so acceptance 1's real-world demonstration waits on H4; the loopback proof is the strongest available now.

## 14. Round-2 resolution log

| Round-1 finding | Resolution |
|---|---|
| P1 one child issue, branch and PR per phase | Every phase header gives the slug and the `feature/<child-issue>-<slug>` pattern, the branch point `origin/main` at its Step 8, and `b4f7c9de` only as the drift baseline. Section 2 checks the branch pattern, `HEAD` = `origin/main` before the first commit, and landed-dependency markers; section 7 and 8 diffs use `git merge-base HEAD origin/main`. File names keep `phase-<k>-<slug>.md`. |
| P1b every landed state shippable | Six phases, download last (A9); section 6 shows every state; H5 names the one unavoidable window. |
| P2 per-phase `Status:` | Present in all six phase files. |
| P3 landing and build ownership | Tech-lead opens and lands PRs and collects exact-head CI (every phase header, section 9 gates 1, 3, 8, phase 3 section 7); shipper builds once from `origin/main` after phase 6. |
| B1 pin test versus scan | A10: no exemption; segment pin test; phase 3 self-test case 11; phase 6 checks `2 rows, 2 literals`. Webpage-ui's count was right (section 3 simulation). Phase 3 declares its phase 1 dependency. |
| B2 production flag read | A11: `run_remote_blocking_menus_for_app` plus test 9 and the three-mutation proof. |
| Omissions | S6 fixture line in phase 2 (with a negative control); `settings.md:548` in phase 5 section 5.2 and section 7; phase 2 marker grep in phase 5 section 2; anchors `SettingsModal.tsx:2102-2113` and round-trip model `:2018-2048` in phase 4. |
| Non-blocking notes | Case-insensitivity positive control: phase 3 case 6. Loopback write errors ignored: phase 6 `serve_once`. Compiled-program bound: section 2, phase 1 D6. Row-1 `0.8.43`: verified by tag contents and npm versions (section 3). |

## 15. Round-3 resolution log

| Round-2 finding | Resolution |
|---|---|
| R1 phase 6 criterion 3 contradicts the `should_check` extraction | A14: criterion 3 is now three commands (a 134-line `mod tests` prefix `cmp`, plus an `awk` over `-U0` hunks that allows removals only at `:10` and `:84-92`). Section 5.3 step 3 pins `interval_elapsed` directly after `should_check`; section 6 puts the new submodule last in `mod tests`. Simulated pass and four controls in section 3. |
| R2 CRLF behavior of the phase 3 script | A13: phase 3 D8 and section 5.3 remove one trailing `\r` per inventory line; self-test case 12 (CRLF) fails a parser that keeps it; control C runs the real inventory as CRLF. No attributes pin, so phase 3 stays at 5 files. |
| Grinch: phases 4 and 5 lack the environment-risk sentence | Added to both as section 2 step 4, appended so no step number moves. |
| Grinch: phase 1 acceptance omits `export_blocking_menus_to_local_file` | Added to phase 1 criterion 3's no-change list. |
| Grinch: boundary tests are ASCII only | Phase 1 test 3 adds a pattern of 257 `é` (514 bytes) and a notification of 101 `é` (202 bytes); a character count accepts both, so each fails that slip. |
| Writer: case-sensitive `two files` grep | Phase 5 section 7 now uses a case-insensitive grep for "two" or "both" (blocking-menus) files, 6 lines at base (section 3); section 5.3 names `:138` and `:205`. |
| Writer: stamp row `Source` | Phase 5 section 5.4 gives both new rows `config/settings.rs`, `update_check.rs`, following the existing rule that Source names the reader and writer, not the constant's home. |

Unchanged since round 2: phase 2 bytes, and every decision the round-2 reviewers approved.
