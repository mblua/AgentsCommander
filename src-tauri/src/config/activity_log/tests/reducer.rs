use super::*;
use std::collections::BTreeMap;

// ---------------------------------------------------------------------
// §10.3 - the reference reducer
//
// A test-only implementation of the consumer contract, which is what makes
// "total busy time is derivable from the file alone" objectively
// verifiable without shipping a consumer surface.
//
// It walks records in FILE order and uses `at` only for arithmetic. That
// ordering is what makes an inverted pair from the spawn-order race
// observable at all; sorting the stream by `at` would turn every such pair
// into an orphan plus an unclosed interval and the clamp could never fire.
// ---------------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Eq)]
struct ReducerReport {
    total_busy_ms: i64,
    total_busy_raw_ms: i64,
    per_cwd_busy_ms: BTreeMap<String, i64>,
    unparseable_lines: usize,
    orphan_idles: usize,
    busy_after_app_stop: usize,
    discarded_unclosed: usize,
}

type IntervalKey = (String, String);

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str()
}

fn moment(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(field(value, "at")?)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn key_of(value: &Value) -> Option<IntervalKey> {
    Some((
        field(value, "runId")?.to_string(),
        field(value, "sessionId")?.to_string(),
    ))
}

fn reduce(stream: &str) -> ReducerReport {
    let mut report = ReducerReport::default();
    let mut records: Vec<Value> = Vec::new();
    for line in stream.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value)
                if value.get("v").is_some()
                    && field(&value, "runId").is_some()
                    && field(&value, "event").is_some()
                    && moment(&value).is_some() =>
            {
                records.push(value)
            }
            _ => report.unparseable_lines += 1,
        }
    }

    // Rule 3 and rule 4 candidates, collected first because both are stated
    // by records that can appear anywhere in the stream.
    let mut recovery: HashMap<String, DateTime<Utc>> = HashMap::new();
    let mut heartbeat: HashMap<IntervalKey, DateTime<Utc>> = HashMap::new();
    for value in &records {
        match field(value, "event").unwrap_or_default() {
            EVENT_APP_START => {
                let Some(previous) = value.get("previousRun") else {
                    continue;
                };
                // `anchorSeen` is deliberately not consulted.
                if previous.get("clean").and_then(Value::as_bool) != Some(false)
                    || value.get("concurrentInstancePid").is_some()
                {
                    continue;
                }
                let Some(run) = field(previous, "runId") else {
                    continue;
                };
                let Some(last) = field(previous, "lastRecordAt")
                    .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                else {
                    continue;
                };
                recovery.insert(run.to_string(), last.with_timezone(&Utc));
            }
            "app_alive" => {
                let (Some(run), Some(at)) = (field(value, "runId"), moment(value)) else {
                    continue;
                };
                let listed = value
                    .get("workingSessionIds")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for id in listed.iter().filter_map(Value::as_str) {
                    heartbeat.insert((run.to_string(), id.to_string()), at);
                }
            }
            _ => {}
        }
    }

    let mut open: HashMap<IntervalKey, DateTime<Utc>> = HashMap::new();
    let mut cwd_of: HashMap<IntervalKey, String> = HashMap::new();
    let mut stopped: HashMap<String, DateTime<Utc>> = HashMap::new();
    for value in &records {
        let Some(at) = moment(value) else { continue };
        let run = field(value, "runId").unwrap_or_default().to_string();
        match field(value, "event").unwrap_or_default() {
            "busy" => {
                let Some(key) = key_of(value) else { continue };
                if stopped.contains_key(&run) {
                    // No close candidate can be later than this open.
                    report.busy_after_app_stop += 1;
                    continue;
                }
                if let Some(cwd) = field(value, "cwd") {
                    cwd_of.insert(key.clone(), cwd.to_string());
                }
                open.entry(key).or_insert(at);
            }
            "idle" => {
                let Some(key) = key_of(value) else { continue };
                match open.remove(&key) {
                    Some(opened) => {
                        let threshold = value
                            .get("idleThresholdMs")
                            .and_then(Value::as_i64)
                            .unwrap_or_default();
                        close(&mut report, &cwd_of, &key, opened, at, threshold);
                    }
                    None => report.orphan_idles += 1,
                }
            }
            EVENT_APP_STOP => {
                stopped.insert(run.clone(), at);
                let keys: Vec<IntervalKey> = open
                    .keys()
                    .filter(|(candidate, _)| *candidate == run)
                    .cloned()
                    .collect();
                for key in keys {
                    if let Some(opened) = open.remove(&key) {
                        close(&mut report, &cwd_of, &key, opened, at, 0);
                    }
                }
            }
            _ => {}
        }
    }

    let mut leftovers: Vec<(IntervalKey, DateTime<Utc>)> = open.into_iter().collect();
    leftovers.sort();
    for (key, opened) in leftovers {
        let candidate = recovery
            .get(&key.0)
            .copied()
            .or_else(|| heartbeat.get(&key).copied())
            .filter(|close_at| *close_at >= opened);
        match candidate {
            Some(close_at) => close(&mut report, &cwd_of, &key, opened, close_at, 0),
            // Discarded, not extended.
            None => report.discarded_unclosed += 1,
        }
    }
    report
}

fn close(
    report: &mut ReducerReport,
    cwd_of: &HashMap<IntervalKey, String>,
    key: &IntervalKey,
    opened: DateTime<Utc>,
    closed: DateTime<Utc>,
    threshold_ms: i64,
) {
    let raw = closed.signed_duration_since(opened).num_milliseconds();
    let corrected = (raw - threshold_ms).max(0);
    report.total_busy_raw_ms += raw;
    report.total_busy_ms += corrected;
    if let Some(cwd) = cwd_of.get(key) {
        *report.per_cwd_busy_ms.entry(cwd.clone()).or_default() += corrected;
    }
}

// Stream builders for the reducer tests.

fn busy_line(run: &str, session: &str, millis: i64, continues: bool, gap: Option<u64>) -> String {
    let mut record = json!({
        "v": 1,
        "at": at_offset(millis),
        "runId": run,
        "event": "busy",
        "sessionId": session,
        "name": "wg-14-dev-team/dev-rust",
        "cwd": "C:\\replica",
        "reason": "mark_busy",
        "continuesBlock": continues,
    });
    if let Some(gap) = gap {
        record["gapMs"] = json!(gap);
    }
    format!("{record}\n")
}

fn idle_line(run: &str, session: &str, millis: i64, threshold: Option<u64>) -> String {
    let mut record = json!({
        "v": 1,
        "at": at_offset(millis),
        "runId": run,
        "event": "idle",
        "sessionId": session,
        "name": "wg-14-dev-team/dev-rust",
        "cwd": "C:\\replica",
        "reason": "mark_idle",
    });
    match threshold {
        Some(threshold) => record["idleThresholdMs"] = json!(threshold),
        None => record["reason"] = json!("app_stop"),
    }
    format!("{record}\n")
}

fn app_stop_line(run: &str, millis: i64, enumerated: bool, count: usize) -> String {
    format!(
        "{}\n",
        json!({
            "v": 1,
            "at": at_offset(millis),
            "runId": run,
            "event": "app_stop",
            "openSessionsEnumerated": enumerated,
            "openSessionCount": count,
        })
    )
}

fn app_alive_line(run: &str, millis: i64, ids: &[&str]) -> String {
    format!(
        "{}\n",
        json!({
            "v": 1,
            "at": at_offset(millis),
            "runId": run,
            "event": "app_alive",
            "workingSessionIds": ids,
        })
    )
}

fn app_start_line(
    run: &str,
    millis: i64,
    previous: Option<(&str, i64, bool, bool)>,
    concurrent: Option<u32>,
) -> String {
    let mut record = json!({
        "v": 1,
        "at": at_offset(millis),
        "runId": run,
        "event": "app_start",
        "pid": 1234,
        "appVersion": "0.20.0",
        "previousRunScan": "ok",
        "previousRun": Value::Null,
        "metrics": {
            "supported": ["totalBusyMs", "totalBusyRawMs"],
            "unsupported": ["intervalCount", "blockCount"]
        },
    });
    if let Some((previous_run, last_at, clean, anchor_seen)) = previous {
        record["previousRun"] = json!({
            "runId": previous_run,
            "lastRecordAt": at_offset(last_at),
            "lastEvent": "app_alive",
            "clean": clean,
            "anchorSeen": anchor_seen,
        });
    }
    if let Some(pid) = concurrent {
        record["concurrentInstancePid"] = json!(pid);
    }
    format!("{record}\n")
}

const SESSION_A: &str = "3f51d2df-0000-0000-0000-000000000001";
const SESSION_B: &str = "8a5cc5f1-0000-0000-0000-000000000002";

#[test]
fn a_normal_busy_idle_pair_yields_its_span_minus_the_idle_threshold() {
    let stream = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 10_000, Some(2_500))
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 10_000);
    assert_eq!(report.total_busy_ms, 7_500);
    assert_eq!(report.per_cwd_busy_ms.get("C:\\replica"), Some(&7_500));
    assert_eq!(report.orphan_idles, 0);
    assert_eq!(report.discarded_unclosed, 0);
}

#[test]
fn raw_and_corrected_totals_differ_by_exactly_the_sum_of_subtracted_thresholds() {
    let stream = format!(
        "{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 10_000, Some(2_500)),
        busy_line("run-a", SESSION_A, 20_000, true, Some(10_000)),
        idle_line("run-a", SESSION_A, 45_000, Some(2_500))
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 35_000);
    assert_eq!(report.total_busy_ms, 35_000 - 5_000);
}

#[test]
fn an_inverted_pair_from_the_reordering_race_clamps_to_zero() {
    // Only a synthetic stream reaches this: the edge guard means a starved
    // `mark_busy` task cannot produce a recorded pair at all.
    let stream = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 1_000, false, None),
        idle_line("run-a", SESSION_A, 800, Some(2_500))
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_ms, 0, "the clamp holds the total at zero");
    assert_eq!(report.orphan_idles, 0, "the pair was paired, not orphaned");
    assert_eq!(report.discarded_unclosed, 0);
}

#[test]
fn a_run_closed_by_app_stop_leaves_no_open_interval() {
    let stream = format!(
        "{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 4_000, None),
        app_stop_line("run-a", 4_000, true, 1)
    );
    let report = reduce(&stream);
    assert_eq!(report.discarded_unclosed, 0);
    assert_eq!(report.total_busy_raw_ms, 4_000);
    assert_eq!(
        report.total_busy_ms, 4_000,
        "a synthetic close subtracts nothing"
    );
}

#[test]
fn app_stop_closes_intervals_even_when_open_sessions_enumerated_is_false() {
    let stream = format!(
        "{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        busy_line("run-a", SESSION_B, 1_000, false, None),
        app_stop_line("run-a", 6_000, false, 0)
    );
    let report = reduce(&stream);
    assert_eq!(report.discarded_unclosed, 0);
    assert_eq!(report.total_busy_raw_ms, 6_000 + 5_000);
}

#[test]
fn an_unclean_run_is_closed_at_previous_run_last_record_at() {
    let stream = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, true)), None)
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 30_000);
    assert_eq!(report.discarded_unclosed, 0);
}

#[test]
fn an_unclean_run_with_a_concurrent_instance_pid_is_not_closed_by_recovery() {
    let stream = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_start_line(
            "run-b",
            90_000,
            Some(("run-a", 30_000, false, true)),
            Some(41_256)
        )
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 0);
    assert_eq!(report.discarded_unclosed, 1);
}

#[test]
fn recovery_ignores_anchor_seen() {
    let with_anchor = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, true)), None)
    );
    let without_anchor = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_start_line("run-b", 90_000, Some(("run-a", 30_000, false, false)), None)
    );
    assert_eq!(reduce(&with_anchor), reduce(&without_anchor));
    assert_eq!(reduce(&without_anchor).total_busy_raw_ms, 30_000);
}

#[test]
fn an_interval_with_no_closing_record_is_closed_at_the_last_heartbeat_that_listed_it() {
    let stream = format!(
        "{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_alive_line("run-a", 60_000, &[SESSION_A]),
        app_alive_line("run-a", 120_000, &[SESSION_A]),
        app_alive_line("run-a", 180_000, &[SESSION_B])
    );
    let report = reduce(&stream);
    assert_eq!(
        report.total_busy_raw_ms, 120_000,
        "the last heartbeat that listed the session closes it"
    );
    assert_eq!(report.discarded_unclosed, 0);
}

#[test]
fn an_interval_no_rule_can_close_is_discarded_rather_than_extended() {
    let stream = format!(
        "{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        app_alive_line("run-a", 60_000, &[SESSION_B])
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 0);
    assert_eq!(report.total_busy_ms, 0);
    assert_eq!(report.discarded_unclosed, 1);
}

#[test]
fn a_busy_after_app_stop_is_discarded_and_counted_not_clamped() {
    let stream = format!(
        "{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 4_000, None),
        app_stop_line("run-a", 4_000, true, 1),
        busy_line("run-a", SESSION_B, 4_200, false, None)
    );
    let report = reduce(&stream);
    assert_eq!(report.busy_after_app_stop, 1);
    assert_eq!(
        report.discarded_unclosed, 0,
        "it never became an open interval"
    );
    assert_eq!(report.total_busy_raw_ms, 4_000);
}

#[test]
fn an_orphan_idle_is_discarded_and_counted_not_faulted() {
    let stream = idle_line("run-a", SESSION_A, 4_000, Some(2_500));
    let report = reduce(&stream);
    assert_eq!(report.orphan_idles, 1);
    assert_eq!(report.total_busy_raw_ms, 0);
    assert_eq!(report.total_busy_ms, 0);
}

#[test]
fn unparseable_lines_are_skipped_and_counted() {
    let stream = format!(
        concat!(
            "not json at all\n{}",
            "{{\"v\":1,\"at\":\"2026-07-26T00:00:00.000Z\",\"runId\":\"run-a\"}}\n{}"
        ),
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 3_000, Some(2_500))
    );
    let report = reduce(&stream);
    assert_eq!(
        report.unparseable_lines, 2,
        "one non-JSON line and one lacking `event`"
    );
    assert_eq!(report.total_busy_raw_ms, 3_000);
    assert_eq!(report.total_busy_ms, 500);
}

#[test]
fn a_coalesced_block_reports_the_same_total_busy_ms_as_the_raw_pairs() {
    // Three pairs separated by sub-window gaps, so the second and third
    // opening edges are annotated as continuations.
    let annotated = format!(
        "{}{}{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 20_000, Some(2_500)),
        busy_line("run-a", SESSION_A, 28_000, true, Some(8_000)),
        idle_line("run-a", SESSION_A, 50_000, Some(2_500)),
        busy_line("run-a", SESSION_A, 55_000, true, Some(5_000)),
        idle_line("run-a", SESSION_A, 70_000, Some(2_500))
    );
    // The same edges with the annotation stripped to `false`.
    let unannotated = format!(
        "{}{}{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        idle_line("run-a", SESSION_A, 20_000, Some(2_500)),
        busy_line("run-a", SESSION_A, 28_000, false, None),
        idle_line("run-a", SESSION_A, 50_000, Some(2_500)),
        busy_line("run-a", SESSION_A, 55_000, false, None),
        idle_line("run-a", SESSION_A, 70_000, Some(2_500))
    );
    let report = reduce(&annotated);
    assert_eq!(report.total_busy_raw_ms, 20_000 + 22_000 + 15_000);
    assert_eq!(report.total_busy_ms, 57_000 - 7_500);
    assert_eq!(
        report,
        reduce(&unannotated),
        "the block annotation must cost nothing in either total"
    );
}

#[test]
fn records_from_two_interleaved_run_ids_are_reduced_independently() {
    // The same session id under two runs, interleaved, plus an app_stop for
    // one run that must not close the other run's interval.
    let stream = format!(
        "{}{}{}{}{}",
        busy_line("run-a", SESSION_A, 0, false, None),
        busy_line("run-b", SESSION_A, 1_000, false, None),
        idle_line("run-a", SESSION_A, 5_000, Some(2_500)),
        app_stop_line("run-a", 5_000, true, 0),
        idle_line("run-b", SESSION_A, 9_000, Some(2_500))
    );
    let report = reduce(&stream);
    assert_eq!(report.total_busy_raw_ms, 5_000 + 8_000);
    assert_eq!(report.total_busy_ms, 2_500 + 5_500);
    assert_eq!(report.orphan_idles, 0);
    assert_eq!(report.discarded_unclosed, 0);
}

#[test]
fn a_session_start_busy_opens_an_interval_that_the_first_idle_closes() {
    let mut opening = busy_line("run-a", SESSION_A, 0, false, None);
    opening = opening.replace("\"mark_busy\"", "\"session_start\"");
    let stream = format!(
        "{opening}{}",
        idle_line("run-a", SESSION_A, 6_000, Some(2_500))
    );
    let report = reduce(&stream);
    assert!(stream.contains("\"reason\":\"session_start\""));
    assert_eq!(report.total_busy_raw_ms, 6_000);
    assert_eq!(report.total_busy_ms, 3_500);
    assert_eq!(report.orphan_idles, 0);
}
