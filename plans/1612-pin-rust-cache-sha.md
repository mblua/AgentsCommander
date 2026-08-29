# #1612 — Pin swatinem/rust-cache to a full commit SHA across all workflows (S7637)

Status: READY_FOR_IMPLEMENTATION

Issue: `#1612` — "ci: pin swatinem/rust-cache to a full commit SHA across all workflows (S7637, follow-up to #1610)"
Repository: `repo-AgentsCommander`
Target: `main` pinned at `d7008b34e155a8bd6481be5feecfc7d96575328f` (== `origin/main`, verified)
Branch: `ci/1612-pin-rust-cache-sha` (already created off `d7008b34`, working tree clean)
Delivery: Lite (mechanical, mirrors the #1610 precedent — 4 workflow files, 11 single-line replacements, no new abstraction)

## Objective

Replace every unpinned `uses: swatinem/rust-cache@v2` reference in the workflows
with a full 40-character commit SHA plus a trailing `# v2` comment, matching the
convention already applied repository-wide to `dtolnay/rust-toolchain` by #1610.
After the change, all 11 `swatinem/rust-cache` references in `.github/workflows/`
resolve to the same commit SHA, and no `swatinem/rust-cache@v2` reference remains.
The mutable tag `@v2` is a supply-chain exposure of the same class #1610 removed
(SonarCloud `githubactions:S7637`, "Use full commit SHA hash for this
dependency."): a tag can be moved with a force-push, and whoever controls that
ref decides what code runs in our CI. The cache action is a particularly
attractive target — it has write access to the Rust build cache, and what it
writes is restored into later builds, including the `release.yml` build that
produces shipped artifacts. Pinning to a full SHA makes the action's code
immutable and auditable.

**Pinning freezes the action's code, not the cache contents or the cache key.**
Caching behavior, hit rates and keys are unaffected.

Accepted tradeoff (same as #1610): the repository stops picking up `v2.x`
bugfixes automatically; updates become a deliberate, reviewable PR.

## Cause and compact evidence (re-verified 2026-08-28 at `d7008b34`)

All 11 references use the mutable ref `@v2`. Exact grep output, line numbers as
of `d7008b34` (re-derive via grep before editing):

```
.github/workflows/bundle-validation.yml:50:        uses: swatinem/rust-cache@v2
.github/workflows/cache-warm.yml:51:        uses: swatinem/rust-cache@v2
.github/workflows/cache-warm.yml:100:        uses: swatinem/rust-cache@v2
.github/workflows/cache-warm.yml:124:        uses: swatinem/rust-cache@v2
.github/workflows/cache-warm.yml:152:        uses: swatinem/rust-cache@v2
.github/workflows/pr-regression-gates.yml:75:        uses: swatinem/rust-cache@v2
.github/workflows/pr-regression-gates.yml:128:        uses: swatinem/rust-cache@v2
.github/workflows/pr-regression-gates.yml:170:        uses: swatinem/rust-cache@v2
.github/workflows/pr-regression-gates.yml:220:        uses: swatinem/rust-cache@v2
.github/workflows/pr-regression-gates.yml:256:        uses: swatinem/rust-cache@v2
.github/workflows/release.yml:163:        uses: swatinem/rust-cache@v2
```

Per-file counts: `bundle-validation.yml` 1, `cache-warm.yml` 4,
`pr-regression-gates.yml` 5, `release.yml` 1 — eleven in total; 0 in
`lockfile-check.yml`, `validate-branch-name.yml`, `version-sync-check.yml`.

SHA of record (re-verified 2026-08-28 via `gh api`; `v2` is an **annotated tag**,
so resolution takes two steps — the obvious one-liner returns the tag object,
not the commit):

```
gh api repos/Swatinem/rust-cache/git/ref/tags/v2 --jq '.object.type + " " + .object.sha'
  -> tag 49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c
gh api repos/Swatinem/rust-cache/git/tags/49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c --jq '.object.type + " " + .object.sha'
  -> commit 6323deb102c322ba6fcbdcafc7e3dddab59af2b6
```

Cross-check: `v2.9.2` (latest release, published 2026-08-06) also dereferences to
the **same** commit `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (tag object
`63fed3e2fecf6f7b51dc6f043341b79ef82a9ae7` -> commit). Both agree; one SHA covers
all 11 references.

Existing convention to mirror (12 `dtolnay/rust-toolchain` references, all
identical shape, pinned by #1610):

```
        uses: dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c # stable
```

Task class: routine CI-configuration maintenance on a trusted repository.
Accepted threat model: trusted developers and GitHub-hosted build machines,
trusted repository toolchain/lockfiles; the pin itself is the supply-chain
hardening. No signing, provenance, untrusted-host, or security-boundary changes
exist; no enhanced provenance controls are applicable. Local validation is
complementary; GitHub CI is the authoritative execution evidence.

## Scope

Change exactly these 11 lines in exactly these 4 files (all under `.github/workflows/`):

| file | line(s) as of `d7008b34` | job containing the step |
| --- | --- | --- |
| `.github/workflows/bundle-validation.yml` | 50 | `bundle-validation` |
| `.github/workflows/cache-warm.yml` | 51, 100, 124, 152 | `warm-debug`, `warm-release`, `verify-debug-cache`, `verify-release-cache` |
| `.github/workflows/pr-regression-gates.yml` | 75, 128, 170, 220, 256 | `rust-regression`, `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable`, `windows-release-cli-smoke` |
| `.github/workflows/release.yml` | 163 | `build` |

Out of scope (explicitly decided):

- Any file outside `.github/workflows/`, including historical `plans/*.md`
  documents that mention `swatinem/rust-cache` (inert documentation; leave
  unchanged).
- No version upgrade. Pin the commit that `v2` currently resolves to; do not
  take the opportunity to move to a different version.
- No change to any `with:` block, cache key, cache strategy, job logic or step
  ordering. Each `uses:` line's surrounding YAML is untouched.
- No change to `actions/*` references (`actions/checkout@v5`,
  `actions/setup-node@v5`, `actions/upload-artifact@v4`) — GitHub-owned and
  exempted by S7637.
- No change to `dtolnay/rust-toolchain`, already pinned repository-wide in #1610.
- No change to the owner casing `swatinem/` (the canonical repository is
  `Swatinem/rust-cache`; GitHub resolves `uses:` owner case-insensitively). Leave
  the casing alone; changing it would enlarge the diff for no benefit.
- `tauri-apps/tauri-action@v0` at `.github/workflows/release.yml:171` is also
  unpinned and carries the same class of risk. It is deliberately NOT folded in
  here (mechanical scope; `@v0` major-version ref on the release-publishing
  action deserves its own consideration). Track it separately; do not touch it
  in this change.

## Decided solution

For each of the 11 unpinned lines, replace the reference with the full commit
SHA and keep a trailing comment naming the ref it corresponds to, byte-identical
in form to the #1610 convention (8-space indent, one space before the comment):

```yaml
uses: swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2
```

One SHA for all 11 occurrences — internally consistent. No other change.

Git handling (decided, two commits on `ci/1612-pin-rust-cache-sha` off `d7008b34`, PR to `main`; never push directly to `main`):

1. Commit 1 — this plan file. `/plans/` is gitignored (`.gitignore` line 11), so it
   MUST be force-added: `git add -f plans/1612-pin-rust-cache-sha.md`, then a
   dedicated docs commit (`docs(plan): add #1612 pin rust-cache SHA plan`).
   Every precedent plan is tracked this way (e.g. the #1610 plan commit
   `4545e3d3`); a plain `git add plans/...` silently stages nothing.
2. Commit 2 — the implementation: exactly the 4 workflow files, 11 lines changed.
   Keep the plan commit and the implementation commit separate so the
   implementation commit touches only files under `.github/workflows/` (the
   issue's scope rule), with the plan commit as the only other commit in the PR.

Rules that bind the implementer (no choice delegated):

- Use the exact SHA of record `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`, which
  must equal the output of the two-step dereference in verification check 6.
- Failure rule (SHA moved): if the two-step dereference resolves to a different
  commit at implementation time, pin **all 11** references to the new commit in
  the same commit. Never leave a split where two references use different SHAs.
  If this rule fires, the plan is unchanged otherwise; the new SHA simply
  replaces `6323deb...` everywhere. If either `gh api` step fails or returns a
  non-40-hex value, stop and report the blocker; never guess a SHA.

## Required behavior and failure behavior

Required behavior after the change:

- Every `swatinem/rust-cache` reference in `.github/workflows/` is of the form
  `uses: swatinem/rust-cache@<exactly-40-hex-chars> # v2`; zero references use
  `@v2` or any other non-40-char ref.
- All references share the same SHA; the action code executed by every step is
  identical to what CI ran before (the SHA is the same commit `v2` pointed at
  before the change), so runtime behavior of every workflow is unchanged.
- Cache contents, cache keys, hit rates and job logic are unchanged (the pin
  freezes action code, not the cache) — unchanged behavior.

Failure behavior:

- Any `@v2` reference remaining, any reference not a 40-char SHA, or two
  different SHAs → acceptance check fails; rework until the checks below pass.
- Any PR-exercised workflow failing on the PR (in particular a cache-restore
  step failure in one of the six PR jobs that restore a cache — a broken pin
  shows up as a step failure, not a silent slowdown) → investigate, fix, re-run;
  never merge red, never bypass.
- If the SHA-of-record commands fail or return a non-40-char value → stop and
  report the blocker; do not guess a SHA.
- `release.yml` and `cache-warm.yml` cannot be exercised by PR CI (triggers:
  `release.yml` = tag push `v*` only; `cache-warm.yml` = push to `main`, cron
  `0 6 * * *`, manual `workflow_dispatch`). Their changes are verified by diff
  reading only. The residual risk — a transcription break there would only
  surface at the next main push, scheduled run, or release — is the same
  coverage profile #1610 had, not a better one, and must be stated explicitly in
  the PR description. It is bounded: the replaced token is byte-identical in
  shape to the token already executed in PR CI by the six PR jobs, so the
  residual risk is limited to YAML transcription, which the diff-reading steps
  below cover.

## Verification (implementer runs these; success = stated output)

Run from the repository root (`repo-AgentsCommander`) on the final state of the
branch, before pushing: both commits made, working tree clean, `HEAD` = the
implementation commit. `d7008b34..HEAD` therefore spans exactly the 4 workflow
files (the plan commit touches nothing under `.github/workflows/`).

Machine quirk (known, applies to every check below): grep on this machine
intermittently returns a false empty result, and piping `git diff` into another
command can also come back empty. Redirect output to a file and read the file
before concluding that a reference is absent. An empty result is NOT evidence
that the SHA-moved failure rule fired; cross-check with the other checks first.

1. `grep -rn "swatinem/rust-cache@v2" .github/workflows/`
   — SUCCESS: no output, exit status 1. (Zero matches is the valid acceptance
   state; exit 1 is grep's no-match exit.)
2. `grep -rho "swatinem/rust-cache@[^[:space:]#]*" .github/workflows/ | sort -u`
   — SUCCESS: exactly one line:
   `swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6`
   (or the new SHA if the SHA-moved rule fired). This enumerates every ref form
   (`@v2`, `@master`, 40-char SHA — anything before whitespace or `#`), so any
   non-SHA ref that survives appears as a second line and fails the check.
   Note: GNU grep 3.0 here can spuriously report a no-match for `-o` with `{m,n}`
   intervals against regular files — the command above avoids `{m,n}` for that
   reason.
3. `grep -rc "swatinem/rust-cache" .github/workflows/*.yml | grep -v ':0$'`
   — SUCCESS: `bundle-validation.yml:1`, `cache-warm.yml:4`,
   `pr-regression-gates.yml:5`, `release.yml:1` — 11 references total, none
   lost, none duplicated. Unfiltered, `grep -rc` also prints the three
   zero-count files (`lockfile-check.yml:0`, `validate-branch-name.yml:0`,
   `version-sync-check.yml:0`), so the `grep -v ':0$'` filter keeps the output
   to the 4 files that have references. `-c` counts matching lines, not
   occurrences; here every reference is on its own line, so the counts equal
   occurrences.
4. `git show --stat HEAD` (implementation commit), `git diff --stat d7008b34..HEAD -- .github/workflows/`, and `git status --porcelain`
   — SUCCESS: exactly the 4 files above in the implementation commit; 11
   insertions, 11 deletions; working tree clean after both commits.
5. `git diff d7008b34..HEAD -- .github/workflows/`
   — SUCCESS: exactly the 4 files; every hunk is exactly one `uses:` line
   replacement of the form
   `-        uses: swatinem/rust-cache@v2` →
   `+        uses: swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2`;
   no other content changes anywhere. (At the run point above — clean tree,
   `HEAD` = implementation commit — bare `git diff` would emit nothing, so the
   explicit `d7008b34..HEAD` base range is required; it is the same range check
   4 uses. The changed lines are all ≥23 lines apart, so each forms its own
   hunk: 11 hunks, one per changed line; totals 11 insertions, 11 deletions.)
6. SHA-of-record tie-back:
   `gh api repos/Swatinem/rust-cache/git/ref/tags/v2 --jq '.object.sha'`
   → must output `49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c` (the tag object,
   NOT the commit — do not paste this into `uses:`), then
   `gh api repos/Swatinem/rust-cache/git/tags/49a0bdc70d2e1b713ca9e2869b211fcce03d3c1c --jq '.object.sha'`
   → must output `6323deb102c322ba6fcbdcafc7e3dddab59af2b6`, equal to the SHA
   used in the diff. Cross-check (optional but cheap):
   `gh api repos/Swatinem/rust-cache/git/ref/tags/v2.9.2 --jq '.object.sha'` and
   dereference it the same way — it must resolve to the same commit. If it
   resolves to a different commit, apply the SHA-moved failure rule.

CI verification (remote-owned; exact-head rule): PR checks must be green for the
exact PR-head SHA. Expected triggered workflows on this PR:

- `pr-regression-gates.yml` — triggers on every PR; all jobs run, including the
  six jobs that restore a cache: `bundle-validation` is its own workflow (below),
  and in this file `rust-regression`, `rust-regression-linux`,
  `rust-regression-macos`, `terminal-snapshot-portable`,
  `windows-release-cli-smoke`.
- `bundle-validation.yml` — triggers on PR because
  `.github/workflows/bundle-validation.yml` is in its `paths:` filter; the
  `bundle-validation` job runs and restores a cache.
- `cache-warm.yml` and `release.yml` — not triggered by PR CI; verified by diff
  reading (checks 4–5) only, residual risk stated in the PR description.

Objective acceptance criteria (all must hold):

- `grep -rn "swatinem/rust-cache@v2" .github/workflows/` returns zero matches
  (verified by command, not by eye).
- `grep -rho "swatinem/rust-cache@[^[:space:]#]*" .github/workflows/ | sort -u`
  returns exactly one line, the pinned 40-character SHA (verified by check 2;
  a surviving `@v2`, `@master` or a differing SHA appears as a second line and
  fails the check).
- Per-file reference counts are `bundle-validation.yml` 1, `cache-warm.yml` 4,
  `pr-regression-gates.yml` 5, `release.yml` 1 — eleven in total, none lost or
  duplicated (verified by check 3).
- The diff is exactly 11 single-line `uses:` replacements across those 4 files,
  11 insertions and 11 deletions, with no other content change anywhere
  (verified by checks 4–5).
- Only files under `.github/workflows/` are modified by the implementation
  commit; the only other commit in the PR adds this plan file (verified by
  check 4, `git status --porcelain` clean at the end).
- CI green on the exact PR-head SHA, with attention to the six jobs that restore
  a cache on the PR: `bundle-validation`, `rust-regression`,
  `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable`,
  `windows-release-cli-smoke`. A cache miss caused by a broken pin shows up as a
  step failure, not a silent slowdown.
- The `cache-warm.yml` and `release.yml` changes are reviewed by reading the
  diff, and the risk of only discovering a break at the next main push,
  scheduled run or release is stated explicitly in the PR description.
