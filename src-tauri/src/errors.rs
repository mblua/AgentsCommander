use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("PTY error: {0}")]
    PtyError(String),

    #[error("No active session")]
    NoActiveSession,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("{0}")]
    Other(String),
}

impl From<AppError> for String {
    fn from(e: AppError) -> String {
        e.to_string()
    }
}
