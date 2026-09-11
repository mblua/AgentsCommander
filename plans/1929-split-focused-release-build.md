# Plan — #1929 ci: split build from run in the focused issue_1850 release steps

Status: READY_FOR_IMPLEMENTATION

Author: `ac-dev-rust-v4` (room-17-ac-dev-team-v4), round 1, 2026-09-11 UTC. Delivery band: Lite
(1-25), Grinch approval required, no plan digest required by this round.

Issue: [mblua/AgentsCommander#1929](https://github.com/mblua/AgentsCommander/issues/1929), verified
OPEN with `gh issue view 1929`.

This is a workflow-only change. It introduces no product code, no test code, no new abstraction,
dependency, crate, schema, IPC surface, event, configuration, or migration. It inserts three
identical 17-line build steps into `.github/workflows/pr-regression-gates.yml` and changes nothing
else.

## 1. Objective and cause

**Objective.** In each of the three OS regression jobs (`rust-regression`,
`rust-regression-linux`, `rust-regression-macos`), give the focused `issue_1850` release **compile**
its own step with room to finish, so the existing 20-minute bound applies only to **running** the
tests it proves.

**Cause, verified.** Today one step compiles a cold release profile and runs the tests under a
single `timeout-minutes: 20`. On a cold cache the compile alone can consume the entire cap. Issue
#1917 recorded two runs of byte-identical trees with opposite outcomes; independently re-verified
here through the GitHub API on 2026-09-11:

| Run | Event | Job | Step window (UTC) | Outcome |
|---|---|---|---|---|
| `34448858377` | push | `rust-regression-macos` (`102779658789`) | 07:24:56 → 07:44:57 | **failure**, killed at 20m01s |
| `34448861693` | pull_request | `rust-regression-macos` (`102779669186`) | 07:26:57 → 07:44:22 | success, **17m25s** |

The tests had already passed when the kill landed (`test result: ok. 5 passed; 0 failed`). This is a
runner-timing race, not a product failure, and it can hit any of the three legs.

**Precedent already in the repository.** The dedicated `issue-1850-windows-profile` job
(base lines 1012-1075) already splits its work this way: a `--no-run` build step with
`timeout-minutes: 40` and an independent proof step with `timeout-minutes: 20`. #1929 applies the
same split, with the same 40/20 bounds, to the three focused release steps.

## 2. Frozen base and entry gate

Verified on 2026-09-11 in `repo-AgentsCommander`, branch `ci/1929-split-focused-release-build`:

- `HEAD` = `origin/main` = `fe1ab9cc8884ef878b0b5a1581db2dcc66e676d5` after `git fetch origin main`;
  the branch has no commits on top and is not yet pushed (`git ls-remote --heads origin` empty).
- Working index and tree clean (`git status --porcelain=v1 --untracked-files=all` empty).
- Base workflow blob: `5d5cc3b040b6d8fab5f702338a10eb188ffbd3fe` (`git rev-parse HEAD:.github/workflows/pr-regression-gates.yml`),
  1182 lines, raw sha256 `234fa697636415b3055ad81df12d6bf2753e0211a2c59e18399745305358eb97`.

Entry gate for implementation: re-fetch `origin/main`; if it, the local branch head, or their merge
base is no longer `fe1ab9cc…`, stop for re-plan instead of rebasing or substituting a newer base.

## 3. Verified current state (all facts read at the frozen base)

1. The three jobs start at base lines 46 (`rust-regression`, `windows-latest`), 184
   (`rust-regression-linux`, `ubuntu-latest`), 721 (`rust-regression-macos`, `macos-latest`).
   **None of the three has a job-level `timeout-minutes`**, so GitHub's 360-minute default applies.
   Only `issue-1850-windows-profile` has a job cap (75 minutes, line 974).
2. Each leg's focused release block is 3 lines of comment plus a step, at base lines 152-182
   (Windows, 31 lines), 406-432 (Linux, 27 lines), 814-840 (macOS, 27 lines):
   `working-directory: src-tauri`, `shell: bash`, `timeout-minutes: 20`, and
   `cargo test --locked --release --lib issue_1850 -- --test-threads=1 --nocapture 2>&1 | tee test-1850-release.log`
   followed by the guards.
3. The Linux and macOS blocks are byte-identical (sha256
   `bf227ba7b8dbfb060e9a1204c5bf763d261ba2d5c3057a9f7d87fcfdaf4fb9cc`). The Windows block is the
   same block plus a 4-line negative check on `ISSUE1850_WINDOWS_PROFILE_PROOF_OK` (sha256
   `df17662be98c1f68a0026ed5cba176bf368925b347441b3c128fe1c0270a1a54`).
4. Other step bounds in the three jobs are unchanged by this plan: debug 20m (3×), Windows
   integration acceptance 20m, Linux/macOS default-root acceptance 30m.
5. Shell/working-directory: every focused step in the three jobs uses `shell: bash` with
   `working-directory: src-tauri`. The split precedent uses `shell: pwsh` with
   `--manifest-path src-tauri/Cargo.toml` from the repository root — equivalent package selection,
   different presentation. The new step must match **its own job's siblings** (bash + `src-tauri`)
   so cargo selects exactly the same package and target.
6. Flags in the run step: `--locked --release --lib issue_1850 -- --test-threads=1 --nocapture`.
   The planned build step uses the same flags with `--no-run` instead of the harness arguments.
7. Rust caches (base): Windows `shared-key: 'gate-debug'` with `save-if: 'false'`; Linux
   `key: rust-regression-linux`; macOS `key: rust-regression-macos`.
8. Measured durations on the last all-green `pr-regression-gates` run for these steps
   (run `34582429701`, 2026-09-11): focused release Windows 11m49s (09:33:57 → 09:45:46), Linux
   11m55s (09:16:30 → 09:28:25), macOS 9m35s (09:17:39 → 09:27:14). The dedicated split precedent's
   build step ran 10m17s (release) with a 40-minute bound, and its proof step 7m20s.
9. `needs:` does not appear anywhere in the workflow, no job consumes the three jobs' internals,
   and `test-1850-release.log` is referenced only inside its own step (no artifact upload).
10. The workflow runs on `push` (non-main branches) and `pull_request`; the #1917 failure was on a
    **push** run, so a push to this branch is a representative acceptance run.

**Hidden constraints found (reported as required):**

- **H1 — no job cap conflict.** The three jobs have no job-level timeout, so 40m build + 20m run
  fits the 360-minute default. Nothing else in the workflow imposes a shared budget.
- **H2 — Windows release artifacts are not cached between runs.** `save-if: 'false'` and
  `shared-key: 'gate-debug'` mean the `rust-regression` job never saves a cache; the separate
  `cache-warm` workflow maintains a *different* key (`gate-release`). The new build step therefore
  carries the cold release compile on every Windows run. The proven precedent runs an even heavier
  target (integration binary) under the same 40-minute bound.
- **H3 — `--no-run` cannot catch a renamed/gated-out test.** An inert filter in `--no-run` mode
  still exits 0 (mechanism probe in §8, probe P2). The five identifier greps must stay in the run
  step, exactly where they are; the build step must add no assertions beyond cargo's exit code.
- **H4 — artifacts are reused only if the commands match.** With identical profile, target and
  package selection, the subsequent run step does not recompile (probe P1: 1.30s build, then 0.234s
  run with no `Compiling` line). A flag mismatch would silently reintroduce the race.
- **H5 — line endings.** The committed blob is LF; the working tree is CRLF under
  `core.autocrlf=true`. Verification must use git-normalized content (`git hash-object --path=…`,
  `git diff`), never raw file bytes, and must not flip line endings.
- **H6 — the plan file is git-ignored.** Root `.gitignore` line 11 is `/plans/`; like
  `plans/1154-*.md`, this plan is committed only with `git add -f`, and it is not part of the
  runtime change.
- **H7 — issue-text discrepancy, confirmed by the tech lead on 2026-09-11.** The issue says "six
  identifier greps". The base actually has **five test identifiers** per focused release loop (the
  loop body is one `grep -qF` line executed for five names), plus the Windows-only negative
  `ISSUE1850_WINDOWS_PROFILE_PROOF_OK` check. No sixth test identifier exists. Nothing is added,
  renamed, or removed to make the count six.

## 4. The decided modification (exact YAML, three insertions)

For each of the three legs, insert the block below **immediately before** the existing comment line

```
      # IS #1850: focused release execution of every issue_1850 test. Guards
```

(base lines 152, 406, 814; the string occurs exactly three times in the file). The existing
comment, the existing step and its guards stay byte-identical and move down by 17 lines.

Insertion block, verbatim and normative (17 lines, 6-space base indent):

```yaml
      # IS #1929: the release step below used to compile and run under a single
      # 20-minute bound, so on a cold cache the build alone could consume the
      # whole budget and the step could die after the tests had already passed
      # (#1917: the same tree was killed at 20m00s on a push run and went green
      # in 17m25s on a pull request run). Split it the way the dedicated
      # `issue-1850-windows-profile` job already splits its build and proof:
      # the build gets room, the run keeps the tight assertion bound. The build
      # uses the same cargo flags as the run so the run reuses these artifacts
      # instead of rebuilding.
      - name: "IS #1850 focused release build"
        working-directory: src-tauri
        shell: bash
        timeout-minutes: 40
        run: |
          set -euo pipefail
          cargo test --locked --release --lib issue_1850 --no-run

```

Decisions inside the block, each closed:

- **Step name**: `"IS #1850 focused release build"`, parallel to
  `"IS #1850 real-profile build (${{ matrix.mode }})"` in the precedent. The existing run step
  keeps its name `"cargo test (IS #1850 focused, release)"`, so the human-facing identity of the
  proof is preserved.
- **`timeout-minutes: 40`**: the precedent's proven build bound, as the issue and dispatch require.
- **`working-directory: src-tauri` + `shell: bash`**: identical to the sibling steps in the same
  job, so cargo resolves the same package, target and profile as the run step (constraint H4).
- **`--no-run`** placed as a cargo flag after the filter, mirroring the precedent's argument order.
  The `issue_1850` filter is inert in `--no-run` mode and exists only to keep the flags symmetric
  with the run command.
- **`set -euo pipefail`**: same failure semantics as the sibling steps; no `continue-on-error`,
  no `|| true`, no weakening.

Line-number map at the base, then after the edit (17 added lines per leg, cumulative):

| Leg | Insert before base line | Release block base | Release block after edit | New file lines |
|---|---|---|---|---|
| Windows | 152 | 152-182 | 169-199 | 1182 + 51 = 1233 |
| Linux | 406 | 406-432 | 440-466 | |
| macOS | 814 | 814-840 | 865-891 | |

Nothing else changes: no renames, no timeout changes on existing steps, no flag changes, no
reordering, no cache changes, no new job, no `needs:`.

## 5. Preserved guards (unchanged, per leg)

In each focused release run step, all of the following remain byte-identical:

- `set -euo pipefail` and the `2>&1 | tee test-1850-release.log` pipeline (pipefail catches a
  failing test; it is load-bearing through the `tee`).
- The five test identifier greps, executed by the existing loop for these exact names:
  1. `config::profile::tests::issue_1850_config_dir_name_table_is_profile_independent`
  2. `config::tests::issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome`
  3. `config::tests::issue_1850_overrides_keep_precedence_and_identity_over_canonical_home`
  4. `config::tests::issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes`
  5. `config::tests::issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once`
- The anchored result check `'^test result: ok\. [1-9][0-9]* passed; 0 failed'` on
  `test-1850-release.log`, with its existing error message.
- Windows only: the negative check `if grep -qF 'ISSUE1850_WINDOWS_PROFILE_PROOF_OK'
  test-1850-release.log; then … exit 1` — the ordinary Windows run must not claim the real-profile
  proof.

Out of scope and untouched: the focused debug steps (3×), the Windows integration acceptance step,
the Linux/macOS default-root acceptance steps, the `issue-1850-windows-profile` job, and every other
job in the workflow.

**Count discrepancy (H7), recorded for the Grinch:** the issue's "six identifier greps" is a
wording error; the base has five test identifiers plus the Windows negative check. This plan
preserves what exists and adds no sixth identifier.

## 6. Edge and failure behavior

- **Cold cache (the defect)**: the build runs under its own 40-minute bound; the run keeps its
  20-minute assertion bound. A compile that would have killed the proof now either finishes and
  leaves the run untouched, or fails explicitly as a **build** timeout instead of a misleading
  "test timed out" after the tests already passed.
- **Warm cache**: the build step is a no-op (`Finished` in seconds) and the run step behaves as
  today.
- **Compile error**: the job fails at the build step; the run step is skipped; nothing is masked.
- **Test failure**: unchanged — the run step fails via pipefail and the guards keep their messages.
- **Renamed or gated-out test**: the build step still succeeds (H3); the run step's identifier
  greps catch it exactly as today. Guards are not moved into the build step.
- **Windows refusal route**: the negative check remains in the run step; the build step cannot
  weaken it.
- **Job wall clock**: the compile phase is relocated, not duplicated. Expected job duration is
  unchanged on a healthy run; worst-case cold adds only the difference between the old 20-minute
  kill and the new 40-minute ceiling.
- **No guard weakening**: no `continue-on-error`, no `|| true`, no count relaxation, no flag or
  cache change, no test or product edit.

## 7. File inventory (planned)

| Action | Path | Change |
|---|---|---|
| MODIFIED | `.github/workflows/pr-regression-gates.yml` | +51 / −0 lines: the same 17-line build step inserted before each of the three focused release blocks. Expected git diff: exactly 3 hunks, `51 0` numstat. |
| ADDED | `plans/1929-split-focused-release-build.md` | This plan. Git-ignored via `/plans/` (H6); commit requires `git add -f` when the branch is committed. No runtime effect. |

No other file is added, removed, or modified. No `src-tauri/` file, no test file, no frontend file,
no other workflow.

## 8. Focused validation and acceptance

### 8.1 Local, deterministic, before the first push

- **V1 — YAML parses.** PyYAML 6.0.3 is present in this environment:
  `python -c "import yaml; yaml.safe_load(open('.github/workflows/pr-regression-gates.yml', encoding='utf-8')); print('yaml ok')"`
- **V2 — structural verifier** (below) exits `PASS`. It parses the YAML, asserts each leg has the
  build step immediately before the run step with the exact name, bounds, shell,
  working-directory and commands, re-hashes the 17-line insertion above each anchor, and re-hashes
  each of the three release blocks against their base digests.
- **V3 — normalized diff is exactly the intended change.**
  `git diff --numstat -- .github/workflows/pr-regression-gates.yml` → `51  0`;
  `git diff -U0 -- .github/workflows/pr-regression-gates.yml | grep '^@@'` → 3 hunks.
- **V4 — exact bytes (line-ending robust).** If the insertion block is copied verbatim:
  `git hash-object --path=.github/workflows/pr-regression-gates.yml .github/workflows/pr-regression-gates.yml`
  must equal `a0e2afd7a1cc55301c7b8f6908087701e472c617` (expected post-edit blob), against the base
  blob `5d5cc3b040b6d8fab5f702338a10eb188ffbd3fe`. The raw LF sha256 of the expected result is
  `c5f125c893ea43b397130de7a2e66a9f12654ca250f8bfa00198d749cc9184a5`; the LF-normalized sha256 of
  the insertion is `bcd0b8fef32b8154fc2404eb020869f8db08a1eca1658629df1d2c386283f529`.
- **V5 — preserved-block digests** (also enforced by V2). Release block digests, as 27/31-line
  slices starting at each anchor, must remain:
  Windows `df17662be98c1f68a0026ed5cba176bf368925b347441b3c128fe1c0270a1a54`,
  Linux/macOS `bf227ba7b8dbfb060e9a1204c5bf763d261ba2d5c3057a9f7d87fcfdaf4fb9cc`.

### 8.2 Mechanism probes already run for this plan (disposable, not a repo build)

- **P1** — a throwaway two-test crate in agent scratch: `cargo test --locked --release --lib probe_hit --no-run`
  compiled in 1.30s; the immediately following
  `cargo test --locked --release --lib probe_hit -- --test-threads=1 --nocapture` finished in 0.234s
  with **no recompilation**. Artifact reuse across steps is real (H4).
- **P2** — the same probe with a filter matching nothing and `--no-run` exits 0 and prints
  `Finished`. The compile step cannot false-fail or catch a renamed test (H3).

### 8.3 CI, authoritative acceptance

- **V6 — push the branch** (do not open the PR until the local checks pass; the workflow runs on
  both events, and #1917's failure was on a push). Acceptance:
  - `rust-regression`, `rust-regression-linux`, `rust-regression-macos` all green;
  - each leg shows `IS #1850 focused release build` as its own step with a 40-minute bound,
    followed by the unchanged `cargo test (IS #1850 focused, release)` step with its 20-minute
    bound and a run duration far below it;
  - the run step's log still shows the five identifiers and the anchored
    `test result: ok. … 0 failed` line; the Windows log does not contain
    `ISSUE1850_WINDOWS_PROFILE_PROOF_OK`;
  - no other job's behavior changes.
- **V7 — Grinch evidence package** (not just the diff): the V2 `PASS` output, the V3 numstat and
  hunk list, the V4/V5 hashes, the P1/P2 probe records in §8.2, and the CI run URL with per-step
  timings from V6.

### 8.4 Ready-to-run structural verifier (V2)

Save as `1929-verify.py` anywhere outside the repo (or run via heredoc) and invoke from the
repository root; it is line-ending agnostic.

```python
#!/usr/bin/env python3
"""#1929 focused-release build/run split -- local structural verifier."""
import hashlib
import sys

import yaml

path = sys.argv[1] if len(sys.argv) > 1 else ".github/workflows/pr-regression-gates.yml"
text = open(path, "rb").read().decode("utf-8").replace("\r\n", "\n")
lines = text.split("\n")

ANCHOR = "      # IS #1850: focused release execution of every issue_1850 test. Guards"
BUILD_NAME = "IS #1850 focused release build"
RUN_NAME = "cargo test (IS #1850 focused, release)"
INSERT_SHA = "bcd0b8fef32b8154fc2404eb020869f8db08a1eca1658629df1d2c386283f529"
RUN_SHA = {
    "windows": "df17662be98c1f68a0026ed5cba176bf368925b347441b3c128fe1c0270a1a54",
    "unix": "bf227ba7b8dbfb060e9a1204c5bf763d261ba2d5c3057a9f7d87fcfdaf4fb9cc",
}
IDENTIFIERS = [
    "config::profile::tests::issue_1850_config_dir_name_table_is_profile_independent",
    "config::tests::issue_1850_unsuffixed_executables_select_canonical_home_for_every_probe_outcome",
    "config::tests::issue_1850_overrides_keep_precedence_and_identity_over_canonical_home",
    "config::tests::issue_1850_lazy_helper_never_probes_unsuffixed_or_overridden_routes",
    "config::tests::issue_1850_lazy_helper_probes_suffixed_routes_marker_first_then_write_once",
]

failures = []


def check(ok, message):
    if not ok:
        failures.append(message)


def digest(block_lines):
    return hashlib.sha256(("\n".join(block_lines) + "\n").encode()).hexdigest()


doc = yaml.safe_load(text)
for job_name in ("rust-regression", "rust-regression-linux", "rust-regression-macos"):
    steps = doc["jobs"][job_name]["steps"]
    names = [s.get("name") for s in steps]
    check(BUILD_NAME in names, f"{job_name}: new build step missing")
    check(RUN_NAME in names, f"{job_name}: release run step missing")
    if BUILD_NAME in names and RUN_NAME in names:
        i = names.index(BUILD_NAME)
        build, run = steps[i], steps[i + 1]
        check(run.get("name") == RUN_NAME, f"{job_name}: run step is not directly after the build step")
        check(build.get("timeout-minutes") == 40, f"{job_name}: build timeout is {build.get('timeout-minutes')}, want 40")
        check("--no-run" in build.get("run", ""), f"{job_name}: build step has no --no-run")
        check("--locked --release --lib issue_1850" in build.get("run", ""), f"{job_name}: build step flags drifted")
        check(build.get("working-directory") == "src-tauri", f"{job_name}: build working-directory drifted")
        check(build.get("shell") == "bash", f"{job_name}: build shell drifted")
        check(run.get("timeout-minutes") == 20, f"{job_name}: run timeout is {run.get('timeout-minutes')}, want 20")
        check("cargo test --locked --release --lib issue_1850 -- --test-threads=1 --nocapture" in run.get("run", ""),
              f"{job_name}: run command drifted")
        check(run.get("working-directory") == "src-tauri", f"{job_name}: run working-directory drifted")
        check(run.get("shell") == "bash", f"{job_name}: run shell drifted")

idx = [i for i, line in enumerate(lines) if line == ANCHOR]
check(len(idx) == 3, f"anchor count = {len(idx)}, want 3")
for i in idx:
    insert = digest(lines[i - 17:i])
    check(insert == INSERT_SHA, f"inserted block above anchor at line {i + 1} differs from the plan (sha {insert[:12]})")
    kind = "windows" if "ISSUE1850_WINDOWS_PROFILE_PROOF_OK" in "\n".join(lines[i:i + 31]) else "unix"
    size = 31 if kind == "windows" else 27
    block = lines[i:i + size]
    got = digest(block)
    check(got == RUN_SHA[kind], f"release block at line {i + 1} changed ({kind}, sha {got[:12]})")
    check(lines[i + 4] == '      - name: "cargo test (IS #1850 focused, release)"', f"line {i + 5} is not the release step name")

for ident in IDENTIFIERS:
    check(text.count(ident) == 6, f"identifier occurrences = {text.count(ident)}, want 6: {ident}")
check(text.count("'ISSUE1850_WINDOWS_PROFILE_PROOF_OK' test-1850-release.log") == 1,
      "Windows release refusal-only check missing or duplicated")
check(text.count("grep -qE '^test result: ok\\. [1-9][0-9]* passed; 0 failed' test-1850-release.log") == 3,
      "anchored release result check count != 3")
check(text.count("test-1850-release.log") == 10, "test-1850-release.log reference count drifted")

if failures:
    print(f"FAIL ({len(failures)} check(s))")
    for f in failures:
        print(" -", f)
    sys.exit(1)
print("PASS: #1929 build/run split verified (3 legs; run blocks byte-preserved; guards intact)")
```

Note on `test-1850-release.log == 10`: per leg the log name appears on the `tee` line, on the
single loop `grep -qF` line and on the anchored-result line (3×3 = 9), plus the Windows negative
check (1). The identifier loop runs five times but is one source line.

### 8.5 Acceptance criteria

1. The three legs each gain exactly one `IS #1850 focused release build` step with
   `timeout-minutes: 40` and the `--no-run` release compile.
2. The three focused release run steps — comment, name, bounds, command, and all guards — are
   byte-identical to the base (V2/V5 digests).
3. A cold-cache release compile can no longer kill the assertion step: the build has room, the run
   keeps its 20-minute bound, and a build timeout is attributed to the build step.
4. All five test identifiers, the anchored result check and the Windows negative check survive
   unchanged; no sixth identifier is added (H7).
5. No product, test, or other workflow file changes; `git diff --numstat` is `51 0` on one file.
6. A real `pr-regression-gates` push run is green on the three OS legs with the new topology and
   unchanged guard output.

## 9. Environment risk and disclosed residual risk

**Environment risk (stated as required).** This is a CI-only change against GitHub-hosted runners
whose cold-cache timing cannot be reproduced or falsified locally; the authoritative proof is a
real workflow run (V6), and the Grinch must review the verification evidence (V7), not only the
diff. The defect itself is nondeterministic: #1917 produced opposite outcomes from byte-identical
trees. No expensive build, GUI or interactive test is launched for this plan; the only executed
build was a disposable two-test scratch crate (P1/P2, ~2 seconds).

Residual risks, disclosed and accepted:

- **R1 — 40m is a bound, not a guarantee.** A cold runner slower than 40 minutes still fails the
  leg, now with an explicit build timeout. Accepted: it is the precedent's proven budget for a
  heavier target, and the run bound stays tight by design.
- **R2 — Windows pays the cold release compile every run** (H2). Accepted; it is the same work the
  current single step already performs, only relocated.
- **R3 — line-ending hazard** (H5). Mitigated by verifying with git-normalized hashes only.
- **R4 — base drift.** Mitigated by the §2 entry gate.

## 10. Blockers and next step

No blocker to planning. Implementation is intentionally **not** started: no workflow, product or
test edit and no commit exists yet on `ci/1929-split-focused-release-build`. Next step is the tech
lead's explicit instruction after this plan's review, at which point §4 is copied verbatim, §8.1 is
run, and §8.3 is executed on a push.
