use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::transcript::TranscriptWriter;
use crate::telegram::manager::OutputSenderMap;

struct PtyInstance {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
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

/// Tracks sessions with a pending %%ACRC%% credential injection to prevent duplicates.
pub type AcrcPendingSet = Arc<Mutex<std::collections::HashSet<Uuid>>>;

/// Per-session cooldown to prevent excessive %%ACRC%% injections (e.g. feedback loops).
pub type AcrcCooldownMap = Arc<Mutex<HashMap<Uuid, std::time::Instant>>>;

/// Minimum interval between consecutive ACRC injections for the same session.
const ACRC_COOLDOWN_SECS: u64 = 10;

/// Short cooldown after a failed injection to prevent retry storms
/// when an agent emits %%ACRC%% in a tight loop but injection keeps failing.
const ACRC_FAILURE_COOLDOWN_SECS: u64 = 3;

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
    transcript: TranscriptWriter,
    acrc_pending: AcrcPendingSet,
    acrc_cooldowns: AcrcCooldownMap,
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
        transcript: TranscriptWriter,
    ) -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            output_senders,
            idle_detector,
            git_watcher,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
            ws_broadcaster,
            screen_parsers: Arc::new(Mutex::new(HashMap::new())),
            transcript,
            acrc_pending: Arc::new(Mutex::new(std::collections::HashSet::new())),
            acrc_cooldowns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn spawn(
        &self,
        id: Uuid,
        cmd: &str,
        args: &[String],
        cwd: &str,
        cols: u16,
        rows: u16,
        app_handle: AppHandle,
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
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        // Drop the slave side — we only need the master
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            _child: child,
        };

        self.ptys.lock().unwrap().insert(id, instance);

        // Initialize vt100 screen parser for this session (for WS replay)
        {
            let parser = vt100::Parser::new(rows, cols, 0);
            self.screen_parsers.lock().unwrap().insert(id, parser);
        }

        // Register session for transcript recording (writes to {cwd}/.agentscommander/transcripts/)
        self.transcript.register_session(id, cwd);

        // Spawn read loop that emits PTY output to the frontend,
        // feeds active Telegram bridges, WS clients, and scans for response markers
        let session_id_str = id.to_string();
        let output_senders = self.output_senders.clone();
        let idle_detector = Arc::clone(&self.idle_detector);
        let response_watchers = Arc::clone(&self.response_watchers);
        let ws_broadcaster = self.ws_broadcaster.clone();
        let screen_parsers = Arc::clone(&self.screen_parsers);
        let transcript = self.transcript.clone();
        let acrc_pending = Arc::clone(&self.acrc_pending);
        let acrc_cooldowns = Arc::clone(&self.acrc_cooldowns);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Trailing buffer for detecting %%ACRC%% split across reads (marker is 8 bytes)
            let mut acrc_tail = String::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = buf[..n].to_vec();

                        // Record transcript (agent output)
                        transcript.record_output(id, &data);

                        // Record PTY activity for idle detection
                        idle_detector.record_activity_with_bytes(id, n);

                        // Scan for response markers and credential requests.
                        // Use from_utf8_lossy to prevent silent detection skips
                        // when a multi-byte UTF-8 character is split at the 4096-byte
                        // read boundary. The ACRC marker and response markers are pure
                        // ASCII, so replacement chars (U+FFFD) don't affect detection.
                        let text = String::from_utf8_lossy(&data);
                        if text.contains('\u{FFFD}') {
                            log::debug!(
                                "[PTY] session {} chunk had invalid UTF-8 at buffer boundary ({} bytes, {} replacement chars)",
                                id, n, text.matches('\u{FFFD}').count()
                            );
                        }
                        {
                            scan_response_markers(id, &text, &response_watchers);

                            // Detect %%ACRC%% with cross-buffer support and debounce.
                            // Use line-based matching: only trigger when %%ACRC%% is a
                            // standalone line (trimmed). This prevents false positives
                            // from rendered text that mentions the marker in prose,
                            // search queries, or code references.
                            let scan_text = format!("{}{}", acrc_tail, text);
                            let has_standalone_marker = scan_text
                                .lines()
                                .any(|line| strip_ansi_csi(line).trim() == "%%ACRC%%");
                            if !has_standalone_marker && scan_text.contains("ACRC") {
                                log::debug!("[ACRC] partial match in session {} (not standalone): {:?}",
                                    id, &scan_text[..scan_text.len().min(100)]);
                            }
                            if has_standalone_marker {
                                log::info!("[ACRC] standalone marker detected for session {}", id);
                                log::debug!("[ACRC] scan_text for session {}: {:?}",
                                    id, &scan_text[..scan_text.len().min(100)]);
                                // Cooldown check + pending check + cooldown write
                                // in a single logical block to avoid inconsistency windows.
                                let should_inject = {
                                    let in_cooldown = acrc_cooldowns.lock()
                                        .map(|map| map.get(&id)
                                            .map(|last| last.elapsed().as_secs() < ACRC_COOLDOWN_SECS)
                                            .unwrap_or(false))
                                        .unwrap_or(false);
                                    if in_cooldown {
                                        log::debug!("[ACRC] cooldown active for session {}, skipping", id);
                                        false
                                    } else {
                                        log::info!("[ACRC] cooldown clear for session {}, proceeding", id);
                                        let already_pending = acrc_pending.lock()
                                            .map(|mut set| !set.insert(id))
                                            .unwrap_or(false);
                                        if !already_pending {
                                            // Cooldown is set AFTER successful injection
                                            // (inside the async task) so that failed
                                            // injections don't block retries.
                                            true
                                        } else {
                                            log::info!("[ACRC] already pending for session {}, skipping", id);
                                            false
                                        }
                                    }
                                };
                                if should_inject {
                                    log::info!("[ACRC] spawning inject task for session {}", id);
                                    let app = app_handle.clone();
                                    let pending = Arc::clone(&acrc_pending);
                                    let cooldowns = Arc::clone(&acrc_cooldowns);
                                    tauri::async_runtime::spawn(async move {
                                        // Guard: always clear pending flag, even on panic.
                                        // Without this, a panic in inject_credentials would
                                        // permanently lock out this session from future ACRC.
                                        struct PendingGuard {
                                            pending: AcrcPendingSet,
                                            id: Uuid,
                                        }
                                        impl Drop for PendingGuard {
                                            fn drop(&mut self) {
                                                if let Ok(mut set) = self.pending.lock() {
                                                    set.remove(&self.id);
                                                }
                                            }
                                        }
                                        let _guard = PendingGuard {
                                            pending: Arc::clone(&pending),
                                            id,
                                        };

                                        let success = inject_credentials(&app, id).await;
                                        log::info!("[ACRC] inject task completed for session {} success={}", id, success);
                                        // Set cooldown: full duration on success, short on
                                        // failure to prevent retry storms while still
                                        // allowing prompt retries for transient errors.
                                        if let Ok(mut map) = cooldowns.lock() {
                                            if success {
                                                map.insert(id, std::time::Instant::now());
                                            } else {
                                                let offset = std::time::Duration::from_secs(
                                                    ACRC_COOLDOWN_SECS - ACRC_FAILURE_COOLDOWN_SECS
                                                );
                                                map.insert(id, std::time::Instant::now() - offset);
                                            }
                                        }
                                        // PendingGuard clears pending on drop, but do it
                                        // explicitly here for the log message.
                                        if let Ok(mut set) = pending.lock() {
                                            set.remove(&id);
                                            log::info!("[ACRC] pending flag cleared for session {} (success={})", id, success);
                                        }
                                    });
                                }
                            }
                            // Keep everything after the last newline so that
                            // line-based detection works across buffer splits.
                            // Cap at 512 bytes to bound memory if no newline arrives.
                            acrc_tail = match text.rfind('\n') {
                                Some(i) => text[i..].to_string(),
                                None => {
                                    let combined = format!("{}{}", acrc_tail, text);
                                    if combined.len() > 512 {
                                        // Find nearest valid char boundary to avoid
                                        // panicking on multi-byte UTF-8 sequences.
                                        let target = combined.len() - 512;
                                        let start = (target..combined.len())
                                            .find(|&i| combined.is_char_boundary(i))
                                            .unwrap_or(0);
                                        combined[start..].to_string()
                                    } else {
                                        combined
                                    }
                                }
                            };
                        }

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
            bc.broadcast_event("pty_resized", &serde_json::json!({
                "sessionId": id.to_string(),
                "cols": cols,
                "rows": rows,
            }));
        }

        Ok(())
    }

    pub fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let mut ptys = self.ptys.lock().unwrap();
        // Dropping the PtyInstance will close the master, which signals the child
        ptys.remove(&id);
        self.idle_detector.remove_session(id);
        self.git_watcher.remove_session(id);
        self.transcript.close_session(id);
        if let Ok(mut set) = self.acrc_pending.lock() {
            set.remove(&id);
        }
        if let Ok(mut map) = self.acrc_cooldowns.lock() {
            map.remove(&id);
        }

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

    /// Register a watcher for response markers on a session's output.
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
                let response_content = watcher
                    .buffer
                    .take()
                    .unwrap_or_default()
                    .trim()
                    .to_string();

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

/// Inject session credentials into a PTY in response to a %%ACRC%% marker.
/// Returns `true` if injection succeeded, `false` on any failure.
async fn inject_credentials(app: &AppHandle, session_id: Uuid) -> bool {
    log::info!("[ACRC] inject_credentials START for session {}", session_id);

    // Step 1: Acquire SessionManager read lock
    log::info!("[ACRC] step 1: acquiring SessionManager lock for {}", session_id);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
    let mgr = session_mgr.read().await;
    log::info!("[ACRC] step 1: lock acquired for {}", session_id);

    // Step 2: List sessions and find ours
    let sessions: Vec<crate::session::session::SessionInfo> = mgr.list_sessions().await;
    log::info!("[ACRC] step 2: listed {} sessions, looking for {}", sessions.len(), session_id);

    let session = match sessions.iter().find(|s| s.id == session_id.to_string()) {
        Some(s) => {
            log::info!("[ACRC] step 2: session {} found (name={:?}, cwd={:?})",
                session_id, s.name, s.working_directory);
            s
        }
        None => {
            let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
            log::warn!("[ACRC] session {} NOT FOUND in {} sessions: {:?} — will retry",
                session_id, sessions.len(), ids);
            return false;
        }
    };

    // Step 3: Resolve binary info from current_exe()
    let exe_path = std::env::current_exe().ok();
    let binary_name = exe_path.as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());
    let binary_path = {
        let raw = exe_path.as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "agentscommander.exe".to_string());
        raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
    };
    let local_dir = format!(".{}", &binary_name);
    log::info!("[ACRC] step 3: binary={}, path={}, localDir={}", binary_name, binary_path, local_dir);

    // Step 4: Format credential block
    let cred_block = format!(
        concat!(
            "\n",
            "# === Session Credentials ===\n",
            "# Token: {token}\n",
            "# Root: {root}\n",
            "# Binary: {binary}\n",
            "# BinaryPath: {binary_path}\n",
            "# LocalDir: {local_dir}\n",
            "# === End Credentials ===\n",
        ),
        token = session.token,
        root = session.working_directory,
        binary = binary_name,
        binary_path = binary_path,
        local_dir = local_dir,
    );
    log::info!("[ACRC] step 4: credential block formatted ({} bytes) for session {}", cred_block.len(), session_id);

    drop(mgr);

    // Step 5: Write credential block into PTY
    log::info!("[ACRC] step 5: calling inject_text_into_session for {}", session_id);
    if let Err(e) = crate::pty::inject::inject_text_into_session(
        app,
        session_id,
        &cred_block,
        true, // Claude Code needs explicit Enter (submit=true) to process the injected text
        crate::pty::transcript::InjectReason::TokenRefresh,
        None,
    )
    .await
    {
        log::warn!(
            "[ACRC] step 5 FAILED: inject into PTY for session {} — will retry: {}",
            session_id,
            e
        );
        return false;
    }

    log::info!("[ACRC] credentials injected for session {}", session_id);
    true
}
