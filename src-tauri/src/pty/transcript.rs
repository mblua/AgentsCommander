use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

// ── Entry schema ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    UserKeyboard,
    SystemInject,
    AgentOutput,
    Marker,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectReason {
    InitPrompt,
    TokenRefresh,
    MessageDelivery,
    TelegramInput,
    EnterKeystroke,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Busy,
    Idle,
}

#[derive(Debug, Clone, Serialize)]
pub struct InjectMeta {
    pub reason: InjectReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    pub submit: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarkerMeta {
    pub kind: MarkerKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptEntry {
    pub ts: DateTime<Utc>,
    pub session_id: Uuid,
    pub speaker: Speaker,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inject: Option<InjectMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<MarkerMeta>,
}

// ── TranscriptWriter ────────────────────────────────────────────────

struct SessionTranscript {
    writer: BufWriter<File>,
}

#[derive(Clone)]
pub struct TranscriptWriter {
    inner: Arc<Mutex<HashMap<Uuid, SessionTranscript>>>,
}

impl TranscriptWriter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a session's CWD so transcripts go to
    /// `{cwd}/.agentscommander/transcripts/YYYYMMDD_HHMMSS.jsonl`.
    /// Must be called once per session (typically from PtyManager::spawn).
    pub fn register_session(&self, session_id: Uuid, cwd: &str) {
        let dir = PathBuf::from(cwd)
            .join(".agentscommander")
            .join("transcripts");
        if let Err(e) = fs::create_dir_all(&dir) {
            log::warn!("[transcript] Failed to create transcripts dir for {}: {}", session_id, e);
            return;
        }
        let filename = Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let path = dir.join(format!("{}.jsonl", filename));
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let writer = BufWriter::with_capacity(8192, file);
                self.inner.lock().unwrap().insert(session_id, SessionTranscript { writer });
                log::info!("[transcript] Recording session {} to {}", &session_id.to_string()[..8], path.display());
            }
            Err(e) => {
                log::warn!("[transcript] Failed to open transcript file for {}: {}", session_id, e);
            }
        }
    }

    fn write_entry(&self, entry: &TranscriptEntry) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&entry.session_id) {
            match serde_json::to_string(entry) {
                Ok(json) => {
                    let _ = writeln!(session.writer, "{}", json);
                }
                Err(e) => log::warn!("[transcript] Serialize error: {}", e),
            }
        }
    }

    pub fn flush_session(&self, session_id: Uuid) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&session_id) {
            let _ = session.writer.flush();
        }
    }

    pub fn close_session(&self, session_id: Uuid) {
        let mut map = self.inner.lock().unwrap();
        if let Some(mut session) = map.remove(&session_id) {
            let _ = session.writer.flush();
        }
    }

    // ── Public recording API ────────────────────────────────────────

    fn to_text(data: &[u8]) -> String {
        String::from_utf8_lossy(data).into_owned()
    }

    pub fn record_keyboard(&self, session_id: Uuid, data: &[u8]) {
        self.write_entry(&TranscriptEntry {
            ts: Utc::now(),
            session_id,
            speaker: Speaker::UserKeyboard,
            text: Some(Self::to_text(data)),
            byte_count: Some(data.len()),
            inject: None,
            marker: None,
        });
    }

    pub fn record_inject(
        &self,
        session_id: Uuid,
        data: &[u8],
        reason: InjectReason,
        sender: Option<String>,
        submit: bool,
    ) {
        self.write_entry(&TranscriptEntry {
            ts: Utc::now(),
            session_id,
            speaker: Speaker::SystemInject,
            text: Some(Self::to_text(data)),
            byte_count: Some(data.len()),
            inject: Some(InjectMeta { reason, sender, submit }),
            marker: None,
        });
    }

    pub fn record_output(&self, session_id: Uuid, data: &[u8]) {
        self.write_entry(&TranscriptEntry {
            ts: Utc::now(),
            session_id,
            speaker: Speaker::AgentOutput,
            text: Some(Self::to_text(data)),
            byte_count: Some(data.len()),
            inject: None,
            marker: None,
        });
    }

    pub fn record_marker(&self, session_id: Uuid, kind: MarkerKind) {
        self.write_entry(&TranscriptEntry {
            ts: Utc::now(),
            session_id,
            speaker: Speaker::Marker,
            text: None,
            byte_count: None,
            inject: None,
            marker: Some(MarkerMeta { kind }),
        });
        self.flush_session(session_id);
    }
}
