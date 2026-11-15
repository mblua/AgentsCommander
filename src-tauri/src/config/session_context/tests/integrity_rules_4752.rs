use super::*;

// #4752 P01: the D5 integrity rules render in their own block after the GOLDEN RULE.
const GOLDEN_HEADING: &str = "## GOLDEN RULE";
const INTEGRITY_HEADING: &str = "## Integrity Rules";
const MOVED_SENTENCES: [&str; 5] = [
    "Strictly limited to canonical inter-agent message files",
    "Do NOT modify or delete any message file once written",
    "Never create, edit, move, delete or overwrite",
    "Room status is at",
    "Direct filesystem access (reads/writes/deletes/renames)",
];

fn render_room_replica(temp: &Path) -> (String, PathBuf, String) {
    let (_ac_root, room_root, replica_root, matrix_root) = make_valid_replica(temp);
    let out = super::super::default_context(
        &display_path(&replica_root),
        Some(&matrix_root),
        &no_skill_section(),
    );
    (out, room_root, matrix_root)
}

fn integrity_block(out: &str) -> &str {
    let start = out
        .find(INTEGRITY_HEADING)
        .expect("the room replica must render the Integrity Rules block");
    let rest = &out[start + INTEGRITY_HEADING.len()..];
    let end = rest
        .find("\n## ")
        .map_or(out.len(), |i| start + INTEGRITY_HEADING.len() + i);
    &out[start..end]
}

#[test]
fn p01_room_replica_renders_golden_then_integrity_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (out, _room_root, _matrix_root) = render_room_replica(temp.path());

    assert_eq!(out.matches(GOLDEN_HEADING).count(), 1);
    assert_eq!(out.matches(INTEGRITY_HEADING).count(), 1);
    let golden = out.find(GOLDEN_HEADING).expect("golden heading");
    let integrity = out.find(INTEGRITY_HEADING).expect("integrity heading");
    assert!(golden < integrity, "the GOLDEN RULE must render first");
    assert!(
        out[..integrity].ends_with("Otherwise refuse.\n\n"),
        "only `\\n\\n` may separate the GOLDEN RULE from the Integrity Rules block"
    );
}

#[test]
fn p01_golden_rule_holds_no_integrity_sentence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (out, _room_root, _matrix_root) = render_room_replica(temp.path());

    let golden = out.find(GOLDEN_HEADING).expect("golden heading");
    let integrity = out.find(INTEGRITY_HEADING).expect("integrity heading");
    let golden_block = &out[golden..integrity];
    for sentence in MOVED_SENTENCES {
        assert!(
            !golden_block.contains(sentence),
            "the GOLDEN RULE must not hold `{sentence}`"
        );
    }
}

#[test]
fn p01_integrity_block_holds_messaging_and_task_rules() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (out, room_root, _matrix_root) = render_room_replica(temp.path());
    let block = integrity_block(&out);

    let messaging = display_path(&room_root.join("messaging"));
    let task = display_path(&room_root.join("TASK.md"));
    for literal in [
        messaging.as_str(),
        "YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md",
        "Do NOT modify or delete",
        task.as_str(),
        "task-set-body",
        "task-get --token",
        "task-status-set --token",
        "TASK-clean.pending.json",
    ] {
        assert!(
            block.contains(literal),
            "Integrity Rules must hold `{literal}`"
        );
    }
    for sentence in [
        "trictly limited to canonical inter-agent message files",
        "Do NOT modify or delete any message file once written",
        "ever create, edit, move, delete or overwrite",
        "Room status is at",
        "Direct filesystem access (reads/writes/deletes/renames)",
    ] {
        assert!(
            block.contains(sentence),
            "Integrity Rules must hold `{sentence}`"
        );
    }
}

#[test]
fn p01_non_room_replica_has_no_integrity_block() {
    let out =
        super::super::default_context("C:/fake/.ac/_agent_dev-rust", None, &no_skill_section());

    assert_eq!(out.matches(GOLDEN_HEADING).count(), 1);
    assert!(
        !out.contains(INTEGRITY_HEADING),
        "a direct-Matrix agent must render no Integrity Rules block"
    );
}
