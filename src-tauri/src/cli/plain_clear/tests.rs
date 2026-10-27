use super::*;
use std::cell::Cell;
use std::collections::VecDeque;

fn binding() -> Binding<'static> {
    Binding {
        action: "self-clear",
        request_id: "11111111-1111-4111-8111-111111111111",
        requested_by: "project:room/agent",
    }
}

fn receipt() -> serde_json::Value {
    let b = binding();
    serde_json::json!({"action": b.action, "request_id": b.request_id,
        "requested_by": b.requested_by, "status": "accepted", "session_id": Uuid::new_v4()})
}

fn empty() -> Observation {
    Observation {
        ack: Ok(None),
        rejection: Ok(None),
        delivered: Ok(false),
    }
}

fn with_ack(value: &serde_json::Value) -> Observation {
    Observation {
        ack: Ok(Some(value.to_string())),
        ..empty()
    }
}

fn exercise(
    states: Vec<Observation>,
    timeout: u64,
    publish_error: bool,
) -> (Outcome, u32, Duration, u32) {
    let publications = Cell::new(0);
    let reads = Cell::new(0);
    let clock = Cell::new(Duration::ZERO);
    let mut states: VecDeque<_> = states.into();
    let result = run(
        &binding(),
        timeout,
        Callbacks {
            publish: || {
                publications.set(publications.get() + 1);
                if publish_error {
                    Err("write failed".into())
                } else {
                    Ok(())
                }
            },
            read: || {
                reads.set(reads.get() + 1);
                states.pop_front().unwrap_or_else(empty)
            },
            now: || clock.get(),
            wait: |duration| {
                assert!(duration <= Duration::from_millis(250));
                clock.set(clock.get() + duration);
            },
        },
    );
    (result, publications.get(), clock.get(), reads.get())
}

#[test]
fn accepted_before_delivery_and_despite_transport_failure_is_success() {
    for delivered in [Ok(false), Ok(true), Err("delivered read failed".into())] {
        let state = Observation {
            delivered,
            ..with_ack(&receipt())
        };
        let (result, published, elapsed, reads) = exercise(vec![state], 15, false);
        assert_eq!(result.code, 0);
        assert_eq!(published, 1);
        assert_eq!(elapsed, Duration::ZERO);
        assert_eq!(reads, 1);
        assert!(result.message.contains("not confirmed"));
        assert!(result.message.contains("without idle wait"));
    }
}

#[test]
fn bound_ack_has_priority_over_later_rejection_and_rejection_read_error() {
    for rejection in [
        Ok(Some("delivered failed after launch".into())),
        Err("read failed".into()),
    ] {
        let state = Observation {
            rejection,
            ..with_ack(&receipt())
        };
        assert_eq!(exercise(vec![state], 15, false).0.code, 0);
    }
}

#[test]
fn each_binding_status_and_session_uuid_is_required() {
    for (field, value) in [
        ("action", serde_json::json!("self-handoff-and-clear")),
        ("request_id", serde_json::json!(Uuid::new_v4())),
        ("requested_by", serde_json::json!("other")),
        ("status", serde_json::json!("queued")),
        ("status", serde_json::json!("already_queued")),
        ("session_id", serde_json::json!("")),
        ("session_id", serde_json::json!("invalid")),
        ("session_id", serde_json::json!(42)),
    ] {
        let mut changed = receipt();
        changed[field] = value;
        assert_eq!(
            exercise(vec![with_ack(&changed)], 15, false).0.code,
            2,
            "{field}"
        );
    }
    for field in [
        "action",
        "request_id",
        "requested_by",
        "status",
        "session_id",
    ] {
        let mut changed = receipt();
        changed.as_object_mut().unwrap().remove(field);
        assert_eq!(
            exercise(vec![with_ack(&changed)], 15, false).0.code,
            2,
            "{field}"
        );
    }
}

#[test]
fn malformed_and_non_object_receipts_are_unknown_without_replay() {
    for bytes in ["invalid", "{partial", "null", "[]", "42", "\"accepted\""] {
        let state = Observation {
            ack: Ok(Some(bytes.into())),
            ..empty()
        };
        let (result, published, _, reads) = exercise(vec![state], 15, false);
        assert_eq!(result.code, 2);
        assert_eq!(published, 1);
        assert_eq!(reads, 1);
    }
}

#[test]
fn rejection_without_ack_is_one_and_publication_failure_does_not_poll() {
    let state = Observation {
        rejection: Ok(Some("session conflict".into())),
        ..empty()
    };
    let (result, published, _, _) = exercise(vec![state], 15, false);
    assert_eq!(result.code, 1);
    assert!(result.message.contains("session conflict"));
    assert_eq!(published, 1);
    let (result, published, _, reads) = exercise(vec![], 15, true);
    assert_eq!(result.code, 1);
    assert_eq!(published, 1);
    assert_eq!(reads, 0);
}

#[test]
fn cli_read_failures_are_unknown_and_do_not_cancel_or_republish() {
    for state in [
        Observation {
            ack: Err("ack read failed".into()),
            ..empty()
        },
        Observation {
            rejection: Err("reason read failed".into()),
            ..empty()
        },
    ] {
        let (result, published, _, reads) = exercise(vec![state], 15, false);
        assert_eq!(result.code, 2);
        assert_eq!(published, 1);
        assert_eq!(reads, 1);
        assert!(result
            .message
            .contains("no cancellation or automatic retry"));
    }
}

#[test]
fn unpublished_stage_or_delivered_without_ack_uses_one_deadline() {
    for delivered in [false, true] {
        let state = Observation {
            delivered: Ok(delivered),
            ..empty()
        };
        let (result, published, elapsed, _) = exercise(vec![state], 15, false);
        assert_eq!(result.code, 2);
        assert_eq!(published, 1);
        assert_eq!(elapsed, Duration::from_secs(15));
        assert!(result.message.contains("outcome unknown"));
    }
}

#[test]
fn deadline_starts_at_publication_and_never_restarts_after_delivery() {
    let clock = Cell::new(Duration::ZERO);
    let publications = Cell::new(0);
    let result = run(
        &binding(),
        1,
        Callbacks {
            publish: || {
                clock.set(Duration::from_secs(8));
                publications.set(1);
                Ok(())
            },
            read: || Observation {
                delivered: Ok(true),
                ..empty()
            },
            now: || clock.get(),
            wait: |d| clock.set(clock.get() + d),
        },
    );
    assert_eq!(result.code, 2);
    assert_eq!(publications.get(), 1);
    assert_eq!(clock.get(), Duration::from_secs(9));
}

#[test]
fn args_defaults_flags_and_honest_help() {
    use clap::{CommandFactory, Parser};
    #[derive(Parser)]
    struct Parse {
        #[command(flatten)]
        args: PlainClearArgs,
    }
    let args = Parse::try_parse_from(["test", "--token", "token", "--root", "root"])
        .unwrap()
        .args;
    assert_eq!(args.timeout, 15);
    assert_eq!(args.token.as_deref(), Some("token"));
    assert_eq!(args.root.as_deref(), Some("root"));
    let args = Parse::try_parse_from(["test", "--timeout", "42"])
        .unwrap()
        .args;
    assert_eq!(args.timeout, 42);
    for flag in ["--to", "--target", "--handoff", "--resume", "--idle"] {
        assert!(Parse::try_parse_from(["test", flag, "value"]).is_err());
    }
    assert!(Parse::try_parse_from(["test", "--timeout", "-1"]).is_err());
    let help = Parse::command().render_long_help().to_string();
    assert!(help.contains("OWN live session"));
    assert!(help.contains("unknown"));
    assert!(help.contains("does not confirm"));
    assert!(help.contains("automatically retry"));
}
