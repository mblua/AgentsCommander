# Issue #1446 Full Plan: Instance `.gitignore` artifact registry

- Issue: https://github.com/mblua/AgentsCommander/issues/1446
- Branch: `feature/1446-instance-gitignore-artifact-registry`
- Planning base: `b1eefa7c0e076d79d7ea38d76f998d1c05fd5055`
- Delivery path: Full (architect draft; dev-rust enrichment in Section 11; grinch enrichment in Section 12; consensus round 1 resolved into the body, ledger in Section 13; certified in Section 10)
- Related: closes the instance-dir half of #1209 (see Section 4.6); the AC-root half is follow-up #1448; #1441 and #1443 stay independent
- Supersedes, by user product decision recorded in #1446 and in the consensus round: the #1164 "no cache/database directories" restriction, the #1164 "no generated comments" fresh-file byte spec, and the #1164 `update-check.json.tmp` narrowness control (that name is a live runtime temporary and is now a covered artifact; Section 4.6)

## 1. Objective

Two deliverables in one change, per #1446:

1. **Extend the generated instance `.gitignore`** (the one `ensure_instance_gitignore()` maintains inside `config_dir()`) so it covers every runtime artifact class inventoried in #1446 plus the classes the enrichment inventory added (Section 12.1, ruled by the user; one further sibling found in certification verification, Section 13; four further classes found by the implementation review of round 2 and ruled by the user, Section 14). The existing append-only reconciliation then repairs every existing writable installation on its next app start with no migration and no user action (Section 6 scopes the read-only/locked exception honestly).
2. **Close the retyped-literal drift class, and reduce (not detect) the appearance drift class**: replace the retyped-literal rule list with a single registry of instance artifacts, each declared once with a disposition (`Ignore` or `Track`), from which `required_rules` derives the emitted rules. Writer modules build their paths from the same constants the registry uses, so a rename breaks the build or a guard test instead of silently reopening the gap. Stated plainly so this plan does not overpromise (consensus resolution of finding 12.6, option ii): a brand-new artifact written by a module that never touches the registry still compiles and passes every test; appearance drift is reduced to "one obvious table to add a row to, a fixture that forces a concrete sample per row, and a one-time full inventory taken at `b1eefa7c`", it is not mechanically detected. Any future deliberate exclusion must be recorded as a `Track` row or in this plan's Section 3, so "not in the registry" never again means "nobody looked".

After this change a fresh generated file contains, in order: the 2 dynamic root-agent rules, then the static `Ignore` rows of Section 4.2, every rule preceded by one explanatory `# AgentsCommander: ...` comment line. An existing complete pre-change file receives exactly the new commented entries appended once; user content is never rewritten. Every derived count (rows, rules, lines, appended pairs) lives in exactly one place, Section 4.2's canonical block, and tests derive them from the table, never retype them.

Fixed product decisions (user-confirmed; not reopenable here). Decisions 1-6 were recorded in #1446; decisions 7-10 were ruled by the user in consensus round 1 (tech-lead dispatch of 2026-08-19):

1. All runtime artifacts of the issue are ignored, including the message-bus SQLite database and its sidecars.
2. The constants-derived registry ships in this same change.
3. The #1164 restriction against covering caches/databases is revoked.
4. Settings migration backups are covered by the pattern `settings.pre-*.json`, not a literal.
5. The generated `.gitignore` does not ignore itself (status quo).
6. Explicit Track set, never ignored: `agent-templates/`, `agency-agents_templates/`, `coding-agents/`, `Context.root-agent.md`, `ac-root-agent/` (beyond its two already-ignored `config.json` rules).
7. The seven classes of Section 12.1 are ALL `Ignore`: `api-clients.json`, the `.api-clients-<uuid>.tmp` temporaries, `logs/`, `session-requests/`, `ui-automation/`, `codex-home/`, and the `coordinator_clocks.json.<pid>.<seq>.tmp` temporaries.
8. The message-bus SQLite rule is the single glob `api-message-bus.sqlite3*`, replacing the three enumerated literals (it also covers `-journal` and every other sidecar SQLite can produce).
9. `update-check.json.tmp` moves from fixture control to covered artifact. This is an approved reversal of a #1164 narrowness control, declared as such: the name is a live runtime temporary (its writer names it in a comment, `update_check.rs:76`), squarely inside decision 1.
10. #1209 closes only for the instance dir; the AC-root half (the other `write_file_atomic` callers) is follow-up #1448.

Decisions 11-14 were ruled by the user in recertification round 2 (tech-lead dispatch of 2026-08-20, `20260820-062529`), closing the four classes grinch's implementation review found missing from the registry (Section 14):

11. The agency template cache siblings are `Ignore` under the glob `agency-agents_templates.*`. The glob form is deliberate: the dot is what keeps the rule off the suffix-less `agency-agents_templates` directory, which stays `Track` under decision 6. One row covers all four concrete shapes (`.lock`, `.next-<uuid>/`, `.download-<uuid>/`, `.prev-<uuid>/`).
12. `coding-agent-requests/` is `Ignore` as a `Dir` row, which covers its `results/` subdirectory in the same rule. Parity with the two request queues already ignored under decision 7 (`project-refresh-requests/`, `session-requests/`).
13. `Context.AgentsCommander.md.retired-*.bak` is `Track`: the retirement backups hold the user's own bytes.
14. `Context.AgentsCommander.md`, the live standalone global context template, is `Track`.

Decision 15 was ruled by the user in the same round, closing the fifth class the round-2 enumeration surfaced (Section 14.3):

15. **Rotated generations are `Ignore`**, for all four rotating artifacts: `app.log.<n>`, `api-audit.log.<n>`, `activity.jsonl.<n>` and `orphaned-sessions.archive.json.<n>`. The live file of each is already an `Ignore` row and a rotated generation is the same runtime artifact with a numeric suffix, so ignoring the live file while versioning its generations is the half coverage #1446 exists to end. This makes `app.log.1` a covered artifact, which is a **declared reversal of a #1164 narrowness control**, the second in this plan and enumerated beside the first in Section 4.6.

## 2. Evidence and current-state gap

Evidence gathered via Codebase Memory (gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-11-dev-v5-team-repo-AgentsCommander`, head `b1eefa7c`, 14 graph operations, one 80-line direct read of the layering guard), plus dev-rust's investigation report of 2026-08-19 (`messaging/20260819-172600-...-local-dir-gitignore-drift-findings.md`), the #1446 and #1209 issue bodies, the real generated file of this workspace's instance dir, the measured enrichment evidence of Sections 11 and 12, and the architect's certification-pass direct reads at `b1eefa7c` of every writer site item 9 adds.

1. `src-tauri/src/config/instance_gitignore.rs` is the single policy owner.
   - `const FIXED_RULES: [&str; 12]` (lines 5-17): the 12 static rules, root-anchored `/name` style.
   - `ensure_instance_gitignore()` (27-41): production entry point; sole production caller is `logging::init_logger_inner`, so the ensure runs on every normal startup before `app.log` opens.
   - `ensure_instance_gitignore_at(config_dir: &Path, agent_local_dir: &str)` (43-76): builds rules once, classifies the leaf, one bounded `RetryClassification`.
   - `required_rules(agent_local_dir: &str) -> Result<[String; 14], String>` (99-122): rule 1 `format!("/{}/{}/config.json", super::ROOT_AGENT_DIR_NAME, escaped_agent_local_dir)`, rule 2 `format!("/{}/config.json", super::ROOT_AGENT_DIR_NAME)`, then `FIXED_RULES[0..=11]`.
   - `create_fresh_file(path, rules: &[String; 14])` (124-150): no-clobber create, validate, nonblocking lock, `fresh_file_bytes(rules)`.
   - `ensure_existing_file(path, rules: &[String; 14])` (152-204): byte-exact line detection via `missing_rule_indexes`, append-only repair via `append_buffer` under a locked reread. Never rewrites, reorders, or deletes user content. This is the reconciliation that propagates new rules to existing installations.
   - The `[String; 14]` arity is hardcoded in these signatures and in the helpers they share.
2. In-file test census (all in `instance_gitignore.rs` `#[cfg(test)]`, calling `ensure_instance_gitignore_at`): `fresh_file_has_exact_fourteen_rules_and_dynamic_name`, `injected_name_with_line_break_is_rejected_without_creating_a_file`, `partial_file_preserves_prefix_and_appends_only_missing_rules`, `complete_file_is_byte_stable_across_repeated_ensure`, `byte_scan_preserves_invalid_utf8_and_recognizes_crlf_rules`, `directory_target_is_rejected_and_untouched`, `symlink_target_is_rejected_without_touching_referent`, `read_only_complete_file_needs_no_write_but_partial_file_fails_unchanged`, `locked_partial_file_fails_fast_and_remains_unchanged`, `git_fixture_ignores_exactly_required_paths_without_untracking` (685-796), `literal_gitignore_segment_encoding_is_canonical`, `git_fixture_treats_bracketed_agent_name_as_literal`, `escaped_canonical_line_controls_detection_and_repair`, `instance_gitignore_ignores_injected_messages_artifacts_narrowly`; plus `instance_gitignore_covers_every_injected_messages_artifact` (1003-1021), which imports the three `injected_messages` filename constants in `cfg(test)` and asserts `required_rules` covers them. `TEST_AGENT_LOCAL_DIR` is `".agentscommander_amp-office"`.
3. The git fixture test's current `control_paths` are: `app.log.1`, `update-check.json.tmp`, `api-audit.log`, `cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, `injected-messages.toml.bak`, `injected-messages.json`, `agentscommander-injected-messages.json`, `sub/injected-messages.toml`. Three of them become ignored by this change: `api-audit.log`, `update-check.json.tmp` (product decision 9) and `app.log.1` (product decision 15); the two reversals of a deliberate #1164 control are enumerated together in Section 4.6. The other seven remain valid narrowness controls.
4. `src-tauri/tests/instance_gitignore_layering.rs` (#1273) is a spelling-net layering guard. Facts that bind this plan:
   - The crate has exactly one cyclic SCC ("the knot"; `coverage.graphShape.cyclicSccs = 1`). Its member count drifts as the crate evolves: the guard's #1273-era prose says 87 or 88, and a detector run on the clean tree at `b1eefa7c` during this certification measured **85** members; the binding criterion is member-set identity pre/post, never a count (Section 8). `config::instance_gitignore` was taken out of the knot by #1273 and must stay at `sccSize = 1` (re-measured: it is a size-1 SCC at `b1eefa7c`).
   - `ROOT_AGENT_DIR_NAME` was moved by #1273 into `src/config/mod.rs`, whose zero-outgoing-arcs property is load-bearing; the guard asserts `crate::config` (scanned shallow) names nothing, and asserts `instance_gitignore`'s reference sets by equality against `ALLOWED_*` tables, with the `crate::`-anchored table deliberately empty. `mod` declarations are not references; string literals and comments are stripped before matching. The guard also asserts `the_root_agent_dir_name_constant_is_defined_exactly_once`.
   - The committed arc record is `src-tauri/module-arcs.txt` (1009 arcs at `b1eefa7c`, verified by line count in the consensus round; the guard's doc comments still say 976 in several places, stale prose from #1273 that Section 4.7.5 refreshes), regenerated by `scripts/02-module-arc-record.mjs`; byte-identity of the regenerated record is the acceptance test. `agentscommander_lib::config` has zero outgoing arcs in the record while 51 arcs point into it (measured, Section 12.11.1), which is the leaf premise this design copies.
5. Owner-module facts for the drift set (file:line per dev-rust's gated investigation; the implementer verifies each on checkout):
   - Constants that already exist: `config/activity_log.rs:119` `FILE_NAME = "activity.jsonl"`; `api/message_store.rs:15` `DB_FILENAME = "api-message-bus.sqlite3"`; `config/sessions_persistence.rs:56` `ORPHAN_ARCHIVE_FILENAME`; `config/coding_agents_catalog.rs:47` `CATALOG_DIR_NAME = "coding-agents"`; `config/session_context.rs:11` `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME = "Context.root-agent.md"`; `config/seeded_context_templates.rs:7` `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME = ".agentscommander-context-templates.json"`; `commands/role_templates.rs:47` `AGENCY_TEMPLATES_DIR = "agency-agents_templates"`.
   - Inline literals to replace: `config/session_context.rs:105,1413,2391` `"context-cache"`; `config/coordinator_clocks.rs:314` `"coordinator_clocks.json"`; `api/message_store.rs:648` `"pty-input-locks"`; `api/audit.rs:19` `"api-audit.log"`; `telegram/output.rs:64,141,142` `"telegram-bridge.log"`, `"diag-raw.log"`, `"diag-sent.log"`; `commands/config.rs:80` and `web/commands.rs:723` `"debug-logs.txt"`; `pty/local_backend.rs:334,437` `"git-guard"`; `lib.rs:1821` `"instances"`; `cli/create_agent_matrix.rs:211` and `phone/mailbox.rs:10807` `"project-refresh-requests"`; `config/settings.rs:~3345` the `settings.json.lock` name construction; `commands/role_templates.rs:135,217` `"agent-templates"`.
   - Consensus-round additions (Section 12.1 measured by grinch; every site re-read by the architect at `b1eefa7c` during certification): `api/auth.rs:45` `pub const REGISTRY_FILENAME: &str = "api-clients.json"` is the single canonical constant and every production join site imports it (`api/auth.rs:296`, `cli/api_client.rs:89`, `commands/config.rs:1801`, `pty/container_tokens.rs:45`; the remaining `"api-clients.json"` literals in the tree are tests, doc comments and one error string); `api/auth.rs:637,678` join the literal `"api-clients.lock"` (created with `create(true).truncate(false)` and never deleted, the same persistent-lock class as `settings.json.lock`; found in certification verification, Section 13); `api/auth.rs:758` builds `.api-clients-<uuid>.tmp`; `cli/harness.rs:475` joins literal `"logs"` (holds `harness.log`, line 477); `cli/create_agent.rs:133,295` and `phone/mailbox.rs:10831` join literal `"session-requests"`; `testability/ui_automation.rs:17` `pub const UI_AUTOMATION_DIR: &str = "ui-automation"` (`SESSION_FILE` and deeper names stay owner-side, the dir row covers the contents); `config/agent_command.rs:499,2046` join literal `"codex-home"`; `update_check.rs:76` `path.with_extension("json.tmp")` yields `update-check.json.tmp` (the writer's own comment names it); `config/coordinator_clocks.rs:360-364` `path.with_extension(format!("json.{pid}.{seq}.tmp"))` yields `coordinator_clocks.json.<pid>.<seq>.tmp`.
   - Round-2 additions (grinch's implementation review of 2026-08-20; every site re-read first-hand by the architect at `dcd221fb`): `cli/agency_templates.rs` publishes four siblings of the template cache into `config_dir()`, `format!("{}.lock", AGENCY_TEMPLATES_DIR)` at `:138` inside `CacheLock::acquire` (called from production at `:320`, `:370`, `:389`), `format!("{}.next-{}", AGENCY_TEMPLATES_DIR, uuid)` at `:414-418`, `format!("{}.download-{}", ...)` at `:637-641` inside `fetch_repo_with_git` (a full git clone of an external repository), and `format!("{}.prev-{}", ...)` at `:920-924` inside `publish_staging`; that file never spells `config_dir()`, it resolves the root through the wrapper `config_dir_or_err()` (`:301-304`), and it already imports `AGENCY_TEMPLATES_DIR` from `commands::role_templates` (`:10-15`), which is itself a registry alias (`commands/role_templates.rs:49`), so all four names already resolve to the registry constant. `config/coding_agent_mutations.rs:35` `pub const CODING_AGENT_REQUESTS_DIR: &str = "coding-agent-requests"` (results subdirectory at `:37`) is the single canonical constant, joined onto `config_dir()` by both sides of the queue, `cli/coding_agent.rs:424` (root bound 22 lines earlier at `:402`) and `phone/mailbox.rs:10974` (root bound at `:10970`). `config/seeded_context_templates.rs:1430` joins `session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME` (`config/session_context.rs:10`, value `Context.AgentsCommander.md`) onto a **parameter** named `config_dir`, which `config/root_agent.rs:810-816` supplies as `root_dir.parent()`, that is the instance dir; the retirement backups are created with `create_new` at `:1597-1606` in `live_path.parent()`, named `{filename}.retired-{timestamp}.bak` and `{filename}.retired-{timestamp}.{n}.bak`.
   - Rotation writers (round 2, decision 15; all four read first-hand at `dcd221fb`). Each publishes numbered generations as **direct children** of the instance dir, and each rotates a path whose base name it already holds: `logging.rs:275-330`, the `numbered` closure at `:313` joining `format!("{stem}.{i}")` onto the parent, `stem` from `base.file_name()` (`:300`), `base` = `config_dir().join("app.log")` (`:487-489`), `APP_LOG_KEEP = 5` (`:248`); `api/audit.rs:395-407`, `path.with_extension("log.1")` at `:403` over `config_dir().join(API_AUDIT_LOG_FILE_NAME)` (`:21`), one generation, cap `AUDIT_MAX_BYTES` 10 MB (`:18`); `config/activity_log.rs:606-645`, `parent.join(format!("{name}.{index}"))` at `:624,628,638` over `dir.join(FILE_NAME)` (`:719,745-746`), `ACTIVITY_KEEP = 4` (`:109`); `config/sessions_persistence.rs:780-800`, the `numbered` closure at `:796` over `dir.join(ORPHAN_ARCHIVE_FILENAME)` (`:916`), `ORPHAN_ARCHIVE_KEEP = 3` (`:65`). Three of the four already reach the registry for their base name (`api/audit.rs:15`, `config/activity_log.rs:119`, `config/sessions_persistence.rs:56` are registry aliases), which is what makes the derivation tests of Section 4.1 tie something real; only `logging.rs:489` builds `"app.log"` as a literal, and that writer is a legacy-row writer Section 3 leaves alone. All four live files are opened in **append** mode (`config/activity_log.rs:594-596`, `logging.rs:490-492`, `api/audit.rs:386-388`, `config/sessions_persistence.rs:827`), so none of them has a temporary of its own; any atomic temporary over these paths carries a leading dot and is row 1's, which cannot overlap the anchored rotation globs (measured, Section 14.3).
   - `settings.pre-384-v1.json` HAS one live writer, found in the resumed certification pass: `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`), reached from the pre-384 settings migration path (`config/settings.rs:1875`), write-once (returns `Ok` early when the backup already exists), naming the concrete instance inline via `with_file_name`. The covering glob `settings.pre-*.json` is registry-owned and the writer is deliberately not rewired (Section 4.4).
6. `config/local_config_io.rs`: `write_file_atomic` is `pub` (80-82) and publishes through `temp_config_path(path) -> PathBuf` (122-128, private), which names the sibling `.{file_name}.{pid}.tmp` (file name falls back to `"config.json"` when not UTF-8). Because the pid segment always contributes one interior dot and `.tmp` contributes the final one, every name this scheme can produce matches the `.*.*.tmp` glob shape (emitted unanchored, Section 4.6), while `foo.tmp` and `.foo.tmp` do not match it at any depth. This is issue #1209.
7. The analogous generator `ensure_ac_root_gitignore` (`commands/ac_discovery.rs:1496-1582`) already maintains `(pattern, comment)` pairs, detects presence by trimmed-line equality on the pattern only (comments are transparent to detection), and appends `comment` then `pattern` for each missing entry. Comments, globs, and negations are established house style for generated ignore files.
8. Real generated file of this workspace (14 lines, verified byte-exact): the 2 dynamic rules with the local dir name, then the 12 fixed rules in byte-alphabetical order, all root-anchored, no comments, trailing LF, and no self-ignore line.

Gap, stated precisely (consensus resolution of 12.6): nothing ties "a module writes X into `config_dir()`" to "X is covered by the policy". The list went stale twice before (#1164 seed, #1157 repair), the enrichment inventory proved it was stale again while this very plan was being written (Section 12.1's seven classes plus the `api-clients.lock` sibling), and the implementation review proved it a fourth time (Section 14's four classes, plus a fifth class that is still open). Each recurrence was found by a person looking, never by a test, which is the residual Section 3 records. This change fixes the **rename** half of that class mechanically (registry constants shared with writers) and fixes the current inventory in full; the **appearance** half remains a process risk by recorded decision (Section 3, Section 13 finding 12.6). #1209 names the temporaries subclass explicitly.

## 3. Scope

### In scope

- One new leaf registry module `src-tauri/src/config/instance_artifacts.rs` plus its registration in `config/mod.rs`.
- Rewiring `instance_gitignore.rs` to derive emitted rules from the registry and to emit one comment line per rule.
- The new ignore patterns of Section 4.2 (canonical counts there) and the explicit Track declarations (count in the same canonical block).
- Replacing the listed owner-module literals/constants with registry-backed constants (Section 4.4, const-alias form).
- Closing the instance-dir half of #1209 via the depth-independent `.*.*.tmp` rule plus a writer-side guard test (Section 4.6).
- Updating the in-file tests, the git fixture, and `tests/instance_gitignore_layering.rs`; adding the reconciliation-compatibility test.
- Widening `tests/claude_watcher_layering.rs` by exactly one expected-dependency row so the `telegram/output.rs` writer can be wired (Section 4.8; found in implementation, resolved in recertification).
- Regenerating and committing `src-tauri/module-arcs.txt`; running the cycle detector pre/post (Section 8, Step-N criterion).

### Out of scope

- Untracking files already committed in existing repos (same stance as #1164; a new rule never untracks). Section 6 now documents the manual `git rm --cached` remediation for the newly covered set.
- Ignoring the generated `.gitignore` itself (product decision 5).
- Any `/*` deny-all or generated `!` allowlist for the instance file (still prohibited; the ac-root sibling's negation style is not imported here).
- The AC-root half of #1209 (product decision 10): `write_file_atomic` callers that publish under the project `.ac/` root are governed by `ensure_ac_root_gitignore`, untouched here; follow-up #1448 covers them, and it is a one-row edit against a generator that already carries the exact pattern class (`commands/ac_discovery.rs:1498-1501` emits `/.seed-manifest.*.tmp` with a comment).
- Any mechanical appearance-drift detector (consensus resolution of 12.6, option ii, per the user's "close as much as is feasible without breaking the guards"): a generalized owner-constant coverage test cannot reach non-`config` owners without `crate::`-anchored spellings inside the guarded module (the guard's `crate::` table is deliberately empty and stays empty), and a `#[cfg(test)]`-only coverage sibling would tie only constants somebody remembered to enroll, which is the same failure mode relocated, at the cost of a third list. The honest wording of Sections 1 and 2 is the resolution; the recorded rule is: every future `config_dir()` child gets a registry row, `Ignore` or `Track`, in the same change that introduces it.
- Retrofitting the 12 pre-existing rules' writers (`logging`, `sessions_persistence` sessions path, `settings` settings path, token/pid writers, `injected_messages`, `update_check`) to import registry constants. They are already guarded by the fixture and, for injected-messages, by the #1157 coverage test. Their registry rows are literals (Section 4.2). A follow-up issue may retrofit them; this change does not.
- Normalizing the three off-shape temporary writers (`update_check.rs:76`, `config/coordinator_clocks.rs:360-364`, `api/auth.rs:758`) onto `temp_config_path`, or otherwise touching them: their names are covered by dedicated rows (Section 4.6), two of the rows are derivation-tested against the artifact constants, and all three get fixture samples. The same stance covers the settings migration backup writer `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`): its concrete output `settings.pre-384-v1.json` is fixture-guarded under the registry's `settings.pre-*.json` glob and the writer is not touched. Rewriting correct writers is exactly the blast radius this plan refuses. Round 2 adds the four rotation writers of decision 15 to the same stance (`logging.rs:313`, `api/audit.rs:403`, `config/activity_log.rs:624,628,638`, `config/sessions_persistence.rs:796`): each composes a numeric suffix at runtime onto a base name it already holds, so no constant can express the concrete generation name, and three of the four already obtain that base name from the registry. Their covering globs are registry-owned and derivation-tested against the same constants (Section 4.1). Round 2 also adds a fifth single writer to the stance, `retire_standalone_global_context_with`'s backup naming (`config/seeded_context_templates.rs:1597-1606`): it composes the backup name from the live entry's runtime `file_name()`, so no constant can produce the concrete name, and its covering glob (`Context.AgentsCommander.md.retired-*.bak`) is registry-owned and derivation-tested against the live entry's own constant.
- Moving `ROOT_AGENT_DIR_NAME` out of `config/mod.rs` (its #1273 placement and exactly-once guard stay untouched).
- Log rotation, lock sweeping (#1441, #1443), settings schema, IPC, frontend, or dependency changes.
- Refactoring `ensure_ac_root_gitignore` or the hardened open/lock/validate machinery of `instance_gitignore.rs` (mechanics are reused as-is).

## 4. Decided solution

### 4.1 New module `src-tauri/src/config/instance_artifacts.rs` (the registry)

A leaf module with **zero outgoing references in production code**: no `use crate::...`, no `super::...`, no third-party imports anywhere outside its own `#[cfg(test)] mod tests`. Its acyclicity argument is the same one #1273 established for `crate::config`: a node with zero outgoing arcs cannot join or create a cycle, regardless of how many knot members point at it.

Consensus correction (findings 11.1 and 12.4): the draft's stricter "including in its tests" phrasing conflated the guard's two contracts. The property the acyclicity argument needs is **zero outgoing arcs in the module-arc record**, which is emitted with `includeTests: false`; the file additionally carries exactly one `#[cfg(test)] mod tests { use super::*; ... }`, whose `super` resolves to the module itself and contributes no arc, but which the layering guard's spelling net (which deliberately reads `#[cfg(test)]` regions) observes as one glob pair. Section 4.7 pins both contracts. The registry must contain exactly one test module and no nested test modules (the guard's glob count enforces this, Section 4.7.2).

Contents, all `pub(crate)`:

```rust
pub(crate) enum Disposition {
    /// Emitted as a rule in the generated instance .gitignore.
    Ignore,
    /// Deliberately tracked; never emitted. The row documents the decision
    /// and feeds the fixture's control paths.
    Track,
}

pub(crate) enum ArtifactKind {
    File,         // renders as "/{name}"
    Dir,          // renders as "/{name}/"
    Glob,         // renders as "/{name}"; name contains git wildcard characters
    GlobAnyDepth, // renders as "{name}", unanchored: git applies it at every
                  // depth under the instance dir. Exceptional by design; a
                  // registry test pins the count of such rows.
}

pub(crate) struct InstanceArtifact {
    pub(crate) name: &'static str, // file/dir name or glob, no leading slash
    pub(crate) kind: ArtifactKind,
    pub(crate) disposition: Disposition,
    pub(crate) comment: &'static str, // one "# AgentsCommander: ..." line
}

pub(crate) const INSTANCE_ARTIFACTS: &[InstanceArtifact] = &[ /* Section 4.2 */ ];

/// Pure string predicate mirroring ATOMIC_WRITE_TMP_GLOB, kept next to the
/// pattern so the writer-side tie test uses the policy instead of
/// paraphrasing it (Section 4.6). No external references.
pub(crate) fn matches_atomic_write_tmp_glob(file_name: &str) -> bool { /* pure string logic */ }
```

Name constants declared here and shared with writer modules (values in Section 4.2): `ATOMIC_WRITE_TMP_GLOB`, `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME`, `ACTIVITY_LOG_FILE_NAME`, `ACTIVITY_LOG_ROTATION_GLOB`, `AGENCY_TEMPLATES_SIBLING_GLOB`, `API_AUDIT_LOG_FILE_NAME`, `API_AUDIT_LOG_ROTATION_GLOB`, `ORPHAN_ARCHIVE_ROTATION_GLOB`, `API_CLIENTS_REGISTRY_FILENAME`, `API_CLIENTS_LOCK_FILENAME`, `MESSAGE_BUS_DB_FILENAME`, `MESSAGE_BUS_DB_GLOB`, `CODEX_HOME_DIR_NAME`, `CODING_AGENT_REQUESTS_DIR_NAME`, `GLOBAL_CONTEXT_TEMPLATE_FILENAME`, `GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB`, `CONTEXT_CACHE_DIR_NAME`, `COORDINATOR_CLOCKS_FILE_NAME`, `COORDINATOR_CLOCKS_TMP_GLOB`, `DEBUG_LOGS_FILE_NAME`, `TELEGRAM_DIAG_RAW_LOG_FILE_NAME`, `TELEGRAM_DIAG_SENT_LOG_FILE_NAME`, `GIT_GUARD_DIR_NAME`, `INSTANCES_DIR_NAME`, `LOGS_DIR_NAME`, `ORPHAN_ARCHIVE_FILENAME`, `PROJECT_REFRESH_REQUESTS_DIR_NAME`, `PTY_INPUT_LOCKS_DIR_NAME`, `SESSION_REQUESTS_DIR_NAME`, `SETTINGS_LOCK_FILE_NAME`, `SETTINGS_MIGRATION_BACKUP_GLOB`, `TELEGRAM_BRIDGE_LOG_FILE_NAME`, `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME`, `AGENCY_TEMPLATES_DIR`, `AGENT_TEMPLATES_DIR_NAME`, `CODING_AGENTS_CATALOG_DIR_NAME`, `UI_AUTOMATION_DIR_NAME`. (The draft's `MESSAGE_BUS_DB_SHM_FILENAME`/`MESSAGE_BUS_DB_WAL_FILENAME` are dropped: product decision 8 replaced the enumerated sidecars with `MESSAGE_BUS_DB_GLOB`, and no writer ever names a sidecar, SQLite does.)

Registry-internal `#[cfg(test)]` unit tests (self-references only, all inside the single `mod tests`):

- `ignore_rows_are_unique_and_byte_sorted_by_name`: names of `Ignore` rows are strictly increasing byte-wise (keeps the table append-proof and the generated file deterministic).
- `every_row_has_a_nonempty_single_line_comment`: nonempty, single line, starts with `# AgentsCommander: `.
- `no_name_contains_slash_backslash_newline_or_leading_slash_or_bang`: also refuses a leading `/` or `!`, so `render` stays the only place anchoring is decided and a generated negation is impossible by construction (backs acceptance criterion 10; findings 11.15.3).
- `no_file_or_dir_row_contains_a_git_wildcard`: the invariant that stops someone adding a glob without choosing a glob kind (finding 11.15.2).
- `message_bus_glob_derives_from_the_db_name`: `MESSAGE_BUS_DB_GLOB == format!("{MESSAGE_BUS_DB_FILENAME}*")` (replaces the draft's enumerated-sidecar test under product decision 8).
- `coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name`: `COORDINATOR_CLOCKS_TMP_GLOB == format!("{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp")` (the writer's `with_extension` construction guarantees exactly this prefix, Section 2.5).
- `atomic_write_tmp_predicate_agrees_with_its_glob`: table of positives (`.settings.json.1.tmp`, `.a.b.tmp`) and negatives (`foo.tmp`, `.foo.tmp`, `.a.tmpx`, `.api-clients-x.tmp`); the last negative documents why the api-clients temporaries need their own row (finding 11.6).
- `exactly_one_any_depth_row_exists`: pins `GlobAnyDepth` rows to exactly one (`.*.*.tmp`); widening depth-independence to another pattern is a policy decision, not a table tweak.
- `agency_templates_sibling_glob_derives_from_the_templates_dir` (round 2): `AGENCY_TEMPLATES_SIBLING_GLOB == format!("{AGENCY_TEMPLATES_DIR}.*")`. This is the test that keeps decisions 11 and 6 from drifting apart: the glob is the tracked directory's name plus a literal dot, so it can never be widened into the row it is required not to touch.
- `rotation_globs_derive_from_their_live_artifact_names` (round 2, decision 15): one table-driven test over the pairs `(ACTIVITY_LOG_ROTATION_GLOB, ACTIVITY_LOG_FILE_NAME)`, `(API_AUDIT_LOG_ROTATION_GLOB, API_AUDIT_LOG_FILE_NAME)` and `(ORPHAN_ARCHIVE_ROTATION_GLOB, ORPHAN_ARCHIVE_FILENAME)`, asserting each glob equals `format!("{live}.*")`. A table rather than three near-identical tests, so a fifth rotating artifact adds a row instead of a test, which is the same append-proofing the registry table itself has. `app.log.*` is deliberately absent from it: `app.log` has no registry constant (row 14 is an `L` literal whose writer Section 3 leaves alone), so its rotation row is an `N` literal and inventing a registry constant no writer imports would add a third list rather than a tie.
- `global_context_retired_backup_glob_derives_from_the_context_filename` (round 2): `GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB == format!("{GLOBAL_CONTEXT_TEMPLATE_FILENAME}.retired-*.bak")`, matching the writer's two shapes at `config/seeded_context_templates.rs:1599-1600`.

**Module doc comment (round 2, mandatory).** The registry's own doc carries two load-bearing properties, and its second one, "Every child of the instance config directory has a row here, `Ignore` or `Track`", was **false** as shipped in `dcd221fb`: grinch's implementation review found six classes with no row, and the round-2 enumeration found a seventh (Sections 14.1 and 14.3). Commit 4 makes the sentence true again by adding the rows, never by softening the sentence. Keep the claim as written, and extend that paragraph with the one thing it was missing, which is what a reader needs in order to keep it true: the claim rests on the Section 7 enumeration recipe, whose four legs and stop-the-line criterion are the procedure that produced this inventory, and a new artifact needs a row in the same change that introduces it. Do not weaken the claim to "the enumerated set"; that wording belongs to `instance_gitignore.rs`'s doc (Section 4.6), which describes emitted coverage, not to the registry's, which describes the inventory. Acceptance criterion 11 binds this.

Register in `src-tauri/src/config/mod.rs` with `pub(crate) mod instance_artifacts;` only. A `mod` declaration is not a reference, so the #1273 "config names nothing" premise is preserved; `config/mod.rs` gains no `use`. Note (finding 11.13.1): the guard's `observe` panics rather than skips when a scanned module is missing from the module tree, so forgetting this `mod` line fails loudly, which is expected behavior, not a bug to debug.

Why the constants live in the registry and the writers import them, not the reverse: the registry must reference every owner if constants stay owner-side, and the owners include knot members (`phone/mailbox`, `commands/*`, `web/commands`, `lib.rs`) plus `api/message_store`, which would need arcs in both directions (its `DB_FILENAME` outbound, its `pty-input-locks` inbound), a guaranteed two-node cycle. It would also force first entries into the layering guard's deliberately empty `crate::` table. Inverting the direction makes every new arc terminate in a zero-out-degree leaf, which cannot change any SCC, and leaves `instance_gitignore`'s reference delta at exactly one config-internal module.

### 4.2 The registry table

`Ignore` rows, byte-sorted by `name`. Provenance marks: **L** = pre-#1446 rule, kept as a table literal with a byte-identical pattern; **C** = the name is one of the shared constants of Section 4.1 (the row uses the constant, never a retyped literal); **G** = a glob constant whose value a registry unit test derives from its base-name constant; **N** = plain table literal, fixture-guarded only (writer deliberately untouched, Section 3):

| # | name | kind | comment (`# AgentsCommander: ...`) |
|---|---|---|---|
| 1 | `.*.*.tmp` C | GlobAnyDepth | transient atomic-write temporaries (`.{name}.{pid}.tmp`) at any depth; survive only a crash mid-write |
| 2 | `.agentscommander-context-templates.json` C | File | seeded context-template ownership state |
| 3 | `.agentscommander-injected-messages.json` L | File | injected-messages ownership state |
| 4 | `.api-clients-*.tmp` N | Glob | transient API client registry write temporaries |
| 5 | `activity.jsonl` C | File | append-only working-state activity log |
| 6 | `activity.jsonl.*` G | Glob | rotated generations of the activity log; the same runtime artifact under a numeric suffix |
| 7 | `agency-agents_templates.*` G | Glob | transient agency template cache lock and staging trees (`.lock`, `.next-`, `.download-`, `.prev-`); the tracked cache directory itself carries no dot and is not matched |
| 8 | `api-audit.log` C | File | append-only API audit log |
| 9 | `api-audit.log.*` G | Glob | rotated generations of the API audit log; the same runtime artifact under a numeric suffix |
| 10 | `api-clients.json` C | File | local API client registry |
| 11 | `api-clients.lock` C | File | persistent API client registry write lock |
| 12 | `api-message-bus.sqlite3*` G | Glob | inter-agent message bus database and every SQLite sidecar (`-shm`, `-wal`, `-journal`) |
| 13 | `app-outbox-path.txt` L | File | runtime outbox path handshake file |
| 14 | `app.log` L | File | application log |
| 15 | `app.log.*` N | Glob | rotated generations of the application log; the same runtime artifact under a numeric suffix |
| 16 | `codex-home` C | Dir | per-agent isolated coding-agent home trees |
| 17 | `coding-agent-requests` C | Dir | CLI-to-app coding-agent mutation request queue, including its `results/` subdirectory |
| 18 | `context-cache` C | Dir | regenerable per-session combined-context cache |
| 19 | `coordinator_clocks.json` C | File | coordinator idle-clock runtime state |
| 20 | `coordinator_clocks.json.*.tmp` G | Glob | transient coordinator-clock write temporaries |
| 21 | `daemon.pid` L | File | daemon process id |
| 22 | `debug-logs.txt` C | File | on-demand debug log dump |
| 23 | `diag-raw.log` C | File | Telegram bridge raw diagnostics log |
| 24 | `diag-sent.log` C | File | Telegram bridge sent diagnostics log |
| 25 | `git-guard` C | Dir | generated git-guard shim scripts |
| 26 | `injected-messages.default.toml` L | File | injected-messages reference defaults |
| 27 | `injected-messages.toml` L | File | injected-messages configuration |
| 28 | `injected-messages.toml.bak-*` L | Glob | injected-messages migration backups |
| 29 | `instances` C | Dir | per-instance runtime state directories |
| 30 | `logs` C | Dir | harness policy log directory |
| 31 | `master-token.txt` L | File | local API master token |
| 32 | `orphaned-sessions.archive.json` C | File | archived orphaned-session records |
| 33 | `orphaned-sessions.archive.json.*` G | Glob | rotated generations of the archived orphaned-session records; the same runtime artifact under a numeric suffix |
| 34 | `project-refresh-requests` C | Dir | project refresh request queue |
| 35 | `pty-input-locks` C | Dir | transient cross-process PTY input locks |
| 36 | `session-requests` C | Dir | CLI-to-app session launch request queue |
| 37 | `sessions.json` L | File | persisted session state |
| 38 | `settings.json` L | File | application settings |
| 39 | `settings.json.lock` C | File | transient settings write lock |
| 40 | `settings.pre-*.json` C | Glob | settings migration backups |
| 41 | `telegram-bridge.log` C | File | Telegram bridge log |
| 42 | `ui-automation` C | Dir | UI-automation session handshake state |
| 43 | `update-check.json` L | File | update-check cache |
| 44 | `update-check.json.tmp` N | File | transient update-check write temporary |
| 45 | `web-token.txt` L | File | local web token |

`Track` rows (never emitted; comments state why tracked):

| name | kind | comment |
|---|---|---|
| `Context.AgentsCommander.md` C | File | legacy standalone global context template; production retires it in place and never recreates it, and the entry holds user bytes; tracked on purpose (decision 14) |
| `Context.AgentsCommander.md.retired-*.bak` G | Glob | retirement backups of the standalone global context; user bytes, tracked on purpose (decision 13) |
| `Context.root-agent.md` C | File | user-editable root-agent context template; tracked on purpose |
| `ac-root-agent` (table literal; tie in Section 4.5) | Dir | canonical root-agent state (CLAUDE.md, memory, inbox); only its config.json rules are ignored |
| `agency-agents_templates` C | Dir | user-editable agency template sets; tracked on purpose |
| `agent-templates` C | Dir | user-editable role templates; tracked on purpose |
| `coding-agents` C | Dir | user-configurable coding-agent catalog; tracked on purpose |

The 12 L rows render byte-identically to today's `FIXED_RULES` (all are `File` or `Glob`, so no trailing slash touches a pre-existing rule; relative order preserved, verified in Section 11.3), which is what keeps every pre-change complete file byte-stable except for the one append. `FIXED_RULES` itself is deleted; the table is the single source.

**Canonical counts** (the only place in this plan where the derived numbers appear; every test derives them from the table, never retypes them):

- `Ignore` rows: **45**. `Track` rows: **7**. Table total: **52**.
- Emitted rules on a fresh file: 2 dynamic + 45 = **47**; fresh file length 2 lines per rule = **94 lines**.
- Rows appended to a byte-exact pre-change complete file: 45 minus the 12 L rows = **33** comment+pattern pairs (**66 lines**), namely every non-L row in table order.
- New-coverage provenance, "ni una mas ni una menos": **17** patterns from #1446's confirmed set (its 19 with the three enumerated SQLite literals replaced by one glob, product decision 8) + **7** from Section 12.1 (product decision 7) + **1** `api-clients.lock` (certification-pass completion of the same inventory: the persistent lock sibling of `api-clients.json`, same class as `settings.json.lock`; evidence in Section 2.5, decision logged in Section 13) + **1** `update-check.json.tmp` (product decision 9, #1164-control reversal) + **1** `.*.*.tmp` (under #1446's explicit #1209 delegation) + **2** from the round-2 implementation review (product decisions 11 and 12: `agency-agents_templates.*` and `coding-agent-requests`; Section 14.1) + **4** rotation globs (product decision 15, rows 6, 9, 15 and 33; Section 14.3) = **33**.
- Row numbers moved in round 2 and every reference in this plan was re-resolved against the table above. Round 2 inserts six rows in total, at final positions 6, 7, 9, 15, 17 and 33, so the pre-round-2 numbering maps as: `n <= 5` unchanged; `6 <= n <= 7` to `n + 2`; `8 <= n <= 12` to `n + 3`; `n >= 13` to `n + 4`. Sections 11 and 12 are preserved verbatim as signed enricher input and their row numbers are pre-consensus; Section 13's pointers were remapped.
- Non-obvious byte-sort adjacencies, verified: `.*.*.tmp` first (`*` is 0x2A); `.agentscommander-...` before `.api-clients-*.tmp` (`g` < `p`); `activity.jsonl` < `activity.jsonl.*` (prefix) < `agency-agents_templates.*` < `api-audit.log` < `api-audit.log.*` (prefix) < `api-clients.json` (second byte `c` 0x63 < `g` 0x67 < `p` 0x70, then within `api-` the fifth byte `a` 0x61 < `c` 0x63, the round-2 insertion points); `app.log` < `app.log.*` < `codex-home`, and `app-outbox-path.txt` is unaffected because `app.log.*` requires the literal `app.log.` prefix (`-` 0x2D < `.` 0x2E keeps the two `app` rows in their existing order); `orphaned-sessions.archive.json` < `orphaned-sessions.archive.json.*` < `project-refresh-requests`; `api-audit.log` < `api-clients.json` < `api-clients.lock` (`.j` < `.l`) < `api-message-bus.sqlite3*` < `app-outbox-path.txt` (`i` < `p`); `codex-home` < `coding-agent-requests` < `context-cache` (fourth byte `e` 0x65 < `i` 0x69, then third byte `d` 0x64 < `n` 0x6E, the second round-2 insertion point) < `coordinator_clocks.json` (`n` < `o`) and the `.tmp` glob after its prefix; `injected-messages.toml` < `injected-messages.toml.bak-*` < `instances` < `logs`; `session-requests` < `sessions.json` (`-` 0x2D < `s`); `settings.json` < `settings.json.lock` < `settings.pre-*.json`; `telegram-bridge.log` < `ui-automation` < `update-check.json` (`i` < `p`) < `update-check.json.tmp`.
- Two near-misses the round-2 rows deliberately do not touch, both probed against real `git check-ignore --no-index` (Section 14): `/agency-agents_templates.*` requires the literal dot, so `agency-agents_templates/engineering/role.md` stays visible under its `Track` row; and `coding-agent-requests` sorts before `coding-agents` (`-` 0x2D < `s` 0x73) but renders with a trailing slash, so `coding-agents/agents.json` stays visible under its own `Track` row. Both are already fixture controls (Section 8.5).
- `Track` row order is byte-sorted too, and the two round-2 rows lead the table: `C` 0x43 sorts before `a` 0x61, and `Context.AgentsCommander.md` precedes both its own `.retired-*.bak` glob (prefix) and `Context.root-agent.md` (`A` 0x41 < `r` 0x72).

### 4.3 Rewiring `instance_gitignore.rs`

1. Add module-internal
   ```rust
   pub(crate) struct RenderedRule {
       pattern: String,          // exact .gitignore line
       comment: &'static str,    // one "# AgentsCommander: ..." line
   }
   ```
   and `fn render(artifact: &InstanceArtifact) -> RenderedRule` applying the `ArtifactKind` rendering of Section 4.1: `File | Glob => "/{name}"` (one merged match arm, finding 11.15.2), `Dir => "/{name}/"`, `GlobAnyDepth => "{name}"`. Every pattern is root-anchored except `GlobAnyDepth` rows, which are depth-independent by design (consensus resolution of findings 11.5 and 12.3).
2. `required_rules(agent_local_dir: &str) -> Result<Vec<RenderedRule>, String>`:
   - rule 1: pattern as today (`/{ROOT_AGENT_DIR_NAME}/{escaped}/config.json`), comment `# AgentsCommander: per-instance override of the root agent's config.`
   - rule 2: pattern as today (`/{ROOT_AGENT_DIR_NAME}/config.json`), comment `# AgentsCommander: runtime config of the managed root agent.`
   - then `super::instance_artifacts::INSTANCE_ARTIFACTS` filtered to `Disposition::Ignore`, in table order, mapped through `render`.
   The two dynamic rules stay code because they depend on the runtime `agent_local_dir`; the registry covers only static artifacts. `escape_gitignore_path_segment` and the `\r`/`\n` name rejection are unchanged.
3. Signature migration: `[String; 14]` becomes `&[RenderedRule]` in `create_fresh_file`, `ensure_existing_file`, `missing_rule_indexes`, `append_buffer`, `fresh_file_bytes`, and only in those five. `contains_exact_line(bytes: &[u8], rule: &[u8])` keeps its signature and is called with `rule.pattern.as_bytes()` (finding 11.12). Detection semantics are untouched: `missing_rule_indexes` compares logical line bytes (with at most one trailing `\r` removed) against `rule.pattern` only. Comments are never part of detection, so a user-authored uncommented pattern still counts as present and never gets a comment retrofitted.
4. Emission:
   - `fresh_file_bytes`: for each rule in order, `{comment}\n{pattern}\n`. No blank separator lines, no marker lines, trailing LF via the last rule. Capacity pre-allocation sums `comment.len() + pattern.len() + 2` per rule (finding 12.12.3).
   - `append_buffer`: one leading `\n` if the existing content is nonempty and lacks a trailing `\n` (unchanged), then for each missing rule `{comment}\n{pattern}\n` in table order.
   - The open/validate/lock/reread machinery, error strings, and the fail-soft `[instance-gitignore] warning:` startup contract are untouched.
5. The module's reference delta is exactly one new family: `super::instance_artifacts::...`. No `crate::`-anchored reference is added anywhere in the module, tests included.

### 4.4 Owner-module changes (every writer names its artifact through the registry)

Existing constants become **const-alias definitions** so call sites and any external importers stay untouched. Consensus correction (findings 11.2 and 12.5, compiler-verified): the draft's `pub use ... as X;` re-export form does not compile for the `pub` rows (E0364: a re-export cannot exceed the `pub(crate)` registry item's visibility) and additionally trips `unused_imports` under `-D warnings`. The uniform legal form, used for all nine rows regardless of visibility, preserves each identifier, its type, and its exact measured visibility, and still breaks the build on a registry rename:

```rust
// api/message_store.rs (example)
pub const DB_FILENAME: &str = crate::config::instance_artifacts::MESSAGE_BUS_DB_FILENAME;
```

| file | constant | measured visibility at `b1eefa7c` | aliases registry constant |
|---|---|---|---|
| `config/activity_log.rs:119` | `FILE_NAME` | private | `ACTIVITY_LOG_FILE_NAME` |
| `api/message_store.rs:15` | `DB_FILENAME` | `pub` | `MESSAGE_BUS_DB_FILENAME` |
| `config/sessions_persistence.rs:56` | `ORPHAN_ARCHIVE_FILENAME` | `pub(crate)` | same name |
| `config/seeded_context_templates.rs:7` | `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME` | `pub` | same name |
| `config/coding_agents_catalog.rs:47` | `CATALOG_DIR_NAME` | private | `CODING_AGENTS_CATALOG_DIR_NAME` |
| `config/session_context.rs:11` | `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME` | `pub` | same name |
| `commands/role_templates.rs:47` | `AGENCY_TEMPLATES_DIR` | `pub` | same name |
| `api/auth.rs:45` | `REGISTRY_FILENAME` | `pub` | `API_CLIENTS_REGISTRY_FILENAME` |
| `testability/ui_automation.rs:17` | `UI_AUTOMATION_DIR` | `pub` | `UI_AUTOMATION_DIR_NAME` |
| `config/coding_agent_mutations.rs:35` (round 2) | `CODING_AGENT_REQUESTS_DIR` | `pub` | `CODING_AGENT_REQUESTS_DIR_NAME` |
| `config/session_context.rs:10` (round 2) | `GLOBAL_CONTEXT_TEMPLATE_FILENAME` | `pub` | same name |

The last two consensus-round rows cover the whole consensus-round artifact family for free: every production join of `api-clients.json` already imports `auth::REGISTRY_FILENAME` (Section 2.5), and `ui_automation.rs`'s deeper names (`SESSION_FILE`, request/response dirs) stay owner-side because the `ui-automation` Dir row covers the whole subtree.

The two round-2 rows are the same shape and cost exactly two const-alias edits and no call-site edits at all, which is why Section 4.4 gains no new inline-literal row in round 2:

- `CODING_AGENT_REQUESTS_DIR` is already the single canonical constant for the queue and both production join sites import it (`cli/coding_agent.rs:424` through the `ops` alias, `phone/mailbox.rs:10974` through the `ca` alias), so aliasing the declaration reaches both sides at once. `RESULTS_SUBDIR` (`:37`) stays owner-side: it names a child of the queue directory, which the `Dir` row covers as a subtree, exactly as `harness.log` stays owner-side under the `logs` row.
- `GLOBAL_CONTEXT_TEMPLATE_FILENAME` is the constant `config/seeded_context_templates.rs:1430` already joins onto its `config_dir` parameter, and `config/session_context.rs` is already a registry arc source (it declares `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME` as a const alias and imports `CONTEXT_CACHE_DIR_NAME`), so this row adds **no new arc source**.
- The agency template siblings need **no edit at all**: `cli/agency_templates.rs:10-15` already imports `AGENCY_TEMPLATES_DIR` from `commands::role_templates`, whose declaration at `commands/role_templates.rs:49` is already a registry const alias from commit 2. All four `format!("{}.<suffix>", AGENCY_TEMPLATES_DIR)` sites therefore already resolve to the registry, and a registry rename already breaks that file's build. Adding a direct registry import there would add an arc source and buy nothing; the plan does not.

Inline literals are replaced with the registry constant (one `use` or qualified path per file, literal swapped at each listed site):

| file:line | literal | constant |
|---|---|---|
| `config/session_context.rs:105,1413,2391` | `"context-cache"` | `CONTEXT_CACHE_DIR_NAME` |
| `config/coordinator_clocks.rs:314` | `"coordinator_clocks.json"` | `COORDINATOR_CLOCKS_FILE_NAME` |
| `api/message_store.rs:648` | `"pty-input-locks"` | `PTY_INPUT_LOCKS_DIR_NAME` |
| `api/audit.rs:19` | `"api-audit.log"` | `API_AUDIT_LOG_FILE_NAME` |
| `telegram/output.rs:64,141,142` | the three log names | `TELEGRAM_BRIDGE_LOG_FILE_NAME`, `TELEGRAM_DIAG_RAW_LOG_FILE_NAME`, `TELEGRAM_DIAG_SENT_LOG_FILE_NAME` (wiring this file requires the Section 4.8 guard row; both land in the same commit) |
| `commands/config.rs:80` and `web/commands.rs:723` | `"debug-logs.txt"` | `DEBUG_LOGS_FILE_NAME` (both writers share it) |
| `pty/local_backend.rs:334,437` | `"git-guard"` | `GIT_GUARD_DIR_NAME` |
| `lib.rs:1821` | `"instances"` | `INSTANCES_DIR_NAME` |
| `cli/create_agent_matrix.rs:211` and `phone/mailbox.rs:10807` | `"project-refresh-requests"` | `PROJECT_REFRESH_REQUESTS_DIR_NAME` |
| `config/settings.rs:3345` | `parent.join("settings.json.lock")` | one-token swap to `parent.join(SETTINGS_LOCK_FILE_NAME)` (single site, measured in Section 11.11; the join is on the settings file's parent, which is `config_dir()` in production) |
| `commands/role_templates.rs:135,217` | `"agent-templates"` | `AGENT_TEMPLATES_DIR_NAME` |
| `api/auth.rs:637,678` | `"api-clients.lock"` | `API_CLIENTS_LOCK_FILENAME` |
| `cli/harness.rs:475` | `"logs"` | `LOGS_DIR_NAME` (the nested `harness.log` at line 477 stays a literal; the Dir row covers the subtree) |
| `cli/create_agent.rs:133,295` and `phone/mailbox.rs:10831` | `"session-requests"` | `SESSION_REQUESTS_DIR_NAME` |
| `config/agent_command.rs:499,2046` | `"codex-home"` | `CODEX_HOME_DIR_NAME` (the log tag at line 540 and the temp-dir test at line 1335 are not artifact paths and stay untouched) |

`settings.pre-*.json` is registry-owned as a glob (comment documents it as settings migration backups). Its one live writer, `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`, reached from the pre-384 migration at `:1875`, write-once), names the concrete instance `settings.pre-384-v1.json` inline and is deliberately NOT rewired (Section 3): the glob constant cannot produce the concrete name, and the fixture's `settings.pre-384-v1.json` required sample is byte-equal to that writer's output, so the coverage is guarded where it matters. A future migration writing a new backup instance must add its own fixture sample; the `settings.pre-999-v9.json` sample already proves the glob's wildcard reach.

Nine writers are deliberately NOT rewired (Section 3): `update_check.rs:76` (row 44 is an N literal, like the legacy rows), `config/coordinator_clocks.rs:360-364` (row 20's glob is derivation-tested against `COORDINATOR_CLOCKS_FILE_NAME`, which row 19's writer swap already imports), `api/auth.rs:758` (row 4 is an N literal; the fixture's uuid-shaped sample guards it), `config/settings.rs:1911-1916` (`write_pre_384_v1_backup`; row 40's glob covers its concrete output, and the fixture sample equals that output byte for byte), the four rotation writers of decision 15 (`logging.rs:313` under the N row 15, and `api/audit.rs:403`, `config/activity_log.rs:624,628,638`, `config/sessions_persistence.rs:796` under the derivation-tested G rows 9, 6 and 33: each composes a numeric suffix at runtime, which no constant can express, while three of the four already take their base name from the registry), and `config/seeded_context_templates.rs:1597-1606` (the retirement-backup naming: it composes `{filename}.retired-{timestamp}[.{n}].bak` from the live entry's runtime `file_name()`, so no constant can produce the concrete name; its covering `Track` glob is registry-owned and derivation-tested against `GLOBAL_CONTEXT_TEMPLATE_FILENAME`, and the fixture carries a concrete sentinel of each of its two shapes).

Line numbers above are the evidence anchors from the gated investigation and the consensus-round verification, both at `b1eefa7c`; the implementer resolves each against the checkout and must not blind-patch by line number.

### 4.5 Root-agent naming tie

`ROOT_AGENT_DIR_NAME` stays exactly where #1273 put it (`config/mod.rs`), keeping `the_root_agent_dir_name_constant_is_defined_exactly_once` and the shallow `crate::config` scan untouched. The registry's `ac-root-agent` Track row uses a table literal (string literals are invisible to the spelling net). The tie is a new unit test in `instance_gitignore.rs` tests, using only already-allowed spelling families:

```rust
#[test]
fn root_agent_track_row_matches_the_root_agent_dir_constant() {
    let row = super::instance_artifacts::INSTANCE_ARTIFACTS
        .iter()
        .find(|a| matches!(a.disposition, Disposition::Track) && a.name == super::ROOT_AGENT_DIR_NAME)
        .expect("root-agent Track row");
    assert!(matches!(row.kind, ArtifactKind::Dir));
}
```

(Exact assertion shape up to the implementer; the contract is: a `Track` row whose `name` equals `super::ROOT_AGENT_DIR_NAME` and whose kind is `Dir` must exist.)

Spelling note (finding 12.12.4): `Disposition` and `ArtifactKind` used unqualified in this test need `use super::instance_artifacts::{Disposition, ArtifactKind};`, which the guard still observes as the single leading-segment child `instance_artifacts`; it does not add a table row beyond Section 4.7.1's.

### 4.6 Temporaries: the instance-dir half of #1209, and the off-shape schemes

Decision (product decision 10 plus consensus resolutions of 11.5, 11.6, 12.3): the instance-dir temporaries are covered by four dedicated rows, and this change closes **the instance-dir half** of #1209. The AC-root half (every `write_file_atomic` caller that publishes under the project `.ac/` root) is follow-up #1448, a one-row edit against `ensure_ac_root_gitignore`, which already emits the same pattern class (`/.seed-manifest.*.tmp`, `commands/ac_discovery.rs:1498-1501`).

- **Row 1 (`.*.*.tmp`, GlobAnyDepth, Ignore)** is the `temp_config_path` policy side, emitted **unanchored**. Grinch measured (12.3, direction A) that the draft's root-anchored form misses live, correct writers that publish `.{name}.{pid}.{n}.tmp` inside `config_dir()` subdirectories, including inside the tracked `coding-agents/` and `ac-root-agent/` trees (`config/coding_agents_catalog.rs:446`, `config/root_agent.rs:1682,1735`, `config/seeded_context_templates.rs:706`), where the leftovers would surface in `git status` on every write. Unanchored, git applies the pattern at every depth; the controls `foo.tmp` and `.foo.tmp` still hold at every depth (probed). The glob stays narrow: leading dot, at least two interior dots, `.tmp` suffix.
- **The writer-side tie that #1209 asked for** (its option 1, without exposing the private function): a new `#[cfg(test)]` test inside `config/local_config_io.rs`, `atomic_temp_names_stay_inside_the_ignored_glob`, which imports `crate::config::instance_artifacts::matches_atomic_write_tmp_glob` (safe direction: owner to leaf, and test-only, so it contributes no arc to the record), produces names via the module's own private `temp_config_path` for representative inputs (`settings.json`, a dotless name, a non-UTF-8 name hitting the `config.json` fallback), and asserts the **registry's own predicate** accepts each produced name. Using the predicate instead of a hand-written paraphrase is what makes the tie real (finding 11.6): the predicate lives next to the pattern, a registry unit test pins their agreement on a positive/negative table, and the git fixture remains the ground truth that git agrees. `temp_config_path` stays private; `write_file_atomic` is untouched.
- **The three off-shape schemes that `.*.*.tmp` cannot match** (12.3, direction B; all probed against real git) each get their own row: `coordinator_clocks.json.*.tmp` (row 20, glob derivation-tested against `COORDINATOR_CLOCKS_FILE_NAME`), `.api-clients-*.tmp` (row 4, N literal), and `update-check.json.tmp` (row 44, N literal, product decision 9: this name was a #1164 narrowness control and is in fact a live runtime temporary; the reversal is deliberate and user-approved).
- The git fixture check-ignores concrete samples for all four rows, including a subdirectory sample that proves depth-independence (Section 8).

**The two declared reversals of #1164 narrowness controls, enumerated in one place** (both are product decisions, neither was applied silently, and each moved a path out of the fixture's `control_paths` and into `required_paths`):

1. **`update-check.json.tmp`** (product decision 9, consensus round 1). #1164 kept it as a control on the theory that it was a hypothetical near-miss of `/update-check.json`. It is a live runtime temporary whose writer names it in its own comment (`update_check.rs:76`). Row 44.
2. **`app.log.1`** (product decision 15, round 2). #1164 kept it as a control on the same theory, as a near-miss of `/app.log`. It is a live rotated generation: `logging.rs:275-330` rotates `app.log` into `app.log.1` .. `app.log.5` (`APP_LOG_KEEP = 5`, `:248`). Row 15's glob covers it, and the other three rotating artifacts are covered by rows 6, 9 and 33 without touching any control.

Both reversals share one shape worth naming, because it is how this class stayed hidden through three inventories: a fixture control that asserts "this must NOT be ignored" reads as a deliberate narrowness decision, so nobody re-checks whether the name has a live writer. A control is only evidence of a decision, never evidence that no writer exists; the Section 7 recipe treats controls as candidates like any other name.

`instance_gitignore.rs`'s module doc comment claims coverage for **the enumerated registry set**, not blanket completeness (consensus resolution of 12.3's wording point): it states that the registry in `instance_artifacts.rs` is the single source, that Track rows are deliberate, that the atomic-write and off-shape temporary schemes enumerated there are covered, and that a new artifact requires a new row (Section 3's recorded rule).

### 4.7 Layering guard update (`tests/instance_gitignore_layering.rs`)

The guard's six `ALLOWED_*` tables are its whole contract and it is written to be widened. This change:

1. Widens `ALLOWED_GUARDED_SUPER_REFERENCES` by **exactly one row**, `("src/config/instance_gitignore.rs", "instance_artifacts")`, growing the array from 6 to 7 entries (finding 11.8: `children_under` reports the leading segment, so the production spelling yields this single pair, and the test-module spellings `super::super::instance_artifacts::...` report leading segment `super`, already an allowed row). The `crate::`-anchored table stays empty. The new row gets a doc-comment paragraph on the model of the existing `injected_messages` one, stating why this reference is allowed in production code: the target is a leaf with zero outgoing arcs, so unlike `injected_messages` it does not depend on being `#[cfg(test)]`.
2. Adds `config::instance_artifacts` as a third scanned unit, observed with `Reach::WithSubmodules` (finding 11.7: the shallow reach exists for `config` because its children are separate graph nodes; the registry has no children and must never gain one, so the deeper reach costs nothing and refuses a future child that parks a reference). Its tables (findings 11.1 and 12.4, measured against the guard's own mechanism): `ALLOWED_REGISTRY_CRATE_REFERENCES` empty, `ALLOWED_REGISTRY_SELF_REFERENCES` empty, `ALLOWED_REGISTRY_SUPER_REFERENCES` exactly `[("src/config/instance_artifacts.rs", "*")]`, mirroring the host precedent at `ALLOWED_HOST_SUPER_REFERENCES` (`:565`): the one glob pair is the registry's own `#[cfg(test)] mod tests { use super::*; }` and nothing else may appear. The unit's `Observation` glob count is pinned to exactly 1, the same hardening the host row carries, so a second or nested test module cannot reopen the hole that count exists to close.
3. Extends the file's doc comment: the detector criterion now also includes `sccSize(agentscommander_lib::config::instance_artifacts) = 1`.
4. `the_root_agent_dir_name_constant_is_defined_exactly_once` stays green unchanged (the constant does not move; the registry's `"ac-root-agent"` is a string literal, which the net strips). The registry must not declare any `const` or `static` named `ROOT_AGENT_DIR_NAME`; that test scans every file under `src/`, including the new module (finding 11.13.2; also a Section 7 checklist item).
5. While editing this file's prose (points 1-3 already touch it): the doc comments cite a hardcoded arc count of 976 in several places (grinch anchors: `:34`, `:129`, `:551`, `:1649`, `:1729`, `:1796`, `:1889`), stale since #1273; the real count at `b1eefa7c` is 1009 and changes again with this very change. Replace the hardcoded counts with count-free wording that points at `src-tauri/module-arcs.txt` as the record, so the prose cannot rot a third time. Apply the same count-free treatment to the stale knot-member counts those doc comments state as current fact (87 or 88 members; the measured count at `b1eefa7c` is 85 and keeps drifting); sentences recording historical experiments ("took the knot 87 to 91") are records of past measurements, not current facts, and stay. Prose only; nothing reads the number at runtime (12.11.2).

Round-2 addendum (grinch finding 2, in the scope of commit 4). One hardcoded count survived that treatment inside a sentence that was otherwise de-numeralized, and it is now wrong: `the_constant_home_names_nothing_at_all`'s assertion message (`tests/instance_gitignore_layering.rs:1941-1944`) reads "Measured over the arcs of `src-tauri/module-arcs.txt`, `agentscommander_lib::config` appears on the left of the separator zero times and **on the right 49 times**". The measured value is **51**, at `b1eefa7c` and at `dcd221fb` alike (`grep -c ' -> agentscommander_lib::config$' src-tauri/module-arcs.txt`; re-measured first-hand in round 2, and 12.11.1 had already recorded 51). The other half of the same sentence was correctly de-numeralized ("over the 976 arcs of" became "over the arcs of"), which leaves a stale number presented as a current measurement right beside it, so a reader who checks the non-absorption argument against the record finds 51, not 49, and stops. Replace the count with count-free wording that keeps the argument intact, for example "appears on the left of the separator zero times and on the right many times: it is a pure sink". Prose only; nothing reads it at runtime. This file is already inside acceptance criterion 9's allowed set, so commit 4 carries the fix with no scope change.

### 4.8 Second layering guard update (`tests/claude_watcher_layering.rs`)

Added in recertification (2026-08-20): implementation proved that a second equality guard covers `src/telegram/output.rs`, which this plan had not accounted for: Section 4.4 mandated wiring that file while acceptance criterion 9 forbade touching the guard that the wiring turns red. Facts, verified first-hand at `0e902d85`:

- `production_output_module_owns_the_seam_alone` (`tests/claude_watcher_layering.rs:2539-2547`) builds the real module index from `src/lib.rs` and, via `require_exact_output_report` (`:1968-1975`), requires BY EQUALITY that the analyzed output module's source set is exactly `{src/telegram/output.rs}` and that its dependency set equals `expected_output_dependencies()` (`:1269-1282`): today exactly 4 rows (`agentscommander_lib::config`, `::network`, `::telegram::api`, `::telegram::redact`), with no doc comment on the table.
- The guard observes a registry reference as its own distinct row `agentscommander_lib::config::instance_artifacts`, not as the existing `config` row (dev-rust's measured failure output; the visitor resolves against the crate's real module tree, where the registry is declared).
- The fixed `production_modules()` list (`:1234-1250`) does not feed this test and needs no change.

Change, exactly one row plus prose:

1. `expected_output_dependencies()` grows from 4 to 5 rows: add `(OUTPUT_SOURCE, "agentscommander_lib::config::instance_artifacts")`, placed after the `agentscommander_lib::config` row (the collection is a `BTreeSet`; array position is cosmetic).
2. A comment paragraph on the table (match the file's comment style) stating why the fifth row is allowed: the target is the #1446 artifact registry, a pure-constants leaf with zero outgoing arcs in `src-tauri/module-arcs.txt`, so an arc into it can neither create, grow, nor join any SCC (the same argument `instance_gitignore_layering` records for its own `instance_artifacts` row, Section 4.7.1); it grants the output seam no new capability (no I/O, no transport, no telegram surface): it only lets the module name its three log artifacts (registry rows 23, 24, 41) through the registry, so a rename breaks the build instead of silently reopening the gitignore gap. The set stays equality-pinned: any sixth dependency still turns this guard red.
3. Nothing else in that file changes: the source set stays exactly `{src/telegram/output.rs}`, and every other expected table, `production_modules()`, and the guard mechanism stay untouched. After wiring `telegram/output.rs` and widening the table, run `cargo test --test claude_watcher_layering`; if anything in that file still fails, stop and report to the architect: this plan authorizes no further contract change in that file.

## 5. Required behavior and edge cases

1. **Fresh directory**: the generated file is, in order, comment+pattern pairs for the 2 dynamic rules and every `Ignore` row of Section 4.2 in table order, each pair `{comment}\n{pattern}\n`, no blank lines, trailing LF. Line count per the Section 4.2 canonical block.
2. **Existing complete pre-change file (the 14 rules, no comments)**: byte-stable except for one append of exactly the non-L pairs of Section 4.2 in table order (count in the canonical block). A second ensure is byte-stable. This is the no-migration compatibility guarantee for every existing writable installation (edge 10 scopes the rest).
3. **Existing complete post-change file**: byte-stable, no write.
4. **User-authored occurrences**: a user line equal to a required pattern counts as present (byte-exact, comments transparent) and never receives a generated comment; user comments, negations, duplicates, CRLF content, missing final newline, and invalid UTF-8 are preserved under the existing byte semantics. Three cosmetic residuals, stated so they are discovered here and not filed as bugs (findings 11.12 and 12.9): a user who deletes a generated pattern line but keeps its comment gets the full pair re-appended, leaving the orphaned comment mid-file; a UTF-8 BOM hides the first line from byte-exact detection, so that one rule is re-appended once (self-healing on the second ensure, pre-existing behavior); a CRLF file gains an LF-ending appended block, a mixed-endings file (pre-existing behavior, now with a larger appended block).
5. **Never ignored** (fixture controls): a concrete sentinel for every Track row, including the two round-2 `Context.AgentsCommander.md` rows (one sentinel per backup shape), the generated `.gitignore` itself (product decision 5; asserted without materializing a control file over it, Section 8), `foo.tmp` and `.foo.tmp` at any depth, `app.log.1`, `cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, the four injected-messages near-misses, and a plain file bearing a Dir row's name (the trailing slash is load-bearing; finding 11.9). `update-check.json.tmp` is no longer a control: product decision 9 made it a covered artifact.
6. **No untracking**: adding rules never changes the index; the fixture proves a pre-tracked `instance/app.log` AND a pre-tracked `instance/api-message-bus.sqlite3` (a newly covered artifact, finding 12.10) stay tracked.
7. **Concurrency, symlink/reparse safety, locking, fail-soft startup warning**: unchanged from #1164 behavior; no algorithmic change is authorized by this plan.
8. **Ordering**: emission order is table order (dynamic first). Reconciliation never reorders an existing file; order only affects fresh files and the append block.
9. **The knot must not grow**: every new module arc terminates in `instance_artifacts` (out-degree zero). `cyclicSccs` stays 1; `sccSize` of `instance_gitignore` and `instance_artifacts` stays 1.
10. **Read-only or locked existing files** (finding 12.9.1): this change makes every pre-change complete file partial once, so an instance `.gitignore` that is read-only, on a read-only volume, or held by another process takes the existing failing branch on every startup until it is writable: the ensure returns `Err`, the file is untouched, and the fail-soft `[instance-gitignore] warning:` is printed via `eprintln!`, which `machine_output_enabled()` suppresses entirely. Behavior is the #1164 contract, unchanged; what changes is the affected population. Section 6 documents it and Section 8 pins it with a test.

## 6. Compatibility and security impact

- **Existing installations, writable file (the overwhelming population)**: additive repair on next startup via the existing reconciliation; no migration, no user action, no comment retrofit on already-present rules.
- **Existing installations, read-only or locked file** (finding 12.9.1): the repair does not happen and the startup warning recurs, invisibly under machine output (Section 5.10). Remediation: clear the read-only bit (or release the lock, or delete the file so a fresh one is generated); the next startup repairs. The "no user action" guarantee is scoped to writable files, deliberately.
- **Already-tracked artifacts** (finding 12.10): a new rule never untracks. A user who already committed newly covered artifacts keeps them tracked, which for the message bus means a tracked database whose `-wal`/`-shm` sidecars are now ignored: a partially committed database, not just noise, and `api-clients.json` is credential-adjacent. Remediation, documented here and to be carried into release notes: from the instance directory, `git rm --cached api-clients.json api-clients.lock api-message-bus.sqlite3 api-message-bus.sqlite3-shm api-message-bus.sqlite3-wal api-message-bus.sqlite3-journal activity.jsonl api-audit.log coordinator_clocks.json debug-logs.txt diag-raw.log diag-sent.log orphaned-sessions.archive.json settings.json.lock telegram-bridge.log update-check.json.tmp .agentscommander-context-templates.json` plus any tracked `settings.pre-*.json` backup (e.g. `settings.pre-384-v1.json`), plus `git rm -r --cached` for any of `codex-home coding-agent-requests context-cache git-guard instances logs project-refresh-requests pty-input-locks session-requests ui-automation` that are tracked, plus any tracked `agency-agents_templates.lock` or `agency-agents_templates.next-*`/`.download-*`/`.prev-*` staging tree left behind by an interrupted template update, plus any tracked rotated generation (`app.log.*`, `api-audit.log.*`, `activity.jsonl.*`, `orphaned-sessions.archive.json.*`; product decision 15 makes these covered for the first time, and `app.log.1` in particular is the name most likely to be sitting in an existing repository because the policy previously asserted it was not ignored) (each command only for paths actually in the index; `git status` shows which). Untracking remains out of scope for the code (Section 3).
- **Rust/API surface**: internal only. No IPC, schema, settings, frontend, or dependency changes. Const-alias definitions keep every existing constant importer compiling at its original visibility.
- **Ignore semantics**: all new patterns are root-anchored under the instance dir except the single depth-independent `.*.*.tmp` row (Section 4.6); `Dir` rows use a trailing slash so a plain file with the same name is not silently ignored (tested, finding 11.9). `state.sqlite` and `cache/entry.bin` controls prove the sqlite/cache rules stay narrow.
- **Security**: the generated file still contains only path patterns, never secret values. The hardened open path is untouched. The new comments disclose only artifact purposes.
- **Performance**: the ensure still runs once per startup; the rule set grows from 14 rules to the Section 4.2 count plus comments, negligible.

## 7. Implementation order

### Phase 1: MVP (two commits, finding 11.14: if the second must ever be reverted, the first still ships the whole user-visible fix, and a bisect can separate "the rules changed" from "twenty-odd modules changed")

Commit 1, self-contained and delivering the entire user-visible fix:

1. Add `config/instance_artifacts.rs` (types, constants, predicate, table, internal tests) and register it in `config/mod.rs`. Checklist (finding 11.13): the `mod` line must land (the guard's `observe` panics loudly without it), and the registry must not declare a `ROOT_AGENT_DIR_NAME` const (the exactly-once guard scans every file under `src/`). Then re-run the enumeration recipe below on the implementation tree: every production publication target must have a registry row, and any unregistered child is a stop-the-line question for the coordinator, never a silent omission (12.1's standing requirement, restated in Section 14).

**Enumeration recipe (round 2 replaces the `-A3` window; Section 14).** The recipe this plan carried until round 2 was `grep -rn 'config_dir()' src -A3 | grep -oE '\.join\(...\)'`. It cannot sustain the invariant the plan assigned to it, and it did not: it is blind to a join more than three lines below its root binding (`cli/coding_agent.rs`, root at `:402`, join at `:424`, and `phone/mailbox.rs`, root at `:10970`, join at `:10974`, which escapes by one line), to a join whose root came through a wrapper that never spells `config_dir()` (`cli/agency_templates.rs`, which resolves through `config_dir_or_err()` at `:301-304`), and to a join on a **parameter** named `config_dir` (`config/seeded_context_templates.rs:1430`). Run the four legs below from `src-tauri/`. Legs 1-3 are static and see transient artifacts that exist on no particular machine; leg 4 is ground truth and sees artifacts no static leg spelled. Neither half is sufficient alone, so the candidate set is their union.

```text
# Leg 1 - every way production obtains the instance dir, including wrappers and parameters
rg -n --type rust '\bconfig_dir\(\)' src
rg -n --type rust 'fn [a-z_]*config_dir[a-z_]*\s*\([^)]*\)\s*->\s*(Option<PathBuf>|Result<PathBuf|PathBuf)' src
rg -l --type rust 'config_dir:\s*&Path' src
# union of the FILES those three report is the leg-1 file set

# Leg 2 - publications, FILE-scoped over the leg-1 file set (no line window)
rg -n --type rust '\b(config_dir|cfg_dir|instance_dir|config_directory|config_root)\s*\.(join|with_extension|with_file_name)\(' <leg-1 files>

# Leg 3 - sibling schemes: a sibling inherits its neighbour's directory, so no
# join sweep of any width can see it. Tree-wide, then parent-rooted in leg 1.
rg -n --type rust '\.with_extension\(|\.with_file_name\(' src
rg -n --type rust '\bparent\s*\.(join|with_extension|with_file_name)\(' <leg-1 files>

# Leg 4 - ground truth: list the children of at least two real instance dirs and
# diff them against the table (glob rows matched, not compared literally).
```

**Stop-the-line criterion.** Each leg yields candidate names. Resolve every candidate to the concrete name or names its writer can produce, then classify it as exactly one of: **(a)** matched by a registry row, by literal equality or by glob match; **(b)** not a direct child of the instance dir, either because it is nested under a row that already covers its subtree or because its root is outside `config_dir()`, with that root named; **(c)** recorded in Section 3 as a deliberate exclusion with its evidence; **(d)** (added in round 3) a direct child that **no production writer can produce**, that is, bytes a person placed in the instance directory by hand (a copy, a rename, an editor leftover). A (d) name is outside the registry's subject: it gets no row, it is deliberately not covered, and it stays visible to git, which is the correct outcome for the user's own bytes. A candidate in none of the four **halts the change**: it is a product question for the coordinator, never a silent omission and never an architect's call. Record the classification of every candidate that is not (a), so the next run starts from a decided baseline instead of re-deriving one.

**The evidentiary bar for (d).** (d) is the only classification open to a name that has no writer at all, and that is a thing only leg 4 can produce: legs 1 to 3 enumerate writers, so a leg-4 name with no leg-1-to-3 origin is either an artifact the static legs missed or not an artifact at all, and from a directory listing those two are indistinguishable. (d) is therefore also the classification a missed artifact would hide behind, so it is never available on the strength of "I looked and found no writer". Three independent findings are required, and all three are recorded with the classification: **(i)** leg 3a, the tree-wide `with_extension`/`with_file_name` sweep, run in full with every site accounted for, composes no such name; **(ii)** a search of the **shipped binary** (which carries the JS bundle as well as the Rust) for the concrete name and for its distinctive fragment returns zero, which is what rules out a name assembled at runtime from pieces no single source line spells together; **(iii)** at least one forensic property of the bytes on disk that a runtime sibling cannot have (size, mtime ordering against the live artifact, or a strictly older key set). Anything less is not (d): the candidate is unclassified and the line halts.

**What (c) and (d) mean for the registry doc's strong claim (round 3).** The registry module doc says "Every child of the instance config directory has a row here", and Section 4.1 forbids softening it. That sentence has two standing counterexamples on real disks, and both are correct: the generated `.gitignore` itself, excluded by product decision 5 and recorded in Section 3, classification (c); and hand-made user copies, classification (d), measured as `settings.json.backup.jo` (Section 14.2). Neither is a hole in the inventory, because the claim's subject is the set of artifacts **production code publishes**, which is the only set an enumeration over `src/` can ever produce and the only set the registry exists to govern. A future reader who meets one of them must close nothing, and in particular must not reach for either tempting repair. Adding a row is wrong twice over: a row for the generated `.gitignore` self-ignores the file this change generates, which acceptance criterion 10 forbids outright, and a row for a user copy ignores the user's own bytes, which is the harm the whole registry exists to avoid inflicting. Rewording the sentence to "every enumerated child" is wrong because that is `instance_gitignore.rs`'s wording about emitted coverage (Section 4.6), and importing it here would license exactly the silent omissions round 2 found. The sentence stays as written; a child genuinely outside its subject gets a recorded (c) or (d) classification, and that record, not the sentence, is what the next reviewer checks.

Measured on the implementation tree at `dcd221fb` (Section 14 carries the full result): leg 1 reports 46 files, where the pre-round-2 recipe's direct-call grep alone reports 45 and misses three of the four files that produced the round-2 defect; legs 2 and 3 reduce to a bounded candidate list; leg 4 was run against two real instance dirs. The run reproduced the four classes of decisions 11-14, confirmed the previously recorded test-only hit (`.claude-mb`, a `#[test]`-only tempdir join in `commands/session.rs`, not a `config_dir()` artifact), and surfaced a fifth class that no prior sweep had reported, the rotated generations, which leg 3 found and which no root-anchored `.join` sweep of any width could have found. That class was escalated rather than decided, and the user ruled it `Ignore` as product decision 15 (Section 14.3). Every candidate the run produced is now classified (a), (b) or (c), which is what acceptance criterion 11 requires and what makes the registry's completeness claim true again.
2. Rewire `instance_gitignore.rs` per Section 4.3 (render pipeline, `Vec<RenderedRule>`, comment emission, doc-comment update per Section 4.6); delete `FIXED_RULES`.
3. Update the in-file tests and fixture (Section 8), including the new compatibility, read-only-legacy, dir-semantics and root-agent tie tests.
4. Update `tests/instance_gitignore_layering.rs` per Section 4.7.
5. Regenerate `src-tauri/module-arcs.txt` and commit it in this commit.

Commit 2, the drift-closure wiring:

6. Owner-module edits per Section 4.4 (const aliases first, then literal replacements) and the `local_config_io` tie test per Section 4.6.
7. Regenerate `src-tauri/module-arcs.txt` again (this commit adds the owner arcs) and commit the delta here.

Recertification addendum (2026-08-20 UTC): commits 1 and 2 landed as `2632adb4` and `0e902d85` with one deliberate omission: the `telegram/output.rs` wiring was reverted in-branch because it turns the Section 4.8 guard red, an insufficiency of this plan as originally certified (Section 4.4 mandated the wiring while acceptance criterion 9 forbade touching that guard). Commit 3 closes it, carrying together: the three `telegram/output.rs` literal swaps (Section 4.4), the one-row widening plus its comment paragraph in `tests/claude_watcher_layering.rs` (Section 4.8), the regenerated `src-tauri/module-arcs.txt` (exactly one additional arc, `telegram::output -> config::instance_artifacts`, a source already inside the Section 8 whitelist), and this updated plan file. Re-run the Section 8 cargo gates and the Step-N criterion on that commit.

Recertification addendum, round 2 (2026-08-20 UTC): commit 3 landed as `dcd221fb` and grinch's implementation review found the plan's inventory incomplete, not the implementation wrong (Section 14). Commit 4 carries, together: the six new `Ignore` rows and the two new `Track` rows of Section 4.2 (decisions 11, 12 and 15) with the seven new registry constants and three new derivation tests of Section 4.1; the two const-alias edits of Section 4.4 (`config/coding_agent_mutations.rs:35`, `config/session_context.rs:10`); the fixture samples and control sentinels of Section 8.5, the move of `app.log.1` from `control_paths` to `required_paths` (product decision 15, the declared reversal of Section 4.6), and the widened `track_rows_are_exactly_the_declared_track_set`; the registry doc-comment repair (Section 4.1, point 2 of the module doc); the de-numeralization of the surviving `49` in `tests/instance_gitignore_layering.rs:1941-1944` (Section 4.7.5 round-2 addendum, grinch finding 2); the regenerated `src-tauri/module-arcs.txt` (exactly one additional arc, `config::coding_agent_mutations -> config::instance_artifacts`); and this plan file. Every file it touches is already inside acceptance criterion 9's allowed set. Re-run the Section 8 cargo gates and the Step-N criterion on that commit.

Recertification addendum, round 3 (2026-08-20 UTC): commit 4 landed as `645b870b` and passed grinch's line-by-line review on every technical point (rows, render, doc comment, derived canonical counters, the narrowness of all four rotation globs against real git, the declared `app.log.1` reversal, Step-N, both guards, clippy, the full suite, criteria 9 and 11's rotation half, non-visual classification). The round-3 PLAN-DEFECT is a point of record only (Section 10). Commit 5 therefore carries **this plan file alone**: classification (d) with its evidentiary bar and the doc-claim reconciliation above, the widened acceptance criterion 11, the Section 14.2 (d) entry, and Section 10's round-3 subsection. Zero `.rs`, zero fixture, zero `module-arcs.txt`, zero rows and no canonical count moved, so no gate result from `645b870b` is invalidated and none needs re-running.

### Phase 2: Full features

None beyond the MVP; the registry and the full rule set are the feature.

### Phase 3: Polish

1. Run the Section 8 gates; rustfmt only the files this plan touches (the repo carries pre-existing fmt drift; a workspace-wide fmt is out of scope).
2. Diff review: no production reference added to `instance_artifacts.rs` (only its own single test module); no `crate::` reference added to `instance_gitignore.rs`; no emitted rule beyond Section 4.2; the cumulative module-arcs delta matches the Section 8 Step-N whitelist exactly.

### Phase 4: Extras

None. Rotation (#1441), lock sweeping (#1443), retrofitting legacy-rule writers, and untracking remain out of scope.

## 8. Tests

Updates in `src-tauri/src/config/instance_gitignore.rs`:

1. `fresh_file_has_exact_fourteen_rules_and_dynamic_name` becomes `fresh_file_matches_the_registry_and_dynamic_name`: asserts the fresh bytes equal the exact expected content built by the shared helper from the table (2 dynamic + every Ignore row, table order, trailing LF), the dynamic name appears only in rule 1, every pattern is root-anchored except those of `GlobAnyDepth` rows (which must not start with `/`), no generated `!` or `/*` line, and the rule and line counts equal the table-derived values (never a retyped literal; the Section 4.2 canonical block is prose for humans).
2. New `legacy_fourteen_rule_file_gains_exactly_the_new_entries` (the tech-lead compatibility criterion): seed the file with the byte-exact pre-change 14-line content for `TEST_AGENT_LOCAL_DIR`, built as: the two dynamic lines composed at runtime from `super::ROOT_AGENT_DIR_NAME` (finding 12.9.4: freezing them would couple the test to the constant's value and mask the real cause on a change) plus the 12 historical `FIXED_RULES` lines frozen as a literal block (they leave the production code in this change; the literal preserves the historical bytes). Run the ensure; assert the original bytes are an exact prefix, the appended block is exactly the non-L pairs in table order each once (derived from the table), and a second ensure is byte-stable.
3. New `read_only_legacy_complete_file_fails_without_modification` (finding 12.9.1): seed the same legacy-complete content, mark the file read-only, run the ensure; assert `Err` and byte-identical content. This pins Section 5.10's population statement.
4. New `root_agent_track_row_matches_the_root_agent_dir_constant` (Section 4.5) and new `track_rows_are_exactly_the_declared_track_set`: the `Track` names are exactly `{Context.AgentsCommander.md, Context.AgentsCommander.md.retired-*.bak, Context.root-agent.md, ac-root-agent, agency-agents_templates, agent-templates, coding-agents}` (product decision 6 plus round-2 decisions 13 and 14, frozen in a test). The test sorts the names before comparing, so the expected vector is in byte order and the two round-2 rows lead it.
5. `git_fixture_ignores_exactly_required_paths_without_untracking`:
   - `required_paths` additions (one concrete sample per new pattern, nested samples for every `Dir` row per finding 11.9): `.settings.json.12345.tmp`, `coding-agents/.agents.json.4242.0.tmp` (depth-independence of row 1, inside a Track dir, the exact leftover class 12.3 measured), `.agentscommander-context-templates.json`, `.api-clients-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed.tmp`, `activity.jsonl`, `api-audit.log` (moved from controls), `api-clients.json`, `api-clients.lock`, `api-message-bus.sqlite3`, `api-message-bus.sqlite3-shm`, `api-message-bus.sqlite3-wal`, `api-message-bus.sqlite3-journal` (the sample that makes the glob's reason testable, finding 12.7), `codex-home/agent-1/config.toml`, `context-cache/ac-context-1.md`, `coordinator_clocks.json`, `coordinator_clocks.json.4242.7.tmp`, `debug-logs.txt`, `diag-raw.log`, `diag-sent.log`, `git-guard/git.cmd`, `instances/0f0e/instance.json`, `logs/harness.log`, `orphaned-sessions.archive.json`, `project-refresh-requests/req-1.json`, `pty-input-locks/operation-1.lock`, `session-requests/create-1.json`, `settings.json.lock`, `settings.pre-384-v1.json`, `settings.pre-999-v9.json`, `telegram-bridge.log`, `ui-automation/session.json`, `update-check.json.tmp` (moved from controls, product decision 9); and, for the two round-2 rows (decisions 11 and 12), one sample per shape the writers can produce: `agency-agents_templates.lock`, `agency-agents_templates.next-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed/engineering/role.md`, `agency-agents_templates.download-2c8e7dda-ccae-5c3e-ac6e-bc9eacce5df1/.git-clone-marker`, `agency-agents_templates.prev-3d9f8eeb-ddbf-6d4f-bd7f-cdafbddf6ea2/design/role.md` (the three staging shapes are whole directory trees, so their samples are nested: that is what proves a rule with no trailing slash still reaches through a matched directory), `coding-agent-requests/req-1.json`, and `coding-agent-requests/results/res-1.json` (the nested sample that proves the `Dir` row covers the `results/` subtree in one rule, per decision 12); and, for the four rotation rows (decision 15), `activity.jsonl.1`, `api-audit.log.1`, `orphaned-sessions.archive.json.3` (the `ORPHAN_ARCHIVE_KEEP` edge), plus `app.log.1` **moved here from `control_paths`** and `app.log.5` (the `APP_LOG_KEEP` edge, so the sample set proves the glob spans the whole generation range rather than only its first element). All eleven were probed against real `git check-ignore --no-index` before certification (Section 14).
   - `control_paths`: the seven remaining legacy controls (`cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, `injected-messages.toml.bak`, `injected-messages.json`, `agentscommander-injected-messages.json`, `sub/injected-messages.toml`; `app.log.1` leaves this list by product decision 15, the second declared #1164 reversal, Section 4.6), plus `agent-templates/default-role.md`, `agency-agents_templates/engineering/role.md`, `coding-agents/agents.json`, `Context.root-agent.md`, `ac-root-agent/CLAUDE.md`, `foo.tmp`, `.foo.tmp`, `sub/foo.tmp` (narrowness holds at depth too), plus the round-2 Track sentinels `Context.AgentsCommander.md`, `Context.AgentsCommander.md.retired-20260820-101112Z.bak` and `Context.AgentsCommander.md.retired-20260820-101112Z.3.bak` (one per writer shape, `config/seeded_context_templates.rs:1599-1600`). Two controls already in this list become load-bearing in round 2 and must not be removed: `agency-agents_templates/engineering/role.md` is what proves the new `/agency-agents_templates.*` glob does not reach the suffix-less `Track` directory (the literal dot is the whole mechanism), and `coding-agents/agents.json` is what proves the new `/coding-agent-requests/` row does not reach its byte-order neighbour. Both were probed (Section 14). Decision 15 adds three controls of its own, all probed: `sub/app.log.1` and `sub/activity.jsonl.1` must NOT be ignored, which is what proves the four rotation rows are `Glob` (root-anchored) and not a second `GlobAnyDepth`, the property `exactly_one_any_depth_row_exists` pins from the other side; and `applog.1` must NOT be ignored, which proves the literal `.` in each rotation glob is load-bearing rather than decorative.
   - **`.gitignore` is asserted outside the control write loop** (consensus fix of blocker 12.2: the loop `fs::write`s every control path, and a `.gitignore` control would overwrite the generated file and disarm every later assertion). After the control loop: (a) `git check-ignore --no-index` on `instance/.gitignore` must exit 1 (product decision 5) without any write to that path, and (b) the generated file's bytes must still equal what the ensure produced, which both proves the file survived the fixture's own writes and would have caught this defect class in the first place.
   - Index assertions (finding 12.10): pre-track `instance/app.log` AND `instance/api-message-bus.sqlite3`; after the ensure, `git ls-files --error-unmatch` still finds both. Parent `.gitignore` and `.git/info/exclude` stay byte-identical.
6. New `dir_rows_require_a_real_directory` (consensus resolution of finding 11.9, stronger than the proposed single control): its own small fixture; for every `Dir`-disposition-`Ignore` row derived from the table, assert a plain file bearing the row's name is NOT ignored (exit 1) and a nested file under a real directory of that name IS ignored (exit 0). Derived from the table, so any future `Dir` row is covered automatically and the silent-green trap (a `Dir` row without a nested sample) cannot recur.
7. `partial_file_preserves_prefix_and_appends_only_missing_rules`, `complete_file_is_byte_stable_across_repeated_ensure`, `byte_scan_preserves_invalid_utf8_and_recognizes_crlf_rules`, `escaped_canonical_line_controls_detection_and_repair`, `literal_gitignore_segment_encoding_is_canonical`, `git_fixture_treats_bracketed_agent_name_as_literal`, `instance_gitignore_ignores_injected_messages_artifacts_narrowly`: semantics preserved; expected-content plumbing updated to the comment+pattern renderer (a shared test helper building expected bytes from `RenderedRule`s keeps these terse and is the single derivation point for every count). The safety tests (directory/symlink/read-only/locked/line-break-name) need no content change.
8. `instance_gitignore_covers_every_injected_messages_artifact` stays as-is (it asserts pattern coverage through `required_rules`, which still holds; adjust only the accessor if the return type change requires it).

New test in `config/local_config_io.rs`: `atomic_temp_names_stay_inside_the_ignored_glob` via the registry predicate (Section 4.6).

New tests in `config/instance_artifacts.rs`: the eight registry-internal tests of Section 4.1.

`tests/instance_gitignore_layering.rs`: the Section 4.7 widening plus its own green run.

Gates, from `src-tauri/` unless noted:

```text
cargo test -p agentscommander-new instance_gitignore -- --nocapture
cargo test -p agentscommander-new local_config_io
cargo test --test instance_gitignore_layering
cargo test --workspace
cargo clippy -p agentscommander-new --all-targets -- -D warnings
cargo check --workspace
```

(`-p agentscommander-new` matches `src-tauri/Cargo.toml:2`, verified in Section 11.15.1. Redirect test output to a file when reading `--nocapture` detail on this host.)

**Step-N module-cycle detector criterion** (mandatory because this plan touches module structure). Executable procedure on this workgroup's shared shallow clone (finding 12.11.3: never check out the base in the shared tree; the detector is pure static analysis, so the base extract needs no build and no `target/`):

```text
# pre: extract the base into a scratch dir OUTSIDE the shared clone, e.g. the replica root
git -C repo-AgentsCommander archive b1eefa7c | tar -x -C <scratch>/1446-pre
node "<wg>\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" <scratch>/1446-pre/src-tauri --emit-graph pre.json --quiet
# post: run on the implemented working tree
node "<wg>\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

(Cross-check available for the arc-diff half of the criterion: the committed `src-tauri/module-arcs.txt` at `b1eefa7c` IS the base arc set, 1009 lines, so the diff can also be taken against it without any second checkout; the SCC-shape half still needs `pre.json`. Criterion 4's byte-identity survives `core.autocrlf=true` on this host because `.gitattributes` pins the record to `eol=lf`, per 12.11.)

Green iff all of:

1. `coverage.graphShape.cyclicSccs` is 1 before and after (the knot exists today; detector exit code 1 means "gating cycles exist" and is the NORMAL outcome here; only exit 3 means no graph).
2. Every cyclic SCC member set is identical set-to-set, module by module; `sccSize(config::instance_gitignore) = 1` and `sccSize(config::instance_artifacts) = 1`.
3. The arc-set diff pre vs post consists exclusively of arcs whose target is `config::instance_artifacts`, with sources in: `config::instance_gitignore`, `config::activity_log`, `config::session_context`, `config::coordinator_clocks`, `config::sessions_persistence`, `config::seeded_context_templates`, `config::coding_agents_catalog`, `config::settings`, `config::local_config_io`, `api::message_store`, `api::audit`, `api::auth`, `telegram::output`, `commands::config`, `commands::role_templates`, `web::commands`, `pty::local_backend`, `cli::create_agent_matrix`, `cli::create_agent`, `cli::harness`, `testability::ui_automation`, `config::agent_command`, `config::coding_agent_mutations`, `phone::mailbox`, and the crate root (`lib.rs`). `config::coding_agent_mutations` is the single source round 2 adds (permitted maximum 24, up from 23); the round-2 wiring of `config/session_context.rs` adds no source because that module already points at the registry, and the agency template siblings add none because their writer reaches the registry constant transitively through `commands::role_templates` and this plan does not give it a direct import (Section 4.4). Zero other new or removed arcs; zero arcs cross a previously-clean SCC boundary. Two parentheses so absences are not misread (12.11): `config::local_config_io`'s only new reference is inside `#[cfg(test)]` and the record is generated with `includeTests: false`, so it is expected to contribute NO arc; and the const-alias form is a fully qualified path, which the detector's inline qualified-path discovery DOES record (12.5), so the owner arcs are expected to appear. The whitelist is the permitted maximum; any arc outside it is a gate failure.
4. The regenerated `src-tauri/module-arcs.txt` is committed with the change and byte-identical on re-run (empty `git status` for it afterwards).
5. Structural layering guards stay green, including the updated `instance_gitignore_layering`.

Role/layering hygiene: `instance_artifacts` is pure constants, gains no `tauri`/`AppHandle`/transport dependency, and sits below every consumer; no lower layer gains a UI-transport dependency anywhere in this plan.

## 9. Objective acceptance criteria

1. A fresh normal startup generates the Section 5.1 file, and it is created before `app.log` opens (existing ordering, unchanged).
2. A byte-exact pre-change 14-rule file, ensured once, gains exactly the non-L comment+pattern pairs of Section 4.2 appended once in table order, keeps its original bytes as an exact prefix, and is byte-stable on the second ensure. No migration or user action is involved for writable files; the read-only/locked case fails without modification and is pinned by its own test (Sections 5.10, 8.3).
3. `git check-ignore` (via the fixture) accepts every Section 8.5 required sample and rejects every control; the generated `.gitignore` itself is proven not ignored AND byte-intact after the control writes (blocker 12.2's fix); the dir-semantics fixture (Section 8.6) holds for every `Dir` row; pre-tracked `instance/app.log` and `instance/api-message-bus.sqlite3` remain tracked; parent `.gitignore` and `.git/info/exclude` remain byte-identical.
4. Every emitted rule's pattern derives from `INSTANCE_ARTIFACTS` (plus the two dynamic rules); `FIXED_RULES` no longer exists; the Track set is exactly product decision 6 plus round-2 decisions 13 and 14, and is frozen by a test.
5. Every Section 4.4 writer builds its artifact name from the registry constant (const-alias definitions included); `rg`-level duplication of the Section 4.4 names in path-construction positions outside `instance_artifacts.rs` is limited to test fixtures/expectations and the four writers Section 3 deliberately leaves untouched. Log tags, log/error message strings, and doc comments that mention an artifact name construct no path and stay literals (measured examples at `b1eefa7c`: the `[session-requests]` and `[project-refresh-requests]` log tags in `phone/mailbox.rs`, the `[codex-home]` tag at `config/agent_command.rs:540`, the "Failed to create session-requests dir" error string at `cli/create_agent.rs:135`); the 12 L-row names likewise remain owner-side literals by scope (Section 3).
6. The `local_config_io` tie test proves every `temp_config_path` name shape satisfies the registry's own `matches_atomic_write_tmp_glob` predicate, whose agreement with the unanchored `.*.*.tmp` pattern is itself registry-tested; with that, this change closes the instance-dir half of #1209 (option 1 semantics without widening `temp_config_path` visibility); the AC-root half is #1448.
7. The Step-N detector criterion of Section 8 passes in full, and `module-arcs.txt` is committed and byte-stable.
8. The updated `instance_gitignore_layering` guard passes: `instance_gitignore`'s reference delta is exactly the one `("src/config/instance_gitignore.rs", "instance_artifacts")` row, its `crate::` table is still empty, `instance_artifacts` scans (`WithSubmodules`) with empty `crate::`/self tables, exactly the one `("src/config/instance_artifacts.rs", "*")` super row and a glob count of 1, and `ROOT_AGENT_DIR_NAME` is still defined exactly once in `config/mod.rs`. The updated `claude_watcher_layering` guard also passes: `expected_output_dependencies()` is exactly the four pre-existing rows plus the one `agentscommander_lib::config::instance_artifacts` row, and the analyzed source set is still exactly `{src/telegram/output.rs}` (Section 4.8).
9. All Section 8 cargo gates pass; the final diff touches only: `config/instance_artifacts.rs` (new), `config/mod.rs` (one `mod` line), `config/instance_gitignore.rs`, the Section 4.4 owner files, `config/local_config_io.rs` (test only), `tests/instance_gitignore_layering.rs`, `tests/claude_watcher_layering.rs` (exactly the Section 4.8 one-row widening plus its comment paragraph), `src-tauri/module-arcs.txt`, and this plan file.
10. No `/*`, no generated `!` rule (impossible by construction, backed by the registry charset test), no self-ignore of the generated file, no untracking, exactly one depth-independent rule (pinned by `exactly_one_any_depth_row_exists`), no new dependency, no IPC/frontend change.
11. (Round 2; classification (d) added in round 3.) The Section 7 enumeration recipe is re-run on the final tree and every candidate it yields is classified (a), (b), (c) or (d) per its stop-the-line criterion, with the (b), (c) and (d) classifications recorded (Section 14.2 is the recorded baseline). The registry module doc's claim that every child of the instance config directory has a row is true under the final table, read with the subject Section 7 states for it: the artifacts production code publishes, the recorded (c) and (d) exclusions being decisions and not holes. A candidate that is neither covered nor recorded fails this criterion; it does not get resolved by editing the doc comment, and it does not get resolved by adding a row for bytes no production writer can produce, which is classification (d) and is admissible only against the three-finding evidentiary bar of Section 7. The four rotation rows additionally hold their narrowness: no rotation glob matches at depth (`sub/app.log.1`, `sub/activity.jsonl.1`), none matches without the literal dot (`applog.1`), and none is the rule that matches any byte-order neighbour, verified with `git check-ignore -v` rather than by inspection.

## 10. Certification

**Current status: READY_FOR_IMPLEMENTATION**, certified in recertification round 3 (2026-08-20 UTC); see the round-3 subsection at the end of this section, which is the governing one. The three subsections above it are the dated records of the earlier certifications and are superseded.

Status of round 1: READY_FOR_IMPLEMENTATION (superseded).

Certified by the wg-11 architect on 2026-08-20 UTC, consensus round 1, against planning base `b1eefa7c` (clean tree verified at certification time: `git status --porcelain` empty, HEAD `b1eefa7c`). The round-1 consolidation was written by an architect session that was lost before reporting; this certification was re-issued by the resumed architect session after a full re-audit of the dispatch point by point, source re-verification of every consensus-round claim, and the corrections logged in Section 13's re-verification entry. The Plan-SHA256 of this file's exact bytes is recorded in the certification report to the tech-lead, not here (a file cannot contain its own hash); any byte change after that report invalidates the certification.

**Dependency-cycle gate** (`verify-no-dependency-cycles` skill, applied at plan time via its manual-analysis mode; the Section 8 Step-N criterion is the implementation-side mirror the implementer must run):

- Enumerated arc delta: this plan adds arcs ONLY of the form `X -> config::instance_artifacts`, with X in the Section 8 whitelist (23 recordable sources plus the test-only `local_config_io` reference that records no arc). It removes no arc. No planned reference leaves `instance_artifacts` in production code, and the layering guard plus the arc record (`includeTests: false`) enforce that shape mechanically after implementation.
- Classification: every new arc terminates in a node with zero outgoing arcs, which is a trivial SCC no matter its in-degree; such arcs can neither join, grow, nor create any SCC, and none crosses a previously-clean SCC boundary. This is the same argument the guard file already documents for `crate::config`, whose zero-outgoing premise is measured (zero outgoing arcs in the 1009-arc record at `b1eefa7c`, Section 12.11.1, line count re-verified by the architect).
- Direction check: the inverse design (registry importing owner constants) was rejected in Section 4.1 precisely because it provably creates a two-node cycle at `api::message_store`; dev-rust confirmed the argument (11.13) and grinch confirmed the record mechanics (12.11).
- Baseline, measured by this certification's own detector run on the clean base tree (not inherited from prior sessions): `cyclicSccs = 1` (single knot, 85 members at `b1eefa7c`); `sccSize(instance_gitignore) = 1`; `agentscommander_lib::config` out-degree 0 in the emitted graph; the arc record regenerated from that same graph is byte-identical to the committed `src-tauri/module-arcs.txt` (1009 arcs, SHA-256 equal), proving criterion 4's machinery works on this host. Expected after implementation: `cyclicSccs = 1`, identical knot member set, `sccSize(instance_gitignore) = 1` and `sccSize(instance_artifacts) = 1`; gate green criteria are Section 8's five points. Limitation stated per the gate skill: the post-change run cannot exist before implementation; the per-arc classification above plus the plan's Section 8 Step-N criterion carry that half.
- Role/layering hygiene: `instance_artifacts` is pure constants below every consumer; no lower-layer module gains any `tauri`/`AppHandle`/transport dependency anywhere in this plan.

**Verification performed for this certification** (resumed session, all evidence first-hand at `b1eefa7c`): all four consensus product decisions folded in and checked row by row (Sections 1, 4.2, 4.6); all Section 11 and 12 findings resolved in the body (ledger in Section 13); every Section 4.4 site re-verified by direct search and read, including `api/auth.rs:45,637,678,758`, `phone/mailbox.rs:10807,10831`, `cli/create_agent.rs:133,295`, `cli/harness.rs:475-477`, `config/agent_command.rs:499,2046` (and its non-artifact 540/1335), `config/settings.rs:3345`, and all nine constant declarations with their exact visibilities; the "remaining `api-clients.json` literals are tests, doc comments and one error string" claim confirmed by full-tree sweep; the Section 12.1 enumeration re-run clean (Section 7's checklist item records its one test-only false positive); the fixture control-write loop, `FIXED_RULES` bytes, `TEST_AGENT_LOCAL_DIR`, guard table arities (6-row guarded super table, 1-row host table), `Reach::WithSubmodules` availability, package name `agentscommander-new`, the `.gitattributes` `eol=lf` pin on the arc record, and `logging.rs:481` as the sole production caller all confirmed; the 39-row byte sort re-derived end to end and the canonical counts recomputed (39/5/41/82/27/54 and the 17+7+1+1+1 provenance). Two claims of the lost session were found wrong and corrected: the knot member count (85, not 88) and the "no writer" claim for `settings.pre-384-v1.json` (live write-once writer found; Sections 2.5, 3, 4.4). Residual anchor risk is the normal one: the implementer resolves every file:line against the checkout (Section 4.4's standing rule).

Prior draft status and its two declared debt items were discharged in Sections 11.11 and 11.2 (both measured; the visibility item became the const-alias correction adopted in Section 4.4).

### Recertification after the Step-8 blocker (2026-08-20 UTC)

Status: READY_FOR_IMPLEMENTATION (re-issued). The previously certified bytes (Plan-SHA256 `37A61B657344DCF44E12D2617492A0EC5C77E75959B1DD3F208D35BE67C8509E`) are superseded; the new hash is recorded in the recertification report to the tech-lead.

Trigger: dev-rust's implementation report (messaging `20260820-030500`): wiring `telegram/output.rs:64,141,142` per Section 4.4 turns `tests/claude_watcher_layering.rs :: production_output_module_owns_the_seam_alone` red, and acceptance criterion 9 did not allow touching that file. The plan mandated an outcome while forbidding its only path; dev-rust correctly reverted the three sites, pushed everything else green, and escalated instead of improvising a contract change.

Decision: widen the guard's contract by exactly one argued row (Section 4.8) instead of declaring the writer a fifth deliberately-unwired exception. Rationale: the user's confirmed objective is to close the rename-drift class as far as feasible without breaking guards; a one-row equality-table widening with a documented argument is these guards' designed evolution path, not a break (the table stays equality-pinned and still refuses any sixth dependency), and the argument is genuinely the one already accepted into `instance_gitignore_layering` in this same change: the arc terminates in the zero-out-degree registry leaf. The alternative would leave three covered artifacts (`telegram-bridge.log`, `diag-raw.log`, `diag-sent.log`) rename-fragile when, unlike the four Section 3 exceptions (none of which can be wired meaningfully), this writer is trivially wireable.

Delta over the previously certified bytes, minimal: new Section 4.8; one Section 3 in-scope bullet; a cross-reference in Section 4.4's telegram row; the Section 7 recertification addendum; acceptance criteria 8 and 9 extended. No registry row, no emitted rule, no canonical count, and no Section 8 procedure changed.

Dependency-cycle gate re-applied (`verify-no-dependency-cycles`; manual per-arc analysis over the implemented tree at `0e902d85`, with dev-rust's executed Step-N run as the measured baseline): the residual delta adds exactly one arc, `telegram::output -> config::instance_artifacts` (sites `output.rs:64,141,142`). The target has zero outgoing arcs in the implemented record, so no reverse path can exist: the arc cannot create, grow, or join any SCC and crosses no previously-clean SCC boundary; the source is already in the Section 8 whitelist, so the Step-N criterion is unchanged. Measured baseline (dev-rust's run, machinery validated byte-identical at base): `cyclicSccs = 1` pre and post, knot member set identical (85 members), `sccSize(config::instance_artifacts) = 1` with out-degree 0, 22 new arcs vs `b1eefa7c` all terminating in the registry. Expected after commit 3: the same shape with the 23rd arc, `module-arcs.txt` regenerated and byte-stable, all five Section 8 green criteria re-run. Role/layering hygiene: unchanged; no module gains any UI-transport dependency.

Also ruled within this recertification: `config/agent_command.rs:2046` (plan anchor at `b1eefa7c`) sits inside `#[cfg(test)] mod tests`; dev-rust wired it with a qualified path so the test expectation of `resolve_is_read_only_and_public_build_prepares_isolated_codex_home` (verified at `0e902d85`, `:2037-2077`) derives from the same `CODEX_HOME_DIR_NAME` constant as the production writer `compute_codex_home`. This is inside the plan's contract (Section 4.4 lists the site; criterion 5 bounds where literals MAY remain, it does not mandate them) and contributes no arc (the record is generated with `includeTests: false`). Blessed as implemented.

### Recertification round 2 after the inventory PLAN-DEFECT (2026-08-20 UTC)

Status: READY_FOR_IMPLEMENTATION

Status, in full: **READY_FOR_IMPLEMENTATION** (re-issued). The previously certified bytes (Plan-SHA256 `D57B0B31451222D774A1B59F01E1C75B00F91008EC61DD39D326345E4EE6722D`) are superseded; the new hash is recorded in the round-2 report to the tech-lead, not here. This round ran in two passes: the delta for decisions 11-14 was written first and certification was **withheld** because the new enumeration recipe, run before certifying, surfaced a fifth undecided class; the user ruled it as decision 15 and this certification covers both passes.

Trigger: grinch's implementation review of `b1eefa7c..dcd221fb` (messaging `20260820-051500`) returned PLAN-DEFECT on one finding. Every other point of the implementation passed its line-by-line review, including the table, the render, the emitted rules, the const aliases, both guards, the arc record and all five Section 12 resolutions. The defect is in the inventory this plan certified, not in the work: `src/config/instance_artifacts.rs:25-30` asserts that every child of the instance config directory has a row, and six or seven classes written by production code had none. Grinch also showed why the plan's own Section 7 enumeration could not have caught them: the `-A3` window is blind to joins that sit further from their root binding, to roots obtained through a wrapper, and to joins on a parameter (Section 7 carries the demonstration and its replacement).

Decisions folded in (user, via tech-lead dispatches `20260820-062529` and `20260820-071012`), recorded as product decisions 11-15 in Section 1:

1. `agency-agents_templates.*` is `Ignore`, as a single `Glob` row (new row 6), covering `.lock`, `.next-<uuid>/`, `.download-<uuid>/` and `.prev-<uuid>/`. The glob form is the decision, not a convenience: the literal dot is what keeps the rule off the suffix-less `agency-agents_templates` directory, which stays `Track` under decision 6.
2. `coding-agent-requests/` is `Ignore`, as a `Dir` row (new row 14) whose trailing slash covers the `results/` subtree in one rule.
3. `Context.AgentsCommander.md.retired-*.bak` is `Track`.
4. `Context.AgentsCommander.md` is `Track`.
5. (Decision 15, second dispatch.) Rotated generations are `Ignore` for all four rotating artifacts, which makes `app.log.1` a covered artifact and is the second declared reversal of a #1164 narrowness control. Section 4.6 enumerates both reversals in one place, as the dispatch required.

Rationale for the mechanical choices, which were the architect's to make:

- **Both new `Ignore` rows follow the table's established provenance conventions rather than introducing a fourth.** `agency-agents_templates.*` is a **G** row: a glob constant whose value a registry unit test derives from its base-name constant, exactly as `MESSAGE_BUS_DB_GLOB` derives from `MESSAGE_BUS_DB_FILENAME` and `COORDINATOR_CLOCKS_TMP_GLOB` from `COORDINATOR_CLOCKS_FILE_NAME`. That derivation test is what mechanically ties decision 11 to decision 6: the glob is provably the tracked name plus a dot, so it cannot be widened into the row it must not touch. `coding-agent-requests` is a **C** row against a new registry constant aliased by the writer's existing canonical constant.
- **The wiring is two const-alias edits and zero call-site edits**, which is the smallest change that satisfies acceptance criterion 5 for the new rows. `CODING_AGENT_REQUESTS_DIR` (`config/coding_agent_mutations.rs:35`) is already the single canonical constant that both queue sides import, so aliasing the declaration reaches `cli/coding_agent.rs:424` and `phone/mailbox.rs:10974` at once. `GLOBAL_CONTEXT_TEMPLATE_FILENAME` (`config/session_context.rs:10`) is already what the retirement path joins. The agency siblings need no edit at all: `cli/agency_templates.rs:10-15` already imports `AGENCY_TEMPLATES_DIR` from `commands::role_templates`, whose declaration became a registry alias in commit 2, so all four sibling names already resolve to the registry and a rename already breaks that build. Giving that file a direct registry import would add an arc source and buy nothing.
- **The four rotation rows are one glob per artifact, `<live name>.*`, not a shared form and not a digit class.** A shared form is impossible without a pattern far wider than any of the four artifacts. A digit class (`app.log.[0-9]`) would match today's writers exactly, since every `KEEP` is below ten, and was rejected because it silently stops covering the moment a `KEEP` passes 9, which #1441 makes a live possibility; the plan's house style for exactly this situation is the wide suffix glob anchored on the live name (`api-message-bus.sqlite3*`, `settings.pre-*.json`, `injected-messages.toml.bak-*`). The residual is that `app.log.<anything>` is covered, not only `app.log.<digit>`; that is inside the dispatch's constraint, which was that no new pattern may reach a `Track` row or a byte-order neighbour, and it is proven with `git check-ignore -v` rather than by inspection (Section 14.3).
- **The four rotation rows follow their live rows' provenance, which is why three are `G` and one is `N`.** `activity.jsonl`, `api-audit.log` and `orphaned-sessions.archive.json` are `C` rows whose writers already alias the registry constant, so their rotation globs are `G` rows derivation-tested against those same constants; the tie is real because the writer's own base path is built from the constant the test names. `app.log` is an `L` row whose writer (`logging.rs`) Section 3 deliberately does not retrofit, so there is no constant to derive from and its rotation row is an `N` literal, the same pairing row 44 already has with its `L` live row 43. Inventing an `APP_LOG_FILE_NAME` that no writer imports would have added a third list, which is the failure mode Section 3 rejected option (i) for.
- **`RESULTS_SUBDIR` and the retirement-backup name stay owner-side.** The first names a child of a directory the `Dir` row covers as a subtree, the same stance `harness.log` gets under `logs`. The second is composed from the live entry's runtime `file_name()`, so no constant can produce it; its covering `Track` glob is registry-owned and derivation-tested, and the writer joins the four already-recorded deliberately-unwired writers as the fifth (Sections 3 and 4.4).

Delta over the previously certified bytes: six `Ignore` rows and two `Track` rows in Section 4.2 with the row numbering remapped and the canonical block re-derived (**45/7/52, 47 rules, 94 lines, 33 appended pairs, provenance 17+7+1+1+1+2+4**); seven new registry constants and three new derivation tests in Section 4.1; two const-alias rows and the unwired-writer set grown to nine in Section 4.4; eleven required samples and six controls in Section 8.5, with `app.log.1` moved from `control_paths` to `required_paths`; the widened `track_rows_are_exactly_the_declared_track_set`; one new Step-N whitelist source; the replacement enumeration recipe and its stop-the-line criterion in Section 7; commit 4's scope in Section 7; the two declared #1164 reversals enumerated together in Section 4.6; grinch's finding 2 (the stale `49`) in Section 4.7.5; acceptance criterion 11; and Section 14. No pre-existing row, emitted rule, render rule, guard contract or Section 8 procedure changed.

**Dependency-cycle gate re-applied** (`verify-no-dependency-cycles`, manual per-arc analysis over the implemented tree at `dcd221fb`, with grinch's executed Step-N run as the measured baseline):

- Enumerated arc delta of this round: **exactly one new arc**, `config::coding_agent_mutations -> config::instance_artifacts`, from the single const-alias edit at `config/coding_agent_mutations.rs:35`. No arc is removed. The other three round-2 rows add none: `config::session_context` is already an arc source into the registry (measured, it appears in the 23-arc set at `dcd221fb`), the agency siblings get no direct import, and the `.retired-*.bak` glob is registry-owned with no writer.
- Classification: the target has out-degree **0** in the committed record (`grep -c '^agentscommander_lib::config::instance_artifacts -> ' src-tauri/module-arcs.txt` is 0; grinch re-measured out-degree 0 even counting test code), so it is a trivial SCC regardless of in-degree. An arc terminating in a zero-out-degree node can neither create, grow nor join an SCC, and no reverse path can exist, so it crosses no previously-clean SCC boundary. This is the same argument the plan already carries for all 23 existing arcs and the one the guard file records for `crate::config`.
- Measured baseline (grinch's run at `dcd221fb`, machinery validated byte-identical at base): `cyclicSccs = 1` pre and post; the single cyclic SCC has 85 members with an identical member set both sides (`only pre: []`, `only post: []`); `sccSize(config::instance_gitignore) = 1` and `sccSize(config::instance_artifacts) = 1`; 23 arcs added versus `b1eefa7c`, all terminating in the registry, all sources inside the Section 8 whitelist; the regenerated record is byte-identical to the committed blob. Expected after commit 4: the same shape with the 24th arc, `module-arcs.txt` regenerated and byte-stable, and all five Section 8 green criteria re-run.
- Decision 15 adds **no arc at all**: its four rows are covered by registry-owned globs and none of the four rotation writers is rewired (Sections 3 and 4.4), so the arc delta of the whole round remains the single `config::coding_agent_mutations` arc above.
- Whitelist: grows by exactly one permitted source, `config::coding_agent_mutations` (permitted maximum 24). The whitelist is a maximum, so an arc outside it is still a gate failure.
- Role/layering hygiene: unchanged. `instance_artifacts` stays pure constants below every consumer and gains no `tauri`, `AppHandle` or transport dependency; `config::coding_agent_mutations` gains a dependency on a constants leaf only, no transport, and no lower-layer module gains a UI-transport dependency anywhere in this delta.

**Verification performed for this certification** (all first-hand at `dcd221fb`, working tree clean before and after; the plan is edited but uncommitted by design, and the implementation tree is untouched): the starting digest confirmed equal in the working tree and in the `dcd221fb` blob; every writer site of decisions 11-15 read directly, including the four rotation writers, their `KEEP` constants and their append-mode opens; the Section 7 recipe executed in full rather than merely written, which is what produced decision 15's class and is the reason this plan no longer certifies an inventory nobody took; twenty-three `git check-ignore --no-index` probes run against real git in a throwaway repository, covering every new required sample, every new control, both `Track` sentinels, the disjointness of the rotation globs from row 1, and a `-v` check proving no new glob is the rule that matches any byte-order neighbour; the final table verified by program on the finished file (45 `Ignore` rows, contiguous numbering, strict byte order by real byte comparison, 12 `L` rows, hence the 33 appended pairs the canonical block states, and 7 `Track` rows); every row-number reference in the normative body and in Section 13 re-resolved against the final table; and the layering exposure of the one wiring target checked against both guards (`config::coding_agent_mutations` appears in neither `production_modules()` nor any expected-dependency table of `tests/claude_watcher_layering.rs`, so no guard contract moves).

Residual risk, stated plainly: the completeness claim now rests on an executed four-leg recipe with a recorded classification for every candidate, which is a far stronger basis than the sweep it replaces, but it is still a procedure a person runs, not a test that fails. Section 3 keeps that residual on the record, acceptance criterion 11 binds the next change to re-run it, and Section 14.2 gives that run a decided baseline to start from instead of a blank sheet.

### Recertification round 3 after the ledger PLAN-DEFECT (2026-08-20 UTC)

Status, in full: **READY_FOR_IMPLEMENTATION** (re-issued). This section's canonical `Status: READY_FOR_IMPLEMENTATION` line, in the round-2 subsection above, is left byte-intact and stays the single one in the file by design; it carries this round as well. The previously certified bytes (Plan-SHA256 `52174F209AC96FA325749A020FBA3D7A349B72BE75803466A40B031E652346E5`, the bytes committed at `645b870b`) are superseded; the new hash is recorded in the round-3 report to the tech-lead, not here.

Trigger: grinch's review of commit 4 (`b1eefa7c..645b870b`, messaging `20260820-081136`) passed every technical point of the implementation and returned PLAN-DEFECT on a single point of record. Leg 4 of the Section 7 recipe, run against `.agentscommander_amp-office`, found `settings.json.backup.jo`, a direct child of the instance directory that no registry row matches. dev-rust and grinch established along independent paths that no production writer can produce that name, and both concluded that it must **not** receive a row, because they are the user's own bytes and the correct outcome is that git keeps seeing them. That conclusion is right and is adopted unchanged here. What was missing is the accounting: acceptance criterion 11 says in terms that "a candidate that is neither covered nor recorded fails this criterion", and the stop-the-line criterion offered only (a), (b) and (c). The file is a direct child, is unmatched, and was recorded nowhere in the plan, so it fitted none of the three, and Section 14.2 exists precisely "so the next run starts from a decided baseline rather than re-deriving one". Left as it was, the next leg-4 run halts the line over the same file, or worse, "repairs" the registry by ignoring bytes the user wrote.

Decision, and why it is a fourth classification rather than a ledger line about one file:

- **The gap is structural, not clerical.** (a), (b) and (c) all presuppose a writer: the criterion's own opening sentence says to resolve each candidate "to the concrete name or names its writer can produce". Legs 1 to 3 enumerate writers, so they can only ever yield candidates that have one. Leg 4 reads a directory, so it is the one leg that can yield a name with no writer at all, and the criterion had no verdict for that case. A ledger line naming `settings.json.backup.jo` would have decided this file and left the next `settings.json.old` exactly as unclassified as this one was. So the class is registered where the class is decided: as classification **(d)** in the Section 7 stop-the-line criterion, with the concrete measured instance recorded as the (d) baseline in Section 14.2, which is where a leg-4 runner already looks. Both homes grinch offered are used, for the two different things each is good at.
- **(d) is fenced, because it is the classification a real miss would hide behind.** "I found no writer" is what a missed artifact and a user copy both look like. So (d) is admissible only against three independent findings, all recorded with it: leg 3a run in full with every site accounted for, a zero-hit search of the shipped binary for the name and for its distinctive fragment, and at least one forensic property of the bytes that a runtime sibling cannot have. This is exactly the evidence dev-rust and grinch produced for this file, promoted from a one-off argument into the bar the next candidate has to clear. Below the bar there is no verdict: the candidate is unclassified and the line halts, which keeps (d) from becoming the escape hatch that dissolves criterion 11.
- **Criterion 11 references (d) explicitly**, per the review's condition, and now also names the subject under which the doc's claim is read, so the criterion and the doc sentence stop contradicting each other on paper.

Grinch's point 8.2 is closed in the same pass and in the same place (Section 7, the paragraph after the evidentiary bar). The registry doc's strong sentence has two legitimate counterexamples on real disks, the generated `.gitignore` under (c) and this new class under (d), and the plan now records why neither is repaired by the two moves a future reader would reach for: adding a row (which would self-ignore the generated file, forbidden by criterion 10, or ignore the user's bytes) and softening the sentence to `instance_gitignore.rs`'s "enumerated" wording (which is about emitted coverage and would license the exact omissions round 2 found). Section 4.1's mandate is untouched and still binds through criterion 11, which is the chain a reader follows.

Delta over the previously certified bytes, all of it prose: classification (d), its evidentiary bar and the doc-claim reconciliation in Section 7's stop-the-line block; the round-3 commit addendum in Section 7; acceptance criterion 11 widened to four classifications with the added sentence on rows; Section 14.2's intro qualifier and its one new (d) entry; this subsection and the pointer in Section 10's preamble. **No registry row, no emitted rule, no canonical count, no fixture sample, no control, no Step-N whitelist entry, no guard contract and no Section 8 procedure changed**, and no other section was reopened. Commit 5 touches `plans/1446-instance-gitignore-artifact-registry.md` and nothing else, which is inside acceptance criterion 9's allowed set.

**Dependency-cycle gate re-applied** (`verify-no-dependency-cycles`, manual-analysis mode, over the implemented tree at `645b870b`):

- Enumerated arc delta of this round: **empty**. This round changes no `.rs` file, adds and removes no `use`, no module registration and no const alias, so it adds no module-to-module reference and removes none. There is no arc to classify as internal to a pre-existing SCC or as crossing a previously-clean SCC boundary, because there is no arc.
- Consequently the gate criterion is satisfied by construction: `cyclicSccs` cannot change, no SCC member set can change, the count of cross-boundary arcs is zero, and `src-tauri/module-arcs.txt` stays byte-identical to the blob committed at `645b870b`. The last executed measurement stands as the baseline and remains valid: grinch's Step-N run over commit 4 reported `cyclicSccs = 1` pre and post, an identical 85-member knot on both sides, `sccSize(config::instance_artifacts) = 1` with out-degree 0, every added arc terminating in the registry leaf and every source inside the Section 8 whitelist, and the regenerated record byte-identical to the committed one.
- Role/layering hygiene: unchanged, and unchangeable by a text-only delta. `instance_artifacts` stays pure constants below every consumer, no module gains a `tauri`, `AppHandle` or other UI-transport dependency, and no transport-taking function moves downward.
- Step-N acceptance criterion: unchanged and still binding on the implementation tree (Section 8, acceptance criterion 7). Nothing in this delta touches module structure, so the criterion has nothing new to detect.

**Verification performed for this certification** (first-hand, at `645b870b`, working tree clean at start): the starting digest of this file confirmed equal to the tech-lead's dispatch value and to the `645b870b` blob; grinch's report read in full, and its sections 8.1 and 8.2 line by line; leg 3a re-run independently on the implementation tree, returning the same **12** sites, none composing `.backup`, and `src/path_identity.rs:586` read in context together with the `.{uuid}.terminal-snapshot-private-cleanup` leaf at `:327` and its `#[cfg(unix)]` attribute; `rg 'backup\.jo|settings\.json\.backup' src` confirmed empty; the disputed file itself opened in `.agentscommander_amp-office` and its forensics measured directly (4645 against 4085 bytes, mtimes 17:30:20 against 17:26:04 on 2026-08-13, and a JSON key comparison showing the live file holds six keys the copy lacks and the copy holds none the live file lacks, where grinch named three of the six); its neighbour `settings.json.lock` checked against the table and confirmed as row 39, so no second unclassified child was introduced by looking; and the file re-verified after editing as ASCII with LF endings and no em-dash. Not re-verified and not needed: the counts, samples, rows and gates of commit 4, which this round does not touch and grinch already measured.

Residual risk, stated plainly and unchanged in kind from round 2: completeness still rests on a procedure a person runs, not on a test that fails. What round 3 adds is that the procedure now has a verdict for every candidate it can produce, including the one kind it had no verdict for, and that the one verdict which could be abused carries a bar with three independent findings and a halt below it.

## 11. Dev-rust enrichment

Author: wg-11 dev-rust. Date: 2026-08-19. Base verified: `b1eefa7c` (Codebase Memory gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-11-dev-v5-team-repo-AgentsCommander`, 24145 nodes / 128872 edges; 3 graph operations, one `rg` fallback for constant declarations, plus free reads of the files this change edits).

Verdict (no certification authority): **solid, needs changes**. The architecture is right and the compatibility guarantee is mechanically true. Two items below are correctness defects that would fail at compile or test time as written (11.1 and 11.2), one is a scope-accuracy defect in the #1209 closure claim (11.4), the rest are precision, risk notes and a product question.

### 11.1 CORRECTION: the registry cannot have an empty `super::` table (Sections 4.1 and 4.7.2)

Section 4.1 mandates four `#[cfg(test)]` unit tests inside `instance_artifacts.rs`, and Section 4.7.2 states that "both anchor tables for it are empty". Those two are incompatible under the guard's own rules.

Evidence: `tests/instance_gitignore_layering.rs:565` `const ALLOWED_HOST_SUPER_REFERENCES: [(&str, &str); 1] = [("src/config/mod.rs", "*")];`, whose doc comment says the single row is "the `use super::*;` in that file's own `#[cfg(test)] mod tests`, where `super` is `crate::config` itself". `children_under` reports a glob import as the child `*` (`GLOB_CHILD`, line 643), and `observe` (line 1525) reads whole files including `#[cfg(test)]` regions, which the file's own doc states is deliberate and stricter than the detector.

So a registry with `#[cfg(test)] mod tests { use super::*; ... }` produces exactly one observed `super::` pair, `("src/config/instance_artifacts.rs", "*")`. The correct contract is:

- `ALLOWED_REGISTRY_CRATE_REFERENCES`: empty.
- `ALLOWED_REGISTRY_SELF_REFERENCES`: empty.
- `ALLOWED_REGISTRY_SUPER_REFERENCES`: exactly one row, `("src/config/instance_artifacts.rs", "*")`, mirroring the host precedent, with a doc comment saying the row is the test module's own glob and nothing else may appear.

Section 4.1's phrasing ("no `super::...`, including in its tests") must be qualified the same way: the leaf property that the acyclicity argument needs is **zero outgoing arcs in the module-arc record**, and `use super::*;` inside a `#[cfg(test)]` module contributes none (the record is emitted with `includeTests: false`, per the guard's own doc at lines 1613-1623, and `super` there resolves to the module itself). The guard measures spelling, not arcs; the two contracts differ here and the plan currently states the stricter one by mistake.

Alternative if the architect prefers a literally empty table: move the four registry-internal tests into `instance_gitignore.rs`'s test module (they would reach the registry as `super::super::instance_artifacts::...`, whose leading segment is reported as `super`, an already-allowed row). That keeps `instance_artifacts.rs` free of any `mod tests` at all. My preference is the first option: tests belong next to the data they assert, and the host precedent already exists in the file.

### 11.2 CORRECTION: the "re-export alias" of Section 4.4 does not compile for the four `pub` constants

Section 4.4 says each existing constant "becomes a re-export alias ... preserve each declaration's current visibility". Measured visibilities at `b1eefa7c`:

| declaration | visibility |
|---|---|
| `commands/role_templates.rs:47` `AGENCY_TEMPLATES_DIR` | `pub` |
| `config/seeded_context_templates.rs:7` `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME` | `pub` |
| `api/message_store.rs:15` `DB_FILENAME` | `pub` |
| `config/session_context.rs:11` `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME` | `pub` |
| `config/sessions_persistence.rs:56` `ORPHAN_ARCHIVE_FILENAME` | `pub(crate)` |
| `config/coding_agents_catalog.rs:47` `CATALOG_DIR_NAME` | private |
| `config/activity_log.rs:119` `FILE_NAME` | private |

Section 4.1 declares the registry's contents "all `pub(crate)`". A `pub use` of a `pub(crate)` item is a re-export whose visibility exceeds the item's, which rustc rejects (E0364 family). So four of the seven rows cannot be written as `pub use ... as X;` against a `pub(crate)` registry. Downgrading those four to `pub(crate)` is not an option either: `src-tauri/tests/*.rs` are separate crates linking the lib, so they can only see `pub` items, and any integration test importing one of them would stop compiling.

Fix, and it is simpler than the re-export: keep a **const alias definition** in the owner module instead of an import.

```rust
// api/message_store.rs
pub const DB_FILENAME: &str = crate::config::instance_artifacts::MESSAGE_BUS_DB_FILENAME;
```

This preserves the identifier, its type and its exact visibility, is legal for any registry visibility at or above `pub(crate)`, still breaks the build on a registry rename (satisfying acceptance criterion 5), and sidesteps the re-export visibility question entirely. Use `use ... as ...;` only for the two private rows (`FILE_NAME`, `CATALOG_DIR_NAME`) if the implementer prefers, though the const-alias form works uniformly and is worth using for all seven for consistency.

### 11.3 PRECISION: the rule count is 19 plus 1, and the split should be stated

Tech-lead asked for "ni una más ni una menos" against #1446. Verified pattern by pattern.

#1446's confirmed drift list expands to **19** patterns (17 artifact classes, with the SQLite row contributing three): `activity.jsonl`, `context-cache/`, `coordinator_clocks.json`, `pty-input-locks/`, `api-audit.log`, `api-message-bus.sqlite3`, `api-message-bus.sqlite3-shm`, `api-message-bus.sqlite3-wal`, `telegram-bridge.log`, `diag-raw.log`, `diag-sent.log`, `debug-logs.txt`, `orphaned-sessions.archive.json`, `settings.json.lock`, `settings.pre-*.json`, `instances/`, `git-guard/`, `project-refresh-requests/`, `.agentscommander-context-templates.json`.

Section 4.2's new rows are rows 1, 2, 4, 5, 6, 7, 8, 11, 12, 14, 15, 16, 17, 21, 23, 24, 25, 28, 29, 30 = **20**. The 20th is row 1, `.*.*.tmp`, which is **not** in #1446's confirmed list. It is authorized by #1446's explicit delegation: "the implementation plan decides whether `write_file_atomic` temporaries are representable in the registry (closing #1209) or #1209 stays open for them".

The set therefore matches exactly, but the plan reads as if all 20 came from #1446 (Section 1: "every runtime artifact class inventoried in #1446"; Section 3: "The 20 new ignore patterns"). Restate as **19 from #1446's confirmed set, plus 1 exercised under #1446's #1209 delegation**, so the certification pass can check the equality without re-deriving it.

Legacy byte-identity: verified. `FIXED_RULES` at `b1eefa7c` (`instance_gitignore.rs:5-17`) is unchanged from the investigation snapshot. All 12 `L` rows are `File` or `Glob`, so each renders `/{name}` with no trailing slash, byte-identical to today's literal, and their relative order in the byte-sorted table (rows 3, 9, 10, 13, 18, 19, 20, 22, 26, 27, 31, 32) is the same order as the current array. No `L` row is a `Dir`, so the trailing-slash rendering introduced by this change touches no pre-existing rule. The "existing complete file is byte-stable except for the appended block" guarantee holds.

Byte-sort of the `Ignore` table also verified end to end (the non-obvious adjacencies check out: `.*.*.tmp` before `.agentscommander-...` because `*` is 0x2A and `a` is 0x61; `app-outbox-path.txt` before `app.log` because `-` is 0x2D and `.` is 0x2E; `injected-messages.toml` before `injected-messages.toml.bak-*` before `instances`).

### 11.4 GAP: closing #1209 covers only half of what #1209 describes

#1209 says, as one of its three reasons for excluding the temporaries: "They are not specific to any one feature. `write_file_atomic` has other callers and none of their temporaries are covered either."

Measured caller set of `write_file_atomic` at `b1eefa7c`: the config-dir side is entirely `config/injected_messages.rs` (`publish`, `publish_edits`, `seed_new_file`, `take_backup`, `write_if_different`). Every other caller (`write_team_config_json_atomic`, `create_new_team_config_on_disk_guarded`, `create_workgroup_on_disk`, `create_team`, `persist_agent_delete_metadata_blocking`, `restore_journaled_team_configs`, `publish_missing_default_skill_file_with`, `retire_live_standalone_global`) writes under the **project AC root** (`.ac/`), not under `config_dir()`. Those temporaries are governed by `ensure_ac_root_gitignore` (`commands/ac_discovery.rs:1496-1582`), which this plan explicitly leaves untouched (Section 3, out of scope).

So the plan closes #1209 for the instance dir while leaving uncovered exactly the "other callers" #1209 names. Two honest options, architect's call:

- **(i)** Add the same glob as one more `required_entries` row in `ensure_ac_root_gitignore`. Cost is genuinely small: that generator already takes `(pattern, comment)` pairs and appends missing ones, so it is one array row plus its fixture coverage. Then "closes #1209" is true.
- **(ii)** Keep the scope as planned and change Section 4.6 and Section 9.6 to say the change closes the instance-dir half of #1209 and that #1209 stays open (or spawns a follow-up) for the AC-root callers.

I lean to (ii) plus a follow-up issue, to keep this change's blast radius where it is, but (i) is defensible and cheap. What is not defensible is the current wording, which claims a closure the diff does not deliver.

### 11.5 RISK: `/.*.*.tmp` is root-anchored and today that is only accidentally sufficient

The rule is anchored, so it matches only at the instance-dir root. That is sufficient at `b1eefa7c` because every config-dir-side `write_file_atomic` caller is in `config/injected_messages.rs` and every path it publishes is a direct child of `config_dir()`. It is not a property anyone maintains: the first `write_file_atomic` on a config-dir subpath (`coding-agents/agents.json` and `ac-root-agent/config.json` are the obvious candidates) silently produces an uncovered temporary, which is the exact failure mode this plan exists to end.

Cheapest durable fix: emit the temporaries rule unanchored (`.*.*.tmp`, no leading slash), which git applies at every depth under the instance dir. This costs the "every pattern is root-anchored" assertion in test 1 of Section 8, which would become "every pattern is root-anchored except the atomic-temporaries glob, which is depth-independent by design". Given that the glob is deliberately narrow (leading dot, at least two interior dots, `.tmp` suffix), the widening is safe and the controls `foo.tmp` and `.foo.tmp` still hold at every depth.

If the architect prefers to keep the anchor, then Section 4.6's writer-side test must also assert that the published path's parent is `config_dir()` itself, otherwise nothing detects the drift.

### 11.6 WEAKNESS: the atomic-temporaries tie restates the glob instead of using it

Section 4.6's `atomic_temp_names_stay_inside_the_ignored_glob` imports `ATOMIC_WRITE_TMP_GLOB` and then asserts "starts with `.`, ends with `.tmp`, and contains at least two interior dots before the suffix". That is a hand-written paraphrase of the glob, not a use of it: change the glob and the paraphrase does not follow, so the tie the test is named for is not actually tied.

Proposal: put the predicate in the registry next to the pattern, as a pure function with no external references (so the leaf property is untouched):

```rust
pub(crate) const ATOMIC_WRITE_TMP_GLOB: &str = ".*.*.tmp";
pub(crate) fn matches_atomic_write_tmp_glob(file_name: &str) -> bool { /* pure string logic */ }
```

Then the `local_config_io` test asserts `matches_atomic_write_tmp_glob(produced_name)`, a registry unit test asserts the function agrees with the pattern on a small table of positives and negatives (`.settings.json.1.tmp`, `.a.b.tmp` positive; `foo.tmp`, `.foo.tmp`, `.a.tmpx` negative), and the git fixture remains the ground truth that git itself agrees. One predicate, three users, no paraphrase.

### 11.7 IMPROVEMENT: scan the registry with `Reach::WithSubmodules`, not shallow

Section 4.7.2 proposes scanning `config::instance_artifacts` "shallow like the `crate::config` scan". The shallow reach exists for `config` because, per the guard's own comment on `Reach`, "every child of `config` is a separate module in the graph with its own arcs and most of them are knot members". The registry has no children and is supposed to never have any, so `WithSubmodules` costs nothing today and additionally refuses a future child that parks a reference. `observe(&REGISTRY_MODULE, Reach::WithSubmodules)` is the stronger contract for the same line count.

### 11.8 PRECISION: the widening in `ALLOWED_GUARDED_SUPER_REFERENCES` is exactly one row

Section 4.7.1 says "by exactly the `super::instance_artifacts` family", which is right but not checkable as written. Precisely: `children_under` reports the leading segment, so the production spelling `super::instance_artifacts::INSTANCE_ARTIFACTS` in `required_rules` yields the single pair `("src/config/instance_gitignore.rs", "instance_artifacts")`, and the array grows from `[(&str, &str); 6]` to `[(&str, &str); 7]`. Test-module spellings (`super::super::instance_artifacts::...`) report the leading segment `super`, which is already row 6 of the table, so they add nothing. The certification pass can check the count.

The new row also needs a doc-comment paragraph in the table's prose, on the model of the existing `injected_messages` paragraph, stating why this one is allowed in **production** code: the target is a leaf with zero outgoing arcs, so unlike `injected_messages` it does not depend on being `#[cfg(test)]`.

### 11.9 NEW RISK CLASS: `Dir` rows introduce trailing-slash rules for the first time

All 12 pre-existing rules are file patterns. Rows 11, 17, 21, 24, 25 (`context-cache`, `git-guard`, `instances`, `project-refresh-requests`, `pty-input-locks`) render with a trailing slash, which git only matches against something it considers a directory. Under `git check-ignore --no-index` that decision comes from the filesystem, so the fixture must materialize a real directory for every `Dir` row, not just a path string. The existing fixture loop already does `create_dir_all(path.parent())` before writing each sample, and Section 8.5 gives every `Dir` row a sample with a file inside it (`context-cache/ac-context-1.md`, `git-guard/git.cmd`, `instances/0f0e/instance.json`, `project-refresh-requests/req-1.json`, `pty-input-locks/operation-1.lock`), so the plan is already correct here. Recording it because it is a silent-green trap: a `Dir` row added later without a nested sample would produce a passing test that proves nothing.

Add one control to Section 8.5 to prove the trailing slash is load-bearing: a plain file named `instances` (no children) at the instance root must **not** be ignored. That is the whole point of `ArtifactKind::Dir` and nothing currently tests it.

### 11.10 PRODUCT QUESTION: the SQLite sidecar set is enumerated, not globbed

Rows 6, 7, 8 name `api-message-bus.sqlite3`, `-shm` and `-wal`. Those are the WAL-mode sidecars. If the store ever runs in rollback-journal mode (a `PRAGMA journal_mode` change, or a filesystem where WAL is unavailable), the sidecar is `api-message-bus.sqlite3-journal`, and it is uncovered. `api-message-bus.sqlite3*` would be one rule instead of three and would cover every sidecar SQLite can produce.

I am **not** proposing the change unilaterally, because #1446's confirmed set enumerates the three and the tech-lead's criterion is "ni una más ni una menos". Raising it as a decision: accept the residual, or ask the user to extend the confirmed set to the glob. If the enumeration stays, Section 4.2's comments for rows 7 and 8 should say the coverage is WAL-mode-specific, so the next reader knows the gap is known.

### 11.11 VERIFIED: the Section 10 debt is discharged

Both items the architect flagged as verification debt are now measured at `b1eefa7c`.

**`settings.json.lock` construction.** `config/settings.rs:3345`: `let lock_path = parent.join("settings.json.lock");`, inside the settings save path, immediately after the `SettingsSaveLegacyOutward::SettingsLockUnavailable` mapping and immediately before `let mut options = std::fs::OpenOptions::new();`. It is a single inline literal at a single site, joined onto `parent` (the settings file's own parent directory, not `config_dir()` directly). The Section 4.4 mandate is therefore a one-token swap to `parent.join(SETTINGS_LOCK_FILE_NAME)`; no shape-independence is needed. Note for the registry comment: because the join is on the settings file's parent, the lock lands next to whatever settings path is in use, which is `config_dir()` in production and a temp dir under test. That does not affect the rule, which is scoped to the instance dir by construction.

**Visibilities of the seven constants.** Measured, listed in the table of Section 11.2 above. Four `pub`, one `pub(crate)`, two private. This is what makes Section 11.2 a correctness item rather than a style note.

### 11.12 CONFIRMED: the compatibility test and the reconciliation semantics

Tech-lead's point 4, first half. `legacy_fourteen_rule_file_gains_exactly_the_new_entries` is feasible exactly as specified, and the mechanism is already in place:

- `missing_rule_indexes` (`instance_gitignore.rs:216-222`) delegates to `contains_exact_line` (`:224-233`), which splits on `\n`, strips at most one trailing `\r`, and compares the whole logical line to the rule bytes. Comments can never equal a pattern, so they are transparent to detection, exactly as Section 4.3.3 claims.
- `append_buffer` (`:235-250`) walks `missing` in ascending index order and emits each rule followed by `\n`, prefixing a single `\n` only when the existing content is non-empty and lacks a trailing newline. Table order in, table order out.
- `fresh_file_bytes` (`:206-214`) emits rule then `\n` in array order, which is why today's real file is the two dynamic rules followed by the 12 fixed ones. Section 5.1's 68-line fresh file (34 rules, comment plus pattern each) follows directly.

One signature detail Section 4.3.3 should add: `contains_exact_line(bytes: &[u8], rule: &[u8])` keeps its signature unchanged and is simply called with `rule.pattern.as_bytes()`. Only the five functions the plan lists take the `&[RenderedRule]` change.

One edge case worth one sentence in Section 5.4: if a user deletes a generated pattern line but leaves its comment line, reconciliation re-appends the full `{comment}\n{pattern}\n` pair at the end, leaving the orphaned comment in place mid-file. Harmless and self-inflicted, but it is the one way the generated file can end up looking untidy, and the plan should say so rather than have it discovered as a bug.

### 11.13 CONFIRMED: the acyclicity argument holds, and why

Tech-lead's point 2. The argument is sound and it is the same one the guard file already documents for `crate::config` (`tests/instance_gitignore_layering.rs:555-561`: "a module with no way out cannot reach a knot member, so it cannot share an SCC with one, so nothing that depends only on it can either").

Applied here: `config::instance_artifacts` has out-degree zero in the arc record, so it is a trivial SCC no matter how many knot members point at it, and no arc `X -> instance_artifacts` can close a cycle. The direction the plan chose is the one that works. The inverse (constants staying owner-side, registry importing them) would need `config::instance_artifacts -> api::message_store` for `DB_FILENAME` while `api::message_store -> config::instance_artifacts` is already required for `pty-input-locks`, a guaranteed two-node cycle. Section 4.1's justification is correct as written.

Two mechanical prerequisites the plan should name so the certification pass can check them:

1. `observe` (`:1525-1537`) panics rather than skips when the module cannot be resolved from the module tree. The new scanned unit therefore depends on `pub(crate) mod instance_artifacts;` actually landing in `config/mod.rs`; a forgotten `mod` line fails loudly, which is the desired behavior but should be expected rather than debugged.
2. `the_root_agent_dir_name_constant_is_defined_exactly_once` (`:1965`) scans `every_file_under(src)`, so `instance_artifacts.rs` is inside its scan. The registry must not declare a `const` or `static` named `ROOT_AGENT_DIR_NAME`; the `ac-root-agent` Track row stays a string literal, which `scrub` removes before matching. Section 4.5 already says this; it is worth repeating in Section 7's implementation order as a checklist item, because it is the single edit that would turn a green guard red for a non-obvious reason.

### 11.14 RISK NOTE: blast radius of Section 4.4, and a commit shape that contains it

Section 4.4 edits 18 files, including `lib.rs` and `phone/mailbox.rs`, and Section 8's Step-N whitelist permits 19 arc sources. That is a large surface for a change whose user-visible value is entirely in Section 4.2's table, and a single unrelated breakage in one of those files is hard to attribute once the arc record is regenerated on top.

I am not proposing to cut the scope: #1446 asks for the registry explicitly and the user confirmed it ships in this change. I am proposing the branch carry at least two commits, in the order Section 7 already implies:

1. registry module, `instance_gitignore` rewiring, rule table, its tests and fixture, layering guard update, arc record regeneration. This commit is self-contained and delivers the entire user-visible fix.
2. owner-module rewiring (Section 4.4) plus the `local_config_io` tie test, with its own arc-record regeneration if the first commit's record already covers the new arcs.

If the second commit has to be reverted for any reason, the first still ships the fix. If they land as one commit, a bisect cannot separate "the rules changed" from "eighteen modules changed".

### 11.15 Minor notes

1. Section 8's gates name `-p agentscommander-new`, which matches `src-tauri/Cargo.toml:2`. No adjustment needed; the plan's hedging parenthesis can be dropped.
2. `ArtifactKind::File` and `ArtifactKind::Glob` render identically (`/{name}`). Render them through a single `match` arm (`File | Glob =>`) rather than two identical arms, so `clippy::match_same_arms` stays quiet if the lint level is ever raised. The distinction is still worth keeping: it lets a registry unit test assert that no `File` row contains a git wildcard character, which is the invariant that stops someone adding a glob without thinking about it.
3. Section 4.1's four registry unit tests should gain a fifth: every `Ignore` row's name is non-empty and does not start with `/` or `!`, which is what keeps `render` the only place a leading slash is added and refuses a generated negation by construction (Section 10 acceptance criterion 10 currently has no test behind it).
4. Section 5.1 says 68 lines: 34 rules times two lines each. Correct, and worth keeping as a literal assertion in test 1 since it catches a dropped or duplicated comment.

## 12. Grinch enrichment

Author: wg-11 dev-rust-grinch. Date: 2026-08-19. Base read: `b1eefa7c`, working tree of `repo-AgentsCommander`. Everything below is measured, not inferred: git ignore semantics were probed against real `git check-ignore --no-index` runs in a throwaway repo, and the alias forms of 11.2 were compiled with `rustc --edition 2021 --crate-type lib`. Product decisions 1 (SQLite glob) and 2 (#1209 scope option ii) are taken as given and are not reopened; 12.7 and 12.8 only state their mechanical consequences.

Verdict (no certification authority): **needs changes**. Three blockers (12.1, 12.2, 12.3), each of which makes the plan or its tests claim more than the diff would deliver. Everything else is precision, residual risk, or confirmation of Section 11.

### 12.1 BLOCKER: the registry is not the complete inventory it claims to be, and nothing detects that

Section 1 says the change covers "every runtime artifact class inventoried in #1446"; Section 2's gap statement and Section 4.1's "single source" framing promise that after this change the registry *is* the instance-dir inventory. It is not. Enumerating what production code actually joins onto `config_dir()`:

```
grep -rn 'config_dir()' src-tauri/src --include=*.rs -A3 | grep -oE '\.join\(([^)]*)\)' | sort | uniq -c | sort -rn
```

produces these instance-root artifacts that appear in **neither** the 32-row `Ignore` table nor the 5-row `Track` table:

| artifact | evidence | note |
|---|---|---|
| `api-clients.json` | `api/auth.rs:45` `REGISTRY_FILENAME`; joined on `config_dir()` at `api/auth.rs:296`, `cli/api_client.rs:89`, `commands/config.rs:1801`, `pty/container_tokens.rs:45` | the local API client registry, a direct sibling of `master-token.txt` and `web-token.txt`, both of which this policy has ignored since #1164 |
| `.api-clients-<uuid>.tmp` | `api/auth.rs:758` `parent.join(format!(".api-clients-{}.tmp", uuid::Uuid::new_v4()))` | its own atomic temp, and **not** matched by `.*.*.tmp` (only one dot follows the leading dot); probed: `check-ignore` returns 1 |
| `logs/` (holds `harness.log`) | `cli/harness.rs:475` `config_dir.join("logs")`, then `.join("harness.log")` | a second log directory in the instance root |
| `session-requests/` | `cli/create_agent.rs:130-133` (`config::config_dir()` bound one line above), also `:295`; writes `{id}.json` and `{id}.json.tmp` (`cli/create_agent_matrix.rs:216` shape) | a request queue, exactly the class rows 24/25 cover for their peers |
| `ui-automation/` (holds `session.json`) | `testability/ui_automation.rs:17,18,1384` `config_dir.join(UI_AUTOMATION_DIR).join(SESSION_FILE)` | |
| `codex-home/<agent-id>/` | `config/agent_command.rs:494-499`, `:2046` | a whole per-agent coding-agent home tree |
| `coordinator_clocks.json.<pid>.<seq>.tmp` | `config/coordinator_clocks.rs:361` `path.with_extension(format!("json.{}.{}.tmp", pid, seq))` | see 12.3; `with_extension` **replaces** `json`, so the name has no leading dot and `.*.*.tmp` cannot match it |

That is seven classes, at least one of which (`api-clients.json`) is in the same credential-adjacent family the policy already treats as must-ignore. Whether each should be `Ignore` or `Track` is a product call I am not making; the defect is that the plan asserts an inventory it did not take, and ships a registry whose row set is smaller than the artifact set it claims to enumerate.

Why this is a blocker and not a note: the entire justification for the registry (Section 2, "nothing ties *a module writes X into `config_dir()`* to *X is covered by the policy*", "the list went stale twice") is the drift class. A registry that must be hand-edited when a new artifact appears, with no test that fails when it is not, has the **same** failure mode as `FIXED_RULES`; it only relocates the list. Section 4.1's unit tests check sortedness, uniqueness, comment shape and name charset. None of them can notice a missing row. Acceptance criterion 4 ("every emitted rule derives from `INSTANCE_ARTIFACTS`") is satisfied by an incomplete table.

Required plan changes:

1. Re-run the enumeration above at implementation time and give **every** `config_dir()` child a row, `Ignore` or `Track`, with its comment. If the user has not ruled on one of the seven, that is a question for the coordinator, not a silent omission.
2. Add a decision record in Section 3 for anything deliberately left out of the table, so "not in the registry" never again means "nobody looked".
3. See 12.6 for the only mechanism in this repo that has ever actually closed this class.

### 12.2 BLOCKER: Section 8.5's new `.gitignore` control silently disarms the fixture

Section 8.5 adds `.gitignore` to `control_paths`. Read the loop it lands in (`instance_gitignore.rs:747-751`):

```rust
for relative in control_paths {
    let path = config_dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("control parent")) ...;
    std::fs::write(path, b"control").expect("write control fixture");   // <-- overwrites
    ... check-ignore, assert exit code 1 ...
}
```

The loop **writes `b"control"` into every control path before checking it**. With `.gitignore` in the list, the fixture overwrites `instance/.gitignore`, the file `ensure_instance_gitignore_at` just generated, with the five bytes `control`. Every control evaluated after that point is checked against an empty ruleset, so each one trivially returns exit 1 and the assertion passes for the wrong reason. Depending on array order that is up to seven of the eight new controls, including `foo.tmp` and `.foo.tmp`, the two that guard the `.*.*.tmp` narrowness, plus every Track sentinel of product decision 6. The test stays green and proves nothing. Nothing else catches it: the closing assertions re-read the **parent** `.gitignore` and `.git/info/exclude`, never `instance/.gitignore`.

Fix: assert product decision 5 outside the write loop. After the control loop, without touching the file, `check-ignore --no-index --quiet -- instance/.gitignore` must return exit 1, and additionally the generated bytes must equal what the ensure produced (that second assertion is the one that would have caught this class in the first place). Probed on a real repo: an unmodified generated file is correctly **not** ignored, so the assertion itself is sound; only the write is destructive.

While there: the fixture's controls are written but never removed, and `ensure_instance_gitignore_at` is not re-run afterwards, so the fixture never proves the generated file survives the control writes at all.

### 12.3 BLOCKER: `.*.*.tmp` misses temporaries that exist today, in both directions

Section 11.5 raised the anchor as a future risk ("it is not a property anyone maintains"). It is not a future risk. Measured against the current tree, the rule as specified fails on live writers in two independent ways.

**Direction A, depth.** Root-anchored `/.*.*.tmp` matches only instance-root names. These writers already publish `.{name}.{pid}.{n}.tmp` **inside** `config_dir()` subdirectories:

- `config/coding_agents_catalog.rs:446` `.{CATALOG_MANIFEST_FILENAME}.{pid}.{counter}.tmp`, inside `coding-agents/`, which is a **`Track` row**. Its temporaries therefore land inside a directory the plan deliberately keeps tracked, so they surface in `git status` on every catalog write.
- `config/root_agent.rs:1682,1735` `.{file_name}.{pid}.{n}.tmp`, inside `ac-root-agent/`, also a `Track` row.
- `config/seeded_context_templates.rs:706`, same shape.

Probed: with `/.*.*.tmp`, `coding-agents/.agents.json.4242.0.tmp` returns exit 1 (not ignored). With the unanchored `.*.*.tmp`, it returns 0, and `ac-root-agent/.CLAUDE.md.42.1.tmp` returns 0, while `.foo.tmp` and `foo.tmp` still return 1 at every depth. Section 11.5's unanchored option is therefore correct and should be adopted, and Section 8's "every pattern is root-anchored" assertion becomes "every pattern is root-anchored except `.*.*.tmp`, which is depth-independent by design". The alternative Section 11.5 offers (keep the anchor, assert the parent is `config_dir()`) is not available: the assertion would fail on the three writers above, which are correct code.

**Direction B, shape.** The glob assumes every temporary is `.{something}.{something}.tmp`. Two live writers in the instance dir do not use that shape and are missed at any anchor:

- `config/coordinator_clocks.rs:361`: `path.with_extension(format!("json.{pid}.{seq}.tmp"))` on `config_dir()/coordinator_clocks.json` yields `coordinator_clocks.json.<pid>.<seq>.tmp`, **no leading dot**. Probed: not ignored, either anchored or unanchored. This is the temporary of an artifact the plan explicitly covers (row 12), left behind when the write succeeds but `rename_with_retry` fails, which is precisely the Windows AV/indexer case the comment at `:357` exists for.
- `api/auth.rs:758`: `.api-clients-<uuid>.tmp`, one dot after the leading dot. Probed: not ignored.
- `update_check.rs:76`: `path.with_extension("json.tmp")` on `config_dir()/update-check.json` yields **`update-check.json.tmp`**, and the code's own comment says so. This one is not merely uncovered: `update-check.json.tmp` is item 2 of the fixture's `control_paths` (`instance_gitignore.rs:749`), where it asserts that a **real runtime temporary must stay un-ignored**. Section 8.5 keeps it there ("the other nine remain valid narrowness controls"). So the plan ships a test that pins the opposite of what #1446 asks for, and it does it for the one temporary whose writer names it in a comment. Whoever wrote that control in #1164 took the name for a hypothetical near-miss of `/update-check.json`; it is a live artifact.

So Section 4.6's mandate that the module doc comment "may again claim completeness for the runtime-file policy, including atomic-write temporaries" is false as written, and acceptance criterion 6's "every `temp_config_path` name shape falls inside the rule" is true but narrow: `temp_config_path` is one of at least seven temp-name schemes writing into the instance dir.

Required plan changes: adopt the unanchored glob; decide the three off-shape schemes explicitly (one row each, `/coordinator_clocks.json.*`, `/.api-clients-*`, `/update-check.json.tmp`, or normalise those writers onto `temp_config_path`); **move `update-check.json.tmp` out of `control_paths` into `required_paths`**, which is a product-visible reversal of a #1164 decision and should be stated as such rather than slipped in; and downgrade Section 4.6's doc-comment mandate from "complete" to what the table actually enumerates. Then Section 4.6's writer-side tie is worth having, because it will be tying something true.

A cheaper alternative worth weighing: a single unanchored rule `*.tmp` plus explicit negations would cover all four schemes at once, but the plan forbids generated `!` rules (Section 3, acceptance criterion 10) and `*.tmp` would swallow user files, so I am not proposing it. The per-scheme rows are the option that keeps the narrowness controls meaningful.

### 12.4 CONFIRMED with a caveat: 11.1's guard-table correction

11.1 is right, and the mechanism is where it says. `ALLOWED_HOST_SUPER_REFERENCES` is exactly `[("src/config/mod.rs", "*")]` at `tests/instance_gitignore_layering.rs:565`, documented as the file's own `#[cfg(test)] mod tests` glob; `GLOB_CHILD` is `"*"` at `:643`; `observe` (`:1525`) reads every file of the module including `#[cfg(test)]` regions. A registry with `#[cfg(test)] mod tests { use super::*; }` therefore produces `("src/config/instance_artifacts.rs", "*")` and an empty `super::` table would fail. The detector's `includeTests` default is `false` (`01-rust_module-dependency-cycles.mjs:173`), so that same glob contributes no arc, which is why the spelling contract and the arc contract legitimately differ here.

Caveat 11.1 does not cover: `Observation` carries a `globs` **count** (`:1507-1509`, `:630-643`), and the file's own doc explains the count exists to be exactly 1 and was hardened after `use super::super::*;` and `use super::{*};` were measured slipping past a text-match version. The registry's new scanned unit needs its glob count pinned the same way, or a second test module (or a nested one) reopens exactly the hole entry 14/15 closed. State the expected count in Section 4.7.2 alongside the table rows.

Preference: 11.1's first option (tests stay in the registry). 11.1's alternative would move four data assertions into the guarded module and buy nothing, since `super::super::instance_artifacts::...` reports leading segment `super`, already row 6.

### 12.5 CONFIRMED with compiler evidence: 11.2's alias defect and its fix

Compiled both forms. The plan's Section 4.4 form fails exactly as 11.2 predicts:

```
error[E0364]: `MESSAGE_BUS_DB_FILENAME` is only public within the crate, and cannot be re-exported outside
warning: unused import: `crate::registry::MESSAGE_BUS_DB_FILENAME as DB_FILENAME`
```

Note the second line: the same statement also produces an `unused_imports` warning, so under Section 8's `cargo clippy --all-targets -- -D warnings` the re-export form fails twice. 11.2's const-alias form (`pub const DB_FILENAME: &str = crate::config::instance_artifacts::MESSAGE_BUS_DB_FILENAME;`) type-checks clean against a `pub(crate)` registry, as does the private `use ... as ...;` form for the two private constants. Adopt 11.2 as written, uniformly.

One consequence 11.2 does not draw, relevant to the Step-N gate: the detector has inline qualified-path discovery (`01-rust_module-dependency-cycles.mjs:1432-1448`, `extractQualifiedPaths` at `:2132`), so a const alias spelled as a fully qualified path **does** record an arc even with no `use`. The 19-source whitelist is therefore meaningful rather than vacuous, and Section 8's hedge ("the detector may record fewer sources") applies to spelling variants, not to this form.

### 12.6 The plan closes half the drift class, and should say which half

Section 2 frames the problem as "the list went stale twice". The registry closes **rename drift**: a constant renamed in the registry breaks every owner's build. It does not close **appearance drift**: a new artifact written into `config_dir()` by a module that never touches the registry produces no compile error, no failing test, and no rule. 12.1 is not a one-off oversight, it is that hole, already occupied by seven inhabitants.

The repo already contains the only mechanism that has ever closed this class: `instance_gitignore_covers_every_injected_messages_artifact` (`instance_gitignore.rs:1003-1021`, from #1157) imports the owner's filename constants under `cfg(test)` and asserts `required_rules` covers each one. Generalising it is the change that would make the registry's promise true.

The obstacle, and it is real, is the layering guard: that test reaches its owner as `super::injected_messages` (an allowed row), but `api::message_store`, `api::auth`, `telegram::output`, `commands::*`, `web::commands`, `cli::*`, `testability::*` and `lib.rs` are not `config` children, so a generalised test inside `instance_gitignore.rs` would need `crate::`-anchored spellings, and `ALLOWED_GUARDED_CRATE_REFERENCES` is deliberately empty (acceptance criterion 8 keeps it empty). An integration test under `src-tauri/tests/` cannot help either: separate crate, and `required_rules` is private.

Two honest options, architect's call:

- **(i)** Add a `#[cfg(test)]`-only sibling module (for example `config/instance_artifacts_coverage.rs`, declared `#[cfg(test)] mod ...;`) that is not a scanned unit of the guard, imports each owner constant, and asserts registry coverage. It adds no arc to the committed record (`includeTests: false`), and it is the only thing in the design that would fail when someone adds the eighth uncovered artifact. Its own risk is that it becomes a third list to keep in sync, so it must assert coverage of imported constants, never re-list names.
- **(ii)** Keep the scope as planned and rewrite Sections 1, 2 and 4.1 to claim only what ships: the registry removes retyped literals and makes renames fail loudly; it does not detect an artifact nobody declared. Then 12.1's seven rows are a one-time inventory correction, and the residual is on the record.

I lean to (i) with a follow-up issue if it does not fit this change, but (ii) with corrected wording is acceptable. What is not acceptable is Section 2's current framing plus 12.1's seven omissions in the same document.

### 12.7 Mechanical consequences of product decision 1 (SQLite glob)

Replacing rows 6, 7 and 8 with one `Glob` row `api-message-bus.sqlite3*` gives, and the plan's every count must be restated to these:

- `Ignore` rows: **30** (was 32). Byte-sort position is unchanged, row 6: `api-audit.log` < `api-message-bus.sqlite3*` < `app-outbox-path.txt` (`i` 0x69 < `p` 0x70 at the third byte; `*` is 0x2A and sorts before every sidecar suffix, so the glob also cannot collide with a future literal row).
- Total emitted rules on a fresh file: 2 dynamic + 30 = **32**. Fresh file length: **64 lines** (Section 5.1's 68 and Section 8 test 1's literal assertion both change).
- Rows appended to a byte-exact legacy 14-rule file: **18** (Sections 5.2, 8.2, 9.2 and the row-number list in 5.2).
- Parity against #1446: **17 from #1446's confirmed set** (11.3's 19 minus the 3 enumerated sidecars plus the 1 glob) **plus 1 under the #1209 delegation** = 18. 11.3's restatement request stands with these numbers.

Probed against real git: `/api-message-bus.sqlite3*` ignores `api-message-bus.sqlite3`, `-shm`, `-wal` and `-journal`. Keep all four as fixture samples (adding `-journal` is what makes the glob's reason for existing testable), and keep 11.9's `state.sqlite` control, which still proves the rule is narrow. 11.10 is closed by this decision; drop its "comments should say WAL-mode-specific" note, since the glob is mode-independent.

If 12.1 adds rows, these counts move again. Whatever the final numbers, they must appear in exactly one place in the plan and be derived, not retyped, in the tests (a helper that builds expected bytes from the table, per Section 8.6).

### 12.8 #1209 scope (product decision 2): the wording fix, plus a precedent worth naming

Option (ii) is the right call and matches 11.4. Two additions:

1. The AC-root generator already carries this exact pattern class: `commands/ac_discovery.rs:1498-1501` emits `/.seed-manifest.*.tmp` with a comment. So the FUP (#1448) is a one-row edit against a generator that already has the shape, and the plan can say so, which is more useful to the next reader than "out of scope".
2. Section 9.6's acceptance criterion still says "with that, this change closes #1209". It must say "closes the instance-dir half of #1209; the AC-root half is #1448", and Section 4.6's paragraph likewise. With 12.3 unfixed it would not even be true of the instance-dir half.

### 12.9 The compatibility test can pass while the guarantee it names does not hold

`legacy_fourteen_rule_file_gains_exactly_the_new_entries` is feasible and 11.12's reading of the machinery is correct (`contains_exact_line:224-233`, `append_buffer:235-250`, `fresh_file_bytes:206-214`, all confirmed). Real user-file states that the test as specified does not cover, in descending severity:

1. **Read-only or locked existing files become a recurring startup warning.** `read_only_complete_file_needs_no_write_but_partial_file_fails_unchanged` (`:604-...`) encodes today's split: a **complete** read-only file returns `Ok` with no write; a **partial** one fails. This change makes every existing complete file partial. So every installation whose instance `.gitignore` is read-only, on a read-only volume, or held by another process now takes the failing branch on **every** startup instead of the silent-OK branch. It is fail-soft (`logging.rs:481-485`), but note that the message is `eprintln!` and is suppressed entirely when `machine_output_enabled()`, so in machine-output mode the failure is invisible. Section 6's "additive repair on next startup, no migration, no user action" is not true for that population. One sentence in Section 6 and one test case (complete-legacy + read-only, assert `Err` and unchanged bytes) settle it.
2. **UTF-8 BOM.** Probed: git strips a BOM from `.gitignore` (the rule after it works), but `contains_exact_line` does not, so on a BOM'd file the first rule reads as missing and is appended, now as a comment+pattern pair. One-time duplicate, self-healing on the second ensure, cosmetically worse than before because the duplicate now carries a generated comment. Pre-existing, worth one line in Section 5.4 rather than a fix.
3. **CRLF files.** Detection strips one trailing `\r`, so legacy rules are found, but the appended block is written with bare `\n`, producing a mixed-ending file. Existing behaviour, unchanged by this plan, but the new block is 18 pairs instead of a few lines, so it is now the dominant part of the file. Say so in Section 5.4 next to 11.12's orphan-comment case.
4. **The frozen literal couples the test to `ROOT_AGENT_DIR_NAME`.** Section 8.2 seeds "the byte-exact pre-change 14-line content ... frozen in the test as a literal". Rules 1 and 2 embed `ROOT_AGENT_DIR_NAME`. If that constant ever changes, the seeded dynamic rules stop matching and the ensure appends 20 pairs, so the test fails with "expected 18 appended, got 20" rather than naming the cause. Build the two dynamic lines from `super::ROOT_AGENT_DIR_NAME` and freeze only the 12 `FIXED_RULES` bytes.
5. Reordered rules, user comments, duplicated lines and trailing whitespace: all safe, detection is per-line and order-free. A user who deletes only the pattern of a generated pair gets 11.12's orphan comment. A user who edits a pattern (say `/app.log` to `/app.log*`) gets the canonical line re-appended alongside theirs; that is today's behaviour and is correct.

### 12.10 Already-tracked artifacts: the wording is honest, the tests do not reach it

Section 6 correctly says a tracked artifact still needs `git rm --cached`, and no wording in the plan overclaims untracking. Two gaps:

1. **No test covers a newly-ignored artifact that is already tracked.** The fixture stages `instance/app.log`, which is a *pre-existing* rule, and every check uses `check-ignore --no-index`, which deliberately does not consult the index. So the fixture proves nothing about the population this change actually affects. Cheap fix: stage one new artifact too (`instance/api-message-bus.sqlite3`), and assert after the ensure that `ls-files --error-unmatch` still finds it. That is criterion 3's real content.
2. **The SQLite case is worse than the generic one and deserves a sentence.** A user who has `api-message-bus.sqlite3` committed keeps a tracked database whose `-wal` and `-shm` are now ignored, which is a partially-committed database, not just noise. Combined with 12.1's `api-clients.json`, the user-facing residual of this change is "some credential-adjacent and message-content files may already be in your history and this change does not remove them". #1446's motivation is that these files should never have been committable; shipping the rule with no note tells users the problem is solved. Section 6 should list the `git rm --cached` invocations for the newly covered set, and the change should carry that note wherever release notes live.

### 12.11 Step-N gate: closable, with three corrections

Verified against the tree, so the certification pass does not have to:

1. **`config` still has zero outgoing arcs and `mod` declarations add none.** `grep -c '^agentscommander_lib::config -> ' src-tauri/module-arcs.txt` is **0** while `config/mod.rs` already declares dozens of `mod`s, and 51 arcs point *into* `config`. Section 4.1's premise ("a `mod` declaration is not a reference") and 11.13's argument both hold as measured, not just as argued.
2. **The arc record is 1009 arcs, not 976.** Section 2.4 states 976 as a current fact; `wc -l src-tauri/module-arcs.txt` is 1009 with no header or comment lines. The guard's own doc comments repeat 976 in six places (`:34`, `:129`, `:551`, `:1649`, `:1729`, `:1796`, `:1889`), so they are stale prose from #1273, and Section 4.7.3 is already editing that doc comment. Fix the number in Section 2.4 (a certification pass that checks 976 will find 1009 and stop), and either refresh or de-numeralise the guard's prose while it is being edited. This is prose only: nothing reads the file at runtime, the only `.rs` mention of `module-arcs` is in doc comments.
3. **Section 8's "run on clean trees, base `b1eefa7c` vs the final branch head" has no runnable procedure here.** `repo-AgentsCommander` is a shallow (depth 2) clone shared by every wg-11 agent, so checking out the base to produce `pre.json` mutates a tree other agents are working in. The plan must name the mechanism: extract the base with `git archive b1eefa7c | tar -x` into a scratch directory outside the shared clone and run the detector there, or take the committed `module-arcs.txt` at base as the "pre" arc set (it is committed precisely so the diff can be taken without a second checkout). Also note the detector is pure static analysis, so the scratch tree needs no build and no `target/`.

Not a problem, recorded so nobody spends time on it: criterion 4's byte-identity survives `core.autocrlf=true` on this host, because `.gitattributes` pins `src-tauri/module-arcs.txt text eol=lf`.

Whitelist mechanics: `config::local_config_io` is listed as a permitted arc source, but its only new reference is inside a `#[cfg(test)]` test, and the record is generated with `includeTests: false`, so it will contribute no arc. Harmless (the whitelist is a maximum) but worth a parenthesis so the certification pass does not read its absence as a missing edit. If 12.1 and 12.3 add rows and 12.6 option (i) is taken, re-derive the source list rather than patching it.

### 12.12 Confirmations and minor notes

1. **11.9 verified empirically.** With `/instances/` in force, a plain **file** named `instances` at the instance root returns exit 1 (not ignored), and `instances/0f0e/instance.json`, `context-cache/ac-context-1.md`, `git-guard/git.cmd` all return 0. So `check-ignore --no-index` does match a trailing-slash pattern through a nested path (the mechanism 11.9 relies on works), and 11.9's proposed control is real and load-bearing. Add it.
2. Section 11.15.2's `File | Glob =>` merge is right; add the registry unit test it proposes (no `File` row contains a wildcard) and 11.15.3's leading-`/`-or-`!` test. Both are the only things standing behind acceptance criterion 10.
3. Section 4.3.4's `fresh_file_bytes` capacity calculation currently sums `rule.len() + 1`; with comments it must sum `comment.len() + pattern.len() + 2`. Cosmetic (a wrong capacity only costs a realloc), but the plan specifies the function body, so specify it correctly.
4. Section 4.5's tie test uses `Disposition::Track` and `ArtifactKind::Dir` unqualified inside `instance_gitignore.rs`; those spellings need an import, which is another `super::instance_artifacts` reference and is covered by 12.4's row, but the guard counts pairs by leading segment, so a `use super::instance_artifacts::{Disposition, ArtifactKind};` still reports the single child `instance_artifacts`. No extra row needed; say so, because it looks like it would need one.

## 13. Consensus round 1: resolution ledger

Resolved by the wg-11 architect on 2026-08-19. Sections 11 and 12 above are the enrichers' signed input and are preserved verbatim; where their numbers assumed the pre-consensus table (11.3's 19+1, 12.7's 30/32/18/64), the Section 4.2 canonical block supersedes them. The body (Sections 1-10) is the authoritative spec.

Product decisions applied (user, via tech-lead dispatch `20260819-211545`):

- The seven 12.1 classes: all `Ignore`; rows 4, 10, 16, 20, 30, 36, 42 of Section 4.2 (row numbers as remapped in round 2; see the canonical block); writers wired per Section 4.4 (decision 7).
- SQLite: single glob row 12, `MESSAGE_BUS_DB_GLOB`, derivation-tested; `-journal` fixture sample added; 11.10 thereby closed, its WAL-note suggestion dropped as moot (decision 8).
- `update-check.json.tmp`: control to required, declared as a #1164-control reversal (decision 9); row 44. The second such reversal, `app.log.1`, is decision 15 (Section 4.6 enumerates both).
- #1209: instance-dir half only; Sections 4.6 and 9.6 reworded; #1448 carries the AC-root half with the `/.seed-manifest.*.tmp` precedent named (decision 10; 11.4 and 12.8 adopted as option ii).

Technical findings, one line each:

- **11.1 + 12.4** (registry guard tables): adopted option 1; tests stay in the registry; super table exactly `[("src/config/instance_artifacts.rs", "*")]`; `Observation` glob count pinned to 1; Section 4.1's leaf phrasing corrected to the arc contract. (Sections 4.1, 4.7.2)
- **11.2 + 12.5** (alias form): const-alias definitions adopted uniformly for all nine constants with measured visibilities. (Section 4.4)
- **11.3** (count provenance): restated in the canonical block with post-consensus numbers. (Section 4.2)
- **11.5 + 12.3-A** (anchor): `.*.*.tmp` emitted unanchored via new `ArtifactKind::GlobAnyDepth`, count pinned to one; per-kind anchoring assertion replaces "all root-anchored". (Sections 4.1, 4.3, 4.6, 8.1)
- **11.6** (paraphrased tie): registry-owned `matches_atomic_write_tmp_glob` predicate adopted; `local_config_io` test uses it; agreement table in the registry. (Sections 4.1, 4.6)
- **11.7** (reach): `Reach::WithSubmodules` adopted for the registry scan. (Section 4.7.2)
- **11.8** (exact widening): the single 7th row plus its production-allowed doc paragraph. (Section 4.7.1)
- **11.9 + 12.12.1** (Dir semantics): superseded by a stronger table-derived `dir_rows_require_a_real_directory` fixture covering every Dir row, present and future. (Section 8.6)
- **11.12** (mechanics): `contains_exact_line` signature note and the orphan-comment residual documented. (Sections 4.3.3, 5.4)
- **11.13** (prerequisites): both named as implementation checklist items. (Sections 4.1, 4.7.4, 7)
- **11.14** (blast radius): two-commit shape adopted. (Section 7)
- **11.15** (minors): package-name hedge dropped; `File | Glob` merged arm; charset test extended with leading-`/`/`!` and wildcard checks; the line-count assertion is table-derived rather than a retyped literal (byte-exact expected content subsumes its intent). (Sections 4.1, 4.3, 8)
- **12.1** (incomplete inventory, BLOCKER): the seven classes registered under product decision 7; certification verification added the eighth sibling `api-clients.lock` (`api/auth.rs:637,678`, persistent lock, same class as `settings.json.lock`, evidence Section 2.5), row 11, `API_CLIENTS_LOCK_FILENAME`; Section 3 records the standing rule for future exclusions. (Sections 1, 2, 3, 4.2, 4.4)
- **12.2** (fixture self-disarm, BLOCKER): `.gitignore` removed from the control write loop; asserted post-loop as not-ignored AND byte-intact. (Sections 5.5, 8.5)
- **12.3** (glob misses, BLOCKER): unanchored row 1 plus dedicated rows 4, 20, 44 for the three off-shape schemes (two derivation-tested, one N literal each where no real coupling exists); doc-comment claim downgraded to the enumerated set; depth samples and `sub/foo.tmp` control added. (Sections 4.2, 4.6, 8.5)
- **12.6** (drift class honesty): option ii adopted with unambiguous wording; the registry closes rename drift, reduces and inventories appearance drift, and detects neither by mechanism; rationale for rejecting option i recorded (a `#[cfg(test)]` coverage sibling ties only enrolled constants: the same failure mode relocated, plus a third list; post-change aliases make its assertions tautological). (Sections 1, 2, 3)
- **12.7** (counts): re-derived after decisions 7-9 and the lock row; single canonical block; tests derive, never retype. (Section 4.2)
- **12.9** (compat-test honesty): read-only-legacy test added; Section 6 population scoping; BOM and CRLF residuals documented; dynamic seed lines built from the constant, only the 12 historical lines frozen. (Sections 5.4, 5.10, 6, 8.2, 8.3)
- **12.10** (tracked artifacts): sqlite staged alongside `app.log` with `ls-files --error-unmatch` assertions; `git rm --cached` remediation list and release-note carry in Section 6. (Sections 6, 8.5)
- **12.11** (Step-N): arc count corrected to 1009 (re-measured); guard prose de-numeralized while Section 4.7 already edits that file; executable `git archive`-to-scratch procedure written; whitelist re-derived to 23 recordable sources with the two expected-absence/presence parentheses. (Sections 2.4, 4.7.5, 8)
- **12.12.2/3** (minors): both registry tests added; capacity formula specified correctly. (Sections 4.1, 4.3.4)

### Round 1 re-verification (resumed architect session, 2026-08-20 UTC)

The session that wrote the consolidation above was lost before reporting; the resumed session re-audited the whole document against the dispatch and the tree at `b1eefa7c` before certifying. The consolidation was found complete (no truncation: the earlier "719 to 591 lines" alarm was a blank-line-insensitive count of the 837-line file), all four product decisions and all 11.x/12.x resolutions verified present, and the following residues were found and fixed:

- Section 2.3 still said only `api-audit.log` leaves the controls; under product decision 9 `update-check.json.tmp` leaves too, eight controls remain.
- Knot member count: the guard-era "88 modules" was stale; a fresh detector run measured 85 at `b1eefa7c` (Sections 2.4, 10). Section 4.7.5 now extends the count-free prose treatment to the guard's stale knot counts.
- `settings.pre-384-v1.json` is NOT writerless: `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`, called from `:1875`) is live, write-once. Evidence corrected (Section 2.5), writer classified as the fourth deliberately-untouched writer (Sections 3, 4.4), acceptance criterion 5 recounted.
- Section 2.6 described the temporaries glob as anchored; wording aligned with the adopted unanchored row.
- Acceptance criterion 5 scoped to path-construction positions, with log tags, log/error strings, and doc comments carved out (measured examples cited) so the implementer's self-check can actually pass.
- Section 7 now carries 12.1's standing requirement to re-run the `config_dir()` enumeration on the implementation tree (re-run clean at certification; the only extra hit is test-only).
- Section 6's `git rm --cached` list completed with `api-message-bus.sqlite3-journal` and tracked `settings.pre-*.json` backups.

## 14. Round 2: the inventory, taken with the new recipe

Author: wg-11 architect, 2026-08-20 UTC. Base read: `dcd221fb` (Codebase Memory gate `ready`, project `D-0_repos-AgentsCommander_iac-.ac-wg-11-dev-v5-team-repo-AgentsCommander`, 24218 nodes / 132479 edges, head `dcd221fb`), working tree clean before and after. Every file:line below was read first-hand; every ignore claim below was probed against real `git check-ignore --no-index` in a throwaway repository, not reasoned about.

### 14.1 The four decided classes, and the evidence behind each

| class | writers (production, verified) | decision | row |
|---|---|---|---|
| `agency-agents_templates.lock` | `cli/agency_templates.rs:138` in `CacheLock::acquire`, called from `:320`, `:370`, `:389`; root through the wrapper `config_dir_or_err()` (`:301-304`) | Ignore (11) | 7, via the glob |
| `agency-agents_templates.next-<uuid>/` | `cli/agency_templates.rs:414-418`, update staging | Ignore (11) | 7 |
| `agency-agents_templates.download-<uuid>/` | `cli/agency_templates.rs:637-641` in `fetch_repo_with_git`, a full git clone of an external repository deposited in the instance dir | Ignore (11) | 7 |
| `agency-agents_templates.prev-<uuid>/` | `cli/agency_templates.rs:920-924` in `publish_staging`, a full copy of the previous cache | Ignore (11) | 7 |
| `coding-agent-requests/` and its `results/` | `cli/coding_agent.rs:424-425` (root bound 22 lines earlier at `:402`), `phone/mailbox.rs:10974-10978` (root bound at `:10970`); constants at `config/coding_agent_mutations.rs:35,37` | Ignore (12) | 17 |
| `Context.AgentsCommander.md.retired-<ts>[.<n>].bak` | `config/seeded_context_templates.rs:1597-1606`, `create_new` in `live_path.parent()` | Track (13) | Track table |
| `Context.AgentsCommander.md` | `config/seeded_context_templates.rs:1430`, joined onto the `config_dir` parameter that `config/root_agent.rs:810-816` supplies as `root_dir.parent()` | Track (14) | Track table |

Probe results, all twelve as designed: `agency-agents_templates.lock`, and one nested file inside each of the `.next-`, `.download-` and `.prev-` trees, are ignored; `coding-agent-requests/req-1.json` and `coding-agent-requests/results/res-1.json` are ignored; `agency-agents_templates/engineering/role.md` and `coding-agents/agents.json` are **not** ignored, so neither new rule reaches its `Track` neighbour; `Context.AgentsCommander.md` and both backup shapes are **not** ignored; and a plain file named `coding-agent-requests` is **not** ignored, which is the `Dir` semantics the table-derived `dir_rows_require_a_real_directory` fixture will assert automatically for the new row. The three staging shapes are directories and their rule carries no trailing slash, so the nested samples are what prove a matched directory still ignores its contents under `--no-index`.

### 14.2 What the recipe classified as covered or out of scope

Recorded so the next run starts from a decided baseline rather than re-deriving one (Section 7's stop-the-line criterion, classification (b), except the final entry, which is the round-3 (d) baseline):

- `config/mod.rs:124` `parent.join(format!(".{}", stem))` constructs the instance directory itself (`<exe dir>/.<exe stem>`); it is not a child of it.
- `config/session_context.rs:1099`, `config/coding_agents_catalog.rs:446`, `config/root_agent.rs:1682,1735`, `config/seeded_context_templates.rs:706`, `config/local_config_io.rs:127` all publish `.{name}.{pid}[.{n}].tmp`, which row 1 covers unanchored at every depth.
- `config/injected_messages.rs:1207` composes `.bak-<stamp>`, covered by row 28.
- `testability/ui_automation.rs:1747` `path.with_extension("tmp")` publishes inside `ui-automation/` (`:252-253`, `:1245-1246`, `:1384`), covered by row 42's subtree.
- `config/coding_agents_catalog.rs:943,968` (`backup_master_dir`, `staging_sibling`) publish siblings of a master directory **inside** `coding-agents/`, reached through `master_dir_for_dest` (`:874`) and `catalog_dir` (`:134-135`); they are not direct children, and their parent is a `Track` row by decision 6.
- `config/config_seed.rs:554-555`, `commands/entity_creation.rs:3869`, `config/projects.rs:2382`, `commands/ac_discovery.rs:4347` all publish under the project `.ac/` root, not `config_dir()`; that root is governed by `ensure_ac_root_gitignore` and is #1448's territory (Section 3).
- `phone/mailbox.rs:2328` publishes a `.pty-input-tmp` sibling of a PTY input destination, not an instance-dir child.
- `commands/config.rs:2652`, `config/settings.rs:7349` are `#[cfg(test)]` helpers building temporary project directories.
- `commands/session.rs`'s `.claude-mb` remains the previously recorded test-only hit.
- Confirmed by grinch and unchanged: `teams.json` has no live writer, `outbox/` is `<agent root>/<local dir>/outbox` (`cli/send.rs:658-663`), and `AgentsCommanderContext.md` is a pre-migration name.
- **(d), added in round 3.** `settings.json.backup.jo`, a direct child of `.agentscommander_amp-office` surfaced by leg 4, is a hand-made user copy: it gets **no row**, it is deliberately not covered, and it stays visible to git. The three findings the (d) bar requires, taken along independent paths by dev-rust, grinch and this certification. *(i)* Leg 3a run in full reports **12** `with_extension`/`with_file_name` sites in `src/` and none composes `.backup`; eleven are already classified in this subsection, in 14.3, or are tests, and the twelfth, `src/path_identity.rs:586` in `#[cfg(unix)] unix_cleanup_claim_path`, composes the `.{uuid}.terminal-snapshot-private-cleanup` leaf built at `:327` and is rooted in a terminal-snapshot operation directory, classification (b). *(ii)* Grinch's search of the shipped 59.7 MB binary, which carries the JS bundle as well as the Rust, returns **0** hits for `settings.json.backup` and **0** for `backup.jo`, its only `.backup` hits being UI strings about the coding-agent catalogue's `backupPath`; re-checked at source level for this certification, `rg 'backup\.jo|settings\.json\.backup' src` is empty. *(iii)* The file's own forensics, measured first-hand here, rule out a runtime sibling: 4645 bytes against the live `settings.json`'s 4085, written 4m16s later (17:30:20 against 17:26:04, 2026-08-13), and carrying a key set that is a strict subset of the live one, missing `activityLogEnabled`, `agentAutoUpdate`, `agentUpdateDontAskAgain`, `gitSweepConcurrency`, `gitSweepMinIntervalSecs` and `terminalSnapshotsEnabled` while adding nothing. What this entry decides is the **class**, not the file: a direct child that clears the (d) bar is outside the registry's subject and needs no escalation, and one that does not clear it is unclassified and still halts the line. Its neighbour `settings.json.lock` in the same directory is not this class; it is row 39, matched by literal equality, classification (a).

### 14.3 DECIDED (product decision 15): rotated log and archive generations are `Ignore`

**Status: closed.** Found by leg 3 of the Section 7 recipe, escalated as a stop-the-line rather than decided, and ruled `Ignore` by the user (tech-lead dispatch `20260820-071012`). This is the class no prior inventory reported, in three attempts, because every sweep before leg 3 was anchored on the *root* of the path and this class is published by *sibling* naming.

Four production rotation schemes write numbered generations as **direct children of the instance directory**. Before decision 15 none had a registry row, and no existing rule matched any of them (checked against every `Ignore` row: no leading dot, no `.tmp` suffix, and no glob reaching them):

| artifact | writer | generations |
|---|---|---|
| `app.log.1` .. `app.log.5` (row 15) | `logging.rs:275-330`; the `numbered` closure at `:313` joins `format!("{stem}.{i}")` onto the parent, where `stem` is `base.file_name()` (`:300`) and `base` is `config_dir().join("app.log")` (`:487-489`) | `APP_LOG_KEEP = 5` (`:248`) |
| `api-audit.log.1` (row 9) | `api/audit.rs:395-407`, `rotate_if_needed`, `path.with_extension("log.1")` at `:403`, where `path` is `config_dir().join(API_AUDIT_LOG_FILE_NAME)` (`:21`) | single generation, at `AUDIT_MAX_BYTES` = 10 MB (`:18`) |
| `activity.jsonl.1` .. `activity.jsonl.4` (row 6) | `config/activity_log.rs:606-645`, `parent.join(format!("{name}.{index}"))` at `:624,628,638`, rooted at `config_dir()` via `:719,745-746` | `ACTIVITY_KEEP = 4` (`:109`) |
| `orphaned-sessions.archive.json.1` .. `.3` (row 33) | `config/sessions_persistence.rs:780-800`, the `numbered` closure at `:796` joins `format!("{stem}.{i}")` onto the parent, rooted at `config_dir().join(ORPHAN_ARCHIVE_FILENAME)` (`:916`) | `ORPHAN_ARCHIVE_KEEP = 3` (`:65`) |

Why it was escalated instead of decided, recorded because the reasoning is the reusable part:

1. **The live file of all four is already an `Ignore` row** (rows 14, 8, 5 and 32), so the spirit of product decision 1 pointed at `Ignore`. That is an argument, not an authority.
2. **`app.log.1` was an explicit fixture control asserting it must NOT be ignored** (`control_paths`, first entry, inherited from #1164). Covering it reverses a recorded narrowness control, which is exactly the shape of `update-check.json.tmp`: that one needed its own product decision (9) and its own declaration. Making the same reversal silently, for a second control, would have been an architect deciding product. Section 4.6 now enumerates both reversals together and names the trap they share: a control reads as a deliberate decision, so nobody re-checks whether the name has a live writer.
3. It was not a one-row edit either way, so the counts and the canonical block moved under either answer.

**Decision and its shape.** `Ignore`, as four `Glob` rows of the form `<live name>.*`: rows 6, 9, 15 and 33. Three are `G` rows carrying a derivation test against the live artifact's registry constant (`ACTIVITY_LOG_ROTATION_GLOB`, `API_AUDIT_LOG_ROTATION_GLOB`, `ORPHAN_ARCHIVE_ROTATION_GLOB`); `app.log.*` is an `N` literal because its live row is an `L` literal whose writer Section 3 leaves alone, the same `L`-live/`N`-derived pairing rows 43 and 44 already use. Section 10's round-2 subsection records why a digit class (`app.log.[0-9]`) was rejected and why no writer is rewired.

**Probed against real git**, eleven assertions, in a throwaway repository:

- Ignored: `activity.jsonl.1`, `activity.jsonl.4`, `api-audit.log.1`, `app.log.1`, `app.log.5`, `orphaned-sessions.archive.json.3`. The two `KEEP`-edge samples are what prove each glob spans its whole generation range rather than only its first element.
- Still ignored, unchanged: all four live files.
- **Disjoint from row 1**: `.activity.jsonl.4242.tmp` is matched by `.*.*.tmp` and cannot be matched by `/activity.jsonl.*`, because the rotation globs are anchored and require the name to begin with the live artifact's name, while row 1 requires a leading dot. The two rules cannot overlap by construction, which is the property the dispatch asked to be classified. None of the four artifacts has a temporary of its own in any case: all four are opened in append mode (Section 2.5).
- **Not ignored (narrowness)**: `sub/app.log.1` and `sub/activity.jsonl.1`, proving the rotation rows are `Glob` and not a second `GlobAnyDepth`; `applog.1`, proving the literal dot is load-bearing; and the `Track` sentinels, unaffected.
- **No byte-order neighbour is reached by a new glob**, verified with `git check-ignore -v` rather than by inspection: `app-outbox-path.txt` and `api-clients.json` each report their own rule as the match, not `app.log.*` or `api-audit.log.*`.

With this class covered, every candidate the Section 7 recipe produced is classified (a), (b) or (c), the registry module doc's claim that every child of the instance config directory has a row is true again under the final table, and acceptance criterion 11 is satisfiable rather than aspirational.
