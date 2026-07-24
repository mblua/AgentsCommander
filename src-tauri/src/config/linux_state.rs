use crate::errors::{StartupError, UnsafePathReason};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

const PRIVATE_DIR_MODE: libc::mode_t = 0o700;
const PRIVATE_FILE_MODE: libc::mode_t = 0o600;
const MAX_TEMP_ATTEMPTS: usize = 8;

const GUI_LOCK_BASENAME: &str = "gui-instance.lock";
const MUTATION_LOCK_BASENAME: &str = "coding-agent-mutation.lock";

static PREPARED_ROOT: OnceLock<Arc<SecureConfigRoot>> = OnceLock::new();
static PREPARED_GUI_STATE: OnceLock<Arc<SecureGuiState>> = OnceLock::new();
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestHookPoint {
    ConfiguredParentValidated,
    RootEntryOpened,
    LeafProbed,
    UsableLeafOpened,
    LockFileOpened,
    LockAcquired,
    TempCreated,
    DestinationValidated,
    PostRename,
    BetweenGuiAndMutationUnlock,
}

#[cfg(test)]
type TestHookCallback = Box<dyn FnMut(TestHookPoint, &Path)>;

#[cfg(test)]
std::thread_local! {
    static TEST_HOOK: std::cell::RefCell<Option<TestHookCallback>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_test_hook(point: TestHookPoint, path: &Path) {
    TEST_HOOK.with(|slot| {
        if let Some(callback) = slot.borrow_mut().as_mut() {
            callback(point, path);
        }
    });
}

#[cfg(not(test))]
#[inline]
fn run_test_hook(_point: TestHookPoint, _path: &Path) {}

#[cfg(test)]
struct TestHookGuard;

#[cfg(test)]
impl TestHookGuard {
    fn install(callback: impl FnMut(TestHookPoint, &Path) + 'static) -> Self {
        TEST_HOOK.with(|slot| {
            assert!(
                slot.borrow().is_none(),
                "only one deterministic Linux-state test hook may be installed per thread"
            );
            *slot.borrow_mut() = Some(Box::new(callback));
        });
        Self
    }
}

#[cfg(test)]
impl Drop for TestHookGuard {
    fn drop(&mut self) {
        TEST_HOOK.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    device: libc::dev_t,
    inode: libc::ino_t,
}

impl Identity {
    fn of(stat: &libc::stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        }
    }
}

pub struct SecureConfigRoot {
    display_path: PathBuf,
    configured_parent: PathBuf,
    traversal_anchor_path: PathBuf,
    traversal_anchor: File,
    parent_components: Vec<OsString>,
    parent: File,
    parent_identity: Identity,
    basename: OsString,
    basename_c: CString,
    root: File,
    root_identity: Identity,
    effective_uid: libc::uid_t,
}

impl std::fmt::Debug for SecureConfigRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureConfigRoot")
            .field("display_path", &self.display_path)
            .field("configured_parent", &self.configured_parent)
            .field("parent_identity", &self.parent_identity)
            .field("basename", &self.basename)
            .field("root_identity", &self.root_identity)
            .field("effective_uid", &self.effective_uid)
            .finish_non_exhaustive()
    }
}

pub struct SecureDirectory {
    root: Arc<SecureConfigRoot>,
    parent: Option<Arc<SecureDirectory>>,
    file: File,
    identity: Identity,
    basename: OsString,
    basename_c: CString,
    display_path: PathBuf,
}

impl std::fmt::Debug for SecureDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureDirectory")
            .field("display_path", &self.display_path)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

pub struct PrivateFile {
    parent: SecureParent,
    file: File,
    identity: Identity,
    basename_c: CString,
    display_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct AtomicPublicationFailure {
    pub(crate) error: StartupError,
    pub(crate) published: Option<Box<PrivateFile>>,
}

impl std::fmt::Debug for PrivateFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivateFile")
            .field("display_path", &self.display_path)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
enum SecureParent {
    Root(Arc<SecureConfigRoot>),
    Directory(Arc<SecureDirectory>),
}

pub struct SecureGuiState {
    root: Arc<SecureConfigRoot>,
    instances: Arc<SecureDirectory>,
}

impl std::fmt::Debug for SecureGuiState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecureGuiState")
            .field("root", &self.root)
            .field("instances", &self.instances)
            .finish()
    }
}

pub enum GuiLockOutcome {
    Acquired(Box<GuiInstanceGuard>),
    AlreadyRunning,
}

pub struct GuiInstanceGuard {
    gui: Option<PrivateFile>,
    mutation: Option<PrivateFile>,
}

pub struct MutationGuard {
    mutation: Option<PrivateFile>,
}

enum LockAttempt {
    Acquired(PrivateFile),
    Contended,
}

#[derive(Clone, Copy)]
enum ExistingPolicy {
    Required,
    Optional,
}

#[derive(Clone, Copy)]
enum FileAccess {
    Read,
    ReadWrite,
    Append,
}

pub fn prepare_secure_config_root() -> Result<Arc<SecureConfigRoot>, StartupError> {
    if let Some(root) = PREPARED_ROOT.get() {
        root.validate("reuse prepared Linux config root")?;
        return Ok(Arc::clone(root));
    }

    let display_path = super::config_dir().ok_or_else(|| StartupError::MissingConfigDir {
        executable: super::current_executable(),
    })?;
    let prepared = Arc::new(SecureConfigRoot::prepare(&display_path)?);
    PREPARED_ROOT
        .set(Arc::clone(&prepared))
        .map_err(|_| StartupError::Initialization {
            component: "Linux secure config root",
            message: "concurrent preparation published another root".to_string(),
        })?;
    Ok(prepared)
}

pub fn prepared_secure_config_root(
    operation: &'static str,
) -> Result<Arc<SecureConfigRoot>, StartupError> {
    let path = super::config_dir().unwrap_or_else(|| PathBuf::from("<unresolved-config-dir>"));
    PREPARED_ROOT
        .get()
        .cloned()
        .ok_or(StartupError::SecureStateNotPrepared { operation, path })
}

pub fn prepared_secure_gui_state(
    operation: &'static str,
) -> Result<Arc<SecureGuiState>, StartupError> {
    let path = super::config_dir()
        .map(|path| path.join("instances"))
        .unwrap_or_else(|| PathBuf::from("<unresolved-config-dir>/instances"));
    PREPARED_GUI_STATE
        .get()
        .cloned()
        .ok_or(StartupError::SecureStateNotPrepared { operation, path })
}

/// Re-run a production-state unit test in an exact child with an isolated,
/// prepared Linux config root. The parent returns `true` after the child
/// succeeds and should return from its test body. The exact child prepares the
/// root and returns `false`, allowing the real assertions to run.
#[cfg(test)]
pub(crate) fn rerun_exact_test_with_prepared_root(test_name: &str) -> bool {
    const SENTINEL: &str = "AC_TEST_1111_PREPARED_EXACT";

    if std::env::var(SENTINEL).ok().as_deref() == Some(test_name) {
        prepare_secure_config_root().expect("prepare isolated Linux config root");
        return false;
    }

    let temp = tempfile::tempdir().expect("create isolated Linux config parent");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("make isolated Linux config parent private");
    }
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(SENTINEL, test_name)
        .env(
            "AGENTSCOMMANDER_TEST_CONFIG_DIR",
            temp.path().join("config"),
        )
        .spawn()
        .expect("spawn exact prepared-state child");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll prepared-state child") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("prepared-state child timed out for {test_name}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "prepared-state child failed for {test_name}: {status}"
    );
    true
}

impl SecureConfigRoot {
    fn prepare(display_path: &Path) -> Result<Self, StartupError> {
        let basename = display_path.file_name().ok_or_else(|| {
            StartupError::unsafe_path(
                "validate Linux config-root basename",
                display_path,
                UnsafePathReason::InvalidBasename,
            )
        })?;
        let basename_c = validate_basename(
            basename,
            "validate Linux config-root basename",
            display_path,
        )?;
        let configured_parent = display_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let (traversal_anchor_path, parent_components) = traversal_plan(&configured_parent)?;
        let traversal_anchor_open_path = if configured_parent.is_absolute() {
            traversal_anchor_path.as_path()
        } else {
            Path::new(".")
        };
        let traversal_anchor = open_path_directory(
            traversal_anchor_open_path,
            "open Linux config traversal anchor",
        )?;
        let effective_uid = unsafe {
            // SAFETY: geteuid has no arguments and returns the caller's effective UID.
            libc::geteuid()
        };
        let parent = walk_directory_components(
            &traversal_anchor,
            &traversal_anchor_path,
            &parent_components,
            effective_uid,
            true,
            "prepare Linux config parent",
        )?;
        let parent_stat = stat_fd(
            &parent,
            "inspect prepared Linux config parent",
            &configured_parent,
        )?;
        validate_trusted_directory(
            &parent_stat,
            effective_uid,
            "validate prepared Linux config parent",
            &configured_parent,
        )?;
        let parent_identity = Identity::of(&parent_stat);
        run_test_hook(TestHookPoint::ConfiguredParentValidated, &configured_parent);
        let parent_witness = walk_directory_components(
            &traversal_anchor,
            &traversal_anchor_path,
            &parent_components,
            effective_uid,
            false,
            "revalidate Linux config parent before root mutation",
        )?;
        let parent_witness_stat = stat_fd(
            &parent_witness,
            "revalidate Linux config parent before root mutation",
            &configured_parent,
        )?;
        if Identity::of(&parent_witness_stat) != parent_identity {
            return Err(StartupError::unsafe_path(
                "revalidate Linux config parent before root mutation",
                &configured_parent,
                UnsafePathReason::IdentityChanged,
            ));
        }

        let root = open_or_create_directory_at(
            &parent,
            &basename_c,
            display_path,
            effective_uid,
            "prepare Linux config root",
        )?;
        let root_stat = stat_fd(&root, "inspect prepared Linux config root", display_path)?;
        validate_owned_directory(
            &root_stat,
            effective_uid,
            "validate prepared Linux config root",
            display_path,
        )?;
        run_test_hook(TestHookPoint::RootEntryOpened, display_path);
        require_entry_identity(
            &parent,
            &basename_c,
            Identity::of(&root_stat),
            libc::S_IFDIR,
            "revalidate Linux config-root entry before mode repair",
            display_path,
        )?;
        chmod_fd(
            &root,
            PRIVATE_DIR_MODE,
            "set Linux config-root mode",
            display_path,
        )?;
        let repaired = stat_fd(&root, "reinspect prepared Linux config root", display_path)?;
        require_mode(
            &repaired,
            PRIVATE_DIR_MODE,
            "verify Linux config-root mode",
            display_path,
        )?;
        let root_identity = Identity::of(&repaired);
        require_entry_identity(
            &parent,
            &basename_c,
            root_identity,
            libc::S_IFDIR,
            "bind Linux config root to directory entry",
            display_path,
        )?;

        let prepared = Self {
            display_path: display_path.to_path_buf(),
            configured_parent,
            traversal_anchor_path,
            traversal_anchor,
            parent_components,
            parent,
            parent_identity,
            basename: basename.to_os_string(),
            basename_c,
            root,
            root_identity,
            effective_uid,
        };
        prepared.validate("finalize Linux config-root preparation")?;
        Ok(prepared)
    }

    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    pub fn validate(&self, operation: &'static str) -> Result<(), StartupError> {
        let reopened_parent = walk_directory_components(
            &self.traversal_anchor,
            &self.traversal_anchor_path,
            &self.parent_components,
            self.effective_uid,
            false,
            operation,
        )?;
        let reopened_parent_stat = stat_fd(&reopened_parent, operation, &self.configured_parent)?;
        if Identity::of(&reopened_parent_stat) != self.parent_identity {
            return Err(StartupError::unsafe_path(
                operation,
                &self.configured_parent,
                UnsafePathReason::IdentityChanged,
            ));
        }

        let retained_parent_stat = stat_fd(&self.parent, operation, &self.configured_parent)?;
        if Identity::of(&retained_parent_stat) != self.parent_identity {
            return Err(StartupError::unsafe_path(
                operation,
                &self.configured_parent,
                UnsafePathReason::IdentityChanged,
            ));
        }

        let root_stat = stat_fd(&self.root, operation, &self.display_path)?;
        validate_owned_directory(
            &root_stat,
            self.effective_uid,
            operation,
            &self.display_path,
        )?;
        require_mode(&root_stat, PRIVATE_DIR_MODE, operation, &self.display_path)?;
        if Identity::of(&root_stat) != self.root_identity {
            return Err(StartupError::unsafe_path(
                operation,
                &self.display_path,
                UnsafePathReason::IdentityChanged,
            ));
        }
        require_entry_identity(
            &self.parent,
            &self.basename_c,
            self.root_identity,
            libc::S_IFDIR,
            operation,
            &self.display_path,
        )
    }

    fn parent(self: &Arc<Self>) -> SecureParent {
        SecureParent::Root(Arc::clone(self))
    }

    pub fn open_or_create_directory(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<Arc<SecureDirectory>, StartupError> {
        SecureDirectory::open_or_create(self.parent(), basename, operation)
    }

    pub fn validate_optional_private_file(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<bool, StartupError> {
        Ok(open_private_file(
            self.parent(),
            basename,
            FileAccess::Read,
            ExistingPolicy::Optional,
            false,
            operation,
        )?
        .is_some())
    }

    pub fn open_append_private_file(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<PrivateFile, StartupError> {
        open_private_file(
            self.parent(),
            basename,
            FileAccess::Append,
            ExistingPolicy::Required,
            true,
            operation,
        )?
        .ok_or_else(|| {
            StartupError::io(
                operation,
                self.display_path.join(basename),
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "private file disappeared after creation",
                ),
            )
        })
    }

    pub fn read_private_file(
        self: &Arc<Self>,
        basename: &OsStr,
        limit: usize,
        operation: &'static str,
    ) -> Result<Option<Vec<u8>>, StartupError> {
        let Some(mut private) = open_private_file(
            self.parent(),
            basename,
            FileAccess::Read,
            ExistingPolicy::Optional,
            false,
            operation,
        )?
        else {
            return Ok(None);
        };
        let length = private.metadata_len(operation)?;
        if length > limit as u64 {
            return Err(StartupError::io(
                operation,
                private.display_path.clone(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("private file exceeds {limit}-byte limit"),
                ),
            ));
        }
        let mut bytes = Vec::with_capacity(length as usize);
        std::io::Read::by_ref(&mut private.file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| StartupError::io(operation, &private.display_path, source))?;
        if bytes.len() > limit {
            return Err(StartupError::io(
                operation,
                private.display_path.clone(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("private file exceeds {limit}-byte limit"),
                ),
            ));
        }
        private.validate(operation)?;
        Ok(Some(bytes))
    }

    pub fn atomic_publish(
        self: &Arc<Self>,
        basename: &OsStr,
        payload: &[u8],
        operation: &'static str,
    ) -> Result<(), StartupError> {
        self.atomic_publish_tracked(basename, payload, operation)
            .map(|_| ())
            .map_err(|failure| failure.error)
    }

    pub(crate) fn atomic_publish_tracked(
        self: &Arc<Self>,
        basename: &OsStr,
        payload: &[u8],
        operation: &'static str,
    ) -> Result<PrivateFile, AtomicPublicationFailure> {
        atomic_publish_to_parent(self.parent(), basename, payload, operation)
    }

    pub fn create_if_absent(
        self: &Arc<Self>,
        basename: &OsStr,
        payload: &[u8],
        operation: &'static str,
    ) -> Result<bool, StartupError> {
        if self.validate_optional_private_file(basename, operation)? {
            return Ok(false);
        }
        match create_private_file(self.parent(), basename, FileAccess::ReadWrite, operation) {
            Ok(mut file) => {
                let result = file
                    .file
                    .write_all(payload)
                    .and_then(|_| file.file.flush())
                    .map_err(|source| StartupError::io(operation, &file.display_path, source))
                    .and_then(|_| file.validate(operation));
                if result.is_err() {
                    let _ = cleanup_owned_temp(&file, operation);
                }
                result.map(|_| true)
            }
            Err(StartupError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                self.validate_optional_private_file(basename, operation)?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub fn unlink_private_file_if_present(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<bool, StartupError> {
        let Some(existing) = open_private_file(
            self.parent(),
            basename,
            FileAccess::Read,
            ExistingPolicy::Optional,
            false,
            operation,
        )?
        else {
            return Ok(false);
        };
        existing.validate(operation)?;
        unlink_entry(
            self.root.as_raw_fd(),
            &existing.basename_c,
            0,
            operation,
            &existing.display_path,
        )?;
        self.validate(operation)?;
        Ok(true)
    }

    pub fn rename_private_file(
        self: &Arc<Self>,
        from: &OsStr,
        to: &OsStr,
        operation: &'static str,
    ) -> Result<bool, StartupError> {
        self.validate(operation)?;
        let Some(source) = open_private_file(
            self.parent(),
            from,
            FileAccess::Read,
            ExistingPolicy::Optional,
            false,
            operation,
        )?
        else {
            return Ok(false);
        };
        let destination = open_private_file(
            self.parent(),
            to,
            FileAccess::Read,
            ExistingPolicy::Optional,
            false,
            operation,
        )?;
        source.validate(operation)?;
        if let Some(destination) = destination.as_ref() {
            destination.validate(operation)?;
        }
        let to_path = self.display_path.join(to);
        let to_c = validate_basename(to, operation, &to_path)?;
        let result = unsafe {
            // SAFETY: both directory descriptors and NUL-terminated basenames
            // remain valid for the duration of renameat.
            libc::renameat(
                self.root.as_raw_fd(),
                source.basename_c.as_ptr(),
                self.root.as_raw_fd(),
                to_c.as_ptr(),
            )
        };
        if result != 0 {
            return Err(StartupError::io(
                operation,
                &source.display_path,
                std::io::Error::last_os_error(),
            ));
        }
        let rebound = open_private_file_without_mode_repair(
            self.parent(),
            to,
            FileAccess::Read,
            ExistingPolicy::Required,
            false,
            operation,
        )
        .map_err(|_| StartupError::PublicationAmbiguous {
            operation,
            path: to_path.clone(),
        })?
        .ok_or_else(|| StartupError::PublicationAmbiguous {
            operation,
            path: to_path.clone(),
        })?;
        if rebound.identity != source.identity {
            return Err(StartupError::PublicationAmbiguous {
                operation,
                path: to_path,
            });
        }
        Ok(true)
    }
}

impl SecureDirectory {
    fn open_or_create(
        parent: SecureParent,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<Arc<Self>, StartupError> {
        parent.validate(operation)?;
        let display_path = parent.display_path().join(basename);
        let basename_c = validate_basename(basename, operation, &display_path)?;
        let file = open_or_create_directory_at(
            parent.file(),
            &basename_c,
            &display_path,
            parent.uid(),
            operation,
        )?;
        let stat = stat_fd(&file, operation, &display_path)?;
        validate_owned_directory(&stat, parent.uid(), operation, &display_path)?;
        chmod_fd(&file, PRIVATE_DIR_MODE, operation, &display_path)?;
        let repaired = stat_fd(&file, operation, &display_path)?;
        require_mode(&repaired, PRIVATE_DIR_MODE, operation, &display_path)?;
        let identity = Identity::of(&repaired);
        require_entry_identity(
            parent.file(),
            &basename_c,
            identity,
            libc::S_IFDIR,
            operation,
            &display_path,
        )?;
        let directory = Arc::new(Self {
            root: parent.root(),
            parent: match parent {
                SecureParent::Root(_) => None,
                SecureParent::Directory(directory) => Some(directory),
            },
            file,
            identity,
            basename: basename.to_os_string(),
            basename_c,
            display_path,
        });
        directory.validate(operation)?;
        Ok(directory)
    }

    fn create_new(
        parent: SecureParent,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<Arc<Self>, StartupError> {
        parent.validate(operation)?;
        let display_path = parent.display_path().join(basename);
        let basename_c = validate_basename(basename, operation, &display_path)?;
        let result = unsafe {
            // SAFETY: parent is a retained directory and basename_c is a
            // validated, NUL-terminated single path component.
            libc::mkdirat(
                parent.file().as_raw_fd(),
                basename_c.as_ptr(),
                PRIVATE_DIR_MODE,
            )
        };
        if result != 0 {
            return Err(StartupError::io(
                operation,
                &display_path,
                std::io::Error::last_os_error(),
            ));
        }
        let created = stat_at_required(
            parent.file().as_raw_fd(),
            &basename_c,
            libc::AT_SYMLINK_NOFOLLOW,
            operation,
            &display_path,
        )?;
        let created_identity = Identity::of(&created);
        let cleanup_parent = parent.clone();
        let cleanup_basename = basename_c.clone();
        let cleanup_path = display_path.clone();
        let directory = (|| {
            let file = openat_file(
                parent.file().as_raw_fd(),
                &basename_c,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
                operation,
                &display_path,
            )?;
            let stat = stat_fd(&file, operation, &display_path)?;
            validate_owned_directory(&stat, parent.uid(), operation, &display_path)?;
            if Identity::of(&stat) != created_identity {
                return Err(StartupError::unsafe_path(
                    operation,
                    &display_path,
                    UnsafePathReason::IdentityChanged,
                ));
            }
            chmod_fd(&file, PRIVATE_DIR_MODE, operation, &display_path)?;
            let repaired = stat_fd(&file, operation, &display_path)?;
            require_mode(&repaired, PRIVATE_DIR_MODE, operation, &display_path)?;
            let identity = Identity::of(&repaired);
            require_entry_identity(
                parent.file(),
                &basename_c,
                identity,
                libc::S_IFDIR,
                operation,
                &display_path,
            )?;
            let directory = Arc::new(Self {
                root: parent.root(),
                parent: match parent {
                    SecureParent::Root(_) => None,
                    SecureParent::Directory(directory) => Some(directory),
                },
                file,
                identity,
                basename: basename.to_os_string(),
                basename_c,
                display_path,
            });
            directory.validate(operation)?;
            Ok(directory)
        })();
        if directory.is_err()
            && stat_at_optional(
                cleanup_parent.file().as_raw_fd(),
                &cleanup_basename,
                libc::AT_SYMLINK_NOFOLLOW,
                operation,
                &cleanup_path,
            )
            .ok()
            .flatten()
            .is_some_and(|stat| {
                file_type(&stat) == libc::S_IFDIR && Identity::of(&stat) == created_identity
            })
        {
            let _ = unlink_entry(
                cleanup_parent.file().as_raw_fd(),
                &cleanup_basename,
                libc::AT_REMOVEDIR,
                "rollback incomplete startup directory creation",
                &cleanup_path,
            );
        }
        directory
    }

    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    fn parent_file(&self) -> &File {
        match self.parent.as_ref() {
            Some(parent) => &parent.file,
            None => &self.root.root,
        }
    }

    pub fn validate(&self, operation: &'static str) -> Result<(), StartupError> {
        self.root.validate(operation)?;
        if let Some(parent) = self.parent.as_ref() {
            parent.validate(operation)?;
        }
        let stat = stat_fd(&self.file, operation, &self.display_path)?;
        validate_owned_directory(
            &stat,
            self.root.effective_uid,
            operation,
            &self.display_path,
        )?;
        require_mode(&stat, PRIVATE_DIR_MODE, operation, &self.display_path)?;
        if Identity::of(&stat) != self.identity {
            return Err(StartupError::unsafe_path(
                operation,
                &self.display_path,
                UnsafePathReason::IdentityChanged,
            ));
        }
        require_entry_identity(
            self.parent_file(),
            &self.basename_c,
            self.identity,
            libc::S_IFDIR,
            operation,
            &self.display_path,
        )
    }

    pub fn open_or_create_directory(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<Arc<SecureDirectory>, StartupError> {
        SecureDirectory::open_or_create(
            SecureParent::Directory(Arc::clone(self)),
            basename,
            operation,
        )
    }

    pub fn create_new_directory(
        self: &Arc<Self>,
        basename: &OsStr,
        operation: &'static str,
    ) -> Result<Arc<SecureDirectory>, StartupError> {
        SecureDirectory::create_new(
            SecureParent::Directory(Arc::clone(self)),
            basename,
            operation,
        )
    }

    pub fn atomic_publish(
        self: &Arc<Self>,
        basename: &OsStr,
        payload: &[u8],
        operation: &'static str,
    ) -> Result<(), StartupError> {
        atomic_publish_to_parent(
            SecureParent::Directory(Arc::clone(self)),
            basename,
            payload,
            operation,
        )
        .map(|_| ())
        .map_err(|failure| failure.error)
    }
}

impl SecureParent {
    fn root(&self) -> Arc<SecureConfigRoot> {
        match self {
            Self::Root(root) => Arc::clone(root),
            Self::Directory(directory) => Arc::clone(&directory.root),
        }
    }

    fn file(&self) -> &File {
        match self {
            Self::Root(root) => &root.root,
            Self::Directory(directory) => &directory.file,
        }
    }

    fn display_path(&self) -> &Path {
        match self {
            Self::Root(root) => &root.display_path,
            Self::Directory(directory) => &directory.display_path,
        }
    }

    fn uid(&self) -> libc::uid_t {
        match self {
            Self::Root(root) => root.effective_uid,
            Self::Directory(directory) => directory.root.effective_uid,
        }
    }

    fn validate(&self, operation: &'static str) -> Result<(), StartupError> {
        match self {
            Self::Root(root) => root.validate(operation),
            Self::Directory(directory) => directory.validate(operation),
        }
    }
}

impl PrivateFile {
    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    pub fn metadata_len(&self, operation: &'static str) -> Result<u64, StartupError> {
        let stat = stat_fd(&self.file, operation, &self.display_path)?;
        if stat.st_size < 0 {
            return Err(StartupError::io(
                operation,
                &self.display_path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "regular file reported a negative size",
                ),
            ));
        }
        Ok(stat.st_size as u64)
    }

    pub fn write_all(
        &mut self,
        payload: &[u8],
        operation: &'static str,
    ) -> Result<(), StartupError> {
        self.file
            .write_all(payload)
            .map_err(|source| StartupError::io(operation, &self.display_path, source))
    }

    pub fn flush(&mut self, operation: &'static str) -> Result<(), StartupError> {
        self.file
            .flush()
            .map_err(|source| StartupError::io(operation, &self.display_path, source))
    }

    pub fn validate(&self, operation: &'static str) -> Result<(), StartupError> {
        self.parent.validate(operation)?;
        let stat = stat_fd(&self.file, operation, &self.display_path)?;
        validate_private_regular(&stat, self.parent.uid(), operation, &self.display_path)?;
        require_mode(&stat, PRIVATE_FILE_MODE, operation, &self.display_path)?;
        if Identity::of(&stat) != self.identity {
            return Err(StartupError::unsafe_path(
                operation,
                &self.display_path,
                UnsafePathReason::IdentityChanged,
            ));
        }
        require_entry_identity(
            self.parent.file(),
            &self.basename_c,
            self.identity,
            libc::S_IFREG,
            operation,
            &self.display_path,
        )
    }

    pub(crate) fn unlink_if_still_owned(
        &self,
        operation: &'static str,
    ) -> Result<(), StartupError> {
        self.validate(operation)?;
        unlink_entry(
            self.parent.file().as_raw_fd(),
            &self.basename_c,
            0,
            operation,
            &self.display_path,
        )?;
        self.parent.validate(operation)
    }
}

impl GuiInstanceGuard {
    pub fn validate(&self) -> Result<(), StartupError> {
        let mutation = self
            .mutation
            .as_ref()
            .ok_or_else(|| StartupError::Initialization {
                component: "Linux GUI instance guard",
                message: "mutation lock was already released".to_string(),
            })?;
        let gui = self
            .gui
            .as_ref()
            .ok_or_else(|| StartupError::Initialization {
                component: "Linux GUI instance guard",
                message: "GUI lock was already released".to_string(),
            })?;
        mutation.validate("revalidate Linux mutation lock")?;
        gui.validate("revalidate Linux GUI lock")
    }

    pub fn release(&mut self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Some(gui) = self.gui.take() {
            if let Err(error) = gui.file.unlock() {
                diagnostics.push(format!(
                    "failed to unlock {}: {}",
                    gui.display_path.display(),
                    error
                ));
            }
        }
        if let Some(mutation) = self.mutation.as_ref() {
            run_test_hook(
                TestHookPoint::BetweenGuiAndMutationUnlock,
                &mutation.display_path,
            );
        }
        if let Some(mutation) = self.mutation.take() {
            if let Err(error) = mutation.file.unlock() {
                diagnostics.push(format!(
                    "failed to unlock {}: {}",
                    mutation.display_path.display(),
                    error
                ));
            }
        }
        diagnostics
    }
}

impl Drop for GuiInstanceGuard {
    fn drop(&mut self) {
        for diagnostic in self.release() {
            log::warn!("[linux-instance-lock] {diagnostic}");
        }
    }
}

impl MutationGuard {
    pub fn validate(&self) -> Result<(), StartupError> {
        self.mutation
            .as_ref()
            .ok_or_else(|| StartupError::Initialization {
                component: "Linux coding-agent mutation guard",
                message: "mutation lock was already released".to_string(),
            })?
            .validate("revalidate Linux coding-agent mutation lock")
    }

    pub fn release(&mut self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if let Some(mutation) = self.mutation.take() {
            if let Err(error) = mutation.file.unlock() {
                diagnostics.push(format!(
                    "failed to unlock {}: {}",
                    mutation.display_path.display(),
                    error
                ));
            }
        }
        diagnostics
    }
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        for diagnostic in self.release() {
            log::warn!("[linux-instance-lock] {diagnostic}");
        }
    }
}

pub fn acquire_gui_instance() -> Result<GuiLockOutcome, StartupError> {
    let root = prepared_secure_config_root("acquire Linux GUI instance locks")?;
    let mutation = match try_private_lock(
        Arc::clone(&root),
        MUTATION_LOCK_BASENAME,
        "acquire Linux coding-agent mutation lock for GUI",
    )? {
        LockAttempt::Acquired(lock) => lock,
        LockAttempt::Contended => return Ok(GuiLockOutcome::AlreadyRunning),
    };
    let gui = match try_private_lock(
        Arc::clone(&root),
        GUI_LOCK_BASENAME,
        "acquire Linux GUI instance lock",
    )? {
        LockAttempt::Acquired(lock) => lock,
        LockAttempt::Contended => {
            let mutation_path = mutation.display_path.clone();
            let gui_path = root.display_path.join(GUI_LOCK_BASENAME);
            drop(mutation);
            return Err(StartupError::LockStateInconsistent {
                mutation_path,
                gui_path,
            });
        }
    };
    mutation.validate("finalize Linux GUI mutation lock")?;
    gui.validate("finalize Linux GUI instance lock")?;
    Ok(GuiLockOutcome::Acquired(Box::new(GuiInstanceGuard {
        gui: Some(gui),
        mutation: Some(mutation),
    })))
}

pub enum LinuxMutationRoute {
    QueueToRunningGui,
    DirectWithGuard(MutationGuard),
}

pub fn coding_agent_mutation_route() -> Result<LinuxMutationRoute, StartupError> {
    let root = prepared_secure_config_root("probe Linux coding-agent mutation route")?;
    match try_private_lock(
        Arc::clone(&root),
        MUTATION_LOCK_BASENAME,
        "probe Linux coding-agent mutation lock",
    )? {
        LockAttempt::Acquired(mutation) => {
            match try_private_lock(
                Arc::clone(&root),
                GUI_LOCK_BASENAME,
                "probe Linux GUI lock after mutation acquisition",
            )? {
                LockAttempt::Acquired(gui_probe) => {
                    drop(gui_probe);
                    mutation.validate("finalize direct Linux mutation guard")?;
                    Ok(LinuxMutationRoute::DirectWithGuard(MutationGuard {
                        mutation: Some(mutation),
                    }))
                }
                LockAttempt::Contended => {
                    let mutation_path = mutation.display_path.clone();
                    let gui_path = root.display_path.join(GUI_LOCK_BASENAME);
                    drop(mutation);
                    Err(StartupError::LockStateInconsistent {
                        mutation_path,
                        gui_path,
                    })
                }
            }
        }
        LockAttempt::Contended => {
            match try_private_lock(
                Arc::clone(&root),
                GUI_LOCK_BASENAME,
                "probe Linux GUI lock while mutation lock is contended",
            )? {
                LockAttempt::Contended => Ok(LinuxMutationRoute::QueueToRunningGui),
                LockAttempt::Acquired(gui_probe) => {
                    drop(gui_probe);
                    Err(StartupError::MutationBusy {
                        path: root.display_path.join(MUTATION_LOCK_BASENAME),
                    })
                }
            }
        }
    }
}

pub fn prepare_secure_gui_state(
    guard: &GuiInstanceGuard,
) -> Result<Arc<SecureGuiState>, StartupError> {
    guard.validate()?;
    if let Some(state) = PREPARED_GUI_STATE.get() {
        state.validate("reuse prepared Linux GUI state")?;
        return Ok(Arc::clone(state));
    }
    let root = prepared_secure_config_root("prepare Linux GUI state")?;
    let instances =
        root.open_or_create_directory(OsStr::new("instances"), "prepare instances directory")?;

    for basename in [
        GUI_LOCK_BASENAME,
        MUTATION_LOCK_BASENAME,
        "app.log",
        "app.log.1",
        "app.log.2",
        "app.log.3",
        "app.log.4",
        "app.log.5",
        "settings.json",
        "settings.pre-384-v1.json",
        "web-token.txt",
        "master-token.txt",
        "app-outbox-path.txt",
        "daemon.pid",
    ] {
        root.validate_optional_private_file(
            OsStr::new(basename),
            "preflight Linux security-bearing file",
        )?;
    }

    for basename in [
        "web-token.txt",
        "master-token.txt",
        "app-outbox-path.txt",
        "daemon.pid",
    ] {
        root.unlink_private_file_if_present(
            OsStr::new(basename),
            "invalidate stale Linux startup publication",
        )?;
    }

    let state = Arc::new(SecureGuiState { root, instances });
    state.validate("finalize Linux GUI-state preflight")?;
    PREPARED_GUI_STATE
        .set(Arc::clone(&state))
        .map_err(|_| StartupError::Initialization {
            component: "Linux secure GUI state",
            message: "concurrent preparation published another GUI state".to_string(),
        })?;
    Ok(state)
}

impl SecureGuiState {
    pub fn validate(&self, operation: &'static str) -> Result<(), StartupError> {
        self.root.validate(operation)?;
        self.instances.validate(operation)
    }

    pub fn create_instance_directory(
        &self,
        instance_id: &uuid::Uuid,
    ) -> Result<Arc<SecureDirectory>, StartupError> {
        self.validate("create Linux startup instance")?;
        let instance_text = instance_id.hyphenated().to_string();
        self.instances.create_new_directory(
            OsStr::new(&instance_text),
            "create Linux startup instance directory",
        )
    }

    pub fn create_instance_outbox(
        &self,
        instance: &Arc<SecureDirectory>,
    ) -> Result<Arc<SecureDirectory>, StartupError> {
        self.validate("create Linux startup outbox")?;
        if instance.parent.as_ref().map(Arc::as_ptr) != Some(Arc::as_ptr(&self.instances)) {
            return Err(StartupError::unsafe_path(
                "create Linux startup outbox",
                &instance.display_path,
                UnsafePathReason::IdentityChanged,
            ));
        }
        instance.open_or_create_directory(
            OsStr::new("outbox"),
            "create Linux startup outbox directory",
        )
    }

    pub fn cleanup_stale_instances(
        &self,
        current: Option<&uuid::Uuid>,
    ) -> Result<(), StartupError> {
        self.validate("start Linux stale-instance cleanup")?;
        let names = read_directory_names(
            &self.instances.file,
            "enumerate Linux stale instances",
            &self.instances.display_path,
        )?;
        for name in names {
            let Some(text) = name.to_str() else {
                continue;
            };
            let Ok(uuid) = uuid::Uuid::parse_str(text) else {
                continue;
            };
            if uuid.hyphenated().to_string() != text || current == Some(&uuid) {
                continue;
            }
            if let Err(error) = remove_owned_directory_tree(
                &self.instances,
                &name,
                self.instances.identity.device,
                self.root.effective_uid,
                "remove stale Linux instance",
            ) {
                log::warn!(
                    "[app-outbox] could not safely remove stale instance {}: {}",
                    self.instances.display_path.join(&name).display(),
                    error
                );
            }
        }
        self.validate("finish Linux stale-instance cleanup")
    }

    pub fn remove_owned_instance(
        &self,
        instance: &Arc<SecureDirectory>,
    ) -> Result<(), StartupError> {
        self.validate("rollback Linux startup instance")?;
        if instance.parent.as_ref().map(Arc::as_ptr) != Some(Arc::as_ptr(&self.instances)) {
            return Err(StartupError::unsafe_path(
                "rollback Linux startup instance",
                &instance.display_path,
                UnsafePathReason::IdentityChanged,
            ));
        }
        remove_owned_directory_tree(
            &self.instances,
            &instance.basename,
            self.instances.identity.device,
            self.root.effective_uid,
            "rollback Linux startup instance",
        )
    }
}

fn try_private_lock(
    root: Arc<SecureConfigRoot>,
    basename: &str,
    operation: &'static str,
) -> Result<LockAttempt, StartupError> {
    let file = open_private_file(
        SecureParent::Root(root),
        OsStr::new(basename),
        FileAccess::ReadWrite,
        ExistingPolicy::Required,
        true,
        operation,
    )?
    .ok_or_else(|| {
        StartupError::io(
            operation,
            PathBuf::from(basename),
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "lock file disappeared after creation",
            ),
        )
    })?;
    run_test_hook(TestHookPoint::LockFileOpened, &file.display_path);
    match file.file.try_lock() {
        Ok(()) => {
            run_test_hook(TestHookPoint::LockAcquired, &file.display_path);
            file.validate(operation)?;
            Ok(LockAttempt::Acquired(file))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(LockAttempt::Contended),
        Err(std::fs::TryLockError::Error(source)) => {
            Err(StartupError::io(operation, &file.display_path, source))
        }
    }
}

fn open_private_file(
    parent: SecureParent,
    basename: &OsStr,
    access: FileAccess,
    policy: ExistingPolicy,
    create: bool,
    operation: &'static str,
) -> Result<Option<PrivateFile>, StartupError> {
    open_private_file_with_mode_policy(parent, basename, access, policy, create, true, operation)
}

fn open_private_file_without_mode_repair(
    parent: SecureParent,
    basename: &OsStr,
    access: FileAccess,
    policy: ExistingPolicy,
    create: bool,
    operation: &'static str,
) -> Result<Option<PrivateFile>, StartupError> {
    open_private_file_with_mode_policy(parent, basename, access, policy, create, false, operation)
}

fn open_private_file_with_mode_policy(
    parent: SecureParent,
    basename: &OsStr,
    access: FileAccess,
    policy: ExistingPolicy,
    create: bool,
    repair_mode: bool,
    operation: &'static str,
) -> Result<Option<PrivateFile>, StartupError> {
    parent.validate(operation)?;
    let display_path = parent.display_path().join(basename);
    let basename_c = validate_basename(basename, operation, &display_path)?;

    match probe_leaf(parent.file(), &basename_c, &display_path, operation)? {
        None if create => match create_private_file(parent.clone(), basename, access, operation) {
            Ok(file) => return Ok(Some(file)),
            Err(StartupError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        },
        None if matches!(policy, ExistingPolicy::Optional) => return Ok(None),
        None => {
            return Err(StartupError::io(
                operation,
                display_path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "private file is missing"),
            ))
        }
        Some(_) => {}
    }

    let probe =
        probe_leaf(parent.file(), &basename_c, &display_path, operation)?.ok_or_else(|| {
            StartupError::io(
                operation,
                &display_path,
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "private file changed during secure open",
                ),
            )
        })?;
    validate_private_regular(&probe, parent.uid(), operation, &display_path)?;
    let probe_identity = Identity::of(&probe);
    run_test_hook(TestHookPoint::LeafProbed, &display_path);

    let mut flags = libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    flags |= match access {
        FileAccess::Read => libc::O_RDONLY,
        FileAccess::ReadWrite => libc::O_RDWR,
        FileAccess::Append => libc::O_WRONLY | libc::O_APPEND,
    };
    let file = openat_file(
        parent.file().as_raw_fd(),
        &basename_c,
        flags,
        0,
        operation,
        &display_path,
    )?;
    run_test_hook(TestHookPoint::UsableLeafOpened, &display_path);
    let stat = stat_fd(&file, operation, &display_path)?;
    validate_private_regular(&stat, parent.uid(), operation, &display_path)?;
    if Identity::of(&stat) != probe_identity {
        return Err(StartupError::unsafe_path(
            operation,
            &display_path,
            UnsafePathReason::IdentityChanged,
        ));
    }
    require_entry_identity(
        parent.file(),
        &basename_c,
        probe_identity,
        libc::S_IFREG,
        operation,
        &display_path,
    )?;
    if repair_mode {
        chmod_fd(&file, PRIVATE_FILE_MODE, operation, &display_path)?;
    } else {
        require_mode(&stat, PRIVATE_FILE_MODE, operation, &display_path)?;
    }
    clear_nonblocking(&file, operation, &display_path)?;
    let repaired = stat_fd(&file, operation, &display_path)?;
    validate_private_regular(&repaired, parent.uid(), operation, &display_path)?;
    require_mode(&repaired, PRIVATE_FILE_MODE, operation, &display_path)?;
    let identity = Identity::of(&repaired);
    require_entry_identity(
        parent.file(),
        &basename_c,
        identity,
        libc::S_IFREG,
        operation,
        &display_path,
    )?;
    Ok(Some(PrivateFile {
        parent,
        file,
        identity,
        basename_c,
        display_path,
    }))
}

fn create_private_file(
    parent: SecureParent,
    basename: &OsStr,
    access: FileAccess,
    operation: &'static str,
) -> Result<PrivateFile, StartupError> {
    parent.validate(operation)?;
    let display_path = parent.display_path().join(basename);
    let basename_c = validate_basename(basename, operation, &display_path)?;
    let mut flags =
        libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    flags |= match access {
        FileAccess::Read => libc::O_RDONLY,
        FileAccess::ReadWrite => libc::O_RDWR,
        FileAccess::Append => libc::O_WRONLY | libc::O_APPEND,
    };
    let file = openat_file(
        parent.file().as_raw_fd(),
        &basename_c,
        flags,
        PRIVATE_FILE_MODE,
        operation,
        &display_path,
    )?;
    let stat = stat_fd(&file, operation, &display_path)?;
    validate_private_regular(&stat, parent.uid(), operation, &display_path)?;
    chmod_fd(&file, PRIVATE_FILE_MODE, operation, &display_path)?;
    clear_nonblocking(&file, operation, &display_path)?;
    let repaired = stat_fd(&file, operation, &display_path)?;
    validate_private_regular(&repaired, parent.uid(), operation, &display_path)?;
    require_mode(&repaired, PRIVATE_FILE_MODE, operation, &display_path)?;
    let identity = Identity::of(&repaired);
    require_entry_identity(
        parent.file(),
        &basename_c,
        identity,
        libc::S_IFREG,
        operation,
        &display_path,
    )?;
    Ok(PrivateFile {
        parent,
        file,
        identity,
        basename_c,
        display_path,
    })
}

fn atomic_publish_to_parent(
    parent: SecureParent,
    basename: &OsStr,
    payload: &[u8],
    operation: &'static str,
) -> Result<PrivateFile, AtomicPublicationFailure> {
    let mut uuid_source = uuid::Uuid::new_v4;
    let mut counter_source = || TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    atomic_publish_to_parent_with_sources(
        parent,
        basename,
        payload,
        operation,
        &mut uuid_source,
        &mut counter_source,
    )
}

fn atomic_publish_to_parent_with_sources(
    parent: SecureParent,
    basename: &OsStr,
    payload: &[u8],
    operation: &'static str,
    uuid_source: &mut impl FnMut() -> uuid::Uuid,
    counter_source: &mut impl FnMut() -> u64,
) -> Result<PrivateFile, AtomicPublicationFailure> {
    let failure = |error| AtomicPublicationFailure {
        error,
        published: None,
    };
    parent.validate(operation).map_err(failure)?;
    let destination_path = parent.display_path().join(basename);
    let destination_c =
        validate_basename(basename, operation, &destination_path).map_err(failure)?;
    let destination = open_private_file(
        parent.clone(),
        basename,
        FileAccess::Read,
        ExistingPolicy::Optional,
        false,
        operation,
    )
    .map_err(failure)?;
    let destination_identity = destination.as_ref().map(|file| file.identity);

    let mut last_collision = None;
    for _ in 0..MAX_TEMP_ATTEMPTS {
        let op_id = counter_source();
        let temp_name = format!(
            "{}.{}.{}.tmp",
            basename.to_string_lossy(),
            uuid_source().hyphenated(),
            op_id
        );
        let mut temp = match create_private_file(
            parent.clone(),
            OsStr::new(&temp_name),
            FileAccess::ReadWrite,
            operation,
        ) {
            Ok(file) => file,
            Err(StartupError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                last_collision = Some(source);
                continue;
            }
            Err(error) => return Err(failure(error)),
        };
        run_test_hook(TestHookPoint::TempCreated, &temp.display_path);

        let pre_rename = (|| {
            temp.validate(operation)?;
            temp.file
                .write_all(payload)
                .and_then(|_| temp.file.flush())
                .map_err(|source| StartupError::io(operation, &temp.display_path, source))?;
            temp.validate(operation)?;
            parent.validate(operation)?;

            let current_destination =
                probe_leaf(parent.file(), &destination_c, &destination_path, operation)?;
            match (destination_identity, current_destination.as_ref()) {
                (None, None) => {}
                (Some(expected), Some(stat)) if Identity::of(stat) == expected => {
                    validate_private_regular(stat, parent.uid(), operation, &destination_path)?;
                }
                _ => {
                    return Err(StartupError::unsafe_path(
                        operation,
                        &destination_path,
                        UnsafePathReason::IdentityChanged,
                    ))
                }
            }
            run_test_hook(TestHookPoint::DestinationValidated, &destination_path);
            let destination_recheck =
                probe_leaf(parent.file(), &destination_c, &destination_path, operation)?;
            match (destination_identity, destination_recheck.as_ref()) {
                (None, None) => {}
                (Some(expected), Some(stat)) if Identity::of(stat) == expected => {
                    validate_private_regular(stat, parent.uid(), operation, &destination_path)?;
                }
                _ => {
                    return Err(StartupError::unsafe_path(
                        operation,
                        &destination_path,
                        UnsafePathReason::IdentityChanged,
                    ))
                }
            }
            Ok(())
        })();

        if let Err(error) = pre_rename {
            let _ = cleanup_owned_temp(&temp, operation);
            return Err(failure(error));
        }

        let rename_result = unsafe {
            // SAFETY: parent is an open directory and both basenames are
            // validated, NUL-terminated single components.
            libc::renameat(
                parent.file().as_raw_fd(),
                temp.basename_c.as_ptr(),
                parent.file().as_raw_fd(),
                destination_c.as_ptr(),
            )
        };
        if rename_result != 0 {
            let error = StartupError::io(
                operation,
                &destination_path,
                std::io::Error::last_os_error(),
            );
            let _ = cleanup_owned_temp(&temp, operation);
            return Err(failure(error));
        }

        // The open descriptor still names the published inode after rename.
        // Rebind the retained receipt to the destination before any fallible
        // post-rename verification so rollback can remove only this attempt's
        // object if final identity proof fails.
        temp.basename_c = destination_c.clone();
        temp.display_path = destination_path.clone();
        run_test_hook(TestHookPoint::PostRename, &destination_path);
        let post_rename = (|| {
            temp.validate(operation)
                .map_err(|_| StartupError::PublicationAmbiguous {
                    operation,
                    path: destination_path.clone(),
                })?;
            let rebound = open_private_file_without_mode_repair(
                parent.clone(),
                basename,
                FileAccess::Read,
                ExistingPolicy::Required,
                false,
                operation,
            )
            .map_err(|_| StartupError::PublicationAmbiguous {
                operation,
                path: destination_path.clone(),
            })?
            .ok_or_else(|| StartupError::PublicationAmbiguous {
                operation,
                path: destination_path.clone(),
            })?;
            if rebound.identity != temp.identity {
                return Err(StartupError::PublicationAmbiguous {
                    operation,
                    path: destination_path.clone(),
                });
            }
            Ok(())
        })();

        if let Err(error) = post_rename {
            return Err(AtomicPublicationFailure {
                error,
                published: Some(Box::new(temp)),
            });
        }
        return Ok(temp);
    }

    Err(failure(StartupError::io(
        operation,
        destination_path,
        last_collision.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "private temp-name collision budget exhausted",
            )
        }),
    )))
}

fn cleanup_owned_temp(temp: &PrivateFile, operation: &'static str) -> Result<(), StartupError> {
    let current = probe_leaf(
        temp.parent.file(),
        &temp.basename_c,
        &temp.display_path,
        operation,
    )?;
    if current
        .as_ref()
        .map(Identity::of)
        .is_some_and(|identity| identity == temp.identity)
    {
        unlink_entry(
            temp.parent.file().as_raw_fd(),
            &temp.basename_c,
            0,
            operation,
            &temp.display_path,
        )?;
    }
    Ok(())
}

fn traversal_plan(parent: &Path) -> Result<(PathBuf, Vec<OsString>), StartupError> {
    let anchor = if parent.is_absolute() {
        PathBuf::from("/")
    } else {
        std::env::current_dir().map_err(|source| {
            StartupError::io("capture Linux config traversal anchor", parent, source)
        })?
    };
    let mut components = Vec::new();
    for component in parent.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => components.push(name.to_os_string()),
            Component::ParentDir => components.push(OsString::from("..")),
            Component::Prefix(_) => {
                return Err(StartupError::unsafe_path(
                    "plan Linux config-parent traversal",
                    parent,
                    UnsafePathReason::InvalidBasename,
                ))
            }
        }
    }
    Ok((anchor, components))
}

fn walk_directory_components(
    anchor: &File,
    anchor_path: &Path,
    components: &[OsString],
    uid: libc::uid_t,
    create_missing: bool,
    operation: &'static str,
) -> Result<File, StartupError> {
    let mut current = anchor
        .try_clone()
        .map_err(|source| StartupError::io(operation, anchor_path, source))?;
    let mut display = anchor_path.to_path_buf();
    validate_trusted_directory(
        &stat_fd(&current, operation, &display)?,
        uid,
        operation,
        &display,
    )?;

    for component in components {
        validate_trusted_directory(
            &stat_fd(&current, operation, &display)?,
            uid,
            operation,
            &display,
        )?;
        display.push(component);
        let component_c = cstring_component(component, operation, &display)?;
        let entry = stat_at_optional(
            current.as_raw_fd(),
            &component_c,
            libc::AT_SYMLINK_NOFOLLOW,
            operation,
            &display,
        )?;
        let next = match entry {
            None if create_missing => {
                let result = unsafe {
                    // SAFETY: current is an open directory and component_c is a
                    // validated NUL-terminated component.
                    libc::mkdirat(current.as_raw_fd(), component_c.as_ptr(), PRIVATE_DIR_MODE)
                };
                if result != 0 {
                    let source = std::io::Error::last_os_error();
                    if source.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(StartupError::io(operation, &display, source));
                    }
                }
                let opened = openat_file(
                    current.as_raw_fd(),
                    &component_c,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                    operation,
                    &display,
                )?;
                let opened_stat = stat_fd(&opened, operation, &display)?;
                validate_owned_directory(&opened_stat, uid, operation, &display)?;
                chmod_fd(&opened, PRIVATE_DIR_MODE, operation, &display)?;
                let repaired = stat_fd(&opened, operation, &display)?;
                require_mode(&repaired, PRIVATE_DIR_MODE, operation, &display)?;
                require_entry_identity(
                    &current,
                    &component_c,
                    Identity::of(&repaired),
                    libc::S_IFDIR,
                    operation,
                    &display,
                )?;
                opened
            }
            None => {
                return Err(StartupError::io(
                    operation,
                    &display,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "configured parent component is missing",
                    ),
                ))
            }
            Some(stat) if file_type(&stat) == libc::S_IFDIR => {
                let entry_identity = Identity::of(&stat);
                let opened = openat_file(
                    current.as_raw_fd(),
                    &component_c,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                    operation,
                    &display,
                )?;
                let opened_stat = stat_fd(&opened, operation, &display)?;
                if Identity::of(&opened_stat) != entry_identity {
                    return Err(StartupError::unsafe_path(
                        operation,
                        &display,
                        UnsafePathReason::IdentityChanged,
                    ));
                }
                opened
            }
            Some(stat) if file_type(&stat) == libc::S_IFLNK => {
                let link_identity = Identity::of(&stat);
                let opened = openat_file(
                    current.as_raw_fd(),
                    &component_c,
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                    0,
                    operation,
                    &display,
                )?;
                let link_recheck = stat_at_required(
                    current.as_raw_fd(),
                    &component_c,
                    libc::AT_SYMLINK_NOFOLLOW,
                    operation,
                    &display,
                )?;
                if file_type(&link_recheck) != libc::S_IFLNK
                    || Identity::of(&link_recheck) != link_identity
                {
                    return Err(StartupError::unsafe_path(
                        operation,
                        &display,
                        UnsafePathReason::IdentityChanged,
                    ));
                }
                let followed =
                    stat_at_required(current.as_raw_fd(), &component_c, 0, operation, &display)?;
                let opened_stat = stat_fd(&opened, operation, &display)?;
                if Identity::of(&followed) != Identity::of(&opened_stat) {
                    return Err(StartupError::unsafe_path(
                        operation,
                        &display,
                        UnsafePathReason::IdentityChanged,
                    ));
                }
                opened
            }
            Some(stat) => {
                return Err(StartupError::unsafe_path(
                    operation,
                    &display,
                    UnsafePathReason::WrongObjectType {
                        expected: "directory",
                        observed: object_type(&stat),
                    },
                ))
            }
        };
        let next_stat = stat_fd(&next, operation, &display)?;
        validate_trusted_directory(&next_stat, uid, operation, &display)?;
        current = next;
    }
    Ok(current)
}

fn open_or_create_directory_at(
    parent: &File,
    basename: &CStr,
    display_path: &Path,
    uid: libc::uid_t,
    operation: &'static str,
) -> Result<File, StartupError> {
    let entry = stat_at_optional(
        parent.as_raw_fd(),
        basename,
        libc::AT_SYMLINK_NOFOLLOW,
        operation,
        display_path,
    )?;
    if let Some(stat) = entry {
        if file_type(&stat) == libc::S_IFLNK {
            return Err(StartupError::unsafe_path(
                operation,
                display_path,
                UnsafePathReason::Symlink,
            ));
        }
        if file_type(&stat) != libc::S_IFDIR {
            return Err(StartupError::unsafe_path(
                operation,
                display_path,
                UnsafePathReason::WrongObjectType {
                    expected: "directory",
                    observed: object_type(&stat),
                },
            ));
        }
    } else {
        let result = unsafe {
            // SAFETY: parent is an open directory and basename is a valid
            // NUL-terminated component.
            libc::mkdirat(parent.as_raw_fd(), basename.as_ptr(), PRIVATE_DIR_MODE)
        };
        if result != 0 {
            let source = std::io::Error::last_os_error();
            if source.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(StartupError::io(operation, display_path, source));
            }
        }
    }
    let file = openat_file(
        parent.as_raw_fd(),
        basename,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
        operation,
        display_path,
    )?;
    let stat = stat_fd(&file, operation, display_path)?;
    validate_owned_directory(&stat, uid, operation, display_path)?;
    require_entry_identity(
        parent,
        basename,
        Identity::of(&stat),
        libc::S_IFDIR,
        operation,
        display_path,
    )?;
    Ok(file)
}

fn open_path_directory(path: &Path, operation: &'static str) -> Result<File, StartupError> {
    let bytes = path.as_os_str().as_bytes();
    let path_c = CString::new(bytes).map_err(|_| {
        StartupError::unsafe_path(operation, path, UnsafePathReason::InvalidBasename)
    })?;
    let fd = unsafe {
        // SAFETY: path_c is NUL-terminated and remains alive for the call.
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(StartupError::io(
            operation,
            path,
            std::io::Error::last_os_error(),
        ));
    }
    let file = unsafe {
        // SAFETY: fd is a newly owned descriptor returned by open.
        File::from_raw_fd(fd)
    };
    Ok(file)
}

fn openat_file(
    parent_fd: RawFd,
    basename: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<File, StartupError> {
    let fd = unsafe {
        // SAFETY: parent_fd is an open directory descriptor and basename is a
        // NUL-terminated component. The returned descriptor is handled below.
        libc::openat(parent_fd, basename.as_ptr(), flags, mode)
    };
    if fd < 0 {
        return Err(StartupError::io(
            operation,
            display_path,
            std::io::Error::last_os_error(),
        ));
    }
    let file = unsafe {
        // SAFETY: fd is a newly owned descriptor returned by openat.
        File::from_raw_fd(fd)
    };
    Ok(file)
}

fn stat_fd(
    file: &File,
    operation: &'static str,
    display_path: &Path,
) -> Result<libc::stat, StartupError> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        // SAFETY: stat points to valid uninitialized storage and fstat writes
        // the full structure on success.
        libc::fstat(file.as_raw_fd(), stat.as_mut_ptr())
    };
    if result != 0 {
        return Err(StartupError::io(
            operation,
            display_path,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(unsafe {
        // SAFETY: fstat succeeded and initialized stat.
        stat.assume_init()
    })
}

fn stat_at_optional(
    parent_fd: RawFd,
    basename: &CStr,
    flags: libc::c_int,
    operation: &'static str,
    display_path: &Path,
) -> Result<Option<libc::stat>, StartupError> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        // SAFETY: parent_fd and basename identify a directory entry, and stat
        // points to writable storage for fstatat.
        libc::fstatat(parent_fd, basename.as_ptr(), stat.as_mut_ptr(), flags)
    };
    if result == 0 {
        return Ok(Some(unsafe {
            // SAFETY: fstatat succeeded and initialized stat.
            stat.assume_init()
        }));
    }
    let source = std::io::Error::last_os_error();
    if source.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(StartupError::io(operation, display_path, source))
    }
}

fn stat_at_required(
    parent_fd: RawFd,
    basename: &CStr,
    flags: libc::c_int,
    operation: &'static str,
    display_path: &Path,
) -> Result<libc::stat, StartupError> {
    stat_at_optional(parent_fd, basename, flags, operation, display_path)?.ok_or_else(|| {
        StartupError::io(
            operation,
            display_path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "directory entry disappeared"),
        )
    })
}

fn probe_leaf(
    parent: &File,
    basename: &CStr,
    display_path: &Path,
    operation: &'static str,
) -> Result<Option<libc::stat>, StartupError> {
    let fd = unsafe {
        // SAFETY: parent is an open directory and basename is NUL-terminated.
        // O_PATH ensures a special file cannot block or activate.
        libc::openat(
            parent.as_raw_fd(),
            basename.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let source = std::io::Error::last_os_error();
        return if source.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(StartupError::io(operation, display_path, source))
        };
    }
    let probe = unsafe {
        // SAFETY: fd is newly owned and must be closed by File.
        File::from_raw_fd(fd)
    };
    Ok(Some(stat_fd(&probe, operation, display_path)?))
}

fn chmod_fd(
    file: &File,
    mode: libc::mode_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    let result = unsafe {
        // SAFETY: file is a live owned descriptor and mode is a valid mode mask.
        libc::fchmod(file.as_raw_fd(), mode)
    };
    if result != 0 {
        return Err(StartupError::io(
            operation,
            display_path,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn clear_nonblocking(
    file: &File,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    let current = unsafe {
        // SAFETY: F_GETFL reads flags from a valid descriptor.
        libc::fcntl(file.as_raw_fd(), libc::F_GETFL)
    };
    if current < 0 {
        return Err(StartupError::io(
            operation,
            display_path,
            std::io::Error::last_os_error(),
        ));
    }
    if current & libc::O_NONBLOCK != 0 {
        let result = unsafe {
            // SAFETY: F_SETFL updates status flags on a valid descriptor.
            libc::fcntl(file.as_raw_fd(), libc::F_SETFL, current & !libc::O_NONBLOCK)
        };
        if result < 0 {
            return Err(StartupError::io(
                operation,
                display_path,
                std::io::Error::last_os_error(),
            ));
        }
    }
    Ok(())
}

fn require_entry_identity(
    parent: &File,
    basename: &CStr,
    expected: Identity,
    expected_type: libc::mode_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    let entry = stat_at_required(
        parent.as_raw_fd(),
        basename,
        libc::AT_SYMLINK_NOFOLLOW,
        operation,
        display_path,
    )?;
    if file_type(&entry) == libc::S_IFLNK {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::Symlink,
        ));
    }
    if file_type(&entry) != expected_type {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::WrongObjectType {
                expected: if expected_type == libc::S_IFDIR {
                    "directory"
                } else {
                    "regular file"
                },
                observed: object_type(&entry),
            },
        ));
    }
    if Identity::of(&entry) != expected {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::IdentityChanged,
        ));
    }
    Ok(())
}

fn validate_trusted_directory(
    stat: &libc::stat,
    uid: libc::uid_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    if file_type(stat) != libc::S_IFDIR {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::WrongObjectType {
                expected: "directory",
                observed: object_type(stat),
            },
        ));
    }
    let mode = permission_mode(stat);
    let owner_allowed = stat.st_uid == uid || stat.st_uid == 0;
    let write_allowed = mode & 0o022 == 0 || mode & libc::S_ISVTX != 0;
    if !owner_allowed || !write_allowed {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::UntrustedParent {
                observed_uid: stat.st_uid,
                observed_mode: mode,
            },
        ));
    }
    Ok(())
}

fn validate_owned_directory(
    stat: &libc::stat,
    uid: libc::uid_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    if file_type(stat) != libc::S_IFDIR {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::WrongObjectType {
                expected: "directory",
                observed: object_type(stat),
            },
        ));
    }
    validate_owner(stat, uid, operation, display_path)
}

fn validate_private_regular(
    stat: &libc::stat,
    uid: libc::uid_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    if file_type(stat) == libc::S_IFLNK {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::Symlink,
        ));
    }
    if file_type(stat) != libc::S_IFREG {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::WrongObjectType {
                expected: "regular file",
                observed: object_type(stat),
            },
        ));
    }
    validate_owner(stat, uid, operation, display_path)?;
    if stat.st_nlink != 1 {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::HardLinked {
                observed: stat.st_nlink,
            },
        ));
    }
    Ok(())
}

fn validate_owner(
    stat: &libc::stat,
    uid: libc::uid_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    if stat.st_uid != uid {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::ForeignOwner {
                expected: uid,
                observed: stat.st_uid,
            },
        ));
    }
    Ok(())
}

fn require_mode(
    stat: &libc::stat,
    expected: libc::mode_t,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    let observed = permission_mode(stat);
    if observed != expected {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::WrongObjectType {
                expected: if expected == PRIVATE_DIR_MODE {
                    "owner-only directory mode 0700"
                } else {
                    "owner-only regular-file mode 0600"
                },
                observed: "different permission mode",
            },
        ));
    }
    Ok(())
}

fn file_type(stat: &libc::stat) -> libc::mode_t {
    stat.st_mode & libc::S_IFMT
}

fn permission_mode(stat: &libc::stat) -> u32 {
    stat.st_mode & 0o7777
}

fn object_type(stat: &libc::stat) -> &'static str {
    match file_type(stat) {
        libc::S_IFREG => "regular file",
        libc::S_IFDIR => "directory",
        libc::S_IFLNK => "symbolic link",
        libc::S_IFIFO => "FIFO",
        libc::S_IFSOCK => "socket",
        libc::S_IFCHR => "character device",
        libc::S_IFBLK => "block device",
        _ => "unknown object type",
    }
}

fn validate_basename(
    basename: &OsStr,
    operation: &'static str,
    display_path: &Path,
) -> Result<CString, StartupError> {
    if basename.is_empty()
        || basename == OsStr::new(".")
        || basename == OsStr::new("..")
        || Path::new(basename).components().count() != 1
        || !matches!(
            Path::new(basename).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(StartupError::unsafe_path(
            operation,
            display_path,
            UnsafePathReason::InvalidBasename,
        ));
    }
    cstring_component(basename, operation, display_path)
}

fn cstring_component(
    component: &OsStr,
    operation: &'static str,
    display_path: &Path,
) -> Result<CString, StartupError> {
    CString::new(component.as_bytes()).map_err(|_| {
        StartupError::unsafe_path(operation, display_path, UnsafePathReason::InvalidBasename)
    })
}

fn unlink_entry(
    parent_fd: RawFd,
    basename: &CStr,
    flags: libc::c_int,
    operation: &'static str,
    display_path: &Path,
) -> Result<(), StartupError> {
    let result = unsafe {
        // SAFETY: parent_fd is an open directory and basename is a validated
        // NUL-terminated component.
        libc::unlinkat(parent_fd, basename.as_ptr(), flags)
    };
    if result != 0 {
        return Err(StartupError::io(
            operation,
            display_path,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                // SAFETY: this object owns the DIR pointer returned by fdopendir.
                libc::closedir(self.0);
            }
        }
    }
}

fn read_directory_names(
    directory: &File,
    operation: &'static str,
    display_path: &Path,
) -> Result<Vec<OsString>, StartupError> {
    let duplicate = directory
        .try_clone()
        .map_err(|source| StartupError::io(operation, display_path, source))?;
    let raw_fd = duplicate.into_raw_fd();
    let stream = unsafe {
        // SAFETY: raw_fd is a newly duplicated directory descriptor. fdopendir
        // takes ownership on success.
        libc::fdopendir(raw_fd)
    };
    if stream.is_null() {
        let source = std::io::Error::last_os_error();
        unsafe {
            // SAFETY: fdopendir failed and did not take ownership of raw_fd.
            libc::close(raw_fd);
        }
        return Err(StartupError::io(operation, display_path, source));
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        unsafe {
            // SAFETY: errno is thread-local and the returned pointer remains
            // valid until the next readdir call on this stream.
            *libc::__errno_location() = 0;
        }
        let entry = unsafe {
            // SAFETY: stream owns a valid DIR pointer.
            libc::readdir(stream.0)
        };
        if entry.is_null() {
            let errno = unsafe {
                // SAFETY: errno is thread-local.
                *libc::__errno_location()
            };
            if errno != 0 {
                return Err(StartupError::io(
                    operation,
                    display_path,
                    std::io::Error::from_raw_os_error(errno),
                ));
            }
            break;
        }
        let name = unsafe {
            // SAFETY: d_name is NUL-terminated for the live dirent.
            CStr::from_ptr((*entry).d_name.as_ptr())
        }
        .to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(OsString::from_vec(name.to_vec()));
    }
    Ok(names)
}

fn remove_owned_directory_tree(
    parent: &SecureDirectory,
    basename: &OsStr,
    expected_device: libc::dev_t,
    uid: libc::uid_t,
    operation: &'static str,
) -> Result<(), StartupError> {
    parent.validate(operation)?;
    let display_path = parent.display_path.join(basename);
    let basename_c = validate_basename(basename, operation, &display_path)?;
    let directory = openat_file(
        parent.file.as_raw_fd(),
        &basename_c,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0,
        operation,
        &display_path,
    )?;
    let stat = stat_fd(&directory, operation, &display_path)?;
    validate_owned_directory(&stat, uid, operation, &display_path)?;
    if stat.st_dev != expected_device {
        return Err(StartupError::unsafe_path(
            operation,
            &display_path,
            UnsafePathReason::IdentityChanged,
        ));
    }
    let identity = Identity::of(&stat);
    require_entry_identity(
        &parent.file,
        &basename_c,
        identity,
        libc::S_IFDIR,
        operation,
        &display_path,
    )?;
    remove_directory_contents(&directory, &display_path, expected_device, uid, operation)?;
    require_entry_identity(
        &parent.file,
        &basename_c,
        identity,
        libc::S_IFDIR,
        operation,
        &display_path,
    )?;
    unlink_entry(
        parent.file.as_raw_fd(),
        &basename_c,
        libc::AT_REMOVEDIR,
        operation,
        &display_path,
    )
}

fn remove_directory_contents(
    directory: &File,
    display_path: &Path,
    expected_device: libc::dev_t,
    uid: libc::uid_t,
    operation: &'static str,
) -> Result<(), StartupError> {
    for name in read_directory_names(directory, operation, display_path)? {
        let child_path = display_path.join(&name);
        let child_c = validate_basename(&name, operation, &child_path)?;
        let entry = stat_at_required(
            directory.as_raw_fd(),
            &child_c,
            libc::AT_SYMLINK_NOFOLLOW,
            operation,
            &child_path,
        )?;
        let entry_identity = Identity::of(&entry);
        if file_type(&entry) == libc::S_IFDIR {
            let child = openat_file(
                directory.as_raw_fd(),
                &child_c,
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
                operation,
                &child_path,
            )?;
            let stat = stat_fd(&child, operation, &child_path)?;
            validate_owned_directory(&stat, uid, operation, &child_path)?;
            if stat.st_dev != expected_device || Identity::of(&stat) != entry_identity {
                return Err(StartupError::unsafe_path(
                    operation,
                    &child_path,
                    UnsafePathReason::IdentityChanged,
                ));
            }
            remove_directory_contents(&child, &child_path, expected_device, uid, operation)?;
            require_entry_identity(
                directory,
                &child_c,
                entry_identity,
                libc::S_IFDIR,
                operation,
                &child_path,
            )?;
            unlink_entry(
                directory.as_raw_fd(),
                &child_c,
                libc::AT_REMOVEDIR,
                operation,
                &child_path,
            )?;
        } else {
            let current = stat_at_required(
                directory.as_raw_fd(),
                &child_c,
                libc::AT_SYMLINK_NOFOLLOW,
                operation,
                &child_path,
            )?;
            if Identity::of(&current) != entry_identity {
                return Err(StartupError::unsafe_path(
                    operation,
                    &child_path,
                    UnsafePathReason::IdentityChanged,
                ));
            }
            unlink_entry(directory.as_raw_fd(), &child_c, 0, operation, &child_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    fn private_tempdir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("chmod tempdir");
        temp
    }

    fn prepare_temp_root(temp: &tempfile::TempDir) -> Arc<SecureConfigRoot> {
        Arc::new(
            SecureConfigRoot::prepare(&temp.path().join("config"))
                .expect("prepare secure config root"),
        )
    }

    fn assert_unsafe(error: StartupError, path: &Path) {
        match error {
            StartupError::UnsafePath {
                path: actual_path, ..
            } => assert_eq!(actual_path, path),
            other => panic!("expected unsafe-path error for {}: {other}", path.display()),
        }
    }

    struct ExactChild {
        child: Option<Child>,
        label: String,
    }

    impl ExactChild {
        fn wait_success(&mut self) {
            assert!(self.wait_status(), "exact child '{}' failed", self.label);
        }

        fn wait_status(&mut self) -> bool {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let child = self.child.as_mut().expect("exact child is still owned");
                match child.try_wait().expect("poll exact child") {
                    Some(status) => {
                        self.child = None;
                        return status.success();
                    }
                    None if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    None => {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.child = None;
                        return false;
                    }
                }
            }
        }
    }

    impl Drop for ExactChild {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn spawn_exact_lock_child(
        test_name: &str,
        role: &str,
        config: &Path,
        ready: &Path,
        release: &Path,
    ) -> ExactChild {
        let child = Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env("AC_TEST_1111_LOCK_ROLE", role)
            .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", config)
            .env("AC_TEST_1111_LOCK_READY", ready)
            .env("AC_TEST_1111_LOCK_RELEASE", release)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn exact lock child");
        ExactChild {
            child: Some(child),
            label: format!("{test_name}:{role}"),
        }
    }

    fn wait_for_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_release(path: &Path) {
        wait_for_file(path);
    }

    #[test]
    fn repairs_owned_modes_without_rebinding_existing_objects() {
        let temp = private_tempdir();
        let config = temp.path().join("config");
        std::fs::create_dir(&config).expect("create permissive root");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755))
            .expect("chmod root");
        let root_inode = std::fs::metadata(&config).expect("root metadata").ino();
        let root = Arc::new(SecureConfigRoot::prepare(&config).expect("prepare root"));
        assert_eq!(mode(&config), 0o700);
        assert_eq!(
            std::fs::metadata(&config).expect("root metadata").ino(),
            root_inode
        );

        let settings = config.join("settings.json");
        std::fs::write(&settings, b"{}").expect("seed settings");
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o644))
            .expect("chmod settings");
        let settings_inode = std::fs::metadata(&settings)
            .expect("settings metadata")
            .ino();
        assert!(root
            .validate_optional_private_file(
                OsStr::new("settings.json"),
                "test settings mode repair",
            )
            .expect("validate settings"));
        assert_eq!(mode(&settings), 0o600);
        assert_eq!(
            std::fs::metadata(&settings)
                .expect("settings metadata")
                .ino(),
            settings_inode
        );

        let instances = root
            .open_or_create_directory(OsStr::new("instances"), "test private directory")
            .expect("instances");
        assert_eq!(mode(instances.display_path()), 0o700);
        root.atomic_publish(
            OsStr::new("master-token.txt"),
            b"token",
            "test private publication",
        )
        .expect("publish token");
        assert_eq!(mode(&config.join("master-token.txt")), 0o600);
    }

    #[test]
    fn rejects_final_root_symlink_regular_file_and_fifo_but_allows_ancestor_symlink() {
        let symlink_temp = private_tempdir();
        let real = symlink_temp.path().join("real");
        std::fs::create_dir(&real).expect("real dir");
        let final_link = symlink_temp.path().join("config");
        symlink(&real, &final_link).expect("final symlink");
        assert_unsafe(
            SecureConfigRoot::prepare(&final_link).expect_err("reject final symlink"),
            &final_link,
        );

        let file_temp = private_tempdir();
        let final_file = file_temp.path().join("config");
        std::fs::write(&final_file, b"x").expect("final file");
        assert_unsafe(
            SecureConfigRoot::prepare(&final_file).expect_err("reject final file"),
            &final_file,
        );

        let fifo_temp = private_tempdir();
        let final_fifo = fifo_temp.path().join("config");
        let fifo_c = CString::new(final_fifo.as_os_str().as_bytes()).expect("fifo path");
        let result = unsafe {
            // SAFETY: fifo_c is a valid NUL-terminated path for this test fixture.
            libc::mkfifo(fifo_c.as_ptr(), 0o600)
        };
        assert_eq!(result, 0);
        assert_unsafe(
            SecureConfigRoot::prepare(&final_fifo).expect_err("reject final FIFO"),
            &final_fifo,
        );

        let ancestor_temp = private_tempdir();
        let real_parent = ancestor_temp.path().join("real-parent");
        std::fs::create_dir(&real_parent).expect("real parent");
        std::fs::set_permissions(&real_parent, std::fs::Permissions::from_mode(0o700))
            .expect("chmod real parent");
        let linked_parent = ancestor_temp.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).expect("ancestor symlink");
        let config = linked_parent.join("config");
        let root = SecureConfigRoot::prepare(&config).expect("allow safe ancestor symlink");
        root.validate("revalidate ancestor-symlink root")
            .expect("revalidate root");
        assert_eq!(mode(&real_parent.join("config")), 0o700);
    }

    #[test]
    fn rejects_unsafe_private_leaf_classes_without_touching_outside_data() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let config = root.display_path().to_path_buf();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"sentinel").expect("outside sentinel");

        let symlink_leaf = config.join("web-token.txt");
        symlink(&outside, &symlink_leaf).expect("leaf symlink");
        assert_unsafe(
            root.validate_optional_private_file(
                OsStr::new("web-token.txt"),
                "test unsafe symlink leaf",
            )
            .expect_err("reject leaf symlink"),
            &symlink_leaf,
        );
        std::fs::remove_file(&symlink_leaf).expect("remove leaf symlink");

        let hard_link_leaf = config.join("master-token.txt");
        std::fs::hard_link(&outside, &hard_link_leaf).expect("hard-link leaf");
        assert_unsafe(
            root.validate_optional_private_file(
                OsStr::new("master-token.txt"),
                "test unsafe hard-link leaf",
            )
            .expect_err("reject hard link"),
            &hard_link_leaf,
        );
        std::fs::remove_file(&hard_link_leaf).expect("remove hard link");

        let fifo_leaf = config.join("app-outbox-path.txt");
        let fifo_c = CString::new(fifo_leaf.as_os_str().as_bytes()).expect("fifo path");
        let result = unsafe {
            // SAFETY: fifo_c is a valid NUL-terminated path for this test fixture.
            libc::mkfifo(fifo_c.as_ptr(), 0o600)
        };
        assert_eq!(result, 0);
        assert_unsafe(
            root.validate_optional_private_file(
                OsStr::new("app-outbox-path.txt"),
                "test unsafe FIFO leaf",
            )
            .expect_err("reject FIFO"),
            &fifo_leaf,
        );
        std::fs::remove_file(&fifo_leaf).expect("remove FIFO");

        let directory_leaf = config.join("daemon.pid");
        std::fs::create_dir(&directory_leaf).expect("directory leaf");
        assert_unsafe(
            root.validate_optional_private_file(
                OsStr::new("daemon.pid"),
                "test unsafe directory leaf",
            )
            .expect_err("reject directory"),
            &directory_leaf,
        );
        assert_eq!(
            std::fs::read(&outside).expect("outside sentinel"),
            b"sentinel"
        );
    }

    #[test]
    fn stale_cleanup_removes_only_safe_canonical_uuid_directories() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let instances = root
            .open_or_create_directory(OsStr::new("instances"), "test instances")
            .expect("instances");
        let state = SecureGuiState {
            root,
            instances: Arc::clone(&instances),
        };

        let stale = uuid::Uuid::new_v4();
        let stale_dir = state
            .create_instance_directory(&stale)
            .expect("stale instance");
        state
            .create_instance_outbox(&stale_dir)
            .expect("stale outbox");

        let current = uuid::Uuid::new_v4();
        state
            .create_instance_directory(&current)
            .expect("current instance");

        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(outside.join("sentinel"), b"safe").expect("sentinel");
        let unsafe_uuid = uuid::Uuid::new_v4().hyphenated().to_string();
        symlink(&outside, instances.display_path().join(&unsafe_uuid))
            .expect("unsafe stale symlink");
        let uppercase = uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase();
        std::fs::create_dir(instances.display_path().join(&uppercase)).expect("noncanonical UUID");
        std::fs::create_dir(instances.display_path().join("not-a-uuid")).expect("non-UUID");

        state
            .cleanup_stale_instances(Some(&current))
            .expect("cleanup stale instances");
        assert!(!stale_dir.display_path().exists());
        assert!(instances
            .display_path()
            .join(current.hyphenated().to_string())
            .exists());
        assert!(std::fs::symlink_metadata(instances.display_path().join(&unsafe_uuid)).is_ok());
        assert!(instances.display_path().join(&uppercase).exists());
        assert!(instances.display_path().join("not-a-uuid").exists());
        assert_eq!(
            std::fs::read(outside.join("sentinel")).expect("outside sentinel"),
            b"safe"
        );
    }

    #[test]
    fn instance_directory_creation_is_exclusive() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let instances = root
            .open_or_create_directory(OsStr::new("instances"), "test instances")
            .expect("instances");
        let state = SecureGuiState { root, instances };
        let id = uuid::Uuid::new_v4();
        state
            .create_instance_directory(&id)
            .expect("first instance");
        match state.create_instance_directory(&id) {
            Err(StartupError::Io { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists)
            }
            other => panic!("expected exclusive-create collision, got {other:?}"),
        }
    }

    #[test]
    fn production_secure_operation_before_preparation_fails_in_exact_child() {
        const SENTINEL: &str = "AC_TEST_1111_UNPREPARED_SECURE_STATE";
        if std::env::var_os(SENTINEL).is_some() {
            let error = prepared_secure_config_root("test unprepared secure state")
                .expect_err("fresh child must be unprepared");
            assert!(matches!(error, StartupError::SecureStateNotPrepared { .. }));
            return;
        }

        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg(
                "config::linux_state::tests::production_secure_operation_before_preparation_fails_in_exact_child",
            )
            .arg("--nocapture")
            .env(SENTINEL, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn exact child");
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll child") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("unprepared-state child timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
    }

    #[test]
    fn production_singletons_publish_only_successful_retained_identities() {
        const TEST_NAME: &str =
            "config::linux_state::tests::production_singletons_publish_only_successful_retained_identities";
        if let Ok(role) = std::env::var("AC_TEST_1111_LOCK_ROLE") {
            match role.as_str() {
                "root-failure" => {
                    assert!(prepare_secure_config_root().is_err());
                    assert!(PREPARED_ROOT.get().is_none());
                    assert!(PREPARED_GUI_STATE.get().is_none());
                }
                "gui-failure" => {
                    let root = prepare_secure_config_root().expect("prepare retained root");
                    let guard = match acquire_gui_instance().expect("acquire GUI locks") {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => panic!("fresh GUI lock contended"),
                    };
                    std::fs::write(root.display_path().join("instances"), b"not-a-directory")
                        .expect("seed unsafe instances entry");
                    assert!(prepare_secure_gui_state(&guard).is_err());
                    let retained = prepared_secure_config_root("inspect retained root")
                        .expect("retained root");
                    assert!(Arc::ptr_eq(&root, &retained));
                    assert!(PREPARED_GUI_STATE.get().is_none());
                }
                "success" => {
                    let root = prepare_secure_config_root().expect("prepare retained root");
                    let guard = match acquire_gui_instance().expect("acquire GUI locks") {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => panic!("fresh GUI lock contended"),
                    };
                    let state =
                        prepare_secure_gui_state(&guard).expect("prepare retained GUI state");
                    let retained_root =
                        prepared_secure_config_root("inspect successful root").expect("root");
                    let retained_state =
                        prepared_secure_gui_state("inspect successful state").expect("state");
                    assert!(Arc::ptr_eq(&root, &retained_root));
                    assert!(Arc::ptr_eq(&root, &state.root));
                    assert!(Arc::ptr_eq(&state, &retained_state));
                }
                other => panic!("unexpected singleton child role {other}"),
            }
            return;
        }

        for role in ["root-failure", "gui-failure", "success"] {
            let temp = private_tempdir();
            let config = temp.path().join("config");
            if role == "root-failure" {
                std::fs::write(&config, b"not-a-directory").expect("seed root failure");
            }
            let ready = temp.path().join("unused-ready");
            let release = temp.path().join("unused-release");
            let mut child = spawn_exact_lock_child(TEST_NAME, role, &config, &ready, &release);
            child.wait_success();
        }
    }

    #[test]
    fn configured_parent_rebinding_fails_before_root_creation() {
        let temp = private_tempdir();
        let first_parent = temp.path().join("first-parent");
        let second_parent = temp.path().join("second-parent");
        std::fs::create_dir(&first_parent).expect("create first parent");
        std::fs::create_dir(&second_parent).expect("create second parent");
        std::fs::set_permissions(&first_parent, std::fs::Permissions::from_mode(0o700))
            .expect("chmod first parent");
        std::fs::set_permissions(&second_parent, std::fs::Permissions::from_mode(0o700))
            .expect("chmod second parent");
        let selected_parent = temp.path().join("selected-parent");
        symlink(&first_parent, &selected_parent).expect("create selected-parent symlink");
        let selected_for_hook = selected_parent.clone();
        let second_for_hook = second_parent.clone();
        let _hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::ConfiguredParentValidated && path == selected_for_hook {
                std::fs::remove_file(&selected_for_hook).expect("remove original parent symlink");
                symlink(&second_for_hook, &selected_for_hook)
                    .expect("rebind selected-parent symlink");
            }
        });

        let config = selected_parent.join("config");
        assert_unsafe(
            SecureConfigRoot::prepare(&config).expect_err("reject rebound configured parent"),
            &selected_parent,
        );
        assert!(!first_parent.join("config").exists());
        assert!(!second_parent.join("config").exists());
    }

    #[test]
    fn root_entry_rebinding_fails_before_mode_repair() {
        let temp = private_tempdir();
        let config = temp.path().join("config");
        let retained = temp.path().join("retained-config");
        std::fs::create_dir(&config).expect("create config");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755))
            .expect("set permissive config mode");
        let config_for_hook = config.clone();
        let retained_for_hook = retained.clone();
        let _hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::RootEntryOpened && path == config_for_hook {
                std::fs::rename(&config_for_hook, &retained_for_hook)
                    .expect("retain opened config directory");
                std::fs::create_dir(&config_for_hook).expect("create replacement config");
                std::fs::set_permissions(&config_for_hook, std::fs::Permissions::from_mode(0o711))
                    .expect("set replacement config mode");
            }
        });

        assert_unsafe(
            SecureConfigRoot::prepare(&config).expect_err("reject rebound root entry"),
            &config,
        );
        assert_eq!(mode(&retained), 0o755);
        assert_eq!(mode(&config), 0o711);
    }

    #[test]
    fn leaf_probe_and_open_rebinding_fail_before_mode_repair() {
        for hook_point in [TestHookPoint::LeafProbed, TestHookPoint::UsableLeafOpened] {
            let temp = private_tempdir();
            let root = prepare_temp_root(&temp);
            let settings = root.display_path().join("settings.json");
            let retained = root.display_path().join("retained-settings.json");
            std::fs::write(&settings, b"original").expect("seed settings");
            std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o644))
                .expect("set permissive settings mode");
            let settings_for_hook = settings.clone();
            let retained_for_hook = retained.clone();
            let _hook = TestHookGuard::install(move |point, path| {
                if point == hook_point && path == settings_for_hook {
                    std::fs::rename(&settings_for_hook, &retained_for_hook)
                        .expect("retain probed settings");
                    std::fs::write(&settings_for_hook, b"replacement")
                        .expect("create replacement settings");
                    std::fs::set_permissions(
                        &settings_for_hook,
                        std::fs::Permissions::from_mode(0o640),
                    )
                    .expect("set replacement settings mode");
                }
            });

            assert_unsafe(
                root.validate_optional_private_file(
                    OsStr::new("settings.json"),
                    "test deterministic leaf rebinding",
                )
                .expect_err("reject rebound settings"),
                &settings,
            );
            assert_eq!(
                std::fs::read(&retained).expect("read retained settings"),
                b"original"
            );
            assert_eq!(
                std::fs::read(&settings).expect("read replacement settings"),
                b"replacement"
            );
            assert_eq!(mode(&retained), 0o644);
            assert_eq!(mode(&settings), 0o640);
        }
    }

    #[test]
    fn temp_and_destination_rebinding_fail_without_outside_write() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"outside-sentinel").expect("write outside sentinel");
        let moved_temp = temp.path().join("retained-publisher-temp");
        let outside_for_hook = outside.clone();
        let moved_for_hook = moved_temp.clone();
        let _temp_hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::TempCreated {
                std::fs::rename(path, &moved_for_hook).expect("retain created temp");
                symlink(&outside_for_hook, path).expect("replace temp with symlink");
            }
        });
        let failure = root
            .atomic_publish_tracked(
                OsStr::new("settings.json"),
                b"new-settings",
                "test rebound publication temp",
            )
            .expect_err("reject rebound publication temp");
        assert!(matches!(failure.error, StartupError::UnsafePath { .. }));
        drop(_temp_hook);
        assert_eq!(
            std::fs::read(&outside).expect("read outside sentinel"),
            b"outside-sentinel"
        );
        assert_eq!(
            std::fs::read(&moved_temp).expect("read retained publisher temp"),
            b""
        );

        let destination = root.display_path().join("settings.json");
        let retained_destination = root.display_path().join("retained-destination.json");
        std::fs::write(&destination, b"old-settings").expect("seed destination");
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))
            .expect("chmod destination");
        let destination_for_hook = destination.clone();
        let retained_for_hook = retained_destination.clone();
        let _destination_hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::DestinationValidated && path == destination_for_hook {
                std::fs::rename(&destination_for_hook, &retained_for_hook)
                    .expect("retain validated destination");
                std::fs::write(&destination_for_hook, b"replacement-settings")
                    .expect("create replacement destination");
            }
        });
        let failure = root
            .atomic_publish_tracked(
                OsStr::new("settings.json"),
                b"new-settings",
                "test rebound publication destination",
            )
            .expect_err("reject rebound publication destination");
        assert!(matches!(failure.error, StartupError::UnsafePath { .. }));
        assert_eq!(
            std::fs::read(&retained_destination).expect("read retained destination"),
            b"old-settings"
        );
        assert_eq!(
            std::fs::read(&destination).expect("read replacement destination"),
            b"replacement-settings"
        );
    }

    #[test]
    fn tracked_publication_rollback_refuses_a_replacement_entry() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let destination = root.display_path().join("daemon.pid");
        let retained = root.display_path().join("retained-daemon.pid");
        let published = root
            .atomic_publish_tracked(OsStr::new("daemon.pid"), b"123", "test tracked publication")
            .expect("publish tracked file");
        std::fs::rename(&destination, &retained).expect("retain published file");
        std::fs::write(&destination, b"replacement").expect("create replacement file");
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))
            .expect("chmod replacement");

        assert_unsafe(
            published
                .unlink_if_still_owned("test tracked rollback")
                .expect_err("refuse replacement unlink"),
            &destination,
        );
        assert_eq!(
            std::fs::read(&retained).expect("read retained publication"),
            b"123"
        );
        assert_eq!(
            std::fs::read(&destination).expect("read replacement publication"),
            b"replacement"
        );
    }

    #[test]
    fn post_rename_replacement_is_ambiguous_and_is_not_mode_repaired() {
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let destination = root.display_path().join("settings.json");
        let retained = root.display_path().join("retained-settings.json");
        let destination_for_hook = destination.clone();
        let retained_for_hook = retained.clone();
        let _hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::PostRename && path == destination_for_hook {
                std::fs::rename(&destination_for_hook, &retained_for_hook)
                    .expect("retain renamed publication");
                std::fs::write(&destination_for_hook, b"replacement")
                    .expect("create post-rename replacement");
                std::fs::set_permissions(
                    &destination_for_hook,
                    std::fs::Permissions::from_mode(0o644),
                )
                .expect("chmod post-rename replacement");
            }
        });

        let failure = root
            .atomic_publish_tracked(
                OsStr::new("settings.json"),
                b"published",
                "test post-rename replacement",
            )
            .expect_err("post-rename replacement must be ambiguous");
        assert!(matches!(
            failure.error,
            StartupError::PublicationAmbiguous { .. }
        ));
        assert_eq!(
            std::fs::read(&retained).expect("read retained publication"),
            b"published"
        );
        assert_eq!(
            std::fs::read(&destination).expect("read replacement publication"),
            b"replacement"
        );
        assert_eq!(mode(&destination), 0o644);
    }

    #[test]
    fn injected_temp_collisions_use_eighth_name_and_exhaust_at_eight() {
        fn temp_name(basename: &str, uuid: uuid::Uuid, counter: u64) -> String {
            format!("{basename}.{}.{}.tmp", uuid.hyphenated(), counter)
        }

        let fixed_uuid =
            uuid::Uuid::parse_str("11111111-2222-4333-8444-555555555555").expect("fixed UUID");
        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        for counter in 0..7 {
            let path = root
                .display_path()
                .join(temp_name("settings.json", fixed_uuid, counter));
            std::fs::write(&path, format!("collision-{counter}")).expect("seed collision");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("chmod collision");
        }
        let mut uuid_source = || fixed_uuid;
        let mut next_counter = 0u64;
        let mut counter_source = || {
            let counter = next_counter;
            next_counter += 1;
            counter
        };
        atomic_publish_to_parent_with_sources(
            root.parent(),
            OsStr::new("settings.json"),
            b"published",
            "test bounded temp collisions",
            &mut uuid_source,
            &mut counter_source,
        )
        .expect("eighth temp name succeeds");
        assert_eq!(
            std::fs::read(root.display_path().join("settings.json"))
                .expect("read published settings"),
            b"published"
        );
        for counter in 0..7 {
            let path = root
                .display_path()
                .join(temp_name("settings.json", fixed_uuid, counter));
            assert_eq!(
                std::fs::read(&path).expect("read untouched collision"),
                format!("collision-{counter}").as_bytes()
            );
            assert_eq!(mode(&path), 0o644);
        }

        let exhausted_temp = private_tempdir();
        let exhausted_root = prepare_temp_root(&exhausted_temp);
        for counter in 0..8 {
            let path =
                exhausted_root
                    .display_path()
                    .join(temp_name("settings.json", fixed_uuid, counter));
            std::fs::write(&path, format!("collision-{counter}")).expect("seed collision");
        }
        let mut exhausted_uuid_source = || fixed_uuid;
        let mut next_counter = 0u64;
        let mut exhausted_counter_source = || {
            let counter = next_counter;
            next_counter += 1;
            counter
        };
        let failure = atomic_publish_to_parent_with_sources(
            exhausted_root.parent(),
            OsStr::new("settings.json"),
            b"never-published",
            "test exhausted temp collisions",
            &mut exhausted_uuid_source,
            &mut exhausted_counter_source,
        )
        .expect_err("eight collisions exhaust the budget");
        match failure.error {
            StartupError::Io { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists)
            }
            other => panic!("expected typed collision I/O error, got {other}"),
        }
        assert!(!exhausted_root.display_path().join("settings.json").exists());
    }

    #[test]
    fn injected_parent_metadata_enforces_owner_mode_and_sticky_policy() {
        let effective_uid = unsafe {
            // SAFETY: geteuid has no arguments and returns the caller's effective UID.
            libc::geteuid()
        };
        let foreign_uid = if effective_uid == 1 { 2 } else { 1 };
        let mut stat = unsafe {
            // SAFETY: a zeroed libc::stat is initialized before each field used
            // by the pure validation helpers below.
            std::mem::zeroed::<libc::stat>()
        };
        stat.st_mode = libc::S_IFDIR | 0o700;
        stat.st_uid = foreign_uid;
        assert!(matches!(
            validate_owned_directory(
                &stat,
                effective_uid,
                "test foreign directory",
                Path::new("/injected/foreign")
            ),
            Err(StartupError::UnsafePath {
                reason: UnsafePathReason::ForeignOwner { .. },
                ..
            })
        ));

        stat.st_uid = effective_uid;
        stat.st_mode = libc::S_IFDIR | 0o077;
        assert!(matches!(
            validate_trusted_directory(
                &stat,
                effective_uid,
                "test writable parent",
                Path::new("/injected/writable")
            ),
            Err(StartupError::UnsafePath {
                reason: UnsafePathReason::UntrustedParent { .. },
                ..
            })
        ));

        stat.st_uid = 0;
        stat.st_mode = libc::S_IFDIR | libc::S_ISVTX | 0o777;
        validate_trusted_directory(
            &stat,
            effective_uid,
            "test sticky root parent",
            Path::new("/injected/sticky"),
        )
        .expect("root-owned sticky parent is trusted");
    }

    #[test]
    fn private_modes_are_exact_under_zero_and_standard_umask_children() {
        const TEST_NAME: &str =
            "config::linux_state::tests::private_modes_are_exact_under_zero_and_standard_umask_children";
        if let Ok(role) = std::env::var("AC_TEST_1111_LOCK_ROLE") {
            let requested_umask = match role.as_str() {
                "umask-zero" => 0,
                "umask-standard" => 0o022,
                other => panic!("unexpected umask child role {other}"),
            };
            unsafe {
                // SAFETY: this exact-filter child runs only this test and exits
                // without executing another test that could observe its umask.
                libc::umask(requested_umask);
            }
            let temp = private_tempdir();
            let root = prepare_temp_root(&temp);
            let instances = root
                .open_or_create_directory(OsStr::new("instances"), "test umask instances")
                .expect("create instances");
            let instance = instances
                .create_new_directory(
                    OsStr::new("11111111-2222-4333-8444-555555555555"),
                    "test umask instance",
                )
                .expect("create instance");
            let outbox = instance
                .open_or_create_directory(OsStr::new("outbox"), "test umask outbox")
                .expect("create outbox");
            root.atomic_publish(OsStr::new("settings.json"), b"{}", "test umask settings")
                .expect("publish settings");
            let log_file = root
                .open_append_private_file(OsStr::new("app.log"), "test umask log")
                .expect("create log");
            let lock = match try_private_lock(
                Arc::clone(&root),
                MUTATION_LOCK_BASENAME,
                "test umask lock",
            )
            .expect("create lock")
            {
                LockAttempt::Acquired(lock) => lock,
                LockAttempt::Contended => panic!("fresh lock unexpectedly contended"),
            };

            for directory in [
                root.display_path(),
                instances.display_path(),
                instance.display_path(),
                outbox.display_path(),
            ] {
                assert_eq!(mode(directory), 0o700);
            }
            for file in [
                root.display_path().join("settings.json"),
                log_file.display_path().to_path_buf(),
                lock.display_path().to_path_buf(),
            ] {
                assert_eq!(mode(&file), 0o600);
            }
            return;
        }

        let temp = private_tempdir();
        let config = temp.path().join("unused-config");
        let ready = temp.path().join("unused-ready");
        let release = temp.path().join("unused-release");
        for role in ["umask-zero", "umask-standard"] {
            let mut child = spawn_exact_lock_child(TEST_NAME, role, &config, &ready, &release);
            child.wait_success();
        }
    }

    #[test]
    fn ordered_locks_are_config_scoped_and_route_without_publication_changes() {
        const TEST_NAME: &str =
            "config::linux_state::tests::ordered_locks_are_config_scoped_and_route_without_publication_changes";
        if let Ok(role) = std::env::var("AC_TEST_1111_LOCK_ROLE") {
            let ready =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_READY").expect("ready path"));
            let release =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_RELEASE").expect("release path"));
            let root = prepare_secure_config_root().expect("prepare production root");
            match role.as_str() {
                "owner" => {
                    let guard = match acquire_gui_instance().expect("acquire GUI locks") {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => panic!("fresh GUI lock is contended"),
                    };
                    prepare_secure_gui_state(&guard).expect("prepare GUI state");
                    root.atomic_publish(
                        OsStr::new("sentinel.txt"),
                        b"stable-sentinel",
                        "publish lock-test sentinel",
                    )
                    .expect("publish sentinel");
                    std::fs::write(&ready, b"ready").expect("publish ready barrier");
                    wait_for_release(&release);
                    drop(guard);
                }
                "probe" => {
                    assert!(matches!(
                        acquire_gui_instance().expect("probe GUI locks"),
                        GuiLockOutcome::AlreadyRunning
                    ));
                    assert!(matches!(
                        coding_agent_mutation_route().expect("probe mutation route"),
                        LinuxMutationRoute::QueueToRunningGui
                    ));
                }
                "different" => {
                    let mut guard = match acquire_gui_instance().expect("acquire different config")
                    {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => {
                            panic!("different config unexpectedly contended")
                        }
                    };
                    assert!(guard.release().is_empty());
                }
                "reacquire" => {
                    let mut guard = match acquire_gui_instance().expect("reacquire original config")
                    {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => {
                            panic!("released original config remained contended")
                        }
                    };
                    assert!(guard.release().is_empty());
                    assert!(matches!(
                        coding_agent_mutation_route().expect("direct route after release"),
                        LinuxMutationRoute::DirectWithGuard(_)
                    ));
                }
                other => panic!("unexpected lock child role {other}"),
            }
            return;
        }

        let temp = private_tempdir();
        let config = temp.path().join("config-a");
        let other_config = temp.path().join("config-b");
        let ready = temp.path().join("owner-ready");
        let release = temp.path().join("owner-release");
        let mut owner = spawn_exact_lock_child(TEST_NAME, "owner", &config, &ready, &release);
        wait_for_file(&ready);

        let sentinel = config.join("sentinel.txt");
        let before = std::fs::symlink_metadata(&sentinel).expect("sentinel metadata");
        let before_snapshot = (
            before.dev(),
            before.ino(),
            before.permissions().mode() & 0o7777,
            std::fs::read(&sentinel).expect("read sentinel"),
        );
        let mut probe = spawn_exact_lock_child(TEST_NAME, "probe", &config, &ready, &release);
        probe.wait_success();
        let after = std::fs::symlink_metadata(&sentinel).expect("sentinel metadata after probe");
        let after_snapshot = (
            after.dev(),
            after.ino(),
            after.permissions().mode() & 0o7777,
            std::fs::read(&sentinel).expect("read sentinel after probe"),
        );
        assert_eq!(before_snapshot, after_snapshot);

        let mut different =
            spawn_exact_lock_child(TEST_NAME, "different", &other_config, &ready, &release);
        different.wait_success();
        std::fs::write(&release, b"release").expect("release owner");
        owner.wait_success();

        let mut reacquire =
            spawn_exact_lock_child(TEST_NAME, "reacquire", &config, &ready, &release);
        reacquire.wait_success();
    }

    #[test]
    fn protocol_anomalies_and_direct_writer_contention_fail_closed() {
        const TEST_NAME: &str =
            "config::linux_state::tests::protocol_anomalies_and_direct_writer_contention_fail_closed";
        if let Ok(role) = std::env::var("AC_TEST_1111_LOCK_ROLE") {
            let ready =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_READY").expect("ready path"));
            let release =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_RELEASE").expect("release path"));
            let root = prepare_secure_config_root().expect("prepare production root");
            match role.as_str() {
                "gui-only-owner" => {
                    let gui_lock = match try_private_lock(
                        root,
                        GUI_LOCK_BASENAME,
                        "hold anomalous GUI-only lock",
                    )
                    .expect("acquire GUI-only lock")
                    {
                        LockAttempt::Acquired(lock) => lock,
                        LockAttempt::Contended => panic!("fresh GUI-only lock contended"),
                    };
                    std::fs::write(&ready, b"ready").expect("publish ready barrier");
                    wait_for_release(&release);
                    drop(gui_lock);
                }
                "gui-only-probe" => {
                    assert!(matches!(
                        match acquire_gui_instance() {
                            Err(error) => error,
                            Ok(_) => panic!("GUI-only state unexpectedly acquired or contended"),
                        },
                        StartupError::LockStateInconsistent { .. }
                    ));
                    assert!(matches!(
                        match coding_agent_mutation_route() {
                            Err(error) => error,
                            Ok(_) => panic!("GUI-only route unexpectedly succeeded"),
                        },
                        StartupError::LockStateInconsistent { .. }
                    ));
                }
                "direct-owner" => {
                    let direct = match coding_agent_mutation_route()
                        .expect("acquire direct mutation route")
                    {
                        LinuxMutationRoute::DirectWithGuard(guard) => guard,
                        LinuxMutationRoute::QueueToRunningGui => {
                            panic!("fresh route unexpectedly queued")
                        }
                    };
                    std::fs::write(&ready, b"ready").expect("publish ready barrier");
                    wait_for_release(&release);
                    drop(direct);
                }
                "direct-probe" => {
                    assert!(matches!(
                        acquire_gui_instance().expect("probe GUI against direct writer"),
                        GuiLockOutcome::AlreadyRunning
                    ));
                    assert!(matches!(
                        match coding_agent_mutation_route() {
                            Err(error) => error,
                            Ok(_) => panic!("second direct writer unexpectedly succeeded"),
                        },
                        StartupError::MutationBusy { .. }
                    ));
                }
                other => panic!("unexpected anomaly child role {other}"),
            }
            return;
        }

        for (owner_role, probe_role) in [
            ("gui-only-owner", "gui-only-probe"),
            ("direct-owner", "direct-probe"),
        ] {
            let temp = private_tempdir();
            let config = temp.path().join("config");
            let ready = temp.path().join("owner-ready");
            let release = temp.path().join("owner-release");
            let mut owner =
                spawn_exact_lock_child(TEST_NAME, owner_role, &config, &ready, &release);
            wait_for_file(&ready);
            let mut probe =
                spawn_exact_lock_child(TEST_NAME, probe_role, &config, &ready, &release);
            probe.wait_success();
            std::fs::write(&release, b"release").expect("release owner");
            owner.wait_success();
        }
    }

    #[test]
    fn lock_rebinding_and_inverse_release_window_are_deterministic() {
        const TEST_NAME: &str =
            "config::linux_state::tests::lock_rebinding_and_inverse_release_window_are_deterministic";
        if let Ok(role) = std::env::var("AC_TEST_1111_LOCK_ROLE") {
            let config = PathBuf::from(
                std::env::var_os("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("config path"),
            );
            let ready =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_READY").expect("ready path"));
            let release =
                PathBuf::from(std::env::var_os("AC_TEST_1111_LOCK_RELEASE").expect("release path"));
            prepare_secure_config_root().expect("prepare production root");
            match role.as_str() {
                "release-owner" => {
                    let mut guard = match acquire_gui_instance().expect("acquire GUI guard") {
                        GuiLockOutcome::Acquired(guard) => guard,
                        GuiLockOutcome::AlreadyRunning => panic!("fresh GUI lock contended"),
                    };
                    let probe_succeeded = std::rc::Rc::new(std::cell::Cell::new(false));
                    let observed = std::rc::Rc::clone(&probe_succeeded);
                    let config_for_hook = config.clone();
                    let ready_for_hook = ready.clone();
                    let release_for_hook = release.clone();
                    let _hook = TestHookGuard::install(move |point, _| {
                        if point == TestHookPoint::BetweenGuiAndMutationUnlock {
                            let mut probe = spawn_exact_lock_child(
                                TEST_NAME,
                                "release-probe",
                                &config_for_hook,
                                &ready_for_hook,
                                &release_for_hook,
                            );
                            observed.set(probe.wait_status());
                        }
                    });
                    assert!(guard.release().is_empty());
                    assert!(
                        probe_succeeded.get(),
                        "release-window probe did not observe the required state"
                    );
                }
                "release-probe" => {
                    assert!(matches!(
                        match coding_agent_mutation_route() {
                            Err(error) => error,
                            Ok(_) => panic!("release-window mutation unexpectedly succeeded"),
                        },
                        StartupError::MutationBusy { .. }
                    ));
                }
                other => panic!("unexpected release child role {other}"),
            }
            return;
        }

        let temp = private_tempdir();
        let root = prepare_temp_root(&temp);
        let mutation_path = root.display_path().join(MUTATION_LOCK_BASENAME);
        let retained_path = root.display_path().join("retained-mutation.lock");
        let mutation_for_hook = mutation_path.clone();
        let retained_for_hook = retained_path.clone();
        let _hook = TestHookGuard::install(move |point, path| {
            if point == TestHookPoint::LockAcquired && path == mutation_for_hook {
                std::fs::rename(&mutation_for_hook, &retained_for_hook)
                    .expect("retain acquired mutation lock");
                std::fs::write(&mutation_for_hook, b"replacement")
                    .expect("create replacement mutation lock");
                std::fs::set_permissions(
                    &mutation_for_hook,
                    std::fs::Permissions::from_mode(0o600),
                )
                .expect("chmod replacement lock");
            }
        });
        assert_unsafe(
            match try_private_lock(
                Arc::clone(&root),
                MUTATION_LOCK_BASENAME,
                "test lock rebinding",
            ) {
                Err(error) => error,
                Ok(_) => panic!("rebound lock unexpectedly produced a guard"),
            },
            &mutation_path,
        );
        assert_eq!(
            std::fs::read(&mutation_path).expect("read replacement lock"),
            b"replacement"
        );
        drop(_hook);

        let child_temp = private_tempdir();
        let config = child_temp.path().join("config");
        let ready = child_temp.path().join("unused-ready");
        let release = child_temp.path().join("unused-release");
        let mut release_owner =
            spawn_exact_lock_child(TEST_NAME, "release-owner", &config, &ready, &release);
        release_owner.wait_success();
    }
}
