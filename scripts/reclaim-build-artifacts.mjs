#!/usr/bin/env node
// Reclaims regenerable Rust/Tauri build artifacts from workgroup repo clones, for issue #932.
//
// Workgroup clones accumulate large Cargo/Tauri output under two locations:
//   - repo-root `target`         (current cargo workspace layout)
//   - `src-tauri/target`         (historical pre-workspace layout)
// `cargo clean` alone is insufficient: it only knows the active workspace target,
// so a stale historical `src-tauri/target` from an older clone is left behind.
//
// This is a maintenance/process script, NOT an app feature or app command. It removes
// ONLY directories whose basename is `target` at the two known paths, inside validated
// repo roots. It never touches source, `.git`, config, or other untracked user work.
//
// Usage:
//   node scripts/reclaim-build-artifacts.mjs [options] [root ...]
//
// Options:
//   --root <path>   Scan root (repeatable; may also be given as a positional arg).
//                   Defaults to this repo's root when none supplied.
//   --apply         Actually delete the artifacts. Without it the script is a dry run
//                   and deletes nothing.
//   --json          Emit a machine-readable JSON summary on stdout.
//   -h, --help      Show this help.
//
// A scan root may be:
//   - a single repo clone (has Cargo.toml / package.json / src-tauri), or
//   - a workgroup dir (`wg-*`) holding `repo-*` clones, or
//   - a parent dir holding several `repo-*` clones and/or `wg-*` workgroup dirs.
//
// Exit codes:
//   0 → completed (dry run, or apply with every deletion succeeding)
//   1 → bad arguments, no valid scan root, or an apply deletion failed

import {
  readdirSync,
  lstatSync,
  statSync,
  realpathSync,
  rmSync,
  existsSync,
} from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve, basename, parse, sep } from 'node:path';

const TAG        = '[reclaim-build-artifacts]';
const __filename = fileURLToPath(import.meta.url);
const REPO_ROOT  = resolve(dirname(__filename), '..');

// Artifact locations to reclaim, relative to a repo root. Basename must be `target`
// so the final-segment guard below can double-check every candidate before deletion.
const ARTIFACT_RELS = ['target', join('src-tauri', 'target')];
const byCodeUnit = (a, b) => Number(a > b) - Number(a < b);

function parseArgs(argv) {
  const roots = [];
  let apply = false;
  let json  = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case '--apply': apply = true; break;
      case '--json':  json  = true; break;
      case '-h':
      case '--help':  printHelp(); process.exit(0); break;
      case '--root': {
        const v = argv[++i];
        if (!v) die('--root needs a path');
        roots.push(v);
        break;
      }
      default:
        if (a.startsWith('--')) die(`unknown option: ${a}`);
        roots.push(a);
    }
  }
  if (roots.length === 0) roots.push(REPO_ROOT);
  return { roots, apply, json };
}

function printHelp() {
  const lines = [
    'Reclaim Rust/Tauri build artifacts (target, src-tauri/target) from room/workgroup clones.',
    '',
    'Usage:',
    '  node scripts/reclaim-build-artifacts.mjs [options] [root ...]',
    '',
    'Options:',
    '  --root <path>   Scan root (repeatable; also accepted positionally).',
    '  --apply         Delete artifacts. Omit for a dry run (default).',
    '  --json          Emit a JSON summary.',
    '  -h, --help      Show this help.',
  ];
  console.log(lines.join('\n'));
}

function die(msg) {
  console.error(`${TAG} ${msg}`);
  process.exit(1);
}

// A directory qualifies as a repo root when it carries at least one marker this
// project always ships. Keeps us from ever treating an arbitrary dir as a clone.
function isRepoRoot(dir) {
  return (
    existsSync(join(dir, 'Cargo.toml')) ||
    existsSync(join(dir, 'package.json')) ||
    existsSync(join(dir, 'src-tauri'))
  );
}

function isDir(p) {
  try {
    return lstatSync(p).isDirectory();
  } catch {
    return false;
  }
}

function childDirs(dir) {
  try {
    return readdirSync(dir, { withFileTypes: true })
      .filter(d => d.isDirectory())
      .map(d => d.name);
  } catch {
    return [];
  }
}

// Discover repo roots reachable from a scan root, bounded to two levels:
//   root itself, `repo-*` children, and `repo-*` grandchildren under a replica dir.
// Replica dirs are `room-*` today and `wg-*` in the legacy layout; both are accepted,
// otherwise a scan of `.ac` silently skips every current room clone (#1702).
function discoverRepoRoots(scanRoot) {
  const found = new Set();
  if (isRepoRoot(scanRoot)) found.add(scanRoot);
  for (const name of childDirs(scanRoot)) {
    const child = join(scanRoot, name);
    if (/^repo-/.test(name) && isRepoRoot(child)) {
      found.add(child);
    } else if (/^(?:room|wg)-/.test(name)) {
      for (const gname of childDirs(child)) {
        const g = join(child, gname);
        if (/^repo-/.test(gname) && isRepoRoot(g)) found.add(g);
      }
    }
  }
  return [...found];
}

function humanBytes(n) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`;
}

// Size of one file, or 0 when it vanished mid-walk.
function fileSizeOrZero(p) {
  try {
    return statSync(p).size;
  } catch {
    return 0;
  }
}

// Sum the on-disk size of a directory tree, tolerating races and permission errors.
// Does not follow symlinked directories, matching how deletion treats them.
function dirSize(root) {
  let total = 0;
  const stack = [root];
  while (stack.length) {
    const cur = stack.pop();
    let entries;
    try {
      entries = readdirSync(cur, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const e of entries) {
      const p = join(cur, e.name);
      if (e.isSymbolicLink()) continue;
      if (e.isDirectory()) {
        stack.push(p);
      } else {
        total += fileSizeOrZero(p);
      }
    }
  }
  return total;
}

// Validate a single artifact candidate. Returns { path, rel, ok, reason, real }.
function inspectCandidate(repoRootReal, rel) {
  const path = join(repoRootReal, rel);
  if (!existsSync(path)) return { path, rel, ok: false, reason: 'absent' };

  let st;
  try {
    st = lstatSync(path);
  } catch (e) {
    return { path, rel, ok: false, reason: `stat failed: ${e.message}` };
  }
  // Refuse to follow or delete a symlink; we only reclaim genuine build dirs.
  if (st.isSymbolicLink()) return { path, rel, ok: false, reason: 'symlink (skipped)' };
  if (!st.isDirectory()) return { path, rel, ok: false, reason: 'not a directory' };
  // Final-segment guard: whatever the relative path, the thing we delete is a `target` dir.
  if (basename(path) !== 'target') return { path, rel, ok: false, reason: 'basename is not target' };

  // Containment guard: the resolved path must sit inside the resolved repo root.
  let real;
  try {
    real = realpathSync(path);
  } catch (e) {
    return { path, rel, ok: false, reason: `realpath failed: ${e.message}` };
  }
  if (real !== repoRootReal && !real.startsWith(repoRootReal + sep)) {
    return { path, rel, ok: false, reason: 'resolves outside repo root (skipped)' };
  }
  if (real === repoRootReal) return { path, rel, ok: false, reason: 'resolves to repo root (skipped)' };

  return { path, rel, ok: true, reason: 'artifact', real };
}

// Resolve + validate one scan root. Returns its real path, or null when unusable.
function resolveScanRoot(r) {
  const abs = resolve(r);
  if (!isDir(abs)) {
    console.error(`${TAG} scan root not found or not a directory: ${abs}`);
    return null;
  }
  let real;
  try {
    real = realpathSync(abs);
  } catch (e) {
    console.error(`${TAG} cannot resolve scan root ${abs}: ${e.message}`);
    return null;
  }
  // Never operate at a filesystem root; require some depth.
  if (parse(real).root === real) {
    console.error(`${TAG} refusing filesystem-root scan root: ${real}`);
    return null;
  }
  return real;
}

// Discover the repo roots beneath every valid scan root.
function collectRepoRoots(roots) {
  const repoRoots = new Set();
  for (const r of roots) {
    const real = resolveScanRoot(r);
    if (real === null) continue;
    for (const rr of discoverRepoRoots(real)) repoRoots.add(rr);
  }
  return repoRoots;
}

// Build the result entry for one inspected candidate, deleting it when applying.
function buildEntry(repoRootReal, rel, info, apply) {
  const entry = {
    repo: repoRootReal,
    rel,
    path: info.path,
    ok: info.ok,
    reason: info.reason,
    bytes: info.ok ? dirSize(info.path) : 0,
    removed: false,
    error: null,
  };
  if (info.ok && apply) {
    try {
      rmSync(info.path, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
      entry.removed = true;
    } catch (e) {
      entry.error = e.message;
    }
  }
  return entry;
}

// Inspect every artifact candidate of one repo root.
function inspectRepo(rr, apply) {
  let repoRootReal;
  try {
    repoRootReal = realpathSync(rr);
  } catch {
    return [];
  }
  const entries = [];
  for (const rel of ARTIFACT_RELS) {
    const info = inspectCandidate(repoRootReal, rel);
    if (info.reason === 'absent') continue; // nothing there, stay quiet
    entries.push(buildEntry(repoRootReal, rel, info, apply));
  }
  return entries;
}

function printJsonReport(apply, repoRoots, totalBytes, results) {
  console.log(JSON.stringify(
    {
      mode: apply ? 'apply' : 'dry-run',
      repoRoots: [...repoRoots].sort(byCodeUnit),
      totalBytes,
      totalHuman: humanBytes(totalBytes),
      results,
    },
    null,
    2,
  ));
}

function printTextReport(apply, repoRoots, reclaimable, skipped, totalBytes) {
  console.log(`${TAG} mode: ${apply ? 'APPLY (deleting)' : 'dry-run (no changes)'}`);
  console.log(`${TAG} repo roots scanned: ${repoRoots.size}`);
  if (reclaimable.length === 0) {
    console.log(`${TAG} no target/src-tauri/target artifacts found. Nothing to reclaim.`);
  }
  for (const r of reclaimable) {
    const verb = apply ? (r.removed ? 'removed' : `FAILED (${r.error})`) : 'would remove';
    console.log(`${TAG}   ${verb}: ${r.path}  (${humanBytes(r.bytes)})`);
  }
  for (const s of skipped) {
    // Only surface skips that are not the boring "absent" case.
    console.log(`${TAG}   skipped: ${s.path}  (${s.reason})`);
  }
  console.log(`${TAG} reclaimable total: ${humanBytes(totalBytes)} across ${reclaimable.length} dir(s)`);
  if (!apply && reclaimable.length > 0) {
    console.log(`${TAG} re-run with --apply to delete.`);
  }
}

function main() {
  const { roots, apply, json } = parseArgs(process.argv.slice(2));

  const repoRoots = collectRepoRoots(roots);
  if (repoRoots.size === 0) {
    die('no valid repo roots discovered from the given scan root(s)');
  }

  const results = [];
  for (const rr of [...repoRoots].sort(byCodeUnit)) results.push(...inspectRepo(rr, apply));

  const reclaimable = results.filter(r => r.ok);
  const skipped     = results.filter(r => !r.ok);
  const failed      = results.filter(r => r.error);
  const totalBytes  = reclaimable.reduce((a, r) => a + r.bytes, 0);

  if (json) printJsonReport(apply, repoRoots, totalBytes, results);
  else printTextReport(apply, repoRoots, reclaimable, skipped, totalBytes);

  process.exit(failed.length > 0 ? 1 : 0);
}

main();
