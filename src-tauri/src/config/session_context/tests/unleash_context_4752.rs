use super::*;

// #4752 P03: an unleashed replica's context drops the GOLDEN RULE and the two
// "Filesystem authorization amendment" paragraphs, and keeps everything else.
const GOLDEN_HEADING: &str = "## GOLDEN RULE";
const INTEGRITY_HEADING: &str = "## Integrity Rules";
const AMENDMENT: &str = "Filesystem authorization amendment";
const AMENDMENT_END: &str =
    "This read authorization also applies when no preceding filesystem rule exists.\n\n";

struct Fixture {
    _temp: tempfile::TempDir,
    replica_root: PathBuf,
    matrix_root: String,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, _room_root, replica_root, matrix_root) = make_valid_replica(temp.path());
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        get_default_agent_template(),
    )
    .expect("materialize the default template on disk");
    write_skill(
        Path::new(&matrix_root),
        "agent-skill",
        "---\nname: agent-skill\ndescription: Agent metadata\n---\n",
    );
    let project_skill = ac_root
        .join("project-skills")
        .join("project-skill")
        .join("SKILL.md");
    std::fs::create_dir_all(project_skill.parent().unwrap()).unwrap();
    std::fs::write(
        &project_skill,
        "---\nname: project-skill\ndescription: Project metadata\n---\n",
    )
    .unwrap();
    let team = ac_root.join("_team_dev-team");
    std::fs::create_dir_all(&team).unwrap();
    std::fs::write(
        team.join("config.json"),
        serde_json::json!({"agents": ["../_agent_dev-rust"]}).to_string(),
    )
    .unwrap();
    let team_skill = team.join("team-skills").join("team-skill").join("SKILL.md");
    std::fs::create_dir_all(team_skill.parent().unwrap()).unwrap();
    std::fs::write(
        &team_skill,
        "---\nname: team-skill\ndescription: Team metadata\n---\n",
    )
    .unwrap();
    Fixture {
        _temp: temp,
        replica_root,
        matrix_root,
    }
}

fn render(fixture: &Fixture, restrictions: WorkspaceRestrictions) -> String {
    let agent_root = display_path(&fixture.replica_root);
    let matrix_root = Some(fixture.matrix_root.as_str());
    let index = discover_skill_index(matrix_root);
    let skills = render_skills_section(&index);
    let mut out = resolve_agent_context_with_activation(
        &agent_root,
        matrix_root,
        &skills,
        &fixture.replica_root,
        None,
        None,
        None,
        restrictions,
    )
    .expect("resolve the materialized template");
    out.push_str(&project_skills_for_session(
        &agent_root,
        matrix_root,
        &index,
        restrictions,
    ));
    out.push_str(&team_skills_for_session(
        &agent_root,
        matrix_root,
        restrictions,
    ));
    out
}

/// Cuts the GOLDEN RULE block (up to the Integrity Rules heading) and every
/// amendment sentence group out of a leashed render.
fn cut_removed_blocks(leashed: &str) -> String {
    let golden = leashed.find(GOLDEN_HEADING).expect("leashed GOLDEN RULE");
    let integrity = leashed.find(INTEGRITY_HEADING).expect("leashed integrity");
    let mut out = format!("{}{}", &leashed[..golden], &leashed[integrity..]);
    while let Some(start) = out.find(AMENDMENT) {
        let len = out[start..].find(AMENDMENT_END).expect("amendment end");
        out.replace_range(start..start + len + AMENDMENT_END.len(), "");
    }
    out
}

#[test]
fn p03_lifted_context_has_no_golden_rule_and_keeps_integrity_block() {
    let fixture = fixture();
    let out = render(&fixture, WorkspaceRestrictions::Lifted);

    assert!(
        !out.contains(GOLDEN_HEADING),
        "lifted context keeps GOLDEN RULE"
    );
    assert!(
        !out.contains(AMENDMENT),
        "lifted context keeps an amendment"
    );
    assert_eq!(out.matches(INTEGRITY_HEADING).count(), 1);
}

#[test]
fn p03_lifted_context_equals_leashed_minus_removed_blocks() {
    let fixture = fixture();
    let leashed = render(&fixture, WorkspaceRestrictions::Enforced);
    let lifted = render(&fixture, WorkspaceRestrictions::Lifted);

    assert_eq!(leashed.matches(GOLDEN_HEADING).count(), 1);
    assert_eq!(leashed.matches(AMENDMENT).count(), 2);
    assert_eq!(cut_removed_blocks(&leashed), lifted);
}

#[test]
fn p03_lifted_skills_sections_keep_lists_and_drop_amendments() {
    let fixture = fixture();
    let out = render(&fixture, WorkspaceRestrictions::Lifted);

    assert!(out.contains("## Project Skills\n\nCanonical project skills root: `"));
    assert!(out.contains("`project:project-skill`"));
    assert!(out.contains("### Team dev-team\n\nCanonical team skills root: `"));
    assert!(out.contains("`team:dev-team:team-skill`"));
    assert!(!out.contains(AMENDMENT));
}

#[test]
fn p03_root_agent_is_always_enforced() {
    let temp = tempfile::tempdir().expect("tempdir");
    let agent_root = display_path(temp.path());
    let out = render_agent_context_template_inner(
        get_default_agent_template(),
        &agent_root,
        None,
        &no_skill_section(),
        temp.path(),
        None,
        None,
        true,
        WorkspaceRestrictions::Lifted,
    );

    assert!(
        out.contains(GOLDEN_HEADING),
        "Root must keep the GOLDEN RULE"
    );
}

#[test]
fn p03_lifted_output_has_no_unleash_wording() {
    let fixture = fixture();
    let out = render(&fixture, WorkspaceRestrictions::Lifted);

    assert!(!out.to_lowercase().contains("unleash"));
}

#[test]
fn p03_restrictions_for_lifts_only_local_unleashed() {
    assert_eq!(restrictions_for(false, true), WorkspaceRestrictions::Lifted);
    assert_eq!(
        restrictions_for(true, true),
        WorkspaceRestrictions::Enforced
    );
    assert_eq!(
        restrictions_for(false, false),
        WorkspaceRestrictions::Enforced
    );
    assert_eq!(
        restrictions_for(true, false),
        WorkspaceRestrictions::Enforced
    );
}
