// #4527 P06: daily collector. Reads flaky observation artifacts of finished
// `PR regression gates` runs, updates the 30-day history and writes the rate report.
// All GitHub access goes through `flaky-github.mjs`.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import { ApiError, BudgetExhausted, createGitHub } from './flaky-github.mjs';
import {
  applyBackfill,
  buildReport,
  deferBackfill,
  emptyHistory,
  ingestRecord,
  parseHistory,
  parseRecord,
  prune,
  renderMarkdown,
  windowStart,
} from './flaky-history.mjs';

export const SOURCE_WORKFLOW = 'pr-regression-gates.yml';
export const SELF_WORKFLOW = 'flaky-rate.yml';
export const MAX_API_CALLS = 600;
export const MAX_DAY_PAGES = 10;
export const MAX_DOWNLOADS = 500;
export const MAX_BACKFILL = 100;
export const MAX_ARTIFACT_BYTES = 20 * 1024 * 1024;
export const ARTIFACT_PREFIX = 'flaky-obs-';
export const MEMBER = 'flaky-observations.jsonl';
const SEARCH_LIMIT = 1000;

const DEFAULT_LIMITS = {
  maxDayPages: MAX_DAY_PAGES,
  maxDownloads: MAX_DOWNLOADS,
  maxBackfill: MAX_BACKFILL,
  maxArtifactBytes: MAX_ARTIFACT_BYTES,
};

class DownloadLimit extends Error {}

function newCoverage() {
  return {
    runs_listed: 0,
    attempts_ingested: 0,
    excluded: {
      cancelled: 0,
      other_conclusion: 0,
      fork: 0,
      invalid: 0,
      oversize: 0,
      duplicate: 0,
      corrupt: 0,
    },
    records_ingested: 0,
    incomplete_sources: 0,
    dropped_lines: 0,
    backfill_open: 0,
    history_reset: false,
    budget_exhausted: false,
    api_error: null,
    calls: 0,
    runs_pending: 0,
    list_truncated_days: [],
    complete: false,
  };
}

function daysOfWindow(today) {
  const days = [];
  const date = new Date(`${windowStart(today)}T00:00:00Z`);
  for (let day = windowStart(today); day <= today; day = date.toISOString().slice(0, 10)) {
    days.push(day);
    date.setUTCDate(date.getUTCDate() + 1);
  }
  return days;
}

function listDay(state, day) {
  const seen = [];
  let total = 0;
  for (let page = 1; page <= state.limits.maxDayPages; page += 1) {
    const result = state.gh.listRuns(SOURCE_WORKFLOW, day, page);
    total = result.total_count ?? 0;
    const runs = result.workflow_runs ?? [];
    seen.push(...runs);
    if (runs.length === 0 || seen.length >= total) break;
  }
  if (total > SEARCH_LIMIT || seen.length < total) state.coverage.list_truncated_days.push(day);
  return seen;
}

// The whole window is listed first (oldest day first), then the runs are processed in
// (created_at, id) order. A listing stop marks the unlisted days and processes nothing.
function collectRuns(state) {
  const days = daysOfWindow(state.today);
  for (const [index, day] of days.entries()) {
    try {
      state.listed.push(...listDay(state, day));
    } catch (error) {
      state.coverage.runs_listed = state.listed.length;
      stopFor(state, error);
      state.coverage.list_truncated_days.push(...days.slice(index));
      return;
    }
  }
  state.coverage.runs_listed = state.listed.length;
  state.listed.sort((a, b) =>
    a.created_at === b.created_at ? a.id - b.id : a.created_at < b.created_at ? -1 : 1,
  );
  for (const run of state.listed) collectRun(state, run);
}

function runArtifacts(state, runId) {
  const all = [];
  for (let page = 1; ; page += 1) {
    const { total_count: total = 0, artifacts = [] } = state.gh.runArtifacts(runId, page);
    all.push(...artifacts);
    if (artifacts.length === 0 || all.length >= total) return all;
  }
}

function countRecord(state, record) {
  const { coverage } = state;
  coverage.records_ingested += 1;
  coverage.dropped_lines += record.dropped;
  const sources = Array.isArray(record.header.sources) ? record.header.sources : [];
  coverage.incomplete_sources += sources.filter((source) => source?.complete === false).length;
}

function ingestArtifact(state, run, n, artifact) {
  const { coverage, limits } = state;
  if (artifact.size_in_bytes > limits.maxArtifactBytes) {
    coverage.excluded.oversize += 1;
    return;
  }
  if (state.downloads >= limits.maxDownloads) throw new DownloadLimit();
  state.downloads += 1;
  const text = state.gh.downloadText(artifact.id, MEMBER, state.tmpDir);
  const record = text === null ? null : parseRecord(text);
  if (!record) {
    coverage.excluded.corrupt += 1;
    return;
  }
  const day = String(run.created_at).slice(0, 10);
  const meta = { run_id: String(run.id), attempt: String(n), day, artifact_id: artifact.id };
  // ingestRecord also raises runs[id].attempt; the collector owns that marker and sets
  // it only when the whole attempt is done (step 3c), so restore it here.
  const prior = state.history.runs[run.id];
  const result = ingestRecord(state.history, record, meta);
  if (prior) state.history.runs[run.id] = prior;
  else delete state.history.runs[run.id];
  if (result.duplicate) coverage.excluded.duplicate += 1;
  else if (result.invalid) coverage.excluded.invalid += 1;
  else {
    countRecord(state, record);
    state.cache.set(`${meta.run_id}/${meta.attempt}/${record.header.job}`, record);
    state.needs.push(...result.needsBackfill);
  }
}

function ingestAttempt(state, run, n, artifacts) {
  const suffix = `-${n}`;
  for (const artifact of artifacts) {
    const { name = '' } = artifact;
    if (!name.startsWith(ARTIFACT_PREFIX) || !name.endsWith(suffix) || artifact.expired) {
      continue;
    }
    ingestArtifact(state, run, n, artifact);
  }
  state.coverage.attempts_ingested += 1;
}

function collectRun(state, run) {
  const { history, coverage } = state;
  const done = history.runs[run.id]?.attempt ?? 0;
  if (run.run_attempt <= done) return;
  const day = String(run.created_at).slice(0, 10);
  if (run.head_repository?.full_name !== run.repository?.full_name) {
    coverage.excluded.fork += 1;
    history.runs[run.id] = { attempt: run.run_attempt, day };
    return;
  }
  let artifacts = null;
  for (let n = done + 1; n <= run.run_attempt; n += 1) {
    const conclusion =
      n === run.run_attempt ? run.conclusion : state.gh.attempt(run.id, n).conclusion;
    if (conclusion === 'success' || conclusion === 'failure') {
      artifacts ??= runArtifacts(state, run.id);
      ingestAttempt(state, run, n, artifacts);
    } else if (conclusion === 'cancelled') coverage.excluded.cancelled += 1;
    else coverage.excluded.other_conclusion += 1;
    history.runs[run.id] = { attempt: n, day };
  }
}

function stopFor(state, error) {
  if (error instanceof BudgetExhausted || error instanceof DownloadLimit) {
    state.coverage.budget_exhausted = true;
  } else if (error instanceof ApiError) {
    state.coverage.api_error = error.message;
  } else {
    throw error;
  }
}

function backfillRecord(state, recordKey) {
  const cached = state.cache.get(recordKey);
  if (cached) return cached;
  const meta = state.history.records[recordKey];
  if (!meta) return null;
  if (state.backfills >= state.limits.maxBackfill) throw new DownloadLimit();
  state.backfills += 1;
  let text;
  try {
    text = state.gh.downloadText(meta.artifact_id, MEMBER, state.tmpDir);
  } catch (error) {
    // An expired or deleted artifact is a missing backfill, not an API failure.
    if (error instanceof ApiError && (error.http === 404 || error.http === 410)) return null;
    throw error;
  }
  const record = text === null ? null : parseRecord(text);
  if (record) state.cache.set(recordKey, record);
  return record;
}

function backfillItem(state, item) {
  const id = item.key.split('|').slice(2).join('|');
  const keys = [...item.recordKeys];
  while (keys.length > 0 && !state.backfillStopped) {
    const recordKey = keys[0];
    try {
      const record = backfillRecord(state, recordKey);
      const outcome = record?.tests.find((t) => t.test_id === id)?.outcome ?? 'missing';
      applyBackfill(state.history, item.key, item.sha, recordKey, outcome);
      keys.shift();
    } catch (error) {
      state.backfillStopped = true;
      stopFor(state, error);
    }
  }
  if (keys.length > 0) deferBackfill(state.history, item.key, item.sha, keys);
}

function backfill(state) {
  const pending = state.history.pending.map((p) => ({
    key: p.key,
    sha: p.sha,
    recordKeys: p.record_keys,
  }));
  state.history.pending = [];
  state.backfillStopped = state.coverage.api_error !== null;
  for (const item of [...pending, ...state.needs]) backfillItem(state, item);
}

function finish(state) {
  const { history, coverage, listed } = state;
  coverage.runs_pending = listed.filter(
    (run) => (history.runs[run.id]?.attempt ?? 0) < run.run_attempt,
  ).length;
  let open = 0;
  for (const entry of Object.values(history.tests)) {
    for (const shaEntry of Object.values(entry.shas)) open += shaEntry.open_backfill.length;
  }
  coverage.backfill_open = open;
  coverage.calls = state.gh.calls ?? 0;
  coverage.complete =
    !coverage.budget_exhausted &&
    coverage.api_error === null &&
    coverage.runs_pending === 0 &&
    coverage.list_truncated_days.length === 0;
}

function statusLine(coverage) {
  if (coverage.complete) return 'Report complete: all listed runs ingested.';
  const why = [];
  if (coverage.api_error) why.push(`API error: ${coverage.api_error}`);
  if (coverage.budget_exhausted) why.push('call budget reached (resumes tomorrow)');
  if (coverage.runs_pending > 0) why.push(`${coverage.runs_pending} runs pending`);
  if (coverage.list_truncated_days.length > 0) {
    why.push(`listing truncated on ${coverage.list_truncated_days.join(', ')}`);
  }
  return `Report NOT complete: ${why.join('; ')}.`;
}

export function collect({ gh, today, nowIso, tmpDir, limits = {} }) {
  const coverage = newCoverage();
  coverage.generated_at = nowIso;
  const state = {
    gh,
    today,
    tmpDir,
    coverage,
    limits: { ...DEFAULT_LIMITS, ...limits },
    history: null,
    listed: [],
    downloads: 0,
    backfills: 0,
    cache: new Map(),
    needs: [],
  };
  try {
    state.history = parseHistory(gh.latestHistory(SELF_WORKFLOW, tmpDir));
  } catch (error) {
    stopFor(state, error);
  }
  if (!state.history) {
    state.history = emptyHistory();
    coverage.history_reset = true;
  }
  if (!coverage.api_error && !coverage.budget_exhausted) {
    try {
      collectRuns(state);
    } catch (error) {
      stopFor(state, error);
    }
  }
  backfill(state);
  prune(state.history, today);
  finish(state);
  const report = buildReport(state.history, today, coverage);
  const markdown = `${statusLine(coverage)}\n\n${renderMarkdown(report)}`;
  return { history: state.history, report, markdown };
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--repo') args.repo = argv[(i += 1)];
    else if (argv[i] === '--out') args.out = argv[(i += 1)];
  }
  return args;
}

export function main(argv, env = process.env, deps = {}) {
  const { repo, out } = parseArgs(argv);
  if (!repo || !out) {
    process.stderr.write('usage: flaky-rate.mjs --repo <owner/repo> --out <dir>\n');
    return 2;
  }
  const gh = deps.gh ?? createGitHub({ repo, budget: MAX_API_CALLS });
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'flaky-rate-'));
  const now = deps.now ?? new Date();
  try {
    const result = collect({
      gh,
      today: now.toISOString().slice(0, 10),
      nowIso: now.toISOString(),
      tmpDir,
      limits: deps.limits,
    });
    fs.mkdirSync(out, { recursive: true });
    fs.writeFileSync(path.join(out, 'history.json'), `${JSON.stringify(result.history)}\n`);
    fs.writeFileSync(path.join(out, 'report.json'), `${JSON.stringify(result.report, null, 2)}\n`);
    fs.writeFileSync(path.join(out, 'report.md'), result.markdown);
    if (env.GITHUB_STEP_SUMMARY) fs.appendFileSync(env.GITHUB_STEP_SUMMARY, result.markdown);
    const { coverage } = result.report;
    const counts = [
      `runs_listed=${coverage.runs_listed}`,
      `records_ingested=${coverage.records_ingested}`,
      `runs_pending=${coverage.runs_pending}`,
      `calls=${coverage.calls}`,
      `budget_exhausted=${coverage.budget_exhausted}`,
      `api_error=${coverage.api_error ? 'yes' : 'no'}`,
    ];
    process.stdout.write(`flaky-rate: ${counts.join(' ')}\n`);
    return coverage.api_error ? 1 : 0;
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  process.exitCode = main(process.argv.slice(2));
}
