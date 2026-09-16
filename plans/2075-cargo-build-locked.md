# Plan #2075: `cargo build --release` in `pr-regression-gates.yml` without `--locked`

Status: READY_FOR_IMPLEMENTATION

- Issue: https://github.com/mblua/AgentsCommander/issues/2075 (OPEN): "ci(security): cargo build --release in pr-regression-gates without --locked (Sonar S8549)". Sonar issue `AaCI_U_rCXUCLYag_fqs`.
- Repo: `repo-AgentsCommander`; branch `fix/2075-cargo-build-locked`.
- Base frozen at authoring (2026-09-16 UTC): HEAD = remote branch head = remote `main` = `5203c4e3b0b5909fdc12337194c886b1514966c3`; tracked tree clean. Line numbers below refer to that SHA; if a quoted line stops matching, re-anchor on the quoted text, never on the number.
- Class: Lite (band 1-25). Owner `ac-dev-rust-v4`; reviewer Grinch; coordinator `ac-tech-lead-v4`.
- 1 modified file, 0 added, 0 removed: `.github/workflows/pr-regression-gates.yml` (one content line), plus this plan.
- Root `.gitignore:11` ignores `/plans/`; commit this plan with `git add -f plans/2075-cargo-build-locked.md`.

## 1. Objective

Make the release-parity build consume the tracked `Cargo.lock` exactly: add `--locked` to the one step that builds the release binary, closing Sonar S8549.

## 2. Verified cause

The step at `.github/workflows/pr-regression-gates.yml:1513-1515` (job `rust-linux-release-parity`, `runs-on: ubuntu-22.04`) is:

```yaml
      - name: cargo build --release (links the binary release.yml ships)
        working-directory: src-tauri
        run: cargo build --release --bins
```

Without `--locked`, cargo may re-resolve versions from the manifests instead of failing on a stale lockfile. `Cargo.lock` is tracked at the workspace root; there is no `src-tauri/Cargo.lock`.

- This is the only `cargo build` invocation in `.github/workflows/`; every other match is a comment (`pr-regression-gates.yml:1358`, `:1545`). It is also the only compile-and-link step in the release-parity path without the flag.
- `--locked` is already used by the `cargo test` steps at `pr-regression-gates.yml:108,134,354,700,835,1051,2240,2243`.
- Observed but NOT part of #2075: `cargo check`/`clippy`/`test` steps that also omit `--locked` — `pr-regression-gates.yml:88,92,96,684,688,1648,1652`; `cache-warm.yml:60,65,70`. Reported to the coordinator for a separate decision; Sonar flagged only line 1515.

## 3. Scope

In scope: line 1515 of `.github/workflows/pr-regression-gates.yml` (add `--locked`), and this plan file.

Out of scope: `Cargo.lock` and every manifest (no dependency change expected or allowed); all other workflow lines and workflows; job structure, caching (`workspaces: '. -> target'`), the `ldd` step; `release.yml`; the unlocked check/clippy/test lines listed in §2.

## 4. Exact edit

`.github/workflows/pr-regression-gates.yml`:

```diff
       - name: cargo build --release (links the binary release.yml ships)
         working-directory: src-tauri
-        run: cargo build --release --bins
+        run: cargo build --locked --release --bins
```

Exact final line, 8 leading spaces: `        run: cargo build --locked --release --bins`

`--locked` sits directly after the subcommand, matching the file's existing style (`cargo test --locked --release --lib ...`, `cargo test --locked -p ...`). No other byte changes anywhere in the file.

## 5. Why `--locked` needs no lockfile change

- Root `Cargo.toml` is a `[workspace]` listing `src-tauri` as a member; `src-tauri/Cargo.toml` has no `[workspace]` key and no lockfile, so cargo started in `src-tauri` walks up to the workspace root's tracked `Cargo.lock`. `--locked` binds that file.
- `Cargo.lock` was last changed only by release commits (`f5b4507e` v0.34.0, `4a6bead6` v0.33.0, `18d0055e` v0.32.0). This branch changes no manifest.
- Local evidence (Windows, `cargo 1.97.1 (c980f4866 2026-06-30)`, `rustc 1.97.1 (8bab26f4f 2026-07-14)`), commands run from `src-tauri`:
  1. `cargo metadata --locked --format-version 1` -> exit 0, 1.57 s, empty stderr.
  2. `cargo build --release --bins --locked` (the exact post-fix CI command) -> exit 0, 4 m 32.9 s, `Finished \`release\` profile [optimized] target(s) in 4m 32s`.
  3. `git status --porcelain` empty after both -> `Cargo.lock` untouched.
- The proposed post-edit YAML parses (PyYAML 6.0.3) and yields run `cargo build --locked --release --bins` with `working-directory: src-tauri`.

## 6. Acceptance

1. `.github/workflows/pr-regression-gates.yml:1515` reads exactly `run: cargo build --locked --release --bins`; `git diff` shows this single content line in that file; `Cargo.lock` has no diff.
2. The `rust-linux-release-parity` job on this branch is green (workflow runs on `push` to non-main branches and on `pull_request`), and the step "cargo build --release (links the binary release.yml ships)" succeeds with no lockfile error.
3. Sonar issue `AaCI_U_rCXUCLYag_fqs` closes after the fix lands on `main` and the next Sonar analysis runs; the scan is not part of this workflow and is not blocked by this branch.
4. Grinch review passes.

## 7. Verification (for the implementer)

From `src-tauri`, after the edit:

```
cargo metadata --locked --format-version 1 > /dev/null   # cheap gate, ~2 s
git -C .. diff --stat                                    # workflow line + plan only
git -C .. status --porcelain -- Cargo.lock               # must print nothing
```

Optional locally, authoritative (~5 min on a warm target): `cargo build --release --bins --locked`.

YAML sanity from the repo root:

```
python -c "import yaml;yaml.safe_load(open('.github/workflows/pr-regression-gates.yml',encoding='utf-8'))"
```

## 8. Implementation order

1. Apply §4; run §7.
2. `git add .github/workflows/pr-regression-gates.yml && git add -f plans/2075-cargo-build-locked.md`
3. Commit `ci(2075): build release binaries with --locked`; push to `fix/2075-cargo-build-locked`.
4. Watch the push run's `rust-linux-release-parity` job.
5. Report to the coordinator with commit SHA, the exact line, and the §5 command results. No push to `main`, no merge.

## 9. Risks

- If `Cargo.lock` were stale, `--locked` turns a silent re-resolution into a hard failure. Mitigated: §5 gates pass and no manifest changed on this branch.
- Local proof is Windows while the CI leg is ubuntu-22.04. The flag changes neither the dependency graph nor the toolchain; the only new failure mode is lockfile staleness, already excluded.
- Sonar closure timing is owned by the external scan schedule, not this branch.

## Plan Contract

Scope: exactly one content line of `.github/workflows/pr-regression-gates.yml` (line 1515) plus this plan file. No `Cargo.lock`, manifest, other workflow line, job, or caching change. The implementer runs the §7 gates, keeps the §5 evidence, and reports raw outputs (exit codes, timings, `git status`). Any need for a different line, a second file, or a lockfile update stops the work and returns to the coordinator. Acceptance is §6.
