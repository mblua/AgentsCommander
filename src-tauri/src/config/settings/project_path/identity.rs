use crate::filesystem::projects;
use crate::filesystem::settings_sources;
use std::ops::ControlFlow;
use std::path::Path;

// ── #1077 candidate validity and directory identity (§3.4) ─────────────────

/// Filesystem identity of a canonical directory. On Unix `(st_dev, st_ino)`; on
/// Windows `(dwVolumeSerialNumber, file index)` from an open handle. Used to
/// decide whether two valid candidate spellings name the same directory without
/// lowercasing path strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectoryIdentity {
    pub volume: u64,
    pub file: u128,
}

/// Whether a validated directory is a direct project (`<target>/.ac` is a dir)
/// or a legacy collection root (has a non-dot child project).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectKind {
    DirectProject,
    CollectionRoot,
    None,
}

/// Classified filesystem-probe failure. Kept distinct so a permission/I-O error
/// never collapses into "missing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeError {
    NotFound,
    PermissionDenied,
    Io,
}

/// Per-side resolution status (internal diagnostics, §3.7). `is_valid()` marks
/// the two loadable outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SideStatus {
    /// The field was absent (or its value was JSON null).
    Absent,
    /// No authoritative instance base to resolve a relative companion.
    BaseUnavailable,
    /// The stored value could not be parsed into an absolute candidate.
    Malformed,
    /// Definite `NotFound` at canonicalize/probe.
    Missing,
    /// `PermissionDenied` at canonicalize/probe.
    Inaccessible,
    /// Any other probe I/O failure.
    ProbeIoError,
    /// The canonical target exists but is not a directory.
    NotADirectory,
    /// The directory exists but is neither a direct project nor a collection root.
    AcRootOrCollectionMissing,
    /// The canonical path is not losslessly representable as UTF-8.
    NonUtf8,
    ValidDirectProject,
    ValidCollectionRoot,
    // Note: an identity-unavailable outcome (two spellings that cannot be proven
    // the same directory) is recorded at the pair level as an `Invalid` issue,
    // not as a per-side status, so both sides retain their valid classification.
}

impl SideStatus {
    pub(crate) fn is_valid(self) -> bool {
        matches!(
            self,
            SideStatus::ValidDirectProject | SideStatus::ValidCollectionRoot
        )
    }
}

/// Resolution outcome of one side (absolute or instance-relative) of a logical
/// registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SideOutcome {
    pub status: SideStatus,
    /// The syntactically resolved absolute path (pre-probe), for diagnostics.
    pub syntactic_path: Option<String>,
    /// The raw canonical path (verbatim spelling on Windows) when validated.
    /// Used for comparison and re-encoding; the outward/selected form is derived
    /// via `display_canonical`.
    pub canonical_path: Option<String>,
    /// Directory identity when it could be obtained (best-effort).
    pub identity: Option<DirectoryIdentity>,
}

impl SideOutcome {
    pub(in crate::config) fn failed(status: SideStatus, syntactic: Option<String>) -> Self {
        SideOutcome {
            status,
            syntactic_path: syntactic,
            canonical_path: None,
            identity: None,
        }
    }
}

/// Injected seam: resolve one syntactically-absolute candidate path to a
/// [`SideOutcome`]. The production implementation canonicalizes, proves a
/// directory, probes project/collection kind, and fetches the handle identity;
/// tests supply canned outcomes so permission/I-O/identity matrices need no
/// `chmod` or process-global hooks (§3.4).
pub(crate) trait CandidateResolver {
    fn resolve(&self, syntactic: &Path) -> SideOutcome;
}

/// Production resolver backed by the real filesystem.
pub(crate) struct FsCandidateResolver;

impl CandidateResolver for FsCandidateResolver {
    fn resolve(&self, syntactic: &Path) -> SideOutcome {
        validate_fs_candidate(syntactic)
    }
}

pub(super) fn classify_io_error(e: &std::io::Error) -> SideStatus {
    match e.kind() {
        std::io::ErrorKind::NotFound => SideStatus::Missing,
        std::io::ErrorKind::PermissionDenied => SideStatus::Inaccessible,
        _ => SideStatus::ProbeIoError,
    }
}

pub(super) fn classify_probe_error(e: &std::io::Error) -> ProbeError {
    match e.kind() {
        std::io::ErrorKind::NotFound => ProbeError::NotFound,
        std::io::ErrorKind::PermissionDenied => ProbeError::PermissionDenied,
        _ => ProbeError::Io,
    }
}

/// Real-filesystem validation of a syntactically-absolute candidate. Follows a
/// user-supplied symlink/junction to its canonical target (distinct from
/// `is_real_directory`, which rejects links for child enumeration).
pub(super) fn validate_fs_candidate(syntactic: &Path) -> SideOutcome {
    let syntactic_str = syntactic.to_str().map(str::to_string);
    let canon = match projects::canonical_project_path(syntactic) {
        Ok(c) => c,
        Err(e) => return SideOutcome::failed(classify_io_error(&e), syntactic_str),
    };
    let canonical_str = match canon.to_str() {
        Some(s) => s.to_string(),
        None => return SideOutcome::failed(SideStatus::NonUtf8, syntactic_str),
    };
    match projects::project_path_metadata(&canon) {
        Ok(md) if md.is_dir() => {}
        Ok(_) => {
            return SideOutcome {
                status: SideStatus::NotADirectory,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(e) => {
            return SideOutcome {
                status: classify_io_error(&e),
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
    }
    let status = match probe_project_kind(&canon) {
        Ok(ProjectKind::DirectProject) => SideStatus::ValidDirectProject,
        Ok(ProjectKind::CollectionRoot) => SideStatus::ValidCollectionRoot,
        Ok(ProjectKind::None) => {
            return SideOutcome {
                status: SideStatus::AcRootOrCollectionMissing,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(ProbeError::NotFound) => {
            return SideOutcome::failed(SideStatus::Missing, syntactic_str);
        }
        Err(ProbeError::PermissionDenied) => {
            return SideOutcome {
                status: SideStatus::Inaccessible,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
        Err(ProbeError::Io) => {
            return SideOutcome {
                status: SideStatus::ProbeIoError,
                syntactic_path: syntactic_str,
                canonical_path: Some(canonical_str),
                identity: None,
            };
        }
    };
    let identity = directory_identity(&canon).ok();
    SideOutcome {
        status,
        syntactic_path: syntactic_str,
        canonical_path: Some(canonical_str),
        identity,
    }
}

/// Probe whether `target` (already canonical + proven a directory) is a direct
/// project or a legacy one-level collection root. A permission/I-O failure that
/// prevents proving a collection root is surfaced, never flattened to "missing".
pub(super) fn probe_project_kind(target: &Path) -> Result<ProjectKind, ProbeError> {
    match settings_sources::marker(&target.join(".ac")) {
        Ok(md) if md.is_dir() => return Ok(ProjectKind::DirectProject),
        Ok(_) => {}
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => {}
            std::io::ErrorKind::PermissionDenied => return Err(ProbeError::PermissionDenied),
            _ => return Err(ProbeError::Io),
        },
    }

    let mut probe_error: Option<ProbeError> = None;
    let outcome = projects::visit_project_kind_children(target, |entry| {
        let entry = match entry {
            Ok(en) => en,
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                return ControlFlow::Continue(());
            }
        };
        let name = entry.file_name().unwrap_or_default();
        if name.to_string_lossy().starts_with('.') {
            return ControlFlow::Continue(());
        }
        let child = entry;
        match projects::project_path_metadata(&child) {
            Ok(md) if md.is_dir() => {}
            Ok(_) => return ControlFlow::Continue(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ControlFlow::Continue(()),
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                return ControlFlow::Continue(());
            }
        }
        match settings_sources::marker(&child.join(".ac")) {
            Ok(md) if md.is_dir() => ControlFlow::Break(()),
            Ok(_) => ControlFlow::Continue(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ControlFlow::Continue(()),
            Err(e) => {
                probe_error.get_or_insert(classify_probe_error(&e));
                ControlFlow::Continue(())
            }
        }
    })
    .map_err(|e| classify_probe_error(&e))?;
    if outcome.is_break() {
        return Ok(ProjectKind::CollectionRoot);
    }
    match probe_error {
        Some(err) => Err(err),
        None => Ok(ProjectKind::None),
    }
}

pub(super) fn directory_identity(path: &Path) -> std::io::Result<DirectoryIdentity> {
    projects::project_directory_identity(path)
        .map(|(volume, file)| DirectoryIdentity { volume, file })
}

/// Whether two validated candidates name the same directory. Canonical string
/// equality short-circuits to `Same`; otherwise identity decides. When identity
/// cannot be established (unix/windows handle failure) the result is
/// `Unavailable` and the caller fails closed. On other platforms canonical
/// equality is the sole, documented signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SameDir {
    Same,
    Different,
    Unavailable,
}

pub(crate) fn compare_dirs(
    a_canonical: &str,
    a_identity: Option<&DirectoryIdentity>,
    b_canonical: &str,
    b_identity: Option<&DirectoryIdentity>,
) -> SameDir {
    if a_canonical == b_canonical {
        return SameDir::Same;
    }
    #[cfg(any(unix, windows))]
    {
        match (a_identity, b_identity) {
            (Some(x), Some(y)) => {
                if x == y {
                    SameDir::Same
                } else {
                    SameDir::Different
                }
            }
            _ => SameDir::Unavailable,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a_identity, b_identity);
        SameDir::Different
    }
}

/// Convert a canonical path to its outward/selected form: strip only the
/// ordinary Windows verbatim drive/UNC prefixes for display, preserving any
/// unsupported device-namespace spelling losslessly (§3.4).
#[cfg(windows)]
pub(crate) fn display_canonical(canonical: &str) -> String {
    if let Some(rest) = canonical.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = canonical.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            rest.to_string()
        } else {
            canonical.to_string()
        }
    } else {
        canonical.to_string()
    }
}

#[cfg(not(windows))]
pub(crate) fn display_canonical(canonical: &str) -> String {
    canonical.to_string()
}
