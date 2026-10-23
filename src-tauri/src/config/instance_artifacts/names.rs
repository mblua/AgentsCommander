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
// P14 source coordination is metadata, independent of source content layers.
pub(crate) const SOURCE_CATALOG_BASE_LEDGER_NAME: &str =
    "coding-agent-source-catalogBase.state.no-git.json";
pub(crate) const SOURCE_CATALOG_PROJECT_LEDGER_NAME: &str =
    "coding-agent-source-catalogProject.state.no-git.json";
pub(crate) const SOURCE_CATALOG_PERSONAL_LEDGER_NAME: &str =
    "coding-agent-source-catalogPersonal.state.no-git.json";
pub(crate) const SOURCE_REGISTERED_INSTANCE_LEDGER_NAME: &str =
    "coding-agent-source-registeredInstance.state.no-git.json";
pub(crate) const SOURCE_REGISTERED_INSTANCE_LOCK: &str =
    ".coding-agent-source-registeredInstance.state.no-git.json.lock";
pub(crate) const SOURCE_CATALOG_LEDGER_ARTIFACT: &str =
    "coding-agents/coding-agent-source-*.state.no-git.json";
pub(crate) const SOURCE_CATALOG_LOCK_ARTIFACT: &str =
    "coding-agents/.coding-agent-source-*.state.no-git.json.lock";

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
pub(crate) const PROJECT_PATHS_FILE_NAME: &str = "project-paths.json";
pub(crate) const PROJECT_PATHS_TMP_GLOB: &str = "project-paths.json.*.tmp";
pub(crate) const PROJECT_PATHS_BACKUP_FILE_NAME: &str = "project-paths.pre-migration.no-git.json";
pub(crate) const PROJECT_PATHS_BACKUP_TMP_GLOB: &str =
    "project-paths.pre-migration.no-git.json.*.tmp";
pub(crate) const PROJECT_PATHS_RECOVERED_GLOB: &str =
    "project-paths.pre-migration.recovered-*.no-git.json";
pub(crate) const PROJECT_REFRESH_REQUESTS_DIR_NAME: &str = "project-refresh-requests";
pub(crate) const PTY_INPUT_LOCKS_DIR_NAME: &str = "pty-input-locks";
/// #2374 - the whole-file remote CI view the daemon republishes each sweep and
/// `room activity` reads. Machine-local and regenerated, so never committed.
pub(crate) const REMOTE_ACTIVITY_SNAPSHOT_FILE_NAME: &str = "remote-activity.json";
pub(crate) const SESSION_REQUESTS_DIR_NAME: &str = "session-requests";
/// #1737 - operator-owned Markdown overrides. AC never creates, syncs, upgrades,
/// retires or manifest-records one; the row exists so a machine-local override is
/// not committed by accident.
pub(crate) const LOCAL_MARKDOWN_OVERRIDE_GLOB: &str = "*.local.md";
/// #1737 - the operator-owned settings overlay, read at load and never written by AC.
pub(crate) const SETTINGS_LOCAL_OVERRIDE_FILE_NAME: &str = SETTINGS_LOCAL_TARGET_NAME;
/// #1905 - AC-owned shipped blocking-menu patterns; rewritten from embedded content at startup.
pub(crate) const BLOCKING_MENUS_SHIPPED_FILE_NAME: &str = BLOCKING_MENUS_SHIPPED_TARGET_NAME;
/// #1905 - the operator-owned blocking-menu overlay, read at load; written once by the export.
pub(crate) const BLOCKING_MENUS_LOCAL_FILE_NAME: &str = BLOCKING_MENUS_LOCAL_TARGET_NAME;
/// #1925 - blocking-menu patterns downloaded from GitHub; written only by the startup download.
pub(crate) const BLOCKING_MENUS_REMOTE_FILE_NAME: &str = BLOCKING_MENUS_REMOTE_TARGET_NAME;
/// #1925 - time of the last remote blocking-menu download attempt; throttles it to once per 24 h.
pub(crate) const BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME: &str =
    BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME;
/// #2133 - the operator-owned per-agent help overlay, read at load and never written by AC.
pub(crate) const AGENT_HELP_LOCAL_FILE_NAME: &str = "agent-help.local.json";
/// #2133 - AC-owned shipped per-agent help; rewritten from embedded content at startup.
pub(crate) const AGENT_HELP_SHIPPED_FILE_NAME: &str = "agent-help.json";
/// #2133 - per-agent help downloaded from GitHub; written only by the startup download.
pub(crate) const AGENT_HELP_REMOTE_FILE_NAME: &str = "agent-help.remote.json";
/// #2133 - time of the last remote agent-help download attempt; throttles it to once per 24 h.
pub(crate) const AGENT_HELP_REMOTE_CHECK_FILE_NAME: &str = "agent-help-remote-check.json";
/// AC's own instance settings file. Every production reader and writer of the
/// instance file names it through this constant.
pub(crate) const SETTINGS_FILE_NAME: &str = SETTINGS_TARGET_NAME;
pub(crate) const SETTINGS_LOCK_FILE_NAME: &str = SETTINGS_LOCK_TARGET_NAME;
/// #2714 - the pre-migration lock sidecar. A retired name kept only so the
/// leftover the naming migration deliberately never deletes stays ignored; the
/// one row deliberately not composed by `layered_name!`.
pub(crate) const SETTINGS_RETIRED_LOCK_NAME: &str = "settings.json.lock";
/// #2994 - the settings owner's lifecycle lock: a stable sidecar, separate from
/// the transactional `SETTINGS_LOCK_FILE_NAME`, retained for the owner's whole
/// lifetime and never unlinked on close.
pub(crate) const SETTINGS_OWNER_LOCK_FILE_NAME: &str = "settings.owner.no-git.lock";
/// #2994 - the settings owner's endpoint metadata. Reserved here so the runtime
/// artifact is ignored before any writer publishes it.
pub(crate) const SETTINGS_OWNER_ENDPOINT_FILE_NAME: &str = "settings-owner.endpoint.no-git.json";
/// Covers every settings migration backup instance. The concrete names are
/// composed by their own migrations, so this glob is registry-owned and no
/// writer imports it.
pub(crate) const SETTINGS_MIGRATION_BACKUP_GLOB: &str = SETTINGS_MIGRATION_BACKUP_TARGET_GLOB;
/// #2058 - the bounded rotation of previous `settings.json` generations. Slot 1
/// is the version the most recent save replaced. The writer composes the index
/// at runtime, so the glob is the only tie between the rule and the artifact.
pub(crate) const SETTINGS_BACKUP_PREFIX: &str = SETTINGS_BACKUP_TARGET_PREFIX;
pub(crate) const SETTINGS_BACKUP_SUFFIX: &str = ".json";
pub(crate) const SETTINGS_BACKUP_ROTATION_GLOB: &str = SETTINGS_BACKUP_ROTATION_TARGET_GLOB;
/// #2714 - every file the naming migration demotes, `<old name>.deprecated-<n>.no-git`,
/// in any family: one glob keeps every set-aside copy out of Git.
pub(crate) const SET_ASIDE_GLOB: &str = "*.deprecated-*.no-git";
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
/// The catalog manifest inside `coding-agents/`. The catalog module aliases it.
pub(crate) const CODING_AGENTS_BASE_FILENAME: &str = CODING_AGENTS_BASE_TARGET_NAME;

/// #1968 - declares one machine-local coding-agent catalog child: the FILE name
/// constant the catalog module aliases for its path composition, plus the
/// root-relative table name derived from the single directory prefix, so the two
/// can never drift apart.
macro_rules! coding_agents_child {
    ($file_const:ident, $artifact_const:ident, $filename:literal) => {
        pub(crate) const $file_const: &str = $filename;
        pub(crate) const $artifact_const: &str = concat!("coding-agents/", $filename);
    };
}

// The two immutable migration sidecars. The catalog module aliases these FILE
// names for its path composition, while the table rows use the derived
// root-relative names. The overrides layer and the write lock follow the
// layered target names below.
// Immutable by contract: an interrupted #1968 catalog migration resumes by this
// exact name, so it has no layered target name and is never renamed.
coding_agents_child!(
    CODING_AGENTS_MIGRATION_BACKUP_FILENAME,
    CODING_AGENTS_MIGRATION_BACKUP_ARTIFACT,
    "agents.migration-v1.backup.json"
);
// Immutable by contract, for the same reason as the migration backup above.
coding_agents_child!(
    CODING_AGENTS_MIGRATION_JOURNAL_FILENAME,
    CODING_AGENTS_MIGRATION_JOURNAL_ARTIFACT,
    ".agents.migration-v1.json"
);

// #1968 - the four publication temporaries. The catalog writer composes each
// name at runtime from its destination (`managed_catalog_temp_name_for_destination`),
// so no production code imports these file-name constants; they exist so the
// registry can prove the table's policy patterns are the writer's real shapes.
#[allow(dead_code)] // used by the registry's derivation test only
pub(crate) const CODING_AGENTS_BASE_TMP_GLOB: &str = ".agents.10.default.json.*.tmp";
#[allow(dead_code)] // used by the registry's derivation test only
pub(crate) const CODING_AGENTS_LOCAL_TMP_GLOB: &str = ".agents.50.personal.no-git.json.*.tmp";
#[allow(dead_code)] // used by the registry's derivation test only
pub(crate) const CODING_AGENTS_MIGRATION_BACKUP_TMP_GLOB: &str =
    ".agents.migration-v1.backup.json.*.tmp";
#[allow(dead_code)] // used by the registry's derivation test only
pub(crate) const CODING_AGENTS_MIGRATION_JOURNAL_TMP_GLOB: &str =
    "..agents.migration-v1.json.*.tmp";

pub(crate) const CODING_AGENTS_BASE_TMP_ARTIFACT: &str =
    "coding-agents/.agents.10.default.json.*.tmp";
pub(crate) const CODING_AGENTS_LOCAL_TMP_ARTIFACT: &str =
    "coding-agents/.agents.50.personal.no-git.json.*.tmp";
pub(crate) const CODING_AGENTS_MIGRATION_BACKUP_TMP_ARTIFACT: &str =
    "coding-agents/.agents.migration-v1.backup.json.*.tmp";
pub(crate) const CODING_AGENTS_MIGRATION_JOURNAL_TMP_ARTIFACT: &str =
    "coding-agents/..agents.migration-v1.json.*.tmp";

// #2470 - layered file names: `<name>.<NN>.<owner>[.no-git].<ext>`, as
// `docs/reference/file-naming.md` defines them.

/// NN and owner are ONE token, never two: they are 1:1, and two spellings of one
/// fact drift apart.
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const LAYER_DEFAULT: &str = "10.default";
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const LAYER_REMOTE: &str = "20.remote";
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const LAYER_INSTANCE: &str = "30.instance";
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const LAYER_PROJECT: &str = "40.project";
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const LAYER_PERSONAL: &str = "50.personal";
/// Marks a file that must never be committed.
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const NO_GIT_MARKER: &str = "no-git";
/// AC working memory; no competing layer, so no number.
#[allow(dead_code)] // read by the layer-token test until Phase B
pub(crate) const STATE_MARKER: &str = "state";

/// Composes a layered name at compile time, one arm per layer: a `const` cannot
/// be spliced into `concat!`, so each arm types its token once and
/// `layer_tokens_match_the_macro` pins it to its constant through
/// `LAYERED_NAME_PROBES`.
macro_rules! layered_name {
    ($stem:literal, default, $($rest:tt)+) => { layered_name!(@token $stem, "10.default", $($rest)+) };
    ($stem:literal, remote, $($rest:tt)+) => { layered_name!(@token $stem, "20.remote", $($rest)+) };
    ($stem:literal, instance, $($rest:tt)+) => { layered_name!(@token $stem, "30.instance", $($rest)+) };
    ($stem:literal, project, $($rest:tt)+) => { layered_name!(@token $stem, "40.project", $($rest)+) };
    ($stem:literal, personal, $($rest:tt)+) => { layered_name!(@token $stem, "50.personal", $($rest)+) };
    ($stem:literal, state, $($rest:tt)+) => { layered_name!(@token $stem, "state", $($rest)+) };
    (@token $stem:literal, $token:literal, no_git, $ext:literal) => {
        concat!($stem, ".", $token, ".no-git.", $ext)
    };
    (@token $stem:literal, $token:literal, $ext:literal) => {
        concat!($stem, ".", $token, ".", $ext)
    };
}

/// One composition per macro arm, each paired with the constant it must use.
/// The macro is not visible outside this file, so the tie is checked through
/// this table.
#[cfg(test)]
pub(crate) const LAYERED_NAME_PROBES: [(&str, &str); 7] = [
    (layered_name!("x", default, "e"), LAYER_DEFAULT),
    (layered_name!("x", remote, "e"), LAYER_REMOTE),
    (layered_name!("x", instance, "e"), LAYER_INSTANCE),
    (layered_name!("x", project, "e"), LAYER_PROJECT),
    (layered_name!("x", personal, "e"), LAYER_PERSONAL),
    (layered_name!("x", state, "e"), STATE_MARKER),
    (layered_name!("x", instance, no_git, "e"), NO_GIT_MARKER),
];

// Target names. B1b (#2714) switched the settings and blocking-menus ones;
// the rest stay unused until their own phase.
pub(crate) const SETTINGS_TARGET_NAME: &str = layered_name!("settings", instance, no_git, "json");
pub(crate) const SETTINGS_LOCAL_TARGET_NAME: &str =
    layered_name!("settings", personal, no_git, "json");
/// #2717 (B4a) - a project's `.ac/project-settings.json`, layer 50. The same
/// bytes as `SETTINGS_LOCAL_TARGET_NAME` on purpose: two artifacts, each named
/// from the rule, in two directories.
pub(crate) const PROJECT_SETTINGS_TARGET_NAME: &str =
    layered_name!("settings", personal, no_git, "json");
pub(crate) const SETTINGS_LOCK_TARGET_NAME: &str =
    concat!(layered_name!("settings", instance, no_git, "json"), ".lock");
/// A prefix, not a name: the writer composes the slot index at runtime and
/// appends `SETTINGS_BACKUP_SUFFIX`.
pub(crate) const SETTINGS_BACKUP_TARGET_PREFIX: &str =
    layered_name!("settings", instance, no_git, "backup.");
pub(crate) const SETTINGS_MIGRATION_BACKUP_TARGET_GLOB: &str =
    layered_name!("settings", instance, no_git, "pre-*.json");
pub(crate) const BLOCKING_MENUS_SHIPPED_TARGET_NAME: &str =
    layered_name!("blocking-menus", default, no_git, "json");
pub(crate) const BLOCKING_MENUS_REMOTE_TARGET_NAME: &str =
    layered_name!("blocking-menus", remote, no_git, "json");
pub(crate) const BLOCKING_MENUS_LOCAL_TARGET_NAME: &str =
    layered_name!("blocking-menus", personal, no_git, "json");
pub(crate) const BLOCKING_MENUS_REMOTE_CHECK_TARGET_NAME: &str =
    layered_name!("blocking-menus", state, no_git, "json");
/// #2786 (C1) - the agent state file beside the `config.json` it belongs to.
/// It holds the runtime tooling keys; `config.json` keeps the decisions.
pub(crate) const CONFIG_STATE_TARGET_NAME: &str = layered_name!("config", state, no_git, "json");
/// #2714 - the settings writer's temporaries, `<name>.<pid>.<op>.tmp`: no
/// leading dot, so the atomic-write glob cannot match them.
pub(crate) const SETTINGS_TMP_GLOB: &str = concat!(
    layered_name!("settings", instance, no_git, "json"),
    ".*.tmp"
);
/// #2714 - the one concrete migration backup, written by
/// `write_pre_384_v1_backup`; a registry test ties it to `SETTINGS_MIGRATION_BACKUP_GLOB`.
pub(crate) const SETTINGS_MIGRATION_BACKUP_384_V1_NAME: &str =
    layered_name!("settings", instance, no_git, "pre-384-v1.json");
/// #2714 - the rotation glob in the layered spelling; a registry test derives
/// it from `SETTINGS_BACKUP_PREFIX` and `SETTINGS_BACKUP_SUFFIX`.
pub(crate) const SETTINGS_BACKUP_ROTATION_TARGET_GLOB: &str =
    layered_name!("settings", instance, no_git, "backup.*.json");
/// Tracked, so no `.no-git`.
pub(crate) const CODING_AGENTS_BASE_TARGET_NAME: &str = layered_name!("agents", default, "json");
pub(crate) const CODING_AGENTS_LOCAL_TARGET_NAME: &str =
    layered_name!("agents", personal, no_git, "json");
/// Tracked, optional user/team-authored project patch; AC only reads it.
pub(crate) const CODING_AGENTS_PROJECT_TARGET_NAME: &str = layered_name!("agents", project, "json");
pub(crate) const CODING_AGENTS_LOCK_TARGET_NAME: &str =
    concat!(".", layered_name!("agents", default, "json"), ".lock");
/// #2715 - the user-owned overrides layer and the catalog write lock, switched
/// to their target names. The table rows use the root-relative twins.
pub(crate) const CODING_AGENTS_LOCAL_FILENAME: &str = CODING_AGENTS_LOCAL_TARGET_NAME;
pub(crate) const CODING_AGENTS_LOCAL_ARTIFACT: &str = concat!(
    "coding-agents/",
    layered_name!("agents", personal, no_git, "json")
);
pub(crate) const CODING_AGENTS_LOCK_FILENAME: &str = CODING_AGENTS_LOCK_TARGET_NAME;
pub(crate) const CODING_AGENTS_LOCK_ARTIFACT: &str = concat!(
    "coding-agents/.",
    layered_name!("agents", default, "json"),
    ".lock"
);
/// #2715 - the pre-migration catalog lock sidecar. A retired name kept only so
/// the leftover the naming migration deliberately never deletes stays ignored.
pub(crate) const CODING_AGENTS_RETIRED_LOCK_ARTIFACT: &str = "coding-agents/.agents.json.lock";
/// #2715 - every catalog file the naming migration demotes,
/// `<old name>.deprecated-<n>.no-git`: one glob keeps each set-aside copy out of Git.
pub(crate) const CODING_AGENTS_SET_ASIDE_ARTIFACT: &str = "coding-agents/*.deprecated-*.no-git";
/// #2716 (B3) - the instance file that holds `agents` and `codingAgentProfiles`,
/// beside the settings file.
/// #2718 (B4b) - a Loop's `_loop_*/state.json`: AC working memory with no
/// competing layer.
pub(crate) const LOOP_STATE_TARGET_NAME: &str = layered_name!("loop", state, no_git, "json");
/// #2718 (B4b) - the Loop state write temporary, `<name>.<uuid>.tmp`, derived
/// from the destination name.
pub(crate) const LOOP_STATE_TMP_TARGET_GLOB: &str =
    concat!(layered_name!("loop", state, no_git, "json"), ".*.tmp");
pub(crate) const AGENTS_INSTANCE_TARGET_NAME: &str =
    layered_name!("agents", instance, no_git, "json");
pub(crate) const AGENTS_INSTANCE_FILE_NAME: &str = AGENTS_INSTANCE_TARGET_NAME;
/// #2716 (B3) - the agents-file writer's temporaries, `<name>.<pid>.<op>.tmp`,
/// the same shape as `SETTINGS_TMP_GLOB`.
pub(crate) const AGENTS_INSTANCE_TMP_GLOB: &str =
    concat!(layered_name!("agents", instance, no_git, "json"), ".*.tmp");

/// Private identity intent journal; working state has no competing layer.
pub(crate) const IDENTITY_TRANSITION_STATE_NAME: &str =
    layered_name!("coding-agent-identity-transition", state, no_git, "json");
pub(crate) const IDENTITY_TRANSITION_LOCK_NAME: &str = concat!(
    ".",
    layered_name!("coding-agent-identity-transition", state, no_git, "json"),
    ".lock"
);
pub(crate) const IDENTITY_TRANSITION_TMP_GLOB: &str = concat!(
    ".",
    layered_name!("coding-agent-identity-transition", state, no_git, "json"),
    ".*.tmp"
);

/// Private PAIR packet and stable lock; a complete packet is the reservation.
pub(crate) const CONFIG_IDENTITY_RESERVATION_NAME: &str =
    layered_name!("config-identity-reservation", state, no_git, "json");
pub(crate) const CONFIG_IDENTITY_RESERVATION_LOCK: &str = concat!(
    ".",
    layered_name!("config-identity-reservation", state, no_git, "json"),
    ".lock"
);
pub(crate) const CONFIG_IDENTITY_RESERVATION_TMP: &str = concat!(
    ".",
    layered_name!("config-identity-reservation", state, no_git, "json"),
    ".*.tmp"
);

/// #2713 - the naming-migration journal, which is also its completion marker:
/// one file, so there is one source of truth. `config::naming_migration` owns it.
pub(crate) const NAMING_MIGRATION_STATE_NAME: &str =
    layered_name!("naming-migration", state, no_git, "json");
/// The journal's atomic-commit temporaries, `<name>.<pid>.<seq>.tmp`.
pub(crate) const NAMING_MIGRATION_STATE_TMP_GLOB: &str = concat!(
    layered_name!("naming-migration", state, no_git, "json"),
    ".*.tmp"
);
/// The journal writer lock, `.<name>.lock`: created once, never deleted.
pub(crate) const NAMING_MIGRATION_LOCK_NAME: &str = concat!(
    ".",
    layered_name!("naming-migration", state, no_git, "json"),
    ".lock"
);

/// #1968 - the single publication temporary-name formula,
/// `.{destination}.{pid}.{counter}.tmp`. The catalog writer aliases it and the
/// generated-policy test derives the journal temporary from it, so the writer
/// and the ignore rule cannot drift apart. Leaf on purpose: `format!` only.
pub(crate) fn publication_temp_name_for_destination(
    destination_file_name: &str,
    pid: u32,
    counter: u64,
) -> String {
    format!(".{destination_file_name}.{pid}.{counter}.tmp")
}
