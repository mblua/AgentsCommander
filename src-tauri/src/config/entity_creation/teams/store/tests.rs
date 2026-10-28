use super::*;

#[test]
fn issue_3294_store_order_positive_control() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path();
    let guard = TeamConfigMutationGuard::acquire(ac_root).unwrap();
    let config = TeamConfigResult {
        agents: Vec::new(),
        coordinator: String::new(),
        repos: Vec::new(),
        context_alert_percentages: vec![75, 50],
    };
    let team_dir = ac_root.join("_team_sample");
    std::fs::create_dir(&team_dir).unwrap();
    let path = config_path(ac_root, "sample");
    let previous = b"previous config bytes\n";
    std::fs::write(&path, previous).unwrap();
    let error = write_team_config_guarded_with_publisher(
        ac_root,
        "sample",
        &config,
        &guard,
        |target, bytes| {
            assert_eq!(target, path);
            assert!(team_dir.join("memory").is_dir());
            assert_eq!(std::fs::read(team_dir.join("conventions.md")).unwrap(), b"");
            assert_eq!(
                bytes,
                normalized_team_config_bytes(ac_root, &config).unwrap()
            );
            assert_eq!(std::fs::read(target).unwrap(), previous);
            Err("injected publisher failure".to_string())
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        "Failed to publish config.json: injected publisher failure"
    );
    assert!(team_dir.join("memory").is_dir());
    assert!(team_dir.join("conventions.md").is_file());
    assert_eq!(std::fs::read(&path).unwrap(), previous);

    let invalid = TeamConfigResult {
        context_alert_percentages: vec![0],
        ..config
    };
    let error =
        write_team_config_guarded_with_publisher(ac_root, "invalid", &invalid, &guard, |_, _| {
            panic!("invalid normalization must precede publication")
        })
        .unwrap_err();
    assert_eq!(
        error,
        "contextAlertPercentages values must be integer percentages from 1 through 100"
    );
    assert!(!ac_root.join("_team_invalid").exists());
}
