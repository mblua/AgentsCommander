//! Durable DB-backed send queue for the control-plane API.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::pty::manager::{ResponseCloseState, WorkDeliveryReservation};

mod delivery;
mod lifecycle;
mod locks;
mod migration;
mod pty_actuation;
mod pty_enqueue;
mod pty_offloaded;
mod pty_recovery;
mod rows;
mod types;

pub(crate) use locks::PtyInputTargetOwnership;
#[cfg(test)]
use locks::{sqlite_sidecar_paths, try_stripe_lock_at, verify_existing_sqlite_files};
pub use locks::{
    MessageStoreState, PreparationHeartbeatGuard, PtyInputActiveGuard, PtyInputActiveOperations,
    PtyInputStripeGuard, PtyInputTargetGate, PtyInputTargetGateState, PtyInputTargetGuard,
    PtyInputTargetLocks,
};
pub use rows::retry_backoff_seconds;
pub(crate) use types::DuePtyInputCandidate;
pub use types::{
    ClaimedPtyInputOperation, EnqueueRequest, EnqueueResult, HostPtyInputRejectionRequest,
    LeasedMessage, PtyInputEnqueueRequest, PtyInputEnqueueResult,
};

pub const DB_FILENAME: &str = crate::config::instance_artifacts::MESSAGE_BUS_DB_FILENAME;
pub const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;
pub const DEFAULT_CONTENT_TYPE: &str = "text/markdown";
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_DELIVERING: &str = "delivering";
pub const STATUS_RETRY: &str = "retry";
pub const STATUS_DELIVERED: &str = "delivered";
pub const STATUS_POISONED: &str = "poisoned";

pub const PTY_STATUS_QUEUED: &str = "queued";
pub const PTY_STATUS_PREPARING: &str = "preparing";
pub const PTY_STATUS_RETRY: &str = "retry";
pub const PTY_STATUS_ACTUATING: &str = "actuating";
pub const PTY_STATUS_INJECTED: &str = "injected";
pub const PTY_STATUS_REJECTED: &str = "rejected";
pub const PTY_STATUS_INDETERMINATE: &str = "indeterminate";
pub const PTY_INPUT_MAX_NONTERMINAL_PER_SENDER: i64 = 16;
pub const PTY_INPUT_MAX_NONTERMINAL_GLOBAL: i64 = 512;
pub const PTY_INPUT_MAX_NONTERMINAL_BYTES: i64 = 16 * 1024 * 1024;
pub const PTY_INPUT_OPERATION_LOCK_STRIPES: usize = 4096;
pub const PTY_INPUT_TARGET_LOCK_STRIPES: usize = 1024;

#[derive(Debug, Error)]
pub enum MessageStoreError {
    #[error("config_dir is unavailable")]
    MissingConfigDir,
    #[error("message body exceeds 256 KiB")]
    BodyTooLarge,
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task failed: {0}")]
    BlockingTask(String),
    #[error("idempotency_conflict")]
    IdempotencyConflict,
    #[error("capacity_exceeded")]
    CapacityExceeded,
    #[error("operation_not_found")]
    OperationNotFound,
    #[error("invalid_state_transition")]
    InvalidTransition,
    #[error("actuation_commit_ambiguous")]
    ActuationCommitAmbiguous,
    #[error("store_corrupt")]
    StoreCorrupt,
    #[error("unsafe_store_path")]
    UnsafePath,
}

#[derive(Clone)]
pub struct MessageStore {
    path: PathBuf,
    conn: Arc<Mutex<rusqlite::Connection>>,
    maintenance_cursors: Arc<Mutex<PtyInputMaintenanceCursors>>,
    response_close: Arc<std::sync::OnceLock<ResponseCloseState>>,
    #[cfg(test)]
    test_faults: Arc<Mutex<PtyInputStoreTestFaults>>,
}

/// Opaque delivery ownership; callers cannot reach the tracker through it.
pub(crate) struct MessageDeliveryReservation {
    _inner: Option<WorkDeliveryReservation>,
}

pub(crate) const RESPONSE_CLOSING_DEFERRED: &str = "response_close_deferred";

pub(crate) fn is_response_closing_deferred(error: &str) -> bool {
    matches!(
        error,
        RESPONSE_CLOSING_DEFERRED
            | "PTY error: response_close_deferred"
            | "PTY write failed: PTY error: response_close_deferred"
    )
}

/// A failed or interrupted terminal persistence must not prove settlement.
struct ResponseTerminalPersistence<'a> {
    state: Option<&'a ResponseCloseState>,
    completed: bool,
}

impl Drop for ResponseTerminalPersistence<'_> {
    fn drop(&mut self) {
        if !self.completed {
            if let Some(state) = self.state {
                state.fail_closed();
            }
        }
    }
}

fn response_work_key(plane: &str, sender: &str, op: &str) -> String {
    format!("{}:{}:{}:{}", plane, sender.len(), sender, op)
}

#[derive(Default)]
struct PtyInputMaintenanceCursors {
    runtime_recovery: Option<(String, String)>,
    admission_expiry: Option<(String, String)>,
    compact_before: Option<(String, String)>,
    compact_maintenance: Option<(String, String)>,
    due_container: Option<(String, String, String)>,
}

#[cfg(test)]
#[derive(Default)]
struct PtyInputStoreTestFaults {
    actuation_commit_ambiguous: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests;
