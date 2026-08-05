use std::error::Error as _;
use std::fmt::Debug;
use std::path::PathBuf;

use terminal_snapshot_renderer::{
    decode_api_error, ProtocolError, RenderError, RenderedTerminalPng, TerminalActiveBuffer,
    TerminalBackendKind, TerminalCell, TerminalCellStyle, TerminalCellWidth, TerminalColor,
    TerminalCursor, TerminalDimensions, TerminalLine, TerminalScreen, TerminalScreenModel,
    TerminalSnapshotApiSuccess, TerminalSnapshotDocument, TerminalSnapshotFidelity,
    TerminalSnapshotHostResponse, TerminalSnapshotPayload, TerminalSnapshotSession,
};

struct Canaries {
    cell_left: String,
    cell_right: String,
    osc: String,
    base64: String,
    png: String,
    auth: String,
    path: String,
    wire: String,
}

impl Canaries {
    fn load() -> Self {
        Self {
            cell_left: required("AC_SNAPSHOT_DIAG_CELL_LEFT"),
            cell_right: required("AC_SNAPSHOT_DIAG_CELL_RIGHT"),
            osc: required("AC_SNAPSHOT_DIAG_OSC"),
            base64: required("AC_SNAPSHOT_DIAG_BASE64"),
            png: required("AC_SNAPSHOT_DIAG_PNG"),
            auth: required("AC_SNAPSHOT_DIAG_AUTH"),
            path: required("AC_SNAPSHOT_DIAG_PATH"),
            wire: required("AC_SNAPSHOT_DIAG_WIRE"),
        }
    }
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing fixture environment variable {name}"))
}

fn model(canaries: &Canaries, text: String) -> TerminalScreenModel {
    let mut fidelity = TerminalSnapshotFidelity::version_one(false);
    fidelity.scope = canaries.wire.clone();
    fidelity.backend_parser = canaries.auth.clone();
    fidelity.parser_error_coverage = canaries.osc.clone();
    fidelity.omitted = vec![canaries.path.clone()];
    fidelity.unsupported = vec![canaries.base64.clone()];
    TerminalScreenModel {
        captured_at: canaries.wire.clone(),
        session: TerminalSnapshotSession {
            id: canaries.auth.clone(),
            backend: TerminalBackendKind::ContainerTransport,
        },
        screen: TerminalScreen {
            dimensions: TerminalDimensions {
                rows: 1,
                columns: 1,
            },
            sequence: 1_173,
            active_buffer: TerminalActiveBuffer::Alternate,
            cursor: TerminalCursor {
                row: 0,
                column: 0,
                visible: true,
                in_bounds: true,
            },
            parser_errors: 0,
            lines: vec![TerminalLine {
                wrapped: true,
                cells: vec![TerminalCell {
                    text,
                    width: TerminalCellWidth::Narrow,
                    foreground: TerminalColor::Rgb {
                        red: 17,
                        green: 73,
                        blue: 29,
                    },
                    background: TerminalColor::Indexed { index: 117 },
                    style: TerminalCellStyle {
                        bold: true,
                        italic: true,
                        underline: true,
                        inverse: true,
                    },
                }],
            }],
        },
        fidelity,
    }
}

fn document(canaries: &Canaries, text: String) -> TerminalSnapshotDocument {
    TerminalSnapshotDocument::from_model(
        canaries.auth.clone(),
        canaries.auth.clone(),
        canaries.path.clone(),
        &model(canaries, text),
    )
}

fn rendered(canaries: &Canaries, suffix: &str) -> RenderedTerminalPng {
    let mut bytes = canaries.png.as_bytes().to_vec();
    bytes.extend_from_slice(canaries.base64.as_bytes());
    bytes.extend_from_slice(suffix.as_bytes());
    RenderedTerminalPng {
        bytes,
        pixel_width: 1_173,
        pixel_height: 4_096,
        fallback_glyph_count: 2,
    }
}

fn payload(canaries: &Canaries) -> TerminalSnapshotPayload {
    let rendered = rendered(canaries, &canaries.osc);
    let metadata = rendered.metadata(
        canaries.auth.clone(),
        canaries.auth.clone(),
        canaries.path.clone(),
        &model(canaries, canaries.cell_left.clone()),
    );
    TerminalSnapshotPayload::Png {
        metadata,
        png: rendered.bytes,
    }
}

fn host_response(canaries: &Canaries) -> TerminalSnapshotHostResponse {
    TerminalSnapshotHostResponse {
        api_version: canaries.wire.clone(),
        request_id: canaries.auth.clone(),
        confirmation_tag: canaries.auth.clone(),
        expires_at: canaries.path.clone(),
        result: Some(payload(canaries)),
        error: None,
        detail: Some(canaries.wire.clone()),
    }
}

fn api_success(canaries: &Canaries) -> TerminalSnapshotApiSuccess {
    TerminalSnapshotApiSuccess {
        api_version: canaries.wire.clone(),
        result: payload(canaries),
    }
}

fn persist(case: &str, diagnostic: impl Debug) {
    let diagnostic = format!("case={case}\ndiagnostic={diagnostic:?}\n");
    println!("captured_diagnostic case={case} {diagnostic}");
    eprintln!(
        "captured_log case={case} diagnostic_bytes={}",
        diagnostic.len()
    );
    let directory = PathBuf::from(required("AC_SNAPSHOT_DIAG_PERSIST_DIR"));
    std::fs::create_dir_all(&directory).expect("create persisted fixture diagnostic directory");
    std::fs::write(directory.join(format!("{case}.diagnostic")), diagnostic)
        .expect("write persisted fixture diagnostic");
}

fn wire_failure(canaries: &Canaries) -> Result<(), Box<TerminalSnapshotHostResponse>> {
    Err(Box::new(host_response(canaries)))
}

fn render_failure() -> Result<(), RenderError> {
    Err(RenderError::Invariant)
}

#[test]
fn assert_eq_model_failure() {
    let canaries = Canaries::load();
    let left = model(&canaries, canaries.cell_left.clone());
    let right = model(&canaries, canaries.cell_right.clone());
    persist(
        "assert_eq_model_failure",
        (
            &left,
            &right,
            &left.screen.lines[0].cells[0],
            &right.screen.lines[0].cells[0],
        ),
    );
    assert_eq!(left, right);
}

#[test]
fn assert_ne_document_failure() {
    let canaries = Canaries::load();
    let left = document(&canaries, canaries.osc.clone());
    let right = document(&canaries, canaries.osc.clone());
    persist("assert_ne_document_failure", (&left, &right));
    assert_ne!(left, right);
}

#[test]
fn expect_render_error_failure() {
    let canaries = Canaries::load();
    let rendered = rendered(&canaries, &canaries.cell_right);
    let error = RenderError::Invariant;
    persist("expect_render_error_failure", (&rendered, error));
    render_failure().expect("fixed renderer expectation");
}

#[test]
fn panic_payload_failure() {
    let canaries = Canaries::load();
    let payload = payload(&canaries);
    persist("panic_payload_failure", &payload);
    panic!("snapshot payload diagnostic: {payload:?}");
}

#[test]
fn result_returning_protocol_error_failure() -> Result<(), ProtocolError> {
    let canaries = Canaries::load();
    let malformed = format!(
        r#"{{"apiVersion":"{}","error":"invalid_request","detail":"{}"}}"#,
        canaries.auth, canaries.wire
    );
    let error = match decode_api_error(malformed.as_bytes(), 400) {
        Ok(_) => ProtocolError::Serialization,
        Err(error) => error,
    };
    persist(
        "result_returning_protocol_error_failure",
        format_args!(
            "error={error:?} display={error} source={}",
            if error.source().is_some() {
                "present"
            } else {
                "none"
            }
        ),
    );
    Err(error)
}

#[test]
fn assert_eq_rendered_failure() {
    let canaries = Canaries::load();
    let left = rendered(&canaries, &canaries.cell_left);
    let right = rendered(&canaries, &canaries.cell_right);
    persist("assert_eq_rendered_failure", (&left, &right));
    assert_eq!(left, right);
}

/// The one case that is not a plain in-test failure: the payload is Debug-formatted into a
/// panic raised on a **non-test thread**, so it is printed by the process panic hook rather
/// than by libtest's own handler, and the join failure then produces a second diagnostic.
/// Both are surfaces production can reach, since snapshot work runs on spawned tasks.
///
/// This used `tokio::spawn` until the fixture's dependency on Tokio proved to be what CI could
/// not satisfy. `std::thread` reaches the same panic hook by the same route with no dependency
/// at all. What it no longer covers is `tokio::task::JoinError`'s own formatting, which is the
/// one narrow surface this construction gives up.
#[test]
fn spawned_thread_wire_failure() {
    let canaries = Canaries::load();
    let success = api_success(&canaries);
    persist("spawned_thread_wire_failure", &success);
    let task = std::thread::spawn(move || {
        panic!("spawned task snapshot result: {success:?}");
    });
    task.join().expect("fixed spawned snapshot task expectation");
}

#[test]
fn unwrap_wire_failure() {
    let canaries = Canaries::load();
    let wire = host_response(&canaries);
    persist("unwrap_wire_failure", &wire);
    wire_failure(&canaries).unwrap();
}
