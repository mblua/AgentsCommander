use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

pub const SESSION_ENV_WARNING_EVENT: &str = "session_env_warning";
pub const WARNING_KIND_PROTOCOL_MISMATCH: &str = "protocol-mismatch";
pub const CONTAINER_TRANSPORT_PROTOCOL_KEY: &str = "CONTAINER_TRANSPORT_PROTOCOL";

const MAX_WARNINGS_PER_SESSION: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionWarning {
    pub session_id: String,
    pub key: String,
    pub kind: String,
    pub message: String,
}

impl SessionWarning {
    pub fn new(
        session_id: Uuid,
        key: impl Into<String>,
        kind: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            key: key.into(),
            kind: kind.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionWarningBuffer {
    pending: HashMap<String, Vec<SessionWarning>>,
}

impl SessionWarningBuffer {
    pub fn push(&mut self, warning: SessionWarning) {
        let session_warnings = self.pending.entry(warning.session_id.clone()).or_default();
        if session_warnings.len() >= MAX_WARNINGS_PER_SESSION {
            session_warnings.remove(0);
        }
        session_warnings.push(warning);
    }

    pub fn drain(&mut self, session_id: Option<&str>) -> Vec<SessionWarning> {
        if let Some(session_id) = session_id {
            return self.pending.remove(session_id).unwrap_or_default();
        }

        let mut drained = Vec::new();
        for (_, mut warnings) in self.pending.drain() {
            drained.append(&mut warnings);
        }
        drained
    }
}

pub type SessionWarningState = Arc<Mutex<SessionWarningBuffer>>;

pub fn new_session_warning_state() -> SessionWarningState {
    Arc::new(Mutex::new(SessionWarningBuffer::default()))
}

pub fn emit_session_warning<R: Runtime>(app: &AppHandle<R>, warning: SessionWarning) {
    log::warn!("{}", warning.message);
    if let Some(state) = app.try_state::<SessionWarningState>() {
        match state.lock() {
            Ok(mut buffer) => buffer.push(warning.clone()),
            Err(err) => {
                log::warn!(
                    "[session-warning] warning buffer lock poisoned; live emit only: {}",
                    err
                );
            }
        }
    } else {
        log::warn!("[session-warning] warning buffer state missing; live emit only");
    }
    let _ = app.emit(SESSION_ENV_WARNING_EVENT, warning);
}

#[tauri::command]
pub fn drain_session_warnings(
    state: State<'_, SessionWarningState>,
    session_id: Option<String>,
) -> Result<Vec<SessionWarning>, String> {
    let mut buffer = state
        .lock()
        .map_err(|err| format!("session warning buffer lock poisoned: {err}"))?;
    Ok(buffer.drain(session_id.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_drains_one_session_without_touching_another() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut buffer = SessionWarningBuffer::default();
        buffer.push(SessionWarning::new(first, "A", "kind", "first"));
        buffer.push(SessionWarning::new(second, "B", "kind", "second"));

        let drained = buffer.drain(Some(&first.to_string()));

        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message, "first");
        assert_eq!(buffer.drain(Some(&second.to_string())).len(), 1);
    }

    #[test]
    fn session_warning_serializes_frontend_contract() {
        let session_id = Uuid::nil();
        let warning = SessionWarning::new(session_id, "KEY", "kind", "message");

        let value = serde_json::to_value(warning).unwrap();

        assert_eq!(value["sessionId"], session_id.to_string());
        assert_eq!(value["key"], "KEY");
        assert_eq!(value["kind"], "kind");
        assert_eq!(value["message"], "message");
    }
}
