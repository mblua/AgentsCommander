use super::{
    FS_RETRY_COUNT, FS_RETRY_DELAY_MS, REQUESTS_DIR, RESPONSES_DIR, SESSION_READ_RETRY_COUNT,
    SESSION_READ_RETRY_DELAY_MS,
};
use crate::filesystem::{directories, documents};
use serde::Serialize;
use std::io;
use std::io::Write;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn write_json_atomic_new<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json_atomic(path, value, false)
}

pub(super) fn write_json_atomic_replace<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json_atomic(path, value, true)
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct SessionReadHookState {
    pub(super) active: bool,
    pub(super) path: PathBuf,
    pub(super) callback: Option<Box<dyn FnOnce()>>,
}

#[cfg(test)]
thread_local! {
    pub(super) static SESSION_READ_HOOK: std::cell::RefCell<SessionReadHookState> =
        std::cell::RefCell::new(SessionReadHookState::default());
}

#[cfg(test)]
pub(super) struct SessionReadHookGuard(std::marker::PhantomData<std::rc::Rc<()>>);

#[cfg(test)]
impl SessionReadHookGuard {
    pub(super) fn install(path: PathBuf, callback: impl FnOnce() + 'static) -> Self {
        SESSION_READ_HOOK.with(|state| {
            let mut state = state.borrow_mut();
            assert!(!state.active, "session read hook already registered");
            *state = SessionReadHookState {
                active: true,
                path,
                callback: Some(Box::new(callback)),
            };
        });
        Self(std::marker::PhantomData)
    }
}

#[cfg(test)]
impl Drop for SessionReadHookGuard {
    fn drop(&mut self) {
        let _ = SESSION_READ_HOOK.try_with(|state| {
            if let Ok(mut state) = state.try_borrow_mut() {
                *state = SessionReadHookState::default();
            }
        });
    }
}

#[cfg(test)]
pub(super) fn observe_session_read_not_found(path: &Path) {
    let callback = SESSION_READ_HOOK.with(|state| {
        let mut state = state.borrow_mut();
        if state.active && state.path == path {
            state.callback.take()
        } else {
            None
        }
    });
    if let Some(callback) = callback {
        callback();
    }
}

pub(super) fn read_session_file_with_retry(path: &Path) -> io::Result<String> {
    let mut last_not_found = None;
    for attempt in 0..SESSION_READ_RETRY_COUNT {
        match documents::read_ui_automation_text(path) {
            Ok(raw) => return Ok(raw),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && attempt + 1 < SESSION_READ_RETRY_COUNT =>
            {
                last_not_found = Some(e);
                #[cfg(test)]
                observe_session_read_not_found(path);
                std::thread::sleep(Duration::from_millis(SESSION_READ_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_not_found.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("session file not found: {}", path.display()),
        )
    }))
}

pub(super) fn write_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    replace: bool,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        directories::ensure_ui_automation_directory(parent)?;
    }

    if !replace && directories::ui_automation_path_exists(path) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", path.display()),
        ));
    }

    let tmp_path = path.with_extension("tmp");
    let mut file = documents::create_ui_automation_stage(&tmp_path, replace)?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    drop(file);

    if replace {
        let _ = retry_remove_file(path);
    } else if directories::ui_automation_path_exists(path) {
        let _ = retry_remove_file(&tmp_path);
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", path.display()),
        ));
    }
    retry_rename(&tmp_path, path)
}

pub(super) fn retry_rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_fs(|| documents::rename_ui_automation_file(from, to))
}

pub(super) fn retry_remove_file(path: &Path) -> io::Result<()> {
    if !directories::ui_automation_path_exists(path) {
        return Ok(());
    }
    retry_fs(|| documents::remove_ui_automation_file(path))
}

pub(super) fn retry_remove_dir_all(path: &Path) -> io::Result<()> {
    if !directories::ui_automation_path_exists(path) {
        return Ok(());
    }
    retry_fs(|| directories::remove_ui_automation_directory(path))
}

pub(super) fn retry_fs(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut last_err = None;
    for attempt in 0..FS_RETRY_COUNT {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_fs_error(&e) && attempt + 1 < FS_RETRY_COUNT => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(FS_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("filesystem retry failed")))
}

pub(super) fn is_transient_fs_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32) | Some(33))
}

pub(super) fn cleanup_stale_automation_files(automation_dir: &Path) -> io::Result<()> {
    let requests = automation_dir.join(REQUESTS_DIR);
    let responses = automation_dir.join(RESPONSES_DIR);
    if directories::ui_automation_path_exists(&requests) {
        retry_remove_dir_all(&requests)?;
    }
    if directories::ui_automation_path_exists(&responses) {
        retry_remove_dir_all(&responses)?;
    }
    Ok(())
}
