# Epic: the blocked-menu notice must be impossible to lose, with N sessions blocked at once

Issue: 1855 (parent). Children: 1856 (P1), 1857 (P2), 1858 (P3), 1859 (P4). All five verified OPEN in
`mblua/AgentsCommander` at certification time.
Base SHA: `f551791c8c1c0d8d135e3dd40a2d35b13dee675c` (synchronized with `origin/main`).
Branches: one per phase, named for its own child issue, because each phase is its own PR:
`fix/1856-communication-reconcile`, `fix/1857-aggregated-toast-attention`,
`fix/1858-blocked-menu-glyph`, `fix/1859-collapse-proof-rollups`. The pattern is enforced by
`scripts/validate-branch-name.mjs:15` and the open-issue check by
`.github/workflows/validate-branch-name.yml`.
Repo root: `repo-AgentsCommander`.
Status: READY_FOR_IMPLEMENTATION

## Accepted task class and threat model

Routine application-code change. Frontend only: TypeScript and CSS under `src/`. No Rust, no IPC
surface change, no CLI, no persistence, no schema, no packaging, no release. The repository contract
states no higher-assurance requirement for this change, and neither the task nor the user has stated
one, so every enhanced control in `delivery-nonfunctional-invariants` is non-applicable; each is
listed with its reason in "Delivery invariants" below.

## Objective

A session stuck in an interactive menu must be noticed, and must still be findable later. Today the
announcement and the permanent record are the same object (a toast), and that object has a hard cap
of 4, an eviction rule that prefers exactly this kind of notice, and no republisher. Split the two
jobs: a derived record that scales with N, and a single announcement that does not.

## Evidence (measured or read at the base SHA)

- The toast is pushed inside the event listener at `src/sidebar/App.tsx:729-761`. The `else` branch
  at `src/sidebar/App.tsx:758-760` dismisses by tag on any other communication event.
- `sessionsStore.setCommunication` runs first and unconditionally at `src/sidebar/App.tsx:730`, so
  the store already holds the truth even when the toast does not.
- A derived per-row indicator already exists: `src/sidebar/components/ProjectPanel.tsx:2339`,
  `:2346-2348`, `:2457-2467`. It survives toast eviction.
- The row is hidden by three independent collapses: `src/sidebar/components/ProjectPanel.tsx:2765`
  (project), `:2613` (workgroup subgroup), `:2784` (orchestrators quick group).
- The rollup helper exists with zero non-test callers: `workgroupHasBlockedMenu` at
  `src/sidebar/components/workgroup-session.ts:62-64`. Its raise-hand twin IS wired, at
  `src/sidebar/components/WorkgroupGroupRail.tsx:108`.
- Blocked-menu and raise-hand render the same glyph: `src/sidebar/components/ProjectPanel.tsx:2451`
  versus `:2465`, both `RaiseHandIcon`, both in the amber chip at `src/sidebar/styles/sidebar.css:6018-6029`.
  The class `coord-communication-slot--blocked-menu` is written at
  `src/sidebar/components/ProjectPanel.tsx:2459` and has no CSS rule anywhere.
- `MAX_VISIBLE = 4` at `src/shared/stores/toasts.ts:47`. The eviction victim is the first non-error
  at `src/shared/stores/toasts.ts:131-134`; blocked-menu toasts are `kind: "info"`.
- Measured by `ac-dev-webpage-ui-v4` against the same `toasts.ts` blob
  (`0afff4efc49b00e4bd4b582d804ed54cc8988545`): with 10 tagged notices and no errors, exactly the
  last 4 survive. With 4 sticky errors already visible, ZERO survive, and re-pushing 10 times in a
  row leaves all 10 dead for as long as those 4 errors remain. Dismissing one error makes the same
  push survive.
- Measured, defect 2: `dismissByTag` at t=0 followed by a same-tag push at t=50 returns the SAME id
  and patches the message, but `exiting` stays true and the toast disappears at t=180 exactly,
  carrying the new message. The window is counted from the dismiss and is not restarted by the push.
- Both defects are silent to the caller: `push` returns an id that looks like success in both cases.
- The backend never republishes while the screen is quiet: `ScreenRowsSince::Unchanged` is a no-op at
  `src-tauri/src/pty/menu_guard/mod.rs:291`, and a menu waiting for an answer is a still screen.
- Every listing already carries the truth: `SessionInfo.communication` at
  `src-tauri/src/session/session.rs:298`, and `refreshProfileOutdated` already polls
  `SessionAPI.list()` every 5 seconds (`src/sidebar/App.tsx:316-338`, interval at `:781`) and
  discards that field.
- There are three default notice texts, not one: `src-tauri/src/config/settings.rs:1013`, `:1026`,
  `:1035`. They are user-editable per agent, so the real number is unbounded.

## Closed product decisions (do not reopen)

1. The menu guard deferring a message is not an error; the `log::error!` drops level and the modals go.
2. Messages retry silently, forever, without backoff. Nothing is lost.
3. The aggregated toast is sticky: it stays until the user removes it.
4. A blocked menu DOES flash the taskbar button.
5. "See terminal" goes to the session that produced the text currently on screen, never to the oldest.
6. Splitting `session.communication` into two fields is deferred and owned by issue 1853.
7. One at a time is enough. No list view of the N pending notices is built.

## Scope

In scope, frontend only:

- reconcile `communication` from the listing the app already polls;
- one aggregated toast driven by derived state, with the taskbar flash;
- make the aggregated toast un-evictable and fix the 180 ms dismiss/re-push race;
- give the blocked menu its own glyph and colour, and give the toast action button a CSS rule;
- roll the blocked-menu state up through every collapsible level.

Out of scope, with reasons:

- Splitting `session.communication`. Owned by issue 1853.
- A Rust reconciler in the `Unchanged` arm of `scan_tick`. The frontend reconcile makes the UI
  truthful; what a Rust reconciler would additionally fix is `sessions.json` after a mutual clobber,
  which is the same defect issue 1853 owns. Deliberately excluded, not forgotten.
- A project-level count badge.
- `max-height` and scrolling on the toast host. Replaced by a contract test on `MAX_VISIBLE`; see P2.
- The toast plate overlapping the sidebar at an 800px window width. Known cosmetic issue.
- Any notification outside the application. Named as a limitation in P2, not built.

## Compatibility

No persisted shape changes, no IPC payload changes, no CLI changes. `PushToastOptions.pinned` is a
new optional field, so every existing caller compiles and behaves identically. Nothing in this epic
alters what the backend writes to `sessions.json` or what `list-peers` reads from it.

Two behaviour changes for existing toast callers are in scope and are named here so nobody discovers
them later. First, the kind-aware eviction guarantee documented in TWO comment blocks,
`src/shared/stores/toasts.ts:43-46` and `:127-130`, is narrowed: with four sticky errors visible and
the pinned aggregate arriving, tier 1 finds no victim and tier 2 evicts an error, so an unread error
can now be dropped by an info toast. That is deliberate, because a blocked session is unanswerable
work the user asked to be un-losable while an error is a report, and P2 requires BOTH comment blocks
to be rewritten to say so, with acceptance criterion 8 checking it. Second, a tagged toast that is
dismissed and re-pushed inside the 180 ms exit window now keeps its auto-dismiss timer. Before P2 it
had none to keep: it simply died at t=180. The timer loss is a state that only exists between change 2
and its re-arm, and P2 restricts that re-arm to the revive path so no other caller changes.

## Dependency cycles and layering

The change adds three module arcs, all to new leaf modules. Two land in P2 and P3:

- `src/sidebar/App.tsx` -> `src/sidebar/attention.ts` (P2, new file). `attention.ts` imports only
  `src/shared/platform.ts`, which has zero imports, plus a dynamic `@tauri-apps/api/window`.
- `src/sidebar/components/ProjectPanel.tsx` -> `src/sidebar/components/BlockedMenuIcon.tsx` (P3, new
  file). It imports only `solid-js`, exactly like `src/sidebar/components/RaiseHandIcon.tsx`.

P4 adds one more arc to that same new leaf,
`src/sidebar/components/WorkgroupGroupRail.tsx` -> `src/sidebar/components/BlockedMenuIcon.tsx`,
mirroring that file's existing import of `RaiseHandIcon` on its line 24.

Per-arc verdict: all three cross a previously-clean boundary, and all three are cycle-safe by
construction, a leaf that imports nothing which can reach back to its consumers. P4 adds no arc to
`./workgroup-session`: `src/sidebar/components/ProjectPanel.tsx:85` and
`src/sidebar/components/WorkgroupGroupRail.tsx` already import it, so wiring
`workgroupHasBlockedMenu` adds symbols, not arcs.

Layering: `attention.ts` is placed in `src/sidebar/`, the layer that already owns the window surface,
mirroring `src/main/components/ErrorModal.tsx:87-89`. Putting the same call inside
`src/shared/stores/toasts.ts` would give a store a UI-transport dependency and is forbidden here. If
a second consumer ever appears, `attention.ts` moves beside `src/shared/ipc.ts`, never into `stores/`.

Measurement: the repository's levelization instrument targets `src-tauri` and this change touches no
Rust, so it is not the applicable instrument. The applicable one is
`npm run check:frontend-dependencies` (`scripts/check-frontend-dependencies.mjs`, pinned
dependency-cruiser 18.0.0 over the whole `src` root). It is a local check and is NOT part of any CI
workflow. Every phase carries it as an acceptance command. Limitation stated plainly: the pre/post
comparison is a local run by the implementer, not CI evidence.

## Delivery invariants

### Gate 1, CI-to-plan parity

`.github/workflows/pr-regression-gates.yml` declares eight jobs and carries NO `paths:` filter, so a
frontend-only PR still triggers every one of them: `test-debt`, `rust-regression`,
`rust-regression-linux`, `rust-regression-macos`, `rust-fmt`, `terminal-snapshot-portable`,
`windows-release-cli-smoke`, `frontend-regression`. `.github/workflows/validate-branch-name.yml` runs
on push and calls `scripts/validate-branch-name.mjs --check-issue`, which fails when the branch's
issue number does not exist or is closed.

The jobs this change can actually move are `frontend-regression`
(`.github/workflows/pr-regression-gates.yml:364-365` `npm run typecheck`, `:367-431` `npm test`
behind the temporary issue-480 known-debt guard) and `test-debt` (`npm run test:debt`). The Rust
jobs must stay green untouched; a red Rust job on this branch is a signal, not accepted debt.

Locally reproducible, run by the implementer at the end of every phase: `npm run typecheck` and
`npm test`. Expected: typecheck exits 0 with no output; `npm test` reports every suite passing. Known
accepted debt, stated exactly: `npm test` may exit 1 while reporting all tests passed, and the CI
guard tolerates only the one issue-480 unhandled-WebSocket-rejection signature
(`.github/workflows/pr-regression-gates.yml:418-421`); anything else fails closed. Read the summary
line, not the exit code alone, and never widen that guard.

Remote-only and host-dependent evidence assigned to CI: the Windows, Linux and macOS Rust jobs, the
release CLI smoke, and the branch-name/issue check. Acceptance: every triggered and configured
required check green for the exact PR-head SHA. Evidence from any other SHA, a skip, a waiver or a
bypass does not satisfy this gate. Owner: the implementer opens the PR; the shipper reads the checks.

Re-derive this section if the workflows, the required-check configuration, the base, or the diff drift.

### Gate 2, deterministic toolchain and build

Node 22 and npm pinned to 11.6.2 by the workflow (`.github/workflows/pr-regression-gates.yml:355`,
`:359`); dependencies installed from the lockfile. Locally the implementer records `node --version`
and `npm --version` once per phase. Commands run with the repo root as an explicit working directory.
Vitest configuration is `vitest.config.ts`; the environment is per-file (`// @vitest-environment jsdom`
at the top of a DOM test, as `src/sidebar/App.menu-guard.workflow.test.tsx:1` already does).

Enhanced provenance controls (independently anchored executable hashes, DLL closure inventories,
poisoned-PATH tests, SDK manifests) are NON-APPLICABLE: this is ordinary application code under the
routine task class, and rule 3 of the skill's proportionality section forbids imposing them here.

### Gate 3, authorized traceable Git

State-changing Git runs only inside `repo-AgentsCommander`. Preconditions before the first product
write of each phase: `git rev-parse HEAD` equals the base SHA above, `git status --porcelain` empty,
and the branch created from that base and named for that phase's own child issue as listed at the top
of this file. Delivery is by pull request into `main`; direct push to `main` is forbidden and the
workflow already ignores pushes to it. All four child issues and the parent were verified OPEN with
`gh issue view` at certification time, which is what `--check-issue` will re-check on push.

`/plans/` is `.gitignore:11`, so these five plan files never appear in `git status` and a plain
`git add -A` will not ship them. Decided by the tech lead: they ARE committed, with `git add -f`, so
the implementer and the reviewer read the same bytes.

Two consequences the implementer must not get wrong.

First, ORDER. Force-add and commit the five plan files as their own commit, before the first code edit
of P1. Staged plan files DO show in `git status --porcelain`, so committing them first is what keeps
every phase's "exactly these files and nothing else" criterion honest.

Second, the digests below are `sha256sum` over LF bytes, and this repository will hand a reviewer
CRLF. `.gitattributes` sets `eol=lf` for `.sh`, `.toml`, `.json`, `.rs` and a few named paths, but
NOT for `plans/**` or `*.md`, and `core.autocrlf` is `true`, so a checked-out plan file is CRLF on
Windows. Measured on the already-tracked `plans/1757-codex-hooks-blocking-menu.md` at this base: 1159
CR bytes on disk, disk `sha256sum` `cc0b5998...`, blob `sha256sum` `85acdcda...`. They are different
files by byte.

So verify a digest against the BLOB, never the working file:

```
git show <rev>:plans/blocked-menu-notice/epic.md | sha256sum
```

A mismatch from `sha256sum plans/blocked-menu-notice/epic.md` on Windows is line endings, not
tampering. Do NOT "fix" this by adding `plans/** text eol=lf` to `.gitattributes`: that would
renormalize the twenty-odd plan files already tracked and is far outside this epic's scope.

### Gate 4, process state, configuration and working directory

No inherited environment variable changes the planned commands. Vitest writes nothing outside its own
temp handling; `npm test` in CI writes `npm-test-results.json`, `npm-test.log` and
`npm-test.normalized.log` at the repo root, and the CI step removes them first
(`.github/workflows/pr-regression-gates.yml:372`). Locally, run `npm test` without the JSON reporter
so no such file is created, or delete it before checking `git status`. Every phase ends with
`git status --porcelain` showing only that phase's declared files.

Enhanced controls (namespace quarantine, ancestor-configuration byte maps, environment restoration
frameworks) are NON-APPLICABLE: no hostile-parent threat model has been accepted for this task.

### Gate 5, validation and scope before acceptance

The base is frozen above. Each phase file names its exact path set. After each phase:
`git status --porcelain` and `git diff --stat` must list those paths and nothing else. The change is
hand-written, not generated, so no selection rule or postcondition script is needed. A canonical byte
domain is not material here: no clean/smudge filter, no EOL conversion and no generated payload is
involved, so ordinary `git diff` evidence answers the scoped question. A disposable candidate tree is
NOT required: the writes are small, scoped, and recoverable by the protocol below.

### Gate 6, mutation ownership and no-clobber recovery

`repo-AgentsCommander` is a shared room worktree, so the implementer must assume another agent may
touch it. Immediately before writing, re-check the branch, the base and `git status --porcelain` for
the phase's paths. Copy each file the phase will edit into the session scratchpad first. On failure,
restore only those paths and only after proving their current bytes are still this run's output
(compare against the scratchpad copy, and against the post-edit `sha256sum` the phase recorded); if
the bytes differ, stop, preserve them and report the conflict. Broad `git reset`, unconditional
`git restore` and repository-wide cleanup are forbidden as recovery.

Enhanced controls (OS-handle exclusion, file-identity proofs, cooperation locks, compare-and-swap
writers, per-path mutation ledgers) are NON-APPLICABLE: no destructive migration and no demonstrated
concurrent mutation on these paths.

### Gate 7, bounded execution and durable diagnostics

`npm test` and `npm run typecheck` are run through the session's own tool timeout; neither reads
stdin. On failure, keep the full stdout and the exit code outside the scratchpad before reporting. A
timed-out or failed command is reported as failed, never as passed.

### Gate 8, evidence discipline

Zero and absence are typed states in this plan and each is asserted, not assumed: an empty
blocked-set must dismiss the aggregated toast and must not flash; a session with `communication: null`
in the listing must clear the store entry; a phase whose `git status` is empty for a path it claimed
to change is a failure, not a pass. Every acceptance command below states its expected result and its
failure behaviour. What cannot be verified locally is named in each phase under "Not automatable".

One rule binds ALL FOUR phases, not only the one that states it at length: never write a test whose
declared failure mode is a hang, and never write an acceptance criterion whose check is "it hangs".
A hang is not a deterministic death, an unbounded synchronous loop wedges the vitest worker instead of
failing it, and an implementer cannot honestly tick a box that cannot be observed. Every revert named
in any phase's criterion 6 must die on an assertion, with a number or a name in the failure output.
P2 explains why, because that is where it was violated in the first round; P1, P3 and P4 inherit the
rule even though none of their reverts can currently hang. No linter can enforce it: a reviewer
enforces it by reading the implementer's output.

## Phase table

Trigger note: the coordinator's `code-implementation-workflow` Partition rule is in another agent's
Matrix and is not readable from here, so the partition threshold itself could not be consulted. The
cut below applies the `plan-partitioning` cut rules directly: by owner (one owner, frontend, so no
owner cut), by contract (no IPC, CLI, persistence or schema change in any phase), by green-tree
boundary (each phase ends compiling and green on landed code only), and by budget (largest phase is
six files, under the ten-file rule).

| id | child issue | class | owner | files | depends-on | parallel-with |
|----|-------------|-------|-------|-------|------------|---------------|
| P1 | 1856 | patterned | frontend | 3 | none | none |
| P2 | 1857 | design-bearing | frontend | 6 | P1 | P3 |
| P3 | 1858 | patterned | frontend | 6 | P1 | P2 |
| P4 | 1859 | patterned | frontend | 5 | P3 | none |

Phase digests, `sha256sum` over LF bytes, verified against the blob and not the working file for the
reason given under gate 3:

```
Phase-SHA256: p1-communication-reconcile=0FA2919C2F455D9B106CC5E3B917561AF9B3A009C8E8EA1ED7B028BF5189E8C5
Phase-SHA256: p2-aggregated-toast-and-attention=E4FEC5826DC2A82946509D423ED3EBD28B44D68D595334ADCF2E48EC7AFC5835
Phase-SHA256: p3-blocked-menu-glyph=7394CA56935D84122C31A34F4F322B698CE336FFA3219451ED8A3CA1290A7DD5
Phase-SHA256: p4-collapse-proof-rollups=02040A063AB3A5603BD4FB7934E13CE1A52C9A9A40313318900F70EB9334EF4E
```

P2 and P3 touch disjoint files and may run in parallel. P4 follows P3 because both edit
`src/sidebar/components/ProjectPanel.tsx`. Recommended serial order with a single implementer is
P1, P2, P3, P4, which is the order the tech lead asked for.

## Gate 3 resolution and certification

Gate 3 required issue linkage, and `scripts/validate-branch-name.mjs` enforces it in CI: a branch
name must carry a number that resolves to an OPEN issue in `mblua/AgentsCommander`. At the time this
plan was first written no such issue existed, and that was the single blocker.

Resolved. Parent 1855 and children 1856, 1857, 1858 and 1859 exist and were each confirmed `OPEN` by
`gh issue view <n> --repo mblua/AgentsCommander --json number,title,state` before this file was
finalized. Every other applicable baseline gate above already carried its evidence, owner and failure
behaviour, and every enhanced control is recorded non-applicable with its reason.

### Round 2, what the consensus round changed

The first certification was rejected by both reviewers, independently, on the same central defect. All
five files changed and all five digests moved. The corrections, so a re-reviewer knows where to look:

- P2's claim that removing `untrack` causes a hang was WRONG. Measured on the pinned `solid-js`, the
  effect runs once with `untrack` and twice without, and settles either way. The claim, test 19 and
  acceptance criterion 6 are rewritten around an exact effect-run count. `untrack` stays, as hygiene.
- P2 introduced a REGRESSION the first version did not see: a memo that filtered only on `kind` and
  `visible` was subscribed to neither `message` nor `updatedAt`, so a session moving from one blocking
  menu to another would have kept the old toast text forever. The memo now returns value objects that
  read both fields in tracked scope, and test 21 is the falsifier.
- P2 change 4 deleted lines 731-759; the `else` closes on 760.
- Eleven smaller corrections across all five files, each recorded in place.

Nothing in the design changed. No product decision was reopened.

### Round 3, what the second review round changed

One reviewer approved round 2's bytes; the other required two changes. Both are in, plus eight smaller
corrections. All five files changed again.

- Test 19's spy is now filtered by the aggregate's tag. `vi.spyOn` replaces a property on a SHARED
  object and `toastStore.error`, `.info` and `.success` call `push` back through it, so the unfiltered
  counter measured the whole sidebar, not this effect. Eighteen sites reachable from a mounted `App`
  could have moved it; P2 enumerates all eighteen.
- The eviction trade-off now has acceptance criterion 8 forcing it, and names BOTH stale comment
  blocks in `toasts.ts`, not one. The second, above the victim search, contains the sentence tier 2
  falsifies. An instruction without a criterion is an intention.
- P2 also carried a surviving false claim from round 1, in "Why there is no runtime self-healing
  loop", which still justified the ban with an infinite loop that had already been measured away. It
  is corrected; the two real reasons stand on their own.

Nothing in the design changed. No product decision was reopened.

### Round 4, what the third review round changed

Both reviewers approved round 3's bytes and then asked for three non-blocking items, two of them
independently. All of them are in, and only P2 and this file changed.

- The re-arm restriction got its reason back. The round-3 line trim had kept the rule and dropped the
  clause that says why it exists, which is the only thing that lets an implementer resist widening it.
- The site census under test 19 was INCOMPLETE, not wrong: thirteen listed, eighteen real. The five
  missing ones are the worst to omit, two of them in the very file P2 edits, and one of those is a
  session-warning error toast. Re-derived here: nineteen non-test matches of
  `toastStore.(error|info|success|push)(` under `src/sidebar/`, minus `src/sidebar/App.tsx:736` which
  change 4 deletes.
- `BLOCKED_MENU_TAG` was described as both component-scope and module-level. It is now module-level
  and exported, so the test can import it, while the flash bookkeeping Set stays in component scope
  because it is per-instance state.

Nothing in the design changed. No product decision was reopened.

### Round 5, what the fourth review round changed

One reviewer approved round 4's bytes; the other blocked on two words. Four edits, only in this file
and P2. `p1`, `p3` and `p4` are byte-identical to round 3 and carry two approvals each.

- The Compatibility section above named ONE stale comment block when round 3 had established there
  are two. That correction landed twice in P2 and never here. It is the fourth time a repair landed
  where it was pointed and a copy survived elsewhere, and this file's own sentence promises the
  opposite ("named here so nobody discovers them later").
- P2's census recipe omitted its directory scope, so re-running it over `src/` returned 22 instead of
  19. The number and the list were right; the recipe did not reproduce.
- The timer sentence above described an intermediate state. Before P2 a dismissed tagged toast had no
  auto-dismiss timer to lose; it died at t=180.
- The reason given in P2 for keeping the flash bookkeeping Set in component scope did not survive
  measurement. A module-level Set passes tests 16 and 17 under the order this harness actually
  produces, because the first effect run finds an empty list and clears the Set. The unconditional
  reason, also measured, is that two `App` instances mounted at once would share it and the second
  would never flash. The decision did not change; the justification did.

Nothing in the design changed. No product decision was reopened.

### Round 6, the status line

Both reviewers approved round 5's bytes without conditions. This round adds no content: every one of
the five files now carries `Status: READY_FOR_IMPLEMENTATION` on its own line in its header block, so
a phase file states from inside that it is certified. A cold-start implementer reads one phase file
and nothing else, and until now none of the four said so.

All five digests moved, unavoidably: the line goes in every file. Two other changes in P2, both
mechanical, both offered by the reviewers: the census paragraph was re-wrapped (one line had drifted
to 138 columns against the file's ~100, so the 400-line count was partly paid by an over-wide line),
and the `untrack` measurement passage was tightened by three lines to pay for the header line. No
fact, citation, test, criterion or preserve-list entry was removed.

Verdict: READY_FOR_IMPLEMENTATION.

Two standing conditions, neither of which reopens the design. The open-issue check re-runs on every
push, so closing a child issue before its PR merges will fail its own branch. And the base SHA above
is the pinned base for this round: ordinary movement of `main` does not invalidate this plan, but
before the first product write and again before each PR is opened, fetch the live target and check
whether the drift touches the files this epic names, the frontend workflows, or the required-check
configuration. If it does, refresh only the affected evidence; if it does not, record it and move on.
