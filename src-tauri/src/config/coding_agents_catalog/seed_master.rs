use super::*;

// ---------------------------------------------------------------------------
// #769 Phase 2 - dest-keyed config-folder masters + the re-seed button.
//
// A "master" is the shipped default config folder for a built-in, stored at
// `<config_dir>/coding-agents/_seed/<dest>/` (e.g. `_seed/.claude/`). It is
// seeded once at boot (create-if-absent, from the compiled-in embedded default),
// user-editable thereafter, and used two ways:
//   - the absent-only #598 `CatalogDefault` tier fills a NEW replica's `<dest>`
//     from it (never overwriting existing config), and
//   - the Settings re-seed button restores it to the embedded default (`.bak`
//     first, then a trash-first atomic swap).
// Masters are stored VERBATIM (raw `%AC_...%` tokens); substitution happens only
// when the master is copied into a replica. Only Claude/Codex/OpenCode ship a
// master (D5): agents with no verified config-folder convention get none.
// ---------------------------------------------------------------------------

/// Subdirectory of `coding-agents/` holding the dest-keyed masters.
pub(crate) const SEED_MASTERS_DIR_NAME: &str = "_seed";

/// One embedded default file within a master: a forward-slash relative path plus
/// its raw bytes (compiled in via `include_bytes!`).
pub(crate) struct EmbeddedMasterFile {
    pub(crate) rel_path: &'static str,
    pub(crate) bytes: &'static [u8],
}

/// A dest-keyed embedded master: the shipped default config folder for a built-in.
pub(crate) struct EmbeddedSeedMaster {
    /// The catalog `key` this master belongs to (the #1912 support identity).
    pub(crate) key: &'static str,
    /// The command executable basename (lowercase) this master belongs to. Used
    /// for the re-seed button's exact-basename gating.
    pub(crate) command_basename: &'static str,
    /// The dest folder NAME (e.g. `.claude`): both the `_seed/<dest>/` master dir
    /// and the replica fill destination.
    pub(crate) dest: &'static str,
    pub(crate) files: &'static [EmbeddedMasterFile],
}

/// The built-in default config-folder masters. Only agents with a real, minimal,
/// non-intrusive default ship one; Hermes/Pi/Cursor CLI ship none (no verified
/// convention) and therefore get no re-seed button. Content is provisional
/// (Maria approves before land); swapping it is a resource-file-only change.
pub(crate) const EMBEDDED_SEED_MASTERS: &[EmbeddedSeedMaster] = &[
    EmbeddedSeedMaster {
        key: "claude",
        command_basename: "claude",
        dest: ".claude",
        files: &[EmbeddedMasterFile {
            rel_path: "settings.json",
            bytes: include_bytes!("../../../resources/coding-agents/_seed/.claude/settings.json"),
        }],
    },
    EmbeddedSeedMaster {
        key: "codex",
        command_basename: "codex",
        dest: ".codex",
        files: &[EmbeddedMasterFile {
            rel_path: "config.toml",
            bytes: include_bytes!("../../../resources/coding-agents/_seed/.codex/config.toml"),
        }],
    },
    EmbeddedSeedMaster {
        key: "opencode",
        command_basename: "opencode",
        dest: ".opencode",
        files: &[EmbeddedMasterFile {
            rel_path: "opencode.json",
            bytes: include_bytes!("../../../resources/coding-agents/_seed/.opencode/opencode.json"),
        }],
    },
];

/// #1912 - the masters whose key is enabled in the table in force.
pub(crate) fn supported_embedded_masters() -> impl Iterator<Item = &'static EmbeddedSeedMaster> {
    let table = active_builtin_agent_support();
    EMBEDDED_SEED_MASTERS
        .iter()
        .filter(move |m| is_supported_builtin(m.key, table))
}

/// Result of a re-seed, returned to the frontend for the success toast.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReseedResult {
    pub dest: String,
    pub backup_path: String,
}

/// Absolute path to the dest-keyed master: `<ac_dir>/coding-agents/_seed/<dest>/`.
pub fn master_dir_for_dest(ac_dir: &Path, dest: &str) -> PathBuf {
    catalog_dir(ac_dir).join(SEED_MASTERS_DIR_NAME).join(dest)
}

pub(crate) fn embedded_master_for_command_basename(
    basename: &str,
) -> Option<&'static EmbeddedSeedMaster> {
    supported_embedded_masters().find(|m| m.command_basename == basename)
}

/// Lowercased command executable basenames that ship a non-empty embedded default
/// config-folder master AND whose key is enabled in `BUILTIN_AGENT_SUPPORT` (the
/// table in force). The frontend enables the re-seed button only for a catalog
/// def whose command reduces to one of these; the reseed command
/// re-checks server-side. Derived from the supported (enabled) masters, so it
/// stays in sync.
pub fn reseedable_command_basenames() -> Vec<String> {
    supported_embedded_masters()
        .map(|m| m.command_basename.to_string())
        .collect()
}

/// Reduce a coding-agent command to its executable basename (lowercase), mirroring
/// the settings save path. `None` if the command does not tokenize.
///
/// #1171 promoted this from private to `pub(crate)`. It is now the ONLY stem rule in the
/// tree and a second one must not be written, in Rust or in TypeScript: the `starts_with`
/// rule in the frontend's `suggestedContextRegex` must not be ported here or reused, for the
/// reason `reseed_master_for_command` states below - `pi` and `agent` false-match under a
/// prefix rule. The watcher Settings UI gets its reach from `preview_watcher_reach` rather
/// than reimplementing this.
pub(crate) fn command_executable_basename(command: &str) -> Option<String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command).ok()?;
    Some(crate::config::settings::command_token_basename(
        &normalized.shell,
    ))
}

/// #2566 - the ACCOUNT identity of a configured agent command: its program token,
/// verbatim. Same key means same account and one shared weekly-quota reading. NOT
/// `command_executable_basename` above, whose basename step folds `/a/claude` onto
/// `claude`, and NOT resolved through PATH or the filesystem: see #2566 D1/D2.
pub(crate) fn command_account_key(command: &str) -> Option<String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(command).ok()?;
    Some(normalized.shell)
}

/// Write a master's embedded files verbatim into `dir` (creating it and any
/// parents). Rejects a rel_path with empty/`.`/`..` segments.
pub(crate) fn write_embedded_files_into(
    dir: &Path,
    master: &EmbeddedSeedMaster,
) -> Result<(), String> {
    for f in master.files {
        let mut path = dir.to_path_buf();
        for seg in f.rel_path.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(format!("invalid embedded master rel_path '{}'", f.rel_path));
            }
            path.push(seg);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, f.bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Seed the dest-keyed masters ONCE at boot: for each SUPPORTED built-in
/// master (its key is enabled in `BUILTIN_AGENT_SUPPORT`), if
/// `_seed/<dest>/` is absent, stage the embedded default into a sibling temp dir
/// and atomically rename it into place (first-writer-wins across a first-run
/// race). Present masters are user-owned and never touched. Fail-soft: logs and
/// continues; never panics or aborts boot.
///
/// #1318 - `ac_dir` is the project's `.ac` directory; `legacy_catalog_dir` is
/// the legacy `<config_dir>/coding-agents` directory. When the project master is
/// ABSENT and the legacy `<legacy>/_seed/<dest>/` is a real directory, the tree
/// is copied VERBATIM (`copy_tree`, substitution OFF, symlinks skipped); an
/// EMPTY legacy master dir is copied verbatim too (present-but-empty master:
/// the spawn tier stays inert and the embedded default is NOT seeded, per the
/// present = user-owned rule; the Settings re-seed button restores it). On a
/// legacy-copy ERROR the partial destination is removed and the embedded master
/// is staged instead (a partial copy must never win). No size cap: a large
/// legacy master is copied into every registered project (the verbatim promise
/// wins). #1912: a de-supported master (a `false` row) is skipped ENTIRELY from
/// every source - embedded staging and legacy `_seed/<dest>` tree copy alike.
pub fn ensure_seeded_masters(ac_dir: &Path, legacy_catalog_dir: Option<&Path>) {
    if all_masters_present(ac_dir) {
        return;
    }
    for master in supported_embedded_masters() {
        let dir = master_dir_for_dest(ac_dir, master.dest);
        match std::fs::symlink_metadata(&dir) {
            Ok(_) => continue, // present (any form) -> user-owned, leave alone
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                log::warn!(
                    "[coding-agents] cannot stat master {} ({e}); skipping seed",
                    dir.display()
                );
                continue;
            }
        }

        // #1318 migration: absent project master + legacy REAL-directory master
        // -> verbatim tree copy.
        let mut copied_from_legacy = false;
        if let Some(legacy) = legacy_catalog_dir {
            let legacy_master = legacy.join(SEED_MASTERS_DIR_NAME).join(master.dest);
            match std::fs::symlink_metadata(&legacy_master) {
                Ok(meta) if meta.is_dir() => {
                    match crate::config::config_seed::copy_tree(
                        &legacy_master,
                        &dir,
                        0,
                        None,
                        false,
                    ) {
                        Ok(()) => {
                            copied_from_legacy = true;
                            let bytes = tree_byte_count(&dir);
                            log::info!(
                                "[coding-agents] migrated legacy master tree {} -> {} ({} bytes \
                                    verbatim, no size cap)",
                                legacy_master.display(),
                                dir.display(),
                                bytes
                            );
                        }
                        Err(e) => {
                            // A partial copy must never win over the embedded
                            // default; the legacy tree itself is never touched.
                            let _ = std::fs::remove_dir_all(&dir);
                            log::warn!(
                                "[coding-agents] failed to copy legacy master {} ({e}); falling \
                                    back to the embedded default",
                                legacy_master.display()
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        if copied_from_legacy {
            continue;
        }

        let sfx = unique_suffix();
        let staging = staging_sibling(&dir, "seedtmp", &sfx);
        let _ = std::fs::remove_dir_all(&staging);
        let result = write_embedded_files_into(&staging, master)
            .and_then(|()| rename_into_place(&staging, &dir));
        match result {
            Ok(()) => log::info!(
                "[coding-agents] seeded default config master at {}",
                dir.display()
            ),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                log::warn!(
                    "[coding-agents] failed to seed master {} ({e})",
                    dir.display()
                );
            }
        }
    }
}

/// #1912 - existence-only steady-state check for the MASTER dirs only. A
/// de-supported master is never required. The catalog itself deliberately has
/// no existence shortcut: every startup/registration must validate ownership
/// and revision and recover or refresh, so only master staging keeps this
/// optimization.
pub(crate) fn all_masters_present(ac_dir: &Path) -> bool {
    supported_embedded_masters()
        .all(|m| std::fs::symlink_metadata(master_dir_for_dest(ac_dir, m.dest)).is_ok())
}

/// Seed the catalog + masters for one registered project root, then record the
/// catalog publication in that project's seed manifest.
///
/// Preconditions (enforced before ANY filesystem effect, in both entry points):
/// a non-absolute root (a hand-edited relative settings entry must never seed
/// relative to the process CWD) or a missing root (a deleted/stale registered
/// root must never be resurrected by the seed's `create_dir_all`) is logged and
/// skipped. The CATALOG has no existence shortcut: every startup/registration
/// takes the catalog lock (and, in production, the soft project gate) to
/// validate ownership/revision and to recover, refresh or migrate. Only the
/// MASTER staging keeps its existence optimization.
pub(crate) fn ensure_seeded_for_project(project_root: &Path) {
    #[cfg(not(test))]
    let activation = Some(ManifestActivationToken::production());
    #[cfg(test)]
    let activation: Option<ManifestActivationToken> = None;
    ensure_seeded_for_project_with_token(project_root, activation.as_ref());
}

pub(crate) fn tree_byte_count(dir: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else if meta.is_file() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0_u64;
    walk(dir, &mut total);
    total
}

/// Re-seed the master for `command` back to AC's embedded default (the Settings
/// button). Gating is re-checked server-side: `command`'s executable basename must
/// EXACTLY equal a built-in that ships a master (never `starts_with`, so `pi` and
/// `agent` cannot false-match). On success: a timestamped `.bak` of the current
/// master is made FIRST, then a trash-first atomic swap installs the embedded
/// default. On failure the prior master is restored; never a partial state.
pub fn reseed_master_for_command(ac_dir: &Path, command: &str) -> Result<ReseedResult, String> {
    let basename = command_executable_basename(command)
        .ok_or_else(|| format!("'{command}' is not a valid coding-agent command"))?;
    let master = embedded_master_for_command_basename(&basename).ok_or_else(|| {
        format!("'{command}' is not a recognized built-in with a shipped default config folder")
    })?;
    let dir = master_dir_for_dest(ac_dir, master.dest);

    let _guard = RESEED_LOCK
        .lock()
        .map_err(|_| "re-seed lock poisoned".to_string())?;

    // 1. `.bak` the current master (verbatim), BEFORE any swap.
    let backup_path = if dir.exists() {
        Some(backup_master_dir(&dir)?)
    } else {
        None
    };

    // 2. Trash-first atomic swap of the embedded default into the master.
    let sfx = unique_suffix();
    let staging = staging_sibling(&dir, "reseedtmp", &sfx);
    let trash = staging_sibling(&dir, "reseedold", &sfx);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&trash);

    write_embedded_files_into(&staging, master)?;

    if dir.exists() {
        if let Err(e) = std::fs::rename(&dir, &trash) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "master {} is in use ({e}); re-seed aborted, master unchanged",
                dir.display()
            ));
        }
    }
    if let Err(e) = std::fs::rename(&staging, &dir) {
        // Restore the prior master so we never leave a hole.
        if !dir.exists() && trash.exists() {
            let _ = std::fs::rename(&trash, &dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "failed to install re-seeded master {} ({e}); prior config restored",
            dir.display()
        ));
    }
    let _ = std::fs::remove_dir_all(&trash);

    Ok(ReseedResult {
        dest: master.dest.to_string(),
        backup_path: backup_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    })
}

/// Verbatim, timestamped `.bak` copy of a master dir (unique-suffix loop, mirrors
/// `seeded_context_templates::create_backup`). Copies with substitution OFF so the
/// raw `%AC_...%` tokens in the master are preserved in the backup.
pub(crate) fn backup_master_dir(dir: &Path) -> Result<PathBuf, String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("master {} has no parent", dir.display()))?;
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("master {} has no name", dir.display()))?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();
    for index in 0..1000u32 {
        let bak_name = match index {
            0 => format!("{name}.bak-{ts}"),
            n => format!("{name}.bak-{ts}.{n}"),
        };
        let bak = parent.join(&bak_name);
        if bak.exists() {
            continue;
        }
        crate::config::config_seed::copy_tree(dir, &bak, 0, None, false)
            .map_err(|e| format!("backup {} -> {}: {e}", dir.display(), bak.display()))?;
        return Ok(bak);
    }
    Err(format!(
        "could not find a unique .bak path for {}",
        dir.display()
    ))
}

pub(crate) fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SEED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn staging_sibling(dir: &Path, tag: &str, sfx: &str) -> PathBuf {
    let parent = dir.parent().unwrap_or_else(|| Path::new("."));
    let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("_seed");
    parent.join(format!("{name}.{tag}-{sfx}"))
}

/// Publish a staged dir into `dest` (dest expected absent). Cleans staging on
/// failure. `std::fs::rename` is first-writer-wins if `dest` appeared concurrently.
pub(crate) fn rename_into_place(staging: &Path, dest: &Path) -> Result<(), String> {
    std::fs::rename(staging, dest).map_err(|e| format!("install {} ({e})", dest.display()))
}

// E2432: narrow physical IO, no catalog initialization, migration or refresh.
pub(crate) fn replace_selected_personal_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let paths = CatalogPaths::new(path.parent().ok_or("sourceContextUnavailable")?);
    if path != paths.local {
        return Err("sourceReadOnly".into());
    }
    publish_replace(path, bytes)
}

pub(crate) fn publish_selected_personal(
    proof: &mut crate::config::settings::CatalogSourceOperationGuard<'_, '_>,
    edits: &crate::config::settings::AgentFileEdits,
) -> crate::config::settings::AgentFileSaveReply {
    use crate::config::settings::{AgentFileSaveOutcome, OperationEffect};
    let path = proof.target().to_path_buf();
    let paths = CatalogPaths::new(path.parent().expect("authorized catalog parent"));
    let lock = (|| {
        if path != paths.local {
            return Err("sourceReadOnly".into());
        }
        proof.verify_target_parent_chain()?;
        // Locking never initializes a catalog or creates its parent directory.
        acquire_catalog_lock(&paths)
    })();
    let _lock = match lock {
        Ok(lock) => lock,
        Err(reason) => return proof.finish(AgentFileSaveOutcome::Rejected, Some(&reason)),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let bytes = proof.prepare(edits)?;
        proof.recheck()?;
        proof.publish(&bytes)
    }));
    let reason = match result {
        Ok(Ok(())) => None,
        Ok(Err(reason)) => Some(reason),
        Err(_) => Some("catalogPublicationPanicked".into()),
    };
    let outcome = if reason.is_none() {
        AgentFileSaveOutcome::Committed
    } else if proof.effect() == OperationEffect::Possible {
        AgentFileSaveOutcome::PartialOrUnknown
    } else {
        AgentFileSaveOutcome::Rejected
    };
    // Both panic containment and recovery remain INSIDE the catalog lock scope.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        proof.finish(outcome, reason.as_deref())
    }))
    .unwrap_or_else(|_| proof.interrupted())
}
