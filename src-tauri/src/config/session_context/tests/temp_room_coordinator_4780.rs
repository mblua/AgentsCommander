use super::*;

const SECTION: &str = "## Temporary Rooms";

fn replica_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let matrix = ac.join("_agent_dev-rust");
    let replica = ac.join("wg-19-dev-team").join("__agent_dev-rust");
    std::fs::create_dir_all(&matrix).expect("create matrix root");
    std::fs::create_dir_all(&replica).expect("create replica root");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_dev-rust","context":["$AGENTSCOMMANDER_CONTEXT"]}"#,
    )
    .expect("write replica config");
    (temp, matrix, replica)
}

fn render(dir: &std::path::Path, is_coordinator: bool) -> String {
    let path = materialize_agent_context_file(
        &path_string(dir),
        ManagedContextTarget::Codex,
        is_coordinator,
    )
    .expect("materialize context")
    .expect("context path");
    std::fs::read_to_string(path).expect("read materialized context")
}

#[test]
fn coordinator_replica_context_has_temp_room_section() {
    let (_temp, _matrix, replica) = replica_fixture();
    let content = render(&replica, true);
    for needle in [
        SECTION,
        "temp-room-trNN-to-<roomN>-<you>-<slug>.md",
        "to-temp-room-trNN-<slug>.md",
        "agentscommander://temp-room/trNN",
        "MAY READ exactly that one file",
        "additional exception to the GOLDEN RULE entry ranges",
        "forbidden-read scope",
        "instruction and the Integrity Rules filename patterns above",
        "restrictions remain in force for all other paths",
    ] {
        assert!(content.contains(needle), "missing {needle:?}");
        assert!(
            TEMP_ROOM_COORDINATOR_CONTEXT
                .lines()
                .any(|line| line.contains(needle)),
            "{needle:?} must be on one source line"
        );
    }
    let orchestrator = content
        .find("# Orchestrator Context")
        .expect("orchestrator heading");
    let section = content.find(SECTION).expect("temp-room section");
    assert!(
        section > orchestrator,
        "section must follow # Orchestrator Context"
    );
}

#[test]
fn member_replica_context_has_no_temp_room_section() {
    let (_temp, _matrix, replica) = replica_fixture();
    assert!(!render(&replica, false).contains(SECTION));
}

#[test]
fn matrix_coordinator_context_has_no_temp_room_section() {
    let (_temp, matrix, _replica) = replica_fixture();
    let content = render(&matrix, true);
    assert!(content.contains("# Orchestrator Context"));
    assert!(!content.contains(SECTION));
}

#[test]
fn temp_room_coordinator_text_lines_fit() {
    for line in TEMP_ROOM_COORDINATOR_CONTEXT.lines() {
        assert!(line.chars().count() <= 100, "line too long: {line:?}");
        assert!(!line.contains('{'), "brace in line: {line:?}");
    }
}
