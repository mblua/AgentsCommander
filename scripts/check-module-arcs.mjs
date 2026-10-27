#!/usr/bin/env node

// Generate a validated, ignored record. Cycle policy belongs to check:cycles.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { realRunner, runSelfTest } from './gate-support.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DETECTOR = path.join(ROOT, 'scripts', '01-rust_module-dependency-cycles.mjs');
const PROJECTION = path.join(ROOT, 'scripts', '02-module-arc-record.mjs');
const RECORD = path.join(ROOT, 'src-tauri', 'module-arcs.txt');
const CRATE = path.join(ROOT, 'src-tauri');
const EXPECTED_TOOL_VERSION = '1.1.1';
const USAGE = `Usage: node scripts/check-module-arcs.mjs [--self-test] [--help]
Generates validated ignored src-tauri/module-arcs.txt from the current tree.
Run check:cycles separately. Never commit the generated record.
--self-test runs validation/publication controls; --help prints this usage.`;
class GateError extends Error {}

function arcCount(bytes) {
  let text;
  try {
    text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch { throw new GateError('projection is not valid UTF-8'); }
  if (text === '') return 0;
  if (text.includes('\r') || !text.endsWith('\n')) {
    throw new GateError('projection must use LF lines and a terminal LF');
  }
  const lines = text.slice(0, -1).split('\n');
  let previous;
  for (const line of lines) {
    const pair = line.split(' -> ');
    const invalid = (id) => !id || /\s/u.test(id)
      || id.includes(String.fromCharCode(0));
    if (pair.length !== 2 || pair.some(invalid)) {
      throw new GateError('projection has an invalid arc');
    }
    if (pair[0] === pair[1]) throw new GateError('projection contains a self-arc');
    if (previous !== undefined && previous >= line) {
      throw new GateError('projection must be sorted and unique');
    }
    previous = line;
  }
  return lines.length;
}

export async function checkModuleArcs({
  runner = realRunner, recordPath = RECORD,
  publicationFs = { copyFileSync: fs.copyFileSync, renameSync: fs.renameSync },
} = {}) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'check-module-arcs-'));
  let sibling;
  try {
    const graphPath = path.join(tmp, 'graph.json');
    const candidatePath = path.join(tmp, 'module-arcs.txt');
    const det = await runner([DETECTOR, CRATE, '--emit-graph', graphPath, '--quiet']);
    if (det.exit !== 0 && det.exit !== 1) {
      throw new GateError(`detector exit ${det.exit}: no usable analysis\n${det.stderr}`);
    }
    const graph = JSON.parse(fs.readFileSync(graphPath, 'utf8'));
    if (graph?.tool?.version !== EXPECTED_TOOL_VERSION) {
      throw new GateError(`detector TOOL_VERSION must be ${EXPECTED_TOOL_VERSION}`);
    }
    const proj = await runner([PROJECTION, '--graph', graphPath, '--out', candidatePath]);
    if (proj.exit !== 0) throw new GateError(`projection exit ${proj.exit}\n${proj.stderr}`);
    const candidate = fs.readFileSync(candidatePath);
    const count = arcCount(candidate);
    // Unique sibling reservation, then same-directory rename, never a cross-volume rename.
    const owned = `${recordPath}.${process.pid}.${path.basename(tmp)}.tmp`;
    fs.closeSync(fs.openSync(owned, 'wx'));
    sibling = owned;
    publicationFs.copyFileSync(candidatePath, sibling);
    publicationFs.renameSync(sibling, recordPath);
    return { ok: true,
      message: `Generated ${recordPath}: ${count} arcs, ${candidate.length} bytes.` };
  } catch (err) { throw new GateError(err.message); }
  finally {
    if (sibling) fs.rmSync(sibling, { force: true });
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

const RECORD_TEXT = 'a -> b\na -> c\nb -> c\n';
function stubRunner({ detectorExit = 0, candidate = RECORD_TEXT,
  version = EXPECTED_TOOL_VERSION, graph, projectionExit = 0,
  missing = false, spawnError = false, malformed = false } = {}) {
  return async (argv) => {
    const flag = (name) => argv[argv.indexOf(name) + 1];
    if (spawnError) throw new Error('spawn sentinel');
    if (argv[0] === DETECTOR) {
      if (graph !== null) fs.writeFileSync(flag('--emit-graph'),
        malformed ? '{' : JSON.stringify(graph ?? { tool: { version } }));
      return { exit: detectorExit, stdout: '', stderr: '' };
    }
    if (!missing) fs.writeFileSync(flag('--out'), candidate);
    return { exit: projectionExit, stdout: '', stderr: '' };
  };
}

async function publicationCase(options, failure, prior, publicationFs) {
  if (options === undefined) options = {};
  if (prior === undefined) prior = RECORD_TEXT;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'module-arcs-self-'));
  try {
    const recordPath = path.join(dir, 'module-arcs.txt');
    if (prior !== null) fs.writeFileSync(recordPath, prior);
    let error;
    try {
      const result = await checkModuleArcs({
        runner: stubRunner(options), recordPath, publicationFs,
      });
      if (!result.ok) throw new Error(result.message);
    } catch (err) { error = err; }
    if (failure) {
      if (!(error instanceof GateError) || !error.message.includes(failure)) {
        throw new Error(`expected ${failure}, got ${error?.message ?? 'success'}`);
      }
      if (!fs.readFileSync(recordPath).equals(Buffer.from(prior))) {
        throw new Error('failure changed prior final');
      }
    } else {
      if (error) throw error;
      if (!fs.readFileSync(recordPath).equals(Buffer.from(options.candidate ?? RECORD_TEXT))) {
        throw new Error('published bytes differ');
      }
    }
    if (fs.readdirSync(dir).some((name) => name !== 'module-arcs.txt')) {
      throw new Error('owned sibling temp leaked');
    }
  } finally { fs.rmSync(dir, { recursive: true, force: true }); }
}

const CASES = [
  ['absent final generated', () => publicationCase({}, null, null)],
  ['stale final replaced', () => publicationCase({}, null, 'stale')],
  ['valid DAG arc added', () => publicationCase({ candidate: `${RECORD_TEXT}c -> d\n` })],
  ['valid DAG arc removed', () => publicationCase({ candidate: 'a -> b\n' })],
  ['zero arcs accepted', () => publicationCase({ candidate: '' })],
  ['detector1 accepted', () => publicationCase({ detectorExit: 1 })],
  ['detector3 rejected', () => publicationCase({ detectorExit: 3 }, 'detector exit 3')],
  ['unexpected exit rejected', () => publicationCase({ detectorExit: 7 }, 'detector exit 7')],
  ['spawn failure rejected', () => publicationCase({ spawnError: true }, 'spawn sentinel')],
  ['missing graph rejected', () => publicationCase({ graph: null }, 'ENOENT')],
  ['malformed graph rejected', () => publicationCase({ malformed: true }, 'JSON')],
  ['bad version rejected', () => publicationCase({ version: 'wrong' }, 'TOOL_VERSION')],
  ['projection failure rejected',
    () => publicationCase({ projectionExit: 3 }, 'projection exit')],
  ['missing candidate cannot reuse final',
    () => publicationCase({ missing: true }, 'ENOENT')],
  ['bad UTF8 rejected', () => publicationCase({ candidate: Buffer.from([255]) }, 'UTF-8')],
  ['CRLF rejected', () => publicationCase({ candidate: 'a -> b\r\n' }, 'LF')],
  ['missing terminal LF rejected', () => publicationCase({ candidate: 'a -> b' }, 'LF')],
  ['unsorted rejected', () => publicationCase({ candidate: 'b -> c\na -> b\n' }, 'sorted')],
  ['duplicate rejected', () => publicationCase({ candidate: 'a -> b\na -> b\n' }, 'unique')],
  ['self arc rejected', () => publicationCase({ candidate: 'a -> a\n' }, 'self-arc')],
  ['empty endpoint rejected', () => publicationCase({ candidate: 'a -> \n' }, 'invalid arc')],
  ...['copyFileSync', 'renameSync'].map((operation) => [
    `${operation} sentinel preserves final`,
    () => publicationCase({}, `${operation} sentinel`, RECORD_TEXT, {
      copyFileSync: fs.copyFileSync, renameSync: fs.renameSync,
      [operation]: () => { throw new Error(`${operation} sentinel`); },
    }),
  ]),
];

async function main(args) {
  if (args.includes('--help')) { console.log(USAGE); return 0; }
  if (args.includes('--self-test')) return runSelfTest(CASES);
  if (args.length) { console.error(USAGE); return 2; }
  try {
    const result = await checkModuleArcs();
    console.log(result.message);
    return 0;
  } catch (err) {
    console.error(`check-module-arcs: gate error: ${err.message}`);
    return 3;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
