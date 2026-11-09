// #4143: struct_excessive_bools ratchet tests. Small embedded fixtures only.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import * as m from './check-struct-excessive-bools.mjs';

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const ROOT = path.resolve(os.tmpdir(), 'seb-fixture-root');
const A = 'src-tauri/src/a.rs';
const B = 'src-tauri/src/b.rs';

const baseline = (entries, extra = {}) => ({ version: 1, lint: m.LINT, toolchain: m.TOOLCHAIN,
  threshold: 3, seedCommit: m.SEED_COMMIT, platforms: ['linux', 'windows'], entries, ...extra });
const entry = (id, platforms = ['linux', 'windows']) => ({ id, platforms });

/** A primary span for the first occurrence of `marker` through the next closing brace. */
function spanFor(file, text, marker, overrides = {}) {
  const bytes = Buffer.from(text, 'utf8');
  const index = typeof marker === 'number' ? marker : text.indexOf(marker);
  const start = Buffer.byteLength(text.slice(0, index));
  const end = Buffer.byteLength(text.slice(0, text.indexOf('}', index) + 1));
  const at = b => m.positionOf(bytes, b);
  return { file_name: file, byte_start: start, byte_end: end, line_start: at(start).line,
    column_start: at(start).column, line_end: at(end).line, column_end: at(end).column,
    is_primary: true, expansion: null, ...overrides };
}
const diag = (spans, extra = {}) => ({ reason: 'compiler-message', package_id: 'p',
  target: { name: 't' }, message: { code: { code: m.LINT }, level: 'warning',
    message: m.MESSAGE, spans, ...extra } });
const FINISHED = { reason: 'build-finished', success: true };
const jsonl = records => records.map(r => JSON.stringify(r)).join('\n') + '\n';

const SRC_A = 'pub struct Alpha {\n    a: bool,\n    b: bool,\n    c: bool,\n    d: bool,\n}\n';
const SRC_B = 'struct Beta { a: bool, b: bool, c: bool, d: bool }\n';

/** Runs native mode over an in-memory checkout. */
function native({ files = { [A]: SRC_A, [B]: SRC_B }, records, base, host = 'windows',
  exit = 0, tracked } = {}) {
  const store = new Map();
  for (const [file, text] of Object.entries(files))
    store.set(path.join(ROOT, file), Buffer.isBuffer(text) ? text : Buffer.from(text, 'utf8'));
  store.set('capture.jsonl', Buffer.from(records ?? jsonl([FINISHED])));
  store.set('baseline.json', Buffer.from(typeof base === 'string' ? base :
    JSON.stringify(base ?? baseline([]))));
  const out = [], err = [];
  const code = m.main(['--capture', 'capture.jsonl', '--clippy-exit', String(exit),
    '--platform', host, '--rustc-version', m.TOOLCHAIN, '--workspace-root', ROOT,
    '--baseline', 'baseline.json'], {
    tracked: tracked ?? Object.keys(files),
    readFile: f => { if (!store.has(f)) throw new Error(`ENOENT ${f}`); return store.get(f); },
    out: s => out.push(s), err: s => err.push(s) });
  return { code, out: out.join('\n'), err: err.join('\n') };
}
const ID_A = `rust:${A}::Alpha`, ID_B = `rust:${B}::Beta`;
const capAB = () => jsonl([diag([spanFor(A, SRC_A, 'pub struct')]),
  diag([spanFor(B, SRC_B, 'struct Beta')]), FINISHED]);

test('accepted sites on the host pass and print observed ids', () => {
  const r = native({ records: capAB(), base: baseline([entry(ID_A), entry(ID_B)]) });
  assert.equal(r.code, 0, r.err);
  assert.match(r.out, /OBSERVED rust:src-tauri\/src\/a\.rs::Alpha/);
  assert.match(r.out, /observed=2 baseline=2 new=0 stale=0/);
});

test('NEW site fails with exit 1', () => {
  const r = native({ records: capAB(), base: baseline([entry(ID_A)]) });
  assert.equal(r.code, 1);
  assert.match(r.out, /NEW rust:src-tauri\/src\/b\.rs::Beta/);
});

test('stale site and stale host membership fail', () => {
  const caps = jsonl([diag([spanFor(A, SRC_A, 'pub struct')]), FINISHED]);
  const r = native({ records: caps, base: baseline([entry(ID_A), entry(ID_B)]) });
  assert.equal(r.code, 1);
  assert.match(r.out, /STALE rust:src-tauri\/src\/b\.rs::Beta/);
  const linuxOnly = native({ records: capAB(), host: 'linux',
    base: baseline([entry(ID_A), entry(ID_B, ['windows'])]) });
  assert.equal(linuxOnly.code, 1);
  assert.match(linuxOnly.out, /NEW .*Beta/);
});

test('removing A does not buy B', () => {
  const caps = jsonl([diag([spanFor(B, SRC_B, 'struct Beta')]), FINISHED]);
  const r = native({ records: caps, base: baseline([entry(ID_A)]) });
  assert.equal(r.code, 1);
  assert.match(r.out, /NEW .*Beta/);
  assert.match(r.out, /STALE .*Alpha/);
});

test('valid complete zero diagnostics is a typed zero', () => {
  assert.equal(native({ base: baseline([]) }).code, 0);
  assert.equal(native({ base: baseline([entry(ID_A)]) }).code, 1);
});

test('failed, empty, nonzero, incomplete and error captures are invalid input', () => {
  const bad = [
    { records: '' }, { records: jsonl([FINISHED]), exit: 101 },
    { records: jsonl([diag([spanFor(A, SRC_A, 'pub struct')])]) },
    { records: jsonl([{ reason: 'build-finished', success: false }]) },
    { records: jsonl([FINISHED, FINISHED]) },
    { records: jsonl([FINISHED, diag([spanFor(A, SRC_A, 'pub struct')])]) },
    { records: jsonl([{ reason: 'compiler-message', message: { level: 'error',
      message: 'boom', spans: [] } }, FINISHED]) },
    { records: '{"reason":"build-finished","success":true,"success":true}\n' },
    { records: 'not json\n' + jsonl([FINISHED]) },
    { records: jsonl([{ no: 'reason' }, FINISHED]) },
  ];
  for (const b of bad) assert.equal(native({ ...b, base: baseline([]) }).code, 2, b.records);
});

test('only the pinned diagnostic wording is valid', () => {
  const wrong = jsonl([diag([spanFor(A, SRC_A, 'pub struct')],
    { message: 'more than 100 bools in a struct' }), FINISHED]);
  const r = native({ records: wrong, base: baseline([entry(ID_A)]) });
  assert.equal(r.code, 2);
  assert.match(r.err, /wording/);
});

test('max-struct-bools override is rejected before equality, root and nested', () => {
  for (const [file, text] of [['clippy.toml', 'max-struct-bools = 100\n'],
    ['.clippy.toml', '"max-struct-bools"=100\n'],
    ['src-tauri/clippy.toml', "  'max-struct-bools'   =  3\n"],
    ['crates/x/.clippy.toml', 'too-many-lines-threshold = 100\nmax-struct-bools=3\n']]) {
    const r = native({ files: { [A]: SRC_A, [file]: text }, base: baseline([]) });
    assert.equal(r.code, 2, file);
    assert.match(r.err, /max-struct-bools/);
  }
  const other = native({ files: { [A]: SRC_A, 'clippy.toml': 'too-many-lines-threshold = 1\n' },
    base: baseline([]) });
  assert.equal(other.code, 0, other.err);
  const untracked = native({ files: { [A]: SRC_A, 'clippy.toml': 'max-struct-bools = 9\n' },
    tracked: [A], base: baseline([]) });
  assert.equal(untracked.code, 0, 'only tracked config files are policy');
});

test('identity is stable across comments, line moves, CRLF and Unicode text', () => {
  const variants = [
    'pub(crate) struct Alpha { a: bool, b: bool, c: bool, d: bool }\n',
    '// ñandú 日本\r\n\r\n\r\npub struct Alpha {\r\n  a: bool, b: bool,\r\n}\r\n',
    '/* é */ pub(in crate::x) /* a /* nested */ b */ struct // c\n  Alpha { a: bool }\n',
    'pub(super)\nstruct\tAlpha { x: bool }\n',
  ];
  for (const text of variants) {
    const marker = text.includes('/* é */') ? '/* é */' : 'pub';
    const caps = jsonl([diag([spanFor(A, text, marker)]), FINISHED]);
    const r = native({ files: { [A]: text }, records: caps, base: baseline([entry(ID_A)]) });
    assert.equal(r.code, 0, `${JSON.stringify(text)}: ${r.err}`);
  }
});

test('raw identifiers and private structs keep their name', () => {
  const text = 'struct r#Gamma { a: bool }\n';
  const caps = jsonl([diag([spanFor(A, text, 'struct')]), FINISHED]);
  const r = native({ files: { [A]: text }, records: caps,
    base: baseline([entry(`rust:${A}::r#Gamma`)]) });
  assert.equal(r.code, 0, r.err);
});

test('containers enter the identity; moves create a new site', () => {
  const text = 'mod inner {\n    pub struct Alpha { a: bool }\n}\n';
  const caps = jsonl([diag([spanFor(A, text, 'pub struct')]), FINISHED]);
  const r = native({ files: { [A]: text }, records: caps, base: baseline([entry(ID_A)]) });
  assert.equal(r.code, 1);
  assert.match(r.out, /NEW rust:src-tauri\/src\/a\.rs::mod:inner::Alpha/);
});

test('unsupported declaration prefixes fail closed', () => {
  for (const text of ['enum Alpha { A }\n', 'pub(foo) struct Alpha {}\n',
    'struct Ålpha { a: bool }\n', 'structAlpha {}\n', 'struct _ { a: bool }\n',
    '/* open struct Alpha { a: bool }\n']) {
    const caps = jsonl([diag([spanFor(A, text, text[0])]), FINISHED]);
    const r = native({ files: { [A]: text }, records: caps, base: baseline([]) });
    assert.equal(r.code, 2, text);
  }
});

test('whole-item allow diagnostics after an attribute are accepted', () => {
  const text = '#[allow(clippy::struct_excessive_bools)]\npub struct Alpha { a: bool }\n';
  const caps = jsonl([diag([spanFor(A, text, 'pub struct')]), FINISHED]);
  assert.equal(native({ files: { [A]: text }, records: caps,
    base: baseline([entry(ID_A)]) }).code, 0);
});

test('duplicate targets deduplicate; colliding declarations fail', () => {
  const dup = jsonl([diag([spanFor(A, SRC_A, 'pub struct')]),
    diag([spanFor(A, SRC_A, 'pub struct')]), FINISHED]);
  const r = native({ records: dup, base: baseline([entry(ID_A)]) });
  assert.equal(r.code, 0, r.err);
  assert.match(r.out, /observed=1/);
  const text = '#[cfg(unix)]\nstruct Alpha { a: bool }\n' +
    '#[cfg(windows)]\nstruct Alpha { b: bool }\n';
  const second = text.indexOf('struct Alpha', text.indexOf('windows'));
  const collide = jsonl([diag([spanFor(A, text, 'struct Alpha')]),
    diag([spanFor(A, text, second)]), FINISHED]);
  const c = native({ files: { [A]: text }, records: collide, base: baseline([entry(ID_A)]) });
  assert.equal(c.code, 2);
  assert.match(c.err, /share identity/);
});

test('conflicting, expanded, outside-root, untracked and multi-primary spans fail', () => {
  const s = spanFor(A, SRC_A, 'pub struct');
  const cases = [
    [{ ...s, line_start: 2 }], [{ ...s, column_start: 3 }], [{ ...s, byte_end: 9999 }],
    [{ ...s, byte_start: 1.5 }], [{ ...s, expansion: { span: {} } }],
    [{ ...s, file_name: '../outside.rs' }], [{ ...s, file_name: 'src-tauri/src/gen.rs' }],
    [s, { ...s, byte_start: 0 }], [{ ...s, is_primary: false }],
  ];
  for (const spans of cases) {
    const r = native({ records: jsonl([diag(spans), FINISHED]), base: baseline([entry(ID_A)]) });
    assert.equal(r.code, 2, JSON.stringify(spans[0]));
  }
});

test('invalid UTF-8 source bytes and non-boundary offsets fail', () => {
  const bytes = Buffer.concat([Buffer.from('// \xff'), Buffer.from([0xff]),
    Buffer.from('\nstruct Alpha { a: bool }\n')]);
  const caps = jsonl([diag([spanFor(A, SRC_A, 'pub struct')]), FINISHED]);
  assert.equal(native({ files: { [A]: bytes }, records: caps, base: baseline([]) }).code, 2);
  const text = '// é\nstruct Alpha { a: bool }\n';
  const s = spanFor(A, text, 'struct');
  const mid = { ...s, byte_start: 4 };
  assert.equal(native({ files: { [A]: text }, records: jsonl([diag([mid]), FINISHED]),
    base: baseline([]) }).code, 2);
});

test('wrong host, toolchain and malformed arguments are invalid input', () => {
  const run = argv => m.main(argv, { out: () => {}, err: () => {} });
  const ok = ['--capture', 'c', '--clippy-exit', '0', '--platform', 'windows',
    '--rustc-version', m.TOOLCHAIN, '--workspace-root', '.', '--baseline', 'b'];
  const swap = (k, v) => ok.map((x, i) => ok[i - 1] === k ? v : x);
  assert.equal(run(swap('--platform', 'macos')), 2);
  assert.equal(run(swap('--rustc-version', '1.99.0')), 2);
  assert.equal(run(swap('--clippy-exit', 'x')), 2);
  assert.equal(run([...ok, '--capture', 'd']), 2);
  assert.equal(run([...ok, '--bogus', 'x']), 2);
  assert.equal(run([...ok, '--baseline-diff']), 2);
  assert.equal(run(ok.slice(0, -1)), 2);
  assert.equal(run(['--baseline-diff']), 2);
  assert.equal(run(['--baseline-diff', '--base-ref', 'x', '--platform', 'linux']), 2);
});

test('baseline schema rejects edits, duplicates and unsorted data', () => {
  const good = baseline([entry(ID_A), entry(ID_B, ['windows'])]);
  assert.doesNotThrow(() => m.validateBaseline(good));
  const bad = [
    { ...good, threshold: 100 }, { ...good, toolchain: '1.99.0' }, { ...good, version: 2 },
    { ...good, lint: 'clippy::x' }, { ...good, seedCommit: 'f'.repeat(40) },
    { ...good, platforms: ['windows'] }, { ...good, extra: 1 },
    baseline([entry(ID_B), entry(ID_A)]), baseline([entry(ID_A), entry(ID_A)]),
    baseline([entry(ID_A, [])]), baseline([entry(ID_A, ['windows', 'linux'])]),
    baseline([entry(ID_A, ['macos'])]), baseline([{ id: ID_A, platforms: ['linux'], n: 1 }]),
  ];
  for (const b of bad) assert.throws(() => m.validateBaseline(b), m.InputError);
  assert.throws(() => m.parseBaseline('{"version":1,"version":1}'), /duplicate JSON key/);
  const metaLess = { ...good }; delete metaLess.seedCommit;
  assert.throws(() => m.validateBaseline(metaLess), m.InputError);
});

test('ratchet allows pruning ids and hosts but rejects refill and growth', () => {
  const base = baseline([entry(ID_A), entry(ID_B)]);
  assert.deepEqual(m.ratchet(baseline([entry(ID_A, ['windows'])]), base), []);
  assert.deepEqual(m.ratchet(baseline([]), base), []);
  assert.match(m.ratchet(baseline([entry(ID_A), entry(`rust:${A}::Gamma`)]), base).join(),
    /NEW baseline site/);
  assert.match(m.ratchet(base, baseline([entry(ID_A, ['windows']), entry(ID_B)])).join(),
    /NEW host linux/);
  assert.match(m.ratchet(baseline([entry(ID_A)]), baseline([entry(ID_B)])).join(),
    /NEW baseline site/);
  assert.match(m.ratchet({ ...base, threshold: 4 }, base).join(), /metadata threshold/);
});

test('adoption requires the exact frozen seed, not just 33 ids', () => {
  assert.equal(m.SEED.length, 33);
  assert.deepEqual([...m.SEED].sort(), [...m.SEED]);
  const seeded = baseline(m.SEED.map(id => entry(id, ['windows'])));
  assert.deepEqual(m.ratchet(seeded, null), []);
  const other = baseline([...m.SEED.slice(1), `rust:${A}::Zeta`].sort().map(id => entry(id)));
  assert.match(m.ratchet(other, null).join(), /exact frozen 33-site seed/);
  assert.match(m.ratchet(baseline([]), null).join(), /exact frozen/);
});

test('committed baseline is valid and lists exactly the seed', () => {
  const b = m.parseBaseline(fs.readFileSync(path.join(REPO, m.BASELINE), 'utf8'));
  assert.deepEqual(b.entries.map(e => e.id), [...m.SEED]);
});

function gitRepo() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'seb-git-'));
  const git = (...args) => {
    const r = spawnSync('git', args, { cwd: dir, encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr);
    return r.stdout.trim();
  };
  git('init', '-q', '-b', 'main');
  git('config', 'user.email', 't@example.invalid'); git('config', 'user.name', 't');
  git('config', 'core.autocrlf', 'false');
  const commit = (text, msg) => {
    if (text === null) fs.rmSync(path.join(dir, m.BASELINE), { force: true });
    else fs.writeFileSync(path.join(dir, m.BASELINE), text);
    fs.writeFileSync(path.join(dir, 'f.txt'), msg);
    git('add', '-A'); git('commit', '-q', '-m', msg);
    return git('rev-parse', 'HEAD');
  };
  return { dir, git, commit };
}
const diffRun = (dir, ref) => m.runDiff(ref, dir);

test('baseline-diff reads committed blobs: prune passes, refill/deletion/new fail', () => {
  const { dir, git, commit } = gitRepo();
  const j = v => JSON.stringify(v, null, 2) + '\n';
  const base = commit(j(baseline([entry(ID_A), entry(ID_B)])), 'base');
  git('branch', 'base');
  commit(j(baseline([entry(ID_A, ['windows'])])), 'prune');
  assert.equal(diffRun(dir, 'base').exit, 0);
  fs.writeFileSync(path.join(dir, m.BASELINE), j(baseline([entry(ID_A), entry(`rust:${A}::Z`)])));
  assert.equal(diffRun(dir, 'base').exit, 0, 'uncommitted disk edits are ignored');
  git('checkout', '-q', '--', m.BASELINE);
  commit(j(baseline([entry(ID_A), entry(ID_B)])), 'refill');
  assert.equal(diffRun(dir, 'base').exit, 0, 'refill relative to merge-base equals base');
  git('checkout', '-q', '-b', 'grow', base);
  commit(j(baseline([entry(ID_A), entry(`rust:${A}::Z`), entry(ID_B)])), 'grow');
  assert.equal(diffRun(dir, 'base').exit, 1);
  const pruned = commit(j(baseline([entry(ID_A, ['windows'])])), 'pruned');
  git('branch', 'merged-prune', pruned);
  commit(j(baseline([entry(ID_A)])), 'refill-after-merge');
  assert.equal(diffRun(dir, 'merged-prune').exit, 1, 'merged pruning cannot be refilled');
  git('checkout', '-q', '-b', 'del', base);
  commit(null, 'delete');
  assert.throws(() => diffRun(dir, 'base'), /deleted/);
  assert.equal(m.main(['--baseline-diff', '--base-ref', 'base'], { root: dir,
    out: () => {}, err: () => {} }), 2);
  assert.throws(() => diffRun(dir, 'no-such-ref'), /fetch/);
});

test('baseline-diff adoption: missing base baseline needs the exact seed', () => {
  const { dir, git, commit } = gitRepo();
  commit(null, 'base'); git('branch', 'base');
  const j = v => JSON.stringify(v, null, 2) + '\n';
  commit(j(baseline(m.SEED.map(id => entry(id, ['windows'])))), 'adopt');
  assert.equal(diffRun(dir, 'base').exit, 0);
  commit(j(baseline(m.SEED.slice(1).map(id => entry(id)))), 'short');
  assert.equal(diffRun(dir, 'base').exit, 1);
});

// ---- workflow contracts ----

const WORKFLOW = fs.readFileSync(path.join(REPO, '.github/workflows/pr-regression-gates.yml'),
  'utf8').replace(/\r\n/g, '\n');
function jobBody(job) {
  const start = WORKFLOW.indexOf(`\n  ${job}:\n`);
  assert.ok(start >= 0, `missing job ${job}`);
  const rest = WORKFLOW.slice(start + 1), next = rest.slice(1).search(/\n  [a-z][\w-]*:\n/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}
function steps(body) {
  return body.split(/\n(?=      - name: )/).slice(1).map(s => ({
    name: /^      - name: (.*)$/m.exec(s)[1].replace(/^"|"$/g, ''), text: s }));
}

for (const [job, host] of [['rust-regression-windows', 'windows'], ['clippy-linux', 'linux']]) {
  test(`${host} capture carries the flag once and the blocking gate follows seven-lint`, () => {
    const list = steps(jobBody(job));
    const i = list.findIndex(s => s.name === 'cargo clippy');
    assert.ok(i >= 0);
    const cap = list[i].text;
    assert.equal(cap.split('--force-warn clippy::struct_excessive_bools').length, 2);
    assert.match(cap, /cargo clippy --locked --workspace --all-targets --message-format=json/);
    assert.ok(cap.includes(`"\${RUNNER_TEMP}/cog-${host}.jsonl"`));
    assert.match(cap, /"1\.98\.1"/);
    assert.equal(list[i + 1].name, 'Seven-lint gate');
    const gate = list[i + 2];
    assert.equal(gate.name, 'Struct-excessive-bools gate (#4143)');
    assert.doesNotMatch(gate.text, /continue-on-error|\n        if:/);
    assert.match(gate.text, /timeout-minutes: 5\n/);
    for (const part of ['node scripts/check-struct-excessive-bools.mjs',
      `--capture "\${RUNNER_TEMP}/cog-${host}.jsonl"`, '--clippy-exit "$COG_CLIPPY_EXIT"',
      `--platform ${host}`, '--rustc-version "$COG_RUSTC_VERSION"',
      '--workspace-root "$GITHUB_WORKSPACE"', '--baseline struct-excessive-bools.baseline.json'])
      assert.ok(gate.text.includes(part), part);
  });
}

test('test-debt runs the self-test and committed ratchet blocking', () => {
  const list = steps(jobBody('test-debt'));
  const self = list.find(s => s.name === 'Struct-excessive-bools self-test (#4143)');
  const diff = list.find(s => s.name === 'Struct-excessive-bools baseline ratchet (#4143)');
  assert.ok(self && diff);
  assert.ok(self.text.includes('node --test scripts/check-struct-excessive-bools.test.mjs'));
  assert.ok(diff.text.replace(/\n\s+/g, ' ').includes(
    'node scripts/check-struct-excessive-bools.mjs --baseline-diff --base-ref "${GATE_BASE_REF:-origin/main}"'));
  assert.ok(diff.text.includes("format('origin/{0}', github.event.pull_request.base.ref)"));
  for (const s of [self, diff]) {
    assert.doesNotMatch(s.text, /continue-on-error/);
    assert.match(s.text, /timeout-minutes: 5\n/);
  }
  assert.match(jobBody('test-debt'), /node-version: 22/);
});
