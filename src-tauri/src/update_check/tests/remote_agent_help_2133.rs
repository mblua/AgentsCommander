use super::remote_blocking_menus_1925::{offline_url, serve_once};
use super::*;

fn now() -> DateTime<Utc> {
    "2026-09-24T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
}

#[tokio::test]
async fn a_disabled_run_touches_no_disk_and_no_network() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let network = crate::network::OutboundNetwork::new_for_tests(4);
    let outcome = run_remote_agent_help_check(
        &network,
        &settings_path,
        false,
        crate::config::settings::REMOTE_AGENT_HELP_URL,
        now(),
    )
    .await;
    assert_eq!(outcome, RemoteAgentHelpCheck::Disabled);
    assert!(network.acquired_labels_for_tests().is_empty());
    assert!(!crate::config::settings::agent_help_remote_check_path(&settings_path).exists());
    assert!(!crate::config::settings::agent_help_remote_path(&settings_path).exists());
}

#[tokio::test]
async fn a_not_due_run_does_not_fetch() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings_path = temp.path().join("settings.json");
    let network = crate::network::OutboundNetwork::new_for_tests(4);
    let stamped = now() - chrono::Duration::hours(23);
    crate::config::settings::write_remote_agent_help_check_stamp(&settings_path, stamped).unwrap();
    let outcome = run_remote_agent_help_check(
        &network,
        &settings_path,
        true,
        crate::config::settings::REMOTE_AGENT_HELP_URL,
        now(),
    )
    .await;
    assert_eq!(outcome, RemoteAgentHelpCheck::NotDue);
    assert!(network.acquired_labels_for_tests().is_empty());
    assert_eq!(
        crate::config::settings::read_remote_agent_help_check_stamp(&settings_path),
        Some(stamped)
    );
    assert!(!crate::config::settings::agent_help_remote_path(&settings_path).exists());
}

#[tokio::test]
async fn every_due_attempt_stamps_now() {
    let valid = br#"{"schemaVersion":1,"byCommand":{"codex":{"label":"Codex"}}}"#.to_vec();
    let by_agent = br#"{"schemaVersion":1,"byAgent":{"a1":{"label":"x"}}}"#.to_vec();
    for label in ["accepted", "rejected", "unreachable"] {
        let temp = tempfile::TempDir::new().unwrap();
        let settings_path = temp.path().join("settings.json");
        let network = crate::network::OutboundNetwork::new_for_tests(4);
        // A stamp older than 24 h: the attempt is due and must move it to `now`.
        crate::config::settings::write_remote_agent_help_check_stamp(
            &settings_path,
            now() - chrono::Duration::hours(25),
        )
        .unwrap();
        let url = match label {
            "accepted" => serve_once(200, valid.clone()).await,
            "rejected" => serve_once(200, by_agent.clone()).await,
            _ => offline_url().await,
        };
        let outcome =
            run_remote_agent_help_check(&network, &settings_path, true, &url, now()).await;
        match (label, &outcome) {
            ("accepted", RemoteAgentHelpCheck::Accepted) => {
                let cached = crate::config::settings::load_remote_agent_help_file(&settings_path);
                assert_eq!(cached.by_command["codex"].label.as_deref(), Some("Codex"));
                assert!(cached.note.unwrap().contains(&url));
            }
            ("rejected", RemoteAgentHelpCheck::Rejected(reason)) => {
                assert_eq!(reason, "byAgent must be empty");
                assert!(!crate::config::settings::agent_help_remote_path(&settings_path).exists());
            }
            ("unreachable", RemoteAgentHelpCheck::Unreachable(_)) => {}
            _ => panic!("{label}: unexpected outcome {outcome:?}"),
        }
        assert_eq!(
            network.acquired_labels_for_tests(),
            vec!["update_check.remote_agent_help"],
            "{label}"
        );
        assert_eq!(
            crate::config::settings::read_remote_agent_help_check_stamp(&settings_path),
            Some(now()),
            "{label}: every due attempt stamps now"
        );
    }
}
