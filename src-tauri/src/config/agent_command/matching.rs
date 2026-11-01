use std::collections::BTreeMap;

use super::{canonical_command_text, cell_identity};
use crate::config::settings::{AgentConfig, AppSettings};

/// #2434 - which tier of `resolve_portable_reference` matched. A tier-0 local
/// id hit guesses nothing and is `None` wherever this is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchTier {
    Hash,
    LabelAndLetter,
    CommandAndLetter,
}

impl MatchTier {
    /// The wire string the #2451 `matchTier` carrier holds; equals the serde form.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            MatchTier::Hash => "hash",
            MatchTier::LabelAndLetter => "labelAndLetter",
            MatchTier::CommandAndLetter => "commandAndLetter",
        }
    }
}

/// #2434 - a stored coding-agent reference, built by the caller from
/// `tooling.currentCodingAgent` / `lastCodingAgent` plus, when present, the
/// `tooling.codingAgents[<id>]` descriptor #2433 wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredReference {
    /// The stored id; it IS the reference.
    pub id: String,
    /// `descriptor.app`.
    pub app_label: Option<String>,
    /// `descriptor.command`, already canonical.
    pub command: Option<String>,
    /// Letter -> cell digest; empty when an older build wrote the descriptor.
    pub identity: BTreeMap<String, String>,
    /// The letter this launch asks for.
    pub source_letter: Option<String>,
}

impl StoredReference {
    /// Build the reference for `id` from a parsed agent `config.json`. A missing
    /// or non-object descriptor yields no descriptor fields; empty strings read
    /// as absent (an older build wrote them).
    pub fn from_config(config: Option<&serde_json::Value>, id: &str) -> Self {
        let descriptor = config
            .and_then(|value| value.get("tooling"))
            .and_then(|tooling| tooling.get("codingAgents"))
            .and_then(|agents| agents.get(id))
            .filter(|entry| entry.is_object());
        let text = |key: &str| {
            descriptor
                .and_then(|entry| entry.get(key))
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let identity = descriptor
            .and_then(|entry| entry.get("identity"))
            .and_then(|value| value.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(letter, digest)| {
                        digest.as_str().map(|d| (letter.clone(), d.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        StoredReference {
            id: id.to_string(),
            app_label: text("app"),
            command: text("command"),
            identity,
            source_letter: None,
        }
    }
}

/// #2434 - a resolved reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMatch {
    pub agent_id: String,
    /// The LOCAL letter to launch, or `None` when the caller asked for no letter
    /// and none was matched. `Some` at tier 1, where the match IS a letter.
    pub profile_letter: Option<String>,
    /// `None` for a tier-0 local-id hit: nothing was guessed. Wire: `null`.
    pub tier: Option<MatchTier>,
    /// The key in `reference.identity` whose digest matched, when it differs
    /// from `profile_letter`; at tiers 2-3 the source letter when it differs.
    pub original_letter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    Matched(AgentMatch),
    NoMatch { reference: String },
}

/// #2434 - true when `letter` launches for `agent_id`: an enabled cell, or the
/// implicit `A` that `resolve_profile` always resolves (to an empty cell when
/// missing or disabled). Mirrors `coding_agent_profiles::cell_for_letter`.
fn profile_letter_exists(settings: &AppSettings, agent_id: &str, letter: &str) -> bool {
    letter == "A"
        || settings
            .coding_agent_profiles
            .profiles_by_agent
            .get(agent_id)
            .and_then(|cells| cells.get(letter))
            .is_some_and(|cell| cell.enabled)
}

/// #2434 - resolve a stored reference to a local agent and letter. Pure: it
/// writes nothing, and callers must not write the result back. Tiers apply in
/// strict precedence: local id, cell digest, label + letter, command + letter.
/// Every tie takes the earliest entry in `settings.agents` order and logs both.
pub fn resolve_portable_reference(
    settings: &AppSettings,
    reference: &StoredReference,
) -> MatchOutcome {
    let mut warnings = Vec::new();
    let outcome = resolve_portable_reference_logged(settings, reference, &mut warnings);
    for warning in warnings {
        log::warn!("[agent-match] {}", warning);
    }
    outcome
}

fn match_reference_digest(
    settings: &AppSettings,
    reference: &StoredReference,
    warnings: &mut Vec<String>,
) -> Option<AgentMatch> {
    let source = reference.source_letter.as_deref();
    if !reference.identity.is_empty() {
        let targets: Vec<(&String, &String)> =
            match source.and_then(|letter| reference.identity.get_key_value(letter)) {
                Some(target) => vec![target],
                None => reference.identity.iter().collect(),
            };
        // (agent index, local letter, matched descriptor key), settings order.
        let mut hits: Vec<(usize, String, String)> = Vec::new();
        for (index, agent) in settings.agents.iter().enumerate() {
            let Some(cells) = settings
                .coding_agent_profiles
                .profiles_by_agent
                .get(&agent.id)
            else {
                continue;
            };
            for (letter, cell) in cells.iter().filter(|(_, cell)| cell.enabled) {
                let digest = cell_identity(agent, cell);
                let matched: Vec<&String> = targets
                    .iter()
                    .filter(|(_, target)| **target == digest)
                    .map(|(key, _)| *key)
                    .collect();
                let key = matched
                    .iter()
                    .find(|key| **key == letter)
                    .or_else(|| matched.first());
                if let Some(key) = key {
                    hits.push((index, letter.clone(), (*key).clone()));
                }
            }
        }
        if let Some((winner, _, _)) = hits.first() {
            let winner = *winner;
            let agent_id = &settings.agents[winner].id;
            if let Some((other, letter, _)) = hits.iter().find(|(index, _, _)| *index != winner) {
                warnings.push(format!(
                    concat!(
                        "reference '{}' matched by digest in agents '{}' and '{}' ",
                        "(letter {}); using '{}'",
                    ),
                    reference.id, agent_id, settings.agents[*other].id, letter, agent_id
                ));
            }
            let tied: Vec<&(usize, String, String)> = hits
                .iter()
                .filter(|(index, _, _)| *index == winner)
                .collect();
            let chosen = tied
                .iter()
                .find(|(_, letter, _)| Some(letter.as_str()) == source)
                .unwrap_or(&tied[0]);
            if tied.len() > 1 {
                let letters: Vec<&str> = tied.iter().map(|(_, l, _)| l.as_str()).collect();
                warnings.push(format!(
                    "reference '{}' matched letters {} of agent '{}' by digest; using {}",
                    reference.id,
                    letters.join(" and "),
                    agent_id,
                    chosen.1
                ));
            }
            let (_, letter, key) = (*chosen).clone();
            return Some(AgentMatch {
                agent_id: agent_id.clone(),
                original_letter: (key != letter).then_some(key),
                profile_letter: Some(letter),
                tier: Some(MatchTier::Hash),
            });
        }
    }

    None
}

pub(super) fn resolve_portable_reference_logged(
    settings: &AppSettings,
    reference: &StoredReference,
    warnings: &mut Vec<String>,
) -> MatchOutcome {
    let source = reference.source_letter.as_deref();

    // Tier 0: the id is local. Same machine, unchanged behaviour.
    if settings.agents.iter().any(|agent| agent.id == reference.id) {
        return MatchOutcome::Matched(AgentMatch {
            agent_id: reference.id.clone(),
            profile_letter: reference.source_letter.clone(),
            tier: None,
            original_letter: None,
        });
    }

    // Tier 1: one target digest (the source letter's), else every descriptor digest.
    if let Some(matched) = match_reference_digest(settings, reference, warnings) {
        return MatchOutcome::Matched(matched);
    }

    // Tiers 2-3 test the candidate against the source letter, or the implicit
    // `A`; that substitute never becomes `profile_letter`.
    let test_letter = source.unwrap_or("A");
    let tiers: [(MatchTier, Option<&str>); 2] = [
        (MatchTier::LabelAndLetter, reference.app_label.as_deref()),
        (MatchTier::CommandAndLetter, reference.command.as_deref()),
    ];
    for (tier, wanted) in tiers {
        let Some(wanted) = wanted.filter(|value| !value.is_empty()) else {
            continue;
        };
        let candidates: Vec<&AgentConfig> = settings
            .agents
            .iter()
            .filter(|agent| match tier {
                MatchTier::LabelAndLetter => agent.label.eq_ignore_ascii_case(wanted),
                _ => canonical_command_text(&agent.command) == wanted,
            })
            .filter(|agent| profile_letter_exists(settings, &agent.id, test_letter))
            .collect();
        let Some(first) = candidates.first() else {
            continue;
        };
        if let Some(second) = candidates.get(1) {
            warnings.push(format!(
                "reference '{}' matched agents '{}' and '{}' by {}; using '{}'",
                reference.id,
                first.id,
                second.id,
                tier.as_wire_str(),
                first.id
            ));
        }
        return MatchOutcome::Matched(AgentMatch {
            agent_id: first.id.clone(),
            profile_letter: reference.source_letter.clone(),
            tier: Some(tier),
            // The profile letter IS the source letter here, so it never differs.
            original_letter: None,
        });
    }

    MatchOutcome::NoMatch {
        reference: reference.id.clone(),
    }
}
