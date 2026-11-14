import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ApiError } from './flaky-github.mjs';
import {
  MAX_COMMENT_LISTS,
  main,
  marker,
  planActions,
  runQuarantine,
  sha256hex,
} from './flaky-quarantine.mjs';

const tempDirs = [];
after(() => {
  for (const dir of tempDirs) fs.rmSync(dir, { recursive: true, force: true });
});

const CLOSED = ['clo', 'sed'].join('');
const sha = (n) => String(n).padStart(40, '0');
const ev = (h, s) => `<!-- flaky-evidence:v1:${h}:${s} -->`;

function report({ candidates = {}, release = [] } = {}) {
  const ids = Object.keys(candidates);
  const tests = ids.map((id) => ({
    test_id: id, suite: 'vitest', platform: 'linux', executed: 30, fails: 3,
    failure_rate: 0.1, flaky_shas: 2, class: 'flaky-evidence',
  }));
  const map = {};
  for (const id of ids) {
    map[id] = { evidence: candidates[id].map(([s, day]) => ({ platform: 'linux', sha: s,
      last_day: day })) };
  }
  return { tests, quarantine_candidates: ids, candidates: map, release_ready: release };
}

// Stateful fake GitHub: issues and comments survive between runs.
function fakeGh(issues = [], { failOn } = {}) {
  const state = { issues, comments: {}, writes: [], next: 1000 };
  const write = (kind, payload) => {
    state.writes.push({ kind, ...payload });
    if (failOn?.(state.writes.length, kind, payload)) {
      state.writes.pop();
      throw new ApiError('fake', 1, 500);
    }
  };
  const gh = {
    state,
    listLabeledIssues: (label, page) => (page === 1 ? state.issues.map((i) => ({ ...i })) : []),
    listComments: (n, page) => (page === 1 ? (state.comments[n] ?? []) : []),
    ensureLabel: () => {},
    createIssue: (t, body, labels) => {
      write('create', { t, body, labels });
      state.next += 1;
      state.issues.push({ number: state.next, state: 'open', body, title: t });
      return { number: state.next };
    },
    reopenIssue: (n) => {
      write('reopen', { n });
      state.issues.find((i) => i.number === n).state = 'open';
    },
    comment: (n, body) => {
      write('comment', { n, body });
      (state.comments[n] ??= []).push({ body });
    },
  };
  return gh;
}

const run = (gh, rep) => runQuarantine({ gh, report: rep, nowIso: 'x', runUrl: 'U' });
const issueFor = (id, number, extra = {}) =>
  ({ number, state: 'open', body: `${marker(id)}\n`, ...extra });

function allMarkers(gh, n) {
  const issue = gh.state.issues.find((i) => i.number === n);
  const text = [issue.body, ...(gh.state.comments[n] ?? []).map((c) => c.body)].join('\n');
  return text.match(/<!-- flaky-(?:evidence|release):[^>]+-->/g) ?? [];
}

test('1: five candidates, no issues -> 3 creates, 2 overflow', () => {
  const cands = {};
  for (const n of [1, 2, 3, 4, 5]) cands[`t${n}`] = [[sha(n), '2026-11-01']];
  const plan = planActions(report({ candidates: cands }), [], new Map(), 'x');
  assert.deepEqual(plan.actions.map((a) => a.type), ['create', 'create', 'create']);
  assert.equal(plan.overflow.notices.length, 2);
});

test('2: closed read issue, new SHA after close -> reopen + comment, never close', () => {
  const h = sha256hex('t');
  const issue = issueFor('t', 5, { state: CLOSED, closed_at: '2026-11-03T10:00:00Z' });
  issue.body += ev(h, sha(1));
  const rep = report({ candidates: { t: [[sha(1), '2026-11-01'], [sha(2), '2026-11-04']] } });
  const plan = planActions(rep, [issue], new Map([[5, new Set([ev(h, sha(1))])]]), 'x');
  assert.deepEqual(plan.actions.map((a) => a.type), ['reopen', 'comment']);
  assert.ok(plan.actions[1].body.includes(ev(h, sha(2))));
  assert.ok(!plan.actions[1].body.includes(ev(h, sha(1))));
});

test('3: open issue missing a marker -> one comment; all present -> none', () => {
  const h = sha256hex('t');
  const rep = report({ candidates: { t: [[sha(1), '2026-11-01'], [sha(2), '2026-11-02']] } });
  const issue = issueFor('t', 5);
  let plan = planActions(rep, [issue], new Map([[5, new Set([ev(h, sha(1))])]]), 'x');
  assert.equal(plan.actions.length, 1);
  assert.ok(plan.actions[0].body.includes(ev(h, sha(2))));
  plan = planActions(rep, [issue], new Map([[5, new Set([ev(h, sha(1)), ev(h, sha(2))])]]), 'x');
  assert.equal(plan.actions.length, 0);
});

test('4: release comment once, retried after error, again for a new streak', () => {
  const D = '2026-11-01';
  const rel = (since) => [{ test_id: 't', streak: 50, streak_since: since }];
  const issue = issueFor('t', 5, { created_at: '2026-11-02T00:00:00Z' });
  let gh = fakeGh([{ ...issue }]);
  assert.equal(run(gh, report({ release: rel(D) })).applied.length, 1);
  assert.equal(run(gh, report({ release: rel(D) })).applied.length, 0);
  gh = fakeGh([{ ...issue }], { failOn: (n) => n === 1 });
  assert.ok(run(gh, report({ release: rel(D) })).error);
  const fake2 = fakeGh(gh.state.issues);
  fake2.state.comments = gh.state.comments;
  assert.equal(run(fake2, report({ release: rel(D) })).applied.length, 1);
  assert.equal(run(fake2, report({ release: rel(D) })).applied.length, 0);
  assert.equal(run(fake2, report({ release: rel('2026-11-09') })).applied.length, 1);
  assert.equal(allMarkers(fake2, 5).length, 2);
  assert.equal(new Set(allMarkers(fake2, 5)).size, 2);
});

test('5: marker hash exact, other hash no match, duplicates -> lowest', () => {
  assert.equal(marker('a'), `<!-- flaky-test-id:v1:${sha256hex('a')} -->`);
  assert.equal(sha256hex('a').length, 64);
  const rep = report({ candidates: { t: [[sha(1), '2026-11-01']] } });
  const other = { number: 2, state: 'open', body: marker('u') };
  let plan = planActions(rep, [other], new Map(), 'x');
  assert.equal(plan.actions[0].type, 'create');
  const issues = [issueFor('t', 9), issueFor('t', 4)];
  plan = planActions(rep, issues, new Map([[4, new Set()]]), 'x');
  assert.equal(plan.actions[0].issue, 4);
  assert.deepEqual(plan.overflow.duplicates, [9]);
});

test('6: hostile test name stays inside a longer fence; nothing on stdout', () => {
  const id = 'a ``` @user | ::warning:: x > name ````';
  const writes = [];
  const original = process.stdout.write;
  process.stdout.write = (chunk) => (writes.push(chunk), true);
  let plan;
  try {
    plan = planActions(report({ candidates: { [id]: [[sha(1), '2026-11-01']] } }), [], [], 'x');
  } finally {
    process.stdout.write = original;
  }
  const { body, title } = plan.actions[0];
  assert.ok(body.includes(`\`\`\`\`\`text\n${id}\n\`\`\`\`\``));
  assert.ok(!/@user/.test(title));
  assert.ok(!/@user/.test(body.split('`````')[2]));
  assert.deepEqual(writes, []);
});

test('7: ApiError on 2nd write -> first kept, error recorded, exit 1', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-q-'));
  tempDirs.push(dir);
  const rep = report({ candidates: { a: [[sha(1), 'd']], b: [[sha(2), 'd']] } });
  fs.writeFileSync(path.join(dir, 'r.json'), JSON.stringify(rep));
  const gh = fakeGh([], { failOn: (n) => n === 2 });
  const argv = ['--repo', 'o/r', '--report', path.join(dir, 'r.json'), '--out', dir];
  assert.equal(main(argv, {}, { gh }), 1);
  const out = JSON.parse(fs.readFileSync(path.join(dir, 'quarantine.json'), 'utf8'));
  assert.equal(out.applied.length, 1);
  assert.equal(out.error.http, 500);
});

function manyOpen(count) {
  const cands = {};
  const issues = [];
  for (let n = 1; n <= count; n += 1) {
    cands[`t${n}`] = [[sha(n), '2026-11-01']];
    issues.push(issueFor(`t${n}`, n));
  }
  return { rep: report({ candidates: cands }), issues };
}

test('8: 11 comment actions -> 10 applied, 1 overflow', () => {
  const { rep, issues } = manyOpen(11);
  const result = run(fakeGh(issues), rep);
  assert.equal(result.applied.length, 10);
  assert.equal(result.overflow.notices.length, 1);
});

test('9: cap over two runs delivers all 12 and repeats none', () => {
  const { rep, issues } = manyOpen(12);
  const gh = fakeGh(issues);
  assert.equal(run(gh, rep).applied.length, 10);
  assert.equal(run(gh, rep).applied.length, 2);
  assert.equal(run(gh, rep).applied.length, 0);
  for (let n = 1; n <= 12; n += 1) assert.equal(allMarkers(gh, n).length, 1);
});

test('10: ApiError on 1st comment -> next run delivers it once', () => {
  const { rep, issues } = manyOpen(1);
  const gh = fakeGh(issues, { failOn: (n) => n === 1 });
  assert.ok(run(gh, rep).error);
  const again = fakeGh(gh.state.issues);
  assert.equal(run(again, rep).applied.length, 1);
  assert.equal(run(again, rep).applied.length, 0);
  assert.equal(allMarkers(again, 1).length, 1);
});

test('11: closed issue, evidence not newer -> held; one day later -> reopen', () => {
  const issue = issueFor('t', 5, { state: CLOSED, closed_at: '2026-11-03T23:00:00Z' });
  let rep = report({ candidates: { t: [[sha(1), '2026-11-03']] } });
  let plan = planActions(rep, [issue], new Map([[5, new Set()]]), 'x');
  assert.deepEqual(plan.actions, []);
  assert.deepEqual(plan.held_closed, [{ test_id: 't', issue: 5 }]);
  const gh = fakeGh([issue]);
  assert.equal(run(gh, rep).applied.length, 0);
  rep = report({ candidates: { t: [[sha(1), '2026-11-04']] } });
  assert.deepEqual(run(gh, rep).applied.map((a) => a.type), ['reopen', 'comment']);
});

test('12: issues beyond MAX_COMMENT_LISTS are not read and go to overflow', () => {
  const { rep, issues } = manyOpen(MAX_COMMENT_LISTS + 1);
  const comments = new Map(issues.slice(0, MAX_COMMENT_LISTS).map((i) => [i.number, new Set()]));
  const plan = planActions(rep, issues, comments, 'x');
  assert.ok(!plan.actions.some((a) => a.issue === MAX_COMMENT_LISTS + 1));
  assert.ok(plan.overflow.notices.some((o) => o.reason === 'not_read'));
  const closed = issueFor('c', 7, { state: CLOSED, closed_at: '2026-11-01T00:00:00Z' });
  const p2 = planActions(report({ candidates: { c: [[sha(1), '2026-11-05']] } }), [closed],
    new Map(), 'x');
  assert.deepEqual(p2.actions, []);
  assert.deepEqual(p2.overflow.notices, [{ test_id: 'c', kind: 'reopen', reason: 'not_read' }]);
});

test('13: reopen comments only the SHA not yet marked; no marker twice', () => {
  const h = sha256hex('t');
  const body = `${marker('t')}\n${ev(h, sha(1))}\n${ev(h, sha(2))}\n`;
  const gh = fakeGh([{ number: 5, state: CLOSED, closed_at: '2026-11-05T00:00:00Z', body }]);
  const rep = report({ candidates: { t: [[sha(1), '2026-11-01'], [sha(2), '2026-11-01'],
    [sha(3), '2026-11-07']] } });
  const first = run(gh, rep);
  assert.deepEqual(first.applied.map((a) => a.type), ['reopen', 'comment']);
  const posted = gh.state.comments[5][0].body;
  assert.ok(posted.includes(ev(h, sha(3))));
  assert.ok(!posted.includes(ev(h, sha(1))) && !posted.includes(ev(h, sha(2))));
  assert.equal(run(gh, rep).applied.length, 0);
  const markers = allMarkers(gh, 5);
  assert.equal(markers.length, new Set(markers).size);
});
