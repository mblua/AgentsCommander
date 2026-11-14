// #4527 P07: advisory quarantine issues from the daily flaky report. Opens or reopens one
// issue per flaky test (label `flaky-test`), comments new evidence and release proposals,
// and never closes an issue or touches merge gating. Notices carry markers, so a notice
// lost to a cap or an error is planned again the next day and delivered exactly once.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import { ApiError, createGitHub } from './flaky-github.mjs';

export const LABEL = 'flaky-test';
export const MAX_NEW_OR_REOPENED = 3;
export const MAX_COMMENTS = 10;
export const MAX_ISSUE_PAGES = 10;
export const MAX_COMMENT_LISTS = 30;
export const MAX_COMMENT_PAGES = 3;
export const MARKER_RE = /<!-- flaky-test-id:v1:([0-9a-f]{64}) -->/;

const NOTICE_RE = /<!-- flaky-(?:evidence|release):v1:[0-9a-f]{64}:[^ ]+ -->/g;
const PAGE_SIZE = 100;
const TITLE_LIMIT = 120;
const TEXT_LIMIT = 200;
const CONTROL = /[\u0000-\u001f\u007f]/g;
const ADVISORY =
  'Quarantine is advisory (#4527): the test still runs and still blocks merge. ' +
  'Only a person closes this issue.';

export const sha256hex = (text) =>
  crypto.createHash('sha256').update(Buffer.from(text, 'utf8')).digest('hex');
export const marker = (testId) => `<!-- flaky-test-id:v1:${sha256hex(testId)} -->`;
const evidenceMarker = (h, sha) => `<!-- flaky-evidence:v1:${h}:${sha} -->`;
const releaseMarker = (h, since) => `<!-- flaky-release:v1:${h}:${since} -->`;

function cell(value) {
  return String(value)
    .replace(CONTROL, ' ')
    .replaceAll('|', '/')
    .replaceAll('`', "'")
    .replaceAll('@', '@​')
    .slice(0, TEXT_LIMIT);
}

function fenced(text) {
  const longest = Math.max(0, ...(String(text).match(/`+/g) ?? []).map((run) => run.length));
  const fence = '`'.repeat(Math.max(3, longest + 1));
  return `${fence}text\n${text}\n${fence}`;
}

export function title(testId) {
  const segments = String(testId).split(/::| > |\//).filter((part) => part !== '');
  const short = cell(segments.slice(-2).join(' > ') || testId);
  return `Flaky test: ${short}`.slice(0, TITLE_LIMIT);
}

function table(report, testId) {
  const out = ['| suite | platform | executed | fails | failure rate | flaky SHAs | class |'];
  out.push('| --- | --- | --- | --- | --- | --- | --- |');
  for (const row of (report.tests ?? []).filter((r) => r.test_id === testId)) {
    const rate = row.failure_rate === null ? 'n/a' : `${(row.failure_rate * 100).toFixed(1)}%`;
    const fields = [cell(row.suite), cell(row.platform), row.executed, row.fails, rate];
    out.push(`| ${[...fields, row.flaky_shas, cell(row.class)].join(' | ')} |`);
  }
  return out.join('\n');
}

// Evidence SHAs of one candidate, one entry per SHA with its latest day on any platform.
function evidenceShas(report, testId) {
  const days = new Map();
  for (const item of report.candidates?.[testId]?.evidence ?? []) {
    const day = days.get(item.sha);
    if (!day || item.last_day > day) days.set(item.sha, item.last_day);
  }
  return [...days.entries()].sort().map(([sha, lastDay]) => ({ sha, last_day: lastDay }));
}

function shaLines(h, shas) {
  return shas.map((sha) => `- \`${cell(sha).slice(0, 7)}\` ${evidenceMarker(h, sha)}`);
}

function issueBody(report, testId, shas, runUrl) {
  const h = sha256hex(testId);
  return [
    marker(testId),
    '',
    fenced(testId),
    '',
    table(report, testId),
    '',
    'Flaky SHAs:',
    ...shaLines(h, shas),
    '',
    ADVISORY,
    '',
    `Report run: ${runUrl}`,
    '',
  ].join('\n');
}

function evidenceComment(report, testId, shas, runUrl) {
  const h = sha256hex(testId);
  return [
    'New flaky evidence.',
    '',
    table(report, testId),
    '',
    'Flaky SHAs:',
    ...shaLines(h, shas),
    '',
    `Report run: ${runUrl}`,
    '',
  ].join('\n');
}

function releaseComment(item) {
  const h = sha256hex(item.test_id);
  return [
    `${item.streak} executed passes in a row since ${cell(item.streak_since)}. ` +
      'A person may close this issue; automation never closes it.',
    releaseMarker(h, item.streak_since),
    '',
  ].join('\n');
}

// Marker hash -> issue (lowest number wins); duplicates are the other issues.
export function indexIssues(issues) {
  const byHash = new Map();
  const duplicates = [];
  for (const issue of [...issues].sort((a, b) => a.number - b.number)) {
    const match = MARKER_RE.exec(issue.body ?? '');
    if (!match) continue;
    if (byHash.has(match[1])) duplicates.push(issue.number);
    else byHash.set(match[1], issue);
  }
  return { byHash, duplicates };
}

const closedDay = (issue) => String(issue.closed_at ?? '').slice(0, 10);

// Issues whose markers `main` reads, in plan order, at most MAX_COMMENT_LISTS.
export function issuesToRead(report, byHash) {
  const wanted = [];
  for (const id of report.quarantine_candidates ?? []) {
    const issue = byHash.get(sha256hex(id));
    if (!issue) continue;
    if (issue.state === 'open') wanted.push(issue);
    else if (evidenceShas(report, id).some((s) => s.last_day > closedDay(issue))) {
      wanted.push(issue);
    }
  }
  for (const item of report.release_ready ?? []) {
    const issue = byHash.get(sha256hex(item.test_id));
    if (issue?.state === 'open') wanted.push(issue);
  }
  return [...new Set(wanted)].slice(0, MAX_COMMENT_LISTS);
}

export function markersOf(text) {
  return String(text ?? '').match(NOTICE_RE) ?? [];
}

export function planActions(report, issues, comments, nowIso, runUrl = '', indexComplete = true) {
  const { byHash, duplicates } = indexIssues(issues);
  const actions = [];
  const notices = [];
  const heldClosed = [];
  let opened = 0;
  let commented = 0;
  const read = (issue) => {
    const set = comments instanceof Map ? comments.get(issue.number) : comments[issue.number];
    return set ? new Set(set) : null;
  };
  const defer = (testId, kind, reason) => notices.push({ test_id: testId, kind, reason });

  for (const id of report.quarantine_candidates ?? []) {
    const h = sha256hex(id);
    const shas = evidenceShas(report, id);
    const issue = byHash.get(h);
    if (!issue) {
      // An index that may miss this test's issue never creates (no duplicate issue).
      if (!indexComplete) {
        defer(id, 'create', 'index_incomplete');
        continue;
      }
      if (opened >= MAX_NEW_OR_REOPENED) {
        defer(id, 'create', 'cap');
        continue;
      }
      opened += 1;
      const body = issueBody(report, id, shas.map((s) => s.sha), runUrl);
      actions.push({ type: 'create', test_id: id, title: title(id), body });
      continue;
    }
    const markers = read(issue);
    if (!markers) {
      defer(id, issue.state === 'open' ? 'evidence' : 'reopen', 'not_read');
      continue;
    }
    const missing = shas.filter((s) => !markers.has(evidenceMarker(h, s.sha)));
    if (issue.state !== 'open') {
      const fresh = missing.filter((s) => s.last_day > closedDay(issue)).map((s) => s.sha);
      if (fresh.length === 0) {
        heldClosed.push({ test_id: id, issue: issue.number });
        continue;
      }
      if (opened >= MAX_NEW_OR_REOPENED || commented >= MAX_COMMENTS) {
        defer(id, 'reopen', 'cap');
        continue;
      }
      opened += 1;
      commented += 1;
      actions.push({ type: 'reopen', test_id: id, issue: issue.number });
      const body = evidenceComment(report, id, fresh, runUrl);
      actions.push({ type: 'comment', kind: 'evidence', test_id: id, issue: issue.number, body });
      continue;
    }
    if (missing.length === 0) continue;
    if (commented >= MAX_COMMENTS) {
      defer(id, 'evidence', 'cap');
      continue;
    }
    commented += 1;
    const body = evidenceComment(report, id, missing.map((s) => s.sha), runUrl);
    actions.push({ type: 'comment', kind: 'evidence', test_id: id, issue: issue.number, body });
  }

  for (const item of report.release_ready ?? []) {
    const h = sha256hex(item.test_id);
    const issue = byHash.get(h);
    if (!issue || issue.state !== 'open') continue;
    const markers = read(issue);
    if (!markers) {
      defer(item.test_id, 'release', 'not_read');
      continue;
    }
    if (markers.has(releaseMarker(h, item.streak_since))) continue;
    if (commented >= MAX_COMMENTS) {
      defer(item.test_id, 'release', 'cap');
      continue;
    }
    commented += 1;
    const body = releaseComment(item);
    actions.push({ type: 'comment', kind: 'release', test_id: item.test_id, issue: issue.number,
      body });
  }
  return { actions, overflow: { notices, duplicates }, held_closed: heldClosed };
}

function errorOf(error) {
  if (!(error instanceof ApiError)) throw error;
  return { endpoint: error.endpoint, status: error.status, http: error.http ?? null };
}

export function applyActions(gh, actions) {
  const applied = [];
  try {
    if (actions.some((a) => a.type === 'create')) gh.ensureLabel(LABEL);
    for (const action of actions) {
      if (action.type === 'create') {
        const issue = gh.createIssue(action.title, action.body, [LABEL]);
        applied.push({ type: 'create', test_id: action.test_id, issue: issue?.number ?? null });
      } else if (action.type === 'reopen') {
        gh.reopenIssue(action.issue);
        applied.push({ type: 'reopen', test_id: action.test_id, issue: action.issue });
      } else {
        gh.comment(action.issue, action.body);
        applied.push({ type: 'comment', kind: action.kind, test_id: action.test_id,
          issue: action.issue });
      }
    }
  } catch (error) {
    return { applied, error: errorOf(error) };
  }
  return { applied, error: null };
}

function listIssues(gh) {
  const issues = [];
  for (let page = 1; page <= MAX_ISSUE_PAGES; page += 1) {
    const result = gh.listLabeledIssues(LABEL, page);
    issues.push(...result.issues);
    if (!result.more) return { issues, complete: true };
  }
  return { issues, complete: false };
}

function readMarkers(gh, issue) {
  const found = new Set(markersOf(issue.body));
  for (let page = 1; page <= MAX_COMMENT_PAGES; page += 1) {
    const items = gh.listComments(issue.number, page);
    for (const item of items) for (const m of markersOf(item.body)) found.add(m);
    if (items.length < PAGE_SIZE) break;
  }
  return found;
}

// One full pass: read issues and markers, plan, apply. Pure apart from `gh`.
export function runQuarantine({ gh, report, nowIso, runUrl }) {
  let issues;
  let complete;
  const comments = new Map();
  try {
    ({ issues, complete } = listIssues(gh));
    for (const issue of issuesToRead(report, indexIssues(issues).byHash)) {
      comments.set(issue.number, readMarkers(gh, issue));
    }
  } catch (error) {
    const empty = { notices: [], duplicates: [] };
    return { applied: [], overflow: empty, held_closed: [], error: errorOf(error) };
  }
  const plan = planActions(report, issues, comments, nowIso, runUrl, complete);
  const { applied, error } = applyActions(gh, plan.actions);
  return { applied, overflow: plan.overflow, held_closed: plan.held_closed, error };
}

function summary(result) {
  const count = (type) => result.applied.filter((a) => a.type === type).length;
  return [
    '## Flaky quarantine issues',
    '',
    `created: ${count('create')}, reopened: ${count('reopen')}, ` +
      `comments: ${count('comment')}, deferred: ${result.overflow.notices.length}, ` +
      `held closed: ${result.held_closed.length}, ` +
      `duplicates: ${result.overflow.duplicates.length}, ` +
      `error: ${result.error ? 'yes' : 'no'}`,
    '',
  ].join('\n');
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--repo') args.repo = argv[(i += 1)];
    else if (argv[i] === '--report') args.report = argv[(i += 1)];
    else if (argv[i] === '--out') args.out = argv[(i += 1)];
  }
  return args;
}

export function main(argv, env = process.env, deps = {}) {
  const { repo, report, out } = parseArgs(argv);
  if (!repo || !report || !out) {
    process.stderr.write('usage: flaky-quarantine.mjs --repo <o/r> --report <file> --out <dir>\n');
    return 2;
  }
  const gh = deps.gh ?? createGitHub({ repo });
  const runUrl = `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/` +
    `${env.GITHUB_RUN_ID}`;
  const result = runQuarantine({
    gh,
    report: JSON.parse(fs.readFileSync(report, 'utf8')),
    nowIso: deps.nowIso ?? new Date().toISOString(),
    runUrl,
  });
  fs.mkdirSync(out, { recursive: true });
  fs.writeFileSync(path.join(out, 'quarantine.json'), `${JSON.stringify(result, null, 2)}\n`);
  if (env.GITHUB_STEP_SUMMARY) fs.appendFileSync(env.GITHUB_STEP_SUMMARY, summary(result));
  return result.error ? 1 : 0;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main(process.argv.slice(2));
}
