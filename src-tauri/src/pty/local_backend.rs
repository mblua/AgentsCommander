use std::collections::HashMap;
use std::io::{Read, Write};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
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

#[cfg(windows)]
use crate::config::instance_artifacts::GIT_GUARD_DIR_NAME;
use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend, ResolvedAgentHostShell};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::pty::spawn_diagnostics::{self, ChildLiveness, ExitCause, SpawnRecord, SpawnRecordInit};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::telegram::manager::OutputSenderMap;

// #3029: a single startup transaction, owned by the existing Windows reader.
#[cfg(windows)]
#[derive(Clone, Copy)]
enum CursorQueryState {
    Ground,
    Escape,
    Csi,
    String(bool), // OSC alone also accepts BEL.
    StringEscape(bool),
}

#[cfg(windows)]
struct OneShotCursorQuery {
    state: CursorQueryState,
    pending: Vec<u8>,
    armed: bool,
}

#[cfg(windows)]
impl OneShotCursorQuery {
    fn new() -> Self {
        Self {
            state: CursorQueryState::Ground,
            pending: Vec::with_capacity(3),
            armed: true,
        }
    }

    // The callback sees each preceding segment before the query event and the tail after it.
    fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8], bool)) {
        if !self.armed {
            if !bytes.is_empty() {
                emit(bytes, false);
            }
            return;
        }
        let mut output = Vec::with_capacity(bytes.len() + 3);
        for &byte in bytes {
            if !self.armed {
                output.push(byte);
                continue;
            }
            match (self.pending.len(), byte) {
                (1, b'[') => {
                    self.pending.push(byte);
                    self.state = CursorQueryState::Csi;
                    continue;
                }
                (2, b'6') => {
                    self.pending.push(byte);
                    continue;
                }
                (3, b'n') => {
                    self.pending.clear();
                    self.armed = false;
                    if !output.is_empty() {
                        emit(&output, false);
                        output.clear();
                    }
                    emit(&[], true);
                    continue;
                }
                _ => output.append(&mut self.pending),
            }
            use CursorQueryState::*;
            self.state = match self.state {
                String(_) | StringEscape(_) if byte == 0x18 || byte == 0x1a => Ground,
                String(osc) | StringEscape(osc) if byte == 0x07 && osc => Ground,
                String(osc) if byte == 0x1b => StringEscape(osc),
                StringEscape(_) if byte == 0x5c => Ground,
                StringEscape(osc) if byte == 0x1b => StringEscape(osc),
                String(osc) | StringEscape(osc) => String(osc),
                _ if byte == 0x18 || byte == 0x1a => Ground,
                _ if byte == 0x1b => {
                    self.pending.push(byte);
                    Escape
                }
                Escape => match byte {
                    b']' => String(true),
                    b'P' | b'X' | b'^' | b'_' => String(false),
                    b'[' => Csi,
                    _ => Ground,
                },
                Csi if (0x40..=0x7e).contains(&byte) => Ground,
                Csi => Csi,
                Ground => Ground,
            };
            if byte != 0x1b || matches!(self.state, String(_) | StringEscape(_)) {
                output.push(byte);
            }
        }
        if !output.is_empty() {
            emit(&output, false);
        }
    }

    fn finish(&mut self, mut output: impl FnMut(&[u8])) {
        if !self.pending.is_empty() {
            output(&self.pending);
            self.pending.clear();
        }
    }
}

#[cfg(all(test, windows))]
#[derive(Default)]
struct CursorReplyObserver {
    consumed: AtomicUsize,
    written: AtomicUsize,
    finished: AtomicBool,
}

#[cfg(windows)]
fn write_cursor_reply(
    writer: &std::sync::Weak<Mutex<Box<dyn Write + Send>>>,
    position: Option<(u16, u16)>,
) -> std::io::Result<()> {
    let (row, col) = position.ok_or_else(|| std::io::Error::other("cursor unavailable"))?;
    let writer = writer
        .upgrade()
        .ok_or_else(|| std::io::Error::other("cursor writer retired"))?;
    let mut writer = writer
        .lock()
        .map_err(|_| std::io::Error::other("cursor writer poisoned"))?;
    writer.write_all(format!("\x1b[{row};{col}R").as_bytes())?;
    writer.flush()
}

#[cfg(windows)]
fn pump_cursor_query(
    reader: &mut dyn Read,
    mut raw: impl FnMut(&[u8]) -> bool,
    mut output: impl FnMut(&[u8]),
    mut reply: impl FnMut() -> std::io::Result<()>,
    #[cfg(test)] observer: Option<&CursorReplyObserver>,
) {
    let mut query = OneShotCursorQuery::new();
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if !raw(&buf[..n]) {
                    continue;
                }
                query.feed(&buf[..n], |segment, consumed| {
                    if consumed {
                        #[cfg(test)]
                        if let Some(observer) = observer {
                            observer.consumed.fetch_add(1, Ordering::SeqCst);
                        }
                        match reply() {
                            Ok(()) =>
                            {
                                #[cfg(test)]
                                if let Some(observer) = observer {
                                    observer.written.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                            Err(error) => log::warn!(
                                "[pty] startup cursor reply failed: {error}; \
                                    retaining drainer and Stop"
                            ),
                        }
                    } else {
                        output(segment);
                    }
                });
            }
        }
    }
    query.finish(&mut output);
    #[cfg(test)]
    if let Some(observer) = observer {
        observer.finished.store(true, Ordering::SeqCst);
    }
}

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
    let guard_dir = config_dir.join(GIT_GUARD_DIR_NAME);
    std::fs::create_dir_all(&guard_dir)
        .map_err(|e| AppError::Other(format!("Failed to create git-guard dir: {}", e)))?;

    let cmd_path = guard_dir.join("git.cmd");
    let ps1_path = guard_dir.join("git-guard.ps1");

    let cmd_content = "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \
        \"%~dp0git-guard.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n";
    let ps1_content = concat!(
        r#"$ErrorActionPreference = 'Stop'
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

  if ($arg -eq '--git-dir' -or $arg -like '--git-dir=*' -or $arg -eq "#,
        r#"'--work-tree' -or $arg -like '--work-tree=*') {
    Write-Error 'AgentsCommander git guard: --git-dir and --work-tree are not "#,
        r#"allowed in agent sessions.'
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
  Write-Error ('AgentsCommander git guard: git is only allowed inside repo-* "#,
        r#"directories. Target path: ' + $target)
  exit 1
}

& $realGit @originalArgs
exit $LASTEXITCODE
"#,
    );

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
        .unwrap_or(GIT_GUARD_DIR_NAME);
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
    #[cfg(all(test, windows))]
    cursor_observers: Arc<Mutex<HashMap<Uuid, Arc<CursorReplyObserver>>>>,
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

/// #1498 - Windows-only: `git-bash.exe` (the Git for Windows MinTTY launcher)
/// and `mintty.exe` detach from the console, so a ConPTY host would acquire
/// resources for a child that never attaches to them (blank PTY / detached
/// window). Rejected before any resource acquisition, naming the program and
/// the working alternative. Deliberately does NOT reuse `adapter_error`'s
/// resolved-agent phrasing: this rejection also applies to bare terminals and
/// direct launches, not only to resolved-agent sessions.
fn reject_detached_launcher(program: &str) -> Result<(), AppError> {
    let basename = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    if basename == "git-bash.exe" || basename == "mintty.exe" {
        return Err(AppError::Other(format!(
            "'{}' is a detached launcher and cannot host a ConPTY terminal; \
             use Git for Windows bin\\bash.exe or a direct console shell",
            program
        )));
    }
    Ok(())
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
        (
            host_shell.program.as_str(),
            "configured default-shell program",
        ),
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
        return Err(adapter_error(
            host_program,
            token,
            "a duplicate configured option",
        ));
    }
    seen.push(option.to_string());
    Ok(())
}

// --- Section 4.3.2 - PowerShell products -----------------------------------

const POWERSHELL_FLAG_OPTIONS: &[&str] = &["NoLogo", "NoProfile", "NonInteractive", "Sta", "Mta"];
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
    "Command",
    "c",
    "CommandWithArgs",
    "cwa",
    "File",
    "f",
    "EncodedCommand",
    "enc",
    "e",
    "NoExit",
    "noe",
    "Version",
    "v",
    "Help",
    "h",
    "?",
    "SSHServerMode",
    "ServerMode",
    "WindowsPowerShell",
    "Login",
    "Interactive",
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
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
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
        if let Some(flag) = flags
            .iter()
            .find(|option| option.eq_ignore_ascii_case(name))
        {
            if attached.is_some() {
                return Err(adapter_error(
                    host_program,
                    token,
                    "an unknown or ambiguous token",
                ));
            }
            mark_seen(&mut seen, flag, host_program, token)?;
            continue;
        }
        if let Some(option) = values
            .iter()
            .find(|option| option.eq_ignore_ascii_case(name))
        {
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
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
        }
        return Err(adapter_error(
            host_program,
            token,
            "an unknown or ambiguous token",
        ));
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
            return Err(adapter_error(
                host_program,
                token,
                "an unknown or ambiguous token",
            ));
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
        return Err(adapter_error(
            host_program,
            token,
            "an unknown or ambiguous token",
        ));
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
            "an unsupported cmd payload character (payload-final backslash is doubled by \
                append_quoted)",
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
    let batch_unsupported = args
        .iter()
        .any(|arg| arg.contains('%') || arg.contains('"'));
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
    script.push_str(
        "    try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } \
        catch { exit 1 };\n",
    );
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
    script.push_str(
        "  try { $ac_process = [System.Diagnostics.Process]::Start($ac_start) } \
        catch { exit 1 };\n",
    );
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

/// #1498 - terminal-only flags a POSIX host may keep for a bare terminal but
/// that never reach a resolved-agent launch: the login/interactive flags and
/// the PowerShell `-NoLogo` relic that can ride along in migrated configs
/// (today's Windows default shell argument; #1499 moves fresh defaults to Git
/// Bash). Case-insensitive exact match only: payload and rc flags such as `-c`
/// or `--norc` are NOT terminal-only and stay rejected.
fn is_terminal_only_posix_arg(arg: &str) -> bool {
    matches!(
        arg.to_ascii_lowercase().as_str(),
        "-l" | "--login" | "-i" | "-nologo"
    )
}

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
    // #1498 - bare terminals and direct `.exe` agents: a detached launcher
    // would acquire ConPTY resources for a child that never attaches, so
    // reject before any resource acquisition. The compile-time `cfg!(windows)`
    // guard keeps non-Windows behavior byte-identical (the branch is dead
    // at runtime on non-Windows but still compiled, so the helper must
    // compile on every platform).
    if cfg!(windows) {
        reject_detached_launcher(command)?;
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
    // #1498 - a configured host shell that is a detached launcher would
    // spawn a detached MinTTY child on ConPTY resources; reject before
    // `openpty` and before any spawn accounting.
    reject_detached_launcher(&host_shell.program)?;
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
            // #1498 - terminal-only configured flags (`-l`, `--login`, `-i`,
            // `-NoLogo`) may configure a bare terminal but never reach a
            // resolved-agent launch: the adapter owns only `-c` plus its
            // `exec` payload. Any other configured argument keeps the
            // historical conflicting/terminal rejection, naming the offending
            // token.
            for arg in &host_shell.args {
                if !is_terminal_only_posix_arg(arg) {
                    return Err(adapter_error(
                        &host_shell.program,
                        arg,
                        "a conflicting/terminal configured option (custom shell accepts no \
                            arguments)",
                    ));
                }
            }
            launch_args.clear();
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
            #[cfg(all(test, windows))]
            cursor_observers: Arc::clone(&self.cursor_observers),
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
            #[cfg(all(test, windows))]
            cursor_observers: Arc::new(Mutex::new(HashMap::new())),
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
            launch_witness,
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
        // #2683 - set PATH to the #2589 effective search path, which is also
        // what portable_pty searches to resolve a bare program name. Placement
        // is the precedence rule: the env-removal loop and the configured-env
        // loop below run later, so a configured PATH still wins and a removed
        // PATH is still removed. The Windows git-guard PATH also runs later.
        crate::config::agent_path::apply_search_path_to_pty_command(&mut command);

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
                "[pty] Applied {} configured env vars and removed {} inherited env vars for \
                    session {}",
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
        // #2413 - a child exists from here on; every later `Err` is fenced.
        launch_witness.mark();
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

        #[cfg(windows)]
        let cursor_writer = Arc::downgrade(&instance.writer);
        #[cfg(all(test, windows))]
        let cursor_observer = self.cursor_observers.lock().unwrap().get(&id).cloned();

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
            #[cfg(windows)]
            pump_cursor_query(
                &mut reader,
                |bytes| {
                    let read_at = Instant::now();
                    record.note_output(bytes);
                    fanout.observe_raw_read_for_reader(&output_token, bytes.len(), read_at)
                },
                |bytes| {
                    fanout.handle_output(&output_token, &session_id_str, bytes.to_vec());
                    if !rendered.load(Ordering::Relaxed) && fanout.has_rendered_visible_content(id)
                    {
                        gate_backend.open_startup_gate(id);
                    }
                },
                || {
                    write_cursor_reply(
                        &cursor_writer,
                        fanout.cursor_position_for_reader(&output_token),
                    )
                },
                #[cfg(test)]
                cursor_observer.as_deref(),
            );
            #[cfg(not(windows))]
            {
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
                            // #973 (B) - has the child PAINTED anything yet? Asked of the real
                            // vt100 parser that `handle_output` has just fed this chunk to, which
                            // is why it is asked after it and not before.
                            //
                            // Once the child has painted, this is a single relaxed load per chunk
                            // and nothing else: no lock, no scan, no allocation. Before it has, it
                            // is one uncontended lock and a bounded scan of the grid - and it
                            // REPLACED a second `from_utf8_lossy` + `strip_ansi_csi` over the whole
                            // chunk, which `handle_output` was already paying for the idle detector
                            // on the very same bytes. One less chunk-sized allocation per chunk,
                            // not one more.
                            //
                            // Locks: the query takes `screen_parsers` and RELEASES it before
                            // `open_startup_gate` takes `ptys`. The two are sequential, never
                            // nested, so the order stays `ptys -> startup_gate -> size -> master`.
                            if !rendered.load(Ordering::Relaxed)
                                && fanout.has_rendered_visible_content(id)
                            {
                                gate_backend.open_startup_gate(id);
                            }
                        }
                        Err(_) => break,
                    }
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
                                    "[pty] child-exit session={} pid={:?} cause={} \
                                        detail=observed-at-stop \
                                        child={}",
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
                                "[pty] Failed to kill session {} child pid {:?} after poll error: \
                                    {}",
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
                    "[pty] child-exit session={} pid={:?} cause=ac-requested \
                        detail=reaped-after-stop \
                        child={}",
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
mod tests;
