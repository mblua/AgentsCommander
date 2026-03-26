use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
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

pub struct PtyManager {
    ptys: Arc<Mutex<HashMap<Uuid, PtyInstance>>>,
    output_senders: OutputSenderMap,
    idle_detector: Arc<IdleDetector>,
    git_watcher: Arc<GitWatcher>,
    pub response_watchers: ResponseWatcherMap,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PtyOutputPayload {
    session_id: String,
    data: Vec<u8>,
}

impl PtyManager {
    pub fn new(output_senders: OutputSenderMap, idle_detector: Arc<IdleDetector>, git_watcher: Arc<GitWatcher>) -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            output_senders,
            idle_detector,
            git_watcher,
            response_watchers: Arc::new(Mutex::new(HashMap::new())),
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

        // Spawn read loop that emits PTY output to the frontend,
        // feeds active Telegram bridges, and scans for response markers
        let session_id_str = id.to_string();
        let output_senders = self.output_senders.clone();
        let idle_detector = Arc::clone(&self.idle_detector);
        let response_watchers = Arc::clone(&self.response_watchers);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = buf[..n].to_vec();

                        // Record PTY activity for idle detection
                        idle_detector.record_activity_with_bytes(id, n);

                        // Scan for response markers
                        if let Ok(text) = std::str::from_utf8(&data) {
                            scan_response_markers(id, text, &response_watchers);
                        }

                        // Feed Telegram bridge if active (non-blocking)
                        if let Ok(senders) = output_senders.lock() {
                            if let Some(tx) = senders.get(&id) {
                                let _ = tx.try_send(data.clone());
                            }
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

        Ok(())
    }

    pub fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let mut ptys = self.ptys.lock().unwrap();
        // Dropping the PtyInstance will close the master, which signals the child
        ptys.remove(&id);
        self.idle_detector.remove_session(id);
        self.git_watcher.remove_session(id);

        // Clean up any response watchers for this session
        if let Ok(mut watchers) = self.response_watchers.lock() {
            watchers.retain(|(sid, _), _| *sid != id);
        }

        Ok(())
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
