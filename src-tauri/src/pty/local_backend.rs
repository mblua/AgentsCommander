use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
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
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, ResolvedAgentHostShell};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::pty::spawn_diagnostics::{self, ChildLiveness, ExitCause, SpawnRecord, SpawnRecordInit};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
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
/// **That argument only holds if the trigger actually tests rendering, so it does.** It asks the
/// real, stateful vt100 parser - the one `handle_output` already feeds every byte of every chunk -
/// whether the screen now holds a cell a human could see:
/// `SessionIoFanout::has_rendered_visible_content`.
///
/// It is worth saying what it must NOT be, because the first version of this gate got it wrong and
/// the mistake was invisible. It used the idle detector's text predicate
/// (`output_has_printable_activity`), which asks whether a printable byte survives a STATELESS,
/// per-chunk escape stripper. That is a different question, and it answers this one wrong twice:
/// a chunk that ends mid-CSI or mid-OSC hands the tail to the next call with no leading `ESC`
/// (`1049h`, `2J` and `cmd.exe` are all printable, and conhost really does split its writes), and
/// a three-byte charset designator like `ESC ( B` - which ncurses and half the TUI world emit on
/// the way up - outran a stripper that consumed `ESC` plus one char. Either one opens the gate on
/// a child that has painted nothing, which is precisely the bug the gate exists to prevent, and
/// it would have failed silently and intermittently inside an already intermittent bug. A parser
/// carries state across reads and knows what an escape is, so both are closed by construction
/// rather than by another special case.
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
    // #973 - refuse a degenerate size FIRST, ahead of the gate.
    //
    // The gate is last-wins (`StartupGate::on_resize`), so a `0x0` reaching it OVERWRITES the
    // real size the view is waiting to give the child. The hand-over then pops the `0x0`,
    // `send_size_to_conpty` refuses it, the pending slot is consumed, and nothing retries: the
    // ConPTY is wedged at the size it was opened at for good. On cold start that is a 120x30
    // child behind a 74x23 terminal. A size that must never reach the child must never be
    // allowed to displace one that must.
    //
    // Where a zero comes from, since we got this wrong once: NOT from xterm. `fit()` clamps to
    // its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1 (`@xterm/addon-fit`), and `CoreTerminal.resize`
    // rejects NaN and clamps again - the worst it can produce is NaN, which the frontend's
    // `Number.isInteger` check catches. The zero comes off the WIRE: `pty_resize` on the web
    // transport takes cols/rows straight from a JSON payload (`web/commands.rs`), and a client
    // that is not xterm, or is simply broken, can put a 0 in it.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

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
    // #973 - refuse a degenerate size. Defence in depth: the request path is guarded ahead of
    // the gate in `resize_instance`, and this is the last line before `master.resize()`, which
    // is also what `hand_over_held_size` calls with a size that has been sitting in the gate.
    //
    // It has to be refused SOMEWHERE, because ConPTY does not refuse it: `master.resize(0x0)`
    // returns Ok and the child is left with no screen at all. A stale size is recoverable; a
    // zero-column terminal is not. The zero itself comes off the wire, not from xterm - see
    // `resize_instance`.
    if cols == 0 || rows == 0 {
        log::warn!("[pty] refusing degenerate resize {id} to {cols}x{rows} (#973)");
        return Ok(false);
    }

    if !instance.size_changed(cols, rows) {
        log::debug!("[pty] resize {id} skipped: already {cols}x{rows} (#973)");
        return Ok(false);
    }

    {
        let master = instance.master.lock().unwrap_or_else(|e| e.into_inner());
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

/// #973 (B) - the child has rendered something, so the startup window is behind us. Open the
/// gate for good and hand the ConPTY the size the view has been waiting to give it.
///
/// Returns the size the ConPTY ACTUALLY took, so the caller can bring the vt100 screen along
/// with it - and `None` if nothing was held, or the held size was refused, or it failed.
///
/// Free over the instance, like `resize_instance`, so the hand-over can be driven by a test
/// against a real ConPTY: `open_startup_gate`, its only caller, needs a `LocalProcessBackend`,
/// whose `GitWatcher` needs a Tauri `AppHandle`. A test that re-implemented these lines rather
/// than calling them would not be testing this code.
fn hand_over_held_size(instance: &PtyInstance, id: Uuid) -> Option<(u16, u16)> {
    // One lock covers both "take the held size" and "open the gate", so a resize landing at
    // this exact instant cannot be recorded into a gate that is already open and then dropped
    // on the floor, leaving the terminal stuck at the wrong size.
    let pending = {
        let mut gate = instance
            .startup_gate
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        gate.open()
    };
    // Only a fast path for the read loop; the gate above is the truth. Publishing it late is
    // safe: a stale `false` costs one extra no-op call to this function.
    instance.rendered.store(true, Ordering::Relaxed);

    let (cols, rows) = pending?;

    match send_size_to_conpty(instance, id, cols, rows) {
        Ok(true) => {
            log::info!(
                "[pty] startup gate open for {id}: applied the held resize {cols}x{rows} (#973)"
            );
            Some((cols, rows))
        }
        Ok(false) => None,
        Err(e) => {
            // Non-critical: the child is up and painting, it is just the wrong size.
            log::warn!("[pty] held resize {id} to {cols}x{rows} failed: {e} (#973)");
            None
        }
    }
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
    /// #1271 - per-instance, invocation-scoped pre-PTY attempt observer, test
    /// builds only. Incremented inside the real `spawn_sync` immediately before
    /// `native_pty_system()`, so a deterministic invalid-input rejection (which
    /// happens at the TOP of `spawn_sync`, before any spawn accounting) leaves
    /// it at zero. Each backend-level test constructs its own instance and reads
    /// its own observer; there is no global to reset and no cross-test race.
    #[cfg(test)]
    pre_pty_attempts: Arc<AtomicUsize>,
}

fn write_to_local_pty(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    id: Uuid,
    data: &[u8],
) -> Result<(), AppError> {
    let writer = {
        let ptys = ptys.lock().unwrap_or_else(|error| error.into_inner());
        let instance = ptys
            .get(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        Arc::clone(&instance.writer)
    };

    // The global PTY map is released before a potentially blocking pipe
    // write. Teardown can remove and terminate the child to unblock it.
    let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
    writer
        .write_all(data)
        .map_err(|error| AppError::PtyError(error.to_string()))?;
    writer
        .flush()
        .map_err(|error| AppError::PtyError(error.to_string()))
}

fn remove_local_pty(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> Option<PtyInstance> {
    ptys.lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id)
}

// ---------------------------------------------------------------------------
// #1271 - Windows host-shell adapter for resolved agent commands.
//
// Four supported protocol classes, selected from the lower-cased final path
// component of the configured default-shell program: Windows PowerShell
// (`powershell`/`powershell.exe`), PowerShell 7+ (`pwsh`/`pwsh.exe`), Command
// Prompt (`cmd`/`cmd.exe`), and any other basename as an explicitly declared
// POSIX-compatible custom shell. The exact configured program string is ALWAYS
// the launched program; a hard-coded `cmd.exe` is never substituted.
//
// PowerShell uses a managed `ProcessStartInfo` child instead of native `&` or
// `--%` because ordinary native invocation can alter empty values and embedded
// quotes, and `--%` is a physical-line parser hazard that would destroy the
// logical-argv boundary. The generated script resolves the agent as an external
// command only (never an alias/function/cmdlet) and starts a native
// `Application` with a standard Windows-encoded `Arguments` property; a
// resolved `.cmd`/`.bat` target runs through one explicit system-cmd child
// (`/D /V:OFF /S /C`) because a batch file is not a native argv consumer.
//
// cmd runs with delayed expansion disabled (`/V:OFF`) so `!` is literal data.
// All validation happens before any PTY is acquired; the configured host's own
// parser never sees a token the closed grammar (Section 4.3) did not classify.
// ---------------------------------------------------------------------------

/// One adapted launch: the exact program and argument vector handed to
/// `CommandBuilder`, which is also the `exec_argv` diagnostic provenance. Built
/// from ONE adapter decision so the two representations can never desynchronize.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedLaunch {
    program: String,
    args: Vec<String>,
}

impl PreparedLaunch {
    fn command_builder(&self) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        for arg in &self.args {
            command.arg(arg);
        }
        command
    }

    fn exec_argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// The four #1271 Windows host-shell protocol classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsHostShellKind {
    PowerShell,
    Pwsh,
    Cmd,
    Posix,
}

/// True only for a command whose text ends in `.exe` or whose path extension
/// equals `.exe`, case-insensitively. Direct `.exe` agents never enter the
/// adapter; everything else on Windows is a candidate.
fn is_direct_exe(command: &str) -> bool {
    command.to_lowercase().ends_with(".exe")
        || std::path::Path::new(command)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// Selects the adapter protocol from the lower-cased final Windows path
/// component of the configured program, accepting both extensionless and `.exe`
/// spellings. A custom path ending in a recognized name uses that adapter.
fn configured_shell_kind(program: &str) -> WindowsHostShellKind {
    let basename = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    match basename.as_str() {
        "powershell" | "powershell.exe" => WindowsHostShellKind::PowerShell,
        "pwsh" | "pwsh.exe" => WindowsHostShellKind::Pwsh,
        "cmd" | "cmd.exe" => WindowsHostShellKind::Cmd,
        _ => WindowsHostShellKind::Posix,
    }
}

/// Standard #1271 error shape: names the configured default-shell program, the
/// offending token, and the rejection category, and states that the agent
/// adapter owns command execution (Section 4.3.1 item 6).
fn adapter_error(host_program: &str, token: &str, category: &str) -> AppError {
    AppError::Other(format!(
        "Configured default shell '{}' cannot host a resolved agent: token '{}' is {}; \
         the agent adapter owns command execution for this session",
        host_program, token, category
    ))
}

/// Section 4.3 - the common rule, applied BEFORE adapter selection: NUL, CR,
/// and LF in the configured program, every configured argument, and every
/// logical agent argv element, plus the blank-program rejection.
fn validate_common_adapter_input(
    command: &str,
    args: &[String],
    host_shell: &ResolvedAgentHostShell,
) -> Result<(), AppError> {
    if host_shell.program.trim().is_empty() {
        return Err(adapter_error(
            &host_shell.program,
            "(program)",
            "a blank configured default-shell program",
        ));
    }
    for (value, label) in [
        (host_shell.program.as_str(), "configured default-shell program"),
        (command, "logical agent program"),
    ] {
        if value.contains('\0') || value.contains('\r') || value.contains('\n') {
            return Err(adapter_error(
                &host_shell.program,
                value,
                &format!("the {label} contains a forbidden line separator (NUL, CR, or LF)"),
            ));
        }
    }
    for argument in host_shell.args.iter().chain(args.iter()) {
        if argument.contains('\0') || argument.contains('\r') || argument.contains('\n') {
            return Err(adapter_error(
                &host_shell.program,
                argument,
                "contains a forbidden line separator (NUL, CR, or LF)",
            ));
        }
    }
    Ok(())
}

/// Duplicate detection shared by every closed grammar: a permitted option may
/// appear at most once, because host parsers disagree on duplicate handling
/// across versions and no configured duplicate is ever needed.
fn mark_seen(
    seen: &mut Vec<String>,
    option: &str,
    host_program: &str,
    token: &str,
) -> Result<(), AppError> {
    if seen.iter().any(|s| s.eq_ignore_ascii_case(option)) {
        return Err(adapter_error(host_program, token, "a duplicate configured option"));
    }
    seen.push(option.to_string());
    Ok(())
}

// --- Section 4.3.2 - PowerShell products -----------------------------------

const POWERSHELL_FLAG_OPTIONS: &[&str] =
    &["NoLogo", "NoProfile", "NonInteractive", "Sta", "Mta"];
const POWERSHELL_VALUE_OPTIONS: &[&str] = &[
    "ExecutionPolicy",
    "WindowStyle",
    "InputFormat",
    "OutputFormat",
    "PSConsoleFile",
];
const PWSH_FLAG_OPTIONS: &[&str] = &["NoLogo", "NoProfile", "NonInteractive"];
const PWSH_VALUE_OPTIONS: &[&str] = &[
    "ExecutionPolicy",
    "InputFormat",
    "OutputFormat",
    "WorkingDirectory",
    "ConfigurationName",
    "SettingsFile",
];
/// Command-ownership, keep-open, terminate-early, server/host-mode, and parser
/// terminators, rejected conservatively for BOTH PowerShell classes. Each
/// spelling may appear with `-` or `/`; `--` and `--%` are literal tokens.
const POWERSHELL_CONFLICTING_OPTIONS: &[&str] = &[
    "Command", "c", "CommandWithArgs", "cwa", "File", "f", "EncodedCommand", "enc",
    "e", "NoExit", "noe", "Version", "v", "Help", "h", "?", "SSHServerMode",
    "ServerMode", "WindowsPowerShell", "Login", "Interactive",
];
/// powershell.exe-only spellings that pwsh does not define; rejected for pwsh
/// with the unknown/ambiguous category.
const PWSH_UNKNOWN_OPTIONS: &[&str] = &["Sta", "Mta", "WindowStyle", "PSConsoleFile"];

/// Section 4.3.1 - the closed configured-host grammar for PowerShell products.
/// Exact, case-insensitive spelling matching only; every permitted value-bearing
/// option binds its operand attached (`-Name:value` / `-Name=value`) or as the
/// single immediately following non-option token.
fn validate_powershell_arguments(
    host_program: &str,
    args: &[String],
    is_pwsh: bool,
) -> Result<(), AppError> {
    let flags = if is_pwsh {
        PWSH_FLAG_OPTIONS
    } else {
        POWERSHELL_FLAG_OPTIONS
    };
    let values = if is_pwsh {
        PWSH_VALUE_OPTIONS
    } else {
        POWERSHELL_VALUE_OPTIONS
    };
    let mut seen: Vec<String> = Vec::new();
    let mut pending_operand: Option<String> = None;
    for token in args {
        if let Some(option) = pending_operand.take() {
            if token.starts_with('-') || token.starts_with('/') {
                return Err(adapter_error(
                    host_program,
                    token,
                    "an option-shaped separated operand (value-bearing option has no bound value)",
                ));
            }
            mark_seen(&mut seen, &option, host_program, token)?;
            continue;
        }
        if token == "--" || token == "--%" {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if !token.starts_with('-') && !token.starts_with('/') {
            return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
        }
        let body = &token[1..];
        // Conflicting/terminal spellings are matched against the FULL body before
        // any operand split: `-E:O` is not the `-e` spelling, so an attached
        // operand can never turn an unknown token into a terminal one.
        if POWERSHELL_CONFLICTING_OPTIONS
            .iter()
            .any(|option| option.eq_ignore_ascii_case(body))
        {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        let (name, attached) = match body.find([':', '=']) {
            Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
            None => (body, None),
        };
        if let Some(flag) = flags.iter().find(|option| option.eq_ignore_ascii_case(name)) {
            if attached.is_some() {
                return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
            }
            mark_seen(&mut seen, flag, host_program, token)?;
            continue;
        }
        if let Some(option) = values.iter().find(|option| option.eq_ignore_ascii_case(name)) {
            match attached {
                Some(value) if !value.is_empty() => {
                    mark_seen(&mut seen, option, host_program, token)?;
                }
                Some(_) => {
                    return Err(adapter_error(
                        host_program,
                        token,
                        "a missing or option-shaped operand",
                    ));
                }
                None => {
                    pending_operand = Some(option.to_string());
                }
            }
            continue;
        }
        if is_pwsh
            && PWSH_UNKNOWN_OPTIONS
                .iter()
                .any(|option| option.eq_ignore_ascii_case(name))
        {
            return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
        }
        return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
    }
    if let Some(option) = pending_operand {
        return Err(adapter_error(
            host_program,
            &option,
            "a missing or option-shaped operand (value-bearing option has no operand)",
        ));
    }
    Ok(())
}

// --- Section 4.3.3 - cmd(.exe) ---------------------------------------------

const CMD_FLAG_OPTIONS: &[&str] = &["A", "U", "Q", "D", "E:ON", "E:OFF", "F:ON", "F:OFF"];

/// Section 4.3.3 - closed configured-host grammar for cmd: `/`-prefixed exact
/// spellings only, `/T:<value>` as the sole attached-value form, no separated
/// operands, no `-` prefix.
fn validate_cmd_arguments(host_program: &str, args: &[String]) -> Result<(), AppError> {
    let mut seen: Vec<String> = Vec::new();
    for token in args {
        if !token.starts_with('/') {
            return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
        }
        let body = &token[1..];
        if body.eq_ignore_ascii_case("C")
            || body.eq_ignore_ascii_case("K")
            || body.eq_ignore_ascii_case("S")
            || body.eq_ignore_ascii_case("?")
        {
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if body.eq_ignore_ascii_case("V:OFF") {
            mark_seen(&mut seen, "V", host_program, token)?;
            continue;
        }
        if body.to_ascii_uppercase().starts_with("V:") {
            // /V:ON and any other /V: value change `!` expansion semantics.
            return Err(adapter_error(
                host_program,
                token,
                "a conflicting/terminal configured option",
            ));
        }
        if body.to_ascii_uppercase().starts_with("T:") {
            let value = &body[2..];
            if value.is_empty() {
                return Err(adapter_error(
                    host_program,
                    token,
                    "a missing or option-shaped operand",
                ));
            }
            mark_seen(&mut seen, "T", host_program, token)?;
            continue;
        }
        if let Some(flag) = CMD_FLAG_OPTIONS
            .iter()
            .find(|option| option.eq_ignore_ascii_case(body))
        {
            let canonical = flag.split(':').next().unwrap_or(flag);
            mark_seen(&mut seen, canonical, host_program, token)?;
            continue;
        }
        return Err(adapter_error(host_program, token, "an unknown or ambiguous token"));
    }
    Ok(())
}
// --- Section 4.5 - cmd payload domain --------------------------------------

/// Section 4.5 - the configured-cmd payload is the logical program plus args
/// joined with single ASCII spaces and NO per-token quoting: portable-pty's
/// `append_quoted` re-encodes every `CommandBuilder` argument with C-runtime
/// rules and cmd re-parses that string with its own grammar, so per-token
/// quoting cannot survive the round trip (plan Finding A). The accepted domain
/// is exactly the set for which `append_quoted` is inert apart from the outer
/// pair; every other form is rejected before PTY acquisition.
fn cmd_payload(command: &str, args: &[String]) -> String {
    let mut payload = command.to_string();
    for arg in args {
        payload.push(' ');
        payload.push_str(arg);
    }
    payload
}

fn validate_cmd_payload(
    host_program: &str,
    command: &str,
    args: &[String],
) -> Result<(), AppError> {
    if command.starts_with('@') {
        return Err(adapter_error(
            host_program,
            command,
            "an unsupported cmd payload character (program token starts with '@')",
        ));
    }
    for token in std::iter::once(command).chain(args.iter().map(|s| s.as_str())) {
        if token.is_empty() {
            return Err(adapter_error(
                host_program,
                "(empty)",
                "an unsupported cmd payload character (empty token)",
            ));
        }
        if token.contains('%') {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character ('%' expands in cmd)",
            ));
        }
        if token.chars().any(|c| c.is_whitespace()) {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character (whitespace splits tokens in cmd)",
            ));
        }
        if token.contains('"')
            || token.contains('&')
            || token.contains('|')
            || token.contains('<')
            || token.contains('>')
            || token.contains('(')
            || token.contains(')')
            || token.contains('^')
            || token.contains('=')
            || token.contains(',')
            || token.contains(';')
        {
            return Err(adapter_error(
                host_program,
                token,
                "an unsupported cmd payload character (cmd re-parses it as syntax)",
            ));
        }
    }
    if cmd_payload(command, args).ends_with('\\') {
        return Err(adapter_error(
            host_program,
            "(payload)",
            "an unsupported cmd payload character (payload-final backslash is doubled by append_quoted)",
        ));
    }
    Ok(())
}

// --- Section 4.4 - PowerShell/pwsh protocol ---------------------------------

/// Standard Windows command-line-to-argv quote/backslash encoding (Section 4.5):
/// outer double quotes; `2n+1` backslashes before an embedded double quote; a
/// run unchanged before any other character; `2*trailing_run` backslashes
/// before the closing outer quote. An empty value encodes as `""`.
fn windows_arg(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let mut run = 0;
        while i < chars.len() && chars[i] == '\\' {
            run += 1;
            i += 1;
        }
        if i < chars.len() {
            if chars[i] == '"' {
                out.extend(std::iter::repeat_n('\\', 2 * run + 1));
                out.push('"');
            } else {
                out.extend(std::iter::repeat_n('\\', run));
                out.push(chars[i]);
            }
            i += 1;
        } else {
            out.extend(std::iter::repeat_n('\\', 2 * run));
        }
    }
    out.push('"');
    out
}

/// Section 4.4 - the native `Application` branch's `ProcessStartInfo.Arguments`
/// string: logical arguments ONLY (never the resolved program), each
/// `windows_arg` encoded, joined with one ASCII space. Empty vector -> empty
/// string; one empty argument -> `""`.
fn windows_raw_argv(args: &[String]) -> String {
    args.iter()
        .map(|arg| windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// PowerShell single-quoted literal: wrap in single quotes, double every
/// embedded single quote. Always ends with `'`, so the generated script can
/// never end with a backslash (the only generator invariant, plan Finding B).
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// POSIX single-quoted literal: wrap in single quotes, replace an embedded
/// single quote with the exact fragment `'"'"'`.
fn posix_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// #1271 - the resolved batch child is launched through one explicit system-cmd
/// child (`/D /V:OFF /S /C`). Its payload tokens are `'"' + <raw value> + '"'`:
/// cmd's batch tokenizer is quote-aware but NOT backslash-aware (probe-proven),
/// so the C-runtime `windows_arg` trailing-run doubling would corrupt a value
/// ending in a backslash. Values containing `%` or `"` are already rejected
/// before this branch runs, so the raw quote wrapper is safe for the whole
/// batch domain.
/// Section 4.4 - the generated `-Command` script. Resolves the logical program
/// as an external command only (two-argument `GetCommand` with the
/// `Application | ExternalScript` filter, never a profile alias/function),
/// starts a native `Application` through `ProcessStartInfo` with separate
/// `FileName` and a standard Windows-encoded `Arguments` literal, runs a
/// resolved `.cmd`/`.bat` through one explicit system-cmd child, and keeps
/// literal invocation only for an `ExternalScript`. Contains no `--%` token and
/// never ends with a backslash (the trailing-run invariant).
fn powershell_script(command: &str, args: &[String]) -> String {
    let batch_unsupported = args.iter().any(|arg| arg.contains('%') || arg.contains('"'));
    let mut script = String::new();
    script.push_str("$global:LASTEXITCODE = $null;\n");
    script.push_str("$ac_batch_unsupported_logical_arg = $");
    script.push_str(if batch_unsupported { "true" } else { "false" });
    script.push_str(";\n");
    script.push_str(
        "$ac_kind = [System.Management.Automation.CommandTypes]::Application -bor \
         [System.Management.Automation.CommandTypes]::ExternalScript;\n",
    );
    script.push_str("$ac_command = $ExecutionContext.InvokeCommand.GetCommand(");
    script.push_str(&ps_literal(command));
    script.push_str(", $ac_kind);\n");
    script.push_str("if ($null -eq $ac_command) { exit 1 };\n");
    script.push_str(
        "if ($ac_command.CommandType -eq \
         [System.Management.Automation.CommandTypes]::Application) {\n",
    );
    script.push_str(
        "  if (([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.cmd') -or \
         ([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.bat')) {\n",
    );
    script.push_str(
        "    if ($ac_batch_unsupported_logical_arg -or $ac_command.Path.Contains('%') -or \
         $ac_command.Path.Contains([char]34)) { exit 1 };\n",
    );
    script.push_str("    $ac_batch_payload = '\"' + $ac_command.Path + '\"'");
    for arg in args {
        script.push_str(" + ' ' + '\"' + ");
        script.push_str(&ps_literal(arg));
        script.push_str(" + '\"'");
    }
    script.push_str(";\n");
    script.push_str("    $ac_start = New-Object System.Diagnostics.ProcessStartInfo;\n");
    script.push_str("    $ac_start.UseShellExecute = $false;\n");
    script.push_str("    $ac_start.RedirectStandardInput = $false;\n");
    script.push_str("    $ac_start.RedirectStandardOutput = $false;\n");
    script.push_str("    $ac_start.RedirectStandardError = $false;\n");
    script.push_str(
        "    $ac_start.FileName = \
         [System.IO.Path]::Combine([System.Environment]::SystemDirectory, 'cmd.exe');\n",
    );
    script.push_str("    $ac_start.Arguments = '/D /V:OFF /S /C \"' + $ac_batch_payload + '\"';\n");
    script.push_str("    try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } catch { exit 1 };\n");
    script.push_str("    if ($null -eq $ac_process) { exit 1 };\n");
    script.push_str("    $ac_process.WaitForExit();\n");
    script.push_str("    exit $ac_process.ExitCode\n");
    script.push_str("  }\n");
    script.push_str("  $ac_start = New-Object System.Diagnostics.ProcessStartInfo;\n");
    script.push_str("  $ac_start.UseShellExecute = $false;\n");
    script.push_str("  $ac_start.RedirectStandardInput = $false;\n");
    script.push_str("  $ac_start.RedirectStandardOutput = $false;\n");
    script.push_str("  $ac_start.RedirectStandardError = $false;\n");
    script.push_str("  $ac_start.FileName = $ac_command.Path;\n");
    script.push_str("  $ac_start.Arguments = ");
    script.push_str(&ps_literal(&windows_raw_argv(args)));
    script.push_str(";\n");
    script.push_str("  try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } catch { exit 1 };\n");
    script.push_str("  if ($null -eq $ac_process) { exit 1 };\n");
    script.push_str("  $ac_process.WaitForExit();\n");
    script.push_str("  exit $ac_process.ExitCode\n");
    script.push_str("}\n");
    script.push_str("& $ac_command.Path");
    for arg in args {
        script.push(' ');
        script.push_str(&ps_literal(arg));
    }
    script.push_str(";\n");
    script.push_str("$ac_succeeded = $?; $ac_exit_code = $LASTEXITCODE;\n");
    script.push_str("if ($null -ne $ac_exit_code) { exit $ac_exit_code };\n");
    script.push_str("if ($ac_succeeded) { exit 0 }; exit 1");
    script
}

// --- Section 4.6 - custom POSIX-compatible shell ----------------------------

/// Section 4.6 - one `exec` command, so the custom shell replaces itself with
/// the agent and the agent owns the PTY child exit code directly.
fn posix_script(command: &str, args: &[String]) -> String {
    let mut script = String::from("exec ");
    script.push_str(&posix_literal(command));
    for arg in args {
        script.push(' ');
        script.push_str(&posix_literal(arg));
    }
    script
}

// --- Section 4.1/4.4/4.5 - entry points ------------------------------------

/// Section 4.4 - a configured logical program that explicitly ends in `.cmd` or
/// `.bat` rejects `%` or `"` in ANY logical argument in Rust, before any PTY is
/// acquired. A bare name that resolves to batch only at runtime is handled by
/// the generated script instead (post-PTY, Section 4.4).
fn validate_explicit_batch_args(
    host_program: &str,
    command: &str,
    args: &[String],
) -> Result<(), AppError> {
    let lower = command.to_ascii_lowercase();
    let explicit_batch = lower.ends_with(".cmd") || lower.ends_with(".bat");
    if explicit_batch
        && args
            .iter()
            .any(|arg| arg.contains('%') || arg.contains('"'))
    {
        return Err(adapter_error(
            host_program,
            "(batch target)",
            "an unsupported explicit-batch payload character ('%' or '\"' in a logical argument)",
        ));
    }
    Ok(())
}

/// The single private adapter seam (Section 4.1.4): logical agent argv plus the
/// optional host-shell snapshot in, executable plus argv and matching provenance
/// out. On Windows a resolved agent that is not a direct `.exe` enters the
/// adapter; a no-resolved-agent session keeps the historical `cmd.exe /C`
/// fallback byte-for-byte; non-Windows and direct `.exe` launches are unchanged.
fn prepare_launch(
    command: &str,
    args: &[String],
    resolved_agent_host_shell: Option<&ResolvedAgentHostShell>,
) -> Result<PreparedLaunch, AppError> {
    if cfg!(windows) && !is_direct_exe(command) {
        if let Some(host_shell) = resolved_agent_host_shell {
            return prepare_windows_resolved_agent_launch(command, args, host_shell);
        }
        // #1271 - no resolved agent: keep the existing `cmd.exe /C` construction
        // byte-for-byte (a bare non-direct configured shell on Windows).
        let mut wrapped = vec!["/C".to_string(), command.to_string()];
        wrapped.extend(args.iter().cloned());
        return Ok(PreparedLaunch {
            program: "cmd.exe".to_string(),
            args: wrapped,
        });
    }
    Ok(PreparedLaunch {
        program: command.to_string(),
        args: args.to_vec(),
    })
}

fn prepare_windows_resolved_agent_launch(
    command: &str,
    args: &[String],
    host_shell: &ResolvedAgentHostShell,
) -> Result<PreparedLaunch, AppError> {
    validate_common_adapter_input(command, args, host_shell)?;
    let mut launch_args = host_shell.args.clone();
    match configured_shell_kind(&host_shell.program) {
        WindowsHostShellKind::PowerShell => {
            validate_powershell_arguments(&host_shell.program, &host_shell.args, false)?;
            validate_explicit_batch_args(&host_shell.program, command, args)?;
            launch_args.push("-Command".to_string());
            launch_args.push(powershell_script(command, args));
        }
        WindowsHostShellKind::Pwsh => {
            validate_powershell_arguments(&host_shell.program, &host_shell.args, true)?;
            validate_explicit_batch_args(&host_shell.program, command, args)?;
            launch_args.push("-Command".to_string());
            launch_args.push(powershell_script(command, args));
        }
        WindowsHostShellKind::Cmd => {
            validate_cmd_arguments(&host_shell.program, &host_shell.args)?;
            validate_cmd_payload(&host_shell.program, command, args)?;
            launch_args.extend(["/V:OFF".to_string(), "/S".to_string(), "/C".to_string()]);
            launch_args.push(cmd_payload(command, args));
        }
        WindowsHostShellKind::Posix => {
            if let Some(first) = host_shell.args.first() {
                return Err(adapter_error(
                    &host_shell.program,
                    first,
                    "a conflicting/terminal configured option (custom shell accepts no arguments)",
                ));
            }
            launch_args.push("-c".to_string());
            launch_args.push(posix_script(command, args));
        }
    }
    Ok(PreparedLaunch {
        program: host_shell.program.clone(),
        args: launch_args,
    })
}
impl Clone for LocalProcessBackend {
    fn clone(&self) -> Self {
        Self {
            ptys: Arc::clone(&self.ptys),
            fanout: self.fanout.clone(),
            git_watcher: Arc::clone(&self.git_watcher),
            #[cfg(test)]
            pre_pty_attempts: Arc::clone(&self.pre_pty_attempts),
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
            #[cfg(test)]
            pre_pty_attempts: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn spawn_sync(&self, spec: BackendSpawnSpec) -> Result<(), AppError> {
        let BackendSpawnSpec {
            id,
            agent_id,
            coding_agent,
            cmd,
            args,
            resolved_agent_host_shell,
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

        // #1271 - validate and construct the adapted launch at the TOP of
        // `spawn_sync`, BEFORE any spawn accounting or PTY resource acquisition
        // (Grinch Finding 10): a rejected configured-host token is counted
        // nowhere and never opens a PTY. The same adapter result feeds both
        // `CommandBuilder` and the `exec_argv` provenance, so the two can never
        // desynchronize.
        let launch = prepare_launch(&cmd, &args, resolved_agent_host_shell.as_ref())?;

        // #942 - how many sessions were spawned in the window just before this one,
        // and how many of them were the same CLI. Concurrent startups against shared
        // agent state (the global ~/.codex) are a prime suspect for the intermittent
        // blank terminal, so every spawn record carries its own concurrency context.
        // Keyed on the CLI, never on the profile id: several profiles run the same
        // codex binary against the same ~/.codex. Counting only, no behavior change.
        let diag_thresholds = spawn_diagnostics::Thresholds::from_env();
        let spawn_window = spawn_diagnostics::note_spawn_attempt(coding_agent, diag_thresholds);
        // #1271 - accepted spawns only: the increment sits immediately before
        // `native_pty_system()`, so every invalid-input rejection above left the
        // per-instance observer at zero.
        #[cfg(test)]
        self.pre_pty_attempts.fetch_add(1, Ordering::SeqCst);
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

        let mut command = launch.command_builder();
        command.cwd(&spawn_cwd);

        // #942 - the argv exactly as executed, adapted host-shell wrapper
        // included. Built from the SAME `PreparedLaunch` the `CommandBuilder`
        // came from (one adapter decision), never mirrored by hand.
        let exec_argv = launch.exec_argv();

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
        log::info!(
            "[pty] Spawned session {} with child pid {:?}",
            id,
            child_pid
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
        let output_token =
            match self
                .fanout
                .register_session(id, idle_tuning, rows, cols, output_target)
            {
                Ok(token) => token,
                Err(_) => {
                    let _ = self.kill(id);
                    return Err(AppError::PtyError(
                        "terminal output registration failed".to_string(),
                    ));
                }
            };

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
                        fanout.handle_output(&output_token, &session_id_str, buf[..n].to_vec());
                        // #973 (B) - has the child PAINTED anything yet? Asked of the real vt100
                        // parser that `handle_output` has just fed this chunk to, which is why it
                        // is asked after it and not before.
                        //
                        // Once the child has painted, this is a single relaxed load per chunk and
                        // nothing else: no lock, no scan, no allocation. Before it has, it is one
                        // uncontended lock and a bounded scan of the grid - and it REPLACED a
                        // second `from_utf8_lossy` + `strip_ansi_csi` over the whole chunk, which
                        // `handle_output` was already paying for the idle detector on the very
                        // same bytes. One less chunk-sized allocation per chunk, not one more.
                        //
                        // Locks: the query takes `screen_parsers` and RELEASES it before
                        // `open_startup_gate` takes `ptys`. The two are sequential, never nested,
                        // so the order stays `ptys -> startup_gate -> size -> master`.
                        if !rendered.load(Ordering::Relaxed)
                            && fanout.has_rendered_visible_content(id)
                        {
                            gate_backend.open_startup_gate(id);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    /// #973 (B) - the child rendered. Hand the ConPTY the size the view has been waiting to
    /// give it, and bring the vt100 screen with it. Called from the PTY read loop, once per
    /// session in practice. The decision itself is `hand_over_held_size`.
    fn open_startup_gate(&self, id: Uuid) {
        let applied = {
            let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            let Some(instance) = ptys.get(&id) else {
                return;
            };
            hand_over_held_size(instance, id)
        };

        // Outside the `ptys` guard: the screen and the broadcast take locks of their own, and
        // every terminal write, resize and kill in the app queues behind that one.
        //
        // Only for a size the ConPTY actually took. The vt100 screen models the CHILD's
        // screen, so it must not be moved to a size the child was never given.
        if let Some((cols, rows)) = applied {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }
    }

    /// #942 - liveness of the child of a session, without disturbing it. Never blocks
    /// (zero timeout) and never reports a child it could not query as running.
    fn probe_child(&self, id: Uuid) -> ChildLiveness {
        probe_child_in(&self.ptys, id)
    }
}

/// #942 - the body of `probe_child`, free over the map so #1032's liveness gate can be
/// driven by a test against a real ConPTY child.
///
/// The guard is a local and `ChildLiveness` is owned, so the `ptys` lock is released at the
/// return. That is what makes the gate below safe by construction rather than by care: the
/// `match` scrutinee holds no borrow of the map, so no temporary-lifetime extension can
/// carry the guard into an arm and nest `screen_parsers` inside `ptys`.
fn probe_child_in(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> ChildLiveness {
    let mut ptys = ptys.lock().unwrap_or_else(|e| e.into_inner());
    let Some(instance) = ptys.get_mut(&id) else {
        return ChildLiveness::Gone;
    };
    let Some(child) = instance.child.as_mut() else {
        return ChildLiveness::Gone;
    };
    probe_child_contained(child)
}

/// #1032 - a local session's screen rows, gated on its child actually being alive.
///
/// Why a gate at all: a coding agent's statusline SURVIVES on the frozen grid after the
/// child dies. Verbatim, ~8s after a confirmed `code: 0`, on every exit path that matters -
/// a killed process cannot repaint, and Claude Code never uses the alternate screen, never
/// clears and never restores. PTY EOF never arrives either. Nothing signals the death from
/// any direction, so the frozen grid presents a perfectly well-formed row - glyph present,
/// `%` present, column 2, last match on the grid - that passes every defence the user's
/// pattern has. Liveness is not a regex problem, and asking the child is the only thing
/// that can tell a live 42% from a dead one.
///
/// Free over the map and the fanout, like `resize_instance` above, so the gate can be driven
/// by a test against a real ConPTY child: `LocalProcessBackend` itself cannot be built in a
/// unit test (its `GitWatcher` needs a Tauri `AppHandle`), and none of that has anything to
/// do with whether a child is alive.
pub(crate) fn context_liveness_from_child_liveness(
    liveness: &ChildLiveness,
) -> ContextSessionLiveness {
    match liveness {
        ChildLiveness::Alive => ContextSessionLiveness::Live,
        ChildLiveness::Unqueryable(_) => ContextSessionLiveness::Unavailable,
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ContextSessionLiveness::SessionOver,
    }
}

#[cfg(test)]
mod context_liveness_tests {
    use super::{context_liveness_from_child_liveness, ChildLiveness, ContextSessionLiveness};

    #[test]
    fn maps_every_contained_child_liveness_state() {
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Alive),
            ContextSessionLiveness::Live
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Unqueryable("denied".into())),
            ContextSessionLiveness::Unavailable
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Exited {
                code: 0,
                success: true,
            }),
            ContextSessionLiveness::SessionOver
        );
        assert_eq!(
            context_liveness_from_child_liveness(&ChildLiveness::Gone),
            ContextSessionLiveness::SessionOver
        );
    }
}

fn screen_rows_if_child_alive(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    fanout: &SessionIoFanout,
    id: Uuid,
) -> ScreenRowsRead {
    match probe_child_in(ptys, id) {
        ChildLiveness::Alive => match fanout.get_screen_rows(id) {
            Some(rows) => ScreenRowsRead::Rows(rows),
            // The child is alive, so the session is NOT over. A missing or poisoned parser
            // here is a desync or a poisoned lock, never a statement about the session.
            None => ScreenRowsRead::Unavailable,
        },
        ChildLiveness::Exited { .. } | ChildLiveness::Gone => ScreenRowsRead::SessionOver,
        // "We could not ask" is NOT "the child is dead". #942 built a three-valued oracle
        // exactly so those two never merge, and this is the arm that keeps them apart: the
        // same running process reads Alive through a full-rights handle and Unqueryable
        // through one stripped of SYNCHRONIZE. Reporting that as over would deregister a
        // live session permanently, on a child that never died.
        ChildLiveness::Unqueryable(_) => ScreenRowsRead::Unavailable,
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

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        // #942 - poison-tolerant: a panic anywhere under this guard (portable-pty unwraps
        // inside its own child polling) must not brick every terminal write that follows.
        write_to_local_pty(&self.ptys, id, data)
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.ptys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        context_liveness_from_child_liveness(&probe_child_in(&self.ptys, id))
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        // The idle grace sees every resize the view ASKED for, held or refused. Idle semantics
        // are #954's, not this fix's.
        self.fanout.record_resize(id);

        let sent = {
            let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            let instance = ptys
                .get(&id)
                .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
            resize_instance(instance, id, cols, rows)?
        };

        // #973 - the vt100 screen models the CHILD's screen, so it may only follow a size the
        // ConPTY actually took.
        //
        // A refused `0x0` would otherwise set the parser to zero rows (`output.rs`), and #955's
        // `get_screen_snapshot` reads `contents_formatted()` off that grid: an empty snapshot,
        // and a black tile on re-attach - the exact bug #955 shipped to kill.
        //
        // A HELD size is not the child's size either. The child is still emitting for the size
        // the PTY was opened at, and a screen moved ahead of it would parse that output against
        // a geometry the child is not using. `open_startup_gate` moves the screen at the instant
        // the ConPTY takes the size, which is the first moment the two agree.
        //
        // A DEDUPED size is already the screen's size: `register_session` seeds the parser with
        // the size the PTY was opened at, and from there the two only ever move together.
        if sent {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }

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

        let instance = remove_local_pty(&self.ptys, id);

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
                                let cause = if matches!(child_at_stop, ChildLiveness::Exited { .. })
                                {
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

    fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        self.fanout.has_rendered_visible_content(id)
    }

    fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
    ) -> Result<
        Option<crate::pty::output::PtyScreenSnapshot>,
        crate::pty::output::TerminalOutputAttachError,
    > {
        self.fanout
            .activate_terminal_output(id, label, include_history)
    }

    fn detach_terminal_output(&self, id: Uuid, label: &str) {
        self.fanout.detach_terminal_output(id, label);
    }

    fn release_window_attachments(&self, label: &str) {
        self.fanout.release_window_attachments(label);
    }

    fn shutdown_terminal_output(&self) {
        self.fanout.shutdown_terminal_output();
    }

    #[allow(private_interfaces)]
    fn copy_terminal_screen(&self, id: Uuid) -> crate::pty::backend::TerminalScreenCopyRead {
        self.fanout.copy_terminal_screen(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        screen_rows_if_child_alive(&self.ptys, &self.fanout, id)
    }

    /// #1171 - straight to the fanout, with **no child liveness probe**.
    ///
    /// `get_screen_rows` above probes the child under the `ptys` guard, "the one every
    /// terminal write, resize and kill locks on" (`:1116-1117`). At 200 ms that probe would be
    /// taken 25x more often than the 5 s scraper takes it, on the hottest lock in the PTY
    /// layer, for a question the watcher engine does not ask on this path: it runs its own
    /// liveness probe once every 25th tick, that is once per 5 s per session, exactly today's
    /// rate. What remains here is one `screen_parsers` acquisition, and that map is per
    /// backend rather than per process.
    ///
    /// An absent parser is `Missing` and not `Gone`: for a local process, parser-absence is a
    /// desync or a poisoned lock, never a statement about the child. Only the 5 s probe, or
    /// the child oracle behind `get_screen_rows`, may retire a local session.
    fn screen_rows_since(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince {
        self.fanout.get_screen_rows_since(id, seen)
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
        ptys.lock().unwrap().insert(
            id,
            Box::new(PoisonedChild) as Box<dyn portable_pty::Child + Send + Sync>,
        );

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
        assert!(
            PtyInstance::size_changed_in(&size, 74, 24),
            "one row is a real resize"
        );
        assert!(
            PtyInstance::size_changed_in(&size, 120, 30),
            "a real resize"
        );
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
    use super::{
        hand_over_held_size, resize_instance, send_size_to_conpty, PtyInstance, StartupGate,
    };
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

    /// #973 - a degenerate resize that lands WHILE THE GATE IS HOLDING must not evict the
    /// real size the view is waiting to give the child.
    ///
    /// This walks the path `pty_resize` actually walks - `resize_instance`, gate and all -
    /// which is the entire point of it. The guard used to sit in `send_size_to_conpty`, PAST
    /// the gate, so nothing checked a size on its way IN: the gate took the `0x0` last-wins
    /// over the real `74x23`, the hand-over popped it, `send_size_to_conpty` refused it, and
    /// the pending slot was consumed and gone. **Nothing retries.** The ConPTY then stays at
    /// the size it was OPENED at, for good - on cold start, a 120x30 child behind a 74x23
    /// terminal, until the user drags the window.
    ///
    /// Red before the guard moved ahead of the gate:
    /// `assertion failed: left: (120, 30)  right: (74, 23)`.
    #[test]
    fn a_degenerate_resize_cannot_evict_the_size_held_for_the_child() {
        let (instance, master) = conpty(120, 30);
        let id = Uuid::new_v4();

        // the view mounts and fits, while the child is still starting up: held, as it should be
        assert!(
            !resize_instance(&instance, id, 74, 23).expect("resize"),
            "nothing may reach a child that has not rendered yet"
        );

        // ...and now a zero dimension arrives, still inside the startup window. It comes off the
        // wire: `web/commands.rs` takes cols/rows straight from a JSON payload. (Not from xterm -
        // `fit()` clamps to 2x1 and cannot produce a zero.)
        assert!(
            !resize_instance(&instance, id, 0, 0).expect("must not error"),
            "a zero dimension must never be sent to the ConPTY"
        );

        // the child paints. This is the read loop's hand-over itself, not a copy of it.
        hand_over_held_size(&instance, id);

        assert_eq!(
            size_the_child_sees(&master),
            (74, 23),
            "the real held size must survive a degenerate resize"
        );
    }

    /// #973 - the guard INSIDE `send_size_to_conpty`, which is defence in depth: the last line
    /// before `master.resize()`, and the one `hand_over_held_size` relies on when it applies a
    /// size that has been sitting in the gate. The guard that keeps a `0x0` off the request
    /// path is in `resize_instance`, ahead of the gate - see
    /// `a_degenerate_resize_cannot_evict_the_size_held_for_the_child`, which is the one that
    /// walks the path `pty_resize` walks. This test calls `send_size_to_conpty` DIRECTLY and
    /// makes no claim about the gate.
    ///
    /// It caught a real one. I had assumed portable-pty would reject `0x0`. **It does not**:
    /// `master.resize(0x0)` returns Ok and the ConPTY is genuinely set to zero, so before the
    /// guard this failed with `left: (0, 0)`.
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
        assert_eq!(
            gate.open(),
            Some((74, 24)),
            "the last held size, handed over once"
        );
        assert_eq!(gate.open(), None, "a second open hands over nothing");
        assert_eq!(
            gate.on_resize(80, 25),
            Some((80, 25)),
            "the gate is open now: resizes go straight through"
        );
    }
}

#[cfg(test)]
mod context_gate_tests {
    use super::{
        probe_child_in, screen_rows_if_child_alive, ChildLiveness, PtyInstance, StartupGate,
    };
    use crate::pty::context_scrape::ScreenRowsRead;
    use crate::pty::idle_detector::IdleDetector;
    use crate::pty::output::SessionIoFanout;
    use crate::session::profile::IdleTuning;
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    /// The statusline the agent painted, and the one that stays on the grid after it dies.
    const ROW: &str = "  Context \u{2591}\u{2591}\u{2588} 42%";

    /// A real ConPTY with a REAL CHILD on the far end, plus the real fanout.
    ///
    /// The child is the entire point. This gate's whole job is to answer a question the grid
    /// cannot - is the child alive? - so a fake child cannot be asked it wrongly, which means
    /// it cannot be asked at all. `startup_gate_tests::conpty` already opens the real ConPTY
    /// and builds the real `PtyInstance`; this fills in its `child: None`.
    fn conpty_with_child(cols: u16, rows: u16) -> (Mutex<HashMap<Uuid, PtyInstance>>, Uuid) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        // A child that sits and waits, the way an agent CLI does.
        let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
        let child = pair
            .slave
            .spawn_command(CommandBuilder::new(shell))
            .expect("spawn a real child");
        drop(pair.slave);

        let writer = pair.master.take_writer().expect("take_writer");
        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Some(child),
            job: None,
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::new(AtomicBool::new(false)),
        };

        let id = Uuid::new_v4();
        let mut map = HashMap::new();
        map.insert(id, instance);
        (Mutex::new(map), id)
    }

    fn fanout() -> SessionIoFanout {
        SessionIoFanout::new(
            Arc::new(Mutex::new(HashMap::new())),
            IdleDetector::new(|_| {}, |_| {}),
            None,
        )
    }

    fn paint(fanout: &SessionIoFanout, id: Uuid) {
        let token = fanout
            .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
            .expect("register test session");
        fanout.handle_output(&token, &id.to_string(), ROW.as_bytes().to_vec());
    }

    /// Kill the child and wait until it is REALLY gone - which is NOT what
    /// `portable_pty::Child::wait()` waits for, and finding that out cost this test a failing
    /// run.
    ///
    /// `TerminateProcess` is asynchronous, and `wait()` short-circuits on `try_wait()` ->
    /// `is_complete()` -> `GetExitCodeProcess`. The exit code is readable ~14 ms (measured
    /// here) BEFORE the kernel signals the process object, so `wait()` returns `Ok(1)` while
    /// `WaitForSingleObject(h, 0)` still answers WAIT_TIMEOUT. That is #942 arriving from the
    /// other side: `is_complete` is exactly the accessor AC's oracle refuses to trust, and
    /// the oracle asks the stronger question on purpose. Production never notices the gap -
    /// 14 ms against a 5 s tick - but a test that kills and reads in the same breath lands
    /// inside it every time.
    fn kill_and_await_real_exit(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) {
        {
            let mut map = ptys.lock().unwrap();
            let child = map
                .get_mut(&id)
                .expect("instance")
                .child
                .as_mut()
                .expect("child");
            child.kill().expect("kill the child");
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !matches!(probe_child_in(ptys, id), ChildLiveness::Alive) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the oracle answered Alive for 10s after TerminateProcess: it cannot see deaths");
    }

    /// The live case: a real child, a real grid with a real statusline on it, and the gate
    /// hands the rows straight through.
    #[test]
    fn rows_are_readable_while_the_child_is_alive() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        paint(&fanout, id);

        match screen_rows_if_child_alive(&ptys, &fanout, id) {
            ScreenRowsRead::Rows(rows) => assert_eq!(rows[0], ROW),
            ScreenRowsRead::Unavailable => panic!("a live child with a live parser must read"),
            ScreenRowsRead::SessionOver => panic!("the child is alive; the session is not over"),
        }
    }

    /// The case the whole gate exists for, and it is RED if the probe call is deleted.
    ///
    /// The parser is never touched here, so the row stays on the frozen grid verbatim,
    /// exactly as it does in production: a killed process cannot repaint, and no EOF ever
    /// arrives to notice. Without the probe this reads a perfectly well-formed `42%` off a
    /// dead session forever, and no rule expressible in the user pattern can tell.
    #[test]
    fn session_over_once_the_child_actually_exits() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        paint(&fanout, id);

        kill_and_await_real_exit(&ptys, id);

        assert!(
            matches!(
                screen_rows_if_child_alive(&ptys, &fanout, id),
                ScreenRowsRead::SessionOver
            ),
            "the child is gone, so the session is over - whatever the frozen grid still says"
        );

        // ...and the row really is still sitting there. This is the fact that makes the probe
        // load-bearing rather than belt-and-braces.
        assert_eq!(
            fanout.get_screen_rows(id).expect("the parser is untouched")[0],
            ROW,
            "if this goes red the grid started self-correcting and the gate premise changed"
        );
    }

    /// No instance at all is the other definite answer: `Gone`, so the session is over.
    #[test]
    fn session_over_when_there_is_no_pty_instance() {
        let (ptys, _id) = conpty_with_child(120, 30);
        let fanout = fanout();
        assert!(matches!(
            screen_rows_if_child_alive(&ptys, &fanout, Uuid::new_v4()),
            ScreenRowsRead::SessionOver
        ));
    }

    /// A live child whose parser is missing is a DESYNC, not a dead session, and the caller
    /// must keep sampling it. This is the two-state collapse `ScreenRowsRead` exists to
    /// prevent, pinned at the one seam where the real oracle is involved.
    #[test]
    fn a_live_child_with_no_parser_is_unavailable_not_over() {
        let (ptys, id) = conpty_with_child(120, 30);
        let fanout = fanout();
        // deliberately never registered with the fanout

        assert!(
            matches!(
                screen_rows_if_child_alive(&ptys, &fanout, id),
                ScreenRowsRead::Unavailable
            ),
            "the child is alive, so nothing here may claim the session is over"
        );
    }

    /// #1171, 9.1.4 (local half) - an unknown id is `Missing` here, where the container
    /// backend answers `Gone` to the identical question.
    ///
    /// Asserted against the fanout call the `screen_rows_since` override delegates to, and not
    /// against a `LocalProcessBackend`, because that type cannot be built in a unit test: its
    /// `GitWatcher` needs a Tauri `AppHandle` (`:95-96`), which is the same reason
    /// `screen_rows_if_child_alive` above is a free function. The override adds nothing to
    /// this call on purpose - **no child liveness probe** - so this is the whole of its
    /// behavior for an unknown id.
    #[test]
    fn the_watcher_seam_reports_missing_for_an_unknown_id() {
        let fanout = fanout();

        assert!(
            matches!(
                fanout.get_screen_rows_since(Uuid::new_v4(), None),
                crate::pty::watchers::ScreenRowsSince::Missing
            ),
            "for a local process, parser-absence is a desync, never a statement about the child"
        );
    }
}

#[cfg(all(test, windows))]
mod blocked_writer_teardown_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn ptys_with_nonreading_child() -> (Arc<Mutex<HashMap<Uuid, PtyInstance>>>, Uuid) {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open real ConPTY");
        let mut command = CommandBuilder::new("powershell.exe");
        command.arg("-NoProfile");
        command.arg("-NonInteractive");
        command.arg("-Command");
        command.arg("Start-Sleep -Seconds 30");
        let child = pair
            .slave
            .spawn_command(command)
            .expect("spawn non-reading child");
        drop(pair.slave);
        let writer = pair.master.take_writer().expect("take ConPTY writer");
        let id = Uuid::new_v4();
        let ptys = Arc::new(Mutex::new(HashMap::from([(
            id,
            PtyInstance {
                master: Arc::new(Mutex::new(pair.master)),
                writer: Arc::new(Mutex::new(writer)),
                child: Some(child),
                job: None,
                size: Mutex::new((120, 30)),
                startup_gate: Mutex::new(StartupGate::Holding(None)),
                rendered: Arc::new(AtomicBool::new(false)),
            },
        )])));
        (ptys, id)
    }

    // Real-ConPTY teardown evidence, kept out of the default parallel run.
    //
    // This spawns a genuine ConPTY child and relies on the OS pipe buffer filling so a
    // 65,536-byte write stays blocked long enough to be observed within a ~250 ms detection
    // window. That precondition (the `observed_block` assertion below) is parallel-load
    // sensitive: it holds deterministically when the test runs in isolation, but under the
    // full parallel `cargo test --lib` run the write can drain before detection, so the
    // precondition flakes. The teardown/unblock behavior actually under test is sound; only
    // the setup precondition is timing fragile, and it is a real platform boundary that
    // cannot be reliably simulated under automated parallel load.
    //
    // The frozen plan therefore routes real-ConPTY behavior to manual Windows evidence
    // (section 14 acceptance item 10: automated assertions everywhere, with ConPTY manual
    // evidence only where the real platform boundary cannot be simulated; section 15 Windows
    // manual ConPTY/API matrix). It stays `#[ignore]`d out of the default automated gate and
    // is exercised on demand, in isolation, as part of that manual matrix:
    //
    //   cargo test --lib real_blocked_conpty_writer_is_unblocked_by_session_teardown -- --ignored --test-threads=1
    #[test]
    #[ignore = "real ConPTY block precondition is parallel-load sensitive; run in isolation as manual Windows matrix evidence (plan section 14 item 10 / section 15)"]
    fn real_blocked_conpty_writer_is_unblocked_by_session_teardown() {
        let (ptys, id) = ptys_with_nonreading_child();
        let started = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = mpsc::channel();
        let writer_ptys = Arc::clone(&ptys);
        let writer_started = Arc::clone(&started);
        let writer_completed = Arc::clone(&completed);
        let writer = std::thread::spawn(move || {
            let chunk = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES];
            let outcome = (0..4096).try_for_each(|_| {
                writer_started.fetch_add(1, Ordering::SeqCst);
                write_to_local_pty(&writer_ptys, id, &chunk).map_err(|error| error.to_string())?;
                writer_completed.fetch_add(1, Ordering::SeqCst);
                Ok::<(), String>(())
            });
            done_tx.send(outcome).expect("publish writer completion");
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed_block = false;
        while Instant::now() < deadline {
            let before = (
                started.load(Ordering::SeqCst),
                completed.load(Ordering::SeqCst),
            );
            if before.0 > before.1 {
                std::thread::sleep(Duration::from_millis(250));
                let after = (
                    started.load(Ordering::SeqCst),
                    completed.load(Ordering::SeqCst),
                );
                if after == before {
                    observed_block = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            observed_block,
            "the real ConPTY child must leave a writer blocked before teardown"
        );

        let kill_started = Instant::now();
        let mut instance = remove_local_pty(&ptys, id).expect("remove blocked PTY instance");
        if let Some(mut child) = instance.child.take() {
            child.kill().expect("terminate non-reading child");
        }
        drop(instance);
        assert!(
            kill_started.elapsed() < Duration::from_secs(2),
            "teardown must not wait on the blocked writer mutex"
        );
        let outcome = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("child teardown must unblock the real ConPTY write");
        assert!(outcome.is_err(), "the broken PTY write must report failure");
        writer.join().expect("join blocked writer thread");
        assert!(!ptys.lock().unwrap().contains_key(&id));
    }
}

// #1271 - pure adapter and closed configured-host grammar tests. Platform
// independent: they exercise the classifier, encoders, and generators directly,
// not through the real PTY path. The spawn_sync ordering proof lives in
// `adapter_spawn_sync_tests` (windows-gated, real backend).
#[cfg(test)]
mod adapter_tests {
    use super::*;
    use crate::errors::AppError;
    use crate::pty::backend::ResolvedAgentHostShell;

    fn host(program: &str, args: &[&str]) -> ResolvedAgentHostShell {
        ResolvedAgentHostShell {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn launch_error(launch: Result<PreparedLaunch, AppError>) -> String {
        match launch {
            Ok(_) => panic!("expected adapter rejection"),
            Err(e) => e.to_string(),
        }
    }

    fn assert_error_shape(error: &str, host_program: &str, token: &str, category: &str) {
        assert!(
            error.contains(host_program),
            "error must name the configured host '{host_program}': {error}"
        );
        assert!(
            error.contains(token),
            "error must name the offending token '{token}': {error}"
        );
        assert!(
            error.contains(category),
            "error must state the category '{category}': {error}"
        );
        assert!(
            error.contains("agent adapter owns command execution"),
            "error must state that the agent adapter owns command execution: {error}"
        );
    }

    // --- Bare agent + configured PowerShell ---------------------------------

    #[test]
    fn configured_default_shell_powershell_hosts_bare_agent_with_managed_native_script() {
        let shell = host(
            r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe",
            &["-NoProfile", "-ExecutionPolicy", "Bypass"],
        );
        let launch = prepare_windows_resolved_agent_launch("claude", &[], &shell)
            .expect("valid configured PowerShell host");

        // The adapter program is the exact configured path, never cmd.exe.
        assert_eq!(
            launch.program,
            r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe"
        );
        assert_eq!(
            launch.args[..3],
            ["-NoProfile".to_string(), "-ExecutionPolicy".to_string(), "Bypass".to_string()]
        );
        assert_eq!(launch.args[3], "-Command");
        let script = &launch.args[4];
        assert!(
            !script.contains("--%"),
            "generated script must never contain the stop-parsing token"
        );
        assert!(
            !script.contains("cmd.exe /C"),
            "generated script must not be a configured cmd fallback"
        );
        // Two-argument GetCommand application-only lookup, never `$true` third arg.
        assert!(script.contains("GetCommand('claude', $ac_kind)"));
        assert!(!script.contains("GetCommand('claude', $ac_kind, $true)"));
        assert!(script.contains(
            "[System.Management.Automation.CommandTypes]::Application -bor \
             [System.Management.Automation.CommandTypes]::ExternalScript"
        ));
        // Native branch: separate FileName + ProcessStartInfo with inherited handles.
        assert!(script.contains("$ac_start.FileName = $ac_command.Path"));
        assert!(script.contains("$ac_start.UseShellExecute = $false"));
        assert!(script.contains("$ac_start.RedirectStandardInput = $false"));
        assert!(script.contains("$ac_start.RedirectStandardOutput = $false"));
        assert!(script.contains("$ac_start.RedirectStandardError = $false"));
        assert!(script.contains("$ac_process.WaitForExit()"));
        assert!(script.contains("exit $ac_process.ExitCode"));
        // No logical args: Arguments is the empty ps-literal.
        assert!(script.contains("$ac_start.Arguments = '';"));
        // Provenance matches the launch exactly (one adapter result).
        let mut expected_argv = vec![shell.program.clone()];
        expected_argv.extend(shell.args.iter().cloned());
        expected_argv.push("-Command".to_string());
        expected_argv.push(script.clone());
        assert_eq!(launch.exec_argv(), expected_argv);
    }

    #[test]
    fn configured_default_shell_pwsh_full_path_with_spaces_is_used_exactly() {
        let shell = host(r"C:\Program Files\PowerShell\7\pwsh.exe", &["-NoProfile"]);
        let args = vec!["agent's tool".to_string(), "a b".to_string()];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("valid configured pwsh host");

        assert_eq!(launch.program, r"C:\Program Files\PowerShell\7\pwsh.exe");
        assert_eq!(launch.args[0], "-NoProfile");
        assert_eq!(launch.args[1], "-Command");
        let script = &launch.args[2];
        // The lookup name retains spaces and apostrophes through ps-literal.
        assert!(script.contains("'claude'"), "bare lookup name: {script}");
        // The native Arguments value is the standard Windows encoding of the
        // logical argv through a PowerShell literal.
        assert!(script.contains(
            "$ac_start.Arguments = '\"agent''s tool\" \"a b\"';"
        ));
        assert!(!script.contains("--%"));
        // The only cmd.exe mention is the documented nested system-cmd child
        // for a resolved .cmd/.bat target, never the top-level host.
        assert!(script.contains("'cmd.exe'"));
    }

    #[test]
    fn configured_default_shell_powershell_script_carries_batch_branch_and_raw_argv() {
        let shell = host("powershell.exe", &[]);
        let args = vec![
            "".to_string(),
            "a\"b".to_string(),
            "with space".to_string(),
            "o'clock".to_string(),
            "tail\\".to_string(),
            "a%z|p".to_string(),
            "a\\b\"c".to_string(),
        ];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("percent and pipe are data in the native branch");
        let script = &launch.args[1];
        assert!(script.contains("$ac_batch_unsupported_logical_arg = $true;"));
        // Batch sub-branch with the system-cmd child.
        assert!(script.contains(
            "([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.cmd')"
        ));
        assert!(script.contains(
            "[System.IO.Path]::Combine([System.Environment]::SystemDirectory, 'cmd.exe')"
        ));
        assert!(script.contains("'/D /V:OFF /S /C \"' + $ac_batch_payload + '\"'"));
        assert!(script.contains("$ac_command.Path.Contains([char]34)"));
        // The native Arguments literal is windows_raw_argv of the logical args.
        assert!(script.contains(
            "$ac_start.Arguments = '\"\" \"a\\\"b\" \"with space\" \"o''clock\" \
             \"tail\\\\\" \"a%z|p\" \"a\\b\\\"c\"';"
        ));
        // ExternalScript branch keeps ps-literal invocation.
        assert!(script.contains(
            "& $ac_command.Path '' 'a\"b' 'with space' 'o''clock' 'tail\\' 'a%z|p' 'a\\b\"c';"
        ));
        assert!(script.contains("$ac_succeeded = $?; $ac_exit_code = $LASTEXITCODE;"));
        assert!(script.ends_with("if ($ac_succeeded) { exit 0 }; exit 1"));
        assert!(!script.ends_with('\\'), "trailing-run invariant");
    }

    #[test]
    fn configured_default_shell_powershell_script_flags_batch_unsupported_arguments() {
        let shell = host("powershell.exe", &[]);
        let args = vec!["ok".to_string(), "bad%value".to_string()];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("native branch accepts percent");
        let script = &launch.args[1];
        assert!(script.contains("$ac_batch_unsupported_logical_arg = $true;"));
        assert!(script.contains("if ($ac_batch_unsupported_logical_arg -or"));
    }

    // --- Explicit batch targets through PowerShell --------------------------

    #[test]
    fn configured_default_shell_powershell_explicit_batch_rejects_percent_or_quote_args() {
        let shell = host("powershell.exe", &[]);
        let err = launch_error(prepare_windows_resolved_agent_launch(
            r"C:\tools\claude.cmd",
            &["a%b".to_string()],
            &shell,
        ));
        assert_error_shape(&err, "powershell.exe", "(batch target)", "unsupported explicit-batch");
        let err = launch_error(prepare_windows_resolved_agent_launch(
            r"C:\tools\claude.bat",
            &["a\"b".to_string()],
            &shell,
        ));
        assert_error_shape(&err, "powershell.exe", "(batch target)", "unsupported explicit-batch");
        // Clean explicit batch argv is accepted (nested system-cmd path).
        let launch = prepare_windows_resolved_agent_launch(
            r"C:\tools\claude.cmd",
            &["flag".to_string(), "bang!".to_string()],
            &shell,
        )
        .expect("explicit batch with clean args is supported");
        assert!(launch.args[1].contains("$ac_batch_unsupported_logical_arg = $false;"));
    }

    // --- cmd host -----------------------------------------------------------

    #[test]
    fn configured_default_shell_cmd_uses_unquoted_single_payload() {
        let shell = host(r"C:\Windows\System32\cmd.exe", &["/D", "/Q"]);
        let args = vec!["--flag".to_string(), "bang!".to_string(), r"a\b".to_string()];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("valid configured cmd host");
        assert_eq!(launch.program, r"C:\Windows\System32\cmd.exe");
        assert_eq!(
            launch.args[..2],
            ["/D".to_string(), "/Q".to_string()]
        );
        assert_eq!(
            launch.args[2..5],
            ["/V:OFF".to_string(), "/S".to_string(), "/C".to_string()]
        );
        // One single payload: program + args joined with ASCII spaces, no
        // per-token quoting, no outer quote pair of our own, no %% or caret.
        assert_eq!(launch.args[5], r"claude --flag bang! a\b");
        assert_eq!(launch.args.len(), 6);
        assert_eq!(
            launch.exec_argv(),
            vec![
                shell.program.clone(),
                "/D".to_string(),
                "/Q".to_string(),
                "/V:OFF".to_string(),
                "/S".to_string(),
                "/C".to_string(),
                r"claude --flag bang! a\b".to_string(),
            ]
        );
    }

    #[test]
    fn configured_default_shell_cmd_payload_rejects_out_of_domain_tokens() {
        let shell = host("cmd.exe", &[]);
        let rejected: &[&str] = &[
            "with space", "a\"b", "a&b", "a|b", "a^b", "a<b", "a>b", "a(b", "a)b",
            "a%b", "a=b", "a,b", "a;b", "", "a\\",
        ];
        for token in rejected {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[token.to_string()],
                &shell,
            ));
            if token.ends_with('\\') {
                // The payload-final backslash rule reports the payload itself.
                assert_error_shape(&err, "cmd.exe", "(payload)", "unsupported cmd payload character");
            } else {
                assert_error_shape(&err, "cmd.exe", token, "unsupported cmd payload character");
            }
        }
        // Leading '@' on the program token is rejected.
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "@claude",
            &[],
            &shell,
        ));
        assert_error_shape(&err, "cmd.exe", "@claude", "unsupported cmd payload character");
        // NUL/CR/LF remain forbidden (common rule).
        for bad in ["a\0b", "a\rb", "a\nb"] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[bad.to_string()],
                &shell,
            ));
            assert!(err.contains("forbidden line separator"), "{err}");
        }
    }

    #[test]
    fn configured_default_shell_cmd_accepts_flag_style_bang_and_internal_backslashes() {
        let shell = host("cmd.exe", &[]);
        let args = vec![
            "--flag".to_string(),
            "-x".to_string(),
            "!bang!".to_string(),
            r"a\b\c".to_string(),
            "plain".to_string(),
        ];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("accepted cmd payload domain");
        assert_eq!(
            launch.args[launch.args.len() - 1],
            r"claude --flag -x !bang! a\b\c plain"
        );
    }

    // --- Custom POSIX-compatible shell --------------------------------------

    #[test]
    fn configured_default_shell_custom_posix_host_gets_exec_script() {
        let shell = host(r"C:\tools\bash.exe", &[]);
        let args = vec!["it's".to_string(), "a b".to_string()];
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("custom posix host with empty args");
        assert_eq!(launch.program, r"C:\tools\bash.exe");
        assert_eq!(launch.args[0], "-c");
        assert_eq!(
            launch.args[1],
            "exec 'claude' 'it'\"'\"'s' 'a b'"
        );
        assert_eq!(launch.args.len(), 2);
    }

    #[test]
    fn configured_default_shell_custom_posix_host_rejects_any_configured_args() {
        let shell = host("bash.exe", &["--norc"]);
        let err = launch_error(prepare_windows_resolved_agent_launch("claude", &[], &shell));
        assert_error_shape(&err, "bash.exe", "--norc", "conflicting/terminal");
    }

    // --- Direct .exe and host-name extraction --------------------------------

    #[test]
    fn direct_exe_agent_keeps_historical_shape_without_host_context() {
        let shell = host("powershell.exe", &["-NoProfile"]);
        for program in ["claude.exe", r"C:\tools\Claude.EXE", "agent.EXE"] {
            let launch = prepare_launch(program, &["--x".to_string()], Some(&shell))
                .expect("direct exe must stay direct");
            assert_eq!(launch.program, program);
            assert_eq!(launch.args, vec!["--x".to_string()]);
        }
    }

    #[test]
    fn configured_shell_kind_extracts_final_path_component() {
        for (program, expected) in [
            ("powershell", WindowsHostShellKind::PowerShell),
            ("powershell.exe", WindowsHostShellKind::PowerShell),
            (r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe", WindowsHostShellKind::PowerShell),
            (r"\\server\share\POWERSHELL.EXE", WindowsHostShellKind::PowerShell),
            (r"\\?\C:\tools\pwsh.exe", WindowsHostShellKind::Pwsh),
            ("pwsh", WindowsHostShellKind::Pwsh),
            ("cmd", WindowsHostShellKind::Cmd),
            (r"C:\Program Files (x86)\thing\cmd.exe", WindowsHostShellKind::Cmd),
            ("cmd.exe", WindowsHostShellKind::Cmd),
            (r"C:\Program Files\Git\bin\bash.exe", WindowsHostShellKind::Posix),
            ("/usr/bin/sh", WindowsHostShellKind::Posix),
        ] {
            assert_eq!(
                configured_shell_kind(program),
                expected,
                "classification for {program}"
            );
        }
    }

    #[test]
    fn is_direct_exe_predicate_is_case_insensitive_and_extension_based() {
        assert!(is_direct_exe("claude.exe"));
        assert!(is_direct_exe("CLAUDE.EXE"));
        assert!(is_direct_exe(r"C:\tools\Agent.Exe"));
        assert!(!is_direct_exe("claude"));
        assert!(!is_direct_exe("claude.cmd"));
        assert!(!is_direct_exe("claude.bat"));
    }

    // --- Encoders -----------------------------------------------------------

    #[test]
    fn windows_arg_encodes_with_standard_quote_and_backslash_rules() {
        assert_eq!(windows_arg(""), "\"\"");
        assert_eq!(windows_arg("a"), "\"a\"");
        assert_eq!(windows_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(windows_arg("with space"), "\"with space\"");
        assert_eq!(windows_arg("o'clock"), "\"o'clock\"");
        assert_eq!(windows_arg("tail\\"), "\"tail\\\\\"");
        assert_eq!(windows_arg("a%z|p"), "\"a%z|p\"");
        assert_eq!(windows_arg("a\\b\"c"), r#""a\b\"c""#);
        assert_eq!(windows_arg("a\\\""), r#""a\\\"""#);
        assert_eq!(windows_arg("a\\\\\"c"), r#""a\\\\\"c""#);
        assert_eq!(windows_arg("a\\"), "\"a\\\\\"");
        assert_eq!(windows_arg("\\\\server\\share"), r#""\\server\share""#);
    }
    #[test]
    fn windows_raw_argv_joins_encoded_arguments_only() {
        assert_eq!(windows_raw_argv(&[]), "");
        assert_eq!(windows_raw_argv(&["".to_string()]), "\"\"");
        assert_eq!(
            windows_raw_argv(&["a b".to_string(), "".to_string()]),
            "\"a b\" \"\""
        );
    }

    #[test]
    fn ps_and_posix_literals_follow_their_quoting_rules() {
        assert_eq!(ps_literal("plain"), "'plain'");
        assert_eq!(ps_literal("o'clock"), "'o''clock'");
        assert_eq!(ps_literal(""), "''");
        assert_eq!(ps_literal("a\"b"), "'a\"b'");
        assert_eq!(ps_literal("a\\"), r"'a\'");
        assert_eq!(posix_literal("plain"), "'plain'");
        assert_eq!(posix_literal("it's"), "'it'\"'\"'s'");
    }

    // --- Closed configured-host grammar: rejected forms ----------------------

    #[test]
    fn powershell_grammar_rejects_command_ownership_and_terminal_options() {
        // Every conflicting/terminal spelling, in both - and / prefix spellings.
        for prefix in ["-", "/"] {
            for spelling in [
                "Command", "c", "CommandWithArgs", "cwa", "File", "f", "EncodedCommand",
                "enc", "e", "NoExit", "noe", "Version", "v", "Help", "h", "?",
                "SSHServerMode", "ServerMode", "WindowsPowerShell", "Login", "Interactive",
            ] {
                for is_pwsh in [false, true] {
                    let token = format!("{prefix}{spelling}");
                    let err = launch_error(prepare_windows_resolved_agent_launch(
                        "claude",
                        &[],
                        &host(
                            if is_pwsh { "pwsh.exe" } else { "powershell.exe" },
                            &[token.as_str()],
                        ),
                    ));
                    assert_error_shape(
                        &err,
                        if is_pwsh { "pwsh.exe" } else { "powershell.exe" },
                        &token,
                        "conflicting/terminal configured option",
                    );
                }
            }
        }
        // Parser terminators are literal tokens.
        for token in ["--", "--%"] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("powershell.exe", &[token]),
            ));
            assert_error_shape(&err, "powershell.exe", token, "conflicting/terminal");
        }
    }

    #[test]
    fn pwsh_grammar_rejects_powershell_only_spellings_as_unknown() {
        for prefix in ["-", "/"] {
            for spelling in ["Sta", "Mta", "WindowStyle", "PSConsoleFile"] {
                let token = format!("{prefix}{spelling}");
                let err = launch_error(prepare_windows_resolved_agent_launch(
                    "claude",
                    &[],
                    &host("pwsh.exe", &[token.as_str()]),
                ));
                assert_error_shape(&err, "pwsh.exe", &token, "unknown or ambiguous token");
            }
        }
        // The same spellings remain PERMITTED for powershell.exe.
        let launch = prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &["-Sta", "-WindowStyle", "Hidden"]),
        )
        .expect("powershell.exe accepts its own spellings");
        assert_eq!(launch.args[0], "-Sta");
    }

    #[test]
    fn powershell_grammar_rejects_bare_unknown_and_abbreviated_tokens() {
        for token in ["foo", "echo", "-NoPro", "/NoPro", "-Bogus", "/Bogus", "-noex", "/E:O"] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("powershell.exe", &[token]),
            ));
            assert_error_shape(&err, "powershell.exe", token, "unknown or ambiguous token");
        }
    }

    #[test]
    fn powershell_grammar_rejects_missing_and_option_shaped_operands() {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &["-ExecutionPolicy"]),
        ));
        assert_error_shape(&err, "powershell.exe", "ExecutionPolicy", "missing or option-shaped");
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &["-ExecutionPolicy", "-NoProfile"]),
        ));
        assert_error_shape(
            &err,
            "powershell.exe",
            "-NoProfile",
            "option-shaped separated operand",
        );
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &["-ExecutionPolicy:"]),
        ));
        assert_error_shape(&err, "powershell.exe", "-ExecutionPolicy:", "missing or option-shaped");
    }

    #[test]
    fn powershell_grammar_rejects_duplicates() {
        for (args, offending) in [
            (vec!["-NoProfile", "-NoProfile"], "-NoProfile"),
            (vec!["-NoProfile", "/NoProfile"], "/NoProfile"),
            (vec!["-ExecutionPolicy:Bypass", "-ExecutionPolicy", "RemoteSigned"], "RemoteSigned"),
        ] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("powershell.exe", &args),
            ));
            assert_error_shape(&err, "powershell.exe", offending, "duplicate configured option");
        }
    }

    #[test]
    fn cmd_grammar_rejects_ownership_terminals_and_unknowns() {
        for token in ["/C", "/K", "/S", "/V:ON", "/V:", "/V:ZZ", "/?", "-D", "foo", "/help", "/E:O"] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("cmd.exe", &[token]),
            ));
            let category = if matches!(token, "/C" | "/K" | "/S" | "/V:ON" | "/V:" | "/V:ZZ" | "/?") {
                "conflicting/terminal"
            } else {
                "unknown or ambiguous"
            };
            assert_error_shape(&err, "cmd.exe", token, category);
        }
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("cmd.exe", &["/T:"]),
        ));
        assert_error_shape(&err, "cmd.exe", "/T:", "missing or option-shaped");
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("cmd.exe", &["/D", "/D"]),
        ));
        assert_error_shape(&err, "cmd.exe", "/D", "duplicate configured option");
    }

    // --- Closed configured-host grammar: accepted forms ----------------------

    #[test]
    fn powershell_grammar_accepts_permitted_spellings_with_operand_binding() {
        // Flags + one separated operand + one attached `:` operand, order kept.
        let launch = prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host(
                "powershell.exe",
                &[
                    "-NoProfile",
                    "/NoLogo",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-WindowStyle",
                    "Hidden",
                ],
            ),
        )
        .expect("accepted powershell spellings");
        // Order preserved, every operand bound exactly once, suffix appended.
        assert_eq!(
            launch.args[..7],
            [
                "-NoProfile".to_string(),
                "/NoLogo".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-WindowStyle".to_string(),
                "Hidden".to_string(),
            ]
        );
        assert_eq!(launch.args[7], "-Command");

        // Each attached binding form is a separate accepted spelling.
        for (token, operand) in [
            ("-ExecutionPolicy:Bypass", "-ExecutionPolicy:Bypass"),
            ("-ExecutionPolicy=Bypass", "-ExecutionPolicy=Bypass"),
            ("/NoProfile", "/NoProfile"),
        ] {
            let launch = prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("powershell.exe", &[token]),
            )
            .expect("accepted attached binding form");
            assert_eq!(launch.args[0], operand);
            assert_eq!(launch.args[1], "-Command");
        }

        let launch = prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host(
                "pwsh.exe",
                &["-WorkingDirectory", r"C:\Program Files", "-SettingsFile", "x.json"],
            ),
        )
        .expect("accepted pwsh spellings");
        assert_eq!(
            launch.args[..4],
            [
                "-WorkingDirectory".to_string(),
                r"C:\Program Files".to_string(),
                "-SettingsFile".to_string(),
                "x.json".to_string(),
            ]
        );
        assert_eq!(launch.args[4], "-Command");
    }

    #[test]
    fn cmd_grammar_accepts_permitted_spellings_in_order() {
        let launch = prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("cmd.exe", &["/D", "/Q", "/E:ON", "/T:1F", "/V:OFF", "/A", "/F:OFF"]),
        )
        .expect("accepted cmd spellings");
        assert_eq!(
            launch.args[..7],
            [
                "/D".to_string(),
                "/Q".to_string(),
                "/E:ON".to_string(),
                "/T:1F".to_string(),
                "/V:OFF".to_string(),
                "/A".to_string(),
                "/F:OFF".to_string(),
            ]
        );
        assert_eq!(launch.args[7], "/V:OFF"); // adapter suffix
        assert_eq!(launch.args[8], "/S");
        assert_eq!(launch.args[9], "/C");
    }

    // --- Common validation ---------------------------------------------------

    #[test]
    fn common_validation_rejects_blank_host_and_line_separators() {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("   ", &[]),
        ));
        assert!(err.contains("blank configured default-shell program"), "{err}");

        for bad in ["pow\0er", "pow\rer", "pow\ner"] {
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host(bad, &[]),
            ));
            assert!(err.contains("forbidden line separator"), "{err}");
        }
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &["a\nb".to_string()],
            &host("powershell.exe", &[]),
        ));
        assert!(err.contains("forbidden line separator"), "{err}");
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "cla\0ude",
            &[],
            &host("powershell.exe", &[]),
        ));
        assert!(err.contains("forbidden line separator"), "{err}");
    }

    // --- No-resolved-agent fallback and cross-platform shape ------------------

    #[test]
    fn prepare_launch_keeps_cmd_exe_fallback_without_host_shell_and_direct_otherwise() {
        // No host shell + non-direct command: historical cmd.exe /C wrapper.
        let launch = prepare_launch("claude", &["--x".to_string()], None)
            .expect("fallback launch");
        assert_eq!(launch.program, "cmd.exe");
        assert_eq!(
            launch.args,
            vec!["/C".to_string(), "claude".to_string(), "--x".to_string()]
        );
        // Direct exe: unchanged program + argv.
        let launch = prepare_launch("claude.exe", &["--x".to_string()], None)
            .expect("direct launch");
        assert_eq!(launch.program, "claude.exe");
        assert_eq!(launch.args, vec!["--x".to_string()]);
    }

    #[test]
    fn powershell_script_never_ends_with_backslash_across_awkward_values() {
        let shell = host("powershell.exe", &[]);
        for args in [
            vec![],
            vec!["tail\\".to_string()],
            vec!["a'b".to_string()],
            vec!["a\\b\"c".to_string(), "x".to_string()],
        ] {
            let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
                .expect("accepted native values");
            let script = launch.args.last().unwrap();
            assert!(!script.ends_with('\\'), "trailing-run invariant for {args:?}");
        }
    }

    #[test]
    fn command_builder_and_exec_argv_share_one_adapter_result() {
        let shell = host("powershell.exe", &["-NoProfile"]);
        let launch = prepare_windows_resolved_agent_launch("claude", &["--x".to_string()], &shell)
            .expect("adapter launch");
        let builder = launch.command_builder();
        let _ = builder; // CommandBuilder has no public argv; exec_argv is the
                         // provenance, asserted equal to the adapter result above.
        assert_eq!(launch.exec_argv().len(), 4);
    }
}

// #1271 - spawn_sync ordering proof: deterministic invalid-input rejections
// happen at the TOP of the REAL `spawn_sync` (before `note_spawn_attempt` and
// before `native_pty_system()`), so the per-instance observer stays at zero,
// no PTY map entry exists, and no launch provenance was recorded. Each test
// builds its own backend instance and reads its own observer; there is no
// global to reset and no cross-test race.
#[cfg(all(test, windows))]
mod adapter_spawn_sync_tests {
    use super::*;
    use crate::pty::backend::ResolvedAgentHostShell;
    use crate::session::manager::SessionManager;
    use std::sync::atomic::Ordering;

    fn test_backend() -> (LocalProcessBackend, tauri::App) {
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        // GitWatcher takes a plain (Wry) AppHandle; the pty_lifecycle pattern
        // builds a default-runtime mock app the same way.
        let app = crate::test_support::test_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build adapter spawn-sync test app");
        let git_watcher = GitWatcher::new(session_mgr, app.handle().clone());
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        (
            LocalProcessBackend::new(output_senders, idle_detector, git_watcher, None),
            app,
        )
    }

    fn spawn_spec(
        cmd: &str,
        args: &[&str],
        host_shell: Option<ResolvedAgentHostShell>,
    ) -> BackendSpawnSpec {
        BackendSpawnSpec {
            id: Uuid::new_v4(),
            agent_id: None,
            coding_agent: None,
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            resolved_agent_host_shell: host_shell,
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            selected_cwd: None,
            cols: 80,
            rows: 24,
            container_image: None,
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            env_unset: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target: crate::pty::output::PtyOutputTarget::noop(),
            resource_registration: None,
            logical_resource_slot: None,
            container_credential: None,
            container_repo_mounts: Vec::new(),
        }
    }

    fn host(program: &str, args: &[&str]) -> ResolvedAgentHostShell {
        ResolvedAgentHostShell {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn invalid_configured_shell_token_never_reaches_pty() {
        let (backend, _app) = test_backend();
        // A conflicting/terminal configured option for the PowerShell host.
        let spec = spawn_spec(
            "claude",
            &[],
            Some(host("powershell.exe", &["-NoProfile", "-Command"])),
        );
        let id = spec.id;

        let error = backend.spawn_sync(spec).expect_err("must reject before PTY");
        assert!(error.to_string().contains("-Command"), "{error}");
        assert!(error.to_string().contains("conflicting/terminal"), "{error}");
        assert_eq!(
            backend.pre_pty_attempts.load(Ordering::SeqCst),
            0,
            "a rejected configured-host token must never count as a spawn attempt"
        );
        assert!(
            backend.ptys.lock().unwrap().is_empty(),
            "no PTY map entry may exist for a rejected input"
        );
        assert!(
            crate::pty::spawn_diagnostics::record_for(id).is_none(),
            "no launch provenance may be recorded for a rejected input"
        );
        assert!(
            backend.fanout.get_pty_size(id).is_none(),
            "no output task may be attached for a rejected input"
        );
    }

    #[test]
    fn invalid_cmd_payload_token_never_reaches_pty() {
        let (backend, _app) = test_backend();
        let spec = spawn_spec(
            "claude",
            &["a b".to_string().as_str()],
            Some(host("cmd.exe", &["/D"])),
        );
        let id = spec.id;

        let error = backend.spawn_sync(spec).expect_err("must reject before PTY");
        assert!(error.to_string().contains("unsupported cmd payload character"), "{error}");
        assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
        assert!(backend.ptys.lock().unwrap().is_empty());
        assert!(crate::pty::spawn_diagnostics::record_for(id).is_none());
    }

    #[test]
    fn invalid_explicit_batch_argument_never_reaches_pty() {
        let (backend, _app) = test_backend();
        let spec = spawn_spec(
            r"C:\tools\claude.cmd",
            &["bad%value".to_string().as_str()],
            Some(host("powershell.exe", &["-NoProfile"])),
        );
        let id = spec.id;

        let error = backend.spawn_sync(spec).expect_err("must reject before PTY");
        assert!(error.to_string().contains("unsupported explicit-batch"), "{error}");
        assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
        assert!(backend.ptys.lock().unwrap().is_empty());
        assert!(crate::pty::spawn_diagnostics::record_for(id).is_none());
    }

    #[test]
    fn blank_configured_program_never_reaches_pty() {
        let (backend, _app) = test_backend();
        let spec = spawn_spec("claude", &[], Some(host("   ", &[])));
        let id = spec.id;

        let error = backend.spawn_sync(spec).expect_err("must reject before PTY");
        assert!(error.to_string().contains("blank configured default-shell program"), "{error}");
        assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
        assert!(backend.ptys.lock().unwrap().is_empty());
        assert!(crate::pty::spawn_diagnostics::record_for(id).is_none());
    }

    #[test]
    fn non_empty_custom_shell_args_never_reaches_pty() {
        let (backend, _app) = test_backend();
        let spec = spawn_spec("claude", &[], Some(host("bash.exe", &["--norc"])));
        let id = spec.id;

        let error = backend.spawn_sync(spec).expect_err("must reject before PTY");
        assert!(error.to_string().contains("conflicting/terminal"), "{error}");
        assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 0);
        assert!(backend.ptys.lock().unwrap().is_empty());
        assert!(crate::pty::spawn_diagnostics::record_for(id).is_none());
    }

    #[test]
    fn accepted_spawn_increments_observer_and_registers_session() {
        let (backend, _app) = test_backend();
        // Direct .exe configured shell: the ordinary shell-session shape, no
        // adapter. Proves the observer is not vacuously always-zero.
        let spec = spawn_spec("cmd.exe", &["/C", "exit", "0"], None);
        let id = spec.id;

        backend.spawn_sync(spec).expect("valid spawn must succeed");
        assert_eq!(backend.pre_pty_attempts.load(Ordering::SeqCst), 1);
        assert!(backend.ptys.lock().unwrap().contains_key(&id));
        assert!(crate::pty::spawn_diagnostics::record_for(id).is_some());
        assert!(backend.fanout.get_pty_size(id).is_some());

        backend.kill(id).expect("kill spawned child");
        assert!(!backend.ptys.lock().unwrap().contains_key(&id));
    }
}
