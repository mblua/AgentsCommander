use super::*;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_container_image_env(value: Option<&str>, test: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap();
    let previous = std::env::var_os("AGENTSCOMMANDER_CONTAINER_IMAGE");
    match value {
        Some(value) => std::env::set_var("AGENTSCOMMANDER_CONTAINER_IMAGE", value),
        None => std::env::remove_var("AGENTSCOMMANDER_CONTAINER_IMAGE"),
    }
    test();
    match previous {
        Some(value) => std::env::set_var("AGENTSCOMMANDER_CONTAINER_IMAGE", value),
        None => std::env::remove_var("AGENTSCOMMANDER_CONTAINER_IMAGE"),
    }
}

#[test]
fn api_url_rejects_loopback_and_maps_wildcard_to_docker_host() {
    assert!(api_url_for_container("127.0.0.1", 8765).is_err());
    assert_eq!(
        api_url_for_container("0.0.0.0", 8765).unwrap(),
        "http://host.docker.internal:8765"
    );
    assert_eq!(
        api_url_for_container("192.168.1.10", 8765).unwrap(),
        "http://192.168.1.10:8765"
    );
}

#[test]
fn resolve_container_image_uses_per_agent_then_env() {
    with_container_image_env(Some("env/image:latest"), || {
        assert_eq!(
            resolve_container_image(Some(" per-agent/image:latest ")).unwrap(),
            "per-agent/image:latest"
        );
        assert_eq!(
            resolve_container_image(Some("  ")).unwrap(),
            "env/image:latest"
        );
        assert_eq!(resolve_container_image(None).unwrap(), "env/image:latest");
    });

    with_container_image_env(Some("  "), || {
        let err = resolve_container_image(None).unwrap_err().to_string();
        assert!(err.contains("backend.image"), "{err}");
    });

    with_container_image_env(None, || {
        let err = resolve_container_image(None).unwrap_err().to_string();
        assert!(err.contains("AGENTSCOMMANDER_CONTAINER_IMAGE"), "{err}");
    });
}

#[test]
fn container_diagnostics_formats_bounded_ui_and_full_log() {
    let long_tail = "x".repeat(DIAGNOSTIC_UI_LOG_LIMIT + 25);
    let diagnostics = ContainerDiagnostics {
        container_id: "abc123".to_string(),
        state: Some(ContainerStateSnapshot {
            status: Some("exited".to_string()),
            running: Some(false),
            exit_code: Some(127),
            error: None,
        }),
        inspect_error: None,
        log_tail: Some(format!("line one\n{long_tail}")),
        logs_error: None,
    };

    let ui = diagnostics.ui_summary();
    assert!(ui.contains("container id abc123"), "{ui}");
    assert!(ui.contains("status=exited"), "{ui}");
    assert!(ui.contains("exitCode=127"), "{ui}");
    let ui_logs = ui.split("logs: ").nth(1).expect("ui logs");
    assert!(ui_logs.ends_with("..."), "{ui}");
    assert!(
        ui_logs.chars().count() <= DIAGNOSTIC_UI_LOG_LIMIT + 3,
        "{ui}"
    );

    let full = diagnostics.log_summary();
    assert!(full.contains(&long_tail), "{full}");
}

#[test]
fn container_diagnostics_redacts_tokens_from_ui_and_log_summary() {
    let api_token =
        "ac-container-11111111-1111-4111-8111-111111111111-22222222-2222-4222-8222-222222222222";
    let registration_ticket =
        "acst-33333333-3333-4333-8333-333333333333-44444444-4444-4444-8444-444444444444";
    let openai_key = "openai-secret-without-sensitive-prefix";
    let gemini_key = "gemini-secret-without-sensitive-prefix";
    let github_token = "github-token-without-sensitive-prefix";
    let bare_openai_key = "sk-proj-bare-openai-secret";
    let bare_gemini_key = "AIzaBareGeminiSecret";
    let bare_github_token = "ghp_bare_github_secret";
    let child_env = vec![
        ("OPENAI_API_KEY".to_string(), openai_key.to_string()),
        ("GEMINI_API_KEY".to_string(), gemini_key.to_string()),
        ("GITHUB_TOKEN".to_string(), github_token.to_string()),
        ("PATH".to_string(), "/usr/local/bin".to_string()),
    ];
    let child_env_json = serde_json::to_string(&child_env).expect("serialize child env");
    let bridge_args = vec![
        bare_openai_key,
        bare_gemini_key,
        bare_github_token,
        "--verbose",
    ];
    let bridge_args_json = serde_json::to_string(&bridge_args).expect("serialize bridge args");
    let diagnostics = ContainerDiagnostics {
        container_id: "abc123".to_string(),
        state: None,
        inspect_error: Some("inspect failed".to_string()),
        log_tail: Some(format!(
            "AGENTSCOMMANDER_API_TOKEN={api_token}\n\
             AGENTSCOMMANDER_SESSION_REGISTRATION_TOKEN={registration_ticket}\n\
             AGENTSCOMMANDER_BRIDGE_ARGS_JSON={bridge_args_json}\n\
             AGENTSCOMMANDER_BRIDGE_ENV_JSON={child_env_json}"
        )),
        logs_error: Some(format!(
            "failed after token {api_token} and ticket {registration_ticket}"
        )),
    };

    let ui = diagnostics.ui_summary();
    let full = diagnostics.log_summary();

    for secret in [
        api_token,
        registration_ticket,
        openai_key,
        gemini_key,
        github_token,
        bare_openai_key,
        bare_gemini_key,
        bare_github_token,
    ] {
        assert!(!ui.contains(secret), "{ui}");
        assert!(!full.contains(secret), "{full}");
    }
    assert!(ui.contains(REDACTED_SECRET), "{ui}");
    assert!(full.contains(REDACTED_SECRET), "{full}");
    assert!(ui.contains("PATH"), "{ui}");
    assert!(ui.contains("/usr/local/bin"), "{ui}");
    assert!(full.contains("PATH"), "{full}");
    assert!(full.contains("/usr/local/bin"), "{full}");
    assert!(ui.contains("--verbose"), "{ui}");
    assert!(full.contains("--verbose"), "{full}");
}

#[test]
fn retained_owner_redaction_is_idempotent() {
    let raw = "owner=dockerCommand lastError=OPENAI_API_KEY=sk-proj-secret\nnested failure";
    let once = normalize_retained_owner_diagnostic("containerShutdown", raw);
    let twice = normalize_retained_owner_diagnostic("containerShutdown", &once);

    assert_eq!(
        once,
        "owner=dockerCommand lastError=OPENAI_API_KEY=[REDACTED]\\nnested failure"
    );
    assert_eq!(twice, once);
    assert!(!twice.contains("[REDACTED]]"), "{twice}");

    let direct_once = redact_container_diagnostic_text(
        "owner=dockerCommand lastError=OPENAI_API_KEY=sk-proj-secret",
    );
    assert_eq!(redact_container_diagnostic_text(&direct_once), direct_once);

    let sentinel_prefix_only = "OPENAI_API_KEY=[REDACTED]-still-untrusted";
    assert_ne!(
        redact_container_diagnostic_text(sentinel_prefix_only),
        sentinel_prefix_only,
        "only an exact delimiter-bounded sentinel may bypass another redaction pass"
    );
}

#[test]
fn ui_summary_reports_empty_tail_logs_error_bounded() {
    let token = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789";
    let diagnostics = ContainerDiagnostics {
        container_id: "abc123".to_string(),
        state: None,
        inspect_error: None,
        log_tail: Some(String::new()),
        logs_error: Some(format!(
            "docker logs failed CLAUDE_CODE_OAUTH_TOKEN={token} {}",
            "e".repeat(DIAGNOSTIC_UI_LOG_LIMIT + 25)
        )),
    };

    let ui = diagnostics.ui_summary();
    let ui_error = ui.split("logs unavailable: ").nth(1).expect("logs error");

    assert!(ui.contains("logs unavailable"), "{ui}");
    assert!(!ui.contains(token), "{ui}");
    assert!(ui.contains(REDACTED_SECRET), "{ui}");
    assert!(
        ui_error.chars().count() <= DIAGNOSTIC_UI_LOG_LIMIT + 3,
        "{ui}"
    );
}
