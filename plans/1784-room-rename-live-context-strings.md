# 1784 - Room rename incomplete: 5 stale workgroup/WG strings still reach live sessions

Status: READY_FOR_IMPLEMENTATION
Issue: https://github.com/mblua/AgentsCommander/issues/1784
Base: `eabaf21a2a98afffb55cf036999758f390c070b6` (`main`)
Branch: `fix/1784-room-rename-live-context-strings`
Author: ac-dev-rust-v4. Reviewer: ac-dev-rust-grinch-v4 (Lite band, plan reviewed).
Round 1 reviewed plan bytes `f17003c58a113dba2b4c23d691033faeb905e9e1fc3832047386ce5bc0131ea5`
and round 2 reviewed `b6358c022d621c20eee49d3efaee3c6f6a109ab64bf970fc324c5aa48496e034`; both
returned CHANGES_REQUIRED against the prose only. In both rounds all sixteen pinned values,
section 4, section 5's fence, the count table, the frozen digest and the byte pin were
re-derived independently and reproduced exactly. This is round 3. No prescribed byte moved.

## 1. Objective

Five stale "workgroup" / "WG" / `wg-` occurrences on four lines of
`src-tauri/src/config/session_context.rs` are rendered into live session context.
Reword all five into Room vocabulary, update the one behavioural test that pins one
of them, and change nothing else. The frozen historical copies of that same prose
must survive byte-identical.

## 2. Cause

The Room rename swept the replica-facing arms and left the Root Agent arm and the
privileged-PTY block behind. `session_context.rs:3909` is the worst case: it states
the peer shape as `` `<project>:<room>/<agent>` `` and then illustrates it with
`agentscommander:wg-15-dev-team/tech-lead`, so a Root Agent copying the example
produces a name `list-peers-lean` no longer returns. The sibling arm on the very next
line (`:3910`) already renders `agentscommander:room-15-dev-team/dev-rust`.

## 3. Scope

In scope, and nothing else:

- Four lines of `src-tauri/src/config/session_context.rs` carrying five occurrences.
- One assertion in `default_context_root_agent_documents_verified_wg_coordinators_only`,
  plus two assertions added to that same test (no test added, no test removed).

Out of scope, decided, not deferred:

- Frozen historical snapshots: `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`,
  `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME`, the `GLOBAL_CONTEXT_TEMPLATE_BEFORE_*` and
  `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_*` family in `seeded_context_templates.rs`, and the
  `OLD_*` / `*_BEFORE_ROOM_RENAME` constants in `root_agent.rs`.
- `legacy_rendered_default_context_for_generation` (`session_context.rs:4061-4317`,
  measured, see 7.2). Its `:4161` and `:4162` carry `agentscommander:wg-15-dev-team/...`
  and must not move: the function is the frozen reconstruction the self-heal classifier
  compares an on-disk context file against.
- Factual legacy statements: `WORKGROUP_GIT_SCOPE`, "or `wg-<N>-*` if legacy",
  "(legacy: `<wgN>`)", and the `--send` root wording.
- The public `wg` CLI surface: `purge-wg`, `--wg`, the clap aliases.
- Rust identifiers and internal doc comments, including the name of the modified test
  (`..._verified_wg_coordinators_only`), which stays as it is. Renaming is identifier
  churn; the issue rules it out.
- A drift-resistant Root-side mirror of `default_context_for_a_room_replica_says_room`
  (`:11311`), which fails on *any* line of the rendered replica context containing
  "workgroup". The two assertions this plan adds are fragment-pinned instead, so they catch
  this regression and nothing after it. Considered and rejected on cost alone, and not because
  the guard could not be written. Its precondition holds, and was established statically in
  round 2: every remaining case-insensitive "workgroup" hit after E1 to E5 sits in a comment,
  an identifier or variant, a frozen constant, or a Workgroup-only branch, and none of them is
  on the Root render path. That path is `default_context` (`:3988`) ->
  `render_default_agent_context` (`:2778`) -> `render_agent_context_template` (`:2643`) ->
  `render_agent_context_template_inner` (`:2666`) -> `default_context_dynamic_values`
  (`:3761`), whose `is_root_agent` branch at `:3900` selects `ROOT_GIT_SCOPE` and never
  `WORKGROUP_GIT_SCOPE`; that chain was re-walked from the base bytes while revising this
  plan. What rules the guard out is only that it is not free: a fourth added line
  moves the hunk to `+6395,4`, the numstat to `8 5`, and the section 7.6 sha256 and byte
  count, every one of which is already measured and pinned. A drift-resistant Root guard is
  worth filing, as its own issue, against a base that is not carrying five pinned edits.
- `scripts/room-rename-allowlist.tsv`. It carries verbatim copies of all five literals
  (rows 2190, 2228, 3155, 3156, 3157, 3162) as a frozen #1614 census artifact derived at
  `d7008b34` and refreshed at `df494bfa`. Its `gate` subcommand is not referenced anywhere
  outside `scripts/`, so it is not a CI gate. It is not edited and not run here.

## 4. The decided wording, as literal bytes

Each edit is an exact-substring replacement. Every OLD string below was measured to occur
exactly the stated number of times in `session_context.rs` at base, and every NEW string
was measured to occur zero times at base, so each edit is unambiguous when applied as a
whole-file substring replacement.

### E1 - `:2608`, inside `PTY_INPUT_COORDINATOR_CONTEXT` (const head at `:2606`)

OLD (1 occurrence in file):

```text
in this same project and workgroup.
```

NEW:

```text
in this same project and room.
```

Lowercase `room` matches the same sentence, which already says "identity-verified room
orchestrator replica", and `:2624` ("room, message its orchestrator").

### E2 - `:3473`, the `const ROOT_PROJECT_SCOPE_ENTRY` line

OLD (1 occurrence in file):

```text
workgroup directories, messaging directories
```

NEW:

```text
room directories, messaging directories
```

Lowercase, matching the surrounding lowercase noun list.

### E3 - `:3481`, the `const ROOT_AUTHORITY_SECTION` line

OLD (1 occurrence in file):

```text
other agents, workgroup orchestrators, tech-leads
```

NEW:

```text
other agents, room orchestrators, tech-leads
```

Lowercase, matching the surrounding lowercase list ("other agents, ... tech-leads, peers").

### E4 - `:3909`, `MessagingContextMode::Root(_)` arm of `peer_name_format`

The bare example string `agentscommander:wg-15-dev-team/tech-lead` occurs **twice** in the
file (`:3909` live, `:4161` frozen). The anchor below includes the shape fragment that only
the live line carries (`<room>` where the frozen line has `<workgroup>`), so it is unique.

OLD (1 occurrence in file):

```text
`<project>:<room>/<agent>`, e.g. `agentscommander:wg-15-dev-team/tech-lead`.
```

NEW:

```text
`<project>:<room>/<agent>`, e.g. `agentscommander:room-15-dev-team/tech-lead`.
```

`room-15-dev-team` and not some other room name: the non-Root arm on `:3910` already uses
`agentscommander:room-15-dev-team/dev-rust`, so the two arms stay a matched pair.

### E5 - `:3909` and `:6395`, both occurrences

OLD (2 occurrences in file, at `:3909` and `:6395`; both change):

```text
non-orchestrator WG replicas
```

NEW:

```text
non-orchestrator Room replicas
```

Capital `Room` and not lowercase: on `:3909` itself the replica category is already spelled
"verified **Room** orchestrator replicas only", and the sibling arm on `:3910` labels the
category "**Room replicas**". Lowercase here would leave `:3909` internally inconsistent in
exactly the way the issue objects to. `:3909` is not otherwise recased; the untouched half of
that line is already correct.

The frozen `:4161` says "non-**coordinator** WG replicas", a different literal, so it is not
matched by E5.

### Resulting `:3909` in full

```text
        MessagingContextMode::Root(_) => "- **Root Agent sessions**: verified Room orchestrator replicas only, shaped `<project>:<room>/<agent>`, e.g. `agentscommander:room-15-dev-team/tech-lead`. Origin orchestrators and non-orchestrator Room replicas are not valid Root Agent targets in #277.".to_string(),
```

## 5. The test change

`src-tauri/src/config/session_context.rs`, test `default_context_root_agent_documents_verified_wg_coordinators_only`
(`#[test]` at `:6390`, `fn` at `:6391`). E5 rewrites its second assertion. Two assertions are
added immediately after the first one. The test body becomes, verbatim:

```rust
    #[test]
    fn default_context_root_agent_documents_verified_wg_coordinators_only() {
        let out = default_context("C:/fake/ac-root-agent", None, &no_skill_section());

        assert!(out.contains("verified Room orchestrator replicas only"));
        assert!(out.contains("e.g. `agentscommander:room-15-dev-team/tech-lead`."));
        assert!(!out.contains("wg-15-dev-team"));
        assert!(out.contains("Origin orchestrators and non-orchestrator Room replicas are not valid Root Agent targets in #277"));
        assert!(out.contains("Use only the JSON `name` values returned by `list-peers-lean`"));
    }
```

Why the two added assertions are required rather than optional: the suite at base pins no
part of the `wg-15-dev-team` example, so without them a branch that skipped E4 entirely is
green. They add zero new tests, so the suite total does not move.

E4 is not, however, the one edit the suite is blind to. The suite is equally blind to E1, E2
and E3. Measured at base: `PTY_INPUT_COORDINATOR_CONTEXT` has exactly two references in the
repository, its definition at `:2606` and the `push_str` at `:2280`, and no test names it;
and each of the E1, E2 and E3 OLD literals occurs exactly once under `src-tauri`, at its own
definition line, with no test asserting any fragment of any of the three. The only other copy
of each is its `scripts/room-rename-allowlist.tsv` row. Corroborating E1 specifically,
`default_context_for_a_room_replica_says_room` (`:11311`) fails on any line of the rendered
replica context containing "workgroup" and is green at base while `:2608` contains
"workgroup", so E1's constant does not reach that render either.

What distinguishes E4 is therefore not that it alone is untested. It is the only edit that
carries the blind-spot risk **and** the destroy-the-frozen-copy risk, because its literal has
a frozen twin at `:4161`. Nothing here weakens the gate: E1, E2 and E3 are caught by Leg 1's
hunk anchors at `-2608`, `-3473` and `-3481` and by Leg 3's after-count rows at those same
three lines, and both legs are already mandatory.

Both are satisfiable, argued from the source rather than from a run:

- The positive one. The existing `:6394` assertion pins a fragment of the same single string
  literal on `:3909`, so the whole of that literal is present in `out`. The new fragment is
  part of the same literal after E4.
- The negative one. After the change, `wg-15-dev-team` survives only at `:4161` and `:4162`,
  which live inside `legacy_rendered_default_context_for_generation`. A caller trace of that
  function returns only the compat/classifier chain and its own tests; `default_context` is
  not among them, so it cannot appear in `out`. The negative assertion is deliberately the
  narrow `wg-15-dev-team` and not a broad `wg-` sweep, because the Root render legitimately
  still carries "(legacy: `<wgN>`)".

## 6. Required behaviour and failure behaviour

Behaviour: a pure string-literal content change. No control flow, no signature, no public
surface, no serialization key. Every Root Agent render and every verified room orchestrator
render carries Room vocabulary in the five places.

Failure behaviour, stated because it is the reason for the do-not-touch list: if
`legacy_rendered_default_context_for_generation` moves by one byte, every pre-#1369
`Context.AgentsCommander.md` reclassifies from `StaleGenerated` to `NotLegacy`, never
self-heals again, and keeps its pre-rename write restrictions permanently. That failure is
silent at runtime. It is caught only by the digest in 7.2.

Known limitation, carried from the issue and not addressed here: these constants are the
source. A room whose context file is already materialised on disk is rewritten only if the
classifier can prove it is an unedited AC-generated default. Edited files keep the old
wording. That population is not measurable from the source tree.

## 7. Proof

The suite alone does not decide this change, so the proof is four independent legs. Legs 1,
2 and 3 run without building.

### 7.1 Leg 1 - diff shape (the primary discriminator)

From the repo root, against base `eabaf21a`:

```bash
git diff --numstat eabaf21a2a98afffb55cf036999758f390c070b6 -- src-tauri
git diff -U0 eabaf21a2a98afffb55cf036999758f390c070b6 -- src-tauri/src/config/session_context.rs | grep '^@@'
```

Required, exactly one numstat row:

```text
7	5	src-tauri/src/config/session_context.rs
```

and exactly five hunk headers, in this order, with these old-side line numbers:

```text
@@ -2608 +2608 @@
@@ -3473 +3473 @@
@@ -3481 +3481 @@
@@ -3909 +3909 @@
@@ -6395 +6395,3 @@
```

Also run the same numstat with no pathspec (`git diff --numstat eabaf21a`) to confirm nothing
outside `src-tauri` moved. Once the plan file is committed, that unscoped form lists a second
row for `plans/1784-room-rename-live-context-strings.md`; that row is expected and is the only
additional row permitted.

This leg is what separates the three outcomes the issue names. Editing a frozen copy instead
of the live text produces a hunk at `-4161` or `-4162`, not at `-3909`. Editing both produces
six hunks. Any other source file produces a second numstat row under the `src-tauri` pathspec.

### 7.2 Leg 2 - the frozen region digest

`legacy_rendered_default_context_for_generation` spans `:4061` (`fn legacy_rendered_default_context_for_generation(`)
through `:4317` (the first bare `}` at or after the head). The repository already ships the
digest that pins it, in `legacy_rendered_default_context_for_generation_source_is_frozen`
(`:11384`): normalised length `15027` bytes and

```text
sha256 940fa35733c78cdf513391e5aed64438afd50fe6472a26e4d317270e5ee716c2
```

after replacing the single occurrence of `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` with
`WORKGROUP_GIT_SCOPE`. Both values were reproduced independently from the base bytes while
writing this plan, and both must still hold after the change. This is the leg that
distinguishes "the live example was fixed" from "the frozen example was fixed" by content
rather than by position.

It can be recomputed without building, but only under two rules, because the natural non-Rust
recomputation gets the length wrong while the sha256 matches perfectly:

- `15027` counts **UTF-8 bytes**, because Rust's `String::len()` does. The region holds 18
  non-ASCII characters (U+2014 and U+2264) contributing 36 bytes, so a recomputation whose
  string length is UTF-16 code units or characters returns `14991` and reports a spurious
  failure on a correct implementation. Measure the UTF-8 byte length explicitly.
- The region runs from the head line `fn legacy_rendered_default_context_for_generation(` to
  the **first line that is exactly `}`** at or after it, which is what the shipped test's
  `.lines().position(|line| line == "}")` means: a column-0 closing brace alone on its line.
  At base that is `:4061` through `:4317` inclusive, each line re-joined with a trailing `\n`.

Both values were reproduced by hand from the base bytes under exactly those two rules, once
while writing this plan and again while revising it, and both times the 18 characters were the
only thing standing between `14991` and `15027`. If you would rather not reimplement the rule,
AC3 is satisfied just as well by `cargo test` running
`legacy_rendered_default_context_for_generation_source_is_frozen`, which is the shipped
implementation of it.

### 7.3 Leg 3 - the asymmetric count table

Run over `src-tauri/src/config/session_context.rs` only, with fixed-string matching. The
figures are **occurrence counts**, the `String::matches().count()` semantics. `rg -c -F`
counts matching *lines*, which is not the same thing, and may be substituted only because no
hit line in this table carries a probe twice: both semantics were measured for all twelve
rows, at base and in the end state, and they coincide. Should a line ever carry a literal
twice the two would diverge, and the occurrence count is what the criterion means.
Zero-hit probes print nothing under `rg -c`;
treat a missing line as `0`, not as an error. Every "before" figure below was measured at
base; every "after" figure was measured on a fully applied copy of the change.

| probe (fixed string) | before | after | after: which lines |
|---|---|---|---|
| `in this same project and workgroup` | 1 | **0** | - |
| `in this same project and room` | 0 | **1** | 2608 |
| `workgroup directories, messaging directories` | 1 | **0** | - |
| `room directories, messaging directories` | 0 | **1** | 3473 |
| `other agents, workgroup orchestrators, tech-leads` | 1 | **0** | - |
| `other agents, room orchestrators, tech-leads` | 0 | **1** | 3481 |
| `agentscommander:wg-15-dev-team/tech-lead` | 2 | **1** | 4161 |
| `agentscommander:room-15-dev-team/tech-lead` | 0 | **2** | 3909, 6395 |
| `non-orchestrator WG replicas` | 2 | **0** | - |
| `non-orchestrator Room replicas` | 0 | **2** | 3909, 6397 |
| `non-coordinator WG replicas` | 2 | **2** | 4161, 4178 |
| `wg-15-dev-team` | 3 | **3** | 4161, 4162, 6396 |

Two rows in that table are the point of it:

- `agentscommander:wg-15-dev-team/tech-lead` must go to **1**, not to 0. Zero would mean the
  frozen copy at `:4161` was destroyed, which is the failure mode, not the success condition.
- `wg-15-dev-team` must stay at **3**. It does not fall to 2, because the new negative
  assertion at `:6396` contains the literal it forbids. An acceptance criterion expecting
  `3 -> 2` here is wrong and would fail a correct implementation.

Repo-wide the old literals do **not** reach zero: `scripts/room-rename-allowlist.tsv` keeps
one copy each of E1, E2, E3 and two copies each of E4 and E5, unchanged. Restrict this leg to
`src-tauri/src/config/session_context.rs`.

### 7.4 Leg 4 - the build and the suite

```bash
cd src-tauri
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

`cargo fmt --all -- --check` was pre-verified: `rustfmt 1.9.0-stable --edition 2021 --check`
returns exit 0 with an empty diff on both the base file and a copy carrying the full change,
so none of the prescribed bytes is reflowed. There is no `rustfmt.toml`, so defaults apply
(`max_width` 100, `fn_call_width` 60), and the long lines here are single string literals that
rustfmt leaves alone.

Named tests that must still pass untouched:
`frozen_pre_room_rename_coordinator_template_is_recognized`,
`frozen_snapshots_are_byte_exact_at_d7008b34` (both the `root_agent.rs` and the
`seeded_context_templates.rs` copies), `legacy_rendered_default_context_is_frozen`,
`legacy_rendered_default_context_for_generation_source_is_frozen`,
`workgroup_git_scope_frozen_half_is_byte_exact`, and
`default_context_for_a_room_replica_says_room`.

### 7.5 What a satisfied check would look like while the change was wrong, and how each gap is closed

- **Leg 3 alone.** `agentscommander:wg-15-dev-team/tech-lead` going 2 to 1 is equally
  satisfied by editing the frozen `:4161` and leaving the live `:3909` alone. Closed by the
  "which lines" column (the survivor must be `4161`), by Leg 1 (the hunk must be at `-3909`),
  and by Leg 2 (the frozen digest must be unchanged).
- **Leg 4 alone.** This leg constrains the change almost not at all, and the worst case is
  worse than "skipped E4". The only fragment of the live text the suite pins at base is the
  one E5 rewrites (section 5), so **a branch that applied only E5 is fully green** - as, for
  that matter, is a branch that applied nothing. Closed by the two assertions added in
  section 5, by Leg 1's four other hunk anchors at `-2608`, `-3473`, `-3481` and `-3909`, and
  by Leg 3's after-count rows at 2608, 3473 and 3481 together with the
  `agentscommander:room-15-dev-team/tech-lead` = 2 row.
- **Leg 1 alone.** `7 5` with five hunks at the right line numbers is equally satisfied by
  five single-line edits carrying the wrong wording. Closed by Leg 3's positive rows and by
  the byte pin in 7.6.
- **Leg 2 alone.** The digest is unchanged by any edit outside `:4061-:4317`, including doing
  nothing at all. Closed by Legs 1 and 3.
- **A frozen edit plus a same-length compensating edit.** Changes the digest in Leg 2 and adds
  a hunk in Leg 1.

No single leg is sufficient. All four are required.

### 7.6 Final byte pin

The complete intended end state was constructed and measured while writing this plan. After
the change, `src-tauri/src/config/session_context.rs` must be 12379 lines, LF, 544836 bytes:

```text
sha256      86b3b52909f80d8b1cb03eee405d33c0d339aee99c48369921fab6a3142910ee
git blob    4c1145e4d069c456088da68f03baedd6890cf5a3
```

For reference, the base file is 12377 lines, 544710 bytes,
sha256 `4a1aca47936c2fdeb3240f06bc6c681fc240e8577d55acb491556a342874243d`, git blob
`3717b0d50f1c472d54f5a33dfc18edba7af78b36`. Both base values were confirmed against
`git cat-file -p eabaf21a:src-tauri/src/config/session_context.rs`.

`.gitattributes` pins `*.rs text eol=lf` and the base blob is already LF, so hash the working
tree file directly; if a checkout produced CRLF, normalise `\r\n` to `\n` before hashing.

This pin is valid only against base `eabaf21a`, and so are AC1 and AC2. Section 9 says to open
a PR, so a merge before landing is the expected path and not an edge case. The pin is still
worth keeping: it fails loudly with a hash mismatch rather than silently, which is exactly why
it needs this caveat rather than removal.

Those three do **not** break together, and any reading of this section that says they do is
wrong. A docs-only commit on `main` breaks none of the three. A commit touching some other
file under `src-tauri` breaks AC1's one-row requirement on its own, while AC2 and AC8 still
hold. Only a commit touching `session_context.rs` itself breaks all three.

**Every base-derived criterion must be revalidated after a rebase or a merge**, and that is
AC1 through AC5, AC7 and AC8. Not naming `eabaf21a` is not evidence that a criterion survives:
AC3 pins content an upstream commit can edit, AC4 pins counts an upstream commit can move, AC5
pins the literal line number `3909`, and AC7 compares against a run captured before the merged
commits existed. Revalidate each of the following before judging any of them:

- **AC8 and the pin above: drop outright.** There is no substitute value; the file's bytes now
  include someone else's change.
- **AC1 and AC2: re-derive against the new merge base.** `git diff --numstat $(git merge-base
  HEAD main) -- src-tauri` must still print exactly one row for `session_context.rs`, and the
  diff must still be exactly five single-purpose hunks carrying exactly the five edits of
  section 4, at whatever old-side line numbers the merged file gives them. The row count and
  the hunk count carry over; the specific line numbers do not.
- **AC3: recompute both values at the merge base.** An upstream commit that edits any line
  inside `legacy_rendered_default_context_for_generation` changes the length and the sha256,
  after which `15027` and `940fa357...` would fail a correct implementation. Recompute the
  pair on the merge-base file under the two rules in 7.2, or let `cargo test` recompute it via
  `legacy_rendered_default_context_for_generation_source_is_frozen`, and adopt the result as
  the pinned pair. What carries over is that the pair must be *unchanged by this change*, not
  the two literal values.
- **AC4: re-measure every "before" figure on the merge-base file**, then re-derive each
  "after" from it by applying this change's own deltas. An upstream commit that adds or
  removes one occurrence of any probe shifts a whole row. The "which lines" column does not
  survive at all: every line number in it moves with any earlier insertion, so re-derive that
  column too.
- **AC5: re-locate `:3909` by content, not by number**, and update the number. Any upstream
  insertion above it shifts it. What AC5 means is the line text quoted at the end of section 4
  and the test body quoted in section 5; the line number is only a pointer to them.
- **AC6: unaffected.** It runs three tools over the current tree and references no base commit.
- **AC7: recapture the baseline `B` at the new merge base.** A `B` captured at `eabaf21a` is
  worthless after a merge. Concretely: if a merged commit brings in a test that fails on
  `main`, that failure is absent from the old `B` and present in `A`, and AC7 then reports
  upstream's behaviour as a #1784 regression. Recapture `B`, then apply AC7's retry rule
  against the new `B`.

  Recapturing does not need the edits stashed or reverted, and by this point they exist. Use a
  second worktree checked out at the merge base, run from the repo root:

  ```bash
  BASEDIR="$(mktemp -d)/ac-1784-base"
  git worktree add "$BASEDIR" "$(git merge-base HEAD main)"
  (cd "$BASEDIR/src-tauri" && cargo test) 2>&1 | tee "$BASEDIR.log"
  git worktree remove "$BASEDIR"
  ```

  The edited worktree is never touched. Three cautions. Give the directory a fresh unique path
  rather than a fixed one, because the temp directory is shared. If `git worktree remove`
  refuses because the tree is dirty, delete the directory with `rm -rf "$BASEDIR"` and run
  `git worktree prune`; do not reach for `git worktree remove --force`, which deletes whatever
  is at the target path. And the second worktree has its own `target/`, so expect a cold
  build.

## 8. Acceptance criteria

All are objective and mechanically checkable.

1. `git diff --numstat eabaf21a -- src-tauri` prints exactly one row:
   `7	5	src-tauri/src/config/session_context.rs`. With no pathspec, the only additional
   row permitted is `plans/1784-room-rename-live-context-strings.md` once it is committed.
   Subject to the rebase caveat in 7.6.
2. `git diff -U0 eabaf21a -- src-tauri/src/config/session_context.rs` produces exactly five
   hunks, with old-side starts `2608`, `3473`, `3481`, `3909`, `6395`, the last being
   `+6395,3`. Subject to the rebase caveat in 7.6.
3. The frozen-region digest of section 7.2 is unchanged: normalised length `15027`, sha256
   `940fa35733c78cdf513391e5aed64438afd50fe6472a26e4d317270e5ee716c2`. Compute it under the
   two rules stated in 7.2 (UTF-8 bytes, and the terminator is the first line equal to `}`),
   or let `cargo test` compute it via
   `legacy_rendered_default_context_for_generation_source_is_frozen`. Subject to the rebase
   caveat in 7.6.
4. Every row of the section 7.3 table holds at its stated "after" value, including
   `agentscommander:wg-15-dev-team/tech-lead` = 1 and `wg-15-dev-team` = 3. Subject to the
   rebase caveat in 7.6.
5. `:3909` reads exactly the line quoted at the end of section 4, and the test body reads
   exactly the block quoted in section 5. Subject to the rebase caveat in 7.6.
6. `cargo fmt --all -- --check`, `cargo check --all-targets` and
   `cargo clippy --workspace --all-targets -- -D warnings` all exit 0.
7. `cargo test` is green against a captured baseline, and the six tests named in 7.4 pass.
   Capture the baseline first: on a clean worktree at the base commit, before applying any
   edit, run `cargo test` from `src-tauri` and save the output, both the list of failures and
   the `test result:` lines. Call that set of failing tests `B`. Then apply the change, run it
   again, and call that set `A`. Without that capture a pre-existing failure is
   indistinguishable from a regression this change caused. Subject to the rebase caveat in
   7.6, which requires `B` to be recaptured at the new merge base.

   **The retry rule, and which run is authoritative.** If `A` is a subset of `B` and none of
   the six tests named in 7.4 appears in `A` or in `B`, AC7 passes, the after run is the
   authoritative run, and you stop there. If `A` is a subset of `B` but a named test does
   appear in either set, AC7 fails. Otherwise exactly one retry
   cycle is permitted, and it is mandatory before any failure is called a regression:

   1. For each test in `A` minus `B`, run it alone from `src-tauri`
      (`cargo test <full::test::path> -- --exact`) and record pass or fail.
   2. Re-run the whole suite once, unchanged, from the same worktree. Call that set of failing
      tests `A2`. **`A2` is the authoritative run.**

   AC7 passes if and only if `A2` is a subset of `B` and none of the six tests named in 7.4
   appears in `A2` or in `B`. A test in `A` minus `B` that is absent from `A2` is an observed
   flake: it is recorded in the PR by name with its step-1 isolated result, and it is not a
   regression. This repository has at least one known Windows-only network flake and that is
   the shape it takes. A test that is in `A2` and not in `B` is a regression and AC7 fails,
   **even if step 1 passed it in isolation**. An isolated pass never removes a test from `A2`
   and never on its own excuses a failure, because a change can break a test through the full
   suite's ordering or parallelism alone; step 1 is diagnostic evidence for the PR, not the
   verdict. There is no second retry cycle: if `A2` is still not a subset of `B`, AC7 fails
   and that result is reported rather than re-run.

   There is no separate test-count check: AC2 already proves the count is unchanged, since
   the only hunk in the test region is `-6395 +6395,3`, three added lines that are all three
   `assert!` statements with no `#[test]` among them, so no test can have been added or
   removed.
8. sha256 of `src-tauri/src/config/session_context.rs` equals
   `86b3b52909f80d8b1cb03eee405d33c0d339aee99c48369921fab6a3142910ee`, subject to the rebase
   caveat in 7.6.

## 9. Commit

`.gitignore:11` is `/plans/`. This plan file is untracked and invisible to `git status`,
`git add .` and `git add -A`. Stage it explicitly:

```bash
git add -f plans/1784-room-rename-live-context-strings.md
git add src-tauri/src/config/session_context.rs
```

Commit both together on `fix/1784-room-rename-live-context-strings`. Do not merge or push to
`main`; open a PR.
