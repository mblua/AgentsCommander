use std::fmt;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::session::session::SessionStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionSource {
    InitialHydration,
    SessionCreated,
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart,
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor,
    BackgroundCleanup,
    LivenessReconcile,
}

impl fmt::Display for SelectionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::InitialHydration => "initialHydration",
            Self::SessionCreated => "sessionCreated",
            Self::UserSwitch => "userSwitch",
            Self::ManualClose => "manualClose",
            Self::AutoClose => "autoClose",
            Self::Restart => "restart",
            Self::Restore => "restore",
            Self::Detach => "detach",
            Self::Attach => "attach",
            Self::SpawnRollback => "spawnRollback",
            Self::ResourceMonitor => "resourceMonitor",
            Self::BackgroundCleanup => "backgroundCleanup",
            Self::LivenessReconcile => "livenessReconcile",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    None,
    Live,
    Dormant,
}

impl fmt::Display for SelectionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Live => f.write_str("live"),
            Self::Dormant => f.write_str("dormant"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSelection {
    pub(super) epoch: String,
    pub(super) source: SelectionSource,
    pub(super) user_initiated: bool,
    pub(super) revision: u64,
    pub(super) mode: SelectionMode,
    pub(super) id: Option<String>,
    pub(super) status: Option<SessionStatus>,
    pub(super) has_pty: bool,
    pub(super) detached: bool,
    pub(super) displayable: bool,
}

impl SessionSelection {
    pub(in crate::session) fn initial(epoch: Uuid) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: SelectionSource::InitialHydration,
            user_initiated: false,
            revision: 0,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(in crate::session) fn none(epoch: Uuid, revision: u64, cause: SelectionCause) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::None,
            id: None,
            status: None,
            has_pty: false,
            detached: false,
            displayable: false,
        }
    }

    pub(in crate::session) fn live(
        epoch: Uuid,
        revision: u64,
        cause: SelectionCause,
        id: Uuid,
    ) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Live,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Active),
            has_pty: true,
            detached: false,
            displayable: true,
        }
    }

    pub(in crate::session) fn dormant(
        epoch: Uuid,
        revision: u64,
        cause: SelectionCause,
        id: Uuid,
        exit_code: i32,
        has_pty: bool,
    ) -> Self {
        Self {
            epoch: epoch.to_string(),
            source: cause.source(),
            user_initiated: cause.user_initiated(),
            revision,
            mode: SelectionMode::Dormant,
            id: Some(id.to_string()),
            status: Some(SessionStatus::Exited(exit_code)),
            has_pty,
            detached: false,
            displayable: false,
        }
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    pub fn source(&self) -> SelectionSource {
        self.source
    }

    pub fn user_initiated(&self) -> bool {
        self.user_initiated
    }

    pub fn id(&self) -> Option<Uuid> {
        self.id.as_deref().and_then(|id| Uuid::parse_str(id).ok())
    }

    pub fn status(&self) -> Option<&SessionStatus> {
        self.status.as_ref()
    }

    pub fn has_pty(&self) -> bool {
        self.has_pty
    }

    pub fn detached(&self) -> bool {
        self.detached
    }

    pub fn displayable(&self) -> bool {
        self.displayable
    }

    // #1842: compiled on exactly the targets that compile `screenshot::native`,
    // whose `live_capture_resolution_...` test is the only caller in the crate.
    // Keep this predicate equal to `screenshot/mod.rs`'s `native` predicate. A
    // plain `#[cfg(test)]` would leave these caller-less on targets outside the
    // three, where `native` is not compiled, and `dead_code` under
    // `clippy -D warnings` would fail those CI legs.
    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn live_for_test(id: Uuid) -> Self {
        Self::live(Uuid::new_v4(), 1, SelectionCause::UserSwitch, id)
    }

    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn dormant_for_test(id: Uuid, exit_code: i32) -> Self {
        Self::dormant(
            Uuid::new_v4(),
            1,
            SelectionCause::Restore,
            id,
            exit_code,
            false,
        )
    }

    #[cfg(all(
        test,
        any(target_os = "windows", target_os = "linux", target_os = "macos")
    ))]
    pub(crate) fn none_for_test() -> Self {
        Self::none(Uuid::new_v4(), 1, SelectionCause::AutoClose)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedCreateIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedRestartIntent {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedResourceIntent {
    User,
    Watchdog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionCause {
    SessionCreated(TrustedCreateIntent),
    UserSwitch,
    ManualClose,
    AutoClose,
    Restart(TrustedRestartIntent),
    Restore,
    Detach,
    Attach,
    SpawnRollback,
    ResourceMonitor(TrustedResourceIntent),
    BackgroundCleanup,
    LivenessReconcile,
}

impl SelectionCause {
    pub(in crate::session) fn source(self) -> SelectionSource {
        match self {
            Self::SessionCreated(_) => SelectionSource::SessionCreated,
            Self::UserSwitch => SelectionSource::UserSwitch,
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::Restart(_) => SelectionSource::Restart,
            Self::Restore => SelectionSource::Restore,
            Self::Detach => SelectionSource::Detach,
            Self::Attach => SelectionSource::Attach,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::ResourceMonitor(_) => SelectionSource::ResourceMonitor,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
            Self::LivenessReconcile => SelectionSource::LivenessReconcile,
        }
    }

    pub(in crate::session) fn user_initiated(self) -> bool {
        match self {
            Self::SessionCreated(TrustedCreateIntent::User)
            | Self::Restart(TrustedRestartIntent::User)
            | Self::ResourceMonitor(TrustedResourceIntent::User)
            | Self::UserSwitch
            | Self::ManualClose
            | Self::Detach
            | Self::Attach => true,
            Self::SessionCreated(TrustedCreateIntent::Background)
            | Self::Restart(TrustedRestartIntent::Background)
            | Self::ResourceMonitor(TrustedResourceIntent::Watchdog)
            | Self::AutoClose
            | Self::Restore
            | Self::SpawnRollback
            | Self::BackgroundCleanup
            | Self::LivenessReconcile => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSelectionSnapshot {
    pub session_id: Uuid,
    pub has_pty: bool,
    pub detached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveRuntimeWitness {
    pub(in crate::session) session_id: Uuid,
    pub(in crate::session) has_pty: bool,
    pub(in crate::session) detached: bool,
}

impl LiveRuntimeWitness {
    pub(in crate::session) fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (snapshot.has_pty && !snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: true,
            detached: false,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DormantRuntimeWitness {
    pub(in crate::session) session_id: Uuid,
    pub(in crate::session) has_pty: bool,
    pub(in crate::session) detached: bool,
}

impl DormantRuntimeWitness {
    pub(in crate::session) fn from_snapshot(snapshot: RuntimeSelectionSnapshot) -> Option<Self> {
        (!snapshot.detached).then_some(Self {
            session_id: snapshot.session_id,
            has_pty: snapshot.has_pty,
            detached: false,
        })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCoordinatorError {
    #[error("selectionCoordinatorBusy")]
    Busy,
    #[error("selectionCoordinatorUnavailable")]
    Unavailable,
    #[error("selectionCoordinatorRecursiveSubmission")]
    RecursiveSubmission,
}
