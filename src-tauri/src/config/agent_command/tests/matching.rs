use super::*;

#[test]
fn hash_match_wins_over_label_and_command() {
    let settings = match_settings(
        vec![
            labelled("claude", "Claude Code", "claude"),
            labelled("other", "Other", "codex"),
        ],
        &[("other", "A", "--h", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Claude Code".to_string());
    r.command = Some(super::canonical_command_text("claude"));
    r.identity = identity(&[("A", &digest_of(&settings, "other", "A"))]);
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "other");
    assert_eq!(found.tier, Some(super::MatchTier::Hash));
    assert_eq!(found.profile_letter.as_deref(), Some("A"));
}

#[test]
fn hash_match_finds_a_moved_letter() {
    let settings = match_settings(
        vec![agent("codex", "codex")],
        &[("codex", "A", "--one", true), ("codex", "B", "--two", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.identity = identity(&[("A", &digest_of(&settings, "codex", "B")), ("B", "y")]);
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "codex");
    assert_eq!(found.profile_letter.as_deref(), Some("B"));
    assert_eq!(found.original_letter.as_deref(), Some("A"));
    assert_eq!(found.tier, Some(super::MatchTier::Hash));
}

#[test]
fn hash_match_targets_only_the_source_letters_digest() {
    // The first agent carries the descriptor's OTHER digest (B=y); only the
    // second carries the source letter's (A=x). Searching every digest
    // would pick the first agent.
    let settings = match_settings(
        vec![agent("first", "codex"), agent("second", "codex")],
        &[("first", "C", "--y", true), ("second", "D", "--x", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.identity = identity(&[
        ("A", &digest_of(&settings, "second", "D")),
        ("B", &digest_of(&settings, "first", "C")),
    ]);
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "second");
    assert_eq!(found.profile_letter.as_deref(), Some("D"));
    assert_eq!(found.original_letter.as_deref(), Some("A"));
    assert_eq!(found.tier, Some(super::MatchTier::Hash));
}

#[test]
fn hash_match_on_the_source_letter_reports_no_original_letter() {
    let settings = match_settings(
        vec![agent("codex", "codex")],
        &[("codex", "A", "--one", true), ("codex", "B", "--two", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.identity = identity(&[("A", &digest_of(&settings, "codex", "A"))]);
    let found = resolve(&settings, &r);
    assert_eq!(found.profile_letter.as_deref(), Some("A"));
    assert_eq!(found.original_letter, None);
    assert_eq!(found.tier, Some(super::MatchTier::Hash));
}

#[test]
fn hash_match_without_a_source_letter_reports_the_matched_key() {
    let settings = match_settings(
        vec![agent("codex", "codex")],
        &[("codex", "A", "--a", true), ("codex", "C", "--c", true)],
    );
    let mut r = reference("foreign", None);
    r.identity = identity(&[("B", &digest_of(&settings, "codex", "C"))]);
    let found = resolve(&settings, &r);
    assert_eq!(found.profile_letter.as_deref(), Some("C"));
    assert_eq!(found.original_letter.as_deref(), Some("B"));
    assert_eq!(found.tier, Some(super::MatchTier::Hash));
}

#[test]
fn tier_zero_without_a_source_letter_yields_no_profile_letter() {
    let settings = match_settings(vec![agent("codex", "codex")], &[]);
    let found = resolve(&settings, &reference("codex", None));
    assert_eq!(found.agent_id, "codex");
    assert_eq!(found.profile_letter, None, "never Some(\"A\")");
    assert_eq!(found.tier, None);
}

#[test]
fn tier_two_and_three_without_a_source_letter_yield_no_profile_letter() {
    let settings = match_settings(vec![labelled("local", "Codex", "codex")], &[]);

    let mut by_label = reference("foreign", None);
    by_label.app_label = Some("Codex".to_string());
    let found = resolve(&settings, &by_label);
    assert_eq!(found.tier, Some(super::MatchTier::LabelAndLetter));
    assert_eq!(found.profile_letter, None);
    assert_eq!(found.original_letter, None);

    let mut by_command = reference("foreign", None);
    by_command.command = Some(super::canonical_command_text("codex"));
    let found = resolve(&settings, &by_command);
    assert_eq!(found.tier, Some(super::MatchTier::CommandAndLetter));
    assert_eq!(found.profile_letter, None);
    assert_eq!(found.original_letter, None);
}

#[test]
fn intra_agent_letter_tie_takes_the_earliest_letter() {
    let settings = match_settings(
        vec![agent("codex", "codex")],
        &[
            ("codex", "A", "--a", true),
            ("codex", "B", "--same", true),
            ("codex", "C", "--same", true),
        ],
    );
    let same = digest_of(&settings, "codex", "B");

    let mut r = reference("foreign", Some("A"));
    r.identity = identity(&[("A", &same)]);
    let (outcome, warnings) = resolve_with_warnings(&settings, &r);
    let found = matched(outcome);
    assert_eq!(found.profile_letter.as_deref(), Some("B"));
    assert_eq!(found.original_letter.as_deref(), Some("A"));
    assert!(
        warnings.iter().any(|w| w.contains("B and C")),
        "{warnings:?}"
    );

    // The source letter wins when it is one of the tied letters.
    let mut r = reference("foreign", Some("C"));
    r.identity = identity(&[("C", &same)]);
    let (outcome, warnings) = resolve_with_warnings(&settings, &r);
    let found = matched(outcome);
    assert_eq!(found.profile_letter.as_deref(), Some("C"));
    assert_eq!(found.original_letter, None);
    assert!(
        warnings.iter().any(|w| w.contains("B and C")),
        "{warnings:?}"
    );
}

#[test]
fn label_match_is_case_insensitive() {
    let settings = match_settings(vec![labelled("local", "Codex", "codex")], &[]);
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("CODEX".to_string());
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "local");
    assert_eq!(found.tier, Some(super::MatchTier::LabelAndLetter));
    assert_eq!(found.profile_letter.as_deref(), Some("A"));
}

#[test]
fn label_tier_outranks_command_tier() {
    // agents[0] matches only by command, agents[1] only by label: the label
    // tier runs first, so the later agent wins.
    let settings = match_settings(
        vec![
            labelled("other", "Other", "codex"),
            labelled("mine", "Codex", "x"),
        ],
        &[],
    );
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Codex".to_string());
    r.command = Some(super::canonical_command_text("codex"));
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "mine");
    assert_eq!(found.tier, Some(super::MatchTier::LabelAndLetter));
}

#[test]
fn command_match_is_used_when_label_differs() {
    let settings = match_settings(vec![labelled("local", "Mine", "Codex --Yolo")], &[]);
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Theirs".to_string());
    r.command = Some(super::canonical_command_text("codex --yolo"));
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "local");
    assert_eq!(found.tier, Some(super::MatchTier::CommandAndLetter));
}

#[test]
fn letter_a_always_exists_even_without_a_cell() {
    // No cells at all: the implicit `A` still resolves, so `A` matches.
    let settings = match_settings(vec![labelled("local", "Codex", "codex")], &[]);
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Codex".to_string());
    assert_eq!(resolve(&settings, &r).profile_letter.as_deref(), Some("A"));
    // A disabled `A` still resolves too.
    let disabled = match_settings(
        vec![labelled("local", "Codex", "codex")],
        &[("local", "A", "--a", false)],
    );
    assert_eq!(resolve(&disabled, &r).agent_id, "local");
    // Any other missing letter does not exist.
    r.source_letter = Some("B".to_string());
    assert_eq!(
        super::resolve_portable_reference(&settings, &r),
        super::MatchOutcome::NoMatch {
            reference: "foreign".to_string()
        }
    );
}

#[test]
fn disabled_cells_never_match() {
    let settings = match_settings(
        vec![labelled("local", "Codex", "codex")],
        &[("local", "A", "--a", true), ("local", "B", "--b", false)],
    );
    let mut r = reference("foreign", Some("B"));
    r.identity = identity(&[("B", &digest_of(&settings, "local", "B"))]);
    r.app_label = Some("Codex".to_string());
    assert_eq!(
        super::resolve_portable_reference(&settings, &r),
        super::MatchOutcome::NoMatch {
            reference: "foreign".to_string()
        },
        "a disabled B is neither a digest candidate nor an existing letter"
    );
}

#[test]
fn a_tie_across_agents_takes_the_first_in_settings_order() {
    let build = || {
        match_settings(
            vec![
                labelled("first", "Codex", "codex"),
                labelled("second", "codex", "codex"),
            ],
            &[("first", "A", "--x", true), ("second", "A", "--x", true)],
        )
    };
    let settings = build();
    let mut by_hash = reference("foreign", Some("A"));
    by_hash.identity = identity(&[("A", &digest_of(&settings, "first", "A"))]);
    let mut by_label = reference("foreign", Some("A"));
    by_label.app_label = Some("CODEX".to_string());
    let mut by_command = reference("foreign", Some("A"));
    by_command.command = Some(super::canonical_command_text("codex"));

    for (r, tier) in [
        (&by_hash, super::MatchTier::Hash),
        (&by_label, super::MatchTier::LabelAndLetter),
        (&by_command, super::MatchTier::CommandAndLetter),
    ] {
        for _ in 0..2 {
            let (outcome, warnings) = resolve_with_warnings(&build(), r);
            let found = matched(outcome);
            assert_eq!(found.agent_id, "first", "{tier:?}");
            assert_eq!(found.tier, Some(tier));
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("'first'") && w.contains("'second'")),
                "{tier:?}: {warnings:?}"
            );
        }
    }
}

#[test]
fn matching_never_rewrites_the_stored_reference() {
    let settings = match_settings(
        vec![labelled("local", "Codex", "codex")],
        &[("local", "A", "--a", true), ("local", "B", "--b", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Codex".to_string());
    r.command = Some(super::canonical_command_text("codex"));
    r.identity = identity(&[("A", &digest_of(&settings, "local", "B"))]);
    let reference_before = r.clone();
    let settings_before = serde_json::to_value(&settings).unwrap();
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "local");
    assert_eq!(r, reference_before);
    assert_eq!(serde_json::to_value(&settings).unwrap(), settings_before);
}

#[test]
fn local_id_hit_reports_no_tier() {
    let settings = match_settings(
        vec![labelled("codex", "Codex", "codex")],
        &[("codex", "B", "--b", true)],
    );
    let mut r = reference("codex", Some("B"));
    r.identity = identity(&[("B", "stale")]);
    let found = resolve(&settings, &r);
    assert_eq!(found.agent_id, "codex");
    assert_eq!(found.tier, None);
    assert_eq!(found.profile_letter.as_deref(), Some("B"));
    assert_eq!(found.original_letter, None);
    assert_eq!(
        serde_json::to_value(found.tier).unwrap(),
        serde_json::Value::Null
    );
    assert_eq!(
        serde_json::to_value(super::MatchTier::LabelAndLetter).unwrap(),
        super::MatchTier::LabelAndLetter.as_wire_str()
    );
}

#[test]
fn no_match_returns_nomatch_and_does_not_fall_back() {
    let settings = match_settings(
        vec![labelled("claude", "Claude Code", "claude")],
        &[("claude", "A", "--a", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Codex".to_string());
    r.command = Some(super::canonical_command_text("codex"));
    r.identity = identity(&[("A", "0000000000000000")]);
    assert_eq!(
        super::resolve_portable_reference(&settings, &r),
        super::MatchOutcome::NoMatch {
            reference: "foreign".to_string()
        }
    );
}

#[test]
fn empty_identity_falls_back_to_label_tier() {
    let settings = match_settings(
        vec![labelled("local", "Codex", "codex")],
        &[("local", "A", "--a", true)],
    );
    let mut r = reference("foreign", Some("A"));
    r.app_label = Some("Codex".to_string());
    assert!(r.identity.is_empty());
    let found = resolve(&settings, &r);
    assert_eq!(found.tier, Some(super::MatchTier::LabelAndLetter));
}

#[test]
fn empty_agents_binds_as_nomatch() {
    let settings = match_settings(Vec::new(), &[]);
    let mut r = reference("codex", Some("A"));
    r.app_label = Some("Codex".to_string());
    r.command = Some("codex".to_string());
    r.identity = identity(&[("A", "x")]);
    assert_eq!(
        super::resolve_portable_reference(&settings, &r),
        super::MatchOutcome::NoMatch {
            reference: "codex".to_string()
        }
    );
}
