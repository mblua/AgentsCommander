import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ApiError, BudgetExhausted, createGitHub } from './flaky-github.mjs';

const tempDirs = [];
function tempDir() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-gh-'));
  tempDirs.push(dir);
  return dir;
}
after(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

const ok = (value) => ({ status: 0, stdout: Buffer.from(JSON.stringify(value)), stderr: '' });

function fakeRun(responder) {
  const calls = [];
  const run = (cmd, args) => {
    calls.push([cmd, ...args]);
    return responder(cmd, args);
  };
  return { run, calls };
}

const A = 'repos/o/r/actions';

test('each method passes an exact argument array', () => {
  const { run, calls } = fakeRun(() => ok({ total_count: 0, workflow_runs: [], artifacts: [] }));
  const gh = createGitHub({ repo: 'o/r', run });
  gh.listRuns('pr.yml', '2026-11-01', 2);
  gh.attempt(5, 1);
  gh.runArtifacts(5, 3);
  assert.deepEqual(calls, [
    ['gh', 'api',
      `${A}/workflows/pr.yml/runs?status=completed&created=2026-11-01&per_page=100&page=2`],
    ['gh', 'api', `${A}/runs/5/attempts/1`],
    ['gh', 'api', `${A}/runs/5/artifacts?per_page=100&page=3`],
  ]);
  assert.equal(gh.calls, 3);
});

test('a non-zero status throws ApiError without token text', () => {
  const stderr = Buffer.from('HTTP 403: secret ghp_TOKEN123 rejected');
  const { run } = fakeRun(() => ({ status: 1, stdout: Buffer.alloc(0), stderr }));
  const gh = createGitHub({ repo: 'o/r', run });
  assert.throws(
    () => gh.attempt(5, 1),
    (error) =>
      error instanceof ApiError &&
      error.http === 403 &&
      error.message.includes(`${A}/runs/5/attempts/1`) &&
      !error.message.includes('ghp_TOKEN123'),
  );
});

test('downloadText unzips the member and deletes the zip', () => {
  const dir = tempDir();
  const { run, calls } = fakeRun((cmd, args) => {
    if (cmd === 'gh') return { status: 0, stdout: Buffer.from('ZIP'), stderr: '' };
    assert.ok(fs.existsSync(args[1]));
    return args[2] === 'm.jsonl'
      ? { status: 0, stdout: Buffer.from('text'), stderr: '' }
      : { status: 11, stdout: Buffer.alloc(0), stderr: '' };
  });
  const gh = createGitHub({ repo: 'o/r', run });
  assert.equal(gh.downloadText(9, 'm.jsonl', dir), 'text');
  assert.equal(gh.downloadText(9, 'other', dir), null);
  assert.deepEqual(calls[0], ['gh', 'api', `${A}/artifacts/9/zip`]);
  assert.deepEqual(calls[1], ['unzip', '-p', path.join(dir, '9.zip'), 'm.jsonl']);
  assert.equal(gh.calls, 2);
  assert.deepEqual(fs.readdirSync(dir), []);
});

test('latestHistory skips expired artifacts and returns null when none', () => {
  const dir = tempDir();
  const artifacts = {
    1: [{ id: 11, name: 'flaky-rate-history', expired: true }],
    2: [{ id: 21, name: 'other' }, { id: 22, name: 'flaky-rate-history', expired: false }],
  };
  const responder = (cmd, args) => {
    if (cmd === 'unzip') return { status: 0, stdout: Buffer.from('{"h":1}'), stderr: '' };
    const endpoint = args[1];
    if (endpoint.includes('branch=main')) return ok({ workflow_runs: [{ id: 1 }, { id: 2 }] });
    if (endpoint.endsWith('/zip')) return { status: 0, stdout: Buffer.from('Z'), stderr: '' };
    const runId = /runs\/(\d+)\/artifacts/.exec(endpoint)[1];
    return ok({ total_count: artifacts[runId].length, artifacts: artifacts[runId] });
  };
  const { run, calls } = fakeRun(responder);
  const gh = createGitHub({ repo: 'o/r', run });
  assert.equal(gh.latestHistory('flaky-rate.yml', dir), '{"h":1}');
  assert.ok(calls.some((c) => c[2] === `${A}/artifacts/22/zip`));
  assert.ok(!calls.some((c) => c[2] === `${A}/artifacts/11/zip`));
  assert.ok(calls[0][2].includes('workflows/flaky-rate.yml/runs?branch=main&status=completed'));
  artifacts[2] = [];
  assert.equal(createGitHub({ repo: 'o/r', run }).latestHistory('flaky-rate.yml', dir), null);
});

test('budget 2: the third call throws BudgetExhausted and never runs', () => {
  const { run, calls } = fakeRun(() => ok({}));
  const gh = createGitHub({ repo: 'o/r', run, budget: 2 });
  gh.attempt(1, 1);
  gh.attempt(1, 2);
  assert.throws(() => gh.attempt(1, 3), BudgetExhausted);
  assert.equal(calls.length, 2);
  assert.equal(gh.calls, 2);
});

const I = 'repos/o/r/issues';

function inputRun(responder) {
  const calls = [];
  const run = (cmd, args, input) => {
    calls.push({ args: [cmd, ...args], input });
    return responder(args);
  };
  return { run, calls };
}

test('issue methods: exact args, bodies only on stdin', () => {
  const { run, calls } = inputRun((args) =>
    ok(args[1].includes('?labels=') ? [{ number: 1 }, { number: 2, pull_request: {} }] : []),
  );
  const gh = createGitHub({ repo: 'o/r', run });
  assert.deepEqual(gh.listLabeledIssues('flaky-test', 2), { issues: [{ number: 1 }], more: false });
  gh.listComments(7, 1);
  gh.listComments(7, 3);
  gh.createIssue('T @x', 'B `x`', ['flaky-test']);
  gh.reopenIssue(7);
  gh.comment(7, 'C');
  const write = (method, endpoint) => ['gh', 'api', '--method', method, endpoint, '--input', '-'];
  assert.deepEqual(
    calls.map((c) => c.args),
    [
      ['gh', 'api', `${I}?labels=flaky-test&state=all&per_page=100&page=2`],
      ['gh', 'api', `${I}/7/comments?per_page=100&page=1`],
      ['gh', 'api', `${I}/7/comments?per_page=100&page=3`],
      write('POST', I),
      write('PATCH', `${I}/7`),
      write('POST', `${I}/7/comments`),
    ],
  );
  assert.deepEqual(JSON.parse(calls[3].input), {
    title: 'T @x',
    body: 'B `x`',
    labels: ['flaky-test'],
  });
  assert.deepEqual(JSON.parse(calls[4].input), { state: 'open' });
  assert.deepEqual(JSON.parse(calls[5].input), { body: 'C' });
  for (const call of calls) assert.ok(!call.args.some((a) => a.includes('B `x`') || a === 'C'));
});

function labelRun(getStatus, postStatus) {
  return inputRun((args) => {
    const status = args.includes('POST') ? postStatus : getStatus;
    if (status === 0) return ok({});
    return { status: 1, stdout: Buffer.alloc(0), stderr: Buffer.from(`HTTP ${status}`) };
  });
}

test('ensureLabel: present = one GET; 404 -> POST; POST 422 ok; other errors throw', () => {
  let fake = labelRun(0, 0);
  createGitHub({ repo: 'o/r', run: fake.run }).ensureLabel('flaky-test');
  assert.deepEqual(fake.calls.map((c) => c.args), [['gh', 'api', 'repos/o/r/labels/flaky-test']]);
  fake = labelRun(404, 0);
  createGitHub({ repo: 'o/r', run: fake.run }).ensureLabel('flaky-test');
  assert.deepEqual(fake.calls[1].args,
    ['gh', 'api', '--method', 'POST', 'repos/o/r/labels', '--input', '-']);
  assert.deepEqual(JSON.parse(fake.calls[1].input), {
    name: 'flaky-test',
    color: 'd93f0b',
    description: 'Flaky test (advisory, #4527)',
  });
  fake = labelRun(404, 422);
  createGitHub({ repo: 'o/r', run: fake.run }).ensureLabel('flaky-test');
  fake = labelRun(404, 500);
  assert.throws(() => createGitHub({ repo: 'o/r', run: fake.run }).ensureLabel('x'), ApiError);
  fake = labelRun(403, 0);
  assert.throws(() => createGitHub({ repo: 'o/r', run: fake.run }).ensureLabel('x'), ApiError);
});

test('AC-4527-006: a PR on a full page does not stop paging or duplicate an issue', async () => {
  const { marker, runQuarantine } = await import('./flaky-quarantine.mjs');
  const items = [];
  for (let n = 1; n <= 150; n += 1) {
    if (n === 50) items.push({ number: n, pull_request: {} });
    else if (n === 120) {
      items.push({ number: n, state: 'closed', closed_at: '2026-11-10T00:00:00Z',
        body: marker('t::120') });
    } else items.push({ number: n, state: 'open', body: '' });
  }
  const pages = [];
  const { run, calls } = inputRun((args) => {
    const endpoint = args.find((a) => a.startsWith(I));
    const page = Number(/[?&]page=(\d+)/.exec(endpoint)?.[1] ?? 1);
    if (endpoint.includes('?labels=')) {
      pages.push(page);
      return ok(items.slice((page - 1) * 100, page * 100));
    }
    return ok([]);
  });
  const gh = createGitHub({ repo: 'o/r', run });
  const report = {
    tests: [],
    quarantine_candidates: ['t::120'],
    candidates: {
      't::120': { evidence: [{ platform: 'linux', sha: 's', last_day: '2026-11-01' }] },
    },
    release_ready: [],
  };
  const result = runQuarantine({ gh, report, nowIso: 'x', runUrl: 'U' });
  assert.deepEqual(pages, [1, 2]);
  assert.deepEqual(result.applied, []);
  // Issue #120 is found (old evidence, so not read: known F1), never created again.
  assert.deepEqual(result.overflow.notices,
    [{ test_id: 't::120', kind: 'reopen', reason: 'not_read' }]);
  assert.ok(!calls.some((c) => c.args.includes('POST')));
});
