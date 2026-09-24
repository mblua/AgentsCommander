#!/usr/bin/env node

// Gate for #2447: fails when src-tauri/module-arcs.txt does not match what the two recording
// steps produce from the current tree. It regenerates a candidate record into an OS temp
// directory (never the work tree) and compares it with the committed record as raw bytes.
//
// The committed record was produced on Windows under Node v24.13.0 (#2462). If a later Node or
// OS change moves the output, that is where to start looking.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DETECTOR = path.join(ROOT, 'scripts', '01-rust_module-dependency-cycles.mjs');
const PROJECTION = path.join(ROOT, 'scripts', '02-module-arc-record.mjs');
const RECORD = path.join(ROOT, 'src-tauri', 'module-arcs.txt');
const CRATE = path.join(ROOT, 'src-tauri');
const EXPECTED_TOOL_VERSION = '1.1.1';

const REGENERATE = `To regenerate the record, from the repo root:

  node scripts/01-rust_module-dependency-cycles.mjs src-tauri --emit-graph graph.json --quiet
  npm run record:arcs -- --graph graph.json

then delete graph.json (it is gitignored and must never be committed) and commit
src-tauri/module-arcs.txt. The detector exits 1 when it finds cycles; that is expected.`;

const USAGE = `Usage: node scripts/check-module-arcs.mjs [--self-test] [--help]

Regenerates the module-arc record from the current tree into a temp directory and
fails when it differs from src-tauri/module-arcs.txt byte for byte.

  --self-test  Run the embedded self-test (10 cases).
  --help       Print this usage and exit 0.`;

class GateError extends Error {}

export function realRunner(argv) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, argv, { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d; });
    child.stderr.on('data', (d) => { stderr += d; });
    child.on('error', reject);
    child.on('close', (code) => resolve({ exit: code, stdout, stderr }));
  });
}

function readRecord(recordPath) {
  let bytes;
  try {
    bytes = fs.readFileSync(recordPath);
  } catch (err) {
    throw new GateError(`committed record ${recordPath} is missing or unreadable: ${err.message}`);
  }
  if (bytes.length === 0) throw new GateError(`committed record ${recordPath} is zero bytes`);
  return bytes;
}

function splitLines(text) {
  const lines = text.split('\n');
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}

// Lines keep their terminator, so a CRLF line, or a last line without a newline, differs from
// its LF twin.
function terminatedLines(text) {
  return text.split(/(?<=\n)/).filter((line) => line !== '');
}

const CONTEXT = 3;
const NO_NEWLINE = '\\ No newline at end of file';

// A unified diff (git-style headers, @@ hunks, 3 context lines, LCS) from the committed record to
// the regenerated one, so `git apply` accepts it. Line ends are kept verbatim, never escaped.
export function unifiedDiff(oldText, newText) {
  const a = terminatedLines(oldText);
  const b = terminatedLines(newText);
  const n = a.length;
  const m = b.length;
  const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  // ops: [kind, oldIndex, newIndex]
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      ops.push([' ', i, j]);
      i += 1;
      j += 1;
    } else if (j < m && (i === n || lcs[i][j + 1] >= lcs[i + 1][j])) {
      ops.push(['+', i, j]);
      j += 1;
    } else {
      ops.push(['-', i, j]);
      i += 1;
    }
  }
  const out = ['--- a/src-tauri/module-arcs.txt', '+++ b/src-tauri/module-arcs.txt'];
  const emit = (prefix, line) => {
    if (line.endsWith('\n')) out.push(prefix + line.slice(0, -1));
    else out.push(prefix + line, NO_NEWLINE);
  };
  let k = 0;
  while (k < ops.length) {
    if (ops[k][0] === ' ') {
      k += 1;
      continue;
    }
    // One hunk absorbs every later change that sits within 2 * CONTEXT unchanged lines.
    const first = Math.max(0, k - CONTEXT);
    let last = k;
    for (let e = k + 1; e < ops.length && e - last <= 2 * CONTEXT; e += 1) {
      if (ops[e][0] !== ' ') last = e;
    }
    const stop = Math.min(ops.length, last + CONTEXT + 1);
    const hunk = ops.slice(first, stop);
    const oldCount = hunk.filter((op) => op[0] !== '+').length;
    const newCount = hunk.filter((op) => op[0] !== '-').length;
    const oldStart = oldCount === 0 ? hunk[0][1] : hunk[0][1] + 1;
    const newStart = newCount === 0 ? hunk[0][2] : hunk[0][2] + 1;
    out.push(`@@ -${oldStart},${oldCount} +${newStart},${newCount} @@`);
    for (const [kind, oi, ni] of hunk) emit(kind, kind === '+' ? b[ni] : a[oi]);
    k = stop;
  }
  return `${out.join('\n')}\n`;
}

// Applies a diff produced by unifiedDiff to `oldText`, strictly: headers, hunk counts and every
// context or removed line must match. Used only by the self-test to prove the format.
export function applyUnifiedDiff(oldText, diff) {
  const a = terminatedLines(oldText);
  const lines = diff.split('\n');
  if (lines.pop() !== '') throw new Error('diff must end with a newline');
  if (lines[0] !== '--- a/src-tauri/module-arcs.txt' || lines[1] !== '+++ b/src-tauri/module-arcs.txt') {
    throw new Error('missing ---/+++ headers');
  }
  const result = [];
  let pos = 0;
  let li = 2;
  if (li === lines.length) throw new Error('no hunk');
  while (li < lines.length) {
    const h = /^@@ -(\d+),(\d+) \+(\d+),(\d+) @@$/.exec(lines[li]);
    if (!h) throw new Error(`bad hunk header: ${lines[li]}`);
    li += 1;
    let [oldStart, oldCount, , newCount] = h.slice(1).map(Number);
    const start = oldCount === 0 ? oldStart : oldStart - 1;
    if (start < pos) throw new Error('overlapping hunks');
    result.push(...a.slice(pos, start));
    pos = start;
    while (oldCount > 0 || newCount > 0) {
      const line = lines[li];
      if (line === undefined) throw new Error('hunk shorter than its header');
      li += 1;
      let body = line.slice(1);
      if (lines[li] === NO_NEWLINE) li += 1;
      else body += '\n';
      if (line[0] === '+') {
        result.push(body);
        newCount -= 1;
        continue;
      }
      if (line[0] !== ' ' && line[0] !== '-') throw new Error(`bad hunk line: ${line}`);
      if (a[pos] !== body) throw new Error(`hunk does not match line ${pos + 1}`);
      pos += 1;
      oldCount -= 1;
      if (line[0] === ' ') {
        result.push(body);
        newCount -= 1;
      }
    }
    if (oldCount !== 0 || newCount !== 0) throw new Error('hunk counts do not match its lines');
  }
  result.push(...a.slice(pos));
  return result.join('');
}

// Runs the two steps with `runner` and compares. Returns { ok, message }; throws GateError.
export async function checkModuleArcs({ runner = realRunner, recordPath = RECORD } = {}) {
  const committed = readRecord(recordPath);
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'check-module-arcs-'));
  try {
    const graphPath = path.join(tmp, 'graph.json');
    const candidatePath = path.join(tmp, 'module-arcs.txt');

    const det = await runner([DETECTOR, CRATE, '--emit-graph', graphPath, '--quiet']);
    if (det.exit === 3) {
      throw new GateError(`detector exit 3: analysis incomplete, no graph written\n${det.stderr}`);
    }
    if (det.exit !== 0 && det.exit !== 1) {
      throw new GateError(`detector exit ${det.exit}: unexpected exit code (only 0 and 1 are accepted)\n${det.stderr}`);
    }

    let graph;
    try {
      graph = JSON.parse(fs.readFileSync(graphPath, 'utf8'));
    } catch (err) {
      throw new GateError(`detector exit ${det.exit} but its graph is unreadable: ${err.message}`);
    }
    const version = graph && graph.tool && graph.tool.version;
    if (version !== EXPECTED_TOOL_VERSION) {
      throw new GateError(`detector TOOL_VERSION is ${JSON.stringify(version)}, expected '${EXPECTED_TOOL_VERSION}'`);
    }

    const proj = await runner([PROJECTION, '--graph', graphPath, '--out', candidatePath]);
    if (proj.exit !== 0) {
      throw new GateError(`projection exit ${proj.exit}\n${proj.stderr}`);
    }
    let candidate;
    try {
      candidate = fs.readFileSync(candidatePath);
    } catch (err) {
      throw new GateError(`projection exit 0 but wrote no record: ${err.message}`);
    }

    if (Buffer.compare(committed, candidate) === 0) {
      const arcs = splitLines(committed.toString('utf8')).length;
      return { ok: true, message: `module-arcs.txt is current: ${arcs} arcs, ${committed.length} bytes.` };
    }
    const diff = unifiedDiff(committed.toString('utf8'), candidate.toString('utf8'));
    const crNote = committed.includes(13) || candidate.includes(13)
      ? ' A carriage return is present; the diff keeps line endings verbatim.'
      : '';
    return {
      ok: false,
      message: `module-arcs.txt is STALE: it does not match the current tree `
        + `(committed ${committed.length} bytes, regenerated ${candidate.length} bytes).${crNote}\n\n${diff}\n${REGENERATE}`,
    };
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------------------------------------
// Self-test. Stub runners stand in for the detector and the projection; every file lives in its
// own directory under os.tmpdir(), removed in a finally.

const RECORD_TEXT = 'a -> b\na -> c\nb -> c\n';

// A stub runner: the detector call writes a graph (unless exit is 3), the projection call
// writes `candidate` to its --out.
function stubRunner({ detectorExit = 0, candidate = RECORD_TEXT, version = EXPECTED_TOOL_VERSION } = {}) {
  return async (argv) => {
    const flag = (name) => argv[argv.indexOf(name) + 1];
    if (argv[0] === DETECTOR) {
      if (detectorExit === 0 || detectorExit === 1) {
        fs.writeFileSync(flag('--emit-graph'), JSON.stringify({ tool: { version } }));
      }
      return { exit: detectorExit, stdout: '', stderr: '' };
    }
    fs.writeFileSync(flag('--out'), candidate);
    return { exit: 0, stdout: '', stderr: '' };
  };
}

async function withRecord(bytes, fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'check-module-arcs-self-'));
  try {
    const recordPath = path.join(dir, 'module-arcs.txt');
    if (bytes !== null) fs.writeFileSync(recordPath, bytes);
    return await fn(recordPath);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

async function expectResult(recordBytes, runner, ok) {
  return withRecord(recordBytes, async (recordPath) => {
    const result = await checkModuleArcs({ runner, recordPath });
    if (result.ok !== ok) throw new Error(`expected ok=${ok}, got ok=${result.ok}: ${result.message}`);
    if (!ok && !result.message.includes('npm run record:arcs -- --graph graph.json')) {
      throw new Error('a stale result must print the regeneration commands');
    }
  });
}

async function expectGateError(recordBytes, runner, needle) {
  return withRecord(recordBytes, async (recordPath) => {
    try {
      await checkModuleArcs({ runner, recordPath });
    } catch (err) {
      if (!(err instanceof GateError)) throw err;
      if (!err.message.includes(needle)) throw new Error(`gate error lacks "${needle}": ${err.message}`);
      return;
    }
    throw new Error('expected a gate error, got a result');
  });
}

// Every pair must round-trip through a strict unified-diff parser: headers, @@ counts, context.
function diffFormatCase() {
  const many = Array.from({ length: 40 }, (_, n) => `m${String(n).padStart(2, '0')} -> x\n`);
  const pairs = [
    ['a -> b\n', 'a -> b\na -> c\n'],
    [RECORD_TEXT, 'a -> b\nb -> c\n'],
    [RECORD_TEXT, RECORD_TEXT.replace(/\n/g, '\r\n')],
    [RECORD_TEXT, RECORD_TEXT.slice(0, -1)],
    [many.join(''), [...many.slice(0, 5), 'new -> y\n', ...many.slice(5, 30), ...many.slice(31)].join('')],
    ['', RECORD_TEXT],
  ];
  for (const [oldText, newText] of pairs) {
    const diff = unifiedDiff(oldText, newText);
    if (!/^--- a\/\S+\n\+\+\+ b\/\S+\n@@ -\d+,\d+ \+\d+,\d+ @@\n/.test(diff)) {
      throw new Error(`not a unified diff:\n${diff}`);
    }
    if (/\(line \d+\)/.test(diff)) throw new Error('diff lines must carry the arc text only');
    if (applyUnifiedDiff(oldText, diff) !== newText) throw new Error(`diff does not reproduce the candidate:\n${diff}`);
  }
  const twoHunks = unifiedDiff(pairs[4][0], pairs[4][1]);
  if ((twoHunks.match(/^@@ /gm) || []).length !== 2) throw new Error(`expected two hunks:\n${twoHunks}`);
}

const CASES = [
  ['equal bytes pass', () => expectResult(RECORD_TEXT, stubRunner(), true)],
  ['one arc removed fails', () => expectResult(RECORD_TEXT, stubRunner({ candidate: 'a -> b\nb -> c\n' }), false)],
  ['one arc added fails', () => expectResult(RECORD_TEXT, stubRunner({ candidate: `${RECORD_TEXT}c -> d\n` }), false)],
  ['CRLF against LF fails', () => expectResult(RECORD_TEXT.replace(/\n/g, '\r\n'), stubRunner(), false)],
  ['missing record is a gate error', () => expectGateError(null, stubRunner(), 'missing or unreadable')],
  ['zero-byte record is a gate error', () => expectGateError('', stubRunner(), 'zero bytes')],
  ['detector exit 1 with a written graph passes', () => expectResult(RECORD_TEXT, stubRunner({ detectorExit: 1 }), true)],
  ['detector exit 3 is a gate error', () => expectGateError(RECORD_TEXT, stubRunner({ detectorExit: 3 }), 'detector exit 3')],
  ['detector exit 7 is a gate error naming the code', () => expectGateError(RECORD_TEXT, stubRunner({ detectorExit: 7 }), 'detector exit 7')],
  ['the diff is a valid unified diff that turns the committed record into the candidate', diffFormatCase],
];

async function selfTest() {
  let failed = 0;
  for (const [name, run] of CASES) {
    try {
      await run();
      console.log(`ok   ${name}`);
    } catch (err) {
      failed += 1;
      console.log(`FAIL ${name}: ${err.message}`);
    }
  }
  console.log(`${CASES.length - failed}/${CASES.length} self-test cases passed`);
  return failed === 0 ? 0 : 4;
}

async function main(args) {
  if (args.includes('--help')) {
    console.log(USAGE);
    return 0;
  }
  if (args.includes('--self-test')) return selfTest();
  if (args.length > 0) {
    console.error(`unknown argument: ${args[0]}\n\n${USAGE}`);
    return 2;
  }
  try {
    const result = await checkModuleArcs();
    (result.ok ? console.log : console.error)(result.message);
    return result.ok ? 0 : 1;
  } catch (err) {
    if (!(err instanceof GateError)) throw err;
    console.error(`check-module-arcs: gate error: ${err.message}\n\n${REGENERATE}`);
    return 3;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
