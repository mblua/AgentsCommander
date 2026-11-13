// Flaky-test observation records (#4527). Turns one CI test report into a
// JSONL record: a header line, then one {test_id, outcome} line per test.
// Outcomes are pass | fail | skip | not_run. Anything that is not a complete,
// trusted report is written as not_run, never as pass.
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

export const SCHEMA = 1;

const MAX_ID_LENGTH = 512;
const RANK = { fail: 3, pass: 2, skip: 1, not_run: 0 };
const VITEST_STATUS = {
  passed: 'pass',
  failed: 'fail',
  skipped: 'skip',
  pending: 'skip',
  todo: 'skip',
};

function str(value) {
  return typeof value === 'string' ? value : '';
}

export function identityFromEnv(env, job) {
  const os = str(env.RUNNER_OS);
  const arch = str(env.RUNNER_ARCH);
  return {
    platform: os && arch ? `${os}-${arch}` : 'unknown',
    job: str(job),
    run_id: str(env.GITHUB_RUN_ID),
    run_attempt: str(env.GITHUB_RUN_ATTEMPT),
    sha: str(env.GITHUB_SHA),
    head_sha: str(env.PR_HEAD),
    event: str(env.GITHUB_EVENT_NAME),
    repository: str(env.GITHUB_REPOSITORY),
  };
}

function safeId(id) {
  if (id.length > MAX_ID_LENGTH) return false;
  for (const ch of id) {
    const code = ch.codePointAt(0);
    if (code < 0x20 || code === 0x7f) return false;
  }
  return true;
}

export function collapseOutcomes(entries) {
  const byId = new Map();
  let collapsed = 0;
  for (const entry of entries) {
    const seen = byId.get(entry.test_id);
    if (seen === undefined) {
      byId.set(entry.test_id, entry.outcome);
      continue;
    }
    collapsed += 1;
    if (RANK[entry.outcome] > RANK[seen]) byId.set(entry.test_id, entry.outcome);
  }
  const tests = [...byId].map(([test_id, outcome]) => ({ test_id, outcome }));
  return { tests, collapsed };
}

function relativeFile(root, name) {
  if (typeof name !== 'string' || name === '') return '?';
  const relative = path.relative(root, name);
  if (relative === '' || path.isAbsolute(relative)) return '?';
  const parts = relative.split(/[\\/]/);
  if (parts.includes('..')) return '?';
  return parts.join('/');
}

function testName(assertion) {
  if (typeof assertion.fullName === 'string' && assertion.fullName !== '') {
    return assertion.fullName;
  }
  const ancestors = Array.isArray(assertion.ancestorTitles) ? assertion.ancestorTitles : [];
  return [...ancestors, assertion.title].filter(Boolean).join(' > ');
}

function isComplete(report, count) {
  const total = report.numTotalTests;
  return Number.isInteger(total) && total >= 0 && count === total;
}

function headerFor(identity, source, collapsed, rejected) {
  return {
    schema: SCHEMA,
    kind: 'header',
    suite: 'vitest',
    ...identity,
    sources: [source],
    collapsed,
    rejected,
  };
}

export function vitestRecord(report, { root, identity, file = 'npm-test-results.json' }) {
  const results = report && Array.isArray(report.testResults) ? report.testResults : null;
  if (results === null) {
    const source = { file, present: true, complete: false, tests: 0 };
    return { header: headerFor(identity, source, 0, 0), tests: [] };
  }
  const entries = [];
  let count = 0;
  let rejected = 0;
  for (const suite of results) {
    const fileId = relativeFile(root, suite?.name);
    const assertions = Array.isArray(suite?.assertionResults) ? suite.assertionResults : [];
    for (const assertion of assertions) {
      count += 1;
      const test_id = `${fileId}::${testName(assertion ?? {})}`;
      if (!safeId(test_id)) {
        rejected += 1;
        continue;
      }
      const status = assertion?.status;
      const outcome = Object.hasOwn(VITEST_STATUS, status) ? VITEST_STATUS[status] : 'not_run';
      entries.push({ test_id, outcome });
    }
  }
  const complete = isComplete(report, count);
  const { tests, collapsed } = collapseOutcomes(entries);
  if (!complete) for (const test of tests) test.outcome = 'not_run';
  const source = { file, present: true, complete, tests: tests.length };
  return { header: headerFor(identity, source, collapsed, rejected), tests };
}

export function serializeRecord({ header, tests }) {
  const sorted = [...tests].sort((a, b) => {
    if (a.test_id < b.test_id) return -1;
    return a.test_id > b.test_id ? 1 : 0;
  });
  const lines = [header, ...sorted].map((line) => JSON.stringify(line));
  return lines.map((line) => `${line}\n`).join('');
}

// ---- cargo test (libtest) logs ------------------------------------------------
// A section is one test binary: from its `Running` header to the next header.
// Child processes that re-run the same binary print into the parent's output,
// so a name printed twice is tainted (not_run) and the LAST result line of a
// section is the parent's. Any doubt gives not_run, never pass.

const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]/g;
const HEADER = /^\s*Running (?:unittests )?\S+ \((.+)\)\s*$/;
const RUNNING = /^running (\d+) tests?$/;
const TEST_LINE = /^test (\S+?)(?: - should panic)? \.\.\.(?: (.*))?$/;
const RESULT =
  /^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;(?: (\d+) measured;)?/;
const LIST_NAME = /^ {4}(\S+)$/;

function targetName(binary) {
  const base = binary.split(/[\\/]/).pop() ?? '';
  return base.replace(/\.exe$/, '').replace(/-[0-9a-f]{16}$/, '');
}

function newSection(target) {
  return {
    target,
    expected: null,
    lines: new Map(),
    failedNames: new Set(),
    result: null,
    childResults: 0,
    children: 0,
    afterResult: false,
  };
}

function lineOutcome(rest) {
  const text = rest.trim();
  if (text === 'ok') return 'pass';
  if (text === 'FAILED') return 'fail';
  if (text.startsWith('ignored')) return 'skip';
  if (text === '') return 'pending';
  if (text.endsWith(' ok')) return 'pass';
  if (text.endsWith('FAILED')) return 'fail';
  return 'pending';
}

function addEntry(section, name, outcome) {
  const list = section.lines.get(name) ?? [];
  list.push(outcome);
  section.lines.set(name, list);
  if (section.result !== null) section.afterResult = true;
}

// Rules 3 and 5 of the P03 algorithm; returns true when the line was consumed.
function testOrResult(section, line) {
  const test = TEST_LINE.exec(line);
  if (test) {
    const rest = test[2] ?? '';
    if (rest.startsWith('test ')) {
      addEntry(section, test[1], 'pending');
      section.children += 1;
      testOrResult(section, rest);
    } else {
      addEntry(section, test[1], lineOutcome(rest));
    }
    return true;
  }
  const result = RESULT.exec(line);
  if (!result) return false;
  if (section.result !== null) section.childResults += 1;
  const [P, F, I, M] = result.slice(1, 5).map((n) => Number(n ?? 0));
  section.result = { P, F, I, M };
  section.afterResult = false;
  return true;
}

function nextNonEmpty(lines, index) {
  for (let i = index + 1; i < lines.length; i += 1) {
    if (lines[i] !== '') return lines[i];
  }
  return '';
}

export function parseCargoLog(text) {
  const lines = text.replace(ANSI, '').split(/\r?\n/);
  const sections = [];
  let section = null;
  let inList = false;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const header = HEADER.exec(line);
    if (header) {
      section = newSection(targetName(header[1]));
      sections.push(section);
      inList = false;
      continue;
    }
    if (section === null) continue;
    if (inList) {
      const name = LIST_NAME.exec(line);
      if (name) {
        section.failedNames.add(name[1]);
        continue;
      }
      if (line !== '' || section.failedNames.size > 0) inList = false;
      if (line === '') continue;
    }
    if (line === 'failures:') {
      const next = nextNonEmpty(lines, i);
      inList = next.startsWith('    ') && !next.trimStart().startsWith('----');
      continue;
    }
    const running = RUNNING.exec(line);
    if (running) {
      if (section.expected === null) section.expected = Number(running[1]);
      else section.children += 1;
      continue;
    }
    testOrResult(section, line);
  }
  return sections;
}

function resolveNames(section, childSign) {
  const outcomes = new Map();
  const tainted = new Set();
  const pending = [];
  for (const [name, entries] of section.lines) {
    if (entries.length >= 2) {
      tainted.add(name);
      continue;
    }
    const outcome = entries[0];
    if (outcome === 'pass' && section.failedNames.has(name)) tainted.add(name);
    else if (outcome !== 'pending') outcomes.set(name, outcome);
    else if (section.failedNames.has(name)) outcomes.set(name, 'fail');
    else if (childSign) outcomes.set(name, 'not_run');
    else pending.push(name);
  }
  return { outcomes, tainted, pending };
}

function countOf(outcomes, wanted) {
  let n = 0;
  for (const outcome of outcomes.values()) if (outcome === wanted) n += 1;
  return n;
}

// Returns { outcomes: Map(name -> outcome), complete, tainted, children }.
export function reconcileSection(section) {
  const children = section.children + section.childResults;
  const childSign = children > 0;
  const { result, expected } = section;
  const names = section.lines.size;
  const allNotRun = (tainted) => ({
    outcomes: new Map([...section.lines.keys()].map((name) => [name, 'not_run'])),
    complete: false,
    tainted,
    children,
  });
  if (result === null || section.afterResult) return allNotRun(0);
  const total = result.P + result.F + result.I + result.M;
  if (expected !== null && total !== expected) return allNotRun(0);
  const { outcomes, tainted, pending } = resolveNames(section, childSign);
  for (const name of tainted) outcomes.set(name, 'not_run');
  if (pending.length > 0) {
    const fits = countOf(outcomes, 'pass') + pending.length === result.P
      && countOf(outcomes, 'fail') === result.F
      && countOf(outcomes, 'skip') === result.I;
    if (!fits) return allNotRun(tainted.size);
    for (const name of pending) outcomes.set(name, 'pass');
  }
  const complete = countOf(outcomes, 'pass') <= result.P
    && countOf(outcomes, 'fail') <= result.F
    && countOf(outcomes, 'skip') <= result.I
    && names === total;
  if (!complete) return allNotRun(tainted.size);
  return { outcomes, complete, tainted: tainted.size, children };
}

function cargoSource({ file, text }) {
  if (text === null || text === undefined) {
    const source = { file, present: false, complete: false, tests: 0, children: 0, tainted: 0 };
    return { source, entries: [] };
  }
  const sections = parseCargoLog(text);
  const entries = [];
  let complete = sections.length > 0;
  let children = 0;
  let tainted = 0;
  for (const section of sections) {
    const reconciled = reconcileSection(section);
    complete &&= reconciled.complete;
    children += reconciled.children;
    tainted += reconciled.tainted;
    for (const [name, outcome] of reconciled.outcomes) {
      entries.push({ test_id: `${section.target}::${name}`, outcome });
    }
  }
  return { source: { file, present: true, complete, tests: 0, children, tainted }, entries };
}

export function cargoRecord(logs, { identity }) {
  const sources = [];
  const all = [];
  let rejected = 0;
  for (const log of logs) {
    const { source, entries } = cargoSource(log);
    const safe = entries.filter((entry) => safeId(entry.test_id));
    rejected += entries.length - safe.length;
    source.tests = new Set(safe.map((entry) => entry.test_id)).size;
    sources.push(source);
    all.push(...safe);
  }
  const { tests, collapsed } = collapseOutcomes(all);
  const header = {
    schema: SCHEMA,
    kind: 'header',
    suite: 'cargo',
    ...identity,
    sources,
    collapsed,
    rejected,
  };
  return { header, tests };
}

function parseArgs(argv) {
  const [mode, ...rest] = argv;
  const options = { mode, log: [] };
  for (let i = 0; i < rest.length; i += 2) {
    const key = rest[i];
    const value = rest[i + 1];
    if (!key?.startsWith('--') || value === undefined) return null;
    if (key === '--log') options.log.push(value);
    else options[key.slice(2)] = value;
  }
  return options;
}

function readLog(file) {
  if (!fs.existsSync(file)) return { file: path.basename(file), text: null };
  return { file: path.basename(file), text: fs.readFileSync(file, 'utf8') };
}

function runCargo(options, env) {
  const identity = identityFromEnv(env, options.job);
  if (!identity.run_id || !identity.run_attempt || !identity.sha) {
    process.stderr.write('flaky-observations: missing GITHUB_RUN_ID/ATTEMPT/SHA; skipped\n');
    return 0;
  }
  writeRecord(options.out, cargoRecord(options.log.map(readLog), { identity }));
  return 0;
}

function readReport(input) {
  if (!fs.existsSync(input)) return { present: false };
  try {
    return { present: true, report: JSON.parse(fs.readFileSync(input, 'utf8')) };
  } catch {
    return { present: true, report: null };
  }
}

function writeRecord(out, record) {
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, serializeRecord(record));
}

function runVitest(options, env) {
  const identity = identityFromEnv(env, options.job);
  if (!identity.run_id || !identity.run_attempt || !identity.sha) {
    process.stderr.write('flaky-observations: missing GITHUB_RUN_ID/ATTEMPT/SHA; skipped\n');
    return 0;
  }
  const file = path.basename(options.in);
  const { present, report } = readReport(options.in);
  if (!present) {
    const source = { file, present: false, complete: false, tests: 0 };
    writeRecord(options.out, { header: headerFor(identity, source, 0, 0), tests: [] });
    return 0;
  }
  writeRecord(options.out, vitestRecord(report, { root: process.cwd(), identity, file }));
  return 0;
}

export function main(argv, env) {
  try {
    const options = parseArgs(argv);
    if (options?.mode === 'cargo' && options.out && options.job) return runCargo(options, env);
    if (options?.mode !== 'vitest' || !options.in || !options.out || !options.job) {
      process.stderr.write(
        'usage: flaky-observations.mjs vitest --in <json> --out <jsonl> --job <id>\n'
        + '       flaky-observations.mjs cargo --out <jsonl> --job <id> [--log <file>]...\n',
      );
      return 0;
    }
    return runVitest(options, env);
  } catch (error) {
    process.stderr.write(`flaky-observations: ${error?.message ?? 'unexpected error'}\n`);
    return 0;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2), process.env);
}
