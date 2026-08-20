//! The registry of artifacts the running instance writes into its own config
//! directory, and the disposition of each one.
//!
//! This module is the single source for the generated instance `.gitignore`
//! (#1446): `instance_gitignore::required_rules` derives every static rule from
//! the table below, so a rule can no longer be a retyped literal that drifts
//! away from the writer that produces the file. Writer modules define their
//! artifact-name constants as aliases of the constants declared here, so a
//! rename in this table breaks their build instead of silently reopening the
//! coverage gap.
//!
//! Two properties of this module are load-bearing and must survive every future
//! edit:
//!
//! 1. **It is a leaf.** Production code here makes no reference to anything
//!    outside the module: no `crate::` path, no anchor one level up, no
//!    third-party import. A module with zero outgoing arcs is a trivial SCC no
//!    matter how many modules point at it, so every arc into this registry is
//!    safe regardless of whether its source sits inside the crate's cyclic
//!    component. That is the same argument #1273 established for the parent
//!    module, and `tests/instance_gitignore_layering.rs` asserts it by equality
//!    against empty tables. The one exception the guard allows is the glob
//!    import of the single test module at the bottom of this file, which the arc
//!    record never sees because it is emitted without test code.
//! 2. **Every child of the instance config directory has a row here**, `Ignore`
//!    or `Track`. A new artifact needs a new row in the same change that
//!    introduces it. Nothing detects an omission mechanically, which is why the
//!    rule is written down: "not in the registry" must never again mean "nobody
//!    looked".
//!
//!    That claim rests entirely on the enumeration recipe of the #1446 plan
//!    (`plans/1446-instance-gitignore-artifact-registry.md`, Section 7), which
//!    is the procedure that produced this inventory and the only way to keep it
//!    true. Its four legs exist because narrower sweeps missed real writers: leg
//!    1 collects every way production obtains the instance directory, including
//!    wrapper functions and parameters, not only literal `config_dir()` calls;
//!    leg 2 finds publications file-scoped over that set, with no line window,
//!    because a join can sit far below its root binding; leg 3 finds sibling
//!    naming (`with_extension`, `with_file_name`, `parent.join`), which no
//!    root-anchored sweep can see and which is how the rotated generations
//!    stayed hidden through three inventories; leg 4 lists the real children of
//!    at least two live instance directories as ground truth. Its stop-the-line
//!    criterion is the other half: every candidate must be either matched by a
//!    row, nested under a row that covers its subtree or rooted outside the
//!    instance directory, or recorded in the plan as a deliberate exclusion. A
//!    candidate in none of the three halts the change and becomes a product
//!    question. Re-run the recipe when adding an artifact, and repair this claim
//!    by adding rows, never by softening the sentence.

/// What the policy does with an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Emitted as a rule in the generated instance .gitignore.
    Ignore,
    /// Deliberately tracked; never emitted. The row documents the decision
    /// and feeds the fixture's control paths.
    Track,
}

/// How a row's name renders into a .gitignore pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    /// Renders as `/{name}`.
    File,
    /// Renders as `/{name}/`. The trailing slash is load-bearing: it keeps a
    /// plain file of the same name out of the rule.
    Dir,
    /// Renders as `/{name}`; the name carries git wildcard characters.
    Glob,
    /// Renders as `{name}`, unanchored, so git applies it at every depth under
    /// the instance directory. Exceptional by design; a registry test pins the
    /// number of such rows.
    GlobAnyDepth,
}

/// One artifact of the instance config directory.
pub(crate) struct InstanceArtifact {
    /// File name, directory name or glob. Never carries a leading slash.
    pub(crate) name: &'static str,
    pub(crate) kind: ArtifactKind,
    pub(crate) disposition: Disposition,
    /// The single `# AgentsCommander: ...` line emitted above the pattern.
    pub(crate) comment: &'static str,
}

// ---------------------------------------------------------------------------
// Artifact names shared with the modules that write them
// ---------------------------------------------------------------------------

/// Temporaries published by the atomic config writer, which names them
/// `.{file_name}.{pid}.tmp`. Emitted unanchored: correct writers publish
/// through subdirectories of the instance dir too, including directories this
/// registry deliberately tracks, and an anchored rule would leave those
/// leftovers visible in `git status` on every write (#1209, instance-dir half).
pub(crate) const ATOMIC_WRITE_TMP_GLOB: &str = ".*.*.tmp";
pub(crate) const SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME: &str =
    ".agentscommander-context-templates.json";
pub(crate) const ACTIVITY_LOG_FILE_NAME: &str = "activity.jsonl";
/// Rotated generations of the activity log. The rotation writer composes the
/// numeric suffix at runtime, so no constant can express a concrete generation;
/// a registry test derives this glob from `ACTIVITY_LOG_FILE_NAME`.
pub(crate) const ACTIVITY_LOG_ROTATION_GLOB: &str = "activity.jsonl.*";
/// Transient siblings of the agency template cache: its lock and the three
/// staging trees (`.next-`, `.download-`, `.prev-`). The literal dot is the
/// whole mechanism keeping this rule off the suffix-less cache directory, which
/// is a `Track` row; a registry test derives the glob from `AGENCY_TEMPLATES_DIR`
/// so it can never be widened into the row it must not touch.
pub(crate) const AGENCY_TEMPLATES_SIBLING_GLOB: &str = "agency-agents_templates.*";
pub(crate) const API_AUDIT_LOG_FILE_NAME: &str = "api-audit.log";
/// Rotated generations of the API audit log; same shape and same reason as
/// `ACTIVITY_LOG_ROTATION_GLOB`.
pub(crate) const API_AUDIT_LOG_ROTATION_GLOB: &str = "api-audit.log.*";
pub(crate) const API_CLIENTS_REGISTRY_FILENAME: &str = "api-clients.json";
pub(crate) const API_CLIENTS_LOCK_FILENAME: &str = "api-clients.lock";
// No table row carries this name on its own: the glob below covers the database
// and its sidecars in one rule. Its readers are the message store's own
// `DB_FILENAME` alias and the derivation test.
pub(crate) const MESSAGE_BUS_DB_FILENAME: &str = "api-message-bus.sqlite3";
/// Covers the database and every sidecar SQLite can produce (`-shm`, `-wal`,
/// `-journal`), which is why it is a glob and not three literals. A registry
/// test derives it from `MESSAGE_BUS_DB_FILENAME`.
pub(crate) const MESSAGE_BUS_DB_GLOB: &str = "api-message-bus.sqlite3*";
pub(crate) const CODEX_HOME_DIR_NAME: &str = "codex-home";
/// The CLI-to-app coding-agent mutation request queue. Its `results/`
/// subdirectory stays owner-side: the `Dir` row covers the whole subtree in one
/// rule, the same stance `harness.log` gets under the `logs` row.
pub(crate) const CODING_AGENT_REQUESTS_DIR_NAME: &str = "coding-agent-requests";
pub(crate) const CONTEXT_CACHE_DIR_NAME: &str = "context-cache";
pub(crate) const COORDINATOR_CLOCKS_FILE_NAME: &str = "coordinator_clocks.json";
/// The clocks writer names its temporary by replacing the extension with
/// `json.{pid}.{seq}.tmp`, so the name has no leading dot and the atomic-write
/// glob cannot match it. A registry test derives this glob from
/// `COORDINATOR_CLOCKS_FILE_NAME`.
pub(crate) const COORDINATOR_CLOCKS_TMP_GLOB: &str = "coordinator_clocks.json.*.tmp";
pub(crate) const DEBUG_LOGS_FILE_NAME: &str = "debug-logs.txt";
pub(crate) const TELEGRAM_DIAG_RAW_LOG_FILE_NAME: &str = "diag-raw.log";
pub(crate) const TELEGRAM_DIAG_SENT_LOG_FILE_NAME: &str = "diag-sent.log";
pub(crate) const GIT_GUARD_DIR_NAME: &str = "git-guard";
pub(crate) const INSTANCES_DIR_NAME: &str = "instances";
pub(crate) const LOGS_DIR_NAME: &str = "logs";
pub(crate) const ORPHAN_ARCHIVE_FILENAME: &str = "orphaned-sessions.archive.json";
/// Rotated generations of the orphaned-session archive; same shape and same
/// reason as `ACTIVITY_LOG_ROTATION_GLOB`.
pub(crate) const ORPHAN_ARCHIVE_ROTATION_GLOB: &str = "orphaned-sessions.archive.json.*";
pub(crate) const PROJECT_REFRESH_REQUESTS_DIR_NAME: &str = "project-refresh-requests";
pub(crate) const PTY_INPUT_LOCKS_DIR_NAME: &str = "pty-input-locks";
pub(crate) const SESSION_REQUESTS_DIR_NAME: &str = "session-requests";
pub(crate) const SETTINGS_LOCK_FILE_NAME: &str = "settings.json.lock";
/// Covers every settings migration backup instance. The concrete names are
/// composed by their own migrations, so this glob is registry-owned and no
/// writer imports it.
pub(crate) const SETTINGS_MIGRATION_BACKUP_GLOB: &str = "settings.pre-*.json";
pub(crate) const TELEGRAM_BRIDGE_LOG_FILE_NAME: &str = "telegram-bridge.log";
pub(crate) const UI_AUTOMATION_DIR_NAME: &str = "ui-automation";
pub(crate) const ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME: &str = "Context.root-agent.md";
/// The live standalone global context template. Production retires it in place
/// and never recreates it, and the entry holds the user's own bytes.
pub(crate) const GLOBAL_CONTEXT_TEMPLATE_FILENAME: &str = "Context.AgentsCommander.md";
/// Retirement backups of the standalone global context. The writer composes the
/// timestamp (and a disambiguating index) at runtime from the live entry's own
/// `file_name()`, so no constant can produce a concrete name; a registry test
/// derives this glob from `GLOBAL_CONTEXT_TEMPLATE_FILENAME`.
pub(crate) const GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB: &str =
    "Context.AgentsCommander.md.retired-*.bak";
pub(crate) const AGENCY_TEMPLATES_DIR: &str = "agency-agents_templates";
pub(crate) const AGENT_TEMPLATES_DIR_NAME: &str = "agent-templates";
pub(crate) const CODING_AGENTS_CATALOG_DIR_NAME: &str = "coding-agents";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Every artifact of the instance config directory, `Ignore` rows first in
/// strict byte order of `name`, then the deliberately tracked ones.
///
/// The byte order keeps the generated file deterministic and makes the table
/// append-proof: a new row has exactly one correct position and a registry test
/// refuses any other. Rows whose name is not one of the constants above are
/// either rules that predate this registry, kept byte-identical so no existing
/// generated file is rewritten, or names whose writer is deliberately left
/// alone and guarded by the git fixture instead.
pub(crate) const INSTANCE_ARTIFACTS: &[InstanceArtifact] = &[
    InstanceArtifact {
        name: ATOMIC_WRITE_TMP_GLOB,
        kind: ArtifactKind::GlobAnyDepth,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient atomic-write temporaries (.{name}.{pid}.tmp) at any depth; survive only a crash mid-write",
    },
    InstanceArtifact {
        name: SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: seeded context-template ownership state",
    },
    InstanceArtifact {
        name: ".agentscommander-injected-messages.json",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: injected-messages ownership state",
    },
    InstanceArtifact {
        name: ".api-clients-*.tmp",
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient API client registry write temporaries",
    },
    InstanceArtifact {
        name: ACTIVITY_LOG_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: append-only working-state activity log",
    },
    InstanceArtifact {
        name: ACTIVITY_LOG_ROTATION_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: rotated generations of the activity log; the same runtime artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: AGENCY_TEMPLATES_SIBLING_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient agency template cache lock and staging trees (.lock, .next-, .download-, .prev-); the tracked cache directory itself carries no dot and is not matched",
    },
    InstanceArtifact {
        name: API_AUDIT_LOG_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: append-only API audit log",
    },
    InstanceArtifact {
        name: API_AUDIT_LOG_ROTATION_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: rotated generations of the API audit log; the same runtime artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: API_CLIENTS_REGISTRY_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: local API client registry",
    },
    InstanceArtifact {
        name: API_CLIENTS_LOCK_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: persistent API client registry write lock",
    },
    InstanceArtifact {
        name: MESSAGE_BUS_DB_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: inter-agent message bus database and every SQLite sidecar (-shm, -wal, -journal)",
    },
    InstanceArtifact {
        name: "app-outbox-path.txt",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: runtime outbox path handshake file",
    },
    InstanceArtifact {
        name: "app.log",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: application log",
    },
    InstanceArtifact {
        // Table literal: `app.log` itself is a pre-registry rule whose writer
        // this change deliberately leaves alone, so there is no constant to
        // derive this glob from and inventing one no writer imports would add a
        // third list rather than a tie.
        name: "app.log.*",
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: rotated generations of the application log; the same runtime artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: CODEX_HOME_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: per-agent isolated coding-agent home trees",
    },
    InstanceArtifact {
        name: CODING_AGENT_REQUESTS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: CLI-to-app coding-agent mutation request queue, including its results/ subdirectory",
    },
    InstanceArtifact {
        name: CONTEXT_CACHE_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: regenerable per-session combined-context cache",
    },
    InstanceArtifact {
        name: COORDINATOR_CLOCKS_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: coordinator idle-clock runtime state",
    },
    InstanceArtifact {
        name: COORDINATOR_CLOCKS_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient coordinator-clock write temporaries",
    },
    InstanceArtifact {
        name: "daemon.pid",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: daemon process id",
    },
    InstanceArtifact {
        name: DEBUG_LOGS_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: on-demand debug log dump",
    },
    InstanceArtifact {
        name: TELEGRAM_DIAG_RAW_LOG_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: Telegram bridge raw diagnostics log",
    },
    InstanceArtifact {
        name: TELEGRAM_DIAG_SENT_LOG_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: Telegram bridge sent diagnostics log",
    },
    InstanceArtifact {
        name: GIT_GUARD_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: generated git-guard shim scripts",
    },
    InstanceArtifact {
        name: "injected-messages.default.toml",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: injected-messages reference defaults",
    },
    InstanceArtifact {
        name: "injected-messages.toml",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: injected-messages configuration",
    },
    InstanceArtifact {
        name: "injected-messages.toml.bak-*",
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: injected-messages migration backups",
    },
    InstanceArtifact {
        name: INSTANCES_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: per-instance runtime state directories",
    },
    InstanceArtifact {
        name: LOGS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: harness policy log directory",
    },
    InstanceArtifact {
        name: "master-token.txt",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: local API master token",
    },
    InstanceArtifact {
        name: ORPHAN_ARCHIVE_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: archived orphaned-session records",
    },
    InstanceArtifact {
        name: ORPHAN_ARCHIVE_ROTATION_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: rotated generations of the archived orphaned-session records; the same runtime artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: PROJECT_REFRESH_REQUESTS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: project refresh request queue",
    },
    InstanceArtifact {
        name: PTY_INPUT_LOCKS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient cross-process PTY input locks",
    },
    InstanceArtifact {
        name: SESSION_REQUESTS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: CLI-to-app session launch request queue",
    },
    InstanceArtifact {
        name: "sessions.json",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: persisted session state",
    },
    InstanceArtifact {
        name: "settings.json",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: application settings",
    },
    InstanceArtifact {
        name: SETTINGS_LOCK_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient settings write lock",
    },
    InstanceArtifact {
        name: SETTINGS_MIGRATION_BACKUP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: settings migration backups",
    },
    InstanceArtifact {
        name: TELEGRAM_BRIDGE_LOG_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: Telegram bridge log",
    },
    InstanceArtifact {
        name: UI_AUTOMATION_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: UI-automation session handshake state",
    },
    InstanceArtifact {
        name: "update-check.json",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: update-check cache",
    },
    InstanceArtifact {
        name: "update-check.json.tmp",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient update-check write temporary",
    },
    InstanceArtifact {
        name: "web-token.txt",
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: local web token",
    },
    InstanceArtifact {
        name: GLOBAL_CONTEXT_TEMPLATE_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: legacy standalone global context template; production retires it in place and never recreates it, and the entry holds user bytes; tracked on purpose",
    },
    InstanceArtifact {
        name: GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: retirement backups of the standalone global context; user bytes, tracked on purpose",
    },
    InstanceArtifact {
        name: ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: user-editable root-agent context template; tracked on purpose",
    },
    InstanceArtifact {
        name: "ac-root-agent",
        kind: ArtifactKind::Dir,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: canonical root-agent state (CLAUDE.md, memory, inbox); only its config.json rules are ignored",
    },
    InstanceArtifact {
        name: AGENCY_TEMPLATES_DIR,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: user-editable agency template sets; tracked on purpose",
    },
    InstanceArtifact {
        name: AGENT_TEMPLATES_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: user-editable role templates; tracked on purpose",
    },
    InstanceArtifact {
        name: CODING_AGENTS_CATALOG_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: user-configurable coding-agent catalog; tracked on purpose",
    },
];

/// Whether `file_name` is matched by `ATOMIC_WRITE_TMP_GLOB`.
///
/// The predicate lives next to the pattern so the writer-side tie test in
/// `local_config_io` can assert the policy itself instead of paraphrasing it: a
/// paraphrase does not follow the pattern when the pattern changes, which would
/// leave the tie the test is named for untied. A registry test pins the
/// agreement between the two on a table of positives and negatives.
///
/// `.*.*.tmp` is a leading dot, then anything, then a dot, then anything, then
/// the `.tmp` suffix, so the name needs a leading dot, the suffix, and at least
/// one dot between them.
#[allow(dead_code)] // called only from test code: the tie test and the agreement table
pub(crate) fn matches_atomic_write_tmp_glob(file_name: &str) -> bool {
    let Some(rest) = file_name.strip_prefix('.') else {
        return false;
    };
    let Some(middle) = rest.strip_suffix(".tmp") else {
        return false;
    };
    middle.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

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
            for forbidden in ['/', '\\', '\n', '\r'] {
                assert!(
                    !name.contains(forbidden),
                    "name {name:?} must not contain {forbidden:?}"
                );
            }
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
}
