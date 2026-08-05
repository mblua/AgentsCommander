use std::any::Any;
use std::path::PathBuf;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::output::{CapturedVtScreen, PtyOutputTarget, PtyScreenSnapshot};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};
use crate::resource_monitor::{ResourceLaunchRegistration, ResourceLogicalAgentSlot};
use crate::session::profile::{CodingAgentKind, IdleTuning};

/// Maximum exact UTF-8 payload accepted by privileged PTY input.
///
/// The payload is handed to a backend in one call. It is never split into
/// chunks, appended with Enter, or interpreted as a command line.
pub const PTY_INPUT_MAX_BYTES: usize = 65_536;

pub(crate) enum TerminalScreenCopyRead {
    Copied(CapturedVtScreen),
    Unavailable,
    TooLarge,
}

impl std::fmt::Debug for TerminalScreenCopyRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Copied(captured) => formatter
                .debug_tuple("TerminalScreenCopyRead::Copied")
                .field(captured)
                .finish(),
            Self::Unavailable => formatter.write_str("TerminalScreenCopyRead::Unavailable"),
            Self::TooLarge => formatter.write_str("TerminalScreenCopyRead::TooLarge"),
        }
    }
}

pub(crate) enum TerminalScreenRead {
    Captured(std::sync::Arc<terminal_snapshot_renderer::TerminalScreenModel>),
    Unavailable,
    TooLarge,
}

impl std::fmt::Debug for TerminalScreenRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Captured(model) => formatter
                .debug_tuple("TerminalScreenRead::Captured")
                .field(model)
                .finish(),
            Self::Unavailable => formatter.write_str("TerminalScreenRead::Unavailable"),
            Self::TooLarge => formatter.write_str("TerminalScreenRead::TooLarge"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionBackendKind {
    #[default]
    LocalProcess,
    ContainerTransport,
}

/// #973 - the terminal size a session's PTY is opened at.
///
/// AC used to open every ConPTY at a hardcoded 120x30 and let the frontend correct it a few
/// hundred milliseconds later. That correction lands in the middle of a coding agent's TUI
/// startup, and a resize there makes Codex redraw its still-empty viewport and lose the
/// wakeup for the content that becomes ready right after: the terminal stays blank, alive,
/// until any key is pressed. Measured, outside AC: a resize burst at 250 ms hangs it 8/10;
/// opening at the right size and never resizing hangs it 0/10.
///
/// So the size the view has already fitted to is handed down at spawn, and no resize has to
/// happen at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyViewport {
    pub cols: u16,
    pub rows: u16,
}

impl PtyViewport {
    /// What AC has always opened PTYs at. Used by every caller with no view of its own:
    /// the startup-restore loop, the delivery loop, the phone mailbox, the CLI, tests.
    /// Those sessions are not attached to a terminal, so nothing resizes them, so they were
    /// never exposed to this bug (18/18 of them painted in the user's production log).
    pub const DEFAULT: Self = Self {
        cols: 120,
        rows: 30,
    };

    /// The smallest viewport that could plausibly be a terminal a human is looking at.
    ///
    /// A `> 0` check is not enough. xterm does not hand back a zero for a degenerate box - it
    /// clamps to its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1 (`@xterm/addon-fit`) - so a
    /// container that is laid out but collapsed yields a perfectly well-formed **2x1**, which
    /// sails straight through a zero check and opens the ConPTY at 2x1. That is not a wedge: it
    /// drifts, the view corrects it, and the drift is now warned about. But it is not a size any
    /// real view asked for, and a TUI that comes up believing it has two columns lays itself out
    /// around that before anyone can tell it otherwise.
    ///
    /// 20x5 is a floor, not a judgement about what is usable: it only has to be low enough that
    /// no real terminal ever trips it and high enough that a collapsed box always does.
    const PLAUSIBLE_MIN: Self = Self { cols: 20, rows: 5 };

    /// A size the frontend fitted before the session existed.
    ///
    /// Anything below `PLAUSIBLE_MIN` warns and falls back to `DEFAULT` rather than failing the
    /// spawn: a bad fit should cost the user a corrective resize, never a session.
    ///
    /// **Spawn only.** There is deliberately NO floor on the resize path. A user really can drag
    /// a window down to a handful of columns, and refusing to follow them there would strand the
    /// PTY at a size the terminal is not - which is the exact bug class #973 exists to close. The
    /// resize path refuses a degenerate size and nothing else.
    pub fn from_fit(cols: u16, rows: u16) -> Self {
        if cols < Self::PLAUSIBLE_MIN.cols || rows < Self::PLAUSIBLE_MIN.rows {
            log::warn!(
                "[pty] ignoring implausible fitted viewport {cols}x{rows} (floor {}x{}), opening at {}x{}",
                Self::PLAUSIBLE_MIN.cols,
                Self::PLAUSIBLE_MIN.rows,
                Self::DEFAULT.cols,
                Self::DEFAULT.rows
            );
            return Self::DEFAULT;
        }
        Self { cols, rows }
    }
}

pub struct BackendSpawnSpec {
    pub id: Uuid,
    /// #942 - the configured coding-agent PROFILE id (`settings.agents[].id`, an
    /// opaque string like `agent_1782513272568_0`), or None. It is NOT the CLI:
    /// several profiles can run the same `codex` binary, and a coding agent can be
    /// launched with no profile id at all. Diagnostics log it, and never key on it.
    pub agent_id: Option<String>,
    /// #942 - the CLI actually being launched, from the canonical detector
    /// (`CodingAgentKind::detect`). This is the identity the diagnostics key on: the
    /// stall predicate and the "concurrent startups on the shared ~/.codex" counter.
    pub coding_agent: Option<CodingAgentKind>,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub selected_cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub container_image: Option<String>,
    pub configured_env: Vec<(String, String)>,
    pub env_remove_keys: Vec<String>,
    pub env_unset: Vec<String>,
    pub extra_env: Vec<(String, String)>,
    pub idle_tuning: IdleTuning,
    pub output_target: PtyOutputTarget,
    pub resource_registration: Option<ResourceLaunchRegistration>,
    pub logical_resource_slot: Option<ResourceLogicalAgentSlot>,
    /// #930 - resolved host-credential copy-in for container sessions. None for
    /// local-process sessions and when copy-in is disabled/not applicable.
    pub container_credential: Option<crate::pty::container_credentials::ContainerCredentialPlan>,
    /// #935 - read-write repo bind mounts for container sessions. Empty for
    /// local-process sessions and when the replica has no admissible repos.
    pub container_repo_mounts: Vec<crate::pty::container_repos::ContainerRepoMount>,
}

pub trait PtyBackend: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn spawn(&self, spec: BackendSpawnSpec) -> BoxFuture<'_, Result<(), AppError>>;

    fn write(
        &self,
        authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError>;

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError>;

    fn kill(&self, id: Uuid) -> Result<(), AppError>;

    fn has_session(&self, id: Uuid) -> bool;

    /// Lightweight context-session liveness. Backends with a richer contained child oracle
    /// override this; the default preserves source compatibility and is truthful for routes
    /// whose `has_session` means an active transport.
    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        if self.has_session(id) {
            ContextSessionLiveness::Live
        } else {
            ContextSessionLiveness::SessionOver
        }
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot>;

    /// Read-only fixed-cell viewport copy. Backends unrelated to the two live
    /// production fanouts remain source-compatible and report unavailable.
    #[allow(private_interfaces)]
    fn copy_terminal_screen(&self, _id: Uuid) -> TerminalScreenCopyRead {
        TerminalScreenCopyRead::Unavailable
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)>;

    /// #1032 - the session's screen rows for the context scrape, plus what this backend
    /// knows about whether there is still a session behind the id.
    ///
    /// - `Rows`: the live grid.
    /// - `Unavailable`: no reading this tick, and NO claim about the session. Keep
    ///   sampling it. A child that is alive but whose handle cannot be queried lands
    ///   here, and calling that "over" would strand a live session forever.
    /// - `SessionOver`: there is no session here any more. Report unavailable once, then
    ///   stop sampling it.
    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead;

    /// #1171 - the session's screen, but only when it CHANGED since `seen`, plus the wrap
    /// flags and cursor row the watcher engine's frame diff needs.
    ///
    /// **Defaulted**, following `context_session_liveness` above, so the two `PtyBackend` test
    /// fakes (`pty/manager.rs:926`, `:1025`) and any out-of-tree implementor keep compiling.
    /// The default delegates to `get_screen_rows` and reports `stamp: None`, which reads as
    /// "changed": the property "the default never reports `Unchanged`" therefore falls out of
    /// the type instead of out of a rule an implementor has to remember.
    ///
    /// - `Unchanged`: the stamp matched. No rows were cloned.
    /// - `Frame`: the live grid.
    /// - `Missing`: no reading this tick, and NO claim about the session. Keep sampling it.
    /// - `Gone`: there is no session behind this id. Retire it now.
    ///
    /// The `Missing` / `Gone` split is the same distinction `get_screen_rows` draws between
    /// `Unavailable` and `SessionOver`, carried on a type that can also say "nothing changed".
    fn screen_rows_since(&self, id: Uuid, _seen: Option<FrameStamp>) -> ScreenRowsSince {
        crate::pty::watchers::frame_from_screen_rows_read(self.get_screen_rows(id))
    }

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: PathBuf,
    );

    fn terminate_job_for_session(&self, id: Uuid) -> bool;

    /// #942 - tag an imminent AC stop with the liveness of the child BEFORE any process
    /// is touched, for callers that are about to kill outside the PTY layer (the resource
    /// monitor kills a process tree by pid). Diagnostics only; the default no-op covers
    /// backends with no local child (container transport).
    fn publish_stop_witness(&self, _id: Uuid, _source: &str) {}

    fn kill_all_jobs(&self) -> (usize, usize);
}
#[cfg(test)]
mod pty_viewport_tests {
    use super::PtyViewport;

    /// #973 - the size a real view fitted is handed down untouched. That is the whole point of
    /// the type: the PTY opens at the size the terminal already is, so nothing has to resize it
    /// during the child's startup.
    #[test]
    fn a_real_fitted_size_is_handed_down_as_is() {
        assert_eq!(
            PtyViewport::from_fit(74, 23),
            PtyViewport { cols: 74, rows: 23 }
        );
        // right on the floor, and still a real terminal
        assert_eq!(
            PtyViewport::from_fit(20, 5),
            PtyViewport { cols: 20, rows: 5 }
        );
    }

    /// #973 - the case a zero check misses. xterm clamps a collapsed container to 2x1 rather
    /// than reporting a zero, so `2x1` is what a degenerate box actually looks like coming out
    /// of `fit()` - well-formed, positive, and not a terminal anybody is looking at.
    #[test]
    fn a_collapsed_box_does_not_open_the_pty_at_two_by_one() {
        assert_eq!(PtyViewport::from_fit(2, 1), PtyViewport::DEFAULT);
        assert_eq!(PtyViewport::from_fit(19, 40), PtyViewport::DEFAULT);
        assert_eq!(PtyViewport::from_fit(80, 4), PtyViewport::DEFAULT);
        assert_eq!(PtyViewport::from_fit(0, 0), PtyViewport::DEFAULT);
    }
}
