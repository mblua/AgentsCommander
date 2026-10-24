//! #786 - shared op layer for scriptable Coding Agent configuration.
//!
//! Single entry point (`apply_coding_agent_op`) used by BOTH the direct CLI
//! write path (GUI closed) and the daemon-routed poller (GUI running), so the
//! two paths behave identically by construction. The op layer is PURE: it
//! mutates an in-memory `AppSettings` and runs the shared validator suite, but
//! it NEVER touches disk. Callers own persistence (save + in-memory swap).
//!
//! The NEW rules this feature adds (agent id format/uniqueness, color hex,
//! non-empty label) live ONLY here, never in `validate_and_repair_settings`, so
//! legacy settings.json files with arbitrary ids/colors keep loading and saving.
//! Command/env/instructions/config-seed validation is delegated to the existing
//! `validate_and_repair_settings` -> `validate_agent_commands` suite that every
//! other writer already runs.
//!
//! Queue transport (daemon path): the CLI drops a `CodingAgentRequest` JSON into
//! `<config_dir>/coding-agent-requests/`; the `MailboxPoller` claims it with an
//! atomic rename to `.processing` (#786 R2, closes the cancel false-negative
//! race), applies it, writes a `CodingAgentResult` into `results/`, and deletes
//! the `.processing` file LAST. The per-request handler is a pure,
//! path-parameterized free function so it unit-tests against a tempdir with no
//! Tauri handle (modeled on `mailbox::collect_project_refresh_requests`).

mod agent_op;
mod identity_transition;
mod request_queue;

pub use agent_op::{
    apply_coding_agent_op, mint_agent_id, move_registered_agent, outcome_from_written,
    reorder_registered_agent, unknown_agent_id_error, validate_agent_color,
    validate_custom_agent_id, AgentMoveDirection, AgentPatch, CodingAgentOp, CodingAgentOpOutcome,
    DEFAULT_CUSTOM_COLOR,
};
pub use request_queue::{
    process_coding_agent_request, read_coding_agent_result, write_coding_agent_request,
    CodingAgentRequest, CodingAgentResult, RequestDisposition, CODING_AGENT_REQUESTS_DIR,
    RESULTS_SUBDIR,
};

#[cfg(test)]
use request_queue::{is_expired, write_coding_agent_result, REQUEST_EXPIRY_MS};

#[cfg(test)]
use crate::config::settings::{AgentConfig, AppSettings, CodingAgentEnv, ConfigSeedConfig};
#[cfg(test)]
use identity_transition::{
    capture_source_inventory, commit_source_candidate, commit_source_candidate_internal,
    configure_source_durable_cut, drive_identity_transition, get_identity_transition,
    new_source_transition_id, resume_identity_transition, resume_identity_transition_internal,
    source_discovery_evidence, source_original_inventory_slots, IdentityTransitionRead,
    IdentityTransitionReadState, SourceResumeExpectation, SourceTransitionError,
    SOURCE_DURABLE_CUT, SOURCE_FENCE_EFFECT_BARRIER, SOURCE_TRANSITION_UUIDS,
};
#[cfg(test)]
use request_queue::processing_path_for;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;
