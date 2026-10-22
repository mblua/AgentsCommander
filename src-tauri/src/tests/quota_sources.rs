use super::*;
use crate::config::settings::{AgentConfig, AppSettings, QuotaSourceConfig, QuotaSourceEntry};

const PATTERN: &str = r"Weekly (\d{1,3})% used";
const OTHER: &str = r"Week (\d{1,3})%";

fn screen(pattern: &str, enabled: bool) -> QuotaSourceEntry {
    QuotaSourceEntry::Valid(QuotaSourceConfig::ScreenRegex {
        pattern: pattern.to_string(),
        enabled,
    })
}

fn spec(pattern: &str) -> SourceSpec {
    SourceSpec::ScreenRegex {
        pattern: pattern.to_string(),
    }
}

fn agent(id: &str, command: &str) -> AgentConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "label": id,
        "command": command,
        "color": "#000000",
    }))
    .expect("a minimal agent config")
}

fn map(
    agents: Vec<(&str, &str)>,
    entries: Vec<(&str, QuotaSourceEntry)>,
) -> HashMap<String, AgentQuotaSource> {
    let mut settings = AppSettings {
        agents: agents
            .into_iter()
            .map(|(id, command)| agent(id, command))
            .collect(),
        ..AppSettings::default()
    };
    for (agent_id, entry) in entries {
        settings.quota_sources.insert(agent_id.to_string(), entry);
    }
    QuotaSources::sources_map(&settings)
}

/// Every entry's agent exists and runs `claude`: the pre-#2566 adapter cases.
fn specs(entries: Vec<(&str, QuotaSourceEntry)>) -> HashMap<String, AgentQuotaSource> {
    let agents = entries.iter().map(|(id, _)| (*id, "claude")).collect();
    map(agents, entries)
}

#[test]
fn quota_sources_adapter_skips_a_disabled_entry() {
    // Separate commands, so the enabled entry cannot fill the disabled one.
    let specs = map(
        vec![("off", "off-cli"), ("on", "on-cli")],
        vec![
            ("off", screen(PATTERN, false)),
            ("on", screen(PATTERN, true)),
        ],
    );
    assert!(!specs.contains_key("off"));
    assert!(specs.contains_key("on"));
}

#[test]
fn quota_sources_adapter_skips_an_invalid_entry() {
    let specs = specs(vec![(
        "codex",
        QuotaSourceEntry::Invalid(serde_json::json!({ "kind": "fromTheFuture" })),
    )]);
    assert!(specs.is_empty());
}

#[test]
fn quota_sources_adapter_skips_a_blank_pattern_but_hands_over_untrimmed_bytes() {
    let anchored = "  Weekly (\\d{1,3})% used ";
    let specs = map(
        vec![("blank", "blank-cli"), ("anchored", "claude")],
        vec![
            ("blank", screen("   ", true)),
            ("anchored", screen(anchored, true)),
        ],
    );
    assert!(!specs.contains_key("blank"));
    assert_eq!(
        specs.get("anchored").map(|src| &src.spec),
        Some(&spec(anchored)),
        "the leading spaces are the column anchor and must survive"
    );
}

fn remaining(pattern: &str, enabled: bool) -> QuotaSourceEntry {
    QuotaSourceEntry::Valid(QuotaSourceConfig::ScreenRegexRemaining {
        pattern: pattern.to_string(),
        enabled,
    })
}

#[test]
fn the_adapter_maps_a_remaining_entry_to_a_remaining_spec() {
    let pattern = r"Weekly (\d{1,3})% left";
    let specs = specs(vec![("codex", remaining(pattern, true))]);
    assert_eq!(
        specs.get("codex").map(|src| &src.spec),
        Some(&SourceSpec::ScreenRegexRemaining {
            pattern: pattern.to_string(),
        })
    );
}

#[test]
fn the_adapter_skips_a_disabled_or_blank_remaining_entry() {
    let anchored = "  Weekly (\\d+)% left";
    let specs = map(
        vec![
            ("off", "off-cli"),
            ("blank", "blank-cli"),
            ("anchored", "claude"),
        ],
        vec![
            ("off", remaining(PATTERN, false)),
            ("blank", remaining("   ", true)),
            ("anchored", remaining(anchored, true)),
        ],
    );
    assert!(!specs.contains_key("off"));
    assert!(!specs.contains_key("blank"));
    assert_eq!(
        specs.get("anchored").map(|src| &src.spec),
        Some(&SourceSpec::ScreenRegexRemaining {
            pattern: anchored.to_string(),
        }),
        "the leading spaces are the column anchor and must survive"
    );
}

#[test]
fn quota_sources_adapter_returns_an_empty_map_for_empty_settings() {
    assert!(QuotaSources::sources_map(&AppSettings::default()).is_empty());
}

#[test]
fn sources_map_is_empty_when_quota_sources_is_empty() {
    let sources = map(vec![("a1", "claude"), ("a2", "codex")], vec![]);
    assert!(sources.is_empty());
}

#[test]
fn sources_map_skips_an_agent_whose_id_has_no_agent_config() {
    let sources = map(vec![], vec![("deleted", screen(PATTERN, true))]);
    assert!(sources.is_empty());
}

#[test]
fn sources_map_skips_an_agent_whose_command_has_no_account_key() {
    let sources = map(vec![("a1", "")], vec![("a1", screen(PATTERN, true))]);
    assert!(sources.is_empty());
}

#[test]
fn sources_map_gives_two_agents_on_one_command_the_same_account_key() {
    let sources = map(
        vec![
            ("a1", "claude"),
            ("a2", "claude --dangerously-skip-permissions"),
        ],
        vec![("a1", screen(PATTERN, true)), ("a2", screen(PATTERN, true))],
    );
    assert_eq!(sources["a1"].account_key, "claude");
    assert_eq!(sources["a2"].account_key, "claude");
}

#[test]
fn sources_map_includes_an_agent_that_configured_nothing_when_a_sibling_on_its_command_is_enabled()
{
    let sources = map(
        vec![("a1", "claude"), ("a2", "claude")],
        vec![("a1", screen(PATTERN, true))],
    );
    let expected = AgentQuotaSource {
        account_key: "claude".to_string(),
        spec: spec(PATTERN),
    };
    assert_eq!(sources.get("a1"), Some(&expected));
    assert_eq!(sources.get("a2"), Some(&expected));
}

#[test]
fn sources_map_prefers_an_agents_own_enabled_pattern_over_the_key_winner() {
    let sources = map(
        vec![("a1", "claude"), ("a2", "claude")],
        vec![("a1", screen(PATTERN, true)), ("a2", screen(OTHER, true))],
    );
    assert_eq!(sources["a1"].spec, spec(PATTERN));
    assert_eq!(sources["a2"].spec, spec(OTHER));
}

#[test]
fn sources_map_tie_breaks_the_key_winner_by_lowest_agent_id() {
    let sources = map(
        vec![("z", "claude"), ("a2", "claude"), ("a1", "claude")],
        vec![("a2", screen(OTHER, true)), ("a1", screen(PATTERN, true))],
    );
    assert_eq!(sources["z"].spec, spec(PATTERN));
}

#[test]
fn sources_map_drops_a_key_whose_only_source_is_disabled() {
    let sources = map(
        vec![("a1", "claude"), ("a2", "claude")],
        vec![("a1", screen(PATTERN, false))],
    );
    assert!(sources.is_empty());
}

#[test]
fn sources_map_fills_a_disabled_agent_from_an_enabled_sibling_on_the_same_command() {
    let sources = map(
        vec![("a1", "claude"), ("a2", "claude")],
        vec![("a1", screen(PATTERN, false)), ("a2", screen(OTHER, true))],
    );
    let expected = AgentQuotaSource {
        account_key: "claude".to_string(),
        spec: spec(OTHER),
    };
    assert_eq!(sources.get("a1"), Some(&expected));
    assert_eq!(sources.get("a2"), Some(&expected));
    assert!(
        sources.values().all(|src| src.spec != spec(PATTERN)),
        "a1's own disabled pattern appears nowhere"
    );
}
