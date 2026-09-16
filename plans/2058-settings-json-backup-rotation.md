# Issue #2058 - bounded `settings.json` backup rotation

Canonical path: `plans/2058-settings-json-backup-rotation.md` in `repo-AgentsCommander`. Single unit of work.

- Epic / child issue: **#2058** (no child issues; `PARTITION: 1 phase`).
- Branch: `feature/2058-settings-json-backup-rotation`.
- Planning-evidence base SHA (immutable): `f874cc4cbe5cff0f36cc34c56fde85eefe190b6f`.
- Repo root for every command below: `D:\0_repos\AgentsCommander_iac\.ac\room-21-ac-dev-team-v4\repo-AgentsCommander`.
- Owner: Rust implementer (docs rows in the same unit; see Partition decision).
- Class: **patterned**. It mirrors `rotate_orphan_archive`
  (`src-tauri/src/config/sessions_persistence.rs:785-845`) and the
  `ORPHAN_ARCHIVE_ROTATION_GLOB` registry row; no new abstraction.
- Accepted task class / threat model: **routine application-code change**. No release,
  signing, packaging, untrusted host, or supply-chain assurance is in scope. No
  enhanced control from `delivery-nonfunctional-invariants` is applicable; each is
  marked below with its concrete reason.

## 1. Problem

Every successful `settings.json` save replaces the previous file with no recoverable
copy. #2057 showed a real loss: a delete in one coding agent's profile slots destroyed
another agent's configured profiles, and recovery only worked because the user happened
to keep a manual copy of the config directory.

The product must keep a bounded history of previous `settings.json` versions so a user
can be pointed at a previous good copy.

Out of scope: the profile-slot delete guard (#2057, landed); any UI surface; any user
setting to configure retention; restoring a backup from inside the app.

## 2. Evidence (read on the branch at the base SHA)

| Fact | Location |
|---|---|
| `write_value_atomic` is the only byte-writer of `settings.json` | `src-tauri/src/config/settings.rs:5148` |
| Its two call sites | `settings.rs:4621` (production) and `settings.rs:5498` |
| `save_settings_to_path` (the `:5498` caller) is `#[cfg(test)]` | `settings.rs:5486-5498` |
| So the ONLY production write site is `settings.rs:4621`, inside `save_settings_value_locked` | `settings.rs:4445-4621` |
| Production callers of `save_settings_value_locked`, both holding `SettingsFileLock` | `settings.rs:4436` (via `save_settings_value`) and `settings.rs:4872` (compare-and-set, guard alive at `:4922`) |
| `save_settings_value_locked` already reads the previous file's exact bytes, unconditionally, under the lock | `settings.rs:4456` to `read_disk_object_and_contents_for_write_typed` (`:4082`) |
| That read returns `Ok(None)` when absent and `Err` on non-object / invalid JSON, aborting the save | `settings.rs:4086-4130` |
| `write_value_atomic` already verified the directory chain, the target identity and the post-write bytes before returning `Ok` | `settings.rs:5167-5350` |
| One-shot legacy backup `settings.pre-384-v1.json`, refuses to overwrite, and FAILS the save on error | `settings.rs:2620-2631`, called at `:4462` |
| Temp files are `settings.json.<pid>.<op_id>.tmp` | `settings.rs:5232` |
| A test asserts no `settings.json.*.tmp` residue | `settings.rs:5506-5516` |
| Runtime artifacts are declared in ONE registry; a new artifact needs a new row in the same change | `src-tauri/src/config/instance_artifacts.rs:1-19`, `:271` |
| Ignore rows MUST be strictly byte-sorted by `name` | `instance_artifacts.rs:725-737` |
| A row with a wildcard MUST use `ArtifactKind::Glob` | `instance_artifacts.rs:866-882` |
| Rotation-glob precedent, registry row and derivation test | `instance_artifacts.rs:148-150`, `:528-533`, `:928-948` |
| The `.gitignore` rules are DERIVED from the registry; only the test fixture list needs the new names | `instance_gitignore.rs:1-19`, fixture list at `:1001-1097` |
| Rotation precedent: oldest-first renames, failures logged and never fatal | `sessions_persistence.rs:785-845`, keep constant at `:65` |
| Config-dir inventory docs | `docs/reference/directory-layout.md:79-87`, `docs/reference/settings.md:5-24` |

Correction to the assignment's evidence, verified in the tree: `write_value_atomic` DOES
fsync the temp (`temporary.sync_all()`, `settings.rs:5275`) and fsyncs the parent
directory on unix (`:5320-5331`); only the stale doc comment at `:5145-5147` still says
"not fsynced". The temp name is `settings.json.<pid>.<op_id>.tmp`, not
`.settings.json.<pid>.tmp`. Neither changes this design.

## 3. Decisions (all binding; nothing is left to the implementer)

**D1 - Trigger.** Rotation runs inside `save_settings_value_locked`, immediately AFTER
`write_value_atomic` returns `Ok`, and only then. Reason: the previous bytes are already
in hand from the `:4456` read, so a successful save can archive them with no extra disk
read; a failed save leaves the history untouched.

**D2 - Placement, not in `write_value_atomic`.** Rotation is NOT added to
`write_value_atomic`, because its other call site is the `#[cfg(test)]` seeding writer,
which must keep producing a bare config directory for the directory-residue tests.

**D3 - Skip when there is nothing to archive.** No rotation when the previous file was
absent (`disk_read == None`), or when the previous bytes equal the bytes just written.
Reason: startup `root_token` writes, project reconciliation and repeated no-op saves
would otherwise evict real history with identical copies.

**D4 - Location.** Beside `settings.json`, in the same config directory. No new
directory: the directory chain is already identity-verified by the save that just
succeeded, and the existing `settings.pre-*.json` sidecar sets the precedent.

**D5 - Naming.** `settings.backup.<N>.json`, `N` in `1..=5`. `settings.backup.1.json` is
the version replaced by the MOST RECENT save; `settings.backup.5.json` is the oldest
kept. Reason for this stem rather than `settings.json.<N>`: it admits the narrow glob
`settings.backup.*.json`, which cannot collide with `settings.json.lock`,
`settings.json.<pid>.<op_id>.tmp`, `settings.local.json` or `settings.pre-*.json`, and
it keeps the `.json` extension so a user can open a slot directly.

**D6 - Retention = 5.** `SETTINGS_BACKUP_KEEP: u32 = 5`. Bounds worst-case disk at five
times the live file (hard-capped at 16 MiB each by `settings.rs:5220`), and covers a
burst of mis-clicks inside one session, which is the #2057 scenario.

**D7 - Failure behavior: best effort, never fatal.** Any rotation failure logs at
`warn!` and returns; the save still reports success, because the user's bytes are
already on disk and correct. This is deliberately the OPPOSITE of
`write_pre_384_v1_backup` (`settings.rs:4462`), which refuses the write, because that
one guards a one-shot lossy migration whose source disappears, while a rotation slot is
a recovery aid whose source is the file that was just replaced.

**D8 - Non-regular slot = abort rotation.** Before touching a slot, `symlink_metadata`
it; if it exists and is not a regular file, log `warn!` and return without writing.
Reason: never follow a symlink or clobber a directory a user placed there.

**D9 - Permissions.** On unix the slot file is created with mode `0o600` and
`O_NOFOLLOW`; on Windows with `FILE_FLAG_OPEN_REPARSE_POINT`. Same posture as the live
`settings.json` (`settings.rs:5236-5244`, `:5309-5319`), because a slot holds the same
secrets.

**D10 - Concurrency.** Rotation runs with `settings.json.lock` held, because both
production entry points acquire it before reaching `save_settings_value_locked`. No new
lock, no per-slot lock. Cross-process and in-process settings writers are therefore
already serialized with rotation. `save_settings_value_locked` itself does not acquire
the lock and the doc comment must say so.

**D11 - Coexistence.** Rotation never reads, writes, renames or deletes
`settings.pre-*.json`, `settings.local.json`, `settings.json.lock` or any
`settings.json.*.tmp`. The `settings.pre-384-v1.json` backup keeps its current
fail-the-save behavior and runs before the write, so a legacy-shaped first save produces
BOTH the pre-384 backup and `settings.backup.1.json` with the same bytes.

**D12 - Overlay.** The archived bytes are the on-disk base file's bytes.
`local_overlay_state.restore_base` (`settings.rs:4615`) is the last writer over the
output object, so `settings.json` never contains overlay values and neither can a slot.

**D13 - No UI, no setting, no CLI verb.** Nothing is surfaced. The `info!` line emitted
once per rotation names the slot path, and the docs rows tell a user where to look.
Reason: the issue asks for a recoverable copy, and an in-app restore is a larger product
decision that does not belong in this change.

**D14 - `write_value_atomic` returns the bytes it wrote.** Signature becomes
`fn write_value_atomic(value: &Value, path: &Path) -> Result<Vec<u8>, SettingsSaveError>`,
returning `json`. Needed for the D3 equality check without re-serializing. The
`#[cfg(test)]` call site at `:5498` becomes `write_value_atomic(&value, path).map(|_| ())`.

## 4. Change specification

### 4.1 `src-tauri/src/config/instance_artifacts.rs`

Add, next to `SETTINGS_MIGRATION_BACKUP_GLOB` (`:169-172`):

```rust
/// #2058 - the bounded rotation of previous `settings.json` generations. Slot 1
/// is the version the most recent save replaced. The writer composes the index
/// at runtime, so the glob is the only tie between the rule and the artifact.
pub(crate) const SETTINGS_BACKUP_PREFIX: &str = "settings.backup.";
pub(crate) const SETTINGS_BACKUP_SUFFIX: &str = ".json";
pub(crate) const SETTINGS_BACKUP_ROTATION_GLOB: &str = "settings.backup.*.json";
```

Add ONE `InstanceArtifact` row. Its byte-sorted position is **immediately before the
`name: "settings.json"` row** (`:576-581`) and after the
`BLOCKING_MENUS_REMOTE_FILE_NAME` row, because `settings.b` sorts before `settings.j`:

```rust
InstanceArtifact {
    name: SETTINGS_BACKUP_ROTATION_GLOB,
    kind: ArtifactKind::Glob,
    disposition: Disposition::Ignore,
    comment: "# AgentsCommander: rotated previous generations of the application settings; the same runtime artifact under a numeric slot",
},
```

Add one registry test next to `global_context_retired_backup_glob_derives_from_the_context_filename`
(`:949-956`):

```rust
#[test]
fn settings_backup_rotation_glob_derives_from_its_composition_constants() {
    assert_eq!(
        SETTINGS_BACKUP_ROTATION_GLOB,
        format!("{SETTINGS_BACKUP_PREFIX}*{SETTINGS_BACKUP_SUFFIX}")
    );
}
```

### 4.2 `src-tauri/src/config/settings.rs`

Extend the existing `use crate::config::instance_artifacts::{...}` (`:10-13`) with
`SETTINGS_BACKUP_PREFIX` and `SETTINGS_BACKUP_SUFFIX`, keeping the list in the order
`rustfmt` renders.

Add, beside `write_pre_384_v1_backup` (`:2620`):

```rust
/// #2058 - number of previous `settings.json` generations kept beside the live
/// file. Slot 1 is the version the most recent save replaced, slot
/// `SETTINGS_BACKUP_KEEP` the oldest kept; the next rotation drops it.
const SETTINGS_BACKUP_KEEP: u32 = 5;

fn settings_backup_path(settings_path: &Path, index: u32) -> PathBuf {
    settings_path.with_file_name(format!(
        "{SETTINGS_BACKUP_PREFIX}{index}{SETTINGS_BACKUP_SUFFIX}"
    ))
}

/// #2058 - archive `previous` (the bytes a just-completed save replaced) into
/// slot 1, shifting the older slots down and dropping slot
/// `SETTINGS_BACKUP_KEEP`.
///
/// Best effort by design: the caller's save has already succeeded on disk, so a
/// rotation failure is logged and never propagated. Runs with
/// `settings.json.lock` held by the caller, and never touches
/// `settings.pre-*.json`, `settings.local.json`, the lock file or a write
/// temporary. Deliberate local mirror of
/// `sessions_persistence::rotate_orphan_archive`.
fn rotate_settings_backups(settings_path: &Path, previous: &[u8]) { /* per rules below */ }
```

`rotate_settings_backups` body, exactly:

1. For `index` in `1..=SETTINGS_BACKUP_KEEP`, `symlink_metadata(settings_backup_path(..))`;
   if it resolves and `!metadata.is_file()`, `log::warn!` naming the path and `return`
   (D8).
2. For `index` in `(1..SETTINGS_BACKUP_KEEP).rev()` (that is 4, 3, 2, 1): skip when the
   source slot does not exist; otherwise `std::fs::rename(slot(index), slot(index + 1))`.
   On `Err`, `log::warn!` naming both paths and the error, then `return` (D7).
   `std::fs::rename` replaces an existing destination on both unix and Windows, which is
   how slot `SETTINGS_BACKUP_KEEP` is dropped.
3. Write `previous` to `settings_backup_path(settings_path, 1)` through
   `OpenOptions::new().write(true).create(true).truncate(true)`, plus
   `.mode(0o600).custom_flags(libc::O_NOFOLLOW)` under `#[cfg(unix)]` and
   `.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)` under `#[cfg(windows)]` (D9), then
   `write_all`. On `Err`, `log::warn!` and return.
4. On success, `log::info!("[settings] #2058 archived the replaced settings to {:?}", slot_one)`.

Change `write_value_atomic` (`:5148`) to return `Result<Vec<u8>, SettingsSaveError>`:
the success arm returns `json`. Update its doc comment `:5145-5147` to state that the
temp IS fsynced and the parent directory is fsynced on unix, and that the caller owns
backup rotation.

Replace `settings.rs:4621`:

```rust
    let written_bytes = write_value_atomic(&value, path)?;
    if let Some(previous) = previous_contents {
        if previous.as_bytes() != written_bytes.as_slice() {
            rotate_settings_backups(path, previous.as_bytes());
        }
    }
```

Insert the capture immediately before `settings.rs:4477`
(`let disk = disk_read.map(|(map, _)| map);`):

```rust
    // #2058: the exact bytes this save is about to replace, captured before the
    // tuple is reduced to its object half.
    let previous_contents: Option<String> = disk_read.as_ref().map(|(_, c)| c.clone());
```

Update the `#[cfg(test)]` call site `:5498` to `write_value_atomic(&value, path).map(|_| ())`.

Add to `save_settings_value_locked`'s doc comment that it assumes `settings.json.lock` is
already held by the caller and that it performs backup rotation after a successful write.

### 4.3 `src-tauri/src/config/instance_gitignore.rs`

Test fixture only; production rules are derived. In `required_paths` (`:1001-1097`), add
the two edge fixtures in the list's existing byte order, immediately before the
`"settings.json",` entry (`:1079`):

```rust
            // #2058: the first and last rotation slots, the `SETTINGS_BACKUP_KEEP` edge.
            "settings.backup.1.json",
            "settings.backup.5.json",
```

### 4.4 `docs/reference/directory-layout.md`

Add one row to the `### Files` table, directly after the `settings.json.lock` row
(`:82`):

```
| `settings.backup.1.json` .. `settings.backup.5.json` | Bounded history of previous `settings.json` versions; slot 1 is the version the most recent save replaced, slot 5 the oldest kept. Written only when a save actually changed the file. | `config/settings.rs` |
```

### 4.5 `docs/reference/settings.md`

Add a `## Recovering a previous version` section immediately after `## Editing rules`
(after `:24`), stating: the five slots, that slot 1 is the newest previous version, that
identical saves do not rotate, that recovery is a manual copy over `settings.json` while
AC is closed, and that the slots hold the same secrets as `settings.json`.

## 5. Tests (all new, all in the modules above)

In `settings.rs` `mod tests` (`:5504`), each on a `tempfile::TempDir`, driving the real
production entry point `save_settings_to_path_preserving_project_paths`:

1. `issue_2058_save_archives_the_replaced_bytes` - seed `settings.json` with A, save B;
   `settings.backup.1.json` bytes equal A's bytes exactly; no `settings.backup.2.json`.
   Fails before the change because no slot exists.
2. `issue_2058_first_save_with_no_existing_file_writes_no_backup` - absent
   `settings.json`, one save; no `settings.backup.*.json` exists.
3. `issue_2058_identical_save_does_not_rotate` - seed A, save A twice; no slot exists,
   because nothing was ever replaced. Fails if D3's equality check is dropped.
4. `issue_2058_keeps_five_generations_and_drops_the_oldest` - six saves with six distinct
   values; slots 1..=5 exist, `settings.backup.6.json` does not, slot 1 holds the fifth
   value, slot 5 holds the first. Fails for the right reason if `SETTINGS_BACKUP_KEEP`
   or the shift direction changes.
5. `issue_2058_rotation_failure_does_not_fail_the_save` - `std::fs::create_dir` at
   `settings.backup.1.json`, then save; the save returns `Ok`, `settings.json` holds the
   new bytes, and the directory is untouched. Covers D7 and D8 with no production seam.
6. `issue_2058_rotation_leaves_the_pre_384_backup_alone` - seed a legacy-shaped
   `settings.json`, save; `settings.pre-384-v1.json` and `settings.backup.1.json` both
   equal the original bytes, and a second save does not change
   `settings.pre-384-v1.json`.
7. `issue_2058_rotation_leaves_no_temp_residue` - after a rotating save, call the
   existing `assert_no_issue_1330_temp_files` (`:5506`) and additionally assert no
   created file name starts with `settings.json.`. Pins D5's collision-freedom.

In `instance_artifacts.rs`: the derivation test in section 4.1. The existing
`ignore_rows_are_unique_and_byte_sorted_by_name` and
`no_file_or_dir_row_contains_a_git_wildcard` already fail if the new row is misplaced or
mis-kinded; neither needs a change.

In `instance_gitignore.rs`: the existing fixture-driven coverage test now also proves
`git check-ignore` returns 0 for both new fixtures.

## 6. Dependency-cycle and layering statement

**Zero new module arcs.** `settings.rs:10` already imports from
`crate::config::instance_artifacts`; section 4.2 only widens that existing `use` list.
`instance_gitignore.rs` already reads the registry (`:25`, `:528`).
`instance_artifacts.rs` has no outgoing `crate::` or `super::` reference at all
(module doc `:16`), so it cannot gain one. Docs are not modules. No new arc means no arc
can cross an SCC boundary.

**Layering:** the new code is persistence-local (`std::fs` plus `log`). No `tauri`,
`AppHandle` or any UI transport enters a lower layer. No role inversion.

Acceptance criterion the reviewer runs (clean tree, base SHA versus branch head):

```
node "D:\0_repos\AgentsCommander_iac\.ac\room-21-ac-dev-team-v4\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "D:\0_repos\AgentsCommander_iac\.ac\room-21-ac-dev-team-v4\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
git status --porcelain -- src-tauri/module-arcs.txt
```

Green iff `cyclicSccs` is equal pre/post, every cyclic SCC member set is identical
set-to-set, zero new `from -> to` pairs exist at all, and the last command prints
nothing. Detector exit 1 means gating cycles exist and the graph was still written; only
exit 3 means no graph. Never conflate them.

## 7. Delivery gates

Working directory for cargo commands: `<repo>/src-tauri`. For `npm` and `node`:
`<repo>`. The cargo target directory is the repo root, not `src-tauri/target`.

**Gate 1 - CI-to-plan parity.** `.github/workflows/pr-regression-gates.yml` has no path
filters (`:3-12`), so every job runs on this PR: `test-debt`, `rust-regression`
(windows), `rust-regression-linux`, `rust-linux-release-parity`, `rust-regression-macos`,
`rust-fmt`, `terminal-snapshot-portable` (matrix), `windows-release-cli-smoke`,
`issue-1850-windows-profile` (matrix), `frontend-regression`. Locally reproducible and
required before the PR:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib --bins --tests
npm run test:debt
```

Expected: all four exit 0. `rust-regression-macos`, `rust-linux-release-parity`,
`windows-release-cli-smoke` and `terminal-snapshot-portable` are host-dependent and owned
by CI. `frontend-regression` is unaffected (no frontend file changes) but must still
pass. Owner: the implementer, before opening the PR; CI owns the exact-head run.

**Gate 2 - deterministic toolchain.** The repo-pinned Rust toolchain and `Cargo.lock` via
`--locked` where CI uses it; `node` and `npm` as resolved on PATH, with the version
recorded in the handoff. No independently anchored binary hashes: routine change, no
release or signing in scope (`delivery-nonfunctional-invariants` proportionality rules 3
and 4).

**Gate 3 - authorized Git.** Issue #2058 is open; branch
`feature/2058-settings-json-backup-rotation` already exists from synced `main` at
`f874cc4cbe5cff0f36cc34c56fde85eefe190b6f`. All state-changing Git runs inside the
authorized `repo-AgentsCommander` root. Delivery is one PR into `main` closing #2058. No
direct push to `main`. Before the first product mutation and again before PR creation,
fetch the live target and classify drift by changed paths; only drift touching
`src-tauri/src/config/`, `docs/reference/`, the workflows or the toolchain requires
refreshed evidence.

**Gate 4 - process state and working directory.** Every command above names its working
directory. No inherited configuration changes the result; the plan sets no
`AGENTSCOMMANDER_CONFIG_DIR` or similar. All test state is inside `tempfile::TempDir`;
nothing is written to the developer's real config directory.

**Gate 5 - scope.** Frozen path set, exactly five files:

| File | Change | Impact |
|---|---|---|
| `src-tauri/src/config/instance_artifacts.rs` | 3 consts, 1 registry row, 1 test | New ignored artifact declared; byte-sort position is pinned by an existing test |
| `src-tauri/src/config/instance_gitignore.rs` | 2 fixture strings | Test-only; production rules are derived from the registry |
| `src-tauri/src/config/settings.rs` | 1 const, 2 fns, 1 signature change, 2 call-site edits, 1 capture, 3 doc comments, 7 tests | The only behavior change; every production settings write now archives the bytes it replaced |
| `docs/reference/directory-layout.md` | 1 table row | Inventory now matches the writers |
| `docs/reference/settings.md` | 1 section | Tells a user how to recover |

Postcondition before the PR: `git status --porcelain` lists only these five paths, and
`git diff --stat <recorded phase base SHA>..HEAD` names only these five.

**Gate 6 - mutation ownership and recovery.** Recheck branch, base and
`git status --porcelain` immediately before the first edit, and record the actual base
SHA then. Recovery on abandonment is per-path `git checkout -- <path>`, limited to paths
this run changed and still holding this run's bytes; no `git reset --hard`, no
repository-wide clean. Enhanced controls (OS-handle exclusion, mutation ledgers,
compare-and-swap writers) are **not applicable**: the change adds no new concurrency
boundary, because rotation runs under the existing `settings.json.lock` that already
serializes every production settings writer.

**Gate 7 - bounded execution.** `cargo test --lib --bins --tests` is the long command
(minutes, not hours). Run it under the shell's own timeout or CI's job timeout; capture
stdout and stderr to a file outside scratch and retain it until the outcome is reported.
A timed-out or failed run is reported as a failure. No custom runner is created, so no
process-group owner or descendant-pipe detector is required.

**Gate 8 - evidence discipline.** The zero states bind explicitly: test 2 asserts the
empty backup set, test 3 asserts the absent first slot, and the cycle criterion asserts
an empty `git status` line plus a zero new-arc set. No generic hostile-host or
full-byte-domain suite is imposed.

## 8. Acceptance criteria

1. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --lib --bins --tests` and `npm run test:debt` all exit 0 on the branch
   head.
2. All seven `issue_2058_*` tests exist, and each fails on a tree with the production
   change reverted; the implementer records the reverted-run failure names.
3. `settings.backup.1.json` through `settings.backup.5.json` are the only new files a
   save can create in the config directory; rotation changes no byte of
   `settings.pre-*.json`, `settings.local.json`, `settings.json.lock` or
   `settings.json.*.tmp`.
4. `SETTINGS_BACKUP_ROTATION_GLOB` has exactly one `Disposition::Ignore`,
   `ArtifactKind::Glob` row, in byte-sorted position, and `git check-ignore --no-index`
   returns 0 for both fixtures.
5. The cycle criterion in section 6 is green, with `src-tauri/module-arcs.txt`
   byte-identical.
6. `git status --porcelain` lists only the five files in Gate 5.
7. Every triggered and configured-required check passes on the exact PR-head SHA.
8. The PR closes #2058 and no other issue.

## 9. Preserve list (must not change)

`write_pre_384_v1_backup` semantics (`settings.rs:2620-2631`) including its
fail-the-save call at `:4462`; `SettingsFileLock` acquisition points and timeouts
(`:4436`, `:4872`); the temp-file naming scheme (`:5232`) and
`assert_no_issue_1330_temp_files` (`:5506`); `HISTORICAL_FIXED_RULES`
(`instance_gitignore.rs:512-525`); every existing `InstanceArtifact` row;
`src-tauri/module-arcs.txt`; all frontend files.

## 10. Partition decision

`PARTITION: 1 phase`. The file inventory is five files (three Rust, two docs), well under
the 10-file budget; the change touches one contract (persistence) and has exactly one
green-tree boundary, so the partition trigger does not fire. The `plan-partitioning` cut
rule that keeps Rust and docs in separate phases governs how a plan is cut once the
trigger fires; it does not itself force a cut. The two docs rows are inventory entries
that would be stale the moment the Rust lands, so splitting them would ship a knowingly
inconsistent `main` for no gain.
