# #2001 — Isolate the legacy-config-dir reseed test from the shared real config path

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2001 (OPEN).
- Repo: `repo-AgentsCommander`; branch `fix/2001-reseed-test-isolation`.
- Base frozen at authoring (2026-09-15 UTC): branch HEAD = remote branch head = `main` =
  `15545bd59051445819f06270d2a5f8755380147e` (`git rev-parse HEAD`; `git ls-remote origin
  refs/heads/fix/2001-reseed-test-isolation`). Tracked tree clean. Every line number below refers
  to that SHA; if a quoted line no longer matches, re-anchor on the quoted text, never on the
  number.
- Class: Lite (band 1-25); owner `ac-dev-rust-v4`; reviewer Grinch; coordinator `ac-tech-lead-v4`.
- PARTITION: 1 phase, no split. 1 product file + this doc. No new interface, dependency, schema,
  IPC or module arc; no production line changes.
- Canonical plan: this file. Root `.gitignore` ignores `/plans/`, so a future commit uses
  `git add -f plans/2001-reseed-test-isolation.md`.
- Authoring actions: read-only investigation plus two runs of the exact filtered command
  (`-- --list`: 136 tests; full filtered run: `136 passed; 0 failed; 4316 filtered out`). No source
  edit, no commit, no push.

## 1. Objective and acceptance

Isolate `config::coding_agents_catalog::tests::reseed_with_no_primary_targets_legacy_config_dir`
(`src-tauri/src/config/coding_agents_catalog.rs:5139`) from the real per-binary `config_dir()`, so
that concurrent executions of the same lib test binary cannot race on
`<config_dir>/coding-agents/_seed/.claude` (the #1999 follow-up filed as #2001).

Required acceptance (issue #2001 + coordinator):
- 12 runs in groups of 3 concurrent same-binary processes of
  `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib config::coding_agents_catalog`,
  zero failures, raw logs saved under `room-shared/2001-verification/`.
- Proof the test no longer uses/touches the shared real path, with a positive control.
- #1999 scope preserved: no unrelated catalog or injection change.

## 2. Verified cause at the frozen base

Writer — the only real-path mutator in the module. `reseed_with_no_primary_targets_legacy_config_dir`
(5138-5162):
- resolves the real per-binary config dir (5143), deletes `<real>/coding-agents` (5150), recreates
  the legacy shape plus a marker (5151-5152), calls `reseed_master_for_command(&config_dir,
  "claude")` (5154), asserts the re-seeded bytes (5155, 5158), deletes the tree again (5161).
- `reseed_master_for_command` (3960) takes `RESEED_LOCK` (3628; acquire at 3968), a process-local
  `std::sync::Mutex`. It serializes re-seeds inside ONE process and cannot serialize separate test
  binaries.

Same-binary sharing: `crate::config::config_dir()` (`src-tauri/src/config/mod.rs:954`) is resolved
once per process; a suffixed executable gets the portable sibling `<exe-parent>/.<exe-stem>/`
(`resolve_instance_location` :749, `adjacent_paths` :666; `AGENTSCOMMANDER_CONFIG_DIR` /
`AGENTSCOMMANDER_TEST_CONFIG_DIR` overrides :917-920). All processes launched from this workspace
share one intermediate build dir (`D:/ac_temp_builds/{workspace-path-hash}`; observed
`D:/ac_temp_builds/52/f50d875e1ee632`), hence one test-binary path and one `config_dir()`. Cargo's
build lock serializes compilation ("Blocking waiting for file lock" in the #1999 raw logs), never
the test executions.

Readers (read-only, through `ensure_seeded_for_project_with_token`'s legacy resolution at 3845):
- `run_catalog_initialization` reads `<legacy>/agents.json` via `read_instance_legacy_source` (558)
  only while the project base is absent;
- `ensure_seeded_masters` (3706) copies `<legacy>/_seed/<dest>` for each absent supported master.
While another process deletes/recreates `<legacy>`, those reads can fail (`Access is denied` /
`NotFound`) and abort initialization without publishing.

Evidence refs:
- `room-shared/1999-race-evidence/README.md` and its SHA-256 inventory; `head-stress-r2-2-FAIL.log`
  (`NotFound` at `coding_agents_catalog.rs:5092:68`) and `head-stress-r2-3-FAIL.log` (`Access is
  denied` at `:5089:9`). At the frozen base those statements are 5158 and 5155 (revision offset
  +66); `base-stress-*` logs: 12/12 green at `51fff3d` with `127 passed; 0 failed; 4288 filtered
  out`.
- Issue #2001 body and acceptance criteria.
- Authoring baseline at the frozen base: the exact filtered command is green (see header).

## 3. Reader inventory at the frozen base (required by #2001)

Every entry below reads the real `<config_dir>/coding-agents` tree through the `legacy` binding at
3845; none writes it.

| Test (fn line) | Reader call lines | What it reads | When |
|---|---|---|---|
| `ensure_seeded_for_project_steady_state_precheck_skips_gate_and_writes` (5049) | 5054, 5063, 5072 | `agents.json` (5054); absent `_seed/<dest>` (5054); `_seed/.claude` after it was removed (5072) | fresh fixture; steady-state reruns do not read |
| `managed_catalog_all_masters_present_ignores_desupported_master` (5699) | 5731 | `_seed/.codex` (only the removed supported master) | base and other masters pre-seeded |
| `managed_catalog_untracked_publication_is_retried_without_republishing` (7710) | 7723 | `_seed/*` | base pre-seeded, masters never seeded |
| `managed_catalog_degraded_manifest_record_failure_repairs_without_republishing` (8289) | 8299, 8349, 8352 | `agents.json` + `_seed/*` (8299); none later | fresh fixture at 8299; base+masters present later |
| `managed_catalog_stub_failure_is_nonfatal_and_never_retried` (8402) | 8463 | `_seed/*` | base pre-published, masters absent |

Non-readers: `ensure_seeded_for_project_skips_missing_or_relative_root_without_writes` (5035;
returns at 3830-3843 before the 3845 resolution); `managed_catalog_steady_state_takes_the_gate_and_
keeps_bytes` (5743; base and masters pre-seeded, so `all_masters_present` short-circuits at 3707
and a present base never consults legacy).

Out of module: the module's `load_catalog_for_settings*` tests always inject a temp project; #1999's
whole-lib inventory found no other `<config_dir>/coding-agents` writer or reader (session-requests,
`sessions.json`, phone mailbox and the instance-settings snapshot use different subtrees), and none
of them matches the filtered command.

Decision: readers stay unchanged. Once the writer is isolated, no test mutates
`<config_dir>/coding-agents`; every remaining access is read-only, concurrent reads cannot corrupt
each other, and the specific race (a writer deleting the tree under a reader) cannot occur. Their
ambient dependence on a machine-local legacy source (e.g. a corrupt real `agents.json` makes the
steady-state precheck test fail) predates #2001, is not a race, and stays out of scope.

## 4. Decided solution

D1 — the only product change: the writer test builds its legacy fixture under its own
`tempfile::TempDir` (`seed_dir()`, 4165) and passes that path as the `ac_dir` argument of
`reseed_master_for_command`; the real `config_dir()` is never resolved. Byte-exact replacement in
§5.

Why the fixture is equivalent: `master_dir_for_dest` (3631) derives every target as
`<ac_dir>/coding-agents/_seed/<dest>`, so the legacy-layout semantics are a function of the `ac_dir`
argument alone, and `reseed_master_for_command` has no other environment input. The production
decision of which path to pass (no primary root -> `config_dir()`) lives in
`src-tauri/src/commands/config.rs:574-590` (`reseed_coding_agent_default`), is unchanged, and is not
what this test asserts.

Rejected alternatives (closed):
- Cross-process file lock around the real tree: does not stop a reader from observing the
  delete/recreate window between acquisitions (readers would have to take the same lock, a
  production change) and adds test artifacts to the shared real tree. `RESEED_LOCK` stays as the
  production single-process guard.
- Per-process subdirectory under the real `<config_dir>/coding-agents`: still mutates the shared
  real tree, still runs `remove_dir_all` on it, and leaves garbage.
- Mutating `AGENTSCOMMANDER_CONFIG_DIR` from inside the test: the process-global value is cached by
  `OnceLock` and tests in one process run in parallel threads, so this is racy by construction. The
  override stays a verification-harness tool only (§7).
- Threading an injected legacy dir into `ensure_seeded_for_project*` to isolate the readers: not
  needed once the writer is gone; it would change a production signature and six test call sites
  for no race benefit.
- Keeping the real path and relying on `--test-threads=1`: does not serialize separate processes,
  and still deletes a real tree.

## 5. Exact change (one file, one hunk)

File: `src-tauri/src/config/coding_agents_catalog.rs`, symbol
`reseed_with_no_primary_targets_legacy_config_dir` (5138-5162). Keep `#[test]` (5138) unchanged;
replace lines 5139-5162 with exactly:

```rust
    fn reseed_with_no_primary_targets_legacy_config_dir() {
        // The Settings re-seed button must keep working on pre-migration
        // installs with zero registered projects: no primary root -> the legacy
        // `<config_dir>/coding-agents/_seed/<dest>` masters are the target.
        //
        // #2001: this fixture dir stands in for the per-binary `config_dir()`.
        // This test is the ONLY writer of the real `<config_dir>/coding-agents`
        // tree, and separate test processes share that path; an in-process lock
        // cannot serialize processes, so the real path is never resolved here.
        let config_dir = seed_dir();
        let legacy_master = config_dir
            .path()
            .join("coding-agents")
            .join("_seed")
            .join(".claude");
        std::fs::create_dir_all(&legacy_master).unwrap();
        std::fs::write(legacy_master.join("marker"), b"x").unwrap();

        let result = reseed_master_for_command(config_dir.path(), "claude");
        assert!(result.is_ok(), "legacy reseed works: {result:?}");
        let m = master("claude");
        assert_eq!(
            std::fs::read(legacy_master.join(m.files[0].rel_path)).unwrap(),
            m.files[0].bytes
        );
    }
```

Deltas vs base: drop the `config_dir()` resolution and its None-skip (5143-5145), the initial
`remove_dir_all` (5150) and the final cleanup (5161); bind `config_dir` to `seed_dir()`; route the
path uses through `.path()`; the four reseed assertions are byte-unchanged. The test name stays for
traceability; test count and filtered count do not change. The block is `cargo fmt`-stable; run
`cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` after the edit.

## 6. Behavior and edge cases

- Concurrency: each process gets its own tempdir; no cross-process sharing. The `.bak-*` backup,
  `reseedtmp`/`reseedold` staging and trash dirs live inside the fixture and die with it.
- Machines without a resolvable real config dir: the old test silently skipped (`else return`); the
  fixture test always runs, so the legacy-layout behavior is always covered.
- Removed hazard: the old test deleted the real `<config_dir>/coding-agents` tree on every run
  (a pre-migration install's `agents.json` and `_seed` masters included). That single-process data
  destruction is gone as a side effect.
- `reseed_master_for_command` behavior is unchanged: `.bak` first, trash-first swap, embedded
  verification bytes, error strings.
- Readers are unchanged and remain read-only; the production no-primary fallback remains covered by
  inspection only (stated, not a TBD).

## 7. Verification plan (objective)

Evidence root: `room-shared/2001-verification/`. All runs from the repo root, `2>&1` into raw
`*.log` plus a `*.exit` file. Config-dir pinning: `AGENTSCOMMANDER_CONFIG_DIR=<absolute path under
room-shared/2001-verification/>` for every control and baseline run, so the shared "real" path is
observable in the evidence zone and pre-fix runs cannot mutate a shared build-dir location. The
override is honored verbatim (`config/mod.rs:770`, `override_location`); the test binary is a debug
build. The fixture test itself never resolves `config_dir()`.

Pre-fix phase (frozen base, before the edit):
- B0 baseline reproduction: 12 runs (4 rounds x 3 concurrent) of the exact command with override
  `baseline/cfg`; raw logs. Expected: at least one failure as in #1999 (report the actual count).
- C1 positive control (deterministic): pre-create `control-prefix/cfg/coding-agents/_seed/.claude/`
  with `witness.txt` (record its sha256), hold a `cmd.exe` process with CWD
  `<...>/cfg/coding-agents`, then run only the writer test once:
  `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --exact
  config::coding_agents_catalog::tests::reseed_with_no_primary_targets_legacy_config_dir`.
  Acceptance: the witness is gone and/or the tree fingerprint changed; release the holder and
  capture the residue listing. (If the CWD hold cannot be established, the witness comparison alone
  still decides.)

Post-fix phase (after the one-hunk edit):
- C2 negative control: same fixture, hold and command; require the witness byte-identical, no
  `settings.json` and no `.bak-*` created, fingerprint identical.
- Suite A (official acceptance): 12 runs (4 rounds x 3) of the exact command with override
  `suite-a/cfg`; fingerprint (`find -printf '%y %s %T@ %p'` + per-file sha256 + mtime of `cfg` and
  its parent) before, after each round, and after the suite; all fingerprints identical and the
  witness present.
- Suite B (environment parity, no override): 12 runs (4 rounds x 3) of the bare exact command; logs
  only; the machine-local path is neither read nor fingerprinted.
- Static proof S1: `git diff` shows the one hunk; extract the new test body with `awk` and assert
  `grep -c 'crate::config::config_dir'` = 0; list the module's remaining `crate::config::config_dir`
  references (production only: 351, 2175, 3845).
- S2: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`; PR CI green on the branch head;
  the mandatory rust-regression on the exact HEAD per the issue.
- `README.md` under the evidence root with method, exact commands, per-run status, SHA-256
  inventory, and the C1/C2 comparison.

Acceptance criteria (objective):
- AC1 one product path changes (plus this plan); no production line touched.
- AC2 the new test body has zero `crate::config::config_dir` references.
- AC3 a single filtered run reports `test result: ok. 136 passed; 0 failed; ...` (counts as measured
  at the frozen base; re-record the exact line in the evidence README).
- AC4 every Suite A and Suite B log shows `test result: ok.` and `0 failed`, every `.exit` is 0.
- AC5 Suite A fingerprints (before / per round / after) are identical and the witness is intact.
- AC6 C2 passes and C1 demonstrates the touch.
- AC7 PR CI and the mandatory rust-regression are green.

## 8. Environment risk (Windows locks, shared build dir)

- Windows sharing: `remove_dir_all`/`rename` fail with `Access is denied (os error 5)` when any
  handle is open under the directory or the directory is another process's CWD, and with `NotFound`
  when a concurrent process deleted it first. The two #1999 panics are exactly these. A per-process
  mutex cannot serialize processes.
- Shared intermediate build dir: `.cargo/config.toml` sets `jobs = 12`; the user-level cargo
  `build.build-dir` routes intermediates to `D:/ac_temp_builds/{workspace-path-hash}`
  (`.../52/f50d875e1ee632` observed), so all concurrent same-binary processes share the executable
  and its portable `config_dir()`. Cargo's build lock serializes builds, not test executions.
- Destructive pre-fix behavior: the current test deletes `<real>/coding-agents`; therefore every
  pre-fix control/baseline run pins `AGENTSCOMMANDER_CONFIG_DIR` into `room-shared/2001-verification/`
  so nothing out of zone is mutated. Suite B runs only after the fix, when no test mutates that
  path. The authoring baseline run (exact command, no override) touched the default per-binary path
  exactly as the test was written to; it completed its own cleanup (5161).
- Temp dirs: `tempfile::tempdir()` is per process; reader tests keep using `%TEMP%` with distinct
  dirs. Room concurrency: keep the filtered command; do not run the unfiltered lib suite
  concurrently (other module tests mutate other real config subtrees under in-process locks only).

## 9. Plan Contract

No TBD, no open decision, no competing alternative (all rejected alternatives are closed in §4).
Every touched symbol is named with its file and line at the frozen base
`15545bd59051445819f06270d2a5f8755380147e`. Sole product change: the body of
`config::coding_agents_catalog::tests::reseed_with_no_primary_targets_legacy_config_dir`. No
production behavior changes; no new test; the acceptance of §7 is the definition of done.
