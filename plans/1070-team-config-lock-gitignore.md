# Plan #1070: Ignore the persistent team-config coordination lock

Author: architect, wg-14. Certified against current main on 2026-07-20 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1070](https://github.com/mblua/AgentsCommander/issues/1070), `fix: ignore persistent team-config coordination lock in project Git`.

This is a Lite repair. It adds one entry to the existing managed Project AC Root `.gitignore` writer and adds co-located Rust tests. It introduces no new production abstraction, dependency, schema, API, IPC surface, event, protocol, configuration, or migration.

## 1. Frozen authority and fail-closed entry gate

The implementation worktree is `repo-AgentsCommander-1070`, branch `fix/1070-team-config-lock-gitignore`, targeting `main`.

After `git fetch origin main` on 2026-07-20 UTC, all of the following resolved exactly to `ad908daf189ab61e542aeece6cf226258f5a11d6`:

- committed `HEAD`;
- the branch upstream, `origin/fix/1070-team-config-lock-gitignore`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

The index and non-ignored worktree were clean before this plan was created. Root `.gitignore` intentionally ignores `plans/`, so the implementation must carry this exact plan and force-add it. Do not remove or weaken the repository's `plans/` ignore rule.

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if `origin/main`, the local committed branch head, the branch upstream, or their merge base no longer equals the frozen SHA above. Do not rebase, merge a moved base, or silently substitute a newer commit under this certification.

## 2. Objective and non-goals

Objective: ensure the persistent `.ac/.team-config-write.lock` file created by `TeamConfigMutationGuard` is ignored by Git at the Project AC Root only, while preserving every existing managed rule and every user-authored byte in an existing valid UTF-8 `.ac/.gitignore`.

Non-goals:

- Do not delete, truncate, rename, relocate, or change the lifetime of `.team-config-write.lock`.
- Do not edit `TeamConfigMutationGuard` or `src-tauri/src/commands/entity_creation.rs`.
- Do not add or modify any seed-manifest rule from #1038.
- Do not alter existing `.gitignore` parsing, newline, write, concurrency, symlink, or error behavior.
- Do not mutate a user's Git index or run `git rm --cached`; ignore rules do not untrack a file already committed by a user.
- Do not touch Cargo files, lockfiles, workflows, frontend code, TypeScript, documentation other than this plan, or any other source file.

## 3. Verified current gap

Current main contains these facts:

1. `src-tauri/src/commands/entity_creation.rs` defines `TEAM_CONFIG_MUTATION_LOCK_NAME` as `.team-config-write.lock`. `TeamConfigMutationGuard::acquire_with_timing` canonicalizes the Project AC Root, creates or opens that root-level regular non-link file, and owns an OS file lock through its file handle.
2. The guard has no `Drop` deletion. The existing `invalid_new_team_alerts_have_no_team_directory_side_effects` test asserts that the lock file still exists after the guarded operation and that the lock can be reacquired. Persistence is intentional.
3. `src-tauri/src/commands/ac_discovery.rs::ensure_workspace_gitignore` manages eight patterns through `required_entries`. None is `/.team-config-write.lock`.
4. For a missing `.gitignore`, the writer emits each tuple as comment, pattern, and a blank line. For an existing valid UTF-8 file, it checks `content.lines()` using trimmed equality, preserves `content` as the output prefix, and appends only missing entries. If no pattern is missing, it does not write.
5. The writer is used by new-project setup, project registration, discovery, workgroup creation, and Loop mutation. The callers deliberately have different failure policies, documented in section 7; this repair must not change them.
6. Git interprets a leading slash in a `.gitignore` pattern relative to the directory containing that `.gitignore`. Therefore `/.team-config-write.lock` in `.ac/.gitignore` matches `.ac/.team-config-write.lock` and not `.ac/nested/.team-config-write.lock`.

The guard and its persistence test originated in commit `470b0570`. The missing managed rule is a Git-visibility defect, not a lock-lifetime defect.

## 4. Exact managed block and production edit

The emitted block is exactly:

```gitignore
# AgentsCommander: exclude team-config coordination files.
/.team-config-write.lock
```

In `ensure_workspace_gitignore`, append exactly this tuple to the existing `required_entries` slice, after the current `**/__agent_*/AGENTS.md` tuple:

```rust
(
    "/.team-config-write.lock",
    "# AgentsCommander: exclude team-config coordination files.",
),
```

Do not add a constant, helper, second writer, special-case branch, or new control flow. The existing tuple order and writer algorithm are the implementation. Appending the tuple at the end of the current slice minimizes the delta and leaves all existing managed blocks byte-for-byte and order-for-order unchanged.

This team-config block remains independent from the separate seed-manifest block proposed by #1038. When #1038 Stage A is later composed with current main, both blocks must remain distinct; neither block's pattern-presence test, comment, or ordering may be folded into the other.

## 5. Creation, append, and idempotence contract

### 5.1 Missing `.ac/.gitignore`

The existing creation loop writes all current entries in their current order, then writes the new comment and anchored pattern once, followed by the loop's existing blank line. No existing creation output is removed or reordered.

### 5.2 Existing `.ac/.gitignore` without the rule

The existing file must be read with the current `read_to_string` path. Its original valid UTF-8 bytes remain the exact prefix of the rewritten file. The existing separator behavior supplies a blank line before each appended block without normalizing the original prefix's CRLF/LF choices, spacing, comments, user rules, or unrelated managed blocks.

The new block is appended exactly once. Other currently missing managed entries may still be appended by the existing loop in their existing order; this repair neither suppresses nor rewrites that established sweep.

### 5.3 Rule already present and repeated calls

Pattern presence continues to use the writer's existing condition: a line whose trimmed text equals `/.team-config-write.lock` satisfies the entry. In that case the writer does not add another comment or pattern and does not reorder or deduplicate user bytes. Existing duplicates are preserved rather than repaired.

After a normal create or append has supplied every required pattern, a second call produces no write. The complete file bytes before and after the second call must compare equal.

## 6. Focused tests in `ac_discovery.rs`

Add exactly three co-located tests to the existing `#[cfg(test)] mod tests` beside the current managed-Git tests.

### 6.1 `ensure_workspace_gitignore_writes_team_config_lock_block_on_create`

- Create a temporary `.ac` directory with no `.gitignore`.
- Call `ensure_workspace_gitignore`.
- Read the result and prove the exact adjacent comment-plus-pattern block occurs once.
- Prove the anchored pattern line occurs once.
- Leave the current delete-sentinel and Loop assertions intact so the existing managed entries remain covered.

Do not assert that the team-config block is permanently the final block. A later independently reviewed managed block, including #1038 Stage A, must be able to follow it without weakening this test.

### 6.2 `ensure_workspace_gitignore_appends_team_config_lock_block_preserving_bytes_and_is_idempotent`

- Seed `.ac/.gitignore` with valid UTF-8 bytes that include user content, CRLF bytes, and an unrelated pre-existing managed block.
- Save those exact original bytes before the call.
- Call the writer and assert that the resulting byte vector starts with the exact original byte vector.
- Assert the exact team-config comment-plus-pattern block and anchored pattern line each occur once.
- Save the once-updated bytes, call the writer again, and assert byte equality with the saved once-updated bytes.

This test proves preservation without making the new repair responsible for normalizing pre-existing content.

### 6.3 `ensure_workspace_gitignore_team_config_lock_rule_is_root_anchored`

- Create a temporary project with `.ac`, call the writer, initialize Git at the project root with `git init --quiet`, and create both `.ac/.team-config-write.lock` and `.ac/nested/.team-config-write.lock` as regular files.
- Isolate the check from a developer's global exclude file by overriding `core.excludesFile` to an empty temporary file for the two `check-ignore` invocations.
- Run `git check-ignore -v --no-index -- .ac/.team-config-write.lock` from the temporary project root. Require exit code 0 and stdout whose source is `.ac/.gitignore`, whose matched pattern is exactly `/.team-config-write.lock`, and whose target is the root lock path.
- Run `git check-ignore -v --no-index -- .ac/nested/.team-config-write.lock`. Require exit code 1 and empty stdout. Any exit code above 1 is a Git execution failure, not evidence that the nested path is visible.
- A missing `git` executable is a test failure. Repository development and CI already require Git; do not silently skip this acceptance proof.

Use a test-local `std::process::Command` invocation. Do not add production Git execution or a reusable production helper for this test.

## 7. Failure behavior to preserve

The tuple addition does not change how `ensure_workspace_gitignore` reports failures:

- an existing path that cannot be read as valid UTF-8 returns `Failed to read Project AC Root .gitignore: ...` before any write;
- an append write failure returns `Failed to update Project AC Root .gitignore: ...`;
- a create write failure returns `Failed to create Project AC Root .gitignore: ...`; and
- the function returns `Ok(())` only after the existing create/append logic succeeds or determines that no entry is missing.

Caller behavior remains exactly current:

- fresh project creation through `create_ac_project_impl` and `config::projects::register_new_project` fails and attempts the existing fresh `.ac` cleanup;
- existing-root `create_ac_project_impl` and Loop create/update propagate the error without changing their existing behavior;
- pre-existing project registration logs a warning and continues;
- workgroup creation logs a warning and continues; and
- `discover_ac_agents` and `discover_project` retain their existing best-effort ignored result.

Do not add logging, retries, rollback, atomic replacement, locking, or a new error type in this repair.

## 8. Compatibility and security boundaries

- The pattern uses Git's platform-independent forward-slash syntax and is anchored relative to `.ac/.gitignore`, so Windows and Unix receive the same root-only match.
- The rule hides only the AgentsCommander-owned coordination filename at the Project AC Root. It does not hide nested same-name user files, team configuration JSON, workgroups, seed manifests, or other lock names.
- The change does not alter the guard's canonicalization, regular-file/non-link validation, OS lock, timeout, or release semantics. Dropping a guard still releases the OS lock while leaving the file present.
- No user content is parsed as Rust, shell, a path argument, or a command. Existing valid UTF-8 content is retained as an opaque prefix. Existing invalid UTF-8 failure behavior is unchanged.
- A later user negation rule can deliberately override an earlier ignore rule, and a previously tracked lock remains tracked. This repair does not seize control of the user's Git index or rewrite later user policy.
- Existing whole-file write, symlink-following, and concurrent-writer characteristics are unchanged. Hardening those behaviors would exceed the localized issue scope and requires separate review.

## 9. Exact changed-file contract

The final committed PR outcome against frozen base `ad908daf189ab61e542aeece6cf226258f5a11d6` must be byte-equal to this two-row, tab-separated matrix:

```text
A	plans/1070-team-config-lock-gitignore.md
M	src-tauri/src/commands/ac_discovery.rs
```

The plan is an added tracked path even though the repository ignores `plans/`; stage it with `git add -f plans/1070-team-config-lock-gitignore.md`. Stage the Rust file normally. No other added, modified, deleted, renamed, copied, type-changed, untracked non-ignored, or staged path is allowed.

## 10. Implementation order

1. Re-fetch and pass the frozen authority gate in section 1.
2. Verify this plan's SHA-256 against the value recorded by the tech lead from the architect completion report. Stop on any byte mismatch.
3. Add the single tuple in section 4 without changing existing production control flow.
4. Add the three tests from section 6 beside the existing managed-Git tests.
5. Run direct rustfmt only on the matrix-derived Rust path, then run focused and full named-toolchain gates.
6. Force-add this ignored plan, add the Rust file, and commit both paths together under the separately authorized implementation workflow.
7. From a clean committed tree, prove the exact ancestry, status matrix, formatter input, local gates, and repository CI below.

If implementation reveals a need for any additional path, production helper, different pattern, different failure policy, or behavior change, stop and return for issue and plan review.

## 11. Exact local verification

On the frozen base, `rustup run 1.93.1 cargo fmt --all -- --check` exits 1 with 123 diff headers across 26 pre-existing files. Current CI has no rustfmt step. That repository-wide debt is a diagnostic, not a green gate for #1070, and no debt file may be formatted into this repair. The direct formatter check for `src/commands/ac_discovery.rs` exits 0 on the frozen base.

From the repository root, require a fully committed tree, exact ancestry, and exact matrix:

```text
git status --porcelain=v1 --untracked-files=all
git diff --cached --quiet
git merge-base --is-ancestor ad908daf189ab61e542aeece6cf226258f5a11d6 HEAD
git merge-base ad908daf189ab61e542aeece6cf226258f5a11d6 HEAD
git diff --name-status --no-renames ad908daf189ab61e542aeece6cf226258f5a11d6...HEAD
```

The first command must print nothing, the second and third must exit 0, the fourth must print the frozen SHA exactly, and the fifth must print only the two tab-separated rows in section 9.

From `src-tauri`, derive and verify the only Rust formatting input:

```text
git diff --name-only --diff-filter=AM --no-renames --relative ad908daf189ab61e542aeece6cf226258f5a11d6...HEAD -- '*.rs'
```

It must print exactly `src/commands/ac_discovery.rs`. Then run:

```text
rustup run 1.93.1 rustc --version
rustup run 1.93.1 rustfmt --version
rustup run 1.93.1 cargo --version
rustup run 1.93.1 rustfmt --edition 2021 --check --config skip_children=true src/commands/ac_discovery.rs
rustup run 1.93.1 cargo test --lib commands::ac_discovery::tests::ensure_workspace_gitignore_writes_team_config_lock_block_on_create -- --exact
rustup run 1.93.1 cargo test --lib commands::ac_discovery::tests::ensure_workspace_gitignore_appends_team_config_lock_block_preserving_bytes_and_is_idempotent -- --exact
rustup run 1.93.1 cargo test --lib commands::ac_discovery::tests::ensure_workspace_gitignore_team_config_lock_rule_is_root_anchored -- --exact
rustup run 1.93.1 cargo check --all-targets
rustup run 1.93.1 cargo clippy --all-targets -- -D warnings
rustup run 1.93.1 cargo test --lib --bins --tests
```

The version output must equal:

```text
rustc 1.93.1 (01f6ddf75 2026-02-11)
rustfmt 1.8.0-stable (01f6ddf758 2026-02-11)
cargo 1.93.1 (083ac5135 2025-12-15)
```

Rolling `stable` is not a substitute for these named local commands.

After push and PR creation under separate authorization, require all workflows triggered by this exact matrix to finish successfully: branch-protection check `validate-branch-name`, plus `test-debt`, `rust-regression`, `windows-release-cli-smoke`, and `frontend-regression` from `PR regression gates`. `lockfile-drift` and `version-sync` are not path-triggered by this matrix and must not be represented as missing failures.

## 12. Plan Contract corrections required before implementation dispatch

The live #1070 body is technically correct about the base, branch, target, Rust source scope, exact rule, and runtime acceptance. Its Plan Contract statements are stale. The tech lead must make these exact corrections and re-read the final issue body before dispatch:

1. Keep the implementation source scope as one Rust file, but replace every final changed-file claim with the two-row matrix in section 9.
2. Replace acceptance criterion 6's one-row matrix with the two-row matrix in section 9.
3. Change the out-of-scope sentence that excludes a plan so it excludes documentation other than `plans/1070-team-config-lock-gitignore.md`; continue excluding Cargo, workflows, and all other source paths.
4. Record the certified plan path and the uppercase SHA-256 from the architect completion report, and require the implementing branch to carry those exact bytes.
5. Keep `fix/1070-team-config-lock-gitignore`, target `main`, and frozen base `ad908daf189ab61e542aeece6cf226258f5a11d6`; those live fields are already correct.

The current non-authoritative #1038 draft is also stale but must not be edited as part of #1070. Its next recertification must apply these corrections:

1. Replace the unnamed or #1056-labelled P0 prerequisite with existing issue #1070 and branch `fix/1070-team-config-lock-gitignore`.
2. Replace every P0 one-path or "no plan byte" statement with the exact two-path matrix in section 9 and the certified #1070 plan path/digest.
3. Replace the proposed-new-P0-issue mutation with the already-open #1070 coordinate and its corrected Plan Contract.
4. Keep the #1070 team-lock block separate from Stage A's seed-manifest block and keep #1070 landing before Stage A recertification.
5. After #1070 lands, record its observed PR URL and full landing SHA, merge that then-current `main` into the existing #1060 branch, and recertify #1038/Stage A composition and plan bytes. Do not guess future coordinates or landing hashes.
6. Preserve the already approved five-path Stage A implementation patch while treating its eventual PR outcome matrix separately, as the #1038 draft already does for its own tracked plan.

These are governance corrections only. This implementation must not edit issue bodies or `plans/1038-project-seed-manifest.md`.

## 13. Objective acceptance

The repair is acceptable only when all of these statements are true:

1. A newly managed `.ac/.gitignore` contains the exact team-config comment and anchored rule once while retaining every existing managed rule.
2. An existing valid UTF-8 file retains its original bytes as an exact prefix and gains the exact block once.
3. A second writer call is byte-stable and adds no duplicate.
4. Root `git check-ignore -v --no-index` identifies `.ac/.gitignore` and exact pattern `/.team-config-write.lock`; the nested same-name path exits 1 with no match.
5. The persistent lock remains present after guard drop, and no guard code changes.
6. All preserved failure policies, compatibility boundaries, and security limits in sections 7 and 8 remain true.
7. The final committed all-file matrix is exactly the two rows in section 9, including the force-added certified plan.
8. Exact named Rust 1.93.1 versions, focused tests, direct formatting, check, clippy, full tests, and every triggered repository CI job pass.
9. #1070 remains independent from #1038 seed-manifest implementation, and #1038 is not recertified until the reviewed #1070 landing is composed into its current base.

No unresolved implementation choice remains within this Lite scope.
