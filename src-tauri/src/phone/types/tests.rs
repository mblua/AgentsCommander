use super::*;

fn base_message() -> OutboxMessage {
    OutboxMessage {
        id: "msg-1".into(),
        token: None,
        from: "from".into(),
        to: "to".into(),
        body: String::new(),
        mode: "wake".into(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: String::new(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".into(),
        timestamp: "2026-06-28T00:00:00Z".into(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

#[test]
fn outbox_switch_fields_serialize_as_camel_case() {
    let mut msg = base_message();
    msg.switch_coding_agent = Some("codex-main".into());
    msg.switch_profile = Some("B".into());
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["switchCodingAgent"], "codex-main");
    assert_eq!(json["switchProfile"], "B");
}

#[test]
fn outbox_message_requested_profile_round_trips() {
    let mut msg = base_message();
    msg.requested_profile = Some("C".into());
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["requestedProfile"], "C");

    // Round-trip preserves the value.
    let restored: OutboxMessage = serde_json::from_value(json).unwrap();
    assert_eq!(restored.requested_profile.as_deref(), Some("C"));

    // Absence deserializes to None.
    let absent = base_message();
    assert!(absent.requested_profile.is_none());
    assert!(!serde_json::to_value(&absent)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("requestedProfile"));

    // Old JSON without the key still deserializes (backward compatibility).
    let old = serde_json::json!({
        "id": "msg-1", "from": "from", "to": "to", "body": "",
        "timestamp": "2026-06-28T00:00:00Z"
    });
    let legacy: OutboxMessage = serde_json::from_value(old).unwrap();
    assert!(legacy.requested_profile.is_none());
}

#[test]
fn outbox_message_annotation_fields_default_and_omit() {
    // Absent → None/false, and serialize with the keys omitted.
    let msg = base_message();
    assert!(msg.effective_agent_id.is_none());
    assert!(msg.effective_profile.is_none());
    assert!(!msg.profile_fallback_applied);
    assert!(msg.dispatch_not_applied.is_none());
    let value = serde_json::to_value(&msg).unwrap();
    let object = value.as_object().unwrap();
    for key in [
        "effectiveAgentId",
        "effectiveProfile",
        "profileFallbackApplied",
        "dispatchNotApplied",
    ] {
        assert!(
            !object.contains_key(key),
            "{key} must be omitted when absent"
        );
    }

    // Round-trip: populated annotations survive serialization/deserialization.
    let mut annotated = base_message();
    annotated.effective_agent_id = Some("codex".into());
    annotated.effective_profile = Some("B".into());
    annotated.profile_fallback_applied = true;
    annotated.dispatch_not_applied = Some("live-target".into());
    let json = serde_json::to_value(&annotated).unwrap();
    assert_eq!(json["effectiveAgentId"], "codex");
    assert_eq!(json["effectiveProfile"], "B");
    assert_eq!(json["profileFallbackApplied"], true);
    assert_eq!(json["dispatchNotApplied"], "live-target");
    let restored: OutboxMessage = serde_json::from_value(json).unwrap();
    assert_eq!(restored.effective_agent_id.as_deref(), Some("codex"));
    assert_eq!(restored.effective_profile.as_deref(), Some("B"));
    assert!(restored.profile_fallback_applied);
    assert_eq!(
        restored.dispatch_not_applied.as_deref(),
        Some("live-target")
    );

    // Old JSON without any of the five new keys still deserializes.
    let old = serde_json::json!({
        "id": "msg-1", "from": "from", "to": "to", "body": "",
        "timestamp": "2026-06-28T00:00:00Z"
    });
    let legacy: OutboxMessage = serde_json::from_value(old).unwrap();
    assert!(legacy.effective_agent_id.is_none());
    assert!(legacy.effective_profile.is_none());
    assert!(!legacy.profile_fallback_applied);
    assert!(legacy.dispatch_not_applied.is_none());
}

#[test]
fn old_outbox_defaults_privileged_payload_to_none() {
    let json = serde_json::json!({
        "id": "msg-1", "from": "from", "to": "to", "body": "",
        "timestamp": "2026-06-28T00:00:00Z"
    });
    let msg: OutboxMessage = serde_json::from_value(json).unwrap();
    assert!(msg.pty_input.is_none());
}

#[test]
fn privileged_debug_redacts_all_plaintext_and_tokens() {
    let sentinel = "PTY-PLAINTEXT-SENTINEL";
    let token = "TOKEN-SENTINEL";
    let mut msg = base_message();
    msg.token = Some(token.into());
    msg.body = sentinel.into();
    msg.pty_input = Some(PtyInputWirePayload {
        version: 1,
        text: sentinel.into(),
        enter: PtyInputEnterMode::AgentSubmit,
        injection_id: Uuid::new_v4().to_string(),
        op_id: Uuid::new_v4().to_string(),
        issued_at: "2026-07-19T00:00:00.000Z".into(),
        expires_at: "2026-07-19T00:10:00.000Z".into(),
        nonce: Uuid::new_v4().to_string(),
        agent_id: None,
    });
    let rendered = format!("{msg:?}");
    assert!(!rendered.contains(sentinel));
    assert!(!rendered.contains(token));
    assert!(rendered.contains("[REDACTED]"));

    let reason = PtyInputReason {
        code: PtyInputReasonCode::InvalidEnvelope,
        detail: sentinel.to_string(),
    };
    let reason_debug = format!("{reason:?}");
    assert!(!reason_debug.contains(sentinel));
    assert!(reason_debug.contains(safe_detail(PtyInputReasonCode::InvalidEnvelope)));
}

#[test]
fn canonical_timestamp_and_uuid_are_strict() {
    let now = DateTime::parse_from_rfc3339("2026-07-19T00:00:00.123Z")
        .unwrap()
        .with_timezone(&Utc);
    let canonical = canonical_pty_timestamp(now);
    assert_eq!(canonical, "2026-07-19T00:00:00.123Z");
    assert!(parse_canonical_pty_timestamp(&canonical).is_ok());
    assert!(parse_canonical_pty_timestamp("2026-07-19T00:00:00Z").is_err());
    let id = Uuid::new_v4().to_string();
    assert!(parse_canonical_uuid_v4(&id).is_ok());
    assert!(parse_canonical_uuid_v4(&id.to_uppercase()).is_err());
    assert!(parse_canonical_uuid_v4(&Uuid::nil().to_string()).is_err());
}

#[test]
fn every_reason_has_fixed_nonempty_detail() {
    let json = serde_json::to_value(PtyInputReasonCode::ArtifactUnclaimed).unwrap();
    assert_eq!(json, "artifact_unclaimed");
    assert!(!safe_detail(PtyInputReasonCode::ArtifactUnclaimed).is_empty());
}

#[test]
fn test_validate_enqueued_pty_input_result_allows_menu_guard_blocked_under_queued() {
    let now = Utc::now();
    let issued_at = canonical_pty_timestamp(now);
    let queued_at = canonical_pty_timestamp(now);
    let expires_at = canonical_pty_timestamp(now + chrono::Duration::seconds(PTY_INPUT_TTL_SECS));
    let id = Uuid::new_v4().to_string();
    let mut result = PtyInputResult::new(id.clone(), PtyInputPublicStatus::Queued);
    result.op_id = Some(id);
    result.sender = Some("sender".to_string());
    result.target = Some("target".to_string());
    result.payload_bytes = Some(100);
    result.payload_sha256 = Some("a".repeat(64));
    result.source_plane = Some(PtyInputSourcePlane::HostCli);
    result.issued_at = Some(issued_at);
    result.expires_at = Some(expires_at);
    result.queued_at = Some(queued_at);
    result.reason = Some(PtyInputReason::from_code(
        PtyInputReasonCode::MenuGuardBlocked,
    ));
    assert!(validate_enqueued_pty_input_result(&result).is_ok());
}
