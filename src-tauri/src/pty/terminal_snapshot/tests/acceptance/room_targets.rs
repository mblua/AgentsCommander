use super::*;

/// #1614 requirement (D) and (C), section 9.1 "API and snapshot fixture twins".
///
/// This file is on section 6.1's PRESERVED list: no production line in it
/// changes, and section 6.1's table authorizes exactly this `#[cfg(test)]`
/// edit. The snapshot surface treats a peer FQN as opaque, so what has to be
/// proved is that a Room directory name flows through the same shapes the
/// legacy one does and stays a distinct identity.
#[test]
fn room_and_legacy_entity_names_are_both_accepted_and_distinct() {
    use crate::config::entity_prefix::{has_entity_prefix, strip_entity_prefix};

    // (C): both prefixes are accepted by the one shared gate.
    assert!(has_entity_prefix(WORKGROUP), "{WORKGROUP}");
    assert!(has_entity_prefix(ROOM), "{ROOM}");
    // Both resolve to the same slot-and-team remainder, which is what every
    // name parser downstream consumes.
    assert_eq!(strip_entity_prefix(WORKGROUP), Some("1-dev-team"));
    assert_eq!(strip_entity_prefix(ROOM), Some("1-dev-team"));

    // (D) and residual R1: the FQNs this file builds carry the literal
    // directory name, so a Room and a legacy Workgroup at the same slot are
    // two different peers rather than one.
    let legacy_fqn = format!("{PROJECT}:{WORKGROUP}/coordinator");
    let room_fqn = format!("{PROJECT}:{ROOM}/coordinator");
    assert_ne!(legacy_fqn, room_fqn);
    assert!(room_fqn.contains("room-1-dev-team"));
}

const ROOM_ACCEPTANCE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_ROOM_ACCEPTANCE_CHILD";
const ROOM_ACCEPTANCE_TEST_NAME: &str = concat!(
    "pty::terminal_snapshot::tests::acceptance::",
    "room_targets::room_target_capture_and_cross_room_denial_use_the_discovered_fqn",
);

/// #2317: an authorized same-room coordinator captures the discovered room
/// member FQN, while a different room's coordinator is denied without content.
/// The target is the value discovery returned, not a hand-built string, so
/// discovery and capture must agree on one identity.
#[test]
fn room_target_capture_and_cross_room_denial_use_the_discovered_fqn() {
    if std::env::var_os(ROOM_ACCEPTANCE_CHILD_ENV).is_none() {
        let status = std::process::Command::new(
            std::env::current_exe().expect("terminal snapshot acceptance test executable"),
        )
        .args([
            "--exact",
            ROOM_ACCEPTANCE_TEST_NAME,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(ROOM_ACCEPTANCE_CHILD_ENV, "1")
        .status()
        .expect("spawn isolated room snapshot acceptance test");
        assert!(
            status.success(),
            "isolated room snapshot acceptance test failed"
        );
        return;
    }

    let temporary_root = std::env::current_dir()
        .expect("room acceptance current directory")
        .join("target")
        .join("terminal-snapshot-acceptance-temp");
    std::fs::create_dir_all(&temporary_root).expect("room acceptance temporary root");
    let temporary = tempfile::Builder::new()
        .prefix("room-target-")
        .tempdir_in(temporary_root)
        .expect("room acceptance temporary directory");
    let config = temporary.path().join("config");
    std::fs::create_dir_all(&config).expect("room acceptance config directory");
    let _env = ConfigEnvGuard::set(&config);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("room acceptance runtime");
    runtime.block_on(async move {
        let fixture = AcceptanceFixture::new(temporary).await;
        let mut scanner = crate::phone::terminal_snapshot::SnapshotMailboxScanner::default();
        let project_paths = fixture.settings.read().await.project_paths.clone();

        let discovered =
            crate::config::teams::discover_verified_terminal_snapshot_targets(&project_paths)
                .expect("room target discovery");
        let target = discovered
            .iter()
            .map(|identity| identity.canonical_fqn.clone())
            .find(|fqn| fqn == &format!("{PROJECT}:{ROOM}/member-live"))
            .expect("the room member target is discoverable");

        let same_room_coordinator = format!("{PROJECT}:{ROOM}/coordinator");
        let request = host_request(&fixture.room_coordinator, &same_room_coordinator, &target);
        let bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.room_coordinator,
            &request,
        )
        .await;
        let response = decode_host_response(
            &bytes,
            &request.request_id,
            &request.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("room host capture envelope");
        assert_eq!(response.error, None);
        let payload = response.result.as_ref().expect("room capture payload");
        match payload {
            TerminalSnapshotPayload::Json { snapshot } => {
                assert_eq!(snapshot.schema_version, 1);
                assert_eq!(snapshot.target, target);
            }
            TerminalSnapshotPayload::Png { .. } => panic!("JSON capture returned PNG"),
        }
        assert!(payload_has_sentinel(payload));
        let room_copies = fixture
            .local_backend
            .counts(fixture.room_live_member.id)
            .copies;
        assert!(room_copies >= 1, "the room member viewport must be copied");

        let other_room_coordinator = format!("{PROJECT}:{ROOM2}/coordinator");
        let denial = host_request(&fixture.room2_coordinator, &other_room_coordinator, &target);
        let denial_bytes = submit_host_request(
            &fixture,
            &mut scanner,
            &fixture.paths.room2_coordinator,
            &denial,
        )
        .await;
        let denial_response = decode_host_response(
            &denial_bytes,
            &denial.request_id,
            &denial.confirmation_tag,
            &target,
            TerminalSnapshotFormat::Json,
        )
        .expect("room cross-room denial envelope");
        assert_eq!(
            denial_response.error,
            Some(TerminalSnapshotReasonCode::NotAuthorized)
        );
        assert!(denial_response.result.is_none());
        assert!(!denial_bytes
            .windows(SCREEN_SENTINEL.len())
            .any(|window| window == SCREEN_SENTINEL.as_bytes()));
        assert_eq!(
            fixture
                .local_backend
                .counts(fixture.room_live_member.id)
                .copies,
            room_copies,
            "cross-room denial must not read the room member viewport"
        );
    });
}
