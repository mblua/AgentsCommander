#!/usr/bin/env node
// #3528: conserved native lint/platform capacity; locations are capture-local only.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { normalizeFile } from './check-too-many-lines.mjs';
import { createSnapshot, isMain, gitRunner } from './sanitizing_baseline.mjs';

export const CODES = ['doc_markdown', 'implicit_clone', 'map_unwrap_or', 'match_same_arms',
  'semicolon_if_nothing_returned', 'unnested_or_patterns', 'map_err_ignore']
  .map(c => `clippy::${c}`);
const HOSTS = ['linux', 'windows'];
const POLICY = 'lint-platform-total-v1';
const SHA = /^[a-f0-9]{40}$/;
const HASH = /^[a-f0-9]{64}$/;
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BASELINE = 'clippy-seven.baseline.json';
const TEMP = '.github/workflows/clippy-seven-measurement.yml';
const OWNER = new Set([BASELINE, 'scripts/check-clippy-seven.mjs',
  'scripts/check-clippy-seven.test.mjs', 'package.json',
  '.github/workflows/pr-regression-gates.yml',
  'docs/quality/clippy-seven-gate.md']);
const json = value => JSON.stringify(value, null, 2) + '\n';
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const order = (a, b) => {
  if (a < b) return -1;
  return a > b ? 1 : 0;
};
export class InputError extends Error { constructor(m) { super(m); this.exit = 2; } }
const need = (ok, m) => { if (!ok) throw new InputError(m); };
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const keys = (v, expected) => need(v && typeof v === 'object' && !Array.isArray(v) &&
  same(Object.keys(v), expected), `invalid fields/order: ${expected.join(',')}`);

// JSON.parse alone accepts duplicate keys. Validate object member uniqueness before parsing.
export function parseJSON(text) {
  let i = 0;
  const ws = () => { while (/\s/.test(text[i] ?? '') && i < text.length) i++; };
  const string = () => {
    const start = i++;
    while (i < text.length) {
      if (text[i] === '\\') { i += 2; continue; }
      if (text[i++] === '"') return JSON.parse(text.slice(start, i));
    }
    throw new InputError('unterminated JSON string');
  };
  const value = () => {
    ws(); const ch = text[i];
    if (ch === '"') { string(); return; }
    if (ch === '{' || ch === '[') {
      i++; ws(); const names = new Set(), close = ch === '{' ? '}' : ']';
      if (text[i] === close) { i++; return; }
      for (;;) {
        ws();
        if (ch === '{') {
          need(text[i] === '"', 'invalid JSON object key');
          const name = string(); need(!names.has(name), `duplicate JSON key ${name}`);
          names.add(name); ws(); need(text[i++] === ':', 'missing JSON colon');
        }
        value(); ws();
        if (text[i] === close) { i++; return; }
        need(text[i++] === ',', 'missing JSON delimiter');
      }
    }
    const start = i;
    while (i < text.length && !/[\s,}\]]/.test(text[i])) i++;
    need(i > start, 'missing JSON value');
  };
  try { value(); ws(); need(i === text.length, 'trailing JSON content'); return JSON.parse(text); }
  catch (e) {
    if (e instanceof InputError) throw e;
    throw new InputError('invalid JSON');
  }
}
const document = file => parseJSON(fs.readFileSync(file, 'utf8'));
function git(root, args) {
  const r = gitRunner(args, { cwd: root });
  need(!r.error && r.status === 0, `git ${args[0]} failed: ${r.stderr?.toString()}`);
  return r.stdout.toString('utf8').trim();
}
const tree = (root, sha) => git(root, ['rev-parse', `${sha}^{tree}`]);
function trackedSource(root, sha) {
  const files = new Set(git(root, ['ls-tree', '-r', '--name-only', sha]).split('\n'));
  const cache = new Map();
  return file => {
    need(files.has(file) && file.endsWith('.rs'), `untracked/generated source ${file}`);
    if (!cache.has(file)) {
      const r = gitRunner(['show', `${sha}:${file}`], { cwd: root });
      need(!r.error && r.status === 0, `source unavailable ${file}`);
      let text;
      try { text = new TextDecoder('utf-8', { fatal: true }).decode(r.stdout); }
      catch { throw new InputError(`source not UTF-8 ${file}`); }
      cache.set(file, text);
    }
    return cache.get(file);
  };
}
function byteAt(source, line, column) {
  const lines = source.split('\n');
  need(Number.isSafeInteger(line) && line > 0 && line <= lines.length &&
    Number.isSafeInteger(column) && column > 0 && column <= [...lines[line - 1]].length + 1,
  'invalid Unicode source coordinate');
  return Buffer.byteLength(lines.slice(0, line - 1).join('\n')) + (line > 1 ? 1 : 0) +
    Buffer.byteLength([...lines[line - 1]].slice(0, column - 1).join(''));
}
const spanOrder = (a, b) => order(a.file, b.file) || a.byte_start - b.byte_start ||
  a.byte_end - b.byte_end;
const locationKey = l => JSON.stringify([l.code, l.spans]);
const locationOrder = (a, b) => order(locationKey(a), locationKey(b));
const production = file => !file.split('/').some(p => ['tests', 'benches', 'tests.rs'].includes(p));

export function parseCapture(text, exit, { root, readSource }) {
  need(exit === 0 && typeof text === 'string' && text.trim(), 'empty/failed Cargo capture');
  const lines = text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n');
  const records = lines.map(parseJSON);
  need(records.every(r => r && typeof r.reason === 'string'), 'invalid Cargo record');
  const end = records.filter(r => r.reason === 'build-finished');
  need(end.length === 1 && end[0] === records.at(-1) && end[0].success === true,
    'capture needs exactly one final successful build-finished');
  const found = new Map();
  for (const r of records.filter(r => r.reason === 'compiler-message')) {
    const m = r.message;
    need(m && typeof m.level === 'string' && !['error', 'failure-note'].includes(m.level),
      'error/invalid diagnostic');
    if (!CODES.includes(m.code?.code)) continue;
    need(Array.isArray(m.spans), 'missing diagnostic spans');
    const primaries = m.spans.filter(s => s.is_primary === true);
    need(primaries.length > 0, 'empty primary vector');
    const spans = primaries.map(s => {
      need(!s.expansion, 'primary expansion unsupported');
      const file = normalizeFile(s.file_name, root), source = readSource(file);
      need(file.endsWith('.rs'), 'not Rust source');
      const start = byteAt(source, s.line_start, s.column_start);
      const finish = byteAt(source, s.line_end, s.column_end);
      need(Number.isSafeInteger(s.byte_start) && Number.isSafeInteger(s.byte_end) &&
        s.byte_start === start && s.byte_end === finish && finish > start &&
        finish <= Buffer.byteLength(source), `conflicting/invalid span in ${file}`);
      return { file, byte_start: start, byte_end: finish };
    }).sort(spanOrder).filter((s, i, all) => !i || !same(s, all[i - 1]));
    const l = { code: m.code.code, spans, file: spans[0].file,
      production: production(spans[0].file) };
    found.set(locationKey(l), l);
  }
  return [...found.values()].sort(locationOrder);
}
export function counts(locations) {
  return Object.fromEntries(CODES.map(c => [c, locations.filter(l => l.code === c).length]));
}
const META = ['version', 'issue', 'sourceReference', 'referenceTree', 'sourceCommit', 'sourceTree',
  'toolchain', 'platforms', 'codes', 'policy'];
function metadata(v) {
  need(v.version === 1 && v.issue === 3528 && v.toolchain === '1.98.1' &&
    v.policy === POLICY && same(v.platforms, HOSTS) && same(v.codes, CODES) &&
    ['sourceReference', 'referenceTree', 'sourceCommit', 'sourceTree'].every(k => SHA.test(v[k])),
  'invalid baseline metadata');
}
export function validateBaseline(v) {
  keys(v, [...META, 'limits']); metadata(v); keys(v.limits, HOSTS);
  for (const p of HOSTS) {
    keys(v.limits[p], CODES);
    need(CODES.every(c => Number.isSafeInteger(v.limits[p][c]) && v.limits[p][c] >= 0),
      'invalid capacity');
  }
  return v;
}
function observation(v) {
  const base = ['version', 'sourceCommit', 'sourceTree', 'platform', 'toolchain', 'policy',
    'exit', 'totals', 'locations'];
  keys(v, v.provenance ? [...base, 'provenance'] : base);
  need(v.version === 1 && SHA.test(v.sourceCommit) && SHA.test(v.sourceTree) &&
    HOSTS.includes(v.platform) && v.toolchain === '1.98.1' && v.policy === POLICY && v.exit === 0,
  'invalid observation metadata');
  need(Array.isArray(v.locations), 'invalid locations'); keys(v.totals, CODES);
  let previous;
  for (const l of v.locations) {
    keys(l, ['code', 'spans', 'file', 'production']);
    need(CODES.includes(l.code) && Array.isArray(l.spans) && l.spans.length > 0,
      'invalid location');
    for (const [i, s] of l.spans.entries()) {
      keys(s, ['file', 'byte_start', 'byte_end']);
      need(normalizeFile(s.file, '/') === s.file && !s.file.startsWith('/') &&
        s.file.endsWith('.rs') && Number.isSafeInteger(s.byte_start) && s.byte_start >= 0 &&
        Number.isSafeInteger(s.byte_end) && s.byte_end > s.byte_start &&
        (!i || spanOrder(l.spans[i - 1], s) < 0), 'invalid span vector');
    }
    need(l.file === l.spans[0].file && l.production === production(l.file) &&
      (!previous || locationOrder(previous, l) < 0), 'invalid location order/classification');
    previous = l;
  }
  need(same(counts(v.locations), v.totals), 'observation totals disagree with locations');
  if (v.provenance) {
    keys(v.provenance, ['sourceReference', 'referenceTree', 'rawHash', 'manifestHash',
      'captureRoot']);
    need(SHA.test(v.provenance.sourceReference) && SHA.test(v.provenance.referenceTree) &&
      HASH.test(v.provenance.rawHash) && HASH.test(v.provenance.manifestHash) &&
      typeof v.provenance.captureRoot === 'string' && v.provenance.captureRoot.length > 0,
    'invalid adoption provenance');
  }
  return v;
}
export function compare(obs, baseline) {
  observation(obs); validateBaseline(baseline);
  const results = CODES.map(code => {
    const actual = obs.totals[code], limit = baseline.limits[obs.platform][code];
    return { code, platform: obs.platform, actual, limit, excess: Math.max(0, actual - limit),
      slack: Math.max(0, limit - actual), status: actual > limit ? 'GROWN_TOTAL' : 'WITHIN_TOTAL',
      locations: obs.locations.filter(l => l.code === code) };
  });
  return { sourceCommit: obs.sourceCommit, sourceTree: obs.sourceTree,
    platform: obs.platform, results, exit: results.some(r => r.excess) ? 1 : 0 };
}
export function ratchet(head, base) {
  validateBaseline(head); validateBaseline(base);
  if (!META.every(k => same(head[k], base[k])) ||
      HOSTS.some(p => CODES.some(c => head.limits[p][c] > base.limits[p][c]))) {
    const e = new Error('baseline metadata changed or lint/platform capacity increased');
    e.exit = 1; throw e;
  }
}
function historical(root, b) {
  metadata(b);
  need(tree(root, b.sourceReference) === b.referenceTree &&
    tree(root, b.sourceCommit) === b.sourceTree, 'historical source tree mismatch');
  git(root, ['merge-base', '--is-ancestor', b.sourceReference, 'origin/main']);
  git(root, ['merge-base', '--is-ancestor', b.sourceCommit, 'HEAD']);
  git(root, ['diff', '--exit-code', b.sourceReference, b.sourceCommit, '--', '.',
    `:(exclude)${TEMP}`]);
}
export function baselineDiff(ref, root = ROOT) {
  const s = createSnapshot({ baseRef: ref, cwd: root });
  need(s.headTree.has(BASELINE), 'baseline deleted/missing from committed HEAD');
  const b = validateBaseline(parseJSON(s.readBlob(s.headTree.get(BASELINE))));
  if (s.baseTree.has(BASELINE)) {
    ratchet(b, parseJSON(s.readBlob(s.baseTree.get(BASELINE))));
  } else {
    need(['origin/main', 'refs/remotes/origin/main'].includes(ref), 'adoption needs origin/main');
    need(!git(root, ['ls-tree', 'origin/main', '--', BASELINE]), 'main already adopted baseline');
    historical(root, b);
    need(s.changed.every(c => OWNER.has(c.path)), 'unapproved initial authored scope');
  }
  s.assertStableHead();
}
export function mergeObservations(values) {
  need(values.length === 2, 'merge needs two hosts'); values.forEach(observation);
  const sorted = [...values].sort((a, b) => order(a.platform, b.platform));
  need(same(sorted.map(v => v.platform), HOSTS) && sorted.every(v => v.provenance),
    'merge needs distinct historical native hosts');
  const a = sorted[0], b = sorted[1];
  need(['sourceCommit', 'sourceTree', 'toolchain', 'policy'].every(k => same(a[k], b[k])) &&
    ['sourceReference', 'referenceTree'].every(k => same(a.provenance[k], b.provenance[k])),
  'inconsistent native source identities');
  return validateBaseline({ version: 1, issue: 3528,
    sourceReference: a.provenance.sourceReference, referenceTree: a.provenance.referenceTree,
    sourceCommit: a.sourceCommit, sourceTree: a.sourceTree, toolchain: a.toolchain,
    platforms: HOSTS, codes: CODES, policy: POLICY,
    limits: Object.fromEntries(sorted.map(v => [v.platform, v.totals])) });
}
function environment(root, output) {
  const names = ['CLIPPY_ARGS', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
    'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTC',
    'CARGO_BUILD_RUSTC_WRAPPER', 'CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER', 'CARGO_MANIFEST_DIR',
    'CLIPPY_CONF_DIR', ...Object.keys(process.env)
      .filter(k => /^CARGO_TARGET_.*RUSTFLAGS$/.test(k))];
  const values = Object.fromEntries(names.map(k => [k, process.env[k] ?? null]));
  const configs = git(root, ['ls-files']).split('\n').filter(f =>
    /(^|\/)\.cargo\/config(?:\.toml)?$/.test(f)).map(file => {
    const bytes = fs.readFileSync(path.join(root, file));
    return { file, sha256: hash(bytes), text: bytes.toString('utf8') };
  });
  fs.writeFileSync(output, json({ environment: values, configs,
    externalConfiguration: 'standard CI runner; no external config attestation' }));
  for (const c of configs) {
    need(!/\[env[.\]]|(?:rustflags|rustc(?:-wrapper|-workspace-wrapper)?)\s*=/
      .test(c.text), `unexplained Cargo configuration ${c.file}`);
  }
  need(values.CLIPPY_CONF_DIR === null, 'CLIPPY_CONF_DIR must be absent');
  for (const [key, value] of Object.entries(values)) {
    if (!value) continue;
    if (/FLAGS|CLIPPY_ARGS/.test(key)) {
      need(!/cap-lints|clippy|(?:^|\s)-[ADFW]|--(?:allow|warn|deny|forbid)/
        .test(value.replaceAll(String.fromCodePoint(31), ' ')), `unexplained lint override ${key}`);
    } else if (/WRAPPER/.test(key)) {
      need(/^sccache(?:\.exe)?$/.test(path.basename(value)), `unknown wrapper ${key}`);
    } else if (key.endsWith('RUSTC')) {
      need(/^rustc(?:\.exe)?$/.test(path.basename(value)), `unknown compiler ${key}`);
    } else if (key === 'CARGO_MANIFEST_DIR') {
      need(normalizeFile(path.resolve(value), root) !== '', 'unknown Cargo selector');
    }
  }
  return values;
}
function adoptionReceipt(o, raw, sourceTree) {
  const filename = path.join(path.dirname(o.adopt), 'manifest.json');
  const bytes = fs.readFileSync(filename), m = parseJSON(bytes.toString('utf8'));
  need((m.sourceCommit ?? m.actualSourceR) === o.commit &&
    (m.sourceTree ?? m.actualTree) === sourceTree &&
    (m.sourceReference ?? m.referenceQ) === o['source-ref'], 'manifest source binding mismatch');
  const canonicalRoot = value => value.replaceAll('\\', '/')
    .replace(/^\/([a-zA-Z])\//, (_, drive) => drive.toUpperCase() + ':/')
    .replace(/^([a-z]):/, (_, drive) => drive.toUpperCase() + ':');
  const cwd = (m.cwd ?? m.commands?.all?.cwd ?? '').trim();
  need(cwd.endsWith('/src-tauri') && canonicalRoot(cwd.slice(0, -10)) ===
    canonicalRoot(o['capture-root'].replace(/\/$/, '')), 'original capture root mismatch');
  need((m.platform ?? (m.host?.startsWith('Windows') ? 'windows' : null)) === o.platform &&
    Number(m.commands?.all?.exit) === 0, 'manifest platform/exit mismatch');
  const inventory = m.files;
  need(Array.isArray(inventory), 'manifest needs file inventory');
  const f = inventory.find(v => v.name === 'all.jsonl');
  need(f && f.sha256.toLowerCase() === hash(raw) && (f.bytes ?? f.size) === raw.length,
    'manifest raw capture mismatch');
  const savedExit = fs.readFileSync(path.join(path.dirname(o.adopt), 'all.exit'), 'utf8');
  need(savedExit.trim() === '0',
    'saved original exit mismatch');
  return { sourceReference: o['source-ref'], referenceTree: m.referenceTree,
    rawHash: hash(raw), manifestHash: hash(bytes), captureRoot: o['capture-root'] };
}
function capture(o, historicalInput) {
  const root = path.resolve(o['workspace-root']);
  need(SHA.test(o.commit) && HOSTS.includes(o.platform) && o['rustc-version'] === '1.98.1' &&
    /^(0|[1-9]\d*)$/.test(o['clippy-exit']), 'invalid capture arguments');
  if (!historicalInput) {
    need(git(root, ['rev-parse', 'HEAD']) === o.commit, 'checkout SHA mismatch');
  }
  if (!historicalInput) environment(root, o.emit + '.controls.json');
  const sourceTree = tree(root, o.commit), raw = fs.readFileSync(o.adopt ?? o.capture);
  const locations = parseCapture(raw.toString('utf8'), +o['clippy-exit'], {
    root: historicalInput ? o['capture-root'] : root, readSource: trackedSource(root, o.commit) });
  const obs = { version: 1, sourceCommit: o.commit, sourceTree, platform: o.platform,
    toolchain: '1.98.1', policy: POLICY, exit: 0, totals: counts(locations), locations };
  if (historicalInput) {
    obs.provenance = adoptionReceipt(o, raw, sourceTree);
    historical(root, { version: 1, issue: 3528, ...obs.provenance,
      sourceCommit: o.commit, sourceTree, toolchain: '1.98.1', platforms: HOSTS,
      codes: CODES, policy: POLICY });
  }
  return observation(obs);
}
export function selfTestResult(r) {
  need(!r.error && r.status === 0 && /^# tests [1-9]\d*$/m.test(r.stdout ?? ''),
    'self-test failed or executed zero tests');
  return 0;
}
export function workflowContract(text) {
  for (const host of HOSTS) {
    const job = host === 'windows' ? 'rust-regression-windows' : 'clippy-linux';
    const start = text.indexOf(`\n  ${job}:`);
    need(start >= 0, `missing native host ${host}`);
    const rest = text.slice(start + 1), next = rest.slice(1).search(/\n {2}[a-z][\w-]*:/);
    const body = next < 0 ? rest : rest.slice(0, next + 1);
    const captureStart = body.indexOf('      - name: cargo clippy\n');
    const gateStart = body.indexOf('      - name: Seven-lint gate\n');
    const nextStep = body.indexOf('\n      - name:', captureStart + 10);
    need(captureStart >= 0 && gateStart === nextStep + 1, 'gate must follow shared capture');
    const command = body.slice(captureStart, gateStart);
    for (const code of CODES) {
      need(command.split(`--force-warn ${code}`).length === 2, `missing/repeated flag ${code}`);
    }
    const gate = body.slice(gateStart, body.indexOf('\n      - name:', gateStart + 10));
    need(!gate.includes('continue-on-error:') && gate.includes('--capture ') &&
      gate.includes('--check ') && gate.includes(`--platform ${host}`), 'nonblocking/wrong gate');
    need(body.includes('name: Upload seven-lint evidence') &&
      body.includes('retention-days: 14') && body.includes('if-no-files-found: error'),
    'missing durable native evidence');
  }
  need(text.includes('npm run seven:self') && text.includes('npm run seven:baseline'),
    'missing self/ratchet CI');
}
export function parseArgs(argv) {
  const modes = ['self-test', 'capture', 'adopt', 'merge', 'check', 'baseline-diff'];
  const common = ['clippy-exit', 'platform', 'commit', 'workspace-root', 'rustc-version', 'emit'];
  const allowed = { 'self-test': [], 'baseline-diff': ['base-ref'], merge: ['out'],
    check: ['baseline', 'workspace-root'], capture: common,
    adopt: [...common, 'source-ref', 'capture-root'] };
  const o = {}, files = [];
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i].slice(2);
    need(argv[i].startsWith('--') && [...modes, ...Object.values(allowed).flat()].includes(k) &&
      o[k] === undefined, `unknown/duplicate argument ${argv[i]}`);
    if (['self-test', 'baseline-diff'].includes(k)) o[k] = true;
    else if (k === 'merge') {
      o[k] = true;
      while (i + 1 < argv.length && !argv[i + 1].startsWith('--')) files.push(argv[++i]);
    } else {
      need(i + 1 < argv.length && !argv[i + 1].startsWith('--'), `missing value ${k}`);
      o[k] = argv[++i];
    }
  }
  const selected = modes.filter(m => o[m] !== undefined);
  need(selected.length === 1, 'choose exactly one mode'); const mode = selected[0];
  need(Object.keys(o).every(k => k === mode || allowed[mode].includes(k)), 'out-of-mode flag');
  need(allowed[mode].filter(k => k !== 'base-ref').every(k => o[k] !== undefined),
    'missing required arguments');
  need(mode !== 'merge' || files.length === 2, 'merge needs exactly two observations');
  return { ...o, files };
}
export function main(argv) {
  try {
    const o = parseArgs(argv);
    if (o['self-test']) {
      const r = spawnSync(process.execPath, ['--test', '--test-reporter=tap',
        path.join(ROOT, 'scripts/check-clippy-seven.test.mjs')],
      { encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024 });
      process.stdout.write(r.stdout ?? ''); process.stderr.write(r.stderr ?? '');
      return selfTestResult(r);
    }
    if (o.capture || o.adopt) {
      fs.writeFileSync(o.emit, json(capture(o, Boolean(o.adopt)))); return 0;
    }
    if (o.merge) {
      fs.writeFileSync(o.out, json(mergeObservations(o.files.map(document)))); return 0;
    }
    if (o['baseline-diff']) {
      baselineDiff(o['base-ref'] ?? (process.env.GATE_BASE_REF?.trim() || 'origin/main'));
      console.log('seven-lint capacity ratchet: PASS'); return 0;
    }
    const obs = observation(document(o.check)), root = path.resolve(o['workspace-root']);
    need(git(root, ['rev-parse', 'HEAD']) === obs.sourceCommit &&
      tree(root, obs.sourceCommit) === obs.sourceTree, 'observation checkout binding mismatch');
    const readSource = trackedSource(root, obs.sourceCommit);
    for (const l of obs.locations) for (const s of l.spans) {
      const bytes = Buffer.from(readSource(s.file));
      need(s.byte_end <= bytes.length, 'observation span outside source');
      try { new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, s.byte_start));
        new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(s.byte_start, s.byte_end));
      } catch { throw new InputError('observation span splits UTF-8'); }
    }
    const result = compare(obs, document(o.baseline));
    fs.writeFileSync(o.check + '.summary.json', json(result));
    console.log(json(result.results.map(({ locations, ...r }) => ({ ...r,
      files: [...new Set(locations.flatMap(l => l.spans.map(s => s.file)))] }))));
    return result.exit;
  } catch (e) { console.error(e.message); return e.exit ?? 2; }
}
if (isMain(import.meta.url)) process.exitCode = main(process.argv.slice(2));
