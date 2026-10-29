//! Physical TASK operations; policy and transaction orchestration stay with callers.

use sha2::Digest;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

pub(crate) fn read_task_title_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) enum TaskLockError {
    Timeout,
    Io(std::io::Error),
}

pub(crate) struct TaskLock {
    _file: File,
}
impl TaskLock {
    pub(crate) fn acquire(path: &Path, timeout: Duration) -> Result<Self, TaskLockError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(TaskLockError::Io)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if start.elapsed() >= timeout {
                        return Err(TaskLockError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(TaskLockError::Io(e)),
            }
        }
    }
}

pub(crate) fn read_description_snapshot(
    path: &Path,
    limit: u64,
) -> std::io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

pub(crate) fn description_sentinel(path: &Path) -> std::io::Result<(u64, Option<SystemTime>)> {
    let meta = fs::metadata(path)?;
    Ok((meta.len(), meta.modified().ok()))
}

pub(crate) struct StatusWindow {
    pub(crate) bytes: Vec<u8>,
    pub(crate) start: u64,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

pub(crate) fn read_status_window(
    path: &Path,
    window: u64,
) -> std::io::Result<Option<StatusWindow>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    let len = file.seek(SeekFrom::End(0))?;
    let start = len.saturating_sub(window);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(window).read_to_end(&mut bytes)?;
    Ok(Some(StatusWindow {
        bytes,
        start,
        len,
        modified: meta.modified().ok(),
    }))
}

#[derive(PartialEq)]
pub(crate) struct TaskSourceState {
    pub(crate) hash: Option<String>,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

pub(crate) fn task_source_state(path: &Path) -> std::io::Result<TaskSourceState> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TaskSourceState {
                hash: None,
                len: 0,
                modified: None,
            })
        }
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    let mut hash = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut len = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        len += n as u64;
    }
    if len != meta.len() {
        return Err(std::io::Error::other("source changed while hashing"));
    }
    Ok(TaskSourceState {
        hash: Some(format!("{:x}", hash.finalize())),
        len,
        modified: meta.modified().ok(),
    })
}

pub(crate) fn rename_task_artifact_with_retry(source: &Path, target: &Path) -> std::io::Result<()> {
    for attempt in 0..3 {
        match std::fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 2
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(e.raw_os_error(), Some(5) | Some(32))) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub(crate) fn read_task_artifact(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

pub(crate) fn read_clean_journal(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

pub(crate) fn task_artifact_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn reserve_task_backup(path: &Path) -> std::io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    drop(file);
    Ok(())
}

pub(crate) fn copy_description_backup(source: &Path, target: &Path) -> std::io::Result<u64> {
    fs::copy(source, target)
}

pub(crate) struct TaskWriter {
    file: File,
}

pub(crate) fn create_task_artifact(path: &Path) -> std::io::Result<TaskWriter> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|file| TaskWriter { file })
}

pub(crate) fn open_status_append(path: &Path) -> std::io::Result<TaskWriter> {
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map(|file| TaskWriter { file })
}

pub(crate) fn open_task_sync(path: &Path) -> std::io::Result<TaskWriter> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map(|file| TaskWriter { file })
}

pub(crate) fn open_status_repair(path: &Path) -> std::io::Result<TaskWriter> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .map(|file| TaskWriter { file })
}

impl TaskWriter {
    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)
    }

    pub(crate) fn truncate_status(&self, len: u64) -> std::io::Result<()> {
        self.file.set_len(len)
    }

    pub(crate) fn sync_at(&self, boundary: &str) -> std::io::Result<()> {
        io_boundary(boundary)?;
        self.file.sync_all()
    }

    pub(crate) fn copy_archive_from(
        &mut self,
        source: &Path,
        exists: bool,
        boundary: &str,
        owned_hash: &mut String,
    ) -> std::io::Result<()> {
        let mut hash = sha2::Sha256::new();
        io_boundary(&format!("{boundary}_copy"))?;
        if exists {
            let mut source = std::fs::File::open(source)?;
            let mut buffer = [0u8; 65536];
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                let mut offset = 0;
                while offset < count {
                    io_boundary(&format!("{boundary}_write"))?;
                    let written = self.file.write(&buffer[offset..count])?;
                    if written == 0 {
                        return Err(std::io::ErrorKind::WriteZero.into());
                    }
                    hash.update(&buffer[offset..offset + written]);
                    *owned_hash = format!("{:x}", hash.clone().finalize());
                    offset += written;
                    io_boundary(&format!("{boundary}_after_write"))?;
                }
            }
        }
        self.sync_at(&format!("{boundary}_sync"))
    }

    pub(crate) fn append_status_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        #[cfg(test)]
        if IO_FAULT.with(|f| f.borrow().0.as_deref() == Some("append_partial")) {
            self.file.write_all(&bytes[..20])?;
            return Err(std::io::Error::other("fixture partial"));
        }
        io_boundary("append_write")?;
        self.file.write_all(bytes)?;
        self.sync_at("append_sync")
    }
}

pub(crate) fn write_description_stage(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    io_boundary("nonclean_tmp_write")?;
    fs::write(path, bytes)
}

pub(crate) fn publish_description_stage(source: &Path, target: &Path) -> std::io::Result<()> {
    for attempt in 0..=2u32 {
        let rename = || {
            #[cfg(test)]
            io_boundary("nonclean_rename")?;
            std::fs::rename(source, target)
        };
        match rename() {
            Ok(_) => return Ok(()),
            Err(e) => {
                let retry = e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(32) // ERROR_SHARING_VIOLATION
                    || e.raw_os_error() == Some(5); // ERROR_ACCESS_DENIED
                if attempt < 2 && retry {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                return Err(e);
            }
        }
    }
    unreachable!("loop body always returns")
}

pub(crate) fn remove_task_artifact(path: &Path) -> std::io::Result<()> {
    fs::remove_file(path)
}

pub(crate) fn io_boundary(name: &str) -> std::io::Result<()> {
    #[cfg(test)]
    {
        tests::io_boundary(name)
    }
    #[cfg(not(test))]
    {
        let _ = name;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) use tests::{IO_FAULT, IO_MUTATION};

#[cfg(test)]
mod tests;
