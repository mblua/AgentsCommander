// #4527 P05: rolling 30-day flaky history and the rate report. Pure module: no fs, no
// network. Callers (P06) read and write the files and pass text and dates in.

export const WINDOW_DAYS = 30;
export const MIN_EXECUTED = 20;
export const QUARANTINE_SHAS = 2;
export const RELEASE_PASSES = 50;
export const HISTORY_SCHEMA = 1;
export const REPORT_SCHEMA = 2;

const OUTCOMES = new Set(['pass', 'fail', 'skip', 'not_run']);
const HEADER_STRINGS = ['run_id', 'run_attempt', 'job', 'sha', 'suite', 'platform'];
const CLASS_ORDER = ['flaky-evidence', 'suspect', 'failing', 'insufficient-data', 'stable'];
const MARKDOWN_LIMIT = 60000;
const MARKDOWN_ROWS = 200;
const TEXT_LIMIT = 200;
const CONTROL = /[\u0000-\u001f\u007f]/;

// AC-4527-003: maps are keyed by test ids and SHAs from artifacts, so read and write only
// own keys; `__proto__` or `constructor` must never reach Object.prototype.
const own = (map, key) => (Object.hasOwn(map, key) ? map[key] : undefined);
function put(map, key, value) {
  Object.defineProperty(map, key, { value, writable: true, enumerable: true, configurable: true });
  return value;
}

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

export function emptyHistory() {
  return { schema: HISTORY_SCHEMA, runs: {}, records: {}, tests: {}, ids: {}, pending: [] };
}

export function parseHistory(text) {
  let doc;
  try {
    doc = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isObject(doc) || doc.schema !== HISTORY_SCHEMA) return null;
  for (const field of ['runs', 'records', 'tests', 'ids']) {
    if (!isObject(doc[field])) return null;
  }
  if (!Array.isArray(doc.pending)) return null;
  return doc;
}

function validTestId(id) {
  return typeof id === 'string' && id.length > 0 && id.length <= 512 && !CONTROL.test(id);
}

export function parseRecord(text) {
  if (typeof text !== 'string') return null;
  const lines = text.split('\n').filter((line) => line.trim() !== '');
  if (lines.length === 0) return null;
  let header;
  try {
    header = JSON.parse(lines[0] ?? '');
  } catch {
    return null;
  }
  if (!isObject(header) || header.schema !== 1 || header.kind !== 'header') return null;
  if (!HEADER_STRINGS.every((field) => typeof header[field] === 'string')) return null;
  const tests = [];
  let dropped = 0;
  for (const line of lines.slice(1)) {
    let row;
    try {
      row = JSON.parse(line);
    } catch {
      dropped += 1;
      continue;
    }
    if (!isObject(row) || !validTestId(row.test_id) || !OUTCOMES.has(row.outcome)) {
      dropped += 1;
      continue;
    }
    tests.push({ test_id: row.test_id, outcome: row.outcome });
  }
  return { header, tests, dropped };
}

const testKey = (suite, platform, id) => `${suite}|${platform}|${id}`;
const maxDay = (a, b) => (a && a > b ? a : b);
// Code-unit order, the same order as a default sort() of strings (deterministic output).
const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

function testEntry(history, key) {
  let entry = own(history.tests, key);
  if (!entry) entry = put(history.tests, key, { days: {}, shas: {}, streak: 0 });
  return entry;
}

function idEntry(history, id) {
  let entry = own(history.ids, id);
  if (!entry) entry = put(history.ids, id, { streak: 0, streak_since: null });
  return entry;
}

function earlierRecordKeys(history, header, recordKey) {
  return Object.keys(history.records)
    .filter((key) => key !== recordKey)
    .filter((key) => {
      const rec = history.records[key];
      return rec.sha === header.sha && rec.suite === header.suite &&
        rec.platform === header.platform;
    })
    .sort(byCodeUnit);
}

function addFail(history, ctx, id, entry) {
  entry.streak = 0;
  idEntry(history, id).streak = 0;
  let shaEntry = own(entry.shas, ctx.sha);
  if (!shaEntry) {
    const open = earlierRecordKeys(history, ctx.header, ctx.recordKey);
    shaEntry = { day: ctx.day, last_day: ctx.day, pass: 0, fail: 0, open_backfill: open };
    put(entry.shas, ctx.sha, shaEntry);
    ctx.needsBackfill.push({ key: ctx.key, sha: ctx.sha, recordKeys: [...open] });
  }
  shaEntry.fail += 1;
  shaEntry.last_day = maxDay(shaEntry.last_day, ctx.day);
}

function addPass(history, ctx, id, entry) {
  entry.streak += 1;
  const ids = idEntry(history, id);
  ids.streak += 1;
  if (ids.streak === 1) ids.streak_since = ctx.day;
  const shaEntry = own(entry.shas, ctx.sha);
  if (shaEntry) {
    shaEntry.pass += 1;
    shaEntry.last_day = maxDay(shaEntry.last_day, ctx.day);
  }
}

export function ingestRecord(history, record, meta) {
  const { header } = record;
  const recordKey = `${meta.run_id}/${meta.attempt}/${header.job}`;
  if (own(history.records, recordKey)) return { duplicate: true, needsBackfill: [] };
  if (header.run_id !== String(meta.run_id) || header.run_attempt !== String(meta.attempt)) {
    return { invalid: true, needsBackfill: [] };
  }
  const { day } = meta;
  put(history.records, recordKey, {
    day,
    sha: header.sha,
    suite: header.suite,
    platform: header.platform,
    artifact_id: meta.artifact_id,
  });
  const run = own(history.runs, header.run_id);
  const attempt = Number(meta.attempt);
  if (!run || attempt > run.attempt) put(history.runs, header.run_id, { attempt, day });
  const needsBackfill = [];
  for (const { test_id: id, outcome } of record.tests) {
    const key = testKey(header.suite, header.platform, id);
    const entry = testEntry(history, key);
    const counts = own(entry.days, day) ?? put(entry.days, day, [0, 0, 0]);
    const ctx = { header, recordKey, day, sha: header.sha, key, needsBackfill };
    if (outcome === 'fail') {
      counts[1] += 1;
      addFail(history, ctx, id, entry);
    } else if (outcome === 'pass') {
      counts[0] += 1;
      addPass(history, ctx, id, entry);
    } else {
      counts[2] += 1;
    }
  }
  return { needsBackfill };
}

export function applyBackfill(history, key, sha, recordKey, outcome) {
  const test = own(history.tests, key);
  const shaEntry = test && own(test.shas, sha);
  if (!shaEntry) return;
  const at = shaEntry.open_backfill.indexOf(recordKey);
  if (at < 0) return;
  shaEntry.open_backfill.splice(at, 1);
  if (outcome !== 'pass') return;
  shaEntry.pass += 1;
  const recordDay = own(history.records, recordKey)?.day;
  if (recordDay) shaEntry.last_day = maxDay(shaEntry.last_day, recordDay);
}

export function deferBackfill(history, key, sha, recordKeys) {
  history.pending.push({ key, sha, record_keys: [...recordKeys] });
}

export function windowStart(today) {
  const date = new Date(`${today}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() - (WINDOW_DAYS - 1));
  return date.toISOString().slice(0, 10);
}

function pruneTest(entry, start) {
  for (const day of Object.keys(entry.days)) {
    if (day < start) delete entry.days[day];
  }
  for (const sha of Object.keys(entry.shas)) {
    if (entry.shas[sha].day < start) delete entry.shas[sha];
  }
}

export function prune(history, today) {
  const start = windowStart(today);
  for (const map of [history.records, history.runs]) {
    for (const key of Object.keys(map)) {
      if (map[key].day < start) delete map[key];
    }
  }
  const liveIds = new Set();
  for (const key of Object.keys(history.tests)) {
    const entry = history.tests[key];
    pruneTest(entry, start);
    if (Object.keys(entry.days).length === 0) delete history.tests[key];
    else liveIds.add(key.split('|').slice(2).join('|'));
  }
  history.pending = history.pending.filter((item) => {
    const test = own(history.tests, item.key);
    return test && own(test.shas, item.sha);
  });
  for (const id of Object.keys(history.ids)) {
    if (!liveIds.has(id)) delete history.ids[id];
  }
}

function classOf(executed, fails, flakyShas) {
  if (executed < MIN_EXECUTED) return 'insufficient-data';
  if (flakyShas >= QUARANTINE_SHAS) return 'flaky-evidence';
  if (flakyShas === 1) return 'suspect';
  if (fails > 0) return 'failing';
  return 'stable';
}

const isFlakySha = (shaEntry) => shaEntry.pass >= 1 && shaEntry.fail >= 1;

function rowFor(key, entry, start, today) {
  const [suite, platform, ...rest] = key.split('|');
  let executed = 0;
  let fails = 0;
  let other = 0;
  for (const [day, [pass, fail, more]] of Object.entries(entry.days)) {
    if (day < start || day > today) continue;
    executed += pass + fail;
    fails += fail;
    other += more;
  }
  const shas = Object.values(entry.shas).filter((s) => s.day >= start && s.day <= today);
  const flakyShas = shas.filter(isFlakySha).length;
  const openBackfill = shas.reduce((sum, s) => sum + s.open_backfill.length, 0);
  return {
    key,
    suite,
    platform,
    test_id: rest.join('|'),
    executed,
    fails,
    other,
    failure_rate: executed >= MIN_EXECUTED ? fails / executed : null,
    flaky_shas: flakyShas,
    open_backfill: openBackfill,
    class: classOf(executed, fails, flakyShas),
  };
}

export function classify(history, today) {
  const start = windowStart(today);
  return Object.keys(history.tests)
    .sort(byCodeUnit)
    .map((key) => rowFor(key, history.tests[key], start, today));
}

function evidenceFor(history, id, start) {
  const evidence = [];
  for (const key of Object.keys(history.tests).sort(byCodeUnit)) {
    const [, platform, ...rest] = key.split('|');
    if (rest.join('|') !== id) continue;
    for (const [sha, shaEntry] of Object.entries(history.tests[key].shas)) {
      if (shaEntry.day < start || !isFlakySha(shaEntry)) continue;
      evidence.push({ platform, sha, last_day: shaEntry.last_day });
    }
  }
  evidence.sort((a, b) => (a.platform + a.sha < b.platform + b.sha ? -1 : 1));
  const latest = evidence.reduce((day, item) => maxDay(day, item.last_day), null);
  return { latest_evidence_day: latest, evidence };
}

function quarantineOrder(rows) {
  const best = new Map();
  for (const row of rows) {
    if (row.class !== 'flaky-evidence') continue;
    best.set(row.test_id, Math.max(best.get(row.test_id) ?? 0, row.flaky_shas));
  }
  return [...best.entries()]
    .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
    .map(([id]) => id);
}

export function buildReport(history, today, coverage) {
  const start = windowStart(today);
  const rows = classify(history, today);
  const quarantine = quarantineOrder(rows);
  const candidates = {};
  for (const id of quarantine) put(candidates, id, evidenceFor(history, id, start));
  const releaseReady = Object.keys(history.ids)
    .sort(byCodeUnit)
    .filter((id) => history.ids[id].streak >= RELEASE_PASSES)
    .map((id) => ({
      test_id: id,
      streak: history.ids[id].streak,
      streak_since: history.ids[id].streak_since,
    }));
  return {
    schema: REPORT_SCHEMA,
    generated: today,
    window: { start, end: today },
    thresholds: {
      window_days: WINDOW_DAYS,
      min_executed: MIN_EXECUTED,
      quarantine_shas: QUARANTINE_SHAS,
      release_passes: RELEASE_PASSES,
    },
    coverage,
    tests: rows,
    quarantine_candidates: quarantine,
    candidates,
    release_ready: releaseReady,
  };
}

export function codeSpan(text) {
  const clean = String(text)
    .replaceAll('`', "'")
    .replaceAll('|', '/')
    .replaceAll('@', '@​')
    .replace(/[\r\n]+/g, ' ')
    .slice(0, TEXT_LIMIT);
  return `\`${clean}\``;
}

function cell(value) {
  const text = typeof value === 'object' && value !== null ? JSON.stringify(value) : String(value);
  return text.replaceAll('|', '/').replace(/[\r\n]+/g, ' ').slice(0, TEXT_LIMIT);
}

function rowOrder(a, b) {
  const byClass = CLASS_ORDER.indexOf(a.class) - CLASS_ORDER.indexOf(b.class);
  return byClass || b.flaky_shas - a.flaky_shas || (a.key < b.key ? -1 : 1);
}

export function renderMarkdown(report) {
  const out = [`# Flaky test report ${cell(report.window.start)} .. ${cell(report.window.end)}`];
  out.push('', '## Coverage', '', '| field | value |', '| --- | --- |');
  for (const [field, value] of Object.entries(report.coverage ?? {})) {
    out.push(`| ${cell(field)} | ${cell(value)} |`);
  }
  out.push('', '## Classes', '', '| class | tests |', '| --- | --- |');
  for (const name of CLASS_ORDER) {
    out.push(`| ${name} | ${report.tests.filter((row) => row.class === name).length} |`);
  }
  const rows = report.tests.filter((row) => row.class !== 'stable').sort(rowOrder);
  out.push('', '## Tests', '');
  out.push('| class | test | suite | platform | executed | fails | flaky SHAs |');
  out.push('| --- | --- | --- | --- | --- | --- | --- |');
  for (const row of rows.slice(0, MARKDOWN_ROWS)) {
    const fields = [row.class, codeSpan(row.test_id), cell(row.suite), cell(row.platform)];
    fields.push(row.executed, row.fails, row.flaky_shas);
    out.push(`| ${fields.join(' | ')} |`);
  }
  if (rows.length > MARKDOWN_ROWS) {
    out.push('', `${rows.length - MARKDOWN_ROWS} more in the JSON report.`);
  }
  const text = `${out.join('\n')}\n`;
  return text.length <= MARKDOWN_LIMIT ? text : `${text.slice(0, MARKDOWN_LIMIT - 1)}\n`;
}
