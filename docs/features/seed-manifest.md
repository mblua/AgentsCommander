# Seed manifest

AgentsCommander records the project-scoped files it publishes into your project's
`.ac` folder in a small, deterministic text file:
`<project>/.ac/seed-manifest.toml`. It is a **diagnostic inventory**, not an
ownership ledger: it tells you which managed files AC last seeded and when. It
never grants ownership and never authorizes AC to overwrite, repair, or delete
anything. AC's managed `.ac/.gitignore` ignores it, so it is not
version-controlled by default (see [Git behavior](#git-behavior)).

## What it records

The manifest has one row per project-relative **logical destination**, and each
row carries the UTC wall-clock time of that file's most recent successful physical
publication by AgentsCommander. Two publisher families write rows:

- **Project context templates** - `.ac/Context.AgentsCommander.md` (scope
  `context:agentscommander`) and `.ac/Context.coordinator.md` (scope
  `context:coordinator`), created or refreshed when AC registers a project, scans
  it during discovery, materializes a session's context, or you explicitly
  overwrite a template.
- **Coding-agent catalog** (#1318) - `.ac/coding-agents/agents.json` (scope
  `catalog:coding-agents`, source `builtin`). One row is recorded for every
  actual base publication: the first management of a fresh catalog, a one-time
  legacy migration, or a later refresh to a new shipped revision. The row is
  re-timestamped on each such publication; a bookkeeping retry can add the row
  for an already-published base without republishing it. The user-owned
  `agents.local.json`, the migration backup `agents.migration-v1.backup.json`
  and the journal `.agents.migration-v1.json` are never published files and
  never get rows. The `_seed/` masters tree is not rowed.

Replica config folders are **not recorded**. [Config seed](config-seed.md) still
installs `.claude`/`.codex`/... into a room replica at every spawn, but since
[#1480](https://github.com/mblua/AgentsCommander/issues/1480) those publications
create no rows. A manifest written by an older build may still carry
`replica_config_file` rows: AC reads them, never updates them, and omits them from
the next canonical write. The `replica_config_folders` name remains in the
`coverage` declaration below as compatibility vocabulary.

Everything else is deliberately **out of scope**: the manifest does not track
files you create by hand, the `.agentscommander-context-templates.json` ownership
state, agent memory, role files, repos, or anything outside `.ac`.

## Schema (v2)

```toml
# Managed by AgentsCommander. Diagnostic only; never grants file ownership.
schema_version = 1
coverage_version = 2
coverage = ["project_context_templates", "replica_config_folders", "coding_agent_catalog"]

[[files]]
path = ".ac/Context.AgentsCommander.md"
path_encoding = "utf8"
kind = "project_context_template"
scope = "context:agentscommander"
source = "builtin"
last_seeded_at = "2026-07-16T19:40:07.123Z"

[[files]]
path = ".ac/coding-agents/agents.json"
path_encoding = "utf8"
kind = "coding_agent_catalog"
scope = "catalog:coding-agents"
source = "builtin"
last_seeded_at = "2026-07-16T19:43:00.000Z"
```

An empty manifest is written as `files = []`. The file is always UTF-8 with LF line
endings and exactly one trailing newline, its rows sorted by `(path_encoding,
path)`, so re-serialization is deterministic and Git-friendly.

- `path` is always project-relative and begins with `.ac`. It never contains an
  absolute path, drive letter, or your machine/user name. A path whose native
  components are not valid Unicode is stored as `unix_bytes_hex` or
  `windows_utf16_hex` and marked in `path_encoding`; a foreign-platform row checked
  out through Git is preserved verbatim and is never turned into a filesystem
  target.
- `kind`, `scope`, `source`, and `path_encoding` are closed enums. `source` is one
  of `builtin`, `workspace_profile`, `workspace_base`, `matrix_profile`,
  `matrix_base`, or `catalog_default`.
- `last_seeded_at` is a millisecond-precision RFC 3339 UTC timestamp ending in `Z`.

The schema intentionally omits content hashes, file size, source paths, host, user,
process id, and any operation history. A catalog row therefore proves that a
publication was recorded, not which revision is current: it carries no content
hash or revision.

## v1 to v2 upgrade

Coverage grew to v2 with the `coding_agent_catalog` kind (#1318); the wire shape
is unchanged (schema_version stays 1). A **v1 manifest upgrades in place on the
first `ProjectSeedManifestGuard::acquire`** (read or write) under the project
lock: the parse is substitution-only (the coverage declaration is replaced, every
row and timestamp is preserved verbatim) and reuses every existing strict row
check. The upgrade is one-shot (a successful upgrade parses strictly on the next
acquire) and lossless. Any other degraded shape (future schema, bounds
violations, external edits, corrupt bytes) stays byte-preserved with the writer
disabled, exactly as before. A v2 manifest written by this build is NOT readable
by an older build; the old build preserves its bytes and disables its writer.

## Time semantics

`last_seeded_at` is **display data, not a conflict key.** It is the wall-clock time
captured at the physical publication boundary - immediately after the successful
hard-link, atomic replace, or directory install - not a time sampled later by a
higher-level caller.

- A second physical publication updates the row to that event's time even when the
  new bytes are identical to the old ones. It is never an append-only history.
- The clock is never synthetically advanced. On a wall-clock rollback the time is
  not required to increase; lock ordering, not `max(timestamp)`, decides which of
  two publications is later.
- Because v1 records milliseconds, two publications inside the same UTC millisecond
  can produce the same representable value. When every other field is unchanged,
  that is a genuine no-op and the file is not rewritten.

## Normal churn

Every physical publication updates its row to that event's wall-clock time, even
when the published bytes are identical (see [Time semantics](#time-semantics)).
That is the accepted product cost of recording real publication time; AC does not
suppress the timestamp, compare content, or truncate the row list to reduce churn.
The manifest is ignored by Git by default (see [Git behavior](#git-behavior)), so
this churn stops appearing in your diffs once the manifest is untracked; a
manifest AC creates is never tracked.

## Lifecycle removal

When AC removes a room, deletes a team, removes a team member's replica, or
deletes an Agent Matrix, it prunes the now-absent config scopes from the manifest
under the same project lock, after the directory removal commits. Only AC's own
explicit lifecycle events prune rows:

- Deleting a project's files outside AC takes the manifest with them; there is no
  external cleanup registry.
- **Manual deletion or editing** of a seeded file does **not** change the manifest.
  The manifest records what AC published, not what currently exists on disk.
- **Project archive, unarchive, unregister, or re-register** leave the project and
  its `.ac` (and manifest) untouched.
- **Cloning or copying a project** preserves an existing manifest byte-for-byte.
  AC does not invent a new owner, reset times, backfill, or prune room paths
  the clone happens to lack. A row heals to a real new time only on the clone's
  next real publication.

Existing installs receive no invented timestamps: the first row for a file appears
only from its first future real publication after this feature ships.

## Recovery gaps and partial failures

The target file and the manifest cannot be written in one filesystem transaction,
and v1 deliberately chooses **no false rows over guaranteed completeness**:

- If a target publishes successfully but the process dies before the manifest is
  replaced, the manifest simply stays behind. AC never infers success or backfills
  a timestamp from file existence, mtime, content equality, or Git history; the
  next real publication heals the row.
- **Config install-and-restore failure.** Config seed renames the old destination
  aside before installing the new one, and reports a typed failure when the
  install, the restore, or both fail. Since #1480 a config-seed publication
  neither adds nor removes a manifest row, so no failure mode of that install
  changes the manifest; a legacy `replica_config_file` row survives until the
  next canonical write or explicit lifecycle event.
- **Strict-invalid manifest (including a Git conflict).** If the manifest becomes
  corrupt, carries a newer `schema_version`/`coverage`, or contains Git conflict
  markers, AC preserves those bytes exactly and disables the writer for that
  project rather than overwriting or quarantining your only copy. A target can
  still publish successfully; it is just recorded as unrecorded until you resolve
  the file by hand. Resolve a conflict the way you would any other text file.

## Git behavior

`.ac/seed-manifest.toml` is **not version-controlled by default**. AC's managed
`.ac/.gitignore` block ignores the manifest together with its lock and temp
companions: `/.seed-manifest.lock`, `/.seed-manifest.*.tmp` and
`/seed-manifest.toml`. The leading slash anchors all three rules to the `.ac`
root, so same-named files in nested user directories are unaffected. If a project
already tracks the manifest, the ignore rule alone does not untrack it; run
`git rm --cached <project>/.ac/seed-manifest.toml` to stop tracking it. If you
would rather keep reviewing the manifest in Git, add `!/seed-manifest.toml`
after AC's rule in `.ac/.gitignore`: Git applies the last matching rule. An
existing `.ac/.gitignore` that still carries the retired `!/seed-manifest.toml`
block is migrated in place by the next project registration or discovery.

## Durability and support

- On Windows the writer targets local NTFS on Windows 10 1809+ and Windows 11. It
  flushes and verifies the staging temp before the namespace call, but a
  successful flush is **not** a namespace-durability or power-loss guarantee: v1
  makes no Windows kernel-crash or power-loss durability claim and never relies on
  the unsupported `REPLACEFILE_WRITE_THROUGH` flag. After an abrupt power loss the
  canonical file may be the old complete file, the new complete file, absent, or in
  a filesystem-recovered state; AC preserves whatever it finds and never infers
  success.
- On Unix the writer uses an atomic rename with a best-effort parent-directory
  sync.
- Network filesystems (SMB/UNC, NFS/CIFS) and non-NTFS Windows filesystems are not
  a claimed environment; on an unsupported or capability-limited filesystem AC
  degrades to leaving the target published but unrecorded rather than corrupting
  the manifest.

## Trust

The manifest is user-editable diagnostic metadata. AgentsCommander never treats a
row as authority to create, overwrite, repair, or delete a file, and a forged,
traversal-shaped, or foreign row can never direct filesystem I/O or widen a
delete. Treat it as a record of what AC did, not as a control surface.

## See also

- [Config seed](config-seed.md) - the replica config publications, which no
  longer produce manifest rows (#1480)
- [Agent Matrix conventions](../agent-matrix-conventions.md) - the `.ac` layout the
  manifest paths are relative to
