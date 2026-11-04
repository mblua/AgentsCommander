use super::*;

fn with_preflight(
    agent_pf: Option<&str>,
    agent_t: Option<u32>,
    cell_pf: Option<&str>,
    cell_t: Option<u32>,
) -> AppSettings {
    let mut c = cell("", BTreeMap::new());
    c.preflight = cell_pf.map(str::to_string);
    c.preflight_timeout_seconds = cell_t;
    let mut settings = settings_with_cell("codex", "A", c);
    settings.agents[0].preflight = agent_pf.map(str::to_string);
    settings.agents[0].preflight_timeout_seconds = agent_t;
    settings
}

fn resolution(settings: &AppSettings, letter: &str) -> ProfileResolution {
    crate::config::coding_agent_profiles::resolve_profile(
        settings,
        crate::config::coding_agent_profiles::ProfileResolutionRequest {
            coding_agent_id: "codex",
            launch_path: None,
            agent_matrix_name: None,
            requested_profile: Some(letter),
            requested_profile_authoritative: true,
        },
    )
}

fn stages(settings: &AppSettings) -> Result<Vec<PreflightStage>, String> {
    resolve_preflight_stages(&settings.agents[0], &resolution(settings, "A"), None, None)
}

fn sources(settings: &AppSettings) -> Vec<PreflightStageSource> {
    stages(settings)
        .unwrap()
        .into_iter()
        .map(|s| s.source)
        .collect()
}

fn id_of(settings: &AppSettings) -> String {
    cell_identity(&settings.agents[0], &resolution(settings, "A").cell)
}

fn legacy_id(settings: &AppSettings, cell: &ProfileCellConfig) -> String {
    let agent = &settings.agents[0];
    profile_content_hash(
        &compose_effective_command(&agent.command, &cell.command),
        &raw_merged_profile_env(agent, &cell.env),
    )
}

#[test]
fn stages_cover_four_combinations() {
    use PreflightStageSource::{CodingAgent, Profile};
    assert!(sources(&with_preflight(None, None, None, None)).is_empty());
    assert_eq!(
        sources(&with_preflight(Some("ca"), None, None, None)),
        [CodingAgent]
    );
    assert_eq!(
        sources(&with_preflight(None, None, Some("p"), None)),
        [Profile]
    );
    assert_eq!(
        sources(&with_preflight(Some("ca"), None, Some("p"), None)),
        [CodingAgent, Profile]
    );
}

#[test]
fn whitespace_only_is_not_configured() {
    let blank = with_preflight(Some("   "), None, Some("   "), None);
    assert_eq!(stages(&blank), Ok(Vec::new()));
    assert_eq!(
        id_of(&blank),
        id_of(&with_preflight(None, None, None, None))
    );
}

#[test]
fn fallback_uses_effective_cell() {
    let mut settings = with_preflight(None, None, Some("a.cmd"), None);
    let mut b = cell("", BTreeMap::new());
    b.enabled = false;
    b.preflight = Some("b.cmd".to_string());
    let cells = settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap();
    cells.insert("B".to_string(), b);
    let res = resolution(&settings, "B");
    let got = resolve_preflight_stages(&settings.agents[0], &res, None, None).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].command_text, "a.cmd");
    assert_eq!(got[0].label, "Profile A");

    let cells = settings
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap();
    cells.insert("B".to_string(), cell("", BTreeMap::new()));
    let res = resolution(&settings, "B");
    assert_eq!(res.effective_profile, "B");
    let got = resolve_preflight_stages(&settings.agents[0], &res, None, None).unwrap();
    assert!(
        got.is_empty(),
        "enabled empty B must not inherit A: {got:?}"
    );
}

#[test]
fn timeouts_are_per_stage() {
    let t = |s: &AppSettings| -> Vec<u32> {
        stages(s)
            .unwrap()
            .iter()
            .map(|s| s.timeout_seconds)
            .collect()
    };
    assert_eq!(
        t(&with_preflight(Some("a"), None, Some("p"), Some(5))),
        [60, 5]
    );
    assert_eq!(
        t(&with_preflight(Some("a"), Some(9), Some("p"), None)),
        [9, 60]
    );
}

#[test]
fn zero_timeout_is_rejected() {
    let err = stages(&with_preflight(Some("a"), Some(0), None, None)).unwrap_err();
    assert!(err.contains("at least 1 second"), "{err}");
    let err = stages(&with_preflight(None, None, Some("p"), Some(0))).unwrap_err();
    assert!(err.starts_with("Profile A preflight timeout"), "{err}");
    assert_eq!(
        stages(&with_preflight(None, Some(0), None, Some(0))),
        Ok(Vec::new())
    );
}

#[test]
fn quoted_path_with_spaces_tokenizes() {
    let got = stages(&with_preflight(
        Some("\"C:\\x y\\pre.cmd\" --flag"),
        None,
        None,
        None,
    ));
    let got = got.unwrap();
    assert_eq!(got[0].program, "C:\\x y\\pre.cmd");
    assert_eq!(got[0].args, ["--flag"]);
    let got = stages(&with_preflight(
        Some("sh '/tmp/a b/pre.sh'"),
        None,
        None,
        None,
    ))
    .unwrap();
    assert_eq!(got[0].program, "sh");
    assert_eq!(got[0].args, ["/tmp/a b/pre.sh"]);
}

#[test]
fn invalid_command_is_carried_not_raised() {
    let settings = with_preflight(Some("pre \"unclosed"), None, None, None);
    let err = stages(&settings).unwrap_err();
    assert!(
        err.starts_with("Invalid Coding Agent preflight command"),
        "{err}"
    );
    let spawn = resolve_agent_spawn_command(&settings, "codex", None, Some("A"), false).unwrap();
    assert_eq!(spawn.preflight_stages, Err(err));
}

#[test]
fn placeholder_without_launch_path_fails() {
    let settings = with_preflight(Some("%AC_REPLICA_ROOT%\\pre.cmd"), None, None, None);
    assert!(stages(&settings).is_err());
    assert_eq!(
        stages(&with_preflight(None, None, None, None)),
        Ok(Vec::new())
    );
}

#[test]
fn empty_chain_identity_is_legacy_bytes() {
    let env = BTreeMap::from([("K".to_string(), "v".to_string())]);
    let plain = settings_with_cell("codex", "A", cell("--p", BTreeMap::new()));
    let with_env = settings_with_cell("codex", "A", cell("--p", env));
    let mut fallback = settings_with_cell("codex", "A", cell("--a", BTreeMap::new()));
    let mut b = cell("--b", BTreeMap::new());
    b.enabled = false;
    let cells = fallback
        .coding_agent_profiles
        .profiles_by_agent
        .get_mut("codex")
        .unwrap();
    cells.insert("B".to_string(), b);
    for (settings, letter) in [(&plain, "A"), (&with_env, "A"), (&fallback, "B")] {
        let cell = resolution(settings, letter).cell;
        assert_eq!(
            cell_identity(&settings.agents[0], &cell),
            legacy_id(settings, &cell)
        );
    }
}

#[test]
fn configured_chain_changes_identity() {
    let id =
        |a: Option<&str>, at: Option<u32>, p: Option<&str>| id_of(&with_preflight(a, at, p, None));
    assert_ne!(id(None, None, None), id(Some("a"), None, None));
    assert_ne!(id(Some("a"), None, None), id(Some("b"), None, None));
    assert_ne!(id(Some("a"), Some(5), None), id(Some("a"), None, None));
    assert_ne!(
        id(Some("a"), None, Some("b")),
        id(Some("b"), None, Some("a"))
    );
    assert_ne!(id(Some("a b"), None, None), id(Some("\"a b\""), None, None));
    assert_eq!(id(Some("a"), None, None), id(Some("a"), Some(60), None));
    assert_ne!(id(Some("a"), None, None), id(None, None, Some("a")));
    assert_eq!(id(Some("a  b"), None, None), id(Some("a b"), None, None));
}

#[test]
fn spawn_carries_stages_and_identity() {
    let settings = with_preflight(Some("a"), None, Some("p"), None);
    let spawn = resolve_agent_spawn_command(&settings, "codex", None, Some("A"), false).unwrap();
    assert_eq!(spawn.preflight_stages.as_ref().unwrap().len(), 2);
    assert_eq!(
        spawn.profile_content_hash,
        cell_identity(&settings.agents[0], &spawn.profile_resolution.cell)
    );
}

#[test]
fn frame_is_length_prefixed() {
    let frame = |text: &str| {
        let settings = with_preflight(Some(text), None, None, None);
        let agent = &settings.agents[0];
        let cell = resolution(&settings, "A").cell;
        (
            preflight_identity_frame(agent, &cell).unwrap(),
            cell_identity(agent, &cell),
        )
    };
    let left = frame("\"p\0a\0q\" r");
    let right = frame("p \"q\0a\0r\"");
    assert_ne!(left.0, right.0);
    assert_ne!(left.1, right.1);
    assert!(frame("\u{e9}").0.contains("a\u{0}2\u{0}\u{e9}"));
}

#[test]
fn invalid_preflight_keeps_read_only_callers() {
    let settings = with_preflight(Some("pre \"unclosed"), None, None, None);
    let cwd = std::env::temp_dir();
    let spawn = build_agent_spawn_command(&settings, "codex", Some(&cwd), None).unwrap();
    assert!(spawn.preflight_stages.is_err());
    assert_eq!(
        spawn.profile_content_hash,
        cell_identity(&settings.agents[0], &spawn.profile_resolution.cell)
    );
}
