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
const ROOTS = new Set(['lib.rs', 'main.rs', 'build.rs']);

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
  function reasonFor(file, base, head) {
    if (file === DETECTOR) return 'detector';
    if (file === WORKFLOW) return 'workflow';
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
    const changed = git(['diff', '--no-renames', '--name-only', '-z', base, head])
      .split('\0').filter(Boolean);
    const reasons = [];
    for (const file of changed) {
      const reason = reasonFor(file, base, head);
      if (reason) reasons.push(`${file}: ${reason}`);
    }
    return { match: reasons.length > 0, reasons };
  }
  return { git, detect };
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
      for (const [file, text] of Object.entries(changes)) {
        if (text === null) fs.unlinkSync(path.join(repo, file));
        else write(file, text);
      }
      commit();
      heads[id] = git(['rev-parse', 'HEAD']).trim();
      assert.deepEqual(detect(base, heads[id], 'pull_request'),
        { match: reasons.length > 0, reasons }, id);
    }
    const cliCases = [
      ['C1', heads.P1, 'pull_request', 'match=true\n', 0],
      ['C2', heads.N1, 'pull_request', 'match=false\n', 0],
      ['C3', heads.P1, 'push', 'match=false\n', 0],
      ['E1', 'deadbeef', 'pull_request', '', 2],
    ];
    for (const [id, head, event, expected, status] of cliCases) {
      const output = path.join(repo, `${id}.output`);
      fs.writeFileSync(output, '');
      let code = 0, stdout = '';
      try {
        stdout = execFileSync(process.execPath, [fileURLToPath(import.meta.url),
          '--base', 'HEAD^1', '--head', head, '--repo', repo], {
          env: { ...env, GITHUB_EVENT_NAME: event, GITHUB_OUTPUT: output },
          encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
        });
      } catch (error) { code = error.status; }
      assert.equal(code, status, id);
      assert.equal(fs.readFileSync(output, 'utf8'), expected, id);
      if (id === 'C3') assert.ok(stdout.includes('(event push)'), id);
    }
    console.log('ci-unix-rust-changes self-test: 32 cases ok');
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
