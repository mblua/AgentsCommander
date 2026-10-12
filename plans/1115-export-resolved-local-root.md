# #1115 — Export the resolved local root as AGENTSCOMMANDER_LOCAL_DIR

Status: READY_FOR_IMPLEMENTATION (round 4; supersedes round-3 SHA 8470BD5F…)
Band: Lite. Owner: ac-dev-rust-v4. Reviewer: Grinch (verification veto).
Issue: https://github.com/mblua/AgentsCommander/issues/1115 (parent #1110)
Branch: `fix/1115-export-resolved-local-root`, pinned base `main` 5203c4e3.
Task class / threat model: routine application-code change; no supply-chain, signing or untrusted-host requirement. Enhanced controls: none applicable.

## 0b. Round-4 changes (Grinch AC2 wiring blocker)

| Finding | Resolution |
|---|---|
| T12 misses `.unwrap_or_default()` in place of `?` (non-Unicode root would export `""`) | T12 (vi) now pins `resolve_local_dir(crate::config::config_dir().as_deref(),)?` whitespace-stripped, including rustfmt's trailing comma and the `?`. |
| Stale line refs | Updated to current `main` 5203c4e3: `discover_teams` `:1546`, `backend_kind` `:1636`, `set_pending_start_fresh_on_restore` `:1806`, `set_pending_effective_shell_args` `:2243`. |
| No verbatim exe-parent fixture (non-blocking) | Accepted: the `None` branch is legacy fallback; normalization there is covered by the shared `normalize_windows_verbatim_path` tests. No new test. |

## 0a. Round-3 changes (Grinch test-oracle blockers)

| Blocker | Resolution |
|---|---|
| B1 T12 passes with resolver arg `None` or gate without `agent_id.is_some()` | T12 (v) and (vi) pin both texts exactly. |
| B2 T6 literal `/opt/x/.agentscommander_s` fails on Windows | T6 oracle is `Path::new("/opt/x").join(".agentscommander_s")` via `to_str()`. |
| B3 AC4 "warned" unasserted | Warning goes through an injected sink; T6 asserts it fires exactly once on `None`, T4 that it never fires on `Some`; T12 (vii) pins the production sink to `log::warn!`. |

## 0. Round-2 changes (Grinch F1–F4, verified at 5203c4e3)

| Finding | Verified | Resolution |
|---|---|---|
| F1 permit leak | `try_reserve_agent_slot(...)?` at `commands/session.rs:1629`; `AgentLaunchPermit` (`resource_monitor/registry.rs:99`) has no `Drop`; later error paths call `release_resource_launch_permit` + `drop(mgr)` + `rollback_pre_created_session` (`:2202`, `:2226`, `:2379`). A `?` at `:2249` would leak the permit and the pending session row. | The only fallible step moves **before** permit reservation, pending row and coordinator-clock mutations (§2.2). The late step becomes infallible. No new cleanup path exists. Source-order test T12. |
| F2 lossy stem | `binary` is built with `to_string_lossy` (`credentials.rs:53`) before the legacy join. | Legacy derivation works on `OsStr` from `exe` directly (stem + parent), never on `binary`. Test T9b. |
| F3 verbatim | `strip_prefix(r"\\?\")` turns `\\?\UNC\s\share` into `UNC\s\share` and strips on Linux. `crate::path_utils::normalize_windows_verbatim_path` (`path_utils.rs:15`) handles `\\?\`, `\\?\UNC\`, `\??\`, `\??\UNC\` and is a no-op off Windows. | `local_dir` uses `normalize_windows_verbatim_path`. `binary_path` keeps its current strip (out of scope). Tests T5a/T5b. |
| F4 containers | Container backend drops `extra_env` (`pty/container_backend.rs:2048 extra_env: _`); container value comes from `request.local_dir` (`pty/docker_runtime.rs:773`). | Fallible resolution is gated to `agent_id.is_some()` **and** `LocalProcess`, matching the issue ("every host local PTY child"). Container sessions get `extra_env = Vec::new()`; since the field is discarded, container behavior is unchanged and a non-Unicode root cannot abort a container spawn. |

## 1. Problem (verified at 5203c4e3)

- `src-tauri/src/pty/credentials.rs:42 build_credential_values` computes `local_dir` as `<current_exe parent>/.<stem>` with lossy conversion, ignoring the #1850 resolver, and corrupts non-UTF-8 paths.
- Sole production caller of credential builders: `commands/session.rs:2249` in `create_session_inner_impl` (also reached by the restore path).
- Resolver: `crate::config::config_dir() -> Option<PathBuf>` (`config/mod.rs:954`, cached `InstanceLocation`).

## 2. Design

### 2.1 `pty/credentials.rs`

Imports: `std::ffi::{OsStr, OsString}`, `std::path::{Path, PathBuf}`, and `crate::path_utils::normalize_windows_verbatim_path`. No `crate::config`, Tauri or UI import (AC6; see §3).

```rust
/// #1115 - resolve the exported AGENTSCOMMANDER_LOCAL_DIR. Fallible; call before any launch residue.
pub fn resolve_local_dir(local_root: Option<&Path>) -> Result<String, String> {
    resolve_local_dir_with(
        local_root,
        || std::env::current_exe().ok(),
        |message| log::warn!("{message}"),
    )
}

const LEGACY_LOCAL_DIR_WARNING: &str =
    "[credentials] instance local root unresolved; exporting legacy adjacent AGENTSCOMMANDER_LOCAL_DIR";

fn resolve_local_dir_with(
    local_root: Option<&Path>,
    exe: impl FnOnce() -> Option<PathBuf>,
    warn: impl FnOnce(&str),
) -> Result<String, String>

pub fn build_credential_values(token: &Uuid, cwd: &str, local_dir: String) -> CredentialValues
pub fn build_credentials_env(token: &Uuid, cwd: &str, local_dir: String) -> Vec<(String, String)>
```

Rules for `resolve_local_dir_with`:
1. `Some(path)`: never calls `exe` or `warn` (closure unused → AC3 provable by a panicking closure in tests). `path.to_str()`; `None` → `Err(unicode_error(path))`. Result → `normalize_windows_verbatim_path`. Nothing else (relative stays relative).
2. `None`: `warn(LEGACY_LOCAL_DIR_WARNING)` exactly once, then call `exe()`.
   - stem: `exe.file_stem()` (`&OsStr`), default `OsStr::new("agentscommander")` when exe or stem is absent (today's default).
   - name: `let mut name = OsString::from("."); name.push(stem);`
   - path: `exe.parent()` → `parent.join(&name)`, else `PathBuf::from(&name)`.
   - `path.to_str()`; `None` → `Err(unicode_error(&path))`. Result → `normalize_windows_verbatim_path`.
3. `build_credential_values` keeps `exe`/`binary`/`binary_path` exactly as today (including its own `\\?\` strip and lossy conversions — out of scope) and stores the given `local_dir` verbatim. It no longer derives `local_dir`.

`fn unicode_error(path: &Path) -> String`:
`format!("Cannot start agent session: local root {} is not valid Unicode and cannot be exported as AGENTSCOMMANDER_LOCAL_DIR. Move the AgentsCommander install/config directory to a Unicode path or set AGENTSCOMMANDER_CONFIG_DIR to one.", path.display())`

### 2.2 `commands/session.rs` (`create_session_inner_impl`)

(a) Insert immediately after the `let (agent_id, agent_label) = { ... };` block (ends `:1542`) and before `discover_teams` (`:1546`). At this point only the creation/archive gates have run; the next existing statement already returns with `?` and no cleanup, so an error here leaves no permit, pending row, clock change or spawn mark.

```rust
// #1115 - resolve the exported local root before any permit, pending row or
// clock mutation, so a non-Unicode root fails with zero launch residue.
// Container transport discards extra_env (container_backend `extra_env: _`)
// and maps its own local dir, so only host local PTYs resolve it.
let credential_local_dir = if agent_id.is_some()
    && resolved_spawn
        .as_ref()
        .map(|spawn| SessionBackendKind::from(&spawn.backend))
        .unwrap_or_default()
        == SessionBackendKind::LocalProcess
{
    Some(crate::pty::credentials::resolve_local_dir(
        crate::config::config_dir().as_deref(),
    )?)
} else {
    None
};
```

The predicate is the same expression as `backend_kind` at `:1636`, which becomes `session.backend_kind` via `create_pending_session(..., backend_kind)`.

(b) Replace `:2248-2252`:

```rust
let extra_env = match credential_local_dir {
    Some(local_dir) => {
        crate::pty::credentials::build_credentials_env(&session.token, &cwd, local_dir)
    }
    None => Vec::new(),
};
```

Infallible; no `?` between permit reservation and spawn is added.

### 2.3 Unchanged (byte-for-byte)

`config/` (incl. `agent_local_dir_name()`, `InstanceLocation`), `path_utils.rs`, `pty/docker_runtime.rs`, `pty/container_backend.rs`, `CREDENTIAL_ENV_KEYS`, scrub/apply helpers, generated context text.

## 3. Dependency direction (AC6)

New arc: `pty::credentials → path_utils`. `path_utils.rs` imports only `std::path` (no `crate::`), so it is a leaf and cannot close a cycle; `pty::container_repos` and `pty::terminal_snapshot` already depend on it. Existing arcs `commands::session → config` and `→ pty::credentials` unchanged.
Evidence: `grep -n "crate::" src-tauri/src/path_utils.rs` is empty; `grep -n "crate::" src-tauri/src/pty/credentials.rs` (production part) lists only `crate::path_utils`.

## 4. Tests (Grinch reviews)

In `pty/credentials.rs mod tests` (panicking closures = `|| panic!("exe must not be read")` and `|_| panic!("must not warn")`; recording sink = `let warnings = RefCell::new(Vec::<String>::new());` with `|m| warnings.borrow_mut().push(m.to_string())`):

| # | Test | AC |
|---|---|---|
| T1 | `some_home_root_exported_exactly`: `Some("/home/u/.agentscommander")`, panicking exe → same string | 1, 3 |
| T2 | `some_suffixed_adjacent_exported_exactly`: `Some("/tools/.agentscommander_dev")`, panicking exe → same | 1, 3 |
| T3 | `some_absolute_and_relative_override_exported_verbatim`: `/srv/cfg`, `rel/cfg` → unchanged | 1, 3 |
| T4 | `some_root_never_reads_exe_or_warns`: panicking exe and panicking warn sink for every T1–T3 input | 3, 4 |
| T5a | `#[cfg(windows)] some_verbatim_drive_and_unc_normalized`: `\\?\C:\x\.a` → `C:\x\.a`; `\\?\UNC\srv\share\cfg` → `\\srv\share\cfg` | 1, scope |
| T5b | `#[cfg(not(windows))] some_verbatim_like_path_kept_off_windows`: `\\?\C:\x` → unchanged | 1, scope |
| T6 | `none_uses_legacy_adjacent_and_warns`: exe `/opt/x/agentscommander_s` → result equals `Path::new("/opt/x").join(".agentscommander_s").to_str().unwrap()` (platform separator, no literal); exe `None` → `.agentscommander`; each case with recording sink asserts `warnings == vec![LEGACY_LOCAL_DIR_WARNING]` | 4 |
| T7 | `#[cfg(unix)] some_non_unicode_root_errors`: `OsStr::from_bytes(b"/tmp/\xff")` → `Err` containing `AGENTSCOMMANDER_LOCAL_DIR` | 2 |
| T8 | `#[cfg(windows)] some_non_unicode_root_errors`: `OsString::from_wide(&[0x43,0x3A,0x5C,0xD800])` → `Err` | 2 |
| T9a | `#[cfg(unix)] none_non_unicode_exe_parent_errors`: exe `/tmp/\xff/agentscommander` → `Err` | 4 |
| T9b | `#[cfg(unix)] none_non_unicode_exe_stem_errors`: exe `/opt/x/ac\xff` → `Err` (not U+FFFD) | 4 |
| T10 | `env_contains_expected_keys_and_values`: pass `r"C:\example\.agentscommander".to_string()`; assert `LOCAL_DIR` equals it | 1 |
| T11 | `pty_apply_helper_overrides_stale_credentials_when_extra_env_present`: update call to new signature | — |

In `commands/session.rs mod tests`, following the existing source-order precedent `create_session_inner_marks_spawning_until_spawn_returns` (`:8481`):

| T12 | `create_session_resolves_local_dir_before_launch_residue`: on whitespace-stripped production source assert (i) `crate::pty::credentials::resolve_local_dir(` occurs exactly once; (ii) its index < `resource_monitor.try_reserve_agent_slot(`, < `letspawn_mark={`, < `letpending_result=`; (iii) the extra_env build text contains `build_credentials_env(&session.token,&cwd,local_dir)` with no following `?`, and `None=>Vec::new()`; (iv) the gating text contains `==SessionBackendKind::LocalProcess`; (v) contains `letcredential_local_dir=ifagent_id.is_some()&&` exactly once (dropping the agent gate would leak the token into non-agent shells, contradicting `pty_apply_helper_removes_stale_credentials_when_extra_env_empty`); (vi) contains `resolve_local_dir(crate::config::config_dir().as_deref(),)?` (a `None` argument breaks AC1; replacing `?` with `.unwrap_or_default()` or similar would export `""` and break AC2). On `pty/credentials.rs` stripped production source (before `#[cfg(test)]`): (vii) contains `|message|log::warn!("{message}")`. | F1, F4, 1, 2, 4 |

T1–T4 use `Path::new` fixtures with verbatim output; T6 builds its oracle with `Path::join`; all pass on Linux and Windows. AC1 "path selected by #1850" = whatever `config_dir()` yields; resolver cases stay covered in `config/mod.rs` tests.

AC5 evidence: `git diff 5203c4e3 -- src-tauri/src/pty/docker_runtime.rs src-tauri/src/pty/container_backend.rs src-tauri/src/config/ src-tauri/src/path_utils.rs` is empty.

## 5. Delivery gates

Scope: only `src-tauri/src/pty/credentials.rs`, `src-tauri/src/commands/session.rs`. `git diff --name-only 5203c4e3` lists nothing else (plan is gitignored).

Local (dev, before PR; stop and report on any failure):
```
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib pty::credentials -- --nocapture
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib commands::session::tests::create_session_resolves_local_dir_before_launch_residue -- --nocapture
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib -- --nocapture
```
Env risk (dev states in report): Linux host cannot run T5a or T8; they compile/run only in Windows CI, which is the authoritative evidence for them on the exact PR head. T5b/T7/T9a/T9b run locally.

Git: commits only on the named branch in `repo-AgentsCommander`; no force-push; no edits outside scope. Recovery: `git restore` of the two files returns to base.
Drift: before first mutation and before PR, fetch `origin/main`; re-verify only if drift touches the two files, `path_utils.rs`, `config/mod.rs::config_dir`, `resource_monitor/registry.rs` permits, toolchain or workflows.
CI: all required checks green on exact PR-head SHA. Release: ships atomically with #1850/#1841/#1118 (tech-lead owned).

## 6. Out of scope

Root selection/migration (#1850), npm upgrade (#1841), new variables/placeholders, container mapping, `%AC_REPLICA_ROOT%`/`%AC_MATRIX_ROOT%`, lossy `BINARY`/`BINARY_PATH`, the pre-existing post-permit `?` leaks `set_pending_effective_shell_args(...)?` at `:2243` and `set_pending_start_fresh_on_restore(...)?` at `:1766` and `:1806` (do not release the permit; tracked in #2093, not worsened here).
