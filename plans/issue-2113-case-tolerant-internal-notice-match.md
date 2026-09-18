# Issue #2113 — case-tolerant internal-notice session match

Status: READY_FOR_IMPLEMENTATION
Class: Lite (score 28, band 26-55)
Issue: https://github.com/mblua/AgentsCommander/issues/2113
Repo: `repo-AgentsCommander`
Branch: `fix/2113-case-tolerant-internal-notice-match`
Base SHA (pinned): `20053458f3847fa6ca4ad8783133df8f5e134e0a`
Round: 3. Round 1 (`71CE2126E95C0B298AB415BDD0539790FD1BC97276AB0E6EC64C134BE8716BC4`) and round 2 (`55E15D54837780E14097721E35B18F4C9D61482E9965F05168F6244EC26EBC74`) were rejected; sections 10 and 11 record what each blocker changed.
Task class / threat model: routine application-code change. No release, signing, packaging, untrusted host, security-boundary or destructive migration. Every enhanced control of `delivery-nonfunctional-invariants` is NON-APPLICABLE for that reason; the baseline gates in section 8 apply.

## 1. Objective

An internal system notice (context alert; remote-activity notice from #2083) addressed to a room orchestrator must reach that orchestrator's live session even when the project path spelled in settings differs from the on-disk folder only in letter case.

## 2. Cause, with evidence

All line numbers are at the pinned base.

1. `src-tauri/src/phone/mailbox.rs:8215` `find_internal_system_candidates` pre-filters sessions with a raw string compare at `:8233`:
   `crate::config::teams::agent_fqn_from_path(&session.working_directory) == target_fqn`.
2. `src-tauri/src/config/teams.rs:80` `agent_fqn_from_path` only replaces `\` with `/` and splits. It does no case folding; the project, room and agent segments keep the caller's spelling.
3. The session side keeps the settings spelling: session working directories are built from `cfg.project_paths`, and `SessionManager::create_session` (`session/manager.rs:2262`) stores `working_directory` verbatim, with no canonicalization (`:2285`).
4. The target side keeps the disk spelling: `InternalSystemTarget::for_context_alert` (`mailbox.rs:90`) canonicalizes the replica dir at `:106`, derives the expected FQN from that canonical path at `:140`, and rejects any FQN that differs (`:141`). So every `InternalSystemTarget` that exists carries on-disk case. Both constructors feed it canonical input (`session/remote_alerts.rs:394`, `session/context_alerts.rs:1657`).
5. Therefore, when settings spell `D:\0_repos\Proj` and disk holds `D:\0_repos\proj`, the two strings differ, the pre-filter yields zero candidates, delivery falls to the background-spawn path, and the path-identity dedup gate answers `sessionRace`. The notice is lost and nothing tells the user.
6. The same raw compare is repeated in the exited-orchestrator destruction guard at `mailbox.rs:8327` (`recheck_internal_exited_record`).

Key fact that decides the design: the pre-filter is NOT the authority. Every candidate that survives it is then re-checked by `canonical_cwd_owned_by_replica` (`mailbox.rs:517`, called at `:8241`, `:8297`, `:8337`, `:8376`, `:8425`, `:8446`), which canonicalizes the session CWD and requires it to equal or sit under the canonical replica dir. A pre-filter false positive is discarded there. A pre-filter false negative is the bug in this issue.

## 3. Scope

In scope:

- `src-tauri/src/phone/mailbox.rs`: the two FQN compares at `:8233` and `:8327`, one new private helper, and new tests in the single `mod tests` at `:12822`.
- `.github/workflows/pr-regression-gates.yml`: one added step in the `rust-regression-linux` job. Required, and only required, because that job runs focused filters, never the suite; see A7 and section 10 (B3).

Out of scope, with reason:

- `config/teams.rs::agent_fqn_from_path`: unchanged. It is used by 40+ call sites (CLI `send`, `list-peers`, API identity binding, container tokens, session manager). Changing its output or adding folding there would alter identity for all of them.
- **The `room-`/`wg-` prefix letters themselves.** This is the one case skew the fix does NOT close, and it is out of scope deliberately. `has_entity_prefix` (`config/entity_prefix.rs:30`) is case-sensitive, and its own test asserts `!has_entity_prefix("Room-1-t")` (`:48`). So for a CWD whose room segment is spelled `Room-1-Team`, `agent_fqn_from_path` fails the prefix gate (`teams.rs:89`) and falls through to `agent_name_from_path` (`:98`), which returns `Room-1-Team/Bob` — a two-segment name with no `proj:` prefix at all. No amount of lowercasing at the pre-filter can turn that into `proj:room-1-team/bob`. Closing it would mean case-folding `has_entity_prefix`, which gates room discovery, addressing and authorization repo-wide; that is a different change with a different blast radius, and this issue's evidence does not reach it. The gap is narrow and self-limiting: AgentsCommander only ever creates room directories with a lowercase `room-` prefix, and a directory spelled `Room-1-Team` is not recognised as a room by any other gate either, so such a session has no working identity to lose. What IS covered is stated in 5.2 and pinned by T2.
- `session_cwd_matches_fqn` (`mailbox.rs:2482`) and the general wake path: unchanged. That path has no `canonical_cwd_owned_by_replica` backstop, so widening it is not justified by this issue's evidence.
- The `session.working_directory != expected_cwd` compare at `mailbox.rs:8326`: unchanged, and it MUST stay a raw compare. It asks "is this the same record byte-for-byte as the one we selected", not "is this the same agent"; folding it would defeat its purpose.
- `InternalSystemTarget::for_context_alert`'s `fqn != expected` check at `:141`: unchanged. A caller-supplied FQN that disagrees with the canonical replica is a construction error that is already reported, not a silent drop.

## 4. Decided solution (single option, no alternative)

Widen the pre-filter; leave path identity authoritative.

Add one private free function to `src-tauri/src/phone/mailbox.rs`, placed immediately after `session_cwd_matches_fqn` (`:2482-2484`):

```rust
/// #2113: case-tolerant pre-filter for internal system notices.
///
/// The target FQN is derived from the CANONICAL replica dir
/// (`InternalSystemTarget::for_context_alert`), so it carries the on-disk
/// spelling. A session CWD carries the spelling in `cfg.project_paths`, stored
/// verbatim by `SessionManager::create_session`. On a case-insensitive
/// filesystem the two can differ by case and still name the same directory,
/// which dropped the notice (#2113).
///
/// Covers a case difference in the project segment, in the agent segment, and
/// in the room segment AFTER its `room-`/`wg-` prefix. It does NOT cover a
/// case difference in the prefix letters themselves: `has_entity_prefix` is
/// case-sensitive, so `Room-1-Team` never reaches this comparison in
/// project-qualified form. That gap is out of scope; see the plan.
///
/// Deliberately over-wide: `to_lowercase` is Unicode simple lowercasing, which
/// matches neither the NTFS uppercase table nor Linux's byte comparison. That is
/// safe HERE and only here, because this function is a pre-filter, never the
/// authority. Every candidate it admits is re-checked by
/// `canonical_cwd_owned_by_replica`, which canonicalizes the CWD and requires
/// real filesystem identity with the replica dir. A false positive from this
/// function is discarded there; a false negative loses the notice.
///
/// Cross-OS risk, stated: on Linux `.../__agent_bob` and `.../__agent_Bob` are
/// two distinct real directories, and this function matches both. It does not
/// deliver to both: the canonical gate admits exactly the one whose canonical
/// path equals the target replica dir. See the `linux` test below.
fn internal_target_fqn_prefilter_matches(cwd: &str, target_fqn: &str) -> bool {
    let candidate = crate::config::teams::agent_fqn_from_path(cwd);
    candidate == target_fqn || candidate.to_lowercase() == target_fqn.to_lowercase()
}
```

Rejected and why: canonicalizing the session CWD inside the compare. The pre-filter at `:8233` runs in a plain synchronous `Iterator::filter` on the async executor over every session; `std::fs::canonicalize` is a blocking syscall and would have to move into the existing `spawn_blocking` below it. That is a larger change that buys nothing, because the canonicalizing check it would duplicate already runs there.

### Call-site changes

Both sites use the same helper, so the selection rule and the destruction guard stay consistent.

`mailbox.rs:8232-8234`, inside `find_internal_system_candidates`, replace

```rust
                    && crate::config::teams::agent_fqn_from_path(&session.working_directory)
                        == target_fqn
```

with

```rust
                    && internal_target_fqn_prefilter_matches(&session.working_directory, &target_fqn)
```

`mailbox.rs:8327`, inside `recheck_internal_exited_record`, replace

```rust
            || crate::config::teams::agent_fqn_from_path(&session.working_directory) != target.fqn()
```

with

```rust
            || !internal_target_fqn_prefilter_matches(&session.working_directory, target.fqn())
```

### CI change

`rust-regression-linux` runs `cargo check`, `cargo clippy` and focused filters by name; it never runs the suite, and the job's own comment (`.github/workflows/pr-regression-gates.yml:723-725`) records that `#[cfg(unix)]` tests "have compiled for months and never once executed". T6 is the only executable evidence for the cross-OS risk stated in the doc comment, and without a step it would compile on Linux and run nowhere. Insert, immediately after the `IS #1842` step and before the `IS #1850 focused, debug` step (currently `pr-regression-gates.yml:826`, the blank line before the `# IS #1850` comment at `:827`):

```yaml
      # IS #2113: the widened internal-notice pre-filter is deliberately
      # over-wide, and on a case-SENSITIVE filesystem it matches two distinct
      # real replica directories. The control that it still cannot deliver to
      # the wrong one is Linux-gated and the suite runs only on the Windows
      # leg, so without this step it would compile here and never execute.
      - name: "cargo test (IS #2113 case-tolerant notice pre-filter)"
        working-directory: src-tauri
        shell: bash
        timeout-minutes: 10
        run: |
          set -euo pipefail
          FILTER='issue_2113'
          # T1-T5 (portable) + T6 (Linux-gated) = 6 here. The Windows leg
          # compiles T7 and T8 instead of T6 and so reports 7. Update in the
          # same commit that adds or removes an issue_2113 test.
          EXPECTED=6
          cargo test --locked --lib "$FILTER" -- --test-threads=1 --nocapture 2>&1 | tee test-2113.log

          # Three guards, matching the #1842 step, none redundant:
          #   1. pipefail (LOAD-BEARING through the tee) catches a test that
          #      ran and FAILED.
          #   2. The sentinel grep is the POSITIVE CONTROL: a filter matching
          #      nothing exits 0 and prints "0 passed". This test is
          #      `cfg(target_os = "linux")` and cannot exist unless it really
          #      compiled here.
          #   3. The anchored count is the MUTATION PROBE: gating a test out
          #      moves the number.
          grep -qF 'issue_2113_canonical_gate_separates_two_case_distinct_linux_dirs' test-2113.log || {
            echo "::error::the Linux-only #2113 canonical-gate control never ran; it was renamed or gated out, and this step was testing nothing."
            exit 1
          }
          grep -qE "^test result: ok\. ${EXPECTED} passed; 0 failed" test-2113.log || {
            echo "::error::expected exactly ${EXPECTED} passing ${FILTER} tests; the count changed or the filter matched something else."
            exit 1
          }
```

## 5. Required behavior and edge cases

1. Case-differing project segment, same real directory: candidate is found; `canonical_cwd_owned_by_replica` confirms it; the notice is injected. This is the fix, and the case in the issue.
2. Case difference in the agent segment (`__agent_Bob` vs `__agent_bob`), or in the room segment after its prefix (`room-1-Team` vs `room-1-team`): also fixed. The helper folds the whole FQN, and both of those shapes still satisfy `has_entity_prefix`, so both still produce a project-qualified FQN that folding can align. A case difference in the `room-`/`wg-` prefix letters is NOT fixed and is out of scope; section 3 states why, and no claim of "identical behavior" is made for it.
3. Exact match: unchanged, and short-circuits before any allocation.
4. Different agent or different room: the folded strings still differ; no candidate. Unchanged.
5. Linux, two real directories differing only in case: both pass the pre-filter, exactly one passes the canonical gate. The other is dropped at `:8241`. No cross-delivery. T6 proves this and A7 makes T6 execute.
6. A session whose CWD cannot be canonicalized (deleted, permission denied): `canonical_cwd_owned_by_replica` returns `Err`; at `:8241` `.unwrap_or(false)` excludes it, unchanged.
7. Root-agent sessions, sessions without an `agent_id`, and shells that do not need an explicit Enter are still excluded by the other three conjuncts, which are untouched.
8. The exited-orchestrator guard cannot now reject a session that the selection step accepted for a case reason alone. Before this change that combination was unreachable only because both compares were equally strict; it stays unreachable because both use the same predicate.

## 6. Failure behavior

- No new error type, no new log line, no change to any error string. The helper cannot fail; it returns `bool`.
- If the canonical gate rejects every widened candidate, behavior is byte-identical to today: no live candidate, background spawn, existing dedup handling.
- Performance: two extra `String` allocations per session per notice (both sides are lowercased), only when the exact compare misses. Notices are rare and session counts are small.

## 7. Tests and acceptance criteria

All tests go in the single `mod tests` at `src-tauri/src/phone/mailbox.rs:12822`. Append them at the END of that module, at module level. Do NOT nest a `#[test] fn` inside another `fn`: a nested test is silently dropped from the harness and the suite still passes.

T1 `issue_2113_prefilter_matches_case_differing_project_segment` — assert BOTH halves in one test:

- `crate::config::teams::agent_fqn_from_path("D:/0_repos/Proj/.ac/room-1-team/__agent_bob") != "proj:room-1-team/bob"` (documents that the old predicate drops it),
- `internal_target_fqn_prefilter_matches("D:/0_repos/Proj/.ac/room-1-team/__agent_bob", "proj:room-1-team/bob")` is `true`.

T2 `issue_2113_prefilter_matches_case_differing_room_suffix_and_agent_segments` — `"D:/0_repos/proj/.ac/room-1-Team/__agent_Bob"` against `"proj:room-1-team/bob"` is `true`. The room segment keeps its lowercase `room-` prefix on purpose: that is what `has_entity_prefix` requires, and it is the shape this fix covers. Also assert, in the same test, the boundary that is NOT covered: `internal_target_fqn_prefilter_matches("D:/0_repos/proj/.ac/Room-1-Team/__agent_Bob", "proj:room-1-team/bob")` is `false`, because `agent_fqn_from_path` returns `"Room-1-Team/Bob"` there. This pins the documented out-of-scope gap as a decided behavior rather than an accident.

T3 `issue_2113_prefilter_still_rejects_a_different_agent` — the T1 CWD against `"proj:room-1-team/alice"` is `false`; and against `"other:room-1-team/bob"` is `false`.

T4 `issue_2113_prefilter_matches_exactly_spelled_fqn` — exact equality still `true` (no regression from the fast path).

T5 `issue_2113_prefilter_folds_non_ascii_segments` — `"D:/0_repos/proj/.ac/room-1-team/__agent_ÑOÑO"` against `"proj:room-1-team/ñoño"` is `true`. This is the executable answer to "case folding must be correct for non-ASCII"; `eq_ignore_ascii_case` would return `false` here.

T6 `issue_2113_canonical_gate_separates_two_case_distinct_linux_dirs` — `#[cfg(target_os = "linux")]`. NOT `cfg(unix)`: macOS is unix and case-insensitive by default, where the two directories below collapse into one and the test would be false. Positive control for the stated cross-OS risk. With `tempfile::tempdir()`, canonicalize the temp root first (same reason as `make_mailbox_fixture` at `:14155`), then create `<t>/proj/.ac/room-1-team/__agent_bob` and `<t>/proj/.ac/room-1-team/__agent_Bob`. Assert first that the two canonicalize to DIFFERENT paths — a tripwire so a case-insensitive host fails loudly instead of passing vacuously. Let `replica` be the canonicalized `__agent_bob`, passed through `crate::path_utils::normalize_windows_verbatim_path_buf`, matching how `InternalSystemTarget` builds `replica_dir` so the comparison at `mailbox.rs:561` sees the same shape. Then assert:

- the pre-filter matches BOTH CWDs against `"proj:room-1-team/bob"` (it is over-wide, as designed), and
- `canonical_cwd_owned_by_replica(<__agent_bob path>, &replica).unwrap()` is `true` while `canonical_cwd_owned_by_replica(<__agent_Bob path>, &replica).unwrap()` is `false`.

Runs in CI only because of A7.

T7 `issue_2113_case_differing_project_segment_still_injects_live` — `#[cfg(windows)]`, `#[tokio::test]`. **This is the only test that exercises the changed call sites, and the only one that fails on revert.** Windows-gated because the whole point is one real directory reachable by two spellings, which needs a case-insensitive filesystem; the full suite is Windows-only anyway (`pr-regression-gates.yml:94-96`). Model it on `internal_live_delivery_uses_exact_payload_guard_and_system_bookkeeping` (`mailbox.rs:15923`), which already proves this harness reaches `deliver_internal_system_notice` end to end and that `add_mailbox_session` stores the CWD verbatim.

- `let fixture = make_mailbox_fixture();` `let app = app_handle(&fixture.app);`
- Build the case-skewed CWD from components, not by string search: take `fixture.sender_cwd`'s three ancestors to reach the project dir (`.../proj-a`), then `project.parent().unwrap().join("Proj-A").join(".ac").join("wg-1-dev-team").join("__agent_tech-lead")`. Do NOT `create_dir_all` it; it must be the SAME directory, differently spelled.
- Tripwire, before anything else: `std::fs::canonicalize(&skewed).unwrap()` equals `std::fs::canonicalize(&fixture.sender_cwd).unwrap()`. If the host filesystem is case-sensitive, this fails loudly rather than the test passing for the wrong reason.
- Register the session with the SKEWED spelling: `add_mailbox_session(&app, &skewed, "wg-1-dev-team/tech-lead", SessionStatus::Running, None).await`.
- Build the target from the canonical spelling: `InternalSystemTarget::for_context_alert(CANONICAL_WAKE_FROM.to_string(), fixture.sender_cwd.clone()).unwrap()`.
- Anti-tautology anchor, asserted inside this test: `crate::config::teams::agent_fqn_from_path(&skewed.to_string_lossy()) != target.fqn()`. This is exactly the compare the two call sites used to make, so the test states in code why it must fail on revert.
- `hooks.pty_presence.lock().unwrap().insert(id, true);` then `poller.deliver_internal_system_notice(&app, target, notice, CancellationToken::new(), Arc::new(|| Ok(()))).await.unwrap();` with the same `InternalSystemNotice::for_context_alert` arguments as the model test.
- Assert `hooks.inject_calls.lock().unwrap().as_slice() == &[id]` and `assert_no_spawn_or_destroy_events(&hooks)`.

T8 `issue_2113_case_differing_project_segment_destroys_exited_orchestrator` - `#[cfg(windows)]`, `#[tokio::test]`. **This is the control for the `mailbox.rs:8327` edit, which T7 cannot reach.** T7 registers a `Running` session, so it drives only `find_internal_system_candidates` (`:8233`); `recheck_internal_exited_record` (`:8327`) runs only on the `Exited` branch (`:7958-7975`). Without T8 every criterion would stay green with `:8327` left raw, while a case-skewed EXITED orchestrator would still be rejected with `Err("Orchestrator session ... restarted or changed immediately before destruction")`, the same dropped-notice class this issue is about. Model it on `internal_exited_recipient_is_destroyed_then_resumed_without_selection_spawn` (`mailbox.rs:16099`), which already asserts the Destroy, Spawn, Inject event order.

- Build `skewed` exactly as in T7, and assert the same canonicalize tripwire first.
- `let exited_id = add_mailbox_session(&app, &skewed, "exited-coordinator", SessionStatus::Exited(0), None).await;`, keeping the model test's status and label.
- Same target, notice, hooks and `deliver_internal_system_notice(...).await.unwrap()` as the model test. No `pty_presence` entry: the exited branch does not need one.
- Assert `destroy_calls.lock().unwrap().as_slice() == &[exited_id]`, `spawn_calls.lock().unwrap().len() == 1`, and the `events` order `Destroy(exited_id)`, `Spawn(_)`, `Inject(_)`, as the model test does.
- On revert this fails two different ways, and both are wanted: with only `:8327` reverted, the `.unwrap()` panics on the `restarted or changed immediately before destruction` error; with only `:8233` reverted, the exited candidate is never selected, `destroy_calls` is empty and the first assertion fails.

Acceptance criteria, all objective:

- A1 Per-OS test deltas, stated separately because no single total is true on both. **Windows** (this box and the `rust-regression` job): `cargo test --locked --lib phone::mailbox` passes and its reported total is exactly the pre-change total plus 7 (T1-T5, T7, T8; T6 is not compiled). Record both numbers in the PR body. **Linux**: the suite does not run at all; the only execution is the focused step in A7, which asserts exactly 6 (T1-T5, T6; T7 and T8 are not compiled). A smaller delta on either leg means a test was silently dropped.
- A2 Anti-tautology controls, with the exact predicted failure of each, and an honest statement of what is not a control. Revert BOTH call-site edits while keeping the helper and all tests, then run `cargo test --locked --lib phone::mailbox` on Windows. **T7 and T8 must both fail.** Verify by hand once before opening the PR and quote both observed failures in the PR body.
  - **T7's predicted signature, stated precisely because the obvious guess is wrong.** Delivery does NOT stop when no live candidate is found: the background-spawn path spawns a session and then injects into it (`mailbox.rs:8151`). `internal_same_fqn_at_another_root_is_not_injected_and_exact_target_spawns` (`mailbox.rs:15991`) pins that today, asserting one `spawn_calls` entry AND `inject_calls[0] != wrong_id`. So on revert `deliver_internal_system_notice` still returns `Ok`, `inject_calls` is `[<spawned session id>]`, one entry and NOT empty, and `spawn_calls` has one entry. T7 fails at `assert_eq!(inject_calls.as_slice(), &[id])`, because the injected id is the spawned session's and not the pre-registered skewed session's; `assert_no_spawn_or_destroy_events(&hooks)` fails as well. The PR body must describe that failure, not an empty `inject_calls`.
  - **T8's predicted signature:** with both edits reverted the exited candidate is not selected either, so `destroy_calls` is empty and T8 fails at its first assertion. Reverting `:8327` alone instead makes the `.unwrap()` panic on `Orchestrator session ... restarted or changed immediately before destruction`; that single-edit revert is what proves `:8327` is load-bearing, and it is worth running separately.
  - Stated plainly so no reviewer has to rediscover it: **T1-T6 do NOT fail on that revert.** They assert the helper and the canonical gate directly, both of which survive the revert untouched; they are unit evidence for the helper's contract, not controls on the call sites. T7 and T8 are the controls, one per call site, which is why neither is optional.
- A3 `cargo clippy --locked --workspace --all-targets -- -D warnings` is clean.
- A4 `cargo fmt --check` is clean. Format only the touched file; a repo-wide `cargo fmt` would enlarge the diff.
- A5 `git diff --stat 20053458f3847fa6ca4ad8783133df8f5e134e0a..HEAD` (`..HEAD`, not a bare working-tree diff: `plans/` is gitignored, so the plan file is only visible once committed with `git add -f`) names exactly three paths: `src-tauri/src/phone/mailbox.rs`, `.github/workflows/pr-regression-gates.yml` and `plans/issue-2113-case-tolerant-internal-notice-match.md`. No lockfile, no other source file, no other workflow. The workflow is in the set only for the single added step in section 4; touching any other job, step or line in it breaks this criterion.
- A6 No new module-to-module arc: every symbol touched is already referenced from `phone/mailbox.rs` at the base. Dependency-cycle state is unchanged by construction; no baseline run is required, and the PR body states this reason.
- A7 The new `rust-regression-linux` step is green on the PR head, and its log shows the `issue_2113_canonical_gate_separates_two_case_distinct_linux_dirs` identifier and `test result: ok. 6 passed; 0 failed`. A green job without both lines in the log does not satisfy this criterion.

## 8. Delivery gates (baseline, per `delivery-nonfunctional-invariants`)

1. CI-to-plan parity. Triggered jobs at this base: `test-debt`, `rust-regression` (windows-latest), `rust-regression-linux` (ubuntu-latest), plus `Lockfile drift check` and `validate-branch-name`. `cargo check --locked --all-targets` and `cargo clippy --locked --workspace --all-targets -- -D warnings` run on both OS jobs; `cargo test --locked --lib --bins --tests` runs on the Windows job ONLY (`pr-regression-gates.yml:94-96`), and the Linux job runs focused filters by name, which is why A7 exists. Owner: CI, at PR time. Acceptance: every triggered and configured-required check green on the EXACT PR-head SHA. Evidence from any other SHA does not count.
2. Deterministic toolchain. Repository-pinned Rust toolchain and `Cargo.lock`; every command uses `--locked`. Working directory for all cargo commands: `src-tauri`. No dependency is added (`tempfile` is already a dev-dependency, used by `make_mailbox_fixture`), so `Cargo.lock` must not change (A5 asserts this).
3. Authorized, traceable Git. Issue #2113 is open; branch `fix/2113-case-tolerant-internal-notice-match` already exists from the pinned base. State-changing Git runs only inside `repo-AgentsCommander`. Delivery by PR to `main`; direct push to `main` is forbidden. `plans/` is gitignored, so the plan file needs `git add -f`; confirm `git show --stat HEAD` lists it after committing.
4. Process state and working directory. Cargo target dir is the repo root, not `src-tauri/target`; a cold-looking `src-tauri/target` is expected and is not a reason to rebuild. No environment override is needed.
5. Validation and scope. Frozen path set is the three paths in A5. Run A1 (Windows leg), A2 (both reverts), A3 and A4 locally before the PR, then A5. A7 can only be observed in CI; do not claim it from a local run.
6. Mutation ownership and recovery. Two small hand edits plus an appended test block in one source file, and one appended step in one workflow. Recovery: `git diff` the two files and revert by hand or `git checkout -- <path>`. No `git reset`, no repo-wide clean.
7. Bounded execution. The cargo commands are the repository's own and are bounded by the CI job timeouts; the new step carries `timeout-minutes: 10`, matching the #1938 step beside it. Keep the local `cargo test` output; a timed-out or failed run is never reported as a pass.
8. Evidence discipline. A1 binds the zero case per OS (a dropped test reads as a smaller delta, not as a pass). A2 names one reachable control per call site, states each one's exact predicted revert signature, and states in writing which tests are NOT controls. A7's identifier grep is the positive control for a filter that selected nothing. A6 states, with its reason, why the cycle check is a proven non-applicability rather than a skipped one.

## 9. Open decisions

None. Every choice in sections 3, 4 and 7 is decided; the implementer picks nothing.

## 10. Round-2 changes

- **B1, the room-segment claim.** Round 1 claimed "identical behavior" for a case difference in the room segment, and T2 used `Room-1-Team`, which cannot pass. Verified at the base: `has_entity_prefix` is case-sensitive (`entity_prefix.rs:13-21`, `:30`, test at `:48`), `teams.rs:89` gates on it, `:98` falls back to `agent_name_from_path` (`:43`), which yields `Room-1-Team/Bob` with no project prefix. **Decision: narrow the scope.** The prefix-letter skew is named out of scope in section 3 with its reason; 5.2 now states exactly what is and is not covered; T2 uses `room-1-Team`, which reaches the prefix path, and additionally asserts the uncovered `Room-1-Team` shape returns `false`, so the gap is pinned rather than left open.
- **B2, A2 was not a control.** Confirmed: no test at the base reaches `:8233` or `:8327`, and T1 passes on revert because the helper survives it. **Decision: add T7**, an end-to-end `deliver_internal_system_notice` test on the `mailbox.rs:15923` harness with a case-differing CWD, which runs on this box and on the Windows CI leg and fails on revert. A2 is rewritten around T7 and states in writing that T1-T6 are not controls.
- **B3, the total pin and T6.** Confirmed: `rust-regression-linux` runs no suite, only named filters, and its own comment (`:723-725`) records that `cfg(unix)` tests have never executed. A single `+6` total was false on Linux and T6 ran nowhere. **Decision: take the Linux-step branch.** Section 4 adds one focused `issue_2113` step to `rust-regression-linux`, modelled on the #1842 step; A5 now allows the workflow as a third path and bounds the edit to that one step; A1 states per-OS deltas; T6 is re-gated to `target_os = "linux"` and gains a case-sensitivity tripwire; A7 is the new criterion for the step's log.

## 11. Round-3 changes

- **G1, A2's predicted revert signature was wrong.** Verified at the base: the post-spawn branch injects into the freshly spawned session (`mailbox.rs:8151`), and `internal_same_fqn_at_another_root_is_not_injected_and_exact_target_spawns` (`mailbox.rs:15991`) already asserts one spawn plus a non-empty `inject_calls`. Round 2 predicted an empty `inject_calls`, which is false. **Decision: correct the prediction, keep T7.** A2 now states the failure that will actually occur, `inject_calls == [<spawned id>]` with one spawn call and failure at the `&[id]` assertion, and instructs the PR body to quote that.
- **G2, the `:8327` edit had no control.** Confirmed: T7 registers a `Running` session and reaches only `:8233`; `recheck_internal_exited_record` runs only on the `Exited` branch (`:7958-7975`), so every round-2 criterion stayed green with `:8327` left raw while a case-skewed exited orchestrator was still rejected. **Decision: add the control rather than declare the gap.** T8 registers a skewed-CWD `Exited` session on the `mailbox.rs:16099` harness and asserts Destroy, Spawn, Inject; A1's Windows delta becomes 7, the CI step comment records that the Windows leg reports 7 while the Linux step still expects 6, and A2 gains T8's signature for both the full revert and the `:8327`-only revert.
- **Three non-blocking corrections, both reviewers.** The `teams.rs` gate and fallback are `:89` and `:98`, not `:86` and `:97` (two places). A5's diff is now `<base>..HEAD`, with the gitignore reason inline. Section 6 says two `String` allocations per non-matching session, not one.
