use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use uuid::Uuid;

#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
#[cfg(windows)]
use std::time::Duration;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::telegram::manager::OutputSenderMap;

struct PtyInstance {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    job: Option<crate::pty::job::JobObject>,
}

#[cfg(windows)]
struct GitGuardEnv {
    path: String,
    pathext: String,
    real_git: String,
}

#[cfg(windows)]
static GIT_GUARD_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
const GIT_GUARD_PUBLISH_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
    Duration::from_millis(1600),
];

#[cfg(windows)]
fn resolve_real_git_path() -> Option<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = std::process::Command::new("where.exe");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    cmd.arg("git.exe").creation_flags(CREATE_NO_WINDOW);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
}

#[cfg(windows)]
fn ensure_git_guard_wrapper() -> Result<std::path::PathBuf, AppError> {
    let config_dir = crate::config::config_dir()
        .ok_or_else(|| AppError::Other("Could not resolve app config directory".to_string()))?;
    let guard_dir = config_dir.join("git-guard");
    std::fs::create_dir_all(&guard_dir)
        .map_err(|e| AppError::Other(format!("Failed to create git-guard dir: {}", e)))?;

    let cmd_path = guard_dir.join("git.cmd");
    let ps1_path = guard_dir.join("git-guard.ps1");

    let cmd_content = "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0git-guard.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n";
    let ps1_content = r#"$ErrorActionPreference = 'Stop'
$realGit = $env:AC_REAL_GIT
if ([string]::IsNullOrWhiteSpace($realGit)) {
  Write-Error 'AgentsCommander git guard: AC_REAL_GIT is not set.'
  exit 1
}

$originalArgs = @($args)
$target = (Get-Location).Path

for ($i = 0; $i -lt $originalArgs.Count; $i++) {
  $arg = [string]$originalArgs[$i]
  if ($arg -eq '-C') {
    if ($i + 1 -ge $originalArgs.Count) {
      Write-Error 'AgentsCommander git guard: missing path after -C.'
      exit 1
    }

    $next = [string]$originalArgs[$i + 1]
    if ([System.IO.Path]::IsPathRooted($next)) {
      $target = [System.IO.Path]::GetFullPath($next)
    } else {
      $target = [System.IO.Path]::GetFullPath((Join-Path -Path $target -ChildPath $next))
    }
    $i++
    continue
  }

  if ($arg -eq '--git-dir' -or $arg -like '--git-dir=*' -or $arg -eq '--work-tree' -or $arg -like '--work-tree=*') {
    Write-Error 'AgentsCommander git guard: --git-dir and --work-tree are not allowed in agent sessions.'
    exit 1
  }
}

function Test-AllowedGitTarget([string]$path) {
  try {
    $current = [System.IO.Path]::GetFullPath($path)
  } catch {
    return $false
  }

  while ($true) {
    $name = [System.IO.Path]::GetFileName($current)
    if ($name -like 'repo-*') {
      return $true
    }

    $parent = Split-Path -Path $current -Parent
    if ([string]::IsNullOrWhiteSpace($parent) -or $parent -eq $current) {
      break
    }
    $current = $parent
  }

  return $false
}

if (-not (Test-AllowedGitTarget $target)) {
  Write-Error ('AgentsCommander git guard: git is only allowed inside repo-* directories. Target path: ' + $target)
  exit 1
}

& $realGit @originalArgs
exit $LASTEXITCODE
"#;

    write_git_guard_file_if_changed(&cmd_path, cmd_content)
        .map_err(|e| AppError::Other(format!("Failed to write git.cmd guard: {}", e)))?;
    write_git_guard_file_if_changed(&ps1_path, ps1_content)
        .map_err(|e| AppError::Other(format!("Failed to write git-guard.ps1: {}", e)))?;

    Ok(guard_dir)
}

#[cfg(windows)]
fn write_git_guard_file_if_changed(path: &Path, content: &str) -> Result<(), String> {
    let desired = content.as_bytes();
    match std::fs::read(path) {
        Ok(existing) if existing == desired => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read existing {}: {}", path.display(), e)),
    }

    write_git_guard_file_atomic(path, desired)
}

#[cfg(windows)]
fn write_git_guard_file_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("target {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("git-guard");
    let counter = GIT_GUARD_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.{}.{counter}.tmp", std::process::id()));

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("create temp {}: {}", temp.display(), e))?;

    if let Err(e) = file.write_all(content) {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.flush() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("flush temp {}: {}", temp.display(), e));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(format!("sync temp {}: {}", temp.display(), e));
    }
    drop(file);

    publish_git_guard_temp_with_retry(&temp, path, content)
}

#[cfg(windows)]
fn publish_git_guard_temp_with_retry(
    temp: &Path,
    path: &Path,
    content: &[u8],
) -> Result<(), String> {
    let attempts = GIT_GUARD_PUBLISH_RETRY_DELAYS
        .iter()
        .copied()
        .map(Some)
        .chain(std::iter::once(None));

    for (attempt, delay) in attempts.enumerate() {
        if git_guard_file_matches(path, content) {
            let _ = std::fs::remove_file(temp);
            return Ok(());
        }

        match crate::config::root_agent::atomic_replace_existing(temp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if git_guard_file_matches(path, content) {
                    let _ = std::fs::remove_file(temp);
                    return Ok(());
                }

                let Some(delay) = delay else {
                    let _ = std::fs::remove_file(temp);
                    return Err(format!(
                        "publish {} from {} failed after {} attempts: {}",
                        path.display(),
                        temp.display(),
                        attempt + 1,
                        e
                    ));
                };
                std::thread::sleep(delay);
            }
        }
    }

    unreachable!("retry loop includes a final no-delay attempt")
}

#[cfg(windows)]
fn git_guard_file_matches(path: &Path, content: &[u8]) -> bool {
    match std::fs::read(path) {
        Ok(existing) => existing == content,
        Err(_) => false,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::{Arc, Barrier};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    fn assert_no_temp_files(dir: &Path) {
        for entry in fs::read_dir(dir).expect("read temp dir") {
            let name = entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string();
            assert!(!name.ends_with(".tmp"), "unexpected temp file: {name}");
        }
    }

    #[allow(clippy::permissions_set_readonly_false)]
    #[test]
    fn git_guard_writer_skips_unchanged_readonly_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git-guard.ps1");
        fs::write(&path, "same").expect("write fixture");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("set readonly");

        let result = write_git_guard_file_if_changed(&path, "same");

        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).expect("clear readonly");

        result.expect("unchanged readonly file should be skipped");
        assert_eq!(fs::read_to_string(&path).expect("read file"), "same");
        assert_no_temp_files(dir.path());
    }

    #[test]
    fn git_guard_writer_replaces_changed_file_without_temp_leftover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git.cmd");
        fs::write(&path, "old").expect("write fixture");

        write_git_guard_file_if_changed(&path, "new").expect("replace changed file");

        assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
        assert_no_temp_files(dir.path());
    }

    #[test]
    fn git_guard_writer_retries_in_use_destination_until_released() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("git.cmd");
        fs::write(&path, "old").expect("write fixture");
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .expect("open destination without delete sharing");
        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = Arc::clone(&barrier);
        let writer_path = path.clone();

        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            write_git_guard_file_if_changed(&writer_path, "new")
        });

        barrier.wait();
        std::thread::sleep(Duration::from_millis(1000));
        drop(held);

        writer
            .join()
            .expect("writer thread")
            .expect("retry should publish after held handle is released");
        assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
        assert_no_temp_files(dir.path());
    }
}

#[cfg(windows)]
fn build_git_guard_env() -> Result<Option<GitGuardEnv>, AppError> {
    let Some(real_git) = resolve_real_git_path() else {
        log::warn!("[pty] git.exe not found; skipping PATH git guard wrapper");
        return Ok(None);
    };

    let guard_dir = ensure_git_guard_wrapper()?;
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path_entries: Vec<std::path::PathBuf> = vec![guard_dir];
    path_entries.extend(std::env::split_paths(&current_path));
    let path = std::env::join_paths(path_entries.iter())
        .map_err(|e| AppError::Other(format!("Failed to join PATH for git guard: {}", e)))?
        .to_string_lossy()
        .to_string();

    Ok(Some(GitGuardEnv {
        path,
        pathext: ".CMD;.BAT;.COM;.EXE".to_string(),
        real_git,
    }))
}

pub struct LocalProcessBackend {
    ptys: Arc<Mutex<HashMap<Uuid, PtyInstance>>>,
    fanout: SessionIoFanout,
    git_watcher: Arc<GitWatcher>,
}

impl Clone for LocalProcessBackend {
    fn clone(&self) -> Self {
        Self {
            ptys: Arc::clone(&self.ptys),
            fanout: self.fanout.clone(),
            git_watcher: Arc::clone(&self.git_watcher),
        }
    }
}

impl LocalProcessBackend {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            fanout: SessionIoFanout::new(output_senders, idle_detector, ws_broadcaster),
            git_watcher,
        }
    }

    fn spawn_sync(&self, spec: BackendSpawnSpec) -> Result<(), AppError> {
        let BackendSpawnSpec {
            id,
            cmd,
            args,
            cwd,
            selected_cwd: _,
            cols,
            rows,
            container_image: _,
            configured_env,
            env_remove_keys,
            env_unset: _,
            extra_env,
            idle_tuning,
            output_target,
            mut resource_registration,
            logical_resource_slot: _,
        } = spec;
        let pty_system = native_pty_system();
        let spawn_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        let is_direct_exe = cmd.to_lowercase().ends_with(".exe")
            || std::path::Path::new(&cmd)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));

        let mut command = if cfg!(windows) && !is_direct_exe {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/C");
            c.arg(&cmd);
            for arg in &args {
                c.arg(arg);
            }
            c
        } else {
            let mut c = CommandBuilder::new(&cmd);
            for arg in &args {
                c.arg(arg);
            }
            c
        };
        command.cwd(&spawn_cwd);
        for key in &env_remove_keys {
            command.env_remove(key);
        }
        for (key, value) in &configured_env {
            command.env(key, value);
        }
        if !configured_env.is_empty() || !env_remove_keys.is_empty() {
            log::info!(
                "[pty] Applied {} configured env vars and removed {} inherited env vars for session {}",
                configured_env.len(),
                env_remove_keys.len(),
                id
            );
        }
        crate::pty::credentials::apply_credential_env_to_pty_command(&mut command, &extra_env);
        command.env("TERM", "xterm-256color");

        if !extra_env.is_empty() {
            log::info!(
                "[pty] Applied {} per-process credential environment variables for session {}",
                extra_env.len(),
                id
            );
        }

        if let Some(git_ceiling_dirs) =
            crate::config::session_context::git_ceiling_directories_for_session_root(&spawn_cwd)
        {
            command.env("GIT_CEILING_DIRECTORIES", &git_ceiling_dirs);
            log::info!(
                "[pty] Applied GIT_CEILING_DIRECTORIES for session cwd {}: {}",
                spawn_cwd,
                git_ceiling_dirs
            );

            #[cfg(windows)]
            if let Some(git_guard_env) = build_git_guard_env()? {
                command.env("PATH", &git_guard_env.path);
                command.env("PATHEXT", &git_guard_env.pathext);
                command.env("AC_REAL_GIT", &git_guard_env.real_git);
                log::info!(
                    "[pty] Enabled git guard wrapper for session cwd {}",
                    spawn_cwd
                );
            }
        }

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| AppError::PtyError(e.to_string()))?;
        log::info!(
            "[pty] Spawned session {} with child pid {:?}",
            id,
            child.process_id()
        );

        let job = child
            .process_id()
            .and_then(crate::pty::job::JobObject::for_child);

        if let Some(registration) = resource_registration.as_mut() {
            let Some(pid) = child.process_id() else {
                let _ = child.kill();
                return Err(AppError::PtyError(
                    "Resource Monitor could not capture spawned child pid".to_string(),
                ));
            };
            if let Err(err) = registration.register_root_pid(pid) {
                let _ = child.kill();
                return Err(AppError::PtyError(err));
            }
        }

        drop(pair.slave);

        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(e) => {
                if let Some(registration) = resource_registration.as_ref() {
                    registration.rollback_registered();
                }
                return Err(AppError::PtyError(e.to_string()));
            }
        };

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => {
                if let Some(registration) = resource_registration.as_ref() {
                    registration.rollback_registered();
                }
                return Err(AppError::PtyError(e.to_string()));
            }
        };

        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Some(child),
            job,
        };

        self.ptys.lock().unwrap().insert(id, instance);
        self.fanout.register_session(id, idle_tuning, rows, cols);

        let session_id_str = id.to_string();
        let fanout = self.fanout.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        fanout.handle_output(&output_target, id, &session_id_str, buf[..n].to_vec())
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }
}

type BlockingSpawnCleanup = dyn Fn(Uuid) + Send + Sync + 'static;

struct BlockingSpawnCancelGuard {
    id: Uuid,
    cancelled: Arc<AtomicBool>,
    cleanup: Arc<BlockingSpawnCleanup>,
    disarmed: bool,
}

impl BlockingSpawnCancelGuard {
    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for BlockingSpawnCancelGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }

        self.cancelled.store(true, Ordering::SeqCst);
        (self.cleanup)(self.id);
    }
}

fn spawn_blocking_cancel_safe<F, C>(
    id: Uuid,
    work: F,
    cleanup: C,
    join_error_context: &'static str,
) -> futures::future::BoxFuture<'static, Result<(), AppError>>
where
    F: FnOnce() -> Result<(), AppError> + Send + 'static,
    C: Fn(Uuid) + Send + Sync + 'static,
{
    Box::pin(async move {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cleanup: Arc<BlockingSpawnCleanup> = Arc::new(cleanup);
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_cleanup = Arc::clone(&cleanup);
        let mut guard = BlockingSpawnCancelGuard {
            id,
            cancelled: Arc::clone(&cancelled),
            cleanup: Arc::clone(&cleanup),
            disarmed: false,
        };
        let handle = tokio::task::spawn_blocking(move || {
            let result = work();
            if worker_cancelled.load(Ordering::SeqCst) && result.is_ok() {
                (worker_cleanup)(id);
            }
            result
        });
        let result = handle
            .await
            .map_err(|e| AppError::Other(format!("{join_error_context}: {e}")))?;
        guard.disarm();
        result
    })
}

impl PtyBackend for LocalProcessBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        let id = spec.id;
        let spawn_backend = self.clone();
        let cleanup_backend = self.clone();
        spawn_blocking_cancel_safe(
            id,
            move || spawn_backend.spawn_sync(spec),
            move |id| {
                if let Err(err) = cleanup_backend.kill(id) {
                    log::warn!("[pty] Failed to clean up cancelled local spawn {id}: {err}");
                }
            },
            "local process spawn task failed",
        )
    }

    fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError> {
        let ptys = self.ptys.lock().unwrap();
        let instance = ptys
            .get(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;

        let mut writer = instance.writer.lock().unwrap();
        writer
            .write_all(data)
            .map_err(|e| AppError::PtyError(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.ptys.lock().unwrap().contains_key(&id)
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        self.fanout.record_resize(id);

        let ptys = self.ptys.lock().unwrap();
        let instance = ptys
            .get(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;

        let master = instance.master.lock().unwrap();
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        self.fanout.resize_screen_and_broadcast(id, cols, rows);

        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let instance = {
            let mut ptys = self.ptys.lock().unwrap();
            ptys.remove(&id)
        };

        if let Some(mut instance) = instance {
            if let Some(job) = instance.job.take() {
                job.terminate();
            }
            if let Some(mut child) = instance.child.take() {
                let pid = child.process_id();
                match child.try_wait() {
                    Ok(Some(status)) => {
                        log::info!(
                            "[pty] Session {} child pid {:?} already exited: {:?}",
                            id,
                            pid,
                            status
                        );
                    }
                    Ok(None) => {
                        if let Err(e) = child.kill() {
                            log::warn!(
                                "[pty] Failed to kill session {} child pid {:?}: {}",
                                id,
                                pid,
                                e
                            );
                        }
                        reap_child_in_background(id, pid, child);
                    }
                    Err(e) => {
                        log::warn!(
                            "[pty] Failed to poll session {} child pid {:?}: {}",
                            id,
                            pid,
                            e
                        );
                        if let Err(kill_err) = child.kill() {
                            log::warn!(
                                "[pty] Failed to kill session {} child pid {:?} after poll error: {}",
                                id,
                                pid,
                                kill_err
                            );
                        }
                        reap_child_in_background(id, pid, child);
                    }
                }
            }
        }

        self.fanout.remove_session(id);
        self.git_watcher.remove_session(id);

        Ok(())
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        let ptys = self.ptys.lock().unwrap();
        match ptys.get(&id).and_then(|inst| inst.job.as_ref()) {
            Some(job) => {
                job.terminate();
                true
            }
            None => false,
        }
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        let mut terminated = 0;
        let mut jobless = 0;
        for (id, instance) in ptys.iter() {
            match instance.job.as_ref() {
                Some(job) => {
                    job.terminate();
                    terminated += 1;
                    log::info!("[pty] terminated job object for session {id} at shutdown");
                }
                None => jobless += 1,
            }
        }
        (terminated, jobless)
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        self.fanout
            .register_response_watcher(session_id, request_id, response_dir);
    }
}

fn reap_child_in_background(
    session_id: Uuid,
    pid: Option<u32>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
) {
    std::thread::spawn(move || match child.wait() {
        Ok(status) => {
            log::info!(
                "[pty] Reaped session {} child pid {:?}: {:?}",
                session_id,
                pid,
                status
            );
        }
        Err(e) => {
            log::warn!(
                "[pty] Failed to reap session {} child pid {:?}: {}",
                session_id,
                pid,
                e
            );
        }
    });
}

#[cfg(test)]
mod cancel_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_blocking_spawn_cleans_up_after_worker_finishes() {
        let id = Uuid::new_v4();
        let registered = Arc::new(AtomicBool::new(false));
        let cleanup_count = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (cleanup_tx, cleanup_rx) = mpsc::channel();

        let registered_for_work = Arc::clone(&registered);
        let work = move || {
            started_tx.send(()).expect("send started");
            release_rx.recv().expect("wait for release");
            registered_for_work.store(true, Ordering::SeqCst);
            done_tx.send(()).expect("send done");
            Ok(())
        };

        let registered_for_cleanup = Arc::clone(&registered);
        let cleanup_count_for_cleanup = Arc::clone(&cleanup_count);
        let expected_id = id;
        let cleanup = move |cleanup_id| {
            assert_eq!(cleanup_id, expected_id);
            registered_for_cleanup.store(false, Ordering::SeqCst);
            cleanup_count_for_cleanup.fetch_add(1, Ordering::SeqCst);
            let _ = cleanup_tx.send(());
        };

        let task = tokio::spawn(spawn_blocking_cancel_safe(
            id,
            work,
            cleanup,
            "test spawn failed",
        ));
        started_rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("blocking worker should start");

        task.abort();
        cleanup_rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("dropping waiter should request cleanup");
        release_tx.send(()).expect("release worker");
        done_rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("blocking worker should finish work");
        cleanup_rx
            .recv_timeout(StdDuration::from_secs(2))
            .expect("cancelled worker should clean up registered state");

        assert!(
            !registered.load(Ordering::SeqCst),
            "registered state should be cleaned up after cancellation"
        );
        assert_eq!(cleanup_count.load(Ordering::SeqCst), 2);
    }
}
