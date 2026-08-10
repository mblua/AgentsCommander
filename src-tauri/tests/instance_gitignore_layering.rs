use std::path::PathBuf;

fn source(relative_path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(relative_path)).expect("read guarded source")
}

#[test]
fn instance_gitignore_uses_the_root_agent_predicate_directly() {
    let instance_gitignore = source("src/config/instance_gitignore.rs");
    assert!(instance_gitignore.contains("root_agent::is_root_agent_path"));
    assert!(instance_gitignore.contains("is_root_agent_path"));
    assert!(!instance_gitignore.contains("as root_agent"));
}

#[test]
fn root_agent_remains_the_predicate_owner() {
    let root_agent = source("src/config/root_agent.rs");
    assert!(root_agent.contains("fn is_root_agent_path"));
}

#[test]
fn instance_gitignore_has_no_inverse_root_agent_dependency() {
    let root_agent = source("src/config/root_agent.rs");
    assert!(!root_agent.contains("instance_gitignore::"));
}
#[test]
fn instance_gitignore_calls_the_root_agent_predicate_for_both_entry_points() {
    let instance_gitignore = source("src/config/instance_gitignore.rs");
    let predicate_calls: Vec<_> = instance_gitignore
        .lines()
        .filter(|line| line.contains("is_root_agent_path("))
        .collect();
    let context = instance_gitignore
        .lines()
        .enumerate()
        .filter(|(index, _)| *index < 140)
        .map(|(index, line)| format!("{}: {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        predicate_calls.len() >= 2,
        "instance gitignore must call the root-agent predicate twice; candidates: {:?}\n{context}",
        instance_gitignore
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("ROOT_AGENT_DIR_NAME"))
            .map(|(index, line)| format!("{}: {}", index + 1, line))
            .collect::<Vec<_>>()
    );
}
