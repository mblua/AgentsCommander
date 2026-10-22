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

#[test]
fn create_workgroup_prepares_task_once_before_mutation() {
    let source = include_str!("../../entity_creation.rs");
    let command = source
        .split_once("pub async fn create_workgroup(")
        .unwrap()
        .1;
    let body_start = command.find('{').unwrap();
    let mut depth = 0;
    let body_end = command[body_start..]
        .char_indices()
        .find_map(|(offset, ch)| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(body_start + offset)
        })
        .unwrap();
    let body = &command[body_start..=body_end];
    let preparation = "let task_content = prepare_new_room_task_content(&task_title)?;";
    assert_eq!(body.matches("prepare_new_room_task_content(").count(), 1);
    assert!(body.find(preparation).unwrap() < body.find("ensure_ac_root_gitignore(").unwrap());
    assert!(body.find(preparation).unwrap() < body.find("std::fs::create_dir_all(").unwrap());
    for forbidden in [
        "build_task_content(",
        "validate_task_title(",
        ".trim()",
        "Task title cannot be empty",
    ] {
        assert!(!body.contains(forbidden), "unexpected {forbidden}");
    }
    assert_eq!(body.matches("let task_content =").count(), 1);
    assert!(body.contains("std::fs::write(wg_dir.join(\"TASK.md\"), &task_content)"));

    // CLI/on-disk creation still validates before generating task content.
    let cli = source
        .split_once("pub(crate) async fn create_workgroup_on_disk(")
        .unwrap()
        .1;
    assert!(
        cli.find("validate_task_title(&task_title)?;").unwrap()
            < cli.find("build_task_content(&task_title)").unwrap()
    );
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
