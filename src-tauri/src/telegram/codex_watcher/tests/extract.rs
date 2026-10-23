use super::*;

// ── extract_assistant_final: accepted current format ──────────────────

#[test]
fn real_current_codex_final() {
    assert_eq!(
        extract_assistant_final(REAL_CURRENT_CODEX_FINAL),
        Some("sanitized assistant reply".to_string())
    );
}

#[test]
fn extract_assistant_final_accepts_synthetic_positive_control() {
    // Independent synthetic record (not the real fixture) exercising the
    // accepted path: response_item / message / assistant / final_answer.
    let line = current_final_record(&["Synthetic final answer."]);
    assert_eq!(
        extract_assistant_final(&line),
        Some("Synthetic final answer.".to_string())
    );
}

#[test]
fn extract_assistant_final_rejects_task_and_tool_records() {
    // Negative controls: task lifecycle and tool traffic must stay silent.
    let task_started = serde_json::json!({
        "timestamp": "2026-09-13T18:28:09.828Z",
        "type": "event_msg",
        "payload": {"type": "task_started"}
    })
    .to_string();
    let task_complete = serde_json::json!({
        "type": "event_msg",
        "payload": {"type": "task_complete", "last_agent_message": "x"}
    })
    .to_string();
    let tool_call = serde_json::json!({
        "type": "response_item",
        "payload": {"type": "function_call", "name": "shell", "arguments": "{}"}
    })
    .to_string();
    let tool_output = serde_json::json!({
        "type": "response_item",
        "payload": {"type": "function_call_output", "call_id": "c1", "output": "ok"}
    })
    .to_string();
    for (label, line) in [
        ("task_started", task_started),
        ("task_complete", task_complete),
        ("function_call", tool_call),
        ("function_call_output", tool_output),
    ] {
        assert_eq!(extract_assistant_final(&line), None, "{}", label);
    }
}

#[test]
fn extract_assistant_final_handles_compact_and_spaced_json() {
    let compact = current_final_record(&["body"]);
    let spaced = r#"{
            "type" : "response_item",
            "payload" : {
                "type" : "message",
                "role" : "assistant",
                "phase" : "final_answer",
                "content" : [ { "type" : "output_text", "text" : "body" } ]
            }
        }"#;
    assert_eq!(extract_assistant_final(&compact), Some("body".to_string()));
    assert_eq!(extract_assistant_final(spaced), Some("body".to_string()));
}

#[test]
fn extract_assistant_final_joins_text_blocks_in_array_order() {
    let line = current_final_record(&["first block", "second block", "third block"]);
    assert_eq!(
        extract_assistant_final(&line),
        Some("first block\nsecond block\nthird block".to_string())
    );
    // Only the outer whitespace of the joined result is trimmed.
    let line = current_final_record(&["  padded  "]);
    assert_eq!(extract_assistant_final(&line), Some("padded".to_string()));
}

#[test]
fn extract_assistant_final_skips_malformed_blocks_but_keeps_valid_ones() {
    let mut v = valid_final_value();
    v["payload"]["content"] = serde_json::json!([
        {"type": "output_text", "text": "kept one"},
        null,
        42,
        "a bare string",
        {"type": "input_text", "text": "not output_text"},
        {"type": 7, "text": "non-string type"},
        {"type": "output_text", "text": 99},
        {"type": "output_text"},
        {"type": "output_text", "text": "kept two"},
    ]);
    assert_eq!(
        extract_assistant_final(&v.to_string()),
        Some("kept one\nkept two".to_string())
    );
}

#[test]
fn extract_assistant_final_returns_none_for_empty_or_all_invalid_content() {
    let cases: Vec<serde_json::Value> = vec![
        serde_json::json!([]),
        serde_json::json!([null, 1, "text"]),
        serde_json::json!([{"type": "input_text", "text": "x"}]),
        serde_json::json!([{"type": "output_text", "text": ""}]),
        serde_json::json!([{"type": "output_text", "text": "   \n\t "}]),
    ];
    for content in cases {
        let mut v = valid_final_value();
        v["payload"]["content"] = content;
        assert_eq!(
            extract_assistant_final(&v.to_string()),
            None,
            "content={}",
            v["payload"]["content"]
        );
    }
}

#[test]
fn extract_assistant_final_returns_none_for_malformed_json() {
    for line in [
        "",
        "not json at all",
        "{\"type\":\"response_item\"",
        "null",
        "[1,2,3]",
        "{ this is not JSON }",
    ] {
        assert_eq!(extract_assistant_final(line), None, "line={:?}", line);
    }
}

#[test]
fn extract_assistant_final_rejects_every_other_type_role_and_phase() {
    // Top-level types other than `response_item` (event_msg included).
    for kind in [
        "session_meta",
        "event_msg",
        "turn_context",
        "world_state",
        "token_usage_record",
        "compacted",
        "Response_Item",
    ] {
        let line = serde_json::json!({"type": kind, "payload": valid_final_payload()}).to_string();
        assert_eq!(extract_assistant_final(&line), None, "type={}", kind);
    }

    // Payload types other than `message`.
    for ptype in [
        "reasoning",
        "function_call",
        "function_call_output",
        "input_text",
        "output_text",
        "message_summary",
        "Message",
    ] {
        let line = serde_json::json!({
            "type": "response_item",
            "payload": {"type": ptype, "role": "assistant", "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "body"}]}
        })
        .to_string();
        assert_eq!(
            extract_assistant_final(&line),
            None,
            "payload.type={}",
            ptype
        );
    }

    // Roles other than `assistant`.
    for role in ["user", "system", "developer", "tool", "Assistant", ""] {
        let mut payload = valid_final_payload();
        payload["role"] = serde_json::json!(role);
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(extract_assistant_final(&line), None, "role={}", role);
    }

    // Phases other than `final_answer`.
    for phase in [
        "commentary",
        "analysis",
        "final",
        "Final_Answer",
        "final_answer ",
        "",
    ] {
        let mut payload = valid_final_payload();
        payload["phase"] = serde_json::json!(phase);
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(extract_assistant_final(&line), None, "phase={}", phase);
    }
    // Missing phase entirely.
    let mut payload = valid_final_payload();
    payload.as_object_mut().unwrap().remove("phase");
    let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
    assert_eq!(extract_assistant_final(&line), None, "phase absent");
}

#[test]
fn extract_assistant_final_rejects_absent_or_non_string_fields() {
    // Missing / non-string top-level type.
    let mut v = valid_final_value();
    v.as_object_mut().unwrap().remove("type");
    assert_eq!(extract_assistant_final(&v.to_string()), None, "type absent");
    let mut v = valid_final_value();
    v["type"] = serde_json::json!(7);
    assert_eq!(
        extract_assistant_final(&v.to_string()),
        None,
        "type non-string"
    );

    // Missing / non-object payload.
    let mut v = valid_final_value();
    v.as_object_mut().unwrap().remove("payload");
    assert_eq!(
        extract_assistant_final(&v.to_string()),
        None,
        "payload absent"
    );
    let mut v = valid_final_value();
    v["payload"] = serde_json::json!("not an object");
    assert_eq!(
        extract_assistant_final(&v.to_string()),
        None,
        "payload non-object"
    );

    // Missing payload fields.
    for field in ["type", "role", "phase", "content"] {
        let mut payload = valid_final_payload();
        payload.as_object_mut().unwrap().remove(field);
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(
            extract_assistant_final(&line),
            None,
            "payload.{} absent",
            field
        );
    }

    // Non-string payload fields.
    for field in ["type", "role", "phase"] {
        let mut payload = valid_final_payload();
        payload[field] = serde_json::json!(["not", "a", "string"]);
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(
            extract_assistant_final(&line),
            None,
            "payload.{} non-string",
            field
        );
    }

    // Content not an array.
    for content in [
        serde_json::json!("body"),
        serde_json::json!({"type": "output_text"}),
        serde_json::json!(3),
    ] {
        let mut payload = valid_final_payload();
        payload["content"] = content;
        let line = serde_json::json!({"type": "response_item", "payload": payload}).to_string();
        assert_eq!(extract_assistant_final(&line), None, "content non-array");
    }
}

#[test]
fn extract_assistant_final_rejects_legacy_event_msg_records() {
    // Historical Codex format: event_msg / agent_message. Rejected even
    // when its phase string matches the current vocabulary.
    for phase in ["commentary", "final", "final_answer"] {
        let line = serde_json::json!({
            "timestamp": "2026-05-19T05:00:00Z",
            "type": "event_msg",
            "payload": {"type": "agent_message", "message": "  legacy prose  ", "phase": phase}
        })
        .to_string();
        assert_eq!(
            extract_assistant_final(&line),
            None,
            "legacy phase={}",
            phase
        );
    }
    // Legacy envelope carrying response-shaped payload fields.
    let line = serde_json::json!({
        "type": "event_msg",
        "payload": {"type": "agent_message", "role": "assistant", "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "legacy-shaped"}]}
    })
    .to_string();
    assert_eq!(
        extract_assistant_final(&line),
        None,
        "event_msg with response fields"
    );
}

#[test]
fn extraction_is_independent_of_session_metadata_variants() {
    let base = current_final_record(&["stable body"]);
    let mut with_meta = serde_json::from_str::<serde_json::Value>(&base).unwrap();
    with_meta["cli_version"] = serde_json::json!("0.154.0");
    with_meta["originator"] = serde_json::json!("codex-tui");
    let mut changed_meta = with_meta.clone();
    changed_meta["cli_version"] = serde_json::json!("9.9.9");
    changed_meta["originator"] = serde_json::json!("other-originator");
    for line in [base, with_meta.to_string(), changed_meta.to_string()] {
        assert_eq!(
            extract_assistant_final(&line),
            Some("stable body".to_string()),
            "line={}",
            line
        );
    }
}
