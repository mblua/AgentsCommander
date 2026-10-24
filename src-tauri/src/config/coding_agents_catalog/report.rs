use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::seed_manifest::{has_catalog_publication, SEED_MANIFEST_FILENAME};
use crate::config::settings::AppSettings;

use super::{
    manifest_path, primary_project_root, read_catalog_snapshot, resolve_catalog_snapshot,
    CatalogReadScope, CodingAgentDefinition, REPORT_CODE_PUBLICATION_UNTRACKED,
};

// ---------------------------------------------------------------------------
// #1963 P1 - persisted-only catalog report (read-only diagnostic view).
//
// `get_coding_agent_catalog_report` serves this report so the frontend can show
// persisted catalog availability and warnings BEFORE the managed-catalog
// migration is enabled. The resolver NEVER seeds, refreshes, creates
// directories, writes files, takes locks or backfills from the embedded default:
// there is no embedded command donor on this path. It returns the persisted
// definitions after the same per-entry validation, duplicate-key and built-in
// support filters the array endpoint applies, plus visible diagnostics.
//
// P5 (#1968) extends this same resolver to compose the local layer; the wire
// shape below stays fixed. Since P4 (#1967) the array endpoints and the updater
// resolve through the SAME resolver via `CatalogReport::into_result`, so they
// can never disagree with the report about availability.
// ---------------------------------------------------------------------------

/// Diagnostic codes of the persisted-catalog report. The `code` field is a
/// plain string so P5 can add its own codes without a wire change
/// (`localInvalid`, `refreshFailed`, `migrationConflict`, `managedBaseEdited`,
/// `publicationUntracked`).
pub(crate) const REPORT_CODE_BASE_UNAVAILABLE: &str = "baseUnavailable";
pub(crate) const REPORT_CODE_BASE_INVALID: &str = "baseInvalid";
pub(crate) const REPORT_CODE_INVALID_DEFINITION: &str = "invalidDefinition";
pub(crate) const REPORT_CODE_DUPLICATE_KEY: &str = "duplicateKey";
pub(crate) const REPORT_CODE_MIGRATION_PENDING: &str = "migrationPending";

/// The authored definition fields the current schema recognizes. A legacy row
/// carrying anything else stays READABLE (serde ignores unknown fields) but its
/// unknown data emits migrationPending: P5 must not take ownership over data it
/// cannot safely migrate.
pub(crate) const KNOWN_DEFINITION_FIELDS: &[&str] = &[
    "key",
    "label",
    "description",
    "color",
    "command",
    "baseCommand",
    "instructionsFilename",
    "envs",
    "isolatedHome",
    "configSeed",
    "removable",
    "updateCommands",
    "autoUpdate",
    "idleBurst",
    "installCommands",
];
pub(crate) const KNOWN_CONFIG_SEED_FIELDS: &[&str] = &["enabled", "dest"];
pub(crate) const KNOWN_INSTALL_COMMANDS_FIELDS: &[&str] = &["default", "windows", "macos", "linux"];
pub(crate) const KNOWN_ENV_FIELDS: &[&str] = &["key", "value", "source", "enabled"];
pub(crate) const KNOWN_ROOT_FIELDS: &[&str] = &[
    "schemaVersion",
    "agents",
    "managed",
    "profileBindingVersion",
    "profilesByBaseHash",
    "profileLabelsByBaseHash",
    "profileSlots",
    "legacyProfileOrphans",
    "codingAgentProfiles",
    "profileLabels",
];

/// One visible report warning or unavailability record: a stable code plus the
/// affected path and an actionable reason. Reasons never echo command text or
/// environment values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDiagnostic {
    pub code: String,
    pub path: String,
    pub reason: String,
}

/// The persisted-only catalog report wire shape (additive IPC; P5 extends the
/// resolver behind it, not the shape). `primaryProjectRoot` is the exact
/// trimmed root selected by `primary_project_root` (`None` only in no-project
/// mode); `sourcePath` is the selected agents.json path even when absent
/// (`None` only when the config dir cannot be resolved). `unavailable` present
/// always comes with an empty catalog; a valid empty catalog is a success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReport {
    pub primary_project_root: Option<String>,
    pub source_path: Option<String>,
    pub catalog: Vec<CodingAgentDefinition>,
    pub warnings: Vec<CatalogDiagnostic>,
    pub unavailable: Option<CatalogDiagnostic>,
}

impl CatalogReport {
    /// #1967 P4 - the array-shaped view of this report, shared by every array
    /// consumer (CLI, Tauri command, WebSocket, updater). Every warning is
    /// logged (the report IPC carries the same records structurally), a readable
    /// catalog - including a valid empty one - becomes `Ok`, and `unavailable`
    /// becomes the typed error instead of an empty success.
    pub fn into_result(self) -> Result<Vec<CodingAgentDefinition>, CatalogUnavailable> {
        for warning in &self.warnings {
            log::warn!(
                "[coding-agents] {} at {}: {}",
                warning.code,
                warning.path,
                warning.reason
            );
        }
        match self.unavailable {
            Some(unavailable) => Err(unavailable.into()),
            None => Ok(self.catalog),
        }
    }
}

/// #1967 P4 - the typed unavailability of an array-shaped catalog read: the
/// report's diagnostic (stable code, affected path, sanitized reason) as an
/// error. `Display` renders `catalog unavailable (code) at path: reason`; the
/// CLI and IPC boundaries stringify it, so code/path/reason survive intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogUnavailable {
    pub code: String,
    pub path: String,
    pub reason: String,
}

impl std::fmt::Display for CatalogUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "catalog unavailable ({}): {}", self.code, self.reason)
        } else {
            write!(
                f,
                "catalog unavailable ({}) at {}: {}",
                self.code, self.path, self.reason
            )
        }
    }
}

impl std::error::Error for CatalogUnavailable {}

impl From<CatalogDiagnostic> for CatalogUnavailable {
    fn from(diagnostic: CatalogDiagnostic) -> Self {
        Self {
            code: diagnostic.code,
            path: diagnostic.path,
            reason: diagnostic.reason,
        }
    }
}

pub(crate) fn catalog_diagnostic(
    code: &str,
    path: &Path,
    reason: impl Into<String>,
) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code: code.to_string(),
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

/// The report when no catalog location can be resolved at all (the config dir
/// itself is unavailable): `sourcePath` is null and there is no path to
/// report, so the diagnostic carries an empty path.
pub(crate) fn report_without_source() -> CatalogReport {
    CatalogReport {
        primary_project_root: None,
        source_path: None,
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: Some(CatalogDiagnostic {
            code: REPORT_CODE_BASE_UNAVAILABLE.to_string(),
            path: String::new(),
            reason: "the AgentsCommander config directory could not be resolved, so no persisted \
                catalog location is available"
                .to_string(),
        }),
    }
}

/// Read one optional catalog artifact. `Ok(None)` means the path does not
/// exist; a link, a reparse point, a nonregular entry, an unreadable path or an
/// inspect failure is an error, never a silent absence. Read-only by
/// construction.
pub(crate) fn read_optional_regular_file(
    path: &Path,
    what: &str,
) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(format!(
                    "the {what} at {} is a symbolic link or reparse point; a regular file is \
                        required",
                    path.display()
                ));
            }
            if !meta.file_type().is_file() {
                return Err(format!(
                    "the {what} at {} is not a regular file; a regular file is required",
                    path.display()
                ));
            }
            std::fs::read(path)
                .map(Some)
                .map_err(|e| format!("the {what} at {} could not be read ({e})", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "the {what} at {} could not be inspected ({e})",
            path.display()
        )),
    }
}

/// The instance legacy source is a REGULAR file or nothing at all: a directory,
/// a link or a missing path means "no instance catalog" and the fresh managed
/// default applies, exactly like the pre-#1968 seed rule. A regular file that
/// cannot be read is an error, never a silent reset.
pub(crate) fn read_instance_legacy_source(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink()
                || is_reparse_point(&meta)
                || !meta.file_type().is_file()
            {
                return Ok(None);
            }
            std::fs::read(path).map(Some).map_err(|e| {
                format!(
                    "the instance catalog at {} could not be read ({e})",
                    path.display()
                )
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "the instance catalog at {} could not be inspected ({error})",
            path.display()
        )),
    }
}

pub(crate) fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        // Reparse points are a Windows attribute; this parameter is only read
        // on Windows, so bind it explicitly to stay lint-clean under the
        // Linux/macOS clippy runs (`-D warnings`).
        let _ = metadata;
        false
    }
}

/// Which surface asked for a catalog report. The wire shape never changes;
/// only the restart guidance attached to a stale managed revision differs:
/// project catalogs refresh during every startup/registration, the instance
/// catalog is initialized or refreshed at startup only when no project is
/// registered (every read stays read-only), and direct callers receive the
/// neutral wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogSourceContext {
    Project,
    Instance,
    #[cfg(test)]
    Direct,
}

impl CatalogSourceContext {
    /// Exact `refreshFailed` reason for this surface (reviewed wording; the
    /// tests assert all three strings verbatim).
    pub(crate) fn stale_revision_reason(self) -> &'static str {
        match self {
            Self::Project => {
                "The persisted managed catalog revision differs from this build. Current persisted \
                    entries remain usable. Restart retries project catalog refresh."
            }
            Self::Instance => {
                "The persisted instance catalog revision differs from this build. Current \
                    persisted entries remain usable. Restart retries instance catalog refresh when \
                    no project is registered."
            }
            #[cfg(test)]
            Self::Direct => {
                "The persisted managed catalog revision differs from this build. Current persisted \
                    entries remain usable. Catalog refresh runs during initialization."
            }
        }
    }
}

/// Load the persisted-only catalog report for one catalog root directory (a
/// project's `.ac` dir, or the legacy `<config_dir>` root in no-project mode).
/// `primaryProjectRoot` is left `None` here; the settings wrapper fills it.
/// This public entry point speaks for DIRECT callers (the array adapter and
/// context-free CLI/IPC reads), so it always uses the neutral wording; the
/// settings wrapper threads its own project/instance context privately.
/// Direct reads resolve 10/50 only; callers needing the primary project layer
/// use [`load_catalog_report_for_settings`].
/// READ-ONLY: never seeds, creates directories, refreshes, locks or writes.
/// `unavailable` is set (with an empty catalog) for a missing/unreadable/
/// nonregular/link source, invalid JSON, an unsupported explicit schemaVersion
/// or an invalid root shape; a valid empty catalog is a success. A managed base
/// composes its local layer behind the same resolver; a legacy base stays
/// readable and reports the pending migration. Definitions keep persisted order
/// after filtering; no embedded donor is ever consulted.
#[cfg(test)]
pub fn load_catalog_report(ac_dir: &Path) -> CatalogReport {
    load_catalog_report_with_context(
        ac_dir,
        CatalogSourceContext::Direct,
        CatalogReadScope::Instance,
    )
    .0
}

pub(crate) fn load_catalog_report_with_context(
    ac_dir: &Path,
    context: CatalogSourceContext,
    scope: CatalogReadScope,
) -> (CatalogReport, bool) {
    let path = manifest_path(ac_dir);
    let mut report = CatalogReport {
        primary_project_root: None,
        source_path: Some(path.display().to_string()),
        catalog: Vec::new(),
        warnings: Vec::new(),
        unavailable: None,
    };
    let snapshot = match read_catalog_snapshot(ac_dir, scope) {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            report.unavailable = Some(catalog_diagnostic(
                REPORT_CODE_BASE_UNAVAILABLE,
                &path,
                reason,
            ));
            return (report, false);
        }
    };
    let resolved = resolve_catalog_snapshot(ac_dir, snapshot, context);
    report.catalog = resolved.catalog;
    report.warnings = resolved.warnings;
    report.unavailable = resolved.unavailable;
    (report, resolved.base_verified_managed)
}

/// Settings-level resolver: the primary project root (first nonblank
/// `project_paths` entry, else the legacy `project_path`) selects the project's
/// `.ac` dir; with no project the INSTANCE catalog at
/// `<config_dir>/coding-agents/agents.json` is read (reads never write; the
/// no-project boot path initializes or refreshes that file).
/// A failure in the primary project is never retried against another project.
/// P5 (#1968) extends this resolver to compose the local layer.
pub fn load_catalog_report_for_settings(settings: &AppSettings) -> CatalogReport {
    load_catalog_report_for_settings_with_config_dir(settings, crate::config::config_dir())
}

/// Testable twin of [`load_catalog_report_for_settings`] with the resolved
/// config dir injected, so the no-project and config-dir-none arms are
/// coverable without depending on the process-global instance location. A
/// verified managed PROJECT base additionally asks the seed manifest whether
/// the base publication is recorded; when it is not, the report carries
/// `publicationUntracked` while the verified base and local layer stay usable.
pub(crate) fn load_catalog_report_for_settings_with_config_dir(
    settings: &AppSettings,
    config_dir: Option<PathBuf>,
) -> CatalogReport {
    match primary_project_root(settings) {
        Some(root) => {
            let ac_dir = root.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
            let (mut report, verified_managed_base) = load_catalog_report_with_context(
                &ac_dir,
                CatalogSourceContext::Project,
                CatalogReadScope::Project,
            );
            report.primary_project_root = Some(root.to_string_lossy().to_string());
            if verified_managed_base {
                let untracked = match has_catalog_publication(&root) {
                    Ok(true) => false,
                    Ok(false) => true,
                    Err(error) => {
                        log::debug!(
                            "[coding-agents] seed-manifest bookkeeping query failed for {} \
                                ({error})",
                            root.display()
                        );
                        true
                    }
                };
                if untracked {
                    let manifest_path = ac_dir.join(SEED_MANIFEST_FILENAME);
                    report.warnings.push(catalog_diagnostic(
                        REPORT_CODE_PUBLICATION_UNTRACKED,
                        &manifest_path,
                        "the seed manifest does not record this managed catalog publication yet; \
                            reads remain usable from the verified base and local overrides and the \
                            next initialization records it",
                    ));
                }
            }
            report
        }
        None => match config_dir {
            // Deliberately the private context-aware builder, NOT the public
            // Direct wrapper: reads never seed or migrate the instance (the
            // no-project boot path initializes it), so its restart guidance
            // must say so.
            Some(dir) => {
                load_catalog_report_with_context(
                    &dir,
                    CatalogSourceContext::Instance,
                    CatalogReadScope::Instance,
                )
                .0
            }
            None => report_without_source(),
        },
    }
}
