# Plan #1038: Record project-scoped seed publications in a Git-diffable manifest

Author: architect, wg-14. Full-path consensus round 1 completed after developer and Grinch enrichment.

Status: READY_FOR_IMPLEMENTATION

This status certifies the cold-start implementation specification only. It does not authorize code changes, issue or branch creation, commits, pull requests, or landing outside the six separately authorized child stages in section 9.

Base: `main` and `feature/1038-project-seed-manifest` at `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe` on 2026-07-16.

Issue: [mblua/AgentsCommander#1038](https://github.com/mblua/AgentsCommander/issues/1038), **Epic: Record project-scoped seed publications in a Git-diffable manifest**.

Related but independent: [#958](https://github.com/mblua/AgentsCommander/issues/958), the parked ownership, validation, and self-healing Epic.

## 1. Objective and product contract

Add a deterministic, schema-versioned text file at each project's canonical `.ac/seed-manifest.toml`. It records the current set of project-scoped files that an in-scope AgentsCommander publisher actually published and the UTC wall-clock time of each file's most recent successful physical publication.

The v1 contract is:

1. One row per project-relative logical destination path, never an append-only event log.
2. Only an explicit typed `Published` outcome can add a row or change `last_seeded_at`.
3. A successful physical replacement sets time from that publication even when the replacement bytes equal the prior bytes. The physical publisher captures the clock at the target commit point itself, immediately after the successful create/link/replace/install syscall and before temp/trash cleanup, directory sync, logging, or any outer state persistence. V1 records milliseconds; two publications inside the same UTC millisecond can therefore have the same representable value and produce no textual rewrite when every other field is unchanged.
4. Observation, equal-default adoption, `AlreadyPresent`, `AlreadyCurrent`, skip, ordinary failure with no committed target/lifecycle effect, lost race, read, and scan do not change the manifest. A separately typed failed-after-logical-removal outcome may only delete the now-staged-away prior scope; it never adds a row or changes time.
5. A directory publication replaces its whole manifest scope in one transaction, using the exact regular-file list accumulated from the staging tree that was installed.
6. The file is safe for Git review: stable path, stable header, stable field and row order, LF, one final newline, no absolute machine or source paths, and no no-op rewrites.
7. The manifest is diagnostic, user-editable metadata. It never grants ownership and never authorizes overwrite, repair, or deletion.
8. Existing `.agentscommander-context-templates.json` hash and user-edit safety remains authoritative and unchanged in meaning.

The initial user-visible surface is the file itself. There is no Tauri command, TypeScript type, frontend listener, CLI query, web endpoint, or GUI in this Epic.

## 2. Verified current-state gap and evidence

### 2.1 Repository state

- The branch and `main` both resolve to `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe`.
- The tracked tree was clean at planning start.
- Four unrelated untracked files already existed and must remain untouched: `plans/747-persist-raise-hand-plan.md`, `plans/796-embed-dist-single-exe-plan.md`, `plans/807-web-dispatcher-project-parity-plan.md`, and `plans/881-archive-project-plan.md`.
- Current main has no `last_seeded_at`, `lastSeededAt`, or general project seed publication inventory.
- `toml = "0.8"`, `chrono`, `serde`, `uuid`, and `sha2` already exist. Rust 1.93.1 provides `std::fs::File::try_lock`, so no locking crate is needed. The architect approves the direct Unix-target dependency `libc = "0.2"` solely for `O_NOFOLLOW` and `O_DIRECTORY` constants used by `seed_manifest` no-follow opens; it is already transitive but requires a direct declaration for legal use.

### 2.2 Required reports and consensus inputs incorporated

This certified plan incorporates and reconciles all required workgroup evidence:

- `20260716-193535-wg14-dev-rust-to-wg14-tech-lead-step1-seed-manifest-validation.md`
- `20260716-155853-wg14-dev-rust-to-wg14-tech-lead-seeded-flow-analysis.md`
- `20260716-154742-wg14-dev-rust-grinch-to-wg14-tech-lead-seeded-registry-risks.md`
- `20260716-161454-wg14-architect-to-wg14-tech-lead-reconciled-seeded-registry.md`
- `20260716-201049-wg14-architect-to-wg14-tech-lead-draft-plan-1038.md`
- `20260716-205236-wg14-dev-rust-to-wg14-tech-lead-enrichment-complete-1038.md`
- `20260716-215246-wg14-dev-rust-grinch-to-wg14-tech-lead-plan-1038-grinch-complete.md`

The live authoritative #1038 body was re-read during consensus. The pre-certification plan bytes matched the Grinch-reported SHA-256 `CB714F929A9AD8A895E0D00E93D84E0F1A537FCA1CC53D1C3B0A2842687D089E` before the edits recorded by this certification.

The earlier reconciled document's central SQLite choice is superseded by #1038's project-local text decision. Its publication-outcome, snapshot, path-safety, crash-gap, and fail-soft analysis remains applicable.

### 2.3 Current boundaries and gaps

| Boundary | Current symbols | Verified gap |
| --- | --- | --- |
| Project context create/update | `seeded_context_templates::{sync_one_template, ensure_project_context_templates, scan_project_context_template_updates, sync_project_context_template_for_read}` and `session_context::write_template_if_missing` | `write_template_if_missing` returns `Ok(())` both for the hard-link winner and an `AlreadyExists` loser. `mark_seeded` also runs for equal-default observation, so it cannot carry time. |
| Explicit context overwrite | `seeded_context_templates::overwrite_context_template_with_default` | The target publish is visible, but no publication event is returned or recorded. Hash state persistence may fail after the target replacement. |
| Legacy global self-heal | `session_context::heal_stale_global_context_template` and `atomically_replace_context_template` | The helper logs internally and returns `()`, collapsing published, changed-under-us, and failure. |
| Replica config-folder seed | `config_seed::{perform_config_seed, copy_tree}` called at `commands/session.rs:1567-1589` | `ConfigSeedReport` has only `Seeded`, `Skipped`, and `Failed`. The exact staged regular-file list and winning tier are lost. Source enumeration would overcount skipped reparse and over-depth entries. |
| Config concurrency | `ConfigSeedLockState` in `lib.rs`, held by `commands/session.rs` | The mutex is process-local. It prevents the current prefix-sweep race only inside one app instance. It does not serialize two AC processes or protect a shared manifest read/merge/write. |
| Context ownership state | `config/seeded_context_templates.rs` | `.agentscommander-context-templates.json` stores version, seeded/observed hashes, and ignored hashes for two project contexts. It has no timestamp, no config-folder rows, and no cross-process read/merge lock. It is safety state, not this feature's replacement. |
| Project Git rules | `commands/ac_discovery::ensure_workspace_gitignore` | Current managed rules do not ignore `seed-manifest.toml`, but there are no explicit rules for the persistent lock and unique temps, nor an explicit keep-visible rule for the manifest. |
| Lifecycle removal | `entity_creation::{delete_agent_matrix, delete_team, delete_workgroup, remove_replica_dir}`, `cli::{workgroup::remove, team::remove_member}` | Successful deletion has no project seed-table cleanup. Several flows can partially succeed, so cleanup cannot be inferred from outer `Ok` alone. |
| Project archive/unregister | `ac_discovery::{remove_project_inner, archive_project_inner, unarchive_project_inner}` and `config/projects.rs` | These operations change settings only and leave the project and `.ac` on disk. They must not be mistaken for seeded-scope removal. |
| Config failed rollback | `config_seed::perform_config_seed` steps 4-5 | The old destination can be renamed to trash, the new install can fail, and restoration can also fail. Aggregate `Failed` then hides that the old logical scope is still staged away, so leaving its rows untouched is falsely current. A crash between the old-destination rename and new install has the same stale-row gap. |
| New-project serialization | `ac_discovery::new_project_inner_with_settings_path` and `config::projects::register_new_project` | The current `SettingsState` write guard incidentally serializes same-process setup. Moving filesystem preparation outside it without a replacement protocol lets one caller register a `.ac` that another caller is about to roll back after partial template setup. |

Reusable current patterns were also verified:

- `workspace::wg_replica_layout_from_agent_dir` is the existing strict shape resolver for `<project>/.ac/wg-*/__agent_*`. Use it to establish an unambiguous project owner. Do not infer ownership from a generic `.ac` ancestor or from an absolute source path.
- `root_agent::atomic_replace_existing` is the reviewed same-directory atomic publish primitive, using `ReplaceFileW(REPLACEFILE_WRITE_THROUGH)` on Windows.
- `seeded_context_templates::persist_state` demonstrates unique create-new temp, file flush, file `sync_all`, atomic replacement, and best-effort parent-directory sync, but not cross-process merge protection.
- `cli::agency_templates::CacheLock` and `cli::task_ops::LockGuard` demonstrate create-new lock files and their stale-owner hazards. V1 instead uses an OS file lock whose ownership ends automatically with the file handle or process.

### 2.4 Developer revalidation against current main

Dev-Rust revalidated every affected symbol and path against `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe`, and compared the relevant tree with the earlier `75954d92` evidence baseline:

- `workspace::wg_replica_layout_from_agent_dir` still requires an already canonicalized agent directory and returns the validated agent, workgroup, workspace, and project layout. The manifest design must use those returned components instead of rediscovering ownership from string ancestors.
- `config_seed::perform_config_seed` still stages, renames the old destination to trash, installs the stage, and then removes trash. `copy_tree` is also used by coding-agent catalog backup and tests, so its existing `Result<()>` surface remains; a seed-specific collecting wrapper/internal recursion returns relative files only after each successful copy.
- `root_agent::atomic_replace_existing` is safe for its existing callers but is not by itself the manifest replacement contract: on Windows it branches through `Path::exists()` and neither platform pins the temp, destination, lock, or `.ac` directory identity across the raw comparison and path-based replace. The manifest gets a dedicated checked wrapper rather than silently inheriting stronger claims from that helper.
- `commands/session.rs` still contains the sole real-spawn config-seed chokepoint. Its manager handle is cloned rather than held as a `RwLock` guard. The settings guard used to choose the context filename is already dropped before context materialization.
- `discover_ac_agents` and `discover_project_inner` currently keep a settings read guard while performing synchronous scans. Their implementation must clone the settings snapshot and drop the guard before any blocking manifest wait.
- The global, coordinator, and Root context versions are now 2, 3, and 5. New recognized upgrade paths since `75954d92` must be exercised as actual `Published` events. The deletion boundaries and config-seed install sequence did not materially change; the intervening entity-creation changes are role-scaffold content, not new deletion commit points.

## 3. Scope

### 3.1 Included publishers

1. Project context templates:
   - `.ac/Context.AgentsCommander.md`
   - `.ac/Context.coordinator.md`
   - first create, recognized generated update, legacy self-heal, and explicit overwrite, only when the final target publication succeeds in this process.
2. Config-folder publication into a strict workgroup replica whose owning project resolves through `workspace::wg_replica_layout_from_agent_dir`:
   - every regular file successfully copied into the exact staging tree installed by `perform_config_seed`;
   - workspace profile, workspace base, matrix profile, matrix base, and catalog-default tiers;
   - one replacement scope and one shared timestamp per successful directory swap.
3. Lifecycle removal needed to keep included replica config scopes current:
   - replica removal from a workgroup;
   - Agent Matrix deletion when it cascades through replica directories;
   - workgroup removal;
   - team deletion for each workgroup actually removed plus valid matching scopes explicitly retired by the committed team delete even when that workgroup was already absent; named workgroups whose removal fails remain active.

### 3.2 Explicit exclusions

- `.ac/.gitignore` is excluded as a row. It is an additive operational scaffold with mixed AC and user content, not an AC-owned seed snapshot. This Epic adds only the managed rules in section 4.1 and must not claim the whole file was seeded.
- Per-instance `<config_dir>` outputs: coding-agent catalog, factory masters, Root Agent context/Role/default skills, and the local agent-template README. They have no unique project owner.
- Config seed into `ac-root-agent` or any launch root that does not resolve as a strict workgroup replica. The existing seed action still follows its current behavior, but it produces no project manifest row.
- Runtime context cache, combined context, `last_ac_context.md`, `CLAUDE.md`, `AGENTS.md`, and `GEMINI.md` materialized for launches.
- Settings, operational `config.json`, `TASK.md`, `conventions.md`, messages, tokens, logs, requests, sessions, clocks, locks, backups, temps, trash, delete sentinels, and Git clone contents.
- Agent Matrix `Role.md` and skills created from role templates, role experiments, Agency cache, generic-agent scaffolds, and team/workgroup state. Those belonged to the superseded broad instance-registry proposal, not #1038's frozen project-publisher scope.
- Legacy `Context.agent.md` to `Context.AgentsCommander.md` hard-link migration. It preserves user content rather than publishing a builtin default, so it has no valid v1 `source` and emits no row/time. A later real recognized default update can publish normally.
- Backfill from existing files, ownership hashes, mtimes, Git history, or logs.
- Existence scanning, content-match reporting, and automatic garbage collection based on `NotFound`.
- New public IPC, CLI, web, or GUI query surfaces.
- #947 protected-destination validation. This manifest neither solves nor weakens that separate safety issue.

## 4. Decided file format and schema

### 4.1 Paths and companion files

| Purpose | Exact project-local path | Git behavior |
| --- | --- | --- |
| Canonical manifest | `.ac/seed-manifest.toml` | Deliberately visible and reviewable. |
| Cross-process gate | `.ac/.seed-manifest.lock` | Persistent empty regular file, ignored by managed `.ac/.gitignore`; never deleted on unlock. |
| Atomic writer temp | `.ac/.seed-manifest.<uuid>.tmp` | Unique create-new sibling, ignored and cleaned after failure or by the next locked writer. |

`ensure_workspace_gitignore` adds these exact managed patterns, in this order after the existing entries:

```gitignore
# AgentsCommander: exclude seed-manifest coordination files.
/.seed-manifest.lock
/.seed-manifest.*.tmp

# AgentsCommander: keep the seed publication manifest reviewable.
!/seed-manifest.toml
```

The leading `/` anchors all three rules to the directory containing `.ac/.gitignore`, so nested user files with the same basename are not accidentally ignored or re-included. The negation keeps the root manifest visible under AC's own managed rules. It cannot re-include the file when a parent repository rule ignores the `.ac/` directory itself. AC must document that limitation and must not silently rewrite a user's parent `.gitignore`.

### 4.2 V1 TOML

The serializer emits this exact shape and field order:

```toml
# Managed by AgentsCommander. Diagnostic only; never grants file ownership.
schema_version = 1
coverage_version = 1
coverage = ["project_context_templates", "replica_config_folders"]

[[files]]
path = ".ac/Context.AgentsCommander.md"
path_encoding = "utf8"
kind = "project_context_template"
scope = "context:agentscommander"
source = "builtin"
last_seeded_at = "2026-07-16T19:40:07.123Z"

[[files]]
path = ".ac/wg-14-dev-team/__agent_architect/.claude/settings.json"
path_encoding = "utf8"
kind = "replica_config_file"
scope = "config:.ac/wg-14-dev-team/__agent_architect/.claude"
source = "workspace_base"
last_seeded_at = "2026-07-16T19:41:12.456Z"
```

The exact canonical empty encoding produced by the same wire struct is:

```toml
# Managed by AgentsCommander. Diagnostic only; never grants file ownership.
schema_version = 1
coverage_version = 1
coverage = ["project_context_templates", "replica_config_folders"]
files = []
```

All four top-level fields are required, including `files`; do not add serde defaults that make a missing field valid. Coverage values and order must match exactly. For a nonempty vector, `toml` 0.8 emits the `[[files]]` array-of-tables form shown above instead of `files = []`.

Allowed v1 values are closed enums:

- `kind`: `project_context_template`, `replica_config_file`.
- Context `scope`: `context:agentscommander`, `context:coordinator`.
- Config scope: `config:` plus the UTF-8 project-relative destination directory using `/` separators.
- `source`: `builtin`, `workspace_profile`, `workspace_base`, `matrix_profile`, `matrix_base`, `catalog_default`.
- `path_encoding`: `utf8`, `unix_bytes_hex`, `windows_utf16_hex`.

The schema intentionally omits content hashes, file size, absolute project path, source path, profile letter, host, user, process id, operation history, and observed existence. Existing context ownership hashes stay in their specialized JSON file.

V1 applies explicit resource bounds: canonical file size at most 128 MiB, at most 250,000 rows, and at most 256 KiB of UTF-8 text in the decoded wire value of any `path` or `scope` field. Enforce the byte cap before an unbounded read allocation by opening the canonical file without following links and reading through `take(128 MiB + 1)`; reject the extra byte. With `toml` 0.8, row and string limits cannot honestly be guaranteed before parser allocation. A custom serde visitor for `files` rejects the 250,001st row, and row visitors reject a decoded `path` or `scope` immediately when its UTF-8 byte length exceeds 256 KiB; post-parse validation repeats all three checks as defense in depth. The 128 MiB cap is the hard parser input-size envelope, not a claim that parser working memory equals input size; the second raw conflict comparison must stream against the retained snapshot through a fixed-size buffer rather than allocating another 128 MiB copy. Activation is blocked unless adversarial near-cap valid and invalid TOML, including many tiny tables, large escaped strings, deep arrays, duplicate headers, and early/late syntax errors, stays within the same 512 MiB additional-working-set limit as the 100k-row acceptance run and returns a typed error rather than aborting the process.

The same bounds apply outbound before allocating the wire `Vec` or TOML `String`. Config path collection uses checked arithmetic and an exact TOML-escaped-length counter for the fixed v1 wire layout. It retains `Exact(Vec<PathBuf>)` only while the per-field, 250,000-row, and 128 MiB prospective-scope budgets remain valid; on first overflow it drops the accumulated identities, switches to `OverBound { reason, observed_at_least }`, and continues staging without further manifest-list growth so fail-soft target behavior is preserved. After merge, compute the exact full canonical byte length with checked arithmetic before serialization. If the complete manifest would exceed a bound, do not serialize the unrecordable batch: a successful target remains a carried publication, the pure mutation removes that publication's now-replaced prior scope from a valid canonical snapshot, the writer attempts at most that bounded removal transaction, records no new rows/time, and reports `PublishedUnrecorded(ResourceBound)`. If the removal write also fails, preserve the canonical bytes and report the persistence failure with the stale scope. Never retain the prior scope by design as though it survived a successful replacement, never truncate the installed file list, and never backfill the unrecorded publication later. These bounds leave the required 100k-row acceptance case inside the contract.

### 4.3 Deterministic serialization

1. Parse into dedicated `SeedManifestWire` and `SeedManifestRowWire` serde structs, each with `deny_unknown_fields`; do not deserialize through `HashMap` or serialize the internal keyed map directly. Declare fields in the exact wire order shown above. TOML duplicate scalar keys, duplicate table declarations, and scalar/table type redefinitions are hard parse errors, not last-one-wins input.
2. Validate the fixed schema and coverage values before allowing a write.
3. Represent `kind`, `source`, and `path_encoding` as closed serde enums. Internally key rows by `(path_encoding, path)`. Reject duplicate keys, invalid enums, malformed timestamps, mismatched kind/scope/source combinations, and a config row outside its declared scope. Every row in one config scope must also have the same `source` and byte-identical canonical `last_seeded_at`; mixed-source or mixed-time batches could not have been produced by the declared one-swap publication and make the whole canonical file read-only.
4. Sort rows by the serialized tuple `(path_encoding, path)` using bytewise ascending order. Do not sort by timestamp, source, locale, filesystem case rules, or discovery order.
5. Convert the sorted internal rows into a `Vec<SeedManifestRowWire>` and serialize the wire structs with the pinned `toml` 0.8 dependency. Golden fixtures pin the resulting byte layout so a dependency upgrade cannot silently change Git output.
6. Format time with `SecondsFormat::Millis` in UTC, always a quoted RFC 3339 string ending in `Z`. On read, parse RFC 3339, convert to UTC with that formatter, and require byte equality with the input. Reject alternative but equivalent spellings such as `+00:00`, missing milliseconds, or excess fractional precision.
7. Emit LF only and exactly one final newline. `.gitattributes` already fixes `*.toml text eol=lf`.
8. Prepend the same one-line managed comment on every write.
9. If the serialized bytes equal the existing canonical bytes, do not create a temp or replace the file. A real publication normally changes a timestamp; lifecycle cleanup may legitimately be a no-op.

### 4.4 Lossless path representation

The logical path is always relative to the project root and begins with the `.ac` component. It never contains an absolute prefix, drive, UNC share, `.` component, `..` component, NUL, or an empty component. Separators in the manifest are `/`. No Unicode normalization or case folding is applied.

- If every native component is valid Unicode, write the readable project-relative path and `path_encoding = "utf8"`.
- On Unix, if any native component is not UTF-8, encode the complete normalized project-relative native byte sequence as lowercase, two-hex-digits-per-byte text and set `path_encoding = "unix_bytes_hex"`. Insert byte `2f` between components.
- On Windows, if any native component is not valid Unicode because it contains unpaired UTF-16 code units, encode the complete normalized project-relative UTF-16 sequence as lowercase, four-hex-digits-per-code-unit text and set `path_encoding = "windows_utf16_hex"`. Insert code unit `002f` between components.

Encoding is selected canonically: use `utf8` whenever lossless Unicode conversion succeeds, otherwise the platform-native hex form. `unix_bytes_hex` must be lowercase fixed-width hex, use `2f` only as the component separator, and decode to at least one invalid UTF-8 sequence; otherwise `utf8` was the canonical encoding and the row is rejected. `windows_utf16_hex` must be lowercase fixed-width hex, use `002f` as the separator, and decode to at least one unpaired surrogate for the same reason. Decoded components must contain no empty, `.`, `..`, separator, or NUL component. The parser understands and preserves all three known forms on every platform, including a foreign-platform row checked out through Git. A foreign encoded row is metadata only and is never converted into a filesystem target.

Validation and lifecycle prefix filtering operate on a decoded component representation, separately for UTF-8, Unix bytes, and Windows UTF-16, never on serialized string prefixes. Config scope text is always UTF-8 and must have exactly `config:.ac/<validated-wg>/<validated-__agent>/<validated-dest>`: reuse the pure workgroup/replica name rules and `validate_config_seed_dest`, which requires one folder component. Every config row path must be a strict component-wise descendant of its declared scope. Context rows must match one of the two exact allowed UTF-8 path, scope, kind, and builtin-source combinations. Foreign native-hex rows remain filterable by converting the validated UTF-8 scope/prefix components into the row variant and comparing components without filesystem access. Team/agent committed-intent filters parse those validated structured workgroup/replica components; they never use lossy suffix, substring, or serialized-prefix matching.

Hardlinks at two relative paths are two logical rows. Case spelling is preserved so Windows case-sensitive directories are not collapsed. Project move and clone need no owner rebind because the identity is project-relative and the manifest travels with the project.

## 5. Publication outcomes and time semantics

### 5.1 Shared outcome rule

Every included boundary must expose a typed result after the real target commit. The following is a semantic vocabulary, not one catch-all production enum:

```text
Published { project_ac_root, kind, scope, source, files, published_at }
AlreadyPresent | AlreadyCurrent | Observed
Skipped { reason }
Failed { error }
```

`Published` means the target publication syscall returned success and any synchronous rollback owned by that publisher is no longer pending. The boundary that executes that syscall captures `published_at` once immediately after success, before cleanup, directory sync, logging, or return; a caller or recorder is forbidden to call `Utc::now()` after the helper returns and label that later observation as publication time. It is wall-clock display data, not a conflict-resolution key. Do not synthetically increment it on a same-millisecond event or clock rollback. Lock order, not `max(timestamp)`, determines which same-scope publication is later.

Production types remain boundary-specific so callers cannot construct impossible combinations:

| Boundary | Required typed surface |
| --- | --- |
| Create-only context link | `Result<CreateOnlyPublication, String>` where `CreateOnlyPublication::{Published { published_at }, AlreadyPresent}`. The hard-link helper captures the injected clock after the winning `hard_link` and before unlinking its temp. |
| Recognized context replacement | `TemplatePublication::{Published { published_at }, AlreadyCurrent, ChangedUnderUs, Observed, Skipped(reason)}` plus the existing error channel and pending-update data. The atomic-replace primitive captures the injected clock before parent-directory sync or return. |
| Config folder | `ConfigSeedReport::{Published(ConfigSeedPublication), Skipped(ConfigSeedSkipReason), Failed(ConfigSeedFailure), FailedAfterLogicalRemoval(ConfigSeedRollbackFailure)}`. `ConfigSeedPublication.files` is `CollectedSeedFiles::{Exact(Vec<PathBuf>), OverBound { reason, observed_at_least }}`; the last report variant is not publication and can only carry a proven prior-scope removal. |
| Manifest transaction after a target commit | `ManifestRecordOutcome::{Recorded, Unchanged, PublishedUnrecorded(ManifestDegradedReason)}`; `Unchanged` means no canonical byte change, such as a lifecycle no-op or an otherwise identical same-millisecond row. It never erases the target's separate `Published` truth. |

Only a boundary-specific physical publisher invoked while its high-level coordinator holds `ProjectSeedManifestGuard` may construct `Published`; the event carries the timestamp captured at the commit point. A recorder never manufactures the time with its own later `Utc::now()`, and no caller converts generic `Ok(())` into `Published`. Cleanup and specialized-state errors after the commit may make the outer operation return an error, but they cannot erase the carried publication or prevent its immediate manifest attempt.

Internal manifest failures use a `thiserror`-derived `SeedManifestError` with distinct unsafe-path, busy-timeout, lock-capability/I/O, bounded-read, outbound-resource-bound, strict-parse/validation, external-edit-conflict, temp-write/sync, and atomic-replace variants. Automatic flows convert those into `ManifestDegradedReason` and structured logs; existing Tauri command boundaries convert to `String` only at their current IPC edge. Do not flatten lock contention, unsupported locking, corrupt input, over-bound output, and post-target persistence failure into one message or boolean.

A physical replace with identical bytes is a publication and sets time from the new event. In particular, config-seed tiers 1 through 4 replace their destination on every successful spawn, so every file row in that scope receives that event's shared timestamp even if the staged bytes are identical. This normally creates high Git churn; a same-millisecond event can serialize identically. Catalog-default remains absent-only.

### 5.2 Project contexts

`session_context::write_template_if_missing` and its test seam return `Result<CreateOnlyPublication, String>`, with timestamp-bearing `Published` only for the hard-link winner and `AlreadyPresent` for the `AlreadyExists` loser. They must never infer publication from final file existence or equal bytes. An injected clock at the hard-link boundary proves cleanup delay cannot shift the recorded time.

`seeded_context_templates` separates its ownership/hash state from publication events:

- Equal-default observation may still call the existing hash-state `mark_seeded`; it emits no timestamp event.
- `create_missing_template` propagates the create-only outcome.
- `auto_update_generated_template` returns timestamp-bearing `Published`, `ChangedUnderUs`, or failure after its re-read and atomic replace.
- `sync_one_template` returns both its existing pending-update result and an explicit optional publication, so callers cannot collapse an actual update into generic `Ok`.
- `overwrite_context_template_with_default` records only after the backup, final revalidation, and atomic replacement succeed.
- `heal_stale_global_context_template` returns a typed internal outcome instead of logging all branches into `()`; only its successful atomic replacement records, using the time carried from that replace.

For every context mutation, acquire the project gate before the final target re-read, backup, hard-link, or atomic replace, revalidate the pinned `.ac` owner and target parent under that gate, and hold it through the manifest transaction. Acquiring only after receiving `Published` is incorrect: two processes can publish A then B but record B then A, leaving the manifest scope ordered differently from the target. Explicit overwrite returns a retryable busy error before creating a backup or changing the target. Automatic session/discovery flows skip disk publication on timeout and use the existing file or an in-memory default; they never publish outside the gate after known contention.

`read_or_create_context_template` must not fall back to a direct ungated `write_template_if_missing` for either managed project filename when synchronization skipped or failed. The synchronized path owns publication; if it cannot publish and the file is still absent, return the in-memory default. `migrate_legacy_agent_context_template_with` remains an unrecorded user-content migration as excluded in section 3.2.

The manifest update happens after the target publish and before or independently of the specialized hash-state persistence. A hash-state failure never retracts a real publication. A manifest failure never changes hash ownership decisions. Fresh project creation treats every gate acquisition/capability/unsafe-path failure as setup failure and never registers or mutates templates without a working project gate. A caller may roll back a newly created `.ac` only after it actually acquired that gate and while the exact directory identity still matches; a pre-acquisition timeout/error cannot prove another process did not win the new lock, so it leaves the root unregistered for that winner or a later retry rather than deleting it. This preserves the guarantee that a successful setup has both templates without deleting a concurrent setup. Record each successful template immediately; if a later template fails and the gate-owning creator rolls back `.ac`, its manifest disappears too. If cleanup itself fails, any surviving row still names a target that really published, and the next explicit new-project setup re-runs ensure rather than trusting mere `.ac` existence.

Current async session/discovery callers must not block a Tokio worker for the gate's 10-second polling loop. Move the complete synchronous context materialization call into `tokio::task::spawn_blocking` with cloned owned arguments. `discover_ac_agents` and `discover_project_inner` first clone the settings snapshot and drop its `RwLockReadGuard`, then run their synchronous scan/publication work in `spawn_blocking`. No settings or session-manager guard may span a manifest wait.

### 5.3 Config-folder seed

Keep the existing `copy_tree -> Result<()>` wrapper for its coding-agent catalog backup and test callers. Add a seed-specific collecting wrapper over the same internal recursion; it accumulates a `PathBuf` relative to the staging root only after each regular-file copy succeeds while the outbound budgets remain valid. Reparse entries, special files, over-depth branches, and any file at or after a failing copy never appear. On bound overflow it switches irreversibly to the constant-memory `OverBound` summary and continues the physical copy without retaining a truncated list.

`ConfigSeedReport` becomes data-bearing:

```text
Published(ConfigSeedPublication { tier, dest, files, published_at })
CollectedSeedFiles::Exact(paths) | CollectedSeedFiles::OverBound { reason, observed_at_least }
Skipped(ConfigSeedSkipReason)
Failed(ConfigSeedFailure)
FailedAfterLogicalRemoval(ConfigSeedRollbackFailure::PreviousScopeStillStaged { scope, trash_path, install_error, restore_error })
```

The current external fail-soft contract remains: `commands/session.rs` never aborts PTY launch for `Skipped`, `Failed`, `FailedAfterLogicalRemoval`, an over-bound publication, lock contention, or manifest persistence failure.

For a strict workgroup replica, the session chokepoint acquires the existing global `ConfigSeedLockState`, then the project kernel gate, before `perform_config_seed`, and holds both through the one manifest transaction. Move the owned global seed guard, project guard, a cloned `ResolvedConfigSeed`, and owned path data into `tokio::task::spawn_blocking`; neither the runtime worker nor a Tokio state guard sleeps while polling the filesystem lock. After the wait and before source selection or scratch creation, re-resolve the original replica path without following any component, require the same pinned project/`.ac`/workgroup/replica identities, and require the destination parent to remain inside that existing replica. A stale resolved spawn must return a typed skip if deletion, rename, or reparse substitution occurred; `copy_tree` must not recreate a missing replica root through `create_dir_all`. Catalog-default's absent-destination check is made only after this revalidation and uses no-follow metadata. This keeps the existing prefix scratch sweep safe across cooperating processes and orders same-scope physical publication with its manifest update. Measure global-seed wait, project-gate wait, gate hold, and total seed time separately. Drop both guards before PTY spawn. Config seed into Root Agent or another unowned launch root retains only the existing global process-local lock and is not recorded.

After the staging-to-destination rename succeeds, the rename boundary captures one UTC time immediately, before best-effort removal of the old trash, and returns it in the data-bearing publication. For `Exact`, replace the entire `config:<project-relative-dest>` scope with the winning tier, exact installed relative files, and carried time. For `OverBound`, remove the prior scope if the valid canonical state can be written, add no partial rows/time, and return `PublishedUnrecorded(ResourceBound)`. Do not rescan the live destination after the rename. Removing old trash is cleanup, not a second publication and not a row.

Because `config_seed.rs` is already touched, replace silent `let _ = remove_dir_all(...)` on seed temp/trash and stale-scratch cleanup with contextual warning logs that name the owned scratch path and session/scope but not file contents. Cleanup failure does not retract a carried publication or abort PTY launch, yet it must remain diagnosable and reclaimable on the next serialized seed. Manifest temp cleanup, explicit unlock, and directory-sync warnings follow the same no-silent-error rule.

The old-destination staging failure path is also typed. If the old destination was renamed to trash, installing the new stage fails, and restoring trash succeeds, return ordinary `Failed` and leave the manifest unchanged. If restoration fails and the old tree remains at the known trash path while the original logical destination is absent, return `FailedAfterLogicalRemoval(ConfigSeedRollbackFailure::PreviousScopeStillStaged { scope, trash_path, install_error, restore_error })`; under the already-held gate, remove that exact config scope without adding a row or timestamp, then preserve the external fail-soft launch contract. A manifest failure leaves a warned stale row but never converts the failed install into `Published`. Do not infer this outcome from a later scan or generic `NotFound`: it is carried only from this operation's successful old-destination rename and failed rollback. A process death after old-destination rename but before install/restore remains an explicitly documented stale-row crash gap; v1 adds no recovered timestamp or startup reconciliation.

### 5.4 Current-table and lifecycle semantics

V1 contains active rows only. It has no retired tombstones or history rows. Here, current means the last AC-declared membership of a successfully published scope in project history, not a live claim that every target still exists on this machine.

| Event | Exact manifest effect |
| --- | --- |
| Scope publish `{a,b}` then `{b,c}` | In one locked transaction, delete every prior row in that exact scope, then insert `b` and `c` with the new shared time. `a` disappears. `b` is updated, not duplicated. |
| Successful publication of an empty staged tree | Remove every prior row in that scope and add none. Keep an already-existing manifest as a valid empty v1 file. If the canonical manifest is absent and there are no rows to record, do not create a header-only file. |
| Source tier changes for the same destination | Same path and scope rows are replaced; `source` changes and time is set from the publication event. Wall-clock rollback means it is not required to increase. |
| AC removes one replica | After the replica is logically absent, remove every `replica_config_file` row whose config scope is under that replica prefix. |
| AC removes a workgroup | Remove every replica config scope under that workgroup after the user-visible workgroup path is gone. A hidden delete orphan does not keep the old logical scope active. |
| AC deletes a team | After the team directory commit, remove scopes for each workgroup whose directory removal succeeded and for valid matching team workgroup identities already absent at the explicit delete event. Preserve rows for each workgroup whose removal failed and whose original path remains. This is lifecycle intent from `delete_team`, not discovery-based `NotFound` pruning. |
| AC deletes an Agent Matrix and cascaded replicas | After metadata persistence commits, remove every decoded config scope whose validated replica component is that exact `__agent_<name>` in this project, including replicas already absent when the explicit delete began; preserve restored targets on a pre-commit rollback and prune only `StillStaged` targets there. The origin `_agent_*` directory has no v1 rows. |
| Manual file/directory deletion or edit | No change. The manifest records AC publication, not observed existence or current bytes. |
| Missing workgroup during discovery, offline share, access denial | No change and no automatic pruning. |
| Project archive, unarchive, unregister, or re-register | No change. Those operations retain the project and `.ac` on disk. |
| Project clone/copy | Preserve the committed manifest byte-for-byte. Do not invent a new owner id, reset times, backfill, or prune ignored workgroup paths that the clone lacks. |
| Project directory deletion outside AC | The manifest disappears with the project; there is no external cleanup registry. |
| Config `dest` changes | The old destination scope remains because AC did not remove it; the new destination gains a separate scope on its first real publication. |

Lifecycle filesystem ordering uses the same project gate as publication. Perform prompts, dirty checks, path collection, and the first live-session check before waiting, with all settings/session/lifecycle guards released. Then acquire and load the manifest transaction before the final reversible target revalidation, hold the gate across the logical removal, any rollback, and the matching prune, and release only after the canonical manifest attempt. This total order prevents a stale spawn from recreating/recording a just-deleted replica between delete and prune. Cleanup never deletes a row before the corresponding logical-removal commit; a failed rollback reports which targets remain staged and therefore are already logically absent. A lock timeout after known contention returns a retryable delete error before mutation. A pre-contention lock-capability error preserves today's deletion behavior as untracked degradation, but a reparse, identity mismatch, or unsafe canonical path fails closed before a still-reversible deletion. Exact boundaries are:

- `remove_replica_dir` returns a typed `Removed | AlreadyAbsent | Failed` outcome rather than letting `Ok(())` hide absence. `AlreadyAbsent` is constructed only from an exact `symlink_metadata`/remove `ErrorKind::NotFound`; never use `Path::exists`, which collapses access and other inspection errors to false. `cli::team::remove_member` holds the gate across the explicit team-membership commit and removal, then prunes only after `Removed` or `AlreadyAbsent`. A plain `NotFound` observed during discovery never calls this path and never prunes. If replica removal fails after the config mutation, preserve rows because the original replica path still exists or absence was not proven.
- `WgDeleteOutcome` exposes `logical_path_removed()`. Both the GUI backend and `cli::workgroup::remove` acquire the gate before the atomic rename, prune `Deleted` and `Partial` while still holding it, release the gate, and only then convert the structured outcome into their existing outer success/error contract. `Partial` means the original workgroup path was renamed away but hidden-orphan cleanup failed, so rows are no longer active even though the command still returns its partial-delete error. `Blocked` and `Other` preserve rows and release the gate before the GUI awaits blocker diagnostics or any caller formats/returns the error.
- GUI `delete_team` acquires the gate after preflight but before deleting the team directory or any workgroup. It retains exact validated names for removal failures, then in one manifest transaction prunes successful removals plus matching valid team workgroup scopes for which no original path existed at this explicit delete event. A failed workgroup removal keeps its rows. It only collects removed workgroup names while gated; coordinator-clock locking/persistence and discovery refresh occur after releasing the project gate. No settings, clock, or lifecycle guard may be held while polling for or holding the project gate.
- `collect_agent_delete_plan` stores the validated replica manifest prefix in each existing `AgentDeleteTarget` while the original path is still present and canonicalizable, plus the exact validated agent component used for committed-intent filtering. After preflight it acquires the gate before staging and holds it across the live recheck, metadata commit or rollback, and prune. The irreversible normal commit point is successful `persist_agent_delete_metadata`; prune every valid config scope for that exact agent component immediately after that commit and before `remove_staged_agent_delete_targets`, because later hidden cleanup failure does not restore the logical paths and already-missing replicas must not leave permanent ownerless rows.
- Agent-delete rollback becomes structured per target, for example `Restored` or `StillStaged { error }`, rather than one aggregate string. On a stage, live-recheck, or metadata failure before the normal commit point, prune only replica targets whose rollback confirms `StillStaged`; a successful rollback prunes none. The origin `_agent_*` target has no v1 row. The post-gate settings/session work runs inside the same blocking critical section through the synchronous access/bridge defined in section 6.3; the file-lock guard never crosses an async `.await`, and no reverse acquisition is allowed.

If cleanup persistence fails after filesystem deletion, the deletion keeps its existing success/error contract and a structured warning names the stale metadata prefix. Manifest failure never triggers filesystem rollback. There is no safe way to roll back an already completed user-visible delete merely to repair diagnostic metadata.

## 6. Cross-process persistence, lock order, and crash policy

### 6.1 Kernel lock protocol

`ProjectSeedManifestGuard::acquire` uses `.ac/.seed-manifest.lock` and this exact protocol:

1. Resolve the canonical project and `.ac` root without following the final component, open the `.ac` directory no-follow, validate a real non-reparse directory, and retain a stable directory identity for the guard (`st_dev` + `st_ino` on Unix; volume serial + file ID from the opened handle on Windows). Unix uses `O_DIRECTORY | O_NOFOLLOW`; Windows uses `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` and explicit read/write/delete sharing so the exact-identity new-project rollback remains possible while the guard is open. A path spelling or `canonicalize` result alone is not an owner proof. For config seed, ownership must first resolve through `wg_replica_layout_from_agent_dir`, then the original project/`.ac`/workgroup/replica chain is walked and pinned without following reparse components; the strict resolver is necessary but not sufficient by itself.
2. Open or create the lock file with `.read(true).write(true)`, never append or truncate. For an absent file use `create_new(true)`; if another process wins and returns `AlreadyExists`, retry the existing-file no-follow open rather than treating that create race as a lock failure. If an external process removes the name between attempts, retry the create/open state machine within the same deadline. On Windows set `FILE_FLAG_OPEN_REPARSE_POINT` plus explicit read/write/delete sharing through `std::os::windows::fs::OpenOptionsExt` and the already-direct `windows-sys` dependency. On Unix set `libc::O_NOFOLLOW` for every file open and `libc::O_DIRECTORY | libc::O_NOFOLLOW` for directory opens through `std::os::unix::fs::OpenOptionsExt`; add exactly `[target.'cfg(unix)'.dependencies] libc = "0.2"`. Validate the opened handle as regular and non-reparse and retain its stable file identity. This is a no-follow identity dependency, not a locking dependency.
3. Poll `File::try_lock()` on that single, un-cloned handle. Its Rust 1.93.1 signature is `Result<(), std::fs::TryLockError>`, so match `TryLockError::WouldBlock` and `TryLockError::Error(io_error)` exactly. Do not match `io::ErrorKind::WouldBlock`. Independent handles are covered by the lock contract; the unspecified case is calling lock again on the already-locked handle or a clone, which the outer-only/borrowed-guard design forbids.
4. Track `saw_contention`. Only `TryLockError::WouldBlock` proves a competing owner. Use one `Instant` deadline and sleep for `min(50 ms, remaining)` so controlled filesystem polling does not overshoot 10 seconds. At zero remaining, return a typed `BusyTimeout`. A single OS path-open call is synchronous and not cancellable by this loop; the 10-second total-return claim is mandatory only on the responsive local NTFS/Unix conformance filesystems, while opt-in network probes record open latency and make no bounded-return certification for a hung server/mount.
5. A timeout after contention skips an automatic target publication; an explicit overwrite or still-reversible lifecycle command returns a retryable busy error before mutation. `TryLockError::Error` is a typed capability/I/O failure, not contention. If it occurs before contention, automatic context/config publishers retain their existing fail-soft target behavior as `PublishedUnrecorded`, lifecycle commands retain their existing untracked removal behavior, explicit overwrite retains its existing overwrite behavior with a tracking warning, and new-project setup fails as specified in the table below. After observed contention, every path skips or fails before the target because racing is known. Reparse/non-regular paths always fail closed before project target mutation.
6. Immediately after locking, verify by no-follow path reopen that both `.ac` and `.seed-manifest.lock` still name the identities held by the guard. Repeat that check at each bounded commit boundary, not once per staged file: before a context target mutation; before config scratch sweep/staging begins and again before old-destination/install renames; before a lifecycle rename/remove; and before canonical manifest replacement. A mismatch before target commit is an unsafe-path skip/error; a mismatch after target commit is `PublishedUnrecorded`. These checks detect lock-file deletion/replacement and root substitution at the defined failpoints without adding per-file target I/O; they do not claim to defeat a hostile rename in the final instruction-level race after the last check.
7. The guard owns the only lock-file handle and pinned root identity through target publication or lifecycle commit and manifest update. Call `File::unlock()` explicitly, warn if explicit release reports an error, then drop the handle. Never clone or leak it to background work.

Architect dependency decision: **APPROVED**. The direct `libc = "0.2"` declaration is target-scoped to `cfg(unix)` in `src-tauri/Cargo.toml`. Production use is limited to the `O_NOFOLLOW` and `O_DIRECTORY` constants passed to safe `OpenOptionsExt::custom_flags` calls inside `config::seed_manifest`; it adds no raw libc syscall, no lock implementation, no cross-platform dependency, and no use outside the no-follow identity boundary. The standard library exposes `OpenOptionsExt` but not portable named constants for those Unix flags, so this narrow declaration is necessary to prevent symlink traversal at open time. No locking crate is approved.

Only outer publication/lifecycle coordinators acquire `ProjectSeedManifestGuard`. Low-level hard-link, atomic-replace, copy, and recorder helpers accept a borrowed transaction/guard or return typed outcomes; they never reacquire the same project lock. This prevents nested self-locking when `ensure`, read-sync, and self-heal helpers call one another.

Apply degraded outcomes consistently:

| Gate/manifest condition | Automatic context/config publication | Explicit context overwrite | Newly created `.ac` setup | Lifecycle command |
| --- | --- | --- | --- | --- |
| Timeout or later error after `saw_contention` | Skip disk target; use existing/in-memory context or continue PTY without config seed. | Retryable busy before backup/target. | Setup error; before gate ownership leave any just-created `.ac` unregistered, because the contender may own it. After gate ownership, roll back only this call's unchanged created identity. | Retryable busy before a still-reversible logical removal; if reached only after an in-operation failed rollback, preserve that already-authoritative outcome and warn. |
| Lock capability/I/O error before any contention, or validly locked corrupt/future manifest | Preserve the existing primary target behavior but report `PublishedUnrecorded` if it publishes. | Preserve existing overwrite behavior but report success with a structured tracking warning. | Pre-acquisition lock failure is setup error and leaves any just-created root unregistered; a validly locked corrupt/future manifest allows truthful untracked template publication because serialization still exists. | Preserve removal and warn that rows may be stale. |
| Reparse/non-regular lock or canonical path, invalid `.ac` root, or pinned-identity mismatch | Fail closed before a target mutation. | Error before backup/target. | Setup error; rollback is allowed only to a gate-owning creator whose exact identity still matches, never merely because this call won `create_dir`. | Error before a reversible removal; never undo an outcome that became irreversible through a failed rollback, and warn if its metadata cannot be pruned. |

The lock file is persistent and is never unlinked on `Drop`. Kernel ownership is released automatically on normal close and process death, so stale-owner recovery requires no PID, wall clock, age heuristic, or unsafe lock-file deletion. A leftover empty file is normal. This avoids PID reuse and the Unix split-inode race where deleting a locked path lets a second writer lock a new inode. A lock-unaware external deletion/replacement is detected by the handle-versus-path identity checks when it occurs before a defined check; the residual race after the last check is outside the cooperating-writer guarantee and is documented and tested as such.

The guarantee is cooperative among #1038-aware AC processes on a filesystem that implements the OS lock. Rust maps this API to `flock(LOCK_EX | LOCK_NB)` on Unix and `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` on Windows. Mandatory conformance covers local NTFS and the CI Unix local filesystem. Windows SMB3 documents byte-range locks, and modern Linux can emulate `flock` on NFS/CIFS, but behavior varies by server, dialect, mount, and failover mode. UNC/SMB and NFS/CIFS tests are opt-in and must record that environment; passing one does not certify every network filesystem. Unsupported or falsely cooperative filesystems follow the degraded policy and must not be advertised as safe. See the official [`File::try_lock`](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock), [`TryLockError`](https://doc.rust-lang.org/std/fs/enum.TryLockError.html), Windows [`LockFileEx`](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-lockfileex), and Linux [`flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html) contracts.

An external editor, an older AC binary, or a filesystem that falsely reports lock support can violate cooperation; the manifest must never be treated as an authority because of that limitation.

### 6.2 Locked read, merge, and atomic write

The transaction begins after gate acquisition, before a guarded target mutation:

1. Remove only sibling temps whose filename parses exactly as `.seed-manifest.<uuid>.tmp` and whose UUID round-trips byte-for-byte through lowercase hyphenated `Uuid::parse_str(...).hyphenated()`. Open each candidate no-follow, compare the path identity with the opened regular-file handle, and remove only that unchanged regular, non-reparse identity. A directory, symlink, junction, reparse point, swapped identity, or malformed lookalike is left untouched and warned. With the project gate held, a matching unchanged regular temp from a cooperating writer is stale.
2. Load one `CanonicalSnapshot` before target mutation. Open the canonical path no-follow, validate the opened handle as regular/non-reparse, retain its identity, and read at most 128 MiB plus one byte. `NotFound` is an explicit absent snapshot and an empty v1 state; no other failure means empty. Preserve the exact raw bytes as well as the one parsed/validated internal state.
3. If the snapshot is valid, apply exactly one pure mutation after the carried outcome's commit boundary: single-row upsert or complete-scope replacement for an exact `Published`; prior-scope removal with no insert for an over-bound `Published` or `FailedAfterLogicalRemoval`; or validated prefix/committed-intent removal for a lifecycle command. The mutation reports whether the row set changed. An ordinary `Skipped`/`Failed`/observation performs no mutation. If a permitted removal or empty-scope replacement changes no row, return `Unchanged` without serializing or creating a header-only manifest.
4. Compute the exact canonical output length with checked arithmetic before allocating the sorted wire vector or TOML string. If it exceeds any bound after an exact publication merge, drop the unrecordable new batch, retain only the pure removal of that publication's prior scope if any, recompute, and report `PublishedUnrecorded(ResourceBound)` after at most that removal write. Otherwise sort and serialize deterministic bytes only for a logical row change. If they equal an existing canonical snapshot, return `Unchanged` without creating a temp. A real target publication normally differs when its formatted timestamp or another field differs; a same-millisecond event can be a no-op.
5. If bytes changed, create `.seed-manifest.<uuid>.tmp` with no-follow `create_new(true)`, retain its file identity, write all bytes, flush, and `sync_all` the temp handle. Keep that one handle open through publication on both platforms; Windows opens explicitly include `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, and Unix rename semantics preserve the open inode. There is no close-and-reopen fallback. Require the same identity, exactly one hard link, and exact expected length/bytes immediately before replacement; abort if any changed so an external alias cannot mutate the newly canonical inode behind the gate. Existing canonical handles on Windows use the same three share flags and remain open through the checked replacement.
6. Immediately before canonical replacement, revalidate the pinned `.ac` and lock identities and stream a second no-follow raw comparison of the canonical path against the retained initial bytes through a fixed-size buffer. Require exact byte equality and the same canonical file identity for an existing snapshot, or require that it is still `NotFound` for an absent snapshot. If a lock-unaware editor changed or replaced it, abort with `PublishedUnrecorded` and preserve the editor's path. This is a raw conflict check, not a second TOML parse or a second manifest-sized allocation.
7. Use a manifest-specific same-directory atomic replacement wrapper selected by the retained initial snapshot, with no `Path::exists()` branch. On Unix, call `rename` on the already-validated sibling temp and canonical entries. On Windows with an absent snapshot, call [`MoveFileExW(MOVEFILE_WRITE_THROUGH)`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw) without `MOVEFILE_REPLACE_EXISTING`, so a newly appeared destination fails as an external-edit conflict. On Windows with an existing snapshot, call [`ReplaceFileW(REPLACEFILE_WRITE_THROUGH)`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-replacefilew) after the exact identity/raw checks. Implement these calls in `seed_manifest`; do not call or widen `root_agent::atomic_replace_existing`, whose path-only contract remains unchanged for its existing callers.
8. After a successful Unix rename, call `sync_all` on the already pinned `.ac` directory handle and warn on failure. Windows durability is provided by the mandatory `MOVEFILE_WRITE_THROUGH` or `REPLACEFILE_WRITE_THROUGH` flag, so no unsupported directory-open fallback is attempted there. A post-replace durability warning never permits target republication or manifest rollback.
9. Clean the temp on every pre-replace failure. Never delete, quarantine, or rewrite a corrupt/future canonical file.

One directory seed performs one initial canonical parse/validation, one pure in-memory merge, at most one streaming raw pre-replace comparison, and at most one temp-fsync/replace sequence for the entire file set. It must never parse, fsync, or replace the manifest once per row. A noncooperating editor can still race after the last identity/raw check and before the path-based atomic replace, including by substituting the `.ac`, lock, temp, or canonical entry. The gate guarantees cooperating AC writers, not arbitrary external writers; every detectable substitution fails closed, the irreducible final race is documented, and the file remains non-authoritative.

One guard may host sequential transactions without reacquiring its own lock. A coordinator that can publish both project contexts completes the target-plus-canonical attempt for each timestamp-bearing `Published` outcome before starting the next target, refreshing its in-memory/raw snapshot after a successful manifest replace. It must not batch both rows until the outer loop returns, because a later context error on a pre-existing project cannot erase an earlier surviving publication. Config scope replacement and multi-prefix lifecycle cleanup remain one transaction apiece.

### 6.3 Lock order

To prevent deadlocks and runtime starvation, the global order is:

1. Perform prompts, Git checks, network work, initial live-session checks, and read/clone/prepare required settings or lifecycle state; release every guard.
2. Acquire the process-local `ConfigSeedLockState` only for config seed.
3. Acquire `.seed-manifest.lock` in blocking work and load the canonical snapshot.
4. Revalidate the pinned owner/target. Only the new-project registration commit and Agent Matrix delete's live-recheck/metadata protocol may then acquire `SettingsState` or consult the session manager, always in the direction project gate -> state lock and wholly inside the same `spawn_blocking` critical section. New-project settings uses `SettingsState::blocking_write` for its synchronous refresh/save. Agent delete gets a dedicated blocking metadata variant and a `SessionManager::list_sessions_blocking` snapshot that uses `blocking_read` on the existing outer/inner Tokio locks in the same sessions-then-order sequence as `list_sessions`; it does not call `Handle::block_on` or run an async future under the file lock. Every path that previously held either state guard while entering a manifest-aware filesystem helper is split first, so the reverse direction does not exist.

No settings, session-manager, coordinator-clock, local-config, or lifecycle guard may be held while waiting for the project gate. No code may acquire `ConfigSeedLockState` while holding the project gate. `ProjectSeedManifestGuard` and every synchronous/Tokio lock guard are forbidden across an async `.await`; the two narrowly enumerated state commits run to completion in blocking work under the audited order above. Config seed releases the global seed mutex and project guard before PTY spawn. Lifecycle commands acquire the project guard before their final logical target mutation and retain it through prune.

Cancellation behavior is fixed. Once a `spawn_blocking` closure starts, dropping or aborting its awaiting async request does not cancel that closure and no cancellation seam is consulted between gate acquisition and the target, metadata, rollback, or manifest commit. The closure runs the already-started critical section to its typed completion, performs owned cleanup, explicitly unlocks, and then returns; only outer UI refresh or response delivery can be lost. A queued blocking task cancelled before it starts performs no filesystem or state mutation. This rule applies to config seed, new-project setup, context materialization, and lifecycle work.

`new_project_inner_with_settings_path` currently calls synchronous project filesystem setup while holding `SettingsState` for the entire `mutate_project_paths_with_settings_path` closure. Split it with an explicit same-project preparation protocol, not a naked unlock/relock: create or locate `.ac`, acquire/pin its project gate, and while holding it run `ensure_project_context_templates` with the borrowed guard for both newly created and pre-existing `.ac` roots so a prior partial cleanup cannot be registered as complete. Only then acquire `SettingsState` in the order above, re-refresh the disk-authoritative project list, and upsert/save. Two same-project callers therefore serialize on the project gate; the second revalidates after the first finishes and cannot register a directory the first is rolling back. Winning `create_dir` records intent but not rollback authority: if this caller never acquires the gate, it leaves the root unregistered and does not delete a possible contender's setup. After acquiring the gate, a later setup failure may roll back only a `.ac` created by this call and only while its pinned directory identity still matches; test and support this exact-identity removal with the open lock handle on Windows. A waiter whose held lock identity became detached by that rollback drops it and returns a retryable setup error, never operating on a replacement root. A settings-save failure after successful template setup preserves the valid `.ac` and truthful rows, matching today's non-transactional project-registration behavior, but returns registration failure. This preserves both project-path concurrency and the successful-new-project template guarantee.

The project gate must not be held during Git operations, repo dirtiness checks, network clone work, user prompts, or PTY operations. Config and context publishers revalidate their target owner after every gate wait; a precomputed `ResolvedConfigSeed`, context directory, or deletion plan is intent only and never permission to recreate or mutate a path that disappeared or changed identity while waiting.

### 6.4 Corrupt, unsupported, and future manifests

- Missing canonical file: start empty, forward-only.
- Valid v1 with exact coverage: merge normally.
- `schema_version > 1`, unknown top-level/row fields, or a different coverage contract: preserve bytes and disable this writer. An old binary must not strip future data.
- Invalid TOML (including Git conflict markers, duplicate TOML keys/tables, and scalar/table redefinition), invalid UTF-8, duplicate identity, mixed source/time within a config scope, invalid path encoding, traversal-shaped path, malformed timestamp, kind/scope/source mismatch, or a size/row/field bound violation: preserve bytes and disable this writer. Do not fall back to `{}` and do not quarantine or rename the only copy automatically.
- Canonical path is a directory, symlink, junction, reparse point, or another non-regular type: do not follow or replace it.
- A target publication may still succeed while a locked manifest is read-only because its canonical bytes are corrupt or future-versioned. Log `PublishedUnrecorded`; do not synthesize a time later.
- Persistent degraded conditions use a process-local warn-once key of manifest path plus error class, with later repetitions at debug level. A post-publication write failure names the affected scope and states that the target succeeded but metadata did not.

### 6.5 Target-to-manifest crash gap

The target and manifest cannot be one filesystem transaction. V1 deliberately chooses no false positives over guaranteed completeness:

| Crash/failure point | Required state after restart |
| --- | --- |
| Before target publish | Old target and old manifest; no new row/time. |
| Target publish syscall fails without a committed logical removal | Old manifest; no new row/time. Publisher follows its existing rollback/skip/failure behavior. |
| Config old destination renamed to trash, process dies before new install or restore | Original config destination may be absent and the prior scope rows may be stale; no new row/time is created. The hidden old tree may remain until the existing scratch cleanup runs. V1 does not infer success, restore from the manifest, or invent a timestamp. |
| Config install fails and old-destination restore succeeds | Old target and old manifest; no new row/time. |
| Config install and restore both fail while this process survives | Return typed `FailedAfterLogicalRemoval(PreviousScopeStillStaged)`, remove the old scope in the already-held transaction, and add no row/time. If that prune itself fails, warn that old rows are stale; never call the failed install `Published`. |
| Target succeeds, process dies before manifest replace | New target may exist with old or missing manifest. Do not infer or backfill. Kernel lock is released automatically. |
| Manifest temp write/fsync fails | Target remains successful; canonical manifest remains old; temp is cleaned now or by the next locked writer. |
| Lock-unaware editor changes canonical bytes after the initial snapshot | Raw pre-replace comparison aborts metadata publication; target remains successful and is reported `PublishedUnrecorded`. |
| Canonical atomic replace succeeds, then process dies | The new row is truthful because target success happened first. Canonical bytes are either the prior valid file or the complete new valid file, never a partial TOML document. |
| Parent-directory sync fails | Keep the successful target and canonical replacement, emit a durability warning, and do not republish the target. |
| Lifecycle path becomes logically absent, process dies before prune | Stale rows may remain. Discovery and generic `NotFound` do not reconcile them; only a later explicit lifecycle event or actual scope publication can change them. The gate prevents a cooperating publisher from interleaving between the logical removal and prune while the process remains alive. |

No pending intent, startup reconciliation, or backfill is added in v1. For create-only files and byte-identical replacements, a recovered intent plus final byte equality cannot prove which process won. Ownership hashes, mtimes, Git history, and observed existence do not add that proof. Marking it committed would invent a potentially false timestamp. The next real publication heals a context row or replaces a config scope with its new actual timestamp; an `AlreadyPresent` or equal-default observation does not.

For config seed, every manifest error remains fail-soft for launch. The PTY starts. If the project gate timed out behind another cooperating writer, seeding is skipped rather than racing that writer. If the target already published and manifest persistence then failed, do not turn the launch into an error and do not retry the seed.

## 7. Compatibility, security, privacy, Git, and performance

### 7.1 Compatibility

- Existing installations start with no invented rows. Existing context files and ownership state are not backfilled.
- Pre-#1038 binaries ignore the new file and can publish without updating it. On downgrade, the manifest remains but may become stale. Re-upgrade still does not infer missing history.
- Future schema or coverage versions are preserved byte-for-byte by v1 writers.
- There is no dependency on #958 and no schema migration between the two files. If #958 later generalizes ownership outcomes, its typed `Published` signal may feed this recorder, but its hashes, backups, classifiers, and overwrite decisions remain independent.
- A `Published` value is evidence emitted after an independently authorized write, never a capability fed backward into overwrite/delete authorization. Removing the manifest recorder or forcing it to fail must not change any #958 hash/classifier/backup decision or target bytes.
- Windows paths, UNC/network volumes, and filesystems without working locks follow the typed degraded policy. A lock-unsupported filesystem cannot claim the multiprocess guarantee.

### 7.2 Security and privacy

- Treat the manifest as untrusted project content. A valid-looking forged row can affect only diagnostic text on the next merge; it cannot cause stat, read, write, overwrite, repair, or delete outside the publisher's independently validated target. Project, `.ac`, lock, workgroup, replica, temp, and canonical identities are derived and revalidated from opened no-follow handles, never from a row or stale path spelling.
- Lifecycle filtering is pure metadata filtering against a validated scope prefix. It does not join a manifest row to the filesystem.
- Reject traversal and absolute identities. Do not call `canonicalize` on row-supplied paths and do not follow row-supplied reparse points.
- Existing context byte/hash checks remain the only permission for recognized automatic overwrite. A manifest row is never consulted for that decision.
- Commit only project-relative destination names and coarse source enum. Do not commit absolute project/source/config-dir paths, account names, machine ids, content hashes, file sizes, tokens, or file contents.
- Logs are local and may use the already-known project path for diagnosis, but must not log file contents or source tree enumeration.

### 7.3 Git behavior

- No-op discovery, registration, archive/unregister, context observation, ordinary failed/skip seed with no committed logical removal, and lost race produce no manifest diff. The separately typed failed-after-logical-removal case may only prune its proven staged-away prior scope.
- A successful config seed from tiers 1 through 4 sets every row to the spawn's shared millisecond timestamp. This normally creates a large but truthful diff; a same-millisecond identical state is byte-stable. Both behaviors are explicitly documented.
- A complete scope replacement makes removals reviewable: `{a,b} -> {b,c}` shows `a` removed, `c` added, and `b`'s time changed whenever the formatted event time differs.
- Array rows are sorted, so nondeterministic `read_dir` order cannot churn the file.
- User or parent Git rules may still hide `.ac`; AC documents but does not override a parent directory exclusion.
- A user rule later in `.ac/.gitignore`, a repository rule, or a global excludes file can also override visibility. AC neither reorders arbitrary user rules nor claims `!seed-manifest.toml` can defeat an excluded parent. `git check-ignore -v` fixtures document the winning rule.
- Concurrent Git branches can conflict in a high-churn manifest. Conflict markers make the file strict-invalid and therefore read-only to AC until the user resolves the merge; AC preserves the conflicted bytes and continues target publications only under the documented `PublishedUnrecorded` degradation.
- Scale evidence records canonical byte size and `git diff --numstat` added/removed line counts plus diff generation wall time for 1k, 10k, and 100k rows. Every-spawn timestamp churn is a product cost, not an optimization target that may silently change time semantics.

### 7.4 Performance

- Manifest parse, validation, sort, and serialization are O(number of rows); one directory publication holds one in-memory state and performs one canonical replacement.
- The current global process-local config seed lock is retained, so config seeds in this process remain serialized even across projects. The additional per-project lock spans project-owned config staging, swap, and manifest commit to preserve current scratch cleanup and cross-process ordering; it does not serialize context/lifecycle publishers in unrelated projects.
- The 10-second bound covers project file-lock polling on responsive local storage. Total spawn preparation can be longer because it also includes the existing global async `ConfigSeedLockState` wait and staging. The global mutex wait remains nonblocking async; filesystem polling and staging run in the blocking pool. Measure global-seed wait, project-gate wait, and staging separately. A timed-out project gate is a launch-safe config skip.
- Large-tree coverage measures 1k, 10k, and 100k rows for both a small-scope mutation in a whole manifest and a whole-scope replacement. Report initial parse/validate, mutate/sort/serialize, temp write/fsync, raw pre-replace comparison, atomic replace, lock wait, lock hold, total wall time, peak additional working set, canonical bytes, and Git diff time/lines.
- Run end-to-end config staging for 1k, 10k, and 100k regular files and separate staging/swap cost from manifest cost. The 1k and 10k cases establish the growth curve and have correctness gates but no independent time threshold.
- Performance pass/fail accounting is fixed. In an isolated release-mode child on the documented Windows reference machine, each 100k small-scope mutation, 100k whole-scope replacement, and 100k end-to-end config staging plus manifest transaction must individually complete in at most 10 seconds and consume at most 512 MiB of additional working set. Each near-cap valid or invalid parser fixture is measured as its own operation against the same limits. Additional working set means the maximum resident/working-set sample during the operation minus the sample immediately before the operation. Fixture generation, child startup, deliberate 10-second contention waits, and the separate Git command are outside that interval; normal uncontended lock acquisition, parse, staging where applicable, mutation, serialization, fsync, raw comparison, and atomic publication are inside it. Record CPU, RAM, filesystem, Windows build, Rust version/profile, and Git version with the results. Any limit failure blocks Stage F; the limits cannot be raised inside an implementation PR.
- Git diff generation is required observational evidence, not a runtime gate: every 1k/10k/100k fixture must complete successfully and record elapsed time, canonical bytes, and `git diff --numstat`, but it has no numeric pass threshold because Git version and repository configuration are outside the AC writer. This is an explicit accepted product limit, not permission to suppress physical-publication timestamps or truncate rows.
- The plan permits one initial parse and one bounded streaming raw pre-replace comparison. It does not add a second parse, second manifest-sized raw allocation, per-row fsync, hash, destination stat, or source/live-destination rescan. A no-op lifecycle mutation performs no temp write or replacement.

## 8. Affected files and symbols on current main

### 8.1 Production code

| File | Planned symbols and change |
| --- | --- |
| `src-tauri/src/config/seed_manifest.rs` (new) | Strict wire structs/enums; decoded native path codec; batch-invariant row/scope validators; exact `TryLockError` handling; no-follow handle/identity helpers for `.ac`, lock, owner chain, canonical, and temp; `ProjectSeedManifestGuard`; bounded loader and streaming raw conflict comparison; deterministic serializer; `upsert_published_file`, `replace_published_scope`, and committed-intent lifecycle filters; exact temp cleanup and manifest-specific checked atomic writer; injectable clock/filesystem seams for tests; non-constructible production activation token until Stage F. |
| `src-tauri/src/config/mod.rs` | Export `seed_manifest`. |
| `src-tauri/src/config/session_context.rs` | Add timestamp-bearing `CreateOnlyPublication`; change `write_template_if_missing` and its injected-clock seam to capture at the winning hard-link; make `heal_stale_global_context_template` typed; have the atomic context target primitive return the commit-point time before directory sync/return; prevent managed read/create from falling back to an ungated direct write; leave legacy user-content hard-link migration unrecorded. |
| `src-tauri/src/config/seeded_context_templates.rs` | Thread boundary-specific outcomes through `create_missing_template`, `auto_update_generated_template`, `sync_one_template`, `ensure_project_context_templates`, `scan_project_context_template_updates`, `sync_project_context_template_for_read`, and `overwrite_context_template_with_default`; acquire the project gate before final target revalidation/mutation and call the recorder only on `Published`; leave `LoadedState`, `mark_seeded`, hashes, ignored hashes, classifiers, backup, and state schema semantics intact. |
| `src-tauri/src/config/config_seed.rs` | Enrich `ConfigSeedReport`; add `ConfigSeedPublication`, bounded `Exact`/`OverBound` collected-file state, exact skip/failure reasons, the distinct `FailedAfterLogicalRemoval(PreviousScopeStillStaged)` outcome, and an injected-clock seam at the install rename; preserve `copy_tree -> Result<()>` and add a seed-specific bounded path-collecting wrapper; retain reparse/depth behavior; prevent stale resolved seeds from recreating missing replica parents; capture the batch immediately after final directory install. |
| `src-tauri/src/config/projects.rs` | Split new-project filesystem/template preparation from settings upsert under the same-project gate protocol; ensure templates for created and pre-existing `.ac`; return creation intent plus pinned identity and require acquired-gate ownership before rollback can remove this call's unchanged root; handle detached waiters as retryable; retain a valid `.ac` after a later settings-save failure. |
| `src-tauri/src/commands/session.rs` | At the single real-spawn chokepoint, resolve strict ownership, then move owned config-seed data and the global seed guard into `spawn_blocking`; acquire the project guard, revalidate the no-follow owner chain, and refuse a stale/missing replica before seed staging; replace the manifest scope only for `Published`, or remove it for the operation-carried `FailedAfterLogicalRemoval(PreviousScopeStillStaged)`; drop both guards before PTY spawn. Move synchronous context materialization into blocking work. |
| `src-tauri/src/commands/ac_discovery.rs` | Extend `ensure_workspace_gitignore`; clone settings snapshots before scans; move blocking synchronization off Tokio workers; split the new-project async settings transaction around filesystem preparation. `remove_project_inner`, `archive_project_inner`, and `unarchive_project_inner` remain manifest no-ops. |
| `src-tauri/src/commands/entity_creation.rs` | Add logical-removal accessors and per-target rollback detail for `remove_replica_dir`, `delete_agent_matrix`, `delete_team`, `delete_workgroup`, and `WgDeleteOutcome`; acquire the project gate before final logical mutations; retain validated replica prefixes and committed agent/team intent; prune only confirmed logically absent or explicitly retired scopes while preserving failed/restored paths. |
| `src-tauri/src/session/manager.rs` | Add a narrow blocking session snapshot for the Agent Matrix delete critical section, using the same sessions-then-order inner-lock order as `list_sessions`; call it only from `spawn_blocking`, never from an async runtime worker. |
| `src-tauri/src/cli/workgroup.rs` | Inspect `WgDeleteOutcome` before outer conversion; prune `Deleted` and `Partial` logical paths before refresh while preserving the partial-delete error. Manifest failure is a warning. |
| `src-tauri/src/cli/team.rs` | Acquire the project gate after preflight, hold it across team-membership commit and typed `remove_replica_dir`, then remove that replica's scopes for `Removed`/explicit `AlreadyAbsent`. The team configuration and filesystem outcomes remain authoritative. |
| `src-tauri/Cargo.toml` (`Cargo.lock` only if Cargo's generated resolution changes) | Add only the approved Unix-target direct `libc = "0.2"` declaration needed for the `O_NOFOLLOW` and `O_DIRECTORY` constants; add no locking crate. `libc` is already transitive, so a lockfile byte change is not assumed. |

`src-tauri/src/lib.rs` keeps `ConfigSeedLockState`; no new Tauri managed state is required. `config/workspace.rs` remains the strict shape resolver, but `seed_manifest.rs` adds the no-follow identity layer it does not provide. `config/root_agent.rs` and `config/local_config_io.rs` keep their existing callers and semantics; do not weaken or silently broaden `root_agent::atomic_replace_existing` to stand in for the checked manifest wrapper.

### 8.2 Tests and documentation

| File | Planned coverage |
| --- | --- |
| `src-tauri/src/config/seed_manifest.rs` `#[cfg(test)]` tests and child helper | Schema/path codec, canonical timestamp and encoding rejection, duplicate TOML keys/tables, config-scope source/time uniformity, deterministic toml 0.8 golden bytes, strict bounds/adversarial parser-memory/compatibility, no-follow root/lock/temp/canonical identity and atomic conflict seams, streaming raw compare, real child-process lock timeout/death/error/replacement/crash recovery by spawning the lib-test executable's exact ignored helper test, scope replacement, committed-intent lifecycle filtering, and 1k/10k/100k structural benchmarks. The helper and activation-token constructor do not exist in a production library build. |
| Existing tests in `config_seed.rs` | Exact staged list, bounded collector transition to constant-memory `OverBound`, commit-point shared clock despite delayed cleanup, equal-byte physical replacement, source tier, empty scope, reparse/depth omission, stale-owner revalidation, install/restore failure including `FailedAfterLogicalRemoval(PreviousScopeStillStaged)`, and no publication outcome before install. |
| Existing tests in `seeded_context_templates.rs` and `session_context.rs` | Create winner/loser, commit-point clock despite temp/directory-sync delay, equal-default observation, current global v1-to-v2 and coordinator v2-to-v3 generated updates, custom preservation, dismiss, explicit overwrite, self-heal, ungated-fallback prevention, and partial two-template outcomes. |
| Existing tests in `config/projects.rs` and `commands/ac_discovery.rs` | Same-project new-project create/lock/setup/rollback races, gate-ownership plus exact-identity rollback, detached waiters, partial cleanup re-ensure, and settings-save failure after successful setup. |
| Existing tests in `entity_creation.rs` | Successful/blocked/partial workgroup removal, team partial cascade plus already-absent committed intent, Agent Matrix metadata commit including already-missing replicas, complete rollback, partial rollback with `StillStaged`, hidden cleanup orphan, deletion-versus-stale-spawn ordering, and exact retained/pruned prefixes. |
| `src-tauri/tests/cli_workgroup_team.rs` | CLI workgroup and member removal update the manifest only after actual logical removal. |
| `src-tauri/tests/cli_project_registration.rs` | Existing project, archive/unregister, clone, and missing ignored workgroups do not backfill or prune. |
| `docs/features/seed-manifest.md` (new) | Full user-facing contract, schema example, included/excluded scope, millisecond/same-tick and wall-clock semantics, physical-publish churn, clone/manual-delete behavior, failed-restore and crash gaps, strict-invalid Git conflict behavior, trust warning, and Git ignore limitations. |
| `docs/features/config-seed.md` | Link successful replica publications to the manifest and explain normal every-spawn timestamp churn, same-millisecond no-diff behavior, stale-owner skips, `FailedAfterLogicalRemoval`, and fail-soft tracking errors. |
| `README.md` | Add the seed manifest page to the feature index. |

No TypeScript, TSX, shared IPC type, frontend store, new public Tauri command, or frontend-facing command contract is affected. Existing Rust command implementations are changed internally as listed above.

## 9. Child issues and implementation order

This Epic is split into exactly six child issues/PRs in the order below. The first five remain non-emitting; the sixth is the first change allowed to create `.ac/seed-manifest.toml`. This prevents a partial rollout from presenting an incomplete inventory as complete coverage.

### MVP foundation

1. **1038-A: V1 manifest core, path codec, project lock, and atomic writer.**
   - Add the dormant module, Unix-target direct `libc` declaration for the `O_NOFOLLOW` and `O_DIRECTORY` constants, and focused unit tests. Every mutating recorder entry point requires an unforgeable `ManifestActivationToken`; A provides a constructor only under `#[cfg(test)]`, so no production target publisher or lifecycle caller can emit even if it imports the module.
   - Add managed `.gitignore` rules.
   - No production publisher calls the recorder.

### Full feature plumbing

2. **1038-B: Project context publication outcomes.** Depends on A.
   - Refactor create/update/self-heal/overwrite outcomes and managed fallback behavior without acquiring a production gate or enabling manifest writes.
   - Prove every observation and lost-race branch remains a no-publication outcome.
3. **1038-C: Config-seed staged manifest and data-bearing outcome.** Depends on A.
   - Add the seed-specific collecting traversal and data-bearing outcome, preserving the non-collecting `copy_tree` callers, without acquiring a production gate or enabling manifest writes.
   - Preserve current target and spawn behavior.
4. **1038-D: Replica/workgroup lifecycle removal outcomes.** Depends on A.
   - Make successful and partial logical prefixes explicit across GUI/backend/CLI deletion flows, including Agent Matrix per-target rollback status.
   - Do not yet mutate a production manifest.

### Polish and activation

5. **1038-E: Conformance, crash, concurrency, Git, and scale harness.** Depends on A, B, C, and D.
   - Add dormant child-process helpers, failpoints, local and opt-in network-filesystem lock probes, Git visibility checks, adversarial parser-memory cases, 1k/10k/100k benchmarks, and mutation proofs against dormant APIs. The multiprocess helper is an exact ignored `#[cfg(test)]` lib test spawned through `current_exe`; no integration test needs a production token, and no production environment variable, hidden command, or exported constructor can activate it.
   - Do not publish user-facing feature docs or a README link while production emission is still absent.
6. **1038-F: Full-coverage activation.** Depends on A through E and is the only emitting PR.
   - F must be based on the reviewed merge SHAs of A through E; branch protection/checklist evidence records those exact dependencies before review. Introduce the sole production `ManifestActivationToken` constructor together with one exhaustive compile-time v1 coverage declaration naming context create/update/self-heal/overwrite, exact/over-bound config publish, failed-restore, replica/workgroup/team/Agent-Matrix lifecycle, and their required adapters. Removing an adapter makes the coverage declaration or activation acceptance tests fail.
   - Wire all context, config, and lifecycle boundaries in one activation, including async `spawn_blocking` boundaries and lifecycle partial outcomes. Every target publisher and logical remover acquires the gate before target revalidation/mutation; activation must not merely consume a post-hoc `Published` value or prune after an unlocked deletion.
   - Set the fixed v1 coverage header.
   - Add `docs/features/seed-manifest.md`, the config-seed cross-link, and the README feature entry only now that emission is complete.
   - Run the complete acceptance matrix and submit the implementation evidence for acceptance against this certified plan. Stage F does not reopen schema, timing, lock, dependency, lifecycle, or performance decisions.

There is no partial `coverage = ["contexts"]` or similar production state. `coverage_version = 1` means both listed publisher families and all specified lifecycle hooks are active. A future publisher-family change must use a new coverage version and a separately reviewed migration/compatibility plan.

Extras are explicitly none. A query command, UI, history log, instance manifest, or #958 migration requires a separate product decision.

## 10. Test, failpoint, mutation, and benchmark plan

### 10.1 Core schema and deterministic bytes

1. Golden empty and populated v1 TOML fixtures assert the exact comment, `toml` 0.8 field order, row order, escaping, UTC milliseconds, LF, and final newline.
2. Two equivalent states inserted in different orders serialize byte-identically.
3. Duplicate path identity, duplicate TOML scalar/table declarations, scalar/table redefinition, mixed source or time inside one config scope, invalid component-wise scope membership, traversal, absolute paths, malformed/non-lowercase hex/code units, native-hex that could canonically be UTF-8, equivalent noncanonical timestamps, unknown field, wrong coverage, future schema, conflict markers, corrupt TOML, and every resource-bound violation are rejected without changing canonical bytes.
4. The 128 MiB plus one-byte seam proves bounded pre-read rejection. Row and 256 KiB field caps are separately exercised during/post typed parse without claiming a stronger `toml` parser allocation bound. Near-cap valid and invalid adversarial inputs run in an isolated measurement child and must remain within 512 MiB additional working set; the production parser uses the same code path. The second raw comparison uses a fixed-size buffer and counters prove it allocates no second manifest-sized byte vector.
5. Valid UTF-8, Unix invalid-byte, and Windows unpaired-UTF-16 paths round-trip without collision or lossy conversion on every host. Two Unix names that collapse under `to_string_lossy` remain distinct, and foreign rows participate in pure component-wise prefix pruning without materialization.
6. `.ac` root, owner-chain, lock, canonical, and temp paths reject symlink, junction/reparse, directory, special-file, and handle/path identity substitutions. A temp with a second hard link is rejected before publication. Temp cleanup accepts only an exact UUID filename whose opened identity still matches at removal. Failpoints replace/delete/link each name before and after its defined identity check and prove detectable substitutions fail closed without outside-tree I/O.
7. Hardlink paths and case-distinct spellings remain separate logical identities.
8. For control characters, quotes, backslashes, non-ASCII text, native hex, minimum/maximum timestamps, and every enum, the checked outbound byte counter equals the actual pinned `toml` 0.8 serialization length. Checked-arithmetic overflow and an estimated 128 MiB plus one byte are rejected before wire-vector/string allocation.

### 10.2 Publication truth

1. A manually created exact-default context followed by scan/read/ensure does not create or refresh a row. This kills timestamping in `mark_seeded`.
2. Two create-only context writers race. Only the hard-link winner records; the `AlreadyExists` loser does not. Advance the injected clock during winner-temp cleanup and prove the row retains the instant captured immediately after `hard_link`.
3. Global context publishes and coordinator fails. Record only the durable global publication unless fresh-project rollback removes the entire `.ac`.
4. Current global v1-to-v2 and coordinator v2-to-v3 recognized updates, explicit overwrite, and legacy self-heal record after atomic replace; changed-under-us, custom, dismiss, read-only, and injected publish failure do not. Delay parent-directory sync/return while advancing the clock and prove replacement rows retain the commit-point instant. Legacy `Context.agent.md` hard-link migration itself never records.
5. Config source `{a,nested/b,reparse,over-depth}` records only `a` and `nested/b` from the installed stage.
6. Inject failure on the Nth staged file, and final-install failure followed by successful restore. Destination and manifest remain unchanged. Separately fail both install and restore: return `FailedAfterLogicalRemoval(PreviousScopeStillStaged)`, leave the old tree at the reported trash identity, remove only the old manifest scope, add no time, and preserve PTY fail-soft behavior.
7. A successful identical-byte directory swap with the injected clock advanced by at least one millisecond updates every row's time. A same-millisecond case is byte-stable and does not invent a later time. A content-change-only implementation fails the first test.
8. `{a,b} -> {b,c}` removes `a`, updates one `b`, adds `c`, and uses one shared time and one manifest replacement.
9. An empty successful tree removes the prior scope without deleting unrelated rows.
10. An empty successful tree against an absent/header-only manifest and a lifecycle prune with no matching row perform no write and never create a header-only file from absence.
11. Pause two same-context publishers after gate acquisition and prove target order and manifest order agree. An implementation that acquires the gate only after a post-hoc `Published` outcome fails.
12. A busy automatic read/create returns the existing file or in-memory default without calling direct `write_template_if_missing`; explicit overwrite returns busy before backup. A fresh-project caller that has not acquired the contested gate does not register or delete the newly created root; a gate-owning creator may remove only its unchanged identity after later setup failure.
13. Config publication time is captured immediately after stage-to-destination install and remains the row time when old-trash cleanup is delayed or fails.
14. Two concurrent `new_project` calls for the same path pause before lock acquisition and around setup failure/rollback. A creator that loses the lock never deletes the winner's root. A gate-owning creator may roll back only its unchanged identity; a waiter on the now-detached lock returns retryable without touching a replacement. After a surviving partial cleanup, a later call re-runs template ensure under the gate. A settings-save failure after complete setup leaves the valid `.ac` and truthful rows but returns failure.
15. Pause a stale resolved config seed until its replica is renamed/deleted, then release it after gate acquisition. It skips without recreating the replica parent, staging scratch, publishing, or changing the manifest.
16. Cross the path-count, field, prospective-scope-byte, and full-merged-output bounds one at a time. The collector drops its accumulated list and stays constant-memory after `OverBound`; the physical directory publication still completes, the prior scope is removed if writable, no truncated row set/time is emitted, and the event is reported `PublishedUnrecorded(ResourceBound)`.

### 10.3 Lifecycle and clone behavior

1. `remove_replica_dir` `Removed` and explicit-member exact-`NotFound` `AlreadyAbsent` prune only that replica after team membership commit; access denied, metadata I/O, `Failed`, and discovery `NotFound` do not. Replacing the exact error match with `Path::exists` makes the test fail.
2. Workgroup `Deleted` prunes every replica scope under it and leaves contexts and other workgroups.
3. Workgroup `Blocked`/`Other` changes nothing. `Partial` prunes because the original logical path is gone while GUI and CLI retain the partial-delete error.
4. Team delete prunes workgroups whose direct removal succeeded plus valid matching team scopes already absent at this explicit delete, using one metadata transaction; it preserves every named workgroup whose removal failed and original path remains.
5. Agent delete prunes every valid config scope for the committed exact agent identity immediately after metadata persistence commits, including replicas already missing when collection ran, even if hidden cleanup later fails. Complete pre-commit rollback preserves all rows; partial rollback prunes only targets reported `StillStaged`.
6. Manual file/dir deletion, discovery of a missing/offline workgroup, archive, unregister, and re-register leave the manifest byte-identical.
7. Copy a project with manifest but without ignored `wg-*` paths, register it, and discover it. The committed rows and times remain unchanged.
8. Pause a same-project publisher and workgroup/replica/Agent-Matrix remover at the gate. Whichever acquires first determines both target and manifest order: publish-then-delete ends pruned; delete-then-stale-publish revalidation skips and cannot recreate the owner. Moving lifecycle gate acquisition after rename makes the test fail.

### 10.4 Multiprocess and crash failpoints

1. Two child processes pause after canonical read and update different rows. Both rows survive. Removing the OS lock makes the test fail by lost update.
2. Two same-scope child publishers serialize target publish and manifest replace; the later lock holder's complete scope is final, independent of wall-clock ordering. Two independent handles in one process also contend normally, while an injected nested call on the already-locked handle/clone is structurally impossible because low-level helpers require the borrowed guard.
3. A child exits while holding the file lock. The next child acquires without deleting the persistent lock file, proving kernel stale-owner recovery.
4. On mandatory responsive local NTFS/Unix storage, a live kernel lock holder forces the second operation to return within the 10-second polling deadline, never hang, and never publish a project target after timeout. Network probes separately report any uninterruptible open latency without claiming the same bound.
5. Inject `TryLockError::Error` before contention and after an observed `WouldBlock`; prove the first retains automatic target behavior as untracked while the second skips. A fake `io::ErrorKind::WouldBlock` matcher fails compile/test review against the exact enum seam.
6. Kill at: before target; after config old-destination rename/before install; after target/before manifest; after temp fsync; after canonical replace; and before return. Inject surviving install+restore failure separately. Assert every row of the exact table in section 6.5.
7. An orphan manifest temp is removed by the next locked writer; a reparse or directory with a temp-looking name is left untouched and warned.
8. Future/corrupt canonical content remains byte-identical while a target may report `PublishedUnrecorded`; no later observation invents its time.
9. A lock-unaware writer changes canonical bytes after the initial snapshot; the raw comparison preserves those bytes and reports the successful target unrecorded.
10. A lock-unaware process deletes/replaces the lock path or swaps the `.ac`, canonical, or temp identity at each failpoint. Checks before target mutation fail closed; checks after a successful target return `PublishedUnrecorded`; a final post-check race is explicitly classified outside the cooperative guarantee rather than asserted impossible.
11. A Tokio ticker and unrelated project publication remain responsive while another project spends the full lock timeout in `spawn_blocking`.
12. Cancel/drop config, new-project, context, and lifecycle requests before a queued blocking task starts and after its blocking critical section starts. The pre-start task performs no mutation. The started closure ignores outer cancellation until its typed completion, releases every lock, performs required rollback/temp cleanup, and never leaves a detached task holding the project gate indefinitely. Post-return UI events may be skipped, but target/metadata ordering remains complete. A mutation that inserts an in-critical-section cancellation branch must fail this test.
13. Run mandatory multiprocess cases on local NTFS and CI Unix storage. Gate opt-in UNC/SMB and NFS/CIFS probes by environment variables and record platform, server/dialect or mount type/options, and result without broad certification.

### 10.5 Git and performance

1. In a temporary Git repo with only AC-managed `.ac/.gitignore`, `git check-ignore` proves root `seed-manifest.toml` visible and root lock/temp paths ignored, while nested `.seed-manifest.lock`, `.seed-manifest.<uuid>.tmp`, and `seed-manifest.toml` lookalikes remain unaffected by the anchored rules.
2. A parent `.gitignore` containing `.ac/` still hides the manifest, proving and documenting the limitation.
3. A later same-file rule, repository exclude, and global exclude can hide the manifest; `git check-ignore -v` identifies the winning user rule. A merge-conflict fixture is preserved byte-for-byte and disables manifest writes until resolved.
4. Counters prove one initial canonical parse, at most one streaming raw pre-replace read with fixed buffer, one temp fsync, and one canonical replace for 1k, 10k, and 100k scope replacements, never one per row. A no-op lifecycle filter writes none.
5. Release-mode benchmarks on documented Windows hardware run small-scope and whole-scope mutations at 1k, 10k, and 100k total rows, end-to-end config staging at all three sizes, and isolated near-cap parser fixtures. Apply the exact per-operation interval, 10-second, and 512-MiB gates from section 7.4. Record every listed measurement; Stage F is blocked rather than silently raising either limit if evidence fails.
6. End-to-end 1k/10k/100k tree tests confirm config staging remains the only per-file target I/O and the manifest transaction count stays one.
7. For each size, require a successful `git diff --numstat` and record serialized byte change and diff-generation time so the every-spawn timestamp cost is reviewable. Apply the explicit observational, no-numeric-threshold decision from section 7.4.

### 10.6 Required mutation proofs

The suite must fail if an implementer:

- records before target publication;
- captures `published_at` in the recorder or after a target helper's cleanup/return instead of at the physical commit point;
- acquires the project gate only after a target has already returned `Published`;
- acquires the project gate only after a lifecycle rename/removal, or lets a stale resolved seed recreate a deleted replica while waiting;
- treats every `Ok(())` or equal-default observation as publication;
- timestamps inside context `mark_seeded`;
- lets managed `read_or_create_context_template` bypass a busy/failed synchronized path with a direct write;
- enumerates the source or later live destination instead of the staged winning tree;
- commits a context batch only when the outer call returns `Ok`, losing an earlier surviving publication;
- performs scope upserts without removing omitted rows;
- suppresses a timestamp for an identical-byte physical replace;
- prunes on `Path::exists`, discovery, clone, archive, or unregister;
- keys rows with absolute, lowercased, or lossy paths;
- compares encoded scope prefixes as strings or materializes a foreign native-hex row;
- resolves a conflict with `max(last_seeded_at)`;
- uses atomic rename without a locked reload-and-merge;
- matches `io::ErrorKind::WouldBlock` instead of `TryLockError::WouldBlock`, deletes the persistent lock file, relocks a cloned handle, or sleeps on a Tokio worker;
- uses production `unwrap`/`expect` on the lock, parser, path codec, serializer, clock/outcome, or atomic writer error paths instead of typed propagation and diagnostic context;
- carries `ProjectSeedManifestGuard` or another synchronous lock guard across `.await`, uses `Handle::block_on` under it, calls a `blocking_*` state API on a runtime worker, or introduces a reverse state-lock -> project-gate acquisition;
- treats corrupt/future TOML as empty and overwrites it;
- accepts duplicate TOML keys/tables or mixed source/time within one config scope;
- follows or fails to identity-pin a reparse/replaced `.ac`, owner, lock, canonical, or temp path, deletes a temp-shaped directory/link, or reuses the unchecked Windows `Path::exists()` replacement branch;
- prunes from deletion intent, outer `Ok`, discovery `NotFound`, or an aggregate rollback error instead of the structured logical-removal state;
- leaves the old scope active after config install and restore both fail with the prior tree still staged, collapses `FailedAfterLogicalRemoval` into ordinary `Failed`, or calls that failure a publication;
- trusts a manifest row to overwrite, stat, or delete a filesystem path;
- uses a manifest row or `Published` outcome as authorization for a #958 hash/classifier/backup overwrite or delete decision, or changes target bytes when the recorder is removed/fails;
- writes/fsyncs the manifest once per file;
- allocates an unbounded publication row list/TOML string before checking outbound limits, truncates an over-bound installed scope, or leaves that scope's prior rows falsely active after a successful unrecordable replacement;
- backfills from `Utc::now`, mtime, ownership hashes, logs, or Git history;
- exposes a production activation constructor, helper command, or emitting adapter before Stage F, or activates Stage F without the exhaustive v1 boundary declaration;
- emits unanchored `.ac/.gitignore` companion/negation rules that affect nested same-name user files.

### 10.7 Verification commands

Each child PR runs the repository's actual Rust gates from `src-tauri/`:

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib --bins --tests
```

The activation PR additionally runs the ignored multiprocess/crash and 1k/10k/100k benchmark tests with `--ignored --nocapture`, plus the temporary-repository Git checks. No frontend test command is needed because there is no frontend change.

## 11. Objective acceptance criteria

1. The only canonical store is `<project>/.ac/seed-manifest.toml`; no SQLite or instance-global registry is introduced.
2. The exact v1 schema, coverage, canonical encodings/timestamp spellings, enum values, wire order, LF, and final newline match section 4 and the pinned `toml` 0.8 goldens.
3. Every included logical destination has at most one active row.
4. Every AC-generated row/time is downstream of an actual successful target publication by that process, and its time was carried from the physical commit boundary rather than sampled by a later caller or recorder.
5. A second physical publication sets the row to that event's millisecond UTC time instead of appending history, including identical-byte replacement; same-millisecond events may remain byte-identical and never receive synthetic time.
6. Context create, recognized update, self-heal, and explicit overwrite acquire the project gate before target mutation and record only the actual winner; legacy user-content hard-link migration and managed fallback never bypass that gate.
7. Equal-default observation, custom preservation, dismiss, scan, read, skip, failure, and lost race are byte-stable no-ops for the manifest.
8. Config seed records exactly the installed staging tree's regular files with one scope and one shared timestamp.
9. `{a,b} -> {b,c}` yields exactly `{b,c}` for that scope and one row for `b`.
10. AC lifecycle removal follows the exact typed `Removed`/`AlreadyAbsent`, `WgDeleteOutcome`, team committed-intent/partial-loop, Agent Matrix metadata-commit, and per-target rollback boundaries in section 5.4; manual absence, clone, archive, and unregister do not auto-prune.
11. Two cooperating processes cannot lose different-scope updates, and same-scope publication or lifecycle target plus manifest ordering is serialized on a supported lock filesystem. A stale seed cannot recreate a removed replica after waiting for the gate.
12. The kernel lock matches `TryLockError::{WouldBlock, Error}` exactly; outer-only acquisition prevents relocking an already-locked handle or clone; polling times out in 10 seconds without controlled-loop overshoot on mandatory responsive local storage, distinguishes known contention from capability/I/O failure, and recovers after owner death without lock-file deletion. Uninterruptible network path-open latency is environment-labelled rather than falsely covered by the polling bound.
13. Target-before-manifest ordering prevents false publication rows. The unavoidable crash gap is a documented false negative, never backfilled.
14. Corrupt, future, unknown/duplicate-field, mixed-batch, externally changed, over-bound, and non-regular canonical manifests remain byte-for-byte intact; reads and temp cleanup are bounded, identity-pinned, and no-follow, and raw comparison uses no second manifest-sized allocation.
15. Relative paths are lossless or explicitly native-hex encoded; no absolute/source machine path is committed.
16. Traversal, reparse, duplicate, forged, noncanonical native-hex, and foreign rows never cause row-directed filesystem I/O or expand write/delete scope; lifecycle filtering is decoded and component-wise.
17. `.agentscommander-context-templates.json`, its schema, hashes, backups, prompts, and user-edit protections retain their behavior.
18. `.ac/.gitignore` is not a seeded row, keeps `seed-manifest.toml` visible under managed rules, and ignores only the lock/temp companions added here.
19. Existing installs receive no invented timestamps. The first row comes from the first future real publication.
20. A directory publication performs one initial parse, at most one streaming raw conflict read, and at most one write transaction, proven at 1k, 10k, and 100k rows/files; inbound and outbound limits are enforced before manifest-sized allocations, and the documented release, adversarial parser-memory, and Git-diff benchmarks pass.
21. Config-seed manifest failure or contention never aborts PTY launch, and lock polling/staging does not block a Tokio worker or retain settings/session guards.
22. No production manifest is emitted until all `coverage_version = 1` publishers and lifecycle hooks are active; A-E expose no production activation token/helper, and F's exhaustive coverage declaration and dependency evidence are mandatory.
23. Documentation states millisecond/same-tick and wall-clock semantics, normal high churn, manual/clone behavior, Git ignore limits, crash incompleteness, and the non-authoritative security contract.
24. Local NTFS and Unix lock conformance passes. Any UNC/SMB or NFS/CIFS evidence is environment-labelled and makes no broader support claim.
25. The only dependency surface added is the architect-approved Unix-target direct `libc = "0.2"` declaration for the `O_NOFOLLOW` and `O_DIRECTORY` constants; no locking crate or frontend dependency is added.
26. All Rust and special acceptance gates pass with no changes to unrelated untracked plans.
27. Config install plus restore failure is never collapsed into ordinary failure or mislabeled as publication: `FailedAfterLogicalRemoval(PreviousScopeStillStaged)` removes only that scope without a time, while the process-death gap remains documented and unreconciled.
28. Same-project new-project setup is serialized independently of `SettingsState`; no caller can register another caller's doomed/identity-replaced `.ac`, and rollback requires both gate ownership and the exact creator-owned root identity. Merely winning `create_dir` never authorizes deletion after lock contention.
29. The manifest-specific writer pins and rechecks `.ac`, lock, canonical, temp, and strict replica-owner identities. Detectable substitutions fail closed; the final lock-unaware path race is explicitly outside the cooperative guarantee rather than hidden by reuse of an unchecked helper.
30. An over-bound config publication never exhausts memory to build metadata or emits a truncated scope: collection switches to constant-memory `OverBound`, the physical publication remains fail-soft, prior rows for the replaced scope are removed when possible, and no later observation backfills its time.

## 12. Accepted limits and mandatory activation evidence

Consensus assigns every enrichment risk below either a frozen v1 limit or mandatory acceptance evidence. There are no unresolved decisions. Evidence marked mandatory is implemented in Stage E and must pass in Stage F; failure blocks production activation. Accepted limits cannot be hidden by changing publication semantics.

1. **Full config lock span, accepted design.** V1 holds the project gate across config scratch cleanup, staging, destination swap, and manifest commit. The mandatory 1k/10k/100k end-to-end evidence measures that complete span under section 7.4. A two-phase gate or narrower span is out of scope until a separate plan supplies active-operation-safe scratch garbage collection and preserves target/manifest order.
2. **Filesystem lock portability, bounded support claim.** Local NTFS and the CI Unix local filesystem must pass multiprocess contention, independent-handle exclusion, owner death, lock-path replacement, and 10-second timeout tests. UNC/SMB and NFS/CIFS remain opt-in environment-labelled probes only. V1 makes no safety or bounded-open-latency claim for an untested network filesystem; capability failure follows the exact degraded table in section 6.1.
3. **Unix no-follow dependency, approved.** Add exactly target-scoped `libc = "0.2"` for the two flag constants and usage boundary approved in section 6.1. Cargo checks must prove non-Unix builds do not compile or link that target dependency and review must reject raw libc syscalls or any use outside `seed_manifest` no-follow opens. No locking crate is permitted.
4. **Lifecycle partial outcomes and lock order, fixed design.** The gate spans final revalidation through logical removal, structured rollback, and prune at the exact points in section 5.4. Mandatory tests cover team partial cascades, workgroup hidden orphans, Agent Matrix metadata commit, per-target `StillStaged`, project-gate-to-settings/session order, reverse-acquisition rejection, and the fixed run-to-completion blocking cancellation rule.
5. **Foreign native paths, mandatory preservation evidence.** Windows UTF-16 hex and Unix byte hex must round-trip and participate in pure component filtering on the opposite host without filesystem materialization. Golden and mutation tests in sections 10.1, 10.3, and 10.6 are the acceptance evidence; lossy conversion or string-prefix filtering blocks Stage F.
6. **Git churn, accepted product cost.** A physical tiers 1 through 4 config publication updates every scope row when the formatted millisecond changes. The exact benchmark evidence and no-numeric-Git-threshold limit are fixed in section 7.4. Content-change time, timestamp suppression, and row truncation are rejected product changes, not performance fixes.
7. **Crash false negatives and stale lifecycle rows, accepted incompleteness.** The table in section 6.5 is the complete v1 recovery contract. Mandatory kill-point tests and activation documentation cover target create, target replace, logical delete, config old-destination staging, temp fsync, and canonical replace. No startup intent, scan, equality, hash, mtime, Git, or existence backfill is permitted.
8. **Final lock-unaware path race, accepted trust boundary.** Identity and raw checks must catch every injected substitution at their defined boundaries. The instruction-level race after the final check is explicitly outside the cooperating-writer guarantee. The manifest remains diagnostic and never authorizes filesystem action; Stage F documentation must state this limit.
9. **Parser and outbound amplification, hard activation gate.** The 128 MiB input limit is not represented as a parser-memory limit. Production-path near-cap parser children, exact outbound sizing, fixed-buffer conflict comparison, and constant-memory `OverBound` collection must each meet the section 7.4 working-set and time gates. Any failure blocks Stage F rather than relaxing bounds or truncating an installed scope.
10. **New-project rollback authority, fixed design.** Same-project gating, re-ensure for a pre-existing root, gate ownership plus exact creator identity, run-to-completion blocking work, and leave-unregistered behavior for a pre-acquisition loser are mandatory. Windows open-handle exact-root removal, detached-waiter, two-caller rollback, partial-cleanup retry, and settings-save failure tests must pass.
11. **Failed config restore, split surviving/crash policy.** A surviving typed `FailedAfterLogicalRemoval(PreviousScopeStillStaged)` prunes only the prior scope and adds no timestamp. A crash in the old-destination-to-trash window remains the accepted unreconciled stale-row case. The manifest never authorizes trash restoration or cleanup, and failpoints must prove both branches.

## 13. Prohibited substitutions and non-goals

- Do not add time to `.agentscommander-context-templates.json` or replace that file.
- Do not use SQLite, settings JSON, session state, logs, mtime, or Git metadata as the canonical store.
- Do not add a generic filesystem-write hook.
- Do not enumerate sources or live destinations to reconstruct a publication.
- Do not store absolute paths or use `to_string_lossy` for identity.
- Do not lower-case Windows identities or follow reparse points.
- Do not write the manifest before the target.
- Do not sample publication time in the recorder or after a target helper has completed cleanup.
- Do not acquire the lifecycle gate only after the logical target has already been removed, and do not let a stale resolved seed recreate a missing replica owner.
- Do not treat metadata persistence failure as permission to republish a target.
- Do not reuse `root_agent::atomic_replace_existing` as the manifest security boundary without the manifest-specific no-follow and identity checks.
- Do not expose partial coverage under v1.
- Do not add a GUI, CLI query, IPC contract, instance manifest, event history, or #958 migration in this Epic.
- Do not infer readiness from developer or Grinch review alone. The standalone status at the top and the verdict below are the architect's consensus certification; implementation still requires separate authorization for the ordered child stages.

## Grinch Review

1. **What:** Publication time was assigned by the high-level caller after a target helper returned. **Why:** Current create/replace helpers perform temp cleanup and directory sync before return, so delayed cleanup records observation time rather than the physical publication time required by #1038. **Fix:** Every physical boundary now carries an injected-clock timestamp captured immediately after its successful hard-link, atomic replace, or directory-install syscall; callers and the recorder cannot manufacture time.
2. **What:** Lifecycle cleanup acquired the project gate after deletion. **Why:** A stale spawn could publish/recreate a replica between an unlocked delete and later prune, after which pruning would erase the new row or a later stale publisher would resurrect the deleted scope. **Fix:** The same gate now spans final target revalidation, logical removal/rollback, and prune, with post-wait owner revalidation and an explicit global lock order.
3. **What:** Config install plus restore failure was collapsed into generic `Failed`. **Why:** The old destination may remain staged at trash while its manifest rows remain falsely active; a crash after the old-destination rename was also missing from the crash table. **Fix:** Add the distinct `FailedAfterLogicalRemoval(PreviousScopeStillStaged)` outcome, remove only that exact scope without a timestamp in the surviving process, and document the unreconciled process-death gap.
4. **What:** The lock/root/temp/canonical contract relied on path checks and the existing Windows atomic helper. **Why:** Lock-file deletion, root or temp substitution, and `Path::exists()`/path-based replacement can split the protected identity or follow a reparse boundary. **Fix:** Pin opened file identities, recheck them at defined boundaries, stream the raw compare, and use a manifest-specific checked atomic wrapper; retain only the explicitly documented final hostile path race.
5. **What:** Splitting new-project setup out of `SettingsState` removed its accidental same-process serialization and made rollback authority ambiguous. **Why:** One caller could register a `.ac` another caller was about to roll back, while a `create_dir` winner that lost the lock could delete the actual lock winner's setup. **Fix:** Serialize preparation on the project gate, re-ensure templates for created and pre-existing roots, require gate ownership plus exact identity for rollback, and freeze detached-waiter/settings-save-failure behavior.
6. **What:** Strict parsing and output construction did not close batch invariants or memory amplification. **Why:** Duplicate TOML structure and mixed source/time rows could be accepted despite being impossible AC batches; a second 128 MiB raw allocation, adversarial `toml` parsing, or an over-bound newly installed tree could exceed memory before rejection. **Fix:** Reject duplicate/mixed batches, stream conflict comparison, size output before allocation, switch collection to constant-memory `OverBound`, and block activation unless near-cap adversarial cases pass the 512 MiB bound.
7. **What:** Explicit owner deletion could leave permanent rows for replicas already missing when target collection ran. **Why:** Agent Matrix/team commit intent retires those scopes even though discovery `NotFound` alone does not. **Fix:** Use exact validated committed agent/team intent while preserving paths whose removal failed or rollback restored them.
8. **What:** The A-E dormant rollout depended only on convention. **Why:** A helper, recorder import, or out-of-order child merge could emit a partial v1 manifest before all coverage hooks existed. **Fix:** Require a non-constructible production activation token through E, test-only helpers, exact A-E dependency evidence, and one exhaustive F coverage declaration.
9. **What:** The proposed `.ac/.gitignore` rules were unanchored. **Why:** Git would apply `.seed-manifest.lock`, `.seed-manifest.*.tmp`, and the negation to same-named files in nested user directories, broadening the managed ignore surface beyond the three root companions. **Fix:** Anchor all patterns with `/` and prove nested lookalikes are unaffected.

## Verdict

`READY_FOR_IMPLEMENTATION`

The architect certifies this plan as a complete cold-start specification after developer enrichment, adversarial review, and consensus resolution. No open design choice remains. This verdict authorizes no implementation, issue creation, branch work, commit, pull request, or landing by itself; execution is limited to separately authorized stages A through F in their fixed dependency order.
