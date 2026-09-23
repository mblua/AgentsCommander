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
const EXPECTED_TOOL_VERSION = '1.1.0';

const REGENERATE = `To regenerate the record, from the repo root:

  node scripts/01-rust_module-dependency-cycles.mjs src-tauri --emit-graph graph.json --quiet
  npm run record:arcs -- --graph graph.json

then delete graph.json (it is gitignored and must never be committed) and commit
src-tauri/module-arcs.txt. The detector exits 1 when it finds cycles; that is expected.`;

const USAGE = `Usage: node scripts/check-module-arcs.mjs [--self-test] [--help]

Regenerates the module-arc record from the current tree into a temp directory and
fails when it differs from src-tauri/module-arcs.txt byte for byte.

  --self-test  Run the embedded self-test (9 cases).
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

// Line-level diff (LCS). Carriage returns are shown as \r so an EOL-only difference is visible.
export function unifiedDiff(oldText, newText) {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  const n = a.length;
  const m = b.length;
  const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const show = (s) => s.replace(/\r/g, '\\r');
  const out = ['--- committed src-tauri/module-arcs.txt', '+++ regenerated from the current tree'];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      i += 1;
      j += 1;
    } else if (j < m && (i === n || lcs[i][j + 1] >= lcs[i + 1][j])) {
      out.push(`+${show(b[j])}  (line ${j + 1})`);
      j += 1;
    } else {
      out.push(`-${show(a[i])}  (line ${i + 1})`);
      i += 1;
    }
  }
  if (out.length === 2) out.push('(no line differs; the byte difference is in line endings or the final newline)');
  return out.join('\n');
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
    return {
      ok: false,
      message: `module-arcs.txt is STALE: it does not match the current tree `
        + `(committed ${committed.length} bytes, regenerated ${candidate.length} bytes).\n\n${diff}\n\n${REGENERATE}`,
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
