use std::path::PathBuf;

fn source(relative_path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(relative_path)).expect("read guarded source")
}

#[test]
fn project_settings_uses_the_commands_owned_emitter() {
    let project_settings = source("src/commands/project_settings.rs");
    assert!(project_settings.contains("web::commands::broadcast_all"));
    assert!(!project_settings.contains("web::event_broadcast"));
}

#[test]
fn commands_is_the_only_emitter_home() {
    let commands = source("src/web/commands.rs");
    let web_module = source("src/web/mod.rs");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    assert!(commands.contains("fn broadcast_all"));
    assert!(!commands.contains("web::event_broadcast"));
    assert!(!web_module.contains("mod event_broadcast"));
    assert!(!root.join("src/web/event_broadcast.rs").exists());
}

#[test]
fn project_settings_has_no_event_broadcast_detour() {
    let project_settings = source("src/commands/project_settings.rs");
    let commands = source("src/web/commands.rs");

    assert!(!project_settings.contains("event_broadcast"));
    assert!(!commands.contains("event_broadcast"));
}
#[test]
fn commands_has_no_project_settings_reverse_arc() {
    let commands = source("src/web/commands.rs");
    let references: Vec<_> = commands
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("commands::project_settings"))
        .map(|(index, line)| format!("{}: {}", index + 1, line))
        .collect();
    let context = commands
        .lines()
        .enumerate()
        .filter(|(index, _)| (740..780).contains(index))
        .map(|(index, line)| format!("{}: {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        references.is_empty(),
        "web commands still reference project settings: {references:?}\n{context}"
    );
}
