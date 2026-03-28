use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use uuid::Uuid;

// ── Types (kept for caller API) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub enum InjectReason {
    InitPrompt,
    TokenRefresh,
    MessageDelivery,
    TelegramInput,
    EnterKeystroke,
}

impl InjectReason {
    fn label(&self) -> &'static str {
        match self {
            Self::InitPrompt => "init_prompt",
            Self::TokenRefresh => "token_refresh",
            Self::MessageDelivery => "message_delivery",
            Self::TelegramInput => "telegram_input",
            Self::EnterKeystroke => "enter",
        }
    }
}

#[derive(Debug, Clone)]
pub enum MarkerKind {
    Busy,
    Idle,
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

    /// Register a session. Writes a header then records all I/O to
    /// `{cwd}/.agentscommander/transcripts/YYYYMMDD_HHMMSS.log`.
    pub fn register_session(&self, session_id: Uuid, cwd: &str) {
        let dir = PathBuf::from(cwd)
            .join(".agentscommander")
            .join("transcripts");
        if let Err(e) = fs::create_dir_all(&dir) {
            log::warn!("[transcript] Failed to create transcripts dir for {}: {}", session_id, e);
            return;
        }
        let now = Utc::now();
        let filename = now.format("%Y%m%d_%H%M%S").to_string();
        let path = dir.join(format!("{}.log", filename));
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let mut writer = BufWriter::with_capacity(8192, file);
                // Write header with one-time metadata
                let _ = writeln!(writer, "# Transcript — {}", now.format("%Y-%m-%d %H:%M:%S UTC"));
                let _ = writeln!(writer, "# session: {}", session_id);
                let _ = writeln!(writer, "# cwd: {}", cwd);
                let _ = writeln!(writer, "#");
                self.inner.lock().unwrap().insert(session_id, SessionTranscript { writer });
                log::info!("[transcript] Recording session {} to {}", &session_id.to_string()[..8], path.display());
            }
            Err(e) => {
                log::warn!("[transcript] Failed to open transcript file for {}: {}", session_id, e);
            }
        }
    }

    fn write_line(&self, session_id: Uuid, line: &str) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&session_id) {
            let _ = writeln!(session.writer, "{}", line);
        }
    }

    fn ts() -> String {
        Utc::now().format("%H:%M:%S").to_string()
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

    pub fn record_keyboard(&self, session_id: Uuid, data: &[u8]) {
        let text = String::from_utf8_lossy(data);
        self.write_line(session_id, &format!("[{}] USER: {}", Self::ts(), text));
    }

    pub fn record_inject(
        &self,
        session_id: Uuid,
        data: &[u8],
        reason: InjectReason,
        sender: Option<String>,
        _submit: bool,
    ) {
        let text = String::from_utf8_lossy(data);
        let tag = match &sender {
            Some(s) => format!("INJECT({}, from=\"{}\")", reason.label(), s),
            None => format!("INJECT({})", reason.label()),
        };
        self.write_line(session_id, &format!("[{}] {}: {}", Self::ts(), tag, text));
    }

    pub fn record_output(&self, session_id: Uuid, data: &[u8]) {
        let text = String::from_utf8_lossy(data);
        self.write_line(session_id, &format!("[{}] AGENT: {}", Self::ts(), text));
    }

    pub fn record_marker(&self, session_id: Uuid, kind: MarkerKind) {
        let label = match kind {
            MarkerKind::Busy => "busy",
            MarkerKind::Idle => "idle",
        };
        self.write_line(session_id, &format!("[{}] -- {} --", Self::ts(), label));
        self.flush_session(session_id);
    }
}
