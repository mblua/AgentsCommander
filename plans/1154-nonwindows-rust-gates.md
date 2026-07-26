# Plan #1154: Add Linux and macOS Rust regression gates to PR CI

Author: architect, wg-14. Authored and certified in a single Lite pass on 2026-07-26 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1154](https://github.com/mblua/AgentsCommander/issues/1154), `Add Linux and macOS Rust regression gates to PR CI`.

This is a Lite CI coverage change. It appends two jobs to one existing GitHub Actions workflow file. It introduces no new production abstraction, dependency, crate, schema, API, IPC surface, event, protocol, configuration, or migration, and it changes no Rust, TypeScript, or shell source file.

## 1. Frozen authority and fail-closed entry gate

The implementation working tree is `repo-AgentsCommander`, branch `ci/1154-nonwindows-rust-gates`, targeting `main`.

After `git fetch origin main` on 2026-07-26T18:44Z, all of the following resolved exactly to `1e7f2350b481918c1e63abdf86149630d924ef2f`:

- committed `HEAD`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

The branch has no upstream yet, so no upstream comparison applies until the first push. The index and the non-ignored working tree were clean, verified by an empty `git status --porcelain=v1 --untracked-files=all`.

Root `.gitignore` line 11 ignores `plans/`, so the implementation must force-add this exact plan file with `git add -f`. Do not remove or weaken the repository's `plans/` ignore rule.

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. Do not rebase, merge a moved base, or silently substitute a newer commit under this certification.

Branch-name validation was checked against `scripts/validate-branch-name.mjs` line 15. `ci/1154-nonwindows-rust-gates` matches the allowed pattern with type `ci`, number `1154`, and slug `nonwindows-rust-gates`, so the only required status check on `main`, `validate-branch-name`, will pass on this branch name.

## 2. Objective and non-goals

Objective: give every pull request a non-Windows Rust compile and lint signal by adding two new jobs, `rust-regression-linux` on `ubuntu-latest` and `rust-regression-macos` on `macos-latest`, to `.github/workflows/pr-regression-gates.yml`, each running `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` against `src-tauri`.

Non-goals, restated from the issue and binding on the implementer:

- Do not run `cargo test` on Linux or macOS.
- Do not promote either new job to a required status check. That is repository configuration, not repository code.
- Do not change anything under `src-tauri/` or `crates/`, including the `dead_code` and clippy defects that the new Linux job is expected to report. Those belong to #1113 and its children #1131, #1132, #1133, #1134, #1135, and #1136.
- Do not modify the existing `rust-regression`, `windows-release-cli-smoke`, `test-debt`, or `frontend-regression` jobs, nor the workflow-level `name`, `on`, `permissions`, or `concurrency` blocks. They must remain byte-unchanged.
- Do not modify `release.yml`, `lockfile-check.yml`, `validate-branch-name.yml`, or `version-sync-check.yml`.
- Do not weaken the new gates to make them green. Do not add `continue-on-error`, do not drop `-D warnings`, do not add `|| true`, and do not add allow attributes anywhere.
- Do not act on the two out-of-scope findings recorded in section 11 as part of this pull request.

## 3. Verified current state

All of the following were read at the frozen base.

1. `.github/workflows/pr-regression-gates.yml` has 179 content lines and defines exactly four jobs, in this order: `test-debt` (`ubuntu-latest`, lines 22 to 40), `rust-regression` (`windows-latest`, lines 42 to 85), `windows-release-cli-smoke` (`windows-latest`, lines 87 to 129), and `frontend-regression` (`ubuntu-latest`, lines 131 to 179). Only `rust-regression` compiles Rust, and it runs on Windows only.
2. Every one of the four jobs carries the identical two-line push guard: the comment `# Skip branch-deletion push events while still requiring pull request validation.` followed by `if: github.event_name != 'push' || (github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000')`.
3. Action versions used in the file are `actions/checkout@v5`, `actions/setup-node@v5`, `dtolnay/rust-toolchain@stable`, `swatinem/rust-cache@v2`, and `actions/upload-artifact@v4`. Node is pinned to `node-version: 22` with `cache: 'npm'`, and every Node job runs `npm install -g npm@11.6.2`.
4. The repository root holds a Cargo workspace. Root `Cargo.toml` is exactly `[workspace]` with `members = ["src-tauri", "crates/session-bridge"]`, `default-members` identical, and `resolver = "2"`. `Cargo.lock` is tracked at the repository root. There is no `src-tauri/Cargo.lock` and no `src-tauri/target`. The real Cargo target directory is the workspace-level `target/`, which is what `release.yml` copies binaries from.
5. Because the existing `rust-regression` job sets `working-directory: src-tauri`, cargo selects only the `agentscommander-new` package. `crates/session-bridge` is not compiled by any existing gate on any platform. The new jobs reproduce that scope exactly, as the issue requires.
6. `src-tauri/build.rs` computes `profile` from `BUILD_PROFILE`, falling back to `dev` whenever the cargo `PROFILE` is not `release`. `cargo check` uses the debug profile, so `profile` resolves to `dev`. In `configure_embedded_dist`, a present `../dist/index.html` emits `cargo:rustc-cfg=has_embedded_dist`; an absent one under the `dev` profile only emits a warning and then calls `omit_missing_dist_resource_for_dev_check`, which sets `TAURI_CONFIG` to `{"bundle":{"resources":[]}}`. Under any non-`dev` profile the absent file panics.
7. Therefore `cargo check --all-targets` does compile without a built frontend, but it compiles a different program. Without `../dist/index.html` the `has_embedded_dist` cfg is unset, which removes about twenty gated items in `src-tauri/src/web/embedded.rs` and at `src-tauri/src/web/mod.rs` lines 71 and 373 from the lib and bin builds, and instead compiles the `#[cfg(not(has_embedded_dist))]` branch at `src-tauri/src/web/mod.rs` line 84, a branch the Windows gate never compiles.
8. `package.json` defines `"build": "vite build"`. `vite.config.ts` uses only `vite`, `vite-plugin-solid`, and a JSON import of `src-tauri/tauri.conf.json`, with no platform-specific code, no shell invocation, and no Windows-only tooling. `npm ci` is already proven on `ubuntu-latest` by the existing `frontend-regression` job and on `windows-latest` by `rust-regression`.
9. `src-tauri/Cargo.toml` declares `tauri = { version = "2", features = ["image-png"] }`. The `tray-icon` feature is not enabled. Linux-only dependency needs come from `wry` and its `webkit2gtk` / `javascriptcore` / `soup3` `-sys` crates, whose build scripts resolve system libraries through `pkg-config`. `cargo check` runs dependency build scripts, so those system packages are mandatory on Linux even for a check-only run.
10. Platform-conditional compilation in `src-tauri` is `target_os` and `unix` based only. `grep` over `src-tauri/src` and `src-tauri/tests` finds zero occurrences of `target_arch`. Every `target_pointer_width` occurrence is in `src-tauri/src/commands/wg_delete_diagnostic.rs` and is gated as `all(windows, target_pointer_width = ...)`. macOS-conditional code is `src-tauri/src/commands/wg_delete_diagnostic.rs` line 274, and `src-tauri/src/path_identity.rs` lines 396 and 425, all keyed on `target_os` only.
11. `release.yml` run `30024311241` for `v0.20.0` on 2026-07-23T16:15Z finished `success` with all four matrix legs `success`, including `release (ubuntu-22.04, --config src-tauri/tauri.prod.conf.json)`. That leg installed exactly `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf` and then performed a full `tauri build`, which is a strict superset of `cargo check`.
12. `linux-setup/install-ubuntu-deps.sh` documents the local Ubuntu developer set as `build-essential pkg-config libssl-dev libwayland-dev libglib2.0-dev libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev`. It names `libayatana-appindicator3-dev` where `release.yml` names `libappindicator3-dev`. On GitHub-hosted Ubuntu images `build-essential`, `pkg-config`, and `libssl-dev` are preinstalled, and `libgtk-3-dev`, `libglib2.0-dev`, and `libwayland-dev` arrive transitively with `libwebkit2gtk-4.1-dev`.
13. Runner label mapping confirmed against `actions/runner-images`: `ubuntu-latest` is Ubuntu 24.04 x64, `macos-latest` is macOS 26 arm64, and `windows-latest` is Windows Server 2025. `macos-latest` is therefore Apple Silicon and its native host triple is `aarch64-apple-darwin`.
14. Package availability confirmed for Ubuntu 24.04 noble: `libwebkit2gtk-4.1-dev` version `2.52.3-0ubuntu0.24.04.1`, and `libappindicator3-dev` version `12.10.1+20.10.20200706.1-0ubuntu5` in `universe`, which is enabled by default on GitHub-hosted Ubuntu images.
15. `swatinem/rust-cache@v2` `action.yml` defines the inputs `prefix-key` (default `v0-rust`), `shared-key`, `key`, `add-job-id-key` (default `true`), `add-rust-environment-hash-key` (default `true`), `env-vars`, `workspaces`, `cache-directories`, `cache-targets`, `cache-on-failure`, `cache-all-crates`, `cache-workspace-crates`, `save-if`, `cache-provider`, `cache-bin`, `lookup-only`, and `cmd-format`. `workspaces` is a `$workspace -> $target` path mapping, not a cache key.
16. Branch protection on `main` requires only `validate-branch-name`, `enforce_admins` is false, and one approving review is required. Neither new job can block a merge. The repository is public, so GitHub-hosted macOS minutes are not billed.
17. `git ls-files --eol .github/workflows/pr-regression-gates.yml` reports `i/lf w/crlf attr/`. The committed blob uses LF; the Windows working-tree copy uses CRLF on all 179 lines; `.gitattributes` covers only `.husky/**`, `*.sh`, `*.toml`, `*.json`, and `*.rs`, so `*.yml` has no repository-level normalization rule and the LF blob depends on the local `core.autocrlf` setting.

## 4. Delegated decisions, resolved

These six decisions were delegated to this plan. Each is now closed. No TBD remains.

### 4.1 Frontend build steps in the new jobs: required, include them

Both new jobs run `Setup Node.js`, `Pin npm version`, `Install frontend dependencies` (`npm ci`), and `Build frontend assets for Tauri config validation` (`npm run build`), exactly as `rust-regression` does.

`cargo check` would technically succeed without them, per facts 6 and 8. Including them is nonetheless mandatory here, because per fact 7 skipping the build silently changes which program is compiled. Without `../dist/index.html` the new gates would drop roughly twenty `has_embedded_dist` items from the lib and bin builds and would instead compile the `not(has_embedded_dist)` branch that the Windows gate never compiles. Two consequences follow, and both defeat the purpose of #1154:

- Under-coverage: real non-Windows defects inside the `has_embedded_dist` code would never be seen by the new gates.
- Contamination: `cargo clippy -D warnings` on the reduced build would very likely report `dead_code` on `embedded::*` helpers that are unused only because the dist was missing. Those findings would be indistinguishable from the genuine non-Windows `dead_code` inventory that #1113 depends on this gate to produce.

Building the frontend keeps the cfg set identical across all three platforms, so any diagnostic difference between them is attributable to the platform and to nothing else. That is the entire value of this issue.

### 4.2 Tauri system dependencies on Linux: required, exact invocation fixed

They are genuinely required. Per fact 9, `cargo check` executes dependency build scripts, and the `webkit2gtk-sys`, `javascriptcore-rs-sys`, and `soup3-sys` build scripts resolve their libraries through `pkg-config`. Without the development packages the job fails inside a dependency build script before any first-party code is analyzed.

The exact invocation is the one already proven by `release.yml` and mandated by acceptance criterion 4:

```yaml
      - name: Install Tauri system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

Sufficiency is proven by fact 11: the same four packages supported a full green `tauri build` on Ubuntu, which strictly exceeds `cargo check`. Availability on `ubuntu-latest`, which is Ubuntu 24.04, is proven by facts 13 and 14. `patchelf` is only needed for AppImage bundling and is redundant for a check-only job, but acceptance criterion 4 names it, so it stays; it is a small, harmless install.

Pre-authorized contingency, to be used only if the apt step fails specifically because `libappindicator3-dev` has no installation candidate on a future `ubuntu-latest` image. In that single case, and in no other, replace only that one package name:

```yaml
          sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf
```

`libayatana-appindicator3-dev` is the successor package, is already the one the repository's own `linux-setup/install-ubuntu-deps.sh` installs, and is what `linux-setup/verify-ubuntu-deps.sh` probes as `ayatana-appindicator3-0.1`. If this contingency is used, the implementer must report it, because it departs from the literal wording of acceptance criterion 4 and the tech lead has to record the substitution on the issue. Any other apt failure is not covered by this contingency and must be reported instead of improvised around.

### 4.3 Intel macOS target: do not add it, macOS stays native arm64 only

Do not add `cargo check --target x86_64-apple-darwin`, and do not add a `targets:` input to the macOS `dtolnay/rust-toolchain@stable` step.

Justification is fact 10. `src-tauri` contains no `target_arch` cfg at all, and every `target_pointer_width` cfg is additionally gated on `windows`. For the two Apple targets the entire conditional-compilation surface is identical: both are `unix`, both are `not(windows)`, both are `target_os = "macos"`, and both are `target_pointer_width = "64"`. A second `cargo check` against `x86_64-apple-darwin` would recompile the full dependency graph for a second triple, roughly doubling the macOS job wall clock and its cache footprint, and would be mathematically incapable of selecting a single line of code the native run did not already select. The issue lists this coverage as optional. It is declined on evidence.

If arch-conditional code is ever introduced under `src-tauri`, adding the Intel check becomes worthwhile and should be a new issue at that time.

### 4.4 Rust cache configuration: mirror `workspaces`, isolate with an explicit `key`

The premise in the dispatch needs one correction, per fact 15: `workspaces` is a path mapping of the form `$workspace -> $target`, not a cache key. Cache identity in `swatinem/rust-cache@v2` comes from `prefix-key`, then either `shared-key` or the automatic `GITHUB_JOB` component plus the optional `key` input, then a hash of the Rust environment including the host triple.

Configuration for both new jobs:

```yaml
      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: src-tauri -> target
          key: rust-regression-linux
```

and the same with `key: rust-regression-macos`.

Collision with the existing Windows cache is prevented three independent ways, which satisfies acceptance criterion 5 with margin:

1. The automatic job component differs: `rust-regression`, `rust-regression-linux`, and `rust-regression-macos` are distinct job ids.
2. The Rust environment hash includes the host triple, so `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, and `aarch64-apple-darwin` cannot share an entry.
3. The explicit `key` input makes the isolation readable directly in the YAML and independent of the job-id default, which is what an auditor of criterion 5 will look for.

`workspaces` is kept byte-identical to the Windows job on purpose. Per fact 4, `src-tauri -> target` does not describe this repository's real layout, so the target-directory half of the cache is inert and only `~/.cargo` is effectively cached. That is a real pre-existing defect, but correcting it here would be an unrequested change with repository-wide blast radius: caching the true workspace `target/` for two additional platforms would add multiple gigabytes against the repository's 10 GB Actions cache budget and could evict the npm caches that the other four jobs depend on. The registry and `~/.cargo` portion, which is the dominant cold-start cost, is cached either way. The defect is recorded in section 11 for a separate issue.

### 4.5 Runner label: `ubuntu-latest`

Use `ubuntu-latest`, not `ubuntu-22.04`.

It is mandated by acceptance criterion 1, it matches the two existing Ubuntu jobs in the same file, and it is the correct choice on the merits. `release.yml` pins `ubuntu-22.04` for a distribution reason: the shipped Linux binary inherits the build machine's glibc floor, so an older image widens the set of systems that can run the release artifact. A pull request gate ships nothing, so that constraint does not apply to it; the gate should instead track the newest supported image so that non-Windows breakage against current toolchains and current system libraries is caught at the pull request. Per facts 13 and 14, `ubuntu-latest` is Ubuntu 24.04 today and carries both critical packages.

Maintenance note, not a blocker: when GitHub advances `ubuntu-latest` beyond 24.04, the apt line must be re-verified, and section 4.2 already records the exact substitution to apply if `libappindicator3-dev` disappears.

### 4.6 Job names, guard, action versions, and step order

- Job ids and `name:` values are `rust-regression-linux` and `rust-regression-macos`, matching acceptance criterion 1 verbatim, so the check names shown on the pull request equal the names in the issue.
- The push guard is copied byte-for-byte from the existing jobs, including the comment line above it, per fact 2.
- Action versions are the ones already used in this file, per fact 3: `actions/checkout@v5`, `actions/setup-node@v5`, `dtolnay/rust-toolchain@stable` with `components: clippy`, and `swatinem/rust-cache@v2`. No new action is introduced and no version is bumped.
- Step order mirrors `rust-regression` exactly, minus the `cargo test` step, which is out of scope. The Linux job inserts one extra step, `Install Tauri system dependencies`, immediately after `checkout`, which is where `release.yml` places the equivalent step.
- Both new jobs are placed immediately after `rust-regression` and before `windows-release-cli-smoke`, keeping the three Rust regression gates contiguous. This is an insertion only; no existing line is edited, moved, or reindented.

## 5. Exact YAML to add

Add exactly the following two job blocks. Indentation is two spaces for the job id, four for job properties, six for the `- ` of each step, eight for step properties, and ten for `with:` keys, matching every existing job in this file. Steps are separated by one blank line, as in the existing jobs.

```yaml
  rust-regression-linux:
    name: rust-regression-linux
    # Skip branch-deletion push events while still requiring pull request validation.
    if: github.event_name != 'push' || (github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000')
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5

      - name: Install Tauri system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

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

      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: src-tauri -> target
          key: rust-regression-linux

      - name: cargo check
        working-directory: src-tauri
        run: cargo check --all-targets

      - name: cargo clippy
        working-directory: src-tauri
        run: cargo clippy --all-targets -- -D warnings

  rust-regression-macos:
    name: rust-regression-macos
    # Skip branch-deletion push events while still requiring pull request validation.
    if: github.event_name != 'push' || (github.event.deleted != true && github.event.after != '0000000000000000000000000000000000000000')
    runs-on: macos-latest
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

      - name: Rust cache
        uses: swatinem/rust-cache@v2
        with:
          workspaces: src-tauri -> target
          key: rust-regression-macos

      - name: cargo check
        working-directory: src-tauri
        run: cargo check --all-targets

      - name: cargo clippy
        working-directory: src-tauri
        run: cargo clippy --all-targets -- -D warnings
```

Notes for the implementer:

- `workspaces: src-tauri -> target` is a plain YAML scalar. Leave it unquoted, exactly as the existing job has it. Do not add quotes and do not turn it into a block scalar.
- The Linux `run:` block is a literal block scalar with two commands. Keep both lines, keep `sudo`, and keep them in that order.
- Do not add `shell:` to any step. The default shell on each runner is correct: `bash` on Linux and macOS.
- Do not add `fail-fast`, `strategy`, `matrix`, `needs`, `timeout-minutes`, `env`, or `continue-on-error` to either job. None of the existing jobs uses them and none is required here.

## 6. Exact insertion point

At the frozen base, `.github/workflows/pr-regression-gates.yml` reads:

```text
line 85:        run: cargo test --lib --bins --tests
line 86: (blank)
line 87:  windows-release-cli-smoke:
```

Insert the `rust-regression-linux` block, then one blank line, then the `rust-regression-macos` block, then one blank line, between existing line 86 and existing line 87. After the edit, existing line 87 must still be `  windows-release-cli-smoke:` and must still be preceded by exactly one blank line.

Every line numbered 1 through 86 must be unchanged, and every line numbered 87 through 179 must be unchanged and only shifted. The resulting diff must contain zero deleted lines, and the insertion is exactly 89 lines: the 46-line `rust-regression-linux` block, one blank separator line, the 41-line `rust-regression-macos` block, and one trailing blank line before the existing `  windows-release-cli-smoke:`. The file grows from 179 content lines to 268. The file must keep its single trailing newline.

Line endings, verified at the frozen base: `git ls-files --eol .github/workflows/pr-regression-gates.yml` reports `i/lf w/crlf attr/`. The committed blob is LF, the Windows working-tree copy is CRLF, and `.gitattributes` does not cover `*.yml`, so this normalization depends on the local `core.autocrlf` setting rather than on a repository rule. Consequences for the implementer:

- Insert the new lines with whatever ending the surrounding file already uses locally. Do not convert the file, and do not let an editor rewrite the whole file to a uniform ending.
- After staging, `git ls-files --eol` must still report `i/lf` for this path, and `git diff --stat` must still show insertions only. A diff that reports the entire file as changed means the line endings were rewritten; undo it and redo the edit rather than committing the normalization.

## 6b. Composition already validated

The exact YAML in section 5 was spliced into the frozen-base file at the insertion point above and parsed with `js-yaml` during certification. The results, which the implementation must reproduce:

- The file parses as valid YAML and yields exactly six jobs, in this order: `test-debt`, `rust-regression`, `rust-regression-linux`, `rust-regression-macos`, `windows-release-cli-smoke`, `frontend-regression`.
- Runners resolve to `ubuntu-latest`, `windows-latest`, `ubuntu-latest`, `macos-latest`, `windows-latest`, `ubuntu-latest` respectively, and every job's `name` equals its id.
- The parsed `if:` expression of both new jobs is string-equal to the parsed `if:` of `rust-regression`.
- `rust-regression-linux` has 10 steps, `rust-regression-macos` has 9, and both end with `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings`, each with `working-directory: src-tauri`.
- The Linux apt step parses to the two-line block `sudo apt-get update` then `sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`.
- The `Rust cache` steps parse to `{"workspaces":"src-tauri -> target","key":"rust-regression-linux"}` and `{"workspaces":"src-tauri -> target","key":"rust-regression-macos"}`.
- Lines 1 through 86 and 87 through 179 of the original file compare equal, line for line, before and after the splice.

## 7. Expected pull request outcome and the triage rule

This section exists so that neither the implementer nor the reviewer tries to make the new gates green.

- `rust-regression-linux` is expected to FAIL, and that failure is the accepted outcome for #1154. The failure is the non-Windows defect inventory that nothing currently produces, and it is exactly the class tracked by #1113 and its open children #1131 through #1136.
- The normal shape of that failure is a green `cargo check` step, because `dead_code` and the clippy lints in question are warnings, followed by a red `cargo clippy` step under `-D warnings`. If `cargo check` itself fails first, the `cargo clippy` step never runs; that is still an accepted outcome for this issue provided the cause is a non-Windows compile defect in `src-tauri`, which belongs to #1113.
- `rust-regression-macos` may fail for the same reason. The issue text names only Linux, but macOS compiles the same `not(windows)` and `unix` code paths, so a macOS clippy failure of the same class is equally expected and equally acceptable. Record it; do not fix it here.
- Any failure whose cause is NOT a `src-tauri` non-Windows defect is a defect in this change and must be fixed inside the workflow file. The three realistic cases are: a failing `Install Tauri system dependencies` step, which points at section 4.2; a failing `npm ci` or `npm run build` step, which points at the frontend steps; and a workflow parse error, which points at section 5.
- The correct response to a red Linux or macOS gate is to capture the full diagnostic list and hand it to #1113. The forbidden responses are: editing any file under `src-tauri/` or `crates/`, adding `continue-on-error`, removing `-D warnings`, adding allow attributes, and narrowing `--all-targets`.
- Both new jobs must actually execute and report a conclusion on the pull request. A skipped, cancelled, or never-scheduled job does not satisfy acceptance criterion 8.
- Merge is not blocked either way. Per fact 16, branch protection on `main` requires only `validate-branch-name`.

Expect the two new jobs to be the long poles of the workflow on a cold cache, since each performs a full Tauri dependency-graph compile for the first time on its platform.

## 8. Exact changed-file contract

The final committed all-file matrix for this branch, relative to the frozen base, must be exactly these two rows and nothing else:

```text
M	.github/workflows/pr-regression-gates.yml
A	plans/1154-nonwindows-rust-gates.md
```

`plans/` is ignored by root `.gitignore`, so the plan file must be staged with `git add -f plans/1154-nonwindows-rust-gates.md`. It must carry the exact certified bytes recorded in the architect completion report; do not reformat, re-wrap, or re-encode it.

No file under `src-tauri/`, `crates/`, `src/`, `scripts/`, `docs/`, or `linux-setup/` may appear. No other workflow file may appear. `Cargo.toml`, `Cargo.lock`, `package.json`, and `package-lock.json` may not appear.

## 9. Implementation order

1. Re-run the entry gate in section 1. Stop if the frozen SHA no longer matches.
2. Edit `.github/workflows/pr-regression-gates.yml`, inserting the two blocks from section 5 at the insertion point in section 6. Make no other edit to the file.
3. Run the local verification in section 10.
4. Stage exactly the two paths in section 8, force-adding the plan.
5. Commit and, under separate authorization, push and open the pull request.
6. Observe the pull request run and apply the triage rule in section 7. Report the observed conclusions of `rust-regression-linux` and `rust-regression-macos`, and, if either is red, the full diagnostic list for handoff to #1113.

## 10. Exact local verification

From the repository root, confirm the change shape:

```text
git status --porcelain=v1 --untracked-files=all
git diff --stat -- .github/workflows/pr-regression-gates.yml
git diff --name-status --no-renames 1e7f2350b481918c1e63abdf86149630d924ef2f...HEAD
```

The second command must report `1 file changed`, `89 insertions(+)`, and no deletions. The third must print only the two tab-separated rows in section 8.

Confirm that no existing line was deleted or edited. In PowerShell, this must print nothing:

```text
git diff -U0 -- .github/workflows/pr-regression-gates.yml | Select-String -Pattern '^-[^-]'
```

Confirm the line endings were not rewritten. This must still report `i/lf` for the path:

```text
git ls-files --eol .github/workflows/pr-regression-gates.yml
```

Confirm the file still parses as YAML and now declares six jobs. `js-yaml` is not a repository dependency and must not become one; run it through the npx cache, which touches neither `package.json` nor `node_modules`:

```text
npx --yes js-yaml .github/workflows/pr-regression-gates.yml > $env:TEMP\pr-gates.json
node -e "const d=require(process.env.TEMP+'/pr-gates.json'); for (const [id, cfg] of Object.entries(d.jobs)) { console.log(id, cfg['runs-on'], cfg.name, cfg.steps.length); }"
```

The command must exit 0 and print exactly these six rows, in this order:

```text
test-debt                 ubuntu-latest   test-debt                 4
rust-regression           windows-latest  rust-regression           10
rust-regression-linux     ubuntu-latest   rust-regression-linux     10
rust-regression-macos     macos-latest    rust-regression-macos     9
windows-release-cli-smoke windows-latest  windows-release-cli-smoke 9
frontend-regression       ubuntu-latest   frontend-regression       6
```

If the npx route is unavailable, for example on a machine without network access, the acceptable substitute is to rely on the pull request itself: GitHub Actions rejects an invalid workflow file and reports the parse error on the run, so acceptance criterion 7 is satisfied by GitHub accepting the file. Do not add a YAML dependency to `package.json` as a workaround.

No Rust toolchain command is required locally for this change, because no Rust source file is modified. Do not run `cargo clippy` locally as evidence for this issue; the required evidence is the pull request conclusions of the two new jobs, per acceptance criterion 8.

## 11. Findings recorded, deliberately out of scope

Two real defects were verified while certifying this plan. Neither may be touched in this pull request. Both warrant a follow-up issue.

1. **The Rust cache path is wrong for this repository.** Per fact 4, the Cargo workspace root is the repository root and the target directory is the root-level `target/`. Every workflow that configures `swatinem/rust-cache@v2` uses `workspaces: src-tauri -> target`, which points at a `src-tauri/target` that never exists, so the target-directory half of the cache is inert in `rust-regression`, in `windows-release-cli-smoke`, and in `release.yml`. Only `~/.cargo` is effectively cached. The correction is `workspaces: . -> target`, but it must be evaluated together with the repository's 10 GB Actions cache budget, since caching real Tauri target directories across several jobs is large. Fixing it here would violate acceptance criterion 6 for the existing jobs and would exceed the application-path matrix.
2. **`crates/session-bridge` has no gate on any platform.** Per fact 5, every Rust gate runs with `working-directory: src-tauri`, so cargo selects only the `agentscommander-new` package and the second workspace member is never checked, linted, or tested on Windows, Linux, or macOS. Acceptance criterion 2 of #1154 explicitly scopes the new jobs to `src-tauri`, so this plan reproduces the gap rather than closing it.

A third, lower-severity inconsistency is noted for completeness: `release.yml` and `linux-setup/install-ubuntu-deps.sh` disagree on the AppIndicator package name, `libappindicator3-dev` versus `libayatana-appindicator3-dev`. This plan follows `release.yml` because acceptance criterion 4 names it, and section 4.2 records the substitution to apply if it ever becomes uninstallable.

## 12. Objective acceptance

This change is acceptable only when all of these statements are true.

1. `.github/workflows/pr-regression-gates.yml` defines `rust-regression-linux` with `runs-on: ubuntu-latest` and `rust-regression-macos` with `runs-on: macos-latest`, both with `name:` equal to their job id.
2. Both new jobs run `cargo check --all-targets` and then `cargo clippy --all-targets -- -D warnings`, both with `working-directory: src-tauri`, and neither runs `cargo test`.
3. Both new jobs carry the existing two-line push guard byte-for-byte, comment included.
4. The Linux job installs `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, and `patchelf` before any cargo step, or reports the section 4.2 contingency if it was required.
5. Each new job passes an explicit `key` to `swatinem/rust-cache@v2` that differs from the other new job and from the Windows job's implicit key.
6. Both new jobs build the frontend with `npm ci` followed by `npm run build` before the cargo steps, so all three platforms compile the same `has_embedded_dist` cfg set.
7. The macOS job checks only the native `aarch64-apple-darwin` host and declares no `targets:` input.
8. `test-debt`, `rust-regression`, `windows-release-cli-smoke`, and `frontend-regression` are byte-unchanged, and the workflow-level `name`, `on`, `permissions`, and `concurrency` blocks are byte-unchanged. The diff for the workflow file is exactly 89 insertions and zero deletions, and `git ls-files --eol` still reports `i/lf` for the path.
9. GitHub Actions accepts the workflow on the pull request, and both new jobs actually execute and report a conclusion.
10. The final committed all-file matrix is exactly the two rows in section 8, with the plan force-added and byte-identical to the certified bytes.
11. `validate-branch-name` passes, and any red conclusion on the two new gates is attributable to a `src-tauri` non-Windows defect belonging to #1113, with the full diagnostic list captured for handoff.
12. Nothing under `src-tauri/` or `crates/` was modified, no gate was weakened, and neither finding in section 11 was acted on.

No unresolved implementation choice remains within this Lite scope.
