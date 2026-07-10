# Reclaiming build artifacts

For operators and agents who maintain workgroup repo clones. How to recover disk space taken by regenerable Rust/Tauri build output, safely and repeatably.

Workgroup clones accumulate large Cargo/Tauri build artifacts whenever an agent or operator runs a build inside a clone. The bytes are fully regenerable, so they are safe to delete, but they recur and can consume many gigabytes across a set of clones. `scripts/reclaim-build-artifacts.mjs` is a maintenance/process script (not an app feature, UI action, or app command) that reclaims them.

## What it removes

Only these two directories, and only when they are real directories named `target`:

| Path (relative to a repo root) | Why it exists |
|---|---|
| `target` | Current Cargo workspace output location. |
| `src-tauri/target` | Historical pre-workspace output location, left behind by older clones. |

Both are covered on purpose. `cargo clean` alone is insufficient: it only knows the active workspace target, so a stale historical `src-tauri/target` from an older clone survives a clean. This script targets both locations by explicit path.

It never deletes source, `.git`, config, or any other untracked user work. See [Safety guardrails](#safety-guardrails).

## Usage

Dry run first (default: deletes nothing, just reports what it would remove):

```
npm run reclaim:artifacts
```

Apply the deletion:

```
npm run reclaim:artifacts:apply
```

Scan somewhere other than this repo, for example a whole workgroup or a parent holding several workgroups. Pass roots positionally or with `--root` (repeatable). Combine with `--apply` to delete:

```
# dry run across a workgroup dir's repo-* clones
node scripts/reclaim-build-artifacts.mjs "C:\path\to\.ac\wg-13-dev-v4-team"

# apply across several roots
node scripts/reclaim-build-artifacts.mjs --apply --root "C:\path\to\clone-a" --root "C:\path\to\clone-b"

# machine-readable summary
node scripts/reclaim-build-artifacts.mjs --json
```

### Options

| Option | Meaning |
|---|---|
| `--root <path>` | Scan root, repeatable. Also accepted as a positional argument. Defaults to this repo's root. |
| `--apply` | Actually delete. Without it the run is a dry run. |
| `--json` | Emit a JSON summary on stdout instead of the human-readable report. |
| `-h`, `--help` | Show usage. |

### Scan roots

A scan root may be any of:

- a single repo clone (has `Cargo.toml`, `package.json`, or `src-tauri`),
- a workgroup dir (`wg-*`) that holds `repo-*` clones, or
- a parent dir that holds several `repo-*` clones and/or `wg-*` workgroup dirs.

Discovery is bounded to two levels: the root itself, its `repo-*` children, and `repo-*` grandchildren under `wg-*` dirs. It does not walk arbitrarily deep.

## Safety guardrails

- Dry run is the default. Deletion happens only with `--apply`.
- A directory is treated as a repo root only when it carries a project marker (`Cargo.toml`, `package.json`, or `src-tauri`).
- Only the two known relative paths are considered, and the final path segment must be `target` before anything is deleted.
- Symlinks are skipped, never followed or deleted.
- Each candidate's resolved real path must sit inside its resolved repo root; anything that resolves outside is skipped.
- Filesystem-root scan roots are refused.
- Windows-safe: paths are handled with `node:path`, and deletion uses `fs.rmSync` with retries.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Completed. Dry run, or apply with every deletion succeeding. |
| `1` | Bad arguments, no valid scan root, or an apply deletion failed. |

## When to run it

- Periodically on machines that host workgroup clones, after heavy build activity.
- Before archiving or duplicating a clone set, to avoid copying regenerable bytes.
- Any time disk pressure traces back to `target` / `src-tauri/target` under clones.

Rebuilds regenerate the artifacts on the next `cargo`/`tauri` build, so reclaiming is non-destructive to work in progress.
