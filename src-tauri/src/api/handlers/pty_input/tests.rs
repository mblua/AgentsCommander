use super::*;

#[tokio::test]
async fn e2992_p43_strict_revalidation() {
    use crate::config::settings::{
        new_settings_state, AppSettings, SettingsAvailability, SettingsOwner, SettingsPublisher,
    };

    let settings = new_settings_state(AppSettings {
        project_paths: vec!["ram-project".to_string()],
        ..AppSettings::default()
    });
    assert_eq!(
        current_pty_input_project_paths(&settings, async { Ok(None) })
            .await
            .unwrap(),
        vec!["ram-project"]
    );
    assert_eq!(
        current_pty_input_project_paths(&settings, async { Ok(Some(Vec::new())) })
            .await
            .unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        current_pty_input_project_paths(&settings, async {
            Ok(Some(vec!["physical-project".into()]))
        })
        .await
        .unwrap(),
        vec!["physical-project"]
    );
    for reason in ["unsafe_path", "settings_invalid", "settings_unavailable"] {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let error = current_pty_input_project_paths(&settings, async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(reason.to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            error.code(),
            "unsafe_path",
            "strict rejection must win over valid RAM"
        );
    }

    let instance = tempfile::tempdir().unwrap();
    let path = instance.path().join("settings.json");
    let ready = AppSettings {
        project_paths: vec!["ram-project".to_string()],
        ..AppSettings::default()
    };
    std::fs::write(&path, serde_json::to_vec(&ready).unwrap()).unwrap();
    let owner = SettingsOwner::open_active(&path, std::time::Duration::ZERO).unwrap();
    let report = owner
        .with_operation("p43-invalid", |_, guard| {
            guard.publish(SettingsPublisher::SettingsSave, |path| {
                std::fs::write(path, b"{").map_err(|error| error.to_string())
            })
        })
        .await
        .unwrap();
    assert_eq!(report.availability, SettingsAvailability::Invalid);
    let error = current_pty_input_project_paths(&owner, async {
        panic!("unavailable RAM must reject before polling the physical reader")
    })
    .await
    .unwrap_err();
    assert_eq!(error.code(), "service_unavailable");
}

#[test]
fn strict_request_rejects_caller_selected_authority_fields() {
    for field in [
        "from",
        "token",
        "root",
        "sourcePlane",
        "sessionId",
        "backend",
        "command",
    ] {
        let body = include_str!("tests/fixtures/authority-fields.json")
            .replace("{op_id}", &Uuid::new_v4().to_string())
            .replace("{field}", field);
        let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
        assert!(
            serde_json::from_value::<PtyInputRequest>(value).is_err(),
            "{field}"
        );
    }
}

#[test]
fn unsupported_enter_value_remains_distinguishable_from_a_malformed_envelope() {
    let body = include_str!("tests/fixtures/unsupported-enter.json")
        .replace("{op_id}", &Uuid::new_v4().to_string());
    let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
    let request: PtyInputRequest = serde_json::from_value(value).unwrap();
    assert_eq!(request.pty_input.enter, PtyInputEnterMode::Unsupported);
}

#[test]
fn duplicate_nested_key_is_rejected() {
    let body = include_str!("tests/fixtures/duplicate-key.json")
        .replace("{op_id}", &Uuid::new_v4().to_string());
    assert!(crate::path_identity::parse_json_no_duplicates(body.as_bytes()).is_err());
}

#[test]
fn content_headers_reject_duplicates_parameters_and_encodings() {
    let mut valid = HeaderMap::new();
    valid.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert!(validate_content_headers(&valid).is_ok());

    let mut utf8 = HeaderMap::new();
    utf8.insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    utf8.insert(header::CONTENT_ENCODING, "identity".parse().unwrap());
    assert!(validate_content_headers(&utf8).is_ok());

    for invalid in [
        "application/json; charset=latin1",
        "application/json; profile=x",
        "text/json",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, invalid.parse().unwrap());
        assert!(validate_content_headers(&headers).is_err(), "{invalid}");
    }

    let mut duplicate = HeaderMap::new();
    duplicate.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
    duplicate.append(header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert!(validate_content_headers(&duplicate).is_err());

    let mut encoded = HeaderMap::new();
    encoded.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    encoded.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
    assert!(validate_content_headers(&encoded).is_err());
}

#[test]
fn escaped_maximum_text_fits_raw_envelope_but_decoded_overage_is_rejected() {
    let op_id = Uuid::new_v4();
    let escaped = "\\u0078".repeat(crate::pty::backend::PTY_INPUT_MAX_BYTES);
    let body = include_str!("tests/fixtures/escaped-maximum.json")
        .replace("{op_id}", &op_id.to_string())
        .replace("{escaped}", &escaped);
    assert!(body.len() <= PTY_INPUT_HOST_ENVELOPE_MAX_BYTES);
    let value = crate::path_identity::parse_json_no_duplicates(body.as_bytes()).unwrap();
    let request: PtyInputRequest = serde_json::from_value(value).unwrap();
    assert_eq!(
        request.pty_input.text.len(),
        crate::pty::backend::PTY_INPUT_MAX_BYTES
    );
    assert!(crate::pty::inject::validate_pty_input_text(&request.pty_input.text).is_ok());

    let over = format!("{}x", request.pty_input.text);
    assert_eq!(over.len(), crate::pty::backend::PTY_INPUT_MAX_BYTES + 1);
    assert!(crate::pty::inject::validate_pty_input_text(&over).is_err());
}
