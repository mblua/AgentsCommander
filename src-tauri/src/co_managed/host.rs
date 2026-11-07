//! Typed capabilities for co-managed policy and its native effects.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::capture::key::ConsumptionKey;
use crate::capture::registry::CaptureRegistry;
use crate::capture::sink::CaptureSlot;
use crate::capture::state::{AbstainReason, CaptureState, CommittedEffect, EffectPreconditions};
use crate::config::co_managed::CoManagedState;
use crate::config::settings::SettingsState;
use crate::network::OutboundNetwork;
use crate::pty::input_activity::SubstantiveInputState;
use crate::session::manager::SessionManager;
use crate::session::selection::task::SelectionTasks;
use crate::session::selection::SelectionEvents;

pub(crate) trait CoManagedEvents: Send + Sync {
    #[allow(dead_code)] // Consumed by P15.
    fn co_managed_event(&self, event: &'static str, value: Value);
}

pub(crate) trait CoManagedHost: Send + Sync + CoManagedEvents + SelectionEvents {
    #[allow(dead_code)] // Consumed by P15.
    fn sessions_optional(&self) -> Option<&Arc<RwLock<SessionManager>>>;
    #[allow(dead_code)] // Consumed by P16.
    fn sessions_required(&self) -> &Arc<RwLock<SessionManager>>;
    #[allow(dead_code)] // Consumed by P15.
    fn settings_required(&self) -> &SettingsState;
    #[allow(dead_code)] // Consumed by P16.
    fn input_optional(&self) -> Option<&SubstantiveInputState>;
    #[allow(dead_code)] // Consumed by P16.
    fn registry_optional(&self) -> Option<&Arc<CaptureRegistry>>;
    #[allow(dead_code)] // Consumed by P16.
    fn network_required(&self) -> &OutboundNetwork;
    #[allow(dead_code)] // Consumed by P16.
    fn tasks(&self) -> &dyn SelectionTasks;
    #[allow(dead_code)] // Consumed by P16.
    fn effective_state<'a>(
        &'a self,
        room: &'a Path,
        id: &'a str,
    ) -> BoxFuture<'a, Result<CoManagedState, String>>;
    #[allow(dead_code)] // Consumed by P16.
    fn commit_effect(
        &self,
        room_root: PathBuf,
        slot: CaptureSlot,
        expected_seq: u64,
        expected_key: ConsumptionKey,
        pre: EffectPreconditions,
        capture: Arc<CaptureState>,
    ) -> BoxFuture<'static, Result<CommittedEffect, AbstainReason>>;
}
