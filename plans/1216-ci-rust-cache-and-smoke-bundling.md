# Implementation Plan: #1216 Rust regression jobs recompile from scratch every run and build installers they discard

Status: READY_FOR_IMPLEMENTATION

Full path. Written by the architect at Step 4, enriched by `dev-rust` (Step 5) and `dev-rust-grinch` (Step 6), certified by the architect at Step 7 consensus round 1, and **amended by the architect at Step 7 consensus round 2 after the round-1 certification was invalidated by a defect found in production.** Round 1's `Plan-SHA256` is superseded.

**Read Section 15 first if you are implementing this.** It is the record of the round-2 amendment and is the authoritative reading wherever it contradicts anything earlier, including Section 14. As in round 1, every section the amendment changed was rewritten in place rather than merely annotated, so the sections and the record agree.

**Section 14 remains the record of the round-1 resolution** and is authoritative over Sections 1 through 13 except where Section 15 overrides it. Sections 12 and 13 are the enrichers' own records; **read them for evidence, not for instructions**, because five of their round-1 recommendations were resolved differently from what they proposed, and one round-1 resolution has since been reversed.

Every implementation decision is closed. There is no `TBD`, no competing alternative and nothing left to the implementer, who is expected to start cold with no knowledge of the discussion that produced this. **One risk is explicitly open and accepted rather than closed**, with its trade-off and its lever recorded in Section 10.6; that is a decision, not an unfinished item.

## 1. Issue, baseline and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1216 (`ci: Rust regression jobs recompile from scratch every run and build installers they discard`).
- Branch: `fix/1216-ci-rust-cache-and-smoke-bundling`, cut from `main`.
- **Baseline for every coordinate, command and number in this plan: `509b7b3915313cb1c6cde6515f1c29d0c20f28b8`** (`main`, and the current tip of the branch). Every `file:line` below was read at that commit and independently re-verified by `dev-rust` (Section 12.4.4).
- Measurement baseline: workflow run **`30780031337`**, event `pull_request`, head branch `fix/1188-watcher-activity-autorefresh`, head sha `b5529d20f24f4c6ee2f481e8f670904ac792b110`, created `2026-08-03T02:44:42Z`, updated `2026-08-03T03:03:54Z`.
- Runner image observed on the baseline run: `win25-vs2026/20260728.188`. Toolchain at baseline: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, host `x86_64-pc-windows-msvc`.
- **The runner image is not a property of this repository and must never be treated as a baseline constant.** GitHub serves more than one `win25-vs2026` generation concurrently and every job draws one independently. M1 and M2 drew both `20260728.188` and `20260714.173`, including two jobs of a single run drawing different generations. Section 2.8 carries the evidence. An earlier revision of this plan recorded the image above as though it were fixed, and Section 4.1 built a cache-key decision on that mistake; Section 15 records the amendment.
- Repository is `public`, default branch `main`. Runner minutes are not billed; the cost this plan attacks is developer wall-clock.
- Delivery classification: FULL.

**Objective.** The two Windows jobs of `pr-regression-gates.yml` must stop recompiling every Rust dependency from scratch on every run, and `windows-release-cli-smoke` must stop producing installer bundles that nothing consumes. The result must be demonstrated by measured before and after CI data, and the measurement must be able to tell cache acceleration apart from runner luck.

**Non-objective.** This is not a redesign of the gate and not an attempt to make the gate cheap in absolute terms. No Rust or TypeScript source file is touched.

**One test surface does change, and it is not hidden.** Removing default bundling from `windows-release-cli-smoke` means a PR that today fails inside the WiX or NSIS bundlers would no longer fail there. Section 4.4 restores that coverage through a narrowly triggered workflow rather than asserting that the tag path is equivalent, because it is not: `release.yml` builds NSIS only, so nothing else in CI would ever execute WiX. **The earlier claim that gate pass/fail semantics are entirely unchanged was wrong and has been removed.**

**No savings figure is asserted up front.** Section 9 separates what is arithmetically certain from measured step data from what is genuinely unknown until measured, and Section 9.7 states the hard ceiling on what a cache can possibly recover here.

## 2. Verified current state

### 2.1 The cache is pointed at a directory that never holds build artefacts

`pr-regression-gates.yml:70-73` and `:112-115` both configure:

```yaml
      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: src-tauri -> target
```

Root `Cargo.toml:1-4` declares the workspace (`members = ["src-tauri", "crates/session-bridge"]`), so cargo emits to `<root>/target`. Four artefacts corroborate this: `scripts/copy-testable-binary.mjs:6-9`, `scripts/smoke-cli-release-windows.ps1:37-41`, `release.yml:111-112`, and the runner's own `Built application at: D:\a\AgentsCommander\AgentsCommander\target\release\agentscommander.exe`. `Cargo.lock` exists only at the repository root.

Confirmed in the baseline log of `rust-regression` (job `91582715554`) at `02:46:30.6125110Z`: `No cache found.`

Per the action's README the `workspaces` default is `. -> target`, so the current value is the one setting that guarantees the cached directory holds no build output.

**A second defect the same input causes, found by `dev-rust`.** `src/workspace.ts` classifies packages with `getPackagesOutsideWorkspaceRoot`, filtering on `!pkg.manifest_path.startsWith(this.root)` where `this.root` comes from `workspaces`. With `root = <repo>/src-tauri`, `crates/session-bridge` does not start with that prefix, so it is misclassified as an external dependency and would be kept in the cache. With `root = <repo>` both workspace members are correctly excluded. Correcting the input fixes package classification as well as the path.

### 2.2 No default-branch cache can exist

`pr-regression-gates.yml:3-12` lists `main` under `push.branches-ignore`. Verified independently: **none of the five workflows runs on `main`.** `lockfile-check.yml`, `validate-branch-name.yml` and `version-sync-check.yml` carry the same block; `release.yml` triggers only on `v*` tags.

GitHub restricts cache reads to the current branch and the default branch, so with no run ever executing on `main`, every feature branch starts from a guaranteed miss. `dev-rust` confirmed this empirically: **zero cache entries exist with `ref: refs/heads/main`.**

### 2.3 The cache quota, corrected

At the time of writing, `GET /actions/cache/usage` returned `10 140 641 209` bytes across 101 entries against GitHub's 10 GB limit; `dev-rust` re-measured shortly after and got `9 752 203 969` across 97. The movement between two readings minutes apart is itself the evidence: the repository is continuously evicting.

Composition as measured by `dev-rust`:

| Prefix | Entries | Size |
|---|---|---|
| `v0-rust-*` | 49 | 7.12 GB |
| `node-cache*` | 50 | 1.96 GB |

Every Rust entry is 148 MB, and that number is meaningful in two directions. It is small because the entries cache an empty `src-tauri/target`, so **148 MB is a direct calibration of the compressed weight of the `~/.cargo` portion alone.**

**How large the corrected entry will be, with the arithmetic corrected.** An earlier draft of this plan reasoned from `target/debug` measuring 21.71 GB locally. `dev-rust` decomposed that figure and it overstates the cacheable tree by roughly 4x:

| Component | Size | Cached? |
|---|---|---|
| `target/debug` total | 21.71 GB | |
| `target/debug/incremental` | 10.26 GB | **No.** `rmExcept(profileDir, {"build",".fingerprint","deps"})` deletes it, and it does not exist in CI at all: the baseline log shows `CARGO_INCREMENTAL: 0` in the step environment. |
| workspace artefacts inside `deps` | 4.97 GB (36 files) | **No.** Pruned by `getPackagesOutsideWorkspaceRoot`. |
| dependency artefacts inside `deps` | 5.10 GB (2 964 files) | **Yes.** |
| rest of `target/debug` top level | ~1.4 GB | **No.** |

So the candidate tree is of the order of **5 GB uncompressed, not 21.71 GB**, and `size_in_bytes` from the API reports the compressed size. `target/release` measures only 2.59 GB in total, so **the two entries are not symmetric in cost**: the debug side dominates.

The decomposition is MEASURED; any projection of the final compressed size is UNVERIFIED and this plan does not assert one. What follows from it is that the quota risk is real but considerably smaller than the raw 21.71 GB suggests, and that a fallback rule must not be triggered pre-emptively.

### 2.4 The discarded bundling is real, and it is small

Measured from the timestamped log of job `91582715500`, step `Build Windows release binary` (902 s):

| Segment | Duration |
|---|---|
| tauri CLI startup, config merge | ~2 s |
| vite build (`beforeBuildCommand`) | 4.7 s |
| **cargo release compilation** | **848.6 s** |
| MSI bundling | 17.5 s |
| NSIS bundling | 29.4 s |

**Bundling is 46.9 s of 902 s, that is 5.2 % of the step and 4.5 % of the job.** Compilation is 94 %. Anyone reading #1216's "Problem 2" and expecting the bundling change to carry the improvement will misread Section 9's result.

The bundling also reaches the public internet on every run (`Downloading .../nsis-3.11.zip`, `.../nsis_tauri_utils.dll`); `dev-rust` confirmed by direct execution that `--no-bundle` eliminates those requests entirely.

### 2.5 Full baseline step data

`rust-regression` (job `91582715554`), job wall-clock **1148 s**:

| # | Step | Duration |
|---|---|---|
| 1-7 | setup, checkout, node, npm pin, `npm ci`, `npm run build`, rust toolchain | 102 s |
| 8 | Rust cache (restore) | 3 s (`No cache found.`) |
| 9 | `cargo check --all-targets` | 264 s |
| 10 | `cargo clippy --all-targets -- -D warnings` | 72 s |
| 11 | `cargo test --lib --bins --tests` | 594 s |
| 20 | **Post Rust cache (save)** | **99 s** |
| 21-23 | post node, post checkout, complete | 13 s |

`windows-release-cli-smoke` (job `91582715500`), job wall-clock **1033 s**:

| # | Step | Duration |
|---|---|---|
| 1-7 | setup through cache restore | 68 s |
| 8 | `npm run build:prod` | 902 s (46.9 s of it bundling) |
| 9 | `npm run smoke:cli-release-windows` | 12 s |
| 10 | upload artifact | 2 s |
| 18 | **Post Rust cache (save)** | **40 s** |
| 19-21 | post node, post checkout, complete | 7 s |

Gate wall-clock is `max(jobs)` = 1148 s, set by `rust-regression`.

### 2.6 Cargo work counters at baseline, and why they matter more than durations

Extracted from the `rust-regression` baseline log after stripping ANSI escapes (the job runs with `CARGO_TERM_COLOR: always`, so a naive grep finds nothing):

| Counter | Baseline value |
|---|---|
| `Compiling ` lines | **654** |
| `Checking ` lines (cargo; excludes `##[group]Checking out the ref`) | **205** |
| **Units worked = Compiling + Checking** | **858** |
| `Fresh ` lines | 18 |
| `Downloaded ` lines | **483** |

`Compiling agentscommander-new` appears exactly 3 times, once under `cargo check`, once under `cargo clippy` and once under `cargo test`, which is the workspace crate being rebuilt three times per run.

These counters are the backbone of the corrected measurement protocol. Unlike step durations they do not move with runner allocation: a restored cache makes dependency units report `Fresh` instead of `Compiling`, deterministically. Section 9 uses them as the primary cache evidence for exactly that reason.

### 2.7 What `windows-release-cli-smoke` consumes, and Decision 3 validated on real Windows

`package.json:9` defines `build:prod` as `cross-env BUILD_PROFILE=prod tauri build --config src-tauri/tauri.prod.conf.json && node scripts/copy-testable-binary.mjs`. `src-tauri/tauri.conf.json:17-36` sets `"bundle": {"active": true, "targets": "all", ...}` with `windows.nsis.installerHooks: "./nsis/hooks.nsh"`; `src-tauri/nsis/hooks.nsh` exists. `tauri.prod.conf.json` does not override `bundle`.

The smoke consumes exactly two plain executables under `target/release` (`scripts/smoke-cli-release-windows.ps1:37-41`), run under `powershell.exe` and `pwsh.exe`. It consumes no installer and no `target/release/bundle/**` path. `dev-rust` grepped the whole repository excluding `node_modules` for `target/release/bundle`, `bundle/nsis`, `bundle/msi`, `setup.exe`, `.msi`, `makensis` and `--bundles`: the only hits are `release.yml:17` and end-user documentation. **Nothing consumes the gate's bundle outputs.**

`BUILD_PROFILE` is load-bearing: `src-tauri/build.rs:8-20` reads it, emits `cargo:rustc-env=BUILD_PROFILE` and `cargo:rerun-if-env-changed=BUILD_PROFILE`, and `src-tauri/src/config/profile.rs:15` consumes it as `env!("BUILD_PROFILE")`.

**Decision 3 is validated end to end, not argued.** `dev-rust` executed the exact command sequence on real Windows (Section 12, and Section 8 step 1 records it as already passed):

| Step | Time | Exit |
|---|---|---|
| `tauri build --no-bundle` | 339.8 s | 0 |
| `node scripts/copy-testable-binary.mjs` | 0.2 s | 0 |
| `npm run smoke:cli-release-windows` | 6.3 s | 0 (`passed=4 skipped=0 failed=0`, 28 assertions) |

`Built application at: ...\target\release\agentscommander.exe` present; `Running light to produce`, `Running makensis to produce`, `Info Verifying wix package`, `Info Verifying NSIS package`, `Info extracting WIX`, `Info extracting NSIS` all absent; `target/release/bundle` absent before and after; no `Downloading https://github.com/tauri-apps/...` lines. `Running beforeBuildCommand 'npm run build'` observed at 2.4 s, confirming `--no-bundle` still runs the frontend build.

### 2.8 GitHub serves two `win25-vs2026` image generations concurrently, and every job draws one independently

This fact was not known when Sections 4.1 and 10.6 were first written, and it invalidates the decision they recorded. It is placed here, in verified current state, so that no future editor re-derives it or re-opens the decision it forces.

Diagnosed by `dev-rust` after the implementation landed and **independently verified by the tech lead from the raw job logs, six jobs for six**:

| Job | Run | Runner image | Env hash |
|---|---|---|---|
| `rust-regression` (gate debug) | M1 | `win25-vs2026/20260728.188` | `3d5bdf05` |
| `windows-release-cli-smoke` (gate release) | M1 | `win25-vs2026/20260714.173` | `cfee4d59` |
| `warm-debug` (writer debug) | M2 | `win25-vs2026/20260714.173` | `cfee4d59` |
| `warm-release` (writer release) | M2 | `win25-vs2026/20260728.188` | `3d5bdf05` |
| `verify-debug-cache` (probe) | M2 | `win25-vs2026/20260728.188` | `3d5bdf05` |
| `verify-release-cache` (probe) | M2 | `win25-vs2026/20260728.188` | `3d5bdf05` |

Correlation between image and hash is perfect, and `ImageOS` is constant on all six.

**The decisive observation is that this is concurrent rollout, not drift over time.** `warm-debug` and `warm-release` are two jobs of the **same run**, `30811303027`, and they drew different image generations. No ordering, scheduling or freshness argument can explain that away, and no amount of waiting makes it settle.

**Mechanism.** In `src/config.ts` at the commit the runs resolved (`e18b497796c12c097a38f9edb9d0641fb99eee32`), the environment hasher consumes `` `${key}=${value}` ``, that is the variable **name and its value**. This resolves what would otherwise look contradictory in the logs: the `Environment considered` block is byte-identical across all six jobs because that block prints names only, while the digests differ because the values differ. `dev-rust` reimplemented the algorithm offline and **the partition reproduces exactly**, the same four jobs in one group and the same two in the other. Absolute digest values do not reproduce; `dev-rust` labels that UNVERIFIED and attributes it to one unknown constant that is identical across all six jobs and therefore cannot produce the split. In the same model, removing `ImageVersion` collapses both image generations to a single digest.

**Methodological warning, and it matters more than it looks.** The `Runner Image` log group prints **two** `Version:` lines, the Image Provisioner version first. A naive first-match extraction returns `20260707.563`, which is identical on all six jobs and makes the images appear the same, hiding the defect completely. The real image version appears only in the `Included Software` and `Image Release` URLs. Anyone re-checking this must read those URLs.

## 3. Scope

**In scope.**

- `.github/workflows/pr-regression-gates.yml`: both `Rust cache` steps, and the build command of `windows-release-cli-smoke`.
- New `.github/workflows/cache-warm.yml`.
- New `.github/workflows/bundle-validation.yml`.
- `package.json`: one added script.
- `.gitignore`: one added line.

**Out of scope, and not folded in.**

- The `push` plus `pull_request` double trigger. Verified to cost no wall-clock. Section 11.3 records that it does cost cache quota and that this plan neutralises that as a side effect without changing the trigger.
- Local pre-push gates.
- Issue #1217 (`crates/session-bridge` not covered by CI). Both gate jobs run cargo with `working-directory: src-tauri`, so the second workspace member is never built; the warm workflow mirrors that restriction deliberately (Section 11.2).
- Issue #1218 (`npm test` flaky under CPU load).
- `release.yml:74`, which carries the identical `workspaces` defect. Section 11.1 explains why correcting it here would work against the objective.
- `Pin npm version`, 47 s and 18 s. Real, unrelated, untouched.
- `cache-workspace-crates`. Section 11.5 records why it is deliberately not used, so it is not re-litigated.

## 4. The decided solution

### 4.1 Decision 1: point the cache at the real workspace target

All seven `swatinem/rust-cache@v2` steps introduced or edited by this plan use an identical input block apart from `shared-key` and `save-if`. **Inputs that feed the cache key must match exactly across every step sharing a `shared-key`, or the key silently diverges and the cache becomes unreachable.**

`rust-regression` (`pr-regression-gates.yml:70-73`) becomes:

```yaml
      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-debug'
          env-vars: 'ImageOS'
          save-if: 'false'
```

`windows-release-cli-smoke` (`pr-regression-gates.yml:112-115`) becomes the same with `shared-key: 'gate-release'`.

**Why `shared-key` is mandatory rather than cosmetic.** Verified in `src/config.ts` at the commit running in CI (`e18b497796c12c097a38f9edb9d0641fb99eee32`): absent `shared-key`, the key ends in the GitHub job id. The warm workflow lives in a different file with different job ids, so without `shared-key` on both sides it would populate caches the gate can never read, and the change would produce zero benefit while looking correct.

Note for future editors: an input `add-job-id-key` exists, defaulting to `true`. `shared-key` short-circuits that branch entirely, but anyone removing `shared-key` and setting `add-job-id-key: false` instead would collide the two gate keys into one.

**Why `env-vars: 'ImageOS'`, and why `ImageVersion` is deliberately excluded.** `action.yml` documents `add-rust-environment-hash-key` (default `true`) as hashing Cargo manifests, lock files, toolchain files and **the values named in `env-vars`**, and `src/config.ts` hashes each entry as `` `${key}=${value}` ``, name and value. rustc identity participates in the key; the runner image and its MSVC toolset do not.

An earlier revision of this plan set `env-vars: 'ImageOS ImageVersion'` to close the U2 stale-MSVC-toolset risk. **That decision was wrong, it was implemented faithfully, and it is corrected here.** Its defect and the evidence are in Section 2.8: `ImageVersion` is not a slowly-changing constant, GitHub serves two `win25-vs2026` generations concurrently, and every job draws one independently.

**The specific analytical error, recorded so the same class of mistake is not repeated.** The earlier text argued that the failure mode of adding these variables is benign because "if a variable is absent the action hashes nothing extra". That analysed the **absent** case. The case that actually occurs is the **varying** case, and it is not benign, because a key input that varies per job independently of the repository does not add entropy, it destroys addressability.

**What the varying case costs, and why it is worse than a plain miss.**

1. The cache half of #1216 becomes a **coin flip rather than a clean failure.** Each gate matches whichever generation its writer happened to draw, roughly half the time, and an intermittent success is harder to diagnose than a consistent miss because it can present as working.
2. **Certification could pass by luck.** Group B could report `cache-hit == 'true'` with units worked in single digits while the key underneath is nondeterministic, which is a false pass on the exact criterion Section 9 was rewritten to protect.
3. It makes the Section 4.2 probe guarantee **unobtainable**, not merely weaker. See Section 6.6.
4. **A second-order cost the earlier revision never priced.** Even with no concurrent rollout at all, `ImageVersion` in the key means every image bump invalidates both entries and forces a full cold warm. Images bump roughly fortnightly, which is inside the seven-day eviction window that the schedule trigger exists to defeat. The design would have spent much of its life recompiling even in the deterministic case that was assumed.

**`ImageOS` is kept.** It was constant across all six jobs of Section 2.8's evidence, and it moves only on a major image transition such as `win22` to `win25`. That is the coarse, deliberate, slow rotation the key should track, so it costs nothing and retains a guard against the one image change that certainly does move the toolchain.

**The residual risk, stated as what it actually is rather than as closed.** A point release inside `win25-vs2026` could move the MSVC toolset and let stale native artefacts from `-sys` crates exact-hit against a new linker. Section 10.6 carries this openly and names the lever. It is not closed and this plan does not claim it is.

**Why hashing a narrower MSVC identity instead is not a better third option**, recorded to foreclose an obvious future proposal. Substituting something like a `VCToolsVersion` probe for `ImageVersion` is either equally broken or unnecessary, and which one it is does not change the decision. If the two concurrently served generations carry different MSVC toolsets, then any faithful MSVC-identity input reproduces exactly the same per-job coin flip. If they carry the same toolset, then there was no correctness risk to close between them and the input buys nothing. It would also require a new step to compute and export the value, widening the scope of a one-token fix. `prefix-key` is the better lever because it rotates on a human decision instead of on a draw.

**Why two keys and not one.** Both jobs cache the same `<root>/target`, but they populate disjoint subtrees: `rust-regression` works entirely in `target/debug`, `windows-release-cli-smoke` entirely in `target/release`. A single key would force both gate jobs to download and decompress both halves on the critical path when each needs one. The measured asymmetry reinforces this: `target/debug` is 21.71 GB locally against `target/release` at 2.59 GB, so a combined entry would make the cheap job pay for the expensive one.

The cost of two keys, quantified by `dev-rust`: each entry also contains `~/.cargo`, so the registry is stored twice, costing about 148 MB of extra quota. Negligible against 10 GB.

**Why `save-if: 'false'` on both gate jobs.**

1. Quota. Multi-gigabyte per-branch and per-PR entries would evict the `main` entry the fix depends on. Restricting saves to the one branch every other branch can read is a precondition, not an optimisation.
2. It deletes the 99 s and 40 s `Post Rust cache` steps. `dev-rust` verified in `src/save.ts` that the `save-if` check returns **before** `cleanTargetDir`, `cleanRegistry` and the upload, so those steps collapse to near zero rather than merely shrinking.
3. It is the pattern the action documents for this shape.

A literal `'false'` is used rather than an expression because these jobs never run on `main`.

**The accepted cost, stated accurately.** An earlier draft called the current caches "pure waste" and the new behaviour "strictly better than today". Both were wrong, and `dev-rust-grinch` demonstrated it with live evidence: entry `6065975894` was created `2026-07-26T19:18:15Z` and last accessed `2026-08-02T18:56:46Z`, seven days later. The wrong path means the entries hold no compilation artefacts, but the `~/.cargo` portion is real and is being restored, and the baseline log shows 483 `Downloaded` lines that such a restore would eliminate. So today a branch that changes `Cargo.lock` persists its new registry data for its next push; under this plan it restores `main`'s registry, downloads its branch-only dependencies, and never persists them, repeating that download every push. Section 10.5 carries this as an explicit accepted trade-off with the reasoning for why it is nonetheless expected to win, and Section 9.2 adds a measurement for it rather than assuming the answer.

### 4.2 Decision 2: a cache-warming workflow on push to `main`, with a recovery trigger

**Chosen: a new `.github/workflows/cache-warm.yml`, triggered by `push` to `main`, a daily `schedule`, and `workflow_dispatch`.**

Rejected alternatives:

- **Remove `main` from `branches-ignore` for a subset of gate jobs.** Reintroduces the gate on `main`, which the brief forbids. It also runs `cargo test` on `main` where no one is waiting, importing #1218's flakiness into a red mark on the default branch that nobody would act on.
- **A schedule *instead of* push.** Decouples the cache from the content of `main`: a cache built at 03:00 does not reflect a `Cargo.lock` merged at 09:00, so the first PR after a dependency change, the case where the benefit is largest, gets nothing.

**Why a schedule is nonetheless present as a *recovery* trigger.** `dev-rust-grinch` showed that a push-only writer cannot self-heal, and the finding is correct on two counts. GitHub evicts caches not accessed for seven days, and `dtolnay/rust-toolchain@stable` advances roughly every six weeks, rotating the key with no `main` push to regenerate it. In both cases every feature branch misses and, with `save-if: 'false'`, cannot repair it; the cache is only restored after some unlucky PR merges. That directly defeats #1216's own "second unchanged run hits" criterion. Adding a schedule *alongside* push repairs both without reintroducing the objection to schedule-only, which was about freshness, not recovery.

**The mechanism that makes all three triggers cheap.** Each warm job gates its compilation steps on `steps.cache.outputs.cache-hit != 'true'`. `action.yml` documents `cache-hit` as "A boolean value that indicates an exact match was found", and an exact match by definition means the Cargo manifests, lock file, toolchain and `ImageOS` are all unchanged, so the dependency artefacts in the cache are still exactly right. The image **point release** is deliberately not among those inputs; Section 4.1 explains why and Section 10.6 carries the residual risk that follows. Consequences:

- Scheduled runs on an unchanged repository restore, skip all compilation, and exit in a few minutes. The restore refreshes the entry's access time, defeating seven-day eviction.
- When rustc or `Cargo.lock` changes, there is no exact match, so the run compiles and saves. Recovery is automatic.
- Push runs after a merge that did not touch dependencies also skip compilation. **The warm is expensive only when dependencies or the toolchain actually change**, which answers the cost objection to Decision 2 far more strongly than the original draft could.
- `src/save.ts` skips saving on an exact hit (`isCacheUpToDate()` then `Cache up-to-date.` then `return`), so no redundant upload occurs.

The schedule restores rather than using `lookup-only`, deliberately: GitHub documents the seven-day rule in terms of access, and it is not documented whether a lookup-only probe counts as an access. A real restore is guaranteed to count, costs nothing that blocks anyone, and continuously proves the entry is restorable rather than merely present.

The complete new file:

```yaml
name: Cache warm

on:
  push:
    branches:
      - main
  schedule:
    - cron: '0 6 * * *'
  workflow_dispatch:

permissions:
  contents: read
  actions: read

concurrency:
  group: cache-warm-main
  cancel-in-progress: false

jobs:
  warm-debug:
    name: warm-debug
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - name: Setup Node.js
        uses: actions/setup-node@v5
        with:
          node-version: 22
          cache: 'npm'

      - name: Pin npm version
        run: npm install -g npm@11.6.2

      - name: Install frontend dependencies
        run: npm ci

      - name: Build frontend assets for Tauri config validation
        run: npm run build

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Record toolchain identity
        run: rustc -vV

      - name: Rust cache
        id: cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-debug'
          env-vars: 'ImageOS'

      - name: cargo check
        if: steps.cache.outputs.cache-hit != 'true'
        working-directory: src-tauri
        run: cargo check --all-targets

      - name: cargo clippy
        if: steps.cache.outputs.cache-hit != 'true'
        working-directory: src-tauri
        run: cargo clippy --all-targets -- -D warnings

      - name: cargo test (compile only)
        if: steps.cache.outputs.cache-hit != 'true'
        working-directory: src-tauri
        run: cargo test --lib --bins --tests --no-run

  warm-release:
    name: warm-release
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - name: Setup Node.js
        uses: actions/setup-node@v5
        with:
          node-version: 22
          cache: 'npm'

      - name: Pin npm version
        run: npm install -g npm@11.6.2

      - name: Install frontend dependencies
        run: npm ci

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - name: Record toolchain identity
        run: rustc -vV

      - name: Rust cache
        id: cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-release'
          env-vars: 'ImageOS'

      - name: Build Windows release binary
        if: steps.cache.outputs.cache-hit != 'true'
        run: npm run build:prod:no-bundle

  verify-debug-cache:
    name: verify-debug-cache
    needs: warm-debug
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Probe debug cache
        id: probe
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-debug'
          env-vars: 'ImageOS'
          save-if: 'false'
          lookup-only: 'true'

      - name: Fail if the debug cache is not present
        if: steps.probe.outputs.cache-hit != 'true'
        run: |
          Write-Error "No exact gate-debug cache entry exists after the warm job. The warm job's save step failed silently or the key diverged. PR gates will run cold until this is fixed."
          exit 1

  verify-release-cache:
    name: verify-release-cache
    needs: warm-release
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - name: Probe release cache
        id: probe
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-release'
          env-vars: 'ImageOS'
          save-if: 'false'
          lookup-only: 'true'

      - name: Fail if the release cache is not present
        if: steps.probe.outputs.cache-hit != 'true'
        run: |
          Write-Error "No exact gate-release cache entry exists after the warm job. The warm job's save step failed silently or the key diverged. PR gates will run cold until this is fixed."
          exit 1
```

Each decision inside that file:

- **Two warm jobs mirroring the two gate jobs**, because the two gate jobs populate disjoint profile subtrees and match the two `shared-key` values.
- **Every step before the cargo commands is copied verbatim from the corresponding gate job.** The toolchain identity is part of the key; divergence produces a silently useless cache.
- **`warm-debug` keeps `npm run build`, and the reason is stronger than "the gate does it".** `src-tauri/build.rs:38-49` panics when `../dist/index.html` is missing for a non-`dev` profile; under `cargo check` the profile resolves to `dev`, so instead of panicking it takes `omit_missing_dist_resource_for_dev_check()` (`:57-69`), which sets `TAURI_CONFIG` to `{"bundle":{"resources":[]}}` and clears the `has_embedded_dist` cfg. That produces **different artefacts from the gate's**. Keeping the step is not cosmetic, it prevents a real compilation divergence.
- **`cargo test ... --no-run` is mandatory, with empirical proof.** `dev-rust` measured its own `target/debug/deps` after the full gate sequence: 1 011 `.rmeta` files against 623 `.rlib`. They are different artefacts coexisting in one directory. A cache warmed by `check` alone would hold dependency `.rmeta` but no linkable `.rlib`, and the gate's `cargo test` would recompile every dependency to produce them. **Decision 2 collapses without `--no-run`.**
- **`cargo check` is not redundant either**, and the original justification was wrong. Under `cargo check`, proc-macros and build scripts are genuinely compiled because they must be executed, so that step contributes proc-macro `.dll`s and build-script executables; `--no-run` contributes the `.rlib`s. All three steps contribute distinct artefacts.
- **`clippy` stays, but for the operative reason.** The original text said clippy artefacts carry their own metadata hash. True, but those artefacts belong to workspace crates and are pruned before saving. The real benefit is ensuring the dependencies clippy needs are present in the cache. `dev-rust` verified the mechanism: clippy uses `RUSTC_WORKSPACE_WRAPPER`, which by design only affects the hash of workspace units, so dependencies share fingerprints with what `cargo check` left.
- **No `save-if`** on the warm jobs, so the default `true` applies. They are the only writers.
- **`concurrency` with `cancel-in-progress: false`**, so an in-flight run finishes and writes its cache; GitHub keeps at most one further run pending per group.
- **No `paths:` filter**, because the key also incorporates the toolchain, which can rotate with no repository change to re-trigger a filtered warm.
- **`permissions: contents: read, actions: read`.** The probe jobs read the cache API.
- **`rustc -vV` recorded** in both warm jobs, so key rotations are attributable after the fact.

**The verify jobs exist because a failed save is otherwise invisible.** `src/save.ts:80-86` catches `saveCache` errors and `reportError` emits `core.error` without ever calling `setFailed`, so the process exits zero. A quota rejection, an over-large entry or a same-key race therefore yields a **green** warm job with no new cache, and the gates treat a miss as non-fatal, so nothing anywhere turns red. The earlier claim in Section 6 that a warm failure is a visible signal was true only for compilation failures and false for the case that matters. `lookup-only: 'true'` makes the probe cheap: it checks existence without downloading the entry. These two jobs are also the owner-facing health signal for the workflow as a whole.

**These probes only mean anything once `ImageVersion` is out of the key, which is why the amendment in Section 4.1 is a precondition for them rather than an unrelated fix.** A probe validates another job's key only if it can be relied on to compute the same key. While a per-job-varying input is present, no job can validate any other job's key at all, so the guarantee is unobtainable rather than merely weak. The first cycle demonstrated exactly that: `verify-release-cache` passed because it happened to draw the same image generation as `warm-release`, while `verify-debug-cache` drew a different one from `warm-debug` and failed. Identical logic, opposite outcomes, decided by the draw. With `env-vars: 'ImageOS'` the probe and its writer share every key input, and the guarantee holds as originally advertised.

### 4.3 Decision 3: `windows-release-cli-smoke` builds with `--no-bundle`

`package.json` gains one line after `:9`:

```json
"build:prod:no-bundle": "cross-env BUILD_PROFILE=prod tauri build --no-bundle --config src-tauri/tauri.prod.conf.json && node scripts/copy-testable-binary.mjs",
```

and `pr-regression-gates.yml:117-118` becomes `run: npm run build:prod:no-bundle`.

`build:prod` is **not** modified: it is what a developer runs locally, what `release.yml` conceptually mirrors, and what Section 4.4's bundle validation invokes.

**Why a new script rather than inlining.** Every build step in this repository's CI invokes `npm run <script>`. Inlining would put a drifting second copy of the `cross-env` and `--config` flags in YAML.

**Why not `cargo build --release` plus the copy script.** Three concrete failures: `cargo build` does not run `beforeBuildCommand`, so `dist/` would be missing or stale in the embedded assets and the smoke would not catch it because it exercises the CLI surface, not the webview; it discards the config merge and the tauri version-mismatch check that run today; and the output filename would come from Cargo's bin target rather than `mainBinaryName`, so a future divergence breaks `copy-testable-binary.mjs:8` on a path mismatch. `dev-rust` observing `Running beforeBuildCommand 'npm run build'` under `--no-bundle` in a real run settles the first point empirically.

**`copy-testable-binary.mjs` still resolves.** It reads `<repoRoot>/target/release/agentscommander.exe` (`:6-8`). The baseline log emits `Built application at: ...` at `03:00:09.58`, **before** the first bundling marker at `03:00:09.66`, so the binary is complete at its final path prior to any bundler running. `dev-rust` then confirmed it end to end at 0.2 s exit 0.

### 4.4 Decision 3b: bundle validation moves to a narrowly triggered workflow, it is not dropped

**What Decision 3 removes.** Execution of the WiX/MSI and NSIS bundlers, and with them the only CI proof that `bundle.windows.nsis.installerHooks` (`tauri.conf.json:31-35`) resolves, that `bundle.icon[]` (`:24-30`) is consumable by the installer generators, and that `bundle.resources` (`:21-23`) maps correctly.

**The tag path is not equivalent cover, and saying so would have been wrong.** `release.yml:17` passes `--bundles nsis`, so **WiX/MSI would have zero CI execution anywhere.** `releaseDraft: true` protects users from publication but does not protect a release operator from discovering a broken hook after merging and tagging, and it does not detect MSI breakage even then.

**Chosen: a new `.github/workflows/bundle-validation.yml`, triggered on `pull_request` restricted to the paths that can break bundling.** This preserves both bundlers on the causal PR at near-zero steady-state cost, which is strictly better than retaining 46.9 s on every PR and strictly better than accepting the loss.

```yaml
name: Bundle validation

on:
  pull_request:
    paths:
      - 'src-tauri/tauri.conf.json'
      - 'src-tauri/tauri.prod.conf.json'
      - 'src-tauri/nsis/**'
      - 'src-tauri/icons/**'
      - 'package.json'
      - 'package-lock.json'
      - 'Cargo.lock'
      - '.github/workflows/bundle-validation.yml'

permissions:
  contents: read

concurrency:
  group: bundle-validation-${{ github.ref }}
  cancel-in-progress: true

jobs:
  bundle-validation:
    name: bundle-validation
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v5

      - name: Setup Node.js
        uses: actions/setup-node@v5
        with:
          node-version: 22
          cache: 'npm'

      - name: Pin npm version
        run: npm install -g npm@11.6.2

      - name: Install frontend dependencies
        run: npm ci

      - name: Install Rust stable
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: '. -> target'
          shared-key: 'gate-release'
          env-vars: 'ImageOS'
          save-if: 'false'

      - name: Build installers (MSI and NSIS)
        run: npm run build:prod

      - name: Assert both installers exist
        shell: pwsh
        run: |
          $msi = Get-ChildItem -Path 'target/release/bundle/msi' -Filter '*.msi' -ErrorAction SilentlyContinue
          $nsis = Get-ChildItem -Path 'target/release/bundle/nsis' -Filter '*-setup.exe' -ErrorAction SilentlyContinue
          if (-not $msi) { Write-Error 'No MSI produced'; exit 1 }
          if (-not $nsis) { Write-Error 'No NSIS installer produced'; exit 1 }
          Write-Host "MSI: $($msi.Name)"
          Write-Host "NSIS: $($nsis.Name)"
```

**Why those paths, measured rather than guessed.** Over the 519 commits of available history, the paths were touched: `tauri.conf.json` 2, `tauri.prod.conf.json` 1, `src-tauri/nsis` 1, `src-tauri/icons` 1, `package.json` 4, `package-lock.json` 3, `Cargo.lock` 5. That is roughly **1 % of commits for the bundle surface alone and about 2.5 % including the lock files.** Including the lock files is therefore cheap, and it closes the real vector of a `@tauri-apps/cli` or Tauri crate bump breaking the bundlers, which a config-only trigger would miss.

**It uses the `gate-release` cache with `save-if: 'false'`**, so on the rare runs it does fire it restores the same dependencies as the smoke job rather than compiling from scratch.

**What is still not covered, stated plainly.** A bundler breakage introduced by a runner image change, or by a transitive change that touches none of these paths, is still discovered at tag time. That residue is accepted; it is far smaller than the gap Decision 3 would leave on its own, and materially smaller than what the original draft implied was zero.

## 5. Affected surfaces: exact files and symbols

| # | File | Location | Change |
|---|---|---|---|
| 1 | `.github/workflows/pr-regression-gates.yml` | `:70-73`, `Rust cache` of `rust-regression` | Replace `with:`: `workspaces: '. -> target'`, `shared-key: 'gate-debug'`, `env-vars: 'ImageOS'`, `save-if: 'false'` |
| 2 | `.github/workflows/pr-regression-gates.yml` | `:112-115`, `Rust cache` of `windows-release-cli-smoke` | Same with `shared-key: 'gate-release'` |
| 3 | `.github/workflows/pr-regression-gates.yml` | `:117-118`, `Build Windows release binary` | `run:` becomes `npm run build:prod:no-bundle` |
| 4 | `package.json` | after `:9` | Add `build:prod:no-bundle` |
| 5 | `.github/workflows/cache-warm.yml` | new file | Content in Section 4.2 |
| 6 | `.github/workflows/bundle-validation.yml` | new file | Content in Section 4.4 |
| 7 | `.gitignore` | end of file | Add `artifacts/` |

Nothing else is edited. Specifically not `src-tauri/tauri.conf.json`, `src-tauri/tauri.prod.conf.json`, `scripts/copy-testable-binary.mjs`, `scripts/smoke-cli-release-windows.ps1`, `.github/workflows/release.yml`, `Cargo.toml`, or anything under `src/`, `src-tauri/src/` or `crates/`.

`package-lock.json` must remain unmodified: no dependency changes, and `lockfile-check.yml` must stay green.

**Surface 7 explained.** `scripts/smoke-cli-release-windows.ps1:19` creates `artifacts/cli-release-smoke/`, and `artifacts` is absent from `.gitignore`, so the mandatory local validation in Section 8 leaves `?? artifacts/` in `git status` and invites an accidental commit. It is a pre-existing gap that this plan's own required step walks the implementer into, so it is fixed here.

## 6. Required behaviour, edge cases and behaviour on failure

1. **The gate's Rust and smoke checks are unchanged.** Same commands, same strictness. `cargo clippy ... -D warnings` still gates; `npm run smoke:cli-release-windows` still runs against the same two binaries.
2. **Bundle checking moves rather than disappearing** (Section 4.4). On PRs that touch no bundle-relevant path, no bundler runs anywhere in CI; on PRs that do, both MSI and NSIS run and must produce installers.
3. **M1, before landing.** `main` has no cache, so `Rust cache` still prints `No cache found.` That is expected and is the control the protocol depends on, not a failure.
4. **Exact hit versus restore-key hit.** `cache-hit == 'true'` means an exact key match. A branch that changed `Cargo.lock` gets a restore-key hit instead, recovering unchanged dependencies and recompiling changed ones. Both are correct; only the exact case is the nominal one the measurement is taken on. **With `env-vars: 'ImageOS'` the hit type is determined by the repository and the toolchain alone.** Under the superseded `env-vars: 'ImageOS ImageVersion'` it was also determined by which image generation the job happened to draw (Section 2.8), so a miss carried no diagnostic meaning: it could equally indicate a changed lock file or an unlucky draw. That ambiguity is removed, and Section 9.6 case 2 is corrected accordingly.
5. **Warm compilation fails.** No cache is written; gates continue restoring the previous entry until it expires, then degrade to today's behaviour. Degradation is to the current state, never worse.
6. **Warm save fails silently.** `verify-debug-cache` or `verify-release-cache` turns the workflow red. This is the case Section 4.2 exists to make visible. **The guarantee is stated precisely: a probe validates its writer's entry only because, with `env-vars: 'ImageOS'`, probe and writer are guaranteed to compute the same key.** While `ImageVersion` was in the key that precondition did not hold and the guarantee was not weaker than advertised, it was unobtainable, because every job drew an image generation independently and therefore no job could validate any other job's key at all. A probe pass then meant only that the probe and its writer drew alike. This is why `verify-release-cache` passed and `verify-debug-cache` failed in the same M2 run on identical logic, and why that `verify-release-cache` pass must not be recorded as validation. Section 4.2 and Section 2.8 carry the evidence.
7. **Two merges in quick succession.** `cancel-in-progress: false` lets the in-flight run finish; GitHub keeps only the newest pending run.
8. **An entry is evicted mid-flight.** The gate prints `No cache found.` and behaves as today. The next scheduled or push warm regenerates it.
9. **Seven quiet days, or rustc advancing without a `main` push.** The daily schedule restores (refreshing access) or, on a rotated key, recompiles and saves. This is the recovery path.
10. **`--no-bundle` and local workflows.** `npm run build:prod` keeps producing MSI and NSIS exactly as before.
11. **`BUILD_PROFILE` is preserved** at `prod` in the new script; `warm-release` and `windows-release-cli-smoke` both set it, `warm-debug` and `rust-regression` both omit it, so each pair matches.
12. **Uploaded artefacts are unchanged.** `pr-regression-gates.yml:123-129` uploads `artifacts/cli-release-smoke`, unrelated to bundles.

## 7. Compatibility and security

- **No IPC, no Rust or TypeScript source, no persisted config, no user-visible surface.**
- **Release artefacts are unchanged.** `release.yml` is not edited.
- **Permissions.** `cache-warm.yml` declares `contents: read` and `actions: read`; `bundle-validation.yml` declares `contents: read`. No secrets are used or needed.
- **Third-party action surface is not widened.** Only actions already used by `pr-regression-gates.yml`.
- **Supply chain.** The gate stops downloading `nsis-3.11.zip` and `nsis_tauri_utils.dll` on every run, confirmed absent in `dev-rust`'s real execution. Those downloads remain on the tag path and on the rare bundle-validation run.
- **Cache poisoning.** Writes are restricted to `push`/`schedule`/`dispatch` on `main`, which requires merge or maintainer access. Today every branch and every PR merge ref can write an entry the gate might read. This is a net reduction in who can influence a cached artefact.

## 8. Implementation order

1. **Local validation. Already executed and passed by `dev-rust`** (Section 2.7): `npm run build:prod:no-bundle` equivalent command, then `copy-testable-binary.mjs`, then the smoke, with every Section 9.4 marker confirmed. It is listed first because a cheap check that can invalidate two other artefacts belongs before writing them. The implementer may take `dev-rust`'s result and proceed. If re-run, note that it leaves `artifacts/` behind until surface 7 is applied.
2. Edit `package.json`, adding `build:prod:no-bundle` after `:9`. Confirm `package-lock.json` is unmodified.
3. Add `artifacts/` to `.gitignore`.
4. Edit `pr-regression-gates.yml` surfaces 1, 2 and 3.
5. Add `.github/workflows/cache-warm.yml`.
6. Add `.github/workflows/bundle-validation.yml`.
7. Open the PR. Record measurement **M1** (Section 9.2). Note that this PR touches `package.json`, so `bundle-validation` will fire on it, which is the intended first exercise of that workflow.
8. Merge to `main`.
9. **Immediately after the merge**, purge stale cache entries (Section 9.3). This is deliberately after the merge, not before: open PRs on branches without the fix keep writing caches, so purging first would let them refill the quota before the warm runs.
10. The warm workflow runs for the first time. Record **M2**, including both verify jobs passing.
11. Cut `test/1216-cache-hit-measurement` from the new `main`, touching no `Cargo.lock`, open a PR, record **M3**.
12. Push one more commit to that branch, record **M4**.
13. Record **M5** (durability) and **M6** (recovery) per Section 9.2.
14. Post the before/after table to #1216.

Steps 7 through 13 are the certification path, not follow-up. **The change is not done at step 7.**

## 9. Measurement protocol, tests and objective acceptance criteria

### 9.1 Why the original protocol was replaced

The first draft made gate wall-clock the primary threshold: M3 strictly under 1148 s. `dev-rust-grinch` showed that this can certify success when the cache contributes nothing, and the tech lead reproduced it independently. Two successful pre-fix, cache-miss runs of the same job:

| Run | Job | Duration |
|---|---|---|
| `30762367273` | `rust-regression` | 1000 s |
| `30780031337` | `rust-regression` | 1148 s |

**148 s of natural run-to-run variance, larger than the 99 s of deterministic saving from deleting the post-save step.** A run could therefore pass by runner allocation alone while every dependency recompiled. Worse, `No cache found.` being absent proves only that a tarball was downloaded, not that Cargo reused anything.

The protocol below fixes that by making the primary cache evidence a set of counters that do not move with runner allocation, and by separating three questions that the original conflated: did the deterministic changes land, did the cache actually accelerate compilation, and did total CI time fall.

### 9.2 The sequence of real runs

The cache cannot exist until the fix is on `main`, so certification spans a landing, which the user has explicitly authorised.

| ID | When | What it establishes |
|---|---|---|
| **M0** | captured | Pre-fix baseline, run `30780031337`, Section 2.5 and 2.6 |
| **M1** | PR of `fix/1216-...`, before merge | **The cold control.** Same code as M3, same absent bundling, same suppressed save, but no `main` cache. This is what M3 is measured against for cache value |
| **K1** | any ordinary PR run after the Section 4.1 amendment is on `main` | **The key-stability gate. New, and it blocks everything after it.** Section 9.2.1 |
| **M2** | first `cache-warm` run on `main` **after K1 passes** | A `main`-scoped cache exists and is provably restorable: both warm jobs green **and** both verify jobs green. Entry sizes recorded |
| **M3** | PR from `test/1216-cache-hit-measurement`, cut from new `main`, touching no `Cargo.lock` | **The deciding measurement** |
| **M4** | second push to that branch | The hit reproduces. Satisfies #1216's first acceptance criterion literally |
| **M5** | after at least 3 further merges to `main` and 5 unrelated PRs | **Durability under real churn**, not a purged lab state: quota, entry presence, and a gate run still hitting |
| **M6** | deliberate recovery drill | Delete the `gate-debug` entry via the API, run `workflow_dispatch` on `cache-warm`, confirm it compiles and saves and both verify jobs pass; dispatch again and confirm `cache-hit == 'true'` with all compile steps skipped |

**The measurement branch must be named `test/1216-cache-hit-measurement` or another `<type>/<issue>-<slug>` form referencing an open issue.** `validate-branch-name` is the only required check on `main` (`strict: true`) and `scripts/validate-branch-name.mjs:15` enforces that shape with `--check-issue` verifying the issue is open, so a free-form name such as `measurement/m3` is rejected outright.

M2 is slow on its first run because nothing exists to restore. That is expected and must not be read as a regression; from the second run the warm restores its own entry and skips compilation entirely.

**M5 additionally records the changed-`Cargo.lock` scenario** identified in Section 4.1: if any of the observed PRs changes `Cargo.lock`, capture its `Downloaded` count and compile-step durations against an unchanged-lock PR in the same window. This measures the accepted trade-off in Section 10.5 rather than assuming it.

**The first certification cycle's M1 and M2 are superseded and must not be reused.** They were taken against `env-vars: 'ImageOS ImageVersion'`, so their keys were drawn rather than derived (Section 2.8). **M2 must be re-run after the amendment lands**, and its `verify-release-cache` pass from the first cycle must be recorded as luck rather than validation (Section 6.6). M1's Group A results are unaffected by the amendment, because Group A tests the deterministic changes and touches no cache key; M1's value as the cold control for Group B is what the re-run restores.

### 9.2.1 K1, the key-stability gate

This check is `dev-rust`'s, adopted because it is stronger than anything the plan previously contained and costs nothing to run.

`rust-cache` prints a `Cache Key:` line within seconds on every job, hit or miss, `lookup-only` or not. So the key can be compared across jobs without any cache existing and without waiting for a warm cycle.

**Procedure.** On any ordinary PR run after the amendment is on `main`, read two things from **both** gate jobs, `rust-regression` and `windows-release-cli-smoke`:

1. the `Image Release:` URL from the `Runner Image` log group, which is the only reliable source of the image version (see the warning in Section 2.8 about the two `Version:` lines), and
2. the emitted `Cache Key:` line, from which the environment hash is taken.

**Verdicts, and the middle one is the point of the whole check.**

- **PROVEN STABLE.** The two gate jobs drew **different** image versions and still emitted the **same** environment hash. The key no longer depends on the draw. No cache needs to exist for this proof to hold.
- **INCONCLUSIVE.** The two gate jobs drew the **same** image version. This is **not** a pass. Two jobs that drew alike would have agreed under the defective configuration too, so the run distinguishes nothing. Record it as inconclusive and check the next PR run.
- **FAIL.** Different hashes from any cause. The amendment did not take effect, or another per-job-varying input is present. Do not proceed; diagnose by comparing the full `Cache Key:` lines.

**K1 blocks the next warm cycle and M3.** Do not trigger `cache-warm` for M2 and **do not take M3 until K1 returns PROVEN STABLE.** The reason is Section 4.1's consequence 2: with a nondeterministic key, Group B can report `cache-hit == 'true'` and single-digit units worked purely because the M3 gate happened to draw the same generation as its writer, which is a false pass on precisely the criterion Section 9 was rewritten to protect. Treating INCONCLUSIVE as a pass reintroduces that hole, so the distinction between "proven stable" and "we got lucky" is an explicit acceptance criterion here rather than a note.

### 9.3 Cache hygiene, required immediately after the merge

Purge the stale Rust entries so eviction pressure does not corrupt the measurement. `dev-rust` measured 49 `v0-rust-*` entries at 7.12 GB, so this frees about 8 GB of headroom.

Two defects in the original command are fixed here. It requested `per_page=100` without pagination while the plan itself had measured 101 entries; and its filter `startswith("v0-rust-")` **also matches the new `gate-debug` and `gate-release` entries**, which is harmless on the first pass but destroys the measurement on any re-cycle contemplated by Sections 9.6 and 10.1, presenting as "M3 shows `No cache found.` again" and routing to the wrong diagnosis.

```
gh api --paginate "repos/mblua/AgentsCommander/actions/caches?per_page=100" \
  --jq '.actions_caches[]
        | select(.key | startswith("v0-rust-"))
        | select((.key | contains("gate-debug")) or (.key | contains("gate-release")) | not)
        | .id'
```

Delete each id with `gh api -X DELETE repos/mblua/AgentsCommander/actions/caches/<id>`, then **repeat the listing until it returns no ids**, then recheck `/actions/cache/usage`. **This requires explicit maintainer confirmation before it is run.** Every deleted entry is regenerable by a subsequent run, and nothing outside the `v0-rust-` prefix is touched, so `node-cache-*` entries survive.

Record `active_caches_size_in_bytes` and `active_caches_count` immediately before and after.

**The two entries stranded by the amendment: a closed decision, not an open item.** Changing `env-vars` changes the environment hash, so both entries currently on `main` become unreachable the instant the amendment lands: `gate-debug` at hash `cfee4d59`, 974 814 537 B, and `gate-release` at hash `3d5bdf05`, 655 629 888 B. Section 9.3's purge filter deliberately excludes `gate-*` keys, so it will not remove them and must not be modified to do so. At 1.51 GB combined they sit inside the Section 10.2 steady-state budget. **Decision, taken by the tech lead: leave them to LRU and the seven-day rule.** They are by construction the least recently used once the new generation exists, which is exactly the case Section 10.2 argues LRU handles correctly, and spending a maintainer confirmation on a two-id deletion is not warranted. No action is required of the implementer.

### 9.4 What constitutes proof

**Log-line evidence**, from raw job logs. Note that the jobs run with `CARGO_TERM_COLOR: always`, so **ANSI escapes must be stripped before matching** or the cargo counters silently read as zero:

```
$clean = [regex]::Replace($log, "\x1b\[[0-9;?]*[ -/]*[@-~]", "")
```

- Cache restore occurred: the `Rust cache` step does not contain `No cache found.`
- **Cache hit was exact**: the `Rust cache` step's `cache-hit` output is `true`. Cross-check by comparing the `Cache Key` line between M2 and M3; identical keys mean an exact hit.
- Cargo work counters: `Compiling `, `Checking `, `Fresh ` and `Downloaded ` line counts, per job, computed on the ANSI-stripped log.
- Bundling removed: the build step contains `Built application at:` and none of `Running light to produce`, `Running makensis to produce`, `Info Verifying wix package`, `Info Verifying NSIS package`.
- Save suppressed: `Post Rust cache` completes in under 5 s.
- `rustc -vV` from the warm jobs, recorded for M1 through M6 so key rotations are attributable.
- **The `Cache Key:` line and the `Image Release:` URL, recorded from every job of every measurement**, not only at K1. `Cache Key:` is emitted within seconds on every job regardless of hit, miss or `lookup-only`, so this costs nothing. Recording the image alongside the key is what makes a later "did the key depend on the draw" question answerable from the record instead of requiring a fresh run, which is the gap that let the first cycle reach certification. Take the image version from the `Image Release:` URL only, never from the first `Version:` line of the `Runner Image` group, which is the Image Provisioner version and is identical across generations (Section 2.8).

**API evidence:** `steps[]` durations from `/actions/runs/<id>/jobs`; entries, `ref`, `key` and `size_in_bytes` from `/actions/caches`; `active_caches_size_in_bytes` and `active_caches_count` from `/actions/cache/usage`.

### 9.5 Pass criteria, in three independent groups

**Group A: the deterministic changes landed. Evaluated on M1, before any cache exists. All must hold.**

- A1. No bundling markers in `Build Windows release binary`, and `Built application at:` present.
- A2. `Post Rust cache` under 5 s in both gate jobs.
- A3. The smoke still passes: `passed=4 skipped=0 failed=0`.
- A4. `bundle-validation` runs on this PR (it touches `package.json`) and produces both an `.msi` and a `-setup.exe`.

**Group B: the cache actually accelerated compilation. Evaluated as M3 and M4 against M1. All must hold. These are the criteria immune to runner variance.**

- **B0. K1 returned PROVEN STABLE before M2 was re-run and before M3 was taken** (Section 9.2.1). This is a precondition, not a measurement, and **Group B cannot be evaluated at all without it.** B1 through B6 are all satisfiable by luck when the key depends on which image generation a job drew, so without B0 a green Group B carries no information.
- B1. `cache-hit == 'true'` in both gate jobs. A restore-key hit does not satisfy this.
- B2. **Units worked** (`Compiling` + `Checking`, cargo lines only) in `rust-regression` at M3 is **at most 10 % of M1's**. M0's value is 858, so the expected M3 value is single or low double digits, dominated by the three unavoidable rebuilds of the workspace crate.
- B3. `Downloaded` count in `rust-regression` at M3 is at most 5. M0's value is 483.
- B4. The sum of `cargo check` + `cargo clippy` + `cargo test` durations at M3 is lower than at M1.
- B5. `Build Windows release binary` at M3 is lower than at M1.
- B6. M4 reproduces B1 through B5.

**A hit with no net compile benefit fails cache certification even if total wall-clock fell.** If B1 holds but B2 or B3 fails, the cache is being downloaded and ignored, which is a worse outcome than no cache at all and must be reported as a failure.

**Group C: total CI time fell. This is the user's requirement. Both must hold.**

- C1. M3 gate wall-clock (max job duration) is lower than M0's 1148 s **by more than 148 s**, the largest run-to-run variance observed between two comparable pre-fix runs. A smaller reduction is not evidence.
- C2. If C1's margin is not met on a single sample, take **three** M3-equivalent runs and compare the **median** against M0's 1148 s, and report the spread. A single sample is sufficient only when the margin exceeds the observed variance.

**The deterministic floor**, retained as a sanity check rather than a primary criterion. Three eliminated segments are arithmetically certain from M0 step data: `Post Rust cache` 99 s and 40 s, and bundling 46.9 s. The baseline also already paid 3 s and 2 s in its own no-op restore steps, so the floor is stated net of both the removed save and the replaced restore:

- `rust-regression`: `(1148 - M3) >= (102 - R)` where `R` is M3's `Rust cache` step duration.
- `windows-release-cli-smoke`: `(1033 - M3) >= (88.9 - R)`.

**Group D: durability. Evaluated at M5 and M6. All must hold.**

- D1. At M5, entries for `gate-debug` and `gate-release` with `ref: refs/heads/main` still exist, and a gate run in that window still satisfies B1 and B2.
- D2. At M5, `active_caches_size_in_bytes` is under the steady-state budget of Section 10.2.
- D3. At M6, the first dispatch compiles and saves and both verify jobs pass; the second dispatch reports `cache-hit == 'true'` and skips every compile step.

**No numeric savings figure is asserted in advance for the compilation the cache recovers.** Section 9.7 explains the ceiling that makes any such prediction unsound.

### 9.6 On failure

If Group B fails, the change is not certified and must not be treated as done. Four distinguishable causes, each with a defined next action:

1. `No cache found.` in M3 → the key does not match. Compare the `Cache Key` line from M2's warm log against M3's. Likely a `shared-key`, `env-vars`, `workspaces` or toolchain divergence between the files. **Do not re-run the Section 9.3 purge as a first move**; the corrected filter now protects the `gate-*` entries, but the diagnosis is a key comparison, not a purge.
2. Restore occurred but `cache-hit` is `false` → a restore-key hit. Confirm the measurement branch did not touch `Cargo.lock`, and confirm the toolchain did not rotate between M2 and M3 by comparing the recorded `rustc -vV`. **Do not compare `ImageVersion`: it is deliberately not a key input** (Section 4.1), so an image difference between M2 and M3 is expected, is not a cause, and diagnosing from it sends the investigation to the wrong place. If the toolchain matches and the lock file is untouched, re-run K1 (Section 9.2.1): a `false` here with both of those held constant is evidence that some other per-job-varying input has entered the key.
3. Exact hit but B2 fails, so units worked barely moved → the restored artefacts are not being reused. Check whether the workspace-crate rebuild is dragging dependents with it, and whether `build.rs`'s `dist/` rerun paths (Section 9.7) are invalidating more than the workspace crate.
4. Group B passes but Group C fails → the cache works and total time still did not drop by more than variance. Report that as the measured finding on #1216. It is a legitimate outcome and would mean the recoverable share is smaller than the fixed overhead of the gate, which must be reported rather than buried.

If a verify job fails at M2, treat a **missing entry as a first-class failure**, not as `S = 0` feeding Section 10.1's keep-the-design branch. A missing entry means the save was rejected or raced, and Section 10.1's rules assume the entry exists.

### 9.7 The ceiling: what a cache can and cannot recover here

This belongs beside the refusal to promise a number, so that a modest M3 is not misread as a design failure.

`src/cleanup.ts` prunes with `packages = getPackagesOutsideWorkspaceRoot()`, so **artefacts of the workspace crate are deleted before saving and never enter the cache.** The gate will always recompile and relink `agentscommander-new` and its test binaries. In `dev-rust`'s tree the split inside `target/debug/deps` was 5.10 GB of dependency artefacts against 4.97 GB of workspace artefacts, so roughly half the tree is structurally uncacheable.

Compounding it, `src-tauri/build.rs:35-36` and `:71-83` emit `cargo:rerun-if-changed` recursively over every file in `dist/`, and vite emits content-hashed filenames (`index-BdsNE8Zt.css` and similar). Every gate run rebuilds the frontend from scratch, so those paths change every run, the build script re-executes, and the workspace crate recompiles unconditionally.

**What is recoverable is dependency compile time, not total compile time.** That is precisely why B2 counts units worked rather than asserting a target duration.

### 9.8 Automated tests

No unit or integration test is added: the change touches CI definitions, one npm script and one ignore line, and the repository has no harness that executes workflow YAML. The gate is the test. `npm run smoke:cli-release-windows` passing at M1 and M3 is the functional proof that `--no-bundle` did not damage the artefact under test, `bundle-validation` is the functional proof that the bundlers still work, and `test-debt`, `frontend-regression`, `lockfile-check` and `version-sync-check` must stay green throughout.

## 10. Risks, each with a closed decision rule

### 10.1 The corrected cache may not fit the quota

Measured: 9.75 to 10.14 GB against a 10 GB limit across two readings; `v0-rust-*` is 7.12 GB of it and is purged at step 9; the candidate tree is of the order of 5 GB uncompressed for debug and materially less for release (Section 2.3).

Mitigated by design: `save-if: 'false'` removes per-branch and per-PR writes; Section 9.3 purges the stale entries.

**Decision rule, keyed to M2.** Let `S` be the sum of `size_in_bytes` of the `gate-debug` and `gate-release` entries with `ref: refs/heads/main`.

- **Either entry absent** → **failure**, not `S = 0`. Route to Section 9.6's verify-job branch. The warm's save was rejected or raced, and no size-based rule applies.
- `S <= 6 GB` → keep the design. No action. Given Section 2.3's decomposition this is the expected outcome.
- `S > 6 GB` → **prioritise `gate-debug` and narrow the release side.** Set `cache-targets: 'false'` on every `gate-release` step (both gate, warm and bundle-validation), which keeps the cargo registry, calibrated at 148 MB compressed, and drops the release target tree. `rust-regression` at 1148 s sets the gate's wall-clock and `windows-release-cli-smoke` at 1033 s does not, so the critical-path job keeps the full benefit. Re-run M3 and M4; Group B and C still govern.

6 GB leaves roughly 4 GB for the `node-cache-*` entries (measured at 1.96 GB across 50) plus headroom for one superseded `main` generation during a key rotation.

**The `gate-shared` collapse rule from the previous draft is deleted, because it cannot create the cache it described.** GitHub caches are immutable, and two isolated jobs with the same key and separate filesystems both miss on restore, after which whichever post-step saves first stores only its own profile and the other loses the same-key race. If one job starts late enough to see the first entry it gets an exact hit, and `src/save.ts` then reports `Cache up-to-date.` and deliberately does not save the second profile. The result would contain debug **or** release, never their union, leaving one gate permanently cold. Even a correctly constructed combined archive would occupy roughly `debug + release` bytes, so it would not have solved a quota measured in bytes anyway. The replacement rule above degrades deterministically instead.

### 10.2 Quota survival is a steady-state property, not a one-time purge

`save-if: 'false'` is necessary but not sufficient. `actions/setup-node` keeps writing ref-scoped entries (1.96 GB across 50 measured); `release.yml` still writes Rust caches on tags; and open PRs on branches without the fix keep saving until they rebase. A lock or toolchain rotation creates a new `main` generation while the old one still exists.

**Steady-state byte budget, with deterministic priority.** Ceiling 10 GB. Allocation: `gate-debug` and `gate-release` current generation first, `node-cache-*` second, everything else last. If `active_caches_size_in_bytes` exceeds 9 GB at M5, re-run the Section 9.3 purge (whose corrected filter now protects the `gate-*` entries) and re-check.

**Superseded generations are deliberately not deleted automatically.** A workflow step that deletes caches needs `actions: write` and is a script that can destroy the entry the gate depends on. LRU plus the seven-day rule already converges: once a new generation exists, the old one stops being accessed and expires, and the entry most at risk from LRU is by definition the least recently used, which is never the active one because every PR touches it. The verify jobs detect the pathological case if it ever occurs. This is a deliberate rejection of the automatic-deletion proposal on cost-of-failure grounds, not an oversight.

### 10.3 Restore may be slower than the compilation it saves

Bounded and measured: B4 and B5 compare compile-step durations against the cold control, and the deterministic floor is stated net of the restore duration. Section 9.6 case 4 routes the outcome where restore cost dominates.

### 10.4 The warm consumes runner time on `main`

Substantially smaller than the previous draft implied, because of the `cache-hit` gating in Section 4.2: the warm compiles only when dependencies or the toolchain actually change, and otherwise restores and exits. It blocks nobody, is not a required check, and costs no billed minutes on a public repository. The tech lead confirmed this reading of the constraint at consensus round 1.

### 10.5 A branch that changes `Cargo.lock` may be worse off than today. Accepted trade-off

Today such a branch saves its new registry data and reuses it on its next push. Under `save-if: 'false'` it restores `main`'s registry, downloads its branch-only dependencies, and repeats that download on every push. If the repeated download exceeds the removed 99 s post-save, that branch is slower than today.

**Accepted, with reasoning, and measured rather than assumed.** The compile-side gain from restoring `main`'s dependency artefacts is expected to dominate a re-download of a handful of crates, since M0 spent 483 downloads and 858 units of compilation to reach the same state. But it is not asserted: M5 captures a changed-lock PR against an unchanged-lock PR in the same window (Section 9.2). If that measurement shows a regression for changed-lock branches, the remedy is a separate decision informed by real numbers, not a pre-emptive design change here.

This also corrects two claims in the previous draft that were false: the current caches are not "pure waste", because their `~/.cargo` portion is genuinely restored on later runs of the same branch, and the new behaviour is not "strictly better than today" in every case.

### 10.6 Runner image drift. Open and accepted, with a named lever

**This risk is not closed. An earlier revision of this plan claimed it was, and that claim is withdrawn.**

**What is guarded.** `env-vars: 'ImageOS'` keeps a major image transition, such as `win22` to `win25`, in the key. That is the image change most certain to move the toolchain, and it rotates the cache deliberately.

**What is not guarded, stated plainly.** A **point release inside** `win25-vs2026` can move the MSVC toolset while `ImageOS` and rustc are unchanged. Native artefacts from `-sys` crates built against the previous toolset would then exact-hit and be linked by a new linker.

**Why that is accepted rather than fixed.** The comparison was made explicitly and is recorded so it is not re-opened:

| | Cost of keeping `ImageVersion` | Cost of removing it |
|---|---|---|
| Status | **MEASURED, total and already realised** | **UNVERIFIED and never observed in this repository** |
| Effect | The cache never works reliably, which is the whole of #1216. Plus a forced cold warm on every fortnightly image bump, inside the seven-day eviction window | A toolset move inside a point release could link stale `-sys` artefacts |
| Failure mode | Silent, intermittent, roughly half the time, and can present as success | **Loud.** A link error, not silent corruption of output |
| Lever | None. The key rotates at random on every draw | **A manual `prefix-key` bump**, which rotates the key on purpose |

The decision is lopsided enough that the tech lead ruled it directly, and the architect agrees: **remove `ImageVersion`, record the residual as an accepted trade-off, and do not re-open it.** Section 4.1 records why substituting a narrower MSVC-identity input is not a better third option.

**The lever, so that it is actionable rather than theoretical.** If a link failure against restored native artefacts is ever observed, add or increment a `prefix-key` input on all seven `swatinem/rust-cache@v2` steps at once. It must be all seven, because the steps sharing a `shared-key` must agree on every key input or the cache becomes unreachable, which is the same failure this amendment exists to remove. That rotates both entries deterministically, the Section 4.2 recovery path regenerates them without waiting for a `main` push, and the event is attributable because both warm jobs record `rustc -vV`.

**What would make this worth revisiting**, recorded so a future editor has a threshold rather than an instinct: a first observed occurrence of the link failure, or GitHub publishing a stable per-image toolset identity that does not vary between concurrently served generations of the same image. Neither holds today.

## 11. Adjacent findings, reported and not acted on

### 11.1 `release.yml:74` carries the identical defect

Same `workspaces: src-tauri -> target`, same consequence. **Deliberately not corrected here**: doing so would make the tag path start writing a large cache scoped to `refs/tags/v*`, competing for the same quota that Section 10.1 identifies as the binding constraint, with no benefit to #1216. It should be corrected, with its own `save-if` decision, once M2 establishes what these entries actually weigh. Recommend a separate issue; this plan does not open one.

### 11.2 The gate compiles only one of the two workspace members

Both gate jobs run cargo with `working-directory: src-tauri`, so `crates/session-bridge` is never built. That is #1217. The warm mirrors the restriction deliberately: warming a member the gate does not build would inflate the cache without accelerating anything, and changing what the gate compiles would invalidate the M0 comparison.

### 11.3 The double trigger costs cache quota, though not wall-clock

The tech lead verified it costs no wall-clock, and it stays out of scope. The cache listing shows identical keys stored twice, under `refs/heads/<branch>` and `refs/pull/N/merge`, doubling that branch's footprint. `save-if: 'false'` neutralises this for the Rust caches as a side effect. The `node-cache-*` entries still duplicate.

### 11.4 A caution preserved from `dev-rust`

In M0, `cargo check` takes 264 s and `cargo clippy` only 72 s. The tempting inference is that `cargo check` is redundant. **It is not.** Clippy is cheap only because it reuses the dependency artefacts the check step just built; from a cold target it would pay that compilation itself, and nobody has measured "clippy alone from a cold target". No step is removed from `rust-regression` here, and none should be on the basis of those two numbers.

### 11.5 `cache-workspace-crates` is deliberately unused

An input `cache-workspace-crates` (default `false`) would cache workspace-crate artefacts and appears at first glance to attack the ceiling in Section 9.7. It is correctly omitted: those artefacts invalidate whenever their source changes, which is what every PR does by definition, and Section 9.7's `build.rs` finding means the workspace crate recompiles on every run regardless. Their hit rate would be near zero while inflating the entry. Recorded here so it is not re-litigated.

### 11.6 Two operational notes for the implementer

- `dev-rust`'s validation run overwrote `dist/` in the shared replica and generated a 2.59 GB `target/release`. Anyone re-running Section 8 step 1 in a shared working copy should expect the same.
- The `plans/` directory is gitignored (`.gitignore:11`), yet 9 of the 11 plan files on disk are tracked, so committing this plan requires `git add -f plans/1216-ci-rust-cache-and-smoke-bundling.md`.

## 12. Enrichment: dev-rust (Step 5)

Recorded as its own evidence, unaltered in substance. Verdict: the plan is solid and the three core decisions work as written. Verified `Plan-SHA256` match against the Step 4 artefact.

1. **Decision 3 validated end to end on real Windows.** Times, exit codes and every marker: Section 2.7 above. `[Integrated into Sections 2.7 and 8.]`
2. **`shared-key` replaces the job component**, verified in `src/config.ts` at `e18b497796c12c097a38f9edb9d0641fb99eee32`; `add-job-id-key` noted as a future-editor trap. `[Integrated into Section 4.1.]`
3. **`save-if` returns before clean and upload**, so the post steps collapse entirely. `[Integrated into Section 4.1.]`
4. **`workspaces: '. -> target'` also fixes package classification** for `crates/session-bridge`. `[Integrated into Section 2.1.]`
5. **`--no-run` is mandatory**, with the measured 1 011 `.rmeta` against 623 `.rlib`. `[Integrated into Section 4.2.]`
6. **`cargo check` and `clippy` contribute distinct artefacts**, and the plan's original justification for clippy was not the operative one. `[Integrated into Section 4.2, justification rewritten.]`
7. **The 21.71 GB figure overstates the cacheable tree by ~4x**, with a full decomposition. `[Integrated into Section 2.3; Section 10.1's alarm level reduced accordingly.]`
8. **The workspace crate is pruned and `build.rs` forces its recompilation** via recursive `rerun-if-changed` over `dist/`. `[Integrated as Section 9.7, the ceiling.]`
9. **The Section 9.3 purge filter would delete the new entries.** `[Fixed in Section 9.3.]`
10. **M3/M4 need a branch name referencing an open issue**, because `validate-branch-name` is the only required check on `main`. `[Fixed in Section 9.2.]`
11. **The deterministic floor was off by the baseline's own 3 s restore.** `[Corrected in Section 9.5 to `102 - R` and `88.9 - R`.]`
12. **Exact hit versus restore-key hit was not distinguished.** `[Resolved in Section 9.5 B1 using the documented `cache-hit` output, which is stronger than the proposed `Cache Key` comparison; that comparison is retained as the cross-check.]`
13. **`artifacts/` is not gitignored.** `[Fixed as surface 7.]`
14. **Sequencing: local validation first, purge after the merge.** `[Both adopted in Section 8.]`
15. **`cache-workspace-crates` correctly omitted.** `[Recorded as Section 11.5.]`
16. **Codebase Memory gate now returns `ready`** after the upstream fix.

## 13. Enrichment: dev-rust-grinch (Step 6)

Verdict BLOCK on V1 through V6, advisory on V7 and V8, with U1 and U2 labelled UNVERIFIED by the reviewer. Verified `Plan-SHA256` match. Grinch also confirmed the GitHub scope premise and the `shared-key` behaviour are sound, locating the defects in durability, quota fallback, observability and proof.

- **V1 `gate-shared` cannot create the union it describes.** `[ACCEPTED. Rule deleted, replaced in Section 10.1.]`
- **V2 a push-only writer cannot self-heal.** `[ACCEPTED. Schedule plus `workflow_dispatch` plus `cache-hit` gating, Section 4.2.]`
- **V3 M0-M4 can certify acceleration worth zero.** `[ACCEPTED, and it is the finding that reshaped Section 9. Resolved with cold-control comparison and variance-immune counters, Sections 9.1 and 9.5.]`
- **V4 a failed save leaves a green workflow.** `[ACCEPTED. Two `lookup-only` verify jobs, Section 4.2; Section 6.6 corrected.]`
- **V5 the tag path is not equivalent bundle coverage and the plan contradicted itself.** `[ACCEPTED. Section 4.4 adds a narrowly triggered workflow covering both MSI and NSIS; the false equivalence and "semantics unchanged" claims are removed from Sections 1 and 6.]`
- **V6 quota survival needs a steady-state policy; the purge command is unpaginated.** `[ACCEPTED in part. Budget and priority in Section 10.2, soak as M5, pagination fixed in Section 9.3. Automatic deletion of superseded generations REJECTED with reasoning in Section 10.2.]`
- **V7 current caches are not "pure waste" and changed-lock branches may regress.** `[ACCEPTED. Claims corrected in Section 4.1; trade-off and its measurement in Section 10.5 and Section 9.2.]`
- **V8 no owner-facing health signal.** `[ACCEPTED. The verify jobs of Section 4.2 are that signal.]`
- **U1 over-limit upload behaviour unknown.** `[ACCEPTED as a consequence: a missing entry at M2 is a first-class failure, Sections 9.6 and 10.1.]`
- **U2 runner image may not participate in the key.** `[CONFIRMED by the architect from `action.yml`: the key hashes manifests, lock files, toolchain files and `env-vars`, and image identity is not among them. The round-1 fix `env-vars: 'ImageOS ImageVersion'` was DEFECTIVE and is superseded by `env-vars: 'ImageOS'`; U2 is now recorded as an accepted open risk with a named lever rather than as closed. Sections 2.8, 4.1, 10.6 and 15.]`

## 14. Architect's resolution, Step 7 consensus round 1

**Verdict: READY_FOR_IMPLEMENTATION.** No further enrichment round is required. Every blocking finding is resolved by a closed decision, and each resolution rests on evidence verified at this step rather than on judgement alone.

**What the architect verified independently during this round**, rather than accepting either enricher's report at face value:

1. `action.yml` at `e18b497796c12c097a38f9edb9d0641fb99eee32` documents an output `cache-hit` ("indicates an exact match was found") and an input `lookup-only` ("Check if a cache entry exists without downloading the cache"). Both are load-bearing in the resolutions of V3 and V4, and neither was known when the Step 4 draft was written.
2. `src/save.ts` confirms all three of grinch's mechanical claims: the `save-if` short-circuit, `isCacheUpToDate()` returning before any save (which is what makes V1 correct), and `reportError` without `setFailed` (which is what makes V4 correct).
3. `add-rust-environment-hash-key` hashes manifests, lock files, toolchain files and the values named in `env-vars`. Runner image identity is absent, which **confirms U2**. **The fix inferred from that confirmation at this step was wrong**, because the step verified only that `env-vars` feeds the key and never established how `ImageVersion` behaves in production. Section 15 records the correction and what the verification should have included.
4. The baseline log yields cargo work counters of 654 `Compiling`, 205 `Checking`, 18 `Fresh` and 483 `Downloaded`, after stripping ANSI escapes. These are the variance-immune criteria that answer V3.
5. The bundle surface was touched in roughly 1 % of the 519 available commits, or 2.5 % including lock files, which is what makes Section 4.4's trigger cheap enough to include the lock files.
6. `src-tauri/build.rs` confirms `dev-rust`'s reading precisely: the panic path at `:44-49`, the `TAURI_CONFIG` override at `:57-69`, and the recursive `dist/` rerun emission at `:71-83`.

**Where the enrichers were overruled, with reasoning.**

- **Grinch V6's automatic deletion of superseded cache generations: rejected.** It requires `actions: write` in a workflow and a script whose failure mode is destroying the entry the gate depends on. LRU plus the seven-day rule already converges, the active entry is by construction the most recently used and therefore last to be evicted, and the verify jobs detect the pathological case. The budget and priority rule in Section 10.2 addresses the underlying concern at a fraction of the risk.
- **Grinch V6's framing that two generations at `S = 6 GB` implies about 12 GB: not adopted as an estimate.** It uses this plan's *threshold* as if it were the *expected size*. `dev-rust`'s decomposition puts the candidate tree near 5 GB uncompressed for debug and materially less for release, so the realistic two-generation peak is well under that figure. The concern is still addressed, but the arithmetic is not carried forward.
- **`dev-rust`'s proposal to distinguish hit types by comparing the `Cache Key` line: superseded by something stronger.** The documented `cache-hit` output is authoritative and machine-checkable; the key comparison is kept only as a cross-check.
- **`dev-rust`'s suggestion to leave the `102 - R` floor uncorrected as cosmetic: not taken.** It was corrected, because the floor is no longer the primary criterion and a wrong constant in a demoted check is still a wrong constant.

**What changed most.** Section 9 was rewritten rather than patched. The original protocol asked one question with one threshold; the replacement asks four independent questions (deterministic changes, cache acceleration, total time, durability) and answers the cache question with counters that runner allocation cannot move. That is the direct consequence of V3, and it is the reason this plan can discharge the user's requirement of measured proof rather than merely claim to.

**What is deliberately still open to measurement, and why that is not a `TBD`.** Three quantities cannot exist before the change runs: the size of the corrected entries, the share of compile time that is recoverable, and whether changed-lock branches regress. Each is bound by a deterministic rule keyed to a measured threshold (Sections 10.1, 9.5 Group B, 10.5) rather than left to anyone's judgement at implementation time.

## 15. Architect's amendment, Step 7 consensus round 2

**Verdict: READY_FOR_IMPLEMENTATION.** This section is authoritative wherever it contradicts anything earlier in this plan, Section 14 included.

**Why this round exists.** Round 1 certified `env-vars: 'ImageOS ImageVersion'` on all seven `swatinem/rust-cache@v2` steps. `dev-rust` implemented it faithfully and `dev-rust-grinch` passed the implementation and the merge resolution. **The implementation was correct and the plan was wrong.** Issue #1216 was reopened on that finding. The evidence is Section 2.8, diagnosed by `dev-rust` and independently verified by the tech lead from raw job logs, six jobs for six.

### 15.1 The amendment, in full

**`env-vars: 'ImageOS ImageVersion'` becomes `env-vars: 'ImageOS'`. One token, seven occurrences, three files.** Line numbers are given at `main` = `fd90894136c5096b087ed8b32eed53f76df83112`:

| File | Lines | Steps |
|---|---|---|
| `.github/workflows/pr-regression-gates.yml` | `:75`, `:120` | `rust-regression`, `windows-release-cli-smoke` |
| `.github/workflows/cache-warm.yml` | `:55`, `:104`, `:128`, `:156` | `warm-debug`, `warm-release`, `verify-debug-cache`, `verify-release-cache` |
| `.github/workflows/bundle-validation.yml` | `:51` | `bundle-validation` |

**All seven must change together.** Steps sharing a `shared-key` must agree on every key input or the key diverges and the cache becomes unreachable, which is the same class of failure this amendment removes. A partial application is worse than no application.

**Nothing else changes.** No job, step, trigger, command, permission, concurrency group, script or ignore rule is touched. `package.json`, `.gitignore` and `release.yml` are not touched. This plan's Sections 3 and 5 are otherwise unaltered, and #1217, #1218 and #1219 remain out of scope.

Note that this plan describes the seven steps in **six** literal YAML blocks: Section 4.1 writes the `rust-regression` block in full and specifies `windows-release-cli-smoke` by reference. All six blocks and the Section 5 surface table now carry `env-vars: 'ImageOS'`.

### 15.2 The architect's position on the residual-risk ruling

The tech lead ruled that `ImageVersion` be removed, that the residual MSVC-toolset risk be recorded as an accepted trade-off, and that it not be re-opened. **The architect agrees, without reservation, and the ruling stands as the plan's decision.**

`dev-rust` was right to flag that this judgement belonged to the tech lead and the user rather than to itself, and right that the plan should record it explicitly instead of leaving it to be made silently. It is recorded in Section 10.6 as a comparison rather than as an assertion, so that a future reader can see what was traded for what.

The reasoning the architect verified rather than merely accepted: the cost of keeping `ImageVersion` is measured, total and already realised, while the cost of removing it is unverified, never observed here, and fails loudly with a link error rather than silently. Section 4.1 adds one argument of the architect's own that strengthens the ruling and forecloses the obvious counter-proposal: substituting a narrower MSVC-identity input such as a `VCToolsVersion` probe is either equally broken or unnecessary, because if the concurrently served generations differ in toolset it reproduces the same per-job coin flip, and if they do not, there was no risk between them to close.

**Nothing in the tech lead's message is disputed.** One clarification is recorded rather than raised as a disagreement: the message assigns the guarantee correction to Section 6.4, while this plan's own round-1 cross-reference maps the V4 verify-job guarantee to Section 6.6. Both items are genuinely affected and **both were corrected**, so no interpretation of the instruction is left unserved.

### 15.3 Consequences recorded, including the two that are worse than a plain miss

1. As deployed, the cache half of #1216 is a **coin flip rather than a clean failure**, matching roughly half the time and able to present as success. Section 4.1.
2. **M3 could have passed by luck**, reporting `cache-hit == 'true'` with single-digit units worked over a nondeterministic key. That is a false pass on the exact criterion the rewritten Section 9 exists to protect. M3 has not been run and must not be run before K1 passes. Sections 4.1 and 9.2.1.
3. **The Section 4.2 probe guarantee was unobtainable, not merely weak.** While a per-job-varying input is in the key, no job can validate any other job's key at all. `verify-release-cache`'s M2 pass was luck, not validation, and is recorded as such. Sections 4.2 and 6.6.
4. `bundle-validation.yml` carried identical exposure through its `gate-release` reader. It is not a separate defect and needs no separate fix beyond its one occurrence in the table above.
5. **A second-order cost round 1 never priced:** even with no concurrent rollout, `ImageVersion` in the key forces a full cold warm on every image bump, roughly fortnightly, inside the seven-day eviction window the schedule trigger exists to defeat. Section 4.1.

### 15.4 Effect on the certification sequence

- **K1, a new key-stability gate, is added as Section 9.2.1** and adopted from `dev-rust`. It blocks the next warm cycle and M3, and it admits a third verdict, INCONCLUSIVE, which is explicitly not a pass. Group B gains **B0** making K1 a precondition of the whole group.
- **M2 must be re-run after this amendment lands.** The first cycle's M2 is superseded.
- **M1's Group A results survive**; Group A touches no cache key. M1's role as the cold control for Group B is restored by the re-run.
- **The two stranded cache entries are left to LRU and the seven-day rule**, recorded in Section 9.3 as a decision of the tech lead's, not an open item. The purge filter already excludes `gate-*` keys and must not be changed to catch them.

### 15.5 Sections changed by this amendment

Header and status block; **1** (image is not a repository constant); **2.8** (new, the evidence); **4.1** (the decision, the analytical error, and the third-option analysis); **4.2** (exact-match inputs, the `paths:` filter rationale, and the probe precondition); **4.4** and the Section **5** surface table (the new value); **6.4** and **6.6** (hit-type diagnosis and the guarantee); **9.2** (K1 in the sequence, M1 and M2 superseded); **9.2.1** (new); **9.3** (the stranded entries); **9.4** (record `Cache Key:` and `Image Release:` on every job); **9.5** (B0); **9.6** case 2 (stop diagnosing from `ImageVersion`); **10.6** (rewritten from closed to open and accepted, with the lever); **13** U2 and **14** item 3 (round-1 resolutions corrected); **15** (this section).

### 15.6 The methodological lesson, recorded because it is reusable

Round 1's verification of U2 established that `env-vars` feeds the key. **It never established how the named variables behave in production**, and the plan then reasoned about the *absent* case while the case that occurs is the *varying* case. The general rule this plan now follows: **an input added to a cache key must be shown to be stable across concurrently running jobs, not merely shown to be read.** `dev-rust`'s K1 method does exactly that at near-zero cost, because `Cache Key:` is emitted on every job within seconds whether or not any cache exists, and it is adopted as a standing gate rather than a one-off check.

The corresponding trap, which cost real time this round: the `Runner Image` log group prints two `Version:` lines and a naive first-match extraction returns the Image Provisioner version, which is identical across generations and hides the defect completely. Section 2.8 records it.

### 15.7 Verification conditions for this amendment

Facts were taken from the tech lead's dispatch, which states the evidence is complete and instructs that no further `dev-rust` or `dev-rust-grinch` round be spent re-establishing it. The architect independently verified, in the repository at `main` = `fd90894136c5096b087ed8b32eed53f76df83112`: the seven `env-vars` occurrences and their exact file and line coordinates, that `release.yml:74` still carries the untouched `workspaces: src-tauri -> target` and remains out of scope, and that the round-1 `Plan-SHA256` reproduces from the committed plan blob. The concurrent-rollout observations and the offline reproduction of the hash partition were not re-derived, by instruction, and are attributed in Section 2.8 to their sources with `dev-rust`'s own UNVERIFIED label preserved on the absolute digest values.

**Codebase Memory was not used for this amendment, under explicit authorization from the tech lead.** Its full gate returns `ready` but every subsequent graph query fails with `Repository changed; rerun the full gate`, reproduced back to back in a single process and attributed to a query-layer defect (`cbm.ps1:2022-2042` against `:2317`) rather than a stale index. Direct reads were authorized and the 80-line fallback limit was waived.
