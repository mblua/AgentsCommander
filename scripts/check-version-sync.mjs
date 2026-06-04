#!/usr/bin/env node
// Verifies every project version location agrees, for issue #201.
//
// Reads each location bump-version.mjs writes and exits non-zero with a
// list of mismatched files/fields when they disagree. Used locally and in
// CI to keep the workflow trustworthy.
//
// Usage:
//   node scripts/check-version-sync.mjs
//
// Exit codes:
//   0 → all locations report the same version
//   1 → one or more locations missing a version, or versions disagree

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';

const __filename = fileURLToPath(import.meta.url);
const ROOT       = resolve(dirname(__filename), '..');

function read(rel) {
  return readFileSync(join(ROOT, rel), 'utf8');
}

function extract(label, rel, re) {
  const txt = read(rel);
  const m = re.exec(txt);
  if (!m) return { label, file: rel, version: null, error: 'pattern did not match' };
  return { label, file: rel, version: m[1], error: null };
}

const checks = [
  extract(
    'package.json:version',
    'package.json',
    /(?:\r?\n)\s*"name":\s*"agentscommander",\s*(?:\r?\n)\s*"version":\s*"([^"]+)"/,
  ),
  extract(
    'npm/package.json:version',
    'npm/package.json',
    /(?:\r?\n)\s*"name":\s*"agentscommander",\s*(?:\r?\n)\s*"version":\s*"([^"]+)"/,
  ),
  extract(
    'npm/install.js:VERSION',
    'npm/install.js',
    /(?:\r?\n)const VERSION\s*=\s*["']([^"']+)["']/,
  ),
  extract(
    'package-lock.json:root.version',
    'package-lock.json',
    /(?:\r?\n)\s*"name":\s*"agentscommander",\s*(?:\r?\n)\s*"version":\s*"([^"]+)"/,
  ),
  extract(
    'package-lock.json:packages[""].version',
    'package-lock.json',
    /(?:\r?\n)\s*"":\s*\{\s*(?:\r?\n)\s*"name":\s*"agentscommander",\s*(?:\r?\n)\s*"version":\s*"([^"]+)"/,
  ),
  extract(
    'src-tauri/Cargo.toml:[package].version',
    'src-tauri/Cargo.toml',
    /(?:\r?\n)\s*name\s*=\s*"agentscommander-new"\s*(?:\r?\n)version\s*=\s*"([^"]+)"/,
  ),
  extract(
    'src-tauri/Cargo.lock:internal-crate.version',
    'src-tauri/Cargo.lock',
    /(?:\r?\n)\s*name\s*=\s*"agentscommander-new"\s*(?:\r?\n)version\s*=\s*"([^"]+)"/,
  ),
  extract(
    'src-tauri/tauri.conf.json:version',
    'src-tauri/tauri.conf.json',
    /(?:\r?\n)\s*"productName":\s*"[^"]+",\s*(?:\r?\n)\s*"version":\s*"([^"]+)"/,
  ),
];

const broken = checks.filter(c => c.error || c.version === null);
if (broken.length > 0) {
  console.error('[check-version-sync] could not read version from:');
  for (const b of broken) console.error(`  - ${b.label} (${b.file}): ${b.error || 'no match'}`);
  console.error('Update scripts/check-version-sync.mjs anchors if a file was renamed or restructured.');
  process.exit(1);
}

const versions = new Set(checks.map(c => c.version));
if (versions.size === 1) {
  const [v] = versions;
  console.log(`[check-version-sync] OK — every location at ${v}`);
  process.exit(0);
}

console.error('[check-version-sync] versions disagree:');
for (const c of checks) console.error(`  - ${c.version}  ${c.label}  (${c.file})`);
console.error('');
console.error('Run `npm run version:bump -- patch|minor|major|X.Y.Z` to bring everything back in sync.');
process.exit(1);
