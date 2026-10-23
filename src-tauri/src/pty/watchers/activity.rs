use std::sync::Arc;
use uuid::Uuid;

/// #2879 - what the engine tells an internal activity consumer about one session.
/// `Present`/`Absent` are verdicts over a complete frame; `Unavailable` says this tick could
/// not tell; `Validate` checks the consumer is still current; `Retired` ends its sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityObservation {
    Present,
    Absent,
    Unavailable,
    Retired,
    Validate,
}

/// #2879 - one attach of an internal consumer. Returns whether the observation was
/// accepted, which is false once this attach is no longer the current one.
pub(crate) type ActivityConsumer = Arc<dyn Fn(ActivityObservation) -> bool + Send + Sync>;

/// #2879 - attaches a consumer to a session at registration, or `None` when nothing wants
/// to observe it. Fixed at construction; the engine never learns what is behind it.
pub(crate) type ActivityFactory = Arc<dyn Fn(Uuid) -> Option<ActivityConsumer> + Send + Sync>;

/// #2879 - Codex's COMPLETE status-indicator row: an optional `•`/`◦` activity glyph, the
/// upstream `fmt_elapsed_compact` duration (`Ns`, `Nm SSs`, `Nh MMm SSs`), the full
/// `esc to interrupt` hint and an optional ` · ` inline context. Internal, never configured.
pub(super) const CODEX_WORKING_PATTERN: &str = concat!(
    r"^[ \t]*(?:[•◦] )?Working[ \t]+\(",
    r"(?:[0-9]+s|[1-9][0-9]*m [0-5][0-9]s|[1-9][0-9]*h [0-5][0-9]m [0-5][0-9]s)",
    r"[ \t]*•[ \t]*esc[ \t]+to[ \t]+interrupt\)(?: · [^\r\n]+)?[ \t]*$",
);

/// The observation a tick hands to a consumer once every engine guard is released.
pub(super) type ActivityDelivery = Option<(ActivityConsumer, ActivityObservation)>;
