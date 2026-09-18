# Plan #2129: the branch-stale notice names the remote, and never the same branch twice

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2129 (OPEN, wording only, no behaviour change)
- Repo: `repo-AgentsCommander`; branch `fix/2129-branch-stale-remote-wording`
- Base (frozen at authoring, 2026-09-18 UTC): `f54a9945bed296f7f07e1edbfedefaefe06bf851`
  (`git rev-parse HEAD`), tracked tree clean. Every line number below refers to that SHA; if a
  quoted line no longer matches, re-anchor on the quoted text, never on the number.
- Class: Lite (band 1-25, score 22), one phase, no partition. Owner: Rust implementer; coordinator
  `ac-tech-lead-v4`; architect certification: this file.
- Task class and threat model: **routine application-code change**. No supply-chain, signing,
  release or untrusted-host requirement is stated by the issue or the repository contract, so the
  baseline controls of `delivery-nonfunctional-invariants` apply: repository toolchain, scoped
  local `cargo` gates, clean Git evidence, and all configured required checks green on the exact
  PR-head SHA. No enhanced provenance control is applicable; any such finding is advisory.
- 4 modified files, 0 added, 0 removed: `src-tauri/src/pty/remote_watcher.rs`,
  `src-tauri/src/session/remote_alerts.rs`, `src-tauri/src/config/injected_messages.rs`
  (doc comment + the two transcribed byte pins and their lengths), and
  `src-tauri/src/phone/mailbox.rs` (**test module only**, section 7.2). Production code changes in
  the first three only. No wire shape, IPC, `src/shared/types.ts`, CSS, dependency, workflow,
  release or migration change.
- `plans/` is gitignored at the root: commit this file with
  `git add -f plans/2129-branch-stale-remote-wording.md`.

## 1. Problem and verified cause

Observed notice (2026-09-17, repo on `main`):

```
[AgentsCommander] mblua/AgentsCommander main is now 4 commits behind main as of 2026-09-17 15:42:21-03:00. Any CI running on this branch is validating an out-of-date base.
```

Verified at the frozen base:

- `DEFAULT_BRANCH_STALE_TEMPLATE` (`src-tauri/src/config/injected_messages.rs:75`) renders
  `%REPO% %BRANCH% ... behind %BASE%`. `render_with_template` (`:408`) is **pure token
  substitution**: a template can carry no conditional, so no wording alone can collapse the
  branch == base case.
- The values are bound once, at `src-tauri/src/phone/mailbox.rs:416-427`
  (`TOKEN_BASE, base_branch.as_str()`), from `InternalSystemNotice::for_remote_activity`.
- The single construction site of that notice is `notice_for`
  (`src-tauri/src/session/remote_alerts.rs:199-218`), which passes `transition.base_branch`
  (`:214`) straight through. `notice_for` has exactly one caller, `:153`.
- `base_branch` is produced by `apply_staleness` (`src-tauri/src/pty/remote_watcher.rs:1385-1435`)
  from `resolve_base_branch` (`:1266`): GitHub `repos/<nwo>.default_branch`, else the local
  clone's `refs/remotes/origin/HEAD` with `origin/` stripped, else the sentinel
  `DEFAULT_BRANCH_LABEL = "the default branch"` (`:68`).
- The comparison itself is server-side: `GhQuery::Compare` is
  `repos/<nwo>/compare/HEAD...<sha40>` (`:202-208`). The base of the comparison is **GitHub's**
  default branch, never a local `origin/*` ref, which is why `origin/main` would be a false label.
- #2131 (`bcd108f4`) already suppresses the notice when the branch equals a *resolved* base label
  (`:1405-1410`, `on_default_branch`), and deliberately fails open on the sentinel. So the
  surviving branch == base case at HEAD is precisely the sentinel one: branch `main`, label
  `"the default branch"` - and the reported text, from before #2131, is the resolved variant.
- Nothing says "remote" anywhere in the sentence, in either case.

Cause: the rendered `%BASE%` is a bare branch label with no remote qualification, and the only
place that can qualify it is the notice construction site, not the template.

## 2. Decision

**Qualify the value, not the template.** `%BASE%` becomes the *rendered comparison base*,
computed once in `remote_watcher` (the owner of the sentinel) and applied in `notice_for`.

Exact rendering rule, in this order:

| Case | `%BASE%` renders |
|---|---|
| 1. `base_label == DEFAULT_BRANCH_LABEL` (unresolved sentinel) | `the default branch on GitHub` |
| 2. `base_label == branch` (resolved, same name) | `its counterpart on GitHub` |
| 3. otherwise | `<base_label> on GitHub` |

Resulting texts:

- feature branch: `... feature/2083-2064-remote-alerts is now 4 commits behind main on GitHub as of ...`
- unresolved base on `main`: `... main is now 4 commits behind the default branch on GitHub as of ...`
- resolved base equal to the branch (defence in depth; #2131 normally suppresses it first):
  `... main is now 4 commits behind its counterpart on GitHub as of ...`

Why this and not the alternatives:

- **Not `origin/main`.** The answer comes from `repos/<nwo>/compare`, not from a local remote. A
  local remote may be named otherwise, be stale, or be absent; `on GitHub` is the true source.
  It also cannot be applied blindly, because the sentinel would render
  `origin/the default branch`.
- **Not a second message id** (`branch-stale-on-default`). The file's own design forbids twins:
  `DEFAULT_NOTICE_BLIND_GAP_TEMPLATE` (`:77-81`) states "A suffix id, not twin ids: with twins an
  operator who edits one and forgets the other gets two silently diverging texts". A second id
  would also bump `COVERAGE_VERSION` and double the operator surface for one sentence.
- **The template bytes do not change.** This is deliberate and load-bearing:
  `each_new_default_has_exactly_one_known_sha_and_it_matches_the_template`
  (`injected_messages.rs:3040-3058`) asserts `known_default_sha256.len() == 1` and that the single
  digest equals the sha256 of the current template. **There is no shipped-defaults history entry to
  add**: appending one makes that test fail by design ("a second one means the shipped bytes moved
  after shipping and pristine entries stop auto-refreshing"). Leaving the template byte-identical
  keeps `a2f428dc...` correct, keeps every operator-edited template working unchanged, and keeps
  reconciliation untouched.
- Operators who already edited the entry keep their own sentence and silently gain the qualified
  value, which is the intended graceful outcome for a wording-only fix.

Dependency direction: `session::remote_alerts` already imports `crate::pty::remote_watcher`
(`remote_alerts.rs:34`). The helper adds no new module-to-module arc, no new SCC member, and no
cross-boundary reference; the sentinel stays private to `remote_watcher`.

## 3. In scope / out of scope

In scope: the rendered `%BASE%` value, the `%BASE%` doc comment, the two transcribed byte pins and
their lengths, and the tests of section 7 (including new tests in `mailbox.rs`'s test module).

Out of scope: `DEFAULT_BRANCH_STALE_TEMPLATE` bytes, `known_default_sha256`, `SCHEMA_VERSION`,
`COVERAGE_VERSION`, the #2131 suppression rule, `DEFAULT_BRANCH_LABEL`'s own value, the chip/UI use
of `base_branch`, and the raw `base_branch` carried on `RemoteTransition` (it stays unqualified, so
identity comparisons such as `remote_watcher.rs:1013` and `:1405` are unaffected).

## 4. Exact changes

### 4.1 `src-tauri/src/pty/remote_watcher.rs` - the qualifier (new, next to `DEFAULT_BRANCH_LABEL:68`)

```rust
/// #2129 - the rendered `%BASE%` for a branch-stale notice. The comparison is
/// `repos/<nwo>/compare/HEAD...<sha>`, answered by GitHub, so the text says
/// GitHub and never `origin/<x>`: a local remote may be named otherwise, be
/// stale, or not exist. The branch name is never printed twice: an unresolved
/// base keeps its sentinel prose, and a base equal to the branch is named by
/// relation. The raw label is left untouched everywhere else, because the
/// #2131 suppression compares it for identity.
pub(crate) fn base_branch_display(branch: &str, base_label: &str) -> String {
    if base_label == DEFAULT_BRANCH_LABEL {
        format!("{DEFAULT_BRANCH_LABEL} on GitHub")
    } else if base_label == branch {
        "its counterpart on GitHub".to_string()
    } else {
        format!("{base_label} on GitHub")
    }
}
```

### 4.2 `src-tauri/src/session/remote_alerts.rs` - use it at the single construction site

Import: extend `:34` to
`use crate::pty::remote_watcher::{base_branch_display, RemoteTransition, TransitionKind};`.

In `notice_for`, replace `:214` (`transition.base_branch.clone(),`) with:

```rust
        // #2129 - branch-stale text only. CI kinds carry an empty base branch,
        // which `for_remote_activity` requires to stay empty (mailbox.rs:319).
        if matches!(kind, RemoteNoticeKind::BranchStale) {
            base_branch_display(&transition.branch, &transition.base_branch)
        } else {
            transition.base_branch.clone()
        },
```

(The empty-base invariant is asserted by `remote_activity_notice_rejects_a_base_branch_on_a_ci_kind`,
`mailbox.rs:13063`; the guard above is what keeps it true.)

### 4.3 `src-tauri/src/config/injected_messages.rs` - documentation and the byte pins

(a) `BRANCH_STALE_DOC_COMMENT`, replace line `135`:

```
#   %BASE%    default branch label, e.g. main
```

with exactly:

```
#   %BASE%    comparison base, already remote-qualified, e.g. main on GitHub
```

The line keeps the literal `%BASE%`, which
`known_messages_cover_every_id_and_document_every_token` (`:3026-3037`) requires.

(b) The same line occurs inside the two transcribed literals, at `:1511` (`EXPECTED_SEED`) and
`:1587` (`EXPECTED_REFERENCE`). They are **regenerated from produced bytes** by the procedure in
section 6, never by pasting assertion output.

(c) The length pins move by exactly +31 bytes each (the replaced line is 45 bytes, the new one 76),
once per file: `EXPECTED_SEED.len()` `3681 -> 3712` at `:1938` and `:3064`, and
`EXPECTED_REFERENCE.len()` `3413 -> 3444` at `:3067-3070`. These predicted values are a
**cross-check only**: the authority is the produced file, and a mismatch between the produced
length and 3712/3444 stops the work and goes back to the coordinator.

## 5. Behaviour and edge cases

- No behaviour change: same trigger, same cadence, same suppression, same cap, same blind-gap
  suffix, same `MAX_RENDERED_BYTES` truncation path. Only the `%BASE%` string differs.
- Rendered length grows by 10 bytes (`" on GitHub"`) in case 3; the 4096-byte cap
  (`injected_messages.rs:84`) is untouched.
- `" on GitHub"` and `"its counterpart on GitHub"` are control-free ASCII, so the
  `is_control_free(&base_branch)` guard (`mailbox.rs:313`) still passes, and the rendered text
  still satisfies `rendered_output_passes_validate_pty_input_text` (`:2671`).
- CI notices are unaffected: their base stays the empty string.
- An operator who edited the template keeps their sentence; only the substituted value changes.
- The reference companion is rewritten automatically when shipped defaults change; nothing reads it
  back.

## 6. Seed and reference regeneration procedure

Byte pins are transcribed from *produced* bytes, never from a failing assertion's diff.

1. Apply 4.1, 4.2 and 4.3(a) only. Do **not** touch the literals yet.
2. Add this temporary, throwaway test to `injected_messages.rs`'s test module:

   ```rust
   #[test]
   #[ignore]
   fn dump_canonical_bytes_2129() {
       let dir = tempdir();
       ensure_injected_messages(dir.path()).expect("provision");
       std::fs::copy(main_path(dir.path()), "/tmp/2129-seed.toml").unwrap();
       std::fs::copy(reference_path(dir.path()), "/tmp/2129-reference.toml").unwrap();
   }
   ```

   Run it: `cargo test --lib dump_canonical_bytes_2129 -- --ignored --exact --nocapture`.
3. Read the produced files. `wc -c /tmp/2129-seed.toml /tmp/2129-reference.toml` must print
   `3712` and `3444`; if not, stop (section 4.3(c)).
4. Replace the bodies of `EXPECTED_SEED` and `EXPECTED_REFERENCE` with the produced file contents
   verbatim (LF only, one trailing newline), and update the three length pins to the numbers from
   step 3.
5. **Delete the temporary test.** It must not reach the PR.
6. `cargo test --lib injected_messages -- --test-threads=1` must be green, including
   `canonical_seed_bytes_and_reference_bytes_regenerated` and
   `missing_file_seeds_canonical_bytes`.

## 7. Tests

### 7.0 Placement decision (binding; the implementer chooses nothing)

`InternalSystemNotice::line()` is private to `phone::mailbox` (`mailbox.rs:359`, no `pub`), and
`remote_alerts.rs:623-625` records why a test outside that module must not pretend to call it. So
the proof is split along the module boundary, and **`line()`'s visibility is not widened** - that
would be a scope change contradicting a recorded design comment, for no coverage the split does not
already give.

- Composition (helper -> the notice's `base_branch` field) is proven in `remote_alerts.rs` by
  matching the variant, exactly as the existing `delivery_of` helper (`:625-641`) does.
- Rendering (a qualified base -> the sentence) is proven in `mailbox.rs`'s own test module, where
  `line()` is callable, as `remote_activity_line_renders_every_token` (`:13073`) already does.

The two halves meet on a literal string. The `mailbox.rs` tests pass the qualified base as a
**literal** and do **not** import `base_branch_display`: `render_with_template` (`:408`) is pure
token substitution, so the field-to-sentence step cannot depend on how the field was produced, and
a test-only `phone::mailbox -> pty::remote_watcher` import would add a module arc for no evidence.
The three literals are the same three strings asserted in 7.1, which is what keeps the halves
joined; section 8 criterion 7 checks they still match.

### 7.1 `src-tauri/src/session/remote_alerts.rs` test module

The `transition()` fixture at `:592` supplies branch `feature/2083-2064-remote-alerts` and base
`main`. Each test calls `notice_for(&transition)` and reads the field with a match on
`InternalSystemNotice::RemoteActivity { base_branch, .. }`, panicking on `ContextAlert`.

- **T1** `issue_2129_branch_stale_qualifies_a_resolved_base` - `BranchStale` from the fixture
  unchanged: `base_branch == "main on GitHub"`.
- **T2** `issue_2129_branch_stale_qualifies_the_unresolved_sentinel` - fixture with
  `branch = "main"`, `base_branch = "the default branch"`:
  `base_branch == "the default branch on GitHub"`, and it is **not** equal to `"the default branch"`
  or to `"main"`.
- **T3** `issue_2129_branch_stale_names_a_base_equal_to_the_branch_by_relation` - fixture with
  `branch = "main"`, `base_branch = "main"`: `base_branch == "its counterpart on GitHub"`, and the
  value does not contain `"main"`.
- **T4** `issue_2129_ci_kinds_keep_an_empty_base_branch` - both CI kinds still build a notice whose
  `base_branch` is empty.

### 7.2 `src-tauri/src/phone/mailbox.rs` test module (test code only)

- **T5** `issue_2129_branch_stale_line_renders_a_qualified_base` - three assertions on `line()`:
  - `remote_activity_notice(RemoteNoticeKind::BranchStale, "main on GitHub", Some(4), None)` renders
    `[AgentsCommander] mblua/AgentsCommander feature/2083-2064-remote-alerts is now 4 commits behind main on GitHub as of 2026-09-15 23:18:43-03:00. Any CI running on this branch is validating an out-of-date base.`
  - the same helper with `"the default branch on GitHub"` renders `... behind the default branch on GitHub as of ...`.
  - `InternalSystemNotice::for_remote_activity` called directly with `branch = "main"` and
    `base_branch = "its counterpart on GitHub"` (the helper hardcodes the branch at `:12974`, so
    this case cannot use it) renders
    `[AgentsCommander] mblua/AgentsCommander main is now 4 commits behind its counterpart on GitHub as of 2026-09-15 23:18:43-03:00. Any CI running on this branch is validating an out-of-date base.`
    and contains `"main"` exactly once.

### 7.3 `src-tauri/src/pty/remote_watcher.rs` test module

- **T6** `issue_2129_base_branch_display_covers_every_case` - `base_branch_display` directly over
  the three cases of section 2 plus the empty-string label (which takes case 3 and is unreachable
  for stale notices, since `for_remote_activity` rejects an empty stale base).

### 7.4 Unchanged and expected to stay green

They assert the template or the raw field, not the qualified value:
`remote_activity_line_renders_every_token` (`mailbox.rs:13073`, `behind main`, constructed below the
qualifier), `remote_activity_notice_rejects_a_branch_stale_with_an_empty_base_branch` (`:13053`),
`remote_activity_notice_rejects_a_base_branch_on_a_ci_kind` (`:13063`),
`each_new_default_has_exactly_one_known_sha_and_it_matches_the_template`
(`injected_messages.rs:3040`), and the whole #2131 suppression suite in `remote_watcher.rs`.

## 8. Acceptance criteria

1. Section 2's table is the rendered behaviour, proven by T1-T3.
2. `DEFAULT_BRANCH_STALE_TEMPLATE` and its `known_default_sha256` are byte-identical to the base;
   `git diff` shows no change on `injected_messages.rs:75` or `:174-178`.
3. `SCHEMA_VERSION` and `COVERAGE_VERSION` are unchanged.
4. `EXPECTED_SEED` / `EXPECTED_REFERENCE` were regenerated by section 6 and their pins are 3712 and
   3444.
5. The temporary dump test is absent from the diff.
6. Exactly the 4 files of the header, plus this plan, are changed; the `mailbox.rs` diff is inside
   its `mod tests` only, and `line()` is still declared `fn line(&self)` with no `pub`.
7. The three qualified strings asserted in 7.1 (T1-T3) are byte-identical to the three passed as
   literals in 7.2 (T5).

## 9. Proof protocol (for the reviewer)

1. On the base `f54a994`, add only T2 (`issue_2129_branch_stale_qualifies_the_unresolved_sentinel`,
   section 7.1) and run `cargo test --lib issue_2129 -- --test-threads=1 --nocapture`. It compiles
   on the base: it calls only `notice_for` and matches the variant, never `line()`. Capture the raw
   red - the field is `the default branch`, unqualified.
2. Implement sections 4 and 6, then run, in order:

   ```
   cargo fmt --all -- --check
   cargo test --lib injected_messages -- --test-threads=1
   cargo test --lib issue_2129 -- --test-threads=1 --nocapture
   cargo test --lib remote_alerts -- --test-threads=1
   cargo test --lib remote_watcher -- --test-threads=1
   cargo test --lib mailbox -- --test-threads=1
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --lib --bins --tests
   ```

   All green. Record the `cargo --version` / `rustc --version` actually used.
3. `git status --porcelain` and `git diff --stat` must show exactly the 4 files (plus the
   force-added plan). The `issue_2129` filter must match 6 tests: T1-T4, T5, T6. Nothing under `/tmp` is referenced by the tree.
4. Push the branch, open the PR against `main`, and require every triggered and configured-required
   check of `pr-regression-gates.yml` (and any other triggered workflow) to pass on the **exact
   PR-head SHA**. Host-dependent evidence belongs to CI; local runs do not substitute for it.
5. Before the PR is created, re-fetch `origin/main` and classify drift from `f54a994`: drift
   touching these 4 files, the workflows, or the toolchain pinning requires re-running step 2's
   gates; unrelated drift is recorded only.
6. Reply to `ac-tech-lead-v4` with the raw pre-fix red, the post-fix green, the commit SHA, the
   changed-file list, and the produced byte lengths.

## 10. Risks and compatibility

| Risk | Assessment |
|---|---|
| Byte pins drift from produced bytes | Section 6 forbids pasting from failing output and gates on `wc -c` matching 3712/3444 before transcription |
| A second `known_default_sha256` entry is added out of habit | Explicitly forbidden: `injected_messages.rs:3040` asserts exactly one digest, and the template does not move |
| Operator-edited templates | Untouched. They keep their sentence and receive the qualified value; reconciliation never fires, since the default sha is unchanged |
| `%BASE%` changes meaning for operators | Documented in 4.3(a), which is refreshed in the seed and the reference companion. Advisory: an operator who already edited the entry may keep a stale comment in their own file; no behaviour depends on it |
| "on GitHub" is wrong for a self-hosted forge | Out of scope at HEAD: every query is `gh api` against GitHub (`build_gh_command_spec`, `:191-213`) |
| Case 2 is unreachable in production | Intended. #2131 suppresses it first; the branch is defence in depth against a future suppression change, and T3 documents it |
| New module cycle | None: `remote_alerts.rs:34` already imports `remote_watcher`; no new arc, no SCC change. Section 7.0 deliberately keeps `mailbox.rs`'s tests free of a `pty::remote_watcher` import, so no test-only arc appears either |
| Split proof drifts apart | The three qualified strings are literals in both halves; acceptance criterion 7 compares them, and T6 pins the helper that produces them |
| Cold Rust build in a fresh replica | Tooling only, not a correctness risk. Budget a long first `cargo` run |

## 11. Implementation order

1. Add T2 (red) on the base and capture it.
2. 4.1 helper, 4.2 call site, 4.3(a) doc comment.
3. Section 6 regeneration, ending with the temporary test deleted.
4. Add T1, T3, T4 (7.1), T5 (7.2) and T6 (7.3); run the gates of section 9.2.
5. Commit the 4 files plus this plan (`git add -f plans/2129-branch-stale-remote-wording.md`),
   push, open the PR, verify exact-head checks.
6. Report per section 9.6.

## Plan Contract

Scope: the 3 files of section 4, the test-module-only addition to `mailbox.rs` (section 7.2), plus
this plan. Any change to
`DEFAULT_BRANCH_STALE_TEMPLATE`, to `known_default_sha256`, to `COVERAGE_VERSION`, to the #2131
suppression, or any additional message id stops the work and goes back to the coordinator before
implementation. Section 2's table is the contract: the implementer chooses no wording.
