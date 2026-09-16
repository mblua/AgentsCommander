# Plan #2075: `--locked` for every unlocked cargo invocation in `.github/workflows/`

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2075 (OPEN): "ci(security): cargo build --release in pr-regression-gates without --locked (Sonar S8549)". Sonar issue `AaCI_U_rCXUCLYag_fqs`.
- Repo: `repo-AgentsCommander`; branch `fix/2075-cargo-build-locked`.
- Code base frozen at authoring (2026-09-16 UTC): remote `main` = `5203c4e3b0b5909fdc12337194c886b1514966c3`. Plan-only revisions on the branch: `2bd02887` (initial), `d6fe03f8` (inventory completed), this revision (user widened scope from 1 to 13 lines). Line numbers refer to the base SHA; re-anchor on quoted text if they drift.
- Class: Lite (band 1-25). Owner `ac-dev-rust-v4`; reviewer Grinch; coordinator `ac-tech-lead-v4`.
- 2 modified files, 13 content lines, plus this plan: `.github/workflows/pr-regression-gates.yml` (10 lines), `.github/workflows/cache-warm.yml` (3 lines). No Rust, frontend, dependency, `Cargo.lock`, `Cargo.toml`, rust-cache-config or release-workflow change.
- Root `.gitignore:11` ignores `/plans/`; commit this plan with `git add -f plans/2075-cargo-build-locked.md`.

## 1. Objective

Every explicit `cargo` invocation in `.github/workflows/` either consumes the tracked `Cargo.lock` (`--locked`) or cannot re-resolve dependencies by design. This closes Sonar S8549 at `pr-regression-gates.yml:1515` and the 12 sibling instances the review surfaced.

## 2. Cause and inventory

The original finding is the step at `.github/workflows/pr-regression-gates.yml:1513-1515` (job `rust-linux-release-parity`, `runs-on: ubuntu-22.04`):

```yaml
      - name: cargo build --release (links the binary release.yml ships)
        working-directory: src-tauri
        run: cargo build --release --bins
```

Without `--locked`, cargo may re-resolve versions from the manifests instead of failing on a stale lockfile. `Cargo.lock` is tracked at the workspace root; there is no `src-tauri/Cargo.lock`.

Complete inventory of every explicit `cargo` invocation in `.github/workflows/`, re-grepped at `d6fe03f8` (only `pr-regression-gates.yml` and `cache-warm.yml` contain the word, and no line ends in a backslash continuation, so a line scan cannot miss a command):

- Unlocked before this plan (13), all edited by §4:
  - `pr-regression-gates.yml` — `:88`, `:92`, `:96` (job `rust-regression`, windows-latest); `:684`, `:688`, `:733`, `:796` (job `rust-regression-linux`, ubuntu-latest); `:1515` (job `rust-linux-release-parity`, ubuntu-22.04); `:1648`, `:1652` (job `rust-regression-macos`, macos-latest).
  - `cache-warm.yml` — `:60`, `:65`, `:70` (job `warm-debug`, windows-latest).
- Already locked (18, untouched): `pr-regression-gates.yml:108,134,333,354,700,835,1030,1051,1336,1664,1687,1882,1903,2184,2240,2243` plus the PowerShell matrix invocations `:2355`/`:2374`, whose argument arrays at `:2351`/`:2370` begin `test --locked`.
- Lock flag not applicable (4, untouched): `:329`, `:1026`, `:1878` (`cargo --version` probes) and `:2213` (`cargo fmt --all -- --check`; formats only).
- No `cargo build` appears anywhere else in `.github/workflows/`; the other `cargo build` matches are comments (`:1358`, `:1545`).

## 3. Scope

In scope: the 13 lines in §4 (2 files) and this plan.

Out of scope: `Cargo.lock`, `Cargo.toml`, any manifest or dependency; the 18 locked and 4 not-applicable lines; rust-cache configuration (`workspaces`, `shared-key`/`key`, `env-vars`, `save-if`, `lookup-only`); job or workflow structure; `npm run build:prod:no-bundle`, `npm run smoke:cli-release-windows` and every npm/Tauri wrapper (no explicit `cargo` line, not among the 13); `release.yml`; other workflows.

## 4. Exact edits (before -> after)

Flag placement follows the existing locked steps (`cargo test --locked --lib ...`): `--locked` immediately after the subcommand. Every other byte of each line — including `"$TEST"` / `"$FILTER"` quoting and the 8- or 10-space indentation — stays identical.

`pr-regression-gates.yml` (8-space indent except `:733`/`:796`, which stay at 10):

- `:88` `run: cargo check --all-targets` -> `run: cargo check --locked --all-targets`
- `:92` `run: cargo clippy --workspace --all-targets -- -D warnings` -> `run: cargo clippy --locked --workspace --all-targets -- -D warnings`
- `:96` `run: cargo test --lib --bins --tests` -> `run: cargo test --locked --lib --bins --tests`
- `:684` `run: cargo check --all-targets` -> `run: cargo check --locked --all-targets`
- `:688` `run: cargo clippy --workspace --all-targets -- -D warnings` -> `run: cargo clippy --locked --workspace --all-targets -- -D warnings`
- `:733` `cargo test --lib "$TEST" -- --exact --test-threads=1 --nocapture 2>&1 | tee test-1577.log` -> `cargo test --locked --lib "$TEST" -- --exact --test-threads=1 --nocapture 2>&1 | tee test-1577.log`
- `:796` `cargo test --lib "$FILTER" -- --test-threads=1 --nocapture 2>&1 | tee test-1842.log` -> `cargo test --locked --lib "$FILTER" -- --test-threads=1 --nocapture 2>&1 | tee test-1842.log`
- `:1515` `run: cargo build --release --bins` -> `run: cargo build --locked --release --bins`
- `:1648` `run: cargo check --all-targets` -> `run: cargo check --locked --all-targets`
- `:1652` `run: cargo clippy --all-targets -- -D warnings` -> `run: cargo clippy --locked --all-targets -- -D warnings`

`cache-warm.yml` (8-space indent):

- `:60` `run: cargo check --all-targets` -> `run: cargo check --locked --all-targets`
- `:65` `run: cargo clippy --workspace --all-targets -- -D warnings` -> `run: cargo clippy --locked --workspace --all-targets -- -D warnings`
- `:70` `run: cargo test --lib --bins --tests --no-run` -> `run: cargo test --locked --lib --bins --tests --no-run`

Simulated at this revision: applying exactly these replacements to the base files changes 10 + 3 lines and nothing else, and both edited files reparse as YAML (PyYAML 6.0.3).

## 5. Flag and lockfile evidence

- Flag acceptance (local `cargo 1.97.1`, `clippy 0.1.97`; flag directly after the subcommand): `cargo check --locked --help`, `cargo test --locked --help`, `cargo build --locked --help` and `cargo clippy --locked --help` all exit 0. The clippy probe matters: `clippy` is an external subcommand and must accept the flag itself.
- Lockfile current, commands run from `src-tauri`:
  - `cargo metadata --locked --format-version 1` -> exit 0, 1.57 s, empty stderr.
  - `cargo check --locked --all-targets` -> exit 0, 1 m 16 s.
  - `cargo test --locked --lib --bins --tests --no-run` -> exit 0, 3 m 4 s.
  - `cargo build --release --bins --locked` -> exit 0, 4 m 33 s (the exact post-fix command of `:1515`).
  - `git status --porcelain -- Cargo.lock` empty after all of them.

## 6. Cache interaction (required finding)

- `gate-debug` producer: `cache-warm.yml:49-55` (`warm-debug`, windows-latest) saves after the guarded steps `:60`/`:65`/`:70` (`if: steps.cache.outputs.cache-hit != 'true'`). Consumers: `rust-regression` (`pr-regression-gates.yml:79-84`, identical `workspaces`/`shared-key`/`env-vars`, `save-if: 'false'`) running `:88`/`:92`/`:96`, and the `issue-1850-windows-profile` debug matrix leg (`:2342` `gate-${{ matrix.mode }}`, cargo lines `:2355`/`:2374` already `--locked`). Verifier: job `verify-debug-cache` at `cache-warm.yml:110`, probe step `:122` (lookup-only).
- After this plan, producer and consumer stay command-for-command consistent: `:60` == `:88` and `:65` == `:92` exactly; `:70` keeps its existing `--no-run` difference from `:96` (compile-only vs run) and now carries the same flag.
- Keys cannot change: swatinem/rust-cache v2 derives its key from the toolchain, lockfile/manifest hashes, the listed `env-vars` and the `shared-key`; a workflow command string is not an input. No manifest, lockfile, toolchain, env-var or cache-config line is touched, so `gate-debug` hits are preserved.
- Compile units cannot diverge: `--locked` is a resolution constraint; with a current lockfile the resolved graph, features, profiles and unit fingerprints are identical to today's unlocked resolution, so producer artifacts remain valid for consumers.
- `gate-release` is untouched: producer `warm-release` (`cache-warm.yml:98-104`) and consumers `windows-release-cli-smoke` (`pr-regression-gates.yml:2274`/`:2279`), the `issue-1850-windows-profile` release leg and `bundle-validation.yml:53` compile through `npm run build:prod:no-bundle` (no explicit `cargo` line) or already-locked cargo; this plan changes no line they read.
- Failure mode if the lock were ever stale: `warm-debug` fails at `:60` instead of silently re-resolving, `verify-debug-cache` fails, and the PR jobs fail identically. That is the intended S8549 behavior.

## 7. Verification per changed job

- `rust-regression` (windows-latest), `pr-regression-gates.yml:88/92/96`: runs on every push to this branch and on PRs, unconditionally. Check the push run's job log: three steps green, each showing the `--locked` command and no "the lock file ... needs to be updated".
- `rust-regression-linux` (ubuntu-latest), `:684/688/733/796`: same push run; `:733`/`:796` must still carry intact `"$TEST"`/`"$FILTER"` and their grep guards must still pass.
- `rust-linux-release-parity` (ubuntu-22.04), `:1515`: same push run; step "cargo build --release (links the binary release.yml ships)" green.
- `rust-regression-macos` (macos-latest), `:1648/1652`: same push run.
- `cache-warm.yml` triggers only on `push` to `main`, nightly cron or `workflow_dispatch`. Dispatch it against the branch: `gh workflow run cache-warm.yml --ref fix/2075-cargo-build-locked`. Because `warm-debug`'s three steps are guarded by cache-hit != true, a warm `gate-debug` entry makes them skip; the dispatch then verifies the key match through `verify-debug-cache` rather than executing the changed lines, and the identical commands already run in `rust-regression` on every push. The cold path executes on the next cache-key change or eviction — the first post-merge `main` push or nightly. If a pre-merge cold-path execution is required, it needs a cache-key change, which this plan must not make; that is a coordinator decision.
- Sonar `AaCI_U_rCXUCLYag_fqs` closes after merge to `main` and the next Sonar analysis (not part of these workflows).

## 8. Acceptance

1. All 13 lines match §4 exactly; no other line in either file changes; `Cargo.lock` has no diff.
2. `rust-linux-release-parity`, `rust-regression`, `rust-regression-linux` and `rust-regression-macos` are green on the implementation push run, with the changed steps passing and no lockfile error.
3. `cache-warm` dispatch green: `warm-debug` completes and `verify-debug-cache` finds the exact `gate-debug` entry (key unchanged), `verify-release-cache` also green.
4. Sonar issue closes after main + analysis. 5. Grinch review passes.

## 9. Implementation order

1. Apply §4 (13 lines, 2 files); run the §5 checks locally.
2. `git add .github/workflows/pr-regression-gates.yml .github/workflows/cache-warm.yml && git add -f plans/2075-cargo-build-locked.md`
3. Commit `ci(2075): use --locked for every cargo invocation in workflows`; push to `fix/2075-cargo-build-locked`.
4. Watch the push run; dispatch `cache-warm` on the branch if the coordinator wants the key check pre-merge.
5. Report run URLs, the dispatch result and raw step outputs to the coordinator. No push to `main`, no merge.

## 10. Risks

- Stale lockfile -> hard failure instead of silent re-resolution. Mitigated: §5 all green.
- `clippy` is an external subcommand and could reject `--locked`: probed locally (exit 0); the push run exercises it on windows (job `rust-regression`) and ubuntu (`rust-regression-linux`).
- Local evidence is Windows; the linux/macos legs are flag-only and are exercised by the push run.
- `cache-warm`'s changed lines may stay dormant pre-merge on a warm cache (§7); the equivalent consumer commands are exercised by `rust-regression`.

## Plan Contract

Scope: exactly the 13 content lines in §4 across `.github/workflows/pr-regression-gates.yml` and `.github/workflows/cache-warm.yml`, plus this plan file. No `Cargo.lock`, manifest, cache-config, job or workflow-structure change; no other cargo line touched. The implementer runs the §5 checks, preserves the §6 cache finding and reports raw outputs. Any need for a 14th line, a cache-key change or a lockfile update stops the work and returns to the coordinator. Acceptance is §8.
