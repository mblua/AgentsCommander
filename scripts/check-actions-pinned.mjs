#!/usr/bin/env node
// #4690: CI gate `actions-pinned` (workflow .github/workflows/actions-pinned.yml, also
// `npm run actions-pinned`). Fails when any `uses:` under .github/workflows or
// .github/actions is external and not `@<40-hex lowercase SHA> # vX.Y.Z`. Zero rule, no
// baseline. Two fixed exemptions:
// - local `./` refs: they come from the same commit as the workflow;
// - `dtolnay/rust-toolchain`: pinned to its `stable` branch, which has no release tags, so
//   it needs the SHA but no version comment.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolveGit } from './resolve-git.mjs';

const FULL_USE = /^\s*(?:-\s+)?uses:\s*(['"]?)([^'"\s#]+)\1\s*(?:(#.*?))?\s*$/;
const ANY_USE = /^\s*(?:-\s*)?\{?\s*uses:/;
const SHA_REF = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_./-]+@[0-9a-f]{40}$/;
const DOCKER_DIGEST = /^docker:\/\/.+@sha256:[0-9a-f]{64}$/;
const VERSION_COMMENT = /^#\s*v\d+\.\d+\.\d+\s*$/;
const REASONS = {
  unpinned: 'not pinned to a full commit SHA',
  'no-version': 'missing "# vX.Y.Z" version comment',
  unparsed: 'uses: form not recognized',
};

export function classifyUse(ref, comment) {
  if (ref.startsWith('./')) return 'local';
  if (DOCKER_DIGEST.test(ref)) return 'pinned';
  if (SHA_REF.test(ref)) {
    if (ref.startsWith('dtolnay/rust-toolchain@')) return 'exempt';
    return VERSION_COMMENT.test(comment) ? 'pinned' : 'no-version';
  }
  return 'unpinned';
}

export function scanText(text) {
  const uses = [];
  text.split('\n').forEach((raw, index) => {
    const lineText = raw.endsWith('\r') ? raw.slice(0, -1) : raw;
    const match = FULL_USE.exec(lineText);
    if (match) {
      const ref = match[2];
      const comment = match[3] ?? '';
      uses.push({ line: index + 1, ref, comment, kind: classifyUse(ref, comment) });
    } else if (ANY_USE.test(lineText)) {
      uses.push({ line: index + 1, ref: lineText.trim(), comment: '', kind: 'unparsed' });
    }
  });
  return uses;
}

const SHA = 'fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09';
const TOOLCHAIN = 'dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c';
const CASES = [
  ['      - uses: actions/checkout@v5', 'unpinned'],
  ['        uses: actions/cache/restore@v6', 'unpinned'],
  ['uses: actions/checkout@fbc6f39 # v5.1.0', 'unpinned'],
  [`uses: actions/checkout@${SHA.toUpperCase()} # v5.1.0`, 'unpinned'],
  ['uses: a/b@main', 'unpinned'],
  ['uses: docker://alpine:3.20', 'unpinned'],
  ['uses: x@v1\r', 'unpinned'],
  [`- uses: actions/checkout@${SHA} # v5.1.0`, 'pinned'],
  ["uses: 'actions/cache/save@55cc8345863c7cc4c66a329aec7e433d2d1c52a9' # v6.1.0", 'pinned'],
  [`uses: docker://alpine@sha256:${'a'.repeat(64)}`, 'pinned'],
  [`- uses: actions/checkout@${SHA}`, 'no-version'],
  [`- uses: actions/checkout@${SHA} # v5`, 'no-version'],
  ['uses: swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2', 'no-version'],
  [`uses: ${TOOLCHAIN} # stable`, 'exempt'],
  [`uses: ${TOOLCHAIN}`, 'exempt'],
  ['uses: dtolnay/rust-toolchain@stable', 'unpinned'],
  ['      - uses: ./.github/actions/restore-release-controls', 'local'],
  ['uses: >-', 'unpinned'],
  ['      - {uses: a/b@v1}', 'unparsed'],
  ['# uses: actions/checkout@v5', null],
];

function selfTest() {
  const failures = [];
  for (const [input, expected] of CASES) {
    const found = scanText(input);
    const kind = found.length === 0 ? null : found[0].kind;
    if (found.length > 1 || kind !== expected) {
      failures.push(`${JSON.stringify(input)}: expected ${expected}, got ${kind}`);
    }
  }
  if (CASES.length === 0 || failures.length > 0) {
    for (const failure of failures) process.stderr.write(`[actions-pinned] ${failure}\n`);
    process.stderr.write(`[actions-pinned] self-test FAIL: ${failures.length} case(s)\n`);
    return 1;
  }
  process.stdout.write(`[actions-pinned] self-test OK: ${CASES.length} cases\n`);
  return 0;
}

function check() {
  const root = fileURLToPath(new URL('..', import.meta.url));
  let files;
  try {
    const out = execFileSync(
      resolveGit(),
      ['ls-files', '-z', '--', '.github/workflows', '.github/actions'],
      { cwd: root },
    );
    files = String(out).split('\0').filter((name) => /\.ya?ml$/.test(name));
  } catch (error) {
    process.stderr.write(`[actions-pinned] git ls-files failed: ${error.message}\n`);
    return 2;
  }
  const counts = { pinned: 0, exempt: 0, local: 0 };
  const bad = [];
  for (const file of files) {
    let text;
    try {
      text = fs.readFileSync(path.join(root, file), 'utf8');
    } catch (error) {
      process.stderr.write(`[actions-pinned] cannot read ${file}: ${error.message}\n`);
      return 2;
    }
    for (const use of scanText(text)) {
      if (REASONS[use.kind]) bad.push({ file, ...use });
      else counts[use.kind] += 1;
    }
  }
  const total = counts.pinned + counts.exempt + counts.local + bad.length;
  if (files.length === 0 || total === 0) {
    process.stderr.write('[actions-pinned] FAIL: nothing scanned\n');
    return 2;
  }
  if (bad.length > 0) {
    for (const use of bad) {
      process.stderr.write(
        `[actions-pinned] ${use.file}:${use.line}: ${use.ref}: ${REASONS[use.kind]}\n`,
      );
    }
    process.stderr.write(`[actions-pinned] FAIL: ${bad.length} bad uses\n`);
    return 1;
  }
  process.stdout.write(
    `[actions-pinned] OK: ${counts.pinned} pinned, ${counts.exempt} exempt, ` +
      `${counts.local} local\n`,
  );
  return 0;
}

function main(argv) {
  if (argv.length === 0) return check();
  if (argv.length === 1 && argv[0] === '--self-test') return selfTest();
  process.stderr.write('usage: check-actions-pinned.mjs [--self-test]\n');
  return 2;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main(process.argv.slice(2));
}
