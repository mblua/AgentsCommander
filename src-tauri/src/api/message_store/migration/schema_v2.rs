use super::MessageStoreError;
use chrono::Utc;
use rusqlite::Transaction;

pub(super) fn apply(tx: &Transaction<'_>) -> Result<(), MessageStoreError> {
    tx.execute_batch(SQL)?;
    tx.execute(
        "INSERT INTO api_message_schema(version, applied_at) VALUES(2, ?1)",
        [crate::phone::types::canonical_pty_timestamp(Utc::now())],
    )?;
    Ok(())
}

const SQL: &str = r#"
                CREATE TABLE pty_input_operations(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    version INTEGER NOT NULL CHECK(version = 1),
                    enter_mode TEXT NOT NULL CHECK(enter_mode = 'agent-submit'),
                    requested_agent_id TEXT NULL,
                    payload BLOB NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL CHECK(payload_bytes BETWEEN 1 AND 65536),
                    source_plane TEXT NOT NULL CHECK(source_plane IN ('host_cli','container_api')),
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    sender_identity_fingerprint TEXT NULL,
                    target_identity_fingerprint TEXT NULL,
                    authority_session_id TEXT NULL,
                    authority_client_id TEXT NULL,
                    authority_client_generation TEXT NULL,
                    status TEXT NOT NULL CHECK(status IN
                        ('queued', 'preparing', 'retry', 'actuating', 'injected', 'rejected',
                            'indeterminate')),
                    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 5),
                    next_attempt_at TEXT NOT NULL,
                    lease_owner TEXT NULL,
                    lease_until TEXT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    preparing_at TEXT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NULL,
                    host_artifact_at TEXT NULL,
                    updated_at TEXT NOT NULL,
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(authority_session_id IS NULL OR
                          (length(authority_session_id)=36
                           AND substr(authority_session_id,9,1)='-'
                           AND substr(authority_session_id,14,1)='-'
                           AND substr(authority_session_id,15,1)='4'
                           AND substr(authority_session_id,19,1)='-'
                           AND substr(authority_session_id,20,1) GLOB '[89ab]'
                           AND substr(authority_session_id,24,1)='-'
                           AND length(replace(authority_session_id,'-',''))=32
                           AND replace(authority_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(authority_client_generation IS NULL OR
                          (length(authority_client_generation)=36
                           AND substr(authority_client_generation,9,1)='-'
                           AND substr(authority_client_generation,14,1)='-'
                           AND substr(authority_client_generation,15,1)='4'
                           AND substr(authority_client_generation,19,1)='-'
                           AND substr(authority_client_generation,20,1) GLOB '[89ab]'
                           AND substr(authority_client_generation,24,1)='-'
                           AND length(replace(authority_client_generation,'-',''))=32
                           AND replace(authority_client_generation,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB
                        '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND
                        sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND
                        confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK(sender_identity_fingerprint IS NULL OR
                        (length(sender_identity_fingerprint)=64 AND sender_identity_fingerprint NOT
                        GLOB '*[^0-9a-f]*')),
                    CHECK(target_identity_fingerprint IS NULL OR
                        (length(target_identity_fingerprint)=64 AND target_identity_fingerprint NOT
                        GLOB '*[^0-9a-f]*')),
                    CHECK(payload IS NULL OR length(payload)=payload_bytes),
                    CHECK(length(issued_at)=24 AND length(expires_at)=24 AND length(queued_at)=24
                          AND length(next_attempt_at)=24 AND length(updated_at)=24),
                    CHECK(issued_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(expires_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(queued_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(next_attempt_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(updated_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(preparing_at IS NULL OR (length(preparing_at)=24 AND preparing_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(terminal_at IS NULL OR (length(terminal_at)=24 AND terminal_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(host_artifact_at IS NULL OR (length(host_artifact_at)=24 AND
                        host_artifact_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN
                        ('localProcess','containerTransport')),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope', 'mixed_payload', 'unsupported_version',
                            'invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text', 'payload_too_large', 'idempotency_conflict',
                            'capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live', 'sender_backend_not_local',
                            'sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator', 'target_out_of_scope', 'unsafe_path',
                            'api_scope_required',
                        'api_client_unbound', 'api_client_stale', 'api_binding_mismatch',
                            'authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout', 'store_corrupt', 'restore_in_progress',
                            'purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation', 'runtime_actuation_orphan',
                            'terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(
                        (status IN ('queued','preparing','retry')
                         AND payload IS NOT NULL
                         AND authority_session_id IS NOT NULL
                         AND sender_identity_fingerprint IS NOT NULL
                         AND target_identity_fingerprint IS NOT NULL
                         AND actuating_at IS NULL AND terminal_at IS NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status='rejected' AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NULL AND terminal_at IS NOT NULL
                         AND selected_session_id IS NULL AND selected_backend IS NULL)
                        OR
                        (status IN ('actuating','injected','indeterminate')
                         AND payload IS NULL AND requested_agent_id IS NULL
                         AND authority_session_id IS NULL AND authority_client_id IS NULL
                         AND authority_client_generation IS NULL
                         AND sender_identity_fingerprint IS NULL
                         AND target_identity_fingerprint IS NULL
                         AND actuating_at IS NOT NULL
                         AND selected_session_id IS NOT NULL AND selected_backend IS NOT NULL)
                    ),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL
                           AND authority_client_id IS NULL AND authority_client_generation IS NULL)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL
                           AND ((status IN ('queued','preparing','retry')
                                 AND authority_client_id IS NOT NULL
                                 AND authority_client_generation IS NOT NULL)
                             OR (status IN ('actuating','injected','rejected','indeterminate')
                                 AND authority_client_id IS NULL
                                 AND authority_client_generation IS NULL)))),
                    CHECK((status IN ('injected','rejected','indeterminate')) = (terminal_at IS NOT
                        NULL)),
                    CHECK((status = 'preparing') = (lease_owner IS NOT NULL AND lease_until IS NOT
                        NULL)),
                    CHECK(status!='preparing' OR preparing_at IS NOT NULL),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND
                        actuating_at<expires_at)),
                    CHECK(terminal_at IS NULL OR terminal_at>=queued_at),
                    CHECK(host_artifact_at IS NULL OR terminal_at IS NOT NULL),
                    CHECK((status IN ('queued','preparing','retry') AND
                           (reason_code IS NULL OR reason_code IN (
                             'restore_in_progress','purge_in_progress','session_race',
                             'lease_lost','spawn_failed_safe','store_transient',
                             'menu_guard_blocked')))
                       OR (status='actuating' AND reason_code IS NULL)
                       OR (status='injected' AND
                           (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed', 'text_write_failed',
                                 'required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND reason_code IN (
                             'final_revalidation_failed', 'text_write_failed',
                                 'required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed'))),
                    CHECK(source_plane != 'host_cli' OR injection_id=op_id)
                );
                CREATE INDEX idx_pty_input_due
                    ON pty_input_operations(source_plane, status, next_attempt_at, lease_until);
                CREATE TABLE pty_input_audit(
                    event_id TEXT PRIMARY KEY,
                    injection_id TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    payload_bytes INTEGER NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    source_plane TEXT NOT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    status TEXT NOT NULL,
                    reason_code TEXT NULL,
                    at TEXT NOT NULL,
                    FOREIGN KEY(injection_id) REFERENCES pty_input_operations(injection_id) ON
                        DELETE CASCADE,
                    CHECK(version=1),
                    CHECK(payload_bytes BETWEEN 1 AND 65536),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(source_plane IN ('host_cli','container_api')),
                    CHECK(status IN
                        ('queued', 'preparing', 'retry', 'actuating', 'injected', 'rejected',
                            'indeterminate')),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN
                        ('localProcess','containerTransport')),
                    CHECK(length(at)=24 AND at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
                );
                CREATE TABLE pty_input_tombstones(
                    injection_id TEXT PRIMARY KEY,
                    sender_fqn TEXT NOT NULL,
                    target_fqn TEXT NOT NULL,
                    op_id TEXT NOT NULL,
                    nonce_sha256 TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    confirmation_tag TEXT NULL,
                    sender_incarnation_fingerprint TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    payload_bytes INTEGER NOT NULL,
                    source_plane TEXT NOT NULL,
                    selected_session_id TEXT NULL,
                    selected_backend TEXT NULL,
                    issued_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    queued_at TEXT NOT NULL,
                    actuating_at TEXT NULL,
                    terminal_at TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('injected','rejected','indeterminate')),
                    reason_code TEXT NULL,
                    reason_detail TEXT NULL,
                    UNIQUE(sender_fqn, op_id),
                    UNIQUE(sender_fqn, nonce_sha256),
                    CHECK(version=1),
                    CHECK(payload_bytes BETWEEN 1 AND 65536),
                    CHECK(length(injection_id)=36
                          AND substr(injection_id,9,1)='-'
                          AND substr(injection_id,14,1)='-'
                          AND substr(injection_id,15,1)='4'
                          AND substr(injection_id,19,1)='-'
                          AND substr(injection_id,20,1) GLOB '[89ab]'
                          AND substr(injection_id,24,1)='-'
                          AND length(replace(injection_id,'-',''))=32
                          AND replace(injection_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(op_id)=36
                          AND substr(op_id,9,1)='-'
                          AND substr(op_id,14,1)='-'
                          AND substr(op_id,15,1)='4'
                          AND substr(op_id,19,1)='-'
                          AND substr(op_id,20,1) GLOB '[89ab]'
                          AND substr(op_id,24,1)='-'
                          AND length(replace(op_id,'-',''))=32
                          AND replace(op_id,'-','') NOT GLOB '*[^0-9a-f]*'),
                    CHECK(selected_session_id IS NULL OR
                          (length(selected_session_id)=36
                           AND substr(selected_session_id,9,1)='-'
                           AND substr(selected_session_id,14,1)='-'
                           AND substr(selected_session_id,15,1)='4'
                           AND substr(selected_session_id,19,1)='-'
                           AND substr(selected_session_id,20,1) GLOB '[89ab]'
                           AND substr(selected_session_id,24,1)='-'
                           AND length(replace(selected_session_id,'-',''))=32
                           AND replace(selected_session_id,'-','') NOT GLOB '*[^0-9a-f]*')),
                    CHECK(length(nonce_sha256)=64 AND nonce_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB
                        '*[^0-9a-f]*'),
                    CHECK(length(sender_incarnation_fingerprint)=64 AND
                        sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
                    CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
                    CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND
                        confirmation_tag NOT GLOB '*[^0-9a-f]*')),
                    CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL AND
                        injection_id=op_id)
                       OR (source_plane='container_api' AND confirmation_tag IS NULL)),
                    CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
                    CHECK(selected_backend IS NULL OR selected_backend IN
                        ('localProcess','containerTransport')),
                    CHECK(length(issued_at)=24 AND issued_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(expires_at)=24 AND expires_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(queued_at)=24 AND queued_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(length(terminal_at)=24 AND terminal_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
                    CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB
                        '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T' ||
                            '[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
                    CHECK(queued_at>=issued_at AND queued_at<expires_at),
                    CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND
                        actuating_at<expires_at)),
                    CHECK(terminal_at>=queued_at),
                    CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
                    CHECK(reason_code IS NULL OR reason_code IN (
                        'invalid_envelope', 'mixed_payload', 'unsupported_version',
                            'invalid_enter_mode',
                        'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                        'invalid_text', 'payload_too_large', 'idempotency_conflict',
                            'capacity_exceeded',
                        'session_token_required','invalid_session_token','ambiguous_session_token',
                        'sender_session_not_live', 'sender_backend_not_local',
                            'sender_identity_invalid',
                        'sender_not_coordinator','root_identity_invalid','target_not_member',
                        'target_is_coordinator', 'target_out_of_scope', 'unsafe_path',
                            'api_scope_required',
                        'api_client_unbound', 'api_client_stale', 'api_binding_mismatch',
                            'authority_changed',
                        'busy','resize_unsettled','untracked_readiness','unsupported_session',
                        'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                        'readiness_timeout', 'store_corrupt', 'restore_in_progress',
                            'purge_in_progress',
                        'session_race','lease_lost','spawn_failed_safe','store_transient',
                        'menu_guard_blocked',
                        'final_revalidation_failed','text_write_failed','required_enter_failed',
                        'daemon_restart_after_actuation', 'runtime_actuation_orphan',
                            'terminal_store_failed',
                        'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
                    )),
                    CHECK((status='injected' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND (reason_code IS NULL OR reason_code IN (
                             'redundant_enter_failed','boundary_metadata_failed')))
                       OR (status='rejected' AND actuating_at IS NULL
                           AND selected_session_id IS NULL
                           AND reason_code IS NOT NULL AND reason_code NOT IN (
                             'final_revalidation_failed', 'text_write_failed',
                                 'required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed','redundant_enter_failed',
                             'boundary_metadata_failed','artifact_unclaimed'))
                       OR (status='indeterminate' AND actuating_at IS NOT NULL
                           AND selected_session_id IS NOT NULL
                           AND reason_code IN (
                             'final_revalidation_failed', 'text_write_failed',
                                 'required_enter_failed',
                             'daemon_restart_after_actuation','runtime_actuation_orphan',
                             'terminal_store_failed')))
                );
                "#;
