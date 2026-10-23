use super::*;
mod attach_detach_persistence;

#[test]
fn derive_reader_container_claude_requires_memo() {
    let result = derive_reader(
        "claude",
        &[],
        &[],
        r"C:\Users\Test\repo",
        SessionBackendKind::ContainerTransport,
        Some(CodingAgentKind::Claude),
        None,
        None,
    );

    assert!(result
        .expect_err("container Claude memo is required")
        .contains("CLAUDE_CONFIG_DIR is not mapped"));
}

#[test]
fn derive_reader_container_claude_uses_memo() {
    let project_dir = PathBuf::from(r"C:\repo\.ac\wg-1\__agent\.claude\projects\-workspace");
    let result = derive_reader(
        "claude",
        &[],
        &[],
        r"C:\repo\.ac\wg-1\__agent",
        SessionBackendKind::ContainerTransport,
        Some(CodingAgentKind::Claude),
        Some(project_dir.clone()),
        None,
    )
    .unwrap();

    match result {
        Some(SessionReaderKind::Claude {
            project_dir: got, ..
        }) => assert_eq!(got, project_dir),
        other => panic!("unexpected reader: {other:?}"),
    }
}

#[test]
fn derive_reader_container_codex_requires_effective_home_memo() {
    let result = derive_reader(
        "codex",
        &[],
        &[],
        r"C:\Users\Test\repo",
        SessionBackendKind::ContainerTransport,
        Some(CodingAgentKind::Codex),
        None,
        None,
    );

    assert!(result
        .expect_err("container Codex memo is required")
        .contains("CODEX_HOME is not mapped"));
}

#[test]
fn derive_reader_container_codex_uses_effective_home_memo() {
    let home = r"C:\repo\.ac\wg-1\__agent\.codex";
    let result = derive_reader(
        "codex",
        &[],
        &[],
        r"C:\repo\.ac\wg-1\__agent",
        SessionBackendKind::ContainerTransport,
        Some(CodingAgentKind::Codex),
        None,
        Some(home),
    )
    .unwrap();

    match result {
        Some(SessionReaderKind::Codex { search_root, .. }) => {
            assert_eq!(search_root, Path::new(home).join("sessions"));
        }
        other => panic!("unexpected reader: {other:?}"),
    }
}

#[test]
fn derive_reader_pi_uses_pty_fallback_for_all_backends() {
    for backend in [
        SessionBackendKind::LocalProcess,
        SessionBackendKind::ContainerTransport,
    ] {
        let result = derive_reader(
            "pi",
            &["--provider".to_string(), "claude".to_string()],
            &["--provider".to_string(), "claude".to_string()],
            r"C:\Users\Test\repo",
            backend,
            Some(CodingAgentKind::Pi),
            None,
            None,
        );
        assert!(result.unwrap().is_none(), "backend={backend:?}");
    }
}

/// #1873 - Muse is rejected with one exact message on both backends, before
/// `TelegramBridgeManager::attach` could create a bridge or filter.
#[test]
fn derive_reader_rejects_muse_for_all_backends_before_bridge_creation() {
    for backend in [
        SessionBackendKind::LocalProcess,
        SessionBackendKind::ContainerTransport,
    ] {
        for (shell, args) in [
            ("muse", Vec::<String>::new()),
            (
                "/opt/muse/bin/muse",
                vec!["resume".to_string(), "--last".to_string()],
            ),
        ] {
            let result = derive_reader(
                shell,
                &args,
                &args,
                "/srv/work/repo",
                backend,
                Some(CodingAgentKind::Muse),
                None,
                None,
            );
            let err = result.expect_err("Muse must be rejected");
            assert_eq!(
                err,
                "Telegram bridge does not support Muse sessions".to_string(),
                "backend={backend:?} shell={shell:?}"
            );
        }
    }
}

const MINTED_UUID: &str = "16045f91-7d59-4c75-acdb-11ebfcb9aa68";

fn claude_transcript_id(kind: Option<SessionReaderKind>) -> Option<String> {
    match kind {
        Some(SessionReaderKind::Claude { transcript_id, .. }) => transcript_id,
        other => panic!("expected a Claude reader, got {other:?}"),
    }
}

/// #2454 test 9: the id AC injects lives only in `effective_shell_args`.
/// The real caller must hand it to the reader; a session whose effective
/// args are `None` (after an app restart) derives no id.
#[tokio::test]
async fn derive_reader_carries_the_injected_id_through_the_real_caller() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build test app");
    let mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    app.manage(Arc::clone(&mgr));
    let projects = tempfile::tempdir().expect("projects dir");
    let configured = vec!["--dangerously-skip-permissions".to_string()];

    let mut ids = Vec::new();
    for inject in [true, false] {
        let session = {
            let guard = mgr.read().await;
            let session = guard
                .create_session(
                    "claude".to_string(),
                    configured.clone(),
                    r"C:\Users\Test\repo".to_string(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    SessionBackendKind::LocalProcess,
                )
                .await
                .expect("create session");
            guard
                .set_agent_kind(session.id, Some(CodingAgentKind::Claude))
                .await;
            guard
                .set_resolved_claude_projects_dir(session.id, Some(projects.path().to_path_buf()))
                .await;
            if inject {
                // Exactly as `create_session_inner` leaves it: the
                // configured recipe plus the #756 rider.
                let mut effective = configured.clone();
                effective.push("--session-id".to_string());
                effective.push(MINTED_UUID.to_string());
                guard.set_effective_shell_args(session.id, effective).await;
            }
            session
        };
        let kind = reader_kind_for_session(app.handle(), session.id).await;
        ids.push(claude_transcript_id(kind));
    }

    assert_eq!(ids[0].as_deref(), Some(MINTED_UUID));
    assert_eq!(ids[1], None, "no effective args, no id");
}

/// #2454 test 9b: the CONFIGURED `shell_args` alone decide the projects
/// directory. The identity slice carries no Claude token, so feeding it to
/// the resolver instead would find none and fail.
#[test]
fn derive_reader_resolves_the_projects_dir_from_shell_args_only() {
    let cwd = r"C:\Users\Test\repo";
    let shell_args = vec!["claude".to_string()];
    let identity_args = vec!["--session-id".to_string(), MINTED_UUID.to_string()];
    let expected = crate::commands::session::resolve_claude_projects_dir("bash", &shell_args, cwd);
    assert!(
        expected.is_some(),
        "the home base must resolve here or this guard is inconclusive"
    );

    let result = derive_reader(
        "bash",
        &shell_args,
        &identity_args,
        cwd,
        SessionBackendKind::LocalProcess,
        Some(CodingAgentKind::Claude),
        None,
        None,
    )
    .expect("the configured recipe resolves");

    match result {
        Some(SessionReaderKind::Claude {
            project_dir,
            transcript_id,
        }) => {
            assert_eq!(Some(project_dir), expected);
            assert_eq!(transcript_id.as_deref(), Some(MINTED_UUID));
        }
        other => panic!("unexpected reader: {other:?}"),
    }
}

#[test]
fn derive_reader_antigravity_uses_pty_fallback_for_all_backends() {
    for backend in [
        SessionBackendKind::LocalProcess,
        SessionBackendKind::ContainerTransport,
    ] {
        let result = derive_reader(
            "agy",
            &["-m".to_string(), "gpt-5".to_string()],
            &["-m".to_string(), "gpt-5".to_string()],
            r"C:\Users\Test\repo",
            backend,
            Some(CodingAgentKind::Antigravity),
            None,
            None,
        );
        assert!(result.unwrap().is_none(), "backend={backend:?}");
    }
}

#[test]
fn endpoint_photo_under_limit() {
    assert_eq!(choose_endpoint(9 * 1024 * 1024, "png"), Endpoint::Photo);
}

#[test]
fn endpoint_document_size_kickout() {
    assert_eq!(choose_endpoint(11 * 1024 * 1024, "png"), Endpoint::Document);
}

#[test]
fn endpoint_document_extension_kickout() {
    assert_eq!(choose_endpoint(5 * 1024 * 1024, "gif"), Endpoint::Document);
}

#[test]
fn truncate_caption_ascii() {
    let s = "a".repeat(2048);
    let out = truncate_caption(&s);
    assert_eq!(out.encode_utf16().count(), 1024);
    assert!(std::str::from_utf8(out.as_bytes()).is_ok());
}

#[test]
fn truncate_caption_emoji_surrogate() {
    let s: String = "\u{1F600}".repeat(600);
    assert_eq!(s.encode_utf16().count(), 1200);
    let out = truncate_caption(&s);
    let units = out.encode_utf16().count();
    assert!(
        units <= 1024,
        "expected <= 1024 UTF-16 units, got {}",
        units
    );
    assert!(
        !out.contains('\u{FFFD}'),
        "truncate_caption emitted U+FFFD: dangling surrogate not dropped"
    );
}
