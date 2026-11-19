//! #4780 P04: temp-room routes through the full `process_message` outbox path.
use super::temp_room_route::{temp_address, temp_notice, TempRoomDir};
use super::*;

const TECH_LEAD: &str = "proj-a:wg-1-dev-team/tech-lead";
const MEMBER: &str = "proj-a:wg-1-dev-team/dev-rust";

/// Writes `msg` (with `token`) as an outbox file of `sender_root`; returns its path.
fn write_outbox(sender_root: &Path, mut msg: OutboxMessage, id: &str, token: String) -> PathBuf {
    let outbox = sender_root
        .join(crate::config::agent_local_dir_name())
        .join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    msg.id = id.into();
    msg.token = Some(token);
    let path = outbox.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&msg).unwrap()).unwrap();
    path
}

/// Runs a temp-room sender message (with its live token); returns the reason file
/// text plus any later delivery error (authorization passed, no live target).
async fn run_temp_sender(n: u32, to: &str, in_messaging: bool) -> String {
    let room = TempRoomDir::create(n);
    let (temp, _paths) = make_root_route_fixture(false);
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let (_id, token) = seed_self_clear_session(&app, &room.cwd(), "claude").await;
    let dir = if in_messaging {
        room.0.join("messaging")
    } else {
        room.0.clone()
    };
    let name = format!("20261119-120000-tr{n:02}-to-tech-lead-hello.md");
    let msg = temp_notice(n, to, &dir, &name);
    let id = format!("msg-tr{n:02}");
    let path = write_outbox(&room.0, msg, &id, token.to_string());
    let outcome =
        Box::pin(MailboxPoller::default().process_message(&app, &path, OutboxOrigin::Replica))
            .await;
    let reason = read_reject_reason(&room.0, &id).unwrap_or_default();
    format!("{reason}{}", outcome.err().unwrap_or_default())
}

#[tokio::test]
async fn pipeline_temp_to_member_rejected_by_temp_arm() {
    let reason = run_temp_sender(85, MEMBER, true).await;
    assert!(
        reason.contains(
            "Temporary Rooms can only message the Root Agent and verified Room orchestrator \
            replicas"
        ),
        "{reason}"
    );
}

#[tokio::test]
async fn pipeline_temp_to_orchestrator_passes_authorization() {
    let reason = run_temp_sender(86, TECH_LEAD, true).await;
    for denied in [
        "Temporary Rooms can only message",
        "may message a Temporary Room",
        "requires its live session token",
        "Temporary Room messages must",
        "Temporary Room file notification",
        "Sender cannot reach destination",
    ] {
        assert!(!reason.contains(denied), "{reason}");
    }
}

#[tokio::test]
async fn pipeline_temp_payload_outside_own_messaging_rejected() {
    let reason = run_temp_sender(87, TECH_LEAD, false).await;
    assert!(
        reason.contains("must point inside temp-room-tr87/messaging"),
        "{reason}"
    );
}

#[tokio::test]
async fn pipeline_master_member_to_temp_rejected() {
    let room = TempRoomDir::create(88);
    let (temp, _paths) = make_root_route_fixture(false);
    let app_struct = make_mailbox_app(temp.path());
    let app = app_handle(&app_struct);
    let id =
        add_mailbox_session_with_shell(&app, &room.0, "tr88", "claude", SessionStatus::Idle).await;
    register_mock_pty_route(&app, id);

    let member = temp
        .path()
        .join("proj-a/.ac/wg-1-dev-team/__agent_dev-rust");
    let mut msg = temp_notice(88, &temp_address(88), &member.join("notes"), "note.md");
    msg.from = MEMBER.into();
    let path = write_outbox(&member, msg, "msg-member-tr88", MAILBOX_MASTER_TOKEN.into());
    Box::pin(MailboxPoller::default().process_message(&app, &path, OutboxOrigin::Replica))
        .await
        .expect("process_message");
    let reason = read_reject_reason(&member, "msg-member-tr88").unwrap_or_default();
    assert!(
        reason.contains(
            "Only the Root Agent and verified Room orchestrator replicas may message a \
            Temporary Room"
        ),
        "{reason}"
    );
    assert!(mock_pty_writes_for(&app, id).is_empty());
}
