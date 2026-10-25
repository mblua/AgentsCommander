//! Version-probe processes; install cache and scheduling policy live in agent_version.

mod owner;

pub use owner::{
    first_text_line, parse_version_token, probe_version, probe_version_cancellable,
    probe_version_in, sanitize_detail, text_lines, CancellableProbeOutcome, ProbeOutcome,
    DETAIL_MAX_CHARS, PROBE_OUTPUT_CAP, PROBE_TIMEOUT,
};
pub(crate) use owner::{probe_version_cancellable_retained, RetainedProbeOwner};
#[cfg(test)]
pub(crate) use owner::{
    strip_ansi_and_controls, with_probe_settlement_hook, with_probe_settlement_hook_after_spawns,
    ProbeNativeObservation, ProbeReaderSettlementHook, ProbeSettlementHook,
};

#[cfg(test)]
mod tests;
