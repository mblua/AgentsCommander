use std::path::PathBuf;
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

#[derive(Error, Debug)]
pub enum StartupError {
    #[error("cannot determine the authoritative config directory for executable {executable:?}")]
    MissingConfigDir { executable: Option<PathBuf> },

    #[error("{operation} failed at {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{operation} refused unsafe path {}: {reason}", path.display())]
    UnsafePath {
        operation: &'static str,
        path: PathBuf,
        reason: UnsafePathReason,
    },

    #[error(
        "{operation} reached secure state before Linux config preparation at {}",
        path.display()
    )]
    SecureStateNotPrepared {
        operation: &'static str,
        path: PathBuf,
    },

    #[error(
        "coding-agent settings mutation is busy at {}; retry the command",
        path.display()
    )]
    MutationBusy { path: PathBuf },

    #[error(
        "Linux instance-lock state is inconsistent: mutation lock {} was free while GUI lock {} was held",
        mutation_path.display(),
        gui_path.display()
    )]
    LockStateInconsistent {
        mutation_path: PathBuf,
        gui_path: PathBuf,
    },

    #[error(
        "{operation} may have committed but final identity could not be proven at {}",
        path.display()
    )]
    PublicationAmbiguous {
        operation: &'static str,
        path: PathBuf,
    },

    #[error("logger installation failed: {message}")]
    LoggerInstall { message: String },

    #[error("{component} initialization failed: {message}")]
    Initialization {
        component: &'static str,
        message: String,
    },

    #[error("Tauri application build failed: {source}")]
    TauriBuild {
        #[source]
        source: tauri::Error,
    },

    #[error("Tauri setup failed: {message}")]
    TauriSetup { message: String },

    #[error(
        "{primary}; rollback diagnostics: {diagnostics}",
        diagnostics = diagnostics.join("; ")
    )]
    Rollback {
        primary: Box<StartupError>,
        diagnostics: Vec<String>,
    },
}

impl StartupError {
    pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    pub fn unsafe_path(
        operation: &'static str,
        path: impl Into<PathBuf>,
        reason: UnsafePathReason,
    ) -> Self {
        Self::UnsafePath {
            operation,
            path: path.into(),
            reason,
        }
    }

    pub fn with_rollback_diagnostics(self, diagnostics: Vec<String>) -> Self {
        if diagnostics.is_empty() {
            self
        } else {
            Self::Rollback {
                primary: Box::new(self),
                diagnostics,
            }
        }
    }
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum UnsafePathReason {
    #[error("symbolic links are not allowed")]
    Symlink,

    #[error("expected {expected}, observed {observed}")]
    WrongObjectType {
        expected: &'static str,
        observed: &'static str,
    },

    #[error("foreign owner: expected UID {expected}, observed UID {observed}")]
    ForeignOwner { expected: u32, observed: u32 },

    #[error("regular file has {observed} hard links; exactly one is required")]
    HardLinked { observed: u64 },

    #[error("opened handle and directory entry changed identity")]
    IdentityChanged,

    #[error(
        "resolved parent is not trusted: observed UID {observed_uid}, mode {observed_mode:#06o}"
    )]
    UntrustedParent {
        observed_uid: u32,
        observed_mode: u32,
    },

    #[error("invalid security-bearing basename")]
    InvalidBasename,
}
