import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { resolveGit } from './resolve-git.mjs';

const WIN = String.raw`(?:windows|target_os\s*=\s*"windows"|target_family\s*=\s*"windows")`;
const PRED = String.raw`(?:\bunix\b|not\s*\(\s*` + WIN + String.raw`\s*\)`
  + String.raw`|target_os\s*=\s*"(?:linux|macos)")`;
const OWN = new RegExp(String.raw`\bcfg(?:_attr)?!?\s*\([^;{}\]]*?` + PRED);
const WINM = new RegExp(String.raw`\bcfg!\s*\(\s*` + WIN + String.raw`\s*\)`);
const gated = name => new RegExp(String.raw`#\[\s*cfg\s*\([^\]]*?` + PRED
  + String.raw`[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+`
  + name + String.raw`\s*;`);
const SCOPE = new RegExp(String.raw`^(?:src-tauri\/|crates\/`
  + String.raw`(?:terminal-snapshot-renderer|rust-source-scrub)\/src\/).+\.rs$`);
const GATED_SCOPE = /^src-tauri\/.+\.rs$/;
const MANIFESTS = new Set(['Cargo.toml', 'Cargo.lock', 'src-tauri/Cargo.toml',
  'crates/terminal-snapshot-renderer/Cargo.toml', 'crates/rust-source-scrub/Cargo.toml']);
const DETECTOR = 'scripts/ci-unix-rust-changes.mjs';
const WORKFLOW = '.github/workflows/pr-regression-gates.yml';
const CATALOG = 'scripts/pr-regression-paths.json';
const ROOTS = new Set(['lib.rs', 'main.rs', 'build.rs']);

// The catalog is data: a JSON array of exact, unique, sorted Git paths that the
// workflow reads as its own definition. Anything else is an error, never a match.
function parseCatalog(text, where) {
  if (text.length === 0) throw new Error(`${where}: empty catalog`);
  let list;
  try { list = JSON.parse(text); } catch { throw new Error(`${where}: invalid JSON`); }
  if (!Array.isArray(list)) throw new Error(`${where}: not a JSON array`);
  list.forEach((entry, i) => {
    const parts = typeof entry === 'string' ? entry.split('/') : [''];
    if (typeof entry !== 'string' || entry.startsWith('/') || /[\\*?[\]{}]/.test(entry)
      || parts.some(part => part === '' || part === '.' || part === '..'))
      throw new Error(`${where}: invalid path ${JSON.stringify(entry)}`);
    if (i > 0 && !(list[i - 1] < entry))
      throw new Error(`${where}: paths not unique and sorted at ${JSON.stringify(entry)}`);
  });
  return list;
}

function detector(repo, env = process.env) {
  const git = (args, extra = {}) => execFileSync(resolveGit(), args, {
    cwd: repo, env, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...extra,
  });
  function readAt(rev, file) {
    try {
      git(['cat-file', '-e', `${rev}:${file}`], { stdio: ['ignore', 'ignore', 'ignore'] });
    } catch { return null; }
    return git(['show', `${rev}:${file}`]);
  }
  function declaringAt(rev, file) {
    const base = path.posix.basename(file);
    if (ROOTS.has(base) || /^src-tauri\/(?:tests|benches|examples)\/[^/]+\.rs$/.test(file))
      return null;
    const dir = path.posix.dirname(file);
    const name = base === 'mod.rs' ? path.posix.basename(dir) : base.slice(0, -3);
    const parent = base === 'mod.rs' ? path.posix.dirname(dir) : dir;
    for (const candidate of [`${parent}.rs`, `${parent}/mod.rs`,
      `${parent}/lib.rs`, `${parent}/main.rs`]) {
      if (candidate === file) continue;
      const text = readAt(rev, candidate);
      if (text !== null) return { file: candidate, text, name };
    }
    return null;
  }
  function gatedBy(rev, file) {
    for (let depth = 0; depth < 32; depth++) {
      const declaring = declaringAt(rev, file);
      if (!declaring) return null;
      if (gated(declaring.name).test(declaring.text)) return declaring.file;
      file = declaring.file;
    }
    return null;
  }
  function reasonAt(rev, file) {
    const text = readAt(rev, file);
    if (text === null) return null;
    if (OWN.test(text)) return 'own-cfg';
    if (WINM.test(text)) return 'cfg-macro';
    if (!GATED_SCOPE.test(file)) return null;
    const parent = gatedBy(rev, file);
    return parent ? `gated-by ${parent}` : null;
  }
  // Reads the catalog from the Git object of `rev`, never from the worktree. A
  // tree without it (history before the catalog existed) yields no data paths.
  function catalogAt(rev) {
    const row = git(['ls-tree', '-z', rev, '--', CATALOG]).split('\0').filter(Boolean);
    if (row.length === 0) return [];
    const [meta] = row[0].split('\t');
    const [mode, type, oid] = meta.split(' ');
    if (row.length !== 1 || mode !== '100644' || type !== 'blob')
      throw new Error(`${rev}:${CATALOG}: not a regular file`);
    return parseCatalog(git(['cat-file', 'blob', oid]), `${rev}:${CATALOG}`);
  }
  function reasonFor(file, base, head, data) {
    if (file === DETECTOR) return 'detector';
    if (file === WORKFLOW || file === CATALOG || data.has(file)) return 'workflow';
    if (MANIFESTS.has(file)) return 'manifest';
    if (!SCOPE.test(file)) return null;
    for (const rev of [head, base]) {
      const reason = reasonAt(rev, file);
      if (reason) return reason;
    }
    return null;
  }
  function detect(base, head, event) {
    if (event && event !== 'pull_request') return { match: false, reasons: [], event };
    for (const rev of [base, head])
      git(['rev-parse', '--verify', '--quiet', `${rev}^{commit}`],
        { stdio: ['ignore', 'pipe', 'ignore'] });
    const changed = git(['diff', '--no-renames', '--name-only', '-z', base, head])
      .split('\0').filter(Boolean);
    const data = new Set([...catalogAt(base), ...catalogAt(head)]);
    const reasons = [];
    for (const file of changed) {
      const reason = reasonFor(file, base, head, data);
      if (reason) reasons.push(`${file}: ${reason}`);
    }
    return { match: reasons.length > 0, reasons };
  }
  return { git, detect };
}

// Write or delete (null text) each fixture file of one case.
function applyChanges(repo, write, changes) {
  for (const [file, text] of Object.entries(changes)) {
    if (text === null) fs.unlinkSync(path.join(repo, file));
    else write(file, text);
  }
}

// Run this script as a CLI against the fixture repo and check status, output and stdout.
function runCliCase(repo, env, base, [id, head, event, expected, status]) {
  const output = path.join(repo, `${id}.output`);
  fs.writeFileSync(output, '');
  let code = 0, stdout = '';
  try {
    stdout = execFileSync(process.execPath, [fileURLToPath(import.meta.url),
      '--base', base, '--head', head, '--repo', repo], {
      env: { ...env, GITHUB_EVENT_NAME: event, GITHUB_OUTPUT: output },
      encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (error) { code = error.status; }
  assert.equal(code, status, id);
  assert.equal(fs.readFileSync(output, 'utf8'), expected, id);
  if (id === 'C3') assert.ok(stdout.includes('(event push)'), id);
  if (id === 'C4') assert.ok(stdout.includes(`  ${CATALOG}: workflow\n`), id);
}

function selfTest() {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'ac4510-'));
  const env = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' };
  const { git, detect } = detector(repo, env);
  const write = (file, text) => {
    fs.mkdirSync(path.dirname(path.join(repo, file)), { recursive: true });
    fs.writeFileSync(path.join(repo, file), text);
  };
  const commit = () => {
    git(['add', '.']);
    git(['-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=',
      'commit', '-qm', 'fixture', '--allow-empty']);
  };
  const src = 'src-tauri/src/';
  const DATA = '.github/actions/fixture/action.yml';
  const NEIGHBOUR = '.github/actions/fixture/other.yml';
  const ADDED = '.github/actions/added/action.yml';
  const catalog = list => `${JSON.stringify(list, null, 2)}\n`;
  const flow = file => `${file}: workflow`;
  const fixture = {
    [`${src}lib.rs`]: 'mod screenshot;\nmod plainmod;\nmod a;\n',
    [`${src}screenshot.rs`]: '#[cfg(unix)]\nmod native;\n#[cfg(windows)]\nmod win;\n',
    [`${src}screenshot/native.rs`]: 'mod portal;\n',
    [`${src}screenshot/native/portal.rs`]: 'pub fn p() {}\n',
    [`${src}plainmod.rs`]: '#[cfg(unix)]\nfn u() {}\nmod child;\n',
    [`${src}plainmod/child.rs`]: 'pub fn c() {}\n',
    [`${src}a.rs`]: '#[cfg(not(windows))]\npub(crate) mod b;\n',
    [`${src}a/b/mod.rs`]: 'pub fn b() {}\n',
    [`${src}old.rs`]: '#[cfg(unix)]\nfn o() {}\n',
    [`${src}win.rs`]: 'fn w() {}\n',
    'Cargo.toml': '[workspace]\n', 'Cargo.lock': 'version = 4\n',
    'src-tauri/Cargo.toml': '[package]\nname = "t"\n',
    'crates/rust-source-scrub/Cargo.toml': '[package]\nname = "s"\n',
    [DATA]: 'name: d\n', [NEIGHBOUR]: 'name: n\n',
    [CATALOG]: catalog([DATA]),
  };
  const own = (id, file, text) => [id, { [src + file]: text }, [`${src + file}: own-cfg`]];
  const cfg = (id, file, text) => [id, { [src + file]: text }, [`${src + file}: cfg-macro`]];
  const manifest = file => `${file}: manifest`;
  const cases = [
    own('P1', 'x.rs', '#[cfg(unix)]\nfn x() {}'),
    own('P2', 'y.rs', '#[cfg(any(\n  target_os = "linux",\n  windows\n))]\nfn y() {}'),
    own('P3', 'z.rs', 'fn z() { if cfg!(not(windows)) {} }'),
    ['P4', { [`${src}screenshot/native.rs`]: 'mod portal;\npub fn n() {}' },
      [`${src}screenshot/native.rs: gated-by ${src}screenshot.rs`]],
    ['P5', { [`${src}screenshot/native/portal.rs`]: 'pub fn p() { let x = 1; }' },
      [`${src}screenshot/native/portal.rs: gated-by ${src}screenshot.rs`]],
    ['P6', { [`${src}old.rs`]: null }, [`${src}old.rs: own-cfg`]],
    ['P7', { [DETECTOR]: '// x\n' }, [`${DETECTOR}: detector`]],
    ['P8', { [`${src}a/b/mod.rs`]: 'pub fn b() { let x = 1; }' },
      [`${src}a/b/mod.rs: gated-by ${src}a.rs`]],
    own('P9', 'q.rs', '#[cfg(not(target_os = "windows"))]\nfn q() {}'),
    own('P10', 'r.rs', '#[cfg(not(target_family = "windows"))]\nfn r() {}'),
    own('P11', 's.rs', '#[cfg(target_family = "unix")]\nfn s() {}'),
    own('P12', 't.rs', '#[cfg_attr(unix, derive(Debug))]\nstruct T;'),
    own('P13', 'm.rs', '#[cfg(target_os = "macos")]\nfn m() {}'),
    cfg('P14', 'w1.rs', 'fn f() { if cfg!(windows) { a() } else { b() } }'),
    cfg('P15', 'w2.rs', 'fn f() -> bool { cfg!(target_os = "windows") }'),
    cfg('P16', 'w3.rs', 'fn f() -> bool { cfg!(target_family = "windows") }'),
    ['P17', { 'src-tauri/Cargo.toml': fixture['src-tauri/Cargo.toml']
      + "[target.'cfg(unix)'.dependencies]\n" }, [manifest('src-tauri/Cargo.toml')]],
    ['P18', { 'Cargo.lock': fixture['Cargo.lock'] + '# x\n' }, [manifest('Cargo.lock')]],
    ['P19', { 'crates/terminal-snapshot-renderer/src/u.rs': '#[cfg(unix)]\nfn u() {}' },
      ['crates/terminal-snapshot-renderer/src/u.rs: own-cfg']],
    ['P20', { 'Cargo.toml': fixture['Cargo.toml'] + '# x\n' }, [manifest('Cargo.toml')]],
    ['P21', { 'crates/rust-source-scrub/Cargo.toml':
      fixture['crates/rust-source-scrub/Cargo.toml'] + '# x\n',
    'crates/terminal-snapshot-renderer/Cargo.toml': '[package]\n' },
    [manifest('crates/rust-source-scrub/Cargo.toml'),
      manifest('crates/terminal-snapshot-renderer/Cargo.toml')]],
    ['P22', { [WORKFLOW]: 'jobs: {}\n' }, [`${WORKFLOW}: workflow`]],
    ['N1', { [`${src}win.rs`]: '#[cfg(windows)]\nfn w() {}' }, []],
    ['N2', { 'docs/n.md': '#[cfg(unix)]' }, []],
    ['N3', { [`${src}plainmod/child.rs`]: 'pub fn c() { let x = 1; }' }, []],
    ['N4', { 'crates/session-bridge/src/lib.rs': '#[cfg(unix)]\nfn k() {}' }, []],
    ['N5', {}, []],
    ['N6', { 'crates/rust-source-scrub/tests/k.rs': '#[cfg(unix)]\nfn k() {}' }, []],
    ['K1', { [DATA]: 'name: d2\n' }, [flow(DATA)]],
    ['K2', { [ADDED]: 'name: a\n', [CATALOG]: catalog([ADDED, DATA]) },
      [flow(ADDED), flow(CATALOG)]],
    ['K3', { [DATA]: null }, [flow(DATA)]],
    ['K4', { [CATALOG]: null }, [flow(CATALOG)]],
    ['K5', { [CATALOG]: null, 'scripts/pr-regression-paths.old.json': catalog([DATA]) },
      [flow(CATALOG)]],
    ['K6', { [CATALOG]: null, [DATA]: null }, [flow(DATA), flow(CATALOG)]],
    ['K7', { [DATA]: null, [CATALOG]: catalog([]) }, [flow(DATA), flow(CATALOG)]],
    ['K8', { [DATA]: null, '.github/actions/fixture/renamed.yml': 'name: d\n' }, [flow(DATA)]],
    ['N7', { [NEIGHBOUR]: 'name: n2\n' }, []],
  ];
  const broken = [
    ['B1', ''], ['B2', '{"a": 1}\n'], ['B3', 'not json\n'], ['B4', catalog([DATA, DATA])],
    ['B5', catalog([NEIGHBOUR, DATA])], ['B6', catalog(['/abs.yml'])],
    ['B7', catalog(['.github/*.yml'])], ['B8', catalog(['a/../b.yml'])],
    ['B9', catalog([String.raw`a\b.yml`])], ['B10', catalog([7])],
  ];
  try {
    git(['init', '-q']);
    git(['config', 'user.name', 'fixture']);
    git(['config', 'user.email', 'fixture@example.invalid']);
    git(['config', 'core.autocrlf', 'false']);
    for (const [file, text] of Object.entries(fixture)) write(file, text);
    commit();
    const base = git(['rev-parse', 'HEAD']).trim();
    const heads = {};
    for (const [id, changes, reasons] of cases) {
      git(['reset', '--hard', base]);
      git(['clean', '-fdq']);
      applyChanges(repo, write, changes);
      commit();
      heads[id] = git(['rev-parse', 'HEAD']).trim();
      assert.deepEqual(detect(base, heads[id], 'pull_request'),
        { match: reasons.length > 0, reasons }, id);
    }
    for (const [id, text] of broken) {
      git(['reset', '--hard', base]);
      write(CATALOG, text);
      commit();
      heads[id] = git(['rev-parse', 'HEAD']).trim();
      assert.throws(() => detect(base, heads[id], 'pull_request'), /pr-regression-paths\.json/, id);
    }
    git(['reset', '--hard', base]);
    fs.rmSync(path.join(repo, CATALOG));
    write(`${CATALOG}/x`, '[]\n');
    commit();
    heads.B11 = git(['rev-parse', 'HEAD']).trim();
    assert.throws(() => detect(base, heads.B11, 'pull_request'), /not a regular file/, 'B11');
    assert.deepEqual(detect(base, heads.B1, 'push'),
      { match: false, reasons: [], event: 'push' }, 'B12');
    const cliCases = [
      ['C1', heads.P1, 'pull_request', 'match=true\n', 0],
      ['C2', heads.N1, 'pull_request', 'match=false\n', 0],
      ['C3', heads.P1, 'push', 'match=false\n', 0],
      ['E1', 'deadbeef', 'pull_request', '', 2],
      ['C4', heads.K4, 'pull_request', 'match=true\n', 0],
      ['E2', heads.B3, 'pull_request', '', 2],
    ];
    const total = cases.length + broken.length + 2 + cliCases.length;
    for (const cliCase of cliCases) runCliCase(repo, env, base, cliCase);
    console.log(`ci-unix-rust-changes self-test: ${total} cases ok`);
  } finally { fs.rmSync(repo, { recursive: true, force: true }); }
}

try {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === '--self-test') selfTest();
  else {
    const options = {};
    for (let i = 0; i < args.length; i += 2) {
      if (!['--base', '--head', '--repo'].includes(args[i]) || !args[i + 1]
        || args[i + 1].startsWith('--') || options[args[i]]) throw new Error('invalid arguments');
      options[args[i]] = args[i + 1];
    }
    if (!options['--base'] || !options['--head'])
      throw new Error('usage: --base <rev> --head <rev> [--repo <dir>] | --self-test');
    const result = detector(options['--repo']).detect(
      options['--base'], options['--head'], process.env.GITHUB_EVENT_NAME);
    console.log(`unix-rust-changes: match=${result.match}`
      + (result.event ? ` (event ${result.event})` : ''));
    for (const reason of result.reasons) console.log(`  ${reason}`);
    if (process.env.GITHUB_OUTPUT)
      fs.appendFileSync(process.env.GITHUB_OUTPUT, `match=${result.match}\n`);
  }
} catch (error) {
  console.error(`unix-rust-changes: ${error.message}`);
  process.exitCode = process.argv.includes('--self-test') ? 1 : 2;
}
