use crate::session::maintenance::{Lease, PendingSelfClear, PlainIdentity, PlainReservation};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) const PLAIN_SELF_CLEAR_ACTION: &str = "self-clear";
pub(crate) type Driver = Pin<Box<dyn Future<Output = ()> + Send>>;

pub(crate) struct Request {
    pub identity: PlainIdentity,
    pub root: PathBuf,
    pub local_dir: String,
    pub clear_text: &'static str,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    action: String,
    status: String,
    session_id: Uuid,
    request_id: Uuid,
    requested_by: String,
}

impl Request {
    pub(crate) fn receipt_path(&self) -> PathBuf {
        self.root
            .join(&self.local_dir)
            .join("responses")
            .join(format!("{}.json", self.identity.request_id))
    }

    fn receipt(&self) -> Receipt {
        Receipt {
            action: PLAIN_SELF_CLEAR_ACTION.into(),
            status: "accepted".into(),
            session_id: self.identity.session_id,
            request_id: self.identity.request_id,
            requested_by: self.identity.requested_by.clone(),
        }
    }

    fn validate_receipt(&self, bytes: &[u8]) -> Result<(), String> {
        let receipt: Receipt = serde_json::from_slice(bytes)
            .map_err(|_| "self-clear: corrupt or incompatible receipt".to_string())?;
        if receipt != self.receipt() {
            return Err("self-clear: receipt identity mismatch".into());
        }
        Ok(())
    }
}

pub(crate) fn validate_caller(
    identity: &PlainIdentity,
    canonical_sender: &str,
    to: &str,
    target: Option<&str>,
) -> Result<(), String> {
    if identity.action != PLAIN_SELF_CLEAR_ACTION || identity.requested_by != canonical_sender {
        return Err("self-clear: sender must match the authenticated session".into());
    }
    if !to.is_empty() || target.is_some() {
        return Err("self-clear: to must be empty and target must be absent".into());
    }
    Ok(())
}

pub(crate) struct Prepared<S> {
    pub stage: S,
    pub bytes: Vec<u8>,
}

pub(crate) struct Callbacks<R, P, W, L, T, I, B> {
    pub read_receipt: R,
    pub prepare_receipt: P,
    pub publish_prepared: W,
    pub launch: L,
    pub transport: T,
    pub inject: I,
    pub note_boundary: B,
}

#[derive(Debug)]
pub(crate) enum PlainError {
    Rejected(String),
    Retryable(String),
    Transport(String),
}

pub(crate) async fn accept<S, R, P, W, L, T, TF, I, IF, B, BF>(
    request: Request,
    pending: Arc<PendingSelfClear>,
    callbacks: Callbacks<R, P, W, L, T, I, B>,
) -> Result<(), PlainError>
where
    R: FnMut(&Path) -> Result<Option<Vec<u8>>, String>,
    P: FnOnce(&Path, &[u8], &Lease) -> Result<Prepared<S>, String>,
    W: FnOnce(&S, &Path) -> Result<(), String>,
    L: FnOnce(Driver),
    T: FnOnce() -> TF,
    TF: Future<Output = Result<(), String>>,
    I: FnOnce(Uuid, String) -> IF + Send + 'static,
    IF: Future<Output = Result<(), String>> + Send + 'static,
    B: FnOnce(Uuid) -> BF + Send + 'static,
    BF: Future<Output = ()> + Send + 'static,
{
    let Callbacks {
        mut read_receipt,
        prepare_receipt,
        publish_prepared,
        launch,
        transport,
        inject,
        note_boundary,
    } = callbacks;
    let destination = request.receipt_path();
    if let Some(bytes) = read_receipt(&destination).map_err(PlainError::Rejected)? {
        request
            .validate_receipt(&bytes)
            .map_err(PlainError::Rejected)?;
        return transport().await.map_err(PlainError::Transport);
    }
    let lease = match pending.reserve_plain(request.identity.clone()) {
        PlainReservation::New(lease) => lease,
        PlainReservation::Conflict => {
            return Err(PlainError::Rejected(
                "self-clear: session maintenance conflict".into(),
            ));
        }
        PlainReservation::SamePublishing => {
            return Err(PlainError::Retryable(
                "self-clear: receipt publication pending".into(),
            ));
        }
        PlainReservation::SameArmed => {
            let bytes = read_receipt(&destination)
                .map_err(PlainError::Retryable)?
                .ok_or_else(|| PlainError::Retryable("self-clear: armed without receipt".into()))?;
            request
                .validate_receipt(&bytes)
                .map_err(PlainError::Retryable)?;
            return transport().await.map_err(PlainError::Transport);
        }
    };
    let prepare = || -> Result<Prepared<S>, String> {
        let bytes = serde_json::to_vec_pretty(&request.receipt()).map_err(|e| e.to_string())?;
        let prepared = prepare_receipt(&destination, &bytes, &lease)?;
        if prepared.bytes != bytes {
            return Err("self-clear: stage bytes differ from bound receipt".into());
        }
        request.validate_receipt(&prepared.bytes)?;
        Ok(prepared)
    };
    let prepared = match prepare() {
        Ok(stage) => stage,
        Err(error) => {
            pending.release(&lease);
            return Err(PlainError::Rejected(error));
        }
    };
    let driver_pending = Arc::clone(&pending);
    let driver_lease = lease.clone();
    let sid = request.identity.session_id;
    let text = request.clear_text.to_string();
    let driver: Driver = Box::pin(async move {
        match inject(sid, text).await {
            Ok(()) => note_boundary(sid).await,
            Err(error) => log::warn!("[mailbox] self-clear attempt for {} failed: {}", sid, error),
        }
        driver_pending.release(&driver_lease);
    });
    if let Err(error) = pending.arm_plain(&lease) {
        pending.release(&lease);
        return Err(PlainError::Rejected(error.to_string()));
    }
    // Synchronous arm -> no-clobber rename -> launch; no fallible post-rename operation.
    if let Err(error) = publish_prepared(&prepared.stage, &destination) {
        pending.release(&lease);
        return Err(PlainError::Rejected(error));
    }
    launch(driver);
    transport().await.map_err(PlainError::Transport)
}

#[cfg(test)]
mod tests;
