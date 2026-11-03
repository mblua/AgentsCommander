import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { CODES, parseJSON, parseCapture, counts, compare, ratchet, validateBaseline,
  mergeObservations, parseArgs, selfTestResult, workflowContract, baselineDiff, main }
  from './check-clippy-seven.mjs';

const root = 'D:/capture';
const file = 'src-tauri/src/a.rs';
const source = 'abcdefghijklmnopqrstuvwxyz\n';
const sha = 'a'.repeat(40), q = 'b'.repeat(40), tr = 'c'.repeat(40), qt = 'd'.repeat(40);
const zeros = () => Object.fromEntries(CODES.map(c => [c, 0]));
const clone = v => structuredClone(v);
const fail = fn => assert.throws(fn, e => e.exit === 2);
const growth = fn => assert.throws(fn, e => e.exit === 1);
const span = (f = file, start = 0, end = start + 1) => ({ file_name: f,
  byte_start: start, byte_end: end, line_start: 1, line_end: 1,
  column_start: start + 1, column_end: end + 1, is_primary: true, expansion: null });
const message = (spans = [span()], code = CODES[0]) => ({ reason: 'compiler-message',
  message: { level: 'warning', code: { code }, spans, children: [
    { code: { code: CODES[1] }, level: 'help', spans: [span()] }] } });
const end = { reason: 'build-finished', success: true };
const stream = (messages = []) => [...messages, end].map(v => JSON.stringify(v)).join('\n') + '\n';
const parse = (messages, readSource = f => {
  if (!f.startsWith('src-tauri/') && !f.startsWith('crates/')) throw Error('untracked source');
  return source;
}) => parseCapture(stream(messages), 0, { root, readSource });
const observation = (locations, platform = 'linux') => ({ version: 1, sourceCommit: sha,
  sourceTree: tr, platform, toolchain: '1.98.1', policy: 'lint-platform-total-v1', exit: 0,
  totals: counts(locations), locations });
const provenance = () => ({ sourceReference: q, referenceTree: qt, rawHash: 'e'.repeat(64),
  manifestHash: 'f'.repeat(64), captureRoot: root });
const baseline = (n = 5) => ({ version: 1, issue: 3528, sourceReference: q, referenceTree: qt,
  sourceCommit: sha, sourceTree: tr, toolchain: '1.98.1', platforms: ['linux', 'windows'],
  codes: CODES, policy: 'lint-platform-total-v1', limits: {
    linux: { ...zeros(), [CODES[0]]: n }, windows: zeros() } });
const locations = n => parse(Array.from({ length: n }, (_, i) => message([span(file, i)])));

test('complete zero capture valid; children do not add findings', () => {
  assert.deepEqual(parse([]), []);
  assert.equal(parse([message()]).length, 1);
  assert.equal(compare(observation([]), baseline(0)).exit, 0);
  assert.equal(compare(observation(locations(1)), baseline(0)).exit, 1);
});
test('all primaries define identity, unique ordering and repeated targets', () => {
  const a = span(), b = span(file, 3);
  const observed = parse([message([a, b]), message([b, a, a]), message([a]), message([b])]);
  assert.equal(observed.length, 3);
  assert.equal(observed.find(l => l.spans.length === 2).spans.length, 2);
  const r = message(); r.package_id = 'target-one';
  const s = clone(r); s.package_id = 'target-two';
  assert.equal(parse([r, s]).length, 1);
});
test('Windows/Linux/crates roots, absolute original mapping, multiple files', () => {
  for (const f of [file, file.replaceAll('/', '\\'), root + '/' + file,
    'crates/member/src/lib.rs']) assert.equal(parse([message([span(f)])]).length, 1);
  const l = parse([message([span(file), span('crates/a/src/lib.rs')])])[0];
  assert.equal(l.file, 'crates/a/src/lib.rs'); assert.equal(l.spans.length, 2);
  for (const f of ['../escape.rs', '/other/a.rs', 'src/a.rs', 'src-tauri/src-tauri/a.rs']) {
    assert.throws(() => parse([message([span(f)])], name => {
      assert.equal(name, file); return source;
    }));
  }
});
test('UTF-8 bytes and Unicode scalar columns, not UTF-16 units', () => {
  const s = { ...span(), byte_start: 6, byte_end: 7, column_start: 3, column_end: 4 };
  assert.equal(parse([message([s])], () => 'é🙂x\n').length, 1);
  fail(() => parse([message([{ ...s, column_start: 4, column_end: 5 }])], () => 'é🙂x\n'));
});
test('malformed/truncated/terminal/error input is not zero findings', () => {
  const options = { root, readSource: () => source };
  for (const text of ['', '{', '\n', JSON.stringify(message()), stream([]) + '{}\n',
    JSON.stringify(end) + '\n' + stream([]),
    JSON.stringify({ ...end, success: false }) + '\n',
    stream([{ reason: 'compiler-message', message: { level: 'error' } }]),
    stream([{ reason: 'compiler-message', message: { level: 'failure-note' } }])]) {
    fail(() => parseCapture(text, 0, options));
  }
  fail(() => parseCapture(stream([]), 1, options));
  fail(() => parseCapture(stream([]), 124, options));
});
test('bad spans, expansion, source mismatch and unresolved data fail', () => {
  for (const s of [{ ...span(), expansion: {} }, { ...span(), byte_end: 0 },
    { ...span(), byte_start: 2 }, { ...span(), line_start: 0 },
    { ...span(), column_end: 99 }, { ...span(), byte_end: 1.5 }]) {
    fail(() => parse([message([s])]));
  }
  fail(() => parse([message([])]));
  fail(() => parse([message([{ ...span(), is_primary: false }])]));
  assert.throws(() => parse([message()], () => { throw Error('untracked/generated'); }));
});
test('moves/splits/rename/reorder retain capacity; distinct copies consume capacity', () => {
  const b = baseline(5);
  const moved = parse(Array.from({ length: 5 }, (_, i) => message([
    span('crates/split' + (i % 3) + '/src/lib.rs', i)])));
  assert.equal(compare(observation(moved), b).exit, 0);
  assert.equal(compare(observation(locations(6)), b).exit, 1);
  const copied = [...moved, ...parse(Array.from({ length: 5 }, (_, i) =>
    message([span('src-tauri/tests/copy.rs', i)])))];
  copied.sort((a, z) => JSON.stringify([a.code, a.spans]) <
    JSON.stringify([z.code, z.spans]) ? -1 : 1);
  assert.equal(compare(observation(copied), b).exit, 1);
  assert.equal(compare(observation(parse(Array.from({ length: 5 }, () => message()))), b).exit, 0);
});
test('retained slack explicitly permits remove/replace/copy/regrow and new files', () => {
  const b = baseline(5);
  for (const n of [0, 3, 5]) assert.equal(compare(observation(locations(n)), b).exit, 0);
  assert.equal(compare(observation(locations(6)), b).exit, 1);
  const different = parse(Array.from({ length: 5 }, (_, i) =>
    message([span('src-tauri/tests/different.rs', i)])));
  assert.equal(compare(observation(different), b).exit, 0);
  b.limits.linux[CODES[0]] = 3;
  assert.equal(compare(observation(locations(4)), b).exit, 1);
});
test('lint/host isolation; production labels do not partition capacity', () => {
  const b = baseline(5);
  assert.equal(compare(observation(locations(1), 'windows'), b).exit, 1);
  const other = parse([message([span()], CODES[1])]);
  assert.equal(compare(observation(other), b).exit, 1);
  const tests = parse([message([span('src-tauri/tests/tests.rs')])]);
  assert.equal(tests[0].production, false);
  assert.equal(compare(observation(tests), b).exit, 0);
  assert.equal(compare(observation(locations(1)), b).results[0].status, 'WITHIN_TOTAL');
});
test('strict JSON and complete nonnegative fourteen-key schema', () => {
  validateBaseline(baseline(0));
  fail(() => parseJSON('{"a":1,"a":2}'));
  fail(() => parseJSON('{"a":{"x":1,"x":2}}'));
  fail(() => parseJSON('{"a":1} trailing'));
  for (const n of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1', null]) {
    const b = baseline(); b.limits.linux[CODES[0]] = n; fail(() => validateBaseline(b));
  }
  for (const mutate of [b => delete b.limits.windows[CODES[1]],
    b => { b.limits.linux.other = 1; }, b => { b.platforms.reverse(); },
    b => { b.policy = 'per-file'; }, b => { b.unknown = 0; }]) {
    const b = baseline(); mutate(b); fail(() => validateBaseline(b));
  }
});
test('ratchet lowers/zeros; rejects cross-key swaps, growth and metadata edits', () => {
  const b = baseline(), lower = clone(b); lower.limits.linux[CODES[0]] = 0; ratchet(lower, b);
  const grown = clone(b); grown.limits.linux[CODES[0]]++; growth(() => ratchet(grown, b));
  const swap = clone(lower); swap.limits.windows[CODES[0]] = 5; growth(() => ratchet(swap, b));
  const changed = clone(b); changed.sourceCommit = q; growth(() => ratchet(changed, b));
  const missing = clone(b); delete missing.limits.linux[CODES[1]];
  fail(() => ratchet(missing, b)); fail(() => ratchet(null, b));
});
test('native merge derives host limits and rejects mismatched identities/totals', () => {
  const a = { ...observation(locations(5)), provenance: provenance() };
  const b = { ...observation(locations(3), 'windows'), provenance: provenance() };
  const merged = mergeObservations([b, a]);
  assert.equal(merged.limits.linux[CODES[0]], 5);
  assert.equal(merged.limits.windows[CODES[0]], 3);
  fail(() => mergeObservations([a, a]));
  const bad = clone(b); bad.sourceCommit = q; fail(() => mergeObservations([a, bad]));
  bad.sourceCommit = sha; bad.totals[CODES[0]]++;
  fail(() => mergeObservations([a, bad]));
});
test('strict CLI and failing/zero-test children', () => {
  for (const args of [[], ['--wat'], ['--self-test', '--self-test'],
    ['--self-test', '--baseline', 'x'], ['--capture', 'x'],
    ['--merge', 'a', 'b', 'c', '--out', 'x'], ['--baseline-diff', '--base-ref']]) {
    fail(() => parseArgs(args));
  }
  assert.equal(parseArgs(['--baseline-diff']).files.length, 0);
  fail(() => selfTestResult({ status: 1, stdout: '# tests 10' }));
  fail(() => selfTestResult({ status: 0, stdout: '# tests 0' }));
  assert.equal(selfTestResult({ status: 0, stdout: '# tests 12\n' }), 0);
});
test('actual workflow and mutations preserve required native flags/blockers/hosts', () => {
  const text = fs.readFileSync(new URL('../.github/workflows/pr-regression-gates.yml',
    import.meta.url), 'utf8').replaceAll('\r\n', '\n');
  workflowContract(text);
  for (const mutate of [t => t.replace('--force-warn ' + CODES[0], '--force-warn clippy::other'),
    t => t.replace('  clippy-linux:', '  no-linux:'),
    t => t.replace('      - name: Seven-lint gate\n',
      '      - name: Seven-lint gate\n        continue-on-error: true\n'),
    t => t.replace('npm run seven:baseline', 'echo no-ratchet')]) {
    fail(() => workflowContract(mutate(text)));
  }
});
test('Git committed-HEAD adoption survives inherited main drift; ratchet ignores worktree', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'i3528-seven-'));
  const run = (...args) => execFileSync('git', args,
    { cwd: dir, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  const write = (f, text) => {
    fs.mkdirSync(path.dirname(path.join(dir, f)), { recursive: true });
    fs.writeFileSync(path.join(dir, f), text);
  };
  const commit = () => { run('add', '.'); run('commit', '-qm', 'fixture'); return run('rev-parse',
    'HEAD'); };
  try {
    run('init', '-q'); run('config', 'user.name', 'fixture');
    run('config', 'user.email', 'fixture@example.invalid'); run('config', 'core.autocrlf', 'false');
    write(file, source); const ref = commit(), refTree = run('rev-parse', 'HEAD^{tree}');
    write('.github/workflows/clippy-seven-measurement.yml', 'temporary\n'); const r = commit();
    const rTree = run('rev-parse', 'HEAD^{tree}');
    // Main inherits changed Rust; current feature keeps historical Q/R capacity provenance.
    write(file, source + '// changed on main\n'); commit();
    run('update-ref', 'refs/remotes/origin/main', 'HEAD');
    const b = baseline(); b.sourceReference = ref; b.referenceTree = refTree;
    b.sourceCommit = r; b.sourceTree = rTree;
    write('clippy-seven.baseline.json', JSON.stringify(b)); commit();
    baselineDiff('origin/main', dir);
    const captureFile = path.join(dir, 'raw.jsonl'), obsFile = path.join(dir, 'obs.json');
    write('raw.jsonl', stream([message()]));
    const captureArgs = ['--capture', captureFile, '--clippy-exit', '0', '--platform', 'linux',
      '--commit', run('rev-parse', 'HEAD'), '--workspace-root', dir,
      '--rustc-version', '1.98.1', '--emit', obsFile];
    assert.equal(main(captureArgs), 0);
    const checkArgs = ['--check', obsFile, '--baseline', path.join(dir,
      'clippy-seven.baseline.json'), '--workspace-root', dir];
    assert.equal(main(checkArgs), 0);
    const forged = JSON.parse(fs.readFileSync(obsFile));
    forged.sourceTree = qt; fs.writeFileSync(obsFile, JSON.stringify(forged));
    assert.equal(main(checkArgs), 2);
    for (const f of ['raw.jsonl', 'obs.json', 'obs.json.controls.json', 'obs.json.summary.json']) {
      fs.unlinkSync(path.join(dir, f));
    }
    run('update-ref', 'refs/remotes/origin/main', 'HEAD');
    const larger = clone(b); larger.limits.linux[CODES[0]]++;
    write('clippy-seven.baseline.json', JSON.stringify(larger));
    baselineDiff('origin/main', dir); // uncommitted inflation is not read as HEAD
    commit(); growth(() => baselineDiff('origin/main', dir));
    fs.unlinkSync(path.join(dir, 'clippy-seven.baseline.json')); commit();
    fail(() => baselineDiff('origin/main', dir));
    assert.equal(main(['--self-test', '--out', 'forbidden']), 2);
  } finally {
    assert.equal(path.dirname(dir), path.resolve(os.tmpdir()));
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
