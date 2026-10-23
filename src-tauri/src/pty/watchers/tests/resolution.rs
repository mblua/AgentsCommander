use super::*;
use crate::config::settings::WatcherConfig;

fn agent(id: &str, command: &str) -> WatcherAgent {
    WatcherAgent {
        id: id.to_string(),
        command: command.to_string(),
    }
}

fn watcher(commands: Option<&[&str]>) -> WatcherEntry {
    WatcherEntry::Valid(WatcherConfig {
        enabled: true,
        mode: WatcherMode::Occurrence,
        pattern: "x".to_string(),
        commands: commands.map(|list| list.iter().map(|s| s.to_string()).collect()),
        dedupe: WatcherDedupe::Row,
        dedupe_window_ms: 2000,
        captured_against: None,
    })
}

fn map(entries: Vec<(&str, WatcherEntry)>) -> BTreeMap<String, WatcherEntry> {
    entries
        .into_iter()
        .map(|(id, entry)| (id.to_string(), entry))
        .collect()
}

fn running_ids(resolved: &HashMap<String, AgentResolution>, agent_id: &str) -> Vec<String> {
    resolved
        .get(agent_id)
        .map(|entry| entry.running.iter().map(|w| w.id.clone()).collect())
        .unwrap_or_default()
}

/// 9.2.10 - the configuration the issue says cannot be expressed today: one watcher, every
/// agent. And its exact opposite, which only exists because `commands` is an `Option`.
#[test]
fn commands_absent_reaches_every_agent_and_an_empty_list_reaches_none() {
    let agents = [agent("a1", "claude"), agent("a2", "codex")];

    let (all, _) = resolve_watchers(&agents, &map(vec![("w", watcher(None))]));
    assert_eq!(running_ids(&all, "a1"), vec!["w"]);
    assert_eq!(running_ids(&all, "a2"), vec!["w"]);

    let (none, _) = resolve_watchers(&agents, &map(vec![("w", watcher(Some(&[])))]));
    assert!(
        none.is_empty(),
        "`[]` is the opposite of absent and must reach nobody"
    );
}

/// 9.2.11 and 9.2.12 - the stem rule, in full. EXACT equality on the executable stem,
/// never a prefix: the catalog rejects `starts_with` in writing because `pi` and `agent`
/// false-match under it (`coding_agents_catalog.rs:494-497`), and this is the rule that
/// must not be re-derived anywhere else, in Rust or in TypeScript.
#[test]
fn a_commands_selector_matches_the_executable_stem_exactly() {
    let agents = [
        agent("plain", "claude"),
        agent("shouting", "CLAUDE.EXE"),
        agent("full-path", r"C:\Users\x\claude-sandbox-runtime\claude.cmd"),
        agent("pi", "pi --provider claude"),
        agent("through-cmd", "cmd /c claude"),
        agent("through-npx", "npx claude"),
        agent("prefix", "claude-phi"),
    ];

    let (resolved, _) = resolve_watchers(&agents, &map(vec![("w", watcher(Some(&["claude"])))]));
    let mut reached: Vec<&String> = resolved.keys().collect();
    reached.sort();
    assert_eq!(reached, vec!["full-path", "plain", "shouting"]);
}

/// 9.2.13 - disabled is a state the user chose. It reaches nobody and keeps every byte of
/// its configuration, so re-enabling it is one flag and not a re-entry.
#[test]
fn a_disabled_watcher_reaches_nobody_and_keeps_its_configuration() {
    let mut entry = watcher(None);
    if let WatcherEntry::Valid(config) = &mut entry {
        config.enabled = false;
    }
    let watchers = map(vec![("w", entry)]);

    let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
    assert!(resolved.is_empty());
    assert!(
        notices.is_empty(),
        "disabled is not a problem and must not produce a log line"
    );
    assert_eq!(watchers["w"].valid().expect("still valid").pattern, "x");
}

/// 9.2.14 - a selector entry that is not a command skips the WHOLE watcher. The failure
/// direction matters: "reaches nobody" loses a detection, "reaches everybody" would run a
/// user's pattern on agents they never selected.
#[test]
fn a_selector_entry_that_does_not_tokenize_skips_the_whole_watcher() {
    let agents = [agent("a1", "claude"), agent("a2", "codex")];
    let watchers = map(vec![("w", watcher(Some(&["claude", ""])))]);

    let (resolved, notices) = resolve_watchers(&agents, &watchers);
    assert!(
        resolved.is_empty(),
        "not even the readable half of the selector may reach anything"
    );
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].subject, "commands:w");

    // ...and the notice is written once, however many ticks resolve the same thing.
    let log = ResolutionLog::default();
    assert_eq!(log.publish(notices.clone()), 1);
    for _ in 0..10 {
        assert_eq!(log.publish(notices.clone()), 0);
    }
    assert_eq!(log.remembered(), 1, "one subject, not one per tick");
}

/// 9.2.15 - an agent whose own command does not tokenize has no stem to compare against.
/// Selectorless watchers still reach it; no watcher WITH a selector does, because the
/// alternative would be inventing a stem and matching on the guess.
#[test]
fn an_agent_whose_command_does_not_tokenize_is_reached_only_without_a_selector() {
    let agents = [agent("broken", "\"unterminated")];
    let watchers = map(vec![
        ("selectorless", watcher(None)),
        ("selected", watcher(Some(&["claude"]))),
    ]);

    let (resolved, _) = resolve_watchers(&agents, &watchers);
    assert_eq!(running_ids(&resolved, "broken"), vec!["selectorless"]);
}

/// 9.2.16 - the budget, and the fact that it resolves by alphabetical id order. Declared
/// debt (10.8), surfaced rather than fixed: the dropped ids come back in `over_budget` so
/// Settings can show "not running on <agent> (budget)" per row.
#[test]
fn only_the_first_eight_watchers_in_key_order_run_on_one_agent() {
    let entries: Vec<(String, WatcherEntry)> = (1..=12)
        .map(|i| (format!("w{i:02}"), watcher(None)))
        .collect();
    let watchers: BTreeMap<String, WatcherEntry> = entries.into_iter().collect();

    let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
    let entry = resolved.get("a1").expect("reached");
    assert_eq!(
        entry
            .running
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec!["w01", "w02", "w03", "w04", "w05", "w06", "w07", "w08"]
    );
    assert_eq!(entry.over_budget, vec!["w09", "w10", "w11", "w12"]);

    assert_eq!(notices.len(), 1, "the four dropped ids are ONE line");
    assert!(notices[0].message.contains("w09,w10,w11,w12"));
    let log = ResolutionLog::default();
    assert_eq!(log.publish(notices.clone()), 1);
    assert_eq!(log.publish(notices), 0);
}

/// 9.2.20 - the clamp, and its one log line. Without it a large window plus a `row` key
/// grows the dedupe key set to every distinct row seen inside the window.
#[test]
fn a_dedupe_window_over_the_maximum_is_clamped_and_logged_once() {
    let mut entry = watcher(None);
    if let WatcherEntry::Valid(config) = &mut entry {
        config.dedupe_window_ms = 3_600_000;
    }

    let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &map(vec![("w", entry)]));
    assert_eq!(
        resolved["a1"].running[0].dedupe_window_ms,
        MAX_DEDUPE_WINDOW_MS
    );
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].subject, "clamp:w");

    let log = ResolutionLog::default();
    assert_eq!(log.publish(notices.clone()), 1);
    assert_eq!(log.publish(notices), 0);
}

/// An entry that did not deserialize is skipped and logged once, and every OTHER watcher
/// still resolves. The settings half of this is pinned in `settings.rs`; this is the
/// resolution half.
#[test]
fn an_invalid_entry_is_skipped_and_logged_once_while_the_others_resolve() {
    let watchers = map(vec![
        (
            "bad",
            WatcherEntry::Invalid(serde_json::json!({ "mode": "State" })),
        ),
        ("good", watcher(None)),
    ]);

    let (resolved, notices) = resolve_watchers(&[agent("a1", "claude")], &watchers);
    assert_eq!(running_ids(&resolved, "a1"), vec!["good"]);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].subject, "invalid:bad");

    let log = ResolutionLog::default();
    assert_eq!(log.publish(notices.clone()), 1);
    assert_eq!(log.publish(notices), 0);
}

/// The log-once map is keyed by SUBJECT, so a user typing a selector in Settings cannot
/// accumulate one entry per keystroke - and a CHANGED value still speaks again, because a
/// second mistake is not the first one.
#[test]
fn the_log_once_map_holds_one_entry_per_subject_and_speaks_again_on_a_change() {
    let log = ResolutionLog::default();
    for attempt in 0..50 {
        let notice = ResolutionNotice {
            subject: "commands:w".to_string(),
            detail: format!("half-typed-{attempt}"),
            message: "…".to_string(),
        };
        assert_eq!(log.publish(vec![notice]), 1, "a changed value speaks again");
    }
    assert_eq!(log.remembered(), 1);
}

/// Two watchers reaching the same agent both run: there is no "most specific wins" rule,
/// because it would silently discard a pattern the user configured.
#[test]
fn two_watchers_reaching_the_same_agent_both_run() {
    let watchers = map(vec![
        ("a", watcher(Some(&["claude"]))),
        ("b", watcher(None)),
    ]);

    let (resolved, _) = resolve_watchers(&[agent("a1", "claude")], &watchers);
    assert_eq!(running_ids(&resolved, "a1"), vec!["a", "b"]);
}

/// A stem no agent has is not an error, it just reaches nobody. Settings shows
/// "reaches 0 agents" so the typo is visible where it was made.
#[test]
fn a_stem_no_agent_has_reaches_nobody_and_is_not_an_error() {
    let (resolved, notices) = resolve_watchers(
        &[agent("a1", "claude")],
        &map(vec![("w", watcher(Some(&["gemni"])))]),
    );
    assert!(resolved.is_empty());
    assert!(notices.is_empty());
}
