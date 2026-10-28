#[cfg(all(test, windows))]
use super::WindowsNamespaceOperation;
use super::{
    acquire_kernel_lock, classify_open_error, classify_toml_parse_error,
    cleanup_current_owned_temp, handle_facts, is_exact_not_found, is_exact_temp_name,
    native_name_has_temp_shape, open_or_create_lock_file, open_regular_no_follow,
    parse_manifest_bytes, read_bounded_file, sanitize_canonical_validation_error, serialize_state,
    stream_matches, verify_temp_path, CanonicalSnapshot, FileIdentity, ManifestDegradedReason,
    ManifestRecordOutcome, ManifestState, OpenedRegularFile, PinnedDirectory, PinnedOwnerChain,
    ProjectPublicationPermit, ProjectPublicationPermitState, ResourceBoundKind, SeedManifestError,
    SeedManifestWire, COVERAGE, COVERAGE_VERSION, DEFAULT_LOCK_TIMEOUT, MAX_MANIFEST_BYTES,
    MAX_TEMP_DIAGNOSTIC_SAMPLES, MAX_TEMP_INVENTORY_ENTRIES, SEED_MANIFEST_FILENAME,
    SEED_MANIFEST_LOCK_FILENAME, SEED_MANIFEST_TEMP_PREFIX, SEED_MANIFEST_TEMP_SUFFIX,
};
#[cfg(unix)]
use super::{atomic_publish_manifest, remove_prior_unix_temp_if_safe};
use crate::filesystem::seed_manifest::{self as manifest_disk, ManifestFile as File};
use std::fmt;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Default)]
pub(crate) struct TempInventory {
    pub(crate) exact_names: Vec<String>,
    pub(crate) exact_count: u64,
    pub(crate) malformed_count: u64,
    pub(crate) malformed_native_count: u64,
    pub(crate) exact_samples: Vec<String>,
    pub(crate) malformed_samples: Vec<String>,
    pub(crate) entry_errors: u64,
    pub(crate) scan_truncated: bool,
    pub(crate) scan_error: Option<String>,
}

#[cfg(test)]
pub(crate) type TestPathHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(test)]
pub(crate) type TestTempInventoryHook = std::sync::Arc<dyn Fn(&TempInventory) + Send + Sync>;

#[cfg(all(test, windows))]
pub(crate) type TestWindowsNamespaceHook = std::sync::Arc<
    dyn Fn(WindowsNamespaceOperation, &Path, &Path) -> Result<(), u32> + Send + Sync,
>;

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct TestFilesystemHooks {
    pub(crate) before_temp_validation: Option<TestPathHook>,
    pub(crate) before_raw_conflict_check: Option<TestPathHook>,
    pub(crate) on_prior_temp_diagnostic: Option<TestTempInventoryHook>,
    #[cfg(windows)]
    pub(crate) windows_namespace_call: Option<TestWindowsNamespaceHook>,
}

pub(crate) struct ProjectSeedManifestGuard {
    pub(crate) project: PinnedDirectory,
    pub(crate) ac_root: PinnedDirectory,
    pub(crate) lock_path: PathBuf,
    pub(crate) lock_file: File,
    pub(crate) lock_identity: FileIdentity,
    pub(crate) snapshot: CanonicalSnapshot,
    pub(crate) locked: bool,
    #[cfg(test)]
    pub(crate) hooks: TestFilesystemHooks,
}

impl fmt::Debug for ProjectSeedManifestGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSeedManifestGuard")
            .field("project", &self.project.path)
            .field("ac_root", &self.ac_root.path)
            .field("lock_path", &self.lock_path)
            .field("locked", &self.locked)
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

impl ProjectSeedManifestGuard {
    pub(crate) fn acquire(project_root: &Path) -> Result<Self, SeedManifestError> {
        Self::acquire_with_timeout(project_root, DEFAULT_LOCK_TIMEOUT)
    }

    pub(crate) fn acquire_with_timeout(
        project_root: &Path,
        timeout: Duration,
    ) -> Result<Self, SeedManifestError> {
        #[cfg(test)]
        let hooks = TestFilesystemHooks::default();
        Self::acquire_inner(
            project_root,
            timeout,
            #[cfg(test)]
            hooks,
        )
    }

    #[cfg(test)]
    pub(crate) fn acquire_with_hooks(
        project_root: &Path,
        timeout: Duration,
        hooks: TestFilesystemHooks,
    ) -> Result<Self, SeedManifestError> {
        Self::acquire_inner(project_root, timeout, hooks)
    }

    pub(crate) fn acquire_inner(
        project_root: &Path,
        timeout: Duration,
        #[cfg(test)] hooks: TestFilesystemHooks,
    ) -> Result<Self, SeedManifestError> {
        let supplied_project = PinnedDirectory::open(project_root)?;
        let canonical_project_path = manifest_disk::canonical_manifest_project(project_root)
            .map_err(|source| SeedManifestError::Io {
                operation: "canonicalize pinned project root",
                path: project_root.to_path_buf(),
                source,
            })?;
        let project = PinnedDirectory::open(&canonical_project_path)?;
        if project.identity != supplied_project.identity {
            return Err(SeedManifestError::UnsafePath {
                path: project_root.to_path_buf(),
                reason: "canonical project identity differs from the opened project handle"
                    .to_string(),
            });
        }

        let ac_path = canonical_project_path.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
        let ac_root = PinnedDirectory::open(&ac_path)?;
        let lock_path = ac_path.join(SEED_MANIFEST_LOCK_FILENAME);
        let started = Instant::now();
        let deadline = started.checked_add(timeout).unwrap_or(started);
        let lock_file = open_or_create_lock_file(&lock_path, deadline)?;
        let lock_identity = handle_facts(&lock_file)
            .map_err(|source| SeedManifestError::LockIo {
                path: lock_path.clone(),
                saw_contention: false,
                source,
            })?
            .identity;
        acquire_kernel_lock(&lock_file, &lock_path, started, deadline)?;

        let mut guard = Self {
            project,
            ac_root,
            lock_path,
            lock_file,
            lock_identity,
            snapshot: CanonicalSnapshot::Writable {
                raw: Vec::new(),
                state: ManifestState::default(),
                canonical: None,
            },
            locked: true,
            #[cfg(test)]
            hooks,
        };
        guard.revalidate_owner()?;
        let inventory = guard.inventory_prior_temps();
        match guard.load_canonical_snapshot() {
            Ok(snapshot) => {
                let valid_canonical = snapshot.has_valid_existing_canonical();
                guard.snapshot = snapshot;
                guard.process_prior_temp_inventory(inventory, valid_canonical);
            }
            Err(error) => {
                guard.process_prior_temp_inventory(inventory, false);
                return Err(error);
            }
        }
        // #1318 - one-shot v1 -> v2 coverage upgrade. Runs after the prior-temp
        // policy evaluated the PRE-migration state, and its own temp (created
        // and renamed inside `write_canonical`, cleaned on failure) never
        // interacts with that inventory. After a successful upgrade the strict
        // parse succeeds, so the next acquire never re-enters the migration.
        guard.try_upgrade_v1_to_v2();
        Ok(guard)
    }

    pub(crate) fn ac_root(&self) -> &Path {
        &self.ac_root.path
    }

    pub(crate) fn revalidate_owner(&self) -> Result<(), SeedManifestError> {
        self.project.revalidate()?;
        self.ac_root.revalidate()?;
        let reopened = OpenedRegularFile::open_existing_lock(&self.lock_path)?;
        if reopened.identity != self.lock_identity {
            return Err(SeedManifestError::UnsafePath {
                path: self.lock_path.clone(),
                reason: "lock-file identity changed while its original handle remained locked"
                    .to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn pin_owner_chain_from_ac(
        &self,
        relative: &Path,
    ) -> Result<PinnedOwnerChain, SeedManifestError> {
        if relative.is_absolute() {
            return Err(SeedManifestError::Validation(format!(
                "owner chain must be relative to .ac: {}",
                relative.display()
            )));
        }
        self.revalidate_owner()?;
        let mut path = self.ac_root.path.clone();
        let mut directories = Vec::new();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(SeedManifestError::Validation(format!(
                    "owner chain contains a non-normal component: {}",
                    relative.display()
                )));
            };
            path.push(component);
            directories.push(PinnedDirectory::open(&path)?);
        }
        let chain = PinnedOwnerChain { directories };
        self.revalidate_owner()?;
        chain.revalidate()?;
        Ok(chain)
    }

    pub(crate) fn publication_permit(&mut self) -> ProjectPublicationPermit<'_> {
        if let Some(reason) = self.snapshot.reason() {
            ProjectPublicationPermit {
                state: ProjectPublicationPermitState::DegradedUntracked {
                    held_guard: Some(self),
                    reason,
                },
            }
        } else {
            ProjectPublicationPermit {
                state: ProjectPublicationPermitState::Tracked(self),
            }
        }
    }

    pub(crate) fn release(mut self) {
        self.unlock_explicitly();
    }

    pub(crate) fn unlock_explicitly(&mut self) {
        if !self.locked {
            return;
        }
        if let Err(error) = File::unlock(&self.lock_file) {
            log::warn!(
                "[seed_manifest] explicit unlock failed path={} error={}",
                self.lock_path.display(),
                error
            );
        }
        self.locked = false;
    }

    pub(crate) fn load_canonical_snapshot(&self) -> Result<CanonicalSnapshot, SeedManifestError> {
        let canonical_path = self.ac_root.path.join(SEED_MANIFEST_FILENAME);
        let mut canonical = match open_regular_no_follow(&canonical_path, false, false) {
            Ok(file) => OpenedRegularFile::from_file(&canonical_path, file)?,
            Err(error) if is_exact_not_found(&error) => {
                return Ok(CanonicalSnapshot::Writable {
                    raw: Vec::new(),
                    state: ManifestState::default(),
                    canonical: None,
                });
            }
            Err(error) => {
                return Err(classify_open_error(
                    &canonical_path,
                    error,
                    "open canonical manifest",
                    false,
                ));
            }
        };

        #[cfg(windows)]
        {
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;

            if canonical.attributes & FILE_ATTRIBUTE_READONLY != 0 {
                log::warn!(
                    "[seed_manifest] canonical manifest is read-only and was preserved path={}",
                    canonical_path.display()
                );
                return Ok(CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::ReadOnlyCanonical,
                    canonical: Some(canonical),
                });
            }
        }

        let raw = match read_bounded_file(
            &mut canonical.file,
            canonical.length,
            MAX_MANIFEST_BYTES,
            &canonical_path,
        ) {
            Ok(raw) => raw,
            Err(SeedManifestError::ResourceBound { .. }) => {
                return Ok(CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::ResourceBound(ResourceBoundKind::InputBytes),
                    canonical: Some(canonical),
                });
            }
            Err(error) => return Err(error),
        };
        let reopened = OpenedRegularFile::open_existing(&canonical_path, false)?;
        if reopened.identity != canonical.identity {
            return Err(SeedManifestError::UnsafePath {
                path: canonical_path,
                reason: "canonical identity changed during its bounded read".to_string(),
            });
        }
        match parse_manifest_bytes(&raw) {
            Ok(state) => Ok(CanonicalSnapshot::Writable {
                raw,
                state,
                canonical: Some(canonical),
            }),
            Err(error) => {
                let reason = error.degraded_reason();
                log::warn!(
                    "[seed_manifest] canonical manifest is preserved read-only path={} reason={:?} \
                        diagnostic_kind={}",
                    canonical_path.display(),
                    reason,
                    error.diagnostic_kind()
                );
                Ok(CanonicalSnapshot::ReadOnly {
                    reason,
                    canonical: Some(canonical),
                })
            }
        }
    }

    /// #1318 - one-shot v1 -> v2 coverage upgrade, run under the held lock on
    /// the first acquire of an exact-shape v1 manifest. Deterministic and
    /// strict: the v2 state is built by SUBSTITUTION from the parsed v1 wire
    /// (`coverage_version` and the coverage list only), so every existing row
    /// check and bound runs verbatim and no timestamp is invented. Canonical
    /// output preserves supported rows and retains legacy replica-config rows
    /// only in the held state. The atomic write goes through
    /// [`Self::write_canonical`] DIRECTLY (not
    /// `persist_current_state`, whose `Unchanged` fast path and `Recorded`
    /// classification are publication semantics, not migration semantics):
    /// `verify_canonical_unchanged` stream-compares the CURRENT disk file
    /// against the retained v1 bytes under the lock, so a concurrent external
    /// edit between the re-read and the write fails the raw-conflict check.
    ///
    /// Only a `ReadOnly { reason: InvalidCanonical, canonical: Some(..) }`
    /// snapshot is migratable; every other degraded reason (ResourceBound,
    /// FutureSchema, ExternalEdit, ReadOnlyCanonical, UnsafePath,
    /// PersistenceFailure) is preserved as-is. Detection keys ONLY on the wire
    /// pre-parse, never on the sanitized message string.
    pub(crate) fn try_upgrade_v1_to_v2(&mut self) {
        let snapshot = std::mem::replace(
            &mut self.snapshot,
            CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: None,
            },
        );
        let mut canonical = match snapshot {
            CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: Some(canonical),
            } => canonical,
            other => {
                self.snapshot = other;
                return;
            }
        };
        let canonical_path = canonical.path.clone();
        let restore = |guard: &mut Self, canonical: OpenedRegularFile| {
            guard.snapshot = CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical: Some(canonical),
            };
        };

        // b. Re-read the raw bytes from the retained handle (the ReadOnly
        // snapshot holds no raw copy; the handle stays valid under the lock).
        let raw = match read_bounded_file(
            &mut canonical.file,
            canonical.length,
            MAX_MANIFEST_BYTES,
            &canonical_path,
        ) {
            Ok(raw) => raw,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade re-read failed path={} error={}; manifest \
                        stays preserved read-only",
                    canonical_path.display(),
                    error
                );
                restore(self, canonical);
                return;
            }
        };

        // c. Lenient pre-parse. The wire deserializers enforce the MAX_* bounds,
        // so a bound-violating v1 file fails here and stays preserved (the v2
        // strict parse would reject the same rows).
        let text = match std::str::from_utf8(&raw) {
            Ok(text) => text,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade skipped: manifest is not UTF-8 at byte offset \
                        {} path={}; manifest stays preserved read-only",
                    error.valid_up_to(),
                    canonical_path.display()
                );
                restore(self, canonical);
                return;
            }
        };
        let wire = match toml::from_str::<SeedManifestWire>(text) {
            Ok(wire) => wire,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade skipped path={} error={}; manifest stays \
                        preserved read-only",
                    canonical_path.display(),
                    classify_toml_parse_error(error)
                );
                restore(self, canonical);
                return;
            }
        };

        // Exact v1 shape: schema 1, coverage 1, the exact two-item list in that
        // order, nothing else. Any other shape (including `coverage_version = 2`
        // with the old list, or a v1 file with a different list) keeps the
        // strict-invalid handling unchanged.
        const V1_COVERAGE: [&str; 2] = ["project_context_templates", "replica_config_folders"];
        if wire.schema_version != 1
            || wire.coverage_version != 1
            || wire.coverage != V1_COVERAGE.map(str::to_string).to_vec()
        {
            restore(self, canonical);
            return;
        }

        // d. Build the v2 state by SUBSTITUTION and run the existing strict
        // checks verbatim (per-row validate_row, duplicate-identity rejection,
        // mixed-source/timestamp batch checks, MAX_* bounds).
        let mut v2 = wire;
        v2.coverage_version = COVERAGE_VERSION;
        v2.coverage = COVERAGE.map(str::to_string).to_vec();
        let state = match ManifestState::from_wire(v2) {
            Ok(state) => state,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1 manifest rows failed v2 validation path={} error={}; \
                        manifest stays preserved read-only",
                    canonical_path.display(),
                    sanitize_canonical_validation_error(error)
                );
                restore(self, canonical);
                return;
            }
        };

        // e. Serialize canonical output and write under the held lock. The held
        // state still retains every validated v1 row. `write_canonical` updates
        // the snapshot itself on success; on failure its temp is cleaned and the
        // pre-migration degraded snapshot is restored (bytes preserved exactly,
        // writer disabled, published-unrecorded).
        let bytes = match serialize_state(&state) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "[seed_manifest] v1->v2 upgrade serialize failed path={} error={}; manifest \
                        stays preserved read-only",
                    canonical_path.display(),
                    error
                );
                restore(self, canonical);
                return;
            }
        };
        self.snapshot = CanonicalSnapshot::Writable {
            raw,
            state,
            canonical: Some(canonical),
        };
        if let Err(error) = self.write_canonical(bytes) {
            log::warn!(
                "[seed_manifest] v1->v2 upgrade write failed path={} error={}; manifest stays \
                    preserved read-only",
                canonical_path.display(),
                error
            );
            let canonical = match std::mem::replace(
                &mut self.snapshot,
                CanonicalSnapshot::ReadOnly {
                    reason: ManifestDegradedReason::InvalidCanonical,
                    canonical: None,
                },
            ) {
                CanonicalSnapshot::Writable { canonical, .. } => canonical,
                other => {
                    // Unreachable: write_canonical leaves the snapshot untouched
                    // on error, so it is still the Writable we just installed.
                    self.snapshot = other;
                    None
                }
            };
            self.snapshot = CanonicalSnapshot::ReadOnly {
                reason: ManifestDegradedReason::InvalidCanonical,
                canonical,
            };
        }
    }

    pub(crate) fn inventory_prior_temps(&self) -> TempInventory {
        let mut inventory = TempInventory::default();
        let mut entries = match manifest_disk::manifest_temp_entries(&self.ac_root.path) {
            Ok(entries) => entries,
            Err(error) => {
                inventory.scan_truncated = true;
                inventory.scan_error = Some(error.to_string());
                return inventory;
            }
        };

        for _ in 0..MAX_TEMP_INVENTORY_ENTRIES {
            let Some(entry) = entries.next() else {
                return inventory;
            };
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    inventory.entry_errors = inventory.entry_errors.saturating_add(1);
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                if native_name_has_temp_shape(&name) {
                    inventory.malformed_count = inventory.malformed_count.saturating_add(1);
                    inventory.malformed_native_count =
                        inventory.malformed_native_count.saturating_add(1);
                    if inventory.malformed_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                        inventory
                            .malformed_samples
                            .push("<native-non-unicode-temp-name>".to_string());
                    }
                }
                continue;
            };
            if is_exact_temp_name(name) {
                inventory.exact_count = inventory.exact_count.saturating_add(1);
                if inventory.exact_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                    inventory.exact_samples.push(name.to_string());
                }
                inventory.exact_names.push(name.to_string());
            } else if name.starts_with(SEED_MANIFEST_TEMP_PREFIX)
                && name.ends_with(SEED_MANIFEST_TEMP_SUFFIX)
            {
                inventory.malformed_count = inventory.malformed_count.saturating_add(1);
                if inventory.malformed_samples.len() < MAX_TEMP_DIAGNOSTIC_SAMPLES {
                    inventory.malformed_samples.push(name.to_string());
                }
            }
        }
        inventory.scan_truncated = true;
        inventory
    }

    pub(crate) fn process_prior_temp_inventory(
        &self,
        inventory: TempInventory,
        valid_canonical: bool,
    ) {
        #[cfg(not(unix))]
        let _ = valid_canonical;
        #[cfg(unix)]
        let mut removed = 0_u64;
        #[cfg(unix)]
        let mut preserved = inventory.exact_count;
        #[cfg(not(unix))]
        let removed = 0_u64;
        #[cfg(not(unix))]
        let preserved = inventory.exact_count;

        #[cfg(unix)]
        if valid_canonical
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
            && inventory.entry_errors == 0
        {
            for name in &inventory.exact_names {
                let path = self.ac_root.path.join(name);
                if remove_prior_unix_temp_if_safe(&path).is_ok() {
                    removed = removed.saturating_add(1);
                    preserved = preserved.saturating_sub(1);
                }
            }
        }

        if inventory.exact_count == 0
            && inventory.malformed_count == 0
            && inventory.entry_errors == 0
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
        {
            return;
        }

        #[cfg(windows)]
        let policy = "WindowsPriorTempRecoveryCandidate";
        #[cfg(unix)]
        let policy = if valid_canonical
            && !inventory.scan_truncated
            && inventory.scan_error.is_none()
            && inventory.entry_errors == 0
        {
            "UnixValidCanonicalIdentityProof"
        } else {
            "UnixPreserveAmbiguousPriorTemp"
        };
        #[cfg(not(any(unix, windows)))]
        let policy = "UnsupportedPlatformPreservePriorTemp";

        #[cfg(test)]
        if let Some(hook) = &self.hooks.on_prior_temp_diagnostic {
            hook(&inventory);
        }

        log::warn!(
            "[seed_manifest] bounded prior-temp inventory root={} policy={} exact={} malformed={} \
                malformed_native={} removed={} preserved={} entry_errors={} scan_truncated={} \
                scan_error={:?} exact_samples={:?} malformed_samples={:?}",
            self.ac_root.path.display(),
            policy,
            inventory.exact_count,
            inventory.malformed_count,
            inventory.malformed_native_count,
            removed,
            preserved,
            inventory.entry_errors,
            inventory.scan_truncated,
            inventory.scan_error,
            inventory.exact_samples,
            inventory.malformed_samples
        );
    }

    pub(crate) fn persist_current_state(
        &mut self,
    ) -> Result<ManifestRecordOutcome, SeedManifestError> {
        let (bytes, current_raw, empty_absent) = match &self.snapshot {
            CanonicalSnapshot::Writable {
                raw,
                state,
                canonical,
            } => (
                serialize_state(state)?,
                raw.as_slice(),
                state.rows.is_empty() && canonical.is_none(),
            ),
            CanonicalSnapshot::ReadOnly { reason, .. } => {
                return Ok(ManifestRecordOutcome::PublishedUnrecorded(*reason));
            }
        };
        if empty_absent || bytes == current_raw {
            return Ok(ManifestRecordOutcome::Unchanged);
        }
        self.write_canonical(bytes)?;
        Ok(ManifestRecordOutcome::Recorded)
    }

    pub(crate) fn write_canonical(&mut self, bytes: Vec<u8>) -> Result<(), SeedManifestError> {
        self.revalidate_owner()?;
        let canonical_path = self.ac_root.path.join(SEED_MANIFEST_FILENAME);
        let temp_path = self.unique_temp_path();
        let mut temp = OpenedRegularFile::create_new(&temp_path)?;
        let write_result = temp
            .file
            .write_all(&bytes)
            .and_then(|_| temp.file.flush())
            .and_then(|_| temp.file.sync_all());
        if let Err(source) = write_result {
            self.cleanup_owned_temp(&temp, None);
            return Err(SeedManifestError::TempFile {
                operation: "write-flush-sync",
                path: temp_path,
                source,
            });
        }
        if let Err(error) = temp.refresh_facts() {
            self.cleanup_owned_temp(&temp, None);
            return Err(error);
        }

        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_temp_validation {
            hook(&temp_path);
        }

        if let Err(error) = verify_temp_path(&temp_path, &temp, &bytes) {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }
        if let Err(error) = self.revalidate_owner() {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }

        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_raw_conflict_check {
            hook(&canonical_path);
        }

        if let Err(error) = self.verify_canonical_unchanged(&canonical_path) {
            self.cleanup_owned_temp(&temp, Some(&bytes));
            return Err(error);
        }

        #[cfg(windows)]
        let published = self.publish_windows_temp(temp, &canonical_path, &bytes)?;

        #[cfg(not(windows))]
        let published = {
            let had_canonical = matches!(
                self.snapshot,
                CanonicalSnapshot::Writable {
                    canonical: Some(_),
                    ..
                }
            );
            if let Err(error) = atomic_publish_manifest(&temp_path, &canonical_path, had_canonical)
            {
                self.cleanup_owned_temp(&temp, Some(&bytes));
                return Err(error);
            }

            #[cfg(unix)]
            if let Err(error) = self.ac_root.file.sync_all() {
                log::warn!(
                    "[seed_manifest] parent-directory sync failed after canonical replace root={} \
                        error={}",
                    self.ac_root.path.display(),
                    error
                );
            }

            temp.path = canonical_path.clone();
            temp.length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            temp
        };
        match &mut self.snapshot {
            CanonicalSnapshot::Writable { raw, canonical, .. } => {
                *raw = bytes;
                *canonical = Some(published);
            }
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only manifest unexpectedly reached the atomic writer".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn verify_canonical_unchanged(
        &self,
        canonical_path: &Path,
    ) -> Result<(), SeedManifestError> {
        let (expected_raw, expected_identity) = match &self.snapshot {
            CanonicalSnapshot::Writable { raw, canonical, .. } => {
                if let Some(canonical) = canonical {
                    let facts =
                        handle_facts(&canonical.file).map_err(|source| SeedManifestError::Io {
                            operation: "reinspect retained canonical handle",
                            path: canonical_path.to_path_buf(),
                            source,
                        })?;
                    if facts.identity != canonical.identity
                        || !facts.is_regular_file
                        || facts.is_reparse
                    {
                        return Err(SeedManifestError::ExternalEditConflict {
                            path: canonical_path.to_path_buf(),
                        });
                    }
                }
                (raw.as_slice(), canonical.as_ref().map(|file| file.identity))
            }
            CanonicalSnapshot::ReadOnly { .. } => {
                return Err(SeedManifestError::Validation(
                    "read-only manifest cannot perform a raw conflict check".to_string(),
                ));
            }
        };

        match open_regular_no_follow(canonical_path, false, false) {
            Ok(file) => {
                let mut reopened = OpenedRegularFile::from_file(canonical_path, file)?;
                if expected_identity != Some(reopened.identity)
                    || !stream_matches(&mut reopened.file, expected_raw).map_err(|source| {
                        SeedManifestError::Io {
                            operation: "stream canonical conflict comparison",
                            path: canonical_path.to_path_buf(),
                            source,
                        }
                    })?
                {
                    return Err(SeedManifestError::ExternalEditConflict {
                        path: canonical_path.to_path_buf(),
                    });
                }
                Ok(())
            }
            Err(error) if is_exact_not_found(&error) && expected_identity.is_none() => Ok(()),
            Err(error) if is_exact_not_found(&error) => {
                Err(SeedManifestError::ExternalEditConflict {
                    path: canonical_path.to_path_buf(),
                })
            }
            Err(error) => Err(classify_open_error(
                canonical_path,
                error,
                "reopen canonical manifest for raw conflict comparison",
                false,
            )),
        }
    }

    pub(crate) fn unique_temp_path(&self) -> PathBuf {
        self.ac_root.path.join(format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::new_v4().hyphenated()
        ))
    }

    pub(crate) fn cleanup_owned_temp(
        &self,
        temp: &OpenedRegularFile,
        expected_bytes: Option<&[u8]>,
    ) {
        if let Err(error) = cleanup_current_owned_temp(temp, expected_bytes) {
            log::warn!(
                "[seed_manifest] owned temp cleanup left path={} error={}",
                temp.path.display(),
                error
            );
        }
    }
}

impl Drop for ProjectSeedManifestGuard {
    fn drop(&mut self) {
        self.unlock_explicitly();
    }
}
