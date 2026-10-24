use super::*;
use tempfile::tempdir;

#[test]
fn creates_folder_and_claude_md_matching_ui_modal() {
    let tmp = tempdir().expect("tempdir");
    let parent = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(&parent).expect("parent");
    let parent_s = parent.to_string_lossy().to_string();

    let created = create_agent_folder_on_disk(&parent_s, "architect").expect("created");

    let expected_dir = parent.join("architect");
    assert_eq!(created.agent_dir, expected_dir);
    assert_eq!(created.display_name, "ProjectAlpha/architect");
    assert_eq!(
        created.claude_md,
        "You are the agent ProjectAlpha/architect"
    );
    assert!(expected_dir.is_dir());
    assert_eq!(
        std::fs::read_to_string(expected_dir.join("CLAUDE.md")).expect("claude"),
        "You are the agent ProjectAlpha/architect"
    );
}

#[test]
fn trims_name_before_creating_folder_and_display_name() {
    let tmp = tempdir().expect("tempdir");
    let parent = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(&parent).expect("parent");
    let parent_s = parent.to_string_lossy().to_string();

    let created = create_agent_folder_on_disk(&parent_s, " MyAgent ").expect("created");

    let expected_dir = parent.join("MyAgent");
    assert_eq!(created.agent_dir, expected_dir);
    assert_eq!(created.display_name, "ProjectAlpha/MyAgent");
    assert_eq!(created.claude_md, "You are the agent ProjectAlpha/MyAgent");
    assert!(expected_dir.is_dir());
    assert!(!parent.join(" MyAgent ").exists());
}

#[test]
fn errors_when_parent_folder_is_missing() {
    let tmp = tempdir().expect("tempdir");
    let missing = tmp.path().join("missing");
    let missing_s = missing.to_string_lossy().to_string();

    let err = create_agent_folder_on_disk(&missing_s, "architect").expect_err("missing parent");

    assert_eq!(err, format!("Parent folder does not exist: {}", missing_s));
}

#[test]
fn errors_when_agent_name_is_empty_after_trim() {
    let tmp = tempdir().expect("tempdir");
    let parent_s = tmp.path().to_string_lossy().to_string();

    let err = create_agent_folder_on_disk(&parent_s, "   ").expect_err("empty");

    assert_eq!(err, "Agent name cannot be empty");
}

#[test]
fn errors_when_agent_name_contains_path_separator_or_nul() {
    let tmp = tempdir().expect("tempdir");
    let parent_s = tmp.path().to_string_lossy().to_string();

    for name in ["a/b", "a\\b", "a\0b"] {
        let err = create_agent_folder_on_disk(&parent_s, name).expect_err("separator");
        assert_eq!(err, "Agent name cannot contain path separators");
    }
}

#[test]
fn errors_when_agent_folder_already_exists_without_overwriting() {
    let tmp = tempdir().expect("tempdir");
    let parent = tmp.path().join("ProjectAlpha");
    let agent_dir = parent.join("architect");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(agent_dir.join("CLAUDE.md"), "keep me").expect("seed");
    let parent_s = parent.to_string_lossy().to_string();

    let err = create_agent_folder_on_disk(&parent_s, "architect").expect_err("exists");

    assert_eq!(
        err,
        format!("Folder already exists: {}", agent_dir.display())
    );
    assert_eq!(
        std::fs::read_to_string(agent_dir.join("CLAUDE.md")).expect("claude"),
        "keep me"
    );
}
