import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { ApiError, BudgetExhausted } from './flaky-github.mjs';
import { collect, main } from './flaky-rate.mjs';

const TODAY = '2026-11-30';
const DAY = '2026-11-20';
const REPO = { full_name: 'o/r' };
const tempDirs = [];
function tempDir() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-rate-'));
  tempDirs.push(dir);
  return dir;
}
after(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

function recordText(runId, attempt, sha, outcomes, job = 'rust-regression-linux') {
  const header = {
    schema: 1, kind: 'header', suite: 'cargo', platform: 'Linux', job,
    run_id: String(runId), run_attempt: String(attempt), sha,
    sources: [{ file: 'a.log', complete: true }],
  };
  const lines = Object.entries(outcomes).map(([id, outcome]) => ({ test_id: id, outcome }));
  return [header, ...lines].map((line) => JSON.stringify(line)).join('\n');
}

function run(id, extra = {}) {
  return {
    id, run_attempt: 1, conclusion: 'success', created_at: `${DAY}T10:00:${String(id % 60)
      .padStart(2, '0')}Z`, head_repository: REPO, repository: REPO, ...extra,
  };
}

let nextArtifact = 1;
function artifact(name, text, extra = {}) {
  nextArtifact += 1;
  return { id: nextArtifact, name, size_in_bytes: 100, expired: false, text, ...extra };
}

// Fake adapter with the same surface as createGitHub().
function fakeGh({ runs = [], attempts = {}, artifacts = {}, history = null, budget = Infinity,
  dayTotals = {}, failOn = () => false } = {}) {
  const gh = { calls: 0, downloads: [], attemptCalls: [], artifactPages: [] };
  const byId = new Map();
  for (const list of Object.values(artifacts)) for (const a of list) byId.set(a.id, a);
  const call = (what) => {
    if (gh.calls >= budget) throw new BudgetExhausted(budget);
    gh.calls += 1;
    if (failOn(what)) throw new ApiError(what, 1, 500);
  };
  gh.latestHistory = () => {
    call('latest');
    return history;
  };
  gh.listRuns = (file, day, page) => {
    call(`list ${day}`);
    const all = runs.filter((r) => r.created_at.startsWith(day));
    const total = dayTotals[day] ?? all.length;
    return { total_count: total, workflow_runs: all.slice((page - 1) * 100, page * 100) };
  };
  gh.attempt = (runId, n) => {
    call(`attempt ${runId}/${n}`);
    gh.attemptCalls.push(`${runId}/${n}`);
    return { conclusion: attempts[runId]?.[n] ?? 'success' };
  };
  gh.runArtifacts = (runId, page) => {
    call(`artifacts ${runId}`);
    gh.artifactPages.push(`${runId}/${page}`);
    const list = artifacts[runId] ?? [];
    return { total_count: list.length, artifacts: list.slice((page - 1) * 100, page * 100) };
  };
  gh.downloadText = (id) => {
    call(`download ${id}`);
    gh.downloads.push(id);
    const found = byId.get(id);
    if (!found || found.text === 'expired') throw new ApiError(`artifacts/${id}/zip`, 1, 410);
    return found.text;
  };
  return gh;
}

const go = (gh, limits) => collect({ gh, today: TODAY, nowIso: `${TODAY}T05:17:00Z`,
  tmpDir: os.tmpdir(), limits });
const executed = (result, id) =>
  result.report.tests.find((row) => row.test_id === id)?.executed ?? 0;

test('1: a cancelled attempt is excluded even with an artifact', () => {
  const gh = fakeGh({
    runs: [run(1), run(2, { conclusion: 'cancelled' })],
    artifacts: {
      1: [artifact('flaky-obs-j-1', recordText(1, 1, 's', { t: 'pass' }))],
      2: [artifact('flaky-obs-j-1', recordText(2, 1, 's', { t: 'pass' }))],
    },
  });
  const result = go(gh);
  assert.equal(result.report.coverage.records_ingested, 1);
  assert.equal(result.report.coverage.excluded.cancelled, 1);
  assert.equal(executed(result, 't'), 1);
  assert.equal(result.report.coverage.complete, true);
});

test('2: failure then success attempts are both ingested with distinct keys', () => {
  const gh = fakeGh({
    runs: [run(3, { run_attempt: 2 })],
    attempts: { 3: { 1: 'failure' } },
    artifacts: {
      3: [
        artifact('flaky-obs-j-1', recordText(3, 1, 's', { t: 'fail' })),
        artifact('flaky-obs-j-2', recordText(3, 2, 's', { t: 'pass' })),
      ],
    },
  });
  const result = go(gh);
  assert.deepEqual(Object.keys(result.history.records).sort(), [
    '3/1/rust-regression-linux',
    '3/2/rust-regression-linux',
  ]);
  assert.equal(result.history.runs[3].attempt, 2);
});

test('3: a fork run is excluded without downloads', () => {
  const gh = fakeGh({
    runs: [run(4, { head_repository: { full_name: 'x/r' } })],
    artifacts: { 4: [artifact('flaky-obs-j-1', recordText(4, 1, 's', { t: 'pass' }))] },
  });
  const result = go(gh);
  assert.equal(result.report.coverage.excluded.fork, 1);
  assert.deepEqual(gh.downloads, []);
  assert.equal(result.history.runs[4].attempt, 1);
});

function twoRunSetup(history = null) {
  return fakeGh({
    history,
    runs: [run(5), run(6)],
    artifacts: {
      5: [artifact('flaky-obs-j-1', recordText(5, 1, 's', { t: 'pass' }))],
      6: [artifact('flaky-obs-j-1', recordText(6, 1, 's', { t: 'pass' }))],
    },
  });
}

test('4: a second collect with the first history downloads nothing', () => {
  const first = go(twoRunSetup());
  const gh = twoRunSetup(JSON.stringify(first.history));
  const second = go(gh);
  assert.deepEqual(gh.downloads, []);
  assert.equal(second.report.coverage.history_reset, false);
  assert.equal(executed(second, 't'), 2);
});

test('5: the download limit stops early and the next collect resumes', () => {
  const runs = [run(7), run(8), run(9)];
  const artifacts = {};
  for (const r of runs) {
    artifacts[r.id] = [artifact('flaky-obs-j-1', recordText(r.id, 1, 's', { t: 'pass' }))];
  }
  const first = go(fakeGh({ runs, artifacts }), { maxDownloads: 2 });
  assert.equal(first.report.coverage.budget_exhausted, true);
  assert.equal(first.history.runs[9], undefined);
  assert.equal(first.report.coverage.runs_pending, 1);
  const gh = fakeGh({ runs, artifacts, history: JSON.stringify(first.history) });
  const second = go(gh, { maxDownloads: 2 });
  assert.equal(gh.downloads.length, 1);
  assert.equal(executed(second, 't'), 3);
  assert.equal(second.report.coverage.complete, true);
});

test('6: a pass before the fail on the same sha is backfilled from the cache', () => {
  const gh = fakeGh({
    runs: [run(10, { run_attempt: 2 })],
    attempts: { 10: { 1: 'failure' } },
    artifacts: {
      10: [
        artifact('flaky-obs-j-1', recordText(10, 1, 's', { t: 'pass' })),
        artifact('flaky-obs-j-2', recordText(10, 2, 's', { t: 'fail' })),
      ],
    },
  });
  const result = go(gh);
  const shaEntry = result.history.tests['cargo|Linux|t'].shas.s;
  assert.equal(shaEntry.pass, 1);
  assert.equal(shaEntry.fail, 1);
  assert.deepEqual(shaEntry.open_backfill, []);
  assert.equal(gh.downloads.length, 2);
});

test('7: an expired backfill artifact closes the item without a pass', () => {
  const first = go(fakeGh({
    runs: [run(11)],
    artifacts: { 11: [artifact('flaky-obs-j-1', recordText(11, 1, 's', { t: 'pass' }))] },
  }));
  const expired = artifact('flaky-obs-j-1', 'expired');
  first.history.records['11/1/rust-regression-linux'].artifact_id = expired.id;
  const gh = fakeGh({
    history: JSON.stringify(first.history),
    runs: [run(11), run(12)],
    artifacts: {
      11: [expired],
      12: [artifact('flaky-obs-j-1', recordText(12, 1, 's', { t: 'fail' }))],
    },
  });
  const result = go(gh);
  const shaEntry = result.history.tests['cargo|Linux|t'].shas.s;
  assert.equal(shaEntry.pass, 0);
  assert.deepEqual(shaEntry.open_backfill, []);
  assert.equal(result.report.coverage.api_error, null);
});

function withQuietStdout(fn) {
  const write = process.stdout.write;
  process.stdout.write = () => true;
  try {
    return fn();
  } finally {
    process.stdout.write = write;
  }
}

test('8: an ApiError keeps earlier data and main returns 1', () => {
  const setup = () => fakeGh({
    runs: [run(13), run(14)],
    artifacts: {
      13: [artifact('flaky-obs-j-1', recordText(13, 1, 's', { t: 'pass' }))],
      14: [artifact('flaky-obs-j-1', recordText(14, 1, 's', { t: 'pass' }))],
    },
    failOn: (what) => what === 'artifacts 14',
  });
  const result = go(setup());
  assert.match(result.report.coverage.api_error, /artifacts 14/);
  assert.equal(executed(result, 't'), 1);
  assert.match(result.markdown.split('\n')[0], /^Report NOT complete: API error/);
  const out = tempDir();
  const code = withQuietStdout(() =>
    main(['--repo', 'o/r', '--out', out], {}, {
      gh: setup(),
      now: new Date(`${TODAY}T05:17:00Z`),
    }));
  assert.equal(code, 1);
  for (const file of ['history.json', 'report.json', 'report.md']) {
    assert.ok(fs.existsSync(path.join(out, file)));
  }
});

test('9: an oversize artifact is excluded without a download', () => {
  const gh = fakeGh({
    runs: [run(15)],
    artifacts: {
      15: [artifact('flaky-obs-j-1', 'x', { size_in_bytes: 20 * 1024 * 1024 + 1 })],
    },
  });
  const result = go(gh);
  assert.equal(result.report.coverage.excluded.oversize, 1);
  assert.deepEqual(gh.downloads, []);
});

test('10: bootstrap spreads over days under the call budget', () => {
  const start = { created_at: '2026-11-01T01:00:00Z' };
  const runs = Array.from({ length: 50 }, (_, i) => run(100 + i, start));
  const first = fakeGh({ runs, budget: 20 });
  const out = tempDir();
  const code = withQuietStdout(() =>
    main(['--repo', 'o/r', '--out', out], {}, { gh: first, now: new Date(`${TODAY}T05:17:00Z`) }));
  assert.equal(code, 0);
  assert.equal(first.calls, 20);
  const report = JSON.parse(fs.readFileSync(path.join(out, 'report.json'), 'utf8'));
  assert.equal(report.coverage.history_reset, true);
  assert.equal(report.coverage.budget_exhausted, true);
  assert.equal(report.coverage.api_error, null);
  assert.ok(report.coverage.runs_pending > 0);
  let history = fs.readFileSync(path.join(out, 'history.json'), 'utf8');
  let result;
  for (let day = 0; day < 10; day += 1) {
    result = go(fakeGh({ runs, budget: 80, history }));
    history = JSON.stringify(result.history);
    if (result.report.coverage.complete) break;
  }
  assert.equal(result.report.coverage.complete, true);
  assert.equal(Object.keys(result.history.runs).length, 50);
  const unbounded = go(fakeGh({ runs }));
  assert.deepEqual(Object.keys(unbounded.history.runs).sort(),
    Object.keys(result.history.runs).sort());
});

test('11: a day over the search limit is reported as truncated', () => {
  const gh = fakeGh({ runs: [run(16)], dayTotals: { [DAY]: 1001 } });
  const result = go(gh);
  assert.deepEqual(result.report.coverage.list_truncated_days, [DAY]);
  assert.equal(result.report.coverage.complete, false);
  assert.match(result.markdown.split('\n')[0], /listing truncated on 2026-11-20/);
});

test('12: the latest attempt uses the listed conclusion', () => {
  const gh = fakeGh({ runs: [run(17, { run_attempt: 3 })], attempts: { 17: { 1: 'failure' } } });
  go(gh);
  assert.deepEqual(gh.attemptCalls, ['17/1', '17/2']);
});

test('13: artifacts on a second page are read', () => {
  const list = Array.from({ length: 119 }, (_, i) => artifact(`other-${i}`, 'x'));
  list.push(artifact('flaky-obs-j-1', recordText(18, 1, 's', { t: 'pass' })));
  const gh = fakeGh({ runs: [run(18)], artifacts: { 18: list } });
  const result = go(gh);
  assert.deepEqual(gh.artifactPages, ['18/1', '18/2']);
  assert.equal(executed(result, 't'), 1);
});

test('14: a corrupt artifact is excluded and the next one is ingested', () => {
  const gh = fakeGh({
    runs: [run(19)],
    artifacts: {
      19: [
        artifact('flaky-obs-a-1', null),
        artifact('flaky-obs-b-1', 'not json'),
        artifact('flaky-obs-c-1', recordText(19, 1, 's', { t: 'pass' }, 'c')),
      ],
    },
  });
  const result = go(gh);
  assert.equal(result.report.coverage.excluded.corrupt, 2);
  assert.equal(result.report.coverage.records_ingested, 1);
  assert.equal(result.report.coverage.api_error, null);
});
