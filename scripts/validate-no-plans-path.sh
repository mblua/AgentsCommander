#!/bin/sh
# Rejects any tracked file under the repository-root `plans/` directory.
# Shared by .husky/pre-commit, .husky/pre-push (local) and
# .github/workflows/validate-no-plans-path.yml (server, authoritative).
#
# Plans live in the AgentsCommander shared plans directory (`.ac/plans`), not in this
# repository. See issue #2183: `/plans/` was in .gitignore while 105 files were tracked,
# because .gitignore never applies to an already-tracked path and `git add -f` bypasses it
# for new ones. This reads the git index, so neither loophole hides a violation.
#
# Exit codes: 0 → clean; 1 → at least one tracked path under plans/.

set -eu

offenders=$(git ls-files -- 'plans/')

if [ -z "$offenders" ]; then
  echo "[no-plans-path] OK: no tracked file under plans/"
  exit 0
fi

count=$(printf '%s\n' "$offenders" | wc -l | tr -d ' ')

echo "[no-plans-path] $count path(s) under \"plans/\" are tracked. This directory is banned in this repository." >&2
printf '%s\n' "$offenders" | head -20 | sed 's/^/    /' >&2
if [ "$count" -gt 20 ]; then
  echo "    ... and $((count - 20)) more" >&2
fi
cat >&2 <<'MSG'
  Rule:    no file may be committed under "plans/".
  Instead: keep plans in .ac/plans (the AgentsCommander shared plans directory,
           outside this repository).
  Fix:     git rm -r --cached plans && move the files there.
  Note:    neither "git add -f" nor "git push --no-verify" bypasses this;
           the server-side GitHub Action is authoritative.
MSG
exit 1
