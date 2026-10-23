use super::*;

/// #2124 T14 - the pure launch resolver: session -> effective catalog entry
/// correspondence, then the burst subfields with the profile defaults.
#[test]
fn resolve_launch_idle_tuning_matches_the_effective_catalog_entry() {
    fn catalog_rows(rows: serde_json::Value) -> Vec<CodingAgentDefinition> {
        serde_json::from_value(rows).expect("catalog rows")
    }
    fn claude_entry(idle_burst: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "key": "claude",
            "label": "Claude Code",
            "description": "d",
            "color": "#111",
            "command": "claude",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": idle_burst,
        })
    }

    // A configured command with arguments matches by executable basename.
    let mut settings = test_settings();
    settings.agents[0].command = "claude --dangerously-skip-permissions".to_string();
    let catalog = catalog_rows(serde_json::json!([claude_entry(
        serde_json::json!({ "maxBytes": 2048 })
    )]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &catalog,
    );
    assert_eq!(resolved.burst_max_bytes, 2048);
    assert_eq!(
        resolved.burst_window,
        Duration::from_secs(3),
        "an absent maxSecs takes the default"
    );
    assert_eq!(resolved.burst_prior_silence, Duration::from_secs(60));
    // The no-catalog tuning is the per-kind default (T10 pins it), and the
    // burst subfields must not move `idle_threshold` or `resize_grace`.
    let per_kind =
        crate::session::profile::idle_tuning_for_launch(Some(CodingAgentKind::Claude), None);
    assert_eq!(resolved.idle_threshold, per_kind.idle_threshold);
    assert_eq!(resolved.resize_grace, per_kind.resize_grace);

    // #2156 - an entry with `idleBurst: null` means no override, so the
    // per-kind filter stays ON.
    let off = catalog_rows(serde_json::json!([claude_entry(serde_json::Value::Null)]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &off,
    );
    assert_eq!(resolved, per_kind);

    // No agent id, no configured agent, or no matching command: the
    // override is dropped. #2156 - the per-kind tuning then decides, so a
    // recognised kind keeps the filter ON and an unrecognised one is OFF.
    for (agent_id, command, agent_kind) in [
        (None, "claude", Some(CodingAgentKind::Claude)),
        (Some("nope"), "claude", Some(CodingAgentKind::Claude)),
        (Some("claude"), "mystery", Some(CodingAgentKind::Claude)),
        (None, "claude", None),
    ] {
        let mut probe = test_settings();
        probe.agents[0].command = command.to_string();
        let resolved = resolve_launch_idle_tuning(&probe, agent_id, agent_kind, &catalog);
        let expected = match agent_kind {
            Some(_) => crate::session::profile::IdleTuning::AGENT_DEFAULT,
            None => crate::session::profile::IdleTuning::DEFAULT,
        };
        assert_eq!(
            resolved, expected,
            "agent_id={agent_id:?} kind={agent_kind:?}"
        );
        if agent_kind.is_none() {
            assert_eq!(resolved.burst_max_bytes, 0);
        }
    }

    // Two entries share the basename; the second states the configured
    // command identically and must win.
    let catalog = catalog_rows(serde_json::json!([
        claude_entry(serde_json::json!({ "maxBytes": 111 })),
        {
            "key": "claude-beta",
            "label": "Claude beta",
            "description": "d",
            "color": "#222",
            "command": "claude --dangerously-skip-permissions",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": { "maxBytes": 222 }
        }
    ]));
    let resolved = resolve_launch_idle_tuning(
        &settings,
        Some("claude"),
        Some(CodingAgentKind::Claude),
        &catalog,
    );
    assert_eq!(resolved.burst_max_bytes, 222, "the identical command wins");
}

/// #2124 T15 - from the seeded files on disk to the resolved tuning: an
/// `agents.50.personal.no-git.json` edit applies at the next spawn without touching
/// `settings.agents[]`.
#[test]
fn resolve_launch_idle_tuning_reads_the_effective_catalog_from_disk() {
    use crate::config::coding_agents_catalog::{
        ensure_seeded_for_project, load_catalog_for_settings,
    };

    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    ensure_seeded_for_project(project);
    let mut settings = test_settings();
    settings.project_paths = vec![project.to_string_lossy().to_string()];
    let resolve = || {
        let catalog = load_catalog_for_settings(&settings).expect("seeded catalog");
        resolve_launch_idle_tuning(
            &settings,
            Some("claude"),
            Some(CodingAgentKind::Claude),
            &catalog,
        )
    };

    let resolved = resolve();
    assert_eq!(resolved.burst_max_bytes, 1024);
    assert_eq!(resolved.burst_window, Duration::from_secs(3));
    assert_eq!(resolved.burst_prior_silence, Duration::from_secs(60));

    let local = project
        .join(".ac")
        .join("coding-agents")
        .join("agents.50.personal.no-git.json");
    std::fs::write(
        &local,
        r#"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxBytes":0}}]}"#,
    )
    .unwrap();
    assert_eq!(
        resolve().burst_max_bytes,
        0,
        "a local edit applies to the next spawn"
    );

    std::fs::write(
        &local,
        r#"{"schemaVersion":1,"agents":[{"key":"claude","idleBurst":{"maxSecs":1.5}}]}"#,
    )
    .unwrap();
    assert_eq!(resolve().burst_window, Duration::from_millis(1500));
    assert_eq!(
        settings.agents[0].command, "claude",
        "settings.agents[] was never touched"
    );
}

#[test]
fn resolve_launch_auto_self_clear_uses_capability_and_setting_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("__agent_dev-rust");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.to_string_lossy().to_string();
    let mut settings = AppSettings::default();

    assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
    assert!(!resolve_launch_auto_self_clear(
        &settings, "pi", &cwd, false
    ));

    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), true);
    assert!(resolve_launch_auto_self_clear(&settings, "pi", &cwd, false));
    settings
        .auto_self_clear_by_agent
        .insert("dev-rust".to_string(), false);
    assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));

    settings.auto_self_clear_by_agent.clear();
    for shell in ["claude", "codex-wrapper", "agy"] {
        assert!(resolve_launch_auto_self_clear(&settings, shell, &cwd, true));
    }
    for shell in ["agent", "cmd.exe", "pwsh", "pip", "pi-agent"] {
        assert!(!resolve_launch_auto_self_clear(
            &settings, shell, &cwd, true
        ));
    }

    settings.auto_self_clear_enabled = false;
    assert!(!resolve_launch_auto_self_clear(&settings, "pi", &cwd, true));
    assert!(!resolve_launch_auto_self_clear(
        &settings, "claude", &cwd, true
    ));
}

#[test]
fn configured_pi_materialization_wires_auto_self_clear_to_agents_md() {
    let temp = tempfile::tempdir().unwrap();
    let ac_root = temp.path().join(".ac");
    let matrix = ac_root.join("_agent_dev-rust");
    std::fs::create_dir_all(&matrix).unwrap();
    let cwd = matrix.to_string_lossy().to_string();
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "pi".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Pi".to_string(),
            command: "pi".to_string(),
            color: "#10b981".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: Some("AGENTS.md".to_string()),
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    let target = crate::config::agent_command::resolve_target_filename(Some("pi"), &settings, None);
    assert_eq!(target.as_deref(), Some("AGENTS.md"));
    assert_eq!(
        crate::config::agent_command::resolve_target_filename(None, &settings, None),
        None,
        "an ad hoc Pi launch has no managed target filename"
    );
    let managed = crate::config::agent_command::managed_instructions_filenames(&settings);
    let enabled = resolve_launch_auto_self_clear(&settings, "pi", &cwd, true);
    assert!(enabled);

    crate::config::session_context::materialize_agent_context_file_with_filename(
        &cwd,
        target.as_deref().unwrap(),
        &managed,
        true,
        enabled,
        None,
    )
    .unwrap()
    .expect("configured Pi context should materialize");
    let on = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
    assert_eq!(on.matches("## Self-Maintenance").count(), 1);

    crate::config::session_context::materialize_agent_context_file_with_filename(
        &cwd,
        "AGENTS.md",
        &managed,
        true,
        false,
        None,
    )
    .unwrap()
    .expect("configured Pi context should rematerialize");
    let off = std::fs::read_to_string(matrix.join("AGENTS.md")).unwrap();
    assert!(!off.contains("## Self-Maintenance"));
}

pub(super) fn inert_pi_spawn() -> crate::config::agent_command::AgentSpawnCommand {
    let settings = AppSettings {
        agents: vec![AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: "pi".to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: "Pi".to_string(),
            command: "pi".to_string(),
            color: "#10b981".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }],
        ..AppSettings::default()
    };
    crate::config::agent_command::resolve_agent_spawn_command(&settings, "pi", None, None, false)
        .expect("Pi test spawn should resolve without filesystem preparation")
}

fn probe_map() -> ContainerPathMap {
    ContainerPathMap::new(r"C:\Users\maria\repo\.ac\wg-1\__agent_x", "/workspace").unwrap()
}

fn norm(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn pi_provider_values_never_resolve_a_claude_probe() {
    for args in [
        vec![
            "--provider".to_string(),
            "claude".to_string(),
            "--continue".to_string(),
        ],
        vec!["--model".to_string(), "claude-sonnet".to_string()],
    ] {
        assert_eq!(
            CodingAgentKind::detect("pi", &args),
            Some(CodingAgentKind::Pi)
        );
        let probe = claude_resume_probe_target_for_kind(
            Some(CodingAgentKind::Pi),
            SessionBackendKind::LocalProcess,
            None,
            None,
            None,
            "pi",
            &args,
            r"Z:\path-that-must-not-be-probed",
        );
        assert!(probe.is_none());
    }
}

#[test]
fn resume_probe_target_local_uses_effective_config_dir_on_host() {
    let cwd = r"C:\Users\maria\repo\.ac\wg-1\__agent_x";
    let config_dir = r"C:\Users\maria\.claude-work";
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        Some(config_dir),
        "claude",
        &[],
        cwd,
    );

    assert_eq!(got.filesystem, "host");
    assert_eq!(
        got.host_probe_path,
        Some(claude_projects_dir_for_config_dir(config_dir, cwd))
    );
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_local_bare_claude_uses_default_resolver() {
    let cwd = r"C:\Users\Test\repo";
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        None,
        "claude",
        &[],
        cwd,
    );

    let expected = resolve_claude_projects_dir("claude", &[], cwd);
    let Some(expected) = expected else {
        return;
    };
    assert_eq!(got.filesystem, "host");
    assert_eq!(got.host_probe_path, Some(expected));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_local_wrapper_uses_wrapper_config_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let cwd = r"C:\Users\Test\repo";

    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::LocalProcess,
        None,
        None,
        wrapper.to_str().unwrap(),
        &[],
        cwd,
    );

    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(got.filesystem, "host");
    assert_eq!(got.host_probe_path, Some(expected));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_container_no_value_warns_and_skips_probe() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        None,
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-unreachable");
    assert!(got.host_probe_path.is_none());
    assert_eq!(
        got.warning.as_ref().map(|w| w.kind),
        Some(WARNING_KIND_NO_VALUE)
    );
}

#[test]
fn resume_probe_target_container_maps_bind_mounted_config_dir() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        Some(r"C:\Users\maria\repo\.ac\wg-1\__agent_x\.claude"),
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-via-mount");
    let host_probe = got.host_probe_path.expect("host probe");
    assert!(norm(&host_probe).ends_with("/__agent_x/.claude/projects/-workspace"));
    assert!(got.warning.is_none());
}

#[test]
fn resume_probe_target_container_unmappable_config_dir_skips_probe_without_duplicate_warning() {
    let map = probe_map();
    let got = resume_probe_target_for_config_dir(
        SessionBackendKind::ContainerTransport,
        Some(&map),
        Some("/workspace/.claude"),
        "claude",
        &[],
        map.host_root(),
    );

    assert_eq!(got.filesystem, "container-unreachable");
    assert!(got.host_probe_path.is_none());
    assert!(got.warning.is_none());
}

fn cred_host_root() -> &'static str {
    if cfg!(windows) {
        r"C:\Users\maria\repo\.ac\wg-1\__agent_x"
    } else {
        "/Users/maria/repo/.ac/wg-1/__agent_x"
    }
}

fn cred_plan_map() -> ContainerPathMap {
    ContainerPathMap::new(cred_host_root(), "/workspace").unwrap()
}

fn cred_plan() -> ContainerCredentialPlan {
    let dest = std::path::Path::new(cred_host_root())
        .join(".claude")
        .join(".credentials.json");
    ContainerCredentialPlan {
        source: PathBuf::from("unused-host-source"),
        dest,
        first_run: None,
    }
}

#[test]
fn injected_config_dir_defaults_to_copy_dir_and_maps_into_container() {
    // #930 - host-login copy will happen (plan Some) and the user set no
    // CLAUDE_CONFIG_DIR -> inject the copy dir (host path), which the container
    // env translation maps to /workspace/.claude so the copied token is read.
    let plan = cred_plan();
    let injected = injected_claude_config_dir_for_copy(Some(&plan), false)
        .expect("copy without a user value must inject the copy dir");
    let expected_dir = format!("{}/.claude", cred_host_root().replace('\\', "/"));
    assert_eq!(injected.replace('\\', "/"), expected_dir);

    let translated = container_child_env(
        vec![(CLAUDE_CONFIG_DIR_KEY.to_string(), injected)],
        Vec::new(),
        &cred_plan_map(),
    );
    assert_eq!(
        translated.child_env,
        vec![(
            CLAUDE_CONFIG_DIR_KEY.to_string(),
            "/workspace/.claude".to_string()
        )]
    );
    assert!(translated.env_unset.is_empty());
    assert!(translated.warnings.is_empty());
}

#[test]
fn injected_config_dir_respects_explicit_user_value() {
    // #930 - the user already configured CLAUDE_CONFIG_DIR; never overwrite it,
    // even though a copy plan exists.
    assert_eq!(
        injected_claude_config_dir_for_copy(Some(&cred_plan()), true),
        None
    );
}

#[test]
fn injected_config_dir_none_without_copy_plan() {
    // #930 - host-login-reuse off or no host creds => no plan => inject nothing,
    // even when the user set no CLAUDE_CONFIG_DIR.
    assert_eq!(injected_claude_config_dir_for_copy(None, false), None);
}

#[test]
fn container_path_context_uses_canonical_host_root_for_guard_and_map() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp
        .path()
        .join("project")
        .join(".ac")
        .join("wg-1-team")
        .join("repo-foo");
    std::fs::create_dir_all(&repo).unwrap();
    let raw = repo.join(".");
    let canonical = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
        &std::fs::canonicalize(&repo).unwrap(),
    );

    let got = container_path_context_for_cwd(raw.to_str().unwrap()).unwrap();

    assert_eq!(got.host_root, canonical);
    assert_eq!(got.map.host_root(), canonical);
}

#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .expect("failed to invoke mklink");
    assert!(
        output.status.success(),
        "failed to create junction link='{}' target='{}': stdout={} stderr={}",
        link.display(),
        target.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_junction_targeting_workgroup_root() {
    // B4 regression fence: before cwd canonicalization, the textual guard
    // saw only `link-to-wg` and permitted a bind mount of the workgroup.
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp
        .path()
        .join("project")
        .join(".ac")
        .join("wg-11-dev-v4-team");
    std::fs::create_dir_all(&target).unwrap();
    let link = tmp.path().join("link-to-wg");
    create_junction(&link, &target);

    let err = container_path_context_for_cwd(link.to_str().unwrap())
        .expect_err("junction to workgroup root must be refused");

    // #1614 section 9.3 clause 1: the string this pins is produced in a
    // DIFFERENT file, pty/container_paths.rs, which section 5.3 renames
    // under Rule R. The fixture below stays a legacy `wg-*` directory
    // (Rule P2) precisely so this proves the refusal still fires for one.
    assert!(err.contains("room root"), "{err}");
    assert!(err.contains("selected path"), "{err}");
    assert!(err.contains("canonical path"), "{err}");
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_junction_targeting_home_dir() {
    // Guard coverage, not the main B4 regression fence: the pre-B4 home
    // rule already canonicalized both sides. This test mainly preserves the
    // selected-vs-canonical rejection text for reparse-point inputs.
    //
    // Safety note: `%TEMP%` usually lives under the user's home directory,
    // so this junction points from a tempdir back to its own ancestor.
    // The test relies on std::fs::remove_dir_all treating the junction as a
    // reparse point and deleting only the link, not descending into home.
    let tmp = tempfile::tempdir().unwrap();
    let home = dirs::home_dir().expect("home dir required for junction guard test");
    let link = tmp.path().join("link-to-home");
    create_junction(&link, &home);

    let err = container_path_context_for_cwd(link.to_str().unwrap())
        .expect_err("junction to home dir must be refused");

    assert!(err.contains("home directory"), "{err}");
    assert!(err.contains("selected path"), "{err}");
    assert!(err.contains("canonical path"), "{err}");
}

#[cfg(windows)]
#[test]
fn container_path_context_refuses_cwd_that_cannot_be_canonicalized() {
    // Pins the fail-closed DD9 behavior from 8d0d3fd5. It is not the B4
    // junction bypass regression fence.
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");

    let err = container_path_context_for_cwd(missing.to_str().unwrap())
        .expect_err("missing cwd must be refused");

    assert!(
        err.contains("failed to canonicalize container mount source"),
        "{err}"
    );
    assert!(err.contains(missing.to_str().unwrap()), "{err}");
}
