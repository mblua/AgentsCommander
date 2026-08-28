# #1610 — Pin dtolnay/rust-toolchain to a full commit SHA across all workflows (S7637)

Status: READY_FOR_IMPLEMENTATION

Issue: `#1610` — "ci: pin dtolnay/rust-toolchain to a full commit SHA across all workflows (S7637)"
Repository: `repo-AgentsCommander`
Target: `main` pinned at `29ecfee2532274b47316b52ab06a8d3bb96c86ef` (== `origin/main`, verified)
Branch: `ci/1610-pin-rust-toolchain-sha` (already created off `29ecfee2`, working tree clean)
Delivery: Lite (mechanical, precedent `7325e57a` / #1609 already in the tree, no new abstraction)

## Objective

Replace every unpinned `uses: dtolnay/rust-toolchain@stable` reference in the
workflows with a full 40-character commit SHA plus a trailing `# stable` comment,
matching the convention already established at `pr-regression-gates.yml:192`
(`rust-fmt`, pinned in #1609). After the change, all 12 `dtolnay/rust-toolchain`
references in `.github/workflows/` resolve to the same commit SHA, and no
`dtolnay/rust-toolchain@stable` reference remains. The mutable branch ref `@stable`
is a supply-chain exposure (SonarCloud `githubactions:S7637`, MAJOR, type
VULNERABILITY, security impact HIGH, message "Use full commit SHA hash for this
dependency."): whoever controls that branch decides what code runs in CI, and the
ref can be repointed without any change landing in this repository. Pinning to a
full SHA makes the action's code immutable and auditable.

**Pinning the SHA freezes the action's code, not the Rust toolchain version.** The
action continues to install the current stable Rust toolchain at run time; `@stable`
is the action's own branch name, not the rustc channel.

## Cause and compact evidence (re-verified 2026-08-28 at `29ecfee2`)

11 of 12 references use the mutable ref `@stable`; one is already pinned. Exact
grep output, line numbers as of `29ecfee2` (re-derive via grep before editing):

```
.github/workflows/bundle-validation.yml:45:        uses: dtolnay/rust-toolchain@stable
.github/workflows/cache-warm.yml:42:        uses: dtolnay/rust-toolchain@stable
.github/workflows/cache-warm.yml:91:        uses: dtolnay/rust-toolchain@stable
.github/workflows/cache-warm.yml:118:        uses: dtolnay/rust-toolchain@stable
.github/workflows/cache-warm.yml:146:        uses: dtolnay/rust-toolchain@stable
.github/workflows/pr-regression-gates.yml:70:        uses: dtolnay/rust-toolchain@stable
.github/workflows/pr-regression-gates.yml:123:        uses: dtolnay/rust-toolchain@stable
.github/workflows/pr-regression-gates.yml:165:        uses: dtolnay/rust-toolchain@stable
.github/workflows/pr-regression-gates.yml:192:        uses: dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c # stable
.github/workflows/pr-regression-gates.yml:217:        uses: dtolnay/rust-toolchain@stable
.github/workflows/pr-regression-gates.yml:251:        uses: dtolnay/rust-toolchain@stable
.github/workflows/release.yml:158:        uses: dtolnay/rust-toolchain@stable
```

`gh api repos/dtolnay/rust-toolchain/git/refs/heads/stable --jq '.object.sha'`
resolves to `4360b52568e2003a75bf9bc1d59f33a8e3fc893c` — identical to the SHA
already pinned at line 192. `stable` has not moved since #1609; there is no split
to reconcile, and one SHA covers all 12 references.

Workflow files are the only executed-code mentions of `dtolnay/rust-toolchain`
in the repository. All other mentions are inert: historical `plans/*.md` /
`plans/*.html` documents (e.g. `plans/1154-*`, `plans/1481-*`, `plans/1571-*`,
`plans/1596-*`), vendored binaries
(`node_modules/@tauri-apps/cli-win32-x64-msvc/cli.win32-x64-msvc.node`), and
`target/` build artifacts depending on build state. None of these are executed
code; all are out of scope (see Scope).

Task class: routine CI-configuration maintenance on a trusted repository.
Accepted threat model: trusted developers and GitHub-hosted build machines,
trusted repository toolchain/lockfiles; the pin itself is the supply-chain
hardening. No signing, provenance, untrusted-host, or security-boundary changes
exist; no enhanced provenance controls are applicable. Local validation is
complementary; GitHub CI is the authoritative execution evidence.

## Scope

Change exactly these 11 lines in exactly these 4 files (all under `.github/workflows/`):

| file | line(s) as of `29ecfee2` | job containing the step |
| --- | --- | --- |
| `.github/workflows/bundle-validation.yml` | 45 | `bundle-validation` |
| `.github/workflows/cache-warm.yml` | 42, 91, 118, 146 | `warm-debug`, `warm-release`, `verify-debug-cache`, `verify-release-cache` |
| `.github/workflows/pr-regression-gates.yml` | 70, 123, 165, 217, 251 | `rust-regression`, `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable`, `windows-release-cli-smoke` |
| `.github/workflows/release.yml` | 158 | `build` |

Do NOT touch `pr-regression-gates.yml:192` — it is already correct — unless the
failure rule below (SHA moved) applies, in which case it is updated in the same
commit.

Out of scope (explicitly decided):

- Any file outside `.github/workflows/`, including historical `plans/*.md`
  mentions of `dtolnay/rust-toolchain` (inert documentation; leave unchanged).
- No job logic, step ordering, `with:` inputs, toolchain components, or toolchain
  version changes. Each `uses:` line's surrounding YAML is untouched.
- No change to `actions/*` references (`actions/checkout`, `actions/setup-node`,
  `actions/upload-artifact` are GitHub-owned and exempted by S7637).
- No decision on `swatinem/rust-cache@v2` — explicitly out of scope in the issue;
  left for a separate decision.

## Decided solution

For each of the 11 unpinned lines, replace the reference with the full commit SHA
and keep a trailing comment naming the ref it corresponds to, byte-identical in
form to the existing pinned line (8-space indent, one space before the comment):

```yaml
uses: dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c # stable
```

One SHA for all occurrences — internally consistent. No other change.

Git handling (decided, two commits on `ci/1610-pin-rust-toolchain-sha` off `29ecfee2`, PR to `main`; never push directly to `main`):

1. Commit 1 — this plan file. `/plans/` is gitignored (`.gitignore` line 11), so it
   MUST be force-added: `git add -f plans/1610-pin-rust-toolchain-sha.md`, then a
   dedicated docs commit (`docs(plan): add #1610 pin rust-toolchain SHA plan`).
   Every precedent plan is tracked this way (e.g. `1e9cddfc` for #1592); a plain
   `git add plans/...` silently stages nothing.
2. Commit 2 — the implementation: exactly the 4 workflow files, 11 lines changed.
   Keep the plan commit and the implementation commit separate so the
   implementation commit touches only files under `.github/workflows/` (the
   issue's scope rule), with the plan commit as the only other commit in the PR.

Rules that bind the implementer (no choice delegated):

- Use the exact SHA of record `4360b52568e2003a75bf9bc1d59f33a8e3fc893c`, which
  must equal the output of `gh api repos/dtolnay/rust-toolchain/git/refs/heads/stable --jq '.object.sha'`.
- Failure rule (SHA moved): if that command resolves to a different SHA at
  implementation time, pin **all 12** references — the 11 unpinned lines **and**
  the already-pinned line 192 — to the new SHA in the same commit. Never leave a
  split where two references use different SHAs. If this rule fires, the plan is
  unchanged otherwise; the new SHA simply replaces `4360b52...` everywhere.
- Do not run actionlint. Verified: actionlint is not installed locally
  (`command not found` on PATH; `go` is also unavailable, so `go run
  github.com/rhysd/actionlint/...` is not possible) and no workflow or script in
  this repository invokes it — it is not a CI check here. The issue's actionlint
  criterion therefore has no executable gate in this repo; the gate is the grep
  acceptance checks below plus green PR CI, and the PR description must state
  that actionlint is neither installed locally nor wired into this repository's CI.

## Required behavior and failure behavior

Required behavior after the change:

- Every `dtolnay/rust-toolchain` reference in `.github/workflows/` is of the form
  `uses: dtolnay/rust-toolchain@<exactly-40-hex-chars> # stable`; zero references
  use `@stable` or any other non-40-char ref.
- All references share the same SHA; the action code executed by every step is
  identical to what CI ran before (the SHA is the same commit `stable` pointed at
  before the change), so runtime behavior of every workflow is unchanged.
- The Rust toolchain version installed is still the current stable channel
  (the pin freezes action code, not the rustc channel) — unchanged behavior.

Failure behavior:

- Any `@stable` reference remaining, any reference not a 40-char SHA, or two
  different SHAs → acceptance check fails; rework until the checks below pass.
- Any PR-exercised workflow failing on the PR → investigate, fix, re-run; never
  merge red, never bypass.
- If the SHA-of-record command fails or returns a non-40-char value → stop and
  report the blocker; do not guess a SHA.
- `release.yml` and `cache-warm.yml` cannot be exercised by PR CI (triggers:
  `release.yml` = tag push `v*` only; `cache-warm.yml` = push to `main`, cron
  `0 6 * * *`, manual `workflow_dispatch`). Their changes are verified by diff
  reading only. The residual risk — a break in these two workflows would only
  surface at the next tag push / main push / schedule run — must be stated
  explicitly in the PR description. This is bounded: the replaced token is
  byte-identical to the one already executed in PR CI by the `rust-fmt` job
  (`pr-regression-gates.yml:192`), so the residual risk is limited to YAML
  transcription, which the diff-reading step below covers.

## Verification (implementer runs these; success = stated output)

Run from the repository root (`repo-AgentsCommander`) on the final state of the
branch, before pushing: both commits made, working tree clean, `HEAD` = the
implementation commit. `29ecfee2..HEAD` therefore spans exactly the 4 workflow
files (the plan commit touches nothing under `.github/workflows/`).

1. `grep -rn "dtolnay/rust-toolchain@stable" .github/workflows/`
   — SUCCESS: no output, exit status 1. (Zero matches is the valid acceptance
   state; exit 1 is grep's no-match exit.)
2. `grep -rho "dtolnay/rust-toolchain@[^[:space:]#]*" .github/workflows/ | sort -u`
   — SUCCESS: exactly one line:
   `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c`
   (or the new SHA if the SHA-moved rule fired). This enumerates every ref form
   (`@stable`, `@v1`, `@master`, 40-char SHA — anything before whitespace or
   `#`), so any non-SHA ref that survives appears as a second line and fails
   the check; the surviving ref's 40-hex length is confirmed by comparing it
   with check 6's output. Note (observed on this box): GNU grep 3.0 here can
   spuriously report a no-match for `-o` with `{m,n}` intervals against regular
   files (e.g. `[0-9a-f]\{40\}`) — the command above avoids `{m,n}` for that
   reason. If any grep check here unexpectedly returns empty while the refs are
   demonstrably present, cross-check with checks 3 and 6 before any rework; an
   empty result is NOT evidence that the SHA-moved failure rule fired.
3. `grep -rc "dtolnay/rust-toolchain" .github/workflows/*.yml | grep -v ':0$'`
   — SUCCESS: `bundle-validation.yml:1`, `cache-warm.yml:4`,
   `pr-regression-gates.yml:6`, `release.yml:1` — 12 references total, none lost,
   none duplicated. Unfiltered, `grep -rc` also prints the three zero-count
   files (`lockfile-check.yml:0`, `validate-branch-name.yml:0`,
   `version-sync-check.yml:0`), so the `grep -v ':0$'` filter keeps the output
   to the 4 files that have references. `-c` counts matching lines, not
   occurrences; here every reference is on its own line, so the counts equal
   occurrences.
4. `git show --stat HEAD` (implementation commit), `git diff --stat 29ecfee2..HEAD -- .github/workflows/`, and `git status --porcelain`
   — SUCCESS: exactly the 4 files above in the implementation commit; 11
   insertions, 11 deletions; working tree clean after both commits.
5. `git diff 29ecfee2..HEAD -- .github/workflows/`
   — SUCCESS: exactly the 4 files; every hunk is exactly one `uses:` line
   replacement of the form
   `-        uses: dtolnay/rust-toolchain@stable` →
   `+        uses: dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c # stable`;
   no other content changes anywhere. (At the run point above — clean tree,
   `HEAD` = implementation commit — bare `git diff` would emit nothing, so the
   explicit `29ecfee2..HEAD` base range is required; it is the same range check
   4 uses. The changed lines are all ≥27 lines apart, so each forms its own
   hunk: 11 hunks, one per changed line; totals 11 insertions, 11 deletions.)
6. `gh api repos/dtolnay/rust-toolchain/git/refs/heads/stable --jq '.object.sha'`
   — SUCCESS: output equals the SHA used in the diff (i.e., the rule of one SHA,
   matching the SHA of record; if it differs, apply the SHA-moved failure rule).

CI verification (remote-owned; exact-head rule): PR checks must be green for the
exact PR-head SHA. Expected triggered workflows on this PR:

- `pr-regression-gates.yml` — triggers on every PR; all jobs run, including the
  toolchain-installing jobs `rust-regression`, `rust-regression-linux`,
  `rust-regression-macos`, `rust-fmt`, `terminal-snapshot-portable`,
  `windows-release-cli-smoke`.
- `bundle-validation.yml` — triggers on PR because `.github/workflows/bundle-validation.yml`
  is in its `paths:` filter; the `bundle-validation` job runs.
- `cache-warm.yml` and `release.yml` — not triggered by PR CI; verified by diff
  reading (checks 4–5) only, residual risk stated in the PR description.

Objective acceptance criteria (all must hold):

- `grep -rn "dtolnay/rust-toolchain@stable" .github/workflows/` returns zero
  matches (verified by command, not by eye).
- Every `dtolnay/rust-toolchain` reference in `.github/workflows/` resolves to
  the same 40-character commit SHA (verified by commands 1, 2, 5 and 6: check 1
  proves zero `@stable` remains; check 2 enumerates every ref form and `sort -u`
  must yield exactly that one line; check 5 proves each of the 11 changed lines
  is exactly the pinned form; check 6 ties the surviving ref to the SHA of
  record).
- The implementation commit modifies only files under `.github/workflows/`;
  the only other commit in the PR adds this plan file (verified by command 4,
  `git status --porcelain` clean at the end).
- PR CI green on the exact PR-head SHA for the two PR-triggered workflows;
  `release.yml`/`cache-warm.yml` changes reviewed by diff reading with the
  residual risk stated in the PR description.
