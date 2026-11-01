import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import {
  parseCapture, observations, compare, merge, ratchet, counts, originalAbsent,
  validateEnvelope, validateBaseline, normalizeFile, checkEnvironment, parseArgs, selfTestResult,
  capture, baselineDiff,
} from './check-too-many-lines.mjs';

const sha = 'a'.repeat(40), anchor = 'a'.repeat(12);
const entry = (lines = 150, file = 'src/a.rs', name = 'heavy', hash = anchor) =>
  ({ id: `rust:${file}::${name}`, name, anchor: hash, lines, production: true });
const envelope = (entries = [entry()], platform = 'windows') =>
  ({ version: 1, platform, sourceCommit: sha, toolchain: '1.98.1',
    productionMeasured: true, entries });
const baseline = (entries = [entry()]) =>
  merge(['linux', 'macos', 'windows'].map(p => envelope(entries, p)));
const sorted = entries => entries.sort((a, b) => {
  const x = `${a.id}\0${a.anchor}`, y = `${b.id}\0${b.anchor}`;
  return x < y ? -1 : x > y ? 1 : 0;
});
const end = { reason: 'build-finished', success: true };
const jsonl = records => records.map(r => JSON.stringify(r)).join('\n') + '\n';
function diagnostic(source = 'fn heavy() {}', lines = 150, file = 'src/a.rs', line = 1) {
  const header = source.split('\n')[line - 1];
  return { reason: 'compiler-message', message: { code: { code: 'clippy::too_many_lines' },
    message: `this function has too many lines (${lines}/100)`, spans: [{ is_primary: true,
      file_name: file, line_start: line, line_end: line, column_start: 1,
      column_end: [...header.slice(0, header.indexOf('{')).trimEnd()].length + 1 }] } };
}
function measured(source, file = 'src/a.rs', line = 1, lines = 150) {
  const d = diagnostic(source, lines, file, line);
  const entries = observations(parseCapture(jsonl([d, end]), 0), '/repo', () => source);
  return envelope(entries.map(e => ({ ...e, production: true })));
}

for (const [count, exit] of [[151, 1], [150, 0], [149, 0], [101, 0]])
  test(`ceiling150 observed${count}`, () =>
    assert.equal(compare(envelope([entry(count)]), baseline(), () => false).exit, exit));
test('new101 fails; exactly100, short split and removal have no warnings', () => {
  assert.equal(compare(envelope([entry(101)]), baseline([]), () => true).exit, 1);
  for (const label of ['100', 'split', 'removed']) {
    const clean = observations(parseCapture(jsonl([end]), 0), '/repo', () => label);
    assert.deepEqual(clean, []);
    assert.equal(compare(envelope(clean), baseline(), () => false).exit, 0);
  }
});
test('committed180 permits150->160 and same identity reintroduction', () => {
  const b = baseline([entry(180)]), before = JSON.stringify(b);
  for (const entries of [[entry(150)], [entry(160)], [], [entry(180)]])
    assert.equal(compare(envelope(entries), b, () => false).exit, 0);
  assert.equal(JSON.stringify(b), before);
  const lowered = baseline([entry(150)]);
  assert.doesNotThrow(() => ratchet(lowered, b));
  assert.equal(compare(envelope([entry(160)]), lowered, () => false).exit, 1);
});
test('exact growth cannot retry relocation; absent and below-ceiling are reported', () => {
  const r = compare(envelope([entry(151)]), baseline(), () => true);
  assert.equal(r.results[0].status, 'GROWN');
  assert.equal(compare(envelope([]), baseline(), () => false).results[0].status, 'ABSENT');
  assert.equal(compare(envelope([entry(149)]), baseline(), () => false)
    .results[0].ceiling, 150);
});
for (const [count, exit] of [[150, 0], [149, 0], [151, 1]])
  test(`bounded move count${count}`, () => {
    const b = baseline(), before = JSON.stringify(b);
    const r = compare(envelope([entry(count, 'src/b.rs')]), b, () => true);
    assert.equal(r.exit, exit); assert.equal(JSON.stringify(b), before);
    assert.equal(r.results[0].status, exit ? 'GROWN' : 'MOVED');
  });
test('retained long or short donor forbids copying; consumed donor forbids second copy', () => {
  assert.equal(compare(envelope([entry(150, 'src/b.rs')]), baseline(), () => false).exit, 1);
  assert.equal(compare(envelope(sorted([entry(), entry(150, 'src/b.rs')])),
    baseline(), () => false).exit, 1);
});
test('multiple donors or receivers are ambiguous regardless of counts', () => {
  const b = baseline(sorted([entry(), entry(140, 'src/b.rs')]));
  assert.equal(compare(envelope([entry(101, 'src/c.rs')]), b, () => true)
    .results[0].status, 'AMBIGUOUS');
  assert.equal(compare(envelope(sorted([entry(101, 'src/b.rs'), entry(102, 'src/c.rs')])),
    baseline(), () => true).results[0].status, 'AMBIGUOUS');
});
test('equal-count identity swap fails', () => {
  const h = envelope([entry(150, 'src/a.rs', 'heavy', 'b'.repeat(12))]);
  assert.equal(compare(h, baseline(), () => true).exit, 1);
});

for (const qualifier of ['', 'pub ', 'pub(crate) ', 'pub(super) ', 'async ', 'pub unsafe '])
  test(`whole-header name-token identity: ${qualifier || 'private'}`, () => {
    const plain = measured('fn heavy() {}');
    const current = measured(`${qualifier}fn heavy() {}`);
    assert.equal(current.entries[0].id, plain.entries[0].id);
    assert.equal(current.entries[0].anchor, plain.entries[0].anchor);
    const b = baseline(plain.entries);
    assert.equal(compare(current, b, () => false).exit, 0);
    const moved = measured(`${qualifier}fn heavy() {}`, 'src/b.rs');
    assert.equal(compare(moved, b, () => true).results[0].status, 'MOVED');
  });
test('name/parameters/return/generics/where signature changes remain new debt', () => {
  const b = baseline(measured('fn heavy() {}').entries);
  for (const source of ['fn other() {}', 'fn heavy(x: u8) {}', 'fn heavy() -> u8 {}',
    'fn heavy<T>() {}', 'fn heavy<T>() where T: Copy {}'])
    assert.equal(compare(measured(source), b, () => true).exit, 1);
});
test('Unicode, CRLF, line shifts and nested containers preserve name-token position', () => {
  const a = measured('fn heavy() {}').entries[0];
  const b = measured('// é😀\r\n\r\nfn heavy() {}', 'src/a.rs', 3).entries[0];
  assert.equal(a.anchor, b.anchor); assert.equal(a.id, b.id);
  const nested = measured('impl A {\n pub async fn heavy() {}\n}', 'src/a.rs', 2);
  assert.equal(nested.entries[0].id, 'rust:src/a.rs::impl:A::heavy');
});
test('same-path container move uses bounded transfer', () => {
  const old = measured('impl A {\n fn heavy() {}\n}', 'src/a.rs', 2);
  const moved = measured('impl B {\n fn heavy() {}\n}', 'src/a.rs', 2);
  assert.equal(compare(moved, baseline(old.entries), () => true).results[0].status, 'MOVED');
});
test('original definition scan sees shortened, cfg-inactive and changed-header originals', () => {
  for (const source of ['fn heavy() {}', '#[cfg(nope)]\nfn heavy() {}',
    'fn heavy(x: u32) {}'])
    assert.equal(originalAbsent(entry(), { root: '/repo', stat: () => ({}),
      readSource: () => source }), false);
  assert.equal(originalAbsent(entry(), { root: '/repo', stat: () => ({}),
    readSource: () => 'fn other() {}' }), true);
});
test('missing donor only ENOENT; IO, unreadable and ambiguous macro fail closed', () => {
  assert.equal(originalAbsent(entry(), { stat: () => { throw { code: 'ENOENT' }; } }), true);
  assert.throws(() => originalAbsent(entry(), { stat: () => { throw Error('EACCES'); } }));
  assert.throws(() => originalAbsent(entry(), { stat: () => ({}),
    readSource: () => { throw Error('unreadable'); } }));
  assert.throws(() => originalAbsent(entry(), { stat: () => ({}),
    readSource: () => 'macro_rules! m { () => { fn heavy() {} } }' }));
});

test('duplicate identical spans collapse; conflicting counts fail', () => {
  const d = diagnostic(), reader = () => 'fn heavy() {}';
  assert.equal(observations(parseCapture(jsonl([d, d, end]), 0), '/repo', reader).length, 1);
  assert.throws(() => observations(parseCapture(jsonl([d, diagnostic(undefined, 151), end]), 0),
    '/repo', reader));
});
test('macro expansion, invalid spans, unreadable source and indistinguishable keys fail', () => {
  const d = diagnostic(); d.message.spans[0].expansion = {};
  assert.throws(() => observations(parseCapture(jsonl([d, end]), 0), '/repo', () => ''));
  const broken = diagnostic(); broken.message.spans[0].line_start = 99;
  assert.throws(() => observations(parseCapture(jsonl([broken, end]), 0), '/repo', () => ''));
  assert.throws(() => observations(parseCapture(jsonl([diagnostic(), end]), 0), '/repo',
    () => { throw Error('IO'); }));
  const source = 'fn heavy() {}\nfn heavy() {}';
  assert.throws(() => observations(parseCapture(jsonl([
    diagnostic(source), diagnostic(source, 150, 'src/a.rs', 2), end]), 0), '/repo', () => source));
});
test('strict capture/proof completeness and denominator controls', () => {
  assert.equal(parseCapture(jsonl([diagnostic(), end]), 0).length, 1);
  for (const raw of ['', 'garbage\n', '{\n', jsonl([diagnostic()]),
    jsonl([{ reason: 'build-finished', success: false }]), jsonl([end, end]),
    jsonl([end, diagnostic()])]) assert.throws(() => parseCapture(raw, 0));
  assert.throws(() => parseCapture(jsonl([end]), 101));
  for (const message of ['this function has too many lines (101/101)',
    'this function has too many lines (100/100)', 'unknown']) {
    const d = diagnostic(); d.message.message = message;
    assert.throws(() => parseCapture(jsonl([d, end]), 0));
  }
});
test('mixed cognitive diagnostic remains visible', () => {
  const rendered = [], d = { reason: 'compiler-message', message: {
    code: { code: 'clippy::cognitive_complexity' }, rendered: 'cognitive control' } };
  assert.equal(parseCapture(jsonl([d, diagnostic(), end]), 0, s => rendered.push(s)).length, 1);
  assert.deepEqual(rendered, ['cognitive control']);
});
test('normalization rejects escape and normalizes Windows/absolute paths', () => {
  assert.equal(normalizeFile('src\\a.rs', '/repo'), 'src/a.rs');
  assert.equal(normalizeFile('/repo/src/a.rs', '/repo'), 'src/a.rs');
  for (const f of ['../a.rs', '/foreign/a.rs', 'C:/foreign/a.rs'])
    assert.throws(() => normalizeFile(f, '/repo'));
});
test('platform/SHA/toolchain/schema and duplicate keys fail closed', () => {
  for (const patch of [{ platform: 'other' }, { sourceCommit: 'abc' }, { toolchain: '1.99.0' },
    { version: 2 }, { entries: [entry(), entry()] }])
    assert.throws(() => validateEnvelope({ ...envelope(), ...patch }));
  assert.throws(() => validateBaseline({ ...baseline(), threshold: 101 }));
});
test('merge requires all three measured production captures and identical source', () => {
  const inputs = ['linux', 'macos', 'windows'].map(p => envelope([entry()], p));
  assert.deepEqual(counts(merge(inputs)).union, { production: 1, allTargets: 1, testOnly: 0 });
  assert.throws(() => merge(inputs.slice(1)));
  assert.throws(() => merge([inputs[0], inputs[0], inputs[2]]));
  assert.throws(() => merge(inputs.map((e, i) => i ? e : { ...e, productionMeasured: false })));
  assert.throws(() => merge(inputs.map((e, i) => i ? e : { ...e, sourceCommit: 'b'.repeat(40) })));
});
test('platform-only sites and production/test union counts', () => {
  const b = merge([envelope([entry()], 'windows'), envelope([], 'linux'),
    envelope([{ ...entry(130, 'src/b.rs'), production: false }], 'macos')]);
  assert.deepEqual(counts(b).union, { production: 1, allTargets: 2, testOnly: 1 });
  assert.equal(compare(envelope([], 'linux'), b, () => false).exit, 0);
  assert.equal(compare(envelope([entry()], 'linux'), b, () => false).exit, 1);
});
test('merge serialization is identical for every platform input permutation', () => {
  const inputs = ['linux', 'macos', 'windows'].map(p => envelope([entry()], p));
  const expected = JSON.stringify(merge(inputs));
  for (const order of [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]])
    assert.equal(JSON.stringify(merge(order.map(i => inputs[i]))), expected);
});
test('ratchet allows removal/lowering only', () => {
  assert.doesNotThrow(() => ratchet(baseline([]), baseline()));
  assert.doesNotThrow(() => ratchet(baseline([entry(149)]), baseline()));
  for (const b of [baseline([entry(151)]), baseline([entry(150, 'src/b.rs')]),
    { ...baseline(), sourceCommit: 'b'.repeat(40) }]) assert.throws(() => ratchet(b, baseline()));
});
test('environment lint overrides cannot silence capture', () => {
  assert.doesNotThrow(() => checkEnvironment({}));
  for (const env of [{ CLIPPY_CONF_DIR: '' }, { RUSTFLAGS: '--cap-lints allow' },
    { RUSTFLAGS: '-Awarnings' }, { CARGO_ENCODED_RUSTFLAGS: '-A\x1fwarnings' }])
    assert.throws(() => checkEnvironment(env));
});
test('strict modes and invalid flags', () => {
  for (const args of [[], ['--tml-invalid-flag'], ['--self-test', '--base-ref', 'main'],
    ['--merge', 'a', 'b', 'c'], ['--capture', 'a'], ['--self-test', '--baseline-diff']])
    assert.throws(() => parseArgs(args));
});
test('self-test runner rejects zero tests and nonzero child', () => {
  assert.equal(selfTestResult({ status: 0, stdout: '# tests 12\n' }), 0);
  assert.throws(() => selfTestResult({ status: 0, stdout: '# tests 0\n' }));
  assert.throws(() => selfTestResult({ status: 1, stdout: '# tests 12\n' }));
});
test('both imports are inert and invalid CLI flags exit2; cognitive cases execute', () => {
  for (const file of ['check-cognitive-complexity.mjs', 'check-too-many-lines.mjs']) {
    const imported = spawnSync(process.execPath, ['--input-type=module', '-e',
      `import './scripts/${file}';`], { encoding: 'utf8' });
    assert.equal(imported.status, 0); assert.equal(imported.stdout, '');
    assert.equal(imported.stderr, '');
    const bad = spawnSync(process.execPath, [`scripts/${file}`, '--tml-invalid-flag']);
    assert.equal(bad.status, 2);
  }
  const cog = spawnSync(process.execPath, ['scripts/check-cognitive-complexity.mjs', '--self-test'],
    { encoding: 'utf8' });
  assert.equal(cog.status, 0); assert.match(cog.stdout, /passed \([1-9]\d* cases/);
});

// A bounded reader for this repository's job/step/run indentation, not general YAML.
function jobs(text) {
  assert.ok(!text.includes('\t'), 'tabs in workflow');
  const result = new Map(); let job;
  for (const line of text.replaceAll('\r', '').split('\n')) {
    const found = /^  ([a-z][a-z0-9-]*):$/.exec(line);
    if (found && !['pull_request', 'workflow_dispatch', 'contents'].includes(found[1])) {
      // Only parse keys after the jobs: root.
      if (!job && !result.started) continue;
      assert.ok(!result.has(found[1]), 'duplicate job');
      job = []; result.set(found[1], job);
    }
    if (line === 'jobs:') { result.started = true; continue; }
    if (job) job.push(line);
  }
  return result;
}
function steps(lines) {
  const result = []; let step;
  for (const line of lines) {
    if (/^      - /.test(line)) { step = []; result.push(step); }
    if (step) step.push(line);
  }
  return result.map(s => s.join('\n'));
}
function exactly(steps, pattern) {
  const found = steps.filter(s => pattern.test(s));
  assert.equal(found.length, 1, `expected exactly one step ${pattern}`); return found[0];
}
function blocking(text) {
  assert.doesNotMatch(text, /continue-on-error:/);
  assert.doesNotMatch(run(text), /\|\||exit\s+0|--report|set\s+\+e/);
}
function run(step) {
  const keys = [...step.matchAll(/^        ([a-z-]+):/gm)].map(m => m[1]);
  assert.equal(new Set(keys).size, keys.length, 'duplicate step keys');
  const m = /^        run: (.*)$/m.exec(step); assert.ok(m, 'missing run');
  if (m[1] !== '|') return m[1];
  const after = step.slice(m.index + m[0].length).split('\n').slice(1);
  const lines = [];
  for (const line of after) {
    if (line && !line.startsWith('          ')) break;
    lines.push(line.trim());
  }
  return lines.join('\n').replaceAll('\\\n', ' ').trim();
}
function workflowContract(workflow, warm) {
  assert.doesNotMatch(workflow + warm, /CLIPPY_CONF_DIR/);
  const parsed = jobs(workflow);
  assert.ok(parsed.has('tml-adoption'), 'missing manual adoption job');
  const adoption = "(github.event_name == 'workflow_dispatch' && inputs.tml_adoption_source != '')";
  const regular = "github.event_name != 'push' || (github.event.deleted != true && " +
    "github.event.after != '0000000000000000000000000000000000000000')";
  const full = new Set(['rust-regression-linux', 'rust-release-parity-linux',
    'rust-regression-macos', 'issue-1850-release-macos', 'terminal-snapshot-portable',
    'release-cli-smoke-windows', 'issue-1850-profile-windows', 'frontend-regression']);
  const expectedJobs = ['ci-tier', 'test-debt', 'rust-regression-windows',
    'rust-slow-test-windows', 'issue-1850-release-windows', 'clippy-linux', 'rust-fmt',
    'tml-adoption', ...full];
  assert.deepEqual([...parsed.keys()].sort(), expectedJobs.sort(), 'unknown/missing job layout');
  for (const [name, lines] of parsed) {
    const keys = lines.flatMap(l => /^    ([a-z-]+):/.exec(l)?.[1] ?? []);
    assert.equal(new Set(keys).size, keys.length, 'duplicate job keys');
    const conditions = lines.filter(l => /^    if: /.test(l));
    assert.equal(conditions.length, 1, `job condition ${name}`);
    const actual = conditions[0].slice(8);
    if (name === 'tml-adoption') {
      assert.equal(actual, adoption.slice(1, -1));
      const control = exactly(steps(lines), /^      - name: Capture observations/m);
      const commands = run(control);
      for (const flag of ['cognitive_complexity', 'too_many_lines'])
        assert.equal(commands.split(`--force-warn clippy::${flag}`).length - 1, 6);
      assert.match(lines.join('\n'), /timeout-minutes: 60/);
      assert.match(lines.join('\n'), /fail-fast: false/);
      continue;
    }
    let expected = name === 'ci-tier' ? `(!${adoption})` :
      `(!${adoption}) && (${full.has(name) ?
        `needs.ci-tier.outputs.full == 'true' && (${regular})` : regular})`;
    assert.equal(actual, expected, `ordinary job eligibility ${name}`);
    assert.ok(!lines.slice(0, lines.indexOf('    steps:'))
      .some(l => /continue-on-error:/.test(l)), 'nonblocking job');
  }
  for (const [name, p] of [['rust-regression-windows', 'windows'], ['clippy-linux', 'linux'],
    ['rust-regression-macos', 'macos']]) {
    assert.ok(parsed.has(name));
    const all = steps(parsed.get(name));
    const clippy = exactly(all, /^      - name: cargo clippy$/m);
    assert.doesNotMatch(clippy, /^        (continue-on-error|if):/m);
    assert.match(clippy, /^        shell: bash$/m);
    assert.ok(run(clippy).includes(
      'cargo clippy --locked --workspace --all-targets --message-format=json'));
    for (const flag of ['cognitive_complexity', 'too_many_lines'])
      assert.ok(run(clippy).includes(`--force-warn clippy::${flag}`));
    assert.ok(run(clippy).includes(`> "\${RUNNER_TEMP}/cog-${p}.jsonl"`));
    assert.match(run(clippy), /code=\$\?/); assert.match(run(clippy), /exit "\$code"$/);
    assert.ok(run(clippy).includes('echo "COG_CLIPPY_EXIT=$code" >> "$GITHUB_ENV"'));
    const gate = exactly(all, /^      - name: Length gate$/m); blocking(gate);
    assert.match(gate, /^        shell: bash$/m);
    assert.doesNotMatch(gate, /^        if:/m);
    const commands = run(gate).split('\n').filter(Boolean);
    assert.equal(commands.length, 2, 'capture-to-check chain');
    assert.match(commands[0], /^node scripts\/check-too-many-lines\.mjs /);
    for (const flag of [`--capture "\${RUNNER_TEMP}/cog-${p}.jsonl"`,
      '--clippy-exit "$COG_CLIPPY_EXIT"', `--platform ${p}`,
      '--rustc-version "$COG_RUSTC_VERSION"', '--commit "${GITHUB_SHA}"',
      '--workspace-root "${GITHUB_WORKSPACE}"',
      `--emit "\${RUNNER_TEMP}/tml-observed-${p}.json"`]) assert.ok(commands[0].includes(flag));
    assert.equal(commands[1].replace(/\s+/g, ' '),
      `node scripts/check-too-many-lines.mjs --check "\${RUNNER_TEMP}/tml-observed-${p}.json" ` +
      '--baseline too-many-lines.baseline.json');
    const tests = all.map((s, i) => /run: cargo test/.test(s) ? i : -1).filter(i => i >= 0);
    assert.ok(tests.every(i => i < all.indexOf(gate)), 'gate must follow cargo tests');
  }
  const debt = steps(parsed.get('test-debt'));
  const self = exactly(debt, /^      - name: Length gate self-test$/m); blocking(self);
  assert.equal(run(self), 'npm run tml:self'); assert.doesNotMatch(self, /^        if:/m);
  const ratchet = exactly(debt, /^      - name: Length baseline ratchet$/m); blocking(ratchet);
  assert.equal(run(ratchet),
    'npm run tml:baseline -- --base-ref "${GATE_BASE_REF:-origin/main}"');
  assert.match(parsed.get('test-debt').join('\n'), /fetch-depth: 0/);
  const warmClippy = steps(jobs(warm).get('warm-debug'))
    .filter(s => /^      - name: cargo clippy$/m.test(s));
  assert.equal(warmClippy.length, 1);
  for (const flag of ['cognitive_complexity', 'too_many_lines'])
    assert.ok(run(warmClippy[0]).includes(`--force-warn clippy::${flag}`));
  assert.ok(workflow.includes(`${adoption} && format('-{0}', github.run_id) || ''`));
  assert.ok(workflow.includes('group: pr-regression-gates-${{ github.ref }}${{'));
  assert.ok(workflow.includes(`cancel-in-progress: \${{ !${adoption} }}`));
}
const workflow = fs.readFileSync('.github/workflows/pr-regression-gates.yml', 'utf8');
const warm = fs.readFileSync('.github/workflows/cache-warm.yml', 'utf8');
test('committed workflow contract', () => workflowContract(workflow, warm));
const mutants = [
  ['drop length flag', s => s.replace('--force-warn clippy::too_many_lines', '')],
  ['drop cognitive flag', s => s.replace('--force-warn clippy::cognitive_complexity', '')],
  ['remove check', s => s.replace('--check "${RUNNER_TEMP}/tml-observed-windows.json"', '')],
  ['step nonblocking', s => s.replace('- name: Length gate\n',
    '- name: Length gate\n        continue-on-error: true\n')],
  ['job nonblocking', s => s.replace('  rust-regression-windows:\n',
    '  rust-regression-windows:\n    continue-on-error: true\n')],
  ['fallback', s => s.replace('--baseline too-many-lines.baseline.json',
    '--baseline too-many-lines.baseline.json || true')],
  ['exit0', s => s.replace('--baseline too-many-lines.baseline.json',
    '--baseline too-many-lines.baseline.json\n          exit 0')],
  ['wrong emission', s => s.replace('--check "${RUNNER_TEMP}/tml-observed-windows.json"',
    '--check "${RUNNER_TEMP}/other.json"')],
  ['self nonblocking', s => s.replace('- name: Length gate self-test\n',
    '- name: Length gate self-test\n        continue-on-error: true\n')],
  ['invoking env override', s => s.replace('- name: Length gate\n',
    '- name: Length gate\n        env:\n          CLIPPY_CONF_DIR: elsewhere\n')],
  ['skip PR job', s => s.replace('  rust-regression-windows:\n',
    '  rust-regression-windows:\n    if: false\n')],
  ['broaden adoption skip', s => s.replaceAll("inputs.tml_adoption_source != ''",
    "inputs.tml_adoption_source == ''")],
];
for (const [name, mutate] of mutants) test(`workflow mutant: ${name}`, () => {
  const changed = mutate(workflow); assert.notEqual(changed, workflow);
  assert.throws(() => workflowContract(changed, warm));
});
test('cache warm mutant drops length flag', () => assert.throws(() =>
  workflowContract(workflow, warm.replace('--force-warn clippy::too_many_lines', ''))));
for (const job of ['rust-regression-windows', 'clippy-linux', 'rust-regression-macos']) {
  for (const flag of ['cognitive_complexity', 'too_many_lines'])
    test(`workflow mutant: ${job} drops ${flag}`, () => {
      const start = workflow.indexOf(`  ${job}:\n`);
      assert.ok(start > 0);
      const head = workflow.slice(0, start), tail = workflow.slice(start);
      const changed = head + tail.replace(`--force-warn clippy::${flag}`, '');
      assert.notEqual(changed, workflow);
      assert.throws(() => workflowContract(changed, warm));
    });
}

function fixture(runTest) {
  const ignored = spawnSync('git', ['check-ignore', '-q', 'artifacts/3507/fixtures']);
  assert.equal(ignored.status, 0);
  fs.mkdirSync('artifacts/3507/fixtures', { recursive: true });
  const root = fs.mkdtempSync(path.resolve('artifacts/3507/fixtures/case-'));
  const git = (...args) => {
    const r = spawnSync('git', ['-c', 'user.name=TML tests', '-c',
      'user.email=tml-tests@example.invalid', ...args], { cwd: root, encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr); return r.stdout.trim();
  };
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    fs.writeFileSync(path.join(root, file), text);
  };
  const commit = () => { git('add', '.'); git('commit', '-qm', 'fixture');
    return git('rev-parse', 'HEAD'); };
  try {
    git('init', '-q');
    write('.gitignore', '/output/\n');
    write('src/a.rs', 'fn heavy() {}');
    write('clippy.toml', 'cognitive-complexity-threshold = 25\n');
    write('.cargo/config.toml', fs.readFileSync('.cargo/config.toml'));
    const source = commit();
    return runTest({ root, git, write, commit, source });
  } finally { fs.rmSync(root, { recursive: true }); }
}
test('adoption proof transport and candidate pins', () => fixture(f => {
  const raw = jsonl([diagnostic(), end]);
  f.write('output/all.jsonl', raw); f.write('output/production.jsonl', raw);
  f.write('output/proof.jsonl', raw);
  const o = { capture: path.join(f.root, 'output/all.jsonl'), 'clippy-exit': '0',
    platform: 'windows', 'rustc-version': '1.98.1', commit: f.source, 'workspace-root': f.root,
    'production-capture': path.join(f.root, 'output/production.jsonl'),
    'production-clippy-exit': '0', 'adoption-source': f.source,
    'denominator-proof': path.join(f.root, 'output/proof.jsonl'), 'denominator-clippy-exit': '0' };
  assert.equal(capture(o).productionMeasured, true);
  for (const flag of ['production-capture', 'production-clippy-exit', 'adoption-source',
    'denominator-proof', 'denominator-clippy-exit']) {
    const invalid = { ...o }; delete invalid[flag]; assert.throws(() => capture(invalid));
  }
  for (const patch of [{ 'denominator-clippy-exit': '1' }, { commit: sha },
    { 'adoption-source': sha }, { 'rustc-version': '1.99.0' }])
    assert.throws(() => capture({ ...o, ...patch }));
  for (const proof of [jsonl([end]), '', '{', jsonl([diagnostic()]),
    raw.replace('(150/100)', '(150/101)')]) {
    f.write('output/proof.jsonl', proof); assert.throws(() => capture(o));
  }
  f.write('output/proof.jsonl', raw);
  const normal = { ...o };
  for (const flag of ['production-capture', 'production-clippy-exit', 'adoption-source',
    'denominator-proof', 'denominator-clippy-exit']) delete normal[flag];
  assert.throws(() => capture(normal));
  f.write('clippy.toml', 'cognitive-complexity-threshold = 25\n' +
    'too-many-lines-threshold = 100\n');
  normal.commit = f.commit();
  assert.equal(capture(normal).productionMeasured, false);
  for (const config of ['too-many-lines-threshold = 101\n',
    'cognitive-complexity-threshold = 25\ntoo-many-lines-threshold = 100\nextra = 1\n',
    'cognitive-complexity-threshold = 25\ntoo-many-lines-threshold = 100\n' +
      'too-many-lines-threshold = 100\n']) {
    f.write('clippy.toml', config); normal.commit = f.commit();
    assert.throws(() => capture(normal));
  }
}));
test('initial structural adoption and subsequent Git-only ratchet', () => fixture(f => {
  f.git('update-ref', 'refs/remotes/origin/main', f.source);
  const b = { ...baseline(), sourceCommit: f.source };
  f.write('too-many-lines.baseline.json', JSON.stringify(b)); f.commit();
  assert.doesNotThrow(() => baselineDiff('origin/main', f.root));
  assert.throws(() => baselineDiff('missing-ref', f.root));
  f.git('update-ref', 'refs/remotes/origin/main', f.git('rev-parse', 'HEAD'));
  f.write('too-many-lines.baseline.json', JSON.stringify({ ...b, entries: [] })); f.commit();
  assert.doesNotThrow(() => baselineDiff('origin/main', f.root));
  f.write('too-many-lines.baseline.json', JSON.stringify(
    { ...baseline([entry(151)]), sourceCommit: f.source })); f.commit();
  assert.throws(() => baselineDiff('origin/main', f.root));
}));
test('initial adoption rejects protected candidate changes and relevant main drift', () =>
  fixture(f => {
    f.git('update-ref', 'refs/remotes/origin/main', f.source);
    f.write('too-many-lines.baseline.json', JSON.stringify(
      { ...baseline(), sourceCommit: f.source }));
    f.write('src/a.rs', 'fn changed() {}'); f.commit();
    assert.throws(() => baselineDiff('origin/main', f.root));
    f.git('update-ref', 'refs/remotes/origin/main', f.git('rev-parse', 'HEAD'));
    // Baseline already on main closes initial exception, so independent drift fixture follows.
  }));
test('relevant main drift requires re-adoption; unrelated drift preserves S', () => fixture(f => {
  const source = f.source;
  f.write('README.md', 'unrelated'); const unrelated = f.commit();
  f.git('update-ref', 'refs/remotes/origin/main', unrelated);
  f.write('too-many-lines.baseline.json', JSON.stringify(
    { ...baseline(), sourceCommit: source }));
  f.commit();
  assert.doesNotThrow(() => baselineDiff('origin/main', f.root));
  f.git('checkout', '--detach', unrelated);
  f.write('Cargo.toml', '[workspace]\n'); const relevant = f.commit();
  f.git('update-ref', 'refs/remotes/origin/main', relevant);
  f.write('too-many-lines.baseline.json', JSON.stringify({ ...baseline(), sourceCommit: source }));
  f.commit();
  assert.throws(() => baselineDiff('origin/main', f.root));
}));
