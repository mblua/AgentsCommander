use super::*;

const READY_TITLE: &str = r#"ac4248:{"generation":"g1n0","frame":"control"}"#;

fn page(frame_type: &str, beacon: Value, title: &str) -> Value {
    json!({
        "status": "ok",
        "page": {
            "title": title,
            "readyState": "complete",
            "visibilityState": "visible",
            "hasFocus": false,
            "frameType": frame_type,
            "beacon": beacon,
        },
    })
}

fn beacon(frame_calls: u64, raf_requested: u64, raf_ticks: u64) -> Value {
    json!({
        "version": 1,
        "moduleUtc": "2026-11-16T00:00:00.000Z",
        "phase": "module",
        "frameCalls": frame_calls,
        "lastGeneration": null,
        "rafRequested": raf_requested,
        "rafTicks": raf_ticks,
    })
}

#[test]
fn title_log_bounded() {
    let mut log = TitleLog::default();
    let long = "x".repeat(MAX_TITLE_CHARS + 10);
    for _ in 0..=MAX_ENTRIES {
        log.record("control", &long);
    }
    assert_eq!(log.entries.len(), 128);
    assert_eq!(log.dropped, 1);
    assert!(!log.complete());
    assert!(log.seen("control").iter().all(|title| title.len() == 512));
    assert!(log.seen("target").is_empty());
    let value = log.to_json();
    assert_eq!(value["control"].as_array().map(Vec::len), Some(128));
    assert_eq!(value["dropped"], 1);
    assert!(value.get("target").is_none());
}

#[test]
fn probe_unanswered() {
    let timeout = read_probe(None);
    let failed = read_probe(Some(""));
    assert_eq!(timeout["status"], "callback_timeout");
    assert_eq!(failed["status"], "eval_failed");
    assert_eq!(classify(&timeout, &[], true), "probe_unanswered");
    assert_eq!(classify(&failed, &[], true), "probe_unanswered");
}

#[test]
fn read_probe_unparsed() {
    let raw = "y".repeat(MAX_RAW_CHARS + 100);
    let value = read_probe(Some(&raw));
    assert_eq!(value["status"], "unparsed");
    assert_eq!(value["raw"].as_str().map(str::len), Some(2048));
    assert_eq!(read_probe(Some("[1]"))["status"], "unparsed");
    assert_eq!(read_probe(Some(r#"{"title":"t"}"#))["status"], "ok");
}

#[test]
fn h_a_signature() {
    let diagnostics = page("undefined", Value::Null, "ac4248:loading");
    assert_eq!(classify(&diagnostics, &[], true), "H-a");
}

#[test]
fn fixture_inconsistent_shapes() {
    let no_frame_calls = json!({ "version": 1, "rafRequested": 1, "rafTicks": 1 });
    let mut text_ticks = beacon(1, 1, 1);
    text_ticks["rafTicks"] = json!("1");
    let shapes = [
        page("function", Value::Null, "ac4248:loading"),
        page("undefined", beacon(0, 0, 0), "ac4248:loading"),
        page("function", no_frame_calls, "ac4248:loading"),
        page("function", text_ticks, "ac4248:loading"),
    ];
    for diagnostics in &shapes {
        assert_eq!(classify(diagnostics, &[], true), "fixture_inconsistent");
    }
}

#[test]
fn frame_not_called() {
    let diagnostics = page("function", beacon(0, 0, 0), "ac4248:loading");
    assert_eq!(classify(&diagnostics, &[], true), "frame_not_called");
}

#[test]
fn h_b_signature() {
    let diagnostics = page("function", beacon(1, 1, 0), "ac4248:loading");
    assert_eq!(classify(&diagnostics, &[], true), "H-b");
}

#[test]
fn h_c_signature() {
    let diagnostics = page("function", beacon(1, 2, 2), READY_TITLE);
    assert_eq!(classify(&diagnostics, &[], true), "H-c");
    assert_eq!(classify(&diagnostics, &[READY_TITLE], true), "undetermined");
    assert_eq!(classify(&diagnostics, &[], false), "undetermined");
}

#[test]
fn probe_names_match_fixture() {
    let fixture = include_str!("../../../../tests/fixtures/ac4248-snapshot/main.js");
    assert!(fixture.contains("window.ac4248Beacon ="));
    assert!(fixture.contains("window.ac4248Frame ="));
    assert!(PROBE_SCRIPT.contains("window.ac4248Beacon"));
    assert!(PROBE_SCRIPT.contains("window.ac4248Frame"));
}

#[test]
fn frame_script_shape() {
    let request = json!({ "generation": "g1n0", "frame": "control" });
    let script = frame_script(&request);
    assert!(script.contains(&format!("window.ac4248Frame({request})")));
    for part in ["docId", "called", "try"] {
        assert!(script.contains(part), "missing {part}");
    }
    assert!(!script.contains("delete"));
    let fixture = include_str!("../../../../tests/fixtures/ac4248-snapshot/main.js");
    assert!(fixture.contains("docId:"));
}

fn ack(frame_type: &str, called: bool, doc_id: &str) -> Value {
    let page = json!({ "frameType": frame_type, "called": called, "docId": doc_id });
    json!({ "status": "ok", "page": page })
}

#[test]
fn link_break_tokens() {
    let mut probe = page("function", beacon(1, 1, 0), "ac4248:loading");
    probe["page"]["beacon"]["docId"] = json!("d1");
    let mut lost = probe.clone();
    lost["page"]["beacon"]["frameCalls"] = json!(0);
    let good = ack("function", true, "d1");
    let (failed, unparsed) = (read_probe(Some("")), read_probe(Some("[1]")));
    let (missing, threw) = (ack("undefined", false, "d1"), ack("function", false, "d1"));
    let (other, unanswered) = (ack("function", true, "d2"), read_probe(None));
    let cases = [
        (None, &probe, "ack_missing"),
        (Some(&failed), &probe, "ack_eval_failed"),
        (Some(&unparsed), &probe, "ack_unparsed"),
        (Some(&missing), &probe, "ack_frame_missing"),
        (Some(&threw), &probe, "ack_call_threw"),
        (Some(&good), &unanswered, "probe_unanswered"),
        (Some(&other), &probe, "ack_other_document"),
        (Some(&good), &lost, "page_lost_call"),
        (Some(&good), &probe, "called"),
    ];
    for (ack, diagnostics, token) in cases {
        assert_eq!(link_break(ack, diagnostics), token);
    }
    let mut no_doc = probe.clone();
    no_doc["page"]["beacon"]["docId"] = Value::Null;
    assert_eq!(link_break(Some(&good), &no_doc), "ack_other_document");
}

#[test]
fn trace_target_matches_recipe() {
    let recipe = include_str!("../../../../scripts/ac4248-linux-p03.sh");
    assert!(recipe.contains(&format!("RUST_LOG={LINK_TARGET}=trace")));
    assert!(recipe.contains("RUST_LOG=$RUST_LOG"));
    assert!(recipe.contains("'link=load'"));
    let main = include_str!("../main.rs");
    assert!(main.contains(r#"default_filter_or("off")"#));
}
