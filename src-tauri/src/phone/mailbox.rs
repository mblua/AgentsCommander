#[macro_use]
mod diagnostics;
mod handoff;
mod internal_delivery;
mod notice;
mod outbox_auth;
mod peer_fallback;
mod poller;
mod pty_artifact;
mod pty_authority;
mod pty_claim;
mod pty_dispatch;
pub(crate) mod pty_observation;
mod pty_validation;
mod request_polling;
pub(crate) mod response_observation;
mod response_source;
mod self_clear;
mod self_handoff;
mod self_switch;
mod session_lookup;
mod session_request;
mod settle;
mod teardown;
mod wake_delivery;
mod wake_injection;
mod wake_workers;

#[cfg(test)]
use crate::config::injected_messages::render;
#[cfg(test)]
use crate::config::injected_messages::CONTEXT_ALERT_MESSAGE_ID;
#[cfg(test)]
use crate::config::injected_messages::MAX_RENDERED_BYTES;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_AT;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_BRANCH;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_GAP;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_MEMBER;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_OBSERVED;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_REPO;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_SHA;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_SINCE;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_THRESHOLDS;
#[cfg(test)]
use crate::config::injected_messages::TOKEN_WORKGROUP;
#[cfg(test)]
use crate::config::instance_artifacts::SESSION_REQUESTS_DIR_NAME;
#[cfg(test)]
use crate::config::settings::AgentConfig;
#[cfg(test)]
use crate::config::settings::SettingsState;
#[cfg(test)]
use crate::phone::consumption::ConsumptionVerdict;
#[cfg(test)]
use crate::phone::types::OutboxMessage;
#[cfg(test)]
use crate::pty::backend::SessionBackendKind;
#[cfg(test)]
use crate::pty::inject::LogicalPtyCommand;
#[cfg(test)]
use crate::pty::inject::SettleReadiness;
#[cfg(test)]
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::session::SessionCommunication;
#[cfg(test)]
use crate::session::session::SessionCommunicationKind;
#[cfg(test)]
use crate::session::session::SessionInfo;
#[cfg(test)]
use crate::session::session::SessionStatus;
#[cfg(test)]
use crate::AppOutbox;
#[cfg(test)]
use handoff::{
    decode_restart_profile_from_seam, dedup_outbox_dirs_by_object_id, display_profile_for_log,
    encode_restart_profile_for_seam, is_permanent_delivery_error, outbox_message_send_time,
    prune_retry_tracker, release_wake_claim, resolve_remote_pty_command,
    resolve_self_restart_recipe, resolve_switch_targets, summarize_self_forget_text,
    wake_claim_has_settled_receipt, wake_claim_origin, wake_claim_path, RetryPruneProbeBarrier,
    SelfSwitchTargets, WakeHandoff, WakeOutcomeReport, SELF_FORGET_SUMMARY_READ_LIMIT_BYTES,
    SELF_HANDOFF_FLAVOR_RESTART, SELF_HANDOFF_FLAVOR_SWITCH,
};
pub(crate) use handoff::{
    PURGE_WG_ACTION, RAISE_HAND_ACTION, SELF_CLEAR_ACTION, SELF_CLEAR_SETTLE_SECS,
    SELF_RESTART_ACTION, SELF_SWITCH_ACTION,
};
#[cfg(all(test, target_os = "linux"))]
use notice::canonical_cwd_owned_by_replica;
#[cfg(test)]
use notice::{
    container_body_override_for_delivery, derive_project_from_outbox_path, format_wake_content,
    inline_body_from_file_notification, internal_system_envelope, parse_remote_pty_command,
    resolve_wake_agent_command_from_sources, sender_name_for_session_cwd,
    sender_name_for_session_cwd_with_root_flag, validate_coordinator_to_root_route,
    validate_root_sender_payload_with_root_dir, validate_root_sender_route, RetryState,
    WakeContent, ERR_INVALID_DISPATCH_PROFILE, ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND,
    MAX_DELIVERY_ATTEMPTS,
};
pub(crate) use notice::{
    retain_unarchived_session_dirs, InternalNoticeGuard, InternalSystemNotice,
    InternalSystemTarget, RemoteNoticeKind, WakeDeliveryOrigin,
};
pub use poller::MailboxPoller;
#[cfg(test)]
use poller::{
    collect_project_refresh_requests, owned_replacement_launch_recipe, read_guarded_agent_state,
    read_text_bom_tolerant,
};
#[cfg(all(test, windows))]
use poller::{reserve_response_request_at_write, PreparedResponseRequest};
#[cfg(all(test, windows))]
use pty_claim::with_verified_response_pair;
#[cfg(test)]
use pty_claim::{
    classify_outbox_document, finish_pty_input_before_boundary, pty_resume_metadata_matches,
    pty_spawn_scope_may_resume, reacquire_pty_dispatch_target, refresh_pty_claim_for_resume,
    InternalSystemBookkeeping, MailboxSpawnCall, MailboxTestEvent, MailboxTestHooks,
    PtyClaimRefresh, PtyReacquireGoal, ResponseWakeWork,
};
pub(crate) use self_clear::PLAIN_SELF_CLEAR_ACTION;
pub(crate) use settle::is_deferred_internal_delivery_error;
#[cfg(test)]
use settle::{
    archive_handoff_for_inject, archive_root_md, capture_self_forget_summary,
    classify_injection_error, internal_target_fqn_prefilter_matches,
    is_persistent_exited_pty_candidate, observe_pty_spawn_scope, resolve_wg_path_from_session_dirs,
    restore_handoff_after_failed_inject, select_persistent_exited_pty_candidate,
    session_has_current_pty_submission_provenance, OutboxClassification, SelfClearBoundary,
};
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use tauri::Manager;
#[cfg(test)]
use tokio_util::sync::CancellationToken;
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
mod tests;
