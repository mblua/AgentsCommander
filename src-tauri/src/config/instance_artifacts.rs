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
//!    import of the single test module (`instance_artifacts/tests.rs`), which the arc
//!    record never sees because it is emitted without test code.
//! 2. **Every child of the instance config directory has a row here**, `Ignore`
//!    or `Track`. A new artifact needs a new row in the same change that
//!    introduces it. Nothing detects an omission mechanically, which is why the
//!    rule is written down: "not in the registry" must never again mean "nobody
//!    looked".
//!
//!    That claim rests entirely on the enumeration recipe of the #1446 plan
//!    (`.ac/plans/1446-instance-gitignore-artifact-registry.md`, Section 7), which
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

mod names;

#[cfg(test)]
pub(crate) use names::LAYERED_NAME_PROBES;
pub(crate) use names::{
    publication_temp_name_for_destination, ACTIVITY_LOG_FILE_NAME, ACTIVITY_LOG_ROTATION_GLOB,
    AGENCY_TEMPLATES_DIR, AGENCY_TEMPLATES_SIBLING_GLOB, AGENTS_INSTANCE_FILE_NAME,
    AGENTS_INSTANCE_TMP_GLOB, AGENT_HELP_LOCAL_FILE_NAME, AGENT_HELP_REMOTE_CHECK_FILE_NAME,
    AGENT_HELP_REMOTE_FILE_NAME, AGENT_HELP_SHIPPED_FILE_NAME, AGENT_TEMPLATES_DIR_NAME,
    API_AUDIT_LOG_FILE_NAME, API_AUDIT_LOG_ROTATION_GLOB, API_CLIENTS_LOCK_FILENAME,
    API_CLIENTS_REGISTRY_FILENAME, ATOMIC_WRITE_TMP_GLOB, BLOCKING_MENUS_LOCAL_FILE_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME,
    BLOCKING_MENUS_SHIPPED_FILE_NAME, CODEX_HOME_DIR_NAME, CODING_AGENTS_BASE_FILENAME,
    CODING_AGENTS_BASE_TMP_ARTIFACT, CODING_AGENTS_CATALOG_DIR_NAME, CODING_AGENTS_LOCAL_ARTIFACT,
    CODING_AGENTS_LOCAL_FILENAME, CODING_AGENTS_LOCAL_TMP_ARTIFACT, CODING_AGENTS_LOCK_ARTIFACT,
    CODING_AGENTS_LOCK_FILENAME, CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
    CODING_AGENTS_MIGRATION_BACKUP_FILENAME, CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
    CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT, CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
    CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT, CODING_AGENTS_PROJECT_TARGET_NAME,
    CODING_AGENTS_RETIRED_LOCK_ARTIFACT, CODING_AGENTS_SET_ASIDE_ARTIFACT,
    CODING_AGENT_REQUESTS_DIR_NAME, CONFIG_IDENTITY_RESERVATION_LOCK,
    CONFIG_IDENTITY_RESERVATION_NAME, CONFIG_IDENTITY_RESERVATION_TMP, CONFIG_STATE_TARGET_NAME,
    CONTEXT_CACHE_DIR_NAME, COORDINATOR_CLOCKS_FILE_NAME, COORDINATOR_CLOCKS_TMP_GLOB,
    DEBUG_LOGS_FILE_NAME, GIT_GUARD_DIR_NAME, GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB,
    GLOBAL_CONTEXT_TEMPLATE_FILENAME, IDENTITY_TRANSITION_LOCK_NAME,
    IDENTITY_TRANSITION_STATE_NAME, IDENTITY_TRANSITION_TMP_GLOB, INSTANCES_DIR_NAME,
    LOCAL_MARKDOWN_OVERRIDE_GLOB, LOGS_DIR_NAME, LOOP_STATE_TARGET_NAME,
    LOOP_STATE_TMP_TARGET_GLOB, MESSAGE_BUS_DB_FILENAME, MESSAGE_BUS_DB_GLOB,
    NAMING_MIGRATION_LOCK_NAME, NAMING_MIGRATION_STATE_NAME, NAMING_MIGRATION_STATE_TMP_GLOB,
    ORPHAN_ARCHIVE_FILENAME, ORPHAN_ARCHIVE_ROTATION_GLOB, PROJECT_PATHS_BACKUP_FILE_NAME,
    PROJECT_PATHS_BACKUP_TMP_GLOB, PROJECT_PATHS_FILE_NAME, PROJECT_PATHS_RECOVERED_GLOB,
    PROJECT_PATHS_TMP_GLOB, PROJECT_REFRESH_REQUESTS_DIR_NAME, PROJECT_SETTINGS_TARGET_NAME,
    PTY_INPUT_LOCKS_DIR_NAME, REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME,
    ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME, SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME,
    SESSION_REQUESTS_DIR_NAME, SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_ROTATION_GLOB,
    SETTINGS_BACKUP_SUFFIX, SETTINGS_FILE_NAME, SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
    SETTINGS_LOCK_FILE_NAME, SETTINGS_MIGRATION_BACKUP_384_V1_NAME, SETTINGS_MIGRATION_BACKUP_GLOB,
    SETTINGS_OWNER_ENDPOINT_FILE_NAME, SETTINGS_OWNER_LOCK_FILE_NAME, SETTINGS_RETIRED_LOCK_NAME,
    SETTINGS_TMP_GLOB, SET_ASIDE_GLOB, SOURCE_CATALOG_BASE_LEDGER_NAME,
    SOURCE_CATALOG_LEDGER_ARTIFACT, SOURCE_CATALOG_LOCK_ARTIFACT,
    SOURCE_CATALOG_PERSONAL_LEDGER_NAME, SOURCE_CATALOG_PROJECT_LEDGER_NAME,
    SOURCE_REGISTERED_INSTANCE_LEDGER_NAME, SOURCE_REGISTERED_INSTANCE_LOCK,
    TELEGRAM_BRIDGE_LOG_FILE_NAME, TELEGRAM_DIAG_RAW_LOG_FILE_NAME,
    TELEGRAM_DIAG_SENT_LOG_FILE_NAME, UI_AUTOMATION_DIR_NAME,
};
#[cfg(test)]
pub(crate) use names::{
    AGENTS_INSTANCE_TARGET_NAME, BLOCKING_MENUS_LOCAL_TARGET_NAME,
    BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME, BLOCKING_MENUS_REMOTE_TARGET_NAME,
    BLOCKING_MENUS_SHIPPED_TARGET_NAME, CODING_AGENTS_BASE_TARGET_NAME,
    CODING_AGENTS_BASE_TMP_GLOB, CODING_AGENTS_LOCAL_TARGET_NAME, CODING_AGENTS_LOCAL_TMP_GLOB,
    CODING_AGENTS_LOCK_TARGET_NAME, CODING_AGENTS_MIGRATION_BACKUP_TMP_GLOB,
    CODING_AGENTS_MIGRATION_JOURNAL_TMP_GLOB, LAYER_DEFAULT, LAYER_INSTANCE, LAYER_PERSONAL,
    LAYER_PROJECT, LAYER_REMOTE, NO_GIT_MARKER, SETTINGS_BACKUP_ROTATION_TARGET_GLOB,
    SETTINGS_BACKUP_TARGET_PREFIX, SETTINGS_LOCAL_TARGET_NAME, SETTINGS_LOCK_TARGET_NAME,
    SETTINGS_MIGRATION_BACKUP_TARGET_GLOB, SETTINGS_TARGET_NAME, STATE_MARKER,
};

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
    /// #1968 - renders as `/{name}` where `name` is a full path RELATIVE to the
    /// instance root (`coding-agents/agents.local.json`). The only kind whose
    /// name carries a forward slash: the pattern is still anchored by the one
    /// leading slash the renderer adds, and a registry test pins the exact
    /// `coding-agents/` child allowlist rather than allowing arbitrary paths.
    RootRelativeGlob,
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
pub(crate) const RETAINED_PROMPTS: &str = "last-prompts.retained.json";

pub(crate) const INSTANCE_ARTIFACTS: &[InstanceArtifact] = &[
    InstanceArtifact {
        name: SET_ASIDE_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: files the naming migration set aside (<old \
        name>.deprecated-<n>.no-git); every byte kept, never committed",
    },
    InstanceArtifact {
        name: LOCAL_MARKDOWN_OVERRIDE_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: operator-owned .local.md context-template overrides; \
        machine-local by design and never created by AC",
    },
    InstanceArtifact {
        name: ATOMIC_WRITE_TMP_GLOB,
        kind: ArtifactKind::GlobAnyDepth,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: transient atomic-write temporaries (.{name}.{pid}.tmp) at any \
        depth; survive only a crash mid-write",
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
        name: IDENTITY_TRANSITION_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: private identity intent journal write temporaries",
    },
    InstanceArtifact {
        name: IDENTITY_TRANSITION_LOCK_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: private identity intent journal lock; created once and never \
        deleted",
    },
    InstanceArtifact {
        name: SOURCE_REGISTERED_INSTANCE_LOCK,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: source coordination metadata and stable locks",
    },
    InstanceArtifact {
        name: CONFIG_IDENTITY_RESERVATION_TMP,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: PAIR reservation packet write temporaries",
    },
    InstanceArtifact {
        name: CONFIG_IDENTITY_RESERVATION_LOCK,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: stable PAIR reservation locks; never deleted",
    },
    InstanceArtifact {
        name: NAMING_MIGRATION_LOCK_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: naming-migration journal writer lock; created once and never \
        deleted",
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
        comment: "# AgentsCommander: rotated generations of the activity log; the same runtime \
        artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: AGENCY_TEMPLATES_SIBLING_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient agency template cache lock and staging trees \
        (.lock, .next-, .download-, .prev-); the tracked cache directory itself carries no dot and \
        is not matched",
    },
    InstanceArtifact {
        name: AGENT_HELP_REMOTE_CHECK_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: remote per-agent help download throttle stamp",
    },
    InstanceArtifact {
        name: AGENT_HELP_SHIPPED_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: shipped per-agent help content; rewritten from the binary at \
        every start",
    },
    InstanceArtifact {
        name: AGENT_HELP_LOCAL_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: operator-owned per-agent help overlay; machine-local by \
        design",
    },
    InstanceArtifact {
        name: AGENT_HELP_REMOTE_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: downloaded per-agent help content; replaced by the next \
        accepted download",
    },
    InstanceArtifact {
        name: AGENTS_INSTANCE_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: agents and coding-agent profiles moved out of the settings \
        file",
    },
    InstanceArtifact {
        name: AGENTS_INSTANCE_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient agents-file write temporaries \
        ({name}.{pid}.{op}.tmp); survive only a crash mid-save",
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
        comment: "# AgentsCommander: rotated generations of the API audit log; the same runtime \
        artifact under a numeric suffix",
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
        comment: "# AgentsCommander: inter-agent message bus database and every SQLite sidecar \
        (-shm, -wal, -journal)",
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
        comment: "# AgentsCommander: rotated generations of the application log; the same runtime \
        artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: BLOCKING_MENUS_SHIPPED_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: shipped blocking-menu patterns; rewritten from the binary at \
        every start",
    },
    InstanceArtifact {
        name: BLOCKING_MENUS_REMOTE_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: downloaded blocking-menu patterns; replaced by the next \
        accepted download",
    },
    InstanceArtifact {
        name: BLOCKING_MENUS_LOCAL_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: operator-owned blocking-menu overlay; machine-local by design",
    },
    InstanceArtifact {
        name: BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: remote blocking-menu download throttle stamp",
    },
    InstanceArtifact {
        name: CODEX_HOME_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: per-agent isolated coding-agent home trees",
    },
    InstanceArtifact {
        name: IDENTITY_TRANSITION_STATE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: private identity intent journal and original recovery images; \
        never user content",
    },
    InstanceArtifact {
        name: CODING_AGENT_REQUESTS_DIR_NAME,
        kind: ArtifactKind::Dir,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: CLI-to-app coding-agent mutation request queue, including its \
        results/ subdirectory",
    },
    InstanceArtifact {
        name: SOURCE_REGISTERED_INSTANCE_LEDGER_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: source coordination metadata and stable locks",
    },
    InstanceArtifact {
        name: CODING_AGENTS_SET_ASIDE_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: coding-agent catalog files the naming migration set aside \
        (<old name>.deprecated-<n>.no-git); every byte kept, never committed",
    },
    InstanceArtifact {
        name: CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient coding-agent migration journal temporaries \
        (.{destination}.{pid}.{counter}.tmp; the journal name starts with a dot)",
    },
    InstanceArtifact {
        name: CODING_AGENTS_BASE_TMP_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient managed coding-agent base publication temporaries",
    },
    InstanceArtifact {
        name: CODING_AGENTS_LOCK_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: persistent coding-agent catalog write-lock sidecar; transient \
        coordination state",
    },
    InstanceArtifact {
        name: CODING_AGENTS_LOCAL_TMP_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient coding-agent local override publication temporaries",
    },
    InstanceArtifact {
        name: CODING_AGENTS_RETIRED_LOCK_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: retired coding-agent catalog write-lock sidecar; created \
        once, never renamed or deleted",
    },
    InstanceArtifact {
        name: CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient coding-agent migration backup publication \
        temporaries",
    },
    InstanceArtifact {
        name: CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: coding-agent migration journal; immutable AC recovery state, \
        never user content",
    },
    InstanceArtifact {
        name: SOURCE_CATALOG_LOCK_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: source coordination metadata and stable locks",
    },
    InstanceArtifact {
        name: CODING_AGENTS_LOCAL_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: machine-local coding-agent overrides; user-owned and never \
        overwritten after creation",
    },
    InstanceArtifact {
        name: CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: immutable byte-exact backup of the pre-migration coding-agent \
        source",
    },
    InstanceArtifact {
        name: SOURCE_CATALOG_LEDGER_ARTIFACT,
        kind: ArtifactKind::RootRelativeGlob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: source coordination metadata and stable locks",
    },
    InstanceArtifact {
        name: CONFIG_IDENTITY_RESERVATION_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: private PAIR reservation and original recovery images",
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
        name: RETAINED_PROMPTS,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: private retained last prompts",
    },
    InstanceArtifact {
        name: "last-prompts.retained.json.*.tmp",
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: retained prompt atomic temporaries",
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
        name: NAMING_MIGRATION_STATE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: naming-migration journal and completion marker",
    },
    InstanceArtifact {
        name: NAMING_MIGRATION_STATE_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: transient naming-migration journal write temporaries",
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
        comment:
            "# AgentsCommander: rotated generations of the archived orphaned-session records; \
        the same runtime artifact under a numeric suffix",
    },
    InstanceArtifact {
        name: PROJECT_PATHS_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: machine-local raw project paths",
    },
    InstanceArtifact {
        name: PROJECT_PATHS_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: project path write temporaries",
    },
    InstanceArtifact {
        name: PROJECT_PATHS_BACKUP_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: immutable original settings before project path migration",
    },
    InstanceArtifact {
        name: PROJECT_PATHS_BACKUP_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: retained project path backup publication evidence",
    },
    InstanceArtifact {
        name: PROJECT_PATHS_RECOVERED_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: operator-preserved project path backup recovery evidence",
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
        name: REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: published remote CI activity snapshot (schema 1); replaced \
        atomically by the sweeper",
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
        name: SETTINGS_OWNER_ENDPOINT_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: settings owner endpoint metadata; machine-local runtime state",
    },
    InstanceArtifact {
        name: SETTINGS_BACKUP_ROTATION_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: rotated previous generations of the application settings; the \
        same runtime artifact under a numeric slot",
    },
    InstanceArtifact {
        name: SETTINGS_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: application settings",
    },
    InstanceArtifact {
        name: SETTINGS_TMP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Ignore,
        comment:
            "# AgentsCommander: transient settings write temporaries ({name}.{pid}.{op}.tmp); \
        survive only a crash mid-save",
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
        name: SETTINGS_LOCAL_OVERRIDE_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: operator-owned settings overlay; machine-local by design and \
        never written by AC",
    },
    InstanceArtifact {
        name: SETTINGS_RETIRED_LOCK_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: retired pre-migration settings write lock; the naming \
        migration never deletes it, so the leftover stays ignored",
    },
    InstanceArtifact {
        name: SETTINGS_OWNER_LOCK_FILE_NAME,
        kind: ArtifactKind::File,
        disposition: Disposition::Ignore,
        comment: "# AgentsCommander: settings owner lifecycle lock; stable sidecar, never unlinked",
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
        comment:
            "# AgentsCommander: legacy standalone global context template; production retires \
        it in place and never recreates it, and the entry holds user bytes; tracked on purpose",
    },
    InstanceArtifact {
        name: GLOBAL_CONTEXT_RETIRED_BACKUP_GLOB,
        kind: ArtifactKind::Glob,
        disposition: Disposition::Track,
        comment: "# AgentsCommander: retirement backups of the standalone global context; user \
        bytes, tracked on purpose",
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
        comment: "# AgentsCommander: canonical root-agent state (CLAUDE.md, memory, inbox); only \
        its config.json rules are ignored",
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

/// #1968 - the exact `coding-agents/` child allowlist `RootRelativeGlob` rows
/// may carry. A registry test pins the table to this set, so widening the kind
/// to another directory is a policy change rather than a table tweak.
#[cfg(test)]
pub(crate) const ROOT_RELATIVE_GLOB_ALLOWLIST: [&str; 12] = [
    CODING_AGENTS_SET_ASIDE_ARTIFACT,
    CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT,
    CODING_AGENTS_BASE_TMP_ARTIFACT,
    CODING_AGENTS_LOCK_ARTIFACT,
    CODING_AGENTS_LOCAL_TMP_ARTIFACT,
    CODING_AGENTS_RETIRED_LOCK_ARTIFACT,
    CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT,
    CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
    SOURCE_CATALOG_LOCK_ARTIFACT,
    CODING_AGENTS_LOCAL_ARTIFACT,
    CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
    SOURCE_CATALOG_LEDGER_ARTIFACT,
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
mod tests;
