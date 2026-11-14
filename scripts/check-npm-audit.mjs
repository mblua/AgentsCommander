#!/usr/bin/env node
// npm audit PR gate for issue #4490: fails only on advisories not in the baseline.
//
// Runs `npm audit` over every dependency type and compares each advisory key
// (`<GHSA-id>@<package>`) with the reviewed npm-audit.baseline.json. A key that
// is not in the baseline fails. Keys gone from the audit (STALE) or with a new
// severity (ESCALATED / LOWERED) only print a warning. The script never writes
// the baseline; accepting an advisory is a reviewed diff to that file.
//
// Usage:
//   node scripts/check-npm-audit.mjs              # live check
//   node scripts/check-npm-audit.mjs --self-test  # in-memory cases, no npm run
//
// Exit codes:
//   0 → no new advisory (or self-test passed)
//   1 → one or more new advisories (or a self-test case failed)
//   2 → npm audit unavailable or malformed, bad baseline, or bad usage

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { deepStrictEqual } from 'node:assert';

export const AUDIT_ARGS = Object.freeze(['audit', '--json', '--audit-level=info',
  '--include=prod', '--include=dev', '--include=optional', '--include=peer']);

const BASELINE_URL = new URL('../npm-audit.baseline.json', import.meta.url);
const ROOT = fileURLToPath(new URL('..', import.meta.url));
const SEVERITIES = ['info', 'low', 'moderate', 'high', 'critical'];
const URL_RE = /^https:\/\/github\.com\/advisories\/(GHSA(-[23456789cfghjmpqrvwx]{4}){3})$/;
const ID_RE = /^GHSA(-[23456789cfghjmpqrvwx]{4}){3}$/;
const ENTRY_KEYS = ['id', 'package', 'severity', 'reason'];
const TOP_KEYS = ['schemaVersion', 'issue', 'entries'];
const MAX_REASON = 82;
const OUTAGE = 'npm audit unavailable or malformed; rerun the job when npm answers';

export class AuditError extends Error {
  constructor(detail, stderr = '') {
    super(`${OUTAGE} (${detail})`);
    this.name = 'AuditError';
    this.stderr = stderr;
  }
}

export class BaselineError extends Error {
  constructor(detail) {
    super(`npm audit baseline invalid: ${detail}`);
    this.name = 'BaselineError';
  }
}

const isPlainObject = (v) =>
  typeof v === 'object' && v !== null && !Array.isArray(v);
const rank = (s) => SEVERITIES.indexOf(s);
// Own keys are unique, so equal length + every wanted key present = same key set.
const sameKeys = (obj, keys) =>
  Object.keys(obj).length === keys.length && keys.every((k) => Object.hasOwn(obj, k));

// The only comparator: plain JS code-unit order (§3), never locale order.
export function byCodeUnit(a, b) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

// Rules 1-2 of the fail-closed list.
export function checkSpawn({ error, status }) {
  if (error) throw new AuditError(`spawn failed: ${error.message}`);
  if (status === null) throw new AuditError('npm timed out or was killed');
  if (status !== 0 && status !== 1) throw new AuditError(`npm exit status ${status}`);
  return undefined;
}

// Rules 3-5: raw stdout → the `vulnerabilities` object.
function auditVulnerabilities(stdoutText) {
  let data;
  try {
    data = JSON.parse(stdoutText);
  } catch {
    throw new AuditError('stdout is not JSON');
  }
  if (!isPlainObject(data)) throw new AuditError('stdout is not a JSON object');
  if ('error' in data) throw new AuditError('npm reported an error object');
  if (data.auditReportVersion !== 2) throw new AuditError('auditReportVersion is not 2');
  if (!isPlainObject(data.vulnerabilities)) {
    throw new AuditError('vulnerabilities is not an object');
  }
  return data.vulnerabilities;
}

function viaItems(vuln) {
  return isPlainObject(vuln) && Array.isArray(vuln.via) ? vuln.via : [];
}

// Rule 7 for one `via` object → [key, severity].
function advisoryKey(item) {
  const m = isPlainObject(item) && typeof item.url === 'string' && URL_RE.exec(item.url);
  if (!m) throw new AuditError(`bad advisory url: ${JSON.stringify(item?.url)}`);
  if (typeof item.name !== 'string' || item.name === '') {
    throw new AuditError(`empty package name for ${m[1]}`);
  }
  if (rank(item.severity) < 0) {
    throw new AuditError(`unknown severity ${JSON.stringify(item.severity)}`);
  }
  return [`${m[1]}@${item.name}`, item.severity];
}

// Rules 3-7: raw stdout + exit status → Map<key, severity>.
export function liveKeys(stdoutText, status) {
  const vulns = auditVulnerabilities(stdoutText);
  const keys = new Map();
  for (const vuln of Object.values(vulns)) {
    for (const item of viaItems(vuln)) {
      if (typeof item === 'string') continue;
      const [key, sev] = advisoryKey(item);
      const prev = keys.get(key);
      if (prev === undefined || rank(sev) > rank(prev)) keys.set(key, sev);
    }
  }
  if (status === 0 && keys.size > 0) throw new AuditError('exit 0 with advisories');
  if (status === 1 && keys.size === 0) throw new AuditError('exit 1 without advisories');
  return keys;
}

// Baseline text → Map<key, entry>; any schema problem throws BaselineError.
export function parseBaseline(text) {
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    throw new BaselineError('not JSON');
  }
  if (!isPlainObject(data) || !sameKeys(data, TOP_KEYS)) {
    throw new BaselineError(`top-level keys must be exactly ${TOP_KEYS.join(', ')}`);
  }
  if (data.schemaVersion !== 1) throw new BaselineError('schemaVersion must be 1');
  if (!Number.isInteger(data.issue) || data.issue <= 0) {
    throw new BaselineError('issue must be a positive integer');
  }
  if (!Array.isArray(data.entries)) throw new BaselineError('entries must be an array');
  const map = new Map();
  let last = null;
  data.entries.forEach((e, i) => {
    const at = `entry ${i}`;
    if (!isPlainObject(e) || !sameKeys(e, ENTRY_KEYS)) {
      throw new BaselineError(`${at}: keys must be exactly ${ENTRY_KEYS.join(', ')}`);
    }
    if (typeof e.id !== 'string' || !ID_RE.test(e.id)) throw new BaselineError(`${at}: bad id`);
    if (typeof e.package !== 'string' || e.package === '') {
      throw new BaselineError(`${at}: empty package`);
    }
    if (rank(e.severity) < 0) throw new BaselineError(`${at}: bad severity`);
    const r = e.reason;
    if (typeof r !== 'string' || r === '' || r !== r.trim() || r.length > MAX_REASON) {
      throw new BaselineError(`${at}: reason must be trimmed, non-empty, <= ${MAX_REASON} chars`);
    }
    const key = `${e.id}@${e.package}`;
    if (last !== null && last >= key) {
      throw new BaselineError(`${at}: ${key} not strictly after ${last} (sort, no duplicates)`);
    }
    last = key;
    map.set(key, e);
  });
  return map;
}

// live: Map<key, severity>; base: Map<key, entry>. Each result field is a sorted key array.
export function compare(live, base) {
  const added = [];
  const stale = [];
  const escalated = [];
  const lowered = [];
  for (const [key, sev] of live) {
    const entry = base.get(key);
    if (entry === undefined) added.push(key);
    else if (rank(sev) > rank(entry.severity)) escalated.push(key);
    else if (rank(sev) < rank(entry.severity)) lowered.push(key);
  }
  for (const key of base.keys()) if (!live.has(key)) stale.push(key);
  const diff = { added, stale, escalated, lowered };
  for (const list of Object.values(diff)) list.sort(byCodeUnit);
  // Severities for report(); not enumerable so the result keeps its four fields.
  Object.defineProperty(diff, 'live', { value: live });
  Object.defineProperty(diff, 'base', { value: base });
  return diff;
}

export function report(diff, baseSize) {
  const lines = [];
  const warn = (text) => lines.push(`::warning::npm audit baseline: ${text}`);
  for (const key of diff.added) lines.push(`NEW ${key} ${diff.live.get(key)}`);
  for (const key of diff.stale) {
    warn(`STALE ${key} (advisory no longer reported; remove the entry)`);
  }
  for (const key of diff.escalated) {
    warn(`ESCALATED ${key} ${diff.base.get(key).severity}->${diff.live.get(key)}`);
  }
  for (const key of diff.lowered) {
    warn(`LOWERED ${key} ${diff.base.get(key).severity}->${diff.live.get(key)}`);
  }
  if (diff.added.length > 0) return { code: 1, lines };
  lines.push(`npm audit gate: OK (${baseSize} baselined keys)`);
  return { code: 0, lines };
}

// npm's own CLI next to the running node; never looked up on PATH (javascript:S4036).
export function npmCli(execPath = process.execPath) {
  const d = path.dirname(execPath);
  const tried = [
    path.join(d, 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    path.join(d, '..', 'lib', 'node_modules', 'npm', 'bin', 'npm-cli.js'),
    path.join(d, '..', 'share', 'nodejs', 'npm', 'bin', 'npm-cli.js'),
  ];
  const found = tried.find((p) => existsSync(p));
  if (found === undefined) {
    throw new AuditError(`npm CLI not found next to ${execPath}; tried: ${tried.join(', ')}`);
  }
  return found;
}

export function runAudit() {
  const opts = { cwd: ROOT, encoding: 'utf8', timeout: 300000, maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'] };
  // Direct child without a shell; same npm code the npm shim runs, killable on timeout.
  const result = spawnSync(process.execPath, [npmCli(), ...AUDIT_ARGS], opts);
  try {
    checkSpawn(result);
  } catch (err) {
    err.stderr = result.stderr ?? '';
    throw err;
  }
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

// --- self-test (§6 of the #4490 plan): in-memory only, never spawns npm ----------------

const GHSA = 'https://github.com/advisories/';
const advisory = (id, name, severity) => ({ url: GHSA + id, name, severity });
const auditJson = (vias) => JSON.stringify({ auditReportVersion: 2,
  vulnerabilities: { pkg: { via: ['transitive-ref', ...vias] } } });
const REASON = 'Self-test entry.';
const entry = (id, pkg, severity, reason = REASON) => ({ id, package: pkg, severity, reason });
const baselineText = (entries, extra = {}) =>
  JSON.stringify({ schemaVersion: 1, issue: 4490, entries, ...extra });
const A = 'GHSA-22p9-wv53-3rq4';
const B = 'GHSA-82fw-gwwq-j7x9';

function expectThrow(fn, Kind) {
  let thrown = null;
  try {
    fn();
  } catch (err) {
    thrown = err;
  }
  if (!(thrown instanceof Kind)) {
    throw new TypeError(`expected ${Kind.name}, got ${thrown ? thrown.name : 'no error'}`);
  }
}

function run(liveJson, status, entries) {
  const live = liveKeys(liveJson, status);
  const base = parseBaseline(baselineText(entries));
  const diff = compare(live, base);
  return { diff, ...report(diff, base.size) };
}

const CASES = [
  ['1 empty audit, empty baseline → 0', () => {
    const r = run('{"auditReportVersion":2,"vulnerabilities":{}}', 0, []);
    deepStrictEqual([r.code, r.lines], [0, ['npm audit gate: OK (0 baselined keys)']]);
  }],
  ['2 live = baseline → 0, no warning', () => {
    const r = run(auditJson([advisory(A, 'linkify-it', 'high')]), 1,
      [entry(A, 'linkify-it', 'high')]);
    deepStrictEqual([r.code, r.lines], [0, ['npm audit gate: OK (1 baselined keys)']]);
  }],
  ['3 extra live key → 1, NEW', () => {
    const r = run(auditJson([advisory(A, 'linkify-it', 'high'), advisory(B, 'vitest', 'low')]),
      1, [entry(A, 'linkify-it', 'high')]);
    deepStrictEqual([r.code, r.diff.added, r.lines], [1, [`${B}@vitest`], [`NEW ${B}@vitest low`]]);
  }],
  ['4 baseline key not live → 0, STALE warning', () => {
    const r = run(auditJson([advisory(A, 'linkify-it', 'high')]), 1,
      [entry(A, 'linkify-it', 'high'), entry(B, 'vitest', 'low')]);
    deepStrictEqual([r.code, r.diff.stale], [0, [`${B}@vitest`]]);
    deepStrictEqual(r.lines.filter((l) => l.startsWith('::warning::')), [
      `::warning::npm audit baseline: STALE ${B}@vitest `
      + '(advisory no longer reported; remove the entry)']);
  }],
  ['5 escalated and lowered → 0 with warnings', () => {
    const up = run(auditJson([advisory(A, 'linkify-it', 'critical')]), 1,
      [entry(A, 'linkify-it', 'high')]);
    deepStrictEqual([up.code, up.diff.escalated, up.lines[0]], [0, [`${A}@linkify-it`],
      `::warning::npm audit baseline: ESCALATED ${A}@linkify-it high->critical`]);
    const down = run(auditJson([advisory(A, 'linkify-it', 'low')]), 1,
      [entry(A, 'linkify-it', 'high')]);
    deepStrictEqual([down.code, down.diff.lowered, down.lines[0]], [0, [`${A}@linkify-it`],
      `::warning::npm audit baseline: LOWERED ${A}@linkify-it high->low`]);
  }],
  ['6 same GHSA, two packages → two keys', () => {
    const live = liveKeys(auditJson([advisory(B, 'vitest', 'moderate'),
      advisory(B, '@vitest/mocker', 'moderate')]), 1);
    deepStrictEqual([...live.keys()].sort(byCodeUnit), [`${B}@@vitest/mocker`, `${B}@vitest`]);
  }],
  ['7 stdout not JSON', () => expectThrow(() => liveKeys('not json', 1), AuditError)],
  ['8 outage shape', () =>
    expectThrow(() => liveKeys('{"message":"x","error":{}}', 1), AuditError)],
  ['9 auditReportVersion 1', () =>
    expectThrow(() => liveKeys('{"auditReportVersion":1,"vulnerabilities":{}}', 1), AuditError)],
  ['10 status 1 with zero keys', () =>
    expectThrow(() => liveKeys('{"auditReportVersion":2,"vulnerabilities":{}}', 1), AuditError)],
  ['11 status 0 with one key', () =>
    expectThrow(() => liveKeys(auditJson([advisory(A, 'linkify-it', 'high')]), 0), AuditError)],
  ['12 non-GHSA url', () => expectThrow(() => liveKeys(auditJson([
    { url: 'https://example.com/x', name: 'linkify-it', severity: 'high' }]), 1), AuditError)],
  ['13 checkSpawn status 2', () =>
    expectThrow(() => checkSpawn({ error: undefined, status: 2 }), AuditError)],
  ['14 checkSpawn timeout and spawn error; npm CLI not found', () => {
    expectThrow(() => checkSpawn({ error: undefined, status: null }), AuditError);
    expectThrow(() => checkSpawn({ error: new Error('x'), status: 1 }), AuditError);
    expectThrow(() => npmCli(path.join(tmpdir(), 'no-node-4490', 'bin', 'node')), AuditError);
  }],
  ['15 invalid baselines', () => {
    const good = entry(A, 'linkify-it', 'high');
    const bad = [
      baselineText([entry(B, 'vitest', 'low'), good]),
      baselineText([good, good]),
      baselineText([entry(A, 'linkify-it', 'high', '')]),
      baselineText([entry(A, 'linkify-it', 'high', 'x'.repeat(83))]),
      baselineText([entry(A, 'linkify-it', 'high', ' outer space')]),
      baselineText([{ ...good, extra: 1 }]),
      JSON.stringify({ schemaVersion: 2, issue: 4490, entries: [good] }),
      baselineText([entry(A, 'linkify-it', 'severe')]),
      baselineText([entry('GHSA-1111-1111-1111', 'linkify-it', 'high')]),
    ];
    for (const text of bad) expectThrow(() => parseBaseline(text), BaselineError);
  }],
  ['16 frozen argv; status 0 and 1 accepted', () => {
    deepStrictEqual([...AUDIT_ARGS], ['audit', '--json', '--audit-level=info',
      '--include=prod', '--include=dev', '--include=optional', '--include=peer']);
    deepStrictEqual(Object.isFrozen(AUDIT_ARGS), true);
    checkSpawn({ error: undefined, status: 0 });
    checkSpawn({ error: undefined, status: 1 });
  }],
];

export function selfTest() {
  let failed = 0;
  for (const [name, fn] of CASES) {
    try {
      fn();
    } catch (err) {
      failed += 1;
      console.error(`self-test FAIL ${name}: ${err.message}`);
    }
  }
  if (failed > 0) {
    console.error(`self-test: ${failed} of ${CASES.length} failed`);
    return 1;
  }
  console.log(`self-test: ${CASES.length} passed`);
  return 0;
}

export function main(argv) {
  if (argv.length === 1 && argv[0] === '--self-test') return selfTest();
  if (argv.length !== 0) {
    console.error('usage: node scripts/check-npm-audit.mjs [--self-test]');
    return 2;
  }
  try {
    const base = parseBaseline(readFileSync(BASELINE_URL, 'utf8'));
    const result = runAudit();
    let live;
    try {
      live = liveKeys(result.stdout, result.status);
    } catch (err) {
      if (err instanceof AuditError) err.stderr = result.stderr ?? '';
      throw err;
    }
    const { code, lines } = report(compare(live, base), base.size);
    for (const line of lines) console.log(line);
    return code;
  } catch (err) {
    if (!(err instanceof AuditError) && !(err instanceof BaselineError)) throw err;
    console.error(err.message);
    const stderr = (err.stderr ?? '').trim().slice(0, 4000);
    if (stderr) console.error(stderr);
    return 2;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
