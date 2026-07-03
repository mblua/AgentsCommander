//! #598 - seed a template config folder (e.g. `.claude`) into a coding-agent
//! replica at spawn time.
//!
//! Two phases:
//! - [`resolve_config_seed`] is PURE path math (no filesystem) and fail-soft:
//!   it never returns an error that aborts a launch (M7).
//! - [`perform_config_seed`] is the side-effecting, best-effort, Windows-safe
//!   rename-to-trash-first atomic swap (H1): the destination is always either
//!   fully-old or fully-new, never half-written. It never aborts the spawn.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::placeholders::{
    expand_placeholders_in_content, PlaceholderContext, PlaceholderRootKind,
};
use crate::config::settings::{validate_config_seed_dest, ConfigSeedConfig};
use crate::session::profile::CodingAgentKind;

/// Files at or below this size are read so the 3 AC tokens can be substituted in
/// their CONTENT. Larger files are streamed via `std::fs::copy` (M4), never read
/// whole into memory.
const CONTENT_SUBSTITUTION_CAP: u64 = 5 * 1024 * 1024; // 5 MiB

/// Recursion-depth bound for the template tree (M6). Defends against a
/// pathological or cyclic-via-junction layout; junction/symlink entries are
/// already skipped, so this is a belt-and-braces safety net.
const MAX_SEED_DEPTH: u32 = 64;

/// Source tiers, highest precedence first. All workspace tiers outrank all
/// matrix tiers; within a location the profile-lettered folder outranks the
/// base folder. The matrix's bare `<dest>` (e.g. `_agent_<role>/.claude`) is
/// deliberately NOT a tier: that is the agent's OWN live config (the matrix dir
/// is itself a launch root), not a template, so seeding from it would copy real
/// config and clobber it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSeedTier {
    WorkspaceProfile,
    WorkspaceBase,
    MatrixProfile,
    MatrixBase,
    /// #769 Phase 2 - the shipped default config-folder master at
    /// `<config_dir>/coding-agents/_seed/<dest>/`. Appended LAST (lowest
    /// precedence) by the spawn caller. Unlike the four tiers above, this one is
    /// ABSENT-ONLY and NON-EMPTY-GATED in `perform_config_seed`: it fills
    /// `%AC_REPLICA_ROOT%/<dest>` only when the dest does not already exist AND the
    /// master has at least one regular file, so it can never overwrite accumulated
    /// replica config, and an empty/absent master reads as "tier not present"
    /// (today's spawn behavior stays byte-identical).
    CatalogDefault,
}

/// After a successful seed of `.claude`, re-stamp the AC-managed
/// `.claude/settings.local.json` (M1). Presence ALSO signals "hold the sweep
/// lock around the seed + re-apply" (M2). `None` for any non-`.claude` dest,
/// because `ensure_rtk_pretool_hook` hardcodes the `.claude` subdir (the
/// `.claude-amp` limitation is documented in the plan §8.2, follow-up F4).
#[derive(Debug, Clone)]
pub struct ClaudeSettingsReapply {
    /// Global `inject_rtk_hook` toggle, sampled at build time.
    pub inject_rtk_hook: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfigSeed {
    /// Candidate source folders, highest precedence first: workspace profile,
    /// workspace base, matrix profile, matrix base. Pure path math; no
    /// filesystem touched at resolve time.
    pub candidates: Vec<(ConfigSeedTier, PathBuf)>,
    /// Absolute destination, under the replica root.
    pub dest: PathBuf,
    /// Carried for CONTENT substitution at copy time.
    pub context: PlaceholderContext,
    /// Heuristic, log-only warning about a dest vs config-dir-env mismatch
    /// (computed at build time; emitted once at execution).
    pub config_dir_warning: Option<String>,
    /// Set only when dest is `.claude` (drives the M1 re-apply + M2 lock).
    pub claude_settings_reapply: Option<ClaudeSettingsReapply>,
}

/// Outcome of [`perform_config_seed`]. Returned for testing and to gate the
/// post-seed `.claude` re-apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSeedReport {
    Seeded,
    Skipped,
    Failed,
}

/// Resolve the seed candidates + destination. Returns `None` (and warns/debug-
/// logs) on any problem; NEVER returns an error that aborts the launch (M7).
pub fn resolve_config_seed(
    cfg: &ConfigSeedConfig,
    effective_letter: &str,
    context: Option<&PlaceholderContext>,
) -> Option<ResolvedConfigSeed> {
    let context = context?;
    if context.root_kind != PlaceholderRootKind::AcReplicaOrRootAgent {
        log::debug!("[config-seed] launch root is not an AC replica/root-agent; skipping seed");
        return None;
    }
    let dest_name = cfg.dest.trim();
    if let Err(e) = validate_config_seed_dest(dest_name) {
        // M7: a settings.json that bypassed save-time validation can still carry
        // a bad dest. Fail soft rather than abort the launch.
        log::warn!(
            "[config-seed] invalid dest '{}', skipping seed: {}",
            cfg.dest,
            e
        );
        return None;
    }
    // The user's example uses a lowercase profile letter; effective_profile is UPPERCASE.
    let letter = effective_letter.to_ascii_lowercase();

    // Precedence: all workspace candidates beat all matrix candidates; within a
    // location, the profile-lettered folder beats the base folder. The matrix's
    // bare `<dest>` is intentionally never a source (it is the agent's own live
    // config; see ConfigSeedTier), so the matrix tiers mirror the workspace
    // naming (`default_profile_<l><dest>` / `default<dest>`) instead.
    let mut candidates: Vec<(ConfigSeedTier, PathBuf)> = Vec::new();
    if let Some(ws) = context.workspace_root.as_ref() {
        candidates.push((
            ConfigSeedTier::WorkspaceProfile,
            ws.join(format!("default_profile_{}{}", letter, dest_name)),
        ));
        candidates.push((
            ConfigSeedTier::WorkspaceBase,
            ws.join(format!("default{}", dest_name)),
        ));
    }
    if let Some(mx) = context.matrix_root.as_ref() {
        candidates.push((
            ConfigSeedTier::MatrixProfile,
            mx.join(format!("default_profile_{}{}", letter, dest_name)),
        ));
        candidates.push((
            ConfigSeedTier::MatrixBase,
            mx.join(format!("default{}", dest_name)),
        ));
    }
    if candidates.is_empty() {
        return None;
    }

    Some(ResolvedConfigSeed {
        candidates,
        dest: context.replica_root.join(dest_name),
        context: context.clone(),
        config_dir_warning: None,
        claude_settings_reapply: None,
    })
}

/// Heuristic, log-only warning when the seed dest does not line up with the
/// coding agent's configured config-dir env (so the tool would read its config
/// from elsewhere), OR when it EXACTLY matches it (L1: the seed overwrites the
/// tool's ACTIVE config dir every spawn). Never blocks; may warn spuriously if a
/// config-dir env is injected by a path this does not inspect.
///
/// Receives the real launch `shell`/`shell_args` (the agent command) so the
/// coding-agent kind is detected from the actual command.
pub fn compute_config_dir_warning(
    dest: &Path,
    shell: &str,
    shell_args: &[String],
    agent_env: &BTreeMap<String, String>,
    profile_env: &BTreeMap<String, String>,
    effective_codex_home: Option<&Path>,
) -> Option<String> {
    let key = match CodingAgentKind::detect(shell, shell_args) {
        Some(CodingAgentKind::Claude) => "CLAUDE_CONFIG_DIR",
        Some(CodingAgentKind::Codex) => "CODEX_HOME",
        // Gemini has no AC-managed config-dir env to compare against.
        Some(CodingAgentKind::Gemini) => return None,
        None => {
            if crate::config::agent_command::command_runs_opencode(shell, shell_args) {
                "OPENCODE_CONFIG_DIR"
            } else {
                return None;
            }
        }
    };

    let dest_seg = dest.file_name().and_then(|s| s.to_str())?;

    // CODEX_HOME already went through the full resolver (isolation/profile/agent/
    // inherited); prefer its resolved value. Others: profile over agent layer.
    let configured: Option<String> = if key == "CODEX_HOME" {
        effective_codex_home
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| lookup_env_ci(profile_env, agent_env, key).cloned())
    } else {
        lookup_env_ci(profile_env, agent_env, key).cloned()
    };

    match configured {
        Some(value) => {
            let matches = final_segment(&value)
                .as_deref()
                .map(|seg| segments_eq(seg, dest_seg))
                .unwrap_or(false);
            if matches {
                // L1 case 2: seed overwrites the tool's ACTIVE config dir.
                Some(format!(
                    "config seed destination '{}' is the tool's active {}='{}'; it is overwritten on every spawn, so any credentials or session state living there are reset each launch (ensure the template is self-sufficient)",
                    dest_seg, key, value
                ))
            } else {
                // Case 1: dest does not match the configured config-dir env.
                Some(format!(
                    "config seed destination '{}' does not match {}='{}'; the tool will read its config from there, not the seeded folder",
                    dest_seg, key, value
                ))
            }
        }
        // Case 1 (unset but expected for this tool).
        None => Some(format!(
            "config seed destination '{}' is set but {} is not configured; the tool may ignore the seeded folder",
            dest_seg, key
        )),
    }
}

/// Case-insensitive env lookup (Windows env keys are case-insensitive, and the
/// stored key casing is user-controlled), profile layer over agent layer. The
/// plan's illustrative `.get(key)` would miss a lowercase-keyed value; this
/// mirrors `agent_command::find_env_value`.
fn lookup_env_ci<'a>(
    profile_env: &'a BTreeMap<String, String>,
    agent_env: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a String> {
    let want = key.to_ascii_uppercase();
    profile_env
        .iter()
        .find(|(k, _)| k.to_ascii_uppercase() == want)
        .or_else(|| agent_env.iter().find(|(k, _)| k.to_ascii_uppercase() == want))
        .map(|(_, v)| v)
}

/// Final path segment of a configured value, trailing separators trimmed.
fn final_segment(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn segments_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Best-effort, Windows-safe seed execution (H1). NEVER aborts the spawn.
/// `unique_sfx` (the session id) keeps the temp/trash leaves unique per spawn
/// (M3) and short (I2/MAX_PATH).
///
/// CONCURRENCY CONTRACT (grinch HIGH-1): callers MUST serialize this for the
/// same replica, because step 2 sweeps ALL `<dest>.acseed-*` scratch by prefix
/// (see [`clear_stale_seed_scratch`]) and would otherwise delete a concurrent
/// spawn's in-flight temp/trash mid-swap. The only caller, the session spawn
/// chokepoint, holds `RtkSweepLockState` across this call for all dests.
pub fn perform_config_seed(seed: &ResolvedConfigSeed, unique_sfx: &str) -> ConfigSeedReport {
    // 1. Pick the winning tier by source-folder presence (highest precedence first).
    //    The four legacy tiers are unchanged (readable dir wins and overwrites).
    //    The #769 P2 CatalogDefault tier is gated ABSENT-ONLY + NON-EMPTY here, so
    //    it never overwrites an existing replica config and an empty/absent master
    //    reads as "not present".
    let Some((tier, src)) = seed.candidates.iter().find(|(tier, src)| match tier {
        ConfigSeedTier::CatalogDefault => !seed.dest.exists() && is_nonempty_seed_dir(src),
        _ => is_readable_dir(src),
    }) else {
        // #598: seeding is active but no template exists at any tier. This is a
        // benign no-op (nothing to copy), but a SILENT one confused users who
        // enabled the feature, created no template, and saw nothing happen. Log
        // it at info with the exact candidate paths so they know WHERE to create
        // a template. Only fires here, at the real spawn chokepoint (callers gate
        // on a resolved seed), never during prevalidation/delivery builds.
        let replica_label = seed
            .context
            .replica_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        let looked_in = seed
            .candidates
            .iter()
            .map(|(t, p)| format!("{:?}={}", t, p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        log::info!(
            "[config-seed] active for replica '{}' dest '{}' but no source template found at any tier; skipped (looked for: {})",
            replica_label,
            seed.dest.display(),
            looked_in
        );
        return ConfigSeedReport::Skipped;
    };

    let (Some(parent), Some(dest_name)) = (
        seed.dest.parent(),
        seed.dest.file_name().and_then(|s| s.to_str()),
    ) else {
        log::warn!(
            "[config-seed] dest {} has no parent/name; skipping",
            seed.dest.display()
        );
        return ConfigSeedReport::Skipped;
    };

    // M3: unique per spawn. Same-volume siblings of dest, so renames are atomic.
    let temp = parent.join(format!("{}.acseed-tmp-{}", dest_name, unique_sfx));
    let trash = parent.join(format!("{}.acseed-old-{}", dest_name, unique_sfx));

    // 2. Reclaim ALL stale seed scratch for this dest (any session id), not just
    //    this spawn's. A trash dir whose removal failed earlier (locked by a
    //    lingering handle) carries a DIFFERENT session id, so a per-suffix
    //    cleanup would leak it forever; sweep by prefix so it is reclaimed on the
    //    first run after the lock releases.
    clear_stale_seed_scratch(parent, dest_name);

    // 3. Stage the template into temp. On any error the dest is untouched.
    //    master->replica copy substitutes the 3 AC tokens (substitute = true).
    if let Err(e) = copy_tree(src, &temp, 0, Some(&seed.context), true) {
        log::warn!(
            "[config-seed] failed to stage template {} -> {}: {}",
            src.display(),
            temp.display(),
            e
        );
        let _ = std::fs::remove_dir_all(&temp);
        return ConfigSeedReport::Failed;
    }

    // 4. Move the existing dest aside (atomic). If it is locked by a live
    //    process, keep the existing config and SKIP (dest stays fully intact).
    if seed.dest.exists() {
        if let Err(e) = std::fs::rename(&seed.dest, &trash) {
            log::warn!(
                "[config-seed] dest {} is in use ({}); keeping existing config, skipping this seed",
                seed.dest.display(),
                e
            );
            let _ = std::fs::remove_dir_all(&temp);
            return ConfigSeedReport::Skipped;
        }
    }

    // 5. Install the freshly staged temp (atomic).
    if let Err(e) = std::fs::rename(&temp, &seed.dest) {
        log::warn!(
            "[config-seed] failed to install seed at {}: {}",
            seed.dest.display(),
            e
        );
        // Restore the old config if we moved it aside and the dest is now gone.
        if !seed.dest.exists() && trash.exists() {
            if let Err(re) = std::fs::rename(&trash, &seed.dest) {
                log::warn!(
                    "[config-seed] failed to restore previous config at {}: {}",
                    seed.dest.display(),
                    re
                );
            }
        }
        let _ = std::fs::remove_dir_all(&temp);
        return ConfigSeedReport::Failed;
    }

    // 6. Drop the old config (ignore if locked; it lingers, cleared next run).
    let _ = std::fs::remove_dir_all(&trash);

    // 7. Emit the deferred config-dir warning + the success line.
    if let Some(w) = &seed.config_dir_warning {
        log::warn!("[config-seed] {}", w);
    }
    log::info!(
        "[config-seed] seeded '{}' into replica from {:?} source '{}'",
        seed.dest.display(),
        tier,
        src.display()
    );
    ConfigSeedReport::Seeded
}

/// True iff `path` is a readable directory. Follows symlinks on the ROOT (the
/// template folder itself may be a real or symlinked dir); only ENTRIES are
/// filtered for reparse points during the copy (see [`is_symlink_or_reparse`]).
fn is_readable_dir(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// #769 P2 - true iff `path` is a readable directory containing at least one
/// regular, non-reparse FILE (searched recursively). The CatalogDefault tier uses
/// this so an empty master (or a dir of only subdirs/links) reads as "not present"
/// and the tier stays inert, keeping today's spawn behavior byte-identical.
pub(crate) fn is_nonempty_seed_dir(path: &Path) -> bool {
    fn has_file(dir: &Path, depth: u32) -> bool {
        if depth > MAX_SEED_DEPTH {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if is_symlink_or_reparse(&entry) {
                continue;
            }
            match entry.file_type() {
                Ok(ft) if ft.is_file() => return true,
                // A directory counts only if it (recursively) contains a regular
                // file; a dir with no files falls through to `_` (== "keep
                // scanning"), so an empty dir and a dir-of-empty-dirs both yield
                // false, exactly as before the collapse.
                Ok(ft) if ft.is_dir() && has_file(&entry.path(), depth + 1) => return true,
                _ => {}
            }
        }
        false
    }
    is_readable_dir(path) && has_file(path, 0)
}

/// Remove every leftover seed-scratch sibling for `dest_name` (any session-id
/// suffix), best-effort. Reclaims a temp/trash dir leaked by an earlier locked
/// removal; the current spawn's `temp`/`trash` (same prefix) are cleared too.
///
/// Sweeps across ALL session ids, so it is SAFE only when the caller has
/// serialized seeding for this replica (see [`perform_config_seed`]'s
/// concurrency contract); otherwise it can delete a concurrent spawn's in-flight
/// scratch (grinch HIGH-1).
fn clear_stale_seed_scratch(parent: &Path, dest_name: &str) {
    let tmp_prefix = format!("{}.acseed-tmp-", dest_name);
    let old_prefix = format!("{}.acseed-old-", dest_name);
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with(&tmp_prefix) || name.starts_with(&old_prefix) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

pub(crate) fn copy_tree(
    src_dir: &Path,
    dst_dir: &Path,
    depth: u32,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
) -> Result<(), String> {
    if depth > MAX_SEED_DEPTH {
        log::warn!(
            "[config-seed] template tree deeper than {} levels at {}; truncating",
            MAX_SEED_DEPTH,
            src_dir.display()
        );
        return Ok(());
    }
    std::fs::create_dir_all(dst_dir).map_err(|e| format!("create {}: {}", dst_dir.display(), e))?;
    let entries =
        std::fs::read_dir(src_dir).map_err(|e| format!("read {}: {}", src_dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry in {}: {}", src_dir.display(), e))?;
        // M6/I3: skip symlinks AND Windows junctions/reparse points (warn, not debug).
        if is_symlink_or_reparse(&entry) {
            log::warn!(
                "[config-seed] skipping symlink/reparse entry {}",
                entry.path().display()
            );
            continue;
        }
        let ft = entry
            .file_type()
            .map_err(|e| format!("file type of {}: {}", entry.path().display(), e))?;
        let dst = dst_dir.join(entry.file_name());
        if ft.is_dir() {
            copy_tree(&entry.path(), &dst, depth + 1, ctx, substitute)?;
        } else if ft.is_file() {
            copy_file_substituted(&entry.path(), &dst, ctx, substitute)?;
        }
        // Anything else (already-filtered symlinks/reparse) is ignored.
    }
    Ok(())
}

fn copy_file_substituted(
    src: &Path,
    dst: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
) -> Result<(), String> {
    // #769 P2: verbatim stream-copy when substitution is off (embedded->master and
    // the reseed backup keep raw %AC_% tokens intact) or when no context is given.
    let ctx = match (substitute, ctx) {
        (true, Some(ctx)) => ctx,
        _ => {
            return std::fs::copy(src, dst)
                .map(|_| ())
                .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e));
        }
    };
    let len = std::fs::metadata(src)
        .map_err(|e| format!("stat {}: {}", src.display(), e))?
        .len();
    // M4: over-cap files are streamed, never read whole into memory.
    if len > CONTENT_SUBSTITUTION_CAP {
        std::fs::copy(src, dst)
            .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e))?;
        return Ok(());
    }
    let bytes = std::fs::read(src).map_err(|e| format!("read {}: {}", src.display(), e))?;
    // Binary sniff: a NUL in the leading window means "copy verbatim".
    let probe = bytes.len().min(8192);
    if bytes[..probe].contains(&0u8) {
        return std::fs::write(dst, &bytes).map_err(|e| format!("write {}: {}", dst.display(), e));
    }
    match std::str::from_utf8(&bytes) {
        // CRLF and a UTF-8 BOM survive (only known token substrings are swapped).
        Ok(text) => std::fs::write(dst, expand_placeholders_in_content(text, ctx).as_bytes())
            .map_err(|e| format!("write {}: {}", dst.display(), e)),
        // Non-UTF-8 (no NUL): copy verbatim.
        Err(_) => std::fs::write(dst, &bytes).map_err(|e| format!("write {}: {}", dst.display(), e)),
    }
}

/// True for a symlink OR a Windows junction/reparse point. `is_symlink()` alone
/// is FALSE for junctions, so the reparse-attribute check is required (M6).
/// `DirEntry::metadata` does NOT traverse, so the junction's own attributes are
/// observed. An undeterminable file type is treated as skip-worthy.
fn is_symlink_or_reparse(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(ft) if ft.is_symlink() => return true,
        Ok(_) => {}
        Err(_) => return true,
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if let Ok(md) = entry.metadata() {
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(
        replica: &Path,
        workspace: Option<&Path>,
        matrix: Option<&Path>,
    ) -> PlaceholderContext {
        PlaceholderContext {
            replica_root: replica.to_path_buf(),
            root_kind: PlaceholderRootKind::AcReplicaOrRootAgent,
            workspace_root: workspace.map(|p| p.to_path_buf()),
            matrix_root: matrix.map(|p| p.to_path_buf()),
        }
    }

    fn seed_cfg(dest: &str) -> ConfigSeedConfig {
        ConfigSeedConfig {
            enabled: true,
            dest: dest.to_string(),
        }
    }

    fn write_file(path: &Path, content: &[u8]) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn abs(win: &str, unix: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { win } else { unix })
    }

    // ---- resolve_config_seed (pure path math, fail-soft) -------------------

    #[test]
    fn resolve_orders_workspace_then_matrix_each_profile_then_base_lowercased() {
        let workspace = abs(r"C:\ws", "/ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        let matrix = workspace.join("_agent_x");
        let ctx = ctx_with(&replica, Some(&workspace), Some(&matrix));

        let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx)).expect("some");
        // All workspace tiers outrank all matrix tiers; profile beats base in each.
        assert_eq!(resolved.candidates.len(), 4);
        assert_eq!(resolved.candidates[0].0, ConfigSeedTier::WorkspaceProfile);
        assert_eq!(
            resolved.candidates[0].1,
            workspace.join("default_profile_c.claude")
        );
        assert_eq!(resolved.candidates[1].0, ConfigSeedTier::WorkspaceBase);
        assert_eq!(resolved.candidates[1].1, workspace.join("default.claude"));
        assert_eq!(resolved.candidates[2].0, ConfigSeedTier::MatrixProfile);
        assert_eq!(
            resolved.candidates[2].1,
            matrix.join("default_profile_c.claude")
        );
        assert_eq!(resolved.candidates[3].0, ConfigSeedTier::MatrixBase);
        assert_eq!(resolved.candidates[3].1, matrix.join("default.claude"));
        assert_eq!(resolved.dest, replica.join(".claude"));
        // Regression guard: the matrix's bare `.claude` (the agent's own live
        // config) must NEVER be a source candidate.
        assert!(
            !resolved
                .candidates
                .iter()
                .any(|(_, p)| *p == matrix.join(".claude")),
            "bare matrix .claude must not be a seed source"
        );
    }

    #[test]
    fn resolve_omits_matrix_when_absent() {
        let workspace = abs(r"C:\ws", "/ws");
        let replica = workspace.join("ac-root-agent");
        let ctx = ctx_with(&replica, Some(&workspace), None);

        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).expect("some");
        // Root-agent: matrix is None, so only the two workspace tiers remain.
        let tiers: Vec<_> = resolved.candidates.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            tiers,
            vec![ConfigSeedTier::WorkspaceProfile, ConfigSeedTier::WorkspaceBase]
        );
    }

    #[test]
    fn resolve_is_none_for_non_replica_root() {
        let ctx = PlaceholderContext {
            replica_root: abs(r"C:\cwd", "/cwd"),
            root_kind: PlaceholderRootKind::NormalLaunchCwd,
            workspace_root: Some(abs(r"C:\ws", "/ws")),
            matrix_root: None,
        };
        assert!(resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).is_none());
    }

    #[test]
    fn resolve_is_none_for_bad_dest_failsoft() {
        let workspace = abs(r"C:\ws", "/ws");
        let replica = workspace.join("wg-1").join("__agent_x");
        let ctx = ctx_with(&replica, Some(&workspace), None);
        // A dest that bypassed save validation must fail soft (None), not abort.
        assert!(resolve_config_seed(&seed_cfg("../escape"), "A", Some(&ctx)).is_none());
        assert!(resolve_config_seed(&seed_cfg("a/b"), "A", Some(&ctx)).is_none());
    }

    #[test]
    fn resolve_is_none_without_context() {
        assert!(resolve_config_seed(&seed_cfg(".claude"), "A", None).is_none());
    }

    // ---- perform_config_seed (trash-first atomic swap) ---------------------

    #[test]
    fn perform_seeds_from_highest_present_tier_and_clean_replaces_dest() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        // Base AND Profile templates both present; Profile must win.
        write_file(&workspace.join("default.claude").join("f.txt"), b"BASE");
        write_file(
            &workspace.join("default_profile_c.claude").join("f.txt"),
            b"PROFILE",
        );
        // Pre-existing dest content that must be cleanly replaced.
        write_file(&replica.join(".claude").join("stale.txt"), b"OLD");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "C", Some(&ctx)).unwrap();
        assert_eq!(
            perform_config_seed(&resolved, "sfx1"),
            ConfigSeedReport::Seeded
        );

        assert_eq!(
            std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
            b"PROFILE"
        );
        // Clean replace: the prior file is gone.
        assert!(!replica.join(".claude").join("stale.txt").exists());
        // No temp/trash residue.
        assert!(!replica.join(".claude.acseed-tmp-sfx1").exists());
        assert!(!replica.join(".claude.acseed-old-sfx1").exists());
    }

    #[test]
    fn perform_skips_and_preserves_dest_when_no_template_present() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&replica.join(".claude").join("keep.txt"), b"KEEP");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        assert_eq!(perform_config_seed(&resolved, "s"), ConfigSeedReport::Skipped);
        // Dest fully intact.
        assert_eq!(
            std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
            b"KEEP"
        );
    }

    #[test]
    fn perform_failed_copy_leaves_dest_intact() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&workspace.join("default.claude").join("f.txt"), b"NEW");
        write_file(&replica.join(".claude").join("keep.txt"), b"OLD");
        // Fault injection: a FILE where the temp DIR would be created makes
        // copy_tree's create_dir_all fail, so the swap never touches dest.
        write_file(&replica.join(".claude.acseed-tmp-sfx"), b"blocker");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        assert_eq!(
            perform_config_seed(&resolved, "sfx"),
            ConfigSeedReport::Failed
        );
        // Dest fully intact (old content kept).
        assert_eq!(
            std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
            b"OLD"
        );
    }

    /// H1 "dest in use" branch: when the existing dest is locked by a live
    /// process, the dest->trash rename fails and the seed is SKIPPED with the
    /// existing config kept fully intact. Windows-specific (Unix renames a
    /// directory even with open handles inside it), which is the whole reason H1
    /// exists: the prior process still holding handles in `.claude`.
    #[cfg(windows)]
    #[test]
    fn perform_skips_and_preserves_dest_when_dest_is_locked() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&workspace.join("default.claude").join("f.txt"), b"NEW");
        write_file(&replica.join(".claude").join("keep.txt"), b"OLD");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();

        // Hold an open handle to a file inside dest: Windows then refuses to
        // rename the dest directory (File::open grants no FILE_SHARE_DELETE),
        // forcing the dest->trash rename to fail -> skip, dest untouched.
        let locked = std::fs::File::open(replica.join(".claude").join("keep.txt")).unwrap();
        let report = perform_config_seed(&resolved, "sfx");
        drop(locked);

        assert_eq!(report, ConfigSeedReport::Skipped);
        // Existing config fully intact.
        assert_eq!(
            std::fs::read(replica.join(".claude").join("keep.txt")).unwrap(),
            b"OLD"
        );
        // Staged temp was cleaned up.
        assert!(!replica.join(".claude.acseed-tmp-sfx").exists());
    }

    #[test]
    fn perform_clears_stale_temp_and_trash_from_prior_run() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&workspace.join("default.claude").join("f.txt"), b"NEW");
        // Stale dirs from a crashed prior run.
        write_file(
            &replica.join(".claude.acseed-tmp-sfx").join("junk.txt"),
            b"stale",
        );
        write_file(
            &replica.join(".claude.acseed-old-sfx").join("junk.txt"),
            b"stale",
        );

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        assert_eq!(
            perform_config_seed(&resolved, "sfx"),
            ConfigSeedReport::Seeded
        );
        assert_eq!(
            std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
            b"NEW"
        );
        assert!(!replica.join(".claude.acseed-tmp-sfx").exists());
        assert!(!replica.join(".claude.acseed-old-sfx").exists());
    }

    #[test]
    fn perform_reclaims_leaked_scratch_from_other_session_ids() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&workspace.join("default.claude").join("f.txt"), b"NEW");
        // Scratch leaked by a prior run whose removal was locked — note the
        // DIFFERENT session id. A per-suffix cleanup would never reclaim these.
        write_file(
            &replica.join(".claude.acseed-old-OLDID").join("junk.txt"),
            b"stale",
        );
        write_file(
            &replica.join(".claude.acseed-tmp-OLDID").join("junk.txt"),
            b"stale",
        );

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        assert_eq!(
            perform_config_seed(&resolved, "NEWID"),
            ConfigSeedReport::Seeded
        );
        assert!(!replica.join(".claude.acseed-old-OLDID").exists());
        assert!(!replica.join(".claude.acseed-tmp-OLDID").exists());
    }

    /// grinch HIGH-1 characterization: proves WHY `perform_config_seed` must be
    /// serialized for all dests. We reconstruct the exact mid-swap state of a
    /// concurrent spawn A (dest already renamed to trash_A, staged temp_A
    /// present) and run what spawn B's step-2 cleanup does for the SAME dest with
    /// a DIFFERENT session id. The prefix-sweep deletes BOTH of A's in-flight
    /// dirs, after which A can neither install (temp gone) nor restore (trash
    /// gone) -> config lost. This is the data-loss the session chokepoint's
    /// RtkSweepLockState serialization (held across the seed for ALL dests)
    /// prevents; a regression that drops or narrows that lock re-opens it.
    #[test]
    fn unserialized_prefix_sweep_would_destroy_a_concurrent_inflight_swap() {
        let temp = tempfile::tempdir().unwrap();
        let replica = temp.path().join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();

        // Spawn A is mid-swap on dest ".claude-amp" (the previously-unlocked path):
        // dest has been renamed to trash_A (its ONLY surviving old copy) and
        // temp_A holds the freshly staged new config.
        let trash_a = replica.join(".claude-amp.acseed-old-idA");
        let temp_a = replica.join(".claude-amp.acseed-tmp-idA");
        write_file(&trash_a.join("keep.txt"), b"OLD");
        write_file(&temp_a.join("f.txt"), b"NEW");
        assert!(!replica.join(".claude-amp").exists(), "A moved dest aside");

        // Spawn B (different session id) runs its step-2 prefix-sweep concurrently.
        clear_stale_seed_scratch(&replica, ".claude-amp");

        // Hazard: B deleted BOTH of A's in-flight dirs. A's later
        // rename(temp_A->dest) and restore(trash_A->dest) would both fail,
        // leaving the replica with NO .claude-amp -> the exact data loss the
        // chokepoint lock serializes against.
        assert!(!temp_a.exists(), "prefix-sweep deletes a concurrent spawn's staged temp");
        assert!(!trash_a.exists(), "prefix-sweep deletes a concurrent spawn's only old copy");
    }

    // ---- CatalogDefault tier (#769 P2: absent-only + non-empty) ------------

    /// Append the absent-only CatalogDefault candidate LAST, exactly as the spawn
    /// caller (`build_agent_spawn_command`) does.
    fn with_catalog_default(mut resolved: ResolvedConfigSeed, master: &Path) -> ResolvedConfigSeed {
        resolved
            .candidates
            .push((ConfigSeedTier::CatalogDefault, master.to_path_buf()));
        resolved
    }

    #[test]
    fn catalog_default_fills_absent_dest_from_nonempty_master() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        // No workspace/matrix templates on disk: the 4 legacy tiers are absent.
        let master = temp.path().join("master");
        write_file(&master.join("settings.json"), b"{\"seeded\":true}");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        let resolved = with_catalog_default(resolved, &master);

        // dest absent + master non-empty -> CatalogDefault wins and fills.
        assert_eq!(
            perform_config_seed(&resolved, "sfx"),
            ConfigSeedReport::Seeded
        );
        assert_eq!(
            std::fs::read(replica.join(".claude").join("settings.json")).unwrap(),
            b"{\"seeded\":true}"
        );
    }

    #[test]
    fn catalog_default_absent_only_never_overwrites_existing_dest() {
        // The E1/G1 data-loss killer: a present dest is preserved, tier skips.
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        // Live replica config the seed must never touch.
        write_file(&replica.join(".claude").join("creds.json"), b"SECRET");
        let master = temp.path().join("master");
        write_file(&master.join("settings.json"), b"{\"seeded\":true}");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        let resolved = with_catalog_default(resolved, &master);

        // dest present -> CatalogDefault gate fails -> Skipped, dest fully intact.
        assert_eq!(perform_config_seed(&resolved, "sfx"), ConfigSeedReport::Skipped);
        assert_eq!(
            std::fs::read(replica.join(".claude").join("creds.json")).unwrap(),
            b"SECRET"
        );
        // The master content did NOT leak in.
        assert!(!replica.join(".claude").join("settings.json").exists());
    }

    #[test]
    fn catalog_default_empty_master_is_inert() {
        // Non-empty gate: an empty master reads as "not present" -> Skipped, and
        // (dest absent) nothing is created, so spawn behavior is byte-identical.
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        let master = temp.path().join("master");
        std::fs::create_dir_all(&master).unwrap(); // exists but EMPTY

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        let resolved = with_catalog_default(resolved, &master);

        assert_eq!(perform_config_seed(&resolved, "sfx"), ConfigSeedReport::Skipped);
        assert!(!replica.join(".claude").exists());
    }

    #[test]
    fn catalog_default_yields_to_a_present_legacy_tier() {
        // A present workspace tier still wins (unchanged behavior); CatalogDefault
        // is only reached when all 4 legacy tiers are absent.
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("ws");
        let replica = workspace.join("wg-1-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        write_file(&workspace.join("default.claude").join("f.txt"), b"WORKSPACE");
        let master = temp.path().join("master");
        write_file(&master.join("f.txt"), b"CATALOG");

        let ctx = ctx_with(&replica, Some(&workspace), None);
        let resolved = resolve_config_seed(&seed_cfg(".claude"), "A", Some(&ctx)).unwrap();
        let resolved = with_catalog_default(resolved, &master);

        assert_eq!(perform_config_seed(&resolved, "sfx"), ConfigSeedReport::Seeded);
        assert_eq!(
            std::fs::read(replica.join(".claude").join("f.txt")).unwrap(),
            b"WORKSPACE"
        );
    }

    #[test]
    fn copy_tree_verbatim_preserves_tokens_when_substitute_false() {
        // The reseed backup copies the master verbatim (raw %AC_% tokens intact).
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("a.txt"), b"path=%AC_REPLICA_ROOT%/x");
        let dst = temp.path().join("dst");
        copy_tree(&src, &dst, 0, None, false).unwrap();
        assert_eq!(
            std::fs::read(dst.join("a.txt")).unwrap(),
            b"path=%AC_REPLICA_ROOT%/x"
        );
    }

    // ---- copy_file_substituted (size-first, binary/UTF-8) ------------------

    #[test]
    fn copy_file_substitutes_utf8_tokens_forward_slashed() {
        let temp = tempfile::tempdir().unwrap();
        let replica = abs(r"C:\replica\root", "/replica/root");
        let ctx = ctx_with(&replica, Some(&abs(r"C:\ws", "/ws")), None);

        let src = temp.path().join("a.txt");
        std::fs::write(&src, b"path=%AC_REPLICA_ROOT%/x").unwrap();
        let dst = temp.path().join("a.out");
        copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

        let expected = replica.to_string_lossy().replace('\\', "/");
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            format!("path={}/x", expected)
        );
    }

    #[test]
    fn copy_file_preserves_binary_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

        let src = temp.path().join("b.bin");
        let raw: &[u8] = b"%AC_REPLICA_ROOT%\x00binary";
        std::fs::write(&src, raw).unwrap();
        let dst = temp.path().join("b.out");
        copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();
        // NUL present -> verbatim, no token substitution.
        assert_eq!(std::fs::read(&dst).unwrap(), raw);
    }

    #[test]
    fn copy_file_preserves_non_utf8_without_nul_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

        let src = temp.path().join("c.dat");
        let raw: &[u8] = &[0xFF, 0xFE, b'h', b'i'];
        std::fs::write(&src, raw).unwrap();
        let dst = temp.path().join("c.out");
        copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), raw);
    }

    #[test]
    fn copy_file_over_cap_is_streamed_without_substitution() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);

        let src = temp.path().join("big.txt");
        let mut content = vec![b'a'; (CONTENT_SUBSTITUTION_CAP as usize) + 16];
        content[..18].copy_from_slice(b"%AC_REPLICA_ROOT%\n");
        std::fs::write(&src, &content).unwrap();
        let dst = temp.path().join("big.out");
        copy_file_substituted(&src, &dst, Some(&ctx), true).unwrap();

        let out = std::fs::read(&dst).unwrap();
        assert_eq!(out.len(), content.len());
        // Streamed verbatim: token NOT substituted.
        assert!(out.starts_with(b"%AC_REPLICA_ROOT%"));
    }

    // ---- copy_tree (reparse skip + depth bound) ----------------------------

    #[test]
    fn copy_tree_truncates_beyond_max_depth() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        write_file(&src.join("f.txt"), b"x");
        let dst = temp.path().join("dst");
        let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);
        // Start already over the bound -> immediate Ok, nothing created.
        copy_tree(&src, &dst, MAX_SEED_DEPTH + 1, Some(&ctx), true).unwrap();
        assert!(!dst.exists(), "over-depth call must not create the dst tree");
    }

    #[test]
    fn copy_tree_skips_symlink_and_junction_entries() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_file(&src.join("real.txt"), b"real");
        let target = temp.path().join("target");
        write_file(&target.join("inside.txt"), b"x");

        let link = src.join("linked");
        let created;
        #[cfg(unix)]
        {
            created = std::os::unix::fs::symlink(&target, &link).is_ok();
        }
        #[cfg(windows)]
        {
            let mut cmd = std::process::Command::new("cmd");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            let status = cmd
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    link.to_str().unwrap(),
                    target.to_str().unwrap(),
                ])
                .status();
            created = matches!(status, Ok(s) if s.success());
        }
        if !created {
            return; // environment cannot create links — skip gracefully.
        }

        let dst = temp.path().join("dst");
        let ctx = ctx_with(&abs(r"C:\r", "/r"), Some(&abs(r"C:\ws", "/ws")), None);
        copy_tree(&src, &dst, 0, Some(&ctx), true).unwrap();
        assert!(dst.join("real.txt").exists(), "real file must be copied");
        assert!(
            !dst.join("linked").exists(),
            "symlink/junction entry must be skipped"
        );
    }

    // ---- compute_config_dir_warning (two cases) ----------------------------

    #[test]
    fn warning_mismatch_when_config_dir_env_differs() {
        let dest = abs(r"C:\replica\.claude", "/replica/.claude");
        let mut profile_env = BTreeMap::new();
        profile_env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            abs(r"C:\replica\.claude-amp", "/replica/.claude-amp")
                .to_string_lossy()
                .to_string(),
        );
        let w = compute_config_dir_warning(
            &dest,
            "claude",
            &[],
            &BTreeMap::new(),
            &profile_env,
            None,
        )
        .expect("warning");
        assert!(w.contains("does not match"), "{w}");
    }

    #[test]
    fn warning_overwrite_when_dest_equals_active_config_dir() {
        let dest = abs(r"C:\replica\.claude", "/replica/.claude");
        let mut env = BTreeMap::new();
        // Different parent, same final segment -> "equal active dir" case.
        env.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            abs(r"C:\other\.claude", "/other/.claude")
                .to_string_lossy()
                .to_string(),
        );
        let w = compute_config_dir_warning(&dest, "claude", &[], &BTreeMap::new(), &env, None)
            .expect("warning");
        assert!(w.contains("active") && w.contains("overwritten"), "{w}");
    }

    #[test]
    fn warning_unset_when_no_config_dir_env() {
        let dest = abs(r"C:\replica\.claude", "/replica/.claude");
        let w = compute_config_dir_warning(
            &dest,
            "claude",
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
        .expect("warning");
        assert!(w.contains("not configured"), "{w}");
    }

    #[test]
    fn warning_uses_effective_codex_home_for_codex() {
        let dest = abs(r"C:\replica\.codex", "/replica/.codex");
        let eff = abs(r"C:\replica\.codex", "/replica/.codex");
        let w = compute_config_dir_warning(
            &dest,
            "codex",
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(&eff),
        )
        .expect("warning");
        assert!(w.contains("active"), "{w}");
    }

    #[test]
    fn warning_none_for_non_config_dir_agent() {
        let dest = abs(r"C:\replica\.gemini", "/replica/.gemini");
        // Gemini has no managed config-dir env; a custom command also yields None.
        assert!(
            compute_config_dir_warning(&dest, "gemini", &[], &BTreeMap::new(), &BTreeMap::new(), None)
                .is_none()
        );
        assert!(compute_config_dir_warning(
            &dest,
            "my-custom-agent",
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None
        )
        .is_none());
    }
}
