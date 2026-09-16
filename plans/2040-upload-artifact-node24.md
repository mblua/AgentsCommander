# #2040 — Upgrade `actions/upload-artifact` to a Node.js 24 release

Status: READY_FOR_IMPLEMENTATION
Band: Lite (1-25). Owner: `ac-dev-rust-v4` (room-27). Issue: <https://github.com/mblua/AgentsCommander/issues/2040>
Branch: `fix/2040-upload-artifact-node24`, base `f83189a6` (local only; no remote branch exists).
Line numbers below are as of `f83189a6`.

## 0. Environment risk — written before any edit (score veto: Environment = 12)

Question asked by the tech lead: does any release guard or manifest tooling check pinned
action SHAs in `release.yml` against something that would break?

**Answer: no tag-time check exists, and nothing breaks; the one interaction is a
preparation-time frozen-input record that fails closed.**

1. **No in-repo checker of action pins exists.** There is no actionlint, zizmor, ratchet,
   pin-github-action or Sonar configuration anywhere in the repository, no workflow-lint
   npm script, and no `.github/dependabot.yml`. The only pinning policy on record is the
   S7637 exemption for GitHub-owned `actions/*` references (`plans/1610-pin-rust-toolchain-sha.md:91`,
   `plans/1612-pin-rust-cache-sha.md:107`).
2. **The tag-time release guard compares committed bytes, never the tree.** `release.yml:275`
   asserts `release-authority-v1.txt.workflow-base-blob == input-manifest.v1.json.evidence.workflow.blobSha`.
   Both sides are files committed under `docs/releases/<tag>/`; the guard never recomputes a
   workflow blob, never parses `uses:` lines, and never resolves SHAs. Editing `release.yml`
   cannot make it fail.
3. **The preparation-time frozen-input check is the only real coupling, and it is upstream of
   the tag.** The external `prepare-agentscommander-release` skill (not in this repository)
   records `evidence.workflow.blobSha`, `contentSha256` and `actionPins` from the live
   `release.yml` when it generates a bundle. The planning-base gate of every release plan
   requires the contents API to still return that exact blob; any mismatch is
   `FROZEN_INPUT_CHANGED` → discard the bundle and regenerate
   (`docs/releases/v0.33.0/release-plan.md:137`). Measured now:
   `git hash-object .github/workflows/release.yml` = `8817fc60498807a0214d917fe42e5a389aef9ef1`,
   exactly the `workflow-base-blob` recorded by v0.33.0, so the tree is consistent today.
   After this change that value correctly differs. v0.33.0 is already tagged
   (`refs/tags/v0.33.0` object `7bbd680e…`, peeled `f83189a6…` = the current main) and its
   bundle is frozen; the newest bundle directory is `docs/releases/v0.33.0/`, so no future
   bundle exists to invalidate. Consequence, stated as a rule: **this change must land on
   `main` before the next `prepare` run.** A bundle generated before it would have to be
   regenerated; regeneration is the intended, fail-closed behavior, not a breakage.
4. **Re-recorded pins stay self-consistent.** `evidence.workflow.actionPins` records
   `repository`, `ref` and `resolvedSha` per `uses:` line; the new `release.yml` ref is a full
   commit SHA, so the next bundle records `ref = resolvedSha = 043fb46d…` with no tag
   resolution step.
5. **Runner floor is met, and v5 is not a fix.** v6+ requires Actions Runner ≥ 2.327.1; every
   affected job is GitHub-hosted (`windows-latest`, `ubuntu-22.04`, `ubuntu-latest`,
   `macos-latest`), so no self-hosted fleet needs updating. `actions/upload-artifact@v5.0.0`
   still declares `runs.using: node20` (verified from its `action.yml`), so v5 would leave the
   warning in place.
6. **v7's behavior changes do not touch any input used here.** v7 ships ESM and a new optional
   `archive` input defaulting to `true` (zips, as before). Every input used at the seven sites
   (`name`, `path`, `if-no-files-found`, `retention-days`, `compression-level`,
   `include-hidden-files`, `overwrite`) still exists in v7.0.1's `action.yml` with unchanged
   defaults.

Residual risk: MEDIUM, concentrated on the release path. It is mitigated by pinning the
release line to the same commit that PR CI exercises — the `refs/tags/v7` re-check in section 4
keeps that identity verifiable at implementation time — and by the positive and negative probe
controls in section 6 before merge.

## 1. Objective

Remove the GitHub Actions warning `Node.js 20 is deprecated. The following actions target
Node.js 20 but are being forced to run on Node.js 24: actions/upload-artifact@v4` by moving
all seven `actions/upload-artifact` uses to a Node 24 release.

## 2. Cause

`actions/upload-artifact` declares `runs.using: node20` up to and including v5.0.0 (verified
in each tag's `action.yml`: v5.0.0 `node20`; v6.0.0, v7.0.0, v7.0.1 `node24`). The runners
force Node 20 actions onto Node 24 and print the deprecation warning; only a v6+ reference
removes both the warning and the forced-runtime shim.

## 3. Scope

In scope:

- Exactly 7 `uses:` lines in exactly 2 files: `.github/workflows/pr-regression-gates.yml`
  (6 lines) and `.github/workflows/release.yml` (1 line).
- A temporary probe workflow used only to prove the `release.yml` use, deleted before merge
  (section 6B).

Out of scope (decided, not open):

- `actions/upload-artifact@v5` — still `node20`, does not satisfy the issue.
- `actions/download-artifact` — not referenced anywhere in the repository.
- `docs/releases/**` — frozen historical records that keep the old SHA; never edited.
- Every other action reference, including the pinned `actions/checkout`/`actions/setup-node`
  lines inside `release.yml` (owner casing, SHA-pinning policy of `actions/*`, dependabot).
- Application code, versions, `package.json`, Rust sources, and any behavior change beyond the
  action runtime.
- Installing workflow linters locally.

## 4. Decided version and pins

Decided release: **`actions/upload-artifact` v7.0.1**, the latest release at decision time.

- `pr-regression-gates.yml`: `@v4` → `@v7` (major tag, matching that file's existing
  `actions/checkout@v5` / `actions/setup-node@v5` style and the S7637 exemption).
- `release.yml`: `@ea165f8d65b6e75b540449e92b4886f43607fa02` →
  `@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (full commit SHA, keeping `release.yml`'s
  pinned-by-SHA convention).

Verification of the release pins, run and captured (2026-09-15, before any edit):

```text
gh api repos/actions/upload-artifact/git/refs/tags/v7.0.1 --jq '{ref:.ref,type:.object.type,sha:.object.sha}'
{"ref":"refs/tags/v7.0.1","sha":"043fb46d1a93c77aae656e7c1c64a875d1fc6a0a","type":"commit"}

gh api repos/actions/upload-artifact/git/refs/tags/v7 --jq '{ref:.ref,type:.object.type,sha:.object.sha}'
{"ref":"refs/tags/v7","sha":"043fb46d1a93c77aae656e7c1c64a875d1fc6a0a","type":"commit"}

git ls-remote https://github.com/actions/upload-artifact refs/tags/v7.0.1 refs/tags/v7
043fb46d1a93c77aae656e7c1c64a875d1fc6a0a	refs/tags/v7
043fb46d1a93c77aae656e7c1c64a875d1fc6a0a	refs/tags/v7.0.1
```

`043fb46d…` is a commit object, so the SHA is safe to use directly in `uses:` with no
annotation dereference. **Both refs must be re-checked at implementation time — before the
first edit and again before the PR is opened or refreshed** — because they back two different
claims:

- `refs/tags/v7.0.1` backs the full-SHA pin in `release.yml`;
- `refs/tags/v7` must still resolve to that same commit, otherwise the PR CI line (`@v7`) and
  the release line would run different code at merge time and §0's mitigation would be false.

If either ref no longer resolves to `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`, stop and report
— do not substitute silently.

## 5. Exact edits (line by line, as of `f83189a6`)

| # | File | Line | Job / step | From | To |
| - | ---- | ---- | ---------- | ---- | -- |
| 1 | `.github/workflows/pr-regression-gates.yml` | 618 | `rust-regression` / "Upload IS #1850 release diagnostics" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 2 | `.github/workflows/pr-regression-gates.yml` | 1311 | `rust-regression-linux` / "Upload IS #1850 release diagnostics" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 3 | `.github/workflows/pr-regression-gates.yml` | 2163 | `rust-regression-macos` / "Upload IS #1850 release diagnostics" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 4 | `.github/workflows/pr-regression-gates.yml` | 2286 | `windows-release-cli-smoke` / "Upload CLI smoke logs" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 5 | `.github/workflows/pr-regression-gates.yml` | 2413 | `issue-1850-windows-profile` / "Upload IS #1850 profile-proof logs" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 6 | `.github/workflows/pr-regression-gates.yml` | 2510 | `frontend-regression` / "Upload npm test diagnostics" | `actions/upload-artifact@v4` | `actions/upload-artifact@v7` |
| 7 | `.github/workflows/release.yml` | 1377 | `build` (4-leg matrix) / "Upload the closed successful producer manifest for internal verification" | `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02` | `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` |

Rules for the edit: keep the 8-space indent; change the token after `@` only; add no trailing
comment (the adjacent pinned `actions/checkout` / `actions/setup-node` lines in `release.yml`
carry none); do not touch `if:`, `timeout-minutes:` or any `with:` value.

### 5.1 Runner-gate precondition — before every push / PR operation (project rule)

Project rule: at most **three** repo+branch pairs with pending executions project-wide, and all
workflows and jobs of one branch count as **one** pair (the probe workflow, `PR regression
gates` and `validate branch name` runs on this branch are one pair, so the probe adds no extra
pair). A capture with `gate_ok_for_new_pair: false`, `capacity_free: 0`, or any entry in
`errors` blocks the push.

From the repo checkout, before each push and before creating or refreshing the PR:

```bash
PHASE_REPO_DIR="$PWD" bash "D:/0_repos/AgentsCommander_iac/.ac/project-shared/runner-gate/repo-agentscommander-collect-runner-gate.sh"
# prints the snapshot directory, e.g. .../snapshots/snap-<UTC>-<suffix>
node "D:/0_repos/AgentsCommander_iac/.ac/project-shared/runner-gate/repo-agentscommander-count-gate-pairs.js" "<snapshot-dir>"
```

Proceed only when the printed JSON has `"gate_ok_for_new_pair": true` and `"errors": []`
(`pair_count` ≤ 2, `capacity_free` ≥ 1, so the new branch pair fits in the last free slot). If
blocked, wait 10 minutes and re-run the capture; never push on a blocked or errored capture.
Record the snapshot path of each pre-push check in the PR body.

Baseline measured at plan time — snapshot `snap-20260915-164444-7NMd7v` (2026-09-15T16:45:19Z):
`pair_count=2`, `capacity_free=1`, `gate_ok_for_new_pair=true`, `errors=[]`, with
`docs/1936-update-config-directory-guides` and `fix/2030-lock-chip-icon-only` in progress. The
new pair fits only by taking the last free slot, so the check must be repeated live.

Git handling (implementation phase, mirroring `plans/1610-pin-rust-toolchain-sha.md`):

1. Commit 1 — the plan (force-added; `plans/` is gitignored and the project convention keeps
   plans in the PR) plus the 7-line change and the temporary probe file from section 6B:
   `git add -f plans/2040-upload-artifact-node24.md .github/workflows/pr-regression-gates.yml .github/workflows/release.yml .github/workflows/upload-artifact-v7-probe.yml`.
   Message: `ci(2040): move upload-artifact to v7 (node24)`.
2. Pass the runner-gate check (section 5.1), push commit 1, then wait until the probe run and
   the commit-1 `PR regression gates` run are terminal (ordering in section 6B); record both
   run URLs.
3. Commit 2 — delete the probe file. Pass the runner-gate check again and push. The probe file
   is absent from the final head. Never merge or push to `main`.

Final diff (the shape `git diff --name-status f83189a6..HEAD` must show, verified by §7.1):

- `.github/workflows/pr-regression-gates.yml` — `M`, 6 insertions / 6 deletions (`@v4` → `@v7`);
- `.github/workflows/release.yml` — `M`, 1 insertion / 1 deletion (the pin swap);
- `plans/2040-upload-artifact-node24.md` — `A`, kept in the PR by project convention;
- no other path, and `.github/workflows/upload-artifact-v7-probe.yml` is absent (deleted in
  commit 2).

## 6. Proof plan — how each use is proven

### 6A. `pr-regression-gates.yml` — the 6 uses

Triggered by the branch push and by the pull request itself (`on: pull_request` / `on: push`,
no path filters). All six jobs run on this PR (their job-level `if:` only skips branch-deletion
push events), and **all six upload steps carry `if: always()`** — 619, 1312, 2164, 2285, 2412,
2509, the line directly above each `uses:` line. Each step therefore executes once its job has
started, whether or not the preceding steps succeeded; evidence capture must not assume a green
job.

Evidence to capture, per step, from the PR's `PR regression gates` run URL:

1. Step conclusion `success` for all six names in the table (independently of whether files
   were found — for 618/1311/2163 a `warn` outcome with the step green still proves the v7
   action loaded and ran on Node 24).
2. `gh run view <run-id> --log` (or the job logs) grepped for `Node.js 20 is deprecated`:
   zero matches mentioning `upload-artifact`.

The six sites therefore prove: v7.0.1's code executes on `windows-latest`, `ubuntu-latest`
and `macos-latest`, with `name`, `path`, `if-no-files-found: warn|ignore`,
`retention-days: 14`, `compression-level: 6` and `include-hidden-files: false`.

### 6B. `release.yml` — the single use (the hard part)

The release workflow runs only on a `v*.*.*` tag push, and a release tag is a publication act,
not a test (`docs/releasing.md` §3: "Pushing this tag starts publication; it is not a test").
A separate positive control is therefore required. It is one temporary workflow file, exact
content:

```yaml
name: upload-artifact v7 probe (temporary; deleted before merge)

on:
  push:
    paths:
      - '.github/workflows/upload-artifact-v7-probe.yml'

permissions:
  contents: read

jobs:
  probe:
    name: probe (${{ matrix.runner }})
    runs-on: ${{ matrix.runner }}
    strategy:
      fail-fast: false
      matrix:
        runner: [windows-latest, ubuntu-22.04, macos-latest]
    steps:
      - name: Create the probe manifest
        shell: bash
        run: |
          set -euo pipefail
          printf '{"probe":"%s"}\n' "$RUNNER_OS" > "$RUNNER_TEMP/probe-manifest.json"

      - name: Upload the probe manifest with the release.yml inputs
        id: upload
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: probe-manifest-${{ matrix.runner }}-run-${{ github.run_id }}-attempt-${{ github.run_attempt }}
          path: ${{ runner.temp }}/probe-manifest.json
          if-no-files-found: error
          retention-days: 1
          overwrite: false

      - name: Assert the upload reported an artifact
        shell: bash
        run: |
          set -euo pipefail
          test -n "${{ steps.upload.outputs.artifact-id }}"
          test -n "${{ steps.upload.outputs.artifact-url }}"

      - name: Upload a missing path with if-no-files-found error (negative control)
        id: missing
        continue-on-error: true
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: probe-missing-${{ matrix.runner }}-run-${{ github.run_id }}-attempt-${{ github.run_attempt }}
          path: ${{ runner.temp }}/missing-probe-manifest.json
          if-no-files-found: error
          retention-days: 1
          overwrite: false

      - name: Upload the same name twice with overwrite false (negative control)
        id: duplicate
        continue-on-error: true
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: probe-manifest-${{ matrix.runner }}-run-${{ github.run_id }}-attempt-${{ github.run_attempt }}
          path: ${{ runner.temp }}/probe-manifest.json
          if-no-files-found: error
          retention-days: 1
          overwrite: false

      - name: Assert both negative controls failed
        shell: bash
        run: |
          set -euo pipefail
          test "${{ steps.missing.outcome }}" = failure
          test "${{ steps.duplicate.outcome }}" = failure
```

The matrix is exactly the distinct runner images of the `build` job in `release.yml`
(`windows-latest`, `ubuntu-22.04`, `macos-latest`), and the three `upload-artifact` steps use
the exact SHA and the exact `with:` keys and values of `release.yml:1377-1383` (only
`matrix.producer` is replaced by `matrix.runner` in `name`). The `push: paths:` trigger is
used instead of `workflow_dispatch` so the probe cannot depend on the workflow being present
on the default branch.

Probe acceptance evidence, captured from its run URL: all three legs green; the positive step
`success` with non-empty `artifact-id`/`artifact-url`; `missing` and `duplicate` outcomes
`failure`, with the failing steps' `::error::` text copied into the PR body so the reason — not
just the outcome — is evidenced. Expected text: the missing-path control must carry v7.0.1's
`No files were found with the provided path: <runner.temp>/missing-probe-manifest.json. No
artifacts will be uploaded.` (verified in the pinned `dist/upload/index.js`); the duplicate
control must carry the `Received non-retryable error:` line naming the artifact-name conflict
for `probe-manifest-…` (v7.0.1 calls `deleteArtifactIfExists` only when `overwrite` is true, so
the service rejects the second `CreateArtifact`). The artifact list shows the uploaded probe
manifest. This proves the pinned v7 action runs on each release runner image and preserves the
release step's semantics: `if-no-files-found: error` still fails on a missing manifest, and
`overwrite: false` still refuses to clobber.

Deletion and run ordering: commit 2 removes the probe file. Its push **does** start runs —
`pr-regression-gates.yml` and `validate-branch-name.yml` both trigger on push without `paths:`
filters, and both carry `concurrency: <name>-${{ github.ref }}` with `cancel-in-progress: true`.
The other push-triggered workflows stay silent on this commit: `lockfile-check.yml` and
`version-sync-check.yml` are limited by `paths:` to package/version files that commit 2 does not
touch, while `npm-linux-runtime-smoke.yml` (PR/dispatch), `cache-warm.yml` (main/schedule),
`release.yml` (tags) and `bundle-validation.yml` (PR) do not run on push at all. The probe
workflow does not re-run: for a push, GitHub schedules workflows from the pushed commit's tree,
where the probe file no longer exists.

Ordering rule (this is what keeps the deletion push from destroying the evidence):

1. Push commit 1 — starts the probe run, the push-event `PR regression gates` run (group
   `pr-regression-gates-refs/heads/fix/2040-upload-artifact-node24`) and a
   `validate-branch-name` run.
2. Before pushing commit 2, wait until **the probe run is terminal** (all three legs, §6B
   evidence captured) **and the commit-1 `PR regression gates` run is terminal**. If either
   fails, fix on commit 1 and push again; do not delete the probe first.
3. Push commit 2. It cancels any still-in-progress run in the same `pr-regression-gates-<ref>`
   and `validate-branch-name-<ref>` groups — nothing still needed is lost because step 2
   finished first — and starts fresh push runs on the final head (commit 2: v7 change present,
   probe absent).
4. The cited run for §6A/§7.3 is the run covering the **final head**: the `pull_request`
   `PR regression gates` run whose `head_sha` equals the final head SHA (open or refresh the PR
   so the `synchronize` event covers commit 2; the commit-2 push-event run covers the same SHA,
   so cite one of them and record its URL plus `head_sha` in the PR body).

The probes are run-scoped artifacts (retention 1 day); nothing is published, and the final diff
contains no probe file.

## 7. Acceptance criteria

1. The final diff against the base contains exactly three paths and nothing else:
   `.github/workflows/pr-regression-gates.yml` (6 insertions / 6 deletions),
   `.github/workflows/release.yml` (1 insertion / 1 deletion) and
   `plans/2040-upload-artifact-node24.md` as a new file (`A`). Verified by
   `git diff --name-status f83189a6..HEAD` listing exactly those three paths, and
   `git diff --numstat f83189a6..HEAD -- .github/workflows` printing exactly

   ```text
   6	6	.github/workflows/pr-regression-gates.yml
   1	1	.github/workflows/release.yml
   ```

   `.github/workflows/upload-artifact-v7-probe.yml` is absent from the final head.
2. `grep -rn "upload-artifact" .github/` returns exactly the 7 `uses:` lines, all `@v7` or
   `@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a`; zero occurrences of `@v4` or `@ea165f8d…`.
3. A `PR regression gates` run is green on the final head and the six upload steps report the
   conclusions in 6A; the run URL and its `head_sha` (the final head SHA, probe absent) are
   recorded in the PR body; `Node.js 20 is deprecated` has zero matches naming
   `upload-artifact` in that run.
4. The probe run from 6B is green on all three legs with both negative controls failing for the
   asserted reasons (failing-step error text captured); its URL is recorded in the PR body.
5. `docs/releases/**`, all version files and every other action reference are unchanged.
6. Both refs are re-verified with the commands in section 4 before the first edit and before the
   PR is opened or refreshed, and both `refs/tags/v7.0.1` and `refs/tags/v7` still resolve to
   `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (the `v7` tag identity keeps §0's shared-commit
   mitigation true).
7. Landed through a PR from `fix/2040-upload-artifact-node24`; never a direct push to `main`.
8. Every push and the PR creation or refresh were preceded by a runner-gate capture (section
   5.1) with `gate_ok_for_new_pair: true` and `errors: []`; the snapshot paths are recorded in
   the PR body.

## 8. Rollback

Revert the 7-line commit and delete the probe file. No state, schema, migration or published
artifact is involved. Historical release bundles remain valid records and must not be edited.
