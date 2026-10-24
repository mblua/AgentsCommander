use super::*;

fn config_with_alerts() -> TeamConfigResult {
    TeamConfigResult {
        agents: vec!["_agent_alpha".to_string()],
        coordinator: "_agent_alpha".to_string(),
        repos: vec![RepoAssignment {
            url: "https://example.test/repo.git".to_string(),
            agents: vec!["_agent_alpha".to_string()],
        }],
        context_alert_percentages: vec![50, 75, 90],
    }
}

#[test]
fn roster_helpers_preserve_complete_alert_vector() {
    let original_alerts = config_with_alerts().context_alert_percentages;
    let (added, did_add) =
        add_member_to_team_config(config_with_alerts(), "_agent_beta", false).expect("add member");
    assert!(did_add);
    assert_eq!(added.context_alert_percentages, original_alerts);
    assert_eq!(added.repos[0].agents, vec!["_agent_alpha"]);

    let (removed, did_remove) =
        remove_member_from_team_config(added, "_agent_beta").expect("remove member");
    assert!(did_remove);
    assert_eq!(removed.context_alert_percentages, vec![50, 75, 90]);
    assert_eq!(removed.agents, vec!["_agent_alpha"]);
}

#[test]
fn cli_team_list_and_create_results_include_camel_case_alerts() {
    let list = TeamListItem {
        team: "dev-team".to_string(),
        workgroup: None,
        agents: Vec::new(),
        coordinator: String::new(),
        repos: Vec::new(),
        context_alert_percentages: vec![50, 75],
    };
    let create = TeamCreateResult {
        team: "dev-team".to_string(),
        path: "_team_dev-team".to_string(),
        agents: Vec::new(),
        coordinator: String::new(),
        repos: Vec::new(),
        context_alert_percentages: Vec::new(),
    };

    let list_json = serde_json::to_value(list).expect("list JSON");
    let create_json = serde_json::to_value(create).expect("create JSON");
    assert_eq!(
        list_json["contextAlertPercentages"],
        serde_json::json!([50, 75])
    );
    assert_eq!(
        create_json["contextAlertPercentages"],
        serde_json::json!([])
    );
}

// #1063 Stage D-owned exact ignored CHILD HELPER for the CLI team-member
// cross-process lock-order inversion. Frozen fully-qualified name (a Stage E
// parent spawns this verbatim via `current_exe --exact <name> --ignored`):
//   crate::cli::team::tests::cli_member_lock_order_inversion_child
// No-ops (no guard, no mutation) unless the child-mode action + per-spawn nonce
// + control dir are all supplied; when driven, it calls the real private
// `remove_member_hooked` and drives the `after_project_before_team` barrier to
// report project-gate acquisition (before the team guard) and wait for release.
#[test]
#[ignore]
fn cli_member_lock_order_inversion_child() {
    let Some(ctx) = super::stage_d_lock_order_child::child_context(
        super::stage_d_lock_order_child::MEMBER_ACTION,
    ) else {
        return;
    };
    let project = ctx.build_workgroup_fixture();
    let args = TeamRemoveMemberArgs {
        project,
        workgroup: "wg-1-dev-team".to_string(),
        agent: "member".to_string(),
    };
    let result = remove_member_hooked(args, None, |_ac_root: &std::path::Path| {
        ctx.report_and_wait()
    });
    println!(
        "STAGE_D_LOCK_ORDER_DONE {} member ok={}",
        ctx.nonce,
        result.is_ok()
    );
}

// Deterministic (non-ignored) proof that the child-mode tuple validation no-ops
// without a complete, valid tuple - so a bare ignored helper cannot act.
#[test]
fn cli_lock_order_child_context_no_ops_without_tuple() {
    use super::stage_d_lock_order_child::{context_from, MEMBER_ACTION};
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().to_str().expect("utf8 dir");
    assert!(context_from(MEMBER_ACTION, None, None, None).is_none());
    assert!(context_from(MEMBER_ACTION, Some("other"), Some("n"), Some(dir)).is_none());
    assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some(" "), Some(dir)).is_none());
    assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some("n"), None).is_none());
    assert!(context_from(
        MEMBER_ACTION,
        Some(MEMBER_ACTION),
        Some("n"),
        Some("no-such-dir-xyz")
    )
    .is_none());
    assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some("n"), Some(dir)).is_some());
}

// #1063: prove the driven lock-order path in-process (no `current_exe` parent -
// that machinery is Stage E). Sets the child-mode tuple, drives the real
// `cli_member_lock_order_inversion_child`, releases the barrier from a thread,
// and asserts the barrier fired (project gate acquired before team) and the
// member replica was removed after release. `#[ignore]` + env-guarded so the
// parallel `--lib` regression is untouched. It also no-ops unless `DRIVE_VAR` is
// set, so a bare `cargo test --lib -- --ignored` run never drives it. Enable and
// isolate it deliberately (it pins the once-cached config dir, so it must own the
// process): `AC_STAGE_D_LOCK_ORDER_DRIVE=1 cargo test --lib -- --ignored
// --test-threads=1 --exact cli::team::tests::cli_member_lock_order_inversion_driver`.
#[test]
#[ignore]
fn cli_member_lock_order_inversion_driver() {
    use super::stage_d_lock_order_child::{
        driver_enabled, EnvGuard, ACTION_VAR, CONTROL_DIR_VAR, MEMBER_ACTION, NONCE_VAR,
    };
    if !driver_enabled() {
        return;
    }
    let control = tempfile::tempdir().expect("tempdir");
    let nonce = "driver-member";
    let _guard = EnvGuard::capture(&[
        ACTION_VAR,
        NONCE_VAR,
        CONTROL_DIR_VAR,
        "AGENTSCOMMANDER_TEST_CONFIG_DIR",
    ]);
    std::env::set_var(ACTION_VAR, MEMBER_ACTION);
    std::env::set_var(NONCE_VAR, nonce);
    std::env::set_var(CONTROL_DIR_VAR, control.path());

    let reached = control.path().join(format!("reached-{nonce}"));
    let release = control.path().join(format!("release-{nonce}"));
    let releaser = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline && !reached.exists() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            reached.exists(),
            "child never reported reaching the barrier"
        );
        std::fs::write(&release, b"go").expect("write release");
    });

    cli_member_lock_order_inversion_child();
    releaser.join().expect("join releaser");

    assert!(
        control.path().join(format!("reached-{nonce}")).exists(),
        "the barrier must have fired (project gate acquired before team)"
    );
    let replica = control
        .path()
        .join(format!("fixture-{nonce}"))
        .join("Project")
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_member");
    assert!(
        !replica.exists(),
        "the member replica must be removed after the barrier releases"
    );
}
