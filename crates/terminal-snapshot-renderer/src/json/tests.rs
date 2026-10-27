use super::*;
use serde_json::json;

#[test]
fn formatter_is_compact_ascii_and_uses_long_control_escapes() {
    let value = json!({"text": "a\n\u{7f}é😀\\\""});
    let bytes = to_ascii_json(&value, 1024).unwrap();
    assert!(bytes.is_ascii());
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        r#"{"text":"a\u000a\u007f\u00e9\ud83d\ude00\\\""}"#
    );
}

#[test]
fn scanner_rejects_duplicate_keys_at_any_depth() {
    assert!(decode_bounded::<serde_json::Value>(br#"{"a":{"x":1,"x":2}}"#, 1024).is_err());
}

#[test]
fn scanner_rejects_escaped_base64_field() {
    let input = br#"{"pngBase64":"YWJj\u003d"}"#;
    assert!(decode_bounded::<serde_json::Value>(input, 1024).is_err());
}

#[test]
fn canonical_base64_round_trips_without_a_second_reencoding() {
    let decoded = decode_canonical_base64_png("cG5n").unwrap();
    assert_eq!(decoded, b"png");
    assert_eq!(decoded.capacity(), decoded.len());
    assert!(decode_canonical_base64_png("cG5n===").is_err());
    assert!(decode_canonical_base64_png("cG5=").is_err());
}

fn fixture_model() -> crate::protocol::TerminalScreenModel {
    decode_bounded(
        include_bytes!("../../tests/fixtures/blank-cursor-model.json"),
        crate::protocol::MAX_JSON_BYTES,
    )
    .unwrap()
}

#[test]
fn borrowed_model_encoding_is_byte_identical_without_a_screen_clone() {
    let request_id = "00000000-0000-4000-8000-000000000116";
    let requester = "project:wg-1-team/coordinator";
    let target = "project:wg-1-team/member";
    let confirmation_tag = "11".repeat(32);
    let expires_at = "2026-07-31T03:31:00.123Z";
    let model = fixture_model();
    let owned = TerminalSnapshotPayload::Json {
        snapshot: TerminalSnapshotDocument::from_model(
            request_id.to_string(),
            requester.to_string(),
            target.to_string(),
            &model,
        ),
    };
    assert_eq!(
        encode_api_json_success_from_model(request_id, requester, target, &model).unwrap(),
        encode_api_success_payload(&owned).unwrap()
    );
    assert_eq!(
        encode_host_json_success_from_model(
            request_id,
            &confirmation_tag,
            expires_at,
            requester,
            target,
            &model,
        )
        .unwrap(),
        encode_host_success_payload(request_id, &confirmation_tag, expires_at, &owned).unwrap()
    );
    assert_eq!(
        terminal_snapshot_json_bytes_from_model(request_id, requester, target, &model).unwrap(),
        terminal_snapshot_payload_bytes(&owned).unwrap()
    );
}

#[test]
fn borrowed_wire_debug_is_structural_and_non_disclosing() {
    const CELL_CANARY: &str = "CELL_1173_J6M3";
    const BASE64_CANARY: &str = "BASE64_1173_J6M3";
    const AUTH_CANARY: &str = "AUTH_1173_J6M3";
    const PATH_CANARY: &str = r"C:\PATH_1173_J6M3\snapshot.png";
    const TRANSPORT_CANARY: &str = "TRANSPORT_1173_J6M3";

    let mut model = fixture_model();
    model.captured_at = TRANSPORT_CANARY.to_string();
    model.session.id = AUTH_CANARY.to_string();
    model.screen.lines[0].cells[0].text = CELL_CANARY.to_string();
    model.fidelity.scope = TRANSPORT_CANARY.to_string();
    let snapshot = TerminalSnapshotDocument {
        schema_version: SCHEMA_VERSION,
        request_id: AUTH_CANARY.to_string(),
        captured_at: TRANSPORT_CANARY.to_string(),
        requester: AUTH_CANARY.to_string(),
        target: PATH_CANARY.to_string(),
        session: model.session.clone(),
        screen: model.screen.clone(),
        fidelity: model.fidelity.clone(),
    };
    let metadata = TerminalSnapshotPngMetadata {
        schema_version: SCHEMA_VERSION,
        request_id: AUTH_CANARY.to_string(),
        captured_at: TRANSPORT_CANARY.to_string(),
        requester: AUTH_CANARY.to_string(),
        target: PATH_CANARY.to_string(),
        session: model.session.clone(),
        screen: crate::protocol::TerminalPngScreenMetadata {
            dimensions: model.screen.dimensions,
            sequence: model.screen.sequence,
            active_buffer: model.screen.active_buffer,
            cursor: model.screen.cursor,
            parser_errors: model.screen.parser_errors,
        },
        fidelity: model.fidelity.clone(),
        format: TerminalSnapshotFormat::Png,
        png: crate::protocol::TerminalPngInfo {
            bytes: 1173,
            pixel_width: 36,
            pixel_height: 36,
        },
        renderer: crate::protocol::TerminalRendererMetadata::version_one(2),
    };

    let json_result = BorrowedSnapshotResult::Json {
        snapshot: snapshot.clone(),
    };
    let png_result = BorrowedSnapshotResult::Png {
        metadata,
        png_base64: BASE64_CANARY,
    };
    let borrowed_document = BorrowedTerminalSnapshotDocument {
        schema_version: SCHEMA_VERSION,
        request_id: AUTH_CANARY,
        captured_at: TRANSPORT_CANARY,
        requester: AUTH_CANARY,
        target: PATH_CANARY,
        session: &model.session,
        screen: &model.screen,
        fidelity: &model.fidelity,
    };
    let diagnostics = [
        format!("{json_result:?}"),
        format!("{png_result:?}"),
        format!(
            "{:?}",
            BorrowedApiSuccess {
                api_version: TRANSPORT_CANARY.to_string(),
                result: BorrowedSnapshotResult::Json { snapshot },
            }
        ),
        format!(
            "{:?}",
            BorrowedHostResponse {
                api_version: TRANSPORT_CANARY.to_string(),
                request_id: AUTH_CANARY.to_string(),
                confirmation_tag: AUTH_CANARY.to_string(),
                expires_at: TRANSPORT_CANARY.to_string(),
                result: None,
                error: Some(TerminalSnapshotReasonCode::ResponseUnavailable),
                detail: Some(TRANSPORT_CANARY.to_string()),
            }
        ),
        format!("{borrowed_document:?}"),
    ]
    .join("\n");

    for forbidden in [
        CELL_CANARY,
        BASE64_CANARY,
        AUTH_CANARY,
        PATH_CANARY,
        TRANSPORT_CANARY,
    ] {
        assert!(!diagnostics.contains(forbidden));
    }
    for structural in [
        "BorrowedSnapshotResult::Json",
        "BorrowedSnapshotResult::Png",
        "has_detail: true",
        "cells: 2",
    ] {
        assert!(diagnostics.contains(structural));
    }
    assert!(diagnostics.contains(&format!("base64_bytes: {}", BASE64_CANARY.len())));
}

fn assert_safe_protocol_error(error: ProtocolError, expected: ProtocolError, forbidden: &[&str]) {
    use std::error::Error as _;

    assert_eq!(error, expected);
    let diagnostic = format!("{error} {error:?}");
    for canary in forbidden {
        assert!(!diagnostic.contains(canary));
    }
    assert!(error.source().is_none());
}

#[test]
fn malformed_cross_format_over_cap_base64_png_and_parser_errors_collapse_safely() {
    const JSON_CANARY: &str = "JSON_1173_E5V7";
    const BASE64_CANARY: &str = "BASE64_1173_E5V7";
    const PNG_CANARY: &str = "PNG_1173_E5V7";
    const FORMAT_CANARY: &str = "FORMAT_1173_E5V7";
    let forbidden = [JSON_CANARY, BASE64_CANARY, PNG_CANARY, FORMAT_CANARY];

    let malformed = format!(
        include_str!("tests/malformed-request.json"),
        JSON_CANARY = JSON_CANARY,
        FORMAT_CANARY = FORMAT_CANARY,
    );
    assert_safe_protocol_error(
        decode_bounded::<crate::protocol::TerminalSnapshotApiRequest>(
            malformed.as_bytes(),
            crate::protocol::MAX_REQUEST_BYTES,
        )
        .unwrap_err(),
        ProtocolError::Invalid,
        &forbidden,
    );
    assert_safe_protocol_error(
        decode_bounded::<serde_json::Value>(
            &vec![b'x'; crate::protocol::MAX_REQUEST_BYTES + 1],
            crate::protocol::MAX_REQUEST_BYTES,
        )
        .unwrap_err(),
        ProtocolError::TooLarge,
        &forbidden,
    );
    assert_safe_protocol_error(
        decode_canonical_base64_png(BASE64_CANARY).unwrap_err(),
        ProtocolError::Invalid,
        &forbidden,
    );
    assert_safe_protocol_error(
        crate::png_validation::validate_generated_png(PNG_CANARY.as_bytes(), 1, 1).unwrap_err(),
        ProtocolError::InvalidPng,
        &forbidden,
    );
    assert_safe_protocol_error(
        FORMAT_CANARY.parse::<TerminalSnapshotFormat>().unwrap_err(),
        ProtocolError::Invalid,
        &forbidden,
    );

    let request_id = "00000000-0000-4000-8000-000000000117";
    let payload = TerminalSnapshotPayload::Json {
        snapshot: TerminalSnapshotDocument::from_model(
            request_id.to_string(),
            "project:wg-1-team/coordinator".to_string(),
            "project:wg-1-team/member".to_string(),
            &fixture_model(),
        ),
    };
    let mut bytes = encode_api_success_payload(&payload).unwrap();
    let insertion = bytes.len() - 2;
    bytes.splice(
        insertion..insertion,
        format!(",\"pngBase64\":\"{BASE64_CANARY}\"").bytes(),
    );
    assert_safe_protocol_error(
        decode_api_success(
            &bytes,
            request_id,
            "project:wg-1-team/member",
            TerminalSnapshotFormat::Json,
        )
        .unwrap_err(),
        ProtocolError::Invalid,
        &forbidden,
    );
}

#[test]
fn tagged_result_rejects_cross_format_and_unknown_fields() {
    let request_id = "00000000-0000-4000-8000-000000000117";
    let payload = TerminalSnapshotPayload::Json {
        snapshot: TerminalSnapshotDocument::from_model(
            request_id.to_string(),
            "project:wg-1-team/coordinator".to_string(),
            "project:wg-1-team/member".to_string(),
            &fixture_model(),
        ),
    };
    let mut bytes = encode_api_success_payload(&payload).unwrap();
    let insertion = bytes.len() - 2;
    bytes.splice(
        insertion..insertion,
        b",\"pngBase64\":\"cG5n\"".iter().copied(),
    );
    assert!(decode_api_success(
        &bytes,
        request_id,
        "project:wg-1-team/member",
        TerminalSnapshotFormat::Json,
    )
    .is_err());
}

#[cfg(feature = "render")]
#[test]
fn png_wire_encoding_streams_base64_and_decodes_to_raw_bytes() {
    let request_id = "00000000-0000-4000-8000-000000000118";
    let model = fixture_model();
    let rendered = crate::render::render_png(&model).unwrap();
    let expected = rendered.bytes.clone();
    let payload = TerminalSnapshotPayload::Png {
        metadata: rendered.metadata(
            request_id.to_string(),
            "project:wg-1-team/coordinator".to_string(),
            "project:wg-1-team/member".to_string(),
            &model,
        ),
        png: rendered.bytes,
    };
    let bytes = encode_api_success_payload(&payload).unwrap();
    let decoded = decode_api_success(
        &bytes,
        request_id,
        "project:wg-1-team/member",
        TerminalSnapshotFormat::Png,
    )
    .unwrap();
    match decoded.result {
        TerminalSnapshotPayload::Png { png, .. } => assert_eq!(png, expected),
        TerminalSnapshotPayload::Json { .. } => panic!("wrong decoded format"),
    }
}
