//! Startup-only application-state root selection for the isolated validation
//! package. This is intentionally the only module that chooses a state root.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::path_identity::{
    read_bounded_regular, retain_directory, retain_or_create_verified_child_directory, same_object,
    verify_directory, FileObjectId, RetainedDirectory, VerifiedPathIdentity,
};

const PROFILE_PROJECT_CONTAINER: &str = "profile-project";
const PROFILE_WORKGROUP_DIRECTORY: &str = "wg-1271-isolated-gates";
const MARKER_FILENAME: &str = "isolation-root.toml";
const MAX_MARKER_BYTES: usize = 16 * 1024;
const MAX_SETTINGS_BYTES: usize = 4 * 1024 * 1024;
#[cfg(windows)]
const BOOTSTRAP_LOCK_WAIT_MS: u32 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CompiledPackageProfile {
    pub package_id: String,
    pub product_label: String,
    pub bundle_identifier: String,
    pub workspace: String,
    pub matrix: String,
    pub replica_agent: String,
    pub header_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupMode {
    Normal,
    Isolated(CompiledPackageProfile),
}

#[derive(Clone)]
pub struct ResolvedAppStateRoot {
    mode: StartupMode,
    config_root: Option<PathBuf>,
    root_identity: Option<VerifiedPathIdentity>,
    mutex_hash: Option<String>,
    retained_root: Option<RetainedDirectory>,
    retained_webview_data: Option<RetainedDirectory>,
}

impl std::fmt::Debug for ResolvedAppStateRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedAppStateRoot")
            .field("mode", &self.mode)
            .field("config_root", &self.config_root)
            .field("root_identity", &self.root_identity)
            .field("mutex_hash", &self.mutex_hash)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ResolvedAppStateRoot {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.config_root == other.config_root
            && self.root_identity == other.root_identity
            && self.mutex_hash == other.mutex_hash
    }
}

impl Eq for ResolvedAppStateRoot {}

impl ResolvedAppStateRoot {
    pub fn mode(&self) -> &StartupMode {
        &self.mode
    }

    pub fn config_root(&self) -> Option<PathBuf> {
        if matches!(self.mode, StartupMode::Normal) {
            return self.config_root.clone();
        }
        self.verified_isolated_root()
    }

    pub fn isolated_root(&self) -> Option<PathBuf> {
        match self.mode {
            StartupMode::Normal => None,
            StartupMode::Isolated(_) => self.verified_isolated_root(),
        }
    }

    pub fn isolated_profile(&self) -> Option<&CompiledPackageProfile> {
        match &self.mode {
            StartupMode::Normal => None,
            StartupMode::Isolated(profile) => Some(profile),
        }
    }

    fn root_identity(&self) -> Option<&VerifiedPathIdentity> {
        self.root_identity.as_ref()
    }

    pub fn mutex_hash(&self) -> Option<&str> {
        self.mutex_hash.as_deref()
    }

    fn verified_isolated_root(&self) -> Option<PathBuf> {
        let root = self.retained_root.as_ref()?;
        if root.verify_current().is_err() {
            log::error!("[isolated-state] retained root verification failed");
            return None;
        }
        Some(root.identity().canonical_path.clone())
    }

    fn verified_webview_data_directory(&self) -> Result<PathBuf, IsolationError> {
        let root = self.retained_root.as_ref().ok_or_else(|| {
            log::error!("[isolated-state] retained isolated root is unavailable for WebView data");
            IsolationError::UnsafePath
        })?;
        let webview_data = self.retained_webview_data.as_ref().ok_or_else(|| {
            log::error!("[isolated-state] retained WebView directory is unavailable");
            IsolationError::UnsafePath
        })?;
        if root.verify_current().is_err() || webview_data.verify_current().is_err() {
            log::error!("[isolated-state] retained WebView directory verification failed");
            return Err(IsolationError::UnsafePath);
        }
        Ok(webview_data.identity().canonical_path.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolationStatus {
    pub effective_root: String,
    pub package_id: String,
    pub profile_sha256: String,
    pub workspace: String,
    pub matrix: String,
    pub replica_agent: String,
    pub header_identity: String,
    pub bundle_identifier: String,
    pub mutex_hash: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IsolationError {
    #[error("unsupported")]
    Unsupported,
    #[error("invalid_root")]
    InvalidRoot,
    #[error("parent_unavailable")]
    ParentUnavailable,
    #[error("read_only")]
    ReadOnly,
    #[error("normal_root_overlap")]
    NormalRootOverlap,
    #[error("unsafe_path")]
    UnsafePath,
    #[error("bootstrap_lock")]
    BootstrapLock,
    #[error("marker_invalid")]
    MarkerInvalid,
    #[error("profile_invalid")]
    ProfileInvalid,
    #[error("install_conflict")]
    InstallConflict,
}

impl IsolationError {
    pub fn stderr_code(&self) -> &'static str {
        match self {
            Self::Unsupported => "E_ISOLATION_UNSUPPORTED",
            Self::InvalidRoot => "E_ISOLATION_INVALID_ROOT",
            Self::ParentUnavailable => "E_ISOLATION_PARENT_UNAVAILABLE",
            Self::ReadOnly => "E_ISOLATION_READ_ONLY",
            Self::NormalRootOverlap => "E_ISOLATION_NORMAL_ROOT_OVERLAP",
            Self::UnsafePath => "E_ISOLATION_PATH_NOT_SUPPORTED",
            Self::BootstrapLock => "E_ISOLATION_BOOTSTRAP_LOCK",
            Self::MarkerInvalid => "E_ISOLATION_MARKER_INVALID",
            Self::ProfileInvalid => "E_ISOLATION_PROFILE_INVALID",
            Self::InstallConflict => "E_ISOLATION_INSTALL_CONFLICT",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct IsolationMarker {
    package_id: String,
    profile_sha256: String,
    root_volume: u64,
    root_file: u64,
}

static INSTALLED_ROOT: OnceLock<ResolvedAppStateRoot> = OnceLock::new();

/// Parses the fixed, checked-in package profile only in the dedicated build.
/// Normal builds do not carry a profile and therefore fail the isolated option
/// closed before any state or UI initialization.
#[cfg(feature = "isolated-validation-package")]
pub fn compiled_package_profile() -> Option<&'static CompiledPackageProfile> {
    static PROFILE: OnceLock<Option<CompiledPackageProfile>> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            toml::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../packaging/isolated-validation/package-profile.toml"
            )))
            .ok()
        })
        .as_ref()
}

#[cfg(not(feature = "isolated-validation-package"))]
pub fn compiled_package_profile() -> Option<&'static CompiledPackageProfile> {
    None
}

#[cfg(feature = "isolated-validation-package")]
pub fn compiled_profile_sha256() -> Option<&'static str> {
    Some(env!("ISOLATED_PACKAGE_PROFILE_SHA256"))
}

#[cfg(not(feature = "isolated-validation-package"))]
pub fn compiled_profile_sha256() -> Option<&'static str> {
    None
}

/// Resolve and bootstrap an isolated root, or resolve the normal root exactly
/// once through the pre-existing instance-location resolver.
pub fn resolve_startup_state(
    isolated_state_root: Option<&Path>,
) -> Result<ResolvedAppStateRoot, IsolationError> {
    resolve_startup_state_with_normal_resolver(
        isolated_state_root,
        crate::config::normal_config_dir,
    )
}

fn resolve_startup_state_with_normal_resolver<F>(
    isolated_state_root: Option<&Path>,
    normal_config_dir: F,
) -> Result<ResolvedAppStateRoot, IsolationError>
where
    F: FnOnce() -> Option<PathBuf>,
{
    let Some(requested_root) = isolated_state_root else {
        return Ok(ResolvedAppStateRoot {
            mode: StartupMode::Normal,
            config_root: normal_config_dir(),
            root_identity: None,
            mutex_hash: None,
            retained_root: None,
            retained_webview_data: None,
        });
    };

    let profile = compiled_package_profile()
        .cloned()
        .ok_or(IsolationError::Unsupported)?;
    let profile_sha256 = compiled_profile_sha256().ok_or(IsolationError::Unsupported)?;
    let normal_roots = normal_root_exclusion_candidates();
    resolve_isolated_startup_state(requested_root, profile, profile_sha256, &normal_roots)
}

fn resolve_isolated_startup_state(
    requested_root: &Path,
    profile: CompiledPackageProfile,
    profile_sha256: &str,
    normal_roots: &[PathBuf],
) -> Result<ResolvedAppStateRoot, IsolationError> {
    let (parent_path, leaf) = validate_requested_root(requested_root)?;
    let parent = retain_directory(&parent_path).map_err(|_| IsolationError::ParentUnavailable)?;
    reject_read_only(&parent)?;
    parent
        .verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;

    let bootstrap_leaf = canonical_bootstrap_lock_leaf(&parent, &leaf);
    let bootstrap_name = bootstrap_mutex_name(
        &profile.package_id,
        parent.identity().object_id,
        &bootstrap_leaf,
    );
    let _bootstrap_lock = BootstrapLock::acquire(&bootstrap_name)?;
    parent
        .verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    reject_normal_root_parent_overlap(parent.identity(), normal_roots)?;
    parent
        .verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;

    let root = retain_or_create_verified_child_directory(&parent, &leaf)
        .map_err(|_| IsolationError::UnsafePath)?;
    reject_read_only(&root)?;
    reject_normal_root_overlap(root.identity(), normal_roots)?;

    let marker = IsolationMarker {
        package_id: profile.package_id.clone(),
        profile_sha256: profile_sha256.to_string(),
        root_volume: root.identity().object_id.volume,
        root_file: root.identity().object_id.file,
    };
    verify_or_write_marker(&root, &marker)?;
    let webview_data = bootstrap_isolated_root(&root, &profile)?;
    root.verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;

    let mutex_hash = root_mutex_hash(&profile.package_id, root.identity().object_id);
    Ok(ResolvedAppStateRoot {
        mode: StartupMode::Isolated(profile),
        config_root: Some(root.identity().canonical_path.clone()),
        root_identity: Some(root.identity().clone()),
        mutex_hash: Some(mutex_hash),
        retained_root: Some(root),
        retained_webview_data: Some(webview_data),
    })
}

/// Install the resolved state once, before logging, command dispatch, or GUI
/// singleton acquisition. Installing an equal value is harmless for tests that
/// call the startup seam twice in one process; a different value fails closed.
pub fn install_resolved_state_root(state: ResolvedAppStateRoot) -> Result<(), IsolationError> {
    if let Some(existing) = INSTALLED_ROOT.get() {
        if existing == &state {
            return Ok(());
        }
        return Err(IsolationError::InstallConflict);
    }
    INSTALLED_ROOT
        .set(state)
        .map_err(|_| IsolationError::InstallConflict)
}

pub fn active_state_root() -> Option<&'static ResolvedAppStateRoot> {
    INSTALLED_ROOT.get()
}

pub fn isolated_mode_active() -> bool {
    active_state_root().is_some_and(|state| matches!(state.mode(), StartupMode::Isolated(_)))
}

pub fn isolated_webview_data_directory() -> Result<Option<PathBuf>, IsolationError> {
    let Some(state) = active_state_root() else {
        return Ok(None);
    };

    if matches!(state.mode(), StartupMode::Normal) {
        return Ok(None);
    }

    state.verified_webview_data_directory().map(Some)
}

pub fn isolated_servers_disabled() -> bool {
    isolated_mode_active()
}

pub fn root_mutex_name() -> Option<&'static str> {
    if !isolated_mode_active() {
        return None;
    }
    static ROOT_MUTEX_NAME: OnceLock<String> = OnceLock::new();
    Some(ROOT_MUTEX_NAME.get_or_init(|| {
        let hash = active_state_root()
            .and_then(ResolvedAppStateRoot::mutex_hash)
            .expect("isolated state has a mutex hash");
        format!("Local\\AC-ISO-{hash}\0")
    }))
}

pub fn isolation_status() -> Result<IsolationStatus, IsolationError> {
    let state = active_state_root().ok_or(IsolationError::InstallConflict)?;
    let profile = state
        .isolated_profile()
        .ok_or(IsolationError::Unsupported)?;
    let root = state
        .isolated_root()
        .ok_or(IsolationError::InstallConflict)?;
    let _identity = state
        .root_identity()
        .ok_or(IsolationError::InstallConflict)?;
    Ok(IsolationStatus {
        effective_root: root.to_string_lossy().into_owned(),
        package_id: profile.package_id.clone(),
        profile_sha256: compiled_profile_sha256()
            .ok_or(IsolationError::Unsupported)?
            .to_string(),
        workspace: profile.workspace.clone(),
        matrix: profile.matrix.clone(),
        replica_agent: profile.replica_agent.clone(),
        header_identity: profile.header_identity.clone(),
        bundle_identifier: profile.bundle_identifier.clone(),
        mutex_hash: state
            .mutex_hash()
            .ok_or(IsolationError::InstallConflict)?
            .to_string(),
    })
}

/// Candidate normal roots used only to reject identity overlap. This function
/// intentionally never calls `resolve_instance_location` and ignores the test
/// config override, so an isolated invocation cannot inspect normal state.
pub fn normal_root_exclusion_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let (Some(parent), Some(stem)) = (executable.parent(), executable.file_stem()) {
            candidates.push(parent.join(format!(".{}", stem.to_string_lossy())));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(crate::config::profile::config_dir_name()));
    }
    candidates
}

fn validate_requested_root(
    requested_root: &Path,
) -> Result<(PathBuf, std::ffi::OsString), IsolationError> {
    if !requested_root.is_absolute() || requested_root.as_os_str().is_empty() {
        return Err(IsolationError::InvalidRoot);
    }
    if contains_raw_dot_segment(requested_root) {
        return Err(IsolationError::InvalidRoot);
    }

    #[cfg(windows)]
    {
        let Some(Component::Prefix(prefix)) = requested_root.components().next() else {
            return Err(IsolationError::InvalidRoot);
        };
        if !matches!(prefix.kind(), Prefix::Disk(_)) {
            return Err(IsolationError::InvalidRoot);
        }
    }

    let parent = requested_root.parent().ok_or(IsolationError::InvalidRoot)?;
    let leaf = requested_root
        .file_name()
        .filter(|leaf| !leaf.is_empty())
        .ok_or(IsolationError::InvalidRoot)?
        .to_os_string();
    if parent.as_os_str().is_empty() || !parent.is_dir() {
        return Err(IsolationError::ParentUnavailable);
    }
    Ok((parent.to_path_buf(), leaf))
}

fn contains_raw_dot_segment(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|component| matches!(component, "." | ".."))
}

fn reject_read_only(directory: &RetainedDirectory) -> Result<(), IsolationError> {
    directory
        .verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    if std::fs::metadata(directory.identity().canonical_path.as_path())
        .map_err(|_| IsolationError::UnsafePath)?
        .permissions()
        .readonly()
    {
        return Err(IsolationError::ReadOnly);
    }
    Ok(())
}

fn reject_normal_root_parent_overlap(
    parent: &VerifiedPathIdentity,
    normal_roots: &[PathBuf],
) -> Result<(), IsolationError> {
    for candidate in normal_roots {
        let Ok(candidate_identity) = verify_directory(candidate) else {
            continue;
        };
        if same_object(parent, &candidate_identity)
            || identity_is_at_or_below(parent, &candidate_identity)
        {
            return Err(IsolationError::NormalRootOverlap);
        }
    }
    Ok(())
}

fn reject_normal_root_overlap(
    root: &VerifiedPathIdentity,
    normal_roots: &[PathBuf],
) -> Result<(), IsolationError> {
    for candidate in normal_roots {
        let Ok(candidate_identity) = verify_directory(candidate) else {
            continue;
        };
        if same_object(root, &candidate_identity)
            || identity_is_at_or_below(root, &candidate_identity)
            || identity_is_at_or_below(&candidate_identity, root)
        {
            return Err(IsolationError::NormalRootOverlap);
        }
    }
    Ok(())
}

fn identity_is_at_or_below(
    descendant: &VerifiedPathIdentity,
    ancestor: &VerifiedPathIdentity,
) -> bool {
    let mut current = descendant.canonical_path.as_path();
    loop {
        if let Ok(identity) = verify_directory(current) {
            if same_object(&identity, ancestor) {
                return true;
            }
        } else {
            return false;
        }
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent == current {
            return false;
        }
        current = parent;
    }
}

fn verify_or_write_marker(
    root: &RetainedDirectory,
    expected: &IsolationMarker,
) -> Result<(), IsolationError> {
    let marker_path = root.identity().canonical_path.join(MARKER_FILENAME);
    if root.child_is_absent(&marker_path) {
        let bytes = toml::to_string(expected)
            .map_err(|_| IsolationError::MarkerInvalid)?
            .into_bytes();
        write_new_regular_file(root, MARKER_FILENAME, &bytes)
            .map_err(|_| IsolationError::MarkerInvalid)?;
        return Ok(());
    }

    root.verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    let (bytes, _) = read_bounded_regular(&marker_path, MAX_MARKER_BYTES)
        .map_err(|_| IsolationError::MarkerInvalid)?;
    let marker = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| toml::from_str::<IsolationMarker>(text).ok())
        .ok_or(IsolationError::MarkerInvalid)?;
    if marker.package_id != expected.package_id
        || marker.profile_sha256 != expected.profile_sha256
        || marker.root_volume != expected.root_volume
        || marker.root_file != expected.root_file
    {
        return Err(IsolationError::MarkerInvalid);
    }
    root.verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    Ok(())
}

/// Bootstrap the ordinary project and replica contracts underneath an already
/// marked root. Nothing here consults normal settings, session context, or a
/// WG-12 project tree.
pub fn bootstrap_isolated_root(
    root: &RetainedDirectory,
    profile: &CompiledPackageProfile,
) -> Result<RetainedDirectory, IsolationError> {
    let profile_container = retain_or_create_verified_state_child(root, PROFILE_PROJECT_CONTAINER)?;
    let project = retain_or_create_verified_state_child(&profile_container, &profile.workspace)?;
    let workspace = retain_or_create_verified_state_child(&project, ".ac")?;
    let matrix = retain_or_create_verified_state_child(
        &workspace,
        &format!("_agent_{}", profile.replica_agent),
    )?;
    let workgroup = retain_or_create_verified_state_child(&workspace, PROFILE_WORKGROUP_DIRECTORY)?;
    let replica = retain_or_create_verified_state_child(
        &workgroup,
        &format!("__agent_{}", profile.replica_agent),
    )?;
    let _instances = retain_or_create_verified_state_child(root, "instances")?;
    let _templates = retain_or_create_verified_state_child(root, "agent-templates")?;
    let _context_cache = retain_or_create_verified_state_child(root, "context-cache")?;
    let webview_data = retain_or_create_verified_state_child(root, "webview-data")?;

    let expected_identity = format!("../../_agent_{}", profile.replica_agent);
    let replica_config = serde_json::json!({
        "identity": expected_identity,
        "context": ["$AGENTSCOMMANDER_CONTEXT"],
    });
    write_or_validate_json_file(&replica, "config.json", &replica_config)?;

    // A minimally ordinary matrix directory gives the existing identity and
    // context readers a real target without inventing an isolation-only format.
    let matrix_config = serde_json::json!({
        "context": ["$AGENTSCOMMANDER_CONTEXT", "Role.md"],
    });
    write_or_validate_json_file(&matrix, "config.json", &matrix_config)?;
    write_or_validate_text_file(&matrix, "Role.md", "# Gate Tester\n")?;

    let project_path = project
        .identity()
        .canonical_path
        .to_string_lossy()
        .into_owned();
    let settings = crate::config::settings::AppSettings {
        project_path: Some(project_path.clone()),
        project_paths: vec![project_path],
        archived_project_paths: Vec::new(),
        web_server_enabled: false,
        api_server_enabled: false,
        ..Default::default()
    };
    let expected_settings =
        serde_json::to_value(settings).map_err(|_| IsolationError::ProfileInvalid)?;
    write_or_validate_settings(root, &expected_settings)?;

    root.verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    webview_data
        .verify_current()
        .map_err(|_| IsolationError::UnsafePath)?;
    Ok(webview_data)
}

fn retain_or_create_verified_state_child(
    parent: &RetainedDirectory,
    name: &str,
) -> Result<RetainedDirectory, IsolationError> {
    retain_or_create_verified_child_directory(parent, OsStr::new(name))
        .map_err(|_| IsolationError::UnsafePath)
}

fn write_or_validate_settings(
    root: &RetainedDirectory,
    expected: &serde_json::Value,
) -> Result<(), IsolationError> {
    let path = root.identity().canonical_path.join("settings.json");
    if root.child_is_absent(&path) {
        let bytes =
            serde_json::to_vec_pretty(expected).map_err(|_| IsolationError::ProfileInvalid)?;
        write_new_regular_file(root, "settings.json", &bytes)
            .map_err(|_| IsolationError::ProfileInvalid)?;
        return Ok(());
    }
    let (bytes, _) = read_bounded_regular(&path, MAX_SETTINGS_BYTES)
        .map_err(|_| IsolationError::ProfileInvalid)?;
    let current: crate::config::settings::AppSettings =
        serde_json::from_slice(&bytes).map_err(|_| IsolationError::ProfileInvalid)?;
    let expected_project = expected
        .get("projectPath")
        .and_then(serde_json::Value::as_str)
        .ok_or(IsolationError::ProfileInvalid)?;
    let expected_identity = verify_directory(Path::new(expected_project))
        .map_err(|_| IsolationError::ProfileInvalid)?;
    let profile_is_registered = current.project_paths.iter().any(|project_path| {
        verify_directory(Path::new(project_path))
            .is_ok_and(|identity| same_object(&identity, &expected_identity))
    });
    if !profile_is_registered {
        return Err(IsolationError::ProfileInvalid);
    }

    for configured_path in current
        .project_path
        .iter()
        .chain(current.project_paths.iter())
        .chain(current.archived_project_paths.iter())
    {
        let identity = verify_directory(Path::new(configured_path))
            .map_err(|_| IsolationError::ProfileInvalid)?;
        if !identity_is_at_or_below(&identity, root.identity()) {
            return Err(IsolationError::ProfileInvalid);
        }
    }
    Ok(())
}

fn write_or_validate_json_file(
    directory: &RetainedDirectory,
    filename: &str,
    expected: &serde_json::Value,
) -> Result<(), IsolationError> {
    let path = directory.identity().canonical_path.join(filename);
    if directory.child_is_absent(&path) {
        let bytes =
            serde_json::to_vec_pretty(expected).map_err(|_| IsolationError::ProfileInvalid)?;
        write_new_regular_file(directory, filename, &bytes)
            .map_err(|_| IsolationError::ProfileInvalid)?;
        return Ok(());
    }
    let (bytes, _) = read_bounded_regular(&path, MAX_MARKER_BYTES)
        .map_err(|_| IsolationError::ProfileInvalid)?;
    let actual: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| IsolationError::ProfileInvalid)?;
    let actual_object = actual.as_object().ok_or(IsolationError::ProfileInvalid)?;
    let expected_object = expected.as_object().ok_or(IsolationError::ProfileInvalid)?;
    for (key, expected_value) in expected_object {
        let Some(actual_value) = actual_object.get(key) else {
            return Err(IsolationError::ProfileInvalid);
        };
        if key == "context" {
            let Some(actual_context) = actual_value.as_array() else {
                return Err(IsolationError::ProfileInvalid);
            };
            let Some(expected_context) = expected_value.as_array() else {
                return Err(IsolationError::ProfileInvalid);
            };
            if expected_context
                .iter()
                .any(|entry| !actual_context.iter().any(|actual| actual == entry))
            {
                return Err(IsolationError::ProfileInvalid);
            }
            continue;
        }
        if actual_value != expected_value {
            return Err(IsolationError::ProfileInvalid);
        }
    }
    Ok(())
}

fn write_or_validate_text_file(
    directory: &RetainedDirectory,
    filename: &str,
    expected: &str,
) -> Result<(), IsolationError> {
    let path = directory.identity().canonical_path.join(filename);
    if directory.child_is_absent(&path) {
        write_new_regular_file(directory, filename, expected.as_bytes())
            .map_err(|_| IsolationError::ProfileInvalid)?;
        return Ok(());
    }
    let (bytes, _) = read_bounded_regular(&path, MAX_MARKER_BYTES)
        .map_err(|_| IsolationError::ProfileInvalid)?;
    if bytes != expected.as_bytes() {
        return Err(IsolationError::ProfileInvalid);
    }
    Ok(())
}

fn write_new_regular_file(
    directory: &RetainedDirectory,
    filename: &str,
    bytes: &[u8],
) -> Result<(), String> {
    directory.verify_current()?;
    let destination = directory.identity().canonical_path.join(filename);
    let temporary = directory
        .identity()
        .canonical_path
        .join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = directory.create_new_private_file(&temporary)?;
        file.write_all(bytes)
            .map_err(|_| "unsafe_path".to_string())?;
        file.sync_all().map_err(|_| "unsafe_path".to_string())?;
        drop(file);
        directory.publish_new_file_atomic(&temporary, &destination)?;
        directory.verify_current()?;
        let (published, _) = read_bounded_regular(&destination, bytes.len())?;
        if published != bytes {
            return Err("unsafe_path".to_string());
        }
        Ok(())
    })();
    if result.is_err() {
        if let Err((attempts, error_kind)) =
            remove_temporary_file_with_retries(&temporary, 3, |path| std::fs::remove_file(path))
        {
            let leaf = temporary
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or("<unknown>");
            log::warn!(
                "[isolated-state] temporary-file cleanup retained leaf={leaf:?} attempts={attempts} error_kind={error_kind:?}"
            );
        }
    }
    result
}

fn remove_temporary_file_with_retries<F>(
    path: &Path,
    max_attempts: u8,
    mut remove_file: F,
) -> Result<(), (u8, std::io::ErrorKind)>
where
    F: FnMut(&Path) -> std::io::Result<()>,
{
    let attempts = max_attempts.max(1);
    for attempt in 1..=attempts {
        match remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt == attempts => return Err((attempt, error.kind())),
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    unreachable!("a nonzero bounded cleanup loop always returns")
}

fn root_mutex_hash(package_id: &str, root_id: FileObjectId) -> String {
    let mut digest = Sha256::new();
    digest.update(package_id.as_bytes());
    digest.update(root_id.volume.to_le_bytes());
    digest.update(root_id.file.to_le_bytes());
    hex_digest(digest.finalize())
}

fn bootstrap_mutex_name(package_id: &str, parent_id: FileObjectId, leaf: &OsStr) -> String {
    let mut digest = Sha256::new();
    digest.update(package_id.as_bytes());
    digest.update(parent_id.volume.to_le_bytes());
    digest.update(parent_id.file.to_le_bytes());
    digest.update(canonical_bootstrap_leaf_key(leaf).as_bytes());
    format!("Local\\AC-ISO-BOOT-{}\0", hex_digest(digest.finalize()))
}

fn canonical_bootstrap_lock_leaf(parent: &RetainedDirectory, leaf: &OsStr) -> std::ffi::OsString {
    let candidate = parent.identity().canonical_path.join(leaf);
    verify_directory(&candidate)
        .ok()
        .and_then(|identity| identity.canonical_path.file_name().map(OsStr::to_os_string))
        .unwrap_or_else(|| leaf.to_os_string())
}

fn canonical_bootstrap_leaf_key(leaf: &OsStr) -> String {
    #[cfg(windows)]
    {
        leaf.to_string_lossy().to_uppercase()
    }
    #[cfg(not(windows))]
    {
        leaf.to_string_lossy().into_owned()
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(windows)]
struct BootstrapLock(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl BootstrapLock {
    fn acquire(name: &str) -> Result<Self, IsolationError> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

        const WAIT_OBJECT_0: u32 = 0;
        const WAIT_ABANDONED: u32 = 0x0000_0080;

        let wide: Vec<u16> = name.encode_utf16().collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(IsolationError::BootstrapLock);
        }
        let wait = unsafe { WaitForSingleObject(handle, BOOTSTRAP_LOCK_WAIT_MS) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            Ok(Self(handle))
        } else {
            unsafe {
                let _ = CloseHandle(handle);
            }
            Err(IsolationError::BootstrapLock)
        }
    }
}

#[cfg(windows)]
impl Drop for BootstrapLock {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;

        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct BootstrapLock;

#[cfg(not(windows))]
impl BootstrapLock {
    fn acquire(_name: &str) -> Result<Self, IsolationError> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bootstrap_isolated_root, bootstrap_mutex_name, remove_temporary_file_with_retries,
        resolve_isolated_startup_state, resolve_startup_state_with_normal_resolver,
        root_mutex_hash, validate_requested_root, CompiledPackageProfile, FileObjectId,
        IsolationError,
    };
    use crate::path_identity::retain_directory;
    use std::cell::Cell;
    use std::io::{Error, ErrorKind};
    use std::path::PathBuf;

    fn test_profile() -> CompiledPackageProfile {
        CompiledPackageProfile {
            package_id: "agentscommander-1271-isolated-gates".to_string(),
            product_label: "Agents Commander Isolated Gates".to_string(),
            bundle_identifier: "dev.agentscommander.isolatedgates".to_string(),
            workspace: "AgentsCommander_1271_isolated".to_string(),
            matrix: "WG-1271-ISOLATED-GATES".to_string(),
            replica_agent: "gate-tester".to_string(),
            header_identity: "WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated"
                .to_string(),
        }
    }

    #[test]
    fn root_mutex_hash_is_root_identity_bound() {
        let package = "agentscommander-1271-isolated-gates";
        let first = root_mutex_hash(package, FileObjectId { volume: 1, file: 2 });
        let second = root_mutex_hash(package, FileObjectId { volume: 1, file: 3 });
        assert_ne!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn bootstrap_mutex_name_does_not_contain_raw_path() {
        let name = bootstrap_mutex_name(
            "agentscommander-1271-isolated-gates",
            FileObjectId { volume: 7, file: 9 },
            std::ffi::OsStr::new("fixture root with spaces"),
        );
        assert!(name.starts_with("Local\\AC-ISO-BOOT-"));
        assert!(!name.contains("fixture root with spaces"));
        assert!(name.ends_with('\0'));
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_mutex_name_normalizes_windows_case_aliases() {
        let parent = FileObjectId { volume: 7, file: 9 };
        let lower = bootstrap_mutex_name(
            "agentscommander-1271-isolated-gates",
            parent,
            std::ffi::OsStr::new("app-state"),
        );
        let upper = bootstrap_mutex_name(
            "agentscommander-1271-isolated-gates",
            parent,
            std::ffi::OsStr::new("APP-STATE"),
        );
        assert_eq!(lower, upper);
    }

    #[test]
    fn root_validation_rejects_relative_paths() {
        assert_eq!(
            validate_requested_root(&PathBuf::from("relative/app-state")),
            Err(IsolationError::InvalidRoot)
        );
    }

    #[test]
    fn root_validation_rejects_raw_lexical_dot_segments() {
        let raw = if cfg!(windows) {
            PathBuf::from(r"C:\fixture\.\app-state")
        } else {
            PathBuf::from("/fixture/./app-state")
        };
        assert_eq!(
            validate_requested_root(&raw),
            Err(IsolationError::InvalidRoot)
        );
    }

    #[test]
    fn normal_root_overlap_is_rejected_before_the_requested_leaf_is_created() {
        let fixture = tempfile::TempDir::new().unwrap();
        let normal_root = fixture.path().join("normal-root");
        let requested_root = normal_root.join("new-isolated-leaf");
        std::fs::create_dir(&normal_root).unwrap();

        let result = resolve_isolated_startup_state(
            &requested_root,
            test_profile(),
            "profile-hash",
            std::slice::from_ref(&normal_root),
        );

        assert_eq!(result.err(), Some(IsolationError::NormalRootOverlap));
        assert!(
            !requested_root.exists(),
            "normal-root overlap must be rejected before the leaf can be created"
        );
        assert!(
            normal_root.read_dir().unwrap().next().is_none(),
            "a rejected overlap must not create a marker, profile, or child in normal state"
        );
    }

    #[test]
    fn normal_startup_calls_its_default_resolver_exactly_once() {
        let calls = Cell::new(0);
        let expected = PathBuf::from("C:/normal-root");
        let state = resolve_startup_state_with_normal_resolver(None, || {
            calls.set(calls.get() + 1);
            Some(expected.clone())
        })
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(state.config_root(), Some(expected));
    }

    #[cfg(not(feature = "isolated-validation-package"))]
    #[test]
    fn isolated_startup_never_calls_the_normal_resolver_when_the_feature_is_absent() {
        let calls = Cell::new(0);
        let result = resolve_startup_state_with_normal_resolver(
            Some(PathBuf::from("C:/fixture/app-state").as_path()),
            || {
                calls.set(calls.get() + 1);
                Some(PathBuf::from("C:/normal-root"))
            },
        );

        assert_eq!(result.err(), Some(IsolationError::Unsupported));
        assert_eq!(calls.get(), 0);
    }

    #[cfg(feature = "isolated-validation-package")]
    #[test]
    fn isolated_startup_never_calls_the_normal_resolver_with_a_normal_sentinel() {
        let fixture = tempfile::TempDir::new().unwrap();
        let requested_root = fixture.path().join("app-state");
        let calls = Cell::new(0);
        let state = resolve_startup_state_with_normal_resolver(Some(&requested_root), || {
            calls.set(calls.get() + 1);
            Some(PathBuf::from("C:/normal-root-sentinel"))
        })
        .expect("feature build resolves an isolated root without consulting normal state");

        assert_eq!(calls.get(), 0);
        assert_eq!(
            state.isolated_root(),
            Some(std::fs::canonicalize(requested_root).unwrap())
        );
    }

    #[test]
    fn bootstrap_seeds_an_ordinary_profile_project_and_replica_identity() {
        let fixture = tempfile::TempDir::new().unwrap();
        let root_path = fixture.path().join("app-state");
        std::fs::create_dir(&root_path).unwrap();
        let root = retain_directory(&root_path).unwrap();
        let profile = test_profile();

        bootstrap_isolated_root(&root, &profile).unwrap();

        let project = root_path
            .join("profile-project")
            .join("AgentsCommander_1271_isolated");
        let replica = project
            .join(".ac")
            .join("wg-1271-isolated-gates")
            .join("__agent_gate-tester");
        let identity = crate::config::replica_identity::expected_wg_replica_identity(&replica)
            .expect("seeded replica must use the ordinary identity contract");
        assert_eq!(identity.identity, "../../_agent_gate-tester");

        let settings: crate::config::settings::AppSettings =
            serde_json::from_slice(&std::fs::read(root_path.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            settings.project_paths,
            vec![std::fs::canonicalize(&project)
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        assert!(!settings.web_server_enabled);
        assert!(!settings.api_server_enabled);
        assert!(root_path.join("webview-data").is_dir());

        let replica_config_path = replica.join("config.json");
        let mut replica_config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&replica_config_path).unwrap()).unwrap();
        replica_config["context"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("local-notes.md"));
        replica_config["repos"] = serde_json::json!(["../repo-fixture"]);
        std::fs::write(
            &replica_config_path,
            serde_json::to_vec_pretty(&replica_config).unwrap(),
        )
        .unwrap();

        let fixture_project = root_path.join("profile-project").join("FixtureProject");
        std::fs::create_dir_all(fixture_project.join(".ac")).unwrap();
        let mut settings_with_fixture = settings;
        settings_with_fixture.project_paths.push(
            std::fs::canonicalize(&fixture_project)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        std::fs::write(
            root_path.join("settings.json"),
            serde_json::to_vec_pretty(&settings_with_fixture).unwrap(),
        )
        .unwrap();

        bootstrap_isolated_root(&root, &profile)
            .expect("ordinary in-root settings and replica additions remain valid on relaunch");

        let external_project = fixture.path().join("external-project");
        std::fs::create_dir(&external_project).unwrap();
        settings_with_fixture.project_path = Some(
            std::fs::canonicalize(&external_project)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        std::fs::write(
            root_path.join("settings.json"),
            serde_json::to_vec_pretty(&settings_with_fixture).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            bootstrap_isolated_root(&root, &profile),
            Err(IsolationError::ProfileInvalid)
        ));
    }

    #[test]
    fn retained_root_and_webview_handles_block_or_detect_post_validation_replacement() {
        let fixture = tempfile::TempDir::new().unwrap();
        let root_path = fixture.path().join("app-state");
        let state = resolve_isolated_startup_state(&root_path, test_profile(), "profile-hash", &[])
            .expect("fixture root bootstraps");

        let retired_root = fixture.path().join("retired-root");
        if std::fs::rename(&root_path, &retired_root).is_ok() {
            std::fs::create_dir(&root_path).unwrap();
            assert!(state.config_root().is_none());
            return;
        }
        assert!(state.config_root().is_some());

        let webview_path = root_path.join("webview-data");
        let retired_webview = root_path.join("retired-webview-data");
        if std::fs::rename(&webview_path, &retired_webview).is_ok() {
            std::fs::create_dir(&webview_path).unwrap();
            assert!(state.verified_webview_data_directory().is_err());
        } else {
            assert!(state.verified_webview_data_directory().is_ok());
        }
    }

    #[test]
    fn bootstrap_state_routing_keeps_every_profile_path_below_the_retained_root() {
        let fixture = tempfile::TempDir::new().unwrap();
        let root_path = fixture.path().join("app-state");
        std::fs::create_dir(&root_path).unwrap();
        let root = retain_directory(&root_path).unwrap();
        let webview = bootstrap_isolated_root(&root, &test_profile()).unwrap();

        for directory in [
            root_path.join("instances"),
            root_path.join("agent-templates"),
            root_path.join("context-cache"),
            root_path.join("profile-project"),
            webview.identity().canonical_path.clone(),
        ] {
            let identity = crate::path_identity::verify_directory(&directory).unwrap();
            assert!(super::identity_is_at_or_below(&identity, root.identity()));
        }

        let settings: crate::config::settings::AppSettings =
            serde_json::from_slice(&std::fs::read(root_path.join("settings.json")).unwrap())
                .unwrap();
        for project_path in settings
            .project_path
            .iter()
            .chain(settings.project_paths.iter())
            .chain(settings.archived_project_paths.iter())
        {
            let identity =
                crate::path_identity::verify_directory(PathBuf::from(project_path).as_path())
                    .unwrap();
            assert!(super::identity_is_at_or_below(&identity, root.identity()));
        }
    }

    #[test]
    fn temporary_cleanup_retries_are_bounded_and_report_the_final_error_kind() {
        let attempts = Cell::new(0);
        let result = remove_temporary_file_with_retries(
            PathBuf::from("temporary-state-file").as_path(),
            3,
            |_| {
                attempts.set(attempts.get() + 1);
                Err(Error::from(ErrorKind::PermissionDenied))
            },
        );

        assert_eq!(result, Err((3, ErrorKind::PermissionDenied)));
        assert_eq!(attempts.get(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn same_and_different_roots_bootstrap_concurrently_without_state_collisions() {
        let fixture = tempfile::TempDir::new().unwrap();
        let parent = fixture.path().to_path_buf();
        let lower_profile = test_profile();
        let upper_profile = test_profile();
        let lower_parent = parent.clone();
        let upper_parent = parent.clone();

        let lower = std::thread::spawn(move || {
            resolve_isolated_startup_state(
                &lower_parent.join("app-state"),
                lower_profile,
                "profile-hash",
                &[],
            )
            .map(|_| ())
        });
        let upper = std::thread::spawn(move || {
            resolve_isolated_startup_state(
                &upper_parent.join("APP-STATE"),
                upper_profile,
                "profile-hash",
                &[],
            )
            .map(|_| ())
        });

        assert!(lower.join().unwrap().is_ok());
        assert!(upper.join().unwrap().is_ok());
        assert!(parent.join("app-state").join("settings.json").is_file());

        let first_parent = parent.clone();
        let second_parent = parent.clone();
        let first = std::thread::spawn(move || {
            resolve_isolated_startup_state(
                &first_parent.join("first-root"),
                test_profile(),
                "profile-hash",
                &[],
            )
            .map(|state| state.mutex_hash().unwrap().to_string())
        });
        let second = std::thread::spawn(move || {
            resolve_isolated_startup_state(
                &second_parent.join("second-root"),
                test_profile(),
                "profile-hash",
                &[],
            )
            .map(|state| state.mutex_hash().unwrap().to_string())
        });

        let first_hash = first.join().unwrap().unwrap();
        let second_hash = second.join().unwrap().unwrap();
        assert_ne!(first_hash, second_hash);
    }

    #[cfg(feature = "isolated-validation-package")]
    #[test]
    fn compiled_profile_is_fixed_and_matches_the_checked_in_resource_bytes() {
        use sha2::{Digest, Sha256};

        let profile = super::compiled_package_profile().expect("feature carries the fixed profile");
        assert_eq!(profile.package_id, "agentscommander-1271-isolated-gates");
        assert_eq!(profile.product_label, "Agents Commander Isolated Gates");
        assert_eq!(
            profile.bundle_identifier,
            "dev.agentscommander.isolatedgates"
        );
        assert_eq!(profile.workspace, "AgentsCommander_1271_isolated");
        assert_eq!(profile.matrix, "WG-1271-ISOLATED-GATES");
        assert_eq!(profile.replica_agent, "gate-tester");
        assert_eq!(
            profile.header_identity,
            "WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated"
        );

        let expected_hash = Sha256::digest(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packaging/isolated-validation/package-profile.toml"
        )))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
        assert_eq!(
            super::compiled_profile_sha256(),
            Some(expected_hash.as_str())
        );
    }

    #[cfg(feature = "isolated-validation-package")]
    #[test]
    fn package_overlay_declares_the_exact_readonly_profile_resource() {
        let overlay: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tauri.conf.isolated-validation.json"
        )))
        .unwrap();
        assert_eq!(
            overlay["productName"],
            serde_json::Value::String("Agents Commander Isolated Gates".to_string())
        );
        assert_eq!(
            overlay["identifier"],
            serde_json::Value::String("dev.agentscommander.isolatedgates".to_string())
        );
        assert_eq!(
            overlay["bundle"]["resources"]["../packaging/isolated-validation/package-profile.toml"],
            serde_json::Value::String("package-profile.toml".to_string())
        );
    }

    #[cfg(not(feature = "isolated-validation-package"))]
    #[test]
    fn normal_build_does_not_carry_an_isolated_profile() {
        assert!(super::compiled_package_profile().is_none());
        assert!(super::compiled_profile_sha256().is_none());
    }
}
