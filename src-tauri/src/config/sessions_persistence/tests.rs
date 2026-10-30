use super::{
    append_orphan_archive_record, clear_blocked_menu_and_persist_to_dir,
    clear_user_input_transitions_and_persist_to_dir_result, filter_sessions_for_normalized_roots,
    filter_sessions_for_project_paths, filter_sessions_for_project_paths_blocking,
    is_under_normalized_archived_roots, load_sessions_purging_outside_project_paths_in_dir,
    load_sessions_raw_from_dir_for_test, normalize_project_roots, orphan_counters,
    orphan_warned_len, persist_current_state_result,
    persist_current_state_to_dir_for_project_paths_result, persist_current_state_to_dir_result,
    purge_sessions_outside_project_paths_in_dir, raise_hand_and_persist_to_dir_result,
    rename_with_retry, reset_orphan_archived, reset_orphan_counters, reset_orphan_warned,
    sanitize_failed_recoverable, save_sessions_to_dir, session_retention_project_paths,
    sessions_save_lock, set_blocked_menu_and_persist_to_dir, set_test_orphan_archive_cap,
    snapshot_sessions, strip_auto_injected_args, validate_session_creation_cwd,
    working_directory_under_any_project_path, write_start_fresh_and_persist_to_dir_result,
    BlockedMenuPersistOutcome, PersistMode, PersistedSession, RaiseHandPersistOutcome,
    FILTER_PROJECT_PATHS_THREAD_IDS, NORMALIZE_CALLS, ORPHAN_ARCHIVE_FILENAME, RENAME_ATTEMPTS,
};
#[cfg(windows)]
use super::{deduplicate, load_sessions_from_path};
use crate::config::settings::AppSettings;
use crate::session::manager::{acquire_carrier_mutation_permit, SessionManager};
use crate::session::session::{SessionCommunication, SessionCommunicationKind, SessionStatus};
use std::sync::Arc;
use std::time::Duration;

mod creation_cwd;
mod derived_agent_binding;
mod issue_3145_io;
mod legacy_and_save;
mod live_persist;
mod orphan_purge;
mod project_paths;
mod replacement;
mod serde_snapshot;
mod strip_args;

/// §1295 — held for the WHOLE of any test that counts orphan sweeps or
/// reads/writes the shared archive, so a concurrently-running test cannot
/// pollute the process-global counters / dedup registry or race a tiny-cap
/// rotation. Mirrors the `COUNTING_LOCK` pattern in injected_messages.rs.
static ORPHAN_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 checks the same current snapshot and separately owned historical/physical facts \
              without recapture."
)]
fn assert_retired_replacement_current_facts(
    permit: &crate::session::manager::CarrierMutationPermit,
    current: &crate::session::manager::CarrierSnapshot,
    carrier: &super::PublishedCarrier,
    initial: &super::PublishedCarrier,
    directory: &std::path::Path,
    target: &str,
    row_id: uuid::Uuid,
    physical_cwd: &std::path::Path,
    physical_replica: &crate::path_identity::VerifiedPathIdentity,
    replaced_object: &crate::path_identity::VerifiedPathIdentity,
    owner: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
) {
    assert!(carrier.version > initial.version);
    assert!(
        matches!(super::classify_lifecycle_carrier_scope(permit, None, physical_cwd),
        super::CreateCarrierScope::Protected { session_id, target: actual_target, .. }
        if session_id == row_id && actual_target == target),
        "None key is a domain fact, never a fabricated Foreign scope"
    );
    assert!(
        carrier.retired_local.as_ref().unwrap().settled_version < carrier.version,
        "same-A saves preserve historical settlement without relabeling its version"
    );
    assert_eq!(carrier.retired_local_generation(), Some(7));
    assert!(carrier.retired_local_replica_matches(&(*physical_replica).clone().into()));
    assert!(
        !carrier.retired_local_replica_matches(&(*replaced_object).clone().into()),
        "the same pathname cannot adopt a different physical replica"
    );
    let manual = super::manual_discard_carrier(permit, physical_cwd, target, row_id, current)
        .expect("retired ManualDiscard requires the same strict history");
    assert_eq!(manual.nonce, carrier.nonce);
    assert_eq!(manual.retired_local_generation(), Some(7));
    assert!(
        initial.retired_local_generation().is_none(),
        "historical handles are not relabeled"
    );
    for fault in 0..7 {
        let mut changed = current.clone();
        let actual = changed
            .rows
            .iter_mut()
            .find(|actual| actual.id == row_id)
            .unwrap();
        match fault {
            0 => actual.token = uuid::Uuid::new_v4(),
            1 => actual.status = SessionStatus::Running,
            2 => actual.backend_kind = crate::pty::backend::SessionBackendKind::ContainerTransport,
            3 => actual.is_root_agent = true,
            4 => actual.is_coordinator = true,
            5 => actual.was_detached = true,
            _ => {
                changed.pending_ids.insert(row_id);
            }
        }
        assert!(
            super::retained_carrier_for_replacement(permit, directory, target, row_id, &changed)
                .is_none(),
            "changed current identity is never authorized by retirement history"
        );
    }
    assert!(
        !super::begin_carrier_replacement(permit, initial, owner, binding.session_id(), current),
        "initial old version is not relabeled as current"
    );
    assert!(
        super::retained_carrier_for_replacement(
            permit,
            &directory.join("other"),
            target,
            row_id,
            current
        )
        .is_none(),
        "a different directory cannot adopt A"
    );
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 exhausts and restores the same carrier slot under the original borrowed permit."
)]
fn assert_retired_replacement_epoch_exhaustion(
    permit: &crate::session::manager::CarrierMutationPermit,
    current: &crate::session::manager::CarrierSnapshot,
    carrier: &super::PublishedCarrier,
    directory: &std::path::Path,
    target: &str,
    row_id: uuid::Uuid,
    owner: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
) {
    // Fault-control only: exhaust the publication clock consistently without
    // performing another native write or claiming a new readback proof.
    let saved_epoch = {
        let mut state = carrier.slot.lock().unwrap();
        let saved = state.epoch;
        state.epoch = u64::MAX;
        state.checkpoint.as_mut().unwrap().epoch = u64::MAX;
        for protection in &mut state.protections {
            protection.epoch = u64::MAX;
        }
        saved
    };
    assert!(
        super::retained_carrier_for_replacement(permit, directory, target, row_id, current)
            .is_none()
    );
    assert!(
        super::register_published_carrier(permit, carrier, current, |_| -> Option<()> {
            panic!("exhausted epoch must not register")
        })
        .is_none()
    );
    assert!(!super::begin_carrier_replacement(
        permit,
        carrier,
        owner,
        binding.session_id(),
        current
    ));
    assert!(super::with_confirmed_carrier(
        permit,
        carrier,
        current,
        super::CarrierCheckpointTransition::BeginClosing,
        |_| -> Option<()> { panic!("exhausted epoch must not begin closing") }
    )
    .is_none());
    {
        let mut state = carrier.slot.lock().unwrap();
        assert!(state.protections[0].phase == super::ProtectionPhase::Protected);
        state.protections[0].phase = super::ProtectionPhase::Closing;
    }
    assert_eq!(
        super::with_confirmed_carrier(
            permit,
            carrier,
            current,
            super::CarrierCheckpointTransition::FinishClosing,
            |stamp| Some(stamp.epoch)
        ),
        Some(u64::MAX),
        "exhaustion does not strand an already committed closing owner"
    );
    {
        let mut state = carrier.slot.lock().unwrap();
        state.epoch = saved_epoch;
        state.checkpoint.as_mut().unwrap().epoch = saved_epoch;
        for protection in &mut state.protections {
            protection.epoch = saved_epoch;
            protection.retired_local = carrier.retired_local.clone();
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "P05 preserves distinct owner, binding, current row and carrier rollback assertions."
)]
fn assert_retired_replacement_preparing_owner(
    permit: &crate::session::manager::CarrierMutationPermit,
    current: &crate::session::manager::CarrierSnapshot,
    carrier: &super::PublishedCarrier,
    owner: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
    target: &str,
    cwd: &std::path::Path,
    row: &crate::session::session::Session,
    directory: &std::path::Path,
) {
    assert!(super::begin_carrier_replacement(
        permit,
        carrier,
        owner,
        binding.session_id(),
        current
    ));
    assert!(
        !super::begin_carrier_replacement(
            permit,
            carrier,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            current
        ),
        "another owner cannot replace Preparing"
    );
    assert!(
        matches!(super::classify_create_carrier_scope(permit, target, cwd),
        super::CreateCarrierScope::Protected { preparing_owner: Some(actual), .. }
            if actual == owner)
    );
    assert!(
        !super::retire_carrier_for_discard(permit, row.id),
        "manual discard cannot delete a Preparing replacement owner"
    );
    assert!(
        super::retained_carrier_for_replacement(permit, directory, target, row.id, current)
            .is_none(),
        "a historical Protected handle does not recapture Preparing"
    );
    assert!(
        !super::abort_owned_carrier_replacement(
            permit,
            carrier,
            uuid::Uuid::new_v4(),
            binding.session_id(),
            current
        ),
        "wrong owner cannot release Preparing"
    );
    let mut with_b = current.clone();
    let mut b = (*row).clone();
    b.id = binding.session_id();
    with_b.rows.push(b);
    assert!(
        !super::abort_owned_carrier_replacement(
            permit,
            carrier,
            owner,
            binding.session_id(),
            &with_b
        ),
        "a surviving B prevents precommit rollback release"
    );
    assert!(super::abort_owned_carrier_replacement(
        permit,
        carrier,
        owner,
        binding.session_id(),
        current
    ));
    assert!(matches!(
        super::classify_create_carrier_scope(permit, target, cwd),
        super::CreateCarrierScope::Protected {
            preparing_owner: None,
            ..
        }
    ));
    assert!(
        super::begin_carrier_replacement(permit, carrier, owner, binding.session_id(), current),
        "aborting preserves the actual A carrier and retirement history"
    );
}

#[tokio::test]
async fn unavailable_project_state_refuses_creation_and_blocked_menu_mutation() {
    const CHILD_MARKER: &str = "AGENTSCOMMANDER_F1_CHILD_CONFIG_DIR";
    let marker = std::env::var_os(CHILD_MARKER);
    if marker.is_none() {
        // Keep location overrides in the child: parallel parent tests may
        // already have cached a different location.
        let temp = tempfile::tempdir().expect("parent-owned F1 config directory");
        assert!(temp.path().is_absolute());
        let stdout_path = temp.path().join("child-stdout.log");
        let stderr_path = temp.path().join("child-stderr.log");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("compiled test executable"))
                .args([
                    "--exact",
                    "config::sessions_persistence::tests::\
             unavailable_project_state_refuses_creation_and_blocked_menu_mutation",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("AGENTSCOMMANDER_CONFIG_DIR", temp.path())
                .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", temp.path())
                .env(CHILD_MARKER, temp.path())
                .stdout(std::fs::File::create(&stdout_path).expect("child stdout file"))
                .stderr(std::fs::File::create(&stderr_path).expect("child stderr file"))
                .spawn()
                .expect("spawn isolated F1 child");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                pending_or_error => {
                    let reason = match pending_or_error {
                        Err(error) => format!("F1 child wait failed: {error}"),
                        _ => "F1 child exceeded 60 seconds".to_string(),
                    };
                    child.kill().expect("kill unfinished F1 child");
                    child.wait().expect("reap unfinished F1 child");
                    break Err(reason);
                }
            }
        };
        // Files avoid pipe backpressure while preserving child assertions.
        print!(
            "{}",
            std::fs::read_to_string(&stdout_path).expect("read child stdout")
        );
        eprint!(
            "{}",
            std::fs::read_to_string(&stderr_path).expect("read child stderr")
        );
        let status = result.expect("isolated F1 child must finish within its deadline");
        assert!(status.success(), "isolated F1 child failed: {status}");
        return;
    }
    let expected = std::path::PathBuf::from(marker.expect("F1 child marker"));
    assert!(
        expected.is_absolute(),
        "child config directory must be absolute"
    );
    for override_name in [
        "AGENTSCOMMANDER_CONFIG_DIR",
        "AGENTSCOMMANDER_TEST_CONFIG_DIR",
    ] {
        assert_eq!(
            std::env::var_os(override_name).map(std::path::PathBuf::from),
            Some(expected.clone()),
            "child override must match its recursion marker: {override_name}"
        );
    }
    let dir = crate::config::config_dir().expect("resolved isolated config directory");
    assert_eq!(
        dir, expected,
        "cached child location must match its overrides"
    );
    assert!(dir.is_dir(), "parent must retain the disposable directory");
    let file = dir.join("sessions.json");
    let fixture = [PersistedSession {
        name: "retained".into(),
        shell: "powershell.exe".into(),
        working_directory: dir.to_string_lossy().into_owned(),
        ..Default::default()
    }];
    std::fs::write(&file, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let original = std::fs::read(&file).unwrap();
    let settings = AppSettings {
        project_paths_persistence_error: Some("unavailable".into()),
        ..AppSettings::default()
    };
    let state: crate::config::settings::SettingsState =
        crate::config::settings::new_settings_state(settings.clone());
    let creation_result = super::enforce_creation_gate(
        &state,
        &dir.to_string_lossy(),
        super::CreationGateEnforcement::Enforce,
    )
    .await;
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".into(),
            Vec::new(),
            dir.to_string_lossy().into_owned(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let outcome = super::set_blocked_menu_and_persist(
        &mgr,
        session.id,
        "blocked".into(),
        chrono::Utc::now(),
        &settings,
    )
    .await;
    // Capture the actual persistence target before a bypass-triggered
    // creation/outcome/runtime panic can hide evidence of its write.
    let after = std::fs::read(&file).unwrap();
    eprintln!(
        "F1 authoritative sessions path={} changed={} before={:?} after={:?}",
        file.display(),
        after != original,
        String::from_utf8_lossy(&original),
        String::from_utf8_lossy(&after),
    );
    assert_eq!(creation_result, Err("unavailable".into()));
    assert!(matches!(outcome, BlockedMenuPersistOutcome::NotApplicable));
    assert!(mgr.list_sessions().await[0].communication.is_none());
    assert_eq!(after, original);
}
