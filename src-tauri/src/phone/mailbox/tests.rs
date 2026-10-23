mod claim_and;
mod deliver_wake;
mod handle_self;
mod handle_self_2;
mod internal_live;
mod internal_no;
mod is_viable;
mod notice_prefilter;
mod owned_outcome;
#[cfg(windows)]
mod physical_cwd;
mod process_message;
mod project_refresh;
mod raise_hand;
mod remote_activity;
mod retain_unarchived;
mod retained_exited;
mod retry_prune;
mod self_clear;
mod self_restart;
mod self_switch;
mod shared_pty;
mod shared_pty_2;
mod support_commands;
mod support_core;
mod support_retained_asserts;
mod support_retained_cells;
mod support_retained_cleanup;
mod support_retained_failure;
mod support_retained_setup;
mod support_retained_source;
mod support_wake;
mod sustained_idle;
mod test_settle;
mod the_pty;
mod wake_dispatch;
mod wake_dispatch_2;

use super::*;
use crate::config::settings::AppSettings;
use crate::phone::mailbox::handoff::{
    anti_spoof_accept, build_self_clear_handoff_prompt, build_self_restart_handoff_prompt,
    build_self_switch_handoff_prompt, canonicalize_msg_from_in_place, internal_wake_resumes,
    is_viable_root_recipient, is_viable_wake_candidate, next_sustained_idle_state,
    self_clear_handoff_base_prompt, self_restart_handoff_base_prompt,
    self_switch_handoff_base_prompt, wake_action_for, wake_spawn_skip_auto_resume,
    ForgottenSummary, SettleAction, WakeAction, SELF_FORGET_SUMMARY_MAX_CHARS,
};
use crate::phone::mailbox::notice::OutboxOrigin;
use crate::phone::mailbox::poller::{evaluate_gate, PurgeGatePeer, PurgeGateSession};
use crate::phone::mailbox::settle::{
    err_is_pty_session_missing, filter_sessions_by_fqn, live_settle_action, live_wake_route,
    self_clear_gate_advance, settle_tick, wait_for_restore_or_session, wake_settle_ready,
    LiveWakeRoute, RestoreWaitOutcome, SelfClearGateAction, SelfClearGateState, SelfClearPhase,
};
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::manager::PtyManager;
use crate::telegram::manager::TelegramBridgeManager;
use crate::{HashMap, MasterToken};
use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use support_commands::{
    build_raise_hand_message, build_self_clear_message_with_from,
    build_self_restart_message_with_from, build_self_switch_message, dispatch_wake_message,
    logical_command_message, make_gate_peer, make_readiness, make_self_switch_fixture,
    make_session_request_fixture, pending_self_clear_contains, pending_self_clear_len,
    read_reject_reason, read_self_clear_response_status, read_session_request_result_json,
    seed_raise_hand_session, seed_self_switch_session, session_request_file_for,
    session_request_result_file_for, wake_message_to_target, write_logical_command_message,
    write_raise_hand_task_title, write_session_request_for_test, write_wake_message,
    SELF_RESTART_TEST_FROM,
};
#[cfg(windows)]
use support_core::make_mailbox_fixture_in;
use support_core::{
    add_mailbox_session, add_mailbox_session_with_shape, app_handle,
    enqueue_shared_pty_engine_operation, make_mailbox_app, make_mailbox_fixture,
    make_root_route_fixture, make_session_info, make_test_pty_manager, path_string,
    prepare_retained_exited_respawn, remote_activity_notice, root_outbox_message,
    seed_shared_pty_engine_sessions, wake_agent, wake_agents, AppLifetimeProbe, MailboxFixture,
    MailboxMockPtyBackend, CANONICAL_WAKE_FROM, CANONICAL_WAKE_TO, LOCAL_WAKE_FROM, LOCAL_WAKE_TO,
    MAILBOX_MASTER_TOKEN, WAKE_BODY,
};
use support_retained_asserts::{
    assert_retained_cleanup_claimed, assert_retained_cleanup_close, assert_retained_cleanup_common,
    assert_retained_cleanup_finalized, assert_retained_cleanup_runtime_veto,
    assert_retained_committed_failure, assert_retained_failed_rollback,
    assert_retained_native_failure, assert_retained_setup_failure,
    assert_retained_source_inline_boundary, assert_retained_source_lifetime,
    assert_retained_source_newer_result, assert_retained_source_positive,
    assert_retained_source_receipt_order, assert_retained_source_reconciliation,
    assert_retained_source_sealed,
};
use support_retained_cells::{
    retained_exited_respawn_entry_restart_reclaim_cell,
    retained_exited_respawn_entry_root_reclaim_cell, retained_exited_respawn_failure_cell,
    retained_exited_respawn_pty_held_cell, retained_exited_respawn_root_full_cell,
    retained_exited_respawn_submit_refused_cell, retained_fixture_ordinary_wake,
    retained_source_wait_cut,
};
use support_retained_cleanup::{
    retained_exited_respawn_caller_cell, retained_exited_respawn_receive_after_release_cell,
    retained_exited_respawn_reservation_refusal_cell, retained_exited_respawn_root_refusal_cell,
    retained_source_cleanup_race, RetainedCleanupRaceGates,
};
use support_retained_failure::assert_retained_pty_preboundary_rejections;
use support_retained_failure::{
    assert_retained_native_result, observe_retained_native_source_unwind, retained_failure_action,
    retained_failure_context_controller, retained_failure_physical_controller,
    retained_root_reclaim_operation, run_retained_source_child_scenario,
};
use support_retained_setup::{
    configure_retained_seed_fixture, observe_retained_commit_setter, observe_retained_seed_fixture,
    prepare_additional_retained_member, provision_retained_fixture_member,
    retire_retained_fixture_member, set_retained_fixture_communication,
};
use support_retained_source::{
    configure_retained_context_fixture, retained_exited_respawn_native_waiter_cell,
    retained_exited_respawn_source_cell,
};
use support_wake::{
    add_mailbox_session_with_shell, assert_inject_results_consumed,
    assert_no_spawn_or_destroy_events, assert_spawn_call_matches_target,
    assert_spawned_session_matches_target, build_self_clear_message, count_forget_archives,
    enqueue_shared_pty_engine_operation_issued_at, mock_pty_writes_for, read_only_forget_archive,
    read_response_json, register_mock_pty_route, retained_exited_respawn_lifetime_cell,
    retained_exited_respawn_queued_cell, run_mailbox_message, seed_self_clear_session,
    seed_self_handoff, set_mock_pty_write_failure, shared_engine_writes, write_wake_outbox_message,
    write_wake_outbox_message_with_route, RetainedSourceChild, LSA_IDLE_THRESHOLD, LSA_MAX_WAIT,
    LSA_RESIZE_GRACE, LSA_SETTLE,
};
use tauri::Listener;
use tempfile::NamedTempFile;
