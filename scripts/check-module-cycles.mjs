#!/usr/bin/env node

// Gate for #2447 (#2464): fails when the current tree has a module dependency cycle that is not
// in the accepted baseline src-tauri/.rust-cycles-baseline.json. A new cycle, or any change to
// the member set of a known one, gives a new cycle id and fails. Cycles already accepted pass.
//
// The committed baseline was produced on Windows under Node v24.13.0. If a later Node or OS
// change moves the cycle id, that is where to start looking.
//
// Known gap until P4: a cycle that disappears still passes here (the detector exits 0), so this
// gate must not be made a required check on its own.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { realRunner, runSelfTest } from './gate-support.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DETECTOR = path.join(ROOT, 'scripts', '01-rust_module-dependency-cycles.mjs');
const TARGET = path.join(ROOT, 'src-tauri');
const BASELINE = path.join(ROOT, 'src-tauri', '.rust-cycles-baseline.json');
const EXPECTED_TOOL_VERSION = '1.1.1';
const FUNCTION_SCOPE = 'cross-module';

const UPDATE = `To accept a new cycle state, from the repo root:

  npm run write:cycles-baseline

then commit src-tauri/.rust-cycles-baseline.json with a "Cycle baseline change" PR section
(one row per event) that answers:
  1. Which modules joined and which left each cycle? Compare the member sets of the retired
     and the new cycle ids in the JSON report; ids alone do not tell growth from shrink.
  2. Why is this state accepted? For a new or grown cycle, name the arc that closed the loop
     and why the alternative was rejected.
Never regenerate the baseline as a reflex to make CI green.`;

const USAGE = `Usage: node scripts/check-module-cycles.mjs [--write-baseline | --self-test | --help]

Runs the dependency-cycle detector on src-tauri against src-tauri/.rust-cycles-baseline.json
and fails when a module cycle is new against the baseline.

  --write-baseline  Regenerate the baseline from the current tree, under the gate's settings.
  --self-test       Run the embedded self-test.
  --help            Print this usage and exit 0.`;

// `code` is this script's exit code: 2 for a bad baseline or invocation, 3 for everything else
// that leaves no trustworthy verdict.
class GateError extends Error {
  constructor(message, code = 3) {
    super(message);
    this.code = code;
  }
}

// The two argv lists are built separately: --baseline and --write-baseline are mutually
// exclusive in the detector. Both share the settings the wrapper owns: the target,
// --fail-on module, --function-scope cross-module, no tests and no excludes.
function gateArgv(target, baselinePath) {
  return [DETECTOR, target, '--baseline', baselinePath, '--fail-on', 'module', '--function-scope', FUNCTION_SCOPE, '--json'];
}

function writeArgv(target, baselinePath) {
  return [DETECTOR, target, '--write-baseline', baselinePath, '--fail-on', 'module', '--function-scope', FUNCTION_SCOPE];
}

// Asserts only the fields schema 1 stores. --fail-on and --exclude are not stored, so the
// wrapper holds them fixed instead.
function readBaseline(baselinePath) {
  let bytes;
  try {
    bytes = fs.readFileSync(baselinePath);
  } catch (err) {
    throw new GateError(`baseline ${baselinePath} is missing or unreadable: ${err.message}`, 2);
  }
  if (bytes.length === 0) throw new GateError(`baseline ${baselinePath} is zero bytes`, 2);
  let baseline;
  try {
    baseline = JSON.parse(bytes.toString('utf8'));
  } catch (err) {
    throw new GateError(`baseline ${baselinePath} is not valid JSON: ${err.message}`, 2);
  }
  const from = baseline?.generatedFrom || {};
  const checks = [
    ['kind', baseline?.kind, 'rust-cycles-baseline'],
    ['schemaVersion', baseline?.schemaVersion, 1],
    ['generatedFrom.toolVersion', from.toolVersion, EXPECTED_TOOL_VERSION],
    ['generatedFrom.functionScope', from.functionScope, FUNCTION_SCOPE],
    ['generatedFrom.includeTests', from.includeTests, false],
  ];
  for (const [field, actual, expected] of checks) {
    if (actual !== expected) {
      throw new GateError(
        `baseline ${field} is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}; `
          + 'a settings change or detector bump needs a regenerated baseline',
        2,
      );
    }
  }
  return baseline;
}

function unusable(detail, stderr) {
  return new GateError(`the detector report is unusable (${detail}); this is not a verdict about the tree\n${stderr}`);
}

function parseReport(stdout, stderr) {
  if (!stdout || stdout.trim() === '') throw unusable('empty stdout', stderr);
  let report;
  try {
    report = JSON.parse(stdout);
  } catch (err) {
    throw unusable(`stdout is not JSON: ${err.message}`, stderr);
  }
  const version = report?.tool?.version;
  if (!report || report.schemaVersion !== 1) throw unusable(`schemaVersion is ${JSON.stringify(report?.schemaVersion)}`, stderr);
  if (version !== EXPECTED_TOOL_VERSION) {
    throw unusable(`detector TOOL_VERSION is ${JSON.stringify(version)}, expected '${EXPECTED_TOOL_VERSION}'`, stderr);
  }
  if (!Array.isArray(report.moduleCycles)) throw unusable('moduleCycles is not an array', stderr);
  if (report.baseline?.used !== true) throw unusable('baseline.used is not true', stderr);
  if (!Array.isArray(report.baseline.resolvedCycles)) throw unusable('baseline.resolvedCycles is not an array', stderr);
  return report;
}

// Settles every process outcome that leaves no trustworthy report before anything is classified.
async function runDetector(runner, argv) {
  let result;
  try {
    result = await runner(argv);
  } catch (err) {
    throw new GateError(`could not start the detector: ${err.message}`);
  }
  if (result.exit === 2) throw new GateError(`detector exit 2: bad baseline or invocation\n${result.stderr}`, 2);
  if (result.exit === 3) throw new GateError(`detector exit 3: analysis incomplete, never a pass\n${result.stderr}`);
  if (result.exit !== 0 && result.exit !== 1) {
    throw new GateError(`detector exit ${result.exit}: unexpected exit code (only 0 and 1 are accepted)\n${result.stderr}`);
  }
  return result;
}

function describeCycle(cycle) {
  const members = Array.isArray(cycle.members) ? cycle.members : [];
  const memberLines = members.map((m) => `    ${m}`).join('\n');
  return `  ${cycle.id} (${members.length} members)\n${memberLines}`;
}

function blindSpotLines(report) {
  const spots = report.blindSpots || {};
  const lines = [
    `blind spots: unresolvedInternalPaths ${spots.unresolvedInternalPaths}, ambiguousBarePaths ${spots.ambiguousBarePaths}`,
  ];
  if (spots.unresolvedInternalPaths !== 0 || spots.ambiguousBarePaths !== 0) {
    lines.push('warning: a non-zero blind-spot counter makes the module-level verdict less reliable; read it.');
  }
  return lines;
}

// Every list the event lines print is sorted by UTF-16 code unit, so a re-run prints the same
// bytes whatever order the detector emitted.
function compareCodeUnits(a, b) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

const sortedList = (items) => Array.from(items).sort(compareCodeUnits);

// True when every cycle in `inner` is a subset of some single cycle in `outer`.
function eachInsideOne(inner, outer) {
  return inner.every((cycle) => outer.some((o) => cycle.members.every((m) => o.memberSet.has(m))));
}

// Compares partitions, never unions or cycle counts: SHRINK is refinement, GROWN is coarsening.
function labelEvent(rs, ns) {
  if (rs.length === 0) return 'NEW';
  if (ns.length === 0) return 'CYCLE REMOVED';
  if (eachInsideOne(ns, rs)) return 'SCC SHRINK';
  if (eachInsideOne(rs, ns)) return 'SCC GROWN';
  return 'SCC SWAP';
}

// Groups retired (R) and new (N) module cycles into events: the connected components of the
// "member sets intersect" relation. Returns the events sorted as the contract fixes.
export function classifyEvents(resolved, added) {
  const nodes = [
    ...resolved.map((c) => ({ side: 'r', id: c.id, members: c.members, memberSet: new Set(c.members) })),
    ...added.map((c) => ({ side: 'n', id: c.id, members: c.members, memberSet: new Set(c.members) })),
  ];
  const parent = nodes.map((_, i) => i);
  const find = (i) => {
    let root = i;
    while (parent[root] !== root) root = parent[root];
    return root;
  };
  const owner = new Map();
  nodes.forEach((node, i) => {
    for (const m of node.members) {
      if (owner.has(m)) parent[find(i)] = find(owner.get(m));
      else owner.set(m, i);
    }
  });
  const groups = new Map();
  nodes.forEach((node, i) => {
    const root = find(i);
    if (!groups.has(root)) groups.set(root, { rs: [], ns: [] });
    groups.get(root)[node.side === 'r' ? 'rs' : 'ns'].push(node);
  });
  const events = Array.from(groups.values()).map(({ rs, ns }) => {
    const rUnion = new Set(rs.flatMap((c) => c.members));
    const nUnion = new Set(ns.flatMap((c) => c.members));
    return {
      label: labelEvent(rs, ns),
      retired: sortedList(rs.map((c) => c.id)),
      added: sortedList(ns.map((c) => c.id)),
      left: sortedList([...rUnion].filter((m) => !nUnion.has(m))),
      joined: sortedList([...nUnion].filter((m) => !rUnion.has(m))),
      firstMember: sortedList([...rUnion, ...nUnion])[0],
      firstId: sortedList([...rs, ...ns].map((c) => c.id))[0],
    };
  });
  return events.sort((a, b) => compareCodeUnits(a.firstMember, b.firstMember) || compareCodeUnits(a.firstId, b.firstId));
}

export function formatEvent(event) {
  const list = (items) => `[${items.join(', ')}]`;
  return `${event.label}: retired ${list(event.retired)} new ${list(event.added)} left ${list(event.left)} joined ${list(event.joined)}`;
}

const REMOVAL_REASON = 'A cycle in the baseline no longer exists. Leaving it there would read that exact cycle as '
  + '`known` if it ever returns, so the removal must be pruned from the baseline now.';

// Gate mode. `target` and `baselinePath` are the private injection seam: the command line never
// sets them. Returns { ok, exit, message, events, detectorExit, newCycles, resolvedCycles };
// throws GateError.
export async function checkModuleCycles({ runner = realRunner, target = TARGET, baselinePath = BASELINE } = {}) {
  const baseline = readBaseline(baselinePath);
  const result = await runDetector(runner, gateArgv(target, baselinePath));
  const report = parseReport(result.stdout, result.stderr);

  // Module graph only: resolvedCycles carries function-graph entries too, and --fail-on
  // filters neither.
  const resolvedCycles = report.baseline.resolvedCycles.filter((c) => c?.graph === 'module');
  const newCycles = report.moduleCycles.filter((c) => c.status === 'new');
  const events = classifyEvents(resolvedCycles, newCycles).map(formatEvent);
  const removed = events.some((line) => line.startsWith('CYCLE REMOVED:'));
  // A removal overrides the detector, even when a new cycle already made it exit 1.
  const exit = removed ? 3 : result.exit;
  const known = report.moduleCycles.filter((c) => c.status === 'known');
  const knownList = known.map((c) => `${c.id} (${c.members.length} members)`).join(', ');
  const summary = [
    `baseline: ${baselinePath} (${baseline.moduleCycles.length} module cycles)`,
    `known: ${knownList || 'none'}`,
    `new: ${newCycles.length}`,
    `resolvedCycles: ${JSON.stringify(resolvedCycles.map((c) => c.id))}`,
    ...blindSpotLines(report),
  ];
  const outcome = { exit, events, detectorExit: result.exit, newCycles, resolvedCycles, stderr: result.stderr };
  if (exit === 0) {
    return { ...outcome, ok: true, message: `no new module dependency cycle.\n${summary.join('\n')}` };
  }
  const headline = removed
    ? `check-module-cycles: gate error: ${REMOVAL_REASON}`
    : 'a module dependency cycle is new against the baseline.';
  const details = [
    'new module cycles:',
    ...newCycles.map(describeCycle),
    'retired cycles (ids no longer present):',
    ...(resolvedCycles.length > 0 ? resolvedCycles.map(describeCycle) : ['  none']),
  ];
  return { ...outcome, ok: false, message: `${headline}\n${summary.join('\n')}\n${details.join('\n')}` };
}

// Write mode. Never reads an existing baseline, so the first write works.
export async function writeCyclesBaseline({ runner = realRunner, target = TARGET, baselinePath = BASELINE } = {}) {
  let result;
  try {
    result = await runner(writeArgv(target, baselinePath));
  } catch (err) {
    throw new GateError(`could not start the detector: ${err.message}`);
  }
  if (result.exit !== 0) throw new GateError(`detector exit ${result.exit}: baseline not written\n${result.stderr}`);
  const baseline = readBaseline(baselinePath);
  return `wrote ${baselinePath}: ${baseline.moduleCycles.length} module cycles, ${baseline.functionCycles.length} function cycles.`;
}

// The single failure printer: the message, one line per cycle event, the detector's stderr,
// then the update command and questions.
function printFailure(message, { stderr = '', events = [] } = {}) {
  const parts = [message];
  if (events.length > 0) parts.push(events.join('\n'));
  if (stderr) parts.push(`detector stderr:\n${stderr.trimEnd()}`);
  parts.push(UPDATE);
  console.error(parts.join('\n\n'));
}

// ---------------------------------------------------------------------------------------------
// Self-test. Stub runners stand in for the detector, except in the cases marked real. Every file
// lives in its own directory under os.tmpdir(), removed in a finally.

const GOOD_BASELINE = {
  schemaVersion: 1,
  kind: 'rust-cycles-baseline',
  generatedFrom: { toolVersion: EXPECTED_TOOL_VERSION, functionScope: FUNCTION_SCOPE, includeTests: false },
  moduleCycles: [{ id: 'aaaaaaaaaaaaaaaa', members: ['crate::a', 'crate::b'] }],
  functionCycles: [],
};

function stubReport({ moduleCycles = [{ id: 'aaaaaaaaaaaaaaaa', status: 'known', members: ['crate::a', 'crate::b'] }], resolvedCycles = [] } = {}) {
  return {
    schemaVersion: 1,
    tool: { name: 'rust-module-dependency-cycles', version: EXPECTED_TOOL_VERSION },
    moduleCycles,
    blindSpots: { unresolvedInternalPaths: 0, ambiguousBarePaths: 0 },
    baseline: { used: true, resolvedCycles },
  };
}

function stubRunner({ exit = 0, stdout = JSON.stringify(stubReport()), seen = null } = {}) {
  return async (argv) => {
    if (seen) seen.push(argv);
    return { exit, stdout, stderr: 'warn: stub\n' };
  };
}

async function withTemp(fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'check-module-cycles-self-'));
  try {
    return await fn(dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

async function withBaseline(content, fn) {
  return withTemp(async (dir) => {
    const baselinePath = path.join(dir, '.rust-cycles-baseline.json');
    if (content !== null) fs.writeFileSync(baselinePath, typeof content === 'string' ? content : JSON.stringify(content));
    return fn(baselinePath, dir);
  });
}

async function expectGateError(content, runner, needle) {
  return withBaseline(content, async (baselinePath) => {
    try {
      await checkModuleCycles({ runner, baselinePath, target: path.dirname(baselinePath) });
    } catch (err) {
      if (!(err instanceof GateError)) throw err;
      if (!err.message.includes(needle)) throw new Error(`gate error lacks "${needle}": ${err.message}`);
      if (/no new module dependency cycle/.test(err.message)) throw new Error('a gate error must not read as a clean tree');
      return;
    }
    throw new Error('expected a gate error, got a result');
  });
}

function withField(mutate) {
  const copy = structuredClone(GOOD_BASELINE);
  mutate(copy);
  return copy;
}

// A two-module crate with one module cycle, written under a temp directory.
function writeFixtureCrate(dir) {
  const src = path.join(dir, 'crate', 'src');
  fs.mkdirSync(src, { recursive: true });
  fs.writeFileSync(path.join(dir, 'crate', 'Cargo.toml'), '[package]\nname = "fixture"\nversion = "0.1.0"\n\n[lib]\npath = "src/lib.rs"\n');
  fs.writeFileSync(path.join(src, 'lib.rs'), 'pub mod a;\npub mod b;\n');
  fs.writeFileSync(path.join(src, 'a.rs'), 'use crate::b::g;\npub fn f() { g(); }\n');
  fs.writeFileSync(path.join(src, 'b.rs'), 'use crate::a::f;\npub fn g() { f(); }\n');
  return path.join(dir, 'crate');
}

const CASES = [
  // CI runs the self-test before the gate, so a real regression stops here first: print the
  // same guidance the gate would, so the author still gets it.
  ['real detector, real tree, committed baseline: passes', async () => {
    let result;
    try {
      result = await checkModuleCycles();
    } catch (err) {
      if (err instanceof GateError) printFailure(`check-module-cycles: gate error: ${err.message}`);
      throw err;
    }
    if (!result.ok) {
      printFailure(result.message, { stderr: result.stderr, events: result.events });
      throw new Error('the real tree does not match the committed baseline; see the guidance above');
    }
  }],
  ['gate argv is fixed and carries --json, never --write-baseline', () => withBaseline(GOOD_BASELINE, async (baselinePath) => {
    const seen = [];
    await checkModuleCycles({ runner: stubRunner({ seen }), baselinePath, target: 'T' });
    const want = [DETECTOR, 'T', '--baseline', baselinePath, '--fail-on', 'module', '--function-scope', FUNCTION_SCOPE, '--json'];
    if (JSON.stringify(seen[0]) !== JSON.stringify(want)) throw new Error(`argv ${JSON.stringify(seen[0])}`);
  })],
  ['stub exit 0 with a known cycle passes', () => withBaseline(GOOD_BASELINE, async (baselinePath) => {
    const result = await checkModuleCycles({ runner: stubRunner(), baselinePath });
    if (!result.ok || !result.message.includes('new: 0')) throw new Error(result.message);
  })],
  ['stub exit 1 with a new cycle fails and names its members', () => withBaseline(GOOD_BASELINE, async (baselinePath) => {
    const stdout = JSON.stringify(stubReport({
      moduleCycles: [{ id: 'bbbbbbbbbbbbbbbb', status: 'new', members: ['crate::a', 'crate::b', 'crate::c'] }],
      resolvedCycles: [{ graph: 'module', id: 'aaaaaaaaaaaaaaaa', members: ['crate::a', 'crate::b'] }],
    }));
    const result = await checkModuleCycles({ runner: stubRunner({ exit: 1, stdout }), baselinePath });
    if (result.ok || !result.message.includes('bbbbbbbbbbbbbbbb') || !result.message.includes('crate::c')) throw new Error(result.message);
  })],
  ['function-graph resolvedCycles entries are filtered out', () => withBaseline(GOOD_BASELINE, async (baselinePath) => {
    const stdout = JSON.stringify(stubReport({
      resolvedCycles: [{ graph: 'function', id: 'ffffffffffffffff', members: ['x'] }, { graph: 'module', id: 'cccccccccccccccc', members: ['y'] }],
    }));
    const result = await checkModuleCycles({ runner: stubRunner({ stdout }), baselinePath });
    const ids = result.resolvedCycles.map((c) => c.id);
    if (JSON.stringify(ids) !== '["cccccccccccccccc"]') throw new Error(`resolved ${JSON.stringify(ids)}`);
  })],
  ['stub exit 3 is a gate error', () => expectGateError(GOOD_BASELINE, stubRunner({ exit: 3 }), 'detector exit 3')],
  ['stub exit 7 is a gate error naming the code', () => expectGateError(GOOD_BASELINE, stubRunner({ exit: 7 }), 'detector exit 7')],
  ['spawn failure is a gate error', () => expectGateError(GOOD_BASELINE, async () => { throw new Error('ENOENT'); }, 'could not start')],
  ['exit 0 with empty stdout is an unusable report', () => expectGateError(GOOD_BASELINE, stubRunner({ stdout: '' }), 'unusable')],
  ['exit 0 with unparseable stdout is an unusable report', () => expectGateError(GOOD_BASELINE, stubRunner({ stdout: 'not json' }), 'unusable')],
  ['exit 0 without baseline.resolvedCycles is an unusable report', () => {
    const report = stubReport();
    delete report.baseline.resolvedCycles;
    return expectGateError(GOOD_BASELINE, stubRunner({ stdout: JSON.stringify(report) }), 'unusable');
  }],
  ['a report from another detector version is unusable', () => {
    const report = stubReport();
    report.tool.version = '1.0.0';
    return expectGateError(GOOD_BASELINE, stubRunner({ stdout: JSON.stringify(report) }), 'TOOL_VERSION');
  }],
  ['missing baseline is a gate error', () => expectGateError(null, stubRunner(), 'missing or unreadable')],
  ['zero-byte baseline is a gate error', () => expectGateError('', stubRunner(), 'zero bytes')],
  ['wrong kind is a gate error', () => expectGateError(withField((b) => { b.kind = 'other'; }), stubRunner(), 'kind')],
  ['wrong schemaVersion is a gate error', () => expectGateError(withField((b) => { b.schemaVersion = 2; }), stubRunner(), 'schemaVersion')],
  ['wrong generatedFrom.toolVersion is a gate error', () => expectGateError(withField((b) => { b.generatedFrom.toolVersion = '1.1.0'; }), stubRunner(), 'toolVersion')],
  ['metadata mismatch: functionScope module is a gate error', () => expectGateError(withField((b) => { b.generatedFrom.functionScope = 'module'; }), stubRunner(), 'functionScope')],
  ['wrong generatedFrom.includeTests is a gate error', () => expectGateError(withField((b) => { b.generatedFrom.includeTests = true; }), stubRunner(), 'includeTests')],
  ['write argv never carries --baseline', () => withBaseline(null, async (baselinePath) => {
    const seen = [];
    const runner = async (argv) => {
      seen.push(argv);
      fs.writeFileSync(baselinePath, JSON.stringify(GOOD_BASELINE));
      return { exit: 0, stdout: '', stderr: '' };
    };
    await writeCyclesBaseline({ runner, baselinePath, target: 'T' });
    if (seen[0].includes('--baseline') || !seen[0].includes('--write-baseline')) throw new Error(`argv ${JSON.stringify(seen[0])}`);
  })],
  ['real detector through the seam: first write on a temp crate, then the gate passes and a new cycle fails', () => withTemp(async (dir) => {
    const target = writeFixtureCrate(dir);
    const baselinePath = path.join(dir, 'baseline.json');
    await writeCyclesBaseline({ target, baselinePath });
    const written = readBaseline(baselinePath);
    if (written.moduleCycles.length !== 1) throw new Error(`expected one fixture cycle, got ${written.moduleCycles.length}`);
    const pass = await checkModuleCycles({ target, baselinePath });
    if (!pass.ok) throw new Error(pass.message);
    fs.writeFileSync(path.join(target, 'src', 'lib.rs'), 'pub mod a;\npub mod b;\npub mod c;\n');
    fs.writeFileSync(path.join(target, 'src', 'c.rs'), 'use crate::a::f;\npub fn h() { f(); }\n');
    fs.writeFileSync(path.join(target, 'src', 'a.rs'), 'use crate::b::g;\nuse crate::c::h;\npub fn f() { g(); h(); }\n');
    const fail = await checkModuleCycles({ target, baselinePath });
    if (fail.ok || fail.newCycles.length !== 1 || fail.resolvedCycles.length !== 1) throw new Error(fail.message);
  })],
];

// ---------------------------------------------------------------------------------------------
// Classification fixture pairs (#2465). Each row writes its own predecessor baseline from state X
// with the real detector, mutates the crate to state Y, then runs the gate against that baseline.

const FIXTURE_MODULES = ['a', 'b', 'c', 'd', 'e', 'f', 'p', 'q', 'r', 's'];

// Every module is declared in lib.rs in every state; each loop is a ring of `use` arcs.
function writeLoops(crate, loops) {
  const uses = new Map(FIXTURE_MODULES.map((m) => [m, []]));
  for (const loop of loops) loop.forEach((m, i) => uses.get(m).push(loop[(i + 1) % loop.length]));
  fs.mkdirSync(path.join(crate, 'src'), { recursive: true });
  fs.writeFileSync(path.join(crate, 'Cargo.toml'), '[package]\nname = "fixture"\nversion = "0.1.0"\n\n[lib]\npath = "src/lib.rs"\n');
  fs.writeFileSync(path.join(crate, 'src', 'lib.rs'), FIXTURE_MODULES.map((m) => `pub mod ${m};\n`).join(''));
  for (const m of FIXTURE_MODULES) {
    const lines = uses.get(m).map((t) => `use crate::${t}::f as _${t};\n`);
    fs.writeFileSync(path.join(crate, 'src', `${m}.rs`), `${lines.join('')}pub fn f() {}\n`);
  }
}

// Short module name: the last path segment.
const shortName = (member) => member.slice(member.lastIndexOf(':') + 1);

function reportEvents(report) {
  const resolved = report.baseline.resolvedCycles.filter((c) => c?.graph === 'module');
  return classifyEvents(resolved, report.moduleCycles.filter((c) => c.status === 'new'));
}

async function runPair(before, after, editBaseline) {
  return withTemp(async (dir) => {
    const target = path.join(dir, 'crate');
    const baselinePath = path.join(dir, 'baseline.json');
    writeLoops(target, before);
    await writeCyclesBaseline({ target, baselinePath });
    if (editBaseline) editBaseline(baselinePath);
    writeLoops(target, after);
    let stdout = '';
    const runner = async (argv) => {
      const r = await realRunner(argv);
      stdout = r.stdout;
      return r;
    };
    const result = await checkModuleCycles({ target, baselinePath, runner });
    return { result, report: JSON.parse(stdout) };
  });
}

// `want` holds one [label, left, joined, retired id count, new id count] per event, in order.
async function expectPair({ before, after, want, detector, gate, editBaseline }) {
  const { result, report } = await runPair(before, after, editBaseline);
  if (result.detectorExit !== detector) throw new Error(`detector exit ${result.detectorExit}, expected ${detector}`);
  if (result.exit !== gate) throw new Error(`gate exit ${result.exit}, expected ${gate}`);
  if (result.events.length !== want.length) throw new Error(`${result.events.length} event lines, expected ${want.length}: ${result.events.join(' | ')}`);
  const got = reportEvents(report).map((e) => [e.label, e.left.map(shortName).join(','), e.joined.map(shortName).join(','), e.retired.length, e.added.length]);
  if (JSON.stringify(got) !== JSON.stringify(want)) throw new Error(`events ${JSON.stringify(got)}, expected ${JSON.stringify(want)}`);
  return { result, report };
}

function permutations(items) {
  if (items.length <= 1) return [items];
  return items.flatMap((item, i) => permutations([...items.slice(0, i), ...items.slice(i + 1)]).map((rest) => [item, ...rest]));
}

// Every order of both arrays, every member list reversed: the printed lines must not move.
function assertPermutationInvariant(report) {
  const expected = reportEvents(report).map(formatEvent).join('\n');
  const reversed = (cycles) => cycles.map((c) => ({ ...c, members: [...c.members].reverse() }));
  for (const moduleCycles of permutations(reversed(report.moduleCycles))) {
    for (const resolvedCycles of permutations(reversed(report.baseline.resolvedCycles))) {
      const shuffled = { ...report, moduleCycles, baseline: { ...report.baseline, resolvedCycles } };
      const got = reportEvents(shuffled).map(formatEvent).join('\n');
      if (got !== expected) throw new Error(`order moved:\n${got}\nexpected:\n${expected}`);
    }
  }
}

function addStaleFunctionCycle(baselinePath) {
  const baseline = JSON.parse(fs.readFileSync(baselinePath, 'utf8'));
  baseline.functionCycles.push({ id: '0123456789abcdef', members: ['fixture::a::f', 'fixture::b::f'] });
  fs.writeFileSync(baselinePath, JSON.stringify(baseline));
}

const PAIRS = [
  ['1 new loop is NEW', { before: [], after: [['a', 'b', 'c']], want: [['NEW', '', 'a,b,c', 0, 1]], detector: 1, gate: 1 }],
  ['2 growth is SCC GROWN', { before: [['a', 'b', 'c']], after: [['a', 'b', 'c', 'd']], want: [['SCC GROWN', '', 'd', 1, 1]], detector: 1, gate: 1 }],
  ['3 shrink is SCC SHRINK', { before: [['a', 'b', 'c', 'd']], after: [['a', 'b', 'c']], want: [['SCC SHRINK', 'd', '', 1, 1]], detector: 1, gate: 1 }],
  ['4 one out one in is SCC SWAP', { before: [['a', 'b', 'c']], after: [['a', 'b', 'd']], want: [['SCC SWAP', 'c', 'd', 1, 1]], detector: 1, gate: 1 }],
  ['5 removal is a gate error though the detector exits 0', { before: [['a', 'b', 'd']], after: [], want: [['CYCLE REMOVED', 'a,b,d', '', 1, 0]], detector: 0, gate: 3 }],
  ['6 split is one SCC SHRINK', { before: [['a', 'b', 'c', 'd']], after: [['a', 'b'], ['c', 'd']], want: [['SCC SHRINK', '', '', 1, 2]], detector: 1, gate: 1 }],
  ['7 merge is one SCC GROWN', { before: [['a', 'b', 'c'], ['d', 'e']], after: [['a', 'b', 'c', 'd', 'e']], want: [['SCC GROWN', '', '', 2, 1]], detector: 1, gate: 1 }],
  ['8 known plus new is one NEW line', { before: [['a', 'b', 'c']], after: [['a', 'b', 'c'], ['d', 'e']], want: [['NEW', '', 'd,e', 0, 1]], detector: 1, gate: 1 }],
  ['9 a stale function cycle makes no event', { before: [['a', 'b', 'c']], after: [['a', 'b', 'c']], want: [], detector: 0, gate: 0, editBaseline: addStaleFunctionCycle }],
  ['10 re-partition is one SCC SWAP', { before: [['a', 'b'], ['c', 'd']], after: [['a', 'c'], ['b', 'd']], want: [['SCC SWAP', '', '', 2, 2]], detector: 1, gate: 1 }],
  ['11 removal plus unrelated new: both lines, gate error over exit 1', { before: [['a', 'b', 'c']], after: [['d', 'e']], want: [['CYCLE REMOVED', 'a,b,c', '', 1, 0], ['NEW', '', 'd,e', 0, 1]], detector: 1, gate: 3 }],
  ['12 shrink plus unrelated new is two events', { before: [['a', 'b', 'c']], after: [['a', 'b'], ['d', 'e']], want: [['SCC SHRINK', 'c', '', 1, 1], ['NEW', '', 'd,e', 0, 1]], detector: 1, gate: 1 }],
  ['14 cross coupling with equal unions is SCC SWAP', { before: [['a', 'b', 'c'], ['d', 'e', 'f']], after: [['a', 'd'], ['b', 'e'], ['c', 'f']], want: [['SCC SWAP', '', '', 2, 3]], detector: 1, gate: 1 }],
  ['15 coupling into a smaller union is SCC SWAP', { before: [['a', 'b'], ['c', 'd']], after: [['a', 'c']], want: [['SCC SWAP', 'b,d', '', 2, 1]], detector: 1, gate: 1 }],
];

CASES.push(
  ...PAIRS.map(([name, row]) => [`pair ${name}`, () => expectPair(row)]),
  ['pair 13 two events, output invariant under every permutation', async () => {
    const { report } = await expectPair({
      before: [['a', 'b', 'c'], ['d', 'e'], ['p', 'q', 'r', 's']],
      after: [['a', 'b', 'c', 'd', 'e'], ['p', 'q'], ['r', 's']],
      want: [['SCC GROWN', '', '', 2, 1], ['SCC SHRINK', '', '', 1, 2]],
      detector: 1,
      gate: 1,
    });
    assertPermutationInvariant(report);
  }],
);

const selfTest = () => runSelfTest(CASES);

async function main(args) {
  if (args.includes('--help')) {
    console.log(USAGE);
    return 0;
  }
  if (args.length > 1) {
    console.error(`too many arguments\n\n${USAGE}`);
    return 2;
  }
  if (args[0] === '--self-test') return selfTest();
  try {
    if (args[0] === '--write-baseline') {
      console.log(await writeCyclesBaseline());
      return 0;
    }
    if (args.length > 0) {
      console.error(`unknown argument: ${args[0]}\n\n${USAGE}`);
      return 2;
    }
    const result = await checkModuleCycles();
    if (result.ok) {
      console.log(result.message);
      return 0;
    }
    printFailure(result.message, { stderr: result.stderr, events: result.events });
    return result.exit;
  } catch (err) {
    if (!(err instanceof GateError)) throw err;
    printFailure(`check-module-cycles: gate error: ${err.message}`);
    return err.code;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
