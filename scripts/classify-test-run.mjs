#!/usr/bin/env node

// Decides whether a red `npm test` run is the known #480 unhandled WebSocket
// rejection and nothing else (#1206).
//
// Whitelist, not denylist. The question is "is exactly the one known-good state
// present?", never "is anything known-bad present?". The rule every check here
// satisfies, from plan 5.6 as rewritten in round 5:
//
//   Every quantity the tolerate decision depends on must be either RECONCILED
//   against an enumeration present in the same artifact, or PINNED to a single
//   measured value or a measured enum. A quantity that is neither may not decide
//   anything.
//
// Structure is part of the specification, not style (5.6, S1-S3):
//   * exactly one `exit 0` site, reached only after every check returned true;
//   * one top-level try/catch, so any throw exits nonzero;
//   * fully synchronous — no promise can strand and reach the end undecided.
//
// Exit 0 tolerates the run. Exit 1 does not, and so does any error raised while
// deciding: the caller then exits with the original `npm test` status.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

// S3 reaches stdout too. `console.log` to a pipe can be asynchronous, and an
// asynchronous write racing `process.exit` is exactly the "reached the end with
// nothing decided" failure S1 and S3 exist to remove — here it would drop the
// reason string rather than the verdict. `fs.writeSync` cannot race it.
function writeLine(line) {
  fs.writeSync(1, `${line}\n`);
}

const WS_480_MESSAGE =
  'ws does not work in the browser. Browser clients must use the native WebSocket object';

// Vitest prints the offending value with its constructor name, so pinning the
// whole line pins the constructor to plain `Error` too. A `FooError:` carrying
// the same text is not the known-good state.
const WS_480_BLOCK = `Error: ${WS_480_MESSAGE}`;

// A3, round 5: all required, none optional. An absent field is a rejection,
// never a skipped check — an optional check is a check the producer can turn
// off. `numPendingTestSuites` is the ninth because A5 adopts the partition
// identity, and a field an identity consults must be required for the same
// reason the other eight are.
const COUNT_FIELDS = [
  'numTotalTests',
  'numPassedTests',
  'numFailedTests',
  'numPendingTests',
  'numTodoTests',
  'numTotalTestSuites',
  'numPassedTestSuites',
  'numFailedTestSuites',
  'numPendingTestSuites',
];

// A1.9. The four statuses an assertion may carry and that the count fields
// partition. Vitest's `StatusMap` can also emit "pending" (modes `run`,
// `queued`, `only`), which has no count field of its own — `numPendingTests`
// counts `skipped`. Measured reachable only under `--bail`, which this repo does
// not configure, and only on a run that already carries a failure. It is
// deliberately NOT in this list: admitting it would break A4's partition, and
// rejecting it is the fail-closed direction (plan 23).
const ASSERTION_STATUSES = ['passed', 'failed', 'skipped', 'todo'];

// A2. The producer's complete entry-status enum. Measured at `f08b8241`: the
// JSON reporter computes it as a ternary,
//   status: file.result?.state === "fail" || hasFailedTests ? "failed" : "passed"
// so exactly two values are reachable and `passed` is the only tolerated one.
const ENTRY_STATUSES = ['passed', 'failed'];

// Vitest's JSON reporter is Jest-compatible, so `skipped` is declared as
// `numPendingTests` and `todo` as `numTodoTests`. There is no `numSkippedTests`.
const DECLARED_STATUS_FIELD = {
  passed: 'numPassedTests',
  failed: 'numFailedTests',
  skipped: 'numPendingTests',
  todo: 'numTodoTests',
};

const CAUGHT_LINE = /^Vitest caught (\d+) unhandled errors? during the test run\.$/;

// The section header is `Unhandled Errors` (plural); a block header is
// `Unhandled Rejection` or `Unhandled Error` (singular). Requiring the whole
// line to be rule + phrase + rule is what keeps the two apart, and what keeps a
// test name that merely contains the phrase from registering as a block.
const BLOCK_HEADER = /^⎯+ Unhandled (?:Rejection|Error) ⎯+$/;

// A1: shape. Every value the later clauses read is typed here first, so a
// wrong type never reaches an identity and turns into a comparison against
// `undefined`.
function checkShape(report) {
  if (report === null || typeof report !== 'object' || Array.isArray(report)) {
    return 'the report root is not a Vitest report object';
  }
  if (!Array.isArray(report.testResults)) {
    return 'testResults is not an array';
  }
  if (report.testResults.length === 0) {
    return 'testResults is empty';
  }
  for (const suite of report.testResults) {
    if (suite === null || typeof suite !== 'object' || Array.isArray(suite)) {
      return 'a testResults entry is not an object';
    }
    if (typeof suite.name !== 'string' || suite.name === '') {
      return 'a testResults entry has no name';
    }
    if (!Array.isArray(suite.assertionResults)) {
      return 'a testResults entry has no assertionResults array';
    }
    for (const assertion of suite.assertionResults) {
      if (assertion === null || typeof assertion !== 'object' || Array.isArray(assertion)) {
        return 'an assertionResults entry is not an object';
      }
      if (!ASSERTION_STATUSES.includes(assertion.status)) {
        return `an assertion carries an unknown status ${JSON.stringify(assertion.status)}`;
      }
    }
  }
  return null;
}

// A2: entry status. Converted in round 5 (21.3, 22.3 item 1). 19.2 required only
// that no entry carry `status: "failed"`, which is a denylist over one literal:
// a deleted, `null` or `"flaky"` status was tolerated, because "not literally
// failed" is not proof that the file passed. Entry status is the only signal a
// suite or import failure carries, since such a file contributes an entry with
// zero assertions and no failed assertion, so every assertion identity holds.
function checkEntryStatus(report) {
  for (const suite of report.testResults) {
    if (!Object.prototype.hasOwnProperty.call(suite, 'status')) {
      return 'a testResults entry declares no status';
    }
    if (typeof suite.status !== 'string') {
      return `a testResults entry carries a non-string status ${JSON.stringify(suite.status)}`;
    }
    if (!ENTRY_STATUSES.includes(suite.status)) {
      return `a testResults entry carries an unknown status ${JSON.stringify(suite.status)}`;
    }
    if (suite.status !== 'passed') {
      return `a testResults entry reports status ${JSON.stringify(suite.status)}, not "passed"`;
    }
  }
  return null;
}

// A3: count fields, all required and all non-negative integers.
function checkCountFields(report) {
  for (const field of COUNT_FIELDS) {
    if (!Object.prototype.hasOwnProperty.call(report, field)) {
      return `${field} is absent`;
    }
    if (!Number.isInteger(report[field]) || report[field] < 0) {
      return `${field} is not a non-negative integer`;
    }
  }
  return null;
}

// A4: reconciliation over assertions. The aggregate and the parts must agree
// exactly rather than plausibly, which is what no unanticipated input can
// satisfy by accident.
//
// The declared per-status sum and `numTotalTests >= numPassedTests +
// numFailedTests` are deliberately absent: A1 makes the four statuses a
// partition of every assertion that reaches here, A4's first identity makes the
// observed assertions total `numTotalTests`, and A4's four equalities make each
// declared count equal its observed count, so both follow by construction
// (22.3 item 8).
function checkReconciliation(report) {
  const observed = { passed: 0, failed: 0, skipped: 0, todo: 0 };
  let observedTotal = 0;

  for (const suite of report.testResults) {
    for (const assertion of suite.assertionResults) {
      observed[assertion.status] += 1;
      observedTotal += 1;
    }
  }

  if (observedTotal !== report.numTotalTests) {
    return `the entries carry ${observedTotal} assertion(s) against numTotalTests ${report.numTotalTests}`;
  }
  for (const status of ASSERTION_STATUSES) {
    const field = DECLARED_STATUS_FIELD[status];
    if (observed[status] !== report[field]) {
      return `the entries carry ${observed[status]} ${status} assertion(s) against ${field} ${report[field]}`;
    }
  }
  return null;
}

// A5: the suite partition identity, adopted in round 5 after measurement. The
// inequality it replaces forbade one relation and admitted every other, which is
// a denylist by this audit's definition. Measured to hold on all six real
// reports of 20.1, all six probes of 22.7 and the `describe.todo` probe that
// makes the third term non-zero; and it holds by construction at this Vitest
// version, which computes numPassedTestSuites = total - failed - pending.
//
// `numTotalTestSuites` still appears in NO identity against `testResults.length`
// (A6). It counts describe blocks, not files; asserting one is what produced the
// round-3 false red, and that decision stays closed.
function checkSuiteCounts(report) {
  const parts = report.numPassedTestSuites + report.numFailedTestSuites + report.numPendingTestSuites;
  if (report.numTotalTestSuites !== parts) {
    return (
      `numTotalTestSuites ${report.numTotalTestSuites} is not the sum of ` +
      `passed ${report.numPassedTestSuites}, failed ${report.numFailedTestSuites} ` +
      `and pending ${report.numPendingTestSuites}`
    );
  }
  return null;
}

// B: real work happened and none of it failed. `numPassedTests` is the one piece
// of positive evidence and it is reconciled by A4. The two suite counts are
// coherence cross-checks, never evidence (clause D): they are describe-block
// aggregates no enumeration in the report can reconcile.
function checkCounts(report) {
  if (report.numPassedTests === 0) {
    return 'no test passed; an all-skipped or all-todo run is not the known state';
  }
  if (report.numFailedTests !== 0) {
    return `${report.numFailedTests} test(s) failed`;
  }
  if (report.numPassedTestSuites === 0) {
    return 'no suite passed';
  }
  if (report.numFailedTestSuites !== 0) {
    return `${report.numFailedTestSuites} suite(s) failed`;
  }
  return null;
}

// C: the unhandled-error section holds exactly one block, and it is exactly #480.
export function parseUnhandledBlocks(log) {
  const lines = String(log).split('\n');
  let declared = null;
  let duplicateHeader = false;
  const messages = [];

  for (let index = 0; index < lines.length; index += 1) {
    // 5.6 clause C compares byte for byte after ANSI stripping and stripping a
    // trailing CR. Nothing else is stripped, so an indented imitation of the
    // header or of the block line is not the known-good state.
    const line = stripTrailingCR(lines[index]);

    const caught = CAUGHT_LINE.exec(line);
    if (caught) {
      if (declared !== null) duplicateHeader = true;
      declared = Number(caught[1]);
      continue;
    }

    if (!BLOCK_HEADER.test(line)) continue;

    let next = index + 1;
    while (next < lines.length && stripTrailingCR(lines[next]) === '') next += 1;
    messages.push(next < lines.length ? stripTrailingCR(lines[next]) : '');
  }

  return { declared, messages, duplicateHeader };
}

function stripTrailingCR(line) {
  return line.endsWith('\r') ? line.slice(0, -1) : line;
}

// Three mandatory conjuncts, tightened in round 5 (22.6 finding 1). A `null`
// declaration is a rejection on its own: reading it as "not declared, skip the
// equality" is the same `if declared` branch A3 removed, and it would admit a
// log with no header and one forged block.
function checkUnhandled(log) {
  const { declared, messages, duplicateHeader } = parseUnhandledBlocks(log);

  if (duplicateHeader) {
    return 'the log declares the unhandled-error count more than once';
  }
  if (declared === null) {
    return 'the log declares no unhandled-error count';
  }
  // Cardinality is what makes this fail closed: with two blocks already a
  // rejection, a second error's constructor never has to be anticipated.
  if (declared !== 1) {
    return `the run reports ${declared} unhandled error(s), not exactly 1`;
  }
  if (messages.length !== declared) {
    return `parsed ${messages.length} unhandled block(s) against a declared ${declared}`;
  }
  if (messages[0] !== WS_480_BLOCK) {
    return 'the unhandled error is not the #480 signature';
  }
  return null;
}

// D is a prohibition, not a check: no producer-asserted verdict is read.
// `success` is the named instance and the ban is permanent — a real #480 run
// reports "success": true while exiting 1, so consulting it would tolerate
// exactly the runs this guard exists to catch.
//
// Clause order is DIAGNOSTIC ONLY. The verdict is the conjunction, so the order
// decides which reason string is printed and nothing else.
export function classify(report, log) {
  return (
    checkShape(report) ??
    checkEntryStatus(report) ??
    checkCountFields(report) ??
    checkReconciliation(report) ??
    checkSuiteCounts(report) ??
    checkCounts(report) ??
    checkUnhandled(log) ??
    null
  );
}

function decide(reportPath, logPath, log) {
  let report;
  try {
    report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
  } catch {
    return `no readable vitest JSON report at ${reportPath}`;
  }

  let normalized;
  try {
    normalized = fs.readFileSync(logPath, 'utf8');
  } catch {
    return `no readable normalized log at ${logPath}`;
  }

  const reason = classify(report, normalized);
  if (reason === null) {
    log(
      `#480 guard: tolerated — ${report.numPassedTests} passed, 0 failed, ` +
        'every entry passed, the entries reconcile with the aggregate, ' +
        'exactly one unhandled error and it is the #480 signature'
    );
  }
  return reason;
}

function assertSelf(condition, message) {
  if (!condition) throw new Error(message);
}

// A coherent report: the entries carry exactly the assertions the aggregate
// declares, every entry is `passed`, and all nine count fields are present.
function validReport(overrides = {}) {
  return {
    numTotalTests: 2,
    numPassedTests: 2,
    numFailedTests: 0,
    numPendingTests: 0,
    numTodoTests: 0,
    numTotalTestSuites: 2,
    numPassedTestSuites: 2,
    numFailedTestSuites: 0,
    numPendingTestSuites: 0,
    testResults: [
      { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'passed', title: 'a' }] },
      { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'passed', title: 'b' }] },
    ],
    ...overrides,
  };
}

function unhandledLog(count, ...messages) {
  const rule = '⎯'.repeat(4);
  const lines = [
    `${rule}${rule} Unhandled Errors ${rule}${rule}`,
    '',
    `Vitest caught ${count} unhandled error${count === 1 ? '' : 's'} during the test run.`,
    'This might cause false positive tests. Resolve unhandled errors to make sure your tests are not affected.',
    '',
  ];
  for (const message of messages) {
    lines.push(`${rule} Unhandled Rejection ${rule}`, message, '');
  }
  return lines.join('\n');
}

const LOG_480 = unhandledLog(1, WS_480_BLOCK);

function selfTestClauseC() {
  assertSelf(classify(validReport(), LOG_480) === null, 'the known-good state must be tolerated');
  assertSelf(
    classify(validReport(), unhandledLog(2, WS_480_BLOCK, 'AggregateError: unrelated runtime failure')) !== null,
    'AggregateError alongside #480 must be rejected'
  );
  assertSelf(
    classify(validReport(), unhandledLog(2, WS_480_BLOCK, 'FooError: a dependency subclass')) !== null,
    'a custom Error subclass alongside #480 must be rejected'
  );
  assertSelf(
    classify(validReport(), unhandledLog(2, WS_480_BLOCK, 'Unknown Error: a bare string')) !== null,
    'a rejected non-Error value alongside #480 must be rejected'
  );
  assertSelf(
    classify(validReport(), unhandledLog(2, WS_480_BLOCK, WS_480_BLOCK)) !== null,
    'two #480 blocks must be rejected'
  );
  assertSelf(
    classify(validReport(), unhandledLog(1, `Error: ${WS_480_MESSAGE.replace('browser.', 'browsers.')}`)) !== null,
    'a one-character-different message must be rejected'
  );
  assertSelf(
    classify(validReport(), unhandledLog(1, `FooError: ${WS_480_MESSAGE}`)) !== null,
    'the #480 text under another constructor must be rejected'
  );
  assertSelf(classify(validReport(), '') !== null, 'a log with no unhandled section must be rejected');
  assertSelf(
    classify(validReport(), `${LOG_480}\n${'⎯'.repeat(4)} Unhandled Rejection ${'⎯'.repeat(4)}\n${WS_480_BLOCK}\n`) !== null,
    'an extra block beyond the declared count must be rejected'
  );
  assertSelf(
    classify(validReport(), `${LOG_480}\n${LOG_480}`) !== null,
    'two declared counts must be rejected'
  );

  // Round 5, 22.6 finding 1: the header is mandatory. A log with one real block
  // and no header must be rejected, not read as "not declared, skip the check".
  const headerless = LOG_480.split('\n')
    .filter((line) => !CAUGHT_LINE.test(line))
    .join('\n');
  assertSelf(headerless.includes(WS_480_BLOCK), 'the headerless fixture must keep its block');
  assertSelf(
    classify(validReport(), headerless) !== null,
    'a deleted unhandled-error header with one real block must be rejected'
  );

  // Round 5: nothing but a trailing CR is stripped, so an indented imitation of
  // the block line is not the known-good state.
  assertSelf(
    classify(validReport(), unhandledLog(1, `  ${WS_480_BLOCK}`)) !== null,
    'an indented #480 line must be rejected'
  );
  // CRLF is the same line, and must stay tolerated.
  assertSelf(
    classify(validReport(), LOG_480.split('\n').join('\r\n')) === null,
    'a CRLF log must be tolerated'
  );
}

function selfTestClauseB() {
  assertSelf(
    classify(
      validReport({
        numPassedTests: 0,
        numPendingTests: 2,
        testResults: [
          { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'skipped' }] },
          { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'skipped' }] },
        ],
      }),
      LOG_480
    ) !== null,
    'an all-skipped run must be rejected'
  );
  assertSelf(
    classify(validReport({ numPassedTestSuites: 0, numTotalTestSuites: 0 }), LOG_480) !== null,
    'a run with no passed suite must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        numFailedTests: 1,
        numPassedTests: 1,
        testResults: [
          { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'passed' }] },
          { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'failed' }] },
        ],
      }),
      LOG_480
    ) !== null,
    'a failed test must be rejected'
  );
  assertSelf(
    classify(validReport({ numFailedTestSuites: 1, numTotalTestSuites: 3 }), LOG_480) !== null,
    'a failed suite must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        numPassedTests: 1,
        numPendingTests: 1,
        testResults: [
          { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'passed' }] },
          { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'skipped' }] },
        ],
      }),
      LOG_480
    ) === null,
    'passes alongside skips must stay tolerated'
  );
  // Round 5, probe P1: a todo alongside a real pass is a production shape.
  assertSelf(
    classify(
      validReport({
        numPassedTests: 1,
        numTodoTests: 1,
        testResults: [
          { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'passed' }] },
          { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'todo' }] },
        ],
      }),
      LOG_480
    ) === null,
    'passes alongside todos must stay tolerated'
  );
}

function selfTestClauseAShape() {
  assertSelf(classify(null, LOG_480) !== null, 'a null root must be rejected');
  assertSelf(classify([], LOG_480) !== null, 'an array root must be rejected');
  assertSelf(classify('a string', LOG_480) !== null, 'a non-object root must be rejected');
  assertSelf(
    classify(Object.fromEntries(COUNT_FIELDS.map((f) => [f, 1])), LOG_480) !== null,
    'a count-only object must be rejected'
  );
  assertSelf(
    classify(validReport({ testResults: 'not-an-array' }), LOG_480) !== null,
    'a wrong-typed testResults must be rejected'
  );
  assertSelf(
    classify(validReport({ testResults: [] }), LOG_480) !== null,
    'an empty testResults must be rejected'
  );
  assertSelf(
    classify(validReport({ testResults: [{ status: 'passed', assertionResults: [] }] }), LOG_480) !== null,
    'a testResults entry with no name must be rejected'
  );
  assertSelf(
    classify(validReport({ testResults: [{ name: '/repo/src/a.test.ts', status: 'passed' }] }), LOG_480) !== null,
    'a testResults entry with no assertionResults must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        testResults: [
          validReport().testResults[0],
          { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: ['not-an-object'] },
        ],
      }),
      LOG_480
    ) !== null,
    'an assertionResults entry that is not an object must be rejected'
  );
  // A1.9. "pending" is Vitest's fifth status and it has no count field, so it is
  // rejected rather than admitted: admitting it would break A4's partition.
  for (const status of ['flaky', 'pending', undefined, null, 3]) {
    assertSelf(
      classify(
        validReport({
          numPassedTests: 1,
          testResults: [
            validReport().testResults[0],
            { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status }] },
          ],
        }),
        LOG_480
      ) !== null,
      `an assertion status ${JSON.stringify(status)} outside the four must be rejected`
    );
  }
  // One file, many describe blocks: the real shape, and it must be tolerated.
  assertSelf(
    classify(validReport({ numTotalTestSuites: 384, numPassedTestSuites: 384 }), LOG_480) === null,
    'more suites than files is the real full-suite shape and must be tolerated'
  );
}

// A2 and A3, the two round-5 conversions, each as its own set.
function selfTestRound5Conversions() {
  for (const mutate of [
    (r) => { delete r.testResults[0].status; },
    (r) => { r.testResults[0].status = null; },
    (r) => { r.testResults[0].status = 123; },
    (r) => { r.testResults[0].status = 'flaky'; },
    (r) => { r.testResults[0].status = 'failed'; },
  ]) {
    const report = validReport();
    mutate(report);
    assertSelf(classify(report, LOG_480) !== null, 'a mutated entry status must be rejected');
  }

  for (const field of COUNT_FIELDS) {
    const missing = validReport();
    delete missing[field];
    assertSelf(classify(missing, LOG_480) !== null, `a report missing ${field} must be rejected`);
    assertSelf(
      classify(validReport({ [field]: -1 }), LOG_480) !== null,
      `a negative ${field} must be rejected`
    );
    assertSelf(
      classify(validReport({ [field]: 1.5 }), LOG_480) !== null,
      `a non-integer ${field} must be rejected`
    );
    assertSelf(
      classify(validReport({ [field]: 'none' }), LOG_480) !== null,
      `a wrong-typed ${field} must be rejected`
    );
  }

  // A5, the partition identity. The inequality it replaces admitted the first of
  // these; the identity does not.
  assertSelf(
    classify(validReport({ numTotalTestSuites: 3 }), LOG_480) !== null,
    'a suite total above the partition must be rejected'
  );
  assertSelf(
    classify(validReport({ numTotalTestSuites: 1 }), LOG_480) !== null,
    'a suite total below the partition must be rejected'
  );
  assertSelf(
    classify(validReport({ numTotalTestSuites: 3, numPendingTestSuites: 1 }), LOG_480) === null,
    'a non-zero pending suite count that completes the partition must be tolerated'
  );
}

function selfTestReconciliation() {
  const base = validReport();
  assertSelf(
    classify(validReport({ testResults: base.testResults.slice(0, 1) }), LOG_480) !== null,
    'a deleted testResults entry with unchanged counts must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        testResults: [base.testResults[0], { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [] }],
      }),
      LOG_480
    ) !== null,
    'an emptied assertionResults with unchanged counts must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        testResults: [base.testResults[0], { name: '/repo/src/b.test.ts', status: 'passed', assertionResults: [{ status: 'failed' }] }],
      }),
      LOG_480
    ) !== null,
    'a failed assertion against zero failure counts must be rejected'
  );
  assertSelf(
    classify(
      validReport({
        numTotalTests: 1,
        numPassedTests: 1,
        numTotalTestSuites: 1,
        numPassedTestSuites: 1,
        testResults: [
          { name: '/repo/src/a.test.ts', status: 'passed', assertionResults: [{ status: 'passed' }, { status: 'passed' }] },
        ],
      }),
      LOG_480
    ) !== null,
    'two assertions against numTotalTests 1 must be rejected'
  );
  assertSelf(
    classify(validReport({ numPendingTests: 1 }), LOG_480) !== null,
    'a declared skip count the entries do not carry must be rejected'
  );
  assertSelf(
    classify(validReport({ numTodoTests: 1 }), LOG_480) !== null,
    'a declared todo count the entries do not carry must be rejected'
  );
}

function selfTestClauseD() {
  assertSelf(
    classify(validReport({ success: true }), unhandledLog(2, WS_480_BLOCK, 'AggregateError: x')) !== null,
    'success: true must not rescue a rejected run'
  );
  assertSelf(
    classify(validReport({ success: false }), LOG_480) === null,
    'success: false must not reject a tolerated run'
  );
  // Named exception 1: unknown extra keys are accepted, anywhere.
  assertSelf(
    classify(validReport({ someFutureVitestField: { anything: true } }), LOG_480) === null,
    'an unknown extra key must not reject a tolerated run'
  );
}

function selfTestEndToEnd() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ac-classify-'));
  try {
    const reportPath = path.join(root, 'results.json');
    const logPath = path.join(root, 'log.txt');
    fs.writeFileSync(logPath, LOG_480);

    fs.writeFileSync(reportPath, JSON.stringify(validReport()));
    assertSelf(decide(reportPath, logPath, () => {}) === null, 'the known-good state must tolerate end to end');

    fs.writeFileSync(reportPath, '{ not json');
    assertSelf(decide(reportPath, logPath, () => {}) !== null, 'corrupt JSON must be rejected');

    fs.rmSync(reportPath);
    assertSelf(decide(reportPath, logPath, () => {}) !== null, 'a missing report must be rejected');

    fs.writeFileSync(reportPath, JSON.stringify(validReport()));
    assertSelf(
      decide(reportPath, path.join(root, 'absent.log'), () => {}) !== null,
      'a missing normalized log must be rejected'
    );
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function selfTest() {
  selfTestClauseC();
  selfTestClauseB();
  selfTestClauseAShape();
  selfTestRound5Conversions();
  selfTestReconciliation();
  selfTestClauseD();
  selfTestEndToEnd();
  writeLine('classify-test-run self-test passed');
  return true;
}

// Returns true only when every check returned true. Every other path returns
// false, including every early `return`, so the single `exit 0` below is
// reachable only from a complete pass.
function main(argv) {
  if (argv[0] === '--self-test') {
    return selfTest();
  }

  const reason = decide(
    argv[0] ?? 'npm-test-results.json',
    argv[1] ?? 'npm-test.normalized.log',
    writeLine
  );
  if (reason !== null) {
    writeLine(`#480 guard: not tolerated — ${reason}`);
    return false;
  }
  return true;
}

// S1, S2, S3 as one block: one top-level try/catch, fully synchronous, and
// exactly one `exit 0` site guarded by the conjunction. A throw anywhere — a
// missing file, an unreadable log, an unexpected type — arrives here and exits
// nonzero rather than at a special case.
//
// Guarded so the clauses above can be imported by a measurement harness without
// running the CLI; the guard is on entry, not on the verdict, so it adds no path
// by which a real invocation can end other than at the two exits below.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let tolerate = false;
  try {
    tolerate = main(process.argv.slice(2)) === true;
  } catch (error) {
    writeLine(`#480 guard: not tolerated — ${error instanceof Error ? error.message : String(error)}`);
    tolerate = false;
  }

  if (tolerate) {
    process.exit(0);
  }
  process.exit(1);
}
