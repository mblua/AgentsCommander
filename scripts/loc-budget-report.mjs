// Size budget report (report only, no gate). Prints Rust and TS/TSX line totals split into
// non-test, test and generated files, counts of files over 800 / 900 / at or over 1000 lines,
// and the largest files. Lines are counted with the same rule as the 1000-line gate
// (sanitizing_file_size.mjs -> fileMetrics). Usage: node scripts/loc-budget-report.mjs
// [--json] [--top N] [--root DIR]. Exit 0 = report printed, 2 = usage or Git error.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import { classifyPath } from './check-pr-net-lines.mjs';
import { fileMetrics } from './sanitizing_line_metrics.mjs';
import { resolveGit } from './resolve-git.mjs';

export const THRESHOLDS = [800, 900, 1000];
const LANGS = { rust: ['.rs'], ts: ['.ts', '.tsx'] };

export function languageOf(file) {
  for (const [lang, exts] of Object.entries(LANGS)) {
    if (exts.some(e => file.endsWith(e))) return lang;
  }
  return null;
}

export function kindOf(file) {
  const cls = classifyPath(file);
  if (cls === 'tests') return 'test';
  if (cls === 'generated') return 'generated';
  return 'nonTest';
}

function emptyBucket() {
  return { files: 0, lines: 0, over800: 0, over900: 0, atOrOver1000: 0 };
}

// entries: [{ path, lines }] -> totals per language and kind, plus the top N files.
export function summarize(entries, top = 20) {
  const totals = {};
  for (const lang of Object.keys(LANGS)) {
    totals[lang] = { nonTest: emptyBucket(), test: emptyBucket(), generated: emptyBucket() };
  }
  const files = [];
  for (const { path: file, lines } of entries) {
    const lang = languageOf(file);
    if (!lang) continue;
    const kind = kindOf(file);
    const bucket = totals[lang][kind];
    bucket.files += 1;
    bucket.lines += lines;
    if (lines > 800) bucket.over800 += 1;
    if (lines > 900) bucket.over900 += 1;
    if (lines >= 1000) bucket.atOrOver1000 += 1;
    files.push({ path: file, lines, lang, kind });
  }
  files.sort((a, b) => b.lines - a.lines || a.path.localeCompare(b.path));
  return { thresholds: THRESHOLDS, totals, top: files.slice(0, top) };
}

export function collect(root) {
  const git = resolveGit();
  const out = execFileSync(git, ['ls-files', '-z', '--', '*.rs', '*.ts', '*.tsx'],
    { cwd: root, encoding: 'utf8', maxBuffer: 1 << 28 });
  return out.split('\0').filter(Boolean).map(file => ({
    path: file,
    lines: fileMetrics(fs.readFileSync(path.join(root, file), 'utf8')).lines,
  }));
}

export function renderText(report) {
  const rows = [];
  for (const [lang, kinds] of Object.entries(report.totals)) {
    for (const [kind, b] of Object.entries(kinds)) {
      rows.push(`${lang.padEnd(5)} ${kind.padEnd(9)} files ${String(b.files).padStart(5)}` +
        `  lines ${String(b.lines).padStart(7)}  >800 ${b.over800}  >900 ${b.over900}` +
        `  >=1000 ${b.atOrOver1000}`);
    }
  }
  const top = report.top.map((f, i) =>
    `${String(i + 1).padStart(2)}. ${String(f.lines).padStart(5)}  ${f.kind.padEnd(9)} ${f.path}`);
  return ['LOC budget report (report only)', ...rows, '', `Top ${report.top.length} files:`,
    ...top].join('\n') + '\n';
}

export function parseArgs(argv) {
  const opts = { json: false, top: 20, root: process.cwd() };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--json') opts.json = true;
    else if (arg === '--top' || arg === '--root') {
      const value = argv[i + 1];
      if (value === undefined) throw new Error(`${arg} needs a value`);
      i += 1;
      if (arg === '--root') opts.root = value;
      else if (/^\d+$/.test(value)) opts.top = Number(value);
      else throw new Error('--top must be a whole number');
    } else throw new Error(`unknown argument: ${arg}`);
  }
  return opts;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const opts = parseArgs(process.argv.slice(2));
    const report = summarize(collect(opts.root), opts.top);
    process.stdout.write(opts.json ? JSON.stringify(report, null, 2) + '\n' : renderText(report));
  } catch (e) {
    process.stderr.write(`loc-budget-report: ${e.message}\n`);
    process.exitCode = 2;
  }
}
