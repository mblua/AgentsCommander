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
  const pathKey = windows ? Object.keys(env).sort().find(key => key.toUpperCase() === 'PATH') : 'PATH';
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
export function isMain(url) { return !!process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(url); }
export async function gateCli(runGate, argv = process.argv.slice(2)) {
  try {
    const options = parseArgs(argv);
    const snapshot = createSnapshot({ ...options, cwd: path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..') });
    const result = await runGate(snapshot);
    console.log(JSON.stringify(result, null, 2)); return result.exit;
  } catch (e) { console.error(`ERROR ${e.message}`); return e.exit ?? 3; }
}
