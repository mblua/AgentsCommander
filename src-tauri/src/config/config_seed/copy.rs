use crate::filesystem::config_seed as seed_disk;
use std::path::{Component, Path, PathBuf};

use crate::config::placeholders::{expand_placeholders_in_content, PlaceholderContext};
use crate::config::seed_manifest::{
    config_batch_base_serialized_len, exact_config_row_serialized_len, ManifestSource,
    ResourceBoundKind, SeedManifestError, MAX_FIELD_BYTES, MAX_MANIFEST_BYTES, MAX_MANIFEST_ROWS,
};

use super::{
    CollectedSeedFiles, ConfigSeedTier, ResolvedConfigSeed, CONTENT_SUBSTITUTION_CAP,
    MAX_SEED_DEPTH,
};

pub(super) fn is_readable_dir(path: &Path) -> bool {
    seed_disk::inspect_seed_source(path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

pub(super) fn manifest_source_for_tier(tier: ConfigSeedTier) -> ManifestSource {
    match tier {
        ConfigSeedTier::WorkspaceProfile => ManifestSource::WorkspaceProfile,
        ConfigSeedTier::WorkspaceBase => ManifestSource::WorkspaceBase,
        ConfigSeedTier::MatrixProfile => ManifestSource::MatrixProfile,
        ConfigSeedTier::MatrixBase => ManifestSource::MatrixBase,
        ConfigSeedTier::CatalogDefault => ManifestSource::CatalogDefault,
    }
}

pub(super) fn project_relative_dest(seed: &ResolvedConfigSeed) -> Result<PathBuf, String> {
    let ac_root = seed
        .context
        .ac_root
        .as_ref()
        .ok_or_else(|| "launch root has no project workspace".to_string())?;
    let inside_ac_root = seed.dest.strip_prefix(ac_root).map_err(|_| {
        format!(
            "destination {} is outside workspace {}",
            seed.dest.display(),
            ac_root.display()
        )
    })?;
    // `ac_root` is the `.ac` workspace in production. Focused unit tests
    // use a synthetic leaf, so add the stable project-relative `.ac` component
    // rather than depending on that leaf's fixture name.
    let relative = PathBuf::from(".ac").join(inside_ac_root);
    if relative.is_absolute()
        || relative.components().next() != Some(Component::Normal(".ac".as_ref()))
    {
        return Err("project-relative config destination is not under .ac".to_string());
    }
    Ok(relative)
}

pub(super) fn config_scope_for_seed(seed: &ResolvedConfigSeed) -> Result<String, String> {
    let relative = project_relative_dest(seed)?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("config scope contains a non-normal component".to_string());
        };
        let component = component
            .to_str()
            .ok_or_else(|| "config scope is not valid UTF-8".to_string())?;
        components.push(component);
    }
    Ok(format!("config:{}", components.join("/")))
}

pub(super) struct SeedFileCollector {
    pub(super) files: CollectedSeedFiles,
    pub(super) project_relative_dest: Option<PathBuf>,
    pub(super) scope: Option<String>,
    pub(super) source: ManifestSource,
    pub(super) prospective_bytes: usize,
}

impl SeedFileCollector {
    pub(super) fn new(seed: &ResolvedConfigSeed, tier: ConfigSeedTier) -> Self {
        let project_relative_dest = project_relative_dest(seed).ok();
        let scope = config_scope_for_seed(seed).ok();
        let files = if project_relative_dest.is_some() && scope.is_some() {
            CollectedSeedFiles::Exact(Vec::new())
        } else {
            CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ScopeBytes,
                observed_at_least: 1,
            }
        };
        Self {
            files,
            project_relative_dest,
            scope,
            source: manifest_source_for_tier(tier),
            prospective_bytes: config_batch_base_serialized_len(),
        }
    }

    pub(super) fn record(&mut self, staging_relative_path: PathBuf) {
        let CollectedSeedFiles::Exact(paths) = &self.files else {
            return;
        };
        let observed_rows = paths.len().saturating_add(1);
        if observed_rows > MAX_MANIFEST_ROWS {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::Rows,
                observed_at_least: observed_rows as u64,
            };
            return;
        }
        let Some(project_relative_dest) = self.project_relative_dest.as_ref() else {
            return;
        };
        let Some(scope) = self.scope.as_deref() else {
            return;
        };
        if scope.len() > MAX_FIELD_BYTES {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ScopeBytes,
                observed_at_least: scope.len() as u64,
            };
            return;
        }
        let project_relative_path = project_relative_dest.join(&staging_relative_path);
        let row_bytes =
            match exact_config_row_serialized_len(&project_relative_path, scope, self.source) {
                Ok(bytes) => bytes,
                Err(SeedManifestError::ResourceBound {
                    kind,
                    observed_at_least,
                    ..
                }) => {
                    self.files = CollectedSeedFiles::OverBound {
                        reason: kind,
                        observed_at_least,
                    };
                    return;
                }
                Err(_) => {
                    self.files = CollectedSeedFiles::OverBound {
                        reason: ResourceBoundKind::PathBytes,
                        observed_at_least: MAX_FIELD_BYTES.saturating_add(1) as u64,
                    };
                    return;
                }
            };
        let Some(prospective_bytes) = self.prospective_bytes.checked_add(row_bytes) else {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::ArithmeticOverflow,
                observed_at_least: u64::MAX,
            };
            return;
        };
        if prospective_bytes as u64 > MAX_MANIFEST_BYTES {
            self.files = CollectedSeedFiles::OverBound {
                reason: ResourceBoundKind::OutputBytes,
                observed_at_least: prospective_bytes as u64,
            };
            return;
        }
        self.prospective_bytes = prospective_bytes;
        if let CollectedSeedFiles::Exact(paths) = &mut self.files {
            paths.push(staging_relative_path);
        }
    }

    pub(super) fn finish(self) -> CollectedSeedFiles {
        self.files
    }
}

pub(super) fn remove_seed_scratch(path: &Path, context: &str, unique_sfx: &str) {
    match seed_disk::remove_seed_scratch(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "[config-seed] {} failed for owned scratch {} session={}: {}",
            context,
            path.display(),
            unique_sfx,
            error
        ),
    }
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
        let Ok(entries) = seed_disk::seed_entries(dir) else {
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
pub(super) fn clear_stale_seed_scratch(parent: &Path, dest_name: &str) {
    let tmp_prefix = format!("{}.acseed-tmp-", dest_name);
    let old_prefix = format!("{}.acseed-old-", dest_name);
    let entries = match seed_disk::seed_entries(parent) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!(
                "[config-seed] failed to inspect stale scratch siblings for {}: {}",
                parent.display(),
                error
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!(
                    "[config-seed] failed to inspect a stale scratch entry under {}: {}",
                    parent.display(),
                    error
                );
                continue;
            }
        };
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with(&tmp_prefix) || name.starts_with(&old_prefix) {
                let path = entry.path();
                if let Err(error) = seed_disk::remove_seed_scratch(&path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        log::warn!(
                            "[config-seed] failed to reclaim stale scratch {} scope={}: {}",
                            path.display(),
                            dest_name,
                            error
                        );
                    }
                }
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
    let mut collector = None;
    copy_tree_internal(
        src_dir,
        dst_dir,
        depth,
        Path::new(""),
        ctx,
        substitute,
        &mut collector,
    )
}

pub(super) fn copy_tree_collecting(
    src_dir: &Path,
    dst_dir: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
    collector: &mut SeedFileCollector,
) -> Result<(), String> {
    let mut collector = Some(collector);
    copy_tree_internal(
        src_dir,
        dst_dir,
        0,
        Path::new(""),
        ctx,
        substitute,
        &mut collector,
    )
}

pub(super) fn copy_tree_internal(
    src_dir: &Path,
    dst_dir: &Path,
    depth: u32,
    relative_dir: &Path,
    ctx: Option<&PlaceholderContext>,
    substitute: bool,
    collector: &mut Option<&mut SeedFileCollector>,
) -> Result<(), String> {
    if depth > MAX_SEED_DEPTH {
        log::warn!(
            "[config-seed] template tree deeper than {} levels at {}; truncating",
            MAX_SEED_DEPTH,
            src_dir.display()
        );
        return Ok(());
    }
    seed_disk::ensure_seed_directory(dst_dir)
        .map_err(|e| format!("create {}: {}", dst_dir.display(), e))?;
    let entries = seed_disk::seed_entries(src_dir)
        .map_err(|e| format!("read {}: {}", src_dir.display(), e))?;
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
        let relative = relative_dir.join(entry.file_name());
        if ft.is_dir() {
            copy_tree_internal(
                &entry.path(),
                &dst,
                depth + 1,
                &relative,
                ctx,
                substitute,
                collector,
            )?;
        } else if ft.is_file() {
            copy_file_substituted(&entry.path(), &dst, ctx, substitute)?;
            if let Some(collector) = collector.as_deref_mut() {
                collector.record(relative);
            }
        }
        // Anything else (already-filtered symlinks/reparse) is ignored.
    }
    Ok(())
}

pub(super) fn copy_file_substituted(
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
            return seed_disk::copy_seed_file(src, dst)
                .map(|_| ())
                .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e));
        }
    };
    let len = seed_disk::inspect_seed_source(src)
        .map_err(|e| format!("stat {}: {}", src.display(), e))?
        .len();
    // M4: over-cap files are streamed, never read whole into memory.
    if len > CONTENT_SUBSTITUTION_CAP {
        seed_disk::copy_seed_file(src, dst)
            .map_err(|e| format!("copy {} -> {}: {}", src.display(), dst.display(), e))?;
        return Ok(());
    }
    let bytes =
        seed_disk::read_seed_file(src).map_err(|e| format!("read {}: {}", src.display(), e))?;
    // Binary sniff: a NUL in the leading window means "copy verbatim".
    let probe = bytes.len().min(8192);
    if bytes[..probe].contains(&0u8) {
        return seed_disk::write_seed_file(dst, &bytes)
            .map_err(|e| format!("write {}: {}", dst.display(), e));
    }
    match std::str::from_utf8(&bytes) {
        // CRLF and a UTF-8 BOM survive (only known token substrings are swapped).
        Ok(text) => {
            seed_disk::write_seed_file(dst, expand_placeholders_in_content(text, ctx).as_bytes())
                .map_err(|e| format!("write {}: {}", dst.display(), e))
        }
        // Non-UTF-8 (no NUL): copy verbatim.
        Err(_) => seed_disk::write_seed_file(dst, &bytes)
            .map_err(|e| format!("write {}: {}", dst.display(), e)),
    }
}

pub(super) use crate::filesystem::config_seed::is_symlink_or_reparse;
