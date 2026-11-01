#!/usr/bin/env node
// #3507: measured Clippy debt, committed ceilings, and conservative relocation.
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { idFor, anchorFor, lexFile, scanSources } from './check-cognitive-complexity.mjs';
import { isMain, createSnapshot, gitRunner } from './sanitizing_baseline.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BASELINE = 'too-many-lines.baseline.json';
const PLATFORMS = ['linux', 'macos', 'windows'];
const SHA = /^[a-f0-9]{40}$/;
const NAME = /^(?:r#)?[\p{ID_Start}_][\p{ID_Continue}]*$/u;
const key = e => `${e.id}\0${e.anchor}`;
const pair = e => `${e.name}\0${e.anchor}`;
const order = (a, b) => key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0;
const json = value => `${JSON.stringify(value, null, 2)}\n`;
export class InputError extends Error { constructor(message) { super(message); this.exit = 2; } }
const requireInput = (ok, message) => { if (!ok) throw new InputError(message); };
const read = file => fs.readFileSync(file, 'utf8');
const document = file => JSON.parse(read(file));
function git(root, args) {
  let r;
  try { r = gitRunner(args, { cwd: root }); }
  catch (error) { throw new InputError(`git ${args.join(' ')}: ${error.message}`); }
  requireInput(!r.error && r.status === 0,
    `git ${args.join(' ')}: ${r.stderr?.toString() || r.error}`);
  return r.stdout.toString('utf8').trim();
}

export function checkEnvironment(env = process.env) {
  requireInput(env.CLIPPY_CONF_DIR === undefined, 'CLIPPY_CONF_DIR must be absent');
  for (const name of ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS']) {
    const value = (env[name] ?? '').replaceAll(String.fromCharCode(31), ' ');
    const override = /cap-lints|(?:^|\s)-(?:A|W|D|F)|--(?:allow|warn|deny|forbid)/;
    requireInput(!override.test(value) && !/clippy|too.many.lines/i.test(value),
      `${name} contains a lint override`);
  }
}

export function parseCapture(text, exit, render = () => {}) {
  const records = [];
  requireInput(typeof text === 'string' && text.trim(), 'empty capture');
  for (const line of text.trimEnd().split('\n')) {
    let r;
    try { r = JSON.parse(line); } catch { throw new InputError('malformed JSONL capture'); }
    requireInput(r && typeof r.reason === 'string', 'invalid Cargo record');
    records.push(r);
    if (r.reason === 'compiler-message' && r.message?.code?.code !== 'clippy::too_many_lines')
      render(r.message?.rendered ?? r.message?.message ?? '');
  }
  requireInput(exit === 0, `Clippy exited ${exit}`);
  const finished = records.filter(r => r.reason === 'build-finished');
  requireInput(finished.length === 1 && records.at(-1) === finished[0] &&
    finished[0].success === true, 'capture needs one successful terminal build-finished');
  return records.filter(r => r.reason === 'compiler-message' &&
    r.message?.code?.code === 'clippy::too_many_lines').map(r => {
    const m = /^this function has too many lines \((\d+)\/(\d+)\)$/.exec(r.message.message);
    requireInput(m && m[2] === '100' && Number.isSafeInteger(+m[1]) && +m[1] > 100,
      `invalid length diagnostic: ${r.message.message}`);
    const spans = r.message.spans?.filter(s => s.is_primary === true);
    requireInput(spans?.length === 1, 'length diagnostic needs one primary span');
    const s = spans[0];
    requireInput(typeof s.file_name === 'string' && s.file_name.length > 0 &&
      ['line_start', 'line_end', 'column_start', 'column_end']
        .every(k => Number.isInteger(s[k]) && s[k] > 0) &&
      (s.line_end > s.line_start || s.line_end === s.line_start &&
        s.column_end > s.column_start), 'invalid primary span shape');
    return { span: spans[0], lines: +m[1] };
  });
}

export function normalizeFile(file, root) {
  requireInput(typeof file === 'string' && file && !file.includes('\0'), 'invalid source path');
  const slash = file.replaceAll('\\', '/');
  const base = root.replaceAll('\\', '/').replace(/\/$/, '');
  const absolute = slash.startsWith('/') || /^[A-Za-z]:\//.test(slash);
  const rel = absolute ? slash.startsWith(`${base}/`) ? slash.slice(base.length + 1) : null : slash;
  requireInput(rel !== null && !rel.startsWith('/') && !/^[A-Za-z]:/.test(rel),
    'path escapes root');
  const parts = [];
  for (const part of rel.split('/')) {
    if (part === '..') requireInput(parts.length > 0, 'path escapes root');
    if (part === '..') parts.pop();
    else if (part && part !== '.') parts.push(part);
  }
  requireInput(parts.length > 0, 'empty source path');
  return parts.join('/');
}
function offset(source, line, column) {
  const lines = source.split('\n');
  requireInput(Number.isInteger(line) && line > 0 && line <= lines.length &&
    Number.isInteger(column) && column > 0 && column <= [...lines[line - 1]].length + 1,
  `invalid span ${line}:${column}`);
  return lines.slice(0, line - 1).reduce((n, s) => n + s.length + 1, 0) +
    [...lines[line - 1]].slice(0, column - 1).join('').length;
}
function position(source, index) {
  const before = source.slice(0, index), line = before.split('\n').length;
  return [line, [...before.slice(before.lastIndexOf('\n') + 1)].length + 1];
}

export function observations(diagnostics, root, readSource) {
  const spans = new Map(), sites = new Map();
  for (const { span: s, lines } of diagnostics) {
    requireInput(!s.expansion, 'macro expansion identity is unsupported');
    const file = normalizeFile(s.file_name, root), source = readSource(file);
    const start = offset(source, s.line_start, s.column_start);
    const end = offset(source, s.line_end, s.column_end);
    requireInput(end > start, `invalid span in ${file}`);
    // Clippy 1.98.1 highlights the header from its qualifiers through the return type.
    const header = source.slice(start, end);
    const names = [...header.matchAll(/\bfn\s+((?:r#)?[\p{ID_Start}_][\p{ID_Continue}]*)/gu)];
    requireInput(names.length === 1, `ambiguous/unresolved function in ${file}:${s.line_start}`);
    const name = names[0][1], at = start + names[0].index + names[0][0].length - name.length;
    const [line, column] = position(source, at);
    const frames = lexFile(file, readSource);
    requireInput(frames.filter(f => f.kind === 'fn' && f.name === name &&
      f.open >= end && f.kwLine <= line).some(f => f.kwLine ===
        position(source, start + names[0].index)[0]), `unresolved declaration ${file}:${line}`);
    const entry = { id: idFor(file, line, column, name, readSource), name,
      anchor: anchorFor(file, line, column, readSource), lines, file, line, column };
    const spanKey = [file, start, end].join('\0');
    if (spans.has(spanKey)) {
      requireInput(spans.get(spanKey) === lines, `conflicting counts at ${file}:${line}`);
      continue;
    }
    spans.set(spanKey, lines);
    requireInput(!sites.has(key(entry)), `indistinguishable site ${entry.id}`);
    sites.set(key(entry), entry);
  }
  return [...sites.values()].sort(order);
}

export function validateEnvelope(d) {
  requireInput(d?.version === 1 && PLATFORMS.includes(d.platform) && SHA.test(d.sourceCommit) &&
    d.toolchain === '1.98.1' && typeof d.productionMeasured === 'boolean' &&
    Array.isArray(d.entries), 'invalid observation metadata');
  validateEntries(d.entries, false);
  for (const e of d.entries)
    requireInput(typeof e.production === 'boolean', 'invalid production membership');
  return d;
}
function validateEntries(entries, baseline) {
  let previous;
  for (const e of entries) {
    requireInput(e && typeof e.id === 'string' && e.id.startsWith('rust:') &&
      NAME.test(e.name ?? '') && e.id.endsWith(`::${e.name}`) &&
      /^[a-f0-9]{12}$/.test(e.anchor ?? ''), 'invalid identity/name/anchor');
    const file = e.id.slice(5).split('::')[0];
    requireInput(file.endsWith('.rs') && normalizeFile(file, '/') === file, 'invalid id path');
    requireInput(previous === undefined || order(previous, e) < 0, 'unsorted/duplicate site');
    previous = e;
    const values = baseline ? Object.entries(e.sites ?? {}) : [['observation', e]];
    requireInput(values.length > 0, 'empty sites');
    for (const [platform, v] of values) {
      requireInput(!baseline || PLATFORMS.includes(platform), 'unknown platform');
      requireInput(Number.isSafeInteger(v?.lines) && v.lines > 100, 'invalid length ceiling');
      if (baseline) requireInput(typeof v.production === 'boolean', 'invalid production flag');
    }
  }
}
export function validateBaseline(d) {
  requireInput(d?.version === 1 && d.issue === 3507 && d.threshold === 100 &&
    d.toolchain === '1.98.1' && SHA.test(d.sourceCommit) && Array.isArray(d.entries),
  'invalid baseline metadata');
  validateEntries(d.entries, true);
  return d;
}

export function merge(emissions) {
  requireInput(emissions.length === 3, 'merge needs exactly three platforms');
  const platforms = new Set(), entries = new Map();
  const ordered = [...emissions].sort((a, b) =>
    PLATFORMS.indexOf(a.platform) - PLATFORMS.indexOf(b.platform));
  for (const raw of ordered) {
    const d = validateEnvelope(raw);
    requireInput(d.productionMeasured, 'merge needs productionMeasured=true');
    requireInput(!platforms.has(d.platform), 'duplicate platform'); platforms.add(d.platform);
    requireInput(d.sourceCommit === emissions[0].sourceCommit, 'source SHA mismatch');
    for (const e of d.entries) {
      if (!entries.has(key(e))) entries.set(key(e),
        { id: e.id, name: e.name, anchor: e.anchor, sites: {} });
      entries.get(key(e)).sites[d.platform] = { lines: e.lines, production: e.production };
    }
  }
  requireInput(PLATFORMS.every(p => platforms.has(p)), 'missing platform');
  return validateBaseline({ version: 1, issue: 3507, threshold: 100, toolchain: '1.98.1',
    sourceCommit: emissions[0].sourceCommit, entries: [...entries.values()].sort(order) });
}
export function counts(baseline) {
  const result = {};
  for (const p of [...PLATFORMS, 'union']) {
    const sites = baseline.entries.filter(e => p === 'union' || e.sites[p]);
    const production = sites.filter(e => p === 'union' ?
      Object.values(e.sites).some(s => s.production) : e.sites[p].production).length;
    result[p] = { production, allTargets: sites.length, testOnly: sites.length - production };
  }
  return result;
}

// Tokenize only what the macro guard needs; strings/comments cannot name a declaration.
function macroTokens(source) {
  const tokens = [];
  for (let i = 0; i < source.length;) {
    const rest = source.slice(i);
    if (/^\s/.test(rest)) { i++; continue; }
    if (rest.startsWith('//')) {
      const end = source.indexOf('\n', i); i = end < 0 ? source.length : end; continue;
    }
    if (rest.startsWith('/*')) {
      let depth = 1; i += 2;
      while (i < source.length && depth) {
        if (source.startsWith('/*', i)) { depth++; i += 2; }
        else if (source.startsWith('*/', i)) { depth--; i += 2; }
        else i++;
      }
      requireInput(depth === 0, 'unterminated donor comment'); continue;
    }
    const raw = /^(?:br|cr|r)(#*)"/.exec(rest);
    if (raw) {
      const end = source.indexOf(`"${raw[1]}`, i + raw[0].length);
      requireInput(end >= 0, 'unterminated donor raw string');
      i = end + 1 + raw[1].length; continue;
    }
    const quoted = /^(?:b|c)?"/.exec(rest);
    if (quoted) {
      i += quoted[0].length; let closed = false;
      while (i < source.length) {
        if (source[i] === '\\') { i += 2; continue; }
        if (source[i++] === '"') { closed = true; break; }
      }
      requireInput(closed, 'unterminated donor string'); continue;
    }
    const character = /^(?:b)?'(?:\\(?:u\{[\da-fA-F_]+\}|.)|[^'\\\r\n])'/u.exec(rest);
    if (character) { i += character[0].length; continue; }
    const name = /^(?:r#)?[\p{ID_Start}_][\p{ID_Continue}]*/u.exec(rest);
    if (name) { tokens.push(name[0]); i += name[0].length; }
    else tokens.push(source[i++]);
  }
  return tokens;
}
function macroMayCarry(source, name) {
  const tokens = macroTokens(source), matching = { '(': ')', '[': ']', '{': '}' };
  for (let i = 1; i < tokens.length; i++) {
    if (tokens[i] !== '!' || !NAME.test(tokens[i - 1])) continue;
    const macro = tokens[i - 1];
    let start = i + 1;
    if (macro === 'macro_rules' && NAME.test(tokens[start] ?? '')) start++;
    if (!matching[tokens[start]]) continue;
    const stack = [matching[tokens[start]]];
    for (let j = start + 1; j < tokens.length; j++) {
      const token = tokens[j];
      if (token.replace(/^r#/, '') === name.replace(/^r#/, '') ||
        /paste|concat_idents/.test(macro) || token === '[' && tokens[j + 1] === '<') return true;
      if (matching[token]) stack.push(matching[token]);
      else if ([')', ']', '}'].includes(token)) {
        requireInput(stack.pop() === token, 'unbalanced donor macro');
        if (!stack.length) break;
      }
      requireInput(j + 1 < tokens.length || !stack.length, 'unterminated donor macro');
    }
  }
  return false;
}
// Only ENOENT proves deletion. Inactive cfg declarations also reserve identity.
export function originalAbsent(entry, { readSource, stat = fs.statSync, root = ROOT }) {
  const file = entry.id.slice(5).split('::')[0];
  try { stat(path.join(root, file)); }
  catch (e) { if (e.code === 'ENOENT') return true; throw e; }
  requireInput(!macroMayCarry(readSource(file), entry.name),
    `macro donor identity unsupported in ${file}`);
  const frames = lexFile(file, readSource);
  for (const frame of frames.filter(f => f.kind === 'fn')) {
    requireInput(frame.name && NAME.test(frame.name), `unsupported donor declaration ${file}`);
    const enclosing = frames.filter(f => f.open < frame.open && frame.close < f.close)
      .map(f => f.token);
    const id = `rust:${file}::${[...enclosing, frame.name].join('::')}`;
    if (id === entry.id) return false;
  }
  return true;
}
export function compare(observed, baseline, absent) {
  validateEnvelope(observed); validateBaseline(baseline);
  const p = observed.platform;
  const base = baseline.entries.filter(e => e.sites[p]);
  const exact = new Map(base.map(e => [key(e), e])), used = new Set(), result = [];
  const unmatched = [];
  for (const e of observed.entries) {
    const donor = exact.get(key(e));
    if (!donor) { unmatched.push(e); continue; }
    used.add(key(donor));
    const ceiling = donor.sites[p].lines;
    result.push({ ...e, status: e.lines > ceiling ? 'GROWN' : 'EXACT', ceiling });
  }
  for (const e of unmatched) {
    const donors = base.filter(b => pair(b) === pair(e));
    const receivers = unmatched.filter(h => pair(h) === pair(e));
    if (donors.length > 1 || receivers.length > 1) {
      result.push({ ...e, status: 'AMBIGUOUS', ceiling: null }); continue;
    }
    const donor = donors[0];
    if (!donor || used.has(key(donor)) || !absent(donor)) {
      result.push({ ...e, status: 'NEW', ceiling: null }); continue;
    }
    used.add(key(donor));
    const ceiling = donor.sites[p].lines;
    result.push({ ...e, status: e.lines <= ceiling ? 'MOVED' : 'GROWN',
      donor: donor.id, ceiling });
  }
  for (const e of base) if (!used.has(key(e)))
    result.push({ ...e, status: 'ABSENT', ceiling: e.sites[p].lines });
  return { platform: p, sourceCommit: observed.sourceCommit, results: result,
    exit: result.some(e => ['NEW', 'GROWN', 'AMBIGUOUS'].includes(e.status)) ? 1 : 0 };
}
export function ratchet(head, base) {
  validateBaseline(head); validateBaseline(base);
  const rule = (ok, message) => {
    if (!ok) { const error = new Error(message); error.exit = 1; throw error; }
  };
  rule(['version', 'issue', 'threshold', 'toolchain', 'sourceCommit']
    .every(k => head[k] === base[k]), 'baseline metadata changed');
  const old = new Map(base.entries.map(e => [key(e), e]));
  for (const e of head.entries) for (const [p, s] of Object.entries(e.sites)) {
    const b = old.get(key(e));
    rule(b?.name === e.name && b.sites[p] && s.lines <= b.sites[p].lines &&
      s.production === b.sites[p].production, `baseline grew/swapped ${e.id} (${p})`);
  }
}

function protectedPath(file) {
  return /\.rs$|(^|\/)Cargo\.(toml|lock)$|(^|\/)\.cargo\/|clippy|rust-toolchain/.test(file) ||
    /^\.github\/workflows\/|^scripts\/ci-install/.test(file);
}
function allowedCandidate(file) {
  return ['clippy.toml', '.github/workflows/pr-regression-gates.yml',
    '.github/workflows/cache-warm.yml'].includes(file);
}
export function baselineDiff(baseRef, root = ROOT) {
  const s = createSnapshot({ baseRef, cwd: root });
  requireInput(s.headTree.has(BASELINE), `${BASELINE} missing from HEAD; finish native adoption`);
  const head = validateBaseline(JSON.parse(s.readBlob(s.headTree.get(BASELINE))));
  if (s.baseTree.has(BASELINE)) ratchet(head, JSON.parse(s.readBlob(s.baseTree.get(BASELINE))));
  else {
    requireInput(['origin/main', 'refs/remotes/origin/main'].includes(baseRef),
      'initial adoption needs origin/main');
    const remote = git(root, ['rev-parse', 'refs/remotes/origin/main']);
    requireInput(remote === s.refOid && !git(root, ['ls-tree', remote, '--', BASELINE]),
      'initial adoption requires absent baseline on resolved main');
    git(root, ['merge-base', '--is-ancestor', head.sourceCommit, remote]);
    const drift = git(root, ['diff', '--no-renames', '--name-only',
      head.sourceCommit, remote, '--'])
      .split('\n').filter(Boolean);
    requireInput(!drift.some(protectedPath), 'relevant main drift: re-adopt at current main');
    const candidate = git(root, ['diff', '--no-renames', '--name-only',
      head.sourceCommit, s.headOid, '--'])
      .split('\n').filter(Boolean);
    requireInput(!candidate.some(f => protectedPath(f) && !allowedCandidate(f)),
      'candidate protected sources differ from adoption source');
  }
  s.assertStableHead();
}

function sourceFiles(root) {
  const files = [];
  function walk(dir) {
    for (const e of fs.readdirSync(path.join(root, dir), { withFileTypes: true })) {
      if (['.git', 'node_modules', 'target', 'dist', 'artifacts'].includes(e.name)) continue;
      const f = dir ? `${dir}/${e.name}` : e.name;
      requireInput(!e.isSymbolicLink(), `unsupported symbolic link ${f}`);
      if (e.isDirectory()) walk(f); else if (e.isFile()) files.push(f);
    }
  }
  walk(''); return files;
}
function validateConfig(root, adoption) {
  const files = sourceFiles(root), readSource = f => read(path.join(root, f));
  const readBytes = f => fs.readFileSync(path.join(root, f));
  const hits = scanSources({ files, readSource, readBytes });
  requireInput(hits.length === 0, `inherited config/suppression: ${json(hits)}`);
  const config = readSource('clippy.toml').split('\n').filter(l => l && !l.startsWith('#'));
  requireInput(adoption || config.includes('too-many-lines-threshold = 100'),
    'candidate requires explicit length100 pin');
}
export function capture(options, root = options['workspace-root']) {
  checkEnvironment();
  for (const flag of ['clippy-exit', 'production-clippy-exit', 'denominator-clippy-exit']) {
    if (options[flag] === undefined) continue;
    requireInput(/^\d+$/.test(options[flag]) && Number.isSafeInteger(+options[flag]),
      `${flag} must be a nonnegative decimal exit code`);
  }
  requireInput(root && SHA.test(options.commit) && PLATFORMS.includes(options.platform) &&
    options['rustc-version'] === '1.98.1', 'capture needs explicit root/SHA/platform/pinned rustc');
  requireInput(git(root, ['rev-parse', 'HEAD']) === options.commit, 'measured HEAD mismatch');
  const adoptionFlags = ['production-capture', 'production-clippy-exit', 'adoption-source',
    'denominator-proof', 'denominator-clippy-exit'];
  const supplied = adoptionFlags.filter(f => options[f] !== undefined);
  requireInput(supplied.length === 0 || supplied.length === 5, 'all five adoption flags required');
  const adoption = supplied.length === 5;
  if (adoption) {
    requireInput(options['adoption-source'] === options.commit, 'adoption SHA mismatch');
    requireInput(git(root, ['status', '--porcelain', '--untracked-files=no']) === '',
      'adoption measured sources must be clean');
    const proof = parseCapture(read(options['denominator-proof']),
      +options['denominator-clippy-exit'], console.error);
    requireInput(proof.length > 0, 'denominator proof has no actual length diagnostic');
  }
  validateConfig(root, adoption);
  const readSource = f => read(path.join(root, f));
  const entries = observations(parseCapture(read(options.capture), +options['clippy-exit'],
    console.error), root, readSource);
  const production = adoption ? observations(parseCapture(read(options['production-capture']),
    +options['production-clippy-exit'], console.error), root, readSource) : [];
  const prod = new Map(production.map(e => [key(e), e]));
  requireInput(production.every(e => entries.some(a => key(a) === key(e))),
    'production capture is inconsistent with all-target capture');
  return validateEnvelope({ version: 1, platform: options.platform,
    sourceCommit: options.commit, toolchain: options['rustc-version'], productionMeasured: adoption,
    entries: entries.map(e => ({ ...e, production: prod.has(key(e)) })) });
}

export function selfTestResult(r) {
  requireInput(!r.error && r.status === 0 && /^# tests [1-9]\d*$/m.test(r.stdout ?? ''),
    'self-test child failed or executed zero tests');
  return 0;
}
function selfTest() {
  const r = spawnSync(process.execPath, ['--test', '--test-reporter=tap',
    path.join(ROOT, 'scripts/check-too-many-lines.test.mjs')],
  { encoding: 'utf8', windowsHide: true, maxBuffer: 16 * 1024 * 1024 });
  process.stdout.write(r.stdout ?? ''); process.stderr.write(r.stderr ?? '');
  return selfTestResult(r);
}

export function parseArgs(argv) {
  const modes = ['self-test', 'capture', 'check', 'merge', 'baseline-diff'];
  const values = ['clippy-exit', 'platform', 'rustc-version', 'commit', 'workspace-root', 'emit',
    'production-capture', 'production-clippy-exit', 'adoption-source', 'denominator-proof',
    'denominator-clippy-exit', 'baseline', 'out', 'base-ref'];
  const result = {}, files = [];
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i].slice(2);
    requireInput(argv[i].startsWith('--') && [...modes, ...values].includes(flag) &&
      result[flag] === undefined, `invalid/duplicate flag ${argv[i]}`);
    if (['self-test', 'baseline-diff'].includes(flag)) result[flag] = true;
    else if (flag === 'merge') {
      result.merge = true;
      while (i + 1 < argv.length && !argv[i + 1].startsWith('--')) files.push(argv[++i]);
    } else {
      requireInput(i + 1 < argv.length && !argv[i + 1].startsWith('--'), `missing ${flag}`);
      result[flag] = argv[++i];
    }
  }
  requireInput(modes.filter(m => result[m] !== undefined).length === 1, 'choose exactly one mode');
  const mode = modes.find(m => result[m] !== undefined);
  const allowed = { 'self-test': [], 'baseline-diff': ['base-ref'], merge: ['out'],
    check: ['baseline', 'workspace-root'], capture: values.filter(v =>
      !['baseline', 'out', 'base-ref'].includes(v)) };
  requireInput(Object.keys(result).every(k => k === mode || allowed[mode].includes(k)),
    'flag outside selected mode');
  if (mode === 'capture') requireInput(['clippy-exit', 'platform', 'rustc-version', 'commit',
    'workspace-root', 'emit'].every(f => result[f] !== undefined), 'missing capture arguments');
  if (mode === 'check') requireInput(result.baseline, 'check needs baseline');
  if (mode === 'merge') requireInput(result.out && files.length === 3, 'merge needs 3 files/out');
  return { ...result, files };
}
export function main(argv) {
  try {
    const o = parseArgs(argv);
    if (o['self-test']) return selfTest();
    if (o.capture) { fs.writeFileSync(o.emit, json(capture(o))); return 0; }
    if (o.merge) {
      const baseline = merge(o.files.map(document)); fs.writeFileSync(o.out, json(baseline));
      console.log(json(counts(baseline))); return 0;
    }
    if (o['baseline-diff']) {
      baselineDiff(o['base-ref'] ?? (process.env.GATE_BASE_REF?.trim() || 'origin/main'));
      console.log('too-many-lines baseline: PASS'); return 0;
    }
    const observed = validateEnvelope(document(o.check)), root = o['workspace-root'] ?? ROOT;
    requireInput(git(root, ['rev-parse', 'HEAD']) === observed.sourceCommit,
      'observation SHA is not measured checkout HEAD');
    const readSource = f => read(path.join(root, f));
    const result = compare(observed, document(o.baseline), e =>
      originalAbsent(e, { root, readSource }));
    fs.writeFileSync(`${o.check}.summary.json`, json(result)); console.log(json(result));
    for (const e of result.results) if (['NEW', 'GROWN', 'AMBIGUOUS'].includes(e.status))
      console.error(`${e.status} ${e.id} (${observed.platform}): actual ${e.lines}, ` +
        `allowed ${e.ceiling ?? 100}; shrink to the ceiling or split below 101`);
    return result.exit;
  } catch (e) { console.error(`too-many-lines: ${e.message}`); return e.exit ?? 2; }
}
if (isMain(import.meta.url)) process.exitCode = main(process.argv.slice(2));
