use std::path::Path;
use std::time::Duration;

use crate::filesystem::tasks::{TaskLock, TaskLockError};

use super::operation::TaskOpError;

/// Timeout for the cooperative lock. Mirrors the issue acceptance criterion
/// "concurrent writes from two coordinators don't corrupt the file" — first
/// wins, second polls every 50 ms for up to this window, else `LockTimeout`.
pub(crate) const LOCK_TIMEOUT_5S: Duration = Duration::from_secs(5);

/// Compatibility acquisition argument; age no longer controls ownership.
pub(crate) const LOCK_STALE_AFTER_5M: Duration = Duration::from_secs(300);

/// Stable kernel lock. Each acquisition opens a separate handle; the kernel
/// releases ownership on process death. Never unlink or replace this file.
pub(crate) struct LockGuard {
    _lock: TaskLock,
}
impl LockGuard {
    pub(crate) fn acquire(
        path: &Path,
        timeout: Duration,
        _stale_after: Duration,
    ) -> Result<Self, TaskOpError> {
        match TaskLock::acquire(path, timeout) {
            Ok(lock) => Ok(Self { _lock: lock }),
            Err(TaskLockError::Timeout) => Err(TaskOpError::LockTimeout),
            Err(TaskLockError::Io(e)) => Err(TaskOpError::LockIo(path.to_path_buf(), e)),
        }
    }
}
