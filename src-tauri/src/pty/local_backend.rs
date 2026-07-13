use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
use crate::pty::spawn_diagnostics::{self, ChildLiveness, ExitCause, SpawnRecord, SpawnRecordInit};
use crate::telegram::manager::OutputSenderMap;

struct PtyInstance {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    job: Option<crate::pty::job::JobObject>,
    /// #973 (C) - the size the ConPTY is actually at, so a resize that changes nothing is
    /// not sent to the child. Seeded from the size the PTY was opened at.
    size: Mutex<(u16, u16)>,
    /// #973 (B) - held closed until the child has rendered something. See `StartupGate`.
    startup_gate: Mutex<StartupGate>,
    /// #973 (B) - the same fact as the gate, as one relaxed atomic, so the PTY read loop
    /// can skip the lock on every chunk once the child is up. The gate is the truth; this
    /// is only a fast path, and a stale `false` costs one no-op call.
    rendered: Arc<AtomicBool>,
}

/// #973 (B) - a PTY resize that lands while a coding agent's TUI is starting up makes it
/// redraw its still-empty viewport and lose the wakeup for the content that becomes ready
/// right after. The terminal stays blank, the process stays alive, and any keypress paints
/// it. Measured on a bare ConPTY: a resize in that window blanks Codex 8 times in 10.
///
/// So AC does not resize a child that has not rendered anything yet. Resizes are held, the
/// LAST one is kept, and it is applied the moment the child paints - which is the earliest
/// moment it is safe to.
///
/// **Why the trigger is "has rendered" and not a timer.** The danger window is
/// [TUI is up, content is ready], and both ends move with machine load: in production
/// first-paint spans 324 ms to 2163 ms, while on an idle box the window sits at 180-300 ms.
/// Any fixed delay is a constant fitted to one machine, and on a slower one it lands
/// *inside* the window - a fix that reproduces the bug. A render, by contrast, cannot happen
/// inside the window, because the window is defined as the interval in which the child has
/// rendered nothing. The trigger is immune to load, terminal height and frame count.
///
/// **Why not #942's paint floor.** That floor is 256 bytes and the blank child sits at 345:
/// it would fire for a child that is hung.
///
/// **Why there is no timeout escape.** A child that never renders is showing nothing, so a
/// resize it never receives changes nothing a user can see - and the instant it does render,
/// the size it should have had is applied. A timeout would be the very constant this design
/// exists to avoid. Shells are unaffected: a prompt is printable, so their gate opens on the
/// first chunk.
enum StartupGate {
    /// The child has not rendered yet. Resizes are held here, last one wins.
    Holding(Option<(u16, u16)>),
    /// The child has rendered. Resizes go straight through, forever.
    Open,
}

/// #973 - decide what to do with a resize the view asked for, and do it. This is the whole
/// of B and C, and it is a free function over the instance so it can be tested against a
/// real ConPTY: the backend itself cannot be built in a unit test, because its `GitWatcher`
/// needs a Tauri `AppHandle`, and none of that has anything to do with resizing a PTY.
///
/// Returns whether the ConPTY was actually resized.
fn resize_instance(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // B - do not resize a child that has not rendered anything yet. The view fires its fit
    // 300-500 ms after spawn, which is exactly when a coding agent's TUI is coming up, and a
    // resize there costs it its first content render. Hold the size; the read loop hands it
    // over the moment the child paints. See `StartupGate`.
    {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if gate.on_resize(cols, rows).is_none() {
            log::debug!(
                "[pty] resize {id} to {cols}x{rows} held: the child has not rendered yet (#973)"
            );
            return Ok(false);
        }
    }
    send_size_to_conpty(instance, id, cols, rows)
}

/// #973 (C) - do not tell the child about a resize that is not a resize.
///
/// ConPTY delivers even a same-size `ResizePseudoConsole` to the client as a real event, and
/// the view fires 5-20 identical resizes per attach (`TerminalView.tsx:85-95`, a double
/// `requestAnimationFrame` plus a `ResizeObserver`), so today every attach shakes the child
/// for nothing.
///
/// The cached size is written only AFTER the ConPTY has accepted the new one: if the resize
/// failed and we recorded it anyway, every retry would be skipped as a no-op and the PTY
/// would be wedged at the wrong size forever.
fn send_size_to_conpty(
    instance: &PtyInstance,
    id: Uuid,
    cols: u16,
    rows: u16,
) -> Result<bool, AppError> {
    // #973 - refuse a degenerate size. `pty_resize` hands us whatever the view computed, and
    // xterm's `fit()` really does return a zero dimension while its container is still being
    // laid out (the same hazard `PtyViewport::from_fit` guards at spawn). ConPTY does NOT
    // reject it: `master.resize(0x0)` returns Ok and the child is left with no screen at all.
    // A stale size is recoverable; a zero-column terminal is not.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    if !instance.size_changed(cols, rows) {
        log::debug!("[pty] resize {id} skipped: already {cols}x{rows} (#973)");
        return Ok(false);
    }

    {
        let master = instance
            .master
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::PtyError(e.to_string()))?;
    }
    instance.remember_size(cols, rows);
    Ok(true)
}

impl StartupGate {
    /// The view asked for a resize. Returns the size to hand the ConPTY, or `None` if the
    /// child is still starting up and it must be held.
    fn on_resize(&mut self, cols: u16, rows: u16) -> Option<(u16, u16)> {
        match self {
            StartupGate::Holding(pending) => {
                // Only the last size matters: the frontend fires 5-20 of these per attach.
                *pending = Some((cols, rows));
                None
            }
            StartupGate::Open => Some((cols, rows)),
        }
    }

    /// The child rendered. Open the gate for good and hand back the size that was held.
    fn open(&mut self) -> Option<(u16, u16)> {
        match std::mem::replace(self, StartupGate::Open) {
            StartupGate::Holding(pending) => pending,
            StartupGate::Open => None,
        }
    }
}

impl PtyInstance {
    /// #973 (C) - has the size actually moved?
    ///
    /// The frontend calls resize unconditionally (`TerminalView.tsx:85-95`, from a double
    /// `requestAnimationFrame` plus a `ResizeObserver`), so one attach fires 5-20 identical
    /// resizes, and ConPTY hands every one of them to the child as a real event.
    fn size_changed(&self, cols: u16, rows: u16) -> bool {
        Self::size_changed_in(&self.size, cols, rows)
    }

    /// Only after the ConPTY has actually accepted it: if `resize` failed, the cached size
    /// must stay stale so the next attempt is not skipped as a no-op and the PTY is not
    /// wedged at the wrong size forever.
    fn remember_size(&self, cols: u16, rows: u16) {
        Self::remember_size_in(&self.size, cols, rows);
    }

    // The two free functions above are the whole of C. Split out so they can be tested
    // without a live ConPTY: a `PtyInstance` owns real `MasterPty` handles.
    fn size_changed_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) -> bool {
        *size.lock().unwrap_or_else(|e| e.into_inner()) != (cols, rows)
    }

    fn remember_size_in(size: &Mutex<(u16, u16)>, cols: u16, rows: u16) {
        *size.lock().unwrap_or_else(|e| e.into_inner()) = (cols, rows);
    }
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
            agent_id,
            coding_agent,
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
            container_credential: _,
            container_repo_mounts: _,
        } = spec;
        // #942 - how many sessions were spawned in the window just before this one,
        // and how many of them were the same CLI. Concurrent startups against shared
        // agent state (the global ~/.codex) are a prime suspect for the intermittent
        // blank terminal, so every spawn record carries its own concurrency context.
        // Keyed on the CLI, never on the profile id: several profiles run the same
        // codex binary against the same ~/.codex. Counting only, no behavior change.
        let diag_thresholds = spawn_diagnostics::Thresholds::from_env();
        let spawn_window = spawn_diagnostics::note_spawn_attempt(coding_agent, diag_thresholds);
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

        // #942 - the argv exactly as executed, cmd.exe wrapper included. Mirrors the
        // branch above instead of reshaping the CommandBuilder, so the spawn stays
        // byte-for-byte what it was.
        let exec_argv: Vec<String> = if cfg!(windows) && !is_direct_exe {
            let mut argv = vec!["cmd.exe".to_string(), "/C".to_string(), cmd.clone()];
            argv.extend(args.iter().cloned());
            argv
        } else {
            let mut argv = vec![cmd.clone()];
            argv.extend(args.iter().cloned());
            argv
        };

        // #942 - what the child will really see for CODEX_HOME: an explicit configured
        // value wins, then an explicit removal, then the AC environment the child
        // inherits. None here means this Codex shares the global ~/.codex with every
        // other one.
        let codex_home = configured_env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("CODEX_HOME"))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                if env_remove_keys
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case("CODEX_HOME"))
                {
                    None
                } else {
                    std::env::var("CODEX_HOME").ok()
                }
            });

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

        // #942 - time zero for time-to-first-output.
        let spawn_started = Instant::now();
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| AppError::PtyError(e.to_string()))?;
        let child_pid = child.process_id();
        log::info!("[pty] Spawned session {} with child pid {:?}", id, child_pid);

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

        // #973 (B) - the child has rendered nothing yet, so the gate starts closed.
        let rendered = Arc::new(AtomicBool::new(false));
        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Some(child),
            job,
            // #973 - the size we actually opened the ConPTY at (see PtyViewport).
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::clone(&rendered),
        };

        self.ptys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, instance);
        self.fanout.register_session(id, idle_tuning, rows, cols);

        // #942 - app.log is what users paste into issues, so a secret must never be
        // echoed back out through the child output or the argv we log. Keyed on the env
        // NAME (token / key / secret / password / credential), across both AC's own
        // credential env and the configured rows where AC tells users to keep their API
        // keys. Deliberately not a sweep of every value: a Codex profile legitimately
        // carries `MODEL=gpt-5.6-sol`, and blanking that would shred the very argv this
        // instrumentation exists to record.
        let redact: Vec<String> = extra_env
            .iter()
            .chain(configured_env.iter())
            .filter(|(key, _)| spawn_diagnostics::is_secret_env_key(key))
            .map(|(_, value)| value.clone())
            .collect();

        // #942 - emits `[pty] spawn-record` (argv, cwd, CLI, profile, CODEX_HOME,
        // concurrency).
        let record = spawn_diagnostics::register(SpawnRecordInit {
            session_id: id,
            pid: child_pid,
            argv: exec_argv,
            cwd: spawn_cwd,
            cli: coding_agent,
            agent_profile_id: agent_id,
            codex_home,
            configured_env_count: configured_env.len(),
            removed_env_count: env_remove_keys.len(),
            redact,
            window: spawn_window,
            started: spawn_started,
            thresholds: diag_thresholds,
        });

        // #942 - the startup verdict at the deadline and exit attribution. The PTY
        // reader alone cannot carry either: ConPTY holds the pipe open past the death of
        // the child, so a child that dies on its own would stay invisible until the
        // session is torn down. The monitor polls the child handle instead and ends with
        // the session (detached on purpose; the handle is only useful to tests).
        let monitor_backend = self.clone();
        let _monitor = spawn_diagnostics::watch_child(Arc::clone(&record), move || {
            monitor_backend.probe_child(id)
        });

        let session_id_str = id.to_string();
        let fanout = self.fanout.clone();
        let gate_backend = self.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // #942 - time-to-first-output and the retained head bytes. Hot
                        // path: once the first byte is stamped and the head buffer is
                        // full this is two relaxed loads and one relaxed add.
                        record.note_output(&buf[..n]);
                        // #973 (B) - has the child rendered anything yet? Once it has, this
                        // is a single relaxed load per chunk and nothing else: no lock, no
                        // allocation, no scan. Before it has, it is the same predicate the
                        // idle detector runs on this chunk anyway.
                        if !rendered.load(Ordering::Relaxed)
                            && crate::pty::output::output_has_printable_activity(
                                &String::from_utf8_lossy(&buf[..n]),
                            )
                        {
                            gate_backend.open_startup_gate(id);
                        }
                        fanout.handle_output(&output_target, id, &session_id_str, buf[..n].to_vec())
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    /// #973 (B) - the child has rendered something, so the startup window is behind us.
    /// Open the gate for good and hand the ConPTY the size the view has been waiting to give
    /// it. Called from the PTY read loop, once per session in practice.
    fn open_startup_gate(&self, id: Uuid) {
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        let Some(instance) = ptys.get(&id) else {
            return;
        };

        // One lock covers both "take the held size" and "open the gate", so a resize landing
        // at this exact instant cannot be recorded into a gate that is already open and then
        // dropped on the floor, leaving the terminal stuck at the wrong size.
        let pending = {
            let mut gate = instance
                .startup_gate
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            gate.open()
        };
        // Only a fast path for the read loop; the gate above is the truth. Publishing it
        // late is safe: a stale `false` costs one extra no-op call to this function.
        instance.rendered.store(true, Ordering::Relaxed);

        let Some((cols, rows)) = pending else {
            return;
        };

        match send_size_to_conpty(instance, id, cols, rows) {
            Ok(true) => {
                drop(ptys);
                log::info!(
                    "[pty] startup gate open for {id}: applied the held resize {cols}x{rows} (#973)"
                );
                self.fanout.resize_screen_and_broadcast(id, cols, rows);
            }
            Ok(false) => {}
            Err(e) => {
                // Non-critical: the child is up and painting, it is just the wrong size.
                log::warn!("[pty] held resize {id} to {cols}x{rows} failed: {e} (#973)");
            }
        }
    }

    /// #942 - liveness of the child of a session, without disturbing it. Never blocks
    /// (zero timeout) and never reports a child it could not query as running.
    fn probe_child(&self, id: Uuid) -> ChildLiveness {
        let mut ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        let Some(instance) = ptys.get_mut(&id) else {
            return ChildLiveness::Gone;
        };
        let Some(child) = instance.child.as_mut() else {
            return ChildLiveness::Gone;
        };
        probe_child_contained(child)
    }
}

/// #942 - probe a child while the caller holds the `ptys` guard, and CONTAIN any panic.
///
/// portable-pty locks a mutex of its own inside the child and unwraps it (`try_wait`,
/// `do_kill`, `process_id` and `as_raw_handle` all do). If that mutex is ever poisoned,
/// an unwind from the probe would escape while the `ptys` guard is held and poison the
/// PTY map itself, which every terminal write, resize and kill locks on: a diagnostics
/// probe would silently take the terminal subsystem down with it. Catching AT the call
/// means the guard above us is released normally and the map stays usable.
fn probe_child_contained(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe_child_liveness(child)))
        .unwrap_or_else(|_| {
            ChildLiveness::Unqueryable("portable-pty child lock is poisoned".to_string())
        })
}

/// #942 - ask Windows directly instead of going through `portable_pty::Child::try_wait`.
/// `WinChild::is_complete` cannot answer honestly: it swallows a failed
/// `GetExitCodeProcess` and returns "not exited" (so a handle whose query rights were
/// stripped by AV/EDR, a known scenario here, reads as ALIVE), and its `STILL_ACTIVE`
/// sentinel is 259, which is also a legal exit code. The process handle is signalled if
/// and only if the child is gone, whatever it exited with, and a handle we cannot wait
/// on fails loudly instead of pretending.
#[cfg(windows)]
fn probe_child_liveness(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

    let Some(handle) = child.as_raw_handle() else {
        return ChildLiveness::Unqueryable("child exposes no process handle".to_string());
    };

    match unsafe { WaitForSingleObject(handle as _, 0) } {
        WAIT_TIMEOUT => ChildLiveness::Alive,
        WAIT_OBJECT_0 => {
            let mut code: u32 = 0;
            if unsafe { GetExitCodeProcess(handle as _, &mut code) } == 0 {
                let os_error = unsafe { GetLastError() };
                return ChildLiveness::Unqueryable(format!(
                    "GetExitCodeProcess failed (os error {os_error})"
                ));
            }
            ChildLiveness::Exited {
                code,
                success: code == 0,
            }
        }
        WAIT_FAILED => {
            let os_error = unsafe { GetLastError() };
            ChildLiveness::Unqueryable(format!("WaitForSingleObject failed (os error {os_error})"))
        }
        other => ChildLiveness::Unqueryable(format!("WaitForSingleObject returned {other}")),
    }
}

/// #942 - on Unix `try_wait` is `waitpid(WNOHANG)`: it reports a failed poll as `Err`,
/// so it can be trusted to tell "running" from "we could not ask".
#[cfg(not(windows))]
fn probe_child_liveness(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> ChildLiveness {
    match child.try_wait() {
        Ok(Some(status)) => ChildLiveness::from(&status),
        Ok(None) => ChildLiveness::Alive,
        Err(e) => ChildLiveness::Unqueryable(e.to_string()),
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
        // #942 - poison-tolerant: a panic anywhere under this guard (portable-pty unwraps
        // inside its own child polling) must not brick every terminal write that follows.
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
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
        self.ptys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        self.fanout.record_resize(id);

        {
            let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            let instance = ptys
                .get(&id)
                .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
            resize_instance(instance, id, cols, rows)?;
        }

        // The screen and the idle grace see every resize the view asked for, held or not:
        // only the ConPTY call is gated. Idle semantics are #954's, not this fix's.
        self.fanout.resize_screen_and_broadcast(id, cols, rows);

        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        // #942 - probe the child BEFORE we tag the stop and BEFORE we touch job or
        // child. That ordering is the whole trick: nothing AC does can have killed a
        // child this probe already finds dead, so "was it already gone when we asked?"
        // has a witness that no race can flip. The old "already exited" line fired both
        // for a child that had died on its own and for one our own job kill had just
        // taken down; this is what tells the two apart.
        let child_at_stop = self.probe_child(id);
        let record =
            spawn_diagnostics::mark_ac_stop(id, "session-kill", Some(child_at_stop.clone()));

        let instance = {
            let mut ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            ptys.remove(&id)
        };

        if let Some(mut instance) = instance {
            if let Some(job) = instance.job.take() {
                job.terminate();
            }
            if let Some(mut child) = instance.child.take() {
                let pid = child.process_id();
                log::info!(
                    "[pty] session-stop session={} pid={:?} source=session-kill child_at_stop={}",
                    id,
                    pid,
                    child_at_stop.as_log()
                );
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Dead by the time we removed it. The pre-stop probe decides
                        // whose kill this was: already dead before we touched anything
                        // means the child ended itself.
                        let liveness = ChildLiveness::from(&status);
                        match record.as_ref() {
                            Some(record) => {
                                let cause = record.attribute_exit(record.stop_snapshot());
                                record.log_child_exit(cause, &liveness, "observed-at-stop");
                            }
                            None => {
                                let cause =
                                    if matches!(child_at_stop, ChildLiveness::Exited { .. }) {
                                        ExitCause::ChildInitiated
                                    } else {
                                        ExitCause::AcRequested
                                    };
                                log::info!(
                                    "[pty] child-exit session={} pid={:?} cause={} detail=observed-at-stop child={}",
                                    id,
                                    pid,
                                    cause.as_log(),
                                    liveness.as_log()
                                );
                            }
                        }
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
                        reap_child_in_background(id, pid, child, record);
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
                        reap_child_in_background(id, pid, child, record);
                    }
                }
            }
        }

        self.fanout.remove_session(id);
        self.git_watcher.remove_session(id);
        spawn_diagnostics::forget(id);

        Ok(())
    }

    /// #942 - record who asked for the stop and what the child looked like BEFORE any
    /// process is touched. The resource monitor kills a process tree without going through
    /// the PTY layer, so a caller that is about to do that publishes the witness here
    /// first; without it, a child that had already died on its own would be logged as our
    /// kill and its evidence dropped.
    fn publish_stop_witness(&self, id: Uuid, source: &str) {
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, source, Some(child_at_stop));
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        // #942 - AC asked for this stop. Probe first (same reason as `kill`), then tag,
        // so a child that was already dead is never charged to us. This stop can also
        // FAIL (a Quarantined resource-monitor kill leaves the instance intact), which
        // is why the tag only owns exits that follow it inside the attribution window.
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, "job-terminate", Some(child_at_stop));
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        match ptys.get(&id).and_then(|inst| inst.job.as_ref()) {
            Some(job) => {
                job.terminate();
                true
            }
            None => false,
        }
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        let mut ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        let mut terminated = 0;
        let mut jobless = 0;
        for (id, instance) in ptys.iter_mut() {
            // #942 - shutdown stops every live session; tag them all as ours, with the
            // same pre-stop witness every other stop path publishes. Lock order here is
            // ptys -> diagnostics registry; nothing ever takes them the other way round
            // (the monitor holds no registry lock while it probes under ptys).
            let child_at_stop = match instance.child.as_mut() {
                Some(child) => probe_child_contained(child),
                None => ChildLiveness::Gone,
            };
            spawn_diagnostics::mark_ac_stop(*id, "app-shutdown", Some(child_at_stop));
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
    record: Option<Arc<SpawnRecord>>,
) {
    std::thread::spawn(move || match child.wait() {
        Ok(status) => {
            // #942 - the exit AC asked for. The monitor may have reported this child
            // first (it died a hair before our stop); then the event is already on
            // record with the right cause and we only leave a reap crumb.
            let liveness = ChildLiveness::from(&status);
            match record.as_ref() {
                Some(record) => {
                    let cause = record.attribute_exit(record.stop_snapshot());
                    if !record.log_child_exit(cause, &liveness, "reaped-after-stop") {
                        log::debug!(
                            "[pty] Reaped session {} child pid {:?}: {:?} (exit already reported)",
                            session_id,
                            pid,
                            status
                        );
                    }
                }
                None => log::info!(
                    "[pty] child-exit session={} pid={:?} cause=ac-requested detail=reaped-after-stop child={}",
                    session_id,
                    pid,
                    liveness.as_log()
                ),
            }
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
mod probe_containment_tests {
    use super::*;
    use std::collections::HashMap;

    /// A child that panics exactly where portable-pty does when its inner mutex is
    /// poisoned: inside the accessor, with the caller holding the PTY map guard.
    #[derive(Debug)]
    struct PoisonedChild;

    impl portable_pty::ChildKiller for PoisonedChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(PoisonedChild)
        }
    }

    impl portable_pty::Child for PoisonedChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }

        fn process_id(&self) -> Option<u32> {
            None
        }

        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            panic!("called `Result::unwrap()` on a poisoned mutex");
        }
    }

    /// grinch D4: the probe runs while the global `ptys` guard is held. A panic inside
    /// portable-pty must not unwind through that guard, because a poisoned `ptys` map
    /// makes the next terminal write panic, with nothing in the log tying it back here.
    #[test]
    fn a_panicking_child_probe_never_poisons_the_pty_map() {
        let ptys: Mutex<HashMap<Uuid, Box<dyn portable_pty::Child + Send + Sync>>> =
            Mutex::new(HashMap::new());
        let id = Uuid::new_v4();
        ptys.lock()
            .unwrap()
            .insert(id, Box::new(PoisonedChild) as Box<dyn portable_pty::Child + Send + Sync>);

        {
            // Exactly the shape of `probe_child`: guard held, probe called under it.
            let mut guard = ptys.lock().unwrap();
            let child = guard.get_mut(&id).expect("child");
            let liveness = probe_child_contained(child);
            assert!(
                matches!(liveness, ChildLiveness::Unqueryable(_)),
                "a probe that could not ask must never claim the child is alive"
            );
        }

        assert!(
            !ptys.is_poisoned(),
            "the ptys guard must be released normally, not unwound through"
        );
        assert!(
            ptys.lock().is_ok(),
            "the next terminal write still gets the lock"
        );
    }
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

#[cfg(test)]
mod resize_dedup_tests {
    use super::PtyInstance;
    use std::sync::Mutex;

    /// A `PtyInstance` carries real ConPTY handles, so the tests below drive the one piece
    /// that #973 (C) actually changes: the size cache and the decision it drives.
    fn cache(cols: u16, rows: u16) -> Mutex<(u16, u16)> {
        Mutex::new((cols, rows))
    }

    /// #973 (C) - the frontend calls resize unconditionally from a double
    /// `requestAnimationFrame` and a `ResizeObserver`, so a single attach fires 5-20
    /// identical resizes and ConPTY hands every one of them to the child as a real event.
    /// A resize that changes nothing must not reach the child.
    #[test]
    fn a_resize_to_the_size_it_already_has_is_not_sent_to_the_child() {
        let size = cache(74, 23);
        assert!(
            !PtyInstance::size_changed_in(&size, 74, 23),
            "an identical resize must be skipped: today AC fires 5-20 of these per attach"
        );
    }

    /// The other half: a real resize must still get through, or the terminal would never
    /// follow the window.
    #[test]
    fn a_resize_that_moves_the_size_is_sent() {
        let size = cache(74, 23);
        assert!(PtyInstance::size_changed_in(&size, 74, 24), "one row is a real resize");
        assert!(PtyInstance::size_changed_in(&size, 120, 30), "a real resize");
    }

    /// The trap in the dedup: if the cache were updated BEFORE the ConPTY accepted the new
    /// size, a failed resize would leave the cache claiming a size the PTY never took, and
    /// every retry would then be skipped as a no-op - the terminal would be wedged at the
    /// wrong size forever. The cache is only written after `master.resize()` returns Ok.
    #[test]
    fn a_failed_resize_does_not_poison_the_cache_and_the_retry_still_fires() {
        let size = cache(74, 23);
        // resize to 100x40 "fails": remember_size is never reached, so the cache stays put
        assert!(PtyInstance::size_changed_in(&size, 100, 40));
        assert!(
            PtyInstance::size_changed_in(&size, 100, 40),
            "after a failed resize the retry must still be issued, not skipped as a no-op"
        );
        // now it succeeds
        PtyInstance::remember_size_in(&size, 100, 40);
        assert!(!PtyInstance::size_changed_in(&size, 100, 40));
    }
}

#[cfg(test)]
mod startup_gate_tests {
    use super::{resize_instance, send_size_to_conpty, PtyInstance, StartupGate};
    use portable_pty::{native_pty_system, MasterPty, PtySize};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    /// A real ConPTY, with no child. `LocalProcessBackend` itself cannot be built here (its
    /// `GitWatcher` needs a Tauri `AppHandle`), and it does not need to be: `PtyBackend::resize`
    /// is a four-line wrapper - lock, delegate to `resize_instance`, broadcast - and
    /// `resize_instance` is the whole of the decision.
    fn conpty(cols: u16, rows: u16) -> (PtyInstance, Arc<Mutex<Box<dyn MasterPty + Send>>>) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        drop(pair.slave);

        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
        let instance = PtyInstance {
            master: Arc::clone(&master),
            writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
            child: None,
            job: None,
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::new(AtomicBool::new(false)),
        };
        (instance, master)
    }

    /// What the CHILD would see. This is the point of these tests: not what AC believes.
    fn size_the_child_sees(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>) -> (u16, u16) {
        let s = master.lock().unwrap().get_size().expect("get_size");
        (s.cols, s.rows)
    }

    /// #973 - THE COLD-START CASE. This is the one the user hits every morning.
    ///
    /// `TerminalView` lives inside `<Show when={activeSessionId}>`, so when the app opens
    /// with no active session there is no terminal host to measure, and the frontend cannot
    /// supply a size. The PTY is opened at 120x30, the view then mounts, fits, and fires its
    /// resize burst 300-500 ms later - straight into the startup window of the first coding
    /// agent the user launches. **Option A cannot cover this. This is what covers it.**
    ///
    /// Red without the gate: the resize reaches the ConPTY immediately, and the child is
    /// resized while it is still coming up, which is what costs Codex its first content
    /// render (measured: 8 blanks in 10).
    #[test]
    fn a_session_opened_without_a_view_size_holds_the_burst_until_the_child_renders() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        // the view mounts and fits: 6 identical resizes inside a few milliseconds
        for _ in 0..6 {
            let sent = resize_instance(&instance, id, 74, 23).expect("resize");
            assert!(!sent, "nothing may reach a child that has not rendered yet");
        }

        assert_eq!(
            size_the_child_sees(&master),
            (120, 30),
            "the child has rendered nothing yet, so the ConPTY must NOT have been resized"
        );

        // the child paints
        let held = instance
            .startup_gate
            .lock()
            .unwrap()
            .open()
            .expect("the size the view asked for must have been kept");
        send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

        assert_eq!(
            size_the_child_sees(&master),
            (74, 23),
            "once the child has rendered, the size it should have had must be applied"
        );
    }

    /// The view fires 5-20 resizes per attach. Only the last is real; replaying all of them
    /// at the child would be churn for nothing.
    #[test]
    fn only_the_last_held_size_reaches_the_child() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        resize_instance(&instance, id, 74, 23).expect("resize");
        resize_instance(&instance, id, 90, 40).expect("resize");
        resize_instance(&instance, id, 74, 24).expect("resize");
        assert_eq!(size_the_child_sees(&master), (120, 30), "all of them held");

        let held = instance.startup_gate.lock().unwrap().open().expect("held");
        send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

        assert_eq!(
            size_the_child_sees(&master),
            (74, 24),
            "only the last size the view asked for should reach the child"
        );
    }

    /// Once the child is up, AC gets out of the way: every later resize - the user dragging
    /// the window - goes straight through, forever. A gate that never reopened would leave
    /// the terminal unable to follow its window.
    #[test]
    fn once_the_child_has_rendered_resizes_go_straight_through() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        instance.startup_gate.lock().unwrap().open(); // the child rendered, nothing held

        assert!(resize_instance(&instance, id, 74, 23).expect("resize"));
        assert_eq!(size_the_child_sees(&master), (74, 23));

        assert!(resize_instance(&instance, id, 100, 50).expect("resize"));
        assert_eq!(
            size_the_child_sees(&master),
            (100, 50),
            "the gate must not close again"
        );
    }

    /// #973 (C) - a resize that changes nothing must not be sent. ConPTY hands even a
    /// same-size resize to the child as a real event, and the view fires 5-20 of them.
    #[test]
    fn a_resize_that_changes_nothing_is_not_sent() {
        let (instance, id) = (conpty(74, 23).0, Uuid::new_v4());
        instance.startup_gate.lock().unwrap().open();

        assert!(
            !resize_instance(&instance, id, 74, 23).expect("resize"),
            "the ConPTY is already 74x23: nothing should be sent"
        );
        assert!(
            resize_instance(&instance, id, 74, 24).expect("resize"),
            "one row is a real resize and must be sent"
        );
    }

    /// #973 - a degenerate resize must be refused, and it must not be cached.
    ///
    /// This test caught a real one. I had assumed portable-pty would reject `0x0`. **It does
    /// not**: `master.resize(0x0)` returns Ok and the ConPTY is genuinely set to zero, so
    /// before the guard this test failed with `left: (0, 0)`. `pty_resize` passes the view's
    /// numbers straight down, and xterm's `fit()` returns a zero dimension while its
    /// container is still being laid out - the exact hazard `PtyViewport::from_fit` guards
    /// at spawn. The resize path had no such guard.
    ///
    /// The cache matters as much as the refusal: if a size the ConPTY never took were
    /// cached, every retry would be skipped as a no-op and the terminal would be wedged at
    /// the wrong size forever.
    #[test]
    fn a_degenerate_resize_is_refused_and_does_not_poison_the_cache() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();
        instance.startup_gate.lock().unwrap().open();

        assert!(
            !send_size_to_conpty(&instance, id, 0, 0).expect("must not error"),
            "a zero dimension must never be sent to the ConPTY"
        );
        assert_eq!(
            size_the_child_sees(&master),
            (120, 30),
            "ConPTY accepts a 0x0 resize without complaint, so AC has to refuse it"
        );
        assert_eq!(
            *instance.size.lock().unwrap(),
            (120, 30),
            "and it must not be cached, or the next real resize is skipped as a no-op"
        );

        assert!(
            send_size_to_conpty(&instance, id, 74, 23).expect("resize"),
            "a real resize after a refused one must still go through"
        );
        assert_eq!(size_the_child_sees(&master), (74, 23));
    }

    /// The gate is a two-state machine. Pin it directly, including the ordering trap: the
    /// hand-over happens exactly once, and after it the gate is open for good.
    #[test]
    fn the_gate_hands_over_exactly_once() {
        let mut gate = StartupGate::Holding(None);
        assert_eq!(gate.on_resize(74, 23), None, "held");
        assert_eq!(gate.on_resize(74, 24), None, "held, replacing the first");
        assert_eq!(gate.open(), Some((74, 24)), "the last held size, handed over once");
        assert_eq!(gate.open(), None, "a second open hands over nothing");
        assert_eq!(
            gate.on_resize(80, 25),
            Some((80, 25)),
            "the gate is open now: resizes go straight through"
        );
    }
}
