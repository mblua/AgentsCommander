#!/usr/bin/env node
// Rejects any tracked file under the repository-root `plans/` directory.
// Shared by .husky/pre-commit (local) and .github/workflows/validate-no-plans-path.yml (server).
//
// Plans live in the AgentsCommander shared plans directory (`.ac/plans`), not in this
// repository. See issue #2183: `/plans/` was in .gitignore while 105 files were tracked,
// because .gitignore never applies to an already-tracked path and `git add -f` bypasses it
// for new ones. This check reads the git index, so neither loophole hides a violation.
//
// Usage:
//   node scripts/validate-no-plans-path.mjs
//
// Exit codes:
//   0 → no tracked path under plans/
//   1 → at least one tracked path under plans/, or an internal error

import { execFileSync } from 'node:child_process';

const BANNED_PREFIX = 'plans/';
const ALTERNATIVE   = '.ac/plans (the AgentsCommander shared plans directory, outside this repository)';

function die(msg) {
  console.error(`[no-plans-path] ${msg}`);
  process.exit(1);
}

// NUL-delimited so a path containing a newline cannot split into two records.
function trackedPaths() {
  let out;
  try {
    out = execFileSync('git', ['ls-files', '-z', '--', BANNED_PREFIX], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    });
  } catch (err) {
    die(`Could not read the git index: ${err?.message || err}`);
  }
  return out.split('\0').filter(Boolean);
}

const offenders = trackedPaths();

if (offenders.length === 0) {
  console.log('[no-plans-path] OK: no tracked file under plans/');
  process.exit(0);
}

const shown = offenders.slice(0, 20);
die(
  `${offenders.length} path(s) under "${BANNED_PREFIX}" are tracked. This directory is banned in this repository.\n` +
  shown.map((p) => `    ${p}`).join('\n') +
  (offenders.length > shown.length ? `\n    ... and ${offenders.length - shown.length} more` : '') +
  `\n  Rule:    no file may be committed under "${BANNED_PREFIX}".\n` +
  `  Instead: keep plans in ${ALTERNATIVE}.\n` +
  `  Fix:     git rm -r --cached plans && move the files there.\n` +
  `  Note:    neither "git add -f" nor "git push --no-verify" bypasses this;\n` +
  `           the server-side GitHub Action is authoritative.`
);
