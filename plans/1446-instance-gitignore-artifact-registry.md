# Issue #1446 Full Plan: Instance `.gitignore` artifact registry

- Issue: https://github.com/mblua/AgentsCommander/issues/1446
- Branch: `feature/1446-instance-gitignore-artifact-registry`
- Planning base: `b1eefa7c0e076d79d7ea38d76f998d1c05fd5055`
- Delivery path: Full (architect draft; dev-rust enrichment in Section 11; grinch enrichment in Section 12; consensus round 1 resolved into the body, ledger in Section 13; certified in Section 10)
- Related: closes the instance-dir half of #1209 (see Section 4.6); the AC-root half is follow-up #1448; #1441 and #1443 stay independent
- Supersedes, by user product decision recorded in #1446 and in the consensus round: the #1164 "no cache/database directories" restriction, the #1164 "no generated comments" fresh-file byte spec, and the #1164 `update-check.json.tmp` narrowness control (that name is a live runtime temporary and is now a covered artifact; Section 4.6)

## 1. Objective

Two deliverables in one change, per #1446:

1. **Extend the generated instance `.gitignore`** (the one `ensure_instance_gitignore()` maintains inside `config_dir()`) so it covers every runtime artifact class inventoried in #1446 plus the classes the enrichment inventory added (Section 12.1, ruled by the user; one further sibling found in certification verification, Section 13). The existing append-only reconciliation then repairs every existing writable installation on its next app start with no migration and no user action (Section 6 scopes the read-only/locked exception honestly).
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
3. The git fixture test's current `control_paths` are: `app.log.1`, `update-check.json.tmp`, `api-audit.log`, `cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, `injected-messages.toml.bak`, `injected-messages.json`, `agentscommander-injected-messages.json`, `sub/injected-messages.toml`. Two of them become ignored by this change: `api-audit.log` and `update-check.json.tmp` (the latter by product decision 9); the other eight remain valid narrowness controls.
4. `src-tauri/tests/instance_gitignore_layering.rs` (#1273) is a spelling-net layering guard. Facts that bind this plan:
   - The crate has exactly one cyclic SCC ("the knot"; `coverage.graphShape.cyclicSccs = 1`). Its member count drifts as the crate evolves: the guard's #1273-era prose says 87 or 88, and a detector run on the clean tree at `b1eefa7c` during this certification measured **85** members; the binding criterion is member-set identity pre/post, never a count (Section 8). `config::instance_gitignore` was taken out of the knot by #1273 and must stay at `sccSize = 1` (re-measured: it is a size-1 SCC at `b1eefa7c`).
   - `ROOT_AGENT_DIR_NAME` was moved by #1273 into `src/config/mod.rs`, whose zero-outgoing-arcs property is load-bearing; the guard asserts `crate::config` (scanned shallow) names nothing, and asserts `instance_gitignore`'s reference sets by equality against `ALLOWED_*` tables, with the `crate::`-anchored table deliberately empty. `mod` declarations are not references; string literals and comments are stripped before matching. The guard also asserts `the_root_agent_dir_name_constant_is_defined_exactly_once`.
   - The committed arc record is `src-tauri/module-arcs.txt` (1009 arcs at `b1eefa7c`, verified by line count in the consensus round; the guard's doc comments still say 976 in several places, stale prose from #1273 that Section 4.7.5 refreshes), regenerated by `scripts/02-module-arc-record.mjs`; byte-identity of the regenerated record is the acceptance test. `agentscommander_lib::config` has zero outgoing arcs in the record while 51 arcs point into it (measured, Section 12.11.1), which is the leaf premise this design copies.
5. Owner-module facts for the drift set (file:line per dev-rust's gated investigation; the implementer verifies each on checkout):
   - Constants that already exist: `config/activity_log.rs:119` `FILE_NAME = "activity.jsonl"`; `api/message_store.rs:15` `DB_FILENAME = "api-message-bus.sqlite3"`; `config/sessions_persistence.rs:56` `ORPHAN_ARCHIVE_FILENAME`; `config/coding_agents_catalog.rs:47` `CATALOG_DIR_NAME = "coding-agents"`; `config/session_context.rs:11` `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME = "Context.root-agent.md"`; `config/seeded_context_templates.rs:7` `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME = ".agentscommander-context-templates.json"`; `commands/role_templates.rs:47` `AGENCY_TEMPLATES_DIR = "agency-agents_templates"`.
   - Inline literals to replace: `config/session_context.rs:105,1413,2391` `"context-cache"`; `config/coordinator_clocks.rs:314` `"coordinator_clocks.json"`; `api/message_store.rs:648` `"pty-input-locks"`; `api/audit.rs:19` `"api-audit.log"`; `telegram/output.rs:64,141,142` `"telegram-bridge.log"`, `"diag-raw.log"`, `"diag-sent.log"`; `commands/config.rs:80` and `web/commands.rs:723` `"debug-logs.txt"`; `pty/local_backend.rs:334,437` `"git-guard"`; `lib.rs:1821` `"instances"`; `cli/create_agent_matrix.rs:211` and `phone/mailbox.rs:10807` `"project-refresh-requests"`; `config/settings.rs:~3345` the `settings.json.lock` name construction; `commands/role_templates.rs:135,217` `"agent-templates"`.
   - Consensus-round additions (Section 12.1 measured by grinch; every site re-read by the architect at `b1eefa7c` during certification): `api/auth.rs:45` `pub const REGISTRY_FILENAME: &str = "api-clients.json"` is the single canonical constant and every production join site imports it (`api/auth.rs:296`, `cli/api_client.rs:89`, `commands/config.rs:1801`, `pty/container_tokens.rs:45`; the remaining `"api-clients.json"` literals in the tree are tests, doc comments and one error string); `api/auth.rs:637,678` join the literal `"api-clients.lock"` (created with `create(true).truncate(false)` and never deleted, the same persistent-lock class as `settings.json.lock`; found in certification verification, Section 13); `api/auth.rs:758` builds `.api-clients-<uuid>.tmp`; `cli/harness.rs:475` joins literal `"logs"` (holds `harness.log`, line 477); `cli/create_agent.rs:133,295` and `phone/mailbox.rs:10831` join literal `"session-requests"`; `testability/ui_automation.rs:17` `pub const UI_AUTOMATION_DIR: &str = "ui-automation"` (`SESSION_FILE` and deeper names stay owner-side, the dir row covers the contents); `config/agent_command.rs:499,2046` join literal `"codex-home"`; `update_check.rs:76` `path.with_extension("json.tmp")` yields `update-check.json.tmp` (the writer's own comment names it); `config/coordinator_clocks.rs:360-364` `path.with_extension(format!("json.{pid}.{seq}.tmp"))` yields `coordinator_clocks.json.<pid>.<seq>.tmp`.
   - `settings.pre-384-v1.json` HAS one live writer, found in the resumed certification pass: `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`), reached from the pre-384 settings migration path (`config/settings.rs:1875`), write-once (returns `Ok` early when the backup already exists), naming the concrete instance inline via `with_file_name`. The covering glob `settings.pre-*.json` is registry-owned and the writer is deliberately not rewired (Section 4.4).
6. `config/local_config_io.rs`: `write_file_atomic` is `pub` (80-82) and publishes through `temp_config_path(path) -> PathBuf` (122-128, private), which names the sibling `.{file_name}.{pid}.tmp` (file name falls back to `"config.json"` when not UTF-8). Because the pid segment always contributes one interior dot and `.tmp` contributes the final one, every name this scheme can produce matches the `.*.*.tmp` glob shape (emitted unanchored, Section 4.6), while `foo.tmp` and `.foo.tmp` do not match it at any depth. This is issue #1209.
7. The analogous generator `ensure_ac_root_gitignore` (`commands/ac_discovery.rs:1496-1582`) already maintains `(pattern, comment)` pairs, detects presence by trimmed-line equality on the pattern only (comments are transparent to detection), and appends `comment` then `pattern` for each missing entry. Comments, globs, and negations are established house style for generated ignore files.
8. Real generated file of this workspace (14 lines, verified byte-exact): the 2 dynamic rules with the local dir name, then the 12 fixed rules in byte-alphabetical order, all root-anchored, no comments, trailing LF, and no self-ignore line.

Gap, stated precisely (consensus resolution of 12.6): nothing ties "a module writes X into `config_dir()`" to "X is covered by the policy". The list went stale twice before (#1164 seed, #1157 repair) and the enrichment inventory proved it was stale again while this very plan was being written (Section 12.1's seven classes plus the `api-clients.lock` sibling). This change fixes the **rename** half of that class mechanically (registry constants shared with writers) and fixes the current inventory in full; the **appearance** half remains a process risk by recorded decision (Section 3, Section 13 finding 12.6). #1209 names the temporaries subclass explicitly.

## 3. Scope

### In scope

- One new leaf registry module `src-tauri/src/config/instance_artifacts.rs` plus its registration in `config/mod.rs`.
- Rewiring `instance_gitignore.rs` to derive emitted rules from the registry and to emit one comment line per rule.
- The new ignore patterns of Section 4.2 (canonical counts there) and the 5 explicit Track declarations.
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
- Normalizing the three off-shape temporary writers (`update_check.rs:76`, `config/coordinator_clocks.rs:360-364`, `api/auth.rs:758`) onto `temp_config_path`, or otherwise touching them: their names are covered by dedicated rows (Section 4.6), two of the rows are derivation-tested against the artifact constants, and all three get fixture samples. The same stance covers the settings migration backup writer `write_pre_384_v1_backup` (`config/settings.rs:1911-1916`): its concrete output `settings.pre-384-v1.json` is fixture-guarded under the registry's `settings.pre-*.json` glob and the writer is not touched. Rewriting correct writers is exactly the blast radius this plan refuses.
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

Name constants declared here and shared with writer modules (values in Section 4.2): `ATOMIC_WRITE_TMP_GLOB`, `SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME`, `ACTIVITY_LOG_FILE_NAME`, `API_AUDIT_LOG_FILE_NAME`, `API_CLIENTS_REGISTRY_FILENAME`, `API_CLIENTS_LOCK_FILENAME`, `MESSAGE_BUS_DB_FILENAME`, `MESSAGE_BUS_DB_GLOB`, `CODEX_HOME_DIR_NAME`, `CONTEXT_CACHE_DIR_NAME`, `COORDINATOR_CLOCKS_FILE_NAME`, `COORDINATOR_CLOCKS_TMP_GLOB`, `DEBUG_LOGS_FILE_NAME`, `TELEGRAM_DIAG_RAW_LOG_FILE_NAME`, `TELEGRAM_DIAG_SENT_LOG_FILE_NAME`, `GIT_GUARD_DIR_NAME`, `INSTANCES_DIR_NAME`, `LOGS_DIR_NAME`, `ORPHAN_ARCHIVE_FILENAME`, `PROJECT_REFRESH_REQUESTS_DIR_NAME`, `PTY_INPUT_LOCKS_DIR_NAME`, `SESSION_REQUESTS_DIR_NAME`, `SETTINGS_LOCK_FILE_NAME`, `SETTINGS_MIGRATION_BACKUP_GLOB`, `TELEGRAM_BRIDGE_LOG_FILE_NAME`, `ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME`, `AGENCY_TEMPLATES_DIR`, `AGENT_TEMPLATES_DIR_NAME`, `CODING_AGENTS_CATALOG_DIR_NAME`, `UI_AUTOMATION_DIR_NAME`. (The draft's `MESSAGE_BUS_DB_SHM_FILENAME`/`MESSAGE_BUS_DB_WAL_FILENAME` are dropped: product decision 8 replaced the enumerated sidecars with `MESSAGE_BUS_DB_GLOB`, and no writer ever names a sidecar, SQLite does.)

Registry-internal `#[cfg(test)]` unit tests (self-references only, all inside the single `mod tests`):

- `ignore_rows_are_unique_and_byte_sorted_by_name`: names of `Ignore` rows are strictly increasing byte-wise (keeps the table append-proof and the generated file deterministic).
- `every_row_has_a_nonempty_single_line_comment`: nonempty, single line, starts with `# AgentsCommander: `.
- `no_name_contains_slash_backslash_newline_or_leading_slash_or_bang`: also refuses a leading `/` or `!`, so `render` stays the only place anchoring is decided and a generated negation is impossible by construction (backs acceptance criterion 10; findings 11.15.3).
- `no_file_or_dir_row_contains_a_git_wildcard`: the invariant that stops someone adding a glob without choosing a glob kind (finding 11.15.2).
- `message_bus_glob_derives_from_the_db_name`: `MESSAGE_BUS_DB_GLOB == format!("{MESSAGE_BUS_DB_FILENAME}*")` (replaces the draft's enumerated-sidecar test under product decision 8).
- `coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name`: `COORDINATOR_CLOCKS_TMP_GLOB == format!("{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp")` (the writer's `with_extension` construction guarantees exactly this prefix, Section 2.5).
- `atomic_write_tmp_predicate_agrees_with_its_glob`: table of positives (`.settings.json.1.tmp`, `.a.b.tmp`) and negatives (`foo.tmp`, `.foo.tmp`, `.a.tmpx`, `.api-clients-x.tmp`); the last negative documents why the api-clients temporaries need their own row (finding 11.6).
- `exactly_one_any_depth_row_exists`: pins `GlobAnyDepth` rows to exactly one (`.*.*.tmp`); widening depth-independence to another pattern is a policy decision, not a table tweak.

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
| 6 | `api-audit.log` C | File | append-only API audit log |
| 7 | `api-clients.json` C | File | local API client registry |
| 8 | `api-clients.lock` C | File | persistent API client registry write lock |
| 9 | `api-message-bus.sqlite3*` G | Glob | inter-agent message bus database and every SQLite sidecar (`-shm`, `-wal`, `-journal`) |
| 10 | `app-outbox-path.txt` L | File | runtime outbox path handshake file |
| 11 | `app.log` L | File | application log |
| 12 | `codex-home` C | Dir | per-agent isolated coding-agent home trees |
| 13 | `context-cache` C | Dir | regenerable per-session combined-context cache |
| 14 | `coordinator_clocks.json` C | File | coordinator idle-clock runtime state |
| 15 | `coordinator_clocks.json.*.tmp` G | Glob | transient coordinator-clock write temporaries |
| 16 | `daemon.pid` L | File | daemon process id |
| 17 | `debug-logs.txt` C | File | on-demand debug log dump |
| 18 | `diag-raw.log` C | File | Telegram bridge raw diagnostics log |
| 19 | `diag-sent.log` C | File | Telegram bridge sent diagnostics log |
| 20 | `git-guard` C | Dir | generated git-guard shim scripts |
| 21 | `injected-messages.default.toml` L | File | injected-messages reference defaults |
| 22 | `injected-messages.toml` L | File | injected-messages configuration |
| 23 | `injected-messages.toml.bak-*` L | Glob | injected-messages migration backups |
| 24 | `instances` C | Dir | per-instance runtime state directories |
| 25 | `logs` C | Dir | harness policy log directory |
| 26 | `master-token.txt` L | File | local API master token |
| 27 | `orphaned-sessions.archive.json` C | File | archived orphaned-session records |
| 28 | `project-refresh-requests` C | Dir | project refresh request queue |
| 29 | `pty-input-locks` C | Dir | transient cross-process PTY input locks |
| 30 | `session-requests` C | Dir | CLI-to-app session launch request queue |
| 31 | `sessions.json` L | File | persisted session state |
| 32 | `settings.json` L | File | application settings |
| 33 | `settings.json.lock` C | File | transient settings write lock |
| 34 | `settings.pre-*.json` C | Glob | settings migration backups |
| 35 | `telegram-bridge.log` C | File | Telegram bridge log |
| 36 | `ui-automation` C | Dir | UI-automation session handshake state |
| 37 | `update-check.json` L | File | update-check cache |
| 38 | `update-check.json.tmp` N | File | transient update-check write temporary |
| 39 | `web-token.txt` L | File | local web token |

`Track` rows (never emitted; comments state why tracked):

| name | kind | comment |
|---|---|---|
| `Context.root-agent.md` C | File | user-editable root-agent context template; tracked on purpose |
| `ac-root-agent` (table literal; tie in Section 4.5) | Dir | canonical root-agent state (CLAUDE.md, memory, inbox); only its config.json rules are ignored |
| `agency-agents_templates` C | Dir | user-editable agency template sets; tracked on purpose |
| `agent-templates` C | Dir | user-editable role templates; tracked on purpose |
| `coding-agents` C | Dir | user-configurable coding-agent catalog; tracked on purpose |

The 12 L rows render byte-identically to today's `FIXED_RULES` (all are `File` or `Glob`, so no trailing slash touches a pre-existing rule; relative order preserved, verified in Section 11.3), which is what keeps every pre-change complete file byte-stable except for the one append. `FIXED_RULES` itself is deleted; the table is the single source.

**Canonical counts** (the only place in this plan where the derived numbers appear; every test derives them from the table, never retypes them):

- `Ignore` rows: **39**. `Track` rows: **5**. Table total: **44**.
- Emitted rules on a fresh file: 2 dynamic + 39 = **41**; fresh file length 2 lines per rule = **82 lines**.
- Rows appended to a byte-exact pre-change complete file: 39 minus the 12 L rows = **27** comment+pattern pairs (**54 lines**), namely every non-L row in table order.
- New-coverage provenance, "ni una mas ni una menos": **17** patterns from #1446's confirmed set (its 19 with the three enumerated SQLite literals replaced by one glob, product decision 8) + **7** from Section 12.1 (product decision 7) + **1** `api-clients.lock` (certification-pass completion of the same inventory: the persistent lock sibling of `api-clients.json`, same class as `settings.json.lock`; evidence in Section 2.5, decision logged in Section 13) + **1** `update-check.json.tmp` (product decision 9, #1164-control reversal) + **1** `.*.*.tmp` (under #1446's explicit #1209 delegation) = **27**.
- Non-obvious byte-sort adjacencies, verified: `.*.*.tmp` first (`*` is 0x2A); `.agentscommander-...` before `.api-clients-*.tmp` (`g` < `p`); `api-audit.log` < `api-clients.json` < `api-clients.lock` (`.j` < `.l`) < `api-message-bus.sqlite3*` < `app-outbox-path.txt` (`i` < `p`); `codex-home` < `context-cache` < `coordinator_clocks.json` (`d` < `n` < `o`) and the `.tmp` glob after its prefix; `injected-messages.toml` < `injected-messages.toml.bak-*` < `instances` < `logs`; `session-requests` < `sessions.json` (`-` 0x2D < `s`); `settings.json` < `settings.json.lock` < `settings.pre-*.json`; `telegram-bridge.log` < `ui-automation` < `update-check.json` (`i` < `p`) < `update-check.json.tmp`.

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

The last two rows cover the whole consensus-round artifact family for free: every production join of `api-clients.json` already imports `auth::REGISTRY_FILENAME` (Section 2.5), and `ui_automation.rs`'s deeper names (`SESSION_FILE`, request/response dirs) stay owner-side because the `ui-automation` Dir row covers the whole subtree.

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

Four writers are deliberately NOT rewired (Section 3): `update_check.rs:76` (row 38 is an N literal, like the legacy rows), `config/coordinator_clocks.rs:360-364` (row 15's glob is derivation-tested against `COORDINATOR_CLOCKS_FILE_NAME`, which row 14's writer swap already imports), `api/auth.rs:758` (row 4 is an N literal; the fixture's uuid-shaped sample guards it), and `config/settings.rs:1911-1916` (`write_pre_384_v1_backup`; row 34's glob covers its concrete output, and the fixture sample equals that output byte for byte).

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
- **The three off-shape schemes that `.*.*.tmp` cannot match** (12.3, direction B; all probed against real git) each get their own row: `coordinator_clocks.json.*.tmp` (row 15, glob derivation-tested against `COORDINATOR_CLOCKS_FILE_NAME`), `.api-clients-*.tmp` (row 4, N literal), and `update-check.json.tmp` (row 38, N literal, product decision 9: this name was a #1164 narrowness control and is in fact a live runtime temporary; the reversal is deliberate and user-approved).
- The git fixture check-ignores concrete samples for all four rows, including a subdirectory sample that proves depth-independence (Section 8).

`instance_gitignore.rs`'s module doc comment claims coverage for **the enumerated registry set**, not blanket completeness (consensus resolution of 12.3's wording point): it states that the registry in `instance_artifacts.rs` is the single source, that Track rows are deliberate, that the atomic-write and off-shape temporary schemes enumerated there are covered, and that a new artifact requires a new row (Section 3's recorded rule).

### 4.7 Layering guard update (`tests/instance_gitignore_layering.rs`)

The guard's six `ALLOWED_*` tables are its whole contract and it is written to be widened. This change:

1. Widens `ALLOWED_GUARDED_SUPER_REFERENCES` by **exactly one row**, `("src/config/instance_gitignore.rs", "instance_artifacts")`, growing the array from 6 to 7 entries (finding 11.8: `children_under` reports the leading segment, so the production spelling yields this single pair, and the test-module spellings `super::super::instance_artifacts::...` report leading segment `super`, already an allowed row). The `crate::`-anchored table stays empty. The new row gets a doc-comment paragraph on the model of the existing `injected_messages` one, stating why this reference is allowed in production code: the target is a leaf with zero outgoing arcs, so unlike `injected_messages` it does not depend on being `#[cfg(test)]`.
2. Adds `config::instance_artifacts` as a third scanned unit, observed with `Reach::WithSubmodules` (finding 11.7: the shallow reach exists for `config` because its children are separate graph nodes; the registry has no children and must never gain one, so the deeper reach costs nothing and refuses a future child that parks a reference). Its tables (findings 11.1 and 12.4, measured against the guard's own mechanism): `ALLOWED_REGISTRY_CRATE_REFERENCES` empty, `ALLOWED_REGISTRY_SELF_REFERENCES` empty, `ALLOWED_REGISTRY_SUPER_REFERENCES` exactly `[("src/config/instance_artifacts.rs", "*")]`, mirroring the host precedent at `ALLOWED_HOST_SUPER_REFERENCES` (`:565`): the one glob pair is the registry's own `#[cfg(test)] mod tests { use super::*; }` and nothing else may appear. The unit's `Observation` glob count is pinned to exactly 1, the same hardening the host row carries, so a second or nested test module cannot reopen the hole that count exists to close.
3. Extends the file's doc comment: the detector criterion now also includes `sccSize(agentscommander_lib::config::instance_artifacts) = 1`.
4. `the_root_agent_dir_name_constant_is_defined_exactly_once` stays green unchanged (the constant does not move; the registry's `"ac-root-agent"` is a string literal, which the net strips). The registry must not declare any `const` or `static` named `ROOT_AGENT_DIR_NAME`; that test scans every file under `src/`, including the new module (finding 11.13.2; also a Section 7 checklist item).
5. While editing this file's prose (points 1-3 already touch it): the doc comments cite a hardcoded arc count of 976 in several places (grinch anchors: `:34`, `:129`, `:551`, `:1649`, `:1729`, `:1796`, `:1889`), stale since #1273; the real count at `b1eefa7c` is 1009 and changes again with this very change. Replace the hardcoded counts with count-free wording that points at `src-tauri/module-arcs.txt` as the record, so the prose cannot rot a third time. Apply the same count-free treatment to the stale knot-member counts those doc comments state as current fact (87 or 88 members; the measured count at `b1eefa7c` is 85 and keeps drifting); sentences recording historical experiments ("took the knot 87 to 91") are records of past measurements, not current facts, and stay. Prose only; nothing reads the number at runtime (12.11.2).

### 4.8 Second layering guard update (`tests/claude_watcher_layering.rs`)

Added in recertification (2026-08-20): implementation proved that a second equality guard covers `src/telegram/output.rs`, which this plan had not accounted for: Section 4.4 mandated wiring that file while acceptance criterion 9 forbade touching the guard that the wiring turns red. Facts, verified first-hand at `0e902d85`:

- `production_output_module_owns_the_seam_alone` (`tests/claude_watcher_layering.rs:2539-2547`) builds the real module index from `src/lib.rs` and, via `require_exact_output_report` (`:1968-1975`), requires BY EQUALITY that the analyzed output module's source set is exactly `{src/telegram/output.rs}` and that its dependency set equals `expected_output_dependencies()` (`:1269-1282`): today exactly 4 rows (`agentscommander_lib::config`, `::network`, `::telegram::api`, `::telegram::redact`), with no doc comment on the table.
- The guard observes a registry reference as its own distinct row `agentscommander_lib::config::instance_artifacts`, not as the existing `config` row (dev-rust's measured failure output; the visitor resolves against the crate's real module tree, where the registry is declared).
- The fixed `production_modules()` list (`:1234-1250`) does not feed this test and needs no change.

Change, exactly one row plus prose:

1. `expected_output_dependencies()` grows from 4 to 5 rows: add `(OUTPUT_SOURCE, "agentscommander_lib::config::instance_artifacts")`, placed after the `agentscommander_lib::config` row (the collection is a `BTreeSet`; array position is cosmetic).
2. A comment paragraph on the table (match the file's comment style) stating why the fifth row is allowed: the target is the #1446 artifact registry, a pure-constants leaf with zero outgoing arcs in `src-tauri/module-arcs.txt`, so an arc into it can neither create, grow, nor join any SCC (the same argument `instance_gitignore_layering` records for its own `instance_artifacts` row, Section 4.7.1); it grants the output seam no new capability (no I/O, no transport, no telegram surface): it only lets the module name its three log artifacts (registry rows 18, 19, 35) through the registry, so a rename breaks the build instead of silently reopening the gitignore gap. The set stays equality-pinned: any sixth dependency still turns this guard red.
3. Nothing else in that file changes: the source set stays exactly `{src/telegram/output.rs}`, and every other expected table, `production_modules()`, and the guard mechanism stay untouched. After wiring `telegram/output.rs` and widening the table, run `cargo test --test claude_watcher_layering`; if anything in that file still fails, stop and report to the architect: this plan authorizes no further contract change in that file.

## 5. Required behavior and edge cases

1. **Fresh directory**: the generated file is, in order, comment+pattern pairs for the 2 dynamic rules and every `Ignore` row of Section 4.2 in table order, each pair `{comment}\n{pattern}\n`, no blank lines, trailing LF. Line count per the Section 4.2 canonical block.
2. **Existing complete pre-change file (the 14 rules, no comments)**: byte-stable except for one append of exactly the non-L pairs of Section 4.2 in table order (count in the canonical block). A second ensure is byte-stable. This is the no-migration compatibility guarantee for every existing writable installation (edge 10 scopes the rest).
3. **Existing complete post-change file**: byte-stable, no write.
4. **User-authored occurrences**: a user line equal to a required pattern counts as present (byte-exact, comments transparent) and never receives a generated comment; user comments, negations, duplicates, CRLF content, missing final newline, and invalid UTF-8 are preserved under the existing byte semantics. Three cosmetic residuals, stated so they are discovered here and not filed as bugs (findings 11.12 and 12.9): a user who deletes a generated pattern line but keeps its comment gets the full pair re-appended, leaving the orphaned comment mid-file; a UTF-8 BOM hides the first line from byte-exact detection, so that one rule is re-appended once (self-healing on the second ensure, pre-existing behavior); a CRLF file gains an LF-ending appended block, a mixed-endings file (pre-existing behavior, now with a larger appended block).
5. **Never ignored** (fixture controls): the five Track paths, the generated `.gitignore` itself (product decision 5; asserted without materializing a control file over it, Section 8), `foo.tmp` and `.foo.tmp` at any depth, `app.log.1`, `cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, the four injected-messages near-misses, and a plain file bearing a Dir row's name (the trailing slash is load-bearing; finding 11.9). `update-check.json.tmp` is no longer a control: product decision 9 made it a covered artifact.
6. **No untracking**: adding rules never changes the index; the fixture proves a pre-tracked `instance/app.log` AND a pre-tracked `instance/api-message-bus.sqlite3` (a newly covered artifact, finding 12.10) stay tracked.
7. **Concurrency, symlink/reparse safety, locking, fail-soft startup warning**: unchanged from #1164 behavior; no algorithmic change is authorized by this plan.
8. **Ordering**: emission order is table order (dynamic first). Reconciliation never reorders an existing file; order only affects fresh files and the append block.
9. **The knot must not grow**: every new module arc terminates in `instance_artifacts` (out-degree zero). `cyclicSccs` stays 1; `sccSize` of `instance_gitignore` and `instance_artifacts` stays 1.
10. **Read-only or locked existing files** (finding 12.9.1): this change makes every pre-change complete file partial once, so an instance `.gitignore` that is read-only, on a read-only volume, or held by another process takes the existing failing branch on every startup until it is writable: the ensure returns `Err`, the file is untouched, and the fail-soft `[instance-gitignore] warning:` is printed via `eprintln!`, which `machine_output_enabled()` suppresses entirely. Behavior is the #1164 contract, unchanged; what changes is the affected population. Section 6 documents it and Section 8 pins it with a test.

## 6. Compatibility and security impact

- **Existing installations, writable file (the overwhelming population)**: additive repair on next startup via the existing reconciliation; no migration, no user action, no comment retrofit on already-present rules.
- **Existing installations, read-only or locked file** (finding 12.9.1): the repair does not happen and the startup warning recurs, invisibly under machine output (Section 5.10). Remediation: clear the read-only bit (or release the lock, or delete the file so a fresh one is generated); the next startup repairs. The "no user action" guarantee is scoped to writable files, deliberately.
- **Already-tracked artifacts** (finding 12.10): a new rule never untracks. A user who already committed newly covered artifacts keeps them tracked, which for the message bus means a tracked database whose `-wal`/`-shm` sidecars are now ignored: a partially committed database, not just noise, and `api-clients.json` is credential-adjacent. Remediation, documented here and to be carried into release notes: from the instance directory, `git rm --cached api-clients.json api-clients.lock api-message-bus.sqlite3 api-message-bus.sqlite3-shm api-message-bus.sqlite3-wal api-message-bus.sqlite3-journal activity.jsonl api-audit.log coordinator_clocks.json debug-logs.txt diag-raw.log diag-sent.log orphaned-sessions.archive.json settings.json.lock telegram-bridge.log update-check.json.tmp .agentscommander-context-templates.json` plus any tracked `settings.pre-*.json` backup (e.g. `settings.pre-384-v1.json`), plus `git rm -r --cached` for any of `codex-home context-cache git-guard instances logs project-refresh-requests pty-input-locks session-requests ui-automation` that are tracked (each command only for paths actually in the index; `git status` shows which). Untracking remains out of scope for the code (Section 3).
- **Rust/API surface**: internal only. No IPC, schema, settings, frontend, or dependency changes. Const-alias definitions keep every existing constant importer compiling at its original visibility.
- **Ignore semantics**: all new patterns are root-anchored under the instance dir except the single depth-independent `.*.*.tmp` row (Section 4.6); `Dir` rows use a trailing slash so a plain file with the same name is not silently ignored (tested, finding 11.9). `state.sqlite` and `cache/entry.bin` controls prove the sqlite/cache rules stay narrow.
- **Security**: the generated file still contains only path patterns, never secret values. The hardened open path is untouched. The new comments disclose only artifact purposes.
- **Performance**: the ensure still runs once per startup; the rule set grows from 14 rules to the Section 4.2 count plus comments, negligible.

## 7. Implementation order

### Phase 1: MVP (two commits, finding 11.14: if the second must ever be reverted, the first still ships the whole user-visible fix, and a bisect can separate "the rules changed" from "twenty-odd modules changed")

Commit 1, self-contained and delivering the entire user-visible fix:

1. Add `config/instance_artifacts.rs` (types, constants, predicate, table, internal tests) and register it in `config/mod.rs`. Checklist (finding 11.13): the `mod` line must land (the guard's `observe` panics loudly without it), and the registry must not declare a `ROOT_AGENT_DIR_NAME` const (the exactly-once guard scans every file under `src/`). Then re-run the Section 12.1 enumeration (the `config_dir()` join sweep) on the implementation tree: every production join target must have a registry row, and any unregistered child is a stop-the-line question for the coordinator, never a silent omission (12.1's standing requirement). Re-run at certification on `b1eefa7c`: clean; the only extra-tabular hit, `.claude-mb`, is a `#[test]`-only tempdir join in `commands/session.rs` (Claude-wrapper config parsing), not a `config_dir()` artifact.
2. Rewire `instance_gitignore.rs` per Section 4.3 (render pipeline, `Vec<RenderedRule>`, comment emission, doc-comment update per Section 4.6); delete `FIXED_RULES`.
3. Update the in-file tests and fixture (Section 8), including the new compatibility, read-only-legacy, dir-semantics and root-agent tie tests.
4. Update `tests/instance_gitignore_layering.rs` per Section 4.7.
5. Regenerate `src-tauri/module-arcs.txt` and commit it in this commit.

Commit 2, the drift-closure wiring:

6. Owner-module edits per Section 4.4 (const aliases first, then literal replacements) and the `local_config_io` tie test per Section 4.6.
7. Regenerate `src-tauri/module-arcs.txt` again (this commit adds the owner arcs) and commit the delta here.

Recertification addendum (2026-08-20 UTC): commits 1 and 2 landed as `2632adb4` and `0e902d85` with one deliberate omission: the `telegram/output.rs` wiring was reverted in-branch because it turns the Section 4.8 guard red, an insufficiency of this plan as originally certified (Section 4.4 mandated the wiring while acceptance criterion 9 forbade touching that guard). Commit 3 closes it, carrying together: the three `telegram/output.rs` literal swaps (Section 4.4), the one-row widening plus its comment paragraph in `tests/claude_watcher_layering.rs` (Section 4.8), the regenerated `src-tauri/module-arcs.txt` (exactly one additional arc, `telegram::output -> config::instance_artifacts`, a source already inside the Section 8 whitelist), and this updated plan file. Re-run the Section 8 cargo gates and the Step-N criterion on that commit.

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
4. New `root_agent_track_row_matches_the_root_agent_dir_constant` (Section 4.5) and new `track_rows_are_exactly_the_declared_track_set`: the `Track` names are exactly `{Context.root-agent.md, ac-root-agent, agency-agents_templates, agent-templates, coding-agents}` (product decision 6 frozen in a test).
5. `git_fixture_ignores_exactly_required_paths_without_untracking`:
   - `required_paths` additions (one concrete sample per new pattern, nested samples for every `Dir` row per finding 11.9): `.settings.json.12345.tmp`, `coding-agents/.agents.json.4242.0.tmp` (depth-independence of row 1, inside a Track dir, the exact leftover class 12.3 measured), `.agentscommander-context-templates.json`, `.api-clients-1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed.tmp`, `activity.jsonl`, `api-audit.log` (moved from controls), `api-clients.json`, `api-clients.lock`, `api-message-bus.sqlite3`, `api-message-bus.sqlite3-shm`, `api-message-bus.sqlite3-wal`, `api-message-bus.sqlite3-journal` (the sample that makes the glob's reason testable, finding 12.7), `codex-home/agent-1/config.toml`, `context-cache/ac-context-1.md`, `coordinator_clocks.json`, `coordinator_clocks.json.4242.7.tmp`, `debug-logs.txt`, `diag-raw.log`, `diag-sent.log`, `git-guard/git.cmd`, `instances/0f0e/instance.json`, `logs/harness.log`, `orphaned-sessions.archive.json`, `project-refresh-requests/req-1.json`, `pty-input-locks/operation-1.lock`, `session-requests/create-1.json`, `settings.json.lock`, `settings.pre-384-v1.json`, `settings.pre-999-v9.json`, `telegram-bridge.log`, `ui-automation/session.json`, `update-check.json.tmp` (moved from controls, product decision 9).
   - `control_paths`: the eight remaining legacy controls (`app.log.1`, `cache/entry.bin`, `state.sqlite`, `ac-root-agent/unrelated/config.json`, `injected-messages.toml.bak`, `injected-messages.json`, `agentscommander-injected-messages.json`, `sub/injected-messages.toml`), plus `agent-templates/default-role.md`, `agency-agents_templates/engineering/role.md`, `coding-agents/agents.json`, `Context.root-agent.md`, `ac-root-agent/CLAUDE.md`, `foo.tmp`, `.foo.tmp`, `sub/foo.tmp` (narrowness holds at depth too).
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
3. The arc-set diff pre vs post consists exclusively of arcs whose target is `config::instance_artifacts`, with sources in: `config::instance_gitignore`, `config::activity_log`, `config::session_context`, `config::coordinator_clocks`, `config::sessions_persistence`, `config::seeded_context_templates`, `config::coding_agents_catalog`, `config::settings`, `config::local_config_io`, `api::message_store`, `api::audit`, `api::auth`, `telegram::output`, `commands::config`, `commands::role_templates`, `web::commands`, `pty::local_backend`, `cli::create_agent_matrix`, `cli::create_agent`, `cli::harness`, `testability::ui_automation`, `config::agent_command`, `phone::mailbox`, and the crate root (`lib.rs`). Zero other new or removed arcs; zero arcs cross a previously-clean SCC boundary. Two parentheses so absences are not misread (12.11): `config::local_config_io`'s only new reference is inside `#[cfg(test)]` and the record is generated with `includeTests: false`, so it is expected to contribute NO arc; and the const-alias form is a fully qualified path, which the detector's inline qualified-path discovery DOES record (12.5), so the owner arcs are expected to appear. The whitelist is the permitted maximum; any arc outside it is a gate failure.
4. The regenerated `src-tauri/module-arcs.txt` is committed with the change and byte-identical on re-run (empty `git status` for it afterwards).
5. Structural layering guards stay green, including the updated `instance_gitignore_layering`.

Role/layering hygiene: `instance_artifacts` is pure constants, gains no `tauri`/`AppHandle`/transport dependency, and sits below every consumer; no lower layer gains a UI-transport dependency anywhere in this plan.

## 9. Objective acceptance criteria

1. A fresh normal startup generates the Section 5.1 file, and it is created before `app.log` opens (existing ordering, unchanged).
2. A byte-exact pre-change 14-rule file, ensured once, gains exactly the non-L comment+pattern pairs of Section 4.2 appended once in table order, keeps its original bytes as an exact prefix, and is byte-stable on the second ensure. No migration or user action is involved for writable files; the read-only/locked case fails without modification and is pinned by its own test (Sections 5.10, 8.3).
3. `git check-ignore` (via the fixture) accepts every Section 8.5 required sample and rejects every control; the generated `.gitignore` itself is proven not ignored AND byte-intact after the control writes (blocker 12.2's fix); the dir-semantics fixture (Section 8.6) holds for every `Dir` row; pre-tracked `instance/app.log` and `instance/api-message-bus.sqlite3` remain tracked; parent `.gitignore` and `.git/info/exclude` remain byte-identical.
4. Every emitted rule's pattern derives from `INSTANCE_ARTIFACTS` (plus the two dynamic rules); `FIXED_RULES` no longer exists; the Track set is exactly product decision 6 and is frozen by a test.
5. Every Section 4.4 writer builds its artifact name from the registry constant (const-alias definitions included); `rg`-level duplication of the Section 4.4 names in path-construction positions outside `instance_artifacts.rs` is limited to test fixtures/expectations and the four writers Section 3 deliberately leaves untouched. Log tags, log/error message strings, and doc comments that mention an artifact name construct no path and stay literals (measured examples at `b1eefa7c`: the `[session-requests]` and `[project-refresh-requests]` log tags in `phone/mailbox.rs`, the `[codex-home]` tag at `config/agent_command.rs:540`, the "Failed to create session-requests dir" error string at `cli/create_agent.rs:135`); the 12 L-row names likewise remain owner-side literals by scope (Section 3).
6. The `local_config_io` tie test proves every `temp_config_path` name shape satisfies the registry's own `matches_atomic_write_tmp_glob` predicate, whose agreement with the unanchored `.*.*.tmp` pattern is itself registry-tested; with that, this change closes the instance-dir half of #1209 (option 1 semantics without widening `temp_config_path` visibility); the AC-root half is #1448.
7. The Step-N detector criterion of Section 8 passes in full, and `module-arcs.txt` is committed and byte-stable.
8. The updated `instance_gitignore_layering` guard passes: `instance_gitignore`'s reference delta is exactly the one `("src/config/instance_gitignore.rs", "instance_artifacts")` row, its `crate::` table is still empty, `instance_artifacts` scans (`WithSubmodules`) with empty `crate::`/self tables, exactly the one `("src/config/instance_artifacts.rs", "*")` super row and a glob count of 1, and `ROOT_AGENT_DIR_NAME` is still defined exactly once in `config/mod.rs`. The updated `claude_watcher_layering` guard also passes: `expected_output_dependencies()` is exactly the four pre-existing rows plus the one `agentscommander_lib::config::instance_artifacts` row, and the analyzed source set is still exactly `{src/telegram/output.rs}` (Section 4.8).
9. All Section 8 cargo gates pass; the final diff touches only: `config/instance_artifacts.rs` (new), `config/mod.rs` (one `mod` line), `config/instance_gitignore.rs`, the Section 4.4 owner files, `config/local_config_io.rs` (test only), `tests/instance_gitignore_layering.rs`, `tests/claude_watcher_layering.rs` (exactly the Section 4.8 one-row widening plus its comment paragraph), `src-tauri/module-arcs.txt`, and this plan file.
10. No `/*`, no generated `!` rule (impossible by construction, backed by the registry charset test), no self-ignore of the generated file, no untracking, exactly one depth-independent rule (pinned by `exactly_one_any_depth_row_exists`), no new dependency, no IPC/frontend change.

## 10. Certification

Status: READY_FOR_IMPLEMENTATION

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

- The seven 12.1 classes: all `Ignore`; rows 4, 7, 12, 15, 25, 30, 36 of Section 4.2; writers wired per Section 4.4 (decision 7).
- SQLite: single glob row 9, `MESSAGE_BUS_DB_GLOB`, derivation-tested; `-journal` fixture sample added; 11.10 thereby closed, its WAL-note suggestion dropped as moot (decision 8).
- `update-check.json.tmp`: control to required, declared as a #1164-control reversal (decision 9); row 38.
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
- **12.1** (incomplete inventory, BLOCKER): the seven classes registered under product decision 7; certification verification added the eighth sibling `api-clients.lock` (`api/auth.rs:637,678`, persistent lock, same class as `settings.json.lock`, evidence Section 2.5), row 8, `API_CLIENTS_LOCK_FILENAME`; Section 3 records the standing rule for future exclusions. (Sections 1, 2, 3, 4.2, 4.4)
- **12.2** (fixture self-disarm, BLOCKER): `.gitignore` removed from the control write loop; asserted post-loop as not-ignored AND byte-intact. (Sections 5.5, 8.5)
- **12.3** (glob misses, BLOCKER): unanchored row 1 plus dedicated rows 4, 15, 38 for the three off-shape schemes (two derivation-tested, one N literal each where no real coupling exists); doc-comment claim downgraded to the enumerated set; depth samples and `sub/foo.tmp` control added. (Sections 4.2, 4.6, 8.5)
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
