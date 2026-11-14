import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  RELEASE_PASSES,
  applyBackfill,
  buildReport,
  classify,
  emptyHistory,
  ingestRecord,
  parseHistory,
  parseRecord,
  prune,
  renderMarkdown,
} from './flaky-history.mjs';

const TODAY = '2026-11-30';
let runSeq = 1000;

function header(run, attempt, sha, extra = {}) {
  return {
    schema: 1,
    kind: 'header',
    run_id: String(run),
    run_attempt: String(attempt),
    job: 'rust-regression-windows',
    sha,
    suite: 'cargo',
    platform: 'Windows-X64',
    ...extra,
  };
}

function ingest(history, { sha, outcomes, day = TODAY, run, attempt = 1, extra }) {
  const id = run ?? (runSeq += 1);
  const tests = Object.entries(outcomes).map(([testId, outcome]) => ({ test_id: testId, outcome }));
  const record = { header: header(id, attempt, sha, extra), tests };
  return ingestRecord(history, record, { run_id: id, attempt, day, artifact_id: 1 });
}

const key = (id, platform = 'Windows-X64') => `cargo|${platform}|${id}`;
const row = (history, id, today = TODAY) => classify(history, today).find((r) => r.key === key(id));

function padPasses(history, id, count, day = TODAY) {
  for (let i = 0; i < count; i += 1) {
    ingest(history, { sha: `pad${i}`, outcomes: { [id]: 'pass' }, day });
  }
}

test('1: fail then pass on the same sha is flaky; two such shas give flaky-evidence', () => {
  const h = emptyHistory();
  ingest(h, { sha: 's1', run: 1, attempt: 1, outcomes: { t: 'fail' } });
  ingest(h, { sha: 's1', run: 1, attempt: 2, outcomes: { t: 'pass' } });
  const shaEntry = h.tests[key('t')].shas.s1;
  assert.equal(shaEntry.pass, 1);
  assert.equal(shaEntry.fail, 1);
  ingest(h, { sha: 's2', run: 2, attempt: 1, outcomes: { t: 'fail' } });
  ingest(h, { sha: 's2', run: 2, attempt: 2, outcomes: { t: 'pass' } });
  assert.equal(row(h, 't').class, 'insufficient-data');
  padPasses(h, 't', 16);
  assert.equal(row(h, 't').executed, 20);
  assert.equal(row(h, 't').flaky_shas, 2);
  assert.equal(row(h, 't').class, 'flaky-evidence');
});

test('2: failing on every sha without a pass there is failing, never flaky', () => {
  const h = emptyHistory();
  for (let i = 0; i < 5; i += 1) ingest(h, { sha: `f${i}`, outcomes: { t: 'fail' } });
  padPasses(h, 't', 15);
  const r = row(h, 't');
  assert.equal(r.flaky_shas, 0);
  assert.equal(r.class, 'failing');
  assert.equal(r.failure_rate, 5 / 20);
});

test('3: a pass before the fail on the same sha is backfilled once', () => {
  const h = emptyHistory();
  ingest(h, { sha: 's', run: 1, outcomes: { t: 'pass' } });
  const result = ingest(h, { sha: 's', run: 2, outcomes: { t: 'fail' } });
  const earlier = '1/1/rust-regression-windows';
  assert.deepEqual(result.needsBackfill, [{ key: key('t'), sha: 's', recordKeys: [earlier] }]);
  assert.equal(h.tests[key('t')].shas.s.pass, 0);
  applyBackfill(h, key('t'), 's', earlier, 'pass');
  assert.equal(h.tests[key('t')].shas.s.pass, 1);
  assert.deepEqual(h.tests[key('t')].shas.s.open_backfill, []);
  applyBackfill(h, key('t'), 's', earlier, 'pass');
  assert.equal(h.tests[key('t')].shas.s.pass, 1);
});

test('4: the same record key twice is a duplicate and changes nothing', () => {
  const h = emptyHistory();
  ingest(h, { sha: 's', run: 7, outcomes: { t: 'pass' } });
  const before = JSON.stringify(h);
  const result = ingest(h, { sha: 's', run: 7, outcomes: { t: 'fail' } });
  assert.equal(result.duplicate, true);
  assert.equal(JSON.stringify(h), before);
});

test('5: header run_id or attempt different from meta is invalid', () => {
  const h = emptyHistory();
  const record = { header: header(8, 1, 's'), tests: [{ test_id: 't', outcome: 'pass' }] };
  const before = JSON.stringify(h);
  const meta = { run_id: 9, attempt: 1, day: TODAY, artifact_id: 1 };
  assert.equal(ingestRecord(h, record, meta).invalid, true);
  assert.equal(ingestRecord(h, record, { ...meta, run_id: 8, attempt: 2 }).invalid, true);
  assert.equal(JSON.stringify(h), before);
});

test('6: skip and not_run only count as other', () => {
  const h = emptyHistory();
  ingest(h, { sha: 's', outcomes: { a: 'not_run', b: 'skip' } });
  for (const id of ['a', 'b']) {
    const r = row(h, id);
    assert.equal(r.executed, 0);
    assert.equal(r.other, 1);
    assert.deepEqual(h.tests[key(id)].shas, {});
    assert.equal(h.tests[key(id)].streak, 0);
  }
  assert.deepEqual(h.ids, {});
});

test('7: 19 executed with 2 flaky shas is insufficient-data', () => {
  const h = emptyHistory();
  for (const sha of ['s1', 's2']) {
    ingest(h, { sha, outcomes: { t: 'fail' } });
    ingest(h, { sha, outcomes: { t: 'pass' } });
  }
  padPasses(h, 't', 15);
  const r = row(h, 't');
  assert.equal(r.executed, 19);
  assert.equal(r.flaky_shas, 2);
  assert.equal(r.failure_rate, null);
  assert.equal(r.class, 'insufficient-data');
});

test('8: prune drops day 31 back and keeps day 30 back', () => {
  const h = emptyHistory();
  ingest(h, { sha: 'old', run: 1, day: '2026-10-31', outcomes: { gone: 'fail' } });
  ingest(h, { sha: 'start', run: 2, day: '2026-11-01', outcomes: { kept: 'fail' } });
  prune(h, TODAY);
  assert.equal(h.tests[key('gone')], undefined);
  assert.equal(h.records['1/1/rust-regression-windows'], undefined);
  assert.equal(h.runs['1'], undefined);
  assert.deepEqual(Object.keys(h.tests[key('kept')].days), ['2026-11-01']);
  assert.ok(h.tests[key('kept')].shas.start);
  assert.ok(h.records['2/1/rust-regression-windows']);
});

function releaseIds(h) {
  return buildReport(h, TODAY, {}).release_ready.map((item) => item.test_id);
}

test('9: release_ready is a state from the pass streak', () => {
  const h = emptyHistory();
  ingest(h, { sha: 'f', day: '2026-11-20', outcomes: { t: 'fail' } });
  ingest(h, { sha: 'p0', day: '2026-11-21', outcomes: { t: 'pass' } });
  padPasses(h, 't', RELEASE_PASSES - 1, '2026-11-22');
  const ready = buildReport(h, TODAY, {}).release_ready;
  assert.deepEqual(ready, [{ test_id: 't', streak: 50, streak_since: '2026-11-21' }]);
  ingest(h, { sha: 'p51', outcomes: { t: 'pass' } });
  assert.deepEqual(releaseIds(h), ['t']);
  ingest(h, { sha: 'f2', outcomes: { t: 'fail' } });
  assert.deepEqual(releaseIds(h), []);
});

function reportWith(ids) {
  const tests = ids.map((id, i) => ({
    key: `cargo|Linux|${id}`, suite: 'cargo', platform: 'Linux', test_id: id,
    executed: 20, fails: 1, other: 0, failure_rate: 0.05, flaky_shas: i % 3,
    open_backfill: 0, class: 'failing',
  }));
  return {
    window: { start: '2026-11-01', end: TODAY },
    coverage: { runs_listed: 1 },
    tests,
  };
}

test('10: renderMarkdown neutralizes markup and stays under the limit', () => {
  const md = renderMarkdown(reportWith(['a`b|c@d', '::error::boom']));
  assert.ok(md.includes("`a'b/c@​d`"));
  assert.ok(md.includes('`::error::boom`'));
  assert.ok(!md.split('\n').some((line) => line.startsWith('::error::')));
  const big = renderMarkdown(reportWith(Array.from({ length: 5000 }, (_, i) => `t${i}`)));
  assert.ok(big.length <= 60000);
  assert.ok(big.includes('4800 more in the JSON report.'));
});

test('11: parseHistory rejects bad JSON and schema 2', () => {
  assert.equal(parseHistory('{'), null);
  assert.equal(parseHistory(JSON.stringify({ ...emptyHistory(), schema: 2 })), null);
  assert.deepEqual(parseHistory(JSON.stringify(emptyHistory())), emptyHistory());
});

test('12: last_day keeps the max and drives latest_evidence_day', () => {
  const h = emptyHistory();
  ingest(h, { sha: 's', run: 1, day: '2026-11-07', outcomes: { t: 'pass' } });
  ingest(h, { sha: 's', run: 2, day: '2026-11-03', outcomes: { t: 'fail' } });
  ingest(h, { sha: 's', run: 3, day: '2026-11-05', outcomes: { t: 'pass' } });
  assert.equal(h.tests[key('t')].shas.s.last_day, '2026-11-05');
  applyBackfill(h, key('t'), 's', '1/1/rust-regression-windows', 'pass');
  assert.equal(h.tests[key('t')].shas.s.last_day, '2026-11-07');
  ingest(h, { sha: 'u', run: 4, day: '2026-11-04', outcomes: { t: 'fail' } });
  ingest(h, { sha: 'u', run: 5, day: '2026-11-06', outcomes: { t: 'pass' } });
  padPasses(h, 't', 15);
  const report = buildReport(h, TODAY, {});
  assert.deepEqual(report.quarantine_candidates, ['t']);
  assert.equal(report.candidates.t.latest_evidence_day, '2026-11-07');
  assert.equal(report.candidates.t.evidence.length, 2);
});

test('13: building a report twice consumes nothing', () => {
  const h = emptyHistory();
  for (const sha of ['s1', 's2']) {
    ingest(h, { sha, outcomes: { t: 'fail' } });
    ingest(h, { sha, outcomes: { t: 'pass' } });
  }
  padPasses(h, 't', RELEASE_PASSES);
  const first = buildReport(h, TODAY, { runs_listed: 2 });
  const second = buildReport(h, TODAY, { runs_listed: 2 });
  assert.deepEqual(second.candidates, first.candidates);
  assert.deepEqual(second.release_ready, first.release_ready);
  assert.deepEqual(second, first);
});

test('parseRecord keeps valid lines and counts dropped ones', () => {
  const text = [
    JSON.stringify(header(1, 1, 's')),
    JSON.stringify({ test_id: 'ok', outcome: 'pass' }),
    JSON.stringify({ test_id: 'bad\u0001', outcome: 'pass' }),
    JSON.stringify({ test_id: 'x', outcome: 'weird' }),
    '{',
  ].join('\n');
  const record = parseRecord(text);
  assert.deepEqual(record.tests, [{ test_id: 'ok', outcome: 'pass' }]);
  assert.equal(record.dropped, 3);
  assert.equal(parseRecord('{"schema":1}'), null);
});
