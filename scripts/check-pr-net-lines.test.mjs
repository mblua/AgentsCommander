import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  NET_LINE_BUDGET,
  classifyPath,
  evaluate,
  extractJustification,
  parseNumstatZ,
  summarize,
} from './check-pr-net-lines.mjs';

const SCRIPT = fileURLToPath(new URL('./check-pr-net-lines.mjs', import.meta.url));
const TEMPLATE = fileURLToPath(new URL('../.github/PULL_REQUEST_TEMPLATE.md', import.meta.url));
const GIT_CONFIG = ['-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'core.autocrlf=false',
  '-c', 'commit.gpgsign=false'];
const HEADING = '## Net line growth justification';
const REASON = 'This growth adds a new subsystem that cannot be split further.';

const tempDirs = [];
after(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

function git(dir, ...args) {
  return execFileSync('git', [...GIT_CONFIG, ...args], { cwd: dir, encoding: 'utf8' }).trim();
}

function write(dir, file, text) {
  const target = path.join(dir, file);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, text);
}

function lines(n) {
  return Array.from({ length: n }, (_, i) => `line ${i}\n`).join('');
}

function commit(dir) {
  git(dir, 'add', '-A');
  git(dir, 'commit', '-q', '-m', 'c');
  return git(dir, 'rev-parse', 'HEAD');
}

// Temp repo: base commit with one file, then `change(dir)` makes the head commit.
function repo(change, baseFiles = { 'README.txt': 'base\n' }) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pr-net-lines-'));
  tempDirs.push(dir);
  git(dir, 'init', '-q');
  for (const [file, text] of Object.entries(baseFiles)) write(dir, file, text);
  const base = commit(dir);
  change(dir);
  return { dir, base, head: commit(dir) };
}

function cli({ dir, base, head }, body, extra = []) {
  const env = { ...process.env, PR_BODY: body };
  delete env.GITHUB_STEP_SUMMARY;
  if (body === undefined) delete env.PR_BODY;
  const args = extra.length > 0 ? extra : ['--base', base, '--head', head];
  const run = spawnSync(process.execPath, [SCRIPT, ...args], { cwd: dir, env, encoding: 'utf8' });
  return { code: run.status, out: run.stdout };
}

const adds = (n) => repo((dir) => write(dir, 'src/a.rs', lines(n)));
const justified = `${HEADING}\n\n${REASON}\n`;

test('budget constant is the literal 1000', () => {
  assert.equal(NET_LINE_BUDGET, 1000);
});

test('classifyPath: first matching class wins', () => {
  assert.equal(classifyPath('Cargo.lock'), 'lockfile');
  assert.equal(classifyPath('src-tauri/Cargo.lock'), 'lockfile');
  assert.equal(classifyPath('src/bindings/x.ts'), 'generated');
  assert.equal(classifyPath('tml-baseline.json'), 'generated');
  assert.equal(classifyPath('scripts/check-pr-net-lines.test.mjs'), 'tests');
  assert.equal(classifyPath('src-tauri/src/a_tests.rs'), 'tests');
  assert.equal(classifyPath('docs/x.md'), 'docs');
  assert.equal(classifyPath('src/App.tsx'), 'source');
  assert.equal(classifyPath('src-tauri/src/lib.rs'), 'source');
  assert.equal(classifyPath('crates/core/src/lib.rs'), 'source');
  assert.equal(classifyPath('.github/workflows/x.yml'), 'other');
});

test('parser: rename token uses the new path', () => {
  assert.deepEqual(parseNumstatZ('0\t0\t\0docs/data.json\0config/data.json\0'),
    [{ path: 'config/data.json', additions: 0, deletions: 0, binary: false }]);
});

test('parser: binary token counts 0', () => {
  assert.deepEqual(parseNumstatZ('-\t-\tassets/x.png\0'),
    [{ path: 'assets/x.png', additions: 0, deletions: 0, binary: true }]);
});

test('parser: malformed token throws', () => {
  assert.throws(() => parseNumstatZ('x\0'));
});

test('summarize: all six classes, counted = source + tests + other', () => {
  const summary = summarize([
    { path: 'src/a.ts', additions: 10, deletions: 2, binary: false },
    { path: 'a.test.mjs', additions: 5, deletions: 0, binary: false },
    { path: 'ci.yml', additions: 1, deletions: 4, binary: false },
    { path: 'docs/a.md', additions: 100, deletions: 0, binary: false },
  ]);
  assert.deepEqual(Object.keys(summary.classes).sort(),
    ['docs', 'generated', 'lockfile', 'other', 'source', 'tests']);
  assert.equal(summary.counted, 8 + 5 - 3);
  assert.deepEqual(summary.classes.docs, { additions: 100, deletions: 0, net: 100, files: 1 });
});

test('positive control: 1001 counted lines without justification fails', () => {
  const result = cli(adds(1001), '');
  assert.equal(result.code, 1);
  assert.match(result.out, /::error::Net line growth 1001 exceeds budget 1000/);
});

test('justified: 1001 counted lines with a written section passes', () => {
  assert.equal(cli(adds(1001), justified).code, 0);
});

test('boundary: exactly 1000 counted lines passes', () => {
  const result = cli(adds(1000), undefined);
  assert.equal(result.code, 0);
  assert.match(result.out, /Counted net: 1000\./);
});

test('excluded classes: docs and lockfile growth is not counted', () => {
  const result = cli(repo((dir) => {
    write(dir, 'docs/x.md', lines(5000));
    write(dir, 'Cargo.lock', lines(5000));
  }), '');
  assert.equal(result.code, 0);
  assert.match(result.out, /Counted net: 0\./);
});

test('rename across classes nets 0 with rename detection', () => {
  const move = (dir) => {
    fs.mkdirSync(path.join(dir, 'config'));
    git(dir, 'mv', 'docs/data.json', 'config/data.json');
  };
  const result = cli(repo(move, { 'docs/data.json': lines(2000) }), '');
  assert.equal(result.code, 0);
  assert.match(result.out, /Counted net: 0\./);
  assert.match(result.out, /\| other \| 1 \| 0 \| 0 \| 0 \| yes \|/);
});

test('comment-only template section is not a justification', () => {
  const template = fs.readFileSync(TEMPLATE, 'utf8');
  assert.ok(template.includes(HEADING));
  assert.equal(extractJustification(template), '');
  const verdict = evaluate({ counted: 1001 }, template);
  assert.equal(verdict.justified, false);
  assert.equal(verdict.exitCode, 1);
});

test('duplicate heading: only the first section counts', () => {
  const body = `${HEADING}\n\n<!-- fill me -->\n\n${HEADING}\n\n${REASON}\n`;
  assert.equal(extractJustification(body), '');
  const result = cli(adds(1001), body);
  assert.equal(result.code, 1);
  assert.match(result.out, /fill the existing one/);
});

test('CRLF body: justified and returned text has no carriage return', () => {
  const body = `## Why\r\nx\r\n${HEADING}\r\n\r\n${REASON}\r\nmore text\r\n## Next\r\nno\r\n`;
  const text = extractJustification(body);
  assert.equal(text, `${REASON}\nmore text`);
  assert.ok(!text.includes('\r'));
  assert.equal(evaluate({ counted: 1001 }, body).exitCode, 0);
});

test('bad args and unknown SHA exit 2', () => {
  const target = adds(1);
  assert.equal(cli(target, '', ['--base', 'zz', '--head', target.head]).code, 2);
  assert.equal(cli(target, '', ['--head', target.head]).code, 2);
  const unknown = 'f'.repeat(40);
  assert.equal(cli(target, '', ['--base', unknown, '--head', target.head]).code, 2);
});
