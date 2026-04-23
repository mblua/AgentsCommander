#!/usr/bin/env node
// Validates a git branch name against the project convention.
// Shared by .husky/pre-push (local) and .github/workflows/validate-branch-name.yml (server).
//
// Usage:
//   node scripts/validate-branch-name.mjs --branch <name> [--check-issue]
//   node scripts/validate-branch-name.mjs                  (auto-detects current branch)
//
// Exit codes:
//   0 → valid, exempt, or grandfathered
//   1 → invalid format, slug too long, issue missing/closed, timeout, or internal error

import { execFileSync } from 'node:child_process';

const PATTERN          = /^(feature|fix|bug)\/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$/;
const MAX_SLUG         = 50;
const TARGET_REPO      = 'mblua/AgentsCommander';
const CUTOFF_SHA_PATH  = '.github/branch-name-enforcement.cutoff.sha';
const API_TIMEOUT_MS   = 10_000;
const SHA_RE           = /^[0-9a-fA-F]{40}$/;
const EXEMPT = [
  /^main$/,
  /^release\//,
  /^hotfix\//,
  /^dependabot\//,
  /^revert\//,
  /^gh-readonly-queue\//,
];

function parseArgs(argv) {
  const out = { branch: null, checkIssue: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--branch') out.branch = argv[++i];
    else if (argv[i] === '--check-issue') out.checkIssue = true;
  }
  return out;
}

function die(msg) {
  console.error(`[branch-name] ${msg}`);
  process.exit(1);
}

function git(args) {
  return execFileSync('git', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
}
function gitOk(args) {
  try { execFileSync('git', args, { stdio: 'ignore' }); return true; }
  catch { return false; }
}

function resolveBranch() {
  if (process.env.GITHUB_REF_NAME) return process.env.GITHUB_REF_NAME;
  try { return git(['symbolic-ref', '--short', 'HEAD']); }
  catch { die('Could not resolve current branch (detached HEAD?). Pass --branch <name>.'); }
}

function isExempt(branch) {
  return EXEMPT.some(re => re.test(branch));
}

function readCutoffSha() {
  let content;
  try { content = git(['show', `origin/main:${CUTOFF_SHA_PATH}`]); }
  catch { return null; }
  const first = content.split('\n', 1)[0].trim();
  if (!SHA_RE.test(first)) return null;
  return first.toLowerCase();
}

function isGrandfathered(branch) {
  const cutoff = readCutoffSha();
  if (!cutoff) return false;
  if (!gitOk(['rev-parse', '--verify', `${cutoff}^{commit}`])) return false;
  if (!gitOk(['rev-parse', '--verify', branch])) return false;
  if (gitOk(['merge-base', '--is-ancestor', cutoff, branch])) return false;
  return true;
}

function validateFormat(branch) {
  const m = PATTERN.exec(branch);
  if (!m) {
    die(
      `Branch "${branch}" does not match the naming convention.\n` +
      `  Expected: <type>/<issue-number>-<slug>\n` +
      `    <type>   ∈ { feature | fix | bug }\n` +
      `    <issue>  = open GitHub issue number (no leading zeros)\n` +
      `    <slug>   = lowercase-kebab-case, [a-z0-9]+(-[a-z0-9]+)*, ≤ ${MAX_SLUG} chars\n` +
      `  Example:  feature/63-branch-name-enforcement`
    );
  }
  const [, type, issueStr, slug] = m;
  if (slug.length > MAX_SLUG) die(`Slug is ${slug.length} chars (max ${MAX_SLUG}). Shorten it.`);
  return { type, issue: Number(issueStr), slug };
}

async function verifyIssueOpen(issue) {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (!token) die(`Missing GH_TOKEN / GITHUB_TOKEN in environment — cannot verify issue #${issue}.`);
  const url = `https://api.github.com/repos/${TARGET_REPO}/issues/${issue}`;
  let res, data;
  try {
    res = await fetch(url, {
      signal: AbortSignal.timeout(API_TIMEOUT_MS),
      headers: {
        'Accept': 'application/vnd.github+json',
        'Authorization': `Bearer ${token}`,
        'X-GitHub-Api-Version': '2022-11-28',
        'User-Agent': 'agentscommander-branch-validator',
      },
    });
    if (res.status === 404) die(`Issue #${issue} not accessible in ${TARGET_REPO} (missing or auth-denied).`);
    if (!res.ok) die(`GitHub API error (${res.status}) while fetching issue #${issue}.`);
    data = await res.json();
  } catch (err) {
    if (err?.name === 'TimeoutError' || err?.name === 'AbortError') {
      die(`Timed out (${API_TIMEOUT_MS} ms) fetching issue #${issue} from GitHub API.`);
    }
    if (err instanceof SyntaxError) {
      die(`Invalid JSON response from GitHub API for issue #${issue}.`);
    }
    die(`Network error fetching issue #${issue}: ${err?.message || err}`);
  }
  if (data.pull_request) die(`#${issue} is a pull request, not an issue.`);
  if (data.state !== 'open') die(`Issue #${issue} is ${data.state}. Branch must reference an OPEN issue.`);
}

(async () => {
  try {
    const args   = parseArgs(process.argv.slice(2));
    const branch = args.branch || resolveBranch();

    if (isExempt(branch))        { console.log(`[branch-name] exempt: ${branch}`); process.exit(0); }
    if (isGrandfathered(branch)) { console.log(`[branch-name] grandfathered (cut before enforcement): ${branch}`); process.exit(0); }

    const { issue } = validateFormat(branch);
    if (args.checkIssue) await verifyIssueOpen(issue);

    console.log(`[branch-name] OK: ${branch}`);
    process.exit(0);
  } catch (err) {
    die(`Unexpected error: ${err?.message || err}`);
  }
})();
