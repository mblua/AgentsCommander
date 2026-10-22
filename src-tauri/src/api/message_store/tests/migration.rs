use super::*;

fn setup_v2_database(path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS api_message_schema(
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messages(
            message_id TEXT PRIMARY KEY,
            sender_fqn TEXT NOT NULL,
            target_fqn TEXT NOT NULL,
            op_id TEXT NOT NULL,
            content_type TEXT NOT NULL,
            body TEXT NOT NULL,
            body_sha256 TEXT NOT NULL,
            body_bytes INTEGER NOT NULL,
            source_plane TEXT NOT NULL,
            source_ref TEXT NULL,
            status TEXT NOT NULL,
            attempt INTEGER NOT NULL DEFAULT 0,
            next_attempt_at TEXT NOT NULL,
            lease_owner TEXT NULL,
            lease_until TEXT NULL,
            created_at TEXT NOT NULL,
            delivered_at TEXT NULL,
            last_error TEXT NULL,
            UNIQUE(sender_fqn, op_id)
        );
        CREATE INDEX IF NOT EXISTS idx_messages_due
            ON messages(status, next_attempt_at, lease_until);
        CREATE TABLE IF NOT EXISTS message_audit(
            event_id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL,
            status TEXT NOT NULL,
            detail TEXT NULL,
            at TEXT NOT NULL,
            FOREIGN KEY(message_id) REFERENCES messages(message_id) ON DELETE CASCADE
        );

        INSERT INTO api_message_schema(version, applied_at)

        VALUES(1, '2026-08-30T00:00:00.000Z');

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
            status TEXT NOT NULL CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
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
            CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
            CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
            CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
            CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
            CHECK(sender_identity_fingerprint IS NULL OR (length(sender_identity_fingerprint)=64 AND sender_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
            CHECK(target_identity_fingerprint IS NULL OR (length(target_identity_fingerprint)=64 AND target_identity_fingerprint NOT GLOB '*[^0-9a-f]*')),
            CHECK(payload IS NULL OR length(payload)=payload_bytes),
            CHECK(length(issued_at)=24 AND length(expires_at)=24 AND length(queued_at)=24
                  AND length(next_attempt_at)=24 AND length(updated_at)=24),
            CHECK(issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(next_attempt_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(preparing_at IS NULL OR (length(preparing_at)=24 AND preparing_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
            CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
            CHECK(terminal_at IS NULL OR (length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
            CHECK(host_artifact_at IS NULL OR (length(host_artifact_at)=24 AND host_artifact_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
            CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
            CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
            CHECK(reason_code IS NULL OR reason_code IN (
                'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                'session_token_required','invalid_session_token','ambiguous_session_token',
                'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                'sender_not_coordinator','root_identity_invalid','target_not_member',
                'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                'busy','resize_unsettled','untracked_readiness','unsupported_session',
                'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                'session_race','lease_lost','spawn_failed_safe','store_transient',
                'final_revalidation_failed','text_write_failed','required_enter_failed',
                'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
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
            CHECK((status IN ('injected','rejected','indeterminate')) = (terminal_at IS NOT NULL)),
            CHECK((status = 'preparing') = (lease_owner IS NOT NULL AND lease_until IS NOT NULL)),
            CHECK(status!='preparing' OR preparing_at IS NOT NULL),
            CHECK(queued_at>=issued_at AND queued_at<expires_at),
            CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
            CHECK(terminal_at IS NULL OR terminal_at>=queued_at),
            CHECK(host_artifact_at IS NULL OR terminal_at IS NOT NULL),
            CHECK((status IN ('queued','preparing','retry') AND
                   (reason_code IS NULL OR reason_code IN (
                     'restore_in_progress','purge_in_progress','session_race',
                     'lease_lost','spawn_failed_safe','store_transient')))
               OR (status='actuating' AND reason_code IS NULL)
               OR (status='injected' AND
                   (reason_code IS NULL OR reason_code IN (
                     'redundant_enter_failed','boundary_metadata_failed')))
               OR (status='rejected' AND reason_code IS NOT NULL AND reason_code NOT IN (
                     'final_revalidation_failed','text_write_failed','required_enter_failed',
                     'daemon_restart_after_actuation','runtime_actuation_orphan',
                     'terminal_store_failed','redundant_enter_failed',
                     'boundary_metadata_failed','artifact_unclaimed'))
               OR (status='indeterminate' AND reason_code IN (
                     'final_revalidation_failed','text_write_failed','required_enter_failed',
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
            FOREIGN KEY(injection_id) REFERENCES pty_input_operations(injection_id) ON DELETE CASCADE,
            CHECK(version=1),
            CHECK(payload_bytes BETWEEN 1 AND 65536),
            CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
            CHECK(source_plane IN ('host_cli','container_api')),
            CHECK(status IN ('queued','preparing','retry','actuating','injected','rejected','indeterminate')),
            CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
            CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
            CHECK(length(at)=24 AND at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
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
            CHECK(length(request_fingerprint)=64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'),
            CHECK(length(sender_incarnation_fingerprint)=64 AND sender_incarnation_fingerprint NOT GLOB '*[^0-9a-f]*'),
            CHECK(length(payload_sha256)=64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'),
            CHECK(confirmation_tag IS NULL OR (length(confirmation_tag)=64 AND confirmation_tag NOT GLOB '*[^0-9a-f]*')),
            CHECK((source_plane='host_cli' AND confirmation_tag IS NOT NULL AND injection_id=op_id)
               OR (source_plane='container_api' AND confirmation_tag IS NULL)),
            CHECK((selected_session_id IS NULL) = (selected_backend IS NULL)),
            CHECK(selected_backend IS NULL OR selected_backend IN ('localProcess','containerTransport')),
            CHECK(length(issued_at)=24 AND issued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(length(expires_at)=24 AND expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(length(queued_at)=24 AND queued_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(length(terminal_at)=24 AND terminal_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
            CHECK(actuating_at IS NULL OR (length(actuating_at)=24 AND actuating_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')),
            CHECK(queued_at>=issued_at AND queued_at<expires_at),
            CHECK(actuating_at IS NULL OR (actuating_at>=queued_at AND actuating_at<expires_at)),
            CHECK(terminal_at>=queued_at),
            CHECK((reason_code IS NULL) = (reason_detail IS NULL)),
            CHECK(reason_code IS NULL OR reason_code IN (
                'invalid_envelope','mixed_payload','unsupported_version','invalid_enter_mode',
                'invalid_id','invalid_nonce','invalid_timestamp','expired','invalid_target',
                'invalid_text','payload_too_large','idempotency_conflict','capacity_exceeded',
                'session_token_required','invalid_session_token','ambiguous_session_token',
                'sender_session_not_live','sender_backend_not_local','sender_identity_invalid',
                'sender_not_coordinator','root_identity_invalid','target_not_member',
                'target_is_coordinator','target_out_of_scope','unsafe_path','api_scope_required',
                'api_client_unbound','api_client_stale','api_binding_mismatch','authority_changed',
                'busy','resize_unsettled','untracked_readiness','unsupported_session',
                'nonpersistent_live_session','inconsistent_session','unsupported_profile',
                'readiness_timeout','store_corrupt','restore_in_progress','purge_in_progress',
                'session_race','lease_lost','spawn_failed_safe','store_transient',
                'final_revalidation_failed','text_write_failed','required_enter_failed',
                'daemon_restart_after_actuation','runtime_actuation_orphan','terminal_store_failed',
                'redundant_enter_failed','boundary_metadata_failed','artifact_unclaimed'
            )),
            CHECK((status='injected' AND actuating_at IS NOT NULL
                   AND selected_session_id IS NOT NULL
                   AND (reason_code IS NULL OR reason_code IN (
                     'redundant_enter_failed','boundary_metadata_failed')))
               OR (status='rejected' AND actuating_at IS NULL
                   AND selected_session_id IS NULL
                   AND reason_code IS NOT NULL AND reason_code NOT IN (
                     'final_revalidation_failed','text_write_failed','required_enter_failed',
                     'daemon_restart_after_actuation','runtime_actuation_orphan',
                     'terminal_store_failed','redundant_enter_failed',
                     'boundary_metadata_failed','artifact_unclaimed'))
               OR (status='indeterminate' AND actuating_at IS NOT NULL
                   AND selected_session_id IS NOT NULL
                   AND reason_code IN (
                     'final_revalidation_failed','text_write_failed','required_enter_failed',
                     'daemon_restart_after_actuation','runtime_actuation_orphan',
                     'terminal_store_failed')))
        );

        INSERT INTO api_message_schema(version, applied_at)

        VALUES(2, '2026-08-30T00:00:01.000Z');
        "#,
    )
    .unwrap();
    conn
}

fn snapshot_table(conn: &rusqlite::Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let sql = format!("SELECT * FROM {table} ORDER BY injection_id");
    let mut statement = conn.prepare(&sql).unwrap();
    let column_count = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            (0..column_count)
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}

#[test]
fn test_schema_v3_migration_rebuilds_check_constraints() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(DB_FILENAME);
    let conn = setup_v2_database(&path);
    conn.execute_batch(
        r#"
        INSERT INTO pty_input_operations(
            injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
            request_fingerprint,confirmation_tag,version,enter_mode,
            requested_agent_id,payload,payload_sha256,payload_bytes,source_plane,
            sender_incarnation_fingerprint,sender_identity_fingerprint,
            target_identity_fingerprint,authority_session_id,authority_client_id,
            authority_client_generation,status,attempt,next_attempt_at,lease_owner,
            lease_until,selected_session_id,selected_backend,issued_at,expires_at,
            queued_at,preparing_at,actuating_at,terminal_at,host_artifact_at,
            updated_at,reason_code,reason_detail
        ) VALUES
        (
            '00000000-0000-4000-8000-000000000001','proj:wg-1/s','proj:wg-1/t',
            '00000000-0000-4000-8000-000000000001',printf('%064x',101),
            printf('%064x',201),NULL,1,'agent-submit','agent-queued',X'717565756564',
            printf('%064x',301),6,'container_api',printf('%064x',401),
            printf('%064x',501),printf('%064x',601),
            '00000000-0000-4000-8000-000000000011','client-queued',
            '00000000-0000-4000-8000-000000000021','queued',0,
            '2099-08-30T00:00:02.000Z',NULL,NULL,NULL,NULL,
            '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
            '2099-08-30T00:00:01.000Z',NULL,NULL,NULL,NULL,
            '2099-08-30T00:00:01.000Z',NULL,NULL
        ),
        (
            '00000000-0000-4000-8000-000000000002','proj:wg-1/s','proj:wg-1/t',
            '00000000-0000-4000-8000-000000000002',printf('%064x',102),
            printf('%064x',202),NULL,1,'agent-submit','agent-preparing',
            X'707265706172696e67',printf('%064x',302),9,'container_api',
            printf('%064x',402),printf('%064x',502),printf('%064x',602),
            '00000000-0000-4000-8000-000000000012','client-preparing',
            '00000000-0000-4000-8000-000000000022','preparing',2,
            '2099-08-30T00:00:02.000Z','lease-preparing',
            '2099-08-30T00:00:04.000Z',NULL,NULL,
            '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
            '2099-08-30T00:00:01.000Z','2099-08-30T00:00:03.000Z',
            NULL,NULL,NULL,'2099-08-30T00:00:03.000Z',NULL,NULL
        ),
        (
            '00000000-0000-4000-8000-000000000003','proj:wg-1/s','proj:wg-1/t',
            '00000000-0000-4000-8000-000000000003',printf('%064x',103),
            printf('%064x',203),NULL,1,'agent-submit','agent-retry',X'7265747279',
            printf('%064x',303),5,'container_api',printf('%064x',403),
            printf('%064x',503),printf('%064x',603),
            '00000000-0000-4000-8000-000000000013','client-retry',
            '00000000-0000-4000-8000-000000000023','retry',3,
            '2099-08-30T00:00:06.000Z',NULL,NULL,NULL,NULL,
            '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
            '2099-08-30T00:00:01.000Z','2099-08-30T00:00:02.000Z',
            NULL,NULL,NULL,'2099-08-30T00:00:04.000Z','store_transient',
            'A transient operation-store failure prevented actuation.'
        );

        INSERT INTO pty_input_tombstones(
            injection_id,sender_fqn,target_fqn,op_id,nonce_sha256,
            request_fingerprint,confirmation_tag,sender_incarnation_fingerprint,
            version,payload_sha256,payload_bytes,source_plane,selected_session_id,
            selected_backend,issued_at,expires_at,queued_at,actuating_at,
            terminal_at,status,reason_code,reason_detail
        ) VALUES
        (
            '00000000-0000-4000-8000-000000000004','proj:wg-1/s','proj:wg-1/t',
            '00000000-0000-4000-8000-000000000004',printf('%064x',104),
            printf('%064x',204),printf('%064x',704),printf('%064x',404),1,
            printf('%064x',304),8,'host_cli',
            '00000000-0000-4000-8000-000000000034','localProcess',
            '2099-08-30T00:00:00.000Z','2099-08-30T01:00:00.000Z',
            '2099-08-30T00:00:01.000Z','2099-08-30T00:00:03.000Z',
            '2099-08-30T00:00:05.000Z','injected','boundary_metadata_failed',
            'The terminal result was committed but boundary metadata failed.'
        ),
        (
            '00000000-0000-4000-8000-000000000005','proj:wg-1/s','proj:wg-1/t',
            '00000000-0000-4000-8000-000000000005',printf('%064x',105),
            printf('%064x',205),NULL,printf('%064x',405),1,printf('%064x',305),
            7,'container_api',NULL,NULL,'2099-08-30T00:00:00.000Z',
            '2099-08-30T01:00:00.000Z','2099-08-30T00:00:01.000Z',NULL,
            '2099-08-30T00:00:05.000Z','rejected','invalid_text',
            'The payload text contains prohibited control characters.'
        );
        "#,
    )
    .unwrap();

    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 2);
    let v2_schema: String = conn
        .query_row(
            r#"SELECT group_concat(sql, char(10)) FROM sqlite_schema
               WHERE type='table'
                 AND name IN ('pty_input_operations','pty_input_tombstones')"#,
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!v2_schema.contains("menu_guard_blocked"));
    assert!(conn
        .execute(
            r#"UPDATE pty_input_operations
               SET reason_code='menu_guard_blocked', reason_detail='blocked'
               WHERE injection_id='00000000-0000-4000-8000-000000000003'"#,
            [],
        )
        .is_err());

    let expected_operations = snapshot_table(&conn, "pty_input_operations");
    let expected_tombstones = snapshot_table(&conn, "pty_input_tombstones");
    assert_eq!(expected_operations.len(), 3);
    assert_eq!(expected_tombstones.len(), 2);
    drop(conn);

    // Startup recovery would legitimately rewrite a preparing operation. Hold its
    // operation stripe so this test isolates the v2 -> v3 table-copy migration.
    let preparing_guard = try_stripe_lock_at(
        path.parent().unwrap(),
        "00000000-0000-4000-8000-000000000002",
        true,
    )
    .unwrap()
    .unwrap();
    let store = MessageStore::open(path).unwrap();
    drop(preparing_guard);

    let conn = store.conn.lock().unwrap();
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM api_message_schema", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 3);
    let due_index: (String, String, String) = conn
        .query_row(
            r#"SELECT name,tbl_name,sql FROM sqlite_schema
               WHERE type='index' AND name='idx_pty_input_due'"#,
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(due_index.0, "idx_pty_input_due");
    assert_eq!(due_index.1, "pty_input_operations");
    assert_eq!(
        due_index.2.split_whitespace().collect::<Vec<_>>().join(" "),
        "CREATE INDEX idx_pty_input_due ON \
         pty_input_operations(source_plane, status, next_attempt_at, lease_until)"
    );
    assert_eq!(
        snapshot_table(&conn, "pty_input_operations"),
        expected_operations
    );
    assert_eq!(
        snapshot_table(&conn, "pty_input_tombstones"),
        expected_tombstones
    );
    drop(conn);

    let id = Uuid::new_v4().to_string();
    let enqueued = store.enqueue_pty_input(pty_request(&id, "hello")).unwrap();
    assert_eq!(
        enqueued.result.status,
        crate::phone::types::PtyInputPublicStatus::Queued
    );
    store
        .claim_pty_input(
            crate::phone::types::PtyInputSourcePlane::ContainerApi,
            Some(&id),
            "lease-v3",
            Utc::now(),
        )
        .unwrap()
        .unwrap();
    store
        .retry_pty_input(
            &id,
            "lease-v3",
            crate::phone::types::PtyInputReasonCode::MenuGuardBlocked,
            Utc::now(),
        )
        .unwrap();
    let retry = store.query_pty_input_by_injection(&id).unwrap().unwrap();
    assert_eq!(
        retry.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::MenuGuardBlocked
    );

    let terminal = store
        .terminalize_pty_input(
            &id,
            crate::phone::types::PtyInputPublicStatus::Rejected,
            Some(crate::phone::types::PtyInputReasonCode::MenuGuardBlocked),
            Utc::now(),
        )
        .unwrap();
    assert!(terminal.terminal);
    assert_eq!(
        terminal.reason.unwrap().code,
        crate::phone::types::PtyInputReasonCode::MenuGuardBlocked
    );
    let tombstone: (String, String) = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT status,reason_code FROM pty_input_tombstones WHERE injection_id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(tombstone, ("rejected".into(), "menu_guard_blocked".into()));
}
