use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use futures::future::BoxFuture;
use tauri::Manager;
use terminal_snapshot_renderer::{
    decode_api_error, decode_api_success, decode_host_response, TerminalSnapshotFormat,
    TerminalSnapshotPayload, TerminalSnapshotReasonCode,
};
use uuid::Uuid;

use super::*;
use crate::errors::AppError;
use crate::pty::backend::{
    BackendSpawnSpec, PtyBackend, SessionBackendKind, TerminalScreenCopyRead,
};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::manager::BackendWriteAuthority;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionRepo};
use crate::telegram::manager::OutputSenderMap;

const ACCEPTANCE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_ACCEPTANCE_CHILD";
const ACCEPTANCE_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "daemon_paths::real_host_and_api_daemon_paths_enforce_no_oracle_and_final_handoff",
);
const LEAKAGE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_LEAKAGE_CHILD";
const LEAKAGE_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_LEAKAGE_CANARY_FILE";
const LEAKAGE_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "leakage::real_host_and_api_daemon_paths_enforce_secondary_leakage_confinement",
);
const PANIC_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_PANIC_CHILD";
const PANIC_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_PANIC_CANARY_FILE";
const PANIC_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "panic_boundaries::snapshot_production_panic_boundaries_are_payload_free",
);
const API_CANCELLATION_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_API_CANCELLATION_CHILD";
const API_CANCELLATION_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_API_CANCELLATION_CANARY_FILE";
const API_CANCELLATION_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "api_cancellation::api_async_cancellation_reclaims_authority_without_disclosure",
);
const COMMON_BLOCKING_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_COMMON_BLOCKING_CHILD";
const COMMON_BLOCKING_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_COMMON_BLOCKING_CANARY_FILE";
const COMMON_BLOCKING_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "common_blocking::common_blocking_late_completion_retains_authority_without_disclosure",
);
const HOST_FINALIZER_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_FINALIZER_CHILD";
const HOST_FINALIZER_CANARY_FILE_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_FINALIZER_CANARY_FILE";
const HOST_FINALIZER_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "host_finalizer::synchronous_host_finalizer_retains_authority_through_late_completion",
);
const HOST_CANCELLATION_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_HOST_CANCELLATION_CHILD";
const HOST_CANCELLATION_CANARY_FILE_ENV: &str =
    "AC_TERMINAL_SNAPSHOT_HOST_CANCELLATION_CANARY_FILE";
const HOST_CANCELLATION_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "host_cancellation::host_timeout_cancellation_claims_only_the_unaccepted_request",
);
const SCANNER_SHUTDOWN_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_SCANNER_SHUTDOWN_CHILD";
const SCANNER_SHUTDOWN_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "scanner_shutdown::scanner_app_shutdown_owns_composed_host_phases",
);
const BODY_DISCONNECT_SENTINEL: &str = "ACSNAP_BODY_DISCONNECT_1173_C6Q4";
const LATE_BLOCKING_PANIC_SENTINEL: &str = "ACSNAP_LATE_BLOCKING_PANIC_1173_R4M8";
const SCREEN_SENTINEL: &str = "ACSNAP_CELL_CANARY_1173_Z9Q7";
const OSC_TITLE_SENTINEL: &str = "ACSNAP_OSC_TITLE_CANARY_1173_Z9Q7";
const OSC_HYPERLINK_SENTINEL: &str = "ACSNAP_OSC_HYPERLINK_CANARY_1173_Z9Q7";
const OSC_CLIPBOARD_SENTINEL: &str = "ACSNAP_OSC_CLIPBOARD_CANARY_1173_Z9Q7";
const MALFORMED_BODY_SENTINEL: &str = "ACSNAP_MALFORMED_BODY_CANARY_1173_Z9Q7";
const MALFORMED_TARGET_SENTINEL: &str = "ACSNAP_MALFORMED_TARGET_CANARY_1173_Z9Q7";
const CALLER_PATH_SENTINEL: &str = "ACSNAP_CALLER_PATH_CANARY_1173_Z9Q7";
const API_PANIC_SENTINEL: &str = "ACSNAP_API_PANIC_CANARY_1173_P8T4";
const BLOCKING_PANIC_SENTINEL: &str = "ACSNAP_BLOCKING_PANIC_CANARY_1173_B3M6";
const HOST_PANIC_SENTINEL: &str = "ACSNAP_HOST_PANIC_CANARY_1173_H5R2";
const HOST_FINALIZER_PANIC_SENTINEL: &str = "ACSNAP_HOST_FINALIZER_PANIC_1173_V6K2";
const PNG_PANIC_SENTINEL: &str = "ACSNAP_PNG_BYTES_CANARY_1173_G7V4";
const BASE64_PANIC_SENTINEL: &str = "QUNTTkFQX0JBU0U2NF9DQU5BUllfMTE3M19LOFEz";
const HOST_DENIAL_NONCE: &str = "8e9b656f9206198da204c61ef683102b19d1e52c1d8ea385394b04af1c4c26fd";
const HOST_SUCCESS_NONCE: &str = "371f6c57e75bb56177c857f37c31c76f79b552ffa5751a1e3cd11d63b0ec30a5";
const HOST_UNCORRELATED_NONCE: &str =
    "b6bf86d49adfe8b0f8fd34f1767dfc5415e37c3e1fdb781364b5b49de3231b34";
const HOST_FINAL_NONCE: &str = "cfe821546eab901d4b548d56b77f42d5587865d709a76a317855613bd4072525";
const HOST_PANIC_NONCE: &str = "d4ce25af58bfe2f151aebfa3b5865a627a52da87eb8d4ad303049568dc718d33";
const PROJECT: &str = "project";
const WORKGROUP: &str = "wg-1-dev-team";
/// #1614 section 9.1 "API and snapshot fixture twins". Added BESIDE
/// `WORKGROUP`, never replacing it: a legacy fixture is the only way
/// dual-prefix acceptance is testable at all (Rule P2), so this file keeps
/// exercising `wg-1-dev-team` and gains the Room case.
const ROOM: &str = "room-1-dev-team";
/// #2317: a second room, distinct workgroup, used by a foreign-room
/// coordinator to prove cross-room denial.
const ROOM2: &str = "room-2-dev-team";

mod api_cancellation;
mod api_support;
mod common_blocking;
mod daemon_paths;
mod fixture;
mod host_api;
mod host_cancellation;
mod host_finalizer;
mod leakage;
mod panic_boundaries;
mod room_targets;
mod scanner_shutdown;
#[cfg(target_os = "windows")]
mod window_screenshot_factory;
#[cfg(target_os = "windows")]
mod window_screenshot_queue;
#[cfg(target_os = "windows")]
mod window_screenshot_router;

use api_support::{
    abort_direct_api_request_at, assert_api_audit_row, assert_audit_inventory,
    assert_canaries_absent_except, assert_cleanup_and_secondary_surfaces,
    assert_common_blocking_audit_inventory, assert_panic_audit_inventory, await_api_barrier,
    common_blocking_test_audit, contains_raw, direct_api_body, direct_api_state,
    payload_has_sentinel, reserved_loopback_listener, run_direct_api_request, snapshot_audit_rows,
    timeout_api_at_common_blocking_stage, ApiAbortPoint, BodyStreamDropProbe,
    RetainedMaximumAllocation,
};
use fixture::{AcceptanceFixture, BackendLookupCounts, ConfigEnvGuard};
use host_api::{
    api_lifecycle_counts, assert_api_lifecycle_active, assert_api_lifecycle_idle,
    assert_host_finalizer_audit, assert_no_api_test_hooks, audit_contains_request, binding_for,
    consume_host_response, create_mailbox_directories, create_session, host_cancellation_marker,
    host_request, install_host_cancellation_barrier, mint_fixture_token, post_api_bytes,
    post_api_snapshot, prepare_host_finalizer, record_route, retag_host_request,
    start_host_finalizer, submit_host_request, submit_host_request_expect_no_response,
    submit_uncorrelated_host_bytes, terminal_canary_output, wait_for_host_cancellation_barrier,
    write_host_request_bytes, write_security_settings, ApiLifecycleCounts, HostFinalizerHandoff,
    HostFinalizerWorkerOutcome,
};
