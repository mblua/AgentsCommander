#[test]
fn issue_3145_retained_prompt_artifacts_are_ignored() {
    for (name, kind) in [
        (RETAINED_PROMPTS, ArtifactKind::File),
        ("last-prompts.retained.json.*.tmp", ArtifactKind::Glob),
    ] {
        let rows: Vec<_> = INSTANCE_ARTIFACTS
            .iter()
            .filter(|row| row.name == name)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, kind);
        assert_eq!(rows[0].disposition, Disposition::Ignore);
    }
}
use super::*;

#[test]
fn pair_activity_artifact_names_and_ignore_registry() {
    ignore_rows_are_unique_and_byte_sorted_by_name();
    assert_eq!(
        CONFIG_IDENTITY_RESERVATION_NAME,
        "config-identity-reservation.state.no-git.json"
    );
    assert_eq!(
        CONFIG_IDENTITY_RESERVATION_LOCK,
        format!(".{CONFIG_IDENTITY_RESERVATION_NAME}.lock")
    );
    assert_eq!(
        CONFIG_IDENTITY_RESERVATION_TMP,
        format!(".{CONFIG_IDENTITY_RESERVATION_NAME}.*.tmp")
    );
    for name in [
        CONFIG_IDENTITY_RESERVATION_NAME,
        CONFIG_IDENTITY_RESERVATION_LOCK,
        CONFIG_IDENTITY_RESERVATION_TMP,
    ] {
        assert_eq!(
            INSTANCE_ARTIFACTS
                .iter()
                .filter(|row| row.name == name && row.disposition == Disposition::Ignore)
                .count(),
            1
        );
    }
    // Per-target any-depth rows are appended by the PAIR owner. The
    // generated instance registry keeps its existing anchored policy.
    exactly_one_any_depth_row_exists();
}

#[test]
fn prepared_identity_journal_names_and_ignore_registry() {
    ignore_rows_are_unique_and_byte_sorted_by_name();
    assert_eq!(
        IDENTITY_TRANSITION_STATE_NAME,
        "coding-agent-identity-transition.state.no-git.json"
    );
    assert_eq!(
        IDENTITY_TRANSITION_LOCK_NAME,
        format!(".{IDENTITY_TRANSITION_STATE_NAME}.lock")
    );
    assert_eq!(
        IDENTITY_TRANSITION_TMP_GLOB,
        format!(".{IDENTITY_TRANSITION_STATE_NAME}.*.tmp")
    );
    for name in [
        IDENTITY_TRANSITION_STATE_NAME,
        IDENTITY_TRANSITION_LOCK_NAME,
        IDENTITY_TRANSITION_TMP_GLOB,
    ] {
        let row = INSTANCE_ARTIFACTS
            .iter()
            .find(|row| row.name == name)
            .expect("private artifact registered");
        assert_eq!(row.disposition, Disposition::Ignore);
    }
}

const COMMENT_PREFIX: &str = "# AgentsCommander: ";
const GIT_WILDCARDS: [char; 4] = ['*', '?', '[', ']'];

fn ignore_rows() -> Vec<&'static InstanceArtifact> {
    INSTANCE_ARTIFACTS
        .iter()
        .filter(|artifact| artifact.disposition == Disposition::Ignore)
        .collect()
}

#[test]
fn ignore_rows_are_unique_and_byte_sorted_by_name() {
    let names: Vec<&str> = ignore_rows().iter().map(|row| row.name).collect();
    for window in names.windows(2) {
        assert!(
            window[0].as_bytes() < window[1].as_bytes(),
            "Ignore rows must be strictly increasing in byte order of name, \
             which keeps the generated file deterministic and gives every new \
             row exactly one correct position: {:?} is not before {:?}",
            window[0],
            window[1]
        );
    }
}

/// #2133 - pins the overlay row between its two neighbours, so a later re-sort cannot
/// quietly relocate it.
#[test]
fn the_local_artifact_name_is_an_ignore_row_in_sorted_position() {
    let names: Vec<&str> = ignore_rows().iter().map(|row| row.name).collect();
    let position = |name: &str| {
        let hits: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, row)| **row == name)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(hits.len(), 1, "{name} must appear exactly once: {hits:?}");
        hits[0]
    };
    let local = position("agent-help.local.json");
    assert!(position("agency-agents_templates.*") < local);
    assert!(local < position("api-audit.log"));
}

/// #2133 - the shipped row sorts before the local one; a later re-sort must keep that.
#[test]
fn the_shipped_artifact_name_is_an_ignore_row_before_the_local_one() {
    let names: Vec<&str> = ignore_rows().iter().map(|row| row.name).collect();
    let hits: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, row)| **row == "agent-help.json")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "agent-help.json must appear exactly once: {hits:?}"
    );
    let index_of = |name: &str| names.iter().position(|row| *row == name).unwrap();
    assert!(index_of("agency-agents_templates.*") < hits[0]);
    assert!(hits[0] < index_of("agent-help.local.json"));
}

/// #2133 - the two remote rows are split around P2's and P3's rows by byte order.
#[test]
fn both_remote_artifact_names_are_ignore_rows_in_sorted_position() {
    let names: Vec<&str> = ignore_rows().iter().map(|row| row.name).collect();
    let once = |name: &str| {
        let hits: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, row)| **row == name)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(hits.len(), 1, "{name} must appear exactly once: {hits:?}");
        hits[0]
    };
    let check = once("agent-help-remote-check.json");
    let remote = once("agent-help.remote.json");
    assert!(check < once("agent-help.json"));
    assert!(once("agent-help.local.json") < remote);
    assert!(remote < once("api-audit.log"));
}

#[test]
fn every_row_has_a_nonempty_single_line_comment() {
    for artifact in INSTANCE_ARTIFACTS {
        assert!(
            artifact.comment.starts_with(COMMENT_PREFIX),
            "comment of {:?} must start with {COMMENT_PREFIX:?}",
            artifact.name
        );
        assert!(
            artifact.comment.len() > COMMENT_PREFIX.len(),
            "comment of {:?} says nothing beyond the prefix",
            artifact.name
        );
        assert!(
            !artifact.comment.contains('\n') && !artifact.comment.contains('\r'),
            "comment of {:?} must be a single line",
            artifact.name
        );
    }
}

#[test]
fn no_name_contains_slash_backslash_newline_or_leading_slash_or_bang() {
    for artifact in INSTANCE_ARTIFACTS {
        let name = artifact.name;
        assert!(!name.is_empty(), "a row has an empty name");
        assert!(
            !name.starts_with('/') && !name.starts_with('!'),
            "name {name:?} must not decide its own anchoring or negate: rendering \
             is the only place a leading slash is added, and a generated `!` rule \
             is impossible by construction"
        );
        assert!(
            !name.contains('!'),
            "name {name:?} must not contain a negation marker"
        );

        // A slash is legal for exactly one kind, and only inside the pinned
        // `coding-agents/` child allowlist with real single-segment children.
        if artifact.kind == ArtifactKind::RootRelativeGlob {
            assert!(
                ROOT_RELATIVE_GLOB_ALLOWLIST.contains(&name),
                "RootRelativeGlob {name:?} is outside the pinned coding-agents allowlist"
            );
            let mut segments = name.split('/');
            assert_eq!(
                segments.next(),
                Some(CODING_AGENTS_CATALOG_DIR_NAME),
                "RootRelativeGlob {name:?} must be rooted at {CODING_AGENTS_CATALOG_DIR_NAME:?}"
            );
            let children: Vec<&str> = segments.collect();
            assert_eq!(
                children.len(),
                1,
                "RootRelativeGlob {name:?} must name exactly one child of the catalog directory"
            );
            let child = children[0];
            assert!(
                !child.is_empty() && child != "." && child != "..",
                "RootRelativeGlob {name:?} has an empty, dot or dotdot child segment"
            );
            for forbidden in ['\\', '\n', '\r'] {
                assert!(
                    !name.contains(forbidden),
                    "name {name:?} must not contain {forbidden:?}"
                );
            }
        } else {
            for forbidden in ['/', '\\', '\n', '\r'] {
                assert!(
                    !name.contains(forbidden),
                    "name {name:?} must not contain {forbidden:?}"
                );
            }
        }
    }
}

#[test]
fn root_relative_glob_rows_are_exactly_the_pinned_allowlist() {
    let rows: Vec<&str> = INSTANCE_ARTIFACTS
        .iter()
        .filter(|artifact| artifact.kind == ArtifactKind::RootRelativeGlob)
        .map(|artifact| artifact.name)
        .collect();
    assert_eq!(
        rows, ROOT_RELATIVE_GLOB_ALLOWLIST,
        "the kind's rows must equal the pinned allowlist in table order"
    );
}

#[test]
fn root_relative_glob_artifact_names_derive_from_their_filenames() {
    for (artifact, file_name) in [
        (CODING_AGENTS_LOCAL_ARTIFACT, CODING_AGENTS_LOCAL_FILENAME),
        (
            CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
            CODING_AGENTS_MIGRATION_BACKUP_FILENAME,
        ),
        (
            CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
            CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
        ),
        (CODING_AGENTS_LOCK_ARTIFACT, CODING_AGENTS_LOCK_FILENAME),
        (CODING_AGENTS_BASE_TMP_ARTIFACT, CODING_AGENTS_BASE_TMP_GLOB),
        (
            CODING_AGENTS_LOCAL_TMP_ARTIFACT,
            CODING_AGENTS_LOCAL_TMP_GLOB,
        ),
        (
            CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
            CODING_AGENTS_MIGRATION_BACKUP_TMP_GLOB,
        ),
        (
            CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
            CODING_AGENTS_MIGRATION_JOURNAL_TMP_GLOB,
        ),
    ] {
        assert_eq!(
            artifact,
            format!("{CODING_AGENTS_CATALOG_DIR_NAME}/{file_name}"),
            "the table name must be the catalog directory plus the file-name constant"
        );
    }
}

#[test]
fn no_file_or_dir_row_contains_a_git_wildcard() {
    for artifact in INSTANCE_ARTIFACTS {
        if !matches!(artifact.kind, ArtifactKind::File | ArtifactKind::Dir) {
            continue;
        }
        for wildcard in GIT_WILDCARDS {
            assert!(
                !artifact.name.contains(wildcard),
                "{:?} carries the git wildcard {wildcard:?} but is declared {:?}; \
                 a row with a wildcard has to choose a glob kind",
                artifact.name,
                artifact.kind
            );
        }
    }
}

#[test]
fn message_bus_glob_derives_from_the_db_name() {
    assert_eq!(MESSAGE_BUS_DB_GLOB, format!("{MESSAGE_BUS_DB_FILENAME}*"));
}

#[test]
fn coordinator_clocks_tmp_glob_derives_from_the_clocks_file_name() {
    assert_eq!(
        COORDINATOR_CLOCKS_TMP_GLOB,
        format!("{COORDINATOR_CLOCKS_FILE_NAME}.*.tmp")
    );
}

#[test]
fn atomic_write_tmp_predicate_agrees_with_its_glob() {
    assert_eq!(ATOMIC_WRITE_TMP_GLOB, ".*.*.tmp");
    for positive in [".settings.json.1.tmp", ".a.b.tmp"] {
        assert!(
            matches_atomic_write_tmp_glob(positive),
            "{positive:?} is produced by the atomic config writer and must match"
        );
    }
    // `.api-clients-x.tmp` is the last negative on purpose: it documents why
    // the api-clients temporaries need a row of their own.
    for negative in ["foo.tmp", ".foo.tmp", ".a.tmpx", ".api-clients-x.tmp"] {
        assert!(
            !matches_atomic_write_tmp_glob(negative),
            "{negative:?} is outside the glob and must not match"
        );
    }
}

#[test]
fn agency_templates_sibling_glob_derives_from_the_templates_dir() {
    // This is what keeps the sibling rule and the tracked cache directory
    // from drifting apart: the glob is provably the tracked name plus a
    // literal dot, so it cannot be widened into the row it must not touch.
    assert_eq!(
        AGENCY_TEMPLATES_SIBLING_GLOB,
        format!("{AGENCY_TEMPLATES_DIR}.*")
    );
}

#[test]
fn rotation_globs_derive_from_their_live_artifact_names() {
    // A table rather than three near-identical tests, so a fifth rotating
    // artifact adds a row instead of a test. `app.log.*` is deliberately
    // absent: its live row is a table literal whose writer this change
    // leaves alone, so there is no constant to derive it from.
    for (rotation_glob, live_name) in [
        (ACTIVITY_LOG_ROTATION_GLOB, ACTIVITY_LOG_FILE_NAME),
        (API_AUDIT_LOG_ROTATION_GLOB, API_AUDIT_LOG_FILE_NAME),
        (ORPHAN_ARCHIVE_ROTATION_GLOB, ORPHAN_ARCHIVE_FILENAME),
    ] {
        assert_eq!(
            rotation_glob,
            format!("{live_name}.*"),
            "the rotation glob of {live_name:?} must be its live name plus a \
             suffix wildcard: the writer composes the generation number at \
             runtime, so this derivation is the only tie between the rule and \
             the artifact it covers"
        );
    }
}

#[test]
fn global_context_retired_backup_glob_derives_from_the_context_filename() {
    assert_eq!(
        GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB,
        format!("{GLOBAL_CONTEXT_TEMPLATE_FILENAME}.retired-*.bak")
    );
}

#[test]
fn settings_backup_rotation_glob_derives_from_its_composition_constants() {
    assert_eq!(
        SETTINGS_BACKUP_ROTATION_GLOB,
        format!("{SETTINGS_BACKUP_PREFIX}*{SETTINGS_BACKUP_SUFFIX}")
    );
}

#[test]
fn the_384_v1_name_matches_its_glob() {
    assert_eq!(
        SETTINGS_MIGRATION_BACKUP_384_V1_NAME,
        SETTINGS_MIGRATION_BACKUP_GLOB.replace('*', "384-v1")
    );
}

#[test]
fn the_settings_tmp_glob_derives_from_the_settings_name() {
    assert_eq!(SETTINGS_TMP_GLOB, format!("{SETTINGS_FILE_NAME}.*.tmp"));
}

#[test]
fn project_paths_artifacts_cover_publication_and_recovery() {
    assert_eq!(
        PROJECT_PATHS_TMP_GLOB,
        format!("{PROJECT_PATHS_FILE_NAME}.*.tmp")
    );
    assert_eq!(
        PROJECT_PATHS_BACKUP_TMP_GLOB,
        format!("{PROJECT_PATHS_BACKUP_FILE_NAME}.*.tmp")
    );
    assert_eq!(
        PROJECT_PATHS_RECOVERED_GLOB,
        "project-paths.pre-migration.recovered-*.no-git.json"
    );
    for name in [
        PROJECT_PATHS_FILE_NAME,
        PROJECT_PATHS_TMP_GLOB,
        PROJECT_PATHS_BACKUP_FILE_NAME,
        PROJECT_PATHS_BACKUP_TMP_GLOB,
        PROJECT_PATHS_RECOVERED_GLOB,
    ] {
        let rows: Vec<_> = INSTANCE_ARTIFACTS
            .iter()
            .filter(|row| row.name == name)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].disposition, Disposition::Ignore);
    }
}

#[test]
fn the_agents_instance_tmp_glob_derives_from_the_agents_instance_name() {
    assert_eq!(
        AGENTS_INSTANCE_TMP_GLOB,
        format!("{AGENTS_INSTANCE_FILE_NAME}.*.tmp")
    );
}

#[test]
fn the_retired_lock_row_is_the_pre_migration_literal() {
    assert_eq!(SETTINGS_RETIRED_LOCK_NAME, "settings.json.lock");
    assert_ne!(SETTINGS_RETIRED_LOCK_NAME, SETTINGS_LOCK_FILE_NAME);
    assert!(INSTANCE_ARTIFACTS
        .iter()
        .any(|row| row.name == SETTINGS_RETIRED_LOCK_NAME
            && row.kind == ArtifactKind::File
            && row.disposition == Disposition::Ignore));
}

#[test]
fn the_config_state_name_is_composed_from_the_rule() {
    assert_eq!(
        CONFIG_STATE_TARGET_NAME,
        format!("config.{STATE_MARKER}.{NO_GIT_MARKER}.json")
    );
}

#[test]
fn naming_migration_state_name_is_the_layered_state_no_git_journal_name() {
    assert_eq!(
        NAMING_MIGRATION_STATE_NAME,
        format!("naming-migration.{STATE_MARKER}.{NO_GIT_MARKER}.json")
    );
}

#[test]
fn naming_migration_state_tmp_glob_derives_from_the_state_name() {
    assert_eq!(
        NAMING_MIGRATION_STATE_TMP_GLOB,
        format!("{NAMING_MIGRATION_STATE_NAME}.*.tmp")
    );
}

#[test]
fn naming_migration_lock_name_derives_from_the_state_name() {
    assert_eq!(
        NAMING_MIGRATION_LOCK_NAME,
        format!(".{NAMING_MIGRATION_STATE_NAME}.lock")
    );
}

#[test]
fn exactly_one_any_depth_row_exists() {
    let any_depth: Vec<&str> = INSTANCE_ARTIFACTS
        .iter()
        .filter(|artifact| matches!(artifact.kind, ArtifactKind::GlobAnyDepth))
        .map(|artifact| artifact.name)
        .collect();
    assert_eq!(
        any_depth,
        vec![ATOMIC_WRITE_TMP_GLOB],
        "depth independence is a policy decision, not a table tweak: every other \
         rule is anchored to the instance root, and widening that to a second \
         pattern has to be argued rather than added"
    );
}

/// #2717 (B4a) - two artifacts with independent reasons to be layer 50 are
/// each named from the rule, so the collision is intended, not accidental.
#[test]
fn project_settings_and_settings_local_share_the_layer_50_name() {
    assert_eq!(PROJECT_SETTINGS_TARGET_NAME, SETTINGS_LOCAL_TARGET_NAME);
    assert_eq!(
        PROJECT_SETTINGS_TARGET_NAME,
        "settings.50.personal.no-git.json"
    );
}

/// #2718 (B4b) E5: the Loop state write temporary derives from its name.
#[test]
fn loop_state_tmp_glob_derives_from_the_loop_state_name() {
    assert_eq!(
        LOOP_STATE_TMP_TARGET_GLOB,
        format!("{LOOP_STATE_TARGET_NAME}.*.tmp")
    );
    assert_eq!(LOOP_STATE_TARGET_NAME, "loop.state.no-git.json");
}

/// #2994 (P41) - the owner's lifecycle sidecar and endpoint metadata are Ignore
/// rows in sorted position, distinct from the transactional lock, and leave
/// every existing row in place.
#[test]
fn e2992_p41_owner_ignore() {
    ignore_rows_are_unique_and_byte_sorted_by_name();
    assert_eq!(SETTINGS_OWNER_LOCK_FILE_NAME, "settings.owner.no-git.lock");
    assert_eq!(
        SETTINGS_OWNER_ENDPOINT_FILE_NAME,
        "settings-owner.endpoint.no-git.json"
    );
    assert_ne!(SETTINGS_OWNER_LOCK_FILE_NAME, SETTINGS_LOCK_FILE_NAME);
    assert_ne!(SETTINGS_OWNER_LOCK_FILE_NAME, SETTINGS_RETIRED_LOCK_NAME);
    for name in [
        SETTINGS_OWNER_LOCK_FILE_NAME,
        SETTINGS_OWNER_ENDPOINT_FILE_NAME,
    ] {
        let rows: Vec<&InstanceArtifact> = INSTANCE_ARTIFACTS
            .iter()
            .filter(|row| row.name == name)
            .collect();
        assert_eq!(rows.len(), 1, "{name} must have exactly one row");
        assert_eq!(rows[0].kind, ArtifactKind::File);
        assert_eq!(rows[0].disposition, Disposition::Ignore);
    }
    // Neighbouring rows survive unchanged: the transactional lock and the
    // retired lock are still Ignore rows of their own.
    for name in [
        SETTINGS_LOCK_FILE_NAME,
        SETTINGS_RETIRED_LOCK_NAME,
        "sessions.json",
    ] {
        assert!(ignore_rows().iter().any(|row| row.name == name), "{name}");
    }
}
