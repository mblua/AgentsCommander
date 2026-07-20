# Implementation Plan: #1072 Compact Git-Scope Warning

Status: READY_FOR_IMPLEMENTATION

## 1. Issue and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1072
- Branch: `fix/1072-compact-git-scope-warning`
- Base SHA: `ad908daf189ab61e542aeece6cf226258f5a11d6`
- Delivery classification: Lite. This is one Rust module's generated copy and compatibility recognition, with no API, schema, dependency, IPC, or security-boundary change.

Correct the generated Git-scope warning so it no longer says the whole `.ac/` tree is gitignored. Compact the workgroup warning by at least 50 percent in both characters and whitespace-delimited words, while retaining the operative rules: workgroups and origin Matrices have different ignore status, Git discovery above agent roots is blocked, state-changing Git belongs in the correct repository location, Root uses registered project roots rather than `repo-*`, and read-only Git remains allowed only within the agent's existing read scope.

## 2. Evidence and cause

1. The project AC Root is not itself ignored. The inspected `.ac/.gitignore` ignores `wg-*/` at line 3. `git check-ignore -v` identifies that rule for the workgroup and replica, but not for `.ac` or `.ac/_agent_architect`.
2. Origin Agent Matrices can be tracked. `git ls-files .ac/_agent_architect/**` returns tracked files, including `.ac/_agent_architect/Role.md`.
3. Runtime protection is independent of ignore status. `git_ceiling_directories_for_session_root` in `src-tauri/src/config/session_context.rs` builds `GIT_CEILING_DIRECTORIES`, and `src-tauri/src/pty/local_backend.rs` applies it to agent PTYs. The warning copy should describe that guard without attributing it to `.gitignore`.
4. At the base SHA, `default_context_dynamic_values` has three current false literals: Root, workgroup replica with Matrix, and direct Matrix/no-Matrix argument. The workgroup literal is 473 characters and 68 whitespace-delimited words.
5. `legacy_rendered_default_context_for_compat` has two additional old rendered literals, selected by whether `matrix_root` is present. Those bytes participate in `classify_legacy_rendered_default_context`, `looks_like_generated_legacy_default_context`, and `reconstruct_legacy_rendered_default_context`.
6. The current classifier first treats the helper's exact rendered output as `Current`, then reconstructs generated legacy bytes for stale detection. Replacing the two legacy literals without retaining a pre-fix reconstruction candidate would strand previously generated files as custom. Conversely, broadly matching only a phrase would risk overwriting a user-edited template.
7. `get_default_agent_template()` contains `{{WRITE_RESTRICTIONS}}`, not any rendered Git-scope prose. Its bytes and the seeded global template version remain unchanged by this fix.

## 3. Scope

### In scope

- Define one exact current warning for each runtime location: workgroup replica with origin Matrix, direct origin Matrix, and Root Agent.
- Use the same workgroup/direct-Matrix copy in current legacy-rendered reconstruction.
- Preserve exact pre-fix matrix and no-matrix legacy warning bytes as compatibility-only snapshots so pristine old rendered contexts classify stale and self-heal.
- Add Rust regression tests in the existing `session_context.rs` test module for copy, size, materialization, classification, healing, and custom-edit preservation.

### Out of scope

- No changes to `.ac/.gitignore`, repository layout, write/read grants, `GIT_CEILING_DIRECTORIES`, the Windows Git guard, PTY spawning, Git watcher behavior, or repository commands.
- No changes to `get_default_agent_template()`, `src-tauri/src/config/seeded_context_templates.rs`, seeded template hashes, schema, or the global template's current version (`2`).
- No frontend, TypeScript, IPC, configuration, documentation, dependency, or migration-format changes.
- Do not rewrite unrelated context prose or refactor the large context renderer.

## 4. Exact files and symbols

### Modify `src-tauri/src/config/session_context.rs`

Production and compatibility symbols:

- Add `ROOT_GIT_SCOPE`, `WORKGROUP_GIT_SCOPE`, and `DIRECT_MATRIX_GIT_SCOPE` near `DefaultContextDynamicValues`.
- Add compatibility-only `LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072` and `LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072` beside the legacy renderer, with comments that they are frozen shipped bytes and must never be used for current runtime output.
- Update `default_context_dynamic_values` to select the three current constants.
- Add `LegacyGitScopeGeneration::{Current, Before1072}` and an internal generation-aware form of `legacy_rendered_default_context_for_compat`. Keep the existing function name as the `Current` wrapper so existing call sites retain their meaning; add a private pre-1072 wrapper for reconstruction and tests.
- Update `looks_like_generated_legacy_default_context` and `reconstruct_legacy_rendered_default_context` to compare the normalized input against both reconstructed exact candidates: current compact legacy output and the pre-1072 legacy output. Keep all existing structural, heading, ordering, path extraction, and generated-skills proofs ahead of that final equality check.
- Extend the existing `#[cfg(test)] mod tests`; do not create a second test framework or fixture directory.

### Explicitly unchanged

- `src-tauri/src/config/seeded_context_templates.rs`: `project_specs`, `is_known_generated_global_template`, hashes, snapshots, and global version remain untouched because the tokenized template bytes do not change.
- `src-tauri/src/pty/local_backend.rs` and `git_ceiling_directories_for_session_root`: runtime Git isolation remains untouched.

## 5. Decided current copy

Use these strings exactly, including punctuation and backticks.

### Workgroup replica with origin Matrix

```text
`wg-*/` workgroups are gitignored; origin Agent Matrices are not and can be tracked. Git discovery above replica and Matrix roots is blocked. State-changing Git belongs in `repo-*`; read-only Git is allowed within scope.
```

This is 220 characters and 33 whitespace-delimited words. Against the frozen base measurements of 473 characters and 68 words, the reductions are 53.49 percent and 51.47 percent respectively.

### Direct origin Matrix

```text
Origin Agent Matrices are not gitignored and can be tracked. Git discovery above this Matrix root is blocked. State-changing Git belongs in `repo-*`; read-only Git is allowed within scope.
```

This is 188 characters and 29 whitespace-delimited words. It does not claim that the Matrix is ignored, it names only the Matrix ceiling applicable to this session, and it retains the repository and read-only rules.

### Root Agent

```text
Git discovery above the Root Agent session root is blocked. State-changing Git belongs at a registered project root (the `settings.projectPaths` entry, one level above `.ac`), never in the Root Agent directory or another `.ac` subtree; the `repo-*` naming restriction does not apply. Read-only Git is allowed within scope.
```

This is 322 characters and 48 whitespace-delimited words. It directs state-changing Git to the registered project root, does not steer Root to `repo-*`, and retains the prohibition inside the Root Agent directory and other `.ac` subtrees.

## 6. Legacy compatibility design

1. Move the two pre-fix legacy warning strings, byte-for-byte, into the named compatibility constants rather than deleting or approximately matching them.
2. Pin those constants in a test using values captured from the base SHA:
   - With Matrix: length 598 bytes; SHA-256 `db90740fff6b6e44bf2f73fe929e8ec792f3ab5f459350f5e2d612f7151fd050`.
   - Without Matrix: length 570 bytes; SHA-256 `fe831abf46aa70fdefaecdaa2f0932966b2895a6f709f50b3c5981a07327b2f2`.
3. `LegacyGitScopeGeneration::Current` selects `WORKGROUP_GIT_SCOPE` when `matrix_root.is_some()` and `DIRECT_MATRIX_GIT_SCOPE` otherwise. `Before1072` selects the corresponding frozen old constant.
4. `current_legacy_rendered_default_context` continues to use the current wrapper. Therefore a compact rendered context for the same paths remains `Current`.
5. Reconstruction returns both exact rendered candidates after deriving the embedded agent root, optional Matrix root, and provably generated skills section. `looks_like_generated_legacy_default_context` returns true only when the normalized input equals one of those full candidates. An exact pre-fix file consequently becomes `StaleGenerated` and follows the existing atomic self-heal path.
6. Do not add substring, regex, edit-distance, or warning-only matching. A one-byte substantive edit must fail full reconstruction equality and remain `NotLegacy`.
7. Preserve existing normalization semantics: CRLF is normalized to LF and trailing whitespace at the end of the full document is ignored. Do not broaden normalization within the body.

## 7. Behavior, edge cases, and failure behavior

- A workgroup replica renders the `wg-*/` ignore fact and the trackable origin Matrix fact together. It receives the replica-plus-Matrix ceiling wording.
- A directly launched canonical Matrix receives the direct-Matrix wording and is never described as gitignored.
- Root receives only registered-project-root guidance. It is not told to use a `repo-*`, and the warning does not make any ignore-status claim about `.ac/`.
- Custom global templates that contain `{{GIT_SCOPE}}` receive the new location-specific value through the existing replacement chain. Missing mandatory coarse sections continue to use the existing append fallback.
- `CLAUDE.md`, `GEMINI.md`, and `AGENTS.md` are target filenames over the same resolved content. Each must materialize the same location-correct warning; cleanup of the other managed filenames remains unchanged.
- An exact old rendered context, whether its embedded paths equal the current agent or identify an older agent, is recognized as generated, rendered with current copy in memory, and atomically healed to `get_default_agent_template()` once. A second resolve reads the tokenized template and produces the same current output without another legacy heal.
- A one-byte edited old fixture remains custom and its on-disk bytes are not changed. Existing unknown-heading and edited-skills protections remain intact.
- If the existing atomic heal fails, resolution still returns the correct current in-memory context, leaves the old file untouched, logs the existing warning, and retries on a later resolve. Do not change this failure contract.

## 8. Compatibility and security

- This is a wording and exact-recognition change only. It neither grants filesystem access nor weakens the runtime Git ceiling or Windows guard.
- The new copy keeps state-changing Git out of replica, Matrix, Root Agent, and other `.ac` locations. The Root variant preserves its distinct authority to operate at a registered project root. The read-only sentence is explicitly bounded by the already-rendered read scope, so it cannot widen access.
- Exact full-document reconstruction plus frozen snapshots preserves upgrade compatibility without treating user-edited near-matches as generated. The existing revalidation and atomic replacement continue protecting the self-heal path.
- No new crate is needed; `sha2` is already available to the Rust test module.
- Since `get_default_agent_template()` remains byte-identical, changing a seeded template version or known-generated snapshot would be incorrect and could cause unnecessary project-template churn.

## 9. Implementation order

### MVP

1. Add the three current constants and failing tests for exact WG, direct-Matrix, and Root selection, old-claim absence, and WG size reduction.
2. Replace the three branches in `default_context_dynamic_values` with the constants and make the dynamic tests pass.
3. Change current legacy rendering to use the workgroup/direct-Matrix constants.

### Full features

4. Freeze and pin the two pre-1072 legacy warning constants.
5. Add the generation selector, pre-1072 renderer wrapper, and dual-candidate reconstruction.
6. Add exact matrix and no-matrix stale/heal/convergence tests and the one-byte custom negative control.
7. Add the three-provider materialization regression.

### Polish

8. Run targeted module tests, formatting, check, clippy, and diff hygiene. Confirm the tokenized default and seeded template metadata are unchanged.

### Extras

None.

## 10. Required tests

Add these tests, using these names or equally specific names in the existing test module:

1. `git_scope_copy_is_location_correct_and_compact`
   - Inspect `default_context_dynamic_values(...).git_scope` for WG-with-Matrix, direct-Matrix, and Root inputs.
   - Assert exact equality to the decided constants.
   - Assert current rendered outputs do not contain either old false spelling: `` `.gitignore`d `.ac/` `` or `` `.ac/` folder, which is `.gitignore`d ``.
   - Assert Root names `settings.projectPaths` and does not contain the non-root `repo-*` steer; assert WG/direct variants require `repo-*` and do not carry Root wording.
   - Count with `.chars().count()` and `.split_whitespace().count()`. Pin WG at 220/33 and assert `new * 2 <= old` against 473/68.
2. `git_scope_materializes_identically_for_all_managed_targets`
   - Build a real temp `.ac/wg-*/__agent_*` plus origin `_agent_*` identity.
   - Loop `ManagedContextTarget::{Claude, Gemini, Codex}` and verify the selected `CLAUDE.md`, `GEMINI.md`, or `AGENTS.md` contains `WORKGROUP_GIT_SCOPE` exactly once, contains neither old false spelling, and has no unresolved placeholder.
3. `pre_1072_legacy_git_scope_snapshots_are_byte_exact`
   - Assert the two frozen lengths and SHA-256 values from section 6.
4. `pre_1072_legacy_with_matrix_classifies_stale_and_heals_once`
   - Construct the full rendered fixture through `Before1072` with a real temp workgroup replica, Matrix, and generated skills section.
   - Assert direct classification is `StaleGenerated`, first resolution returns current WG copy and writes exactly `get_default_agent_template()`, and second resolution converges to identical output with the tokenized file unchanged.
5. `pre_1072_legacy_without_matrix_classifies_stale_and_heals_once`
   - Repeat the same proof for a real direct canonical Matrix and `matrix_root: None`, expecting `DIRECT_MATRIX_GIT_SCOPE`.
6. `one_byte_edited_pre_1072_git_scope_remains_custom`
   - Change exactly one ASCII byte inside the old Matrix warning, assert equal byte length and exactly one differing byte, then assert `NotLegacy` and byte-identical on-disk preservation after resolution.

Retain and run the existing stale-generated, edited-legacy, generated-skills, self-heal convergence, Root Git-scope, and provider materialization tests because they cover adjacent compatibility behavior.

Run from the repository root:

```bash
cargo test -p agentscommander-new config::session_context::tests::git_scope
cargo test -p agentscommander-new config::session_context::tests::pre_1072
cargo test -p agentscommander-new config::session_context::tests::one_byte_edited_pre_1072_git_scope_remains_custom
cargo test -p agentscommander-new config::session_context::tests
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
git diff --check
```

## 11. Objective acceptance

Implementation is accepted only when all of the following are true:

- Current WG, direct-Matrix, and Root output exactly matches section 5 and rejects both old false `.gitignore` claims.
- WG copy measures no more than half the base warning in both dimensions; the decided 220 characters and 33 words satisfy the 473/68 baseline.
- State-changing and read-only Git semantics remain location-correct for all three modes, including Root's registered-project-root exception to `repo-*` naming.
- The same new WG warning materializes in `CLAUDE.md`, `GEMINI.md`, and `AGENTS.md`.
- Exact pre-fix matrix and no-matrix rendered bytes classify stale, heal to the tokenized default on the first resolve, and converge on the second.
- A one-byte edited pre-fix fixture remains custom and is never healed.
- `get_default_agent_template()`, seeded global version/hash handling, Git ceiling code, and Git guard code have no diff.
- Targeted Rust tests, the full `session_context` test filter, `cargo fmt --check`, `cargo check --all-targets`, clippy with warnings denied, and `git diff --check` all pass.
