use super::mailbox::{action_requires_orchestrator, response_json};
use super::*;

fn normalized(input: &str) -> String {
    normalize_url(input)
        .expect("url should normalize")
        .to_string()
}

#[test]
fn keeps_http_https_file_and_about_urls() {
    assert_eq!(
        normalized("https://example.com/a?b=1"),
        "https://example.com/a?b=1"
    );
    assert_eq!(
        normalized("http://localhost:3000/"),
        "http://localhost:3000/"
    );
    assert_eq!(normalized("about:blank"), "about:blank");
    assert_eq!(
        normalized("file:///tmp/diagram.html"),
        "file:///tmp/diagram.html"
    );
}

#[test]
fn adds_a_scheme_to_bare_hosts() {
    assert_eq!(
        normalized("  example.com/docs  "),
        "https://example.com/docs"
    );
    assert_eq!(normalized("localhost:5173"), "http://localhost:5173/");
    assert_eq!(normalized("127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
}

#[cfg(windows)]
#[test]
fn turns_absolute_windows_paths_into_file_urls() {
    assert_eq!(
        normalized(r"C:\diagrams\flow.html"),
        "file:///C:/diagrams/flow.html"
    );
}

#[cfg(not(windows))]
#[test]
fn turns_absolute_unix_paths_into_file_urls() {
    assert_eq!(
        normalized("/tmp/diagrams/flow.html"),
        "file:///tmp/diagrams/flow.html"
    );
}

#[test]
fn rejects_scripts_blanks_and_unknown_input() {
    for input in [
        "",
        "   ",
        "javascript:alert(1)",
        "javascript:a.b",
        "data:text/html,<b>x</b>",
        "ftp://example.com",
        "not a url",
        "intranet",
    ] {
        assert!(
            normalize_url(input).is_err(),
            "{input:?} should be rejected"
        );
    }
}

#[test]
fn screen_rect_offsets_by_the_client_origin_and_never_collapses() {
    let bounds = PaneBounds {
        x: 10.4,
        y: 20.6,
        width: 300.0,
        height: 0.0,
    };
    assert_eq!(screen_rect((100, -50), bounds), (110, -29, 300, 1));
}

#[test]
fn history_script_moves_one_step_at_most() {
    assert_eq!(history_script(-1), "history.go(-1)");
    assert_eq!(history_script(1), "history.go(1)");
    assert_eq!(history_script(-7), "history.go(-1)");
    assert_eq!(history_script(9), "history.go(1)");
}

#[test]
fn snapshot_defaults_to_a_blank_hidden_page() {
    let snapshot = WebBrowserState::default().with(|model| model.snapshot());
    let expected = WebBrowserSnapshot {
        url: DEFAULT_URL.to_string(),
        title: String::new(),
        loading: false,
        visible: false,
    };
    assert_eq!(snapshot, expected);
}

#[test]
fn only_status_is_open_to_every_agent() {
    assert!(is_web_browser_action(Some(WEB_BROWSER_NAVIGATE_ACTION)));
    assert!(is_web_browser_action(Some(WEB_BROWSER_RELOAD_ACTION)));
    assert!(is_web_browser_action(Some(WEB_BROWSER_STATUS_ACTION)));
    assert!(!is_web_browser_action(Some("raise-hand")));
    assert!(!is_web_browser_action(None));
    assert!(action_requires_orchestrator(WEB_BROWSER_NAVIGATE_ACTION));
    assert!(action_requires_orchestrator(WEB_BROWSER_RELOAD_ACTION));
    assert!(!action_requires_orchestrator(WEB_BROWSER_STATUS_ACTION));
}

#[test]
fn response_carries_the_snapshot() {
    let snapshot = WebBrowserSnapshot {
        url: "https://example.com/".to_string(),
        title: "Example".to_string(),
        loading: true,
        visible: true,
    };
    let response = response_json(WEB_BROWSER_STATUS_ACTION, &snapshot);
    assert_eq!(response["action"], WEB_BROWSER_STATUS_ACTION);
    assert_eq!(response["status"], "ok");
    assert_eq!(response["url"], "https://example.com/");
    assert_eq!(response["title"], "Example");
    assert_eq!(response["loading"], true);
    assert_eq!(response["visible"], true);
}

#[test]
fn app_origins_are_refused() {
    let dev = Url::parse("http://localhost:1420").unwrap();
    for input in [
        "http://tauri.localhost/",
        "https://tauri.localhost/x",
        "HTTP://TAURI.LOCALHOST/",
        "http://tauri.localhost./",
        "http://tauri.localhost:8080/",
        "http://ipc.localhost/cmd",
        "http://asset.localhost/f",
        "tauri://localhost/",
        "ipc://localhost/",
        "http://localhost:1420/x",
    ] {
        assert!(
            is_app_origin(&Url::parse(input).unwrap(), Some(&dev)),
            "{input}"
        );
    }
}

#[test]
fn external_pages_stay_allowed() {
    let dev = Url::parse("http://localhost:1420").unwrap();
    for input in [
        "https://example.com/",
        "http://localhost:3000/",
        "http://127.0.0.1:1420/",
        "http://tauri.localhost.example.com/",
        "about:blank",
        "file:///tmp/a.html",
    ] {
        assert!(
            !is_app_origin(&Url::parse(input).unwrap(), Some(&dev)),
            "{input}"
        );
    }
    assert!(!is_app_origin(
        &Url::parse("http://localhost:1420/").unwrap(),
        None
    ));
}

#[test]
fn navigation_entry_points_refuse_the_app_origin() {
    let app = tauri::test::mock_app();
    let h = app.handle();
    assert!(navigate_to(h, "http://tauri.localhost/")
        .unwrap_err()
        .contains("refused"));
    assert!(agent_navigate(h, "http://ipc.localhost/")
        .unwrap_err()
        .contains("refused"));
    assert_eq!(snapshot(h).url, "about:blank");
    assert!(navigate_to(h, "https://example.com/").is_ok());
    assert_eq!(snapshot(h).url, "https://example.com/");
}

#[tauri::command]
fn acl_probe() -> &'static str {
    "ok"
}

fn acl_app() -> tauri::App<tauri::test::MockRuntime> {
    // The real ACL context macro emits process::exit on configuration errors.
    #[expect(clippy::exit, reason = "Tauri context macro expansion")]
    let context = tauri::generate_context!(test = true, assets = tauri::test::noop_assets());
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![acl_probe])
        .build(context)
        .expect("ACL mock app");
    for label in [WEB_BROWSER_WINDOW_LABEL, "main"] {
        WebviewWindowBuilder::new(&app, label, Default::default())
            .build()
            .unwrap();
    }
    app
}

const LOCAL: &str = crate::test_support::LOCAL_IPC_ORIGIN;

fn invoke(
    webview: &WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    url: &str,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: command.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: url.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
}

#[test]
fn remote_pages_in_the_browser_cannot_call_commands() {
    let app = acl_app();
    let browser = app.get_webview_window(WEB_BROWSER_WINDOW_LABEL).unwrap();
    for command in ["acl_probe", "plugin:app|version", "plugin:dialog|message"] {
        let error = invoke(&browser, command, "https://example.com/").unwrap_err();
        assert!(
            error.to_string().contains("not allowed"),
            "{command}: {error}"
        );
    }
}

#[test]
fn local_origin_is_the_positive_control() {
    let app = acl_app();
    let browser = app.get_webview_window(WEB_BROWSER_WINDOW_LABEL).unwrap();
    let body = invoke(&browser, "acl_probe", LOCAL).unwrap();
    assert_eq!(body.deserialize::<String>().unwrap(), "ok");
    let main = app.get_webview_window("main").unwrap();
    assert!(invoke(&main, "plugin:app|version", LOCAL).is_ok());
}

#[test]
fn a_remote_capability_flips_the_rejection() {
    let app = acl_app();
    app.add_capability(
        r#"{
        "identifier":"probe-remote", "windows":["web-browser"],
        "remote":{"urls":["https://example.com/*"]},
        "permissions":["core:app:allow-version","dialog:allow-message"]
    }"#,
    )
    .unwrap();
    let browser = app.get_webview_window(WEB_BROWSER_WINDOW_LABEL).unwrap();
    assert!(invoke(&browser, "plugin:app|version", "https://example.com/").is_ok());
    let error = invoke(&browser, "plugin:dialog|message", "https://example.com/").unwrap_err();
    assert!(!error.to_string().contains("not allowed"), "{error}");
    // Without an app ACL manifest, no capability can grant app commands remotely.
    let error = invoke(&browser, "acl_probe", "https://example.com/").unwrap_err();
    assert!(error.to_string().contains("not allowed"), "{error}");
}
