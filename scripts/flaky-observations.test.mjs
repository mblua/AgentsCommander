import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import {
  SCHEMA,
  collapseOutcomes,
  identityFromEnv,
  main,
  serializeRecord,
  vitestRecord,
} from './flaky-observations.mjs';

const ROOT = path.resolve('/repo');
const IDENTITY = {
  platform: 'Linux-X64',
  job: 'frontend-regression',
  run_id: '11',
  run_attempt: '2',
  sha: 'abc',
  head_sha: 'def',
  event: 'pull_request',
  repository: 'o/r',
};
const ENV = {
  RUNNER_OS: 'Linux',
  RUNNER_ARCH: 'X64',
  GITHUB_RUN_ID: '11',
  GITHUB_RUN_ATTEMPT: '2',
  GITHUB_SHA: 'abc',
  PR_HEAD: 'def',
  GITHUB_EVENT_NAME: 'pull_request',
  GITHUB_REPOSITORY: 'o/r',
};

const tempDirs = [];
function tempDir() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-obs-'));
  tempDirs.push(dir);
  return dir;
}
after(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

function report(assertions, extra = 0, name = path.join(ROOT, 'src', 'a.test.ts')) {
  return {
    numTotalTests: assertions.length + extra,
    testResults: [{ name, assertionResults: assertions }],
  };
}

function lines(text) {
  return text.trimEnd().split('\n').map((line) => JSON.parse(line));
}

function runMain(dir, content, env = ENV) {
  const input = path.join(dir, 'npm-test-results.json');
  const out = path.join(dir, 'out', 'obs.jsonl');
  if (content !== undefined) fs.writeFileSync(input, content);
  const stderr = process.stderr.write;
  process.stderr.write = () => true;
  try {
    const code = main(['vitest', '--in', input, '--out', out, '--job', 'fe'], env);
    return { code, out };
  } finally {
    process.stderr.write = stderr;
  }
}

test('maps statuses, sorts lines and fills the header', () => {
  const statuses = ['passed', 'failed', 'skipped', 'todo', 'pending', 'weird'];
  const assertions = statuses.map((status, i) => ({ fullName: `t${5 - i}`, status }));
  const record = vitestRecord(report(assertions), { root: ROOT, identity: IDENTITY });
  const out = lines(serializeRecord(record));
  assert.deepEqual(out[0], {
    schema: SCHEMA,
    kind: 'header',
    suite: 'vitest',
    ...IDENTITY,
    sources: [{ file: 'npm-test-results.json', present: true, complete: true, tests: 6 }],
    collapsed: 0,
    rejected: 0,
  });
  assert.deepEqual(out.slice(1), [
    { test_id: 'src/a.test.ts::t0', outcome: 'not_run' },
    { test_id: 'src/a.test.ts::t1', outcome: 'skip' },
    { test_id: 'src/a.test.ts::t2', outcome: 'skip' },
    { test_id: 'src/a.test.ts::t3', outcome: 'skip' },
    { test_id: 'src/a.test.ts::t4', outcome: 'fail' },
    { test_id: 'src/a.test.ts::t5', outcome: 'pass' },
  ]);
});

test('missing fullName joins ancestors and title', () => {
  const assertion = { ancestorTitles: ['A', '', 'B'], title: 'c', status: 'passed' };
  const record = vitestRecord(report([assertion]), { root: ROOT, identity: IDENTITY });
  assert.equal(record.tests[0].test_id, 'src/a.test.ts::A > B > c');
});

test('count mismatch marks every outcome not_run', () => {
  const assertions = [
    { fullName: 'a', status: 'passed' },
    { fullName: 'b', status: 'failed' },
  ];
  const record = vitestRecord(report(assertions, 1), { root: ROOT, identity: IDENTITY });
  assert.equal(record.header.sources[0].complete, false);
  assert.deepEqual(record.tests.map((t) => t.outcome), ['not_run', 'not_run']);
});

test('truncated JSON writes a header-only record', () => {
  const { code, out } = runMain(tempDir(), '{"numTotalTests": 3, "testRes');
  assert.equal(code, 0);
  const out_ = lines(fs.readFileSync(out, 'utf8'));
  assert.equal(out_.length, 1);
  assert.equal(out_[0].job, 'fe');
  assert.deepEqual(out_[0].sources, [
    { file: 'npm-test-results.json', present: true, complete: false, tests: 0 },
  ]);
});

test('missing input writes a header-only record', () => {
  const { code, out } = runMain(tempDir(), undefined);
  assert.equal(code, 0);
  const out_ = lines(fs.readFileSync(out, 'utf8'));
  assert.equal(out_.length, 1);
  assert.deepEqual(out_[0].sources, [
    { file: 'npm-test-results.json', present: false, complete: false, tests: 0 },
  ]);
});

test('duplicate id with pass and fail collapses to fail', () => {
  const assertions = [
    { fullName: 'x', status: 'passed' },
    { fullName: 'x', status: 'failed' },
  ];
  const record = vitestRecord(report(assertions), { root: ROOT, identity: IDENTITY });
  assert.deepEqual(record.tests, [{ test_id: 'src/a.test.ts::x', outcome: 'fail' }]);
  assert.equal(record.header.collapsed, 1);
  const direct = collapseOutcomes([
    { test_id: 'y', outcome: 'skip' },
    { test_id: 'y', outcome: 'not_run' },
    { test_id: 'y', outcome: 'pass' },
  ]);
  assert.deepEqual(direct, { tests: [{ test_id: 'y', outcome: 'pass' }], collapsed: 2 });
});

test('path outside root becomes ? and control chars are rejected', () => {
  const outsideName = path.join(ROOT, '..', 'x.test.ts');
  const outside = report([{ fullName: 'o', status: 'passed' }], 0, outsideName);
  const record = vitestRecord(outside, { root: ROOT, identity: IDENTITY });
  assert.equal(record.tests[0].test_id, '?::o');
  const bad = report([
    { fullName: 'ok', status: 'passed' },
    { fullName: 'bad\nname', status: 'passed' },
    { fullName: 'del\u007f', status: 'passed' },
    { fullName: 'x'.repeat(600), status: 'passed' },
  ]);
  const rejected = vitestRecord(bad, { root: ROOT, identity: IDENTITY });
  assert.equal(rejected.header.rejected, 3);
  assert.equal(rejected.tests.length, 1);
  assert.equal(rejected.header.sources[0].tests, 1);
});

test('missing GITHUB_RUN_ID writes nothing', () => {
  const env = { ...ENV };
  delete env.GITHUB_RUN_ID;
  const dir = tempDir();
  const { code, out } = runMain(dir, JSON.stringify(report([])), env);
  assert.equal(code, 0);
  assert.equal(fs.existsSync(out), false);
});

test('positive control: complete 3-test report gives 3 lines', () => {
  const dir = tempDir();
  const name = path.join(process.cwd(), 'src', 'p.test.ts');
  const assertions = ['a', 'b', 'c'].map((fullName) => ({ fullName, status: 'passed' }));
  const { code, out } = runMain(dir, JSON.stringify(report(assertions, 0, name)));
  assert.equal(code, 0);
  const out_ = lines(fs.readFileSync(out, 'utf8'));
  assert.equal(out_.length, 4);
  assert.equal(out_[0].sources[0].complete, true);
  assert.equal(out_[0].sources[0].tests, 3);
  assert.deepEqual(out_.slice(1).map((l) => l.test_id), [
    'src/p.test.ts::a',
    'src/p.test.ts::b',
    'src/p.test.ts::c',
  ]);
  assert.deepEqual(identityFromEnv({}, 'j').platform, 'unknown');
});
