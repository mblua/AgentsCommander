use super::*;

#[test]
fn prepare_new_room_task_content_starts_clean_only_for_empty_titles() {
    for input in ["", "   "] {
        let content = prepare_new_room_task_content(input).unwrap();
        assert_eq!(content.as_bytes(), b"---\ntitle: 'Clean'\n---\n");
        assert!(!content.contains("USER:"));
        assert_eq!(build_task_content(input), content);
    }
    assert_eq!(
        prepare_new_room_task_content("  Clean  ").unwrap(),
        "---\ntitle: 'USER: Clean'\n---\n"
    );
}

#[test]
fn prepare_new_room_task_content_preserves_title_validation() {
    assert!(prepare_new_room_task_content(&"é".repeat(256)).is_ok());
    assert!(prepare_new_room_task_content(&"é".repeat(257)).is_err());
    for control in ['\n', '\r', '\0', '\u{7f}'] {
        assert!(prepare_new_room_task_content(&format!("a{control}b")).is_err());
    }
    assert_eq!(
        prepare_new_room_task_content(" a\tb ").unwrap(),
        "---\ntitle: 'USER: a\tb'\n---\n"
    );
    assert!(validate_task_title("").is_err());
}

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let command = source.split_once(signature).unwrap().1;
    let start = command.find('{').unwrap();
    let mut depth = 0;
    let end = command[start..]
        .char_indices()
        .find_map(|(offset, ch)| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(start + offset)
        })
        .unwrap();
    &command[start..=end]
}

#[test]
fn create_workgroup_prepares_task_once_before_mutation() {
    let source = include_str!("../team_create.rs");
    let body = function_body(source, "pub async fn create_workgroup(");
    let preparation = "let task_content = prepare_new_room_task_content(&task_title)?;";
    assert_eq!(body.matches("prepare_new_room_task_content(").count(), 1);
    assert!(body.find(preparation).unwrap() < body.find("ensure_ac_root_gitignore(").unwrap());
    assert!(body.find(preparation).unwrap() < body.find("allocate_gui_room(").unwrap());
    for forbidden in [
        "build_task_content(",
        "validate_task_title(",
        ".trim()",
        "Task title cannot be empty",
    ] {
        assert!(!body.contains(forbidden), "unexpected {forbidden}");
    }
    assert_eq!(body.matches("let task_content =").count(), 1);
    assert!(
        body.contains("allocate_gui_room(&base, &safe_team, &task_content, initialize_new_room)")
    );
    let helper = function_body(source, "pub(super) fn allocate_gui_room(");
    assert_eq!(helper.matches("write_task(").count(), 1);
    assert!(helper.contains("write_task(&wg_dir, task_content)"));
    assert!(helper.find("create_room_directory(").unwrap() < helper.find("write_task(").unwrap());
    assert!(!helper.contains("build_task_content("));
    assert!(!helper.contains("validate_task_title("));
    assert!(!helper.contains(".trim()"));
}

#[test]
fn create_workgroup_on_disk_preserves_task_creation_order() {
    let disk_source = include_str!("../workgroup.rs");
    let disk = function_body(
        disk_source,
        "pub(crate) async fn create_workgroup_on_disk_traced(",
    );
    assert!(
        disk.find("validate_task_title(&task_title)?;").unwrap()
            < disk.find("allocate_disk_room(").unwrap()
    );
    assert!(
        disk.contains("allocate_disk_room(&base, &safe_team, &task_title, initialize_new_room)")
    );
    let helper = function_body(disk_source, "pub(super) fn allocate_disk_room(");
    assert!(
        helper.find("create_room_shared_dir(").unwrap()
            < helper.find("build_task_content(").unwrap()
    );
    assert!(helper.find("build_task_content(").unwrap() < helper.find("write_task(").unwrap());
    assert_eq!(helper.matches("build_task_content(").count(), 1);
    assert_eq!(helper.matches("write_task(").count(), 1);
    assert!(helper.contains("write_task(&wg_dir, &task_content)"));
}

#[test]
fn room_allocation_precedes_reporting_and_initialization() {
    let source = include_str!("../team_create.rs");
    let body = function_body(source, "pub async fn create_workgroup(");
    let disk_source = include_str!("../workgroup.rs");
    let disk = function_body(
        disk_source,
        "pub(crate) async fn create_workgroup_on_disk_traced(",
    );

    for (entry, helper, input, replicas, clones) in [
        (
            body,
            "allocate_gui_room(",
            "task_content",
            "initialize_workgroup_replicas_on_disk_traced(",
            "git_clone_async(",
        ),
        (
            disk,
            "allocate_disk_room(",
            "task_title",
            "let mut replicas =",
            "clone_missing_repos_for_workgroup_traced(",
        ),
    ] {
        assert_eq!(entry.matches(helper).count(), 1);
        let compact: String = entry.chars().filter(|c| !c.is_whitespace()).collect();
        let routing = format!(".run(||{helper}&base,&safe_team,&{input},initialize_new_room))?;");
        assert_eq!(compact.matches(&routing).count(), 1);
        let after = compact.find(&routing).unwrap() + routing.len();
        for reporting in [
            "physical.room(",
            "operation.room(",
            replicas,
            clones,
            "Ok(WorkgroupCloneResult",
        ] {
            let reporting: String = reporting.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                after <= compact.find(&reporting).unwrap(),
                "order: {reporting}"
            );
        }
    }
}

fn initial_room_entries(root: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn issue_3294_initial_task_positive_control() {
    use super::super::room_task::initialize_new_room;
    use super::super::team_create::allocate_gui_room;
    use super::super::workgroup::allocate_disk_room;
    use std::io;

    let cases = [
        ("", "---\ntitle: 'Clean'\n---\n"),
        ("Clean", "---\ntitle: 'USER: Clean'\n---\n"),
        ("USER: Existing", "---\ntitle: 'USER: Existing'\n---\n"),
        ("Ana's task", "---\ntitle: 'USER: Ana''s task'\n---\n"),
    ];
    for disk in [false, true] {
        for (title, expected) in cases {
            if disk && title.is_empty() {
                continue; // Standalone validation rejects empty input before allocation.
            }
            if disk {
                validate_task_title(title).unwrap();
            }
            let prepared = prepare_new_room_task_content(title).unwrap();
            for fail in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let mut calls = 0;
                let mut allocated = None;
                let writer = |root: &std::path::Path, content: &str| {
                    calls += 1;
                    allocated = Some(root.to_path_buf());
                    assert_eq!(content.as_bytes(), expected.as_bytes());
                    assert!(root.is_dir());
                    assert!(root.join("messaging").is_dir());
                    assert_eq!(root.join("room-shared").is_dir(), disk);
                    let mut names = vec!["messaging"];
                    if disk {
                        names.push("room-shared");
                    }
                    assert_eq!(initial_room_entries(root), names);
                    if fail {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "task write denied",
                        ));
                    }
                    // Preserve write's truncation when a file appears at the write point.
                    std::fs::write(root.join("TASK.md"), "old bytes".repeat(100)).unwrap();
                    initialize_new_room(root, content)
                };
                let result = if disk {
                    allocate_disk_room(temp.path(), "dev", title, writer)
                } else {
                    allocate_gui_room(temp.path(), "dev", &prepared, writer)
                };
                assert_eq!(calls, 1);
                let root = allocated.unwrap();
                assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
                if fail {
                    assert_eq!(
                        result.unwrap_err(),
                        "Failed to write TASK.md: task write denied"
                    );
                    let mut names = vec!["messaging"];
                    if disk {
                        names.push("room-shared");
                    }
                    assert_eq!(initial_room_entries(&root), names);
                } else {
                    let (created, number) = result.unwrap();
                    assert_eq!(created, root);
                    assert_eq!(number, 1);
                    assert_eq!(
                        std::fs::read(root.join("TASK.md")).unwrap(),
                        expected.as_bytes()
                    );
                    let mut names = vec!["TASK.md", "messaging"];
                    if disk {
                        names.push("room-shared");
                    }
                    assert_eq!(initial_room_entries(&root), names);
                }
            }
        }
    }
}

// ── #738: build_task_content marks creation titles as user-owned ──

#[test]
fn build_task_content_prefixes_user_title() {
    assert_eq!(
        build_task_content("Build login"),
        "---\ntitle: 'USER: Build login'\n---\n"
    );
}

#[test]
fn build_task_content_does_not_double_prefix() {
    assert_eq!(
        build_task_content("USER: Existing"),
        "---\ntitle: 'USER: Existing'\n---\n"
    );
}

#[test]
fn build_task_content_escapes_quotes_after_prefix() {
    // The single quote is YAML-escaped (doubled) after the USER: prefix.
    assert_eq!(
        build_task_content("Ana's task"),
        "---\ntitle: 'USER: Ana''s task'\n---\n"
    );
}

// ── parse_task_title — dev-rust R7 cases ──

#[test]
fn parse_task_title_returns_some_for_canonical_frontmatter() {
    assert_eq!(
        parse_task_title("---\ntitle: Hello world\n---\n\nbody\n"),
        Some("Hello world".to_string())
    );
}

#[test]
fn parse_task_title_strips_double_quotes() {
    assert_eq!(
        parse_task_title("---\ntitle: \"Quoted\"\n---\n"),
        Some("Quoted".to_string())
    );
}

#[test]
fn parse_task_title_strips_single_quotes() {
    assert_eq!(
        parse_task_title("---\ntitle: 'Quoted'\n---\n"),
        Some("Quoted".to_string())
    );
}

#[test]
fn parse_task_title_returns_none_when_no_frontmatter() {
    assert_eq!(parse_task_title("# Heading\n\nbody\n"), None);
}

#[test]
fn parse_task_title_returns_none_for_empty_value() {
    assert_eq!(parse_task_title("---\ntitle:\n---\n"), None);
}

#[test]
fn parse_task_title_returns_none_when_closing_delimiter_missing() {
    assert_eq!(parse_task_title("---\ntitle: foo\nbody only\n"), None);
}

#[test]
fn parse_task_title_returns_none_when_title_field_absent() {
    assert_eq!(parse_task_title("---\nname: foo\n---\n"), None);
}

#[test]
fn parse_task_title_preserves_inner_colon() {
    assert_eq!(
        parse_task_title("---\ntitle: a: b\n---\n"),
        Some("a: b".to_string())
    );
}

#[test]
fn parse_task_title_handles_indented_key() {
    assert_eq!(
        parse_task_title("---\n  title: foo\n---\n"),
        Some("foo".to_string())
    );
}

// ── parse_task_title — dev-rust-grinch G3 / G13 case-insensitivity ──

#[test]
fn parse_task_title_handles_capital_t() {
    assert_eq!(
        parse_task_title("---\nTitle: Foo\n---\n"),
        Some("Foo".to_string())
    );
}

#[test]
fn parse_task_title_handles_all_caps_key() {
    assert_eq!(
        parse_task_title("---\nTITLE: Foo\n---\n"),
        Some("Foo".to_string())
    );
}

#[test]
fn parse_task_title_handles_mixed_case_key() {
    assert_eq!(
        parse_task_title("---\ntItLe: Foo\n---\n"),
        Some("Foo".to_string())
    );
}

#[test]
fn parse_task_title_value_remains_case_sensitive() {
    // The key match is case-insensitive; the value MUST round-trip
    // verbatim (it is user-visible content, not a structural marker).
    assert_eq!(
        parse_task_title("---\nTitle: MixedCASE Value\n---\n"),
        Some("MixedCASE Value".to_string())
    );
}

// ── parse_task_title — UTF-8 BOM (grinch MEDIUM) ──
// Mirrors `cli/task_ops.rs::parse_task` which already strips the BOM.
// Without this, TASK.md saved as "UTF-8 with BOM" breaks gate-4
// idempotency and risks silent overwrite of a user-edited title.

#[test]
fn parse_task_title_strips_utf8_bom() {
    assert_eq!(
        parse_task_title("\u{FEFF}---\ntitle: Foo\n---\n"),
        Some("Foo".to_string())
    );
}

#[test]
fn parse_task_title_returns_none_for_bom_without_frontmatter() {
    assert_eq!(parse_task_title("\u{FEFF}# Heading\n\nbody\n"), None);
}

// ── #177 — determine_next_wg_number lowest-free reuse ──
