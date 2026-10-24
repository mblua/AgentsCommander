use super::*;

fn fragment(text: &str, start: u64) -> TurnFragment {
    TurnFragment {
        text: text.to_owned(),
        record_start: Some(start),
    }
}

fn closure_line(payload: serde_json::Value) -> String {
    serde_json::json!({"type": "event_msg", "payload": payload}).to_string()
}

fn emit_text(events: &[TurnEvent]) -> Option<&AssembledTurn> {
    events.iter().find_map(|event| match event {
        TurnEvent::Emit(turn) => Some(turn),
        _ => None,
    })
}

// Test 7, parser half: the flat `turn_id` belongs to `task_complete`; the
// nested location is a different field and must not be read here.
#[test]
fn parse_reads_the_flat_turn_id_on_both_closure_records() {
    let started = parse_closure_record(&closure_line(
        serde_json::json!({"type": "task_started", "turn_id": "t-1"}),
    ));
    assert_eq!(
        started,
        ClosureParse::Closure(ClosureRecord::TaskStarted {
            turn_id: "t-1".to_owned()
        })
    );

    let complete = parse_closure_record(&closure_line(serde_json::json!({
        "type": "task_complete",
        "turn_id": "t-1",
        "last_agent_message": "the message"
    })));
    assert_eq!(
        complete,
        ClosureParse::Closure(ClosureRecord::TaskComplete {
            turn_id: "t-1".to_owned(),
            last_agent_message: Some("the message".to_owned()),
        })
    );

    // The id nested where the final answer carries it is not this field.
    let nested_only = parse_closure_record(&closure_line(serde_json::json!({
        "type": "task_complete",
        "last_agent_message": "x",
        "internal_chat_message_metadata_passthrough": {"turn_id": "t-1"}
    })));
    assert_eq!(nested_only, ClosureParse::Malformed);
}

#[test]
fn parse_marks_a_closure_without_a_usable_id_malformed() {
    for payload in [
        serde_json::json!({"type": "task_started"}),
        serde_json::json!({"type": "task_started", "turn_id": 7}),
        serde_json::json!({"type": "task_started", "turn_id": ""}),
        serde_json::json!({"type": "task_complete", "last_agent_message": "x"}),
    ] {
        assert_eq!(
            parse_closure_record(&closure_line(payload.clone())),
            ClosureParse::Malformed,
            "payload={payload}"
        );
    }
    // A completion may legitimately omit `last_agent_message`; that is an
    // abstention reason, not a parse failure.
    assert!(matches!(
        parse_closure_record(&closure_line(
            serde_json::json!({"type": "task_complete", "turn_id": "t"})
        )),
        ClosureParse::Closure(ClosureRecord::TaskComplete {
            last_agent_message: None,
            ..
        })
    ));
}

// Test 11, parser half: Claude prose and tool traffic are never closures.
// The Claude bits themselves are pinned by the untouched
// `claude_watcher::tests::assistant_records_carry_claudes_permanent_bits`.
#[test]
fn parse_ignores_non_closure_lines() {
    let claude = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": "claude prose"}]}
    })
    .to_string();
    let tool = serde_json::json!({
        "type": "response_item",
        "payload": {"type": "function_call", "name": "shell", "arguments": "{}"}
    })
    .to_string();
    for line in [claude, tool.clone(), "not json".to_owned(), String::new()] {
        assert_eq!(parse_closure_record(&line), ClosureParse::NotClosure);
    }
    let mut turns = TurnAccumulator::new();
    assert!(turns.on_line(&tool).is_empty());
    assert_eq!(turns.open_turns(), 0);
    assert_eq!(turns.counters(), TurnCounters::default());
}

// Test 3, state-machine half: a multi-record turn is joined in file order
// and leaves exactly one assembled turn.
#[test]
fn a_multi_record_turn_assembles_in_file_order_and_emits_once() {
    let mut turns = TurnAccumulator::new();
    assert!(turns
        .on_final_answer("t1", fragment("first", 10))
        .is_empty());
    assert!(turns
        .on_final_answer("t1", fragment("second", 20))
        .is_empty());
    // Grouping is by identifier, not adjacency: another turn may interleave.
    assert!(turns
        .on_final_answer("t2", fragment("other", 30))
        .is_empty());
    assert!(turns
        .on_final_answer("t1", fragment("third", 40))
        .is_empty());

    let events = turns.on_task_complete("t1", Some("first\nsecond\nthird"));
    let emitted = emit_text(&events).expect("the turn must emit");
    assert_eq!(emitted.turn_id, "t1");
    assert_eq!(emitted.text, "first\nsecond\nthird");
    assert_eq!(emitted.first_record_start, Some(10));
    assert_eq!(turns.counters().emitted, 1);

    // Test 9, state-machine half: the rest of the turn is still open and
    // assembles separately.
    let events = turns.on_task_complete("t2", Some("other"));
    let emitted = emit_text(&events).expect("the second turn must emit");
    assert_eq!(emitted.text, "other");
}

// Test 4: newline kind and trailing whitespace must not cause abstention.
#[test]
fn normalisation_accepts_newline_kind_and_trailing_whitespace() {
    assert_eq!(normalise_text("a\r\nb\r\n"), "a\nb");
    assert_eq!(normalise_text("  a\nb  "), "a\nb");

    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("t", fragment("first", 0));
    turns.on_final_answer("t", fragment("second", 1));
    let events = turns.on_task_complete("t", Some("first\r\nsecond\r\n"));
    let emitted = emit_text(&events).expect("normalised comparison must accept");
    // The candidate carries the assembly, not the provider's field.
    assert_eq!(emitted.text, "first\nsecond");
}

// Test 5: an assembly genuinely missing a record is rejected.
#[test]
fn a_missing_fragment_is_rejected_with_a_reason() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("t", fragment("first", 0));
    let events = turns.on_task_complete("t", Some("first\nsecond"));
    assert_eq!(
        events,
        vec![TurnEvent::Abstain {
            turn_id: "t".to_owned(),
            reason: AbstainReason::CompletenessMismatch
        }]
    );
    assert_eq!(turns.counters().emitted, 0);
    assert_eq!(turns.counters().abstained, 1);
}

#[test]
fn a_completion_without_last_agent_message_abstains() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("t", fragment("body", 0));
    let events = turns.on_task_complete("t", None);
    assert_eq!(
        events,
        vec![TurnEvent::Abstain {
            turn_id: "t".to_owned(),
            reason: AbstainReason::MissingLastAgentMessage
        }]
    );
    assert_eq!(turns.counters().emitted, 0);
}

// Test 2, state-machine half: a completion matching nothing abstains.
#[test]
fn a_completion_without_a_final_answer_abstains() {
    let mut turns = TurnAccumulator::new();
    let events = turns.on_task_complete("t", Some("x"));
    assert_eq!(
        events,
        vec![TurnEvent::Abstain {
            turn_id: "t".to_owned(),
            reason: AbstainReason::NoAccumulatedFinalAnswer
        }]
    );
    assert_eq!(turns.counters().emitted, 0);
    assert_eq!(
        turns.open_turns(),
        1,
        "the closed marker waits for late writes"
    );
}

// Test 6: a `task_started` after the last `task_complete` supersedes the
// closed turn and routes nothing, with a visible counted event.
#[test]
fn a_task_started_supersedes_a_closed_turn_and_does_not_route_it() {
    let mut turns = TurnAccumulator::new();
    assert_eq!(
        turns.on_task_complete("closed", Some("x")),
        vec![TurnEvent::Abstain {
            turn_id: "closed".to_owned(),
            reason: AbstainReason::NoAccumulatedFinalAnswer
        }]
    );
    let events = turns.on_task_started("newer");
    assert_eq!(
        events,
        vec![TurnEvent::Superseded {
            turn_id: "closed".to_owned()
        }]
    );
    assert_eq!(turns.counters().superseded, 1);
    assert_eq!(turns.counters().emitted, 0);
    assert_eq!(turns.open_turns(), 0);
    // The superseded turn's own closure is gone; a later repetition is not
    // a candidate either.
    let events = turns.on_task_complete("closed", Some("x"));
    assert!(emit_text(&events).is_none());
}

// The plan's "Late writes" rule: no time window. A final answer that
// arrives after an early closure can still be closed and emitted.
#[test]
fn a_late_final_answer_after_an_early_closure_can_still_emit() {
    let mut turns = TurnAccumulator::new();
    turns.on_task_complete("t", Some("late body"));
    turns.on_final_answer("t", fragment("late body", 7));
    let events = turns.on_task_complete("t", Some("late body"));
    let emitted = emit_text(&events).expect("the late write must still arm");
    assert_eq!(emitted.text, "late body");
    assert_eq!(emitted.first_record_start, Some(7));
}

// An open turn whose closure has NOT been seen is not superseded by a newer
// start: its own closure can still arrive (no time window).
#[test]
fn a_task_started_keeps_a_turn_that_waits_for_its_closure() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("waiting", fragment("body", 0));
    assert!(turns.on_task_started("newer").is_empty());
    assert_eq!(turns.open_turns(), 1);
    let events = turns.on_task_complete("waiting", Some("body"));
    assert!(emit_text(&events).is_some());
}

// Test 10: nine interleaved open turns leave eight; the dropped one emits
// nothing even when its completion arrives.
#[test]
fn nine_open_turns_leave_eight_and_the_oldest_is_dropped() {
    let mut turns = TurnAccumulator::new();
    for index in 0..9 {
        let events = turns.on_final_answer(&format!("t{index}"), fragment("body", index));
        if index == 8 {
            assert_eq!(
                events,
                vec![TurnEvent::Evicted {
                    turn_id: "t0".to_owned()
                }]
            );
        }
    }
    assert_eq!(turns.open_turns(), MAX_OPEN_TURNS);
    assert_eq!(turns.counters().evicted, 1);

    // The surviving turns still assemble.
    let events = turns.on_task_complete("t1", Some("body"));
    assert_eq!(
        emit_text(&events).map(|turn| turn.turn_id.as_str()),
        Some("t1")
    );

    // The dropped turn emits nothing: its completion opens only an empty
    // marker, which is never a candidate.
    let events = turns.on_task_complete("t0", Some("body"));
    assert!(emit_text(&events).is_none(), "a dropped turn never emits");
}

// "Exactly one CapturedRecord per closed turn": a repeated closure and a
// late fragment must not yield a second candidate.
#[test]
fn a_routed_turn_never_emits_again() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("t", fragment("body", 0));
    assert!(emit_text(&turns.on_task_complete("t", Some("body"))).is_some());

    let repeated = turns.on_task_complete("t", Some("body"));
    assert_eq!(
        repeated,
        vec![TurnEvent::Abstain {
            turn_id: "t".to_owned(),
            reason: AbstainReason::AlreadyRouted
        }]
    );
    let late = turns.on_final_answer("t", fragment("more", 1));
    assert_eq!(
        late,
        vec![TurnEvent::LateAfterRoute {
            turn_id: "t".to_owned()
        }]
    );
    assert_eq!(turns.counters().emitted, 1);
    assert_eq!(turns.counters().late_after_route, 1);
}

// An unparseable closure is skipped and counted, never fatal.
#[test]
fn an_unparseable_closure_is_counted_and_skipped() {
    let mut turns = TurnAccumulator::new();
    let events = turns.on_line(&closure_line(serde_json::json!({"type": "task_started"})));
    assert_eq!(events, vec![TurnEvent::MalformedClosure]);
    assert_eq!(turns.counters().malformed_closures, 1);
    assert_eq!(turns.open_turns(), 0);
}

// #2356 item 1: `turn_complete` is an alias of `task_complete`.
#[test]
fn parse_accepts_the_turn_complete_alias() {
    assert_eq!(
        parse_closure_record(&closure_line(serde_json::json!({
            "type": "turn_complete",
            "turn_id": "t-1",
            "last_agent_message": "m"
        }))),
        ClosureParse::Closure(ClosureRecord::TaskComplete {
            turn_id: "t-1".to_owned(),
            last_agent_message: Some("m".to_owned()),
        })
    );
    assert_eq!(
        parse_closure_record(&closure_line(
            serde_json::json!({"type": "turn_complete", "turn_id": "t-1"})
        )),
        ClosureParse::Closure(ClosureRecord::TaskComplete {
            turn_id: "t-1".to_owned(),
            last_agent_message: None,
        })
    );
    for payload in [
        serde_json::json!({"type": "turn_complete", "last_agent_message": "m"}),
        serde_json::json!({"type": "turn_complete", "turn_id": ""}),
        serde_json::json!({"type": "turn_complete", "turn_id": 7}),
    ] {
        assert_eq!(
            parse_closure_record(&closure_line(payload.clone())),
            ClosureParse::Malformed,
            "payload={payload}"
        );
    }
}

#[test]
fn an_alias_closure_emits_the_turn_through_on_line() {
    let mut turns = TurnAccumulator::new();
    assert!(turns.on_final_answer("t", fragment("body", 3)).is_empty());
    let events = turns.on_line(&closure_line(serde_json::json!({
        "type": "turn_complete",
        "turn_id": "t",
        "last_agent_message": "body"
    })));
    assert_eq!(events.len(), 1);
    let emitted = emit_text(&events).expect("the alias must close the turn");
    assert_eq!(emitted.text, "body");
    assert_eq!(emitted.first_record_start, Some(3));
    assert_eq!(turns.counters().emitted, 1);
}

// #2356 item 2, the section 5.1 fixture of #2268: a superseded turn is
// never recreated by a late fragment and its closure.
#[test]
fn a_superseded_turn_is_not_recreated_by_a_late_fragment() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("s", fragment("a", 0));
    assert_eq!(
        turns.on_task_complete("s", Some("mismatch")),
        vec![TurnEvent::Abstain {
            turn_id: "s".to_owned(),
            reason: AbstainReason::CompletenessMismatch
        }]
    );
    assert_eq!(
        turns.on_task_started("newer"),
        vec![TurnEvent::Superseded {
            turn_id: "s".to_owned()
        }]
    );
    assert_eq!(
        turns.on_final_answer("s", fragment("late", 1)),
        vec![TurnEvent::LateAfterSuperseded {
            turn_id: "s".to_owned()
        }]
    );
    assert_eq!(
        turns.on_task_complete("s", Some("late")),
        vec![TurnEvent::Abstain {
            turn_id: "s".to_owned(),
            reason: AbstainReason::Superseded
        }]
    );
    assert_eq!(turns.counters().emitted, 0);
    assert_eq!(turns.counters().superseded, 1);
    assert_eq!(turns.counters().late_after_superseded, 1);
    assert_eq!(turns.open_turns(), 0);
}

#[test]
fn a_superseded_empty_marker_is_not_recreated_either() {
    let mut turns = TurnAccumulator::new();
    turns.on_task_complete("s", Some("x"));
    turns.on_task_started("n");
    let late = turns.on_final_answer("s", fragment("x", 0));
    let closure = turns.on_task_complete("s", Some("x"));
    assert!(emit_text(&late).is_none());
    assert!(emit_text(&closure).is_none());
    assert_eq!(turns.open_turns(), 0);
}

#[test]
fn a_routed_turn_superseded_then_replayed_emits_only_once() {
    let mut turns = TurnAccumulator::new();
    turns.on_final_answer("r", fragment("body", 0));
    assert!(emit_text(&turns.on_task_complete("r", Some("body"))).is_some());
    assert_eq!(
        turns.on_task_started("n"),
        vec![TurnEvent::Superseded {
            turn_id: "r".to_owned()
        }]
    );
    let late = turns.on_final_answer("r", fragment("body", 1));
    let closure = turns.on_task_complete("r", Some("body"));
    assert!(emit_text(&late).is_none());
    assert!(emit_text(&closure).is_none());
    assert_eq!(turns.counters().emitted, 1);
}

/// Emit and supersede 200 turns `id0`..`id199`.
fn supersede_many(turns: &mut TurnAccumulator) {
    for i in 0..200u64 {
        let id = format!("id{i}");
        turns.on_final_answer(&id, fragment("x", i));
        assert!(emit_text(&turns.on_task_complete(&id, Some("x"))).is_some());
        turns.on_task_started(&format!("next{i}"));
    }
}

#[test]
fn overflow_replay_past_many_supersedes_still_never_emits() {
    let mut turns = TurnAccumulator::new();
    supersede_many(&mut turns);
    for id in ["id0", "id64", "id199"] {
        let late = turns.on_final_answer(id, fragment("x", 0));
        let closure = turns.on_task_complete(id, Some("x"));
        assert!(emit_text(&late).is_none(), "{id}");
        assert!(emit_text(&closure).is_none(), "{id}");
    }
    assert_eq!(turns.counters().emitted, 200);
    assert_eq!(turns.counters().late_after_superseded, 3);
}

// Delivery control: the fix must not block a healthy fresh turn.
#[test]
fn a_healthy_turn_still_emits_after_many_supersedes() {
    let mut turns = TurnAccumulator::new();
    supersede_many(&mut turns);
    turns.on_final_answer("fresh", fragment("fresh body", 5));
    let events = turns.on_task_complete("fresh", Some("fresh body"));
    assert_eq!(events.len(), 1);
    let emitted = emit_text(&events).expect("a fresh turn still emits");
    assert_eq!(emitted.text, "fresh body");
    assert_eq!(emitted.first_record_start, Some(5));
}
