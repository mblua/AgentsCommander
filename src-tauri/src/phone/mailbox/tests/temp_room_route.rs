//! P04: temp-room mailbox route and payload validators.
use super::*;
use crate::config::root_agent::ROOT_AGENT_SENDER;
use crate::config::temp_rooms::temp_room_dir_name;
use crate::phone::mailbox::notice::{sender_name_for_session_cwd, validate_root_sender_route};
use crate::phone::mailbox::temp_room_outbox::{
    validate_temp_room_sender_payload, validate_temp_room_sender_route, validate_to_temp_room_route,
};

const TECH_LEAD: &str = "proj-a:wg-1-dev-team/tech-lead";
const MEMBER: &str = "proj-a:wg-1-dev-team/dev-rust";
const TEMP_TO_REJECTED: &str =
    "Temporary Rooms can only message the Root Agent and verified Room orchestrator replicas";
const TO_TEMP_REJECTED: &str =
    "Only the Root Agent and verified Room orchestrator replicas may message a Temporary Room";

/// A `config_dir()/temp-room-trNN` folder, removed on drop.
pub(super) struct TempRoomDir(pub(super) PathBuf);

impl TempRoomDir {
    pub(super) fn create(n: u32) -> Self {
        let base = crate::config::config_dir().expect("config dir");
        let path = base.join(temp_room_dir_name(n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("messaging")).expect("create temp room");
        Self(path)
    }

    pub(super) fn cwd(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempRoomDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn temp_address(n: u32) -> String {
    format!("agentscommander://temp-room/tr{n:02}")
}

/// A file notification from temp room `n` whose file is `dir/<name>`.
pub(super) fn temp_notice(n: u32, to: &str, dir: &Path, name: &str) -> OutboxMessage {
    std::fs::create_dir_all(dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, "hello").unwrap();
    let mut msg = root_outbox_message(
        crate::phone::messaging::format_file_notification(&file.to_string_lossy()),
        None,
    );
    msg.from = temp_address(n);
    msg.to = to.into();
    msg
}

pub(super) const NOTE: &str = "20261119-120000-tr83-to-tech-lead-hello.md";

#[test]
fn temp_room_session_sender_name_is_address() {
    let room = TempRoomDir::create(83);
    assert_eq!(sender_name_for_session_cwd(&room.cwd()), temp_address(83));
}

#[test]
fn temp_sender_route_accepts_root_and_coordinator() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(
        validate_temp_room_sender_route(ROOT_AGENT_SENDER, &paths, false, true),
        Ok(())
    );
    assert_eq!(
        validate_temp_room_sender_route(TECH_LEAD, &paths, false, true),
        Ok(())
    );
}

#[test]
fn temp_sender_route_rejects_member_origin_and_temp() {
    let (_temp, paths) = make_root_route_fixture(false);
    for to in [MEMBER, "proj-a/tech-lead", temp_address(84).as_str()] {
        assert_eq!(
            validate_temp_room_sender_route(to, &paths, false, true),
            Err(TEMP_TO_REJECTED),
            "{to}"
        );
    }
}

#[test]
fn temp_sender_route_requires_token() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(
        validate_temp_room_sender_route(TECH_LEAD, &paths, false, false),
        Err("Temporary Room sender requires its live session token")
    );
    assert_eq!(
        validate_temp_room_sender_route(TECH_LEAD, &paths, true, false),
        Ok(())
    );
}

#[test]
fn to_temp_route_accepts_coordinator_rejects_member() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(validate_to_temp_room_route(TECH_LEAD, &paths), Ok(()));
    assert_eq!(
        validate_to_temp_room_route(MEMBER, &paths),
        Err(TO_TEMP_REJECTED)
    );
    let (_spoofed, spoofed_paths) = make_root_route_fixture(true);
    assert_eq!(
        validate_to_temp_room_route(TECH_LEAD, &spoofed_paths),
        Err(TO_TEMP_REJECTED)
    );
}

#[test]
fn to_temp_route_rejects_member_even_for_master_caller() {
    // The validator has no master input; the pipeline test proves the master path.
    let (_temp, paths) = make_root_route_fixture(false);
    for from in [MEMBER, "proj-a/dev-rust", temp_address(83).as_str()] {
        assert_eq!(
            validate_to_temp_room_route(from, &paths),
            Err(TO_TEMP_REJECTED),
            "{from}"
        );
    }
}

#[test]
fn root_sender_route_accepts_temp_target() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(
        validate_root_sender_route(&temp_address(83), &paths, false, true, true),
        Ok(())
    );
    assert_eq!(
        validate_root_sender_route(MEMBER, &paths, false, true, true),
        Err("Root Agent can only message verified Room orchestrator replicas")
    );
    assert_eq!(
        validate_root_sender_route(&temp_address(83), &paths, false, false, false),
        Err("Root Agent sender requires the live root session token")
    );
}

#[test]
fn temp_payload_rejects_command_action_and_foreign_dir() {
    let base = tempfile::TempDir::new().unwrap();
    let own = base.path().join(temp_room_dir_name(83)).join("messaging");

    let mut msg = temp_notice(83, TECH_LEAD, &own, NOTE);
    msg.command = Some("ls".into());
    let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
    assert!(err.contains("remote commands are not allowed"), "{err}");

    let mut msg = temp_notice(83, TECH_LEAD, &own, NOTE);
    msg.action = Some("purge-wg".into());
    let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
    assert!(err.contains("action messages are not allowed"), "{err}");

    let mut msg = temp_notice(83, TECH_LEAD, &own, NOTE);
    msg.body = "plain text".into();
    let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
    assert!(err.contains("canonical file notifications"), "{err}");

    let other = base.path().join(temp_room_dir_name(84)).join("messaging");
    let msg = temp_notice(83, TECH_LEAD, &other, NOTE);
    let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
    assert!(
        err.contains("must point inside temp-room-tr83/messaging"),
        "{err}"
    );

    let root_dir = base.path().join(temp_room_dir_name(83));
    let msg = temp_notice(83, TECH_LEAD, &root_dir, NOTE);
    let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
    assert!(
        err.contains("must point inside temp-room-tr83/messaging"),
        "{err}"
    );

    // R5-B2a: `\` is a legal name char on Unix; the body names `x\<NOTE>`, a
    // plain regular file, while `<NOTE>` also exists in the own messaging dir.
    #[cfg(unix)]
    {
        let mut msg = temp_notice(83, TECH_LEAD, &own, NOTE);
        let tricky = own.join(format!("x\\{NOTE}"));
        std::fs::write(&tricky, "other").unwrap();
        msg.body = crate::phone::messaging::format_file_notification(&tricky.to_string_lossy());
        let err = validate_temp_room_sender_payload(&msg, base.path()).unwrap_err();
        assert!(err.contains("must point to a Markdown file"), "{err}");
    }
}

#[test]
fn temp_payload_accepts_own_messaging_file() {
    let base = tempfile::TempDir::new().unwrap();
    let own = base.path().join(temp_room_dir_name(83)).join("messaging");
    let msg = temp_notice(83, TECH_LEAD, &own, NOTE);
    assert_eq!(validate_temp_room_sender_payload(&msg, base.path()), Ok(()));
}

#[tokio::test]
async fn temp_target_lookup_finds_live_session() {
    let room = TempRoomDir::create(84);
    let temp = tempfile::TempDir::new().unwrap();
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let poller = MailboxPoller::default();
    assert!(poller
        .find_temp_room_session_candidate(&app, &temp_address(84))
        .await
        .is_none());

    let id =
        add_mailbox_session_with_shell(&app, &room.0, "tr84", "claude", SessionStatus::Idle).await;
    // No PTY yet: not viable, and the record is kept.
    assert!(poller
        .find_temp_room_session_candidate(&app, &temp_address(84))
        .await
        .is_none());
    register_mock_pty_route(&app, id);
    let found = poller
        .find_temp_room_session_candidate(&app, &temp_address(84))
        .await;
    assert_eq!(found.map(|(found_id, _)| found_id), Some(id));
    assert!(poller
        .find_temp_room_session_candidate(&app, &temp_address(83))
        .await
        .is_none());
}

#[tokio::test]
async fn temp_target_never_spawns() {
    let _room = TempRoomDir::create(84);
    let (temp, _paths) = make_root_route_fixture(false);
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let sender = temp
        .path()
        .join("proj-a/.ac/wg-1-dev-team/__agent_tech-lead");
    let path = write_wake_outbox_message_with_route(
        &sender,
        "msg-tr84-no-session",
        TECH_LEAD,
        &temp_address(84),
    );
    let hooks = MailboxTestHooks::default();
    let poller = MailboxPoller::new_with_test_hooks(hooks.clone());
    let outcome = Box::pin(poller.process_message(&app, &path, OutboxOrigin::Replica)).await;
    assert_no_spawn_or_destroy_events(&hooks);
    let err = outcome.expect_err("no live temp session: delivery fails");
    assert!(err.contains("has no running session"), "{err}");
    assert!(read_reject_reason(&sender, "msg-tr84-no-session").is_none());
}
