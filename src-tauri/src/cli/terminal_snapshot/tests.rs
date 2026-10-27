use super::*;

const ALL_FIELDS: [TerminalSnapshotField; 6] = [
    TerminalSnapshotField::Token,
    TerminalSnapshotField::Root,
    TerminalSnapshotField::To,
    TerminalSnapshotField::Format,
    TerminalSnapshotField::Output,
    TerminalSnapshotField::Timeout,
];

const ALL_REASONS: [TerminalSnapshotRejectionReason; 14] = [
    TerminalSnapshotRejectionReason::TimeoutOutOfRange,
    TerminalSnapshotRejectionReason::TokenNotUuid,
    TerminalSnapshotRejectionReason::TokenIsPersistedStatic,
    TerminalSnapshotRejectionReason::TargetSyntaxInvalid,
    TerminalSnapshotRejectionReason::OutputRequiredForPng,
    TerminalSnapshotRejectionReason::OutputForbiddenForJson,
    TerminalSnapshotRejectionReason::OutputPathRejected,
    TerminalSnapshotRejectionReason::RequesterRootUnverified,
    TerminalSnapshotRejectionReason::RequesterNotCoordinator,
    TerminalSnapshotRejectionReason::RequesterRootObjectMismatch,
    TerminalSnapshotRejectionReason::LocalStateUnsafe,
    TerminalSnapshotRejectionReason::RequestIdCollision,
    TerminalSnapshotRejectionReason::RequestEncodingFailed,
    TerminalSnapshotRejectionReason::Unattributed,
];

/// `crates/terminal-snapshot-renderer/src/protocol.rs` declares exactly
/// these sixteen. The array is exhaustive by construction: a new variant
/// there is not covered here until it is added.
const ALL_CODES: [TerminalSnapshotReasonCode; 16] = [
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
];

fn snapshot_args(
    token: &str,
    to: &str,
    format: SnapshotFormatArg,
    output: Option<PathBuf>,
    timeout: u64,
) -> TerminalSnapshotArgs {
    TerminalSnapshotArgs {
        token: token.to_string(),
        root: PathBuf::from("."),
        to: to.to_string(),
        format,
        output,
        timeout,
    }
}

const VALID_TOKEN: &str = "0b9c4d2e-7a31-4f56-8b2c-1d3e5f7a9b10";

#[test]
fn rejection_line_names_each_invalid_request_cause() {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    let detail = C::InvalidRequest.detail();
    let cases = [
        (
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Timeout),
                R::TimeoutOutOfRange,
            ),
            format!(
                "terminal_snapshot_error code=invalid_request field=timeout \
                reason=timeout_out_of_range detail={detail}"
            ),
        ),
        (
            TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::Token), R::TokenNotUuid),
            format!(
                "terminal_snapshot_error code=invalid_request field=token reason=token_not_uuid \
                detail={detail}"
            ),
        ),
        (
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Token),
                R::TokenIsPersistedStatic,
            ),
            format!(
                "terminal_snapshot_error code=invalid_request field=token \
                reason=token_is_persisted_static detail={detail}"
            ),
        ),
        (
            TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::To), R::TargetSyntaxInvalid),
            format!(
                "terminal_snapshot_error code=invalid_request field=to \
                reason=target_syntax_invalid detail={detail}"
            ),
        ),
        (
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputRequiredForPng,
            ),
            format!(
                "terminal_snapshot_error code=invalid_request field=output \
                reason=output_required_for_png detail={detail}"
            ),
        ),
        (
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputForbiddenForJson,
            ),
            format!(
                "terminal_snapshot_error code=invalid_request field=output \
                reason=output_forbidden_for_json detail={detail}"
            ),
        ),
    ];

    let mut lines = Vec::new();
    for (rejection, expected) in &cases {
        let line = rejection_line(*rejection);
        assert_eq!(&line, expected, "unexpected line for {rejection:?}");
        lines.push(line);
    }

    // The defect in #2223 is that these six were one indistinguishable
    // line. Pairwise distinctness is the property that removes it.
    for left in 0..lines.len() {
        for right in (left + 1)..lines.len() {
            assert_ne!(
                lines[left], lines[right],
                "causes {left} and {right} are still indistinguishable"
            );
        }
    }
}

#[test]
fn rejection_line_keeps_code_and_detail_contract() {
    for code in ALL_CODES {
        for field in [None, Some(TerminalSnapshotField::Root)] {
            let line = rejection_line(TerminalSnapshotRejection::new(
                code,
                field,
                TerminalSnapshotRejectionReason::Unattributed,
            ));
            assert!(
                line.starts_with(&format!("terminal_snapshot_error code={} ", code.as_str())),
                "prefix moved: {line}"
            );
            assert!(
                line.ends_with(&format!("detail={}", code.detail())),
                "detail is no longer last: {line}"
            );
        }
    }
}

#[test]
fn rejection_line_field_token_is_absent_when_unattributed() {
    let line = rejection_line(TerminalSnapshotRejection::new(
        TerminalSnapshotReasonCode::Internal,
        None,
        TerminalSnapshotRejectionReason::Unattributed,
    ));
    assert!(
        !line.contains("field="),
        "empty field token emitted: {line}"
    );
    assert_eq!(
        line,
        format!(
            "terminal_snapshot_error code=internal reason=unattributed detail={}",
            TerminalSnapshotReasonCode::Internal.detail()
        )
    );
}

#[test]
fn rejection_vocabulary_is_closed_and_literal() {
    // Executable form of the no-caller-data invariant: a path, a token or
    // any formatted string cannot satisfy this shape.
    fn assert_literal(value: &str) {
        assert!(!value.is_empty(), "empty vocabulary entry");
        let mut characters = value.chars();
        let first = characters.next().expect("non-empty");
        assert!(
            first.is_ascii_lowercase(),
            "{value} does not start with [a-z]"
        );
        for character in characters {
            assert!(
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_',
                "{value} contains {character:?}"
            );
        }
        for forbidden in ['/', '\\', ':', '.', ' '] {
            assert!(!value.contains(forbidden), "{value} contains {forbidden:?}");
        }
    }

    for field in ALL_FIELDS {
        assert_literal(field.as_str());
    }
    for reason in ALL_REASONS {
        assert_literal(reason.as_str());
    }
}

#[test]
fn execute_inner_attributes_argument_rejections() {
    use TerminalSnapshotField as F;
    use TerminalSnapshotReasonCode as C;
    use TerminalSnapshotRejectionReason as R;

    let cases = [
        (
            "timeout_out_of_range",
            snapshot_args(
                VALID_TOKEN,
                "demo-project/demo-agent",
                SnapshotFormatArg::Json,
                None,
                1,
            ),
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Timeout),
                R::TimeoutOutOfRange,
            ),
        ),
        (
            "token_not_uuid",
            snapshot_args(
                "not-a-uuid",
                "demo-project/demo-agent",
                SnapshotFormatArg::Json,
                None,
                15,
            ),
            TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::Token), R::TokenNotUuid),
        ),
        (
            "target_syntax_invalid",
            snapshot_args(VALID_TOKEN, "###", SnapshotFormatArg::Json, None, 15),
            TerminalSnapshotRejection::new(C::InvalidRequest, Some(F::To), R::TargetSyntaxInvalid),
        ),
        // The PNG/JSON split. Both arms were one `_ =>` on base; if they
        // are ever collapsed again these two cases stop disagreeing.
        (
            "output_required_for_png",
            snapshot_args(
                VALID_TOKEN,
                "demo-project/demo-agent",
                SnapshotFormatArg::Png,
                None,
                15,
            ),
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputRequiredForPng,
            ),
        ),
        (
            "output_forbidden_for_json",
            snapshot_args(
                VALID_TOKEN,
                "demo-project/demo-agent",
                SnapshotFormatArg::Json,
                Some(PathBuf::from("snapshot.png")),
                15,
            ),
            TerminalSnapshotRejection::new(
                C::InvalidRequest,
                Some(F::Output),
                R::OutputForbiddenForJson,
            ),
        ),
    ];

    for (name, args, expected) in cases {
        let rejection = execute_inner(args).expect_err("case must be rejected");
        assert_eq!(rejection, expected, "wrong attribution for {name}");
    }
}

#[test]
fn propagated_string_conversion_preserves_every_code() {
    for code in ALL_CODES {
        let rejection = TerminalSnapshotRejection::from(code.as_str().to_string());
        assert_eq!(
            rejection.code,
            code,
            "propagated code collapsed for {}",
            code.as_str()
        );
        assert_eq!(rejection.field, None);
        assert_eq!(
            rejection.reason,
            TerminalSnapshotRejectionReason::Unattributed
        );
    }

    assert_eq!(
        TerminalSnapshotRejection::from("not_a_code".to_string()).code,
        TerminalSnapshotReasonCode::Internal
    );
}

#[test]
fn requester_identity_diagnostic_filters_unknown_and_path_bearing_reasons() {
    for reason in [
        "sender_identity_invalid",
        "target_not_member",
        "invalid_target",
        "unsafe_path",
        "invalid_envelope",
        "capacity_exceeded",
    ] {
        let text = requester_identity_diagnostic("replica_identity", reason);
        assert!(
            text.starts_with("[terminal-snapshot] requester identity failed stage="),
            "unexpected prefix: {text}"
        );
        assert!(text.contains("stage=replica_identity"), "got {text}");
        assert!(text.ends_with(&format!("reason={reason}")), "got {text}");
    }

    // The five `config::ac_root::ensure_authoritative_ac_root`
    // messages are the only non-literal reasons reachable here, and each
    // formats at least one absolute path. All five must collapse to
    // `unexpected`, so no path text can reach `app.log`. One sample proves
    // the fallback exists; five prove no message shape collides with an
    // allowlisted literal.
    const CALLER: &str = r"C:\Users\someone\proj\.ac";
    const PROJECT: &str = r"C:\Users\someone\proj";
    const AUTHORITATIVE: &str = r"C:\Users\someone\proj\.agentscommander";
    for reason in [
        format!("Project AC Root path '{CALLER}' has no valid directory name"),
        format!("Project AC Root path '{CALLER}' is not a Project AC Root directory"),
        format!("Project AC Root path '{CALLER}' has no parent project directory"),
        format!("project '{PROJECT}' has no Project AC Root directory"),
        format!(
            "Project AC Root '{CALLER}' rejected because authoritative Project AC Root \
            '{AUTHORITATIVE}' exists"
        ),
        String::new(),
    ] {
        let text = requester_identity_diagnostic("root_directory", &reason);
        assert!(text.ends_with("reason=unexpected"), "got {text}");
        assert!(text.contains("stage=root_directory"), "got {text}");
        for forbidden in [r"C:\", "someone", ".ac"] {
            assert!(
                !text.contains(forbidden),
                "diagnostic leaked {forbidden}: {text}"
            );
        }
    }
}

fn write_private_response(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn timeout_cancellation_claims_and_removes_only_the_original_request() {
    let directory = tempfile::TempDir::new().unwrap();
    let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    let caller_output = directory.path().join("caller-output.png");
    let nonce = "a".repeat(64);

    let request_id = Uuid::new_v4();
    let request = directory.path().join(format!("{request_id}.json"));
    let marker = directory
        .path()
        .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
    std::fs::write(&request, b"unclaimed-request").unwrap();
    let expected = crate::path_identity::verify_regular_file(&request).unwrap();
    assert!(cancel_request_observed(
        &request,
        directory.path(),
        request_id,
        &nonce,
        &expected,
        |claimed| {
            assert_eq!(claimed, marker);
            assert!(!request.exists());
            let claimed_identity = crate::path_identity::verify_regular_file(claimed).unwrap();
            assert!(crate::path_identity::same_object(
                &expected,
                &claimed_identity
            ));
            assert!(crate::path_identity::is_verified_descendant(
                &claimed_identity,
                &directory_identity
            ));
        },
    ));
    assert!(!request.exists());
    assert!(!marker.exists());
    assert!(!caller_output.exists());

    let request_id = Uuid::new_v4();
    let request = directory.path().join(format!("{request_id}.json"));
    let processing = directory.path().join(format!(
        ".{request_id}.{}.terminal-snapshot-processing",
        Uuid::new_v4()
    ));
    std::fs::write(&request, b"daemon-claimed-request").unwrap();
    let expected = crate::path_identity::verify_regular_file(&request).unwrap();
    crate::path_identity::publish_new_file_atomic(&request, &processing).unwrap();
    assert!(!cancel_request_for_test(
        &request,
        directory.path(),
        request_id,
        &nonce,
        &expected,
    ));
    assert_eq!(
        std::fs::read(&processing).unwrap(),
        b"daemon-claimed-request"
    );
    assert!(!directory
        .path()
        .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"))
        .exists());

    let request_id = Uuid::new_v4();
    let request = directory.path().join(format!("{request_id}.json"));
    let marker = directory
        .path()
        .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
    std::fs::write(&request, b"collision-request").unwrap();
    let expected = crate::path_identity::verify_regular_file(&request).unwrap();
    std::fs::write(&marker, b"marker-collision").unwrap();
    assert!(!cancel_request_for_test(
        &request,
        directory.path(),
        request_id,
        &nonce,
        &expected,
    ));
    assert_eq!(std::fs::read(&request).unwrap(), b"collision-request");
    assert_eq!(std::fs::read(&marker).unwrap(), b"marker-collision");

    let request_id = Uuid::new_v4();
    let request = directory.path().join(format!("{request_id}.json"));
    let accepted = directory.path().join(format!(
        ".{request_id}.{}.terminal-snapshot-processing",
        Uuid::new_v4()
    ));
    std::fs::write(&request, b"original-request").unwrap();
    let expected = crate::path_identity::verify_regular_file(&request).unwrap();
    crate::path_identity::publish_new_file_atomic(&request, &accepted).unwrap();
    std::fs::write(&request, b"replacement-request").unwrap();
    assert!(!cancel_request_for_test(
        &request,
        directory.path(),
        request_id,
        &nonce,
        &expected,
    ));
    assert_eq!(std::fs::read(&request).unwrap(), b"replacement-request");
    assert_eq!(std::fs::read(&accepted).unwrap(), b"original-request");

    let request_id = Uuid::new_v4();
    let request = directory.path().join(format!("{request_id}.json"));
    let marker = directory
        .path()
        .join(format!(".{request_id}.{nonce}.terminal-snapshot-cancelled"));
    let displaced = directory.path().join("displaced-original-request");
    std::fs::write(&request, b"original-at-marker").unwrap();
    let expected = crate::path_identity::verify_regular_file(&request).unwrap();
    assert!(!cancel_request_observed(
        &request,
        directory.path(),
        request_id,
        &nonce,
        &expected,
        |claimed| {
            std::fs::rename(claimed, &displaced).unwrap();
            std::fs::write(claimed, b"replacement-at-marker").unwrap();
        },
    ));
    assert_eq!(std::fs::read(&marker).unwrap(), b"replacement-at-marker");
    assert_eq!(std::fs::read(&displaced).unwrap(), b"original-at-marker");
    let displaced_identity = crate::path_identity::verify_regular_file(&displaced).unwrap();
    assert!(crate::path_identity::same_object(
        &expected,
        &displaced_identity
    ));
    assert!(!caller_output.exists());
}

#[test]
fn malformed_corrupt_and_late_responses_are_removed_after_rejection() {
    let directory = tempfile::TempDir::new().unwrap();
    let target = "project:wg-1-team/member";
    let tag = "a".repeat(64);
    let request_id = Uuid::new_v4().to_string();
    let deadline = Instant::now() + Duration::from_secs(30);

    let malformed = directory.path().join("malformed.json");
    write_private_response(&malformed, b"{");
    assert_eq!(
        read_response(
            &malformed,
            &request_id,
            &tag,
            target,
            TerminalSnapshotFormat::Json,
            deadline,
        )
        .unwrap_err(),
        "response_unavailable"
    );
    assert!(!malformed.exists());

    let corrupt = directory.path().join("corrupt.json");
    let corrupt_bytes = terminal_snapshot_renderer::encode_host_failure_payload(
        &Uuid::new_v4().to_string(),
        &tag,
        &canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .unwrap();
    write_private_response(&corrupt, &corrupt_bytes);
    assert_eq!(
        read_response(
            &corrupt,
            &request_id,
            &tag,
            target,
            TerminalSnapshotFormat::Json,
            deadline,
        )
        .unwrap_err(),
        "response_unavailable"
    );
    assert!(!corrupt.exists());

    let late = directory.path().join("late.json");
    let late_bytes = terminal_snapshot_renderer::encode_host_failure_payload(
        &request_id,
        &tag,
        &canonical_timestamp(chrono::Utc::now() - chrono::Duration::seconds(1)),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .unwrap();
    write_private_response(&late, &late_bytes);
    let (response, identity) = read_response(
        &late,
        &request_id,
        &tag,
        target,
        TerminalSnapshotFormat::Json,
        deadline,
    )
    .unwrap();
    assert_eq!(
        finish_response(
            &late,
            &identity,
            &response,
            TerminalSnapshotFormat::Json,
            None,
            "project:wg-1-team/coordinator",
            target,
            deadline,
        )
        .unwrap_err(),
        "response_unavailable"
    );
    assert!(!late.exists());
}

#[test]
fn expired_deadline_wins_before_response_read_or_output_handoff() {
    let missing = std::env::temp_dir().join(format!("ac-snapshot-missing-{}.json", Uuid::new_v4()));
    let error = read_response(
        &missing,
        "00000000-0000-4000-8000-000000000119",
        &"a".repeat(64),
        "project:wg-1-team/member",
        TerminalSnapshotFormat::Json,
        Instant::now(),
    )
    .unwrap_err();
    assert_eq!(error, "snapshot_timeout");
    assert_eq!(
        write_stdout_line(b"{}", Instant::now()).unwrap_err(),
        "snapshot_timeout"
    );
}

#[test]
fn argument_debug_omits_token_target_and_path_canaries() {
    const AUTH_CANARY: &str = "AUTH_1173_C3P7";
    const PATH_CANARY: &str = r"C:\PATH_1173_C3P7\snapshot.png";
    let args = TerminalSnapshotArgs {
        token: AUTH_CANARY.to_string(),
        root: PathBuf::from(PATH_CANARY),
        to: AUTH_CANARY.to_string(),
        format: SnapshotFormatArg::Png,
        output: Some(PathBuf::from(PATH_CANARY)),
        timeout: 15,
    };
    let diagnostic = format!("{args:?}");
    assert!(!diagnostic.contains(AUTH_CANARY));
    assert!(!diagnostic.contains(PATH_CANARY));
    assert!(diagnostic.contains("format: Png"));
    assert!(diagnostic.contains("has_output: true"));
    assert!(diagnostic.contains("timeout: 15"));
}

#[test]
fn format_conversion_is_exact() {
    assert_eq!(
        TerminalSnapshotFormat::from(SnapshotFormatArg::Json),
        TerminalSnapshotFormat::Json
    );
    assert_eq!(
        TerminalSnapshotFormat::from(SnapshotFormatArg::Png),
        TerminalSnapshotFormat::Png
    );
}

#[test]
fn p04_private_child_creates_directory_and_preserves_regular_file() {
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("private");
    ensure_private_child(directory.path(), &child).unwrap();
    assert!(child.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&child).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let occupied = directory.path().join("occupied");
    std::fs::write(&occupied, b"keep").unwrap();
    let parent = crate::path_identity::verify_directory(directory.path()).unwrap();
    assert_eq!(
        ensure_private_child(directory.path(), &occupied).unwrap_err(),
        "unsafe_path"
    );
    assert_eq!(std::fs::read(&occupied).unwrap(), b"keep");
    assert!(crate::path_identity::same_object(
        &parent,
        &crate::path_identity::verify_directory(directory.path()).unwrap(),
    ));
}

#[test]
fn p04_snapshot_publication_preserves_bytes_stages_and_collisions() {
    let directory = tempfile::tempdir().unwrap();
    let temporary = directory.path().join("temporary");
    let destination = directory.path().join("request.json");
    let mut stages = Vec::new();
    crate::filesystem::publish_terminal_snapshot_request(
        temporary.clone(),
        &destination,
        b"private request",
        |stage| stages.push(stage),
    )
    .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"private request");
    let mut expected = vec!["publish_request.open_temporary"];
    #[cfg(unix)]
    expected.push("publish_request.unix_mode");
    expected.extend([
        "publish_request.write_temporary",
        "publish_request.verify_temporary",
        "publish_request.publish_atomic",
        "publish_request.verify_published",
    ]);
    assert_eq!(stages, expected);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        crate::filesystem::publish_terminal_snapshot_request(
            temporary.clone(),
            &destination,
            b"replacement",
            |_| {},
        )
        .unwrap_err(),
        "response_unavailable"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"private request");
    std::fs::write(&temporary, b"keep temporary").unwrap();
    let absent = directory.path().join("absent.json");
    assert_eq!(
        crate::filesystem::publish_terminal_snapshot_request(
            temporary.clone(),
            &absent,
            b"replacement",
            |_| {},
        )
        .unwrap_err(),
        "response_unavailable"
    );
    assert_eq!(std::fs::read(&temporary).unwrap(), b"keep temporary");
    assert!(!absent.exists());
}

#[test]
fn p04_snapshot_response_accepts_private_valid_body() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("response.json");
    let request_id = Uuid::new_v4().to_string();
    let tag = "a".repeat(64);
    let bytes = terminal_snapshot_renderer::encode_host_failure_payload(
        &request_id,
        &tag,
        &canonical_timestamp(chrono::Utc::now() + chrono::Duration::seconds(60)),
        TerminalSnapshotReasonCode::InvalidRequest,
    )
    .unwrap();
    write_private_response(&path, &bytes);
    let read = || {
        read_response(
            &path,
            &request_id,
            &tag,
            "project:wg-1-team/member",
            TerminalSnapshotFormat::Json,
            Instant::now() + Duration::from_secs(30),
        )
    };
    assert!(read().is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read().unwrap_err(), "response_unavailable");
    }
}
