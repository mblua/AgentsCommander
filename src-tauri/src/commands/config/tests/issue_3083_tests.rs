use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

fn record() -> Value {
    json!({"version":1,"windowId":"fedcba98-7654-4321-8abc-012345678901",
        "seq":1,"atMs":0.0,"surface":"embedded","event":"pointerdown"})
}

#[derive(Default)]
struct Counter {
    enabled: AtomicUsize,
    emitted: AtomicUsize,
}
impl log::Log for Counter {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        assert_eq!(metadata.level(), log::Level::Debug);
        assert_eq!(metadata.target(), "agentscommander_lib::commands::config");
        self.enabled.fetch_add(1, Ordering::Relaxed);
        true
    }
    fn log(&self, record: &log::Record) {
        assert_eq!(record.level(), log::Level::Debug);
        assert_eq!(record.target(), "agentscommander_lib::commands::config");
        let line = record.args().to_string();
        let json = line.strip_prefix("[rail-diag]").unwrap();
        let parsed: Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["seq"], 1);
        assert_eq!(serde_json::to_string(&parsed).unwrap().len(), json.len());
        assert!(!line.contains('\n'));
        self.emitted.fetch_add(1, Ordering::Relaxed);
    }
    fn flush(&self) {}
}

fn reject(value: Option<Value>) {
    let logger = Counter::default();
    assert_eq!(
        append_rail_diagnostic_to(value.clone(), &logger),
        Err(RAIL_DIAGNOSTIC_ERROR.into())
    );
    assert_eq!(
        append_rail_diagnostic(value),
        Err(RAIL_DIAGNOSTIC_ERROR.into())
    );
    assert_eq!(logger.enabled.load(Ordering::Relaxed), 0);
    assert_eq!(logger.emitted.load(Ordering::Relaxed), 0);
}

#[test]
fn issue_3083_private_boundary_and_single_debug_emission() {
    let logger = Counter::default();
    assert_eq!(append_rail_diagnostic_to(Some(record()), &logger), Ok(()));
    assert_eq!(logger.enabled.load(Ordering::Relaxed), 1);
    assert_eq!(logger.emitted.load(Ordering::Relaxed), 1);
    reject(None);
    for value in [
        Value::Null,
        json!(true),
        json!("private/path"),
        json!(3),
        json!([]),
        json!({}),
    ] {
        reject(Some(value));
    }
    let mut extra = record();
    extra["unknown"] = json!({"nested":"secret"});
    reject(Some(extra));
    for (field, value) in [
        ("surface", json!({"embedded":null})),
        ("event", json!({"pointerdown":null})),
        ("kind", json!({"custom":null})),
        ("windowId", json!([0, 1, 2, 3])),
    ] {
        let mut nested = record();
        nested[field] = value;
        reject(Some(nested));
    }
    let mut huge = record();
    huge["windowId"] = json!("é".repeat(2049));
    assert!(serde_json::to_vec(&huge).unwrap().len() > 2048);
    reject(Some(huge));
}

#[test]
fn issue_3083_numeric_type_and_range_validation() {
    for field in [
        "seq",
        "railId",
        "operationId",
        "projectId",
        "groupId",
        "entityId",
        "nodeId",
        "originalNodeId",
        "objectId",
        "selectionProjectId",
        "selectionGroupId",
    ] {
        for bad in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_992u64),
            json!("1"),
            json!({"unknown":1}),
        ] {
            let mut value = record();
            value[field] = bad;
            reject(Some(value));
        }
        let mut value = record();
        value[field] = json!(RAIL_SAFE_INTEGER);
        assert!(rail_diagnostic_line(Some(value)).is_ok());
    }
    for field in ["count", "changedCount"] {
        for bad in [json!(-1), json!(0.5), json!(9_007_199_254_740_992u64)] {
            let mut value = record();
            value[field] = bad;
            reject(Some(value));
        }
        let mut value = record();
        value[field] = json!(0);
        assert!(rail_diagnostic_line(Some(value)).is_ok());
    }
    for (field, bad) in [
        ("version", json!(2)),
        ("version", json!("1")),
        ("windowId", json!("not-a-uuid")),
        ("atMs", json!(-0.1)),
        ("atMs", json!("0")),
        ("durationMs", json!(-1)),
        ("pointerId", json!(2147483648u64)),
        ("pointerId", json!(-2147483649i64)),
        ("button", json!(-2)),
        ("button", json!(6)),
    ] {
        let mut value = record();
        value[field] = bad;
        reject(Some(value));
    }
    for field in [
        "primary",
        "trusted",
        "connected",
        "originalConnected",
        "captured",
        "documentFocused",
    ] {
        let mut value = record();
        value[field] = json!(1);
        reject(Some(value));
    }
    let mut dto: RailDiagnosticRecord = serde_json::from_value(record()).unwrap();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        dto.at_ms = bad;
        assert!(!dto.validate());
        dto.at_ms = 0.0;
        dto.duration_ms = Some(bad);
        assert!(!dto.validate());
        dto.duration_ms = None;
    }
}

#[test]
fn issue_3083_closed_enums_and_full_record() {
    for (field, values) in [
        ("surface", "embedded sidebar store"),
        ("kind", "custom system nonstop ungrouped project favorites"),
        ("selectionKind", "custom system nonstop ungrouped project favorites"),
        ("phase", "arming dragging saving idle"),
        ("outcome", "applied unchanged stale error disconnected unloaded queued discarded suppressed overflow no-press"),
        ("targetTag", "button span svg path aside body input textarea other none"),
        ("activeTag", "button span svg path aside body input textarea other none"),
        ("event", "pointerdown pointerup pointercancel mousedown mouseup click gotpointercapture lostpointercapture focusin focusout window-focus window-blur handler-enter handler-end selection-change rail-mount rail-dispose section-mount section-dispose node-mount node-dispose project-list project-buttons favorites-entries reload-start reload-end discover-start discover-end snapshot-start snapshot-end snapshots-start snapshots-end task-start task-end loop-start loop-end"),
    ] {
        for good in values.split_whitespace() {
            let mut value = record(); value[field] = json!(good);
            assert!(rail_diagnostic_line(Some(value)).is_ok(), "{field}: {good}");
        }
        for bad in [json!("private text"), json!(1), json!({"unknown":"nested"})] {
            let mut value = record(); value[field] = bad; reject(Some(value));
        }
    }
    let mut full = record();
    for field in [
        "railId",
        "operationId",
        "projectId",
        "groupId",
        "entityId",
        "nodeId",
        "originalNodeId",
        "objectId",
        "selectionProjectId",
        "selectionGroupId",
        "count",
        "changedCount",
    ] {
        full[field] = json!(RAIL_SAFE_INTEGER);
    }
    for field in [
        "primary",
        "trusted",
        "connected",
        "originalConnected",
        "captured",
        "documentFocused",
    ] {
        full[field] = json!(true);
    }
    full["kind"] = json!("custom");
    full["selectionKind"] = json!("ungrouped");
    full["phase"] = json!("dragging");
    full["outcome"] = json!("no-press");
    full["pointerId"] = json!(i32::MIN);
    full["button"] = json!(-1);
    full["targetTag"] = json!("textarea");
    full["activeTag"] = json!("none");
    full["durationMs"] = json!(f64::MAX);
    let line = rail_diagnostic_line(Some(full)).unwrap();
    assert!(line.len() <= 2048 + "[rail-diag]".len());
}
