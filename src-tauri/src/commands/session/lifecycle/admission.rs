use std::sync::Arc;

use tauri::{AppHandle, Manager};

use super::{LifecycleEntryAuthority, LifecycleTargetLease};

pub(super) async fn acquire_target_lease<R: tauri::Runtime>(
    app: &AppHandle<R>,
    key: Option<String>,
    destination: crate::path_identity::VerifiedPathIdentity,
) -> Result<LifecycleTargetLease, String> {
    Ok(if let Some(key) = key {
        let gate = app
            .try_state::<crate::api::message_store::PtyInputTargetGateState>()
            .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
            .or_else(|| {
                app.try_state::<crate::api::message_store::MessageStoreState>()
                    .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
            })
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let stripe = gate
            .acquire_target_lock(&key)
            .await
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let exact = gate.acquire_exact(&key).await;
        LifecycleTargetLease::Keyed {
            key,
            destination,
            gate,
            stripe,
            exact,
        }
    } else {
        LifecycleTargetLease::VerifiedNoReplica { destination }
    })
}

pub(super) fn source_gate_key(
    authority: &LifecycleEntryAuthority,
) -> Result<Option<String>, String> {
    authority
        .source
        .as_ref()
        .map(|source| {
            crate::config::teams::pty_input_create_gate_key_from_cwd(
                &source.directory.canonical_path,
            )
        })
        .transpose()
        .map(Option::flatten)
}
