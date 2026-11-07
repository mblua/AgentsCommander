//! Native state access and effects for co-managed policy.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;

use crate::capture::key::ConsumptionKey;
use crate::capture::registry::CaptureRegistry;
use crate::capture::sink::CaptureSlot;
use crate::capture::state::{AbstainReason, CaptureState, CommittedEffect, EffectPreconditions};
use crate::co_managed::host::CoManagedHost;
use crate::commands::session::selection_executor::turn::NATIVE_SELECTION_TASKS;
use crate::config::co_managed::CoManagedState;
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
use crate::pty::input_activity::SubstantiveInputState;
use crate::session::manager::SessionManager;
use crate::session::selection::task::SelectionTasks;

impl<R: Runtime> CoManagedHost for AppHandle<R> {
    fn sessions_optional(&self) -> Option<&Arc<RwLock<SessionManager>>> {
        self.try_state::<Arc<RwLock<SessionManager>>>()
            .map(|state| state.inner())
    }

    fn sessions_required(&self) -> &Arc<RwLock<SessionManager>> {
        self.state::<Arc<RwLock<SessionManager>>>().inner()
    }

    fn settings_required(&self) -> &SettingsState {
        self.state::<SettingsState>().inner()
    }

    fn input_optional(&self) -> Option<&SubstantiveInputState> {
        self.try_state::<SubstantiveInputState>()
            .map(|state| state.inner())
    }

    fn registry_optional(&self) -> Option<&Arc<CaptureRegistry>> {
        self.try_state::<Arc<CaptureRegistry>>()
            .map(|state| state.inner())
    }

    fn network_required(&self) -> &OutboundNetwork {
        self.state::<OutboundNetwork>().inner()
    }

    fn tasks(&self) -> &dyn SelectionTasks {
        &NATIVE_SELECTION_TASKS
    }

    fn effective_state<'a>(
        &'a self,
        room: &'a Path,
        id: &'a str,
    ) -> BoxFuture<'a, Result<CoManagedState, String>> {
        Box::pin(crate::commands::session::co_managed_effective_state_for_session(self, room, id))
    }

    fn commit_effect(
        &self,
        room_root: PathBuf,
        slot: CaptureSlot,
        expected_seq: u64,
        expected_key: ConsumptionKey,
        pre: EffectPreconditions,
        capture: Arc<CaptureState>,
    ) -> BoxFuture<'static, Result<CommittedEffect, AbstainReason>> {
        Box::pin(async move {
            tauri::async_runtime::spawn_blocking(move || {
                crate::capture::state::commit_effect(
                    &room_root,
                    &slot,
                    expected_seq,
                    &expected_key,
                    &pre,
                    &capture,
                )
            })
            .await
            .unwrap_or(Err(AbstainReason::LockUnavailable(
                "commit task join failed".to_string(),
            )))
        })
    }
}

#[cfg(test)]
mod tests;
