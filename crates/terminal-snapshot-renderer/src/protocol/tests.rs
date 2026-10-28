use super::*;

fn cell() -> TerminalCell {
    TerminalCell {
        text: String::new(),
        width: TerminalCellWidth::Narrow,
        foreground: TerminalColor::Default,
        background: TerminalColor::Default,
        style: TerminalCellStyle::default(),
    }
}

#[test]
fn exact_dimension_boundaries_are_checked() {
    assert!(TerminalDimensions {
        rows: 100,
        columns: 400,
    }
    .checked_image_dimensions()
    .is_ok());
    assert!(TerminalDimensions {
        rows: 101,
        columns: 400,
    }
    .checked_image_dimensions()
    .is_err());
    assert!(TerminalDimensions {
        rows: 0,
        columns: 1,
    }
    .checked_image_dimensions()
    .is_err());
}

#[test]
fn wide_pair_invariants_are_closed() {
    let mut screen = TerminalScreen {
        dimensions: TerminalDimensions {
            rows: 1,
            columns: 2,
        },
        sequence: 0,
        active_buffer: TerminalActiveBuffer::Normal,
        cursor: TerminalCursor {
            row: 0,
            column: 0,
            visible: true,
            in_bounds: true,
        },
        parser_errors: 0,
        lines: vec![TerminalLine {
            wrapped: false,
            cells: vec![cell(), cell()],
        }],
    };
    assert!(validate_screen(&screen).is_ok());
    screen.lines[0].cells[0].width = TerminalCellWidth::WideLead;
    assert!(validate_screen(&screen).is_err());
    screen.lines[0].cells[1].width = TerminalCellWidth::WideContinuation;
    assert!(validate_screen(&screen).is_ok());
    screen.lines[0].cells[1].text = "x".to_string();
    assert!(validate_screen(&screen).is_err());
}

fn valid_model() -> TerminalScreenModel {
    TerminalScreenModel {
        captured_at: "2026-09-21T14:00:45.123Z".to_string(),
        session: TerminalSnapshotSession {
            id: "00000000-0000-4000-8000-000000000230".to_string(),
            backend: TerminalBackendKind::ContainerTransport,
        },
        screen: TerminalScreen {
            dimensions: TerminalDimensions {
                rows: 1,
                columns: 1,
            },
            sequence: 0,
            active_buffer: TerminalActiveBuffer::Normal,
            cursor: TerminalCursor {
                row: 0,
                column: 0,
                visible: true,
                in_bounds: true,
            },
            parser_errors: 0,
            lines: vec![TerminalLine {
                wrapped: false,
                cells: vec![cell()],
            }],
        },
        fidelity: TerminalSnapshotFidelity::version_one(false),
    }
}

#[test]
fn room_and_legacy_fqns_validate_in_target_requester_and_json() {
    const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000231";
    const ROOM_REQUESTER: &str = "AgentsCommander_iac:room-25-ac-dev-team-v4/ac-tech-lead-v4";
    const ROOM_TARGET: &str = "AgentsCommander_iac:room-25-ac-dev-team-v4/ac-dev-rust-v4";
    const LEGACY_REQUESTER: &str = "project:wg-1-team/coordinator";
    const LEGACY_TARGET: &str = "project:wg-1-team/member";

    for fqn in [ROOM_REQUESTER, ROOM_TARGET, LEGACY_REQUESTER, LEGACY_TARGET] {
        assert!(validate_wg_fqn(fqn).is_ok(), "must accept {fqn:?}");
        assert!(
            validate_requester_identity(fqn, false).is_ok(),
            "must accept {fqn:?}"
        );
        assert!(validate_target_syntax(fqn).is_ok(), "must accept {fqn:?}");
    }

    let model = valid_model();
    let document = TerminalSnapshotDocument::from_model(
        REQUEST_ID.to_string(),
        ROOM_REQUESTER.to_string(),
        ROOM_TARGET.to_string(),
        &model,
    );
    assert!(document.validate().is_ok());

    let metadata = TerminalSnapshotPngMetadata {
        schema_version: SCHEMA_VERSION,
        request_id: REQUEST_ID.to_string(),
        captured_at: model.captured_at.clone(),
        requester: ROOM_REQUESTER.to_string(),
        target: ROOM_TARGET.to_string(),
        session: model.session.clone(),
        screen: TerminalPngScreenMetadata {
            dimensions: model.screen.dimensions,
            sequence: model.screen.sequence,
            active_buffer: model.screen.active_buffer,
            cursor: model.screen.cursor,
            parser_errors: model.screen.parser_errors,
        },
        fidelity: model.fidelity.clone(),
        format: TerminalSnapshotFormat::Png,
        png: TerminalPngInfo {
            bytes: 17,
            pixel_width: 26,
            pixel_height: 36,
        },
        renderer: TerminalRendererMetadata::version_one(0),
    };
    assert!(metadata.validate().is_ok());

    let request = TerminalSnapshotApiRequest {
        api_version: API_VERSION.to_string(),
        request_id: REQUEST_ID.to_string(),
        to: ROOM_TARGET.to_string(),
        format: TerminalSnapshotFormat::Json,
    };
    assert!(request.validate().is_ok());

    let bytes = crate::to_ascii_json(&document, MAX_JSON_BYTES).expect("encode document");
    let decoded = crate::decode_bounded::<TerminalSnapshotDocument>(&bytes, MAX_JSON_BYTES)
        .expect("decode room document");
    assert_eq!(decoded.requester, ROOM_REQUESTER);
    assert_eq!(decoded.target, ROOM_TARGET);
    assert!(decoded.validate().is_ok());

    let legacy = TerminalSnapshotDocument::from_model(
        REQUEST_ID.to_string(),
        LEGACY_REQUESTER.to_string(),
        LEGACY_TARGET.to_string(),
        &model,
    );
    let legacy_bytes =
        crate::to_ascii_json(&legacy, MAX_JSON_BYTES).expect("encode legacy document");
    let legacy_decoded =
        crate::decode_bounded::<TerminalSnapshotDocument>(&legacy_bytes, MAX_JSON_BYTES)
            .expect("decode legacy document");
    assert!(legacy_decoded.validate().is_ok());
}

#[test]
fn malformed_room_fqns_stay_rejected() {
    for invalid in [
        "",
        "room-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:rom-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:rooms-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:Room-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:roomx-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:room-ac-dev-team-v4/ac-dev-rust-v4",
        "project:room--ac-dev-team-v4/ac-dev-rust-v4",
        "project:room-2a-ac-dev-team-v4/ac-dev-rust-v4",
        "project:room-٢٥-ac-dev-team-v4/ac-dev-rust-v4",
        "project:room-25-/ac-dev-rust-v4",
        "project:room-25-ac-dev-team-v4/",
        "project:room-25-ac-dev-team-v4/ac-dev/rust-v4",
        "project:room-25-ac-dev-team-v4",
        "project:room-25-ac-dev-team-v4:ac-dev-rust-v4",
        "project:room-25-ac-dev_team-v4/ac-dev-rust-v4",
        "project:room-25-ac-dev-team-v4/ac-dev.rust-v4",
        "pro/ject:room-25-ac-dev-team-v4/ac-dev-rust-v4",
        "project:wg--team/member",
        "project:wg-1-team/member:extra",
    ] {
        assert!(validate_wg_fqn(invalid).is_err(), "must reject {invalid:?}");
    }
    let oversized = format!("project:room-1-{}/agent", "t".repeat(1_024));
    assert!(validate_wg_fqn(&oversized).is_err());
}

fn canary_model() -> TerminalScreenModel {
    let cell_canary = "CELL_1173_D9Q2";
    let osc_canary = "\u{1b}]52;c;OSC_1173_D9Q2\u{7}";
    let auth_canary = "AUTH_1173_D9Q2";
    TerminalScreenModel {
        captured_at: auth_canary.to_string(),
        session: TerminalSnapshotSession {
            id: auth_canary.to_string(),
            backend: TerminalBackendKind::ContainerTransport,
        },
        screen: TerminalScreen {
            dimensions: TerminalDimensions {
                rows: 1,
                columns: 1,
            },
            sequence: 1173,
            active_buffer: TerminalActiveBuffer::Alternate,
            cursor: TerminalCursor {
                row: 0,
                column: 0,
                visible: true,
                in_bounds: true,
            },
            parser_errors: 2,
            lines: vec![TerminalLine {
                wrapped: true,
                cells: vec![TerminalCell {
                    text: format!("{cell_canary}{osc_canary}"),
                    width: TerminalCellWidth::Narrow,
                    foreground: TerminalColor::Indexed { index: 173 },
                    background: TerminalColor::Rgb {
                        red: 17,
                        green: 73,
                        blue: 92,
                    },
                    style: TerminalCellStyle {
                        bold: true,
                        italic: true,
                        underline: false,
                        inverse: false,
                    },
                }],
            }],
        },
        fidelity: TerminalSnapshotFidelity {
            scope: auth_canary.to_string(),
            backend_parser: auth_canary.to_string(),
            backend_scrollback_rows: 0,
            atomic_at_output_sequence: true,
            application_frame_atomic: false,
            all_active_viewport_cells_included: true,
            includes_frontend_state: false,
            exact_frontend_pixels: false,
            parser_had_errors: true,
            parser_error_coverage: auth_canary.to_string(),
            omitted: vec![auth_canary.to_string()],
            unsupported: vec![auth_canary.to_string()],
        },
    }
}

fn canary_png_metadata(model: &TerminalScreenModel) -> TerminalSnapshotPngMetadata {
    let auth_canary = "AUTH_1173_D9Q2";
    let mut renderer = TerminalRendererMetadata::version_one(3);
    renderer.id = auth_canary.to_string();
    renderer.font.family = auth_canary.to_string();
    renderer.font.version = auth_canary.to_string();
    renderer.font.sha256 = auth_canary.to_string();
    renderer.palette_id = auth_canary.to_string();
    renderer.cursor_policy = auth_canary.to_string();
    TerminalSnapshotPngMetadata {
        schema_version: SCHEMA_VERSION,
        request_id: auth_canary.to_string(),
        captured_at: auth_canary.to_string(),
        requester: auth_canary.to_string(),
        target: auth_canary.to_string(),
        session: model.session.clone(),
        screen: TerminalPngScreenMetadata {
            dimensions: model.screen.dimensions,
            sequence: model.screen.sequence,
            active_buffer: model.screen.active_buffer,
            cursor: model.screen.cursor,
            parser_errors: model.screen.parser_errors,
        },
        fidelity: model.fidelity.clone(),
        format: TerminalSnapshotFormat::Png,
        png: TerminalPngInfo {
            bytes: 17,
            pixel_width: 26,
            pixel_height: 36,
        },
        renderer,
    }
}

#[test]
fn terminal_payload_model_render_and_wire_debug_is_structural_only() {
    const CELL_CANARY: &str = "CELL_1173_D9Q2";
    const OSC_CANARY: &str = "\u{1b}]52;c;OSC_1173_D9Q2\u{7}";
    const PNG_CANARY: &str = "PNG_BYTES_1173_D9Q2";
    const AUTH_CANARY: &str = "AUTH_1173_D9Q2";
    const PATH_CANARY: &str = r"C:\PATH_1173_D9Q2\snapshot.png";
    const TRANSPORT_CANARY: &str = "TRANSPORT_1173_D9Q2";

    let model = canary_model();
    let document = TerminalSnapshotDocument {
        schema_version: SCHEMA_VERSION,
        request_id: AUTH_CANARY.to_string(),
        captured_at: AUTH_CANARY.to_string(),
        requester: AUTH_CANARY.to_string(),
        target: PATH_CANARY.to_string(),
        session: model.session.clone(),
        screen: model.screen.clone(),
        fidelity: model.fidelity.clone(),
    };
    let metadata = canary_png_metadata(&model);
    let api_error = TerminalSnapshotApiError {
        api_version: TRANSPORT_CANARY.to_string(),
        error: TerminalSnapshotReasonCode::InvalidRequest,
        detail: TRANSPORT_CANARY.to_string(),
    };
    let api_request = TerminalSnapshotApiRequest {
        api_version: TRANSPORT_CANARY.to_string(),
        request_id: AUTH_CANARY.to_string(),
        to: PATH_CANARY.to_string(),
        format: TerminalSnapshotFormat::Png,
    };

    let mut diagnostics = vec![
        format!("{:?}", TerminalSnapshotFormat::Png),
        format!("{:?}", TerminalSnapshotReasonCode::InvalidRequest),
        format!("{:?}", ProtocolError::Invalid),
        format!("{:?}", TerminalBackendKind::ContainerTransport),
        format!("{:?}", TerminalActiveBuffer::Alternate),
        format!("{:?}", TerminalCellWidth::Narrow),
        format!("{:?}", model.screen.lines[0].cells[0].foreground),
        format!("{:?}", model.screen.lines[0].cells[0].style),
        format!("{:?}", model.screen.lines[0].cells[0]),
        format!("{:?}", model.screen.lines[0]),
        format!("{:?}", model.screen.dimensions),
        format!(
            "{:?}",
            model.screen.dimensions.checked_image_dimensions().unwrap()
        ),
        format!("{:?}", model.screen.cursor),
        format!("{:?}", model.screen),
        format!("{:?}", model.session),
        format!("{:?}", model.fidelity),
        format!("{model:?}"),
        format!("{document:?}"),
        format!("{:?}", metadata.screen),
        format!("{:?}", metadata.png),
        format!("{:?}", metadata.renderer.font),
        format!("{:?}", metadata.renderer.cell),
        format!("{:?}", metadata.renderer),
        format!("{metadata:?}"),
        format!("{api_error:?}"),
        format!("{api_request:?}"),
    ];
    diagnostics.push(format!(
        "{:?}",
        TerminalSnapshotPayload::Json {
            snapshot: document.clone(),
        }
    ));
    diagnostics.push(format!(
        "{:?}",
        TerminalSnapshotPayload::Png {
            metadata: metadata.clone(),
            png: PNG_CANARY.as_bytes().to_vec(),
        }
    ));
    diagnostics.push(format!(
        "{:?}",
        TerminalSnapshotApiSuccess {
            api_version: TRANSPORT_CANARY.to_string(),
            result: TerminalSnapshotPayload::Json {
                snapshot: document.clone(),
            },
        }
    ));
    diagnostics.push(format!(
        "{:?}",
        TerminalSnapshotHostResponse {
            api_version: TRANSPORT_CANARY.to_string(),
            request_id: AUTH_CANARY.to_string(),
            confirmation_tag: AUTH_CANARY.to_string(),
            expires_at: AUTH_CANARY.to_string(),
            result: Some(TerminalSnapshotPayload::Png {
                metadata,
                png: PNG_CANARY.as_bytes().to_vec(),
            }),
            error: None,
            detail: Some(TRANSPORT_CANARY.to_string()),
        }
    ));

    let diagnostics = diagnostics.join("\n");
    for forbidden in [
        CELL_CANARY,
        OSC_CANARY,
        PNG_CANARY,
        AUTH_CANARY,
        PATH_CANARY,
        TRANSPORT_CANARY,
    ] {
        assert!(!diagnostics.contains(forbidden));
    }
    for structural in [
        "text_bytes",
        "cells: 1",
        "rows: 1",
        "decoded_bytes",
        "InvalidRequest",
        "format: Png",
    ] {
        assert!(diagnostics.contains(structural));
    }
}

#[test]
fn protocol_display_debug_and_error_sources_are_fixed_and_source_free() {
    use std::error::Error as _;

    for (error, display, debug) in [
        (
            ProtocolError::Invalid,
            "invalid terminal snapshot protocol data",
            "Invalid",
        ),
        (
            ProtocolError::TooLarge,
            "terminal snapshot protocol limit exceeded",
            "TooLarge",
        ),
        (
            ProtocolError::InvalidPng,
            "invalid terminal snapshot PNG",
            "InvalidPng",
        ),
        (
            ProtocolError::Serialization,
            "terminal snapshot serialization failed",
            "Serialization",
        ),
    ] {
        assert_eq!(error.to_string(), display);
        assert_eq!(format!("{error:?}"), debug);
        assert!(error.source().is_none());
    }
    assert_eq!(TerminalSnapshotFormat::Json.to_string(), "json");
    assert_eq!(
        TerminalSnapshotReasonCode::InvalidRequest.to_string(),
        "invalid_request"
    );
}

#[test]
fn reason_contract_is_fixed() {
    for code in [
        TerminalSnapshotReasonCode::InvalidRequest,
        TerminalSnapshotReasonCode::RequesterUnavailable,
        TerminalSnapshotReasonCode::TerminalSnapshotsDisabled,
        TerminalSnapshotReasonCode::NotAuthorized,
        TerminalSnapshotReasonCode::TargetUnavailable,
        TerminalSnapshotReasonCode::SnapshotUnavailable,
        TerminalSnapshotReasonCode::SnapshotTooLarge,
        TerminalSnapshotReasonCode::AuthorityChanged,
        TerminalSnapshotReasonCode::RateLimited,
        TerminalSnapshotReasonCode::SnapshotTimeout,
        TerminalSnapshotReasonCode::ServiceUnavailable,
        TerminalSnapshotReasonCode::RenderFailed,
        TerminalSnapshotReasonCode::UnsafePath,
        TerminalSnapshotReasonCode::OutputFailed,
        TerminalSnapshotReasonCode::ResponseUnavailable,
        TerminalSnapshotReasonCode::Internal,
    ] {
        assert!(!code.as_str().is_empty());
        assert!(code.detail().ends_with('.'));
    }
}
