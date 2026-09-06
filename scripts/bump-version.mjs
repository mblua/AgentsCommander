#!/usr/bin/env node
// One-command project version bumper for issue #201.
//
// Updates every project version location to the same X.Y.Z so future bumps
// can never leave package.json / package-lock.json / Cargo / Tauri metadata
// out of sync.
//
// Usage:
//   node scripts/bump-version.mjs patch       # 0.8.17 -> 0.8.18
//   node scripts/bump-version.mjs minor       # 0.8.17 -> 0.9.0
//   node scripts/bump-version.mjs major       # 0.8.17 -> 1.0.0
//   node scripts/bump-version.mjs 0.9.0       # explicit X.Y.Z
//
// Files touched:
//   - package.json                             "version"
//   - package-lock.json                        root "version" + packages[""].version
//   - npm/package.json                         "version"
//   - npm/install.js                           VERSION constant
//   - src-tauri/Cargo.toml                     [package] version
//   - Cargo.lock                               internal Cargo crate entry version
//   - src-tauri/tauri.conf.json                "version"
//
// Re-running with the same X.Y.Z target is supported and intentional: it
// synchronizes any drifted location even when package.json is already at
// the target.
//
// Exit codes:
//   0 → success (bumped or already in sync)
//   1 → bad usage, current version unreadable, or any target file's anchor
//       did not match (file renamed/restructured?)

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const SEMVER_RE = /^(\d+)\.(\d+)\.(\d+)$/;

const __filename = fileURLToPath(import.meta.url);
const ROOT       = resolve(dirname(__filename), '..');

const HELP = `Usage:
  node scripts/bump-version.mjs <patch|minor|major|X.Y.Z>

Updates every project version location:
  - package.json
  - package-lock.json (root + packages[""])
  - npm/package.json
  - npm/install.js
  - src-tauri/Cargo.toml
  - Cargo.lock (internal Cargo crate entry)
  - src-tauri/tauri.conf.json

Re-running with the same X.Y.Z target re-synchronizes drifted locations.
`;

// Each entry knows the file it lives in, a human label, and the surgical
// regex that captures the prefix before the version literal. Anchored on
// nearby fields so we can never accidentally rewrite a dependency version.
// Line-end tokens use \r?\n so files with either LF or CRLF endings are
// preserved byte-for-byte outside the version literal.
// The Rust crate is still named agentscommander-new for Cargo/test binary
// compatibility. Production bundle identity comes from Tauri config.
const PATCHES = [
  {
    file: 'npm/install.js',
    label: 'install.js constant',
    re: /(\r?\nconst VERSION\s*=\s*)['"][^'"]+['"]/,
  },
  {
    file: 'package.json',
    label: 'root version',
    re: /(\r?\n\s*"name":\s*"agentscommander",\s*\r?\n\s*"version":\s*)"[^"]+"/,
  },
  {
    file: 'npm/package.json',
    label: 'npm wrapper version',
    re: /(\r?\n\s*"name":\s*"@mblua\/agentscommander",\s*\r?\n\s*"version":\s*)"[^"]+"/,
  },
  {
    file: 'package-lock.json',
    label: 'root version',
    re: /(\r?\n\s*"name":\s*"agentscommander",\s*\r?\n\s*"version":\s*)"[^"]+"/,
  },
  {
    file: 'package-lock.json',
    label: 'packages[""].version',
    re: /(\r?\n\s*"":\s*\{\s*\r?\n\s*"name":\s*"agentscommander",\s*\r?\n\s*"version":\s*)"[^"]+"/,
  },
  {
    file: 'src-tauri/Cargo.toml',
    label: '[package] version',
    re: /(\r?\n\s*name\s*=\s*"agentscommander-new"\s*\r?\nversion\s*=\s*)"[^"]+"/,
  },
  {
    file: 'Cargo.lock',
    label: 'internal Cargo crate entry',
    re: /(\r?\n\s*name\s*=\s*"agentscommander-new"\s*\r?\nversion\s*=\s*)"[^"]+"/,
  },
  {
    file: 'src-tauri/tauri.conf.json',
    label: 'version',
    re: /(\r?\n\s*"productName":\s*"[^"]+",\s*\r?\n\s*"version":\s*)"[^"]+"/,
  },
];

function die(msg) {
  console.error(`[bump-version] ${msg}`);
  process.exit(1);
}

function readCurrentVersion() {
  const txt = readFileSync(join(ROOT, 'package.json'), 'utf8');
  // Anchor on `"name": "agentscommander"` so we never accidentally read a
  // dependency `"version"` literal that happens to appear above the project
  // version (e.g., if package.json keys are reordered or an `overrides`
  // block is added).
  const m = /\r?\n\s*"name":\s*"agentscommander",\s*\r?\n\s*"version":\s*"([^"]+)"/.exec(txt);
  if (!m) die('Cannot read current version from package.json (anchor "name": "agentscommander" not found above "version")');
  return m[1];
}

function bump(current, kind) {
  const m = SEMVER_RE.exec(current);
  if (!m) die(`package.json version "${current}" is not a simple X.Y.Z`);
  const major = Number(m[1]);
  const minor = Number(m[2]);
  const patch = Number(m[3]);
  switch (kind) {
    case 'patch': return `${major}.${minor}.${patch + 1}`;
    case 'minor': return `${major}.${minor + 1}.0`;
    case 'major': return `${major + 1}.0.0`;
  }
  die(`Unknown bump kind: ${kind}`);
}

function resolveTarget(arg, current) {
  if (arg === 'patch' || arg === 'minor' || arg === 'major') return bump(current, arg);
  if (SEMVER_RE.test(arg)) return arg;
  die(`Argument must be patch|minor|major or X.Y.Z (got "${arg}")`);
}

function planFile(file, patches, target) {
  const path = join(ROOT, file);
  const before = readFileSync(path, 'utf8');
  // Apply every patch sequentially against the in-memory string so a file
  // with multiple PATCHES entries (e.g. package-lock.json's root version +
  // packages[""].version) lands all its rewrites in a single write. Each
  // patch's regex is re-tested against the running buffer so we can never
  // silently lose an entry.
  let current = before;
  const steps = [];
  for (const p of patches) {
    // Anchor-not-matched is a hard fail: it means the file was renamed or
    // restructured and our regex needs maintenance — do NOT silently skip.
    if (!p.re.test(current)) {
      die(`${file}: anchor for "${p.label}" did not match — file may have been renamed or restructured. Update PATCHES regexes.`);
    }
    const next = current.replace(p.re, (_match, prefix) => `${prefix}"${target}"`);
    steps.push({ label: p.label, changed: next !== current });
    current = next;
  }
  return { path, file, before, after: current, steps };
}

function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) {
    process.stderr.write(HELP);
    process.exit(1);
  }
  if (args[0] === '-h' || args[0] === '--help') {
    process.stdout.write(HELP);
    process.exit(0);
  }

  const current = readCurrentVersion();
  const target  = resolveTarget(args[0], current);

  // Always run every patch — even when target equals current. That way an
  // explicit X.Y.Z passed to repair drift will overwrite stale lock
  // entries instead of silently skipping the writes.
  if (target === current) {
    console.log(`[bump-version] synchronizing at ${target}`);
  } else {
    console.log(`[bump-version] ${current} -> ${target}`);
  }

  // Group PATCHES by file so each target file is read once, mutated
  // sequentially in memory, and written once. The previous one-plan-per-
  // entry layout re-read the original file for every entry and then
  // wrote each plan independently, so the second write to any file
  // silently clobbered the first — package-lock.json lost its root
  // version rewrite on every bump.
  const byFile = new Map();
  for (const p of PATCHES) {
    const list = byFile.get(p.file) ?? [];
    list.push(p);
    byFile.set(p.file, list);
  }

  // Two-phase to avoid leaving the repo half-updated if a later file fails:
  //   phase 1 — read every file, validate every anchor, compute every new
  //             content. If anything fails, we exit before any writeFileSync.
  //   phase 2 — write the files whose content actually changed.
  const plans = [];
  for (const [file, patches] of byFile) {
    plans.push(planFile(file, patches, target));
  }

  for (const plan of plans) {
    if (plan.after !== plan.before) {
      writeFileSync(plan.path, plan.after);
    }
    for (const step of plan.steps) {
      if (step.changed) {
        console.log(`[bump-version] ${plan.file} (${step.label}) -> ${target}`);
      } else {
        console.log(`[bump-version] ${plan.file} (${step.label}) already at ${target}`);
      }
    }
  }

  console.log(`[bump-version] OK at ${target}`);
}

main();
