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
      const outcome = VITEST_STATUS[assertion?.status] ?? 'not_run';
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

function parseArgs(argv) {
  const [mode, ...rest] = argv;
  const options = { mode };
  for (let i = 0; i < rest.length; i += 2) {
    const key = rest[i];
    const value = rest[i + 1];
    if (!key?.startsWith('--') || value === undefined) return null;
    options[key.slice(2)] = value;
  }
  return options;
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
    if (options?.mode !== 'vitest' || !options.in || !options.out || !options.job) {
      process.stderr.write(
        'usage: flaky-observations.mjs vitest --in <json> --out <jsonl> --job <id>\n',
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
