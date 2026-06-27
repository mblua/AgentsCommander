use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::resource_monitor::ResourceLaunchRegistration;
use crate::session::profile::IdleTuning;
use crate::telegram::manager::OutputSenderMap;

struct PtyInstance {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    /// #632 - Job Object the child (and its descendant tree) is assigned to, when
    /// the OS allowed it. `None` falls back to the identity reaper for cleanup.
    job: Option<crate::pty::job::JobObject>,
}

/// Tracks active response marker watchers per session.
/// Key: (session_id, request_id) → accumulated output buffer.
/// The read loop scans for %%AC_RESPONSE::<rid>::START/END%% markers.
pub type ResponseWatcherMap = Arc<Mutex<HashMap<(Uuid, String), ResponseWatcher>>>;

pub struct ResponseWatcher {
    /// Where to write the extracted response
    pub response_dir: std::path::PathBuf,
    /// Buffer accumulating output between START and END markers
    pub buffer: Option<String>,
    /// Whether we've seen the START marker
    pub capturing: bool,
}

#[cfg(windows)]
struct GitGuardEnv {
    path: String,
    pathext: String,
    real_git: String,
}

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

    std::fs::write(&cmd_path, cmd_content)
        .map_err(|e| AppError::Other(format!("Failed to write git.cmd guard: {}", e)))?;
    std::fs::write(&ps1_path, ps1_content)
        .map_err(|e| AppError::Other(format!("Failed to write git-guard.ps1: {}", e)))?;

    Ok(guard_dir)
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

pub struct PtyManager {
    ptys: Arc<Mutex<HashMap<Uuid, PtyInstance>>>,
    output_senders: OutputSenderMap,
    idle_detector: Arc<IdleDetector>,
    git_watcher: Arc<GitWatcher>,
    pub response_watchers: ResponseWatcherMap,
    /// Optional WS broadcaster for remote access
    ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    /// VT100 screen state per session for replay to late-joining WS clients
    screen_parsers: Arc<Mutex<HashMap<Uuid, vt100::Parser>>>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
}

/// Strip ANSI escape sequences so that marker detection is not fooled
/// by terminal color/cursor codes. Handles:
/// - CSI sequences: ESC [ ... final_byte (colors, cursor, SGR)
/// - OSC sequences: ESC ] ... BEL/ST (title, hyperlinks, shell integration)
/// - DCS sequences: ESC P ... ST (device control strings)
/// - Non-CSI two-byte escapes: ESC + one byte (resets, keypad mode)
fn strip_ansi_csi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // skip '['
                                  // CSI: skip parameter/intermediate bytes until final byte (0x40..=0x7E)
                    while let Some(&next) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next(); // skip ']'
                                  // OSC: consume until BEL (\x07) or ST (ESC \)
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x07' {
                            chars.next();
                            break;
                        }
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break; // proper ST terminator
                            }
                            // ESC not followed by \ — not ST, keep consuming
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(&'P') => {
                    chars.next(); // skip 'P'
                                  // DCS: consume until ST (ESC \)
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                                break; // proper ST terminator
                            }
                            // ESC not followed by \ — not ST, keep consuming
                            continue;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    // Non-CSI two-byte escape (e.g. ESC c, ESC M)
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl PtyManager {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            output_senders,
            idle_detector,
            git_watcher,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
            ws_broadcaster,
            screen_parsers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // PTY spawn requires the full set of session knobs at once; splitting into a
    // builder would just add ceremony for no reuse.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn<R: tauri::Runtime>(
        &self,
        id: Uuid,
        cmd: &str,
        args: &[String],
        cwd: &str,
        cols: u16,
        rows: u16,
        configured_env: &[(String, String)],
        env_remove_keys: &[String],
        extra_env: &[(String, String)],
        idle_tuning: IdleTuning,
        app_handle: AppHandle<R>,
        mut resource_registration: Option<ResourceLaunchRegistration>,
    ) -> Result<(), AppError> {
        let pty_system = native_pty_system();

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        // On Windows, non-.exe commands (like .cmd, .bat, or bare names that
        // resolve to .cmd scripts) need to be wrapped with cmd.exe /C so the
        // shell can resolve them from PATH.
        let is_direct_exe = cmd.to_lowercase().ends_with(".exe")
            || std::path::Path::new(cmd)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));

        let mut command = if cfg!(windows) && !is_direct_exe {
            let mut c = CommandBuilder::new("cmd.exe");
            c.arg("/C");
            c.arg(cmd);
            for arg in args {
                c.arg(arg);
            }
            c
        } else {
            let mut c = CommandBuilder::new(cmd);
            for arg in args {
                c.arg(arg);
            }
            c
        };
        command.cwd(cwd);
        for key in env_remove_keys {
            command.env_remove(key);
        }
        for (key, value) in configured_env {
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
        crate::pty::credentials::apply_credential_env_to_pty_command(&mut command, extra_env);
        command.env("TERM", "xterm-256color");

        if !extra_env.is_empty() {
            log::info!(
                "[pty] Applied {} per-process credential environment variables for session {}",
                extra_env.len(),
                id
            );
        }

        if let Some(git_ceiling_dirs) =
            crate::config::session_context::git_ceiling_directories_for_session_root(cwd)
        {
            command.env("GIT_CEILING_DIRECTORIES", &git_ceiling_dirs);
            log::info!(
                "[pty] Applied GIT_CEILING_DIRECTORIES for session cwd {}: {}",
                cwd,
                git_ceiling_dirs
            );

            #[cfg(windows)]
            if let Some(git_guard_env) = build_git_guard_env()? {
                command.env("PATH", &git_guard_env.path);
                command.env("PATHEXT", &git_guard_env.pathext);
                command.env("AC_REAL_GIT", &git_guard_env.real_git);
                log::info!("[pty] Enabled git guard wrapper for session cwd {}", cwd);
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

        // #632 - assign the child (and the descendant tree it spawns after this
        // point) to a Job Object with KILL_ON_JOB_CLOSE, so session-destroy and
        // app-exit can kill the whole tree atomically via the job handle. Created
        // before the resource registration so the early-return error paths below
        // drop `job` and let KILL_ON_JOB_CLOSE reap the child too.
        let job = child.process_id().and_then(crate::pty::job::JobObject::for_child);

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

        // Drop the slave side — we only need the master
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

        // #260 — register with the idle detector. Seeds activity[id] (when the
        // profile opts in) so an all-suppressed / escape-only session is still
        // evaluated by the watcher. Must run before the read loop starts.
        self.idle_detector.register_session(id, idle_tuning);

        // Initialize vt100 screen parser for this session (for WS replay)
        {
            let parser = vt100::Parser::new(rows, cols, 0);
            self.screen_parsers.lock().unwrap().insert(id, parser);
        }

        // Spawn read loop that emits PTY output to the frontend,
        // feeds active Telegram bridges, WS clients, and scans for response markers
        let session_id_str = id.to_string();
        let output_senders = self.output_senders.clone();
        let idle_detector = Arc::clone(&self.idle_detector);
        let response_watchers = Arc::clone(&self.response_watchers);
        let ws_broadcaster = self.ws_broadcaster.clone();
        let screen_parsers = Arc::clone(&self.screen_parsers);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = buf[..n].to_vec();

                        // #552 auto-close silence clock: ANY PTY output proves the
                        // session is alive, so it resets the silence timer. This is
                        // DELIBERATELY broader than the idle-dot activity signal below
                        // (which ignores escape-only spinner output per #260), so a
                        // silent-but-spinning agent is NOT auto-closed mid-work. The
                        // only case this does NOT cover is a foreground command that
                        // emits literally zero terminal bytes for the whole timeout
                        // (documented residual; see plan §7 H2).
                        idle_detector.touch_silence(id);

                        // Scan for response markers.
                        // Use from_utf8_lossy to prevent silent detection skips
                        // when a multi-byte UTF-8 character is split at the 4096-byte
                        // read boundary. Response markers are pure ASCII, so
                        // replacement chars (U+FFFD) don't affect detection.
                        let text = String::from_utf8_lossy(&data);
                        if text.contains('\u{FFFD}') {
                            log::debug!(
                                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} replacement chars)",
                                id, n, text.matches('\u{FFFD}').count()
                            );
                        }

                        // Record PTY activity for idle detection — but only if the
                        // output contains meaningful visible content. Terminal escape
                        // sequences (cursor moves, title updates, color resets, prompt
                        // redraws) are NOT user/agent activity and must not flip the
                        // session to busy. Strip ANSI escapes and check for printable
                        // characters above ASCII space.
                        let is_printable = |c: char| c > ' ' && c != '\u{FFFD}';
                        let has_printable = if text.contains('\x1b') {
                            strip_ansi_csi(&text).chars().any(is_printable)
                        } else {
                            text.chars().any(is_printable)
                        };
                        if has_printable {
                            idle_detector.record_activity_with_bytes(id, n);
                        } else {
                            log::trace!(
                                "[idle] SKIPPED activity for {} ({} bytes, escape-only output)",
                                &id.to_string()[..8],
                                n
                            );
                        }
                        scan_response_markers(id, &text, &response_watchers);

                        // Feed Telegram bridge if active (non-blocking)
                        if let Ok(senders) = output_senders.lock() {
                            if let Some(tx) = senders.get(&id) {
                                let _ = tx.try_send(data.clone());
                            }
                        }

                        // Feed vt100 screen parser for WS replay
                        if let Ok(mut parsers) = screen_parsers.lock() {
                            if let Some(parser) = parsers.get_mut(&id) {
                                parser.process(&data);
                            }
                        }

                        // Broadcast to WebSocket clients (non-blocking)
                        if let Some(ref bc) = ws_broadcaster {
                            bc.broadcast_pty_output(&session_id_str, &data);
                        }

                        let payload = PtyOutputPayload {
                            session_id: session_id_str.clone(),
                            data,
                        };
                        let _ = app_handle.emit("pty_output", payload);
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    pub fn write(&self, id: Uuid, data: &[u8]) -> Result<(), AppError> {
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

    /// Returns true if this PtyManager holds a live PtyInstance for `id`.
    ///
    /// Used by the mailbox router to filter out SessionManager records whose
    /// PTY entry has gone missing (a desync that produces phantom Best-match
    /// candidates — see issue #223). A live PtyInstance does NOT guarantee the
    /// underlying child process is alive; it only guarantees that a subsequent
    /// `write` will NOT fail with `AppError::SessionNotFound`. Detecting a dead
    /// child of a live PtyInstance is out of scope for this accessor (issue #223
    /// follow-up: state-machine PTY-exit hook).
    pub fn has_session(&self, id: Uuid) -> bool {
        self.ptys.lock().unwrap().contains_key(&id)
    }

    pub fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        // Tell idle detector to ignore PTY output caused by this resize
        self.idle_detector.record_resize(id);

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

        // Keep the vt100 screen parser in sync so snapshots match the new size
        if let Ok(mut parsers) = self.screen_parsers.lock() {
            if let Some(parser) = parsers.get_mut(&id) {
                parser.set_size(rows, cols);
            }
        }

        // Broadcast resize to WS clients so browser mirrors can update dimensions
        if let Some(ref bc) = self.ws_broadcaster {
            bc.broadcast_event(
                "pty_resized",
                &serde_json::json!({
                    "sessionId": id.to_string(),
                    "cols": cols,
                    "rows": rows,
                }),
            );
        }

        Ok(())
    }

    pub fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let instance = {
            let mut ptys = self.ptys.lock().unwrap();
            ptys.remove(&id)
        };

        if let Some(mut instance) = instance {
            // #632 - kill the whole descendant tree via the Job Object first. This
            // catches descendants the identity reaper's #543 gate refuses to
            // terminate (recycled / elevated PIDs) and any spawn-race escapee, with
            // no snapshot walk and no 2s waits. The child handling below then reaps
            // the (now-dead) direct child's exit status as before.
            if let Some(job) = instance.job.take() {
                job.terminate();
                // job drops here -> handle closed (KILL_ON_JOB_CLOSE backstop)
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

        self.idle_detector.remove_session(id);
        self.git_watcher.remove_session(id);

        // Clean up any response watchers for this session
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.retain(|(sid, _), _| *sid != id);
        }

        // Clean up vt100 screen parser
        if let Ok(mut parsers) = self.screen_parsers.lock() {
            parsers.remove(&id);
        }

        Ok(())
    }

    /// #647 A: fire the Job Object tree-kill for a session WITHOUT tearing the
    /// session down. Pure: no `ptys.remove`, no child reap, no idle/git/watcher
    /// teardown, and the job handle is RETAINED so a Force/Retry can re-fire it.
    /// Returns true if a live job was present and `terminate()` was called.
    /// `TerminateJobObject` is idempotent, so repeated calls on a dead tree are safe.
    /// On non-Windows builds the job is always `None`, so this returns false and the
    /// caller falls back to the per-process reaper.
    pub fn terminate_job_for_session(&self, id: Uuid) -> bool {
        let ptys = self.ptys.lock().unwrap();
        match ptys.get(&id).and_then(|inst| inst.job.as_ref()) {
            Some(job) => {
                job.terminate();
                true
            }
            None => false,
        }
    }

    /// #632 - terminate every live agent's Job Object at app shutdown. Atomically
    /// kills each session's whole descendant tree via the job handle (immune to PID
    /// reuse / ACCESS_DENIED, no snapshot walking, no per-process waits). Returns
    /// `(jobs_terminated, jobless_sessions)` so the caller can detect the MED-2
    /// case: a session with no Job Object is reaper-only and can orphan if the
    /// time-boxed reaper does not finish. Does NOT remove the PtyInstances; the
    /// process is exiting and the OS reclaims the rest. No-op on non-Windows (jobs
    /// are always `None`, so it returns `(0, live_session_count)`).
    pub fn kill_all_jobs(&self) -> (usize, usize) {
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

    /// Get a screen snapshot for replay to late-joining WS clients.
    /// Returns the visible screen content as raw bytes that can be written to xterm.js.
    pub fn get_screen_snapshot(&self, id: Uuid) -> Option<Vec<u8>> {
        let parsers = self.screen_parsers.lock().ok()?;
        let parser = parsers.get(&id)?;
        let screen = parser.screen();
        Some(screen.contents_formatted())
    }

    /// Get the current PTY dimensions (rows, cols) from the vt100 parser.
    pub fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        let parsers = self.screen_parsers.lock().ok()?;
        let parser = parsers.get(&id)?;
        Some(parser.screen().size())
    }

    pub fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.insert(
                (session_id, request_id),
                ResponseWatcher {
                    response_dir,
                    buffer: None,
                    capturing: false,
                },
            );
        }
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

/// Scan PTY output text for %%AC_RESPONSE::<rid>::START/END%% markers.
/// This runs on the PTY read thread — must be fast and non-blocking.
fn scan_response_markers(session_id: Uuid, text: &str, watchers: &ResponseWatcherMap) {
    let Ok(mut watchers) = watchers.lock() else {
        return;
    };

    // Collect keys that match this session
    let keys: Vec<(Uuid, String)> = watchers
        .keys()
        .filter(|(sid, _)| *sid == session_id)
        .cloned()
        .collect();

    for key in keys {
        let (_, ref rid) = key;
        let start_marker = format!("%%AC_RESPONSE::{}::START%%", rid);
        let end_marker = format!("%%AC_RESPONSE::{}::END%%", rid);

        let watcher = match watchers.get_mut(&key) {
            Some(w) => w,
            None => continue,
        };

        if watcher.capturing {
            // We're between START and END — accumulate
            if let Some(end_pos) = text.find(&end_marker) {
                // Found END marker — extract final content
                let chunk = &text[..end_pos];
                if let Some(ref mut buf) = watcher.buffer {
                    buf.push_str(chunk);
                }

                // Write the response file
                let response_content = watcher.buffer.take().unwrap_or_default().trim().to_string();

                let response_path = watcher.response_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::create_dir_all(&watcher.response_dir) {
                    log::warn!("Failed to create responses dir: {}", e);
                }

                let response_json = serde_json::json!({
                    "requestId": rid,
                    "content": response_content,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });

                match serde_json::to_string_pretty(&response_json) {
                    Ok(json) => {
                        if let Err(e) = std::fs::write(&response_path, json) {
                            log::warn!("Failed to write response file: {}", e);
                        } else {
                            log::info!(
                                "Captured response for request {} from session {}",
                                rid,
                                session_id
                            );
                        }
                    }
                    Err(e) => log::warn!("Failed to serialize response: {}", e),
                }

                // Remove this watcher — response captured
                watchers.remove(&key);
                return; // Key removed, skip further processing
            } else {
                // No END yet — accumulate everything
                if let Some(ref mut buf) = watcher.buffer {
                    buf.push_str(text);
                }
            }
        } else if let Some(start_pos) = text.find(&start_marker) {
            // Found START marker
            watcher.capturing = true;
            let after_start = &text[start_pos + start_marker.len()..];

            // Check if END is also in this chunk
            if let Some(end_pos) = after_start.find(&end_marker) {
                let content = after_start[..end_pos].trim().to_string();
                let response_path = watcher.response_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::create_dir_all(&watcher.response_dir) {
                    log::warn!("Failed to create responses dir: {}", e);
                }

                let response_json = serde_json::json!({
                    "requestId": rid,
                    "content": content,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });

                match serde_json::to_string_pretty(&response_json) {
                    Ok(json) => {
                        if let Err(e) = std::fs::write(&response_path, json) {
                            log::warn!("Failed to write response file: {}", e);
                        } else {
                            log::info!(
                                "Captured response for request {} from session {}",
                                rid,
                                session_id
                            );
                        }
                    }
                    Err(e) => log::warn!("Failed to serialize response: {}", e),
                }

                watchers.remove(&key);
                return;
            } else {
                watcher.buffer = Some(after_start.to_string());
            }
        }
    }
}
