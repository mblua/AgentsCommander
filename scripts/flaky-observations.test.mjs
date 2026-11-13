import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import {
  SCHEMA,
  cargoRecord,
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

test('prototype-key statuses map to not_run (AC-4527-001)', () => {
  const assertions = ['constructor', '__proto__', 'toString'].map((status, i) => ({
    fullName: `p${i}`,
    status,
  }));
  const record = vitestRecord(report(assertions), { root: ROOT, identity: IDENTITY });
  assert.deepEqual(record.tests.map((t) => t.outcome), ['not_run', 'not_run', 'not_run']);
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

// ---- P03: cargo test (libtest) logs ----------------------------------------

const LIB = '     Running unittests src\\lib.rs '
  + '(target\\debug\\deps\\agentscommander_lib-0123456789abcdef.exe)';
const FOO = '     Running tests/foo.rs (target/debug/deps/foo-fedcba9876543210)';

function result(p, f, i, extra = '0 measured; 0 filtered out; finished in 0.01s') {
  const word = f > 0 ? 'FAILED' : 'ok';
  return `test result: ${word}. ${p} passed; ${f} failed; ${i} ignored; ${extra}`;
}

function cargo(texts) {
  const logs = texts.map((text, i) => ({ file: `log${i}.log`, text }));
  return cargoRecord(logs, { identity: IDENTITY });
}

function outcomes(record) {
  return Object.fromEntries(record.tests.map((t) => [t.test_id, t.outcome]));
}

function log(...rows) {
  return rows.join('\n');
}

test('cargo 1: two sections, exact ids and outcomes', () => {
  const text = log(
    '   Compiling agentscommander v0.1.0',
    LIB, '', 'running 3 tests',
    'test a::b ... ok', 'test a::c ... FAILED', 'test a::d ... ignored',
    '', 'failures:', '', '---- a::c stdout ----', 'boom', '',
    'failures:', '    a::c', '', result(1, 1, 1),
    FOO, 'running 1 test', 'test it_works ... ok', result(1, 0, 0),
  );
  const record = cargo([text]);
  assert.equal(record.header.suite, 'cargo');
  assert.deepEqual(outcomes(record), {
    'agentscommander_lib::a::b': 'pass',
    'agentscommander_lib::a::c': 'fail',
    'agentscommander_lib::a::d': 'skip',
    'foo::it_works': 'pass',
  });
  assert.deepEqual(record.header.sources, [
    { file: 'log0.log', present: true, complete: true, tests: 4, children: 0, tainted: 0 },
  ]);
});

test('cargo 2: --nocapture split resolves by counts or failure list', () => {
  const split = log(LIB, 'running 2 tests', 'test x ... ', 'some output', 'ok',
    'test y ... ok', result(2, 0, 0));
  assert.deepEqual(Object.values(outcomes(cargo([split]))), ['pass', 'pass']);
  const failed = log(LIB, 'running 1 test', 'test x ... ', 'out', 'FAILED', '',
    'failures:', '    x', '', result(0, 1, 0));
  assert.deepEqual(outcomes(cargo([failed])), { 'agentscommander_lib::x': 'fail' });
});

test('cargo 3: missing result voids only that section', () => {
  const text = log(LIB, 'running 1 test', 'test a ... ok',
    FOO, 'running 1 test', 'test b ... ok', result(1, 0, 0));
  const record = cargo([text]);
  assert.deepEqual(outcomes(record), {
    'agentscommander_lib::a': 'not_run',
    'foo::b': 'pass',
  });
  assert.equal(record.header.sources[0].complete, false);
});

test('cargo 4: counts that disagree give not_run', () => {
  const text = log(LIB, 'running 3 tests', 'test a ... ok', 'test b ... ok', result(3, 0, 0));
  assert.deepEqual(Object.values(outcomes(cargo([text]))), ['not_run', 'not_run']);
});

test('cargo 5: a pending name is never guessed into a pass', () => {
  const text = log(LIB, 'running 2 tests', 'test a ... ok', 'test b ... ', result(1, 1, 0));
  const record = cargo([text]);
  assert.deepEqual(Object.values(outcomes(record)), ['not_run', 'not_run']);
  assert.equal(record.header.sources[0].complete, false);
});

test('cargo 6: ANSI colours parse like plain text', () => {
  const plain = log(LIB, 'running 1 test', 'test a ... ok', result(1, 0, 0));
  const coloured = log(LIB, 'running 1 test', 'test a ... \x1b[32mok\x1b[0m',
    '\x1b[1mtest result\x1b[0m: ok. 1 passed; 0 failed; 0 ignored; 0 measured;');
  assert.deepEqual(cargo([coloured]).tests, cargo([plain]).tests);
  assert.equal(cargo([coloured]).header.sources[0].complete, true);
});

test('cargo 7: --list output gives no tests and is incomplete', () => {
  const record = cargo([log('a::b: test', 'a::c: test', '', '2 tests, 0 benchmarks')]);
  assert.equal(record.tests.length, 0);
  assert.equal(record.header.sources[0].complete, false);
});

test('cargo 8: missing file and zero logs', () => {
  const record = cargoRecord([{ file: 'gone.log', text: null }], { identity: IDENTITY });
  assert.deepEqual(record.header.sources, [
    { file: 'gone.log', present: false, complete: false, tests: 0, children: 0, tainted: 0 },
  ]);
  const out = path.join(tempDir(), 'c.jsonl');
  assert.equal(main(['cargo', '--out', out, '--job', 'rust'], ENV), 0);
  const header = lines(fs.readFileSync(out, 'utf8'))[0];
  assert.deepEqual(header.sources, []);
  assert.equal(header.suite, 'cargo');
});

test('cargo 9: same test pass and fail in two logs collapses to fail', () => {
  const pass = log(LIB, 'running 1 test', 'test a ... ok', result(1, 0, 0));
  const fail = log(LIB, 'running 1 test', 'test a ... FAILED', 'failures:', '    a', '',
    result(0, 1, 0));
  const record = cargo([pass, fail]);
  assert.deepEqual(record.tests, [{ test_id: 'agentscommander_lib::a', outcome: 'fail' }]);
  assert.equal(record.header.collapsed, 1);
});

test('cargo 10: should panic suffix and ignored reason', () => {
  const text = log(LIB, 'running 2 tests', 'test a - should panic ... ok',
    'test b ... ignored, needs a display', result(1, 0, 1));
  assert.deepEqual(outcomes(cargo([text])), {
    'agentscommander_lib::a': 'pass',
    'agentscommander_lib::b': 'skip',
  });
});

test('cargo 11: real rust-slow-test-windows tail', () => {
  const name = 'a::single_editor_driver_nonempty_actual_pair_guard_cuts_resume_original_packets';
  const text = log(LIB, '', 'running 1 test', `test ${name} ... ok`, '',
    'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 900 filtered out; '
    + 'finished in 12.34s');
  const record = cargo([text]);
  assert.deepEqual(record.tests, [{ test_id: `agentscommander_lib::${name}`, outcome: 'pass' }]);
  assert.equal(record.header.sources[0].complete, true);
});

test('cargo 12: normal child taints only its name', () => {
  const text = log(LIB, 'running 3 tests', 'test a ... ok',
    'running 1 test', 'test s::c ... ok', result(1, 0, 0),
    'test s::c ... ok', 'test b ... ok', result(3, 0, 0),
    FOO, 'running 1 test', 'test i ... ok', result(1, 0, 0));
  const record = cargo([text]);
  assert.deepEqual(outcomes(record), {
    'agentscommander_lib::a': 'pass',
    'agentscommander_lib::b': 'pass',
    'agentscommander_lib::s::c': 'not_run',
    'foo::i': 'pass',
  });
  assert.equal(record.header.sources[0].complete, true);
  assert.equal(record.header.sources[0].tainted, 1);
  assert.ok(record.header.sources[0].children > 0);
});

test('cargo 13: false-pass guard on one thread', () => {
  const text = log(LIB, 'running 1 test', 'test P ... ',
    'running 1 test', 'test P ... ok', result(1, 0, 0),
    'FAILED', '', 'failures:', '    P', '', result(0, 1, 0));
  assert.deepEqual(outcomes(cargo([text])), { 'agentscommander_lib::P': 'not_run' });
});

test('cargo 14: dead child after exit keeps the section exact', () => {
  const body = (cut) => log(LIB, 'running 5 tests', 'test a ... ok', 'running 1 test',
    cut ? 'test x::child ... test b ... ok' : 'test b ... ok',
    'test x::child ... ok', 'test c ... FAILED', 'test d ... ok',
    '', 'failures:', '    c', '', result(4, 1, 0));
  const exact = {
    'agentscommander_lib::a': 'pass',
    'agentscommander_lib::b': 'pass',
    'agentscommander_lib::c': 'fail',
    'agentscommander_lib::d': 'pass',
  };
  const cut = cargo([body(true)]);
  assert.deepEqual(outcomes(cut), { ...exact, 'agentscommander_lib::x::child': 'not_run' });
  assert.equal(cut.header.sources[0].complete, true);
  const lost = cargo([body(false)]);
  assert.deepEqual(outcomes(lost), { ...exact, 'agentscommander_lib::x::child': 'pass' });
  assert.equal(lost.header.sources[0].complete, true);
});

test('cargo 15: killed child leaves all parent names exact', () => {
  const text = log(LIB, 'running 6 tests', 'test p1 ... ok', 'test p2 ... ok',
    'test p3 ... ok', 'running 1 test', 'test p4 ... ok', 'test p5 ... ok',
    'test p6 ... ok', result(6, 0, 0));
  const record = cargo([text]);
  assert.equal(record.tests.length, 6);
  assert.ok(record.tests.every((t) => t.outcome === 'pass'));
  assert.equal(record.header.sources[0].complete, true);
});

test('cargo 16: cut line glued to the parent result', () => {
  const text = log(LIB, 'running 3 tests', 'test a ... ok', 'test x::child ... ok',
    'test b ... ok', `test x::child ... ${result(3, 0, 0)}`);
  const record = cargo([text]);
  assert.equal(outcomes(record)['agentscommander_lib::x::child'], 'not_run');
  assert.equal(outcomes(record)['agentscommander_lib::a'], 'pass');
  assert.equal(record.header.sources[0].complete, true);
  assert.equal(record.header.sources[0].tainted, 1);
});

test('cargo 17: a child result never stands for the parent', () => {
  const child = ['running 1 test', 'test p ... ok', result(1, 0, 0), 'test p ... FAILED'];
  const full = log(LIB, 'running 1 test', ...child, 'failures:', '    p', '', result(0, 1, 0));
  assert.deepEqual(outcomes(cargo([full])), { 'agentscommander_lib::p': 'not_run' });
  const noParent = log(LIB, 'running 1 test', ...child, 'failures:', '    p', '');
  assert.deepEqual(outcomes(cargo([noParent])), { 'agentscommander_lib::p': 'not_run' });
});

test('cargo 18: unknown glue drops a line and voids the section', () => {
  const text = log(LIB, 'running 2 tests', 'test a ... ok', 'footest b ... ok', result(2, 0, 0));
  const record = cargo([text]);
  assert.deepEqual(outcomes(record), { 'agentscommander_lib::a': 'not_run' });
  assert.equal(record.header.sources[0].complete, false);
});

test('cargo 19: captured-output failures block is not a name list', () => {
  const text = log(LIB, 'running 2 tests', 'test a ... ', 'test b ... ok', '',
    'failures:', '', '---- a stdout ----', '    not_a_name', '',
    'failures:', '    a', '', result(1, 1, 0));
  assert.deepEqual(outcomes(cargo([text])), {
    'agentscommander_lib::a': 'fail',
    'agentscommander_lib::b': 'pass',
  });
});

// Real libtest logs (local cargo test, #4527 P03 test 20), lines verbatim.
const REAL_STORE_LOG = [
  '   Compiling agentscommander v0.47.0 (D:\\0_repos\\AgentsCommander_iac\\.ac' +
    '\\room-43-ac-fixer-dev-team-v1\\repo-AgentsCommander\\src-tauri)',
  'warning: agentscommander@0.47.0: ../dist/index.html missing; embedded we' +
    'b UI disabled for this dev/check build',
  'warning: agentscommander@0.47.0: overriding Tauri bundle resources for t' +
    'his dev/check build because ../dist is missing',
  '    Finished `test` profile [unoptimized + debuginfo] target(s) in 4m 09' +
    's',
  '     Running unittests src\\lib.rs (D:/ac_temp_builds/fe\\31f4f23152a0c7\\d' +
    'ebug\\deps\\agentscommander_lib-a562248946c0148c.exe)',
  '',
  'running 26 tests',
  'test context_history::store::tests::dto_rejects_out_of_range_or_mismatch' +
    'ed_values ... ok',
  'test context_history::store::tests::dto_never_invents_a_lost_start ... o' +
    'k',
  'test context_history::store::tests::dto_identity_is_exact_bounded_and_ne' +
    'ver_truncated ... ok',
  'test context_history::store::tests::dto_accepts_a_valid_change_and_bound' +
    'aries ... ok',
  'test context_history::store::tests::dto_team_ids_are_sorted_unique_and_m' +
    'atch_attribution ... ok',
  'test context_history::store::tests::hot_journal_fixture_child ... ok',
  'test context_history::store::tests::a_disabled_writer_is_refused_without' +
    '_creating_anything ... ok',
  'test context_history::store::tests::reading_a_missing_history_is_empty_a' +
    'nd_creates_nothing ... ok',
  'test context_history::store::tests::dto_json_is_camel_case_and_bounded .' +
    '.. ok',
  'test context_history::store::tests::corrupt_databases_are_preserved ... ' +
    'ok',
  'test context_history::store::tests::a_fresh_database_has_the_fixed_layou' +
    't ... ok',
  'test context_history::store::tests::invalid_batches_write_nothing ... ok',
  'test context_history::store::tests::a_snapshot_past_its_deadline_fails_i' +
    'nstead_of_returning_part ... ok',
  'test context_history::store::tests::a_plain_read_leaves_the_database_byt' +
    'e_identical ... ok',
  'test context_history::store::tests::records_roundtrip_reopen_and_repeat_' +
    'idempotently ... ok',
  'test context_history::store::tests::retention_expires_old_records_in_the' +
    '_append_transaction ... ok',
  '',
  'running 1 test',
  'test context_history::store::tests::hot_journal_fixture_child ... ',
  'running 1 test',
  'test context_history::store::tests::hot_journal_fixture_child ... test c' +
    'ontext_history::store::tests::a_database_beyond_the_page_cap_is_a_limit_' +
    'mismatch ... ok',
  'test context_history::store::tests::newer_and_v1_databases_are_never_mig' +
    'rated ... ok',
  'test context_history::store::tests::the_row_cap_evicts_the_oldest_first ' +
    '... ok',
  'test context_history::store::tests::a_foreign_page_size_is_a_limit_misma' +
    'tch ... ok',
  'test context_history::store::tests::two_writers_share_one_database ... o' +
    'k',
  'test context_history::store::tests::a_reader_makes_commit_busy_and_the_s' +
    'ame_id_lands_once_later ... ok',
  'test context_history::store::tests::storage_full_after_eight_retries_kee' +
    'ps_the_prior_state ... ok',
  'test context_history::store::tests::an_interrupted_transaction_is_recove' +
    'red_on_retry_when_disabled ... ok',
  'test context_history::store::tests::an_interrupted_transaction_is_recove' +
    'red_on_retry_when_enabled ... ok',
  'test context_history::store::tests::real_saturation_keeps_recent_records' +
    '_within_the_page_cap ... ok',
  '',
  'test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 7814 filter' +
    'ed out; finished in 54.79s',
  '',
].join('\n');
const REAL_2837_LOG = [
  '   Compiling agentscommander v0.47.0 (D:\\0_repos\\AgentsCommander_iac\\.ac' +
    '\\room-43-ac-fixer-dev-team-v1\\repo-AgentsCommander\\src-tauri)',
  'warning: agentscommander@0.47.0: ../dist/index.html missing; embedded we' +
    'b UI disabled for this dev/check build',
  'warning: agentscommander@0.47.0: overriding Tauri bundle resources for t' +
    'his dev/check build because ../dist is missing',
  '    Finished `test` profile [unoptimized + debuginfo] target(s) in 3m 46' +
    's',
  '     Running unittests src\\lib.rs (D:/ac_temp_builds/fe\\31f4f23152a0c7\\d' +
    'ebug\\deps\\agentscommander_lib-a562248946c0148c.exe)',
  '',
  'running 26 tests',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_child_fixture' +
    ' ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_complete_append_failed' +
    '_sync_retry_requires_fresh_sync ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_clean_externa' +
    'l_change_before_journal_keeps_pair ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_legacy_missing_bom_crl' +
    'f_user_description ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_append_exact_' +
    'encoded_limit_and_one_byte_over ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_backup_partia' +
    'l_failure_cleanup_and_finished_empty_side ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_clean_externa' +
    'l_change_immediately_before_rename ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_retry_mismatch_overtak' +
    'en_and_cas ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_partial_bounds_and_val' +
    'idation_before_repair ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_first_partial_twenty_b' +
    'ytes_and_split_utf8_repair ... ok',
  '',
  'running 1 test',
  '',
  'running 1 test',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_recovery_pres' +
    'erves_foreign_unused_stage ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_unknown_field' +
    's_and_preceding_corrupt_row_partial ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_clean_backup_collision' +
    '_on_either_side ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_corrupt_complete_schem' +
    'a_shape_and_sequence ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_encoded_size_boundary_' +
    'and_bounded_history_read ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_append_title_' +
    'body_clean_concurrency ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_already_new_p' +
    'air_retries_both_syncs_before_cleanup ... ok',
  '',
  'running 1 test',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_status_backup' +
    '_write_failure_preserves_finished_task_and_retries ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_task_backup_w' +
    'rite_failure_preserves_originals_and_retries ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_kernel_lock_c' +
    'hild_exit_and_stable_file ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_task_backup_a' +
    'fter_write_cleans_first_partial_block_and_retries ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_recovery_exte' +
    'rnal_edits_missing_evidence_invalid_journal ... ok',
  '',
  'running 1 test',
  '',
  'running 1 test',
  '',
  'running 1 test',
  'test cli::task_ops::tests::issue_2837::issue_2837_clean_pair_matrix_exac' +
    't_bytes_noop_and_aba ... ok',
  '',
  'running 1 test',
  'test cli::task_ops::tests::issue_2837::issue_2837_clean_postjournal_fail' +
    'ure_matrix_and_repeat_recovery ... ok',
  'test cli::task_ops::tests::issue_2837_recovery::issue_2837_process_exit_' +
    'publication_boundaries_recover_pair ... ok',
  'test cli::task_ops::tests::issue_2837::issue_2837_clean_prejournal_failu' +
    're_matrix_preserves_originals ... ok',
  '',
  'test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 7814 filter' +
    'ed out; finished in 0.62s',
  '',
].join('\n');

test('cargo 20: positive control from real libtest logs', () => {
  const store = cargo([REAL_STORE_LOG]);
  assert.deepEqual(store.header.sources, [
    { file: 'log0.log', present: true, complete: true, tests: 26, children: 3, tainted: 1 },
  ]);
  const storeOut = outcomes(store);
  const child = 'agentscommander_lib::context_history::store::tests::hot_journal_fixture_child';
  assert.equal(storeOut[child], 'not_run');
  const others = Object.entries(storeOut).filter(([id]) => id !== child);
  assert.equal(others.length, 25);
  assert.ok(others.every(([, outcome]) => outcome === 'pass'));
  const r2837 = cargo([REAL_2837_LOG]);
  assert.deepEqual(r2837.header.sources, [
    { file: 'log0.log', present: true, complete: true, tests: 26, children: 7, tainted: 0 },
  ]);
  const all = Object.values(outcomes(r2837));
  assert.equal(all.length, 26);
  assert.ok(all.every((outcome) => outcome === 'pass'));
});
