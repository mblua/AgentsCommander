use super::*;
use serde_json::json;

#[test]
fn p02_canonical_and_legacy_preserve_literal_argv_and_identity_inputs() {
    let base = r#""C:\Program Files\tool.exe" --fixed "a b""#;
    let suffix = r#"--model "x y""#;
    let legacy: AgentConfig = serde_json::from_value(json!({
        "id": "own",
        "label": "Own",
        "color": "#123456",
        "command": base,
        "envs": [{"key": "MODE", "value": "base"}]
    }))
    .unwrap();
    let canonical: AgentConfig = serde_json::from_value(json!({
        "id": "own",
        "label": "Own",
        "color": "#123456",
        "baseCommand": base,
        "command": "stale",
        "envs": [{"key": "MODE", "value": "base"}]
    }))
    .unwrap();
    let old_cell: ProfileCellConfig =
        serde_json::from_value(json!({"command":suffix,"env":{"MODE":"cell"}})).unwrap();
    let new_cell: ProfileCellConfig = serde_json::from_value(
        json!({"additionalArguments":suffix,"command":"stale","env":{"MODE":"cell"}}),
    )
    .unwrap();
    let combined = compose_effective_command(&canonical.command, &new_cell.command);
    assert_eq!(combined, format!("{base} {suffix}"));
    let parsed = normalize_legacy_agent_command(&combined).unwrap();
    assert_eq!(parsed.shell, r#"C:\Program Files\tool.exe"#);
    assert_eq!(parsed.shell_args, ["--fixed", "a b", "--model", "x y"]);
    assert_eq!(
        cell_identity(&legacy, &old_cell),
        cell_identity(&canonical, &new_cell)
    );
    assert_eq!(
        crate::config::settings::configuration_identity(&legacy.command, &legacy.envs),
        crate::config::settings::configuration_identity(&canonical.command, &canonical.envs)
    );
    assert_eq!(
        crate::config::settings::agent_profile_identity(&legacy),
        crate::config::settings::agent_profile_identity(&canonical)
    );
    assert_eq!(compose_effective_command("", suffix), suffix);
    assert_eq!(compose_effective_command(base, ""), base);
}
