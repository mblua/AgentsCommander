//! #4845 P12: wakes, loops and self-switch read the agent's own file (P03 order).
//! Tempdir only: project-scope named files under a tempdir project root; the 30 file is
//! the live `settings.agents` (`registeredInstance`).

use crate::config::agent_config::set_last_coding_agent_with_source;
use crate::config::coding_agent_files::locator::context_key;
use crate::config::coding_agent_files::{
    with_active, ConfigContext, ConfigFileLocator, ConfigScope,
};
use crate::config::settings::agent::launch_view;
use crate::config::settings::{AgentConfig, AppSettings};
use crate::phone::mailbox::handoff::{resolve_switch_targets, unknown_coding_agent_message};
use crate::phone::mailbox::notice::resolve_wake_agent_command_from_sources;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Proj {
    _temp: tempfile::TempDir,
    root: PathBuf,
    agent_dir: PathBuf,
    settings: AppSettings,
}

fn agent(id: &str) -> Value {
    json!({"id": id, "label": id, "command": "tool", "color": "#123"})
}

fn team() -> ConfigFileLocator {
    ConfigFileLocator::Named {
        scope: ConfigScope::Project,
        name: "team".into(),
    }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A project root with `team` = [codex] and an agent dir; the 30 file = [claude].
fn proj() -> Proj {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("proj");
    let agent_dir = root.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let dir = root.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&dir).unwrap();
    let file = json!({ "agents": [agent("codex")] }).to_string();
    std::fs::write(dir.join("ac.coding-agents-config.team.json"), file).unwrap();
    let settings = AppSettings {
        agents: vec![serde_json::from_value::<AgentConfig>(agent("claude")).unwrap()],
        project_paths: vec![s(&root)],
        ..AppSettings::default()
    };
    Proj {
        _temp: temp,
        root,
        agent_dir,
        settings,
    }
}

impl Proj {
    fn activate_team(&mut self) {
        let ctx = ConfigContext::Project(context_key(&s(&self.root)));
        self.settings.coding_agent_config_files = Some(with_active(None, &ctx, team()));
    }

    fn launch(&self, id: &str, source: Option<&ConfigFileLocator>) {
        let source = source.map(|loc| serde_json::to_value(loc).unwrap());
        let dir = s(&self.agent_dir);
        set_last_coding_agent_with_source(&dir, id, id, None, None, source.as_ref()).unwrap();
    }

    fn wake_ids(&self, last: Option<&str>) -> Option<String> {
        let view = launch_view(&self.settings, &self.agent_dir);
        resolve_wake_agent_command_from_sources(
            &view.settings.agents,
            "auto",
            None,
            last,
            None,
            None,
        )
        .unwrap()
        .and_then(|cmd| cmd.agent_id)
    }
}

#[test]
fn message_wake_uses_last_used_file() {
    let p = proj();
    p.launch("codex", Some(&team()));
    assert_eq!(p.wake_ids(Some("codex")).as_deref(), Some("codex"));
}

#[test]
fn internal_notice_wake_uses_agent_file() {
    let mut p = proj();
    p.activate_team();
    assert_eq!(p.wake_ids(None).as_deref(), Some("codex"));
}

#[test]
fn loop_wake_of_agent_only_in_named_file_resolves() {
    let p = proj();
    p.launch("codex", Some(&team()));
    let command = crate::loops::delivery::resolve_loop_agent_command_from_settings(
        &p.settings,
        &p.agent_dir,
        None,
    )
    .expect("codex from its own file wakes");
    let command = format!("{command:?}");
    assert!(command.contains(r#"agent_id: Some("codex")"#), "{command}");
}

#[test]
fn self_handoff_switch_rejects_id_outside_agent_file_and_lists_its_ids() {
    let mut p = proj();
    p.activate_team();
    let view = launch_view(&p.settings, &p.agent_dir);
    let err = resolve_switch_targets(
        &view.settings,
        &p.agent_dir,
        Some("claude"),
        None,
        Some("codex"),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(err, unknown_coding_agent_message(&view.settings, "claude"));
    assert!(err.contains("codex (codex)"), "{err}");
    assert!(!err.contains("claude (claude)"), "{err}");
}

#[test]
fn no_selection_wake_is_identical_to_base() {
    let p = proj();
    p.launch("claude", None);
    let view = launch_view(&p.settings, &p.agent_dir);
    assert!(matches!(view.settings, std::borrow::Cow::Borrowed(b) if std::ptr::eq(b, &p.settings)));
    let base = resolve_wake_agent_command_from_sources(
        &p.settings.agents,
        "auto",
        None,
        Some("claude"),
        None,
        None,
    )
    .unwrap()
    .and_then(|cmd| cmd.agent_id);
    assert_eq!(p.wake_ids(Some("claude")), base);
}

fn claimed() -> crate::api::message_store::ClaimedPtyInputOperation {
    crate::api::message_store::ClaimedPtyInputOperation {
        injection_id: "i".into(),
        sender_fqn: "project:room/sender".into(),
        target_fqn: "project:room/target".into(),
        op_id: "o".into(),
        source_plane: crate::phone::types::PtyInputSourcePlane::HostCli,
        lease_owner: "l".into(),
        attempt: 1,
        authority_session_id: "a".into(),
        authority_client_id: None,
        authority_client_generation: None,
        sender_identity_fingerprint: "s".repeat(64),
        target_identity_fingerprint: "t".repeat(64),
        requested_agent_id: None,
        expires_at: "2026-11-24T00:00:00Z".into(),
    }
}

fn verified(path: &Path) -> crate::config::teams::VerifiedPtyInputIdentity {
    let identity = crate::path_identity::verify_directory(path).unwrap();
    crate::config::teams::VerifiedPtyInputIdentity {
        canonical_fqn: "project:room/target".into(),
        project: "project".into(),
        workgroup: "room".into(),
        agent: "target".into(),
        replica_root: path.into(),
        matrix_root: path.into(),
        is_coordinator: false,
        project_identity: identity.clone(),
        ac_root_identity: identity.clone(),
        workgroup_identity: identity.clone(),
        replica_identity: identity.clone(),
        matrix_identity: identity,
        incarnation_fingerprint: "d".repeat(64),
        authority_fingerprint: "f".repeat(64),
    }
}

#[test]
fn settle_and_target_lookup_find_agent_only_in_named_file() {
    use crate::session::session::SessionStatus;
    let mut p = proj();
    let wrapper = json!({"id": "codex", "label": "codex", "command": "codex-shell.exe --yolo",
        "color": "#123"});
    let dir = p.root.join(".ac").join("coding-agents");
    let file = json!({ "agents": [wrapper] }).to_string();
    std::fs::write(dir.join("ac.coding-agents-config.team.json"), file).unwrap();
    p.activate_team();
    std::fs::write(p.agent_dir.join("config.json"), "{}").unwrap();
    let cwd = s(&p.agent_dir);
    let (id, spawn, _) = crate::phone::mailbox::MailboxPoller::resolve_pty_target_spawn(
        &p.agent_dir,
        &cwd,
        &p.settings,
        &claimed(),
        &[],
        &verified(&p.agent_dir),
    )
    .expect("codex from the agent's own file is the target");
    assert_eq!(id, "codex");
    let mut session =
        super::make_session_info("wrapper", "member", &cwd, SessionStatus::Idle, true);
    session.shell = spawn.shell.clone();
    session.shell_args = spawn.shell_args.clone();
    session.agent_id = Some(spawn.trusted_agent_id.clone());
    session.agent_kind = Some(crate::session::profile::CodingAgentKind::Codex);
    session.profile_content_hash = Some(spawn.profile_content_hash.clone());
    session.trusted_configured_spawn = true;
    assert!(
        crate::phone::mailbox::settle::session_has_current_pty_submission_provenance(
            &session,
            &p.settings
        )
    );
}
