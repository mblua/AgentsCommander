#!/usr/bin/env node
// #4143: never-grow site ratchet for clippy::struct_excessive_bools.
// Native mode compares one host's validated capture with the reviewed baseline; ratchet mode
// compares committed base/HEAD baseline blobs. Exit 0 equality, 1 policy mismatch, 2 input.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseJSON } from './check-clippy-seven.mjs';
import { normalizeFile } from './check-too-many-lines.mjs';
import { idFor } from './check-cognitive-complexity.mjs';
import { createSnapshot, isMain, gitRunner } from './sanitizing_baseline.mjs';

export const LINT = 'clippy::struct_excessive_bools';
export const MESSAGE = 'more than 3 bools in a struct';
export const BASELINE = 'struct-excessive-bools.baseline.json';
export const TOOLCHAIN = '1.98.1';
export const SEED_COMMIT = 'dd307c2a662d65608aa93c06490f3cc06973b626';
export const HOSTS = ['linux', 'windows'];
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const FIELDS = ['version', 'lint', 'toolchain', 'threshold', 'seedCommit', 'platforms', 'entries'];
const PREFIX_CAP = 4096;

// Frozen adoption seed (#4143 plan v3), independent of baseline contents.
export const SEED = Object.freeze([
  'rust:crates/terminal-snapshot-renderer/src/protocol.rs::TerminalCellStyle',
  'rust:crates/terminal-snapshot-renderer/src/protocol.rs::TerminalSnapshotFidelity',
  'rust:src-tauri/src/capture/state.rs::EffectPreconditions',
  'rust:src-tauri/src/cli/coding_agent.rs::UpdateArgs',
  'rust:src-tauri/src/cli/role_experiment/arguments.rs::RunArgs',
  'rust:src-tauri/src/commands/config/web_server.rs::WebServerOwnedStatus',
  'rust:src-tauri/src/commands/session/create/contracts.rs::PendingLaunch',
  'rust:src-tauri/src/commands/session/create/contracts.rs::PublicationInput',
  'rust:src-tauri/src/commands/session/restart/settlement.rs::RestartFinalizeOptions',
  'rust:src-tauri/src/config/config_seed/perform.rs::ConfigSeedTestHooks',
  'rust:src-tauri/src/config/sessions_persistence/record.rs::PersistedSession',
  'rust:src-tauri/src/config/settings.rs::AppSettings',
  'rust:src-tauri/src/config/settings/project_path/resolution.rs::ProjectPathPersistenceState',
  'rust:src-tauri/src/config/settings/source.rs::SourceCapabilities',
  'rust:src-tauri/src/config/settings/source/coordination.rs::SourceLocalProgress',
  'rust:src-tauri/src/config/teams.rs::PeerInfo',
  'rust:src-tauri/src/config/teams/peer_projection.rs::LeanPeerInfo',
  'rust:src-tauri/src/context_history/report/session_windows.rs::SessionStats',
  'rust:src-tauri/src/phone/mailbox/poller.rs::PurgePeerResult',
  'rust:src-tauri/src/phone/mailbox/pty_claim.rs::InternalSystemBookkeeping',
  'rust:src-tauri/src/phone/mailbox/pty_dispatch/context.rs::SelectionCandidates',
  'rust:src-tauri/src/phone/mailbox/session_lookup.rs::CloseSessionOutcome',
  'rust:src-tauri/src/phone/mailbox/tests/support_retained_source.rs::RetainedSourceFlags',
  'rust:src-tauri/src/pty/response_close.rs::ResponseCloseInner',
  'rust:src-tauri/src/pty/terminal_snapshot/test_support.rs::TerminalSnapshotBlockingControlState',
  'rust:src-tauri/src/resource_monitor/watchdog.rs::ResourceWatchdogDecision',
  'rust:src-tauri/src/session/selection/payload.rs::SessionSelection',
  'rust:src-tauri/src/session/selection/replacement.rs::ReplacementProgress',
  'rust:src-tauri/src/session/selection/replacement.rs::ReplacementState',
  'rust:src-tauri/src/session/selection/replacement_fixture.rs::ReplacementFixtureOwnerObservation',
  'rust:src-tauri/src/session/session.rs::Session',
  'rust:src-tauri/src/session/session.rs::SessionInfo',
  'rust:src-tauri/src/telegram/claude_watcher.rs::ClaudeReader',
]);

export class InputError extends Error { constructor(m) { super(m); this.exit = 2; } }
export class PolicyError extends Error { constructor(m) { super(m); this.exit = 1; } }
const need = (ok, m) => { if (!ok) throw new InputError(m); };
const order = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const sortedUnique = list => list.every((v, i) => !i || order(list[i - 1], v) < 0);
// Imported helpers throw their own error classes; every one of them is invalid input here.
const asInput = fn => {
  try { return fn(); }
  catch (e) { if (e instanceof InputError) throw e; throw new InputError(e.message); }
};

// ---- baseline schema ----

export function validateBaseline(value) {
  need(value && typeof value === 'object' && !Array.isArray(value) &&
    same(Object.keys(value), FIELDS), `baseline fields must be exactly ${FIELDS.join(',')}`);
  need(value.version === 1, 'baseline version must be 1');
  need(value.lint === LINT, `baseline lint must be ${LINT}`);
  need(value.toolchain === TOOLCHAIN, `baseline toolchain must be ${TOOLCHAIN}`);
  need(value.threshold === 3, 'baseline threshold must be 3');
  need(value.seedCommit === SEED_COMMIT, `baseline seedCommit must be ${SEED_COMMIT}`);
  need(same(value.platforms, HOSTS), `baseline platforms must be ${JSON.stringify(HOSTS)}`);
  need(Array.isArray(value.entries), 'baseline entries must be an array');
  for (const e of value.entries) {
    need(e && typeof e === 'object' && !Array.isArray(e) &&
      same(Object.keys(e), ['id', 'platforms']), 'baseline entry fields must be id,platforms');
    need(typeof e.id === 'string' && /^rust:\S+::\S+$/.test(e.id), `invalid entry id ${e.id}`);
    need(Array.isArray(e.platforms) && e.platforms.length > 0 &&
      e.platforms.every(h => HOSTS.includes(h)) && sortedUnique(e.platforms),
    `entry ${e.id} needs nonempty sorted unique host membership`);
  }
  need(sortedUnique(value.entries.map(e => e.id)), 'baseline entry ids must be sorted and unique');
  return value;
}
export const parseBaseline = text => validateBaseline(parseJSON(text));

// ---- threshold guard ----

const CONFIG_NAMES = new Set(['clippy.toml', '.clippy.toml']);
// A line-leading TOML key (bare, "basic" with escapes, or 'literal') before `=` or a dot.
const KEY = /^[ \t]*("(?:[^"\\\r\n]|\\.)*"|'[^'\r\n]*'|[A-Za-z0-9_-]+)[ \t]*[=.]/;
// Case-insensitive: Windows file systems resolve Clippy.toml as clippy.toml.
export const isClippyConfig = file => CONFIG_NAMES.has(file.split('/').at(-1).toLowerCase());
/** Decoded key; an undecodable quoted key fails closed. */
function decodeKey(raw, file) {
  if (raw.startsWith("'")) return raw.slice(1, -1);
  if (!raw.startsWith('"')) return raw;
  try { return JSON.parse(raw); }
  catch { throw new InputError(`${file} has an undecodable quoted key ${raw}`); }
}
/** True when a config text assigns max-struct-bools under any spelling or quoting. */
export function assignsThreshold(text, file = 'clippy.toml') {
  return text.replace(/^﻿/, '').split(/\r?\n/).some(line => {
    const m = KEY.exec(line);
    return m !== null &&
      decodeKey(m[1], file).toLowerCase().replaceAll('_', '-') === 'max-struct-bools';
  });
}
/** Throws when any tracked Clippy config assigns max-struct-bools (any value, even 3). */
export function assertDefaultThreshold(trackedFiles, readText) {
  for (const file of trackedFiles.filter(isClippyConfig)) {
    need(!assignsThreshold(readText(file), file),
      `${file} assigns max-struct-bools; the default threshold 3 must not be overridden`);
  }
}

// ---- capture validation ----

/** One-based line and Unicode column of a UTF-8 byte offset; null when not a boundary. */
export function positionOf(bytes, byte) {
  if (!Number.isSafeInteger(byte) || byte < 0 || byte > bytes.length) return null;
  if (byte < bytes.length && (bytes[byte] & 0xc0) === 0x80) return null;
  const before = bytes.subarray(0, byte).toString('utf8');
  const lineStart = before.lastIndexOf('\n') + 1;
  return { line: before.split('\n').length, column: [...before.slice(lineStart)].length + 1,
    index: before.length };
}

function sourceBytesReader(root, tracked, readBytes) {
  const cache = new Map();
  return file => {
    need(file.endsWith('.rs'), `not Rust source ${file}`);
    need(tracked.has(file), `untracked/generated source ${file}`);
    if (!cache.has(file)) {
      let bytes;
      try { bytes = readBytes(file); }
      catch (e) { throw new InputError(`source unavailable ${file}: ${e.message}`); }
      try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
      catch { throw new InputError(`source not UTF-8 ${file}`); }
      cache.set(file, { bytes, text: bytes.toString('utf8') });
    }
    return cache.get(file);
  };
}

function primarySpan(message, root, source) {
  need(Array.isArray(message.spans), 'missing diagnostic spans');
  const primaries = message.spans.filter(s => s && s.is_primary === true);
  need(primaries.length === 1, 'selected diagnostic needs exactly one primary span');
  const s = primaries[0];
  need(!s.expansion, 'primary span from macro expansion unsupported');
  const file = asInput(() => normalizeFile(s.file_name, root));
  const { bytes } = source(file);
  need(Number.isSafeInteger(s.byte_start) && Number.isSafeInteger(s.byte_end) &&
    s.byte_start >= 0 && s.byte_end > s.byte_start && s.byte_end <= bytes.length,
  `invalid byte offsets in ${file}`);
  const start = positionOf(bytes, s.byte_start), end = positionOf(bytes, s.byte_end);
  need(start && end && start.line === s.line_start && start.column === s.column_start &&
    end.line === s.line_end && end.column === s.column_end,
  `conflicting line/column versus UTF-8 bytes in ${file}`);
  return { file, byte_start: s.byte_start, byte_end: s.byte_end, start };
}

/** Selected whole-item spans, deduplicated across targets by file/start/end. */
export function parseCapture(text, exit, { root, source }) {
  need(exit === 0, `Clippy exited ${exit}`);
  need(typeof text === 'string' && text.trim(), 'empty Cargo capture');
  const lines = text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n');
  const records = lines.map(line => asInput(() => parseJSON(line)));
  need(records.every(r => r && typeof r === 'object' && typeof r.reason === 'string'),
    'invalid Cargo record');
  const end = records.filter(r => r.reason === 'build-finished');
  need(end.length === 1 && end[0] === records.at(-1) && end[0].success === true,
    'capture needs exactly one final successful build-finished');
  const spans = new Map();
  for (const r of records.filter(r => r.reason === 'compiler-message')) {
    const m = r.message;
    need(m && typeof m.level === 'string' && !['error', 'failure-note'].includes(m.level) &&
      !m.level.startsWith('error'), 'error/invalid diagnostic in capture');
    if (m.code?.code !== LINT) continue;
    need(m.message === MESSAGE, `unexpected ${LINT} wording: ${m.message}`);
    const span = primarySpan(m, root, source);
    spans.set(`${span.file}\0${span.byte_start}\0${span.byte_end}`, span);
  }
  return [...spans.values()].sort((a, b) => order(a.file, b.file) || a.byte_start - b.byte_start);
}

// ---- whole-item declaration prefix ----

const isIdentStart = ch => /^[A-Za-z_]$/.test(ch ?? '');
const isIdentContinue = ch => /^[A-Za-z0-9_]$/.test(ch ?? '');

function declarationBlockCommentEnd(text, i, limit) {
  let depth = 0;
  while (i < limit) {
    if (text.startsWith('/*', i)) { depth++; i += 2; }
    else if (text.startsWith('*/', i)) { depth--; i += 2; if (!depth) break; }
    else i++;
  }
  need(depth === 0, 'unterminated block comment in declaration prefix');
  return i;
}
/** Skips whitespace, line comments and nested block comments; fails on an unterminated one. */
function skipTrivia(text, i, limit) {
  for (;;) {
    while (i < limit && /\s/.test(text[i])) i++;
    if (i >= limit) return i;
    if (text.startsWith('//', i)) {
      const nl = text.indexOf('\n', i);
      i = nl < 0 || nl >= limit ? limit : nl + 1;
      continue;
    }
    if (text.startsWith('/*', i)) {
      i = declarationBlockCommentEnd(text, i, limit);
      continue;
    }
    return i;
  }
}
function word(text, i) {
  if (!isIdentStart(text[i])) return null;
  let j = i;
  while (isIdentContinue(text[j])) j++;
  need(!/[^\x00-\x7f]/.test(text[j] ?? ''), 'non-ASCII identifier in declaration prefix');
  return { value: text.slice(i, j), end: j };
}
function keyword(text, i, value) {
  const w = word(text, i);
  return w && w.value === value ? w.end : null;
}

function visibilityPathEnd(text, i, limit) {
  for (;;) {
    const seg = word(text, i);
    need(seg, 'invalid pub(in path)');
    i = skipTrivia(text, seg.end, limit);
    if (!text.startsWith('::', i)) break;
    i = skipTrivia(text, i + 2, limit);
  }
  return i;
}
function declarationVisibilityEnd(text, i, limit) {
  const pub = keyword(text, i, 'pub');
  if (pub === null) return i;
  i = skipTrivia(text, pub, limit);
  if (text[i] !== '(') return i;
  i = skipTrivia(text, i + 1, limit);
  const scope = word(text, i);
  need(scope && ['crate', 'self', 'super', 'in'].includes(scope.value),
    'unsupported visibility');
  i = skipTrivia(text, scope.end, limit);
  if (scope.value === 'in') i = visibilityPathEnd(text, i, limit);
  need(text[i] === ')', 'unterminated visibility');
  return skipTrivia(text, i + 1, limit);
}

/** Name and JS-string index of `[visibility] struct <ident>` at index; fails closed. */
export function declarationName(text, index) {
  const limit = Math.min(text.length, index + PREFIX_CAP);
  const i = declarationVisibilityEnd(text, skipTrivia(text, index, limit), limit);
  const struct = keyword(text, i, 'struct');
  need(struct !== null, 'span does not start with a struct declaration');
  const after = skipTrivia(text, struct, limit);
  need(after > struct, 'missing separator after struct keyword');
  const raw = text.startsWith('r#', after);
  const name = word(text, raw ? after + 2 : after);
  need(name && name.value !== '_', 'missing or unsupported struct identifier');
  return { name: (raw ? 'r#' : '') + name.value, index: after };
}

/** Stable site identities; two distinct declarations with one identity fail. */
export function identities(spans, source) {
  const readSource = file => source(file).text;
  const ids = new Map();
  for (const s of spans) {
    const { text } = source(s.file);
    const decl = declarationName(text, s.start.index);
    const before = text.slice(0, decl.index), lineStart = before.lastIndexOf('\n') + 1;
    const line = before.split('\n').length, column = [...before.slice(lineStart)].length + 1;
    const id = asInput(() => idFor(s.file, line, column, decl.name, readSource));
    need(!id.endsWith('::{closure}'), `unresolved identity at ${s.file}:${line}`);
    const key = `${s.file}\0${s.byte_start}\0${s.byte_end}`;
    need(!ids.has(id) || ids.get(id) === key, `distinct declarations share identity ${id}`);
    ids.set(id, key);
  }
  return [...ids.keys()].sort(order);
}

// ---- comparisons ----

/** NEW and stale ids of one host's observations against the baseline. */
export function compareHost(observed, baseline, host) {
  const expected = baseline.entries.filter(e => e.platforms.includes(host)).map(e => e.id);
  const want = new Set(expected), got = new Set(observed);
  return { expected, added: observed.filter(id => !want.has(id)),
    stale: expected.filter(id => !got.has(id)) };
}

/** Policy failures of HEAD baseline versus base baseline (null base: initial adoption). */
export function ratchet(head, base) {
  const failures = [];
  if (base === null) {
    if (!same(head.entries.map(e => e.id), [...SEED]))
      failures.push('initial baseline ids must equal the exact frozen 33-site seed');
    return failures;
  }
  for (const k of FIELDS.filter(k => k !== 'entries'))
    if (!same(head[k], base[k])) failures.push(`baseline metadata ${k} changed`);
  const prior = new Map(base.entries.map(e => [e.id, e.platforms]));
  for (const e of head.entries) {
    const was = prior.get(e.id);
    if (!was) { failures.push(`NEW baseline site ${e.id}`); continue; }
    for (const h of e.platforms)
      if (!was.includes(h)) failures.push(`NEW host ${h} for ${e.id}`);
  }
  return failures;
}

// ---- CLI ----

const NATIVE = ['capture', 'clippy-exit', 'platform', 'rustc-version', 'workspace-root',
  'baseline'];
export function parseArgs(argv) {
  const opts = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--baseline-diff') {
      need(!opts['baseline-diff'], 'duplicate flag --baseline-diff');
      opts['baseline-diff'] = true;
      continue;
    }
    const name = arg.startsWith('--') ? arg.slice(2) : null;
    need(name && [...NATIVE, 'base-ref'].includes(name), `unknown argument ${arg}`);
    need(!(name in opts), `duplicate flag ${arg}`);
    const value = argv[++i];
    need(typeof value === 'string' && value !== '' && !value.startsWith('--'),
      `missing value for ${arg}`);
    opts[name] = value;
  }
  if (opts['baseline-diff']) {
    need(Object.keys(opts).every(k => ['baseline-diff', 'base-ref'].includes(k)),
      'mixed modes: --baseline-diff takes only --base-ref');
    need(opts['base-ref'], '--baseline-diff requires --base-ref');
    return { mode: 'diff', baseRef: opts['base-ref'] };
  }
  need(!('base-ref' in opts), 'mixed modes: --base-ref needs --baseline-diff');
  for (const k of NATIVE) need(k in opts, `missing --${k}`);
  need(/^-?\d+$/.test(opts['clippy-exit']), '--clippy-exit must be an integer');
  need(HOSTS.includes(opts.platform), '--platform must be windows or linux');
  need(opts['rustc-version'] === TOOLCHAIN, `--rustc-version must be ${TOOLCHAIN}`);
  return { mode: 'native', capture: opts.capture, exit: Number(opts['clippy-exit']),
    platform: opts.platform, root: opts['workspace-root'], baseline: opts.baseline };
}

function trackedFiles(root) {
  const r = gitRunner(['ls-tree', '-r', '--name-only', '-z', 'HEAD'], { cwd: root });
  need(!r.error && r.status === 0, `git ls-tree failed: ${r.error?.message ?? r.stderr}`);
  return r.stdout.toString('utf8').split('\0').filter(Boolean);
}

/** Native host comparison; returns the result lines and exit code. */
export function runNative(o, io = {}) {
  const readFile = io.readFile ?? (f => fs.readFileSync(f));
  const root = path.resolve(o.root);
  const tracked = io.tracked ?? trackedFiles(root);
  const readRepo = file => readFile(path.join(root, file));
  assertDefaultThreshold(tracked, file => {
    try { return readRepo(file).toString('utf8'); }
    catch (e) { throw new InputError(`cannot read ${file}: ${e.message}`); }
  });
  let baselineText, captureText;
  try { baselineText = readFile(o.baseline).toString('utf8'); }
  catch (e) { throw new InputError(`baseline unavailable: ${e.message}`); }
  const baseline = parseBaseline(baselineText);
  try { captureText = readFile(o.capture).toString('utf8'); }
  catch (e) { throw new InputError(`capture unavailable: ${e.message}`); }
  const source = sourceBytesReader(root, new Set(tracked), readRepo);
  const spans = parseCapture(captureText, o.exit, { root, source });
  const observed = identities(spans, source);
  const { expected, added, stale } = compareHost(observed, baseline, o.platform);
  const lines = [`host=${o.platform} toolchain=${TOOLCHAIN} lint=${LINT}`,
    `observed=${observed.length} baseline=${expected.length} new=${added.length} ` +
      `stale=${stale.length}`,
    ...observed.map(id => `OBSERVED ${id}`), ...added.map(id => `NEW ${id}`),
    ...stale.map(id => `STALE ${id}`)];
  return { lines, observed, exit: added.length || stale.length ? 1 : 0 };
}

/** Committed base/HEAD baseline ratchet. */
export function runDiff(baseRef, root = ROOT, runner = gitRunner) {
  const snap = asInput(() => createSnapshot({ baseRef, cwd: root, runner }));
  const headEntry = snap.headTree.get(BASELINE), baseEntry = snap.baseTree.get(BASELINE);
  need(headEntry || !baseEntry, `${BASELINE} deleted; a missing baseline never means empty`);
  need(headEntry, `${BASELINE} missing at HEAD`);
  const head = parseBaseline(asInput(() => snap.readBlob(headEntry)));
  const base = baseEntry ? parseBaseline(asInput(() => snap.readBlob(baseEntry))) : null;
  const failures = ratchet(head, base);
  asInput(() => snap.assertStableHead());
  const lines = [`base=${snap.baseOid} head=${snap.headOid} entries=${head.entries.length}` +
    (base ? ` baseEntries=${base.entries.length}` : ' adoption=seed'), ...failures];
  return { lines, exit: failures.length ? 1 : 0 };
}

export function main(argv, io = {}) {
  const out = io.out ?? (s => process.stdout.write(s + '\n'));
  const err = io.err ?? (s => process.stderr.write(s + '\n'));
  try {
    const o = parseArgs(argv);
    const r = o.mode === 'diff' ? runDiff(o.baseRef, io.root ?? ROOT) : runNative(o, io);
    for (const line of r.lines) out(line);
    out(r.exit ? 'struct-excessive-bools: FAIL' : 'struct-excessive-bools: PASS');
    return r.exit;
  } catch (e) {
    err(`struct-excessive-bools: ${e.message}`);
    return e.exit === 1 ? 1 : 2;
  }
}

if (isMain(import.meta.url)) process.exitCode = main(process.argv.slice(2));
