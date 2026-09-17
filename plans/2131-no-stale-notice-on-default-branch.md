# Plan #2131: no branch-stale notice on the default branch, and the orchestrator row's CI working tint

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2131 (OPEN)
- Repo: `repo-AgentsCommander`; branch `fix/2131-no-stale-notice-on-default-branch`
- Base (frozen at authoring, 2026-09-17 UTC): HEAD =
  `20053458f3847fa6ca4ad8783133df8f5e134e0a` (`git rev-parse HEAD`), tracked tree clean.
  Every line number below refers to that SHA; if a quoted line no longer matches, re-anchor on
  the quoted text, never on the number.
- Class: Lite (band 1-25), one phase, no partition. Requirement A author `ac-dev-rust-v4` (sections
  1-11), reviewer Grinch; requirement B author `ac-dev-webpage-ui-v4` (sections 12-22); coordinator
  `ac-tech-lead-v4`.
- Canonical plan: this file. Root `.gitignore:11` ignores `/plans/`, so the implementation commit
  carries it with `git add -f plans/2131-no-stale-notice-on-default-branch.md`.
- Scope, requirement A (sections 1-11): one production file
  (`src-tauri/src/pty/remote_watcher.rs`): a guard plus in-file tests. Requirement A alone changes
  no frontend, IPC shape, settings, dependency, workflow or migration.
- Scope, requirement B (sections 12-22, the user's "Added scope (user, 2026-09-17)" comment): one
  production file (`src/sidebar/components/ProjectPanel.tsx`): one row-tint predicate, plus tests
  in one existing test file. Requirement B alone changes no backend, CSS, IPC shape, settings,
  dependency, workflow or migration.
- The two requirements are independent: neither is a prerequisite for the other, and each has its
  own acceptance criteria (section 8 for A, section 19 for B). Both land on this branch as one
  deliverable (one PR).

## 1. Problem and verified cause

Reported: a room orchestrator whose repo simply sits on `main` and has not pulled receives the
branch-stale notice. The agent has no branch of its own on that repo, so the notice is noise, and
it reaches every agent in that state. Real case on 2026-09-17: an orchestrator on `main` at
`27ff72e3` received

```
[AgentsCommander] mblua/AgentsCommander main is now 4 commits behind main as of 2026-09-17 15:42:21-03:00. Any CI running on this branch is validating an out-of-date base.
```

The count was correct (`origin/main` had advanced by four commits, PR #2128); the message had no
value because there is no branch of that agent's own to rebase.

Verified cause at the frozen base:

- `apply_staleness` is `src-tauri/src/pty/remote_watcher.rs:1186-1244`. The notice fires on any
  `Current` -> `Stale` transition, with no branch comparison: `let stale = matches!(...)` at
  `:1217-1220` (`(Some(StalenessState::Current), StalenessState::Stale)`), and
  `if stale { self.fan_out(...) }` at `:1221-1233` with `kind: TransitionKind::BranchStale`
  (`:1225`).
- `TransitionKind::BranchStale` is created at no other production site (the enum is `:112-116`;
  the only other occurrences are the tests and the `remote_alerts`/`mailbox` consumers, which read
  the transition, they do not create it).
- The query unit already carries the repo's current branch: `QueryKey { nwo, sha40, branch }` at
  `:524-528`. The branch comes from the published git snapshot
  (`read_local_facts` `:1011-1012` via `git_watcher::read_git_status`), so it is a short branch
  name (`main`, `fix/2131`), never a ref or a remote-tracking name.
- The repo's default branch is already resolved in this same round and cached per `nwo`:
  `state.base_branches` is read at `:1197-1201` for the notice text, and
  `resolve_base_branch` (`:1067-1079`) always stores either a resolved branch name or the sentinel
  `DEFAULT_BRANCH_LABEL` (`:56`, the literal `"the default branch"`).
- #2126 is the in-file precedent for the unresolved case: the CI rule filters the same cache with
  `.filter(|label| label.as_str() != DEFAULT_BRANCH_LABEL)` at `:906` before treating it as an
  identity. #2131 mirrors that precedent for the staleness notice.

## 2. Decisions (fixed by the user, not open)

- D1: the branch-stale notice is sent only when the repo's current branch is not the repository's
  default branch.
- D2: on the default branch there is no notice, and the chip keeps its orange bar. Chip state must
  not change.
- D3: when the default branch cannot be resolved, fail open: do not suppress.

## 3. Solution (decided)

D4 - Add one guard at the single `BranchStale` fan-out site, `apply_staleness`:

```rust
        let base_label = state
            .base_branches
            .get(&key.nwo)
            .cloned()
            .unwrap_or_else(|| DEFAULT_BRANCH_LABEL.to_string());
        // #2131: a repo sitting on its own default branch has no branch of its
        // own to rebase, so the notice is suppressed there while the chip keeps
        // its orange bar. Fail open: the sentinel label means the default branch
        // is unresolved, which is not an identity and never suppresses (the same
        // rule #2126 applies to CI).
        let on_default_branch = base_label != DEFAULT_BRANCH_LABEL && key.branch == base_label;
        let entry = state.keys.entry(key.clone()).or_default();
```

and change `if stale {` at `:1221` to `if stale && !on_default_branch {`.

D5 - The comparison identity is the resolved cache value, exactly like #2126:

- `base_label` is either a branch name returned by step 1 (GitHub `default_branch`) or step 2
  (local `origin/HEAD`, `origin/` stripped, `:1101-1116`) or the sentinel from step 3.
- Equality is exact byte equality between `key.branch` and `base_label`, matching the CI rule
  (`:1392-1394`, `key.branch != default`) and the CI branch filter (`parse_ci_response`, exact match).
  GitHub branch names are case-sensitive, so `Main` and `main` are different branches.
- `key.branch` is the right operand: all paths folded into one `QueryKey` share the branch, because
  the grouping at `:825-836` skips any path whose published branch is `None` and keys on
  `(nwo, sha40, branch)`. `fan_out` writes that same branch into the transition (`:1254`,
  `fact.branch`). One decision per key therefore covers every room in the group consistently.

D6 - The unresolved case fails open on the sentinel check alone: `base_label != DEFAULT_BRANCH_LABEL`
is false when resolution failed, so `on_default_branch` is false and the notice is sent with
`base_branch: DEFAULT_BRANCH_LABEL`, which is what `%BASE%` renders. The `unwrap_or_else` at
`:1201` folds a missing cache entry into the same sentinel, so an absent entry also fails open. The
sentinel can never collide with a real branch: git refnames cannot contain spaces.

D7 - The chip path is untouched. Lines `:1209-1214` (chip, confirmed, behind_by, last_confirmed_at,
failure_interval, next_due) keep running before the guard: the chip still becomes `Stale` with the
correct `behind_by`, the payload change gate still emits the new `stalenessStates`, and the
frontend keeps rendering the existing `stale` class (`src/sidebar/components/ProjectPanel.tsx:396-406`).
No frontend edit is needed or allowed for requirement A. (Requirement B, in Part B, makes an
independent `ProjectPanel.tsx` change: it touches the row's `working` tint, never this chip path,
and it changes no state `apply_staleness` publishes.)

D8 - The guard is placed in the producer, not in `fan_out` and not in the consumer
(`session/remote_alerts.rs`, `phone/mailbox.rs`). `fan_out` is shared with the CI transitions;
the consumers do not know whether the label was resolved and would have to re-derive the sentinel
rule, hiding a producer-side decision behind the channel.

Closed alternatives:

- (a) Reuse the compare answer's `identical` flag: rejected. GitHub reports `behind`, not
  `identical`, for a local `main` that has not pulled (`parse_compare_response`, `:426-430`), which
  is exactly the reported case; the flag cannot answer "am I on the default branch".
- (b) Skip the staleness query or the chip update on the default branch: rejected by D2. The chip
  must keep its orange bar, so the query still runs and the state still advances.
- (c) Suppress in `remote_alerts`/`mailbox`: rejected, see D8.
- (d) Compare against the local `origin/HEAD` directly in `apply_staleness`: rejected. It would add
  a git call per round inside the locked path, and it would disagree with the label the notice
  prints, which comes from the shared cache.

## 4. In scope / out of scope

In scope:

- `src-tauri/src/pty/remote_watcher.rs`: the `apply_staleness` guard and its in-file tests.

Out of scope (no change, no new file, no new dependency):

- The CI axis and its identity suppression (already correct, #2126).
- The chip colours, tooltips, TS types (`src/shared/types.ts`), and any `.ts`/`.tsx`/CSS file.
  (These are requirement A's boundaries. Requirement B, in Part B below, changes
  `src/sidebar/components/ProjectPanel.tsx` and one test file; it still changes no CSS, no
  `src/shared/types.ts`, no store and no IPC.)
- The notice text, `%BASE%` rendering, dials/backoff, notice dedup in `remote_alerts`/`mailbox`.
- The three-step label resolution chain, its caching, and the `DEFAULT_BRANCH_LABEL` comment at
  `:52-56` (it already states fail-open: "A `BranchStale` notice is NEVER suppressed for want of
  it" - that rule stays true and is what this plan implements).
- Settings, persistence, IPC payload shape, workflows, release artifacts.

## 5. Exact changes

### 5.1 Production: `apply_staleness`, `src-tauri/src/pty/remote_watcher.rs`

Current `:1197-1202`:

```rust
        let base_label = state
            .base_branches
            .get(&key.nwo)
            .cloned()
            .unwrap_or_else(|| DEFAULT_BRANCH_LABEL.to_string());
        let entry = state.keys.entry(key.clone()).or_default();
```

Replace with:

```rust
        let base_label = state
            .base_branches
            .get(&key.nwo)
            .cloned()
            .unwrap_or_else(|| DEFAULT_BRANCH_LABEL.to_string());
        // #2131: a repo sitting on its own default branch has no branch of its
        // own to rebase, so the notice is suppressed there while the chip keeps
        // its orange bar. Fail open: the sentinel label means the default branch
        // is unresolved, which is not an identity and never suppresses (the same
        // rule #2126 applies to CI).
        let on_default_branch = base_label != DEFAULT_BRANCH_LABEL && key.branch == base_label;
        let entry = state.keys.entry(key.clone()).or_default();
```

Current `:1221`:

```rust
                if stale {
```

Replace with:

```rust
                if stale && !on_default_branch {
```

Nothing else changes in the function: `base_label` is still moved into
`TransitionMeta { base_branch: base_label, ... }` on the not-suppressed path, and no import is
added (`DEFAULT_BRANCH_LABEL` is already in scope).

### 5.2 Tests: same file, `#[cfg(test)] mod tests`

- Three new tests, inserted immediately after `current_to_stale_emits_once_only` (currently ends at
  `:2618`, before `fn drain` at `:2620`), bodies in section 7.
- Two existing tests updated so their setup is not the newly suppressed case, bodies in section 7.4:
  `current_to_stale_emits_once_only` (`:2588`) and step 1 of
  `base_branch_resolution_walks_the_three_step_chain` (`:3224-3251`).

## 6. Required behaviour, edge cases, failure behaviour

| Case | Chip (`RemoteActivity.staleness`) | Notice |
|---|---|---|
| Resolved base == `key.branch` | `Stale`, `behind_by` published as today | none, ever |
| Resolved base != `key.branch` (any spelling/case difference, e.g. `main-2`) | `Stale` | exactly one `BranchStale` per `Current` -> `Stale`, `base_branch` = resolved name |
| Base unresolved (sentinel value or missing cache entry; gh and `origin/HEAD` both failed) | `Stale` | exactly one `BranchStale`, `base_branch` = `DEFAULT_BRANCH_LABEL` |
| Path with no published branch / detached / unborn | `Unknown` (path is not grouped at all, `:825-828`) | none, unchanged |
| Prior state not `Current` (cold start, `Unknown`, already `Stale`) | unchanged | none, unchanged (`:1217-1220` untouched) |
| Query failure (`Err(kind)`) | `Unknown` + backoff, unchanged | none, unchanged |
| Two paths sharing one `(nwo, sha40, branch)` key | same for both | one decision: both paths' rooms are suppressed or notified together |

Edge cases and failure behaviour:

- The suppression skips only the `fan_out` call. `next_due`, `last_confirmed_at`,
  `failure_interval = None` and `confirmed` all still advance on the suppressed round, so the
  state machine, the once-only `Current` -> `Stale` edge and the payload emission are unchanged.
- No new failure mode, I/O, lock scope, `await`, or `unwrap` is introduced: the guard is a boolean
  over two `String`s already held while `state` is locked, and `drop(state)` still happens before
  `fan_out`. A suppression can never mask a chip update, because the chip update runs first.
- The label cache is per `nwo` and process-lifetime, so the decision is per repository: a failed
  resolution for one repo cannot suppress another repo's notice.
- If the default branch is renamed mid-process the cache may hold the old name, and the guard then
  compares against the old name; that is the same name the notice would print, pre-existing, and
  out of scope.

## 7. Tests (exact)

All three new tests are `#[tokio::test]` and start with `let _guard = round_test_lock().await;`
(the round-driving tests share process-global state and must not run concurrently with each other).

### 7.1 (a) positive control: default branch, no notice, chip still `Stale`

```rust
    #[tokio::test]
    async fn stale_on_default_branch_sends_no_notice_but_keeps_the_chip() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        assert!(
            harness.drain_transitions().is_empty(),
            "the default branch has no branch of its own to rebase: no notice"
        );
        let snapshot = harness.snapshot();
        let activity = snapshot.get(&repo).expect("entry");
        assert_eq!(
            activity.staleness,
            StalenessState::Stale,
            "the chip keeps its orange bar"
        );
        assert_eq!(activity.behind_by, Some(4));
    }
```

Round 1 answers `identical` (chip `Current`, no transition), round 2 answers `behind 4`; the query
ran and the chip moved, so the empty transition list proves suppression, not a dead detector.
`Harness::repo` publishes branch `main`, and the queued `repo_info` resolves the base label to
`main`, so the repo's branch is the default one.

### 7.2 (b) positive control: non-default branch still notifies

```rust
    #[tokio::test]
    async fn stale_on_a_feature_branch_still_sends_the_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2131".to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            gh.repo_info
                .push_back(Ok(ok_output(r#"{"default_branch":"main"}"#)));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(transitions.len(), 1, "a feature branch still notifies");
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].base_branch, "main");
        assert_eq!(transitions[0].behind_by, Some(4));
        assert_eq!(
            harness.snapshot().get(&repo).expect("entry").staleness,
            StalenessState::Stale
        );
    }
```

### 7.3 (c) positive control: unresolved default branch still notifies

```rust
    #[tokio::test]
    async fn stale_with_an_unresolved_default_branch_still_sends_the_notice() {
        let _guard = round_test_lock().await;
        let harness = Harness::new(AppSettings::default());
        let repo = harness.repo("repo-a");
        // Overwrite `Harness::repo`'s `main` with the sentinel literal itself: an
        // unresolved base and a branch equal to that base are the one pair a bare
        // `key.branch == base_label` comparison would suppress, so this test fails
        // if `base_label != DEFAULT_BRANCH_LABEL &&` is removed. Production cannot
        // reach this pair (git refnames forbid spaces), which is why publishing it
        // is safe here and why the sentinel guard is the only thing under test.
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some(DEFAULT_BRANCH_LABEL.to_string()),
                dirty: false,
            }),
        );
        harness.set_work(std::slice::from_ref(&repo));
        {
            let mut gh = harness.gh.lock().unwrap_or_else(|e| e.into_inner());
            // No repo_info and no symbolic-ref answer is queued, so the default
            // branch stays unresolved and the sentinel is the base label; the
            // published branch equals that sentinel, so the sentinel guard is the
            // only thing keeping the notice alive.
            gh.compare
                .push_back(Ok(ok_output(&compare_body("identical", 0))));
            gh.compare
                .push_back(Ok(ok_output(&compare_body("behind", 4))));
        }

        let mut now = Instant::now();
        let mut wall = Local::now();
        harness.round(now, wall).await;
        drain(&harness);
        tick(&mut now, &mut wall, 300);
        harness.round(now, wall).await;

        let transitions = harness.drain_transitions();
        assert_eq!(
            transitions.len(),
            1,
            "an unresolved default branch cannot suppress"
        );
        assert_eq!(transitions[0].kind, TransitionKind::BranchStale);
        assert_eq!(transitions[0].base_branch, DEFAULT_BRANCH_LABEL);
        // Companion premise assertion: the branch this notice was produced for is
        // the sentinel itself, so the test cannot pass on a live `main` by accident.
        assert_eq!(transitions[0].branch, DEFAULT_BRANCH_LABEL);
        assert_eq!(transitions[0].behind_by, Some(4));
    }
```

The published branch is the sentinel string itself and the base is unresolved, so branch and base
label are equal: that is exactly the pair a comparison without the sentinel guard would suppress.
Delete `base_label != DEFAULT_BRANCH_LABEL &&` and this test fails, because the notice it asserts
would not be sent. The companion `transitions[0].branch` assertion pins the premise - the case
really is sentinel-equal-branch, not a live `main` - and it is why no second case is needed.
Production cannot hit this case: git refnames forbid spaces, so the sentinel can never collide with
a real branch name; the test publishes the impossible pair on purpose, and the guard exists so that
an unresolved label is never mistaken for an identity.

### 7.4 Existing tests updated

1. `current_to_stale_emits_once_only` (`:2588-2618`) - its setup is now the suppressed case
   (branch `main`, base `main`). Insert after `let repo = harness.repo("repo-a");`:

```rust
        crate::pty::git_watcher::publish_git_status(
            &repo,
            Some(crate::pty::git_watcher::GitStatus {
                branch: Some("fix/2131".to_string()),
                dirty: false,
            }),
        );
```

   Queue and assertions stay byte-identical: it keeps testing the once-only `Current` -> `Stale`
   dedup, now on a branch where the notice is legal.

2. `base_branch_resolution_walks_the_three_step_chain` (`:3224`), step 1 block only
   (`:3227-3251`) - same insertion after its `let repo = harness.repo("repo-a");`, so the
   `assert_eq!(transitions[0].base_branch, "main")` assertion keeps its meaning. Step 2 (base
   `develop`, branch `main`) and step 3 (base sentinel, branch `main`) already sit on the
   non-suppressed side and stay unchanged.

No other test in the file asserts a `BranchStale` transition on the default branch: the remaining
`compare_body("behind", ...)` call sites are `stale_to_current_emits_nothing` (`:2572`, no
notice expected), `base_branch_is_resolved_once_per_nwo_and_cached` (`:3316`, snapshot-only, base
unresolved) and the two blocks above.

### 7.5 Existing in-file helpers reused (no new helper, no new test file)

- `round_test_lock()` (`:1486`) - the shared async round lock.
- `Harness::new` (`:1663`), `Harness::repo` (`:1731`), `Harness::set_work` (`:1745`),
  `Harness::round` (`:1755`), `Harness::snapshot` (`:1763`), `Harness::drain_transitions`
  (`:1770`).
- `drain(&harness)` (`:2620`), `tick(&mut now, &mut wall, 300)` (`:1799`).
- `ok_output` (`:1494`), `compare_body` (`:1524`).
- `crate::pty::git_watcher::publish_git_status` with `GitStatus { branch, dirty }` (used the same
  way at `:2251-2257`), `AppSettings::default()` (staleness enabled, 300 s dial at
  `config/settings.rs:955-957`).

## 8. Objective acceptance criteria

- A1: the three new tests and the two updated tests pass:
  `cd src-tauri && cargo test --locked --lib pty::remote_watcher -- --test-threads=1` exits 0.
- A2: the default-branch test observes zero transitions in the `Current` -> `Stale` round while
  `snapshot().staleness == Stale` and `behind_by == Some(4)`; that pair is the positive control
  against a broken detector.
- A3: the feature-branch and unresolved-base tests each observe exactly one
  `TransitionKind::BranchStale` with `base_branch == "main"` and `base_branch == DEFAULT_BRANCH_LABEL`
  respectively.
- A4: the full Rust gate stays green before the PR is opened:
  `cd src-tauri && cargo test --locked --lib --bins --tests` exits 0.
- A5: `cargo clippy --locked --workspace --all-targets -- -D warnings` and
  `cargo fmt --all -- --check` both exit 0 (CI gates, `.github/workflows/pr-regression-gates.yml:92`,
  `:2251`).
- A6: requirement A's diff touches exactly one tracked file,
  `src-tauri/src/pty/remote_watcher.rs`, plus the `git add -f` plan file:
  `git diff --name-only -- src-tauri/` lists exactly that file. (A6 is a per-part check; requirement
  B owns the `.tsx` hunks.)
- A7: requirement A introduces no `.ts`, `.tsx`, CSS, settings, or IPC payload change;
  `stalenessStates` still publishes `stale` for a default-branch repo, so the orange bar is
  preserved by the existing frontend path. (Requirement B adds `.tsx` only, so this criterion's
  substance is unaffected by Part B.)

## 9. Proof protocol (for the reviewer)

1. Confirm the two edits against the quoted current text in section 5.1; re-anchor on the quoted
   text, not on line numbers, if the branch has moved.
2. Run the focused command from A1 and capture the tail: it must list
   `stale_on_default_branch_sends_no_notice_but_keeps_the_chip`,
   `stale_on_a_feature_branch_still_sends_the_notice`,
   `stale_with_an_unresolved_default_branch_still_sends_the_notice`,
   `current_to_stale_emits_once_only`,
   `base_branch_resolution_walks_the_three_step_chain`, all `ok`.
3. Re-run clippy and fmt (A5).
4. Diff review: only the guard, the three new tests, and the two test-setup insertions; no change to
   `fan_out`, `apply_ci`, the payload vectors, or any file outside `remote_watcher.rs`.

## 10. Risks and compatibility

- Silent-drop risk (the issue's blast-radius anchor): a wrong guard drops a real notice with no
  error and no visible symptom. Mitigations: two positive controls (b) and (c) prove the detector
  still fires off the default branch and when the label is unresolved; the suppression is fail-open
  by construction; the chip keeps working either way, so the state stays visible on screen.
- False suppression would require `key.branch` to equal a resolved default-branch name; the
  comparison is exact, so a case or prefix difference (`Main`, `main-2`) still notifies.
- No wire-contract change: `RemoteTransition`, `RemoteActivityPayload` and the notice templates are
  untouched, so `remote_alerts`, `mailbox` and the frontend need no coordinated change.
- Round tests remain serialized by `round_test_lock`, so no new flakiness is introduced.

## 11. Implementation order

1. Apply the `apply_staleness` guard (section 5.1).
2. Update the two existing tests (section 7.4), then add the three new tests (sections 7.1-7.3).
3. Run the focused suite, then clippy and fmt (section 8).
4. Commit code + tests, and take the plan with `git add -f plans/2131-no-stale-notice-on-default-branch.md`;
   no push to `main`, no merge; leave the branch for review.

## Plan Contract self-check (requirement A, sections 1-11)

- Issue and objective: section 1.
- Cause with evidence references: section 1 (`:1186-1244`, `:1217-1233`, `:524-528`, `:1197-1201`,
  `:56`, `:906` precedent).
- In scope / out of scope: section 4.
- Decided solution, no TBD, no open choice: sections 2, 3 (D4-D8 plus closed alternatives).
- Exact files and symbols: section 5.
- Required behaviour, edge cases, failure behaviour: section 6.
- Tests with a positive control plus two further controls, and the reused in-file helpers named:
  section 7.
- Objective acceptance criteria: section 8.

# Part B (requirement B) - the orchestrator row's CI working tint

## 12. Requirement B: objective and verified cause

**Objective.** An orchestrator row whose repo chip is in `ci-running` must carry the sidebar's
"someone is working here" wash. CI running on the room's repo is work the room is waiting on; today
the row stays untinted, so the room reads as idle while work is in flight on its behalf. Reported
2026-09-17 on `room-15-ac-dev-team-v4/ac-tech-lead-v4`: the
`AgentsCommander/docs/2080-portable-readme-storage` chip carried the CI border while the row had no
working wash.

**Verified cause at the frozen base - two signals that never meet:**

- Chip: `remoteActivityClasses` (`src/sidebar/components/ProjectPanel.tsx:401-408`) appends
  ` ci-running` when the published entry's `ci === "running"`. The chip `<For>` is `:2658-2670`; the
  border is `.ac-discovery-badge.branch.ci-running` (`src/sidebar/styles/sidebar.css:6887-6889`,
  `inset 0 0 0 1px rgba(234, 179, 8, 0.85)`).
- Row: the `working` entry in `renderReplicaItem`'s `classList` (`ProjectPanel.tsx:2519-2525`) calls
  `rowIsWorking()` (`:2426-2432`): `workgroupIsWorking(wg)` for `quick`, `isReplicaWorking(wg,
  replica)` otherwise. Both (`src/sidebar/components/workgroup-session.ts:27-33`) read only the
  agent's own PTY session through `isSessionWorking`. Remote CI state is never consulted.
- `renderReplicaItem` (`:2415-2701`) is one factory with three row contexts:
  `renderWorkgroupSubgroup(wg, "selected")` (`:2987`) and `renderWorkgroupSubgroup(wg,
  "workgroups")` (`:3047`), both inside `.ac-wg-subgroup`; and the Orchestrators strip
  `renderReplicaItem(..., "quick")` (`:2958`), inside `.coord-quick-access`. All three reach the
  single shared row-tint site, `classList={{ working: rowIsWorking() }}` at `:2523`, so the
  realisation has one `working` row render site, not one per context.
- The wash already exists and is generic: `.replica-item.working::after`
  (`sidebar.css:9621-9623`, `rgba(58, 123, 255, 0.15)`), so this requirement adds no CSS.

## 13. Decisions (fixed by the user, not open)

- B1: an orchestrator row whose repo chip is in `ci-running` gets the `working` row tint.
- B2: row tint only. `workgroupIsWorking` keeps its current meaning, so room ordering
  (`splitWorkgroupsByWorking`), the group-rail dot and the quick-row tint must not change because of
  CI state.
- B3: orchestrator rows only. A non-orchestrator replica row with `ci-running` on its repo is not
  tinted.
- B4: the chip keeps its yellow border. This adds a signal and removes none.

## 14. Solution (decided)

**D-B1 - the tint is the existing `working` class, driven by one new per-row predicate.** No new
class, no new CSS, no new store, no new state: the row joins the `working` class that
`.replica-item.working::after` already paints. The predicate lives in `renderReplicaItem`, beside
the class that consumes it:

```tsx
          const orchestratorCiRunning = () =>
            isCoord() &&
            repoBadges().some(
              (repo) => remoteActivityStore.forPath(repo.sourcePath)?.ci === "running"
            );
```

**D-B2 - the predicate reads exactly what the chip reads, so the two cannot disagree.**
`repoBadges()` (`:2443-2448`) is the same list the chip `<For>` renders (`:2658-2670`), and
`remoteActivityStore.forPath(repo.sourcePath)?.ci` is the same store read `remoteActivityClasses`
does (`:402-403`). "The repo chip is in `ci-running`" is therefore the predicate itself, not a
lookalike: a chip without the marker cannot tint its row, and a marker cannot appear without
tinting its row. Only `ci === "running"` tints (B1): `stale` never tints, `unknown`/`idle`/no entry
never tint.

**D-B3 - the gate is per-row and lives there, not in `workgroup-session.ts`.** `isCoord()`
implements B3, and it is evaluated against the row's own `repoBadges()`. A worker carrying the same
repo path cannot tint, and two orchestrator rows in different rooms cannot tint each other, because
`forPath` is keyed by the exact string a chip already carries
(`src/sidebar/stores/remote-activity.ts:9-22` explains why).

**D-B4 - `workgroupIsWorking` and everything downstream of it are untouched.** `isReplicaWorking`,
`workgroupIsWorking` and `splitWorkgroupsByWorking` (`workgroup-session.ts:27-44`) keep their
bodies; `WorkgroupGroupRail` keeps reading the rail dot and the `working/total` counter from
`splitWorkgroupsByWorking(...).working.length` (`WorkgroupGroupRail.tsx:100-104`, dot at `:291-298`);
the room wash stays `.ac-wg-subgroup` `classList={{ working: workgroupIsWorking(wg) }}`
(`ProjectPanel.tsx:2711`). Room ordering, the rail dot, the rail counter, the group wash and the
quick-access tint are therefore functions of PTY sessions alone.

**D-B5 - declaration order.** `orchestratorCiRunning` reads `repoBadges()`, so it is declared
immediately AFTER the `repoBadges` memo, and the `rowIsWorking` accessor (with its #1783 comment) is
MOVED below it. Both are functions invoked only from the row's JSX (`:2523`), so the move changes no
behaviour; it exists so a reader never has to reason about a closure reading a later `const`.

**D-B6 - the quick-access site is the stated limit.** `rowContext === "quick"` keeps
`workgroupIsWorking(wg)` and gains nothing from CI, because B1 and B2 cannot both hold there: the
strip row is an orchestrator row (B1 would tint it) whose tint B2 forbids CI to change. B2 wins,
because it is the decision that protects room classification; B1 is honoured in both
`.ac-wg-subgroup` row contexts (`selected` and `workgroups`), and both reach the ONE shared
`rowIsWorking` accessor at the ONE row-tint render site (`ProjectPanel.tsx:2523`) - the realisation
touches one site, not two. Consequence, stated rather than discovered: an orchestrator row can be
tinted in the room tree and untinted in the Orchestrators strip in the same frame. The strip row
still shows the yellow chip border, so the CI fact stays visible there. No third behaviour is
invented: the quick branch stays `workgroupIsWorking(wg)`, byte-identical to today's. Test 15 pins
the limit executably: after `publishCi("running")` on the shared repo it gates on the quick row
existing and asserts it carries no `working` class, so the quick-access strip's tint stays
session-only and D-B6 holds.

Closed alternatives:

- (a) Add the CI term to `workgroupIsWorking` or to `splitWorkgroupsByWorking`: rejected. It would
  reach room ordering, the group-rail dot and the quick-row tint at once - exactly what B2 forbids -
  and the group wash would claim a room is working while no agent is.
- (b) Put the predicate in `workgroup-session.ts` next to `isReplicaWorking`: rejected for the same
  reason as (a). That module's three exports feed classification, ordering and the rail, so a CI term
  there is a footgun for every future caller. The predicate is per-row presentation logic and
  belongs in `renderReplicaItem`.
- (c) A new CSS class (`ci-working`) with its own wash: rejected. It is a second tint rule to keep
  consistent with the #1755 ladder for no benefit, and B1 asks for the existing `working` wash.
  `working-tint-css.test.ts` assertion 8 counts exactly three `.working` tokens and assertion 7
  ranks the one group rule; a new rule would put all of that back on the table.
- (d) Tint in the quick strip too: rejected, see D-B6.
- (e) Derive the tint from the staleness axis or from `behindBy`: rejected. Only B1's CI state is in
  scope, and neither has a decision making it a work signal.

## 15. In scope / out of scope (requirement B)

In scope:

- `src/sidebar/components/ProjectPanel.tsx`: the `orchestratorCiRunning` predicate and the
  `rowIsWorking` move/extension (section 16).
- `src/sidebar/components/ProjectPanel.working-tint.test.tsx`: a new `describe` block with the four
  tests of section 18 plus the local helpers they use.

Out of scope (no change, no new file, no new dependency):

- `src/sidebar/components/workgroup-session.ts`, `WorkgroupGroupRail.tsx`, `sidebar.css` (the #1755
  block at `:9568-9633` and the `.ci-running` border at `:6887-6889`), `variables.css`.
- `src/sidebar/stores/remote-activity.ts` and `src/shared/types.ts`: the store and `CiState` are
  consumed as they are; no new field, no new key, no new payload shape.
- `ProjectPanel.remote-activity.test.tsx`: it must keep passing UNMODIFIED. Its five cases are this
  requirement's chip-invariance guard (chip class, chip title, listener lifecycle).
- The chip's class list, tooltip and border; the branch-stale axis; settings; IPC shape; the backend
  (requirement A owns `remote_watcher.rs`).
- The staleness axis as a tint source; the quick-access site (D-B6); any new CSS class or token.

## 16. Exact changes (requirement B)

### 16.1 Production: `src/sidebar/components/ProjectPanel.tsx` (one file, two hunks)

Hunk 1 - delete the current `rowIsWorking` block (`:2426-2432`), comment included:

```tsx
          // #1783 - the quick-access panel answers "is this team busy", so an
          // orchestrator row there tints when ANY agent in its room is working,
          // the orchestrator included. Every other render site (rowContext
          // "workgroups" and "selected", both inside .ac-wg-subgroup) keeps the
          // per-row meaning: own session only. Do not collapse this branch.
          const rowIsWorking = () =>
            rowContext === "quick" ? workgroupIsWorking(wg) : isReplicaWorking(wg, replica);
```

Hunk 2 - insert immediately after the `repoBadges` memo (currently ends `:2448`):

```tsx
          // #2131 - CI running on this orchestrator row's repo is work the room is
          // waiting on, so the row takes the existing wash while its chip carries
          // `ci-running`. It reads the SAME published entry the chip class reads
          // (`remoteActivityClasses`) and the SAME `repoBadges()` list the chip
          // <For> renders, so the chip and the row cannot disagree. It must NOT
          // reach workgroupIsWorking: room ordering, the group-rail dot and the
          // quick-access row stay session-only. Non-orchestrator rows are excluded
          // here, not at the chip.
          const orchestratorCiRunning = () =>
            isCoord() &&
            repoBadges().some(
              (repo) => remoteActivityStore.forPath(repo.sourcePath)?.ci === "running"
            );
          // #1783 - the quick-access panel answers "is this team busy", so an
          // orchestrator row there tints when ANY agent in its room is working,
          // the orchestrator included. Every other render site (rowContext
          // "workgroups" and "selected", both inside .ac-wg-subgroup) keeps the
          // per-row meaning: own session only. Do not collapse this branch.
          // #2131 - the CI term is added ONLY on the non-quick branch, so the
          // quick-access row keeps #1783's room-wide session rule unchanged. That
          // is the stated limit of D-B6: the same orchestrator can be tinted in
          // the room tree and untinted in the Orchestrators strip in one frame.
          const rowIsWorking = () =>
            rowContext === "quick"
              ? workgroupIsWorking(wg)
              : isReplicaWorking(wg, replica) || orchestratorCiRunning();
```

Nothing else changes: `classList` (`:2519-2525`), the chip `<For>` (`:2658-2670`),
`remoteActivityClasses` (`:401-408`), `repoBadges` (`:2443-2448`), `workgroup-session.ts`,
`WorkgroupGroupRail.tsx` and `sidebar.css` are untouched. `classList`'s `working: rowIsWorking()`
at `:2523` is the single row-tint render site: hunk 2 changes the accessor it calls, and both
`.ac-wg-subgroup` contexts (`selected`, `workgroups`) reach that same site through the shared
`renderReplicaItem` factory, so the predicate is applied in both contexts from one code site. No
import changes: `isCoord`,
`isReplicaWorking`, `workgroupIsWorking` (`:85-90`) and `remoteActivityStore` (`:35`) are already in
scope. Both edit targets are CRLF on disk; keep the existing endings.

### 16.2 Tests: `src/sidebar/components/ProjectPanel.working-tint.test.tsx`

Imports added to the file, after `import { automationIdPart } from "./replica-repo-badges";`:

```tsx
import { remoteActivityStore } from "../stores/remote-activity";
import { splitWorkgroupsByWorking } from "./workgroup-session";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import type { CiState } from "../../shared/types";
```

Then the module-scope constants, discovery builder, mount, publish helper and DOM helpers of section
18.1, and the new `describe` block of sections 18.2-18.5 appended after the existing `describe`. The
existing `describe`, its nine tests and the boundary probe are untouched.

## 17. Required behaviour, edge cases, failure behaviour (requirement B)

| Case | Row `working` class | Chip |
|---|---|---|
| orchestrator row, chip `ci-running`, own session idle or absent | tinted (B1) | `ci-running` border, unchanged |
| orchestrator row, own session working | tinted (today's rule, unchanged) | unchanged |
| orchestrator row, chip `ci` = `idle` | not tinted | no marker |
| orchestrator row, chip `ci` = `unknown` | not tinted | no marker |
| orchestrator row, no store entry for any of its repo paths | not tinted | no marker |
| orchestrator row with an empty `repoBadges()` (no repos configured) | not tinted | no chip rendered |
| non-orchestrator row, repo `ci-running` | not tinted (B3) | no chip rendered (chips are orchestrator-only, `:2658`) |
| orchestrator row, chip `stale` but not `ci-running` | not tinted | keeps its `stale` marker |
| orchestrator row in the Orchestrators strip (`quick`), `ci-running` | not tinted (D-B6 limit) | `ci-running` border |
| orchestrator row with an idle / waiting / pending-review session, `ci-running` | tinted (the tint is per-chip, not per-session) | `ci-running` border |
| two orchestrator rows sharing one repo path | both tinted (each reads its own `repoBadges()`) | both chips marked |
| orchestrator row whose session publishes `gitRepos` | same rule: the predicate iterates whatever `repoBadges()` returns | same |

Edge cases and failure behaviour:

- **`unknown` is not running.** No marker, no tint. This is the majority case (no `gh`, feature off,
  not swept yet) and a user in it sees exactly today's UI.
- **No store entry.** `forPath` returns `undefined`, `?.ci` is `undefined`, no tint. A path
  garbage-collected by the next Phase A payload (`remote-activity.ts:43-58`) clears its tint in the
  same update.
- **Repo chip absent** (no repos configured, or a non-orchestrator row): `repoBadges()` is empty or
  the `isCoord()` gate is false, and `Array.prototype.some` on an empty list is `false`. A row with
  no chip can never tint, and no chip can appear to explain a tint that is not there.
- **Path mismatch.** The store is keyed by the EXACT `sourcePath` string on purpose
  (`remote-activity.ts:9-22`). A key that does not match the chip's `sourcePath` produces no tint -
  and no chip marker either, so the row and the chip stay consistent rather than disagreeing.
- **Misaligned payload.** The store rejects it and keeps the previous map
  (`remote-activity.ts:44-58`); the tint follows the last accepted payload, exactly like the chip.
- **CI stops.** The next accepted payload writes `ci: "idle"`, and the tint clears; `clearAll()` on
  teardown clears it too. Test 13 pins the removal half.
- **Reactivity.** The row's `classList` and the chip's `class` are dynamic attributes reading the
  same store, so a publish that repaints the chip repaints the row. Test 13 flips one row untinted
  -> tinted -> untinted on store publishes, so a broken tracking path fails the test instead of
  shipping.
- **No new failure mode:** the predicate is a boolean over data already in a store. No I/O, no
  async, no error path, no new listener, no new lifecycle, no cleanup.

## 18. Tests (exact)

File: `src/sidebar/components/ProjectPanel.working-tint.test.tsx`, a new `describe` appended to the
existing file. All four tests reuse the file's harness: `installBrowserDomStubs`,
`resetUiStoresForTests`, `renderWithFakeTransport`, `FakeTransport`, `baseSettings`, `discovery`,
`session`, `settingsStore.load`, `projectStore.createAndLoad`, `sessionsStore.setSessions`,
`sessionsStore.setVisibleActiveIdForTests`, `waitFor`, `automationIdPart`, and the existing
`row` / `subgroups` / `anySubgroupWorking` / `rowTestId` / `replicaSession` / `sessionId` helpers.
The publish helper mirrors `ProjectPanel.remote-activity.test.tsx:110-118`.

**Why the positive control is mandatory.** A row that is tinted and a test that cannot see the tint
are indistinguishable from "not tinted". Test 13 therefore flips one row untinted -> tinted ->
untinted on store publishes, and tests 14-16 keep a positive control in the same render as their
"nothing happened" assertions.

### 18.1 New module-scope constants and helpers

Placed after the existing `IDLE_ROOM` block and before `tintDiscovery`, so the existing discovery
builders and tests keep their exact behavior:

```tsx
// #2131 - the repo every row in the CI tint block points at. `ciRepoPath()` is the
// exact string the remote-activity payload is keyed by, which is why it is written
// once and shared by the discovery and the publish helper.
const CI_REPO_DIR = "repo-AgentsCommander";
const CI_REPO_LABEL = "AgentsCommander";
const ciRepoPath = (wg: string = wgName): string => `${workgroupPathOf(wg)}\\${CI_REPO_DIR}`;

function ciDiscovery() {
  return discovery({
    workgroups: [
      {
        name: wgName,
        path: workgroupPath,
        task: null,
        taskTitle: "CI working tint",
        agents: [
          // Both room-1 rows carry the SAME repo path: the orchestrator must tint on
          // it and the worker must not, in the same render.
          { name: ORCHESTRATOR, path: replicaPath(ORCHESTRATOR), repoPaths: [ciRepoPath()], isCoordinator: true },
          { name: WORKER, path: replicaPath(WORKER), repoPaths: [ciRepoPath()], isCoordinator: false },
        ],
      },
      {
        name: IDLE_ROOM,
        path: workgroupPathOf(IDLE_ROOM),
        task: null,
        taskTitle: "Idle control room",
        agents: [
          {
            name: IDLE_ORCHESTRATOR,
            path: replicaPath(IDLE_ORCHESTRATOR, IDLE_ROOM),
            repoPaths: [ciRepoPath(IDLE_ROOM)],
            isCoordinator: true,
          },
          { name: IDLE_MEMBER, path: replicaPath(IDLE_MEMBER, IDLE_ROOM), repoPaths: [], isCoordinator: false },
        ],
      },
    ],
  });
}

/** `withRail` mounts the real rail beside the panel for the classification test;
 *  the rail needs the one extra fake invoke, exactly as
 *  ProjectPanel.collapse-state.test.tsx mounts the pair. */
async function mountCiPanel(options: { withRail?: boolean } = {}) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", ciDiscovery());
  if (options.withRail) {
    fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  }
  const rendered = renderWithFakeTransport(
    () =>
      options.withRail ? (
        <div>
          <WorkgroupGroupRail projects={projectStore.projects} />
          <ProjectPanel />
        </div>
      ) : (
        <ProjectPanel />
      ),
    fake
  );
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  // Gate on the row element, not on the word "orchestrator": the coord chip renders
  // that word too, so textContent is not a mount proof.
  await waitFor(() =>
    expect(
      rendered.root.querySelector(`[data-ac-testid="${rowTestId("workgroups", ORCHESTRATOR, wgName)}"]`)
    ).not.toBeNull()
  );
  return rendered;
}

/** One repo's published answer, exactly as the event carries it (same shape as
 *  ProjectPanel.remote-activity.test.tsx:110-118). */
function publishCi(ci: CiState, repoPath: string = ciRepoPath()): void {
  remoteActivityStore.applyRemoteActivityUpdate({
    repoPaths: [repoPath],
    ciStates: [ci],
    stalenessStates: ["current"],
    behindBy: [null],
  });
}

/** The chip element for one row context and replica, by the test id the panel emits:
 *  `replica.repoBadge.<context>.<wg>.<replica>.<index>.<label>`. */
const chipSelector = (context: string, replica: string, wg: string = wgName): string =>
  `[data-ac-testid="replica.repoBadge.${automationIdPart(context)}.${automationIdPart(wg)}.${automationIdPart(replica)}.0.${CI_REPO_LABEL}"]`;

function chip(root: HTMLElement, context: string, replica: string, wg: string = wgName): HTMLElement {
  const el = root.querySelector<HTMLElement>(chipSelector(context, replica, wg));
  if (!el) throw new Error(`missing repo chip: ${chipSelector(context, replica, wg)}`);
  return el;
}

/** The rail's working dots and counters come from splitWorkgroupsByWorking; these are
 *  the selectors WorkgroupGroupRail.test.tsx uses. */
function railDots(root: HTMLElement): string[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.dot."]')
  ).map((dot) => dot.dataset.acTestid ?? "");
}

function railButton(root: HTMLElement, key: string): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="workgroupGroups.button.${key}"]`);
  if (!el) throw new Error(`missing rail button: ${key}`);
  return el;
}

/** The room sequence, read from the first row of each .ac-wg-subgroup. */
function subgroupRowOrder(root: HTMLElement): string[] {
  return subgroups(root).map(
    (sub) =>
      sub.querySelector('[data-ac-testid^="replica.row."]')?.getAttribute("data-ac-testid") ??
      "missing"
  );
}
```

### 18.2 Test 13 - positive control: the orchestrator row tints in both `.ac-wg-subgroup` contexts

```tsx
describe("ProjectPanel orchestrator CI working tint (#2131)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("13. tints the orchestrator row in both room contexts while its chip is ci-running", async () => {
    const rendered = await mountCiPanel();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "idle"),
      ]);
      sessionsStore.setVisibleActiveIdForTests(sessionId(ORCHESTRATOR));
      // Gate: both contexts are rendered. "selected" only exists with an active
      // session, and the assertions below must not pass on an absent row.
      await waitFor(() =>
        expect(rendered.root.querySelector(chipSelector("selected", ORCHESTRATOR))).not.toBeNull()
      );

      // Untinted before the store has anything to say, with the chips present and
      // silent: this half is what makes the flip below a measurement.
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "selected", ORCHESTRATOR).classList.contains("working")).toBe(false);
      expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).not.toContain("ci-running");

      publishCi("running");
      await waitFor(() =>
        expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).toContain("ci-running")
      );
      expect(chip(rendered.root, "selected", ORCHESTRATOR).className).toContain("ci-running");
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(true);
      expect(row(rendered.root, "selected", ORCHESTRATOR).classList.contains("working")).toBe(true);
      // Control in the same render: the worker's session is idle and its row is
      // untinted, so "the CI term tints every row" fails here.
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);

      // Removal half: CI stops, the tint goes with it.
      publishCi("idle");
      await waitFor(() =>
        expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).not.toContain("ci-running")
      );
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(false);
      expect(row(rendered.root, "selected", ORCHESTRATOR).classList.contains("working")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
```

### 18.3 Test 14 - no tint on `idle`, `unknown` or an absent entry

```tsx
  it("14. does not tint an orchestrator row on idle, unknown or absent CI state", async () => {
    const rendered = await mountCiPanel();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "idle"),
      ]);
      await waitFor(() =>
        expect(rendered.root.querySelector(chipSelector("workgroups", ORCHESTRATOR))).not.toBeNull()
      );

      // No store entry at all: a user without `gh`, or with the feature off. The chip
      // is present and silent, and the row is untinted.
      expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).toBe("ac-discovery-badge branch");
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(false);

      // `idle` - checked, nothing running. The tooltip is the gate: that suffix can
      // only be there after the store update reached the DOM.
      publishCi("idle");
      await waitFor(() =>
        expect(chip(rendered.root, "workgroups", ORCHESTRATOR).title).toContain(
          "no CI activity for this commit"
        )
      );
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(false);

      // `unknown` - no answer is not an answer. Same gating shape.
      publishCi("unknown");
      await waitFor(() =>
        expect(chip(rendered.root, "workgroups", ORCHESTRATOR).title).not.toContain("no CI activity")
      );
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(false);
      expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).not.toContain("ci-running");
    } finally {
      rendered.cleanup();
    }
  });
```

### 18.4 Test 15 - orchestrator-only, quick-strip limit, and per-repo

```tsx
  it("15. does not tint a non-orchestrator row, the quick strip, or another room, from the same CI state", async () => {
    const rendered = await mountCiPanel();
    try {
      sessionsStore.setSessions([
        replicaSession(ORCHESTRATOR, "idle"),
        replicaSession(WORKER, "idle"),
      ]);
      await waitFor(() =>
        expect(rendered.root.querySelector(chipSelector("workgroups", ORCHESTRATOR))).not.toBeNull()
      );

      publishCi("running");
      await waitFor(() =>
        expect(chip(rendered.root, "workgroups", ORCHESTRATOR).className).toContain("ci-running")
      );

      // Positive control and the negatives in one render.
      expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(true);
      // WORKER carries the SAME repoPath, so its repoBadges() is non-empty and only
      // the orchestrator gate can keep it untinted.
      expect(row(rendered.root, "workgroups", WORKER).classList.contains("working")).toBe(false);
      // A different repo path in another room: a "CI is running somewhere" predicate
      // fails here.
      expect(
        row(rendered.root, "workgroups", IDLE_ORCHESTRATOR, IDLE_ROOM).classList.contains("working")
      ).toBe(false);
      // D-B6 - the Orchestrators strip keeps #1783's session-only rule. This quick
      // row is an orchestrator row pointing at the SAME ci-running repo as the tinted
      // room-tree row above, so only the `rowContext === "quick"` branch keeps it
      // untinted. The gate proves the strip rendered: without it, an absent quick row
      // would make the "not tinted" assertion pass for the wrong reason. Adding the CI
      // term to the quick branch (`workgroupIsWorking(wg) || orchestratorCiRunning()`)
      // fails HERE, because this row would then carry `working` too.
      await waitFor(() =>
        expect(
          rendered.root.querySelector(`[data-ac-testid="${rowTestId("quick", ORCHESTRATOR)}"]`)
        ).not.toBeNull()
      );
      expect(row(rendered.root, "quick", ORCHESTRATOR).classList.contains("working")).toBe(false);
      // The chip is orchestrator-only (:2658), so no chip exists for the worker; its
      // untinted row is the isCoord() gate talking, not an empty repoBadges().
      expect(
        rendered.root.querySelector(
          `[data-ac-testid^="replica.repoBadge.workgroups.${automationIdPart(wgName)}.${automationIdPart(WORKER)}."]`
        )
      ).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
```

### 18.5 Test 16 - classification, ordering and the rail dot

```tsx
  it("16. CI state alone changes no room classification, no rail dot and no order", async () => {
    const rendered = await mountCiPanel({ withRail: true });
    try {
      await waitFor(() => {
        expect(rendered.root.querySelector(chipSelector("workgroups", ORCHESTRATOR))).not.toBeNull();
        expect(
          rendered.root.querySelector('[data-ac-testid="workgroupGroups.button.all"]')
        ).not.toBeNull();
      });
      // Before half: nobody works, so no room is classified as working.
      expect(anySubgroupWorking(rendered.root)).toBe(false);
      expect(railDots(rendered.root)).toEqual([]);
      expect(railButton(rendered.root, "all").textContent).toContain("0/2");
      const orderBefore = subgroupRowOrder(rendered.root);

      publishCi("running");
      // In-run positive control: the row IS tinted, so the assertions below cannot
      // pass because nothing happened.
      await waitFor(() =>
        expect(row(rendered.root, "workgroups", ORCHESTRATOR).classList.contains("working")).toBe(true)
      );

      // Classification: both rooms are still NOT working, and the panel's group wash
      // and the rail's dot/counter follow that classification, not the tint.
      const split = splitWorkgroupsByWorking(projectStore.projects[0].workgroups);
      expect(split.working.map((group) => group.name)).toEqual([]);
      expect(split.notWorking.map((group) => group.name)).toEqual([wgName, IDLE_ROOM]);
      expect(anySubgroupWorking(rendered.root)).toBe(false);
      expect(railDots(rendered.root)).toEqual([]);
      expect(railButton(rendered.root, "all").textContent).toContain("0/2");

      // Ordering: the room sequence is the byte-identical one from before the publish.
      expect(subgroupRowOrder(rendered.root)).toEqual(orderBefore);
    } finally {
      rendered.cleanup();
    }
  });
});
```

## 19. Objective acceptance criteria (requirement B)

- A-B1: `npm test -- src/sidebar/components/ProjectPanel.working-tint.test.tsx --reporter=verbose`
  exits 0, and the output lists the four new tests by name: `13. tints the orchestrator row in both
  room contexts while its chip is ci-running`, `14. does not tint an orchestrator row on idle,
  unknown or absent CI state`, `15. does not tint a non-orchestrator row, the quick strip, or
  another room, from the same CI state`, `16. CI state alone changes no room classification, no
  rail dot and no order`.
- A-B2: the same run lists the file's nine pre-existing tests and its boundary probe as passing,
  unmodified.
- A-B3: `npm test -- src/sidebar/components/ProjectPanel.remote-activity.test.tsx` exits 0 with the
  file unmodified (`git diff --name-only` does not list it): the chip marker, the chip title and the
  listener lifecycle are unchanged.
- A-B4: `npm run typecheck` exits 0.
- A-B5: the full frontend gate stays green: `npm test` exits 0 (`.github/workflows/
  pr-regression-gates.yml:2479-2493`) and `npm run test:debt` exits 0 (`:40`).
- A-B6: requirement B's diff touches exactly two tracked files:
  `src/sidebar/components/ProjectPanel.tsx` and
  `src/sidebar/components/ProjectPanel.working-tint.test.tsx`:
  `git diff --name-only -- src/sidebar/` lists exactly those two and nothing else - no
  `sidebar.css`, no `workgroup-session.ts`, no `WorkgroupGroupRail.tsx`, no `remote-activity.ts` and
  no `src/shared/types.ts`.
- A-B7: `git diff -- src/sidebar/styles/sidebar.css src/sidebar/styles/variables.css` is empty, so
  the #1755 ladder and the `ci-running` border are byte-identical, and
  `npm test -- src/sidebar/styles/working-tint-css.test.ts` passes unmodified.

## 20. Proof protocol (for the reviewer)

1. Confirm the two hunks against the quoted text in section 16.1; re-anchor on the quoted text, not
   on line numbers, if the branch has moved.
2. Confirm the quick branch is byte-identical: `rowContext === "quick"` still routes only to
   `workgroupIsWorking`, and `workgroupIsWorking`, `isReplicaWorking` and `splitWorkgroupsByWorking`
   have unchanged bodies. Test 15's quick-row assertion is the executable form of this step: gate on
   the quick row existing, assert it carries no `working`, and confirm that moving the CI term into
   the quick branch (`workgroupIsWorking(wg) || orchestratorCiRunning()`) makes test 15 fail.
3. Run A-B1 and capture the output: the four new tests and the nine old ones must all be listed as
   passing, and the new tests must not be skipped, `.only` or placeholder.
4. Run A-B3, A-B4, A-B5 and A-B7.
5. Diff review: only the predicate, the moved accessor and the new test block; no CSS, no store, no
   type, no rail and no `workgroup-session.ts` change.

## 21. Risks and compatibility (requirement B)

- **False positive (a row claims work that is not happening).** The predicate is gated on
  `ci === "running"` from the same entry the chip paints, so a tinted row implies a yellow chip.
  Tests 14 and 15 pin the negative side in the same render as their positive controls.
- **False negative (the reported bug survives).** Test 13 flips the same row on and off a store
  publish, so a predicate that never fires fails loudly instead of looking "quiet".
- **Accidentally widening the room signal.** The most likely wrong implementation is reaching for
  `workgroupIsWorking`; test 16 asserts, in the same render as a tinted row, that both rooms are
  still classified as not working, that no `.ac-wg-subgroup` is washed, that the rail shows no dot
  and that the counters stay `0/2`.
- **The quick-strip limit (D-B6).** The visible consequence is an inconsistency between the room
  tree and the Orchestrators strip; it is recorded as a limit, not a defect. B2 is the decision that
  protects room classification, and the strip still shows the yellow chip border. Test 15 gates on
  the quick row existing and asserts it is untinted while the same orchestrator's room-tree row is
  tinted, so the quick branch cannot silently gain the CI term.
- **A11y.** The tint is colour-only, but the chip's tooltip already carries "CI running"
  (`replica-repo-badges.ts:26-31`), so the fact is not colour-only. No new a11y surface.
- **No wire or persistence surface:** no IPC, no settings, no serialized shape, no listener and no
  lifecycle change. `ProjectPanel.remote-activity.test.tsx` staying green without edits is the
  compatibility proof for the chip.

## 22. Implementation order (requirement B)

1. Apply hunks 1 and 2 of section 16.1 (one production file).
2. Add the imports, module-scope helpers and `describe` block of sections 16.2 / 18 to
   `ProjectPanel.working-tint.test.tsx`.
3. Run A-B1 and A-B3; then A-B4, A-B5 and A-B7.
4. Commit both requirements on `fix/2131-no-stale-notice-on-default-branch` with the plan file
   (`git add -f plans/2131-no-stale-notice-on-default-branch.md`); no push to `main`, no merge;
   leave the branch for review.

## Plan Contract self-check (requirement B, sections 12-22)

- Objective: section 12.
- Cause with evidence references: section 12 (`:401-408`, `:6887-6889`, `:2426-2432`, `:2519-2525`,
  `:2415-2701`, `:2658-2670`, `:2958`, `:2987`, `:3047`, `:9621-9623`, `workgroup-session.ts:27-44`).
- In scope / out of scope: section 15.
- Decided solution, no TBD, no open choice: sections 13, 14 (D-B1 to D-B6, the stated limit and the
  closed alternatives).
- Exact files and symbols: section 16.
- Required behaviour, edge cases, failure behaviour (`unknown`, no store entry, repo chip absent):
  section 17.
- Tests with positive controls, and the reused harness and helpers named: section 18.
- Objective acceptance criteria: section 19.

## Plan Contract self-check (whole file)

- Requirements A and B are covered end to end: A in sections 1-11 with its acceptance criteria in
  section 8, B in sections 12-22 with its acceptance criteria in section 19. Sections 1-11 are
  unchanged in substance; only their boundary notes (section 4, A6, A7) were qualified so the file
  reads as one deliverable.
- The two parts are independent: A changes `src-tauri/src/pty/remote_watcher.rs`, B changes
  `src/sidebar/components/ProjectPanel.tsx` plus one test file. Neither requires the other, and
  neither part's tests can mask the other's.
- One branch, one PR, one plan file: `fix/2131-no-stale-notice-on-default-branch`.
