use super::{
    classify_existing_root, claude_projects_dir_for_config_dir,
    claude_resume_probe_target_for_kind, coding_agent_descriptor, compute_profile_outdated,
    container_path_context_for_cwd, count_working_members, effective_restart_requested_profile,
    execute_manual_coordinator_destroy, inject_codex_resume, inject_pi_resume,
    injected_claude_config_dir_for_copy, maybe_inject_pi_resume, partition_restart_resume_ready,
    pi_has_explicit_session_control, pi_is_non_conversation_invocation,
    prepare_last_coding_agent_write, resolve_actual_agent, resolve_agent_command,
    resolve_agent_from_shell, resolve_claude_projects_dir, resolve_launch_auto_self_clear,
    resolve_launch_idle_tuning, resolve_restart_selected_agent_id, resolve_root_agent_command,
    restart_resume_prompt_for, restart_resume_session_is_ready, resume_probe_target_for_config_dir,
    save_last_coding_agent, should_inject_continue, CreateSelectionIntent, ExistingRootAction,
    RestartResumeTarget,
};
use crate::config::coding_agents_catalog::CodingAgentDefinition;
use crate::config::settings::{AgentConfig, AppSettings, ProfileCellConfig};
use crate::pty::backend::{PtyBackend, SessionBackendKind};
use crate::pty::container_backend::container_child_env;
use crate::pty::container_credentials::ContainerCredentialPlan;
use crate::pty::container_paths::{ContainerPathMap, CLAUDE_CONFIG_DIR_KEY, WARNING_KIND_NO_VALUE};
use crate::session::manager::SessionManager;
use crate::session::profile::CodingAgentKind;
use crate::session::session::{
    SessionCommunication, SessionCommunicationKind, SessionInfo, SessionStatus,
};
use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use uuid::Uuid;

mod admission;
mod agent_binding;
mod agent_stamps;
mod backends;
mod close_markers;
mod create_gates;
mod cwd_gates;
mod default_shell;
mod descriptor;
mod launch_and_probe;
mod lifecycle;
mod matched_selection;
mod memory_rotation;
mod muse_resume;
mod muse_spawn;
mod restart_resume;
mod restore_publication;
mod resume_injection;
mod resume_policy;
mod root_agent;
mod samplers;
mod support;

pub(crate) use backends::ScriptedSpawnBackend;
use backends::{BarrierDestroyBackend, FailingSpawnBackend, GatedSpawnBackend};
use default_shell::CapturingSpawnBackend;
use launch_and_probe::inert_pi_spawn;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use muse_resume::muse_resume_tokens;
use muse_resume::{expected_muse_resume_argv, muse_spawn_for, muse_test_settings};
use resume_policy::write_wrapper;
use root_agent::make_info;
pub(in crate::commands::session) use support::strict_target_fixture;
use support::{
    assert_memory_pristine, assert_memory_rotated_once, assert_resume_left_memory_alone,
    capture_session_lifecycle, create_manual_cascade_fixture, create_target_for_test,
    rotation_fixture, session_test_app,
};
pub(crate) use support::{
    close_test_coordinator, create_scripted_session, session_test_app_with_store, test_settings,
    SessionTestStoreState,
};

// E15: unarmed parent, armed dedicated child, REAL caller. Base at fd32c052
// was already GREEN, n=1, so this is a regression row: it holds n=1 now
// that the caller's own direct `config.json` read is gone and one loader
// snapshot serves both the id and the descriptor.
#[test]
fn matched_selection_loads_once_from_its_own_dir() {
    let out = crate::config::agent_config::load_probe_harness::expect_child_pass(
        "commands::session::tests::inner_matched_selection_loads_once",
    );
    assert!(out.contains("MEASURED-LOADS n=1"), "{out}");
}

#[test]
#[ignore = "#2786 C1: runs only in the dedicated armed process"]
fn inner_matched_selection_loads_once() {
    use crate::config::agent_config::load_probe::Observation;
    use crate::config::agent_config::load_probe_harness as h;
    let settings = h::snapshot_settings();
    let call = |dir: &std::path::Path, want: &str| {
        let obs = Observation::open();
        let selection =
            super::matched_selection(&settings, &dir.to_string_lossy(), None, None, None);
        println!("REACHED-CALLER selection={selection:?}");
        let loads = obs.loads();
        println!("MEASURED-LOADS n={} {:?}", loads.len(), loads);
        let expected = vec![dir.to_path_buf()];
        assert_eq!(
            loads, expected,
            "recorded {loads:?}, expected exactly {expected:?}"
        );
        assert!(
            matches!(
                &selection,
                super::MatchedSelection::Agent { agent_id, .. } if agent_id == want
            ),
            "want {want}: {selection:?}"
        );
    };

    // E14's fixture: exactly one load, of this directory.

    println!("LEG main");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new");
    call(tmp.path(), "via-new");

    // Unkeyed cache leg: two fresh directories with different values.

    println!("LEG unkeyed");
    for command in ["new-a", "new-b"] {
        let tmp = tempfile::tempdir().expect("tempdir");
        h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", command);
        call(tmp.path(), &format!("via-{command}"));
    }

    // Keyed cache leg: the SAME directory, state rewritten between calls.

    println!("LEG keyed");
    let tmp = tempfile::tempdir().expect("tempdir");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new-a");
    call(tmp.path(), "via-new-a");
    h::write_snapshot_fixture(tmp.path(), "currentCodingAgent", "new-b");
    call(tmp.path(), "via-new-b");
}

// ---- #3532 P14: context-history state registration (Tauri edge harness) -------------------

type HistoryReceiver = tokio::sync::mpsc::Receiver<crate::history_runtime::IngressMessage>;

/// A scope state over the production session adapter and environment.
fn context_history_scopes() -> Arc<crate::history_runtime::HistoryScopeState> {
    let sessions = crate::pty_wiring::HistorySessions {
        session_mgr: Arc::new(tokio::sync::RwLock::new(SessionManager::new())),
    };
    Arc::new(crate::history_runtime::HistoryScopeState::new(
        Arc::new(sessions),
        Arc::new(crate::history_runtime::DiskProjectEnvironment),
    ))
}

/// Offer one distinguishable sample through `ingress`; return its id.
fn context_history_offer(ingress: &crate::history_runtime::HistoryIngress) -> Uuid {
    let id = Uuid::new_v4();
    ingress.offer(
        id,
        crate::context_history::observation::ObservedValue::Reading(5),
    );
    id
}

/// The session ids of the samples waiting in `receiver`.
fn context_history_received(receiver: &mut HistoryReceiver) -> Vec<Uuid> {
    let mut ids = Vec::new();
    while let Ok(message) = receiver.try_recv() {
        if let crate::history_runtime::IngressMessage::Sample(sample) = message {
            ids.push(sample.session_id);
        }
    }
    ids
}

#[test]
fn context_history_state_is_registered_once_by_the_lib_helper() {
    use crate::history_runtime::{HistoryIngress, HistoryScopeState};
    let app = tauri::test::mock_app();
    let (ingress, mut receiver) = HistoryIngress::for_test(8);
    let first = context_history_scopes();
    crate::register_context_history_state(&app, ingress.clone(), Arc::clone(&first))
        .expect("first registration");
    assert!(app.try_state::<HistoryIngress>().is_some());
    assert!(app.try_state::<Arc<HistoryScopeState>>().is_some());
    // The original and the managed clone feed the one receiver.
    let original = context_history_offer(&ingress);
    let managed = context_history_offer(&app.state::<HistoryIngress>());
    assert_eq!(context_history_received(&mut receiver), [original, managed]);

    let (other, mut other_receiver) = HistoryIngress::for_test(8);
    let duplicate = crate::register_context_history_state(&app, other, context_history_scopes());
    assert!(duplicate.is_err(), "a duplicate is refused without panic");
    let kept = app.state::<Arc<HistoryScopeState>>();
    assert!(Arc::ptr_eq(kept.inner(), &first), "the first service stays");
    let managed = context_history_offer(&app.state::<HistoryIngress>());
    assert_eq!(context_history_received(&mut receiver), [managed]);
    assert!(context_history_received(&mut other_receiver).is_empty());
}

#[test]
fn context_history_partial_registrations_are_never_completed() {
    use crate::history_runtime::{HistoryIngress, HistoryScopeState};
    // Only the ingress managed: refused, the scope state stays unregistered.
    let only_ingress = tauri::test::mock_app();
    let (ingress, mut receiver) = HistoryIngress::for_test(8);
    only_ingress.manage(ingress.clone());
    let (other, _other_receiver) = HistoryIngress::for_test(8);
    let refused =
        crate::register_context_history_state(&only_ingress, other, context_history_scopes());
    assert!(refused.is_err());
    assert!(only_ingress.try_state::<Arc<HistoryScopeState>>().is_none());
    let managed = context_history_offer(&only_ingress.state::<HistoryIngress>());
    assert_eq!(
        context_history_received(&mut receiver),
        [managed],
        "ingress kept"
    );

    // Only the scope state managed: refused, the ingress stays unregistered.
    let only_scopes = tauri::test::mock_app();
    let first = context_history_scopes();
    only_scopes.manage(Arc::clone(&first));
    let refused =
        crate::register_context_history_state(&only_scopes, ingress, context_history_scopes());
    assert!(refused.is_err());
    assert!(only_scopes.try_state::<HistoryIngress>().is_none());
    let kept = only_scopes.state::<Arc<HistoryScopeState>>();
    assert!(Arc::ptr_eq(kept.inner(), &first), "scope state kept");
}

#[tokio::test]
async fn context_history_registered_runtime_takes_the_callback_then_stops_in_time() {
    use crate::history_runtime::{HistoryIngress, HistoryRuntime};
    let app = tauri::test::mock_app();
    let scopes = context_history_scopes();
    let token = tokio_util::sync::CancellationToken::new();
    let (ingress, actor) = HistoryRuntime::start(Arc::clone(&scopes), token.clone());
    let task = tokio::spawn(actor);
    crate::register_context_history_state(&app, ingress, scopes).expect("registration");
    let managed = app.state::<HistoryIngress>();
    let session_over = crate::context_history::observation::ObservedValue::SessionOver;
    managed.offer(Uuid::new_v4(), session_over);
    token.cancel();
    let stopped = tokio::time::timeout(Duration::from_secs(2), task).await;
    assert!(
        stopped.is_ok_and(|joined| joined.is_ok()),
        "one actor, stopped within 2 s"
    );
}

#[test]
fn context_history_setup_registers_once_and_logs_a_refusal_without_panic() {
    let setup = include_str!("../../app_run/setup.rs");
    let call = "crate::register_context_history_state(";
    assert_eq!(setup.matches(call).count(), 1);
    let registered = setup.find(call).unwrap();
    let observers = setup.find("fn start_session_observers(").unwrap();
    let handed = setup.find("history: Some(history_ingress)").unwrap();
    assert!(observers < registered && registered < handed);
    let handling = &setup[registered..handed];
    let refusal = handling
        .find("if let Err(error) = registered {")
        .expect("refusal handled");
    assert!(handling[refusal..].contains("log::error!(\"[context-history] {error}"));
    for forbidden in [".unwrap()", ".expect(", "panic!("] {
        assert!(!handling.contains(forbidden), "{forbidden} in registration");
    }
}
