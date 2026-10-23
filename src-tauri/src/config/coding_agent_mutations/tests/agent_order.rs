use super::*;

// ---- #2306 P1: explicit registered-agent order -------------------------

/// `a,b,c` with contiguous ordinals, the shape the loaders leave behind.
fn ordered_2306() -> AppSettings {
    let mut settings = AppSettings::default();
    for id in ["a", "b", "c"] {
        apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
            .unwrap();
    }
    settings
}

fn ids_2306(settings: &AppSettings) -> Vec<&str> {
    settings.agents.iter().map(|a| a.id.as_str()).collect()
}

fn positions_2306(settings: &AppSettings) -> Vec<Option<u32>> {
    settings.agents.iter().map(|a| a.order).collect()
}

#[test]
fn add_appends_at_the_next_position_and_ignores_a_caller_supplied_order() {
    let mut settings = ordered_2306();
    let mut incoming = agent("d", "D", "claude");
    incoming.order = Some(0); // forged/stale: must have no authority
    let outcome = apply_coding_agent_op(&mut settings, &add(incoming)).unwrap();
    assert_eq!(ids_2306(&settings), ["a", "b", "c", "d"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(outcome.agent.unwrap().order, Some(3));
}

#[test]
fn remove_compacts_positions_and_update_retains_the_current_position() {
    let mut settings = ordered_2306();
    apply_coding_agent_op(&mut settings, &CodingAgentOp::Remove { id: "b".into() }).unwrap();
    assert_eq!(ids_2306(&settings), ["a", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);

    // A patch carrying an unknown `order` key cannot move the tool.
    let patch: AgentPatch = serde_json::from_str(r#"{"label":"Renamed","order":0}"#).unwrap();
    let outcome = apply_coding_agent_op(
        &mut settings,
        &CodingAgentOp::Update {
            id: "c".into(),
            patch,
        },
    )
    .unwrap();
    assert_eq!(ids_2306(&settings), ["a", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
    assert_eq!(outcome.agent.unwrap().order, Some(1));
    assert_eq!(settings.agents[1].label, "Renamed");
}

#[test]
fn move_swaps_both_directions_and_returns_the_authoritative_order() {
    let mut settings = ordered_2306();
    let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["a", "c", "b"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);

    let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Down).unwrap();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
}

#[test]
fn move_derives_the_effective_order_before_the_adjacency_check() {
    // Vector slots are [b, a] but the stored ordinals put a first; the move must
    // act on the effective order [a, b], and normalize the result.
    let mut settings = AppSettings::default();
    apply_coding_agent_op(&mut settings, &add(agent("a", "A", "claude"))).unwrap();
    apply_coding_agent_op(&mut settings, &add(agent("b", "B", "claude"))).unwrap();
    settings.agents.swap(0, 1);
    settings.agents[0].order = Some(1);
    settings.agents[1].order = Some(0);

    let ids = move_registered_agent(&mut settings, "b", "a", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["b", "a"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
}

#[test]
fn rejected_moves_leave_the_live_settings_untouched() {
    let cases = [
        ("a", "b", AgentMoveDirection::Up, "top"),
        ("c", "b", AgentMoveDirection::Down, "bottom"),
        ("b", "a", AgentMoveDirection::Down, "not adjacent"),
        ("missing", "a", AgentMoveDirection::Up, "not found"),
        ("a", "missing", AgentMoveDirection::Down, "not found"),
    ];
    for (id, neighbor, direction, expected) in cases {
        let mut settings = ordered_2306();
        let ids_before: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
        let positions_before = positions_2306(&settings);
        let error = move_registered_agent(&mut settings, id, neighbor, direction).unwrap_err();
        assert!(
            error.contains(expected),
            "case ({id},{neighbor}): error {error:?} must mention {expected:?}"
        );
        let ids_after: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids_after, ids_before, "case ({id},{neighbor})");
        assert_eq!(
            positions_2306(&settings),
            positions_before,
            "case ({id},{neighbor})"
        );
    }
}

#[test]
fn move_rejects_duplicate_ids_while_case_distinct_ids_stay_distinct() {
    let mut duplicated = AppSettings {
        agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
        ..AppSettings::default()
    };
    let error =
        move_registered_agent(&mut duplicated, "dup", "dup", AgentMoveDirection::Up).unwrap_err();
    assert!(error.contains("more than once"), "{error}");

    // Case-distinct ids are distinct records for ordering (Add keeps rejecting
    // case-insensitive duplicates, so this legacy pair is inserted directly).
    let mut upper = agent("A", "Upper", "claude");
    upper.order = Some(0);
    let mut lower = agent("a", "Lower", "claude");
    lower.order = Some(1);
    let mut cased = AppSettings {
        agents: vec![upper, lower],
        ..AppSettings::default()
    };
    let ids = move_registered_agent(&mut cased, "a", "A", AgentMoveDirection::Up).unwrap();
    assert_eq!(ids, ["a", "A"]);
}

fn reorder_fixture_2542() -> AppSettings {
    let mut settings = AppSettings::default();
    for id in ["a", "b", "c", "d", "e"] {
        apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
            .unwrap();
    }
    settings
}

fn owned_ids_2542(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn reorder_moves_multiple_rows_down_and_up_in_one_step() {
    let mut settings = reorder_fixture_2542();
    let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
    let ids = reorder_registered_agent(&mut settings, "b", &expected, 3).unwrap();
    assert_eq!(ids, ["a", "c", "d", "b", "e"]);
    assert_eq!(ids_2306(&settings), ["a", "c", "d", "b", "e"]);

    let ids = reorder_registered_agent(&mut settings, "e", &ids, 1).unwrap();
    assert_eq!(ids, ["a", "e", "c", "d", "b"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn reorder_reaches_both_ends_with_contiguous_positions() {
    let mut settings = reorder_fixture_2542();
    let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
    let ids = reorder_registered_agent(&mut settings, "d", &expected, 0).unwrap();
    assert_eq!(ids, ["d", "a", "b", "c", "e"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );

    let ids = reorder_registered_agent(&mut settings, "a", &ids, 4).unwrap();
    assert_eq!(ids, ["d", "b", "c", "e", "a"]);
    assert_eq!(
        positions_2306(&settings),
        [Some(0), Some(1), Some(2), Some(3), Some(4)]
    );
}

#[test]
fn reorder_rejections_leave_the_live_settings_byte_identical() {
    let fresh = ["a", "b", "c", "d", "e"];
    let cases: [(&str, &[&str], usize, &str); 5] = [
        ("b", &["a", "x", "c", "d", "e"], 3, "changed since"),
        ("b", &["b", "a", "c", "d", "e"], 3, "changed since"),
        ("missing", &fresh, 1, "not found"),
        ("b", &fresh, 5, "out of range"),
        ("b", &fresh, 1, "nothing to reorder"),
    ];
    for (id, expected_ids, target, needle) in cases {
        let mut settings = reorder_fixture_2542();
        let before = serde_json::to_string(&settings.agents).unwrap();
        let error =
            reorder_registered_agent(&mut settings, id, &owned_ids_2542(expected_ids), target)
                .unwrap_err();
        assert!(
            error.contains(needle),
            "case ({id},{target}): error {error:?} must mention {needle:?}"
        );
        assert_eq!(
            serde_json::to_string(&settings.agents).unwrap(),
            before,
            "case ({id},{target})"
        );
    }

    let mut duplicated = AppSettings {
        agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
        ..AppSettings::default()
    };
    let before = serde_json::to_string(&duplicated.agents).unwrap();
    let error =
        reorder_registered_agent(&mut duplicated, "dup", &owned_ids_2542(&["dup", "dup"]), 1)
            .unwrap_err();
    assert!(error.contains("more than once"), "{error}");
    assert_eq!(serde_json::to_string(&duplicated.agents).unwrap(), before);
}

#[test]
fn reorder_derives_the_effective_order_from_unnormalized_input() {
    // Vector slots are [a, b, c] but the stored ordinals say [c, a, b]; the
    // caller's view and the move act on that effective order.
    let mut settings = ordered_2306();
    settings.agents[0].order = Some(1);
    settings.agents[1].order = Some(2);
    settings.agents[2].order = Some(0);

    let ids =
        reorder_registered_agent(&mut settings, "c", &owned_ids_2542(&["c", "a", "b"]), 2).unwrap();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
}
