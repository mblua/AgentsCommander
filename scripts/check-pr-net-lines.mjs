// #4715: PR net-line budget. Measures a PR's net line growth by file class and requires a
// written justification in the PR body when counted growth is above the budget.
// Exit 0 = within budget or justified, 1 = over budget without justification,
// 2 = usage or Git error (fail closed).
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import process from 'node:process';
import { pathToFileURL } from 'node:url';
import { resolveGit } from './resolve-git.mjs';

export const NET_LINE_BUDGET = 1000;
export const MIN_JUSTIFICATION_CHARS = 20;

const CLASSES = ['source', 'tests', 'other', 'docs', 'generated', 'lockfile'];
const COUNTED = new Set(['source', 'tests', 'other']);
const anyOf = (...parts) => new RegExp(parts.map((part) => part.source).join('|'));
const RULES = [
  ['lockfile', /(^|\/)(cargo\.lock|package-lock\.json|pnpm-lock\.yaml|yarn\.lock)$/],
  ['generated', anyOf(/(^|\/)(gen|generated|bindings)\//, /\.generated\./, /\.snap$/,
    /schemas?\/.*\.json$/, /(^|\/)tml-baseline/, /baseline.*\.json$/)],
  ['tests', anyOf(/(^|\/)(tests?|__tests__|e2e|fixtures?)\//, /\.(test|spec)\.[cm]?[jt]sx?$/,
    /_tests?\.rs$/, /(^|\/)tests?\.rs$/)],
  ['docs', /\.(md|mdx|txt)$|(^|\/)docs?\//],
  ['source', /^(src|src-tauri\/src|crates\/[^/]+\/src)\/.*\.(rs|ts|tsx|js|jsx|css|html)$/],
];
const HEADING = /^#{2,3}\s+net line growth justification\s*$/im;
const ANY_HEADING = /^#{1,6}\s/m;
const SHA = /^[0-9a-f]{40}(?:[0-9a-f]{24})?$/;
const NUMSTAT = /^(-|\d+)\t(-|\d+)\t([^]*)$/;

export function classifyPath(path) {
  const lower = path.toLowerCase();
  for (const [name, rule] of RULES) if (rule.test(lower)) return name;
  return 'other';
}

function count(value) {
  return value === '-' ? 0 : Number(value);
}

export function parseNumstatZ(text) {
  const tokens = text.split('\0');
  if (tokens.at(-1) === '') tokens.pop();
  const entries = [];
  for (let i = 0; i < tokens.length; i += 1) {
    const match = NUMSTAT.exec(tokens[i]);
    if (!match) throw new Error(`malformed numstat token: ${JSON.stringify(tokens[i])}`);
    let path = match[3];
    if (path === '') {
      if (i + 2 >= tokens.length) throw new Error('malformed numstat rename entry');
      path = tokens[i + 2];
      i += 2;
    }
    const binary = match[1] === '-' && match[2] === '-';
    entries.push({ path, additions: count(match[1]), deletions: count(match[2]), binary });
  }
  return entries;
}

export function summarize(entries) {
  const classes = {};
  for (const name of CLASSES) classes[name] = { additions: 0, deletions: 0, net: 0, files: 0 };
  for (const entry of entries) {
    const row = classes[classifyPath(entry.path)];
    row.additions += entry.additions;
    row.deletions += entry.deletions;
    row.net += entry.additions - entry.deletions;
    row.files += 1;
  }
  let counted = 0;
  for (const name of COUNTED) counted += classes[name].net;
  return { classes, counted };
}

// Removes HTML comments until no `<!--` is left; an unterminated one runs to the end.
function stripComments(text) {
  let out = text;
  for (let start = out.indexOf('<!--'); start !== -1; start = out.indexOf('<!--')) {
    const end = out.indexOf('-->', start + 4);
    out = end === -1 ? out.slice(0, start) : out.slice(0, start) + out.slice(end + 3);
  }
  return out;
}

// Only the first matching heading counts; its text runs to the next ATX heading.
export function extractJustification(body) {
  const text = (body ?? '').replace(/\r\n?/g, '\n');
  const heading = HEADING.exec(text);
  if (!heading) return '';
  const rest = text.slice(heading.index + heading[0].length);
  const next = ANY_HEADING.exec(rest);
  const section = next ? rest.slice(0, next.index) : rest;
  return stripComments(section).trim();
}

export function evaluate(summary, body) {
  const overBudget = summary.counted > NET_LINE_BUDGET;
  const chars = extractJustification(body).replace(/\s/g, '').length;
  const justified = chars >= MIN_JUSTIFICATION_CHARS;
  if (!overBudget) {
    const message = `Net line growth ${summary.counted} is within budget ${NET_LINE_BUDGET}.`;
    return { overBudget, justified, exitCode: 0, message };
  }
  if (justified) {
    const message = `Net line growth ${summary.counted} exceeds budget ${NET_LINE_BUDGET}; `
      + 'justified in the PR description.';
    return { overBudget, justified, exitCode: 0, message };
  }
  const message = `::error::Net line growth ${summary.counted} exceeds budget `
    + `${NET_LINE_BUDGET}; write at least ${MIN_JUSTIFICATION_CHARS} characters under the `
    + '"## Net line growth justification" heading in the PR description (only the first '
    + 'such heading counts; fill the existing one).';
  return { overBudget, justified, exitCode: 1, message };
}

export function renderReport(summary, verdict) {
  const lines = [
    '## PR net lines',
    '',
    '| class | files | + | - | net | counted |',
    '| --- | --- | --- | --- | --- | --- |',
  ];
  for (const name of CLASSES) {
    const row = summary.classes[name];
    const counted = COUNTED.has(name) ? 'yes' : 'no';
    lines.push(`| ${name} | ${row.files} | ${row.additions} | ${row.deletions} | ${row.net} `
      + `| ${counted} |`);
  }
  const status = verdict.exitCode === 0 ? 'PASS' : 'FAIL';
  lines.push('', `Counted net: ${summary.counted}. Budget: ${NET_LINE_BUDGET}.`,
    `Verdict: ${status}. ${verdict.message.replace(/^::error::/, '')}`, '');
  return lines.join('\n');
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === '--base') args.base = argv[i + 1];
    else if (argv[i] === '--head') args.head = argv[i + 1];
    else return null;
  }
  return SHA.test(args.base ?? '') && SHA.test(args.head ?? '') ? args : null;
}

function git(args) {
  const options = { encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, stdio: 'pipe' };
  return execFileSync(resolveGit(), args, options);
}

function measure(base, head) {
  const mergeBase = git(['merge-base', base, head]).trim();
  const diffArgs = ['diff', '--numstat', '-z', '-M', '--no-textconv', '--no-ext-diff'];
  return parseNumstatZ(git([...diffArgs, mergeBase, head, '--']));
}

export function main(argv, env = process.env) {
  const args = parseArgs(argv);
  if (!args) {
    process.stderr.write('usage: check-pr-net-lines.mjs --base <sha> --head <sha>\n');
    process.stdout.write('::error::check-pr-net-lines: --base and --head must be full SHAs\n');
    return 2;
  }
  let entries;
  try {
    entries = measure(args.base, args.head);
  } catch (error) {
    const detail = String(error?.stderr ?? error?.message ?? error).trim();
    process.stdout.write(`::error::check-pr-net-lines: git failed: ${detail}\n`);
    return 2;
  }
  const summary = summarize(entries);
  const verdict = evaluate(summary, env.PR_BODY ?? '');
  const report = renderReport(summary, verdict);
  process.stdout.write(report);
  if (env.GITHUB_STEP_SUMMARY) fs.appendFileSync(env.GITHUB_STEP_SUMMARY, report);
  if (verdict.exitCode !== 0) process.stdout.write(`${verdict.message}\n`);
  return verdict.exitCode;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2), process.env);
}
