# Implementation Plan: #1216 Rust regression jobs recompile from scratch every run and build installers they discard

Status: READY_FOR_IMPLEMENTATION

Full path. Written by the architect at Step 4, enriched by `dev-rust` (Step 5) and `dev-rust-grinch` (Step 6), certified by the architect at Step 7 consensus round 1, amended at Step 7 consensus round 2 after the round-1 certification was invalidated by a defect found in production, amended at Step 7 consensus round 3 after `dev-rust-grinch` passed the round-2 implementation but failed the round-2 plan on three defects in its acceptance gate, amended at Step 7 consensus round 4 after `dev-rust-grinch` failed the round-3 acceptance gate on the ground that its static proof cannot be sufficient, amended again at Step 7 consensus round 5 after `dev-rust-grinch` passed the round-4 acceptance gate and the tech lead withdrew a round-4 ruling about failure diagnosis that had been wrong, and **amended a final time at Step 7 consensus round 6 after `dev-rust-grinch` failed round 5 on the ground that the shape of a returned cache key proves grammar compatibility and not provenance.** The `Plan-SHA256` of rounds 1 through 5 are all superseded.

**Read Section 19 first if you are implementing this, then Section 18, then Section 17.** Section 19 is the record of the round-6 amendment and is the authoritative reading wherever it contradicts anything earlier, Sections 14 through 18 included; Section 18 is the record of round 5 and remains authoritative over everything before it; Section 17 is the record of round 4 and remains authoritative over everything before that. As in the previous rounds, every section an amendment changed was rewritten in place rather than merely annotated, so the sections and the record agree.

**The round-2 code change is not touched by rounds 3, 4, 5 or 6.** `env-vars: 'ImageOS'` on all seven `swatinem/rust-cache@v2` steps is correct, was verified exhaustively at commit `93e20674a83eebcafb0a569470dc6a3315b6523b`, and passed review. Rounds 3 through 6 change only how the change is proved, accepted and diagnosed: round 3 touched Sections 8, 9.2, 9.2.1, 9.4, 9.5, 9.6, 10.7 and 16; round 4 touched Sections 2.8, 6.6, 8, 9.2, 9.2.1, 9.4, 9.5, 9.6, 10.6, 10.7, 15.4, 16.3, 16.8 and 17; round 5 touched Sections 6.4, 9.6 case 2, 17.8 and 18; round 6 touches Sections 6.4, 9.6 case 2, 18.1, 18.4, 18.5 and 19, and nothing else.

**The one-sentence statement of what round 4 changed.** A cache key is a function of runner-supplied inputs as well as of repository text, so no desk reading of this repository can prove two jobs compute the same key. **K1 requires an observed match of the complete emitted key values within one `shared-key` cohort**, and the static reading is demoted from *the* proof to the *configuration* proof. The observed comparison is available before the merge, at no extra cost, and is valid whatever runner images the compared jobs draw.

**The one-sentence statement of what round 5 changes.** `restoreCache` looks its keys up by **prefix**, so a non-exact restore proves only that the returned entry's key starts with this run's `restoreKey`, not that only the lockfile digest moved. **Section 9.6 case 2 now requires the actual returned cache key as evidence and attributes the difference to K-8 only once the entry is shown to carry the reviewed layout's single lockfile-digest suffix**, diagnosing K-9 first otherwise. **The acceptance gate is untouched**: round 5 changes failure diagnosis and one superseded sentence in Section 6.4, nothing more.

**The one-sentence statement of what round 6 changes.** The shape of a returned cache key proves only that it is compatible with the reviewed grammar, never that the reviewed layout produced it, so **Section 9.6 case 2 now makes the conservative reading its default** (the emitted prefix matched, the final suffix string differs, K-8 and K-9 both remain open) and demotes sole K-8 attribution to an **optional upgrade available only when writer and reader are shown to have resolved the same exact `swatinem/rust-cache` commit**. The acceptance gate is untouched for the second round running.

**Section 18 remains the record of the round-5 resolution**, **Section 17 of round 4**, **Section 16 of round 3**, **Section 15 of round 2** and **Section 14 of round 1**, each authoritative over the sections before it except where a later section overrides it. Sections 12 and 13 are the enrichers' own records; **read them for evidence, not for instructions**, because five of their round-1 recommendations were resolved differently from what they proposed, and one round-1 resolution has since been reversed.

Every implementation decision is closed. There is no `TBD`, no competing alternative and nothing left to the implementer, who is expected to start cold with no knowledge of the discussion that produced this. **Two risks are explicitly open and accepted rather than closed**, each with its trade-off and its lever recorded: the runner-image residual in Section 10.6 and the floating `swatinem/rust-cache@v2` tag in Section 10.7. Those are decisions, not unfinished items.

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

**What the complete keys show beyond the hash, added in round 4.** The table above records the environment-hash component only, which is what round 2 needed. The complete key values were captured in the same raw logs, and one of them, supplied by the tech lead from that evidence, is `v0-rust-gate-debug-Windows_NT-x64-cfee4d59-a04e7ee9`, which is `warm-debug` at M2. It decomposes exactly onto the construction in Section 9.2.1: `v0-rust` is the whole `prefix-key` default, `gate-debug` is the `shared-key`, **`Windows_NT-x64` is the runner OS type and CPU architecture**, `cfee4d59` is the environment digest this table records, and `a04e7ee9` is the lockfile digest. The OS and architecture segments were in front of us in every recorded key and were missing from round 3's component enumeration; Section 9.2.1 now carries them. **Only the one complete key above is recorded here**, because the earlier rounds carried forward the hash component alone; Section 9.4 has required the whole `Cache Key:` line on every job since round 3, so this gap does not recur.

**The hashed environment is runner-supplied, and this same evidence already showed it.** The `Environment considered` block of these six jobs lists `CARGO_HOME`, `CARGO_INCREMENTAL` and `CARGO_TERM_COLOR` among the matched names. **The repository sets none of them.** Two independent corroborations are already in this plan: the baseline log shows `CARGO_INCREMENTAL: 0` in the step environment (Section 2.3), and the cargo counters could only be extracted after stripping ANSI escapes (Section 2.6), which is direct evidence that `CARGO_TERM_COLOR` was effectively `always` at runtime although it is set nowhere in this repository. `src/config.ts:123-130` enumerates **all** of `process.env` and hashes every prefix-matching name-equals-value pair whoever supplied it, so these variables are in the key. This is the fact that makes a purely static sufficiency proof impossible and is the reason round 4 exists.

**Methodological warning, and it matters more than it looks.** The `Runner Image` log group prints **two** `Version:` lines, the Image Provisioner version first. A naive first-match extraction returns `20260707.563`, which is identical on all six jobs and makes the images appear the same, hiding the defect completely. The real image version appears only in the `Included Software` and `Image Release` URLs. Anyone re-checking this must read those URLs. **The `Cache Key:` log line carries the same shape of trap and it is not hypothetical**: `src/config.ts:335-336` prints a bare `Cache Key:` heading and the value on the next line, so a literal extraction of the heading compares a constant. Section 9.4 specifies the extraction.

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
4. **Exact hit versus restore-key hit.** `cache-hit == 'true'` means an exact key match. A branch that changed `Cargo.lock` gets a restore-key hit instead, recovering unchanged dependencies and recompiling changed ones. Both are correct; only the exact case is the nominal one the measurement is taken on. **With `env-vars: 'ImageOS'` the image generation is no longer in the key, so the repository's own contribution to the hit type is its K-8 repository inputs.** **Round 6 corrects "lockfile and manifest content", which named part of that set only**: the hasher also covers `.cargo/config.toml`, `rust-toolchain` and `rust-toolchain.toml` under the `workspaces` roots alongside `Cargo.lock` and the `Cargo.toml` manifest set (`config.ts:163-268`; `dev-rust-grinch` locates the toolchain-file globs at `:164` and `:171-175`). A branch that edits only `rust-toolchain.toml` moves K-8 and therefore moves the key, and a diagnosis that looks at the lockfile and the manifests alone will not find it. **Round 5 corrects the sentence that stood before that**, which read "the hit type is determined by the repository and the toolchain alone." That does not follow and it is the superseded inference: the runner OS and architecture segments (K-4, K-5) and every runner-supplied `process.env` value the action hashes (the runtime half of K-7) can still move the key, and none of them is closable by reading this repository (Sections 9.2.1, 17.1 and 18.2). Under the superseded `env-vars: 'ImageOS ImageVersion'` the hit type was **additionally** determined by which image generation the job happened to draw (Section 2.8), so a miss carried no diagnostic meaning: it could equally indicate a changed lock file or an unlucky draw. **That specific ambiguity is what the removal eliminates**, and it is the only one it eliminates. Section 9.6 case 2 carries the diagnosis, as corrected in round 5.
5. **Warm compilation fails.** No cache is written; gates continue restoring the previous entry until it expires, then degrade to today's behaviour. Degradation is to the current state, never worse.
6. **Warm save fails silently.** `verify-debug-cache` or `verify-release-cache` turns the workflow red. This is the case Section 4.2 exists to make visible. **The guarantee is stated precisely, and round 4 tightens it: a green probe *is* an observed exact-key match between probe and writer inside one cohort, not a consequence of a prior guarantee that they compute the same key.** Removing `ImageVersion` removes the repository's own contribution to divergence and makes the match achievable; it is the probe that establishes it happened. While `ImageVersion` was in the key even that was unobtainable, because every job drew an image generation independently and therefore no job could validate any other job's key at all. A probe pass then meant only that the probe and its writer drew alike. This is why `verify-release-cache` passed and `verify-debug-cache` failed in the same M2 run on identical logic, and why that `verify-release-cache` pass must not be recorded as validation. **A green pair of probes is K1-V3** (Section 9.2.1): the standing, post-land form of the observed within-cohort complete-key comparison. Section 4.2 and Section 2.8 carry the evidence.
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
7. Open the PR. **Run K1 on it and record its verdict** (Section 9.2.1), and record measurement **M1** from the same run (Section 9.2). K1 has two required parts on this PR. **K1-S** is a desk check over the seven `with:` blocks and can be done before any run finishes. **K1-V** needs the emitted key values, so it waits for the jobs to reach their `Rust cache` steps, which happens within the first couple of minutes. Note that this PR touches `.github/workflows/bundle-validation.yml`, which is in that workflow's `paths:` filter, so `bundle-validation` fires on it; that is both the intended exercise of the workflow and the second live `gate-release` member K1-V1 compares against. `bundle-validation` is a **separate workflow file and therefore a separate workflow run** from `pr-regression-gates`, so record each job's `GITHUB_SHA` and require them equal before comparing keys (Section 9.2.1).
8. **Merge to `main` only if K1 is PASS**, which requires K1-S COMPLETE **and** at least one pre-land K1-V leg MATCHED. A FAIL blocks the merge; there is nothing to cancel or unwind, because nothing has been written to `main`. If neither pre-land leg produced a comparison, K1 is NOT YET ESTABLISHED and the merge waits until it does; that state means a job did not reach its cache step, which is diagnosable, not a deadlock.
9. **Immediately after the merge**, purge stale cache entries (Section 9.3). This is deliberately after the merge, not before: open PRs on branches without the fix keep writing caches, so purging first would let them refill the quota before the warm runs.
10. The warm workflow runs, queued automatically by the landing push and possibly also by the daily cron. **That is expected and requires no intervention** (Section 9.2). Record **M2** from the first such run whose checkout contains the amendment and whose four jobs are all green; the two green verify jobs are K1-V3 (Section 9.2.1).
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

The cache cannot exist until the fix is on `main`, so certification spans a landing, which the user has explicitly authorised. **K1 is the one step that does not span it: it runs before the landing, on the PR of the fix branch.** Round 3 moved it there because the post-land placement was unenforceable, and the reasoning is in Section 16.3.

| ID | When | What it establishes |
|---|---|---|
| **M0** | captured | Pre-fix baseline, run `30780031337`, Section 2.5 and 2.6 |
| **K1** | **the PR of `fix/1216-cache-key-imageversion`, before merge**: the static configuration proof needs no run, the required observed key comparison uses the runs that PR produces anyway | **The key-determinism gate. It blocks the merge and everything after it.** Section 9.2.1 |
| **M1** | the same PR run as K1 | **The cold control.** Same code as M3, same absent bundling, same suppressed save, and no reachable `main` cache under the new key. This is what M3 is measured against for cache value |
| **M2** | first `cache-warm` run on `main` whose checkout contains the amendment | A `main`-scoped cache exists and is provably restorable: both warm jobs green **and** both verify jobs green. Entry sizes recorded. **The two verify jobs are themselves the live within-cohort complete-key check**, in both cohorts at once (Section 9.2.1, K1-V3). Post-land, so they corroborate the pre-land verdict rather than supplying it |
| **M3** | PR from `test/1216-cache-hit-measurement`, cut from new `main`, touching no `Cargo.lock` | **The deciding measurement** |
| **M4** | second push to that branch | The hit reproduces. Satisfies #1216's first acceptance criterion literally |
| **M5** | after at least 3 further merges to `main` and 5 unrelated PRs | **Durability under real churn**, not a purged lab state: quota, entry presence, and a gate run still hitting |
| **M6** | deliberate recovery drill | Delete the `gate-debug` entry via the API, run `workflow_dispatch` on `cache-warm`, confirm it compiles and saves and both verify jobs pass; dispatch again and confirm `cache-hit == 'true'` with all compile steps skipped |

**Why K1 before the merge, in one line.** `cache-warm.yml:3-9` fires on `push` to `main` and on a daily `cron`, so the landing push queues a warm in the same act and no post-land check can be made to precede it. A same-repo PR runs the branch's own workflow definitions, so the gate jobs on that PR already emit keys built with `env-vars: 'ImageOS'` while `main` is untouched and no warm can fire. The ordering problem is removed rather than managed.

**Automatic warms after the merge are expected and are not a failure.** Once the amendment lands, the landing push queues a `cache-warm` run and the daily cron may queue another. `concurrency: cache-warm-main` with `cancel-in-progress: false` lets the in-flight one finish and keeps only the newest pending one. **M2 is the first of those runs whose checkout contains the amendment and whose four jobs are all green**; a second warm that follows it is a cheap restore-only run and is not a second M2. Nothing needs to be cancelled, quarantined or discarded, and a maintainer is not asked to win a race. If a warm run fires from a push to `main` that lands *before* the fix, it writes old-generation entries; those are stranded exactly like the two in Section 9.3 and are left to LRU by the same decision.

**The measurement branch must be named `test/1216-cache-hit-measurement` or another `<type>/<issue>-<slug>` form referencing an open issue.** `validate-branch-name` is the only required check on `main` (`strict: true`) and `scripts/validate-branch-name.mjs:15` enforces that shape with `--check-issue` verifying the issue is open, so a free-form name such as `measurement/m3` is rejected outright.

M2 is slow on its first run because nothing exists to restore. That is expected and must not be read as a regression; from the second run the warm restores its own entry and skips compilation entirely.

**M5 additionally records the changed-`Cargo.lock` scenario** identified in Section 4.1: if any of the observed PRs changes `Cargo.lock`, capture its `Downloaded` count and compile-step durations against an unchanged-lock PR in the same window. This measures the accepted trade-off in Section 10.5 rather than assuming it.

**Every cache-key-dependent reading from the first certification cycle's M1 and M2 is superseded and must not be reused.** They were taken against `env-vars: 'ImageOS ImageVersion'`, so their keys were drawn rather than derived (Section 2.8). **M2 must be re-run after the amendment lands**, and its `verify-release-cache` pass from the first cycle must be recorded as luck rather than validation (Section 6.6). **The readings that do not depend on the key survive**, which is Group A at M1 and the recorded step durations and cargo counters of a run that hit nothing; the paragraph below states exactly which.

**Where the replacement M1 comes from, stated so the implementer is not left to choose.** Group A tests the deterministic changes and touches no cache key, so **the first cycle's M1 already satisfies A1 through A4 and that result stands.** What the first cycle's M1 cannot supply is a cold control for Group B under the new key. The PR run of `fix/1216-cache-key-imageversion` supplies it: the only `gate-*` entries on `main` at that moment are the two stranded old-generation entries of Section 9.3, which cannot be an exact match for a key built with `env-vars: 'ImageOS'`, so both gate jobs on that PR run cold. **That run is therefore K1 and M1 at once, at no extra cost**, and the same four Group A criteria are re-observable on it for free because the same jobs execute. Take Group A from it as well and record both readings; if they disagree, the newer one governs and the disagreement is itself a finding.

### 9.2.1 K1, the key-determinism gate

**Round 4 rewrote this section again.** Round 3 had repaired round 2's two defects but introduced a third: it declared the static reading a **sufficient** proof. `dev-rust-grinch` showed it cannot be, and the tech lead corroborated it from evidence already in this plan. **A desk read of this repository cannot establish runner-supplied values**, and `src/config.ts:123-130` hashes every prefix-matching entry of `process.env` regardless of who supplied it, so round 3's condition (v), the absence of a repository `env:` block, proved repository absence and not process-environment equality. Round 3's own six-job evidence lists runtime-supplied `CARGO_HOME`, `CARGO_INCREMENTAL` and `CARGO_TERM_COLOR` (Section 2.8). K1-S could therefore return COMPLETE while runtime inputs differed, which is a false pass on exactly the class of nondeterminism B0 exists to exclude. Sections 17.1 through 17.3 carry the reasoning. The gate below compares **emitted values**, so it needs no reasoning about what the runner supplied.

**K1 has three parts.** **K1-S** is the *configuration* proof: it establishes that the repository's own text cannot divide a cohort. **K1-V** is the *observed* proof: a byte-identical match of the complete emitted key values of two members of one `shared-key` cohort. **Both are required.** **K1-L** is corroboration and can never withhold the verdict.

**Why the observed leg resolves the deadlock rather than reintroducing it.** Round 2's gate could be starved forever because its only pass verdict demanded that two sampled jobs draw *different* runner images, and GitHub may finish its rollout at any moment. **K1-V has the opposite property: same-image draws satisfy it.** It compares outputs, so any two cohort members that reach their cache step produce a comparison, whatever they drew. It is available before the merge, from runs that have to happen anyway, at no extra cost.

#### What the action actually builds, at the reviewed revision

Read directly from `src/config.ts` at `e18b497796c12c097a38f9edb9d0641fb99eee32`. The architect verified in round 4 that this is the revision `refs/tags/v2` resolves to today: `refs/tags/v2` is an **annotated tag object** `42dc69e1aa15d09112580998cf2ef0119e2e91ae` peeling to commit `e18b497796c12c097a38f9edb9d0641fb99eee32`. The complete key is assembled in this order:

```
  prefix-key                                      config.ts:74      default "v0-rust"
+ "-" + shared-key                                config.ts:76-78
  ( or "-" + key, then "-" + GITHUB_JOB )         config.ts:80-88   unreachable when shared-key is set
+ "-" + os.type() + "-" + os.arch()               config.ts:91-94   RUNTIME
+ "-" + digest8( environment hasher )             config.ts:136-138 gated by add-rust-environment-hash-key
+ "-" + digest8( lockfile hasher )                config.ts:163-268 same gate
```

`restoreKey` is the key up to and including the environment digest (`:140`); `cacheKey` is the whole thing (`:270`). The recorded key `v0-rust-gate-debug-Windows_NT-x64-cfee4d59-a04e7ee9` (Section 2.8) decomposes onto that construction exactly.

#### The complete component enumeration

Round 3's list had five entries and was wrong in three ways: it omitted the runtime OS and architecture segments, it split the `prefix-key` default into a fake `v0` plus a fake literal `rust`, and it filed runtime values under the action's layout. The corrected list:

| # | Key component | Source | Established equal within a cohort by |
|---|---|---|---|
| K-1 | `prefix-key`, whose default is the single value `v0-rust` (`action.yml:5-8`, `config.ts:74`) | workflow input | **static.** `prefix-key` appears nowhere in `.github/workflows/`, so all seven take the same default |
| K-2 | `shared-key` (`config.ts:76-78`) | workflow input | **static.** A literal in all seven blocks, per the cohort table below |
| K-3 | the `key` input, `add-job-id-key` and `GITHUB_JOB` (`config.ts:80-88`) | workflow inputs and runner | **static.** Unreachable: `shared-key` is set on all seven, which short-circuits the branch entirely |
| K-4 | runner OS segment, `os.type()` (`config.ts:92`, `:94`) | **runtime** | **observed.** It is a plain-text segment of the compared key values, so a divergence is visible in K1-V without any hashing |
| K-5 | runner architecture segment, `os.arch()` (`config.ts:93`, `:94`) | **runtime** | **observed.** Same |
| K-6 | `add-rust-environment-hash-key`, default `true` (`action.yml:19-22`, `config.ts:136`, `:163`) | workflow input | **static.** Absent from all seven blocks, so both digests are present in all seven |
| K-7 | environment digest content: the **complete installed-toolchain set** from `getRustVersions` (`config.ts:106-115`, `:388-411`), plus every `process.env` name-equals-value pair whose name starts with `CARGO`, `CC`, `CFLAGS`, `CXX`, `CMAKE`, `RUST` or with any prefix in `env-vars` (`config.ts:118-131`) | **runtime**, plus the `env-vars` and `cmd-format` inputs | **static part:** `env-vars: 'ImageOS'` in all seven, no `cmd-format` override, no repository `env:` block. **Runtime part: observed only.** This is the component that cannot be closed by reading the repository |
| K-8 | lockfile and manifest digest content over the `workspaces` roots (`config.ts:163-268`) | repository content and the `workspaces` input | **static** for the input, `workspaces: '. -> target'` in all seven; **per leg** for the content, via the recorded `GITHUB_SHA` or tree |
| K-9 | the layout itself, that is the order and the separators above | the resolved `@v2` revision | condition (ii); Section 10.7 |

**Not key inputs**, and this list is exhaustive over the inputs the seven blocks could plausibly grow: `save-if`, `lookup-only`, `cache-targets`, `cache-bin`, `cache-all-crates`, `cache-workspace-crates`, `cache-directories`, `cache-on-failure` and `cache-provider`. They select behaviour or paths once the key exists, which is why a `lookup-only` probe still has to match its writer's key exactly.

**Two corrections inside K-7 that round 3 got wrong and that matter operationally.**

1. **The toolchain component is the complete installed set, not the default `rustc -vV`.** `getRustVersions` (`config.ts:388-411`) adds the default toolchain's identity and then, for **every** toolchain listed by `rustup toolchain list --quiet`, runs `rustup run <toolchain> rustc -vV` and adds that identity too. Each is hashed as `` `${release} ${host} ${commit-hash}` `` after sorting (`:106-115`). **A toolchain preinstalled on the runner image, or installed by any earlier step, is therefore in the key.** That is a runtime fact about the image, not a repository fact.
2. **Repository `env:` absence is a necessary check, not a sufficient one.** It removes the repository as a *source* of divergence, which is worth keeping as a standing check because a future editor adding `env: RUSTFLAGS: ...` to one of the three files and not the others would divide a cohort silently. It says nothing about the values the runner supplies.

#### The invariant K1 establishes, stated conditionally

> Given (i) the same repository content under the `workspaces` roots, (ii) the resolved `swatinem/rust-cache@v2` implementation at revision `e18b497796c12c097a38f9edb9d0641fb99eee32`, (iii) the same **complete installed-toolchain set** as enumerated by `getRustVersions`, (iv) the same runner OS type and CPU architecture, and (v) the same value for **every** `process.env` entry the action matches and hashes, **every job in a `shared-key` cohort computes the same complete cache key.**
>
> **Conditions (iii), (iv) and (v) are runtime facts and are discharged by K1-V, the observed comparison, never by reading this repository.** The static reading discharges (i) for a given checkout and establishes that the repository's own configuration cannot divide a cohort. Condition (ii) is Section 10.7's accepted risk.

Nothing here asserts that the key never changes. Rotating on the toolchain, on repository content, and on a deliberate `prefix-key` bump is the intended behaviour.

#### K1-S, the static configuration proof. Necessary, and explicitly not sufficient

`dev-rust` read `src/config.ts` at the resolved revision and enumerated the key inputs; the architect re-read the same file in round 4 to produce the corrected enumeration above. K1-S checks the enumeration against the seven `with:` blocks.

**The two cohorts, read from the repository at `93e20674a83eebcafb0a569470dc6a3315b6523b` and re-verified by the architect in round 4 at branch HEAD.** Every field that feeds K-1, K-2, K-6, the static half of K-7, or K-8 is textually identical within each cohort:

| Cohort | Member | `env-vars` at | `workspaces` | `prefix-key` | Toolchain step |
|---|---|---|---|---|---|
| `gate-debug` | `rust-regression` | `pr-regression-gates.yml:75` | `. -> target` | unset | `dtolnay/rust-toolchain@stable`, `components: clippy` |
| `gate-debug` | `warm-debug` | `cache-warm.yml:55` | `. -> target` | unset | same |
| `gate-debug` | `verify-debug-cache` | `cache-warm.yml:128` | `. -> target` | unset | same |
| `gate-release` | `windows-release-cli-smoke` | `pr-regression-gates.yml:120` | `. -> target` | unset | `dtolnay/rust-toolchain@stable`, `targets: x86_64-pc-windows-msvc` |
| `gate-release` | `warm-release` | `cache-warm.yml:104` | `. -> target` | unset | same |
| `gate-release` | `verify-release-cache` | `cache-warm.yml:156` | `. -> target` | unset | same |
| `gate-release` | `bundle-validation` | `bundle-validation.yml:51` | `. -> target` | unset | same |

All seven carry `env-vars: 'ImageOS'` and all seven run on `runs-on: windows-latest`.

**The argument, component by component.**

- **K-1 is identical.** `prefix-key` appears nowhere in `.github/workflows/`, so all seven steps take the same default, which is the single value `v0-rust`. Verified by search, not by inspection of the seven blocks alone.
- **K-2 is a literal** in every one of the seven blocks, per the table above.
- **K-3 is unreachable.** `shared-key` is set on all seven, and `config.ts:76-89` takes the `shared-key` branch and skips the `key` input and the job-id segment entirely. The `add-job-id-key` trap Section 4.1 warns about is therefore inert as long as `shared-key` stays.
- **K-6 is identical.** `add-rust-environment-hash-key` appears nowhere in the three files, so all seven take the default `true` and all seven keys carry both digests. A future editor setting it to `false` on one step would drop two whole segments from that step's key.
- **K-8's input is identical** (`workspaces: '. -> target'` on all seven) and **its content is equal across jobs that check out the same tree.** Within one workflow run that is true by construction; across runs it is what the recorded `GITHUB_SHA` or tree comparison in K1-V establishes. Across commits it moves only with repository content, which is the behaviour the cache key exists to have.
- **K-9 is fixed by whatever `@v2` resolves to at job start**, which was `e18b497796c12c097a38f9edb9d0641fb99eee32` in every observed job log, and which the architect confirmed in round 4 is still what the annotated `v2` tag peels to. It is the one component the repository does not control, and strictly it is a per-job resolution rather than a per-run one, so the claim is stated as condition (ii) rather than asserted. Section 10.7 records it as an accepted risk with its levers.
- **K-4, K-5 and the runtime half of K-7 are NOT established here, and that is the point of round 4.** The static half of K-7 that *is* established: all seven carry `env-vars: 'ImageOS'`, so the matched prefix set is identical; no `cmd-format` override exists, so the toolchain enumeration runs the same way; and the repository contributes no hashed variable at all. That last is verified exhaustively: `.github/workflows/pr-regression-gates.yml`, `.github/workflows/cache-warm.yml` and `.github/workflows/bundle-validation.yml` contain **no `env:` block at any level**, workflow, job or step, and no `CARGO`, `RUSTUP`, `RUSTC`, `RUSTFLAGS`, `CMAKE`, `CFLAGS`, `CXX` or `CC` token anywhere. **What this proves is that the repository cannot divide a cohort. It does not prove the process environments are equal**, because `config.ts:123-130` hashes whatever is in `process.env`, and Section 2.8 records `CARGO_HOME`, `CARGO_INCREMENTAL` and `CARGO_TERM_COLOR` arriving from outside the repository.
- **All seven use `dtolnay/rust-toolchain@stable`.** Within a cohort the `with:` blocks differ only in `components` and `targets`, and neither appears in the release, host or commit triple the hasher consumes. This is a real narrowing of the exposure, but it is not a proof of condition (iii), because the hashed set is every toolchain `rustup toolchain list` reports, including any the image preinstalls. The genuine deliberate exposure is a stable release landing between a write and a read, which rotates the key on purpose and is made attributable by recording `rustc -vV` (Section 9.4).
- **Different pre-cache step sequences are already measured not to perturb the environment digest.** In M2, `warm-release` runs `setup-node`, the npm pin and `npm ci` before its cache step while `verify-release-cache` runs only checkout and the toolchain. They drew the same image `20260728.188` and emitted the same environment digest `3d5bdf05` (Section 2.8). This closes the obvious objection that writer and probe jobs are not comparable. It is evidence about those samples, which is exactly the status K1-V is designed to supply on the commit under test rather than infer.
- **The runner-supplied values agreed across the two concurrently served generations once `ImageVersion` is gone**, on the evidence available: `dev-rust`'s `Environment considered` block was byte-identical on all six jobs of Section 2.8, so the matched *names* agree, and its offline reimplementation reproduced the observed six-job partition exactly with `ImageVersion` as the only differing *value*. **This is evidence about six jobs, not a static implication**, and round 3 treated it as the latter. It is recorded as support for the design decision, never as a discharge of condition (v).

**Therefore K1-S establishes exactly this: at the commit under test, the repository's configuration is identical within each cohort and cannot itself divide one.** It is a desk check over the seven blocks. It terminates on the first reading, needs no CI run, and cannot be blocked by what GitHub does with its image fleet. **It is necessary for B0 and it is not sufficient.**

**The honest limit of K1-S, stated as the reason K1-V exists.** K1-S cannot see K-4, K-5 or the runtime half of K-7. A desk read establishes what the repository supplies; the runner supplies the rest, and the action hashes both without distinction. Any verdict that rested on K1-S alone would be a claim about configuration dressed up as a claim about keys.

#### K1-V, the observed within-cohort complete-key match. Required

`rust-cache` prints the `Cache Key:` block within seconds on every job, hit or miss, `lookup-only` or not, so complete keys can be compared without any cache existing and without any job compiling anything. **Compare complete key values, never the environment-digest component alone**, and compare only **within** a `shared-key` cohort, because two jobs in different cohorts are supposed to differ.

**At least one pre-land leg must reach MATCHED. K1-V1 is available by construction, so the requirement terminates.**

**K1-V1, `gate-release`, pre-land, on the PR of `fix/1216-cache-key-imageversion`.**

- The two live members are `windows-release-cli-smoke` (`pr-regression-gates.yml:119-120`) and `bundle-validation` (`bundle-validation.yml:50-51`). `bundle-validation` fires because its `paths:` filter lists `.github/workflows/bundle-validation.yml` (`bundle-validation.yml:13`) and the fix changes that file. Both carry `save-if: 'false'`, so this writes nothing and costs nothing beyond runs that have to happen anyway.
- **They are two jobs of two separate workflow runs, not two jobs of one run.** Round 3 asserted the latter and it is wrong: `pr-regression-gates.yml` and `bundle-validation.yml` are separate workflow files, so the same `pull_request` event starts a separate run of each. The comparison is still valid, but K-8 is no longer identical *by construction* and must be established: **record each job's `GITHUB_SHA` and require the two to be equal** before comparing keys. Both are `pull_request` runs of the same event and both check out `refs/pull/N/merge`, so they agree unless the base moved between them or one was re-run later.
- **Take `windows-release-cli-smoke` from the `pull_request`-event run**, not from the `push`-event run that the double trigger at `pr-regression-gates.yml:3-12` also produces. `bundle-validation.yml` triggers on `pull_request` only, so the two `pull_request` runs are the pair that share a checkout. If only the `push` run is available, apply K1-V2's tree test instead of the `GITHUB_SHA` test.
- **Requirement: equal recorded `GITHUB_SHA`, and byte-identical extracted complete key values.** Equal is MATCHED. Different key values with equal `GITHUB_SHA` is MISMATCHED and K1 FAILs. Different `GITHUB_SHA` values make the leg NOT APPLICABLE; re-run the older workflow on the current head and compare again rather than recording a mismatch.

**K1-V2, `gate-debug`, pre-land.** The double trigger at `pr-regression-gates.yml:3-12` produces a `push`-event run and a `pull_request`-event run of `rust-regression` for the same head commit.

- Their `GITHUB_SHA` values differ **by construction**, head commit against merge commit, so `GITHUB_SHA` equality is the wrong test here. **Equality of the checked-out tree is the right one**, since K-8 is a function of content: `gh api repos/mblua/AgentsCommander/commits/<sha> --jq .commit.tree.sha` for both recorded SHAs, and the two tree SHAs must be equal. That holds exactly when the PR base is an ancestor of the head.
- Equal trees and byte-identical key values is MATCHED. Equal trees and differing key values is MISMATCHED. Differing trees makes the leg NOT APPLICABLE, not CONTRADICTED: rebase, or rely on K1-V1.

**K1-V3, at M2, post-land, automatic and free.** `verify-debug-cache` and `verify-release-cache` are `lookup-only` probes whose pass condition is an **exact** key match against their own cohort's writer entry, saved minutes earlier in the same run and therefore at the same `GITHUB_SHA`. **A green probe is a within-cohort complete-key equality result**, covering every component including K-4, K-5 and K-7. Both green at M2 confirms the invariant live in both cohorts at once, and it needs no procedure of its own because the workflow already fails red when it does not hold. **It is post-land, so it cannot satisfy the pre-land requirement**; it is the standing regression form of the same check and it re-runs on every warm cycle.

**Extraction, specified exactly, because the naive form compares a constant.** `config.ts:335-336` emits **two** lines: a bare `Cache Key:` heading, then the key value on the next line indented by four spaces. `:333-334` emits `Restore Key:` in the same shape immediately before it. **A literal match on `Cache Key:` therefore captures a heading that is byte-identical on every job regardless of the key underneath**, so divergent keys would compare equal and K1-V would falsely read MATCHED. This is the same shape as the two-`Version:`-lines trap in Section 2.8. Take the value from the **following** line:

```powershell
$clean = [regex]::Replace($log, "\x1b\[[0-9;?]*[ -/]*[@-~]", "")
$lines = ($clean -split "`r?`n") | ForEach-Object { $_ -replace '^\d{4}-\d{2}-\d{2}T[\d:.]+Z\s?', '' }
$n = 1..$lines.Count | Where-Object { $lines[$_ - 1].Trim() -eq 'Cache Key:' } | Select-Object -First 1
if (-not $n) { throw 'No "Cache Key:" heading line found' }
$key = $lines[$n].Trim()                       # $n is 1-based, so this is the line AFTER the heading
if (-not $key -or $key.EndsWith(':')) { throw "Line after the heading is not a key value: '$key'" }
$key
```

The heading is matched on the **whole trimmed line** rather than as a substring, and the two guards reject an empty line or another heading. Any equivalent unambiguous parser is acceptable; what is not acceptable is grepping for the heading text and comparing what it returns.

**Also record `.. Prefix:`**, emitted at `config.ts:337-338` as `  - <keyPrefix>` two lines below the key. It is everything up to and including the `Windows_NT-x64` segment (`config.ts:96`), so it splits a divergence into a plain-text half and a hashed half without any further tooling. Diagnosis is in Section 9.6.

#### K1-L, the corroboration record. It can never withhold the verdict

Record the `Image Release:` URL for every compared job, taken from that URL only and never from the first `Version:` line of the `Runner Image` group (Section 2.8). If a compared pair drew **different** image versions and still MATCHED, record it: that is the strongest available corroboration, since it is the exact configuration that exposed the original defect. **It is never required, and a same-image draw costs nothing**, because K1-V compares emitted values and is satisfied whatever the draws are.

- **K1-L: CONSISTENT** (a mixed-generation pair matched), **CONTRADICTED** (a mixed-generation pair did not match, which will already have made a K1-V leg MISMATCHED), or **NOT YET OBSERVED**.

#### Verdicts

- **K1-S: COMPLETE** or **DEFECTIVE.** There is no third outcome and no waiting. DEFECTIVE means an enumerated component that the static reading is supposed to establish is not identical within a cohort at the commit under test.
- **K1-V: MATCHED**, **MISMATCHED** or **NOT APPLICABLE**, per leg.
- **K1 = PASS** iff K1-S is COMPLETE **and** at least one pre-land K1-V leg is MATCHED **and** no K1-V leg is MISMATCHED.
- **K1 = FAIL** if K1-S is DEFECTIVE or any K1-V leg is MISMATCHED. Do not merge; diagnose per Section 9.6.
- **K1 = NOT YET ESTABLISHED** if no pre-land leg has reached MATCHED or MISMATCHED. The merge waits. **This is not a deadlock**: K1-V1 exists on every run of this PR and needs no particular image draw, so this state means a job failed before its cache step, which is an ordinary diagnosable failure. Round 2's INCONCLUSIVE, which no action of ours could clear, has no successor here.
- **K1-L never affects the verdict.**

**K1 blocks the merge, not a warm cycle.** Round 2 required K1 to precede the next `cache-warm` run, which is unenforceable: the landing push queues the warm in the same act and a daily cron races independently (Section 16.3). K1 now runs before the merge, so the requirement is satisfied by construction and needs no cancellation, quarantine or manual timing.

#### What K1 does not prove, stated because the name invites the mistake

**K1 is a precondition and a regression check. It is never proof that a cache hit will occur.** A perfectly deterministic key can still miss: the measurement branch may carry a `Cargo.lock` that `main` does not, so K-8 legitimately differs; the entry may have been evicted; cache ref visibility may not reach the run. The exact-hit questions are the M2 probes and B1 at M3 and M4, not K1. A green K1 with a red B1 is a coherent outcome and routes to Section 9.6, which is why B0 is written as a precondition on Group B rather than as a substitute for any of its members.

**And K1-V proves the executions it compared, not a universal property.** A byte-identical full-key match establishes that those two jobs, on that commit, with the images they drew, computed the same key. **It does not establish that every future runner reporting the same `ImageOS` will supply the same matched environment**, because the hashed set is `process.env` and the runner owns it. That is why K1-V3 is a **standing** check that re-runs on every warm cycle rather than a one-off, and why a `windows-latest` image migration is listed below as an event that voids the result rather than merely as a risk.

**Why the round-2 name was withdrawn, and what round 4 changes about the claim.** PROVEN STABLE claimed universal key stability from a comparison of one component between two jobs that carry different `shared-key` values by design; it could not see most of the key. Round 3 fixed the comparison but then rested the verdict on a static reading that cannot see the runner. **What the gate claims now is narrower and true: the repository's configuration cannot divide a cohort at this commit (K1-S), and the compared executions did in fact compute identical complete keys (K1-V).** Neither claim is dressed up as more than it is.

**When each part must be re-run.**

- **K1-S**, whenever a static input could have moved: a change to any of the seven `with:` blocks, an `env:` block or a `CARGO`/`RUST*`/`CC`/`CFLAGS`/`CXX`/`CMAKE` token added to any of the three workflow files, a `prefix-key` or `add-rust-environment-hash-key` change, or a change in what `@v2` resolves to (Section 10.7).
- **K1-V**, whenever a runtime input could have moved: a `windows-latest` major image migration, a change to the toolchain step of any cohort member, or a new step inserted before a cache step in one member of a cohort and not the others. K1-V3 supplies this automatically on every warm cycle, which is why no manual schedule is attached to it.

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

**Log-line evidence**, from raw job logs. **ANSI escapes must be stripped before matching** or the cargo counters silently read as zero. The baseline log demonstrates the need directly: a naive grep for the cargo counters found nothing until the escapes were stripped (Section 2.6), which is evidence that `CARGO_TERM_COLOR` was effectively `always` at runtime. **The repository does not set it**, and round 4 corrects the round-3 note that treated that absence as meaning the variable is not there: it is runner-supplied, it is listed in the `Environment considered` block of all six jobs in Section 2.8, and `config.ts:123-130` therefore hashes it. Strip unconditionally: it is harmless on a log with no escapes and mandatory on one with them.

```
$clean = [regex]::Replace($log, "\x1b\[[0-9;?]*[ -/]*[@-~]", "")
```

- Cache restore occurred: the `Rust cache` step does not contain `No cache found.`
- **Cache hit was exact**: the `Rust cache` step's `cache-hit` output is `true`. Cross-check by comparing the extracted **key value** between M2 and M3; identical values mean an exact hit. Extract per Section 9.2.1, from the line **after** the `Cache Key:` heading, never from the heading itself.
- Cargo work counters: `Compiling `, `Checking `, `Fresh ` and `Downloaded ` line counts, per job, computed on the ANSI-stripped log.
- Bundling removed: the build step contains `Built application at:` and none of `Running light to produce`, `Running makensis to produce`, `Info Verifying wix package`, `Info Verifying NSIS package`.
- Save suppressed: `Post Rust cache` completes in under 5 s.
- `rustc -vV` from the warm jobs, recorded for M1 through M6 so key rotations are attributable.
- **Four things recorded from every job of every measurement**, not only at K1, because the record is what makes a later question answerable without a fresh run. This costs nothing: the whole block is emitted within seconds on every job regardless of hit, miss or `lookup-only`.
  1. **The complete key value**, taken from the line after the `Cache Key:` heading per Section 9.2.1. **Record the whole value, not the environment-digest component alone**, because the cohort comparison is over complete keys and a record that kept only the suffix could not answer it later. This is also what makes the runtime OS and architecture segments visible in the record; round 3 enumerated neither, although `Windows_NT-x64` was present in every key already captured (Section 2.8).
  2. **The `.. Prefix:` value**, emitted two lines below the key value at `config.ts:337-338`. It is the plain-text half of the key and it is what Section 9.6 uses to localise a divergence in one step.
  3. **`GITHUB_SHA`**, so that a K-8 difference can be told apart from a key defect. K1-V1 requires it equal across the compared jobs; K1-V2 requires equal **trees** instead, because its two runs differ in `GITHUB_SHA` by construction.
  4. **The `Image Release:` URL.** Take the image version from that URL only, never from the first `Version:` line of the `Runner Image` group, which is the Image Provisioner version and is identical across generations (Section 2.8). Recording the image alongside the key is what makes a later "did the key depend on the draw" question answerable from the record, which is the gap that let the first cycle reach certification.

**API evidence:** `steps[]` durations from `/actions/runs/<id>/jobs`; entries, `ref`, `key` and `size_in_bytes` from `/actions/caches`; `active_caches_size_in_bytes` and `active_caches_count` from `/actions/cache/usage`.

### 9.5 Pass criteria, in three independent groups

**Group A: the deterministic changes landed. Evaluated on M1, before any cache exists. All must hold.**

- A1. No bundling markers in `Build Windows release binary`, and `Built application at:` present.
- A2. `Post Rust cache` under 5 s in both gate jobs.
- A3. The smoke still passes: `passed=4 skipped=0 failed=0`.
- A4. `bundle-validation` runs on this PR (it touches `package.json`) and produces both an `.msi` and a `-setup.exe`.

**Group B: the cache actually accelerated compilation. Evaluated as M3 and M4 against M1. All must hold. These are the criteria immune to runner variance.**

- **B0. K1 returned PASS before the merge** (Section 9.2.1), which requires **K1-S COMPLETE and at least one pre-land K1-V leg MATCHED and no K1-V leg MISMATCHED.** This is a precondition, not a measurement, and **Group B cannot be evaluated at all without it.** B1 through B6 are all satisfiable by luck when the key depends on which image generation a job drew, so without B0 a green Group B carries no information.
  - **Round 4 changed how B0 is satisfied and nothing else about its role.** Round 3 made a desk check alone sufficient; it is not, because a desk read of this repository cannot establish the runner-supplied inputs the action hashes (Sections 9.2.1 and 17.1). The observed leg closes that hole by comparing emitted values.
  - **B0 still always terminates, and by a stronger argument than round 3's.** K1-S needs no CI run. K1-V1 needs two runs the fix PR produces anyway and **is satisfied whatever runner images the compared jobs draw**, so it depends on no mixed fleet and no scheduler luck. Round 2's version of B0 could be made permanently unsatisfiable and is superseded (Section 16.1); round 3's could be satisfied without proving what it claimed and is superseded (Section 17.1).
  - A NOT YET OBSERVED **K1-L** corroboration record does not withhold B0. A NOT APPLICABLE K1-V leg does not withhold B0 as long as the other pre-land leg MATCHED.
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

1. `No cache found.` in M3 → the key does not match. Compare the extracted **key values** from M2's warm log and M3's, never the `Cache Key:` heading (Section 9.2.1). **Localise before hypothesising**, using the record Section 9.4 requires:
   - **`.. Prefix:` differs** → the divergence is in plain text and is one of K-1, K-2, K-4 or K-5. A different `v0-rust` means someone set `prefix-key` on some steps and not others; a different `gate-*` means a `shared-key` divergence; a different `Windows_NT-x64` means the runner OS or architecture moved, which is a `windows-latest` fleet change and not a repository defect.
   - **Prefixes agree, environment digests differ** → K-7: the installed-toolchain set or some matched `process.env` value. Compare the recorded `rustc -vV` first.
   - **Prefixes and environment digests agree, lockfile digests differ** → K-8, that is repository content. Confirm the branch's `Cargo.lock`, `Cargo.toml` set and toolchain files against `main`'s.
   **Do not re-run the Section 9.3 purge as a first move**; the corrected filter now protects the `gate-*` entries, but the diagnosis is a key comparison, not a purge.
2. Restore occurred but `cache-hit` is `false` → a non-exact restore. **It localises the difference, but not as far as round 4 claimed, and not as far as round 5 claimed either. Round 6 inverts the default: the conservative reading below is the normal path, and sole attribution to K-8 is an optional upgrade that must be earned.** `restore.ts:41` passes `config.restoreKey` as the **restore-keys** argument of `restoreCache`, and `@actions/cache` documents that lookup as **prefix matching** (`packages/cache/src/cache.ts`, the `restoreCache` contract: the primary key is "an explicit key for restoring the cache. Lookup is done with prefix matching", `restoreKeys` is the ordered fallback list, and the return value is "the key for the cache hit"). So the returned entry's key is guaranteed only to **start with** this run's `restoreKey` (`config.ts:140`, the key through the environment digest), and `cache-hit` is `false` because `restore.ts:44-47` compared that returned key against the complete `cacheKey` (`:270`) and they differed.

   **What a non-exact restore unconditionally establishes is a fact about the emitted strings, not about the components: the emitted prefix through the environment digest matched, and the remaining suffix differed.** It does **not** establish that the remaining suffix is one same-layout lockfile digest, so on its own it does **not** mean that K-1 through K-7 agreed and only K-8 moved. Round 4 stated that stronger conclusion; it is withdrawn.

   - **Record the actual returned cache key before diagnosing anything.** `restore.ts:48` already logs it: `Restored from cache key "<key>" full match: false.`, or `Found ...` under `lookup-only`. Extract that string, and compare it against this run's own complete key value taken from the line after the `Cache Key:` heading per Section 9.4; this run's `restoreKey` is that value minus its final separator and digest segment. **Diagnosing this case without the returned key is guesswork**, because the returned key is the only evidence in the log of what the *writer* emitted.
   - **The default diagnosis. Always available, and never wrong.** State the finding in exactly these terms and no stronger: the emitted prefix through the environment digest matched, **the final suffix string differs**, and **K-8 and K-9 both remain open**. Do not assert which one moved. Then work the two open components in this order, which is cheapest-evidence-first and is deliberately not a claim about which is likelier:
     1. **Compare the K-8 repository inputs** between the measurement branch and the commit the writer built from: `Cargo.lock`, the `Cargo.toml` manifest set, and `.cargo/config.toml`, `rust-toolchain` and `rust-toolchain.toml` under the `workspaces` roots (`config.ts:163-268`). This is a comparison of two trees, needs no log, and can be done at a desk. A difference in any of those files accounts for the suffix without further evidence.
     2. **Check the action resolution.** Each job's setup log records the commit the floating `swatinem/rust-cache@v2` tag resolved to for that job. Read it in the writer's job and in the reader's job and compare. Different SHAs put **K-9** in play and route the diagnosis to Section 10.7; equal SHAs close K-9 by observation, which is also leg (a) of the upgrade below.
     **Do not compare `ImageVersion`: it is deliberately not a key input** (Section 4.1), so an image difference between M2 and M3 is expected, is not a cause, and diagnosing from it sends the investigation to the wrong place.
   - **The optional upgrade: sole attribution to K-8.** Permitted only when **both** legs hold, and never on either alone:
     - **(a) Same resolved implementation.** Writer and reader are shown to have resolved the **same exact `swatinem/rust-cache` commit**, taken from the action-resolution SHA in both jobs' setup logs. This is the only way K-9 can be closed: it is a property of the action the runner fetched, and nothing this repository emits records it.
     - **(b) Reviewed-layout suffix shape.** The remainder of the returned key, after this run's `restoreKey`, is exactly one separator followed by one 8-character hex segment and nothing further: `HASH_LENGTH` is 8 (`config.ts:18`, `:378-380`) and the reviewed layout appends exactly one such segment after `restoreKey` (`:266-270`).

     **Why the pair is sufficient, stated so it can be checked rather than trusted.** Leg (a) fixes the writer's construction to the reviewed one, so the returned key is `<P_w>-<env_w>-<lock_w>` with both digests exactly 8 characters. The prefix match makes the reader's `restoreKey`, which is `<P_r>-<env_r>`, a prefix of it. Leg (b) fixes the remainder at 9 characters, so `<P_w>-<env_w>` and `<P_r>-<env_r>` are prefixes of the same string of the same length and are therefore the same string: `P_w = P_r` and `env_w = env_r`. K-1 through K-7 agree and only K-8 differs.

     **Why neither leg can be dropped, and what round 5 got wrong.** Round 5 treated (b) alone as establishing that the reviewed layout produced the entry. That is withdrawn. **Shape proves grammar compatibility, not provenance.** A future `@v2` revision can preserve the outer grammar `<restoreKey>-<8 hex>` while changing the final segment's hash algorithm, its hashed file set or its meaning, and a layout change need not change either the segment count or the segment width. Then, inside Section 10.7's own accepted scenario, the entries on `main` are written under the new revision, the pin-back lever puts the reviewed revision back on the reader, the reader prefix-matches the new entry, `cache-hit` is `false`, the remainder passes (b), and the real difference is **K-9** while the diagnosis says K-8. Leg (a) is what excludes that. Leg (b) is not redundant under (a) either: prefix matching constrains only leading characters, so a writer prefix of a different length can in principle still prefix-match, and it is (b) that forces the two to the same length and lets the equality argument close.

     **When writer provenance is unavailable, the upgrade is unavailable and the default stands.** An expired setup log, a run that was not retained, or a writer job that cannot be identified all produce the same outcome: record "the suffix differs, K-8 and K-9 both open", carry the K-8 tree comparison as far as it goes, and stop there. **That is the correct terminal state of this diagnosis, not a failure to finish it.**
   - **Once the upgrade has been taken and K-8 is the established difference:** confirm the measurement branch did not touch `Cargo.lock`, any `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain` or `rust-toolchain.toml`. If the repository content genuinely matches, the attribution and the evidence disagree, so **re-run K1-S and re-read the recorded key values** (Section 9.2.1) rather than waiting for a live run, and check the static conditions: an `env:` block or a `prefix-key` added to one of the three workflow files. Condition (ii) is already closed by leg (a) on this branch of the diagnosis, so a moved `@v2` is not the explanation here; it is the default path above, not this one, that routes to Section 10.7.
   - **A green K1 with a `false` here is a coherent outcome, not a contradiction**: K1 proves that compared executions computed the same key, never that a cache is reachable.
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

**The lever, so that it is actionable rather than theoretical.** If a link failure against restored native artefacts is ever observed, add or increment a `prefix-key` input on all seven `swatinem/rust-cache@v2` steps at once. **The input's default is the single value `v0-rust`** (`action.yml:5-8`), so the bump is to a value such as `prefix-key: 'v1-rust'` on every step, not to a bare `v1`, which would silently drop the `rust` segment as well as rotating the key. It must be all seven, because the steps sharing a `shared-key` must agree on every key input or the cache becomes unreachable, which is the same failure this amendment exists to remove. That rotates both entries deterministically, the Section 4.2 recovery path regenerates them without waiting for a `main` push, and the event is attributable because both warm jobs record `rustc -vV`.

**What would make this worth revisiting**, recorded so a future editor has a threshold rather than an instinct: a first observed occurrence of the link failure, or GitHub publishing a stable per-image toolset identity that does not vary between concurrently served generations of the same image. Neither holds today.

### 10.7 The floating `swatinem/rust-cache@v2` tag. Open and accepted, with two levers

**Added in round 3**, because `dev-rust-grinch` was right that the key constructor is load-bearing and round 2 left it unmentioned. It is recorded as a decision rather than fixed here.

**What is exposed.** All seven steps use the floating tag `@v2`, so the implementation that *builds* the key is chosen at run time. Every observed job resolved `e18b497796c12c097a38f9edb9d0641fb99eee32`, which is the revision `dev-rust` read and the revision condition (ii) of Section 9.2.1 names. If the `v2` tag moves and the new revision changes the key layout, a writer and a reader that resolve different revisions compute different keys and the cache goes unreachable. This is component **K-9** in Section 9.2.1's enumeration, renumbered from K-2 in round 4 when the enumeration was corrected.

**Position: do not pin in this plan.** The reasoning, so it is not re-litigated:

| | Cost of leaving `@v2` floating | Cost of pinning to a SHA |
|---|---|---|
| Failure mode | A key-layout change makes the caches unreachable | None from the key, but security and correctness fixes stop arriving until someone bumps it |
| Blast radius | **Bounded and self-healing.** One cold cycle. The next warm, from the landing push or from the daily cron, rewrites both entries under the new layout and the readers converge | Bounded, but a stale action can persist indefinitely because nothing forces a review |
| Detection | **Loud and immediate.** `verify-debug-cache` or `verify-release-cache` turns the workflow red the first time the layout diverges, which is K1-V3 | Not applicable |
| Scope | Zero. The change is already a one-token edit across seven steps | Seven more edits, on a branch whose whole content is one token, widening a fix that reviewed cleanly |

The exposure is the same class and roughly the same magnitude as a rustc rotation, which this plan already accepts and already makes attributable. It is caught by machinery that exists and runs daily, and it costs one warm cycle when it fires.

**The levers, so this is actionable rather than theoretical.**

1. **Passive, already in place.** The daily `schedule` in `cache-warm.yml:7-8` regenerates both entries under whatever layout is current, so a layout change heals within one day without anyone acting.
2. **Active, if a layout change ever costs more than that.** Replace `@v2` with `@e18b497796c12c097a38f9edb9d0641fb99eee32` on **all seven steps at once**, for the same reason all seven carry the same `env-vars`: cohort members that disagree on the resolved implementation can disagree on the key. **This lever is not free, and round 4 corrects the round-3 claim that it was.** The action revision is not itself hashed, but the key layout is a function of the implementation, so pinning back to an older revision after `@v2` has already moved reverts the layout while the entries on `main` were written under the new one. In exactly the scenario that would motivate using the lever, applying it costs one cold cycle, which the next warm then repairs. `dev-rust-grinch` raised this in round 4 and it is accepted.

**What would make this worth revisiting**, as a threshold rather than an instinct: a first observed occurrence of a `@v2` layout change breaking a cohort, or a repository-wide policy decision to pin third-party actions, which would cover `actions/checkout@v5`, `actions/setup-node@v5` and `dtolnay/rust-toolchain@stable` in the same act and does not belong to #1216.

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

- **K1, a new key-stability gate, is added as Section 9.2.1** and adopted from `dev-rust`. Group B gains **B0**, making K1 a precondition of the whole group. **Corrected in round 3:** as round 2 wrote it, K1 blocked the next warm cycle and M3 and admitted a third verdict, INCONCLUSIVE. Both were defective. The warm ordering is unenforceable and K1 now blocks the merge instead; INCONCLUSIVE could be permanent and is replaced by a static proof plus non-blocking corroboration. **Corrected again in round 4:** that static proof cannot be sufficient, so the observed within-cohort complete-key match is promoted from non-blocking corroboration to a required leg. Section 9.2.1 as it now stands is the gate; Sections 16.1, 16.3 and 17.1 carry the reasoning. B0 survives both rounds with its role intact.
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

## 16. Architect's amendment, Step 7 consensus round 3

**Verdict: READY_FOR_IMPLEMENTATION.** This section is authoritative wherever it contradicts anything earlier in this plan, Sections 14 and 15 included.

**Why this round exists.** `dev-rust-grinch` reviewed commit `93e20674a83eebcafb0a569470dc6a3315b6523b` on branch `fix/1216-cache-key-imageversion` and **passed the implementation**: exactly three workflow files, seven insertions and seven deletions, seven `env-vars: 'ImageOS'` in the intended files, `ImageVersion` absent from `.github/workflows`, and a byte-level revert proof that substituting the old token back makes all three blobs identical to their `fd90894` originals. **The code is correct and round 3 does not touch it.** What failed review was the acceptance gate round 2 had added: three blocking defects in K1 and B0, two of them independently verified by the tech lead. All three are accepted here. None is disputed.

### 16.1 Finding 1, CRITICAL: B0 could be permanently impossible. Accepted and repaired

**The defect.** Round 2's K1 granted PROVEN STABLE only when two sampled gate jobs drew **different** runner image versions. Every same-version draw was INCONCLUSIVE with no bound and no alternate path. If GitHub finishes the `win25-vs2026` rollout, every ordinary PR draws one generation forever, K1 can never pass, and B0 permanently prohibits M2, M3 and all of Group B for a fix that is statically correct. **An acceptance gate must not depend on a vendor retaining a mixed fleet or on scheduler luck.**

**The repair.** K1 is split. **K1-S is a static proof**: it enumerates every key component from the reviewed action revision `e18b497796c12c097a38f9edb9d0641fb99eee32`, checks each against the seven `with:` blocks and five stated conditions, and terminates on a single desk reading with no CI run at all. It is now the sufficient path for B0. **The mixed-image observation is demoted to corroboration** under K1-L, where it is recorded when it happens and required never. Round 2's INCONCLUSIVE is removed as a terminal state and replaced by NOT YET OBSERVED, which cannot withhold a verdict.

> **Superseded in round 4 on one point.** "It is now the sufficient path for B0" is withdrawn: a static reading cannot be sufficient, because it cannot establish runner-supplied values (Section 17.1). B0 now also requires an observed within-cohort complete-key match. **The finding this section answers survives intact**, and so does the repair's core: the gate must not depend on a mixed fleet or on scheduler luck. Round 4's required leg is satisfied by same-image draws, so it strengthens that property rather than undoing it.

**No new evidence was needed and none was requested.** The material was already gathered: `dev-rust`'s read of `src/config.ts` at the resolved revision, its enumeration of the hashed inputs, and its offline reimplementation that reproduces the observed six-job partition exactly. The architect added what the repository itself proves, which is set out in Section 9.2.1 and summarised in 16.6.

### 16.2 Finding 2, HIGH: PROVEN STABLE overclaimed. Accepted, renamed, and expanded

**The defect.** Round 2 compared only the environment-hash component, between two jobs that carry **different `shared-key` values by design**. Matching suffixes across two point releases showed only that one sampled image difference did not move one component. Left open: the rustc component, the floating `@v2` key constructor, and `ImageOS` in a future `windows-latest` migration. Because complete keys were never compared within the real writer, probe and reader cohorts, the non-`ImageVersion` form of the same "passed by luck" hole B0 exists to close remained logically possible.

**The repair, on all four points the finding raised.**

1. **Renamed.** The gate is the **key-determinism gate**; the verdict PROVEN STABLE is withdrawn. What it now claims is key determinism **under five stated conditions**, and Section 9.2.1 states them as a conditional invariant rather than an assertion.
2. **Every component enumerated**, K-1 through K-5, from the reviewed and resolved action revision, each with what it varies with and each argued against the repository text.
3. **Complete key-input signatures compared within each `shared-key` cohort**, never across cohorts and never on the suffix alone. Section 9.4 now requires the whole `Cache Key:` line to be recorded for exactly this reason.
4. **The floating `@v2` is no longer unmentioned.** It is component K-2, it is condition (ii) of the invariant, and Section 10.7 records the decision not to pin it as an accepted risk with two named levers.

> **Superseded in round 4 on items 2 and 3.** The enumeration was **not** complete: it omitted the runtime OS and architecture segments and mis-stated the `prefix-key` default, and it is replaced by a nine-component list in Section 9.2.1 (Section 17.2). Item 3's instruction to record "the whole `Cache Key:` line" is the defect corrected in Section 17.4: the heading and the value are on separate lines, so the value must be taken from the line that follows. Item 3's numbering of the floating `@v2` as K-2 is renumbered to K-9. Items 1 and 4 stand.

**The two building blocks the finding pointed at were used rather than re-derived.** `dev-rust`'s check 7 collapses each cohort to one key-input signature, three `gate-debug` and four `gate-release`, and `grinch` reproduced that collapse independently at this HEAD. Section 9.2.1's cohort table is that result, recorded with the file and line of each member's `env-vars`.

**Two additions the architect brought, both checkable from the repository and neither requiring a run.**

- **The repository contributes no hashed environment variable at all.** None of `pr-regression-gates.yml`, `cache-warm.yml` or `bundle-validation.yml` contains an `env:` block at workflow, job or step level, and none contains a `CARGO`, `RUSTUP`, `RUSTC`, `RUSTFLAGS`, `CMAKE`, `CFLAGS`, `CXX` or `CC` token anywhere. `prefix-key` likewise appears nowhere in `.github/workflows/`. The hashed set is therefore entirely runner-supplied, which is what makes the static argument close. It is also the condition a future editor is most likely to break by accident, so it is written as condition (v) rather than left implicit.
- **Different pre-cache step sequences are already measured not to perturb the hash.** At M2, `warm-release` ran `setup-node`, the npm pin and `npm ci` before its cache step while `verify-release-cache` ran only checkout and the toolchain; they drew the same image and emitted the same environment hash `3d5bdf05` (Section 2.8). This closes the obvious objection that the writer and probe jobs are not comparable, using evidence already in the plan.

### 16.3 Finding 3, HIGH: the K1-before-warm order was unenforceable. Accepted, and the tech lead's design input adopted

**The defect.** `cache-warm.yml:3-9` runs on `push` to `main` and on a daily `cron`. The landing push queues the warm in the same act, so a post-land K1 cannot precede it. There was no gate, no cancellation path and no quarantine, and manual cancellation is itself a race. The sequence round 2 specified could not happen. **Verified independently by the architect at the file.**

**The repair: K1 runs pre-land, on the PR of `fix/1216-cache-key-imageversion`.** The tech lead's design input is adopted without modification, and the architect's position is that it is the correct resolution rather than merely an acceptable one: it removes the race instead of managing it, and a race that has been removed cannot be lost. For a same-repo PR the gate jobs execute the branch's own workflow definitions, so they emit keys built with `env-vars: 'ImageOS'` while `main` is untouched and no warm can fire.

**Three consequences the architect verified, which make the pre-land placement strictly better than post-land rather than merely equivalent.**

1. **The `gate-release` cohort has two live members on that PR.** `bundle-validation` fires because its `paths:` filter lists `.github/workflows/bundle-validation.yml` (`bundle-validation.yml:13`) and the fix changes that file. So `windows-release-cli-smoke` and `bundle-validation` both run as `save-if: 'false'` readers on the same `pull_request` event. **Their complete keys must be byte-identical, and that is a within-cohort full-key comparison available before anything lands.** No post-land ordinary PR run offers this, because it compares two jobs in different cohorts. **Corrected in round 4:** this sentence originally called them two jobs of **one** `pull_request` run and concluded that `GITHUB_SHA` was shared by construction. They are in separate workflow **files**, so the event starts a separate run of each and `GITHUB_SHA` equality must be **recorded and required** rather than assumed. Section 9.2.1's K1-V1 states the corrected requirement.
2. **The `gate-debug` cohort gets a comparison too**, from the double trigger at `pr-regression-gates.yml:3-12`: the same commit produces a `push`-event run and a `pull_request`-event run, each with its own `rust-regression`. Its precondition is stated rather than assumed, since the merge ref's tree equals the head's tree only while the base is an ancestor of the head.
3. **The same run is also the replacement cold control.** The only `gate-*` entries on `main` at that moment are the two stranded old-generation entries, which cannot exact-match a key built with `env-vars: 'ImageOS'`, so both gate jobs run cold. K1 and M1 are one run and one cost.

**The "block the next warm cycle" requirement is replaced, not weakened.** K1 blocks the **merge**, which is enforceable by the person merging and needs no timing. Section 9.2 now states explicitly that automatic warms after the merge are expected, that `concurrency: cache-warm-main` with `cancel-in-progress: false` makes overlapping warms harmless, that M2 is the first such run whose checkout contains the amendment and whose four jobs are green, and that **nothing needs to be re-run, cancelled or discarded.** A warm fired by an unrelated merge landing before the fix writes old-generation entries, which are stranded exactly like the two in Section 9.3 and are left to LRU by that same decision.

### 16.4 The advisory, folded in

K1 can pass while the cache still misses for an unrelated reason: a `main`-versus-PR `Cargo.lock` difference, a writer and reader `shared-key` mismatch, or cache ref visibility. **K1 is a precondition and a regression check, never proof of cache reachability.** This is now stated in three places rather than one: in Section 9.2.1's closing subsection, in B0 itself, and in Section 9.6 case 2, where a green K1 alongside a `cache-hit == 'false'` is explicitly named a coherent outcome rather than a contradiction. The exact-hit questions remain M2's probes and B1 at M3 and M4.

### 16.5 Judgement calls: closed, no action

All three round-2 judgement calls were disclosed by the architect and accepted by `dev-rust-grinch` with reasoning. **They are recorded here as closed and are not to be re-opened.**

1. **YAML parse instead of `actionlint`.** A scalar-only diff cannot create an Actions-schema shape change.
2. **Omitting `cargo check` and `cargo clippy` from local validation.** Zero Rust paths changed; running them would overwrite `dist/` and generate a multi-gigabyte `target/` in the shared replica for no information.
3. **Leaving the two stranded cache entries to LRU.** Their old full keys cannot exact-match the new generation, 1.51 GB is inside the recorded budget, and keeping the purge exclusion avoids risking deletion of current `gate-*` entries.

### 16.6 Sections changed by this amendment

Header and status block; **8** steps 7, 8 and 10 (K1 pre-land, merge conditioned on it, automatic warms expected); **9.2** (K1 moved before the merge, the M1 replacement named, automatic warms stated as expected); **9.2.1** (rewritten in full: conditional invariant, K-1 through K-5 enumeration, cohort table, K1-S static proof, K1-L1 and K1-L2 corroboration legs, new verdict set, what K1 does not prove, when to re-run); **9.4** (record the complete `Cache Key:` line, not the suffix); **9.5** B0 (PASS instead of PROVEN STABLE, and satisfiable by desk check); **9.6** case 2 (re-run K1-S, check conditions (v) and (ii) first, green K1 with a miss is coherent); **10.7** (new, the floating `@v2` as an accepted risk with two levers); **15.4** first bullet (round-2 record corrected where round 3 reversed it); **16** (this section).

### 16.7 What round 3 deliberately does not change

- **The implementation.** `93e2067` stands as reviewed. No workflow file, job, step, trigger, command, permission, concurrency group, `shared-key` or `workspaces` value moves.
- **The `ImageVersion` removal and Section 10.6's accepted residual risk.** Settled in round 2, not re-opened.
- **Scope.** #1217, #1218 and #1219 stay out. Section 10.7 records a position on pinning `@v2` rather than pinning it, and explicitly routes any broader action-pinning policy away from #1216.
- **Groups A, C and D, and B1 through B6.** Only B0 changes, and only in how it is satisfied.

### 16.8 Verification conditions for this amendment

Facts were taken from the tech lead's dispatch, which states the evidence is complete and instructs that no further `dev-rust` or `dev-rust-grinch` round be spent re-establishing it. **The architect independently verified in the repository at `93e20674a83eebcafb0a569470dc6a3315b6523b`:** the `cache-warm.yml:3-9` trigger block quoted in Finding 3; all seven `env-vars: 'ImageOS'` occurrences with their files and lines; that all seven steps carry `workspaces: '. -> target'` and that `prefix-key` appears nowhere in `.github/workflows/`; that `save-if` and `lookup-only` are the only other differing inputs and that neither feeds the key; that the two cohorts have three and four members and that each member's toolchain step matches the others in its cohort; that all seven run on `windows-latest`; that none of the three workflow files contains an `env:` block or any `CARGO`, `RUSTUP`, `RUSTC`, `RUSTFLAGS`, `CMAKE`, `CFLAGS`, `CXX` or `CC` token; that no `.cargo/config.toml` exists; that `pr-regression-gates.yml:3-12` produces the double trigger K1-L1 relies on; and that `bundle-validation.yml:13` lists `.github/workflows/bundle-validation.yml` in its `paths:` filter, which is what makes `bundle-validation` fire on the fix PR.

**Not re-derived, by instruction, and attributed rather than claimed:** the read of `src/config.ts` at revision `e18b497796c12c097a38f9edb9d0641fb99eee32` and the enumeration of hashed inputs (`dev-rust`); the offline reimplementation reproducing the six-job partition, with `dev-rust`'s own UNVERIFIED label preserved on the absolute digest values (Section 2.8); the cohort-signature collapse of check 7 (`dev-rust`, reproduced by `dev-rust-grinch`); and the implementation verification of `93e2067` (`dev-rust-grinch`).

**One observation recorded rather than acted on, and round 4 corrects its reasoning.** Section 9.4 stated that the jobs run with `CARGO_TERM_COLOR: always`, and that variable is set nowhere in the three workflow files nor anywhere else in the repository, so the plan did not evidence its own claim from repository text. Round 3 then concluded that `CARGO_TERM_COLOR` "is not set by the repository and therefore cannot divide a cohort". **That inference is wrong and it is exactly the error round 4 exists to remove.** `config.ts:123-130` hashes every prefix-matching entry of `process.env` whoever supplied it, and `CARGO_TERM_COLOR` is in the `Environment considered` block of all six jobs in Section 2.8, so it is in the key and could in principle divide a cohort. The evidence that it does not is that those six jobs agreed, which is evidence about six samples rather than a static implication. The ANSI stripping stays, unconditionally, on the stronger ground given in Section 9.4: the baseline log's counters were unreadable until the escapes were stripped, which is direct evidence the variable was live at runtime.

**Codebase Memory was not used for this amendment, under explicit authorization from the tech lead.** The query layer is defective: the full gate returns `ready` and every subsequent query fails with `Repository changed`. Additionally, this branch has no upstream, because it was deliberately not pushed, so the gate can also fail with `status.git.base_sha must be a string`. **Direct reads were authorized and the 80-line fallback limit was waived.** The branch was not pushed to satisfy the gate.

## 17. Architect's amendment, Step 7 consensus round 4

**Verdict: READY_FOR_IMPLEMENTATION.** This section is authoritative wherever it contradicts anything earlier in this plan, Sections 14, 15 and 16 included.

**Why this round exists.** `dev-rust-grinch` verified the round-3 digest `B58CF742...`, confirmed the implementation at `93e20674a83eebcafb0a569470dc6a3315b6523b` is untouched, and **accepted round 3's finding-3 repair**: pre-land K1 removes the warm race and "K1 blocks the merge" is operationally enforceable. **It failed round 3's findings 1 and 2.** READY on `B58CF742...` is invalidated. **Both failures are accepted. Nothing in the dispatch is rejected.** Two items are corrected rather than accommodated, and both are corrections of provenance or remedy, not of substance; they are in 17.6 and 17.7.

**The code is still not touched.** `93e2067` stands as reviewed. Round 4 changes only how the change is proved and accepted.

### 17.1 The core defect, accepted: K1-S cannot be a sufficient proof

**The finding.** A desk read of this repository cannot establish runner-supplied values. `src/config.ts:123-130` enumerates **all** of `process.env` and hashes every prefix-matching name-equals-value pair regardless of who supplied it, so round 3's condition (v), "the repository sets no `env:` block", proved repository absence and not process-environment equality. K1-S could therefore return COMPLETE while runtime inputs differed, which false-passes exactly the class of nondeterminism B0 exists to exclude.

**Accepted without reservation, and the architect verified the mechanism directly** by reading `src/config.ts` at `e18b497796c12c097a38f9edb9d0641fb99eee32` rather than relying on the citation. The plan's own evidence settles it: Section 2.8's six jobs list runtime-supplied `CARGO_HOME`, `CARGO_INCREMENTAL` and `CARGO_TERM_COLOR`, none of which this repository sets, and two independent traces of runner-supplied `CARGO*` variables were already in the plan without being read as such (the baseline `CARGO_INCREMENTAL: 0` in Section 2.3, and the ANSI escapes that forced the stripping step in Section 2.6).

**The general error, named so it is not repeated.** Round 3 proved a property of the *repository* and stated it as a property of the *key*. **A cache key is a runtime output of a process whose environment the repository only partly controls, so no amount of reading this repository can prove two jobs computed the same key.** The type of the proof has to match the type of the thing proved. This is the same class of mistake as round 1's, recorded in Section 15.6: reasoning about the case that was analysed instead of the case that occurs.

**The repair.** B0 becomes: **K1-S COMPLETE and an observed within-cohort complete-key value match.** Section 9.2.1 is rewritten around it, K1-S is demoted from *the* proof to the *configuration* proof, and the observed comparison is promoted from non-blocking corroboration (round 3's K1-L1) to a required leg (K1-V).

### 17.2 Finding 1, accepted: the component enumeration was incomplete and K-1 was wrong

**The finding.** `src/config.ts:91-94` appends a runner OS segment and a runner architecture segment from `os.type()` and `os.arch()`. Both are runtime values and both were absent from round 3's K-1 through K-5 and from its five conditions. K-2 cannot absorb them, since K-2 was about layout varying with the action revision. Additionally the `prefix-key` default is the single value `v0-rust`, not a `v0` default plus a literal `rust`.

**Verified by the architect at the source, not accepted on citation.** `config.ts:92` is `os.type()`, `:93` is `os.arch()`, `:94` appends both, and `:74` is `core.getInput("prefix-key") || "v0-rust"`; `action.yml:5-8` confirms the default. The corroboration from the plan's own evidence is exact: the recorded key `v0-rust-gate-debug-Windows_NT-x64-cfee4d59-a04e7ee9` decomposes onto the construction segment for segment.

**The repair.** Section 9.2.1 now carries a nine-entry enumeration built from a fresh read of the file, with a column stating **how each component is established equal within a cohort** rather than only what it varies with. It separates the components a desk check can close (K-1, K-2, K-3, K-6, the static half of K-7, the input half of K-8, and K-9 as a stated condition) from the ones only an observation can close (K-4, K-5, the runtime half of K-7). Three additional components round 3 never listed are now in it: `add-rust-environment-hash-key`, which gates **both** digest suffixes (`config.ts:136`, `:163`); the `cmd-format` input, which determines how the toolchain identities are gathered; and the key layout as its own entry, so that runtime values are no longer filed under it.

### 17.3 Finding 2, accepted: condition (v) did not close K-4 and condition (iii) was too narrow

**The finding.** Per 17.1 for condition (v). Additionally, condition (iii) must mean the **complete installed-toolchain set**, not the default `rustc -vV`: `getRustVersions` at `src/config.ts:391-407` hashes every installed toolchain identity.

**Verified at the source.** `getRustVersions` (`:388-411`) adds `parseRustVersion(rustc -vV)` and then, for every toolchain reported by `rustup toolchain list --quiet`, adds `rustup run <toolchain> rustc -vV`. The set is sorted and each member hashed as `` `${release} ${host} ${commit-hash}` `` (`:106-115`). **A toolchain preinstalled on the runner image is therefore in the key**, which makes condition (iii) a runtime fact and not a property of the `dtolnay/rust-toolchain@stable` step.

**The repair.** The invariant's conditions are restated as (iii) the same complete installed-toolchain set, (iv) the same runner OS type and architecture, and (v) the same value for every matched `process.env` entry, and the text says explicitly that **(iii), (iv) and (v) are discharged by the observed comparison and never by reading the repository.** The `env:`-absence check survives, correctly labelled: it is necessary, because a future editor adding `env: RUSTFLAGS: ...` to one of the three files would divide a cohort silently, and it is not sufficient.

### 17.4 Finding 3, accepted: "compare the Cache Key line" could compare a constant heading

**The finding.** `src/config.ts:335-336` emits two log lines, a bare `Cache Key:` heading and the indented value on the next line, so a literal extraction of the heading captures an identical constant and divergent keys would read as byte-identical.

**Verified at the source**, along with the aggravating detail that `:333-334` emits `Restore Key:` in the same two-line shape immediately before. The tech lead's own extractions used trailing context, so the keys recorded in Section 2.8 are real values; **the defect was in the plan's instruction, not in the evidence.**

**The repair.** Section 9.2.1 specifies the extraction: match the **whole trimmed line** against `Cache Key:`, take the **following** line, strip the log timestamp and ANSI escapes, trim, and reject an empty line or another heading. A runnable snippet is given. Section 9.4 repeats the rule where the recording obligation is stated. Section 2.8 records the trap next to the two-`Version:`-lines trap it mirrors, since a future editor who trips one is likely to trip the other.

### 17.5 The architect's position on the required observed leg

**Asked directly, answered directly: I agree, and I would have proposed it had I seen the defect.** The reasoning is mine rather than an endorsement of the tech lead's.

1. **It is the only artefact of the right type.** A key is a runtime output. Comparing two runtime outputs is the only evidence that settles their equality without a model of the environment that produced them. Every static argument about K-4, K-5 and K-7 is a model, and the model is exactly what round 1 got wrong about `ImageVersion` and round 3 got wrong about `process.env`.
2. **It kills the round-2 deadlock rather than reintroducing it.** Round 2 could be starved because it demanded *different* image draws. **K1-V is satisfied by same-image draws**, so no fleet composition and no scheduler outcome can withhold it. This is a strictly better termination argument than round 3's "a desk check always terminates", because round 3's terminated by looking at the wrong thing.
3. **It costs nothing and it is available before the merge.** The comparison uses runs the fix PR produces anyway, and both compared jobs are `save-if: 'false'` readers, so nothing is written.
4. **It converts into a standing check for free.** K1-V3 is the two `lookup-only` verify jobs, which already exist and already fail the workflow red. The gate therefore keeps holding as the runner fleet changes, which no static proof can do.

**One cost I am recording rather than hiding.** Requiring an observed leg introduces a new way for the gate to be unsatisfied: both compared jobs must reach their `Rust cache` step. That is why Section 9.2.1 defines **NOT YET ESTABLISHED** as a distinct state from FAIL, and says plainly that reaching it means a job failed early, which is an ordinary diagnosable failure rather than round 2's unclearable INCONCLUSIVE. I judge this the right trade and I am not asking for it to be softened.

### 17.6 Advisory corrections folded in, including one remedy I refined

1. **The cohort claim is corrected: the two `gate-release` members are in separate workflow runs.** `pr-regression-gates.yml` and `bundle-validation.yml` are separate workflow files, so one `pull_request` event starts a separate run of each. Round 3's Section 16.3 called them two jobs of one run and concluded `GITHUB_SHA` was shared by construction; that is withdrawn in Section 16.3 and in Section 9.2.1.
   **The refinement.** The dispatch's remedy, "require matching `GITHUB_SHA` instead", is right for the `gate-release` leg and **wrong for the `gate-debug` leg**, whose two runs come from the double trigger and therefore carry the head commit and the merge commit respectively: their `GITHUB_SHA` values differ **by construction**, so requiring equality would make that leg permanently NOT APPLICABLE. K-8 is a function of content, so the correct test there is **equality of the checked-out tree**, recorded as `gh api repos/mblua/AgentsCommander/commits/<sha> --jq .commit.tree.sha` for both. Section 9.2.1 states the `GITHUB_SHA` test for K1-V1 and the tree test for K1-V2. One further consequence is recorded there: K1-V1 must take `windows-release-cli-smoke` from the **`pull_request`-event** run, because `bundle-validation.yml` triggers on `pull_request` only.
2. **Section 10.7 no longer promises the pin lever is free.** The action revision is not itself hashed, but the key layout is a function of the implementation, so pinning back after `@v2` has already moved reverts the layout while the entries on `main` were written under the new one. In the exact scenario that motivates the lever, applying it costs one cold cycle. Accepted and rewritten.
3. **`CARGO_TERM_COLOR`: the stripping stays, the rationale is replaced.** Round 3's Section 16.8 concluded the variable "is not set by the repository and therefore cannot divide a cohort". That inference is wrong and is the same error as 17.1. It is corrected in Sections 9.4 and 16.8, and it now rests on stronger ground: the baseline log's cargo counters were unreadable until the escapes were stripped, which is direct evidence the variable was live at runtime although this repository never sets it.
4. **The `@v2` tag check is confirmed independently.** The architect resolved it rather than relaying it: `refs/tags/v2` is an **annotated tag object** `42dc69e1aa15d09112580998cf2ef0119e2e91ae` peeling to commit `e18b497796c12c097a38f9edb9d0641fb99eee32`, the reviewed revision. It is recorded in Section 9.2.1 because it is what makes the source analysis an analysis of the revision CI actually resolves today.
5. **The limit of a full-key match is stated where the invariant is stated.** A match proves the compared executions agreed; it does not prove every future runner reporting the same `ImageOS` supplies the same matched environment. Section 9.2.1 says so, and it is the reason K1-V3 is a standing check.

**The one provenance correction.** The dispatch says "every key recorded in Section 2.8 contains `Windows_NT-x64`". Section 2.8 as committed recorded the **environment-digest component only**, not the complete keys; the complete keys are in the raw logs, and one of them was supplied with the dispatch. The finding is unaffected, since the segment is in `config.ts:91-94` and the supplied key decomposes exactly. Section 2.8 now carries that one complete key, states that it is the only one carried forward, and points at Section 9.4, which has required the whole value on every job since round 3 so the gap does not recur.

### 17.7 What round 4 deliberately does not change

- **The implementation.** `93e2067` stands as reviewed. No workflow file, job, step, trigger, command, permission, concurrency group, `shared-key` or `workspaces` value moves.
- **The `ImageVersion` removal and Section 10.6's accepted residual risk.** Settled in round 2. Section 10.6 gains one operational clause only, that a `prefix-key` bump must be to a value such as `v1-rust` rather than a bare `v1`, which follows directly from finding 1's correction of the default.
- **Pre-land K1 blocking the merge.** Settled in round 3 and accepted by `dev-rust-grinch` in round 4. Round 4 changes what K1 requires, not when it runs or what it blocks.
- **Section 10.6's residual and Section 10.7's decision not to pin.** Only 10.7's cost claim changes.
- **Scope.** #1217, #1218 and #1219 stay out.
- **Groups A, C and D, and B1 through B6.** Only B0 changes, and only in what satisfies it.

### 17.8 Sections changed by this amendment

Header and status block; **2.8** (the complete-key evidence, the runtime-supplied hashed variables, the `Cache Key:` heading trap); **6.6** (the probe guarantee restated as an observation, named K1-V3); **8** steps 7 and 8 (K1's two required parts, the separate-runs correction, NOT YET ESTABLISHED); **9.2** (the K1 and M2 rows); **9.2.1** (rewritten in full: the key construction at the reviewed revision, the nine-component enumeration, the restated conditional invariant, K1-S as the configuration proof, K1-V as the required observed leg with three legs and the extraction procedure, K1-L as pure corroboration, the new verdict set, what K1 does not prove, when each part must be re-run); **9.4** (ANSI rationale, extraction rule, the four items recorded per job); **9.5** B0 (the observed leg required, the termination argument restated); **9.6** cases 1 and 2 (localisation by `.. Prefix:` and by digest, the restore-key hit as a localisation; **case 2's localisation is narrowed in round 5, Section 18.1: a non-exact restore proves a prefix match, not that only K-8 moved**); **10.6** (the `prefix-key` default in the lever); **10.7** (the pin lever is not free, and K-2 renumbered to K-9); **15.4** first bullet (round-2 record corrected where round 4 reversed it); **16.1** and **16.2** (round-3 record annotated where round 4 supersedes it: "sufficient path for B0" withdrawn, the K-1 through K-5 enumeration and the "whole `Cache Key:` line" instruction superseded); **16.3** consequence 1 (separate workflow runs); **16.8** (the `CARGO_TERM_COLOR` inference corrected); **17** (this section).

Sections **16.6** and **16.8** are left as round 3's own changelog and verification record, correctly scoped to round 3; 17.8 and 17.9 are round 4's.

### 17.9 Verification conditions for this amendment

Facts were taken from the tech lead's dispatch, which states the evidence is complete and instructs that no further `dev-rust` or `dev-rust-grinch` round be spent re-establishing it. **The architect nonetheless verified the source claims first-hand rather than relaying them**, because round 3 failed on a citation that was accurate and an inference from it that was not.

**Verified directly at `src/config.ts`, revision `e18b497796c12c097a38f9edb9d0641fb99eee32`:** the `prefix-key` default `"v0-rust"` at `:74`; the `shared-key` short-circuit at `:76-89`; the `os.type()` and `os.arch()` segments at `:91-94`; `self.keyPrefix = key` at `:96`, which is what `.. Prefix:` prints; the toolchain enumeration at `:106-115` and `:388-411`; the prefix list and the full `process.env` scan at `:118-131`; the `add-rust-environment-hash-key` gate on the environment digest at `:136-138` and on the lockfile digest at `:163-268`; `restoreKey` at `:140` against `cacheKey` at `:270`; and the two-line `Restore Key:` and `Cache Key:` emissions at `:333-336` with `.. Prefix:` at `:337-338`.

**Verified directly at `action.yml`, same revision:** the defaults for `prefix-key`, `add-job-id-key`, `add-rust-environment-hash-key`, `cmd-format`, `save-if` and `lookup-only`, and that `env-vars` has no default.

**Verified directly against GitHub:** that `refs/tags/v2` is an annotated tag object `42dc69e1aa15d09112580998cf2ef0119e2e91ae` peeling to `e18b497796c12c097a38f9edb9d0641fb99eee32`.

**Verified directly in this repository at branch HEAD:** all seven `env-vars: 'ImageOS'` and `shared-key` pairs with their files and lines, matching the cohort table; that `prefix-key`, `add-rust-environment-hash-key`, `cmd-format` and `add-job-id-key` appear nowhere in the three workflow files; that no `env:` block exists in them at any level; and that every job carrying a `Rust cache` step runs on `windows-latest`.

**Not re-derived, by instruction, and attributed rather than claimed:** the offline reimplementation reproducing the six-job partition, with `dev-rust`'s own UNVERIFIED label preserved on the absolute digest values (Section 2.8); the raw-log extraction of the six jobs' images and environment digests (tech lead); the complete key value quoted in Section 2.8 (tech lead, from those same logs); and the implementation verification of `93e2067` (`dev-rust-grinch`).

**Codebase Memory was not used for this amendment, under explicit authorization from the tech lead**, renewed for round 4. The query layer is defective: the full gate returns `ready` and every subsequent query fails with `Repository changed`. This branch also has no upstream, by the tech lead's decision, so the gate can fail with `status.git.base_sha must be a string`. **Direct reads were authorized and the 80-line fallback limit was waived.** The branch was not pushed to satisfy the gate.

## 18. Architect's amendment, Step 7 consensus round 5

**Verdict: READY_FOR_IMPLEMENTATION.** This section is authoritative wherever it contradicts anything earlier in this plan, Sections 14, 15, 16 and 17 included.

**Why this round exists, and what it does not disturb.** `dev-rust-grinch` verified the round-4 repairs and **passed both round-4 blocking findings**: the nine-entry enumeration covers the construction the action actually performs, K1-S is correctly necessary and explicitly not sufficient, and B0's required observed complete-key match removes the false static implication. It confirmed that a full-key match proves the compared executions computed the same key and that the plan now claims no more than that. **The round-4 acceptance gate stands as written.** The round-4 `Plan-SHA256` `A63CB8D1...` is superseded all the same, because a defect was found *outside* the gate, in the failure-diagnosis text, and the tech lead withdrew the ruling that had produced it.

**This is the narrowest amendment in this plan's history: two items, in Sections 6.4 and 9.6 case 2, neither touching the acceptance gate and neither touching the implementation.** `93e2067` stands as reviewed. K1-S, K1-V and its three legs, B0, the nine-component enumeration, the conditional invariant, the NOT YET ESTABLISHED verdict, the K1-V1 `GITHUB_SHA` and K1-V2 tree tests, and the extraction procedure are all unchanged.

### 18.1 Item 1, BLOCKING and accepted: a non-exact restore proves a prefix match, not that only K-8 moved

**The finding.** Round 4 read a restore-key hit as localising the difference to the lockfile digest, that is as proving K-1 through K-7 agreed and only K-8 moved. That is wrong. `config.ts:140` assigns the key through the environment digest to `restoreKey`, and `restore.ts:41` passes it to `restoreCache(paths, key, [config.restoreKey], ...)` as a **restore key**, whose lookup `@actions/cache` documents as **prefix matched**. A non-exact restore therefore proves only that the returned entry's key **starts with** the current `restoreKey`. It does not prove the remainder is exactly one same-layout lockfile-digest segment.

**The counterexample is the plan's own accepted scenario, not a hypothetical.** Section 10.7 accepts that `@v2` floats and records a pin-back lever whose cost round 4 itself corrected. Under a layout move followed by a pin-back, an entry written under the newer layout can prefix-match a restore key emitted by the reviewed layout. `cache-hit` is `false` and the difference includes **K-9**, yet round 4's Section 9.6 case 2 routed it down the lockfile-only path.

**Accepted without reservation. The tech lead withdrew their own round-4 ruling, and the architect verified the mechanism at the source rather than on citation** (Section 18.5). The correction is not an accommodation: the sources say what `dev-rust-grinch` says they say, and the toolkit contract is in fact slightly stronger than the finding needed.

**The repair, in Section 9.6 case 2.** Three changes and nothing else:

1. **The unconditional localisation is restated as a fact about the emitted strings:** the emitted prefix through the environment digest matched, and the remaining suffix differed. The component-level reading is no longer asserted from a non-exact restore alone.
2. **The actual returned cache key must be recorded and compared.** `restore.ts:48` already logs it as `Restored from cache key "<key>" full match: false.`, or `Found ...` under `lookup-only`, so this costs no new instrumentation. It is the only evidence in the log of what the writer emitted.
3. **The K-8 attribution is gated on the layout.** The remainder after the current `restoreKey` must be exactly one separator plus one 8-character hex segment and nothing further, which is what the reviewed layout appends (`config.ts:18`, `:266-270`, `:378-380`). With that shape, the difference is K-8 and the round-4 guidance applies unchanged. With any other shape, **K-9 is diagnosed first**.

   **Round 6 supersedes this item and Section 19.1 is the authoritative reading of it.** The shape test is retained, but the sentence above claims that the shape establishes the reviewed layout, and it does not: **shape proves grammar compatibility, not provenance.** A `@v2` revision can change the final segment's hash algorithm, hashed file set or meaning while preserving the grammar `<restoreKey>-<8 hex>`, so a shape-conforming remainder is consistent with K-9 having moved. Round 6 keeps the shape test as one leg of an optional upgrade, adds a required second leg of writer-and-reader action-resolution equality, and makes the conservative reading the default.

**What is deliberately not changed by this item.** B1 still requires `cache-hit == 'true'` and still refuses a restore-key hit, which was never in question. K1 is untouched: it compares complete emitted key values between two live cohort members and never reasons from a prefix.

**The methodological note, recorded because it is the same shape as Section 17.1's.** Round 4 took a match on a *prefix* and reported it as a match on the *components* that prefix happens to encode. Those are equal only when both sides share a layout, which is precisely the assumption Section 10.7 declines to guarantee. A proof may not quietly consume a condition the plan has recorded as open.

### 18.2 Item 2, advisory and fixed in the same pass: a superseded sentence in Section 6.4

**The finding.** Section 6 item 4 still read that with `env-vars: 'ImageOS'` "the hit type is determined by the repository and the toolchain alone." That is the pre-round-4 inference. K-4, K-5 and the runner-supplied half of K-7 can all still move the key. `dev-rust-grinch` judged it not a second blocker only because Sections 9.2.1 and 17 override it, so it cannot produce a false gate verdict.

**Corrected rather than left standing**, because two readings of the same fact in one document is a defect whether or not the gate currently absorbs it. Section 6.4 now states what the `ImageVersion` removal actually eliminates, which is the image-generation ambiguity and only that, and names K-4, K-5 and the runtime half of K-7 as still open to a desk read.

### 18.3 What round 5 deliberately does not change

- **The implementation.** `93e2067` stands as reviewed. No workflow file, job, step, trigger, command, permission, concurrency group, `shared-key` or `workspaces` value moves. `7d4ebe4`, `dbad95a` and this amendment are plan-only commits on top of it.
- **The acceptance gate, in full.** K1-S, K1-V1, K1-V2, K1-V3, K1-L, B0, the nine-component enumeration, the conditional invariant, the verdict set including NOT YET ESTABLISHED, and Section 9.4's extraction procedure. Round 5 changes diagnosis text only.
- **Groups A, B, C and D.** No pass criterion moves, B0 and B1 included.
- **Section 10.6's residual, Section 10.7's decision not to pin, and pre-land K1 blocking the merge.** All settled.
- **Scope.** #1217, #1218 and #1219 stay out.

### 18.4 Sections changed by this amendment

Header and status block; **6.4** (the superseded "repository and the toolchain alone" sentence corrected, and what the `ImageVersion` removal actually eliminates); **9.6 case 2** (rewritten: the prefix-match contract and its source, the unconditional localisation restated over emitted strings, the returned key required as evidence, the K-8 attribution gated on the reviewed layout, K-9 diagnosed first otherwise; **the layout gate is superseded in round 6, Section 19.1: shape proves grammar compatibility and not provenance, so the conservative reading becomes the default and sole K-8 attribution becomes an optional upgrade requiring action-resolution equality**); **17.8** (round-4 changelog annotated where round 5 narrows its 9.6 clause); **18** (this section).

Sections **16.6**, **16.8**, **17.8** and **17.9** remain their own rounds' changelogs and verification records, correctly scoped to those rounds; 18.4 and 18.5 are round 5's.

### 18.5 Verification conditions for this amendment

The tech lead's dispatch stated the evidence complete and instructed that no further `dev-rust` or `dev-rust-grinch` round be spent re-establishing it. **The architect nonetheless verified every source claim first-hand**, for the reason recorded in 17.9 and reinforced by this round: round 4 failed on an inference drawn from citations that were themselves accurate.

**Verified directly at `src/config.ts`, revision `e18b497796c12c097a38f9edb9d0641fb99eee32`:** `self.restoreKey = key` at `:140`, immediately after the environment digest is appended at `:136-138`; `self.cacheKey = key` at `:270`, after the lockfile digest is appended at `:266-267`; `HASH_LENGTH = 8` at `:18` and its use in `digest` at `:378-380`, which is what fixes the suffix at one separator plus eight hex characters; and `printInfo` emitting `Cache Key:` and its value at `:335-336` with `.. Prefix:` at `:337-338`.

**Verified directly at `src/restore.ts`, same revision:** `restoreCache(config.cachePaths.slice(), key, [config.restoreKey], { lookupOnly })` at `:41`, which passes `config.restoreKey` as the restore-keys argument and the complete `config.cacheKey` as the primary key; the comparison of the **returned** key against `key` at `:44-47`, which is what sets `cache-hit`; and the log line `` `${lookupOnly ? "Found" : "Restored from"} cache key "${restoreKey}" full match: ${match}.` `` at `:48`, which is what makes the returned key extractable with no new instrumentation.

**Verified directly at `actions/toolkit`, `packages/cache/src/cache.ts`:** the documented `restoreCache` contract, that the primary key is "an explicit key for restoring the cache. Lookup is done with prefix matching", that `restoreKeys` is "an optional ordered list of keys to use for restoring the cache if no cache hit occurred for primaryKey", and that the function "returns the key for the cache hit, otherwise returns undefined". **Stated precisely, because this round exists to stop an overclaim:** this is the published API contract read on `actions/toolkit` `main`, not a read of the exact `@actions/cache` build vendored into `swatinem/rust-cache` at `e18b497`. Prefix matching is the stable documented semantics of the argument `restore.ts:41` uses, which is all the finding needs, and the contract turns out **stronger than the finding required**: prefix matching applies to the primary key as well, so `cache-hit` is decided entirely by `restore.ts`'s own comparison of the returned key against `cacheKey`, never by the service having found an exact entry. **This disclosure is closed as of round 6 and is left standing only as the record of what round 5 could and could not verify.** `dev-rust-grinch` verified the contract at the exact vendored build: `@actions/cache` **6.0.0**, pinned in `package-lock.json` at `e18b497`, whose `lib/cache.d.ts:19-28` and whose exact `dist/restore/index.js` carry the primary-key prefix-match contract and return the service's `matchedKey` (Section 19.5).

**Verified in this repository:** branch `fix/1216-cache-key-imageversion` at `dbad95abfc1a6a2798975be55e31debb4d748112` with a clean worktree before editing, `main` untouched at `fd90894136c5096b087ed8b32eed53f76df83112`, and `93e20674a83eebcafb0a569470dc6a3315b6523b` as the implementation commit beneath the plan-only commits, of which this amendment is the third. `git diff --name-only 93e2067..HEAD` returns this plan file and nothing else.

**Not re-derived, by instruction, and attributed rather than claimed:** everything Section 17.9 lists under the same heading, plus `dev-rust-grinch`'s round-5 verification of the round-4 repairs and its confirmation that the implementation is untouched.

**Codebase Memory was not used for this amendment, under explicit authorization from the tech lead**, renewed for round 5. The query layer is defective as described in 17.9, and this branch still has no upstream by the tech lead's decision, so the gate can fail with `status.git.base_sha must be a string`. **Direct reads were authorized and the 80-line fallback limit was waived.** The branch was not pushed to satisfy the gate.

## 19. Architect's amendment, Step 7 consensus round 6

**Verdict: READY_FOR_IMPLEMENTATION.** This section is authoritative wherever it contradicts anything earlier in this plan, Sections 14 through 18 included.

**Why this round exists.** `dev-rust-grinch` reviewed the round-5 repairs and passed two of the three: restating the unconditional localisation over the emitted strings is correct, and requiring the actual returned cache key as evidence is correct. It failed the third, the layout gate, on the ground that the shape of a returned key proves that the key is compatible with the reviewed grammar and not that the reviewed layout produced it. The finding is correct and is accepted in full. **The round-5 `Plan-SHA256` `9756209306...` is superseded.**

**What this round does not disturb, verified rather than assumed.** `dev-rust-grinch` compared Sections 8, 9.2, 9.2.1, 9.4 and 9.5 across rounds 4 and 5 and found them equal, with Section 9.4 hashing to `876F0F22C816BD30950FA24F442CD717A23F10843A8AB611DBF3BD141B0BBBE6` in both revisions. **The acceptance gate has now stood unchanged through three review rounds.** Round 6 changes failure-diagnosis text and one advisory sentence, exactly as round 5 did, and touches the implementation not at all.

**Two round-5 open items closed by the reviewer rather than by this amendment**, recorded so they are not carried forward as live: the `localeCompare` observation against B1, which the tech lead and `dev-rust-grinch` both ruled to leave as it stands, and the provenance disclosure in Section 18.5, which `dev-rust-grinch` closed by reading the contract at the exact vendored build (19.5). Neither required a plan change.

### 19.1 Item 1, BLOCKING and accepted: shape proves grammar compatibility, not provenance

**The finding.** Round 5's Section 9.6 case 2 read a remainder of one separator plus eight hex characters as establishing that the returned entry was produced by the reviewed K-9 layout. It does not. The shape is a property of the grammar, and a future `@v2` revision can preserve the outer grammar `<restoreKey>-<8 hex>` while changing the final segment's hash algorithm, its hashed file set or its meaning. A layout change need not change the segment count or the segment width.

**The counterexample is again the plan's own accepted scenario, not a hypothetical.** Inside Section 10.7: `@v2` moves, the entries on `main` are written by the new revision, the pin-back lever restores the reviewed revision on the reader, the reader prefix-matches the new entry, `cache-hit` is `false`, the remainder passes the shape test, and condition (ii) has in fact moved while the diagnosis reports K-8.

**Accepted without reservation.** This is the third consecutive round in which Section 9.6 asserted a component-level conclusion the available evidence did not support, and the second in which the assertion was one increment weaker than the round before.

**The repair, and why it is a different kind of repair.** The tech lead's instruction was not to tighten the assertion again but to **invert the default**, and the architect agrees with that judgement for the reason given in 19.6. Section 9.6 case 2 now reads:

1. **The default, always available and never wrong.** On a non-exact restore, state that the emitted prefix through the environment digest matched, that **the final suffix string differs**, and that **K-8 and K-9 both remain open**, without asserting which one moved. The reader is then routed to compare the K-8 repository inputs and to check the action resolution, in that order, with the ordering explicitly labelled as cheapest-evidence-first rather than as a likelihood claim.
2. **The optional upgrade, when the evidence exists.** Sole attribution to K-8 requires two legs: **(a)** writer and reader shown to have resolved the **same exact `swatinem/rust-cache` commit**, from the action-resolution SHA that both jobs' setup logs already record, **and (b)** the suffix-shape check that round 5 introduced. **Without writer provenance the upgrade is unavailable and the default stands**, and the plan says so in those words.
3. **The sufficiency of the pair is argued, not asserted.** Under (a) the returned key is `<P_w>-<env_w>-<lock_w>` with both digests exactly eight characters; the prefix match makes `<P_r>-<env_r>` a prefix of it; (b) fixes the remainder at nine characters, so the two are equal-length prefixes of one string and are therefore identical, giving `P_w = P_r` and `env_w = env_r`. K-1 through K-7 agree and only K-8 differs. Neither leg is redundant: without (a) the shape says nothing about provenance, and without (b) a writer prefix of a different length can in principle still prefix-match and the equality argument does not close.

**Why this ends the line structurally rather than by another increment of precision.** The defect in rounds 4, 5 and now 6 was always the same: a conclusion about *components* drawn from evidence about *emitted strings*. Written the new way, the section's default asserts only what the emitted strings show, so it cannot overclaim; the component-level conclusion survives only inside a branch that first buys the missing evidence, and the price is named. A future finding against this text would have to show that the default itself claims too much, and the default claims nothing beyond a string comparison the reader has in hand.

**The methodological note, the third of its kind and the reason the first two were not enough.** Section 17.1 recorded that a static proof cannot discharge a runtime condition. Section 18.1 recorded that a proof may not quietly consume a condition the plan has recorded as open. Both were correct and both were applied as local repairs to the sentence that had failed, which left the next sentence free to make a weaker version of the same mistake. **The general remedy is to make the conservative statement the default and the strong statement the exception that carries its own admission price**, because a default cannot be overclaimed by omission.

### 19.2 Item 2, advisory and fixed in the same pass: Section 6.4 named part of the K-8 input set

**The finding.** Round 5's correction to Section 6.4 was directionally right but described the repository's contribution as its "lockfile and manifest content". At the reviewed revision K-8 also hashes `.cargo/config.toml`, `rust-toolchain` and `rust-toolchain.toml` under the `workspaces` roots.

**Corrected.** Section 6.4 now says "K-8 repository inputs" and names the files, and records the operational consequence: a branch that edits only `rust-toolchain.toml` moves the key, and a diagnosis that inspects the lockfile and the manifests alone will not find it. Section 9.6 case 2 names the same set in both of its diagnostic paths, so the two readings agree.

### 19.3 What round 6 deliberately does not change

- **The implementation.** `93e2067` stands as reviewed. No workflow file, job, step, trigger, command, permission, concurrency group, `shared-key` or `workspaces` value moves. `7d4ebe4`, `dbad95a`, `b24bf52` and this amendment are plan-only commits on top of it.
- **The acceptance gate, in full and for the second round running.** K1-S, K1-V1, K1-V2, K1-V3, K1-L, B0, the nine-component enumeration, the conditional invariant, the verdict set including NOT YET ESTABLISHED, the K1-V1 `GITHUB_SHA` and K1-V2 tree tests, and Section 9.4's extraction procedure.
- **Groups A, B, C and D.** No pass criterion moves. **B1 is not reopened for the `localeCompare` observation**, by the tech lead's ruling and `dev-rust-grinch`'s concurrence.
- **Section 10.6's residual, Section 10.7's decision not to pin, and pre-land K1 blocking the merge.** All settled. In particular, the upgrade's leg (a) reads the action-resolution SHA that already exists in the logs; it is **not** a pin and it does not reopen 10.7.
- **Scope.** #1217, #1218 and #1219 stay out.

### 19.4 Sections changed by this amendment

Header and status block; **6.4** (the K-8 input set named in full, with its operational consequence); **9.6 case 2** (the default inverted: the conservative reading made the normal path with its two-step routing and the `ImageVersion` caution moved into it, sole K-8 attribution demoted to a two-leg optional upgrade with its sufficiency argued, the unavailable-provenance terminal state stated, and the post-upgrade bullet re-anchored); **18.1** item 3 (round-5 record annotated where round 6 supersedes it); **18.4** (round-5 changelog annotated at the same clause); **18.5** (the provenance disclosure marked closed, with the reviewer's vendored-build citation); **19** (this section).

Sections **16.6**, **16.8**, **17.8**, **17.9**, **18.4** and **18.5** remain their own rounds' changelogs and verification records, annotated only where a later round supersedes a specific clause; 19.4 and 19.5 are round 6's.

### 19.5 Verification conditions for this amendment

The tech lead's dispatch instructed **one pass and no new evidence round**, and stated `dev-rust-grinch`'s citations and its verification at the exact vendored build to be complete. The architect verified what could be verified without opening a new round, and labels the rest as attributed.

**Verified first-hand at `src/config.ts`, revision `e18b497796c12c097a38f9edb9d0641fb99eee32`:** `const HASH_LENGTH = 8;` at `:18`, which is what fixes the digest segments at eight characters and therefore what the shape test rests on; and the file set the lockfile hasher covers, which includes `.cargo/config.toml`, `rust-toolchain` and `rust-toolchain.toml` in a `globFiles` call and again in the per-root glob, plus `Cargo.toml` per workspace member and `Cargo.lock` per workspace root. **Provenance disclosure, in the same spirit as round 5's:** the retrieval path available for this pass returns the file's content reliably but attributes line numbers unreliably, contradicting itself across two requests on the same file. **The file set is therefore recorded as verified first-hand and the fine-grained coordinates `:164` and `:171-175` are attributed to `dev-rust-grinch`, not claimed.** No existing line citation in this plan was altered on the strength of that retrieval, and the citations round 5 verified directly stand as they were.

**Attributed to `dev-rust-grinch`, by instruction and not re-derived:** the byte-identity of the acceptance gate across rounds 4 and 5, including the Section 9.4 hash `876F0F22C816BD30950FA24F442CD717A23F10843A8AB611DBF3BD141B0BBBE6`; the contract read at the exact vendored build, `@actions/cache` **6.0.0** pinned in `package-lock.json` at `e18b497`, with `lib/cache.d.ts:19-28` and the exact `dist/restore/index.js` carrying the primary-key prefix-match contract and returning the service's `matchedKey`; the presence of the action-resolution SHA in job setup logs, which is what makes the upgrade's leg (a) free rather than new instrumentation; and everything Sections 17.9 and 18.5 list under the same heading.

**Verified in this repository:** branch `fix/1216-cache-key-imageversion` at `b24bf5233896fbaa845bdfb46b37804e64e0828e` with a clean worktree before editing, `main` untouched at `fd90894136c5096b087ed8b32eed53f76df83112`, and `93e20674a83eebcafb0a569470dc6a3315b6523b` as the implementation commit beneath the plan-only commits, of which this amendment is the fourth. `git diff --name-only 93e2067..HEAD` returns this plan file and nothing else. The round-5 `Plan-SHA256` control reproduces: the `b24bf52` blob of this file hashes to `9756209306B116E7804F40F78BE5E798FB08B471CB7B627807E5694AC90DC815`.

**Codebase Memory was not used for this amendment, under explicit authorization from the tech lead**, renewed for round 6. The query layer is defective as described in 17.9, and this branch still has no upstream by the tech lead's decision, so the gate can fail with `status.git.base_sha must be a string`. **Direct reads were authorized and the 80-line fallback limit was waived.** The branch was not pushed to satisfy the gate.

### 19.6 The architect's position on closing this line of findings

The tech lead stated an intention to record any further Section 9.6 finding as a known limitation and land, rather than run a seventh round, and invited disagreement. **The architect agrees, and the agreement is reasoned rather than deferential.**

**The load-bearing distinction is between text that can produce a false certification and text that can only misroute a later diagnosis.** The certification is Groups A through D and K1. Every one of those criteria is decided by comparing observed values: complete emitted key values inside a cohort for K1, `cache-hit == 'true'` for B1, entry existence and size for B0 and the verify probes, counters and durations for B2 and C. **None of them consults Section 9.6.** Section 9.6 is read only after Group B has already failed, that is only when the change has already been refused certification. A defect there can send a human to the wrong file; it cannot pass a change that should have been blocked.

**Two conditions make that judgement safe rather than convenient, and both hold.** First, the acceptance gate has been unchanged across three review rounds and its byte-identity was checked mechanically, so the boundary between the gate and the diagnosis text is not an assumption. Second, the failure mode that any residual 9.6 defect needs is a floating-tag layout move that has not been observed once, and Section 10.7 already accepts that exposure with two named levers and a daily self-heal.

**The architect's own condition on landing with a known limitation.** If a future finding shows that a residual defect reaches any Group A through D criterion or K1, it is not a known limitation and must be repaired before landing, whatever round it arrives in. That is the line the architect would defend, and round 6 does not approach it.
