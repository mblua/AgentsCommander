# Plan #1745: sidebar running-peer RUNNING chips must render after the repo/branch chips

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/1745 (OPEN)
- Band: 1 to 25 (`task-technical-scoring` final score 7). Delivery path: Lite. No `Plan-SHA256` freeze in this band.
- Repository: `repo-AgentsCommander`
- Base: `main` @ `302e6c12cad6f1701bbd937c38475ad376fe2afb`
- Branch: `fix/1745-running-chips-after-repos` (validated: `node scripts/validate-branch-name.mjs --branch fix/1745-running-chips-after-repos` prints `[branch-name] OK`)
- Implementation owner: `ac-dev-webpage-ui-v4`
- Adversarial reviewer: `ac-dev-rust-grinch-v4`

Every line and byte quoted in this plan was read at `302e6c12`.

## 1. Issue and objective

In the sidebar quick-access coordinator row, the chip strip `.ac-discovery-badges` renders the
`<peer> RUNNING` chips before the repo/branch chips. The user asked, in Spanish, for
*"Los chips de (Agents Running) tienen que estar abajo de todo. Luego de los (Repos)"*: the
`RUNNING` chips must be the last chips of the row, after the repo chips.

Objective: make every `.ac-discovery-badge.running-peer` chip render after every
`.ac-discovery-badge.branch` chip inside the same `.ac-discovery-badges` strip. Nothing else about
the row changes: same chips, same classes, same `title` attributes, same `data-ac-testid`, same
render gates, same CSS.

## 2. Cause

The two sibling blocks are emitted in the wrong order inside the strip. In
`src/sidebar/components/ProjectPanel.tsx`, inside `renderReplicaItem`:

- `:2529-2540` is the running-peer block, `<Show when={runningPeers && runningPeers()!.length > 0}>`
  wrapping `<For each={runningPeers!()}>` emitting `<span class="ac-discovery-badge running-peer">`.
- `:2541-2553` is the repo block, `<Show when={isCoord() && repoBadges().length > 0}>` wrapping
  `<For each={repoBadges()}>` emitting `class={...ac-discovery-badge branch...}`.
- `:2554` closes the strip `</div>`, so the repo block is the strip's last child today and the
  running-peer block is its immediate previous sibling.

There is no second cause. The order in the DOM is exactly the source order of these two blocks.
`.ac-discovery-badges` is `display: flex; gap: 4px; flex-wrap: wrap` (`sidebar.css:6058-6062`), and
no CSS `order` declaration applies to its children: the whole `src/` tree contains seven `order`
declarations, six in `src/main/styles/main.css` (`:35`, `:50`, `:72`, `:100`, `:104`, `:108`, all on
main-window layout regions) and one in `sidebar.css:6979`, which targets
`[data-sidebar-style="deep-space"] .ac-wg-subgroup > .replica-item:has(.ac-discovery-badge.coord)`,
that is whole rows inside a workgroup subgroup, not chips inside a row. So for the chip strip,
document order is visual order, and because the strip wraps, its last children land on the last line
of the block.

Supporting evidence, all verified at `302e6c12`:

- `runningPeers` occurs on exactly five lines in `ProjectPanel.tsx`
  (`grep -n runningPeers src/sidebar/components/ProjectPanel.tsx | wc -l` reports 5): the optional
  parameter declaration `:2321`, the render gate `:2529`, the `<For>` `:2530`, the per-item memo
  `const runningPeers = createMemo(...)` at `:2774`, and the single call site that passes it, `:2777`
  (`renderReplicaItem(item.replica, item.wg, item.wg.name, runningPeers, item.wg.taskTitle, "quick")`
  inside `.coord-quick-access`). The other call site, `:2601`, passes `undefined`. Only `:2529-2530`
  render anything.
- `repoBadges` occurs at exactly three places: the memo `:2338`, the gate `:2541`, the `<For>` `:2542`.
- `running-peer` occurs in `src/` at exactly seven places (`grep -rn "running-peer" src/ | wc -l`
  reports 7): `ProjectPanel.tsx:2533`,
  `ProjectPanel.chip-order.test.tsx:66` (a comment), `:200`, `:225`,
  `ProjectPanel.regex-filter.test.tsx:249` (a test title), and the two CSS rules
  `sidebar.css:6152` and `:6158`. `AcDiscoveryPanel.tsx` renders no running-peer chip.
- No CSS rule in the repo selects `.ac-discovery-badges` children positionally: a grep of the 65
  `ac-discovery-badge` mentions in `sidebar.css` (the only CSS file that mentions the class) returns
  no `:first-child`, `:last-child`, `:nth-*`, `:first-of-type`, `:last-of-type`, `+` or `~`
  combinator on those children. The one rule that reaches these children as a group,
  `sidebar.css:7457-7461`, is `display: none` applied identically to every child except
  `.agent-name-chip` and `.coord-communication-slot`, and only on non-coordinator rows in the
  `obsidian-mesh` style. It is order-independent and cannot reach a row that has a running-peer chip
  (such a row always carries `.ac-discovery-badge.coord`).

## 3. In scope and out of scope

In scope, exactly two files:

1. `src/sidebar/components/ProjectPanel.tsx`: swap the two sibling blocks at `:2529-2553`.
2. `src/sidebar/components/ProjectPanel.chip-order.test.tsx`: rewrite the four lines at `:200-203` to
   the new order. Of those four, two assertion lines change value and one comment line moves; see
   section 4.2 for the exact post-state.

Out of scope, and the implementer must not touch any of these:

- Any CSS. `sidebar.css:6152` and `:6158` (`.ac-discovery-badge.running-peer`) and
  `sidebar.css:6058` (`.ac-discovery-badges`) stay byte-identical. No forced line break, no new
  wrapper element, no `flex-basis: 100%` spacer, no `order` property, no media query.
- Any new element, class, attribute, memo, prop or import in `ProjectPanel.tsx`.
- `AcDiscoveryPanel.tsx`. It has its own `repoBadges` usage and renders no running-peer chip.
- `ProjectPanel.regex-filter.test.tsx`. See section 6.4.
- `ProjectPanel.menu-guard.test.tsx`. Its only strip reads are
  `expect(strip?.classList.contains("ac-discovery-badges")).toBe(true)` at `:127` and `:146`,
  which assert the identity of the strip element, not its children.
- `ProjectPanel.dirty-repo-badge.test.tsx` and `ProjectPanel.offline-badges.test.tsx`. Both read
  `.ac-discovery-badge`, and neither is order-sensitive. `dirty-repo-badge` resolves a single badge
  by test id and asserts its `className` at `:134` and `:175`; `offline-badges` builds
  class-filtered lists such as `root.querySelectorAll(".ac-discovery-badge.agent")` at `:88` and
  indexes only `[0]`, and every class it filters on sits at strip indices 0 to 8, before the edited
  region. Together with `chip-order`, `menu-guard` and `regex-filter`, plus
  `RootAgentBanner.agent-badge.test.tsx` and `SessionItem.test.tsx` (neither of which renders this
  strip), that is every test file under `src/` mentioning `ac-discovery-badge`
  (`grep -rln "ac-discovery-badge" src/ --include=*.test.tsx` returns exactly those six).
- `src-tauri/`. No Rust file, no `module-arcs.txt` entry: the change adds no import, so it adds no
  module arc.
- Historical documents that show the old order: `plans/1730-sidebar-row-chips.md` and the
  `.visual-specs/1730/*.html` mockups. They are the record of a shipped round, not a live
  specification, nothing imports or tests them, and they are not updated by this change.
- `dist/`. Build output, not a source of truth.

## 4. Decided solution

Move the running-peer `<Show>`/`<For>` block so it renders after the repo `<Show>`/`<For>` block,
inside the same `<div class="ac-discovery-badges">`. This is a pure reorder of two adjacent sibling
blocks: 12 lines moved after 13 lines, no line rewritten, no line added, no line removed.

The alternative reading (forcing the RUNNING chips onto a line of their own with a spacer or a CSS
rule) is decided against and is not to be reopened. The strip already wraps, so placing the chips
last puts them at the end of the block, after the repos, which is what the issue asks for.

### 4.1 Exact post-state of `ProjectPanel.tsx:2529-2553`

After the change, lines 2529 to 2553 must read exactly as follows (18 spaces of indentation on each
`<Show>` line, no other whitespace change). This region is byte-identical to the two blocks at
`302e6c12`, in swapped order:

~~~tsx
                  <Show when={isCoord() && repoBadges().length > 0}>
                    <For each={repoBadges()}>
                      {(repo, index) => (
                        <span
                          class={`ac-discovery-badge branch${repo.dirty === true ? " dirty" : ""}`}
                          title={formatReplicaRepoBadgeTitle(repo)}
                          data-ac-testid={repoBadgeTestId(repo.label, index())}
                        >
                          {formatReplicaRepoBadgeLabel(repo)}
                        </span>
                      )}
                    </For>
                  </Show>
                  <Show when={runningPeers && runningPeers()!.length > 0}>
                    <For each={runningPeers!()}>
                      {(peer) => (
                        <span
                          class="ac-discovery-badge running-peer"
                          title={`${wg.name}/${peer.name}`}
                        >
                          {peer.name} RUNNING
                        </span>
                      )}
                    </For>
                  </Show>
~~~

Line 2528 (`</Show>` closing the `extraBadge` block) and line 2554 (`</div>` closing the strip) are
unchanged. `ProjectPanel.tsx` stays at 4461 lines.

**Line terminators.** The fence above is LF because it reproduces the *tracked blob*, and the tracked
blob is LF. `git config --get core.autocrlf` is `true` and `.gitattributes` has no `*.tsx` entry
(its `text eol=lf` rows cover `.husky/**`, `*.sh`, `*.toml`, `*.json`, `*.rs`,
`src-tauri/module-arcs.txt` and `docs/integrations/rtk_claude/hooks/*.js`, none of which match), so
the working tree and the blob differ in line terminator: `file src/sidebar/components/ProjectPanel.tsx`
reports "with CRLF line terminators" and the working-tree copy has CR 4461 / LF 4461, while
`git show 302e6c12:src/sidebar/components/ProjectPanel.tsx | file -` reports no CRLF. The same split
applies to `src/sidebar/components/ProjectPanel.chip-order.test.tsx` (working tree CRLF, CR 293 /
LF 293; blob LF) and therefore to both fences in section 4.2. This plan file itself is 100% LF.

Consequence for the implementer: write the region with the file's existing CRLF terminators, exactly
as any editor will do when it saves a CRLF file, and let `core.autocrlf` normalise on commit. The
fences reproduce the committed blob, so a working-tree comparison against them differs in 25 line
terminators (4 for section 4.2) on a *correct* implementation. Every fence comparison in the
acceptance criteria is therefore made against `git show HEAD:<path>` and never against the working
tree. No fence in this plan asks for a line-terminator change; the only whitespace claim that applies
to the working tree is the 18-space indentation.

The repository has no Prettier, no ESLint and no formatter step (there is no formatter dependency in
`package.json` and no lint job in `.github/workflows/pr-regression-gates.yml`), so no tool will
reformat this region. The fence above is authoritative.

### 4.2 Exact post-state of `ProjectPanel.chip-order.test.tsx:200-203`

Replace the current four lines

~~~tsx
    expect(classNames[9]).toBe("ac-discovery-badge running-peer");
    // Neither fixture repo is dirty, so both are the bare branch class.
    expect(classNames[10]).toBe("ac-discovery-badge branch");
    expect(classNames[11]).toBe("ac-discovery-badge branch");
~~~

with exactly these four lines:

~~~tsx
    // Neither fixture repo is dirty, so both are the bare branch class.
    expect(classNames[9]).toBe("ac-discovery-badge branch");
    expect(classNames[10]).toBe("ac-discovery-badge branch");
    expect(classNames[11]).toBe("ac-discovery-badge running-peer");
~~~

The comment moves up one line so it still sits directly above the two `branch` assertions it
explains. The file stays at 293 lines. No other line of the test file changes: not the header
comment, not the fixtures, not the `describe` title, not the enumerated token set at `:208-228`
(the same eleven tokens render, so that assertion is already correct), not `expect(classNames).toHaveLength(12)`.

## 5. Affected files and symbols

| File | Symbol | Change |
|---|---|---|
| `src/sidebar/components/ProjectPanel.tsx` | `renderReplicaItem`, JSX inside `<div class="ac-discovery-badges">`, lines 2529-2553 | swap two sibling `<Show>` blocks |
| `src/sidebar/components/ProjectPanel.chip-order.test.tsx` | `it("orders every chip of a maximal quick-access coordinator row")`, lines 200-203 | two index assertions re-valued, one comment repositioned |

No symbol is added, renamed, removed or re-signed. `renderReplicaItem`'s parameter list at `:2317-2324`
is unchanged, including `runningPeers?: () => AcAgentReplica[]` at `:2321`. The `repoBadges` memo at
`:2338` and `runningCoordinatorPeers` at `:358-362` are unchanged.

No shipped citation of a `ProjectPanel.tsx` line number is invalidated. There are exactly **four**
tracked citations outside `plans/`; `git grep -n "ProjectPanel\.tsx:" -- . | grep -v "^plans/"`
returns these four lines and nothing else:

| Citing site | Cited target | Position relative to the edited region |
|---|---|---|
| `src/sidebar/components/SessionItem.tsx:391` | `ProjectPanel.tsx:2271` | above 2529 |
| `src-tauri/src/cli/list_peers.rs:297` | `ProjectPanel.tsx:780-786` | above 2529 |
| `src-tauri/src/cli/list_peers.rs:305` | `ProjectPanel.tsx:60` | above 2529 |
| `scripts/room-rename-allowlist.tsv:23` | `ProjectPanel.tsx:2749` | **below** 2553 |

The fourth is a live, tracked script input, not a plan document, and its target is below the edited
region, so an "all targets are above the edit" argument does not cover it and is not used here. The
argument that does cover all four is the line-count one: the change is a pure reorder of two adjacent
sibling blocks wholly inside 2529-2553, it adds and removes no line, and `ProjectPanel.tsx` stays at
4461 lines. Therefore no line outside 2529-2553 moves, and all four cited targets (`60`, `780-786`,
`2271`, `2749`) still resolve to the same lines they resolve to today. Lines *inside* 2529-2553 do
move, and nothing cites them.

`scripts/room-rename-allowlist.tsv` needs no edit for a second, independent reason. Its allowlist key
is `(path, trimmed line content)`, not a line number: `scripts/room-rename-allowlist.mjs:16-18` says
"The allowlist key is (path, trimmed line content). Content, not line number, so the file survives
line drift. Trimming also absorbs the line-ending split". A reorder changes no line's content and no
line's leading whitespace, so every allowlist row for `ProjectPanel.tsx` still matches. The `:2749`
mention is inside a `#` comment describing a regeneration recipe, not a data row: `grep -n 2749
scripts/room-rename-allowlist.tsv` returns only line 23.

## 6. Required behavior, edge cases and failure behavior

### 6.1 Required behavior

Within one `.ac-discovery-badges` strip, for any rendered pair of a `.ac-discovery-badge.branch`
chip and a `.ac-discovery-badge.running-peer` chip, the branch chip precedes the running-peer chip
in document order. Everything before line 2529 in the strip keeps its current order and indices:
communication slot, profile-outdated badge, idle/AUTO-CLOSED/MANUALLY-CLOSED pill, `agent-name-chip`,
`coord`, `agent`, `profile-badge`, `ctx-badge`, `team`, that is indices 0 through 8 of the maximal
row.

Relative order inside each group is preserved. The repo `<For>` still iterates `repoBadges()` and
still stamps `data-ac-testid={repoBadgeTestId(repo.label, index())}` with the index within that
`<For>`, which is unaffected by where the block sits among its siblings. The running `<For>` still
iterates `runningPeers!()`, whose order comes from `runningCoordinatorPeers` filtering `wg.agents`.

### 6.2 Edge cases

| Case | Behavior after the change |
|---|---|
| Zero running peers, N repos (N >= 1) | The running gate `runningPeers && runningPeers()!.length > 0` is false, the block emits nothing, and the N repo chips are the strip's last children. Identical rendering to today. |
| N running peers (N >= 1), zero repos | The repo gate `isCoord() && repoBadges().length > 0` is false, the block emits nothing, and the N running-peer chips are the strip's last children. Identical rendering to today. Reached when a coordinator's session reports no `gitRepos` and `configuredReplicaRepoBadgesLive` yields none. |
| Both empty | The strip ends at whichever earlier chip last rendered. Identical rendering to today. |
| A dirty repo | The chip's class is `ac-discovery-badge branch dirty` (`:2545` before the change, `:2533` after it, per this plan's "read at `302e6c12`" convention; the `repo.dirty === true` ternary is carried across verbatim and is byte-identical on both lines). It is still a repo chip, so it still precedes every running-peer chip. No dirty-specific handling is added. |
| Non-quick row contexts (`:2601`, `rowContext` defaults to `"workgroups"`) | `runningPeers` is `undefined`, so the gate is false by its first conjunct and the block contributes zero children. The reorder is invisible on these rows: their strips render exactly the children they render today, in the same order. This is asserted by the third test of `chip-order.test.tsx`, which expects the worker strip's children to equal `["agent-name-chip"]`. |
| Non-coordinator row that somehow receives `runningPeers` | Not reachable today: the only call site that passes `runningPeers` iterates `filteredCoordinatorItems()`, whose replicas are coordinators. If it ever became reachable, the running block would render (its gate does not test `isCoord()`) while the repo block would not, and the running-peer chips would be last. That is the intended order and needs no extra gate. |
| Multiple running peers and multiple repos | All repo chips first in `repoBadges()` order, then all running-peer chips in `runningCoordinatorPeers` order. No interleaving is possible: they are two separate `<For>` blocks. |
| Very narrow sidebar | The strip wraps (`flex-wrap: wrap`). The running-peer chips occupy the tail of the block, so they land on the last line. Whether they share that line with a trailing repo chip depends on available width; this is accepted, the requirement is "after the repos", and forcing a dedicated line is explicitly out of scope (section 3). |

### 6.3 Failure behavior

The change introduces no new data access, no new gate, no new async work and no new reactive
dependency, so it introduces no new failure mode. Both blocks are independent siblings: `<Show>` and
`<For>` in SolidJS create their own reactive scopes, and neither block reads the other's memo, so
swapping them cannot create a stale closure, a duplicate listener or a disposal-order problem.
`repoBadges` (`:2338`) and `runningPeers` (created at the call site, `:2774-2776`) are both
`createMemo` and are both defined before the JSX in source order; that does not change.

If either memo throws, it throws exactly where it throws today, only later in the render for the
running-peer memo. No error boundary depends on sibling order.

### 6.4 Why `ProjectPanel.regex-filter.test.tsx` needs no change

It never reads the strip's children. Its running-peer coverage is `input(filterInput, "RUNNING")`
at `:296` followed by `expect(rendered.root.textContent).toContain("dev-rust RUNNING")` at `:299`,
which is a substring check over the whole subtree's text and is order-independent. The production
filter it exercises is `ProjectPanel.tsx:2108`:

~~~tsx
            matchesFilterText(runningCoordinatorPeers(item.wg, item.replica).map((peer) => `${peer.name} RUNNING`).join(" "))
~~~

That line builds its haystack from the workgroup data, not from the DOM, so it is unaffected by
render order. `:449` is a comment. This must be confirmed by running the file unchanged (AC 6), not
assumed.

## 7. Tests

No new test file and no new test case. The existing oracle is
`src/sidebar/components/ProjectPanel.chip-order.test.tsx`, the only test in `src/sidebar/components/`
that asserts the strip's child order by index. It has three tests:

1. `"orders every chip of a maximal quick-access coordinator row"` (`:170`). Its fixture is a
   coordinator with `taskTitle: "Coordinate"`, a working peer `dev-rust`, and a session with two
   clean repos (`alpha`/`main`, `beta`/`dev`), which lights all twelve slots. This is the test that
   changes, per section 4.2, and it is the test that pins the new order.
2. `"lands the auto-closed pill at index 2 with the chip still at index 3"` (`:242`). Asserts
   `children[2]` and `children[3]` only, both before the edited region. Must pass unchanged.
3. `"renders the chip as the only child on a worker row from the other call site"` (`:267`).
   Asserts the worker strip's children equal `["agent-name-chip"]`. Must pass unchanged. This is the
   non-quick row context regression guard.

## 8. Acceptance criteria

Objective and checkable. All commands run from the repository root on
`fix/1745-running-chips-after-repos`.

1. `git show HEAD:src/sidebar/components/ProjectPanel.tsx | sed -n '2529,2553p'` is byte-identical to
   the fence in section 4.1. Both sides are LF. This is pinned against the committed blob and not
   against the working tree because the working tree is CRLF under `core.autocrlf=true` while the
   blob is LF; see the line-terminator note in section 4.1. Additionally,
   `wc -l src/sidebar/components/ProjectPanel.tsx` reports `4461` (the count is the same either way,
   because a CRLF file still has one LF per line).
2. `git show HEAD:src/sidebar/components/ProjectPanel.chip-order.test.tsx | sed -n '200,203p'` is
   byte-identical to the second fence in section 4.2. Both sides are LF, for the same reason as
   AC 1. Additionally, `wc -l src/sidebar/components/ProjectPanel.chip-order.test.tsx` reports `293`.
3. `git show --numstat` on the implementation commit lists exactly two paths:
   `src/sidebar/components/ProjectPanel.tsx` and
   `src/sidebar/components/ProjectPanel.chip-order.test.tsx`. For both, insertions equal deletions.
   No `.css` path, no `src-tauri/` path, no `AcDiscoveryPanel.tsx`, no `regex-filter` or
   `menu-guard` test path appears. The plan file is committed in its own earlier commit, so
   `git diff --name-only main...HEAD` over the whole branch lists those two paths plus
   `plans/1745-running-chips-after-repos.md` and nothing else. The exact per-file insertion and
   deletion counts are pinned by AC 10; note that they are *not* the region sizes 25 and 4, because
   git reports the minimal edit script and several lines of each region are unchanged.
4. `git diff main...HEAD -- src/sidebar/styles/sidebar.css` is empty.
5. `npx vitest run src/sidebar/components/ProjectPanel.chip-order.test.tsx` exits 0 and reports
   3 passed, 0 failed.
6. `npx vitest run src/sidebar/components/ProjectPanel.regex-filter.test.tsx` exits 0 with the file
   unmodified.
7. `npm run typecheck` exits 0.
8. `npm test` exits 0. If it does not, the failing test must be reproduced on `302e6c12` before it
   can be attributed to anything other than this change. These are the two gating commands of the
   `frontend-regression` job (`.github/workflows/pr-regression-gates.yml:365` is
   `run: npm run typecheck`, `:381` is the `npm test` line). The job is not identical to running them
   locally: it also runs `npm ci` at `:362`, and it invokes `npm test` inside the #480 known-debt
   classifier guard with reporter flags,
   `npm test -- --reporter=default --reporter=json --outputFile.json=npm-test-results.json 2>&1 | tee npm-test.log`,
   so a local `npm test` exercises the same suite but not the guard's tolerate/fail classification.
9. Semantic restatement of AC 1, checkable by reading the maximal-row test's assertions: in the
   quick-access coordinator row's `.ac-discovery-badges`, every `.ac-discovery-badge.running-peer`
   chip has a higher child index than every `.ac-discovery-badge.branch` chip, the strip still has
   exactly 12 children, and the enumerated class-token set at `:208-228` is unchanged.
10. Nothing outside the two edited regions is touched. AC 1 and AC 2 pin bytes only inside
    2529-2553 and 200-203, and AC 3 pins only the file set, so without this criterion a stray
    one-line-for-one-line edit elsewhere in either file would satisfy AC 1, AC 2, AC 3 and AC 4.
    With default git settings (`diff.algorithm` is unset in this repository, so Myers; default three
    lines of context):

    - `git diff main...HEAD --numstat -- src/sidebar/components/ProjectPanel.tsx` reports exactly
      `12` insertions and `12` deletions, and
      `git diff main...HEAD -- src/sidebar/components/ProjectPanel.tsx | grep -c '^@@'` reports `2`,
      with the two hunk headers beginning `@@ -2526,18 +2526,6 @@` and `@@ -2551,6 +2539,18 @@`.
    - `git diff main...HEAD --numstat -- src/sidebar/components/ProjectPanel.chip-order.test.tsx`
      reports exactly `2` insertions and `2` deletions, in exactly `1` hunk, whose header begins
      `@@ -197,10 +197,10 @@`.

    These figures are measured, not estimated: the post-state of both files was constructed from the
    `302e6c12` blobs and diffed. They are stable under `--diff-algorithm=myers`, `histogram`,
    `patience` and `minimal`, all four of which give 12/12 in 2 hunks and 2/2 in 1 hunk. Two points
    the implementer and the reviewer must not "correct": the `ProjectPanel.tsx` diff is **two** hunks,
    not one, because the 13-line repo block sits between the two halves of the move and is longer
    than twice the three lines of context, so the hunks cannot merge; and the hunk headers start at
    2526 and 2551, not at 2529, because a header begins three context lines before the first changed
    line.
