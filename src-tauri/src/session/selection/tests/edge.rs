//! The issue's core cut stays independent of native state, events and tasks.

const CORE_PATHS: [&str; 10] = [
    "session/selection.rs",
    "session/selection/coordinator_runtime.rs",
    "session/selection/execute.rs",
    "session/selection/transaction.rs",
    "session/selection/continuation_runtime.rs",
    "session/selection/worker.rs",
    "session/selection/turn.rs",
    "co_managed.rs",
    "co_managed/route.rs",
    "session/selection/task.rs",
];

fn boundary_violations(source: &str) -> Vec<&'static str> {
    let scrubbed = rust_source_scrub::scrub(source, rust_source_scrub::Literals::Drop)
        .expect("valid Rust source for boundary inspection");
    let identifiers: Vec<_> = scrubbed
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .collect();
    let compact: String = scrubbed.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut violations = Vec::new();
    for name in ["tauri", "AppHandle"] {
        if identifiers.contains(&name) {
            violations.push(name);
        }
    }
    for (name, pattern) in [
        ("state", r"\b(?:state|try_state)\s*::\s*<"),
        ("emit", r"\.\s*(?:emit|emit_to)\s*(?:\(|::\s*<)"),
    ] {
        if regex::Regex::new(pattern).unwrap().is_match(&scrubbed) {
            violations.push(name);
        }
    }
    // Qualified native calls and aliases retain the tauri identifier above.
    // Ordinary unit/task/state-policy names and typed getters remain permitted.
    assert!(!compact.is_empty(), "boundary probe must inspect source");
    violations
}

const NEUTRAL: &str = r##"
// tauri::async_runtime::spawn(ignored); AppHandle; app.emit("ignored", ());
/* nested /* app.try_state::<State>(); */ native comment */
trait UnitTaskStatePolicy { fn state(&self) -> &State; }
fn policy(host: &impl UnitTaskStatePolicy) {
    let _ = host.state();
    let _ = serde_json::json!({"state": "tauri::AppHandle", "emit": "emit_to"});
    let _ = r#"app.emit("AppHandle", ());"#;
    let _ = 'x';
}
"##;

#[test]
fn issue_3950_boundary_scrub() {
    assert!(boundary_violations(NEUTRAL).is_empty());
    for (id, source, expected) in [
        (
            "aliased_native",
            "use tauri::{AppHandle as Native, Manager as StateAccess};",
            vec!["tauri", "AppHandle"],
        ),
        (
            "multiline_state",
            "fn bad(app: &Host) { app\n .try_state\n ::\n <State>(); }",
            vec!["state"],
        ),
        (
            "qualified_emit",
            "fn bad(app: &Host) { tauri::Emitter::emit(app, topic, payload); }",
            vec!["tauri"],
        ),
        (
            "qualified_spawn",
            "fn bad() { ::tauri::async_runtime::spawn(work); }",
            vec!["tauri"],
        ),
    ] {
        assert_eq!(boundary_violations(source), expected, "probe {id}");
        assert!(boundary_violations(NEUTRAL).is_empty());
        println!("\nISSUE3950_MUTANT_REJECTED case={id}");
    }
    for source in [
        "fn bad(app: &Host) { app.state::<State>(); }",
        "fn bad(app: &Host) { app.emit(topic, payload); }",
        "fn bad(app: &Host) { app.emit_to(target, topic, payload); }",
        "type Native = AppHandle<Runtime>;",
    ] {
        assert_eq!(boundary_violations(source).len(), 1, "{source}");
    }
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_boundary_scrub");
}

#[test]
fn issue_3950_boundary_each_path() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_eq!(CORE_PATHS.len(), 10);
    for path in CORE_PATHS {
        let source = std::fs::read_to_string(root.join(path)).expect("core source exists");
        assert!(
            boundary_violations(&source).is_empty(),
            "native infrastructure in {path}: {:?}",
            boundary_violations(&source)
        );
        let injected = format!("{source}\nuse tauri::AppHandle as Native;");
        assert_eq!(boundary_violations(&injected), vec!["tauri", "AppHandle"]);
    }
    // The unchanged source_ownership suite separately inspects the original
    // unstripped lifecycle event names across the entire production tree.
    println!("\nISSUE3950_CONTROL_OK case=issue_3950_boundary_each_path");
}
