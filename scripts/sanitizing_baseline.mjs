// Immutable Git inputs shared by the sanitizing gates. No import-time I/O.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export class GateError extends Error {
  constructor(message, exit = 3) { super(message); this.exit = exit; }
}
export const codeOrder = (a, b) => a < b ? -1 : a > b ? 1 : 0;
export function decode(bytes, context) {
  try { return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes); }
  catch { throw new GateError(`${context}: invalid UTF8; repair the Git input`); }
}
function resolveGitExecutable({ cwd, env }) {
  const windows = process.platform === 'win32';
  const pathKey = windows ? Object.keys(env).sort(codeOrder).find(key => key.toUpperCase() === 'PATH') : 'PATH';
  const searchPath = (pathKey === undefined ? undefined : env[pathKey]) ??
    (windows ? process.env.PATH : '/usr/bin:/bin');
  const unavailable = detail => new GateError(`Git executable unavailable${detail ? `: ${detail}` : ''}; install Git/check PATH`);
  // Search only the configured PATH; no implicit Windows cwd-first lookup.
  for (let directory of (searchPath ?? '').split(path.delimiter)) {
    if (windows && directory.startsWith('"') && directory.endsWith('"')) directory = directory.slice(1, -1);
    const candidate = path.resolve(cwd, directory, windows ? 'git.exe' : 'git');
    try {
      if (!fs.statSync(candidate).isFile()) continue;
      fs.accessSync(candidate, windows ? fs.constants.F_OK : fs.constants.X_OK);
      return candidate;
    } catch (error) {
      if (!['ENOENT', 'ENOTDIR', 'EACCES', 'EPERM'].includes(error.code)) throw unavailable(error.message);
    }
  }
  throw unavailable();
}
export function gitRunner(args, { cwd, env = process.env }) {
  return spawnSync(resolveGitExecutable({ cwd, env }), args, {
    cwd, env, stdio: ['ignore', 'pipe', 'pipe'], timeout: 120_000,
    maxBuffer: 64 * 1024 * 1024, windowsHide: true,
  });
}
export function parseArgs(argv, env = process.env) {
  let baseRef;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] !== '--base-ref' || baseRef !== undefined || i + 1 === argv.length)
      throw new GateError(`invalid or duplicate argument ${argv[i]}; use --base-ref <commit-ref>`, 2);
    baseRef = argv[++i];
    if (!baseRef.trim() || baseRef.startsWith('--')) throw new GateError('empty/invalid --base-ref', 2);
  }
  return { baseRef: baseRef ?? (env.GATE_BASE_REF?.trim() ? env.GATE_BASE_REF : 'origin/main') };
}
export function parseTree(bytes) {
  const text = decode(bytes, 'ls-tree');
  if (text && !text.endsWith('\0')) throw new GateError('ls-tree: truncated NUL records');
  const result = new Map();
  for (const record of text.split('\0').filter(Boolean)) {
    const match = /^(\d{6}) (\w+) ([a-f0-9]{40}(?:[a-f0-9]{24})?)\t([\s\S]+)$/.exec(record);
    if (!match || result.has(match[4])) throw new GateError('ls-tree: malformed/duplicate record');
    const [, mode, type, oid, name] = match;
    result.set(name, { path: name, mode, type, oid });
  }
  return result;
}
export function parseChanged(bytes) {
  const text = decode(bytes, 'diff name-status');
  if (text && !text.endsWith('\0')) throw new GateError('diff: truncated NUL records');
  const parts = text.split('\0'); parts.pop();
  if (parts.length % 2) throw new GateError('diff: malformed name-status');
  const changed = [];
  for (let i = 0; i < parts.length; i += 2) {
    if (!/^[ADMT]$/.test(parts[i]) || !parts[i + 1]) throw new GateError('diff: unexpected status/path');
    changed.push({ status: parts[i], path: parts[i + 1] });
  }
  return changed.sort((a, b) => codeOrder(a.path, b.path));
}
export function createSnapshot({ baseRef = 'origin/main', cwd, runner = gitRunner } = {}) {
  if (typeof baseRef !== 'string' || !baseRef.trim()) throw new GateError('empty base ref', 2);
  if (!cwd) throw new GateError('explicit repository cwd required', 2);
  const git = args => {
    let r;
    try { r = runner(args, { cwd }); }
    catch (e) { throw new GateError(`git ${args[0]} spawn: ${e.message}`); }
    if (r.error || r.signal || r.status !== 0 || !Buffer.isBuffer(r.stdout) ||
        r.stdout.length > 64 * 1024 * 1024 || (r.stderr?.length ?? 0) > 64 * 1024 * 1024)
      throw new GateError(`git ${args[0]} failed: exit=${r.status} signal=${r.signal ?? '-'} ${r.error?.message ?? ''} ${r.stderr?.toString() ?? ''}; fetch full history/check objects`);
    return r.stdout;
  };
  const oid = args => {
    const value = decode(git(args), 'Git OID').trim();
    if (!/^[a-f0-9]{40}(?:[a-f0-9]{24})?$/.test(value)) throw new GateError('Git returned invalid OID');
    return value;
  };
  const resolve = ref => oid(['rev-parse', '--verify', '--end-of-options', `${ref}^{commit}`]);
  const headOid = resolve('HEAD'), refOid = resolve(baseRef);
  const baseOid = oid(['merge-base', refOid, headOid]);
  const baseTree = parseTree(git(['ls-tree', '-r', '-z', '--full-tree', baseOid]));
  const headTree = parseTree(git(['ls-tree', '-r', '-z', '--full-tree', headOid]));
  const changed = parseChanged(git(['diff', '--no-renames', '--name-status', '-z', baseOid, headOid, '--']));
  const blobs = new Map();
  function readBlob(entry) {
    if (!entry) return '';
    if (!['100644', '100755'].includes(entry.mode) || entry.type !== 'blob')
      throw new GateError(`${entry.path}: unsupported mode/type ${entry.mode}/${entry.type}; use a regular tracked file`);
    if (!blobs.has(entry.oid)) blobs.set(entry.oid, decode(git(['cat-file', 'blob', entry.oid]), entry.path));
    return blobs.get(entry.oid);
  }
  function addedLines(name) {
    const a = baseTree.get(name), b = headTree.get(name);
    // Verify modes even for deleted/new paths before invoking blob diff.
    readBlob(a); readBlob(b);
    if (!a) return new Set(readBlob(b).split(/\r?\n/).map((_, i) => i + 1));
    if (!b) return new Set();
    const diff = decode(git(['diff', '--text', '--no-ext-diff', '--no-textconv', '--no-renames', '-U0', a.oid, b.oid, '--']), name);
    const lines = new Set();
    for (const m of diff.matchAll(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/gm)) {
      const start = Number(m[1]), count = m[2] === undefined ? 1 : Number(m[2]);
      for (let i = 0; i < count; i++) lines.add(start + i);
    }
    return lines;
  }
  return { requestedRef: baseRef, refOid, baseOid, headOid, baseTree, headTree, changed, readBlob, addedLines,
    assertStableHead() { if (resolve('HEAD') !== headOid) throw new GateError(`HEAD drift from ${headOid}; retry on a stable checkout`); } };
}
// Versioned accepted findings (#3182). An entry turns a matching FAIL into ACCEPTED;
// an entry that matches nothing is only a WARN: these gates compare against the PR base,
// so an entry goes stale once its debt is merged.
export const ALLOWLIST_FILE = 'sanitizing.allowlist.json';
const ALLOWLIST_GATES = ['01', '02', '05', '06', '08', '09', 'cog'];
export function parseAllowlist(text) {
  let document;
  try { document = JSON.parse(text); } catch { throw new GateError(`${ALLOWLIST_FILE}: invalid JSON`); }
  if (document?.version !== 1 || !Array.isArray(document.entries))
    throw new GateError(`${ALLOWLIST_FILE}: must be an object with version 1 and an entries array`);
  const keys = new Set();
  for (const [i, e] of document.entries.entries()) {
    const bad = reason => new GateError(`${ALLOWLIST_FILE}: entry ${i} ${reason}`);
    if (!e || typeof e !== 'object' || Array.isArray(e)) throw bad('must be an object');
    if (!ALLOWLIST_GATES.includes(e.gate)) throw bad(`gate must be one of ${ALLOWLIST_GATES.join(', ')}`);
    for (const field of ['rule', 'path', 'reason', 'since'])
      if (typeof e[field] !== 'string' || !e[field].trim()) throw bad(`${field} must be a non-empty string`);
    if (!/^\d{4}-\d{2}-\d{2}$/.test(e.since)) throw bad('since must be YYYY-MM-DD');
    if (!Number.isInteger(e.issue) || e.issue <= 0) throw bad('issue must be a positive integer');
    for (const field of ['symbol', 'hash'])
      if (e[field] !== undefined && (typeof e[field] !== 'string' || !e[field].trim())) throw bad(`${field} must be a non-empty string when present`);
    if (e.gate === 'cog' && (!['new', 'stale'].includes(e.rule) || !e.symbol || !/^[0-9a-f]{12}$/.test(e.hash ?? '')))
      throw bad('cog entries need rule new|stale, symbol and a 12-hex hash');
    if (e.gate !== 'cog' && e.hash !== undefined) throw bad('hash only applies to cog entries');
    const key = JSON.stringify([e.gate, e.rule, e.path, e.symbol ?? null, e.hash ?? null]);
    if (keys.has(key)) throw bad('duplicates an earlier entry');
    keys.add(key);
  }
  return document.entries;
}
export function readAllowlist(root) {
  const file = path.join(root, ALLOWLIST_FILE);
  if (!fs.existsSync(file)) return [];
  return parseAllowlist(fs.readFileSync(file, 'utf8'));
}
// Sanitizing findings match by gate, rule and path; `symbol` only documents the site.
export function applyAllowlist(result, entries) {
  if (result.status === 'ERROR') return result;
  const own = entries.filter(e => e.gate === result.gate), used = new Set();
  const diagnostics = result.diagnostics.map(d => {
    if (d.status !== 'FAIL') return d;
    const entry = own.find(e => e.rule === d.rule && e.path === d.path);
    if (!entry) return d;
    used.add(entry);
    return { ...d, status: 'ACCEPTED', allowlist: { reason: entry.reason, issue: entry.issue, since: entry.since } };
  });
  for (const e of own) if (!used.has(e)) diagnostics.push({ gate: result.gate, status: 'WARN', rule: 'stale_allowlist',
    path: e.path, line: 1, reason: `${ALLOWLIST_FILE} entry ${e.rule} matches no finding`,
    remedy: `remove the entry from ${ALLOWLIST_FILE}; the debt it accepted is gone` });
  const fail = diagnostics.some(d => d.status === 'FAIL');
  return { ...result, status: fail ? 'FAIL' : 'PASS', exit: fail ? 1 : 0,
    accepted: diagnostics.filter(d => d.status === 'ACCEPTED').length,
    warnings: diagnostics.filter(d => d.status === 'WARN').length, diagnostics };
}
export function isMain(url) { return !!process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(url); }
export async function gateCli(runGate, argv = process.argv.slice(2)) {
  try {
    const options = parseArgs(argv);
    const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
    const entries = readAllowlist(root);
    const snapshot = createSnapshot({ ...options, cwd: root });
    const result = applyAllowlist(await runGate(snapshot), entries);
    console.log(JSON.stringify(result, null, 2));
    for (const d of result.diagnostics ?? [])
      if (d.status === 'WARN') console.error(`WARN stale allowlist entry: gate ${d.gate} ${d.path} (${d.reason})`);
    if (result.status !== 'ERROR') console.error(`gate ${result.gate}: ${result.status}, ACCEPTED ${result.accepted} finding(s) from ${ALLOWLIST_FILE}`);
    return result.exit;
  } catch (e) { console.error(`ERROR ${e.message}`); return e.exit ?? 3; }
}
