#!/usr/bin/env node

// Turns a vitest JSON report into GitHub error annotations, one per failed test,
// so `frontend-regression` names the test that failed instead of naming a grep
// that did not match (#1206).
//
// This script only REPORTS. The workflow classifies first and calls it only for
// a failure it has already decided not to tolerate, and it always exits 0: the
// job's verdict is the original `npm test` status, never this script's.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Untrusted text is data, never markup. `::` is the runner's command sentinel
// and the runner looks for it anywhere in a line, not only at column zero, so a
// test named `::error::x` on a plain line is a command. Breaking the doubled
// colon closes that; a single colon is ordinary punctuation and stays readable,
// which is why `AssertionError: expected 1 to be 2` renders unchanged.
//
// The escape rule stays a character list, which is admissible here and nowhere
// else in 5.6 (named exception 3, 22.5): the runner's workflow-command grammar
// is closed and documented, unlike the open universes of error types, run
// outcomes and JSON shapes. The DECISION lives in assertOnlyIntendedCommands
// below, which is a whitelist over the emitted line.
function escapeData(value) {
  return String(value)
    .replaceAll('%', '%25')
    .replaceAll('\r', '%0D')
    .replaceAll('\n', '%0A')
    .replaceAll('::', '%3A%3A');
}

// Property values additionally escape `:` and `,`, which separate the properties
// themselves. `%` is escaped first, so every `%` introduced here stays literal.
function escapeProperty(value) {
  return escapeData(value).replaceAll(':', '%3A').replaceAll(',', '%2C');
}

function firstLine(message) {
  return String(message ?? '').split(/\r?\n/)[0] ?? '';
}

function normalizePath(filePath) {
  return filePath.replaceAll('\\', '/');
}

// R3, converted in round 5 (22.6 finding 2). The round-3 rule was "refuse any
// path that escapes the repository root", a denylist, and 20.10 is that denylist
// missing: a Windows absolute path replayed under Linux is treated by
// `path.relative` as a relative segment, no refusal fires, and
// `file=C%3A\Users\...` is emitted un-relativised.
//
// The whitelist form states the shape that may be emitted instead: relativise,
// then emit `file=` ONLY for a non-empty relative path that is not absolute on
// either platform and carries no `..` segment. Anything else is refused — and a
// refusal drops the property, never the finding.
const DRIVE_PREFIX = /^[A-Za-z]:/;

function allowedRelativePath(root, name) {
  if (typeof name !== 'string' || name === '') return null;
  const relative = normalizePath(path.relative(root, path.resolve(root, name)));
  if (relative === '') return null;
  // Absolute on either platform: a POSIX root, a UNC/Windows root, or a drive.
  if (relative.startsWith('/') || relative.startsWith('\\')) return null;
  if (DRIVE_PREFIX.test(relative)) return null;
  // `\` is a separator on Windows and a legal filename character on Linux, so it
  // is treated as a separator here in both directions. That is the strict side,
  // and the strict side of this predicate only ever drops a `file=` property.
  if (relative.split(/[\\/]/).includes('..')) return null;
  return relative;
}

function testName(assertion) {
  if (typeof assertion.fullName === 'string' && assertion.fullName !== '') {
    return assertion.fullName;
  }
  const ancestors = Array.isArray(assertion.ancestorTitles) ? assertion.ancestorTitles : [];
  return [...ancestors, assertion.title].filter(Boolean).join(' > ');
}

function collectFailures(report, root) {
  const tests = [];
  const suites = [];
  const results = Array.isArray(report.testResults) ? report.testResults : [];

  for (const suite of results) {
    const file = allowedRelativePath(root, suite?.name);
    const assertions = Array.isArray(suite?.assertionResults) ? suite.assertionResults : [];
    let failedHere = 0;

    for (const assertion of assertions) {
      if (!assertion || assertion.status !== 'failed') continue;
      failedHere += 1;
      tests.push({
        file,
        line: assertion.location?.line ?? null,
        column: assertion.location?.column ?? null,
        name: testName(assertion),
        message: firstLine(assertion.failureMessages?.[0]),
      });
    }

    // A file that fails to import reports a failed suite and zero failed
    // assertions. Without this it would produce no annotation at all, which is
    // the "the log names a grep, not a test" failure #1206 exists to remove.
    if (failedHere === 0 && suite?.status === 'failed') {
      suites.push({
        file,
        name: file ?? 'unknown test file',
        message: firstLine(suite.message),
      });
    }
  }

  return { tests, suites };
}

function annotate(entry) {
  const properties = [];
  if (entry.file) properties.push(`file=${escapeProperty(entry.file)}`);
  if (entry.file && Number.isInteger(entry.line)) {
    properties.push(`line=${escapeProperty(entry.line)}`);
    if (Number.isInteger(entry.column)) {
      properties.push(`col=${escapeProperty(entry.column)}`);
    }
  }

  const head = properties.length > 0 ? `::error ${properties.join(',')}::` : '::error::';
  const message = entry.message === '' ? entry.name : `${entry.name}: ${entry.message}`;
  return `${head}${escapeData(message)}`;
}

// The summary is an intentional workflow command rather than plain stdout, so
// untrusted text is inside a command's data region on every line this script
// writes. Defense in depth: the sentinel escaping above is what makes it safe.
function summarize(label, entries) {
  const more = entries.length > 1 ? ` and ${entries.length - 1} more` : '';
  return `::notice::${escapeData(`${entries.length} failing ${label}: ${entries[0].name}${more}`)}`;
}

function note(message) {
  return `::notice::${escapeData(message)}`;
}

function readReport(reportPath) {
  let raw;
  try {
    raw = fs.readFileSync(reportPath, 'utf8');
  } catch {
    return { report: null, reason: `no vitest JSON report at ${normalizePath(reportPath)}` };
  }

  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { report: null, reason: `vitest JSON report at ${normalizePath(reportPath)} does not parse` };
  }

  if (!parsed || typeof parsed !== 'object' || !Array.isArray(parsed.testResults)) {
    return { report: null, reason: `vitest JSON report at ${normalizePath(reportPath)} has no testResults` };
  }

  return { report: parsed, reason: null };
}

function report(reportPath, root, log) {
  const { report: parsed, reason } = readReport(reportPath);
  if (!parsed) {
    log(note(`report-failed-tests: ${reason}; no annotations emitted`));
    return;
  }

  const { tests, suites } = collectFailures(parsed, root);
  for (const entry of [...tests, ...suites]) {
    log(annotate(entry));
  }

  if (tests.length > 0) {
    log(summarize('test(s)', tests));
  }
  if (suites.length > 0) {
    log(summarize('test file(s)', suites));
  }
  if (tests.length === 0 && suites.length === 0) {
    log(note('report-failed-tests: the vitest JSON report lists no failed test'));
  }
}

function assertSelf(condition, message) {
  if (!condition) throw new Error(message);
}

function captureReport(reportPath, root) {
  const lines = [];
  report(reportPath, root, (line) => lines.push(line));
  return lines;
}

// R2, extended in round 5 (22.3 item 7). The safety property is asserted on the
// OUTPUT, not on the transformation, which is what makes it a whitelist over the
// emitted line rather than a checklist of characters escaped on the way in.
//
// For every emitted line: strip the one intended leading workflow command, and
// the remainder must contain no `::`, no CR and no LF, and each emitted record
// must be exactly one line. CR and LF are the other way a line can be split so
// that its tail is parsed as a fresh command.
const LEADING_COMMAND = /^::(?:error|warning|notice)(?: [^:]*)?::/;

function assertEmittedLine(line) {
  assertSelf(!line.includes('\n'), `an emitted record is more than one line: ${JSON.stringify(line)}`);
  assertSelf(!line.includes('\r'), `an emitted record carries a raw CR: ${JSON.stringify(line)}`);
  const intended = LEADING_COMMAND.exec(line);
  assertSelf(intended !== null, `line is not an intended workflow command: ${line}`);
  const remainder = line.slice(intended[0].length);
  assertSelf(!remainder.includes('::'), `an unintended command sentinel survived: ${line}`);
  assertSelf(!remainder.includes('\r'), `a raw CR survived: ${JSON.stringify(line)}`);
  assertSelf(!remainder.includes('\n'), `a raw LF survived: ${JSON.stringify(line)}`);
}

function assertOnlyIntendedCommands(lines) {
  for (const line of lines) assertEmittedLine(line);
}

const HOSTILE_NAMES = [
  '::error::x',
  '::warning::x',
  '::stop-commands::tok',
  'ordinary text ::error::x',
  'ordinary text ::warning::x',
  'ordinary text ::stop-commands::tok',
  // Round 5, M5: the two line-splitting characters and a name that already
  // looks percent-encoded, so the runner cannot decode it back into a sentinel.
  'breaks\rwith a carriage return',
  'breaks\nwith a line feed',
  'already encoded %3A%3Aerror%3A%3Ax',
];

function selfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ac-report-failed-'));
  try {
    const reportPath = path.join(root, 'results.json');
    const write = (value) =>
      fs.writeFileSync(reportPath, typeof value === 'string' ? value : JSON.stringify(value));

    // `:` and `,` are escaped in the property list, which they would otherwise
    // close. In the message they are data and stay readable, exactly as
    // @actions/core does it: only `%`, CR, LF and the `::` sentinel are escaped.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'a,b:c%d.test.ts'),
          status: 'failed',
          assertionResults: [
            {
              status: 'failed',
              fullName: 'suite > it injects, 100% of the time',
              location: { line: 12, column: 5 },
              failureMessages: ['AssertionError: expected 1 to be 2\n at somewhere'],
            },
          ],
        },
      ],
    });
    let lines = captureReport(reportPath, root);
    assertSelf(lines.length === 2, 'one annotation plus one summary expected');
    assertSelf(
      lines[0] ===
        '::error file=src/a%2Cb%3Ac%25d.test.ts,line=12,col=5::suite > it injects, 100%25 of the time: AssertionError: expected 1 to be 2',
      `property and data escaping wrong: ${lines[0]}`
    );
    // An ordinary single colon survives unescaped, in both annotation and summary.
    assertSelf(lines[0].includes('AssertionError: expected 1 to be 2'), 'a single colon must stay readable');
    assertSelf(
      lines[1] === '::notice::1 failing test(s): suite > it injects, 100%25 of the time',
      `summary wrong: ${lines[1]}`
    );
    assertOnlyIntendedCommands(lines);

    // The sentinel, in a test name, alone and preceded by ordinary text, plus
    // the round-5 additions.
    for (const hostile of HOSTILE_NAMES) {
      write({
        testResults: [
          {
            name: path.join(root, 'src', 'hostile.test.ts'),
            status: 'failed',
            assertionResults: [
              { status: 'failed', fullName: hostile, location: { line: 1, column: 1 }, failureMessages: ['boom'] },
            ],
          },
        ],
      });
      lines = captureReport(reportPath, root);
      assertOnlyIntendedCommands(lines);
      assertSelf(lines.length === 2, `expected one annotation and one summary for ${JSON.stringify(hostile)}`);
    }

    // The sentinel in a suite name, reached through the failed-suite path.
    for (const hostile of HOSTILE_NAMES) {
      write({
        testResults: [
          { name: path.join(root, 'src', 'broken.test.ts'), status: 'failed', message: hostile, assertionResults: [] },
        ],
      });
      lines = captureReport(reportPath, root);
      assertOnlyIntendedCommands(lines);
    }

    // The sentinel in a file path, in the property list and in the summary.
    for (const hostile of ['a::error::b.test.ts', 'ordinary::stop-commands::tok.test.ts']) {
      write({
        testResults: [
          { name: path.join(root, 'src', hostile), status: 'failed', message: 'boom', assertionResults: [] },
        ],
      });
      lines = captureReport(reportPath, root);
      assertOnlyIntendedCommands(lines);
    }

    // The sentinel in a failure message.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'msg.test.ts'),
          status: 'failed',
          assertionResults: [
            {
              status: 'failed',
              fullName: 'plain',
              location: { line: 1, column: 1 },
              failureMessages: ['::stop-commands::tok and more'],
            },
          ],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertOnlyIntendedCommands(lines);

    // CR and LF never reach the output raw.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'crlf.test.ts'),
          status: 'failed',
          assertionResults: [
            {
              status: 'failed',
              fullName: 'breaks\r\n::error::injected',
              location: { line: 1, column: 1 },
              failureMessages: ['boom'],
            },
          ],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertOnlyIntendedCommands(lines);
    assertSelf(lines[0].includes('breaks%0D%0A'), `CR/LF not escaped: ${lines[0]}`);

    // R3, round 5. Every refused shape emits the annotation WITHOUT a `file`
    // property, and no failure is silently dropped.
    const REFUSED = [
      ['a `..` segment', path.join(root, '..', 'outside.test.ts')],
      ['a POSIX absolute path', '/etc/passwd.test.ts'],
      ['a Windows drive path replayed as data', 'C:\\Users\\maria\\outside.test.ts'],
      ['the report root itself', root],
      ['an empty name', ''],
    ];
    for (const [label, name] of REFUSED) {
      write({
        testResults: [
          {
            name,
            status: 'failed',
            assertionResults: [
              { status: 'failed', fullName: 'outside', location: { line: 3, column: 1 }, failureMessages: ['boom'] },
            ],
          },
        ],
      });
      lines = captureReport(reportPath, root);
      assertSelf(lines[0] === '::error::outside: boom', `${label} was not refused: ${lines[0]}`);
      assertSelf(lines.length === 2, `${label} dropped the finding`);
      assertOnlyIntendedCommands(lines);
    }
    // And the allowed shape still carries the property, so the whitelist is not
    // simply refusing everything.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'inside.test.ts'),
          status: 'failed',
          assertionResults: [
            { status: 'failed', fullName: 'inside', location: { line: 3, column: 1 }, failureMessages: ['boom'] },
          ],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertSelf(
      lines[0] === '::error file=src/inside.test.ts,line=3,col=1::inside: boom',
      `an allowed path lost its file property: ${lines[0]}`
    );

    // No location (includeTaskLocation off) still anchors to the file.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'noloc.test.ts'),
          status: 'failed',
          assertionResults: [{ status: 'failed', fullName: 'no location', failureMessages: ['boom'] }],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertSelf(lines[0] === '::error file=src/noloc.test.ts::no location: boom', `no-location fallback wrong: ${lines[0]}`);

    // A file that fails to import has a failed suite and no failed assertion.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'broken.test.ts'),
          status: 'failed',
          message: 'SyntaxError: Unexpected token\n at import',
          assertionResults: [],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertSelf(
      lines[0] === '::error file=src/broken.test.ts::src/broken.test.ts: SyntaxError: Unexpected token',
      `failed suite with no assertions not reported: ${lines[0]}`
    );
    assertSelf(lines[1] === '::notice::1 failing test file(s): src/broken.test.ts', `suite summary wrong: ${lines[1]}`);

    // More than one failure names the first and counts the rest.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'many.test.ts'),
          status: 'failed',
          assertionResults: [
            { status: 'failed', fullName: 'first', location: { line: 1, column: 1 }, failureMessages: ['a'] },
            { status: 'passed', fullName: 'second', failureMessages: [] },
            { status: 'failed', fullName: 'third', location: { line: 9, column: 2 }, failureMessages: ['b'] },
          ],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertSelf(lines.length === 3, `expected two annotations and a summary, got ${lines.length}`);
    assertSelf(lines[2] === '::notice::2 failing test(s): first and 1 more', `plural summary wrong: ${lines[2]}`);

    // Missing and corrupt reports are reported, never thrown.
    fs.rmSync(reportPath);
    lines = captureReport(reportPath, root);
    assertSelf(lines.length === 1 && lines[0].includes('no vitest JSON report'), 'missing report not handled');
    assertOnlyIntendedCommands(lines);

    write('{ not json');
    lines = captureReport(reportPath, root);
    assertSelf(lines.length === 1 && lines[0].includes('does not parse'), 'corrupt report not handled');

    write({ numFailedTests: 0 });
    lines = captureReport(reportPath, root);
    assertSelf(lines.length === 1 && lines[0].includes('no testResults'), 'report without testResults not handled');

    // A green report emits no annotation at all.
    write({
      testResults: [
        {
          name: path.join(root, 'src', 'ok.test.ts'),
          status: 'passed',
          assertionResults: [{ status: 'passed', fullName: 'ok', failureMessages: [] }],
        },
      ],
    });
    lines = captureReport(reportPath, root);
    assertSelf(lines.length === 1 && lines[0].includes('lists no failed test'), 'green report emitted an annotation');

    console.log('report-failed-tests self-test passed');
    return 0;
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

const argv = process.argv.slice(2);
if (argv[0] === '--self-test') {
  try {
    process.exitCode = selfTest();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
} else {
  // Never fail the step. The workflow already holds the real exit status.
  try {
    report(argv[0] ?? 'npm-test-results.json', process.cwd(), (line) => console.log(line));
  } catch (error) {
    console.log(note(`report-failed-tests: ${error instanceof Error ? error.message : String(error)}`));
  }
  process.exitCode = 0;
}
