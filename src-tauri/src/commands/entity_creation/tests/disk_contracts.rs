use super::{
    collect_agent_delete_plan_guarded, create_test_ac_root, create_test_agent,
    persist_agent_delete_metadata_blocking, persist_agent_delete_metadata_blocking_with_writer,
    rollback_staged_targets_reporting, settings_state, still_staged_suffix, test_base,
    write_team_config_json_atomic, write_team_value, AgentTeamMutation, AppSettings, AtomicBool,
    PreparedAgentDeleteMetadata, StagedAgentDeleteTarget, TeamConfigMutationGuard,
};
use crate::config::entity_creation::{directories, teams};
use std::cell::Cell;
use std::io;
use std::path::Path;
use std::sync::Arc;

fn metadata(base: &Path) -> PreparedAgentDeleteMetadata {
    let team_mutations = ["alpha", "beta"]
        .into_iter()
        .map(|name| {
            let (config_path, before_json) = write_team_value(
                base,
                name,
                serde_json::json!({"agents": ["_agent_dev"], "coordinator": ""}),
            );
            AgentTeamMutation {
                team_name: name.to_string(),
                config_path,
                before_json,
                after_json: b"{\"agents\":[]}\n".to_vec(),
            }
        })
        .collect();
    PreparedAgentDeleteMetadata {
        ac_root: base.to_path_buf(),
        team_mutations,
        agent_name: "dev".into(),
        settings_changed: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn issue_3294_restore_conflict_positive_control() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let metadata = metadata(&base);
    let mut initial = AppSettings::default();
    initial.auto_self_clear_by_agent.insert("dev".into(), false);
    let settings = settings_state(initial);
    let guard = TeamConfigMutationGuard::acquire(&base).unwrap();
    let first = &metadata.team_mutations[0];
    let second = &metadata.team_mutations[1];
    let saves = Cell::new(0);
    let external = b"external bytes\0\xff";
    let error = persist_agent_delete_metadata_blocking(&metadata, &settings, &guard, |_| {
        saves.set(saves.get() + 1);
        assert_eq!(std::fs::read(&first.config_path).unwrap(), first.after_json);
        assert_eq!(
            std::fs::read(&second.config_path).unwrap(),
            second.after_json
        );
        std::fs::write(&first.config_path, external).unwrap();
        Err("injected save failure".into())
    })
    .unwrap_err();
    assert_eq!(saves.get(), 1);
    assert_eq!(
        error,
        format!(
            "Failed to save settings after agent delete: injected save failure; snapshot restore \
             failed: Failed to restore team config snapshot(s): {}: restore conflict (content \
             changed under the lock after write)",
            first.config_path.display()
        )
    );
    assert_eq!(std::fs::read(&first.config_path).unwrap(), external);
    assert_eq!(
        std::fs::read(&second.config_path).unwrap(),
        second.before_json
    );
    assert_eq!(
        settings.blocking_read().auto_self_clear_by_agent.get("dev"),
        Some(&false)
    );
}

#[test]
fn issue_3294_error_order_positive_control() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let metadata = metadata(&base);
    let settings = settings_state(AppSettings::default());
    let guard = TeamConfigMutationGuard::acquire(&base).unwrap();
    let first = &metadata.team_mutations[0];
    let second = &metadata.team_mutations[1];
    let saves = Cell::new(0);
    // The second precheck fails even though the first config was ready to write.
    std::fs::write(&second.config_path, b"changed beta").unwrap();
    let error = persist_agent_delete_metadata_blocking(&metadata, &settings, &guard, |_| {
        saves.set(saves.get() + 1);
        Err("must not save".into())
    })
    .unwrap_err();
    assert_eq!(
        error,
        "Team 'beta' config changed under the lock before agent delete; aborting without writes"
    );
    assert_eq!(
        std::fs::read(&first.config_path).unwrap(),
        first.before_json
    );
    assert_eq!(std::fs::read(&second.config_path).unwrap(), b"changed beta");
    // Two simultaneous failures: first mismatch must win over second read error.
    std::fs::write(&first.config_path, b"changed alpha").unwrap();
    std::fs::remove_file(&second.config_path).unwrap();
    let error = persist_agent_delete_metadata_blocking(&metadata, &settings, &guard, |_| {
        saves.set(saves.get() + 1);
        Err("must not save".into())
    })
    .unwrap_err();
    assert_eq!(
        error,
        "Team 'alpha' config changed under the lock before agent delete; aborting without writes"
    );
    assert_eq!(std::fs::read(&first.config_path).unwrap(), b"changed alpha");
    assert!(!second.config_path.exists());
    assert_eq!(saves.get(), 0);

    let agent = create_test_agent(&base, "dev", "Dev");
    std::fs::write(&first.config_path, b"{ invalid developer").unwrap();
    let unrelated = std::fs::read(&first.config_path).unwrap();
    let plan = collect_agent_delete_plan_guarded(&base, &agent, &guard).unwrap();
    assert!(plan.team_mutations.is_empty());
    assert_eq!(std::fs::read(&first.config_path).unwrap(), unrelated);
    std::fs::write(&first.config_path, b"{ invalid _agent_dev").unwrap();
    let error = collect_agent_delete_plan_guarded(&base, &agent, &guard).unwrap_err();
    assert!(error.starts_with(
        "Cannot delete agent 'dev': cannot verify team 'alpha' references because config.json \
         is invalid: "
    ));
    assert_eq!(
        std::fs::read(&first.config_path).unwrap(),
        b"{ invalid _agent_dev"
    );
}

#[test]
fn issue_3294_strict_team_scan_positive_control() {
    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let metadata = metadata(&base);
    let seen = Cell::new(0);
    let observations = [
        Ok(base.join("_team_alpha")),
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "entry denied",
        )),
        Ok(base.join("_team_beta")),
    ]
    .into_iter()
    .inspect(|_| seen.set(seen.get() + 1));
    let error = directories::strict_teams_from_observations(Ok(observations)).unwrap_err();
    assert_eq!(
        error,
        "Cannot read directory entry during agent delete scan: entry denied"
    );
    assert_eq!(seen.get(), 2, "must abort before consuming the third entry");
    let failure: io::Result<Vec<io::Result<std::path::PathBuf>>> = Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "root denied",
    ));
    assert_eq!(
        directories::strict_teams_from_observations(failure).unwrap_err(),
        "Cannot read Project AC Root directory for agent delete: root denied"
    );
    let paths = directories::strict_teams_from_observations(Ok([
        Ok(base.join("_team_beta")),
        Ok(base.join("_agent_dev")),
        Ok(base.join("_team_alpha")),
    ]))
    .unwrap();
    assert_eq!(
        paths,
        vec![base.join("_team_alpha"), base.join("_team_beta")]
    );
    std::fs::create_dir(base.join("_agent_dev")).unwrap();
    std::fs::write(base.join("_team_file"), b"not a directory").unwrap();
    assert_eq!(directories::strict_team_candidates(&base).unwrap(), paths);
    assert!(directories::strict_team_candidates(&base.join("absent")).is_err());
    for mutation in &metadata.team_mutations {
        assert_eq!(
            teams::read_team_config_bytes(&mutation.config_path).unwrap(),
            mutation.before_json
        );
    }
}

#[test]
fn issue_3294_rollback_positive_control() {
    for fail_restore in [false, true] {
        let tmp = create_test_ac_root();
        let base = test_base(&tmp);
        let metadata = metadata(&base);
        let mut initial = AppSettings::default();
        initial.auto_self_clear_by_agent.insert("dev".into(), false);
        let settings = settings_state(initial);
        let before_settings = serde_json::to_value(&*settings.blocking_read()).unwrap();
        let guard = TeamConfigMutationGuard::acquire(&base).unwrap();
        let first = &metadata.team_mutations[0];
        let second = &metadata.team_mutations[1];
        let saves = Cell::new(0);
        let mut calls = Vec::new();
        let error = persist_agent_delete_metadata_blocking_with_writer(
            &metadata,
            &settings,
            &guard,
            |_| {
                saves.set(saves.get() + 1);
                Err("must not save".into())
            },
            |path, bytes| {
                calls.push((path.to_path_buf(), bytes.to_vec()));
                if path == second.config_path {
                    return Err("injected beta write failure".into());
                }
                if fail_restore && bytes == first.before_json {
                    return Err("injected alpha restore failure".into());
                }
                write_team_config_json_atomic(&base, &guard, path, bytes)
            },
        )
        .unwrap_err();
        assert_eq!(
            calls,
            vec![
                (first.config_path.clone(), first.after_json.clone()),
                (second.config_path.clone(), second.after_json.clone()),
                (first.config_path.clone(), first.before_json.clone()),
            ]
        );
        let primary = "Failed to write team 'beta' config during agent delete: \
                       injected beta write failure";
        let expected = if fail_restore {
            format!(
                "{}; snapshot restore failed: Failed to restore team config snapshot(s): \
                 {}: injected alpha restore failure",
                primary,
                first.config_path.display()
            )
        } else {
            primary.to_string()
        };
        assert_eq!(error, expected);
        assert_eq!(saves.get(), 0);
        assert!(!metadata
            .settings_changed
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            serde_json::to_value(&*settings.blocking_read()).unwrap(),
            before_settings
        );
        assert_eq!(
            std::fs::read(&first.config_path).unwrap(),
            if fail_restore {
                &first.after_json
            } else {
                &first.before_json
            }
            .as_slice()
        );
        assert_eq!(
            std::fs::read(&second.config_path).unwrap(),
            second.before_json
        );
    }

    let tmp = create_test_ac_root();
    let base = test_base(&tmp);
    let original_path = base.join("_agent_dev");
    let staged_path = base.join(".deleting-dev");
    std::fs::create_dir(&original_path).unwrap();
    std::fs::write(original_path.join("occupied"), b"occupied").unwrap();
    std::fs::create_dir(&staged_path).unwrap();
    std::fs::write(staged_path.join("payload"), b"staged bytes").unwrap();
    let target = StagedAgentDeleteTarget {
        original_path: original_path.clone(),
        original_key: "original-key".into(),
        staged_path: staged_path.clone(),
        label: "origin dev".into(),
    };
    assert!(original_path.is_dir());
    assert!(staged_path.is_dir());
    let still = rollback_staged_targets_reporting(&[target]);
    assert!(original_path.is_dir());
    assert!(staged_path.is_dir());
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].original_path, original_path);
    assert_eq!(still[0].staged_path, staged_path);
    assert_eq!(still[0].original_key, "original-key");
    assert_eq!(still[0].label, "origin dev");
    assert_eq!(
        std::fs::read(original_path.join("occupied")).unwrap(),
        b"occupied"
    );
    assert_eq!(
        std::fs::read(staged_path.join("payload")).unwrap(),
        b"staged bytes"
    );
    assert_eq!(
        still_staged_suffix(&still),
        " Hidden cleanup dir(s) remain: origin dev [original-key]"
    );
    assert_eq!(still_staged_suffix(&[]), "");
}
