# Plan #2162 — Config seed: OS-aware variants for all five tiers (`default<dest>.<os>`)

Status: READY_FOR_IMPLEMENTATION
Plan class: Full, single phase (PARTITION: 1 phase)
Phase class: design-bearing
Owner: Rust implementer (dev-rust)
Issue: #2162 (no child issues; single unit)
Branch: `feat/2162-os-aware-config-seed-tiers` (already created)
Pinned planning base: `983e651bd924b0fdc801afcbbb40fc0a73dfa231` (= `origin/main` and branch HEAD at planning time)
Repo root: `<room>/repo-AgentsCommander`
Accepted task class: routine application-code change. Threat model: trusted developer host, repository-pinned toolchain, GitHub CI authoritative on exact PR head. No release, signing, packaging, security-boundary or destructive-migration control applies.

## 1. Objective

Config seed must prefer an OS-specific template folder and fall back to the OS-agnostic one: on Linux, `default.claude.linux` before `default.claude`. Add an OS variant to **every** one of the five tiers, as a refinement inside the tier (profile-major), never jumping a tier.

## 2. Scope

In scope:
- `src-tauri/src/config/config_seed.rs` — candidate construction, candidate type, selection, logs, tests.
- `src-tauri/src/config/agent_command.rs` — tier-5 append, one call site, tests.
- `src-tauri/src/config/settings.rs` — OS token constant; save-time collision **warning** (non-blocking).
- `docs/features/config-seed.md` — 10-entry precedence, tokens, inherited semantics, log line.
- `.github/workflows/pr-regression-gates.yml` — **scope change, 4 files to 5** (decided in §6): one new filtered `cargo test` step in the `rust-regression-linux` job, so the config-seed tests execute on Linux in CI. No other job, trigger or step is touched.

Out of scope (do not touch):
- `validate_config_seed_dest` accept/reject behavior — unchanged.
- Seed manifest on-disk format, `ManifestSource`, `config:<dest>` scope strings — unchanged.
- The `BUILTIN_AGENT_SUPPORT` latent gate on tier 5 (tracked in #2146).
- Any user-facing setting or env var for the OS token.

## 3. Decided solution (no alternatives, nothing left to the implementer)

### 3.1 Tokens and their source
Tokens are exactly `linux`, `windows`, `macos` — lowercase, exact, no aliases.

Add to `settings.rs`, next to `validate_config_seed_dest`:

```rust
/// #2162 - the OS suffix tokens a config-seed template folder may carry.
pub const CONFIG_SEED_OS_TOKENS: [&str; 3] = ["linux", "windows", "macos"];
```

It lives in `settings.rs` (not `config_seed.rs`) on purpose: `config_seed` already depends on `settings`; the reverse arc does not exist and must not be created (see §8).

Add to `config_seed.rs`:

```rust
/// #2162 - compile-time host OS token. `None` on any other target, which then
/// contributes no OS candidates. Not user-overridable.
pub fn host_os_token() -> Option<&'static str> {
    if cfg!(target_os = "linux") { Some("linux") }
    else if cfg!(target_os = "windows") { Some("windows") }
    else if cfg!(target_os = "macos") { Some("macos") }
    else { None }
}
```

The token is **passed into** `resolve_config_seed` as a parameter so tests inject all three from one host.

### 3.2 Candidate type
Replace the tuple with a struct in `config_seed.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSeedCandidate {
    pub tier: ConfigSeedTier,
    /// True when this path carries the `.<os>` suffix.
    pub os_specific: bool,
    pub path: PathBuf,
}
```

`ResolvedConfigSeed.candidates` becomes `Vec<ConfigSeedCandidate>`. `ConfigSeedTier` keeps its five variants unchanged.

### 3.3 Signature
```rust
pub fn resolve_config_seed(
    cfg: &ConfigSeedConfig,
    effective_letter: &str,
    context: Option<&PlaceholderContext>,
    os_token: Option<&str>,
) -> Option<ResolvedConfigSeed>
```
`os_token` semantics: `Some(t)` emits the OS variant of each tier immediately above its base; `None` emits exactly today's candidate list. The implementer does **not** validate `t` against `CONFIG_SEED_OS_TOKENS` inside `resolve_config_seed` (it is a compile-time-derived value); the constant exists for the settings warning and for the tests' token table.

### 3.4 Candidate construction (`config_seed.rs:180-200`)
For each existing tier, push the OS variant first (only when `os_token` is `Some`), then the base, keeping today's tier order:

| Rank | Tier | Folder (`os_token = Some("linux")`) | `os_specific` |
|---|---|---|---|
| 1 | WorkspaceProfile | `<ws>/default_profile_<l><dest>.linux` | true |
| 2 | WorkspaceProfile | `<ws>/default_profile_<l><dest>` | false |
| 3 | WorkspaceBase | `<ws>/default<dest>.linux` | true |
| 4 | WorkspaceBase | `<ws>/default<dest>` | false |
| 5 | MatrixProfile | `<mx>/default_profile_<l><dest>.linux` | true |
| 6 | MatrixProfile | `<mx>/default_profile_<l><dest>` | false |
| 7 | MatrixBase | `<mx>/default<dest>.linux` | true |
| 8 | MatrixBase | `<mx>/default<dest>` | false |
| 9 | CatalogDefault | `<ac_root>/coding-agents/_seed/<dest>.linux` | true |
| 10 | CatalogDefault | `<ac_root>/coding-agents/_seed/<dest>` | false |

The suffix is formed as `format!("{}.{}", <today's folder name>, os)`. `letter` stays lowercased (`config_seed.rs:163`). The existing `if candidates.is_empty() { return None; }` guard is unchanged (an OS token alone never creates candidates: both roots must still be present).

### 3.5 Tier 5 (`agent_command.rs:925-960`)
The caller keeps ownership of tier 5. Where it pushes one CatalogDefault candidate today, it pushes the OS variant first, then the base, both via `master_dir_for_dest` (which is a pure join, so the suffixed dest is legal):

```rust
if let Some(ac_or_config_dir) = /* today's ac_root -> config_dir chain, unchanged */ {
    if let Some(os) = os_token {
        r.candidates.push(ConfigSeedCandidate {
            tier: ConfigSeedTier::CatalogDefault,
            os_specific: true,
            path: master_dir_for_dest(&ac_or_config_dir, &format!("{dest}.{os}")),
        });
    }
    r.candidates.push(ConfigSeedCandidate {
        tier: ConfigSeedTier::CatalogDefault,
        os_specific: false,
        path: master_dir_for_dest(&ac_or_config_dir, dest),
    });
}
```
Restructure the existing `.map(...).or_else(...)` chain so the chosen root is bound once and used for both pushes; the root-selection logic itself is unchanged. `os_token` at this call site is `host_os_token()`, also passed to `resolve_config_seed`.

### 3.6 Selection (`config_seed.rs:495-505`) — semantics inherited per tier
Only the destructuring changes; the rules do not. Tiers 1-4: first readable dir wins, overwrites every spawn. Tier 5: only if no 1-4 matched, **and** `is_nonempty_seed_dir`, **and** `destination_absent_no_follow`. Because the OS variant precedes its base **within** the same tier in list order, and both passes scan the list in order, precedence is a property of the list — no extra comparison logic.

```rust
let mut selected = seed.candidates.iter()
    .find(|c| c.tier != ConfigSeedTier::CatalogDefault && is_readable_dir(&c.path));
if selected.is_none() {
    if let Some(candidate) = seed.candidates.iter()
        .find(|c| c.tier == ConfigSeedTier::CatalogDefault && is_nonempty_seed_dir(&c.path))
    { /* today's destination_absent_no_follow gate, unchanged */ }
}
```
The later `let Some((tier, src)) = selected else {...}` becomes `let Some(candidate) = selected else {...}`, with `candidate.tier` / `&candidate.path` used downstream.

### 3.7 Manifest
`manifest_source_for_tier` is **unchanged** and ignores `os_specific`: an OS variant records the same `ManifestSource` as its base. `config:<dest>` scope rows and `source` strings stay byte-compatible; older builds keep reading them.

### 3.8 Logs (exact strings)
Both log sites use one shared, testable helper in `config_seed.rs`:

```rust
/// #2162 - marker appended to the tier name in logs when an OS variant won.
pub(crate) fn os_marker(os_specific: bool) -> &'static str {
    if os_specific { "+os" } else { "" }
}
```

Success line (`config_seed.rs:684-688`) gains the OS marker:
```rust
log::info!(
    "[config-seed] seeded '{}' into replica from {:?}{} source '{}'",
    seed.dest.display(),
    candidate.tier,
    os_marker(candidate.os_specific),
    candidate.path.display()
);
```
"No source found" listing (`config_seed.rs:534-540`) uses the same marker:
```rust
.map(|c| format!("{:?}{}={}", c.tier, os_marker(c.os_specific), c.path.display()))
```

### 3.9 Save-time collision warning (`settings.rs:2392-2397`)
`validate_config_seed_dest` is **not** changed and the save is **not** rejected.

Add next to `CONFIG_SEED_OS_TOKENS` a pure, testable predicate (the log call itself is the only untestable part):

```rust
/// #2162 - `Some(stem)` when `dest` ends with an OS token, where `stem` is the
/// dest it collides with. `stem` is empty when `dest` is exactly `".<token>"`.
pub fn config_seed_dest_os_token_stem(dest: &str) -> Option<String> {
    let lower = dest.trim().to_ascii_lowercase();
    CONFIG_SEED_OS_TOKENS
        .iter()
        .find(|t| lower.ends_with(&format!(".{t}")))
        .map(|t| lower[..lower.len() - t.len() - 1].to_string())
}
```

In the existing per-agent block, after the successful `validate_config_seed_dest(&seed.dest)?`, add:

```rust
if let Some(stem) = config_seed_dest_os_token_stem(&seed.dest) {
    if stem.is_empty() {
        log::warn!(
            "[config-seed] agent \"{}\" dest '{}' is only an OS token suffix",
            agent.label,
            seed.dest.trim()
        );
    } else {
        log::warn!(
            "[config-seed] agent \"{}\" dest '{}' ends with an OS token; it names the same template folder as the OS variant of dest '{}'",
            agent.label,
            seed.dest.trim(),
            stem
        );
    }
}
```
Warning only. No new error path, no IPC contract change.

### 3.10 CI execution step on Linux (the fifth file)

Add one step to job `rust-regression-linux` (`.github/workflows/pr-regression-gates.yml:631`, `runs-on: ubuntu-latest` `:635`), after the existing `cargo test (IS #1842 ...)` step (`:786`). Same three-guard shape as that step and the #1577 step (`:726`); nothing else in the file changes.

```yaml
      - name: cargo test (IS #2162 config seed OS tiers on Linux)
        working-directory: src-tauri
        shell: bash
        run: |
          set -euo pipefail
          FILTER='config::config_seed::tests::'
          # Count observed in the green local run of §7; update it in the same
          # commit that adds or removes a test in that module.
          EXPECTED=<count from the local run>
          cargo test --locked --lib "$FILTER" -- --test-threads=1 --nocapture 2>&1 | tee test-2162.log

          # Guard 1: the exit code through `tee` (pipefail is LOAD-BEARING)
          #          catches a test that ran and failed.
          # Guard 2: the sentinel grep is the POSITIVE CONTROL -- a filter that
          #          matches nothing exits 0 and prints "0 passed".
          # Guard 3: the anchored count is the mutation probe -- a test gated
          #          out moves the number.
          grep -qF 'config::config_seed::tests::os_candidates_for_each_token' test-2162.log || {
            echo "::error::the #2162 sentinel test never ran; the filter matched nothing and this step tested nothing."
            exit 1
          }
          grep -qE "^test result: ok\. ${EXPECTED} passed; 0 failed" test-2162.log || {
            echo "::error::expected exactly ${EXPECTED} passing tests in ${FILTER}; the count changed or the filter matched something else."
            exit 1
          }
```

This executes the `linux` token branch of `host_os_token()` on a real Linux runner, plus every injected-token test, on the exact PR head.

## 4. Required behavior, edge cases, failure behavior

- **Fallback is byte-identical.** With no `.<os>` folder on disk, every OS candidate misses and selection lands on exactly today's winner: same tier, same copy, same manifest scope, same `ManifestSource` string.
- **OS never jumps a tier.** `<ws>/default<dest>` (rank 4) beats `<mx>/default_profile_<l><dest>.linux` (rank 5).
- **Unknown target.** `host_os_token() == None` ⇒ 4 (or 5 with tier 5) candidates, identical to today.
- **Tier 5 gating unchanged.** The OS variant of tier 5 is equally absent-only and non-empty-gated; an OS master that exists but is empty reads as "not present" and the base master is then considered.
- **Fail-soft preserved.** `resolve_config_seed` still returns `None` rather than erroring; `perform_config_seed` still never aborts a spawn.
- **Invalid/odd `dest`.** `dest` cannot contain separators, `..`, `:`, or a trailing dot, so `<dest>.<os>` never yields a second segment, a traversal, or a trailing dot. Composed workspace/matrix names still start with `default`. The reserved-device-name check still runs on `dest` alone (`settings.rs:2353`).
- **Case.** Tokens are emitted lowercase. On case-insensitive filesystems (Windows, default macOS) a `.LINUX`-cased folder would also match; on Linux it would not. Documented, not normalized.

## 5. Tests (all in `config_seed.rs` `#[cfg(test)]`, plus the one caller test)

Mandatory:
1. `os_candidates_for_each_token` — table over `["linux","windows","macos"]`; asserts the full 8-entry `Vec<ConfigSeedCandidate>` from `resolve_config_seed` equals the exact expected paths and `os_specific` flags for that token (ranks 1-8 of §3.4).
2. `no_os_token_matches_today` — `os_token = None` yields exactly today's 4 candidates with `os_specific == false`.
3. `os_variant_wins_within_its_tier` — both `<ws>/default.claude.linux` and `<ws>/default.claude` on disk ⇒ the OS one is copied.
4. `os_never_jumps_a_tier` — `<ws>/default.claude` and `<mx>/default_profile_a.claude.linux` on disk ⇒ the workspace base wins.
5. `fallback_is_byte_identical` — only `<ws>/default.claude` on disk, with `os_token = Some("linux")` ⇒ same published files, same tier, and the manifest row's `source` string equals the one produced with `os_token = None`.
6. `catalog_default_os_variant_keeps_absent_only_and_nonempty` — OS master non-empty + dest absent ⇒ fills; dest present ⇒ skipped; OS master empty + base master non-empty ⇒ base fills.
7. `manifest_source_ignores_os_specific` — winning OS variant of each of the five tiers maps to the same `ManifestSource` as its base.
8. `build_spawn_appends_both_catalog_default_candidates` — update `agent_command.rs:2335-2352` to expect the 10-entry list (host token) and keep asserting no template dirs were created.
9. `os_marker_is_emitted_only_for_os_variants` (`config_seed.rs`) — asserts `os_marker(true) == "+os"` and `os_marker(false) == ""`, and asserts the two formatted strings of §3.8 built from a `ConfigSeedCandidate` pair: `format!("{:?}{}", tier, os_marker(c.os_specific))` yields `WorkspaceBase+os` and `WorkspaceBase`. This covers acceptance criterion 5 for both the success line and the "no source" listing.
10. `dest_os_token_stem_detects_collisions` (`settings.rs` `#[cfg(test)]`) — table: `".claude.linux"`/`".claude.WINDOWS"`/`".claude.macos"` ⇒ `Some(".claude")`; `".claude"`/`".claudelinux"` ⇒ `None`; `".linux"` ⇒ `Some("")`. Covers §3.9's decision logic.

**Positive control (required by the verification-difficulty veto):** `selection_is_order_sensitive` — construct a `ResolvedConfigSeed` by hand with the pair **inverted** inside one tier (base before OS variant), both directories present on disk, and assert the **base** wins. What this proves, exactly: selection has **no** intrinsic preference for `os_specific` and follows list order alone, so precedence is entirely a property of the vector built in §3.4. It does **not** by itself prove that tests 1-4 fail on a wrong construction order — test 1 does that, by asserting the exact candidate vector.

**Recorded negative runs (two mutations, both required).** Before committing, apply each mutation to §3.4 alone, record the result, and restore:

- **Mutation A — swap the two pushes inside every tier** (base before OS variant). List becomes wsProfile, wsProfile.os, wsBase, wsBase.os, mxProfile, mxProfile.os, mxBase, mxBase.os. Expected: tests **1 and 3 fail**; test 4 still **passes**, because it only measures order *between* tiers and mutation A reorders *within* a tier. Do not report test 4 as failing here.
- **Mutation B — emit all four OS variants as one block before the four base tiers.** List becomes wsProfile.os, wsBase.os, mxProfile.os, mxBase.os, wsProfile, wsBase, mxProfile, mxBase. Expected: tests **1 and 4 fail** (test 4's `<mx>/default_profile_a.claude.linux` now outranks its `<ws>/default.claude`); test 3 still passes.

Together the two mutations fail tests 1, 3 and 4. Restore, record all tests passing, and report the three runs as evidence.

## 6. Environment and tooling risk (owning dev writes this statement before touching code)

Concrete points the statement must cover:
- `cfg!(target_os)` is resolved at compile time, so a real build only ever produces its own token; **only** parameter injection exercises the other two. No test may depend on the host's token.
- **Where the tests actually execute (each claim read from the workflow file).** The only unfiltered `cargo test --locked --lib --bins --tests` in the repository is `.github/workflows/pr-regression-gates.yml:96`, in job `rust-regression` (`:50`), `runs-on: windows-latest` (`:54`). The one other occurrence, `.github/workflows/cache-warm.yml:70`, carries `--no-run`: it compiles, it does not execute. Job `rust-regression-linux` (`:631`, `runs-on: ubuntu-latest` `:635`) runs `cargo check --locked --all-targets` (`:682`) and `cargo clippy --locked --workspace --all-targets -- -D warnings` (`:688`), and only **filtered** tests: `issue_1937_config_lock` (`:700`), the IS #1577 case (`:733`), `screenshot::native::tests::` (`:798`), `issue_1850` (`:837`, `:1338`). None of those filters reaches `config_seed`. **Before this plan, therefore, the config-seed tests executed on Windows only**, and the `linux` token branch of `host_os_token()` — the branch the feature exists for — ran on no runner at all.
- **Decision: close that gap (option b).** §3.10 adds one filtered `cargo test` step to `rust-regression-linux`, which makes `.github/workflows/pr-regression-gates.yml` the **fifth file** in scope (§2). Rationale: the feature originates from a Linux need and the user develops on Linux, so leaving the `linux` path executed only on a developer's machine puts the primary target behind the weakest gate; the fix is one step in an existing job, mirroring the guard pattern already used twice in that job, with no new runner, trigger or minute-heavy work. The alternative — accepting the debt and staying at four files — was rejected for that asymmetry, not for cost.
- **There is a macOS runner and it is blocking.** Job `rust-regression-macos` (`.github/workflows/pr-regression-gates.yml:1614`, `runs-on: macos-latest:1618`) triggers on every `pull_request` and runs `cargo check --locked --all-targets` and `cargo clippy --locked --all-targets -- -D warnings`. `ConfigSeedCandidate`, `os_marker`, `host_os_token` and every new test must therefore **compile and be clippy-clean on macOS**; `host_os_token()` returns `Some("macos")` there, so that branch is compiled in CI.
- What macOS CI does **not** do is *run* these tests: its `cargo test` steps are filtered to `screenshot::native::tests::` (`:1672`), `issue_1937_config_lock` (`:1702`) and `issue_1850` (`:1725`, `:2222`). So the *execution* coverage of the `macos` token comes from parameter injection on the Linux and Windows runners only — that is the remaining accepted debt. The `None` branch of `host_os_token()` is executed by no runner at all; it is covered by injection (test 2) and accepted as debt.
- Filesystem case sensitivity differs across the three targets (see §4, "Case"): tests must use exact lowercase names so they behave identically on Linux and Windows.
- Path length: the suffix adds ≤8 characters under the replica/workspace root; Windows `MAX_PATH` margin shrinks slightly. Tests use `tempfile::tempdir()` and short names.
- Clippy with `-D warnings` runs on all three runners, but not with the same flags: `--workspace --all-targets` on windows (`:92`) and ubuntu (`:688`), `--all-targets` **without** `--workspace` on macOS (`:1654`). The new struct must not trip `clippy::struct_excessive_bools` or dead-code lints under either form.

## 7. Verification

```
cd <repo>/src-tauri
cargo fmt --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --lib config_seed
cargo test --locked --lib --bins --tests
```
Expected: all green locally on Linux. Record the passing count of `config::config_seed::tests::` from that run and put it in `EXPECTED` in the §3.10 step.

CI on the **exact PR head SHA** owns the rest; evidence from any other SHA, or a skip/waiver, does not satisfy the gate:
- `rust-regression` (windows-latest) executes the config-seed tests inside the unfiltered suite (`:96`).
- `rust-regression-linux` (ubuntu-latest) executes them through the new filtered step of §3.10, and is the only runner that executes the `linux` token branch of `host_os_token()`.
- `rust-regression-macos` (macos-latest, `:1614`) is blocking for `cargo check` (`:1650`) and `clippy --locked --all-targets -- -D warnings` (`:1654`) and must be green. It does **not** execute the config-seed tests.

Remaining accepted debt, unchanged by this plan: the `macos` branch of `host_os_token()` is compiled but never executed in CI, and the `None` branch is executed by no runner. Both are covered by token injection only.

## 8. Compatibility, security, cycles, layering

- **Persistence:** none. Manifest bytes, scopes and `ManifestSource` strings unchanged (§3.7).
- **Public API:** `resolve_config_seed` gains a 4th parameter and `candidates` changes element type. Both are crate-internal (`ConfigSeedTier` is referenced only by `config_seed.rs` and `agent_command.rs`); no IPC, no TS contract, no frontend change.
- **Security:** no new path input. All new paths are `format!`-composed from an already-validated `dest` and a compile-time constant token.
- **Dependency cycles:** no new module-to-module arc. `config_seed -> settings` and `agent_command -> config_seed` already exist; `settings -> config_seed` is deliberately avoided by putting `CONFIG_SEED_OS_TOKENS` in `settings.rs` (§3.1). Cyclic SCC set unchanged.

## 9. Delivery invariants (baseline gates)

- **Git:** all state-changing Git inside `repo-AgentsCommander` only. Work on the existing `feat/2162-os-aware-config-seed-tiers`, verified from base `983e651b…`; re-fetch `origin/main` before the first mutation and again before opening the PR, and classify drift by changed paths (`config_seed.rs`, `agent_command.rs`, `settings.rs`, `docs/features/config-seed.md`, the two workflows) — unrelated drift is recorded, not re-planned. Deliver by PR into `main`; never push to `main`.
- **Scope:** exactly the five files in §2. Before commit, `git status --porcelain` and `git diff --name-only <base>..HEAD` must list those five and nothing else.
- **Recovery:** on failure restore only files this run changed, via targeted `git restore -- <path>`; no repository-wide reset or clean.
- **Bounded execution:** `cargo` commands run non-interactively with captured stdout/stderr; a timed-out or failed command is reported as failed.
- **Evidence:** the two negative runs of §5, the recorded `EXPECTED` count, and all three CI runners' results (ubuntu, windows, macOS) on the exact PR head, including the new §3.10 step's log.

## 10. Ordered implementation

1. `settings.rs`: add `CONFIG_SEED_OS_TOKENS` and `config_seed_dest_os_token_stem`; add the save-time warning (§3.9).
2. `config_seed.rs`: add `ConfigSeedCandidate` and `host_os_token`; change `ResolvedConfigSeed.candidates`; add the `os_token` parameter and build the 8 candidates (§3.4). Compile — the compiler now lists every consumer.
3. `config_seed.rs`: add `os_marker`; update selection, the "no source" listing and the success line (§3.6, §3.8).
4. `agent_command.rs`: pass `host_os_token()`; push both tier-5 candidates (§3.5).
5. Tests §5 (1-10), including the positive control and both recorded negative runs (mutations A and B).
6. `docs/features/config-seed.md`: replace the 5-row tier table and the `.claude`/profile `A` example with the 10-entry list; add the tokens and their compile-time source, the per-tier inherited semantics, the case note, and the new log marker.
7. `.github/workflows/pr-regression-gates.yml`: add the §3.10 step to `rust-regression-linux`, with `EXPECTED` set from the local run of step 8.
8. Run §7; open one PR closing #2162.

## 11. Acceptance criteria

- All five tiers have an OS variant, ordered profile-major exactly as §3.4.
- Tokens `linux`/`windows`/`macos`, from the build target, not user-overridable; unknown target ⇒ no OS candidates.
- OS variants inherit their tier's overwrite / absent-only semantics.
- With no OS-suffixed folder, behavior is byte-identical to today, manifest included.
- The success log line and the "no source" listing identify whether an OS variant won, via `os_marker` (test 9, which verifies the helper and the formatted strings; the use of the helper at the two `log::` sites of §3.8 is **not verified by test** and is checked by diff).
- A `dest` ending in an OS token is detected at save time and warned about, without rejecting the save (test 10). The emission of the `log::warn!` itself is declared **not verified by test**; only its decision logic is.
- Tests cover all three tokens injected on one host, the fallback, and the order-sensitivity positive control with both recorded negative runs.
- `docs/features/config-seed.md` updated as in step 6.
- The config-seed tests execute on the ubuntu runner via the new `rust-regression-linux` step (§3.10), with its sentinel and count guards green.
- `cargo fmt --check`, clippy `-D warnings`, and the full test suite pass locally; every triggered and configured-required CI check passes on the exact PR-head SHA.
