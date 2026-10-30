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
