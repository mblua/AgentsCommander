use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME: &str =
    crate::config::instance_artifacts::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME;

const STATE_SCHEMA_VERSION: u32 = 1;
static STATE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const CONTEXT_TEMPLATE_CHANGED: &str =
    "Context template changed on disk; reload the project before overwriting.";
const CONTEXT_TEMPLATE_DEFAULT_CHANGED: &str =
    "Context template default changed; reload the project before overwriting.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextPublication {
    pub(crate) published_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextTemplateSkipReason {
    CreationDisabled,
    MissingAfterCreate,
    AmbiguousWithoutState,
    IgnoredByUser,
    AcRootUnavailable,
    TargetMissing,
    /// #1748: a distribution-owned template differs from the current default on a
    /// path that cannot report the replacement; the repair is left to the scan.
    DistributionRepairDeferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplatePublication {
    Published(ContextPublication),
    AlreadyCurrent,
    ChangedUnderUs,
    Observed,
    Skipped(ContextTemplateSkipReason),
}

#[derive(Debug, Clone)]
pub(crate) struct ContextTemplateExecution<T> {
    pub(crate) completion: Result<T, String>,
    pub(crate) published: Option<ContextPublication>,
}

impl<T> ContextTemplateExecution<T> {
    pub(crate) fn from_parts(
        completion: Result<T, String>,
        published: Option<ContextPublication>,
    ) -> Self {
        Self {
            completion,
            published,
        }
    }

    pub(crate) fn completed(value: T) -> Self {
        Self::from_parts(Ok(value), None)
    }

    pub(crate) fn failed(error: String) -> Self {
        Self::from_parts(Err(error), None)
    }

    pub(crate) fn with_publication(
        completion: Result<T, String>,
        publication: ContextPublication,
    ) -> Self {
        Self::from_parts(completion, Some(publication))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TemplateSyncOutcome {
    pub(crate) pending_update: Option<ContextTemplateUpdate>,
    pub(crate) replacement: Option<ContextTemplateReplacement>,
    pub(crate) target_outcome: TemplatePublication,
}

const OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND: &str = "You are the coordinator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests.\n\
     - Clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Always route work to the team member best prepared for each part of the request based on role, skills, and current assignment.\n\
     - Delegate work instead of absorbing technical work when a more specialized agent is available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working.\n\
     - Contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete.\n\
     - Not infer completion solely from files/logs/artifacts/status flags when the assigned agent has not reported the outcome.\n\
     - Give recommendations to help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     As a coordinator, you may need to send screenshots. Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     - --path is required. --caption is optional and limited to 1024 UTF-16 units.\n\
     - If multiple Telegram bots are configured, use --bot-id or --bot-label.\n\
     - jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB.\n\
     - Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work. Important: cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels. In that case ask the user to capture with Greenshot, use latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n";

/// #1005 S6: `get_default_agent_template()` exactly as it shipped from #658
/// (mandatory placeholders) through base commit ec660c17, frozen so a
/// pristine v1 `Context.AgentsCommander.md` on disk keeps being recognized
/// (project auto-update AND standalone root retirement) after the v2
/// token-minimization rewrite. Never edit. Provenance (G3): one-off run of
/// the shipped accessor at ec660c17 printed len 611, sha256
/// c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88; pinned
/// by `global_pre_token_minimization_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager that coordinates multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines who can work together, who coordinates, and which repos are available.
- **Workgroup**: an operational runtime replica instance of a team for a specific task. It contains replica agents and `repo-*` working repositories.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1369 (C4): `get_default_agent_template()` exactly as it shipped from #1005
/// S6 (token minimization, v2) through base commit 8f272a76, frozen so a
/// pristine v2 `Context.AgentsCommander.md` on disk keeps being recognized
/// (project auto-update AND standalone root retirement) after the v3
/// `{{AGENT_REPOS}}` rename. Never edit. Provenance: the raw literal at
/// 8f272a76 is len 567, sha256
/// e5861a9f011967e96e5515f858e1643f7fdf161511ad909fe86ddb4ce1a0cff7; pinned by
/// `global_pre_agent_repos_snapshot_is_byte_exact` against those externally
/// captured values, never against this const itself.
///
/// Do NOT replace this literal with `include_str!`: a raw string literal
/// normalizes `\r\n` to `\n` at compile time and `include_str!` does not.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and `repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1541: `get_default_agent_template()` exactly as it shipped from #1369
/// (the `{{AGENT_REPOS}}` rename, v3) through base commit 6aae531e, frozen so
/// pristine v3 project templates and standalone templates remain exact
/// generated operands after the v4 summarization. Never edit. The shipped
/// accessor is len 563, sha256
/// 99a0aa4a15062d4b68b94597111ae268958cbeb4e3902aafe1b7361b63d34157;
/// pinned by `global_before_summarization_snapshot_is_byte_exact`.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and `repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1605: `get_default_agent_template()` exactly as it shipped through base
/// commit 047248bc (the v4 summarization), frozen so a pristine v4
/// `Context.AgentsCommander.md` on disk keeps being recognized (project
/// auto-update AND standalone root retirement) after the v5
/// `{{HOST_PLATFORM_RULES}}` insertion. Never edit. Provenance: the accessor at
/// 047248bc is len 539, sha256
/// f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a; pinned by
/// `global_before_host_platform_rules_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES: &str = r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and `repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1005 S4: `get_default_coordinator_template()` exactly as it shipped from
/// #684 (raise-hand) through base commit 1dd0b58, frozen as the second legacy
/// snapshot so a pristine v2 `Context.coordinator.md` on disk keeps being
/// recognized and auto-upgraded after the v3 token-minimization rewrite.
/// Never edit. Provenance (G3): one-off run of the shipped accessor at
/// 1dd0b58 printed len 2403, sha256
/// 92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02; pinned by
/// `coordinator_pre_token_minimization_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION: &str =
    "You are the coordinator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests.\n\
     - Clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Always route work to the team member best prepared for each part of the request based on role, skills, and current assignment.\n\
     - Delegate work instead of absorbing technical work when a more specialized agent is available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working.\n\
     - Contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete.\n\
     - Not infer completion solely from files/logs/artifacts/status flags when the assigned agent has not reported the outcome.\n\
     - Give recommendations to help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     As a coordinator, you may need to send screenshots. Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     - --path is required. --caption is optional and limited to 1024 UTF-16 units.\n\
     - If multiple Telegram bots are configured, use --bot-id or --bot-label.\n\
     - jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB.\n\
     - Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work. Important: cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels. In that case ask the user to capture with Greenshot, use latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the user interacts with your session.\n";

/// #1030: `get_default_coordinator_template()` exactly as it shipped through
/// base commit 4acadfe5, frozen as the third legacy snapshot so a pristine v3
/// `Context.coordinator.md` on disk keeps being recognized and auto-upgraded
/// after the v4 rewrite that adds the cross-workgroup rule.
/// Never edit. Provenance (E2): one-off run of the shipped accessor at
/// 4acadfe5 printed len 2296, sha256
/// 9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84; pinned by
/// `coordinator_pre_cross_workgroup_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE: &str =
    "You are the coordinator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and current assignment; delegate instead of absorbing technical work when a more specialized agent is available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete; never infer completion solely from files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work; cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels; then ask the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the user interacts with your session.\n";

/// #1571: `get_default_coordinator_template()` exactly as it shipped through
/// base commit ecc6527b, frozen as the fourth legacy snapshot so a pristine v4
/// `Context.coordinator.md` on disk keeps being recognized and auto-upgraded
/// after the v5 Coordinator-to-Orchestrator rename.
/// Never edit. Provenance (plan #1571 3.6): the shipped accessor measured at
/// base commit ecc6527b printed len 2509, sha256
/// f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6; pinned by
/// `coordinator_pre_orchestrator_rename_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME: &str =
    "You are the coordinator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and current assignment; delegate instead of absorbing technical work when a more specialized agent is available.\n\
     - To reach another workgroup, message its coordinator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to a coordinator who messaged you first is always authorized.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete; never infer completion solely from files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work; cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels; then ask the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the user interacts with your session.\n";

/// #979: the standalone global context template that older builds seeded into the
/// APP CONFIG directory (307 bytes; it predates `## Core Concepts`). Retirement may
/// delete only bytes AgentsCommander provably generated itself, so this snapshot is
/// frozen and pinned by a length + SHA-256 test.
///
/// Do NOT replace this literal with `include_str!`: a raw string literal normalizes
/// `\r\n` to `\n` at compile time and `include_str!` does not, so on a CRLF checkout
/// `include_str!` would silently stop recognizing the generated default and every
/// retirement would fall through to "custom".
const STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager that coordinates multiple AI agents.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1614 D8a: the `global` seeded context template exactly as it shipped
/// through base commit df494bfa, frozen so an installation whose file is
/// still pristine keeps auto-updating after the Room rename. A recognizer
/// only accepts a file byte-for-byte, so a shipped byte that moves without
/// the previous bytes being frozen reclassifies every pristine file as
/// user-authored and it never auto-updates again. Never edit.
///
/// RE-BASED in plan section 15.3 item 1. It previously held the v4 body,
/// which is the generation main already froze under its own name as
/// `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` (#1605). Holding it
/// here too would have shipped two constants with identical bytes under two
/// names and left the v5 generation unrecognized forever, so every
/// installation that reached v5 before upgrading would have had its global
/// template permanently reclassified as user-authored. AC7.1b's `!=` limb is
/// what makes that state unshippable.
/// Provenance: the df494bfa blob, session_context.rs lines 2513-2537;
/// declaration 574 bytes sha256 D9E93582...A844 (plan 3.12 Table A), value
/// 564 bytes sha256 D094106B...4F77 (Table B); pinned by
/// `frozen_snapshots_are_byte_exact_at_d7008b34`.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME: &str = r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and `repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{HOST_PLATFORM_RULES}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

/// #1614 D8a: the `coordinator` seeded context template exactly as it
/// shipped through base commit d7008b34, frozen for the same reason as the
/// global one above. Never edit.
/// Provenance: the d7008b34 blob, session_context.rs lines 2509-2529;
/// declaration 2703 bytes sha256 CC127468...3ABF (plan 3.12 Table A), value
/// 2516 bytes sha256 0B89EB38...198E (Table B); pinned by
/// `frozen_snapshots_are_byte_exact_at_d7008b34`.
const COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME: &str =
    "You are the orchestrator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and current assignment; delegate instead of absorbing technical work when a more specialized agent is available.\n\
     - To reach another workgroup, message its orchestrator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to an orchestrator who messaged you first is always authorized.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete; never infer completion solely from files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop orchestrator: PowerShell System.Drawing / CopyFromScreen can work; cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness orchestrator: CopyFromScreen may return all-zero/black pixels; then ask the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your orchestrator row; it clears when the user interacts with your session.\n";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateUpdate {
    pub project_path: String,
    pub workspace_path: String,
    pub file_path: String,
    pub filename: String,
    pub label: String,
    pub current_file_sha256: String,
    pub current_default_sha256: String,
    pub current_default_version: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateOverwriteResult {
    pub file_path: String,
    pub backup_path: String,
    pub current_default_sha256: String,
}

/// #1748: everything a caller needs to tell the user that AgentsCommander replaced
/// the bytes they had on disk. Produced ONLY when the replaced bytes were not ours.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateReplacement {
    pub project_path: String,
    pub workspace_path: String,
    pub file_path: String,
    pub filename: String,
    pub label: String,
    pub backup_path: String,
    pub local_override_path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeededContextTemplateState {
    schema_version: u32,
    templates: BTreeMap<String, SeededContextTemplateEntry>,
}

impl Default for SeededContextTemplateState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            templates: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeededContextTemplateEntry {
    template_id: String,
    current_version: u32,
    last_seeded_sha256: Option<String>,
    last_observed_sha256: Option<String>,
    ignored_default_sha256: Option<String>,
    ignored_observed_sha256: Option<String>,
}

#[derive(Clone, Copy)]
struct SeededContextTemplateSpec {
    id: &'static str,
    filename: &'static str,
    label: &'static str,
    current_version: u32,
    current_content: fn() -> &'static str,
    /// #1748: `None` for a distribution-owned spec, which consults no recognizer.
    is_known_generated: Option<fn(&str) -> bool>,
    project_actionable: bool,
    suppress_unknown_without_state: bool,
    /// #1748: the file is owned by the distribution. It is repaired on the
    /// reporting sync path instead of being offered as a pending update.
    distribution_owned: bool,
}

#[derive(Clone)]
struct FileSnapshot {
    bytes: Vec<u8>,
    content: String,
    sha256: String,
}

struct LoadedState {
    state: SeededContextTemplateState,
    trusted: bool,
    can_persist: bool,
    dirty: bool,
}

impl LoadedState {
    fn trusted_entry(
        &self,
        spec: SeededContextTemplateSpec,
    ) -> Option<&SeededContextTemplateEntry> {
        if !self.trusted {
            return None;
        }
        self.state
            .templates
            .get(spec.id)
            .filter(|entry| entry.template_id == spec.id)
    }

    fn entry_mut(&mut self, spec: SeededContextTemplateSpec) -> &mut SeededContextTemplateEntry {
        let entry = self.state.templates.entry(spec.id.to_string()).or_default();
        entry.template_id = spec.id.to_string();
        entry.current_version = spec.current_version;
        entry
    }

    fn mark_seeded(&mut self, spec: SeededContextTemplateSpec, current_default_sha256: &str) {
        if !self.trusted {
            return;
        }
        let entry = self.entry_mut(spec);
        let next = Some(current_default_sha256.to_string());
        if entry.last_seeded_sha256 != next
            || entry.last_observed_sha256.is_some()
            || entry.ignored_default_sha256.is_some()
            || entry.ignored_observed_sha256.is_some()
        {
            entry.last_seeded_sha256 = next;
            entry.last_observed_sha256 = None;
            entry.ignored_default_sha256 = None;
            entry.ignored_observed_sha256 = None;
            self.dirty = true;
        }
    }

    fn mark_observed(&mut self, spec: SeededContextTemplateSpec, current_file_sha256: &str) {
        if !self.trusted {
            return;
        }
        let entry = self.entry_mut(spec);
        let next = Some(current_file_sha256.to_string());
        if entry.last_observed_sha256 != next {
            entry.last_observed_sha256 = next;
            self.dirty = true;
        }
    }

    fn mark_ignored(
        &mut self,
        spec: SeededContextTemplateSpec,
        current_file_sha256: &str,
        current_default_sha256: &str,
    ) {
        let entry = self.entry_mut(spec);
        let observed = Some(current_file_sha256.to_string());
        let default = Some(current_default_sha256.to_string());
        if entry.ignored_observed_sha256 != observed || entry.ignored_default_sha256 != default {
            entry.ignored_observed_sha256 = observed;
            entry.ignored_default_sha256 = default;
            entry.last_observed_sha256 = Some(current_file_sha256.to_string());
            self.dirty = true;
        }
    }
}

fn project_specs() -> [SeededContextTemplateSpec; 5] {
    let [windows, linux, macos] = platform_specs();
    [
        SeededContextTemplateSpec {
            id: "global",
            filename: crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            label: "AgentsCommander shared context",
            current_version: 6,
            current_content: crate::config::session_context::get_default_agent_template,
            is_known_generated: None,
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: true,
        },
        SeededContextTemplateSpec {
            id: "coordinator",
            filename: crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            label: "Orchestrator context",
            current_version: 6,
            current_content: crate::config::session_context::get_default_coordinator_template,
            is_known_generated: Some(is_known_generated_coordinator_template),
            project_actionable: true,
            suppress_unknown_without_state: false,
            distribution_owned: false,
        },
        windows,
        linux,
        macos,
    ]
}

/// #1605: the three per-EXECUTION-platform `{{HOST_PLATFORM_RULES}}` files
/// (`Context.platform.<os>.md`), seeded absent-only in project `.ac` roots and
/// carried through `sync_one_template` with seeded/observed state, edit
/// preservation and a pending-update offer.
/// `suppress_unknown_without_state: true` keeps a pre-existing unowned file
/// preserved silently and never prompted.
fn platform_specs() -> [SeededContextTemplateSpec; 3] {
    [
        SeededContextTemplateSpec {
            id: "platform.windows",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
            label: "Windows host platform rules",
            current_version: 1,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
            is_known_generated: Some(is_known_generated_platform_windows),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
        SeededContextTemplateSpec {
            id: "platform.linux",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            label: "Linux host platform rules",
            current_version: 1,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            is_known_generated: Some(is_known_generated_platform_linux),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
        SeededContextTemplateSpec {
            id: "platform.macos",
            filename: crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            label: "macOS host platform rules",
            current_version: 1,
            current_content: || crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            is_known_generated: Some(is_known_generated_platform_macos),
            project_actionable: true,
            suppress_unknown_without_state: true,
            distribution_owned: false,
        },
    ]
}

fn root_spec() -> SeededContextTemplateSpec {
    SeededContextTemplateSpec {
        id: "rootAgent",
        filename: crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME,
        label: "Root agent context",
        current_version: 8,
        current_content: crate::config::root_agent::default_root_context_template,
        is_known_generated: Some(
            crate::config::root_agent::is_known_generated_root_context_template,
        ),
        project_actionable: false,
        suppress_unknown_without_state: false,
        distribution_owned: false,
    }
}

fn project_spec_by_filename(filename: &str) -> Option<SeededContextTemplateSpec> {
    project_specs()
        .into_iter()
        .find(|spec| spec.filename == filename)
}

fn actionable_project_spec_by_filename(
    filename: &str,
) -> Result<SeededContextTemplateSpec, String> {
    if filename.contains('/') || filename.contains('\\') {
        return Err("Context template filename is not managed by AgentsCommander".to_string());
    }
    let spec = project_spec_by_filename(filename)
        .ok_or_else(|| "Context template filename is not managed by AgentsCommander".to_string())?;
    if spec.distribution_owned {
        return Err(
            "Context template filename is managed by the distribution and cannot be overwritten by hand"
                .to_string(),
        );
    }
    if !spec.project_actionable {
        return Err("Context template filename is not actionable for this project".to_string());
    }
    Ok(spec)
}

/// #979: exact recognition of a STANDALONE (app-config) generated global context.
///
/// True only for byte-for-byte UTF-8 equality with the current built-in default or
/// with the frozen 307-byte snapshot above. No normalization of whitespace, line
/// endings, BOMs, or trailing newlines, and seed-state hashes are never consulted:
/// a CRLF copy, an invalid-UTF-8 file, a one-byte edit, or a state entry claiming
/// ownership is UNKNOWN, and unknown content is backed up, never deleted.
///
/// #1748: the PROJECT `global` spec is distribution-owned and names no recognizer,
/// so this is the only recognizer for these bytes. It must never be pointed at
/// `project_specs()`: a second consumer would make a future edit to it silently
/// change project behavior.
fn is_known_generated_standalone_global_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_agent_template()
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        || content == STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}

/// #1605: per-platform generated recognizers — equality with the current
/// platform default const only. A future default change MUST first freeze the
/// previous default as a snapshot const and extend the recognizer, so seeded
/// files auto-update and edited files are preserved with the pending-update
/// offer.
fn is_known_generated_platform_windows(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS
}

fn is_known_generated_platform_linux(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX
}

fn is_known_generated_platform_macos(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS
}

fn is_known_generated_coordinator_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_coordinator_template()
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME
        || content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{:x}", digest)
}

/// #1748: the operator-owned override that replaces this template at render time.
/// Kept in lockstep with `config::local_overlay`, whose PRIVATE `MARKDOWN_LOCAL_SUFFIX`
/// (`local_overlay.rs:28`) is the other half of the contract. The lockstep holds for a
/// `.md` filename, which is the only kind any spec carries; for any other extension
/// `markdown_override_path` returns `None` and this helper has no counterpart.
fn local_override_filename(filename: &str) -> String {
    format!(
        "{}.local.md",
        filename.strip_suffix(".md").unwrap_or(filename)
    )
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_existing_dir(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {} {}: {}", label, path.display(), e))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{} {} exists but is not a regular directory",
            label,
            path.display()
        ));
    }
    Ok(())
}

fn validate_existing_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {} {}: {}", label, path.display(), e))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} {} exists but is not a regular file",
            label,
            path.display()
        ));
    }
    Ok(())
}

fn read_validated_snapshot(path: &Path, label: &str) -> Result<Option<FileSnapshot>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(format!(
                    "{} {} exists but is not a regular file",
                    label,
                    path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Failed to inspect {} {}: {}",
                label,
                path.display(),
                e
            ))
        }
    }

    let bytes = std::fs::read(path)
        .map_err(|e| format!("Failed to read {} {}: {}", label, path.display(), e))?;
    let sha256 = sha256_hex(&bytes);
    let content = String::from_utf8(bytes.clone())
        .map_err(|e| format!("{} {} is not valid UTF-8: {}", label, path.display(), e))?;
    Ok(Some(FileSnapshot {
        bytes,
        content,
        sha256,
    }))
}

fn load_state(ac_root: &Path, strict: bool) -> Result<LoadedState, String> {
    let path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                let message = format!(
                    "Context template state path {} exists but is not a regular file",
                    path.display()
                );
                if strict {
                    return Err(message);
                }
                log::warn!(
                    "[context-templates] {}; skipping state persistence",
                    message
                );
                return Ok(LoadedState {
                    state: SeededContextTemplateState::default(),
                    trusted: false,
                    can_persist: false,
                    dirty: false,
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: true,
                can_persist: true,
                dirty: false,
            })
        }
        Err(e) => {
            let message = format!(
                "Failed to inspect context template state {}: {}",
                path.display(),
                e
            );
            if strict {
                return Err(message);
            }
            log::warn!(
                "[context-templates] {}; skipping state persistence",
                message
            );
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: false,
                can_persist: false,
                dirty: false,
            });
        }
    }

    let bytes = std::fs::read(&path).map_err(|e| {
        format!(
            "Failed to read context template state {}: {}",
            path.display(),
            e
        )
    })?;
    let state = match serde_json::from_slice::<SeededContextTemplateState>(&bytes) {
        Ok(state) => state,
        Err(e) => {
            log::warn!(
                "[context-templates] invalid state JSON at {}; treating as empty: {}",
                path.display(),
                e
            );
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: true,
                can_persist: true,
                dirty: true,
            });
        }
    };

    if state.schema_version != STATE_SCHEMA_VERSION {
        let message = format!(
            "Context template state schema version {} is unsupported; reload or upgrade AgentsCommander.",
            state.schema_version
        );
        if strict {
            return Err(message);
        }
        log::warn!(
            "[context-templates] {}; skipping state persistence",
            message
        );
        return Ok(LoadedState {
            state: SeededContextTemplateState::default(),
            trusted: false,
            can_persist: false,
            dirty: false,
        });
    }

    Ok(LoadedState {
        state,
        trusted: true,
        can_persist: true,
        dirty: false,
    })
}

fn unique_state_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    let counter = STATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

fn cleanup_temp(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "[context-templates] failed to remove temporary file {}: {}",
                path.display(),
                e
            );
        }
    }
}

fn persist_state(ac_root: &Path, state: &SeededContextTemplateState) -> Result<(), String> {
    let path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(format!(
                    "Context template state path {} exists but is not a regular file",
                    path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect context template state {}: {}",
                path.display(),
                e
            ))
        }
    }

    let content = serde_json::to_vec_pretty(state)
        .map_err(|e| format!("Failed to serialize context template state: {}", e))?;
    let temp = unique_state_temp_path(&path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| {
            format!(
                "Failed to create temporary context template state {}: {}",
                temp.display(),
                e
            )
        })?;
    if let Err(e) = file.write_all(&content) {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to write temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.flush() {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to flush temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to sync temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    drop(file);

    if let Err(e) = crate::config::root_agent::atomic_replace_existing(&temp, &path) {
        cleanup_temp(&temp);
        return Err(e);
    }
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[context-templates] failed to sync state directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
    }
    Ok(())
}

fn persist_state_best_effort(ac_root: &Path, loaded: &LoadedState) {
    if !loaded.dirty || !loaded.can_persist {
        return;
    }
    if let Err(e) = persist_state(ac_root, &loaded.state) {
        log::warn!(
            "[context-templates] failed to persist state in {}: {}",
            ac_root.display(),
            e
        );
    }
}

fn persist_state_strict(ac_root: &Path, loaded: &LoadedState) -> Result<(), String> {
    if !loaded.can_persist {
        return Err("Context template state cannot be safely persisted".to_string());
    }
    if loaded.dirty {
        persist_state(ac_root, &loaded.state)?;
    }
    Ok(())
}

fn make_update(
    project_dir: &Path,
    ac_root: &Path,
    path: &Path,
    spec: SeededContextTemplateSpec,
    current_file_sha256: String,
    current_default_sha256: String,
) -> ContextTemplateUpdate {
    ContextTemplateUpdate {
        project_path: display_path(project_dir),
        workspace_path: display_path(ac_root),
        file_path: display_path(path),
        filename: spec.filename.to_string(),
        label: spec.label.to_string(),
        current_file_sha256,
        current_default_sha256,
        current_default_version: spec.current_version,
    }
}

fn create_missing_template(
    path: &Path,
    content: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<crate::config::session_context::CreateOnlyPublication> {
    let outcome = match crate::config::session_context::write_template_if_missing_with_clock(
        path, content, clock,
    ) {
        Ok(outcome) => outcome,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let published = match outcome {
        crate::config::session_context::CreateOnlyPublication::Published { published_at } => {
            Some(ContextPublication { published_at })
        }
        crate::config::session_context::CreateOnlyPublication::AlreadyPresent => None,
    };
    ContextTemplateExecution::from_parts(
        validate_existing_file(path, "Context template").map(|()| outcome),
        published,
    )
}

fn auto_update_generated_template(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplatePublication> {
    auto_update_generated_template_with(
        path,
        spec,
        expected_file_sha256,
        clock,
        crate::config::session_context::atomically_replace_context_template,
    )
}

fn auto_update_generated_template_with<R>(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    replace: R,
) -> ContextTemplateExecution<TemplatePublication>
where
    R: FnOnce(
        &Path,
        &str,
        &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    ) -> Result<chrono::DateTime<chrono::Utc>, String>,
{
    let snapshot = match read_validated_snapshot(path, "Context template") {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs)
        }
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    if snapshot.sha256 != expected_file_sha256 {
        log::warn!(
            "[context-templates] {} changed before generated update; preserving current content",
            path.display()
        );
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }
    if !spec
        .is_known_generated
        .is_some_and(|matches| matches(&snapshot.content))
    {
        log::warn!(
            "[context-templates] {} no longer matches a known generated default; preserving current content",
            path.display()
        );
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }
    let published_at = match replace(path, (spec.current_content)(), clock) {
        Ok(published_at) => published_at,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let publication = ContextPublication { published_at };
    ContextTemplateExecution::with_publication(
        Ok(TemplatePublication::Published(publication)),
        publication,
    )
}

#[allow(clippy::too_many_arguments)]
fn sync_one_template(
    project_dir: Option<&Path>,
    ac_root: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &mut LoadedState,
    allow_create_missing: bool,
    return_pending: bool,
    repair: bool,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let path = ac_root.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let mut snapshot = match read_validated_snapshot(&path, "Context template") {
        Ok(snapshot) => snapshot,
        Err(error) => return ContextTemplateExecution::failed(error),
    };
    let mut carried_publication = None;

    if snapshot.is_none() {
        if !allow_create_missing {
            return ContextTemplateExecution::completed(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome: TemplatePublication::Skipped(
                    ContextTemplateSkipReason::CreationDisabled,
                ),
            });
        }
        let creation = create_missing_template(&path, current_default, clock);
        carried_publication = creation.published;
        if let Err(error) = creation.completion {
            return ContextTemplateExecution::from_parts(Err(error), carried_publication);
        }
        snapshot = match read_validated_snapshot(&path, "Context template") {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return ContextTemplateExecution::from_parts(Err(error), carried_publication)
            }
        };
        if let Some(snapshot) = &snapshot {
            if snapshot.sha256 == current_default_sha256 {
                loaded.mark_seeded(spec, &current_default_sha256);
                let target_outcome = carried_publication
                    .map(TemplatePublication::Published)
                    .unwrap_or(TemplatePublication::AlreadyCurrent);
                return ContextTemplateExecution::from_parts(
                    Ok(TemplateSyncOutcome {
                        pending_update: None,
                        replacement: None,
                        target_outcome,
                    }),
                    carried_publication,
                );
            }
        }
    }

    let Some(snapshot) = snapshot else {
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::Skipped(
                ContextTemplateSkipReason::MissingAfterCreate,
            ));
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    };

    if snapshot.sha256 == current_default_sha256 {
        loaded.mark_seeded(spec, &current_default_sha256);
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::AlreadyCurrent);
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    }

    if spec.distribution_owned {
        if !repair {
            // D2: no reporting channel on this path, so the repair is deferred to
            // the scan rather than replacing operator bytes with no notification.
            let target_outcome = carried_publication
                .map(TemplatePublication::Published)
                .unwrap_or(TemplatePublication::Skipped(
                    ContextTemplateSkipReason::DistributionRepairDeferred,
                ));
            return ContextTemplateExecution::from_parts(
                Ok(TemplateSyncOutcome {
                    pending_update: None,
                    replacement: None,
                    target_outcome,
                }),
                carried_publication,
            );
        }

        // Unreachable today: `repair` is true only on the scan, which always has a
        // project path. It must stay an error rather than a silent `None`.
        let project_dir = match project_dir {
            Some(project_dir) => project_dir,
            None => {
                return ContextTemplateExecution::from_parts(
                    Err(format!(
                        "Cannot create context template update for {} without a project path",
                        path.display()
                    )),
                    carried_publication,
                )
            }
        };

        // D6: silent only when the bytes about to be replaced are exactly the ones
        // we last wrote. Computed before any write.
        let notify = loaded
            .trusted_entry(spec)
            .and_then(|entry| entry.last_seeded_sha256.as_deref())
            != Some(snapshot.sha256.as_str());

        let backup_path = match create_backup(&path, &snapshot.bytes) {
            Ok(backup_path) => backup_path,
            Err(error) => {
                return ContextTemplateExecution::from_parts(Err(error), carried_publication)
            }
        };
        let after_backup = match read_validated_snapshot(&path, "Context template") {
            Ok(after_backup) => after_backup,
            Err(error) => {
                return ContextTemplateExecution::from_parts(Err(error), carried_publication)
            }
        };
        if !after_backup
            .as_ref()
            .is_some_and(|after_backup| after_backup.sha256 == snapshot.sha256)
        {
            log::warn!(
                "[context-templates] {} changed before the distribution repair; the bytes read first are kept in {}",
                path.display(),
                backup_path.display()
            );
            return ContextTemplateExecution::from_parts(
                Ok(TemplateSyncOutcome {
                    pending_update: None,
                    replacement: None,
                    target_outcome: TemplatePublication::ChangedUnderUs,
                }),
                carried_publication,
            );
        }

        let published_at = match crate::config::session_context::atomically_replace_context_template(
            &path,
            current_default,
            clock,
        ) {
            Ok(published_at) => published_at,
            Err(error) => {
                log::warn!(
                    "[context-templates] replacement failed after backup {} was created: {}",
                    backup_path.display(),
                    error
                );
                return ContextTemplateExecution::from_parts(Err(error), carried_publication);
            }
        };
        loaded.mark_seeded(spec, &current_default_sha256);
        let replacement = notify.then(|| ContextTemplateReplacement {
            project_path: display_path(project_dir),
            workspace_path: display_path(ac_root),
            file_path: display_path(&path),
            filename: spec.filename.to_string(),
            label: spec.label.to_string(),
            backup_path: display_path(&backup_path),
            local_override_path: display_path(
                &ac_root.join(local_override_filename(spec.filename)),
            ),
        });
        let publication = ContextPublication { published_at };
        return ContextTemplateExecution::with_publication(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement,
                target_outcome: TemplatePublication::Published(publication),
            }),
            publication,
        );
    }

    let trusted_entry = loaded.trusted_entry(spec).cloned();
    if let Some(entry) = trusted_entry.as_ref() {
        if entry.last_seeded_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.last_seeded_sha256.as_deref() != Some(current_default_sha256.as_str())
            && spec
                .is_known_generated
                .is_some_and(|matches| matches(&snapshot.content))
        {
            let execution = auto_update_generated_template(&path, spec, &snapshot.sha256, clock);
            let published = execution.published;
            return match execution.completion {
                Ok(target_outcome) => {
                    if matches!(target_outcome, TemplatePublication::Published(_)) {
                        loaded.mark_seeded(spec, &current_default_sha256);
                    }
                    ContextTemplateExecution::from_parts(
                        Ok(TemplateSyncOutcome {
                            pending_update: None,
                            replacement: None,
                            target_outcome,
                        }),
                        published,
                    )
                }
                Err(error) => ContextTemplateExecution::from_parts(Err(error), published),
            };
        }
    }

    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        let execution = auto_update_generated_template(&path, spec, &snapshot.sha256, clock);
        let published = execution.published;
        return match execution.completion {
            Ok(target_outcome) => {
                if matches!(target_outcome, TemplatePublication::Published(_)) {
                    loaded.mark_seeded(spec, &current_default_sha256);
                }
                ContextTemplateExecution::from_parts(
                    Ok(TemplateSyncOutcome {
                        pending_update: None,
                        replacement: None,
                        target_outcome,
                    }),
                    published,
                )
            }
            Err(error) => ContextTemplateExecution::from_parts(Err(error), published),
        };
    }

    if spec.suppress_unknown_without_state && !has_valid_entry {
        log::debug!(
            "[context-templates] preserving ambiguous global context template {} without prompting",
            path.display()
        );
        let target_outcome = carried_publication
            .map(TemplatePublication::Published)
            .unwrap_or(TemplatePublication::Skipped(
                ContextTemplateSkipReason::AmbiguousWithoutState,
            ));
        return ContextTemplateExecution::from_parts(
            Ok(TemplateSyncOutcome {
                pending_update: None,
                replacement: None,
                target_outcome,
            }),
            carried_publication,
        );
    }

    if let Some(entry) = trusted_entry.as_ref() {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            let target_outcome = carried_publication
                .map(TemplatePublication::Published)
                .unwrap_or(TemplatePublication::Skipped(
                    ContextTemplateSkipReason::IgnoredByUser,
                ));
            return ContextTemplateExecution::from_parts(
                Ok(TemplateSyncOutcome {
                    pending_update: None,
                    replacement: None,
                    target_outcome,
                }),
                carried_publication,
            );
        }
    }

    loaded.mark_observed(spec, &snapshot.sha256);
    let pending_update = if return_pending {
        let project_dir = match project_dir {
            Some(project_dir) => project_dir,
            None => {
                return ContextTemplateExecution::from_parts(
                    Err(format!(
                        "Cannot create context template update for {} without a project path",
                        path.display()
                    )),
                    carried_publication,
                )
            }
        };
        Some(make_update(
            project_dir,
            ac_root,
            &path,
            spec,
            snapshot.sha256,
            current_default_sha256,
        ))
    } else {
        log::warn!(
            "[context-templates] preserving customized context template {}; a newer default is available",
            path.display()
        );
        None
    };
    let target_outcome = carried_publication
        .map(TemplatePublication::Published)
        .unwrap_or(TemplatePublication::Observed);
    ContextTemplateExecution::from_parts(
        Ok(TemplateSyncOutcome {
            pending_update,
            replacement: None,
            target_outcome,
        }),
        carried_publication,
    )
}

fn compute_pending_update(
    project_dir: &Path,
    ac_root: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &LoadedState,
) -> Result<Option<ContextTemplateUpdate>, String> {
    // D4: a distribution-owned template is never offered as a pending update.
    if spec.distribution_owned {
        return Ok(None);
    }
    let path = ac_root.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Ok(None);
    };
    if snapshot.sha256 == current_default_sha256 {
        return Ok(None);
    }

    let trusted_entry = loaded.trusted_entry(spec);
    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return Ok(None);
    }
    if spec.suppress_unknown_without_state && !has_valid_entry {
        return Ok(None);
    }
    if let Some(entry) = trusted_entry {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            return Ok(None);
        }
    }

    Ok(Some(make_update(
        project_dir,
        ac_root,
        &path,
        spec,
        snapshot.sha256,
        current_default_sha256,
    )))
}

fn validate_project_ac_root(ac_root: &Path) -> Result<PathBuf, String> {
    validate_existing_dir(ac_root, "Project AC Root")?;
    let name = ac_root.file_name().and_then(|name| name.to_str());
    if name != Some(crate::config::ac_root::canonical_ac_root_label()) {
        return Err(format!(
            "{} is not a Project AC Root directory",
            ac_root.display()
        ));
    }
    ac_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("Project AC Root {} has no parent", ac_root.display()))
}

fn validate_project_ac_root_for_scan(project_dir: &Path, ac_root: &Path) -> Result<(), String> {
    validate_existing_dir(ac_root, "Project AC Root")?;
    let expected = crate::config::ac_root::ac_root_for_project(project_dir);
    if ac_root != expected {
        return Err(format!(
            "Project AC Root {} is not the canonical child of {}",
            ac_root.display(),
            project_dir.display()
        ));
    }
    Ok(())
}

fn consume_template_execution(
    spec: SeededContextTemplateSpec,
    execution: ContextTemplateExecution<TemplateSyncOutcome>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<TemplateSyncOutcome, String> {
    if let Some(publication) = execution.published {
        on_publication(spec.filename, publication);
    }
    if let Ok(outcome) = &execution.completion {
        log::debug!(
            "[context-templates] {} target outcome: {:?}",
            spec.filename,
            outcome.target_outcome
        );
    }
    execution.completion
}

pub fn ensure_project_context_templates(ac_root: &Path) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    ensure_project_context_templates_with_publications(ac_root, &mut on_publication)
}

pub(crate) fn ensure_project_context_templates_with_publications(
    ac_root: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    ensure_project_context_templates_with_clock(ac_root, &mut clock, on_publication)
}

fn ensure_project_context_templates_with_clock(
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    std::fs::create_dir_all(ac_root).map_err(|e| {
        format!(
            "failed to create context templates directory {}: {}",
            ac_root.display(),
            e
        )
    })?;
    validate_existing_dir(ac_root, "Context template directory")?;
    let mut loaded = load_state(ac_root, false)?;
    for spec in project_specs() {
        let execution =
            sync_one_template(None, ac_root, spec, &mut loaded, true, false, false, clock);
        let _ = consume_template_execution(spec, execution, on_publication)?;
    }
    persist_state_best_effort(ac_root, &loaded);
    Ok(())
}

/// #1625: absent-only seed of the three per-execution-platform rule files at
/// render time for an already-known project. Mirrors
/// `ensure_project_context_templates_with_clock` but ONLY for `platform_specs()`
/// (global/coordinator are never touched here; their read-sync stays the
/// managed-filename path in session_context).
pub fn ensure_platform_context_templates(context_dir: &Path) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    ensure_platform_context_templates_with_publications(context_dir, &mut on_publication)
}

pub(crate) fn ensure_platform_context_templates_with_publications(
    context_dir: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    ensure_platform_context_templates_with_clock(context_dir, &mut clock, on_publication)
}

fn ensure_platform_context_templates_with_clock(
    context_dir: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    for spec in platform_specs() {
        let execution = sync_one_template(
            None,
            context_dir,
            spec,
            &mut loaded,
            true,
            false,
            false,
            clock,
        );
        let _ = consume_template_execution(spec, execution, on_publication)?;
    }
    persist_state_best_effort(context_dir, &loaded);
    Ok(())
}

pub fn scan_project_context_template_updates(
    project_dir: &Path,
    ac_root: &Path,
) -> Result<Vec<ContextTemplateUpdate>, String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    scan_project_context_template_updates_with_publications(
        project_dir,
        ac_root,
        &mut on_publication,
    )
}

pub(crate) fn scan_project_context_template_updates_with_publications(
    project_dir: &Path,
    ac_root: &Path,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<Vec<ContextTemplateUpdate>, String> {
    let mut clock = chrono::Utc::now;
    scan_project_context_template_updates_with_clock(
        project_dir,
        ac_root,
        &mut clock,
        on_publication,
    )
}

fn scan_project_context_template_updates_with_clock(
    project_dir: &Path,
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<Vec<ContextTemplateUpdate>, String> {
    scan_project_context_templates_with_clock(project_dir, ac_root, clock, on_publication)
        .map(|(updates, _replacements)| updates)
}

/// #1748: the scan is the only sync path whose return value reaches the UI, so it
/// is the only one that repairs a distribution-owned template (D2). Phase 02 turns
/// the returned replacements into a surfaced value; here they are logged.
fn scan_project_context_templates_with_clock(
    project_dir: &Path,
    ac_root: &Path,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(Vec<ContextTemplateUpdate>, Vec<ContextTemplateReplacement>), String> {
    validate_project_ac_root_for_scan(project_dir, ac_root)?;
    let mut loaded = load_state(ac_root, false)?;
    let mut updates = Vec::new();
    let mut replacements = Vec::new();
    for spec in project_specs() {
        let execution = sync_one_template(
            Some(project_dir),
            ac_root,
            spec,
            &mut loaded,
            false,
            true,
            true,
            clock,
        );
        let outcome = consume_template_execution(spec, execution, on_publication)?;
        if let Some(replacement) = outcome.replacement {
            log::info!(
                "[context-templates] {} was replaced with the current default; the previous bytes are in {}",
                replacement.file_path,
                replacement.backup_path
            );
            replacements.push(replacement);
        }
        if let Some(update) = outcome.pending_update {
            updates.push(update);
        }
    }
    persist_state_best_effort(ac_root, &loaded);
    dedupe_context_template_updates(&mut updates);
    Ok((updates, replacements))
}

pub fn sync_project_context_template_for_read(
    context_dir: &Path,
    filename: &str,
) -> Result<(), String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    sync_project_context_template_for_read_with_publications(
        context_dir,
        filename,
        &mut on_publication,
    )
}

pub(crate) fn sync_project_context_template_for_read_with_publications(
    context_dir: &Path,
    filename: &str,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let mut clock = chrono::Utc::now;
    sync_project_context_template_for_read_with_clock(
        context_dir,
        filename,
        &mut clock,
        on_publication,
    )
}

fn sync_project_context_template_for_read_with_clock(
    context_dir: &Path,
    filename: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> Result<(), String> {
    let Some(spec) = project_spec_by_filename(filename) else {
        return Ok(());
    };
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    let execution = sync_one_template(
        None,
        context_dir,
        spec,
        &mut loaded,
        true,
        false,
        false,
        clock,
    );
    let result = consume_template_execution(spec, execution, on_publication).map(|_| ());
    persist_state_best_effort(context_dir, &loaded);
    result
}

pub fn ensure_root_context_template(config_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| {
        format!(
            "Failed to create root agent config directory {}: {}",
            config_dir.display(),
            e
        )
    })?;
    validate_existing_dir(config_dir, "Root agent config directory")?;
    let mut loaded = load_state(config_dir, false)?;
    let mut clock = chrono::Utc::now;
    let execution = sync_one_template(
        None,
        config_dir,
        root_spec(),
        &mut loaded,
        true,
        false,
        false,
        &mut clock,
    );
    execution.completion?;
    persist_state_best_effort(config_dir, &loaded);
    Ok(())
}

/// #979: retire the standalone global context template that older builds seeded
/// into the app-config directory next to `ac-root-agent`.
///
/// Conservative and lossless. Known generated bytes are deleted; every other byte
/// sequence, including invalid UTF-8, is moved to an inert timestamped backup and
/// kept. On any uncertain classification, bytes are preserved.
///
/// The only caller (`root_agent::ensure_root_agent_dir_at`) consumes every `Err` as
/// a warning and continues, so this may report failures freely. It must simply never
/// destroy bytes and never recreate the active global name.
///
/// File retirement runs BEFORE state cleanup on purpose: two directory entries
/// cannot change in one filesystem transaction, so the sequence is made retryable
/// instead. After any crash boundary the live global is either still present (and
/// retried next run) or already absent with its bytes in an inert backup. Never
/// reverse the order: it would erase the only ownership record while leaving a
/// still-active custom global on disk.
pub(crate) fn retire_standalone_global_context(config_dir: &Path) -> Result<(), String> {
    retire_standalone_global_context_with(
        config_dir,
        crate::config::root_agent::atomic_replace_existing,
    )
}

/// Test seam for the failure paths, mirroring the closure-based filesystem seam in
/// `session_context::migrate_legacy_agent_context_template_with`. `publish` moves the
/// live entry onto the reserved inert name. Production always passes
/// `atomic_replace_existing`; this is not a second production algorithm.
fn retire_standalone_global_context_with(
    config_dir: &Path,
    publish: impl Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    validate_existing_dir(config_dir, "Root agent config directory")?;

    let live_path =
        config_dir.join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    match std::fs::symlink_metadata(&live_path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                // Never follow, delete, or move a symlink, reparse point, or
                // non-file. The caller warns and continues; the entry survives.
                return Err(format!(
                    "Standalone global context {} exists but is not a regular file",
                    live_path.display()
                ));
            }
            retire_live_standalone_global(&live_path, publish)?;
        }
        // No live file: nothing to move, but a stale `global` state entry may still
        // need to converge.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect standalone global context {}: {}",
                live_path.display(),
                e
            ))
        }
    }

    remove_global_state_entry(config_dir)
}

fn retire_live_standalone_global(
    live_path: &Path,
    publish: impl Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let backup_path = reserve_retired_backup_path(live_path)?;

    // Same-directory rename / ReplaceFileW, never a copy followed by a delete, so the
    // bytes are never decoded, re-encoded, or truncated. `atomic_replace_existing`
    // publishes `temp -> dest`, so `(live, backup)` is the correct argument order.
    if let Err(e) = publish(live_path, &backup_path) {
        // Never blindly delete the destination after a failed move. Remove the
        // reservation ONLY when the source is still a regular file AND the
        // destination is still the zero-byte reservation this call created. An empty
        // custom source is a valid unknown file, so "the source disappeared" is never
        // proof that an empty destination is disposable.
        let source_intact = std::fs::symlink_metadata(live_path)
            .map(|m| !is_link_or_reparse(&m) && m.is_file())
            .unwrap_or(false);
        let dest_is_reservation = std::fs::symlink_metadata(&backup_path)
            .map(|m| !is_link_or_reparse(&m) && m.is_file() && m.len() == 0)
            .unwrap_or(false);
        if source_intact && dest_is_reservation {
            if let Err(cleanup_error) = std::fs::remove_file(&backup_path) {
                log::warn!(
                    "[979] failed to remove the unused retirement reservation {}: {}",
                    backup_path.display(),
                    cleanup_error
                );
            }
        } else {
            log::warn!(
                "[979] preserving {} after a failed retirement move: the source or the destination is no longer the entry this call created",
                backup_path.display()
            );
        }
        return Err(e);
    }

    if let Some(parent) = backup_path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[979] failed to sync the retirement directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
    }

    // Re-check before reading: a concurrent replacement could have swapped the inert
    // name for a link, a reparse point, or a directory.
    let metadata = std::fs::symlink_metadata(&backup_path).map_err(|e| {
        format!(
            "Failed to inspect retired context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Retired context backup {} is no longer a regular file",
            backup_path.display()
        ));
    }

    // Classify from RAW BYTES. `read_validated_snapshot` is deliberately NOT reused:
    // it converts to `String` and errors on invalid UTF-8, while #979 requires invalid
    // bytes to SURVIVE. Invalid UTF-8 is automatically custom.
    let bytes = std::fs::read(&backup_path).map_err(|e| {
        format!(
            "Failed to read retired context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    let is_generated = std::str::from_utf8(&bytes)
        .ok()
        .is_some_and(is_known_generated_standalone_global_template);

    if !is_generated {
        log::warn!(
            "[979] {} held unknown or custom content and was moved to the inert backup {}; the Root Agent no longer consumes it",
            live_path.display(),
            backup_path.display()
        );
        return Ok(());
    }

    // Known generated bytes: re-read and compare immediately before deleting. If they
    // differ or cannot be read, keep the backup and report. A delete failure leaves an
    // inert backup and returns an error; it never restores the active global name.
    let recheck = std::fs::read(&backup_path).map_err(|e| {
        format!(
            "Failed to re-read retired context backup {} before deleting it: {}",
            backup_path.display(),
            e
        )
    })?;
    if recheck != bytes {
        return Err(format!(
            "Retired context backup {} changed while it was being classified; keeping it",
            backup_path.display()
        ));
    }
    std::fs::remove_file(&backup_path).map_err(|e| {
        format!(
            "Failed to delete the retired generated context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    log::info!(
        "[979] retired the standalone generated global context {}",
        live_path.display()
    );
    Ok(())
}

/// Reserve a unique inert same-directory name with `create_new(true)` and drop the
/// handle before the move: the shared atomic primitive (and Windows replacement
/// semantics) require the destination handle to be closed.
///
/// The name is ALWAYS timestamped, so it can never collide with `create_backup`'s
/// `{f}.bak` / `{f}.{ts}.bak` / `{f}.{ts}.{n}.bak` shapes. `create_backup` itself is
/// unusable here: it write_all's a COPY and its first name is untimestamped.
fn reserve_retired_backup_path(live_path: &Path) -> Result<PathBuf, String> {
    let parent = live_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for standalone global context {}",
            live_path.display()
        )
    })?;
    let filename = live_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid standalone global context {}", live_path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();

    for index in 0..1000_u32 {
        let backup_name = match index {
            0 => format!("{filename}.retired-{timestamp}.bak"),
            n => format!("{filename}.retired-{timestamp}.{n}.bak"),
        };
        let backup_path = parent.join(backup_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => {
                drop(file);
                return Ok(backup_path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to reserve the retirement backup path {}: {}",
                    backup_path.display(),
                    e
                ))
            }
        }
    }

    Err(format!(
        "Failed to reserve a unique retirement backup path for {}",
        live_path.display()
    ))
}

/// #979 remove ONLY the portable `global` state entry, and never rewrite state this
/// function did not change.
///
/// `persist_state_strict` must never be called here. It writes whenever `dirty` is
/// set, regardless of whether the caller removed anything, and `load_state` returns
/// an EMPTY templates map with `trusted: true`, `can_persist: true`, `dirty: true` on
/// malformed JSON at EITHER strictness. Composing the two would overwrite a corrupt
/// manifest with `{"schemaVersion":1,"templates":{}}` and destroy the `coordinator`
/// and `rootAgent` entries this function is required to preserve.
///
/// `strict = true` is a hazard in its own right: a symlinked or unstattable state
/// file makes `load_state` return `Err`, and although the caller is best-effort, a
/// stale entry is not worth the noise. Never infer generated ownership from
/// `lastSeededSha256` or any other state field.
fn remove_global_state_entry(config_dir: &Path) -> Result<(), String> {
    let mut loaded = load_state(config_dir, false)?;
    if !loaded.trusted || !loaded.can_persist || loaded.dirty {
        log::warn!(
            "[979] portable context-template state at {} is unreadable or malformed; leaving it untouched (the standalone global is already retired)",
            config_dir.display()
        );
        return Ok(());
    }
    // `templates` is a BTreeMap, so the removed entry is the ONLY proof that a write
    // is warranted. Only `global` may ever be removed: `coordinator` and `rootAgent`
    // stay.
    match loaded.state.templates.remove("global") {
        None => Ok(()),
        Some(_) => persist_state(config_dir, &loaded.state),
    }
}

fn validate_expected_hashes(
    project_dir: &Path,
    ac_root: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &LoadedState,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(), String> {
    let current_default_sha256 = sha256_hex((spec.current_content)().as_bytes());
    if current_default_sha256 != expected_default_sha256 {
        return Err(CONTEXT_TEMPLATE_DEFAULT_CHANGED.to_string());
    }
    let Some(pending) = compute_pending_update(project_dir, ac_root, spec, loaded)? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if pending.current_file_sha256 != expected_file_sha256
        || pending.current_default_sha256 != expected_default_sha256
    {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    Ok(())
}

pub fn dismiss_context_template_update(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(), String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_ac_root(ac_root)?;
    let mut loaded = load_state(ac_root, true)?;
    validate_expected_hashes(
        &project_dir,
        ac_root,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;
    let path = ac_root.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    loaded.mark_ignored(spec, expected_file_sha256, expected_default_sha256);
    persist_state_strict(ac_root, &loaded)
}

pub fn overwrite_context_template_with_default(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<ContextTemplateOverwriteResult, String> {
    let mut on_publication = |_: &'static str, _: ContextPublication| {};
    let execution = overwrite_context_template_with_default_with_publications(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
        &mut on_publication,
    );
    if let Some(publication) = execution.published {
        log::debug!(
            "[context-templates] {} overwrite published at {}",
            filename,
            publication.published_at
        );
    }
    execution.completion
}

pub(crate) fn overwrite_context_template_with_default_with_publications(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
) -> ContextTemplateExecution<ContextTemplateOverwriteResult> {
    let mut clock = chrono::Utc::now;
    overwrite_context_template_with_default_with(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
        &mut clock,
        on_publication,
        persist_state_strict,
    )
}

fn prepare_context_template_overwrite(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(SeededContextTemplateSpec, LoadedState, PathBuf, PathBuf), String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_ac_root(ac_root)?;
    let loaded = load_state(ac_root, true)?;
    validate_expected_hashes(
        &project_dir,
        ac_root,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;

    let path = ac_root.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    if sha256_hex((spec.current_content)().as_bytes()) != expected_default_sha256 {
        return Err(CONTEXT_TEMPLATE_DEFAULT_CHANGED.to_string());
    }

    let backup_path = create_backup(&path, &snapshot.bytes)?;
    let Some(snapshot_after_backup) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot_after_backup.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }

    Ok((spec, loaded, path, backup_path))
}

fn overwrite_context_template_with_default_with<P>(
    ac_root: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    on_publication: &mut dyn FnMut(&'static str, ContextPublication),
    persist: P,
) -> ContextTemplateExecution<ContextTemplateOverwriteResult>
where
    P: FnOnce(&Path, &LoadedState) -> Result<(), String>,
{
    let (spec, mut loaded, path, backup_path) = match prepare_context_template_overwrite(
        ac_root,
        filename,
        expected_file_sha256,
        expected_default_sha256,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return ContextTemplateExecution::failed(error),
    };

    let published_at = match crate::config::session_context::atomically_replace_context_template(
        &path,
        (spec.current_content)(),
        clock,
    ) {
        Ok(published_at) => published_at,
        Err(error) => {
            log::warn!(
                "[context-templates] replacement failed after backup {} was created: {}",
                backup_path.display(),
                error
            );
            return ContextTemplateExecution::failed(error);
        }
    };
    let publication = ContextPublication { published_at };
    on_publication(spec.filename, publication);

    loaded.mark_seeded(spec, expected_default_sha256);
    let result = ContextTemplateOverwriteResult {
        file_path: display_path(&path),
        backup_path: display_path(&backup_path),
        current_default_sha256: expected_default_sha256.to_string(),
    };
    ContextTemplateExecution::with_publication(
        persist(ac_root, &loaded).map(|()| result),
        publication,
    )
}

fn create_backup(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for context template {}",
            path.display()
        )
    })?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid context template filename {}", path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();

    for index in 0..1000_u32 {
        let backup_name = match index {
            0 => format!("{filename}.bak"),
            1 => format!("{filename}.{timestamp}.bak"),
            n => format!("{filename}.{timestamp}.{n}.bak"),
        };
        let backup_path = parent.join(backup_name);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create backup context template {}: {}",
                    backup_path.display(),
                    e
                ))
            }
        };
        if let Err(e) = file.write_all(bytes) {
            return Err(format!(
                "Failed to write backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.flush() {
            return Err(format!(
                "Failed to flush backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.sync_all() {
            return Err(format!(
                "Failed to sync backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        drop(file);
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[context-templates] failed to sync backup directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
        return Ok(backup_path);
    }

    Err(format!(
        "Failed to create a unique backup path for {}",
        path.display()
    ))
}

pub fn dedupe_context_template_updates(updates: &mut Vec<ContextTemplateUpdate>) {
    let mut seen = HashSet::new();
    updates.retain(|update| {
        seen.insert((
            update.workspace_path.clone(),
            update.filename.clone(),
            update.current_file_sha256.clone(),
            update.current_default_sha256.clone(),
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::session_context::{
        get_default_coordinator_template, COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
    };

    /// #1614 AC7.9 and AC7.11. #1748 retired the PROJECT global recognizer, so
    /// this constant's only recognizer is now the #979 standalone classifier; the
    /// snapshot must still be accepted by it and be != the current default -- the
    /// assert_ne is what proves the rename actually moved the default rather than
    /// freezing a copy of something unchanged.
    #[test]
    fn frozen_pre_room_rename_global_template_is_recognized() {
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
        ));
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
            crate::config::session_context::get_default_agent_template(),
            "the Room rename must actually change the global default or the freeze is pointless"
        );
        assert!(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("**Workgroup**"));
        assert!(
            !crate::config::session_context::get_default_agent_template()
                .to_lowercase()
                .contains("workgroup")
        );
    }

    /// #1614 AC7.1b: the merge-resolution guard for the #1605 collision. Two
    /// limbs, catching two different failures.
    ///
    /// The `||`-chain limb catches a three-way merge over two adjacent
    /// single-line additions to one recognizer chain silently dropping one of
    /// them: main added `_BEFORE_HOST_PLATFORM_RULES` and this branch added
    /// `_BEFORE_ROOM_RENAME` in the same place, and keeping only the newer one
    /// would stop recognizing every pristine v4 file.
    ///
    /// The `!=` limb is the one that catches the genuinely silent failure:
    /// both recognizer lines kept but the frozen body NOT re-based, which
    /// leaves two constants holding byte-identical bytes under two names with
    /// v5 permanently unrecognized. No conflict, no compile error and no other
    /// criterion in this plan surfaces that; before plan section 15.3's
    /// re-base both constants really were 539 bytes and F4406596...316A.
    #[test]
    fn both_frozen_global_generations_are_standalone_recognized_and_distinct() {
        // The ||-chain limb: #1748 left one recognizer for these bytes, and neither
        // generation may be dropped from it.
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        ));
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
        ));

        // The != limb: the v4 body (main's, #1605) and the v5 body (this
        // plan's, re-based in 15.3) are two different generations.
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
            "the pre-Room-rename snapshot was not re-based onto main's v5 body; \
             two constants hold identical bytes and v5 is unrecognized forever"
        );
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.len(),
            539
        );
        assert_eq!(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(), 564);
        assert!(
            !GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.contains("{{HOST_PLATFORM_RULES}}"),
            "the v4 body predates the placeholder"
        );
        assert!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("{{HOST_PLATFORM_RULES}}"),
            "the v5 body carries the placeholder #1605 introduced"
        );
    }

    #[test]
    fn frozen_pre_room_rename_coordinator_template_is_recognized() {
        assert!(is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
        ));
        assert_ne!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME,
            get_default_coordinator_template(),
            "the Room rename must actually change the coordinator default"
        );
        assert!(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.contains("another workgroup"));
        assert!(!get_default_coordinator_template()
            .to_lowercase()
            .contains("workgroup"));
    }

    /// #1614 AC7.11 at the SPEC layer. The persisted-state layer is asserted by
    /// the read_sync tests, so the bump is pinned on both sides.
    #[test]
    fn seeded_template_versions_were_bumped() {
        let [global, coordinator, ..] = project_specs();
        assert_eq!(global.current_version, 6, "global 5 -> 6");
        assert_eq!(coordinator.current_version, 6, "coordinator 5 -> 6");
        assert_eq!(root_spec().current_version, 8, "rootAgent 7 -> 8");
    }

    /// #1614 AC7.1 and AC7.2. The expected values come from the frozen base and
    /// are written into the plan (section 3.12 Table B), NOT read back from the
    /// constants they check. That is what makes this criterion non-self-
    /// referential: a later coordinated rename that moved both the constant and
    /// its test would change the bytes and these hard-coded values would fail.
    /// Round 1's recognizer criteria all built their expected value by calling
    /// the function they then classified, so they went green under any
    /// internally consistent rename.
    #[test]
    fn frozen_snapshots_are_byte_exact_at_d7008b34() {
        use sha2::{Digest, Sha256};

        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(),
            564,
            "frozen pre-Room-rename global template must be the df494bfa v5 bytes"
        );
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.as_bytes())
            ),
            "d094106b386172e714512dbe1d18cc30a82ff2b25df467f3a1be1c328d464f77",
            "frozen global snapshot changed; every pristine installation would stop auto-updating"
        );

        assert_eq!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(),
            2516,
            "frozen pre-Room-rename coordinator template must be the d7008b34 bytes"
        );
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.as_bytes())
            ),
            "0b89eb38608f6272f0d8087fc7df13ecc729fda716aba972673b15b734a2198e",
            "frozen coordinator snapshot changed; every pristine installation would stop auto-updating"
        );
    }

    // Stage E (#1064) recognized-predecessor exhaustiveness sentinel (plan
    // section 10.2 item 17, 10.6, acceptance item 31): the coordinator recognizer
    // must accept the current default AND every frozen generated predecessor
    // (pre-cross-workgroup v3, pre-token-minimization v2, pre-raise-hand), but not
    // custom content. A mutation that recognizes only one generation fails here.
    // #1748 retired the PROJECT global recognizer, so the arms that asserted its
    // narrowness are gone; the standalone arms below are what remains.
    #[test]
    fn stage_e_all_recognized_coordinator_predecessors_are_generated_and_custom_is_not() {
        assert!(is_known_generated_coordinator_template(
            get_default_coordinator_template()
        ));
        assert!(is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
        ));
        assert!(is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        ));
        assert!(is_known_generated_coordinator_template(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
        ));
        assert!(!is_known_generated_coordinator_template(
            "custom operator-authored coordinator content"
        ));

        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        ));
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
        ));
        assert!(is_known_generated_standalone_global_template(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        ));
    }

    #[test]
    fn project_specs_bump_global_to_v6_and_add_platform_specs() {
        let [global, coordinator, windows, linux, macos] = project_specs();
        assert_eq!(global.id, "global");
        assert_eq!(global.current_version, 6);
        assert_eq!(
            (global.current_content)(),
            crate::config::session_context::get_default_agent_template()
        );
        assert!(
            global.is_known_generated.is_none(),
            "#1748: the global spec is distribution-owned and names no recognizer"
        );

        assert_eq!(coordinator.id, "coordinator");
        assert_eq!(coordinator.current_version, 6);
        assert_eq!(
            (coordinator.current_content)(),
            get_default_coordinator_template()
        );
        assert!(coordinator
            .is_known_generated
            .is_some_and(|matches| matches(
                COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
            )));
        assert!(!coordinator
            .is_known_generated
            .is_some_and(|matches| matches(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION)));

        for (spec, id, filename, default) in [
            (
                windows,
                "platform.windows",
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
            ),
            (
                linux,
                "platform.linux",
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            ),
            (
                macos,
                "platform.macos",
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            ),
        ] {
            assert_eq!(spec.id, id);
            assert_eq!(spec.filename, filename);
            assert_eq!(spec.current_version, 1);
            assert_eq!((spec.current_content)(), default);
            assert!(spec
                .is_known_generated
                .is_some_and(|matches| matches(default)));
            assert!(spec.project_actionable);
            assert!(spec.suppress_unknown_without_state);
        }
        assert!(!windows.is_known_generated.is_some_and(|matches| matches(
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX
        )));
    }

    fn hash_text(content: &str) -> String {
        sha256_hex(content.as_bytes())
    }

    /// #1748: until phase 02 returns the replacements from the public scan, the
    /// tests read them through the internal collector the scan already builds.
    fn scan_project_context_template_replacements_for_test(
        project_dir: &Path,
        ac_root: &Path,
    ) -> Result<Vec<ContextTemplateReplacement>, String> {
        let mut clock = fixed_publication_time;
        let mut on_publication = |_: &'static str, _: ContextPublication| {};
        scan_project_context_templates_with_clock(
            project_dir,
            ac_root,
            &mut clock,
            &mut on_publication,
        )
        .map(|(_updates, replacements)| replacements)
    }

    /// Every `*.bak` in the workspace, sorted. The backup COUNT is what detects a
    /// spurious rewrite; mtime cannot (see the new-test-3 comment).
    fn backup_files(ac_root: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(ac_root)
            .expect("read workspace")
            .map(|entry| entry.expect("workspace entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".bak"))
            })
            .collect();
        found.sort();
        found
    }

    /// A trusted `global` state entry written as raw JSON, so the fixture pins the
    /// wire keys rather than the struct shape phase 01b retypes.
    fn write_trusted_global_state(ac_root: &Path, last_seeded_sha256: &str) {
        std::fs::write(
            ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
            format!(
                concat!(
                    "{{\"schemaVersion\":1,\"templates\":{{\"global\":{{",
                    "\"templateId\":\"global\",\"currentVersion\":3,",
                    "\"lastSeededSha256\":\"{}\"}}}}}}"
                ),
                last_seeded_sha256
            ),
        )
        .expect("write trusted global state");
    }

    fn fixed_publication_time() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-07-20T10:30:45.123Z")
            .expect("parse fixed publication time")
            .with_timezone(&chrono::Utc)
    }

    fn sync_for_read_at(
        ac_root: &Path,
        filename: &str,
        published_at: chrono::DateTime<chrono::Utc>,
    ) -> Vec<(&'static str, ContextPublication)> {
        let mut clock = || published_at;
        let mut publications = Vec::new();
        sync_project_context_template_for_read_with_clock(
            ac_root,
            filename,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("sync for read");
        publications
    }

    fn assert_one_publication(
        publications: &[(&'static str, ContextPublication)],
        filename: &'static str,
        published_at: chrono::DateTime<chrono::Utc>,
    ) {
        assert_eq!(
            publications,
            &[(filename, ContextPublication { published_at })]
        );
    }

    // #1065 Stage F activation coverage. These tests drive the real context
    // publication engine through a held `ProjectSeedManifestGuard` with a production
    // recording closure, asserting the resulting manifest row. Removing the
    // `record_project_context_publication` adapter call would leave no manifest and
    // fail them (plan acceptance item 22).

    #[test]
    fn context_create_records_project_templates_under_the_gate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path();
        let ac_root = project.join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");

        let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
        let published_at = fixed_publication_time();
        let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(project)
            .expect("acquire project gate");
        {
            let mut clock = || published_at;
            let mut on_publication = |filename: &'static str, publication: ContextPublication| {
                crate::config::session_context::record_project_context_publication(
                    &mut guard,
                    &token,
                    filename,
                    publication.published_at,
                );
            };
            ensure_project_context_templates_with_clock(&ac_root, &mut clock, &mut on_publication)
                .expect("ensure project context templates");
        }
        guard.release();

        let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
            .expect("fresh project context creation records a seed manifest");
        assert!(
            manifest.contains("scope = \"context:agentscommander\""),
            "manifest: {manifest}"
        );
        assert!(
            manifest.contains("scope = \"context:coordinator\""),
            "manifest: {manifest}"
        );
        assert!(
            manifest.contains("scope = \"context:platform\""),
            "platform seed publications must record the context:platform scope: {manifest}"
        );
        assert!(
            manifest.contains("kind = \"project_context_template\""),
            "manifest: {manifest}"
        );
    }

    #[test]
    fn coordinator_v3_to_v4_update_records_to_the_seed_manifest_under_the_gate() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path();
        let ac_root = project.join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
        )
        .expect("write pristine v3 coordinator");

        let token = crate::config::seed_manifest::ManifestActivationToken::for_test();
        let published_at = fixed_publication_time();
        let mut guard = crate::config::seed_manifest::ProjectSeedManifestGuard::acquire(project)
            .expect("acquire project gate");
        {
            let mut clock = || published_at;
            let mut on_publication = |filename: &'static str, publication: ContextPublication| {
                crate::config::session_context::record_project_context_publication(
                    &mut guard,
                    &token,
                    filename,
                    publication.published_at,
                );
            };
            sync_project_context_template_for_read_with_clock(
                &ac_root,
                COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
                &mut clock,
                &mut on_publication,
            )
            .expect("sync coordinator for read");
        }
        guard.release();

        let manifest = std::fs::read_to_string(ac_root.join("seed-manifest.toml"))
            .expect("a recognized coordinator v3->v4 update records a seed manifest row");
        assert!(
            manifest.contains("scope = \"context:coordinator\""),
            "manifest: {manifest}"
        );
        assert!(
            manifest.contains("2026-07-20T10:30:45.123Z"),
            "the commit-point publication time is recorded: {manifest}"
        );
    }

    #[test]
    fn old_coordinator_default_is_known_generated_without_raise_hand() {
        assert!(
            !OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Raising Your Hand")
        );
        assert!(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Sending Screenshots")
        );
        assert!(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("names can be misleading.")
        );
        assert!(is_known_generated_coordinator_template(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
        ));
    }

    /// #1005 S4 / G3: the frozen v2 snapshot must stay byte-identical to what the
    /// #684..1dd0b58 builds shipped. Expected values captured by a one-off run of
    /// the shipped accessor AT base commit 1dd0b58, never from this const.
    #[test]
    fn coordinator_pre_token_minimization_snapshot_is_byte_exact() {
        assert_eq!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION.len(),
            2403,
            "frozen v2 coordinator snapshot must be the 1dd0b58 bytes"
        );
        assert_eq!(
            hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION),
            "92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02",
            "frozen v2 coordinator snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    /// #1030: the frozen v3 coordinator snapshot must stay byte-identical to
    /// what shipped at base commit 4acadfe5. Expected values captured by a one-off
    /// run of the shipped accessor AT 4acadfe5 (plan E2), never from this const.
    #[test]
    fn coordinator_pre_cross_workgroup_snapshot_is_byte_exact() {
        assert_eq!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE.len(),
            2296,
            "frozen v3 coordinator snapshot must be the 4acadfe5 bytes"
        );
        assert_eq!(
            hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE),
            "9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84",
            "frozen v3 coordinator snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    /// #1571 T1: the frozen v4 coordinator snapshot must stay byte-identical to
    /// what shipped at base commit ecc6527b. Expected values captured by a one-off
    /// run of the shipped accessor AT ecc6527b (plan 3.6), never from this const.
    #[test]
    fn coordinator_pre_orchestrator_rename_snapshot_is_byte_exact() {
        assert_eq!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME.len(),
            2509,
            "frozen v4 coordinator snapshot must be the ecc6527b bytes"
        );
        assert_eq!(
            hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME),
            "f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6",
            "frozen v4 coordinator snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    /// #1571 T5: the frozen v1 coordinator snapshot had no byte pin, and the
    /// existing `old_coordinator_default_is_known_generated_without_raise_hand`
    /// cannot detect an edit to it: its recognizer assertion is self-referential
    /// and its three `contains()` assertions do not cover the first line, which is
    /// the very sentence the #1571 rename rewrites elsewhere. Expected values
    /// captured by a one-off run AT ecc6527b (plan 9.1), never from this const.
    #[test]
    fn old_coordinator_raise_hand_snapshot_is_byte_exact() {
        assert_eq!(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.len(),
            2066,
            "frozen v1 coordinator snapshot must be the ecc6527b bytes"
        );
        assert_eq!(
            hash_text(OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND),
            "31d49d02c12fcc8cd2d5277455074dcae3fbc1a84f1f1a0cf0f37828e03f792f",
            "frozen v1 coordinator snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    /// #1005 S4 failing-first migration proof: the assert_ne fails while the live
    /// template still equals the frozen v2 bytes (pre-rewrite), and the sync half
    /// proves a pristine v2 file on disk auto-upgrades to the current default.
    #[test]
    fn read_sync_updates_pre_token_minimization_coordinator_template() {
        assert_ne!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
            get_default_coordinator_template(),
            "v3 rewrite must actually change the template or the freeze is pointless"
        );
        assert!(is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        ));

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        )
        .expect("write pristine v2 coordinator");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    /// #1571 T2, failing-first migration proof for the fifth recognizer arm: the
    /// assert_ne fails while the live template still equals the frozen v4 bytes
    /// (pre-rewrite), and the sync half proves a pristine v4 file on disk
    /// auto-upgrades to the current default.
    ///
    /// The direct recognizer assertion is deliberately LAST, unlike the sibling
    /// above. With the fifth arm deleted, a first-position direct assert panics
    /// before the sync runs, so a mutation probe would only prove that a predicate
    /// whose one matching arm was just removed returns false. Asserting the
    /// behavior first makes the probe prove that the sync path actually consumes
    /// the recognizer, which is the silent-half-migration risk it exists to close.
    #[test]
    fn read_sync_updates_pre_orchestrator_rename_coordinator_template() {
        assert_ne!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
            get_default_coordinator_template(),
            "the #1571 rename must actually change the template or the freeze is pointless"
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME,
        )
        .expect("write pristine v4 coordinator");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());

        assert!(is_known_generated_coordinator_template(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME
        ));
    }

    /// #1005 S6 / G3: the frozen v1 global snapshot must stay byte-identical to
    /// what the #658..ec660c17 builds shipped. Expected values captured by a
    /// one-off run of the shipped accessor AT base commit ec660c17, never from
    /// this const.
    #[test]
    fn global_pre_token_minimization_snapshot_is_byte_exact() {
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION.len(),
            611,
            "frozen v1 global snapshot must be the ec660c17 bytes"
        );
        assert_eq!(
            hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION),
            "c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88",
            "frozen v1 global snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    /// #1005 S6 failing-first proof for the v2 rewrite (assert_ne), the #979
    /// standalone recognizer and the version bump, retargeted by #1748 at the scan:
    /// the distribution repair, not the read path, is what lands the current
    /// default on an old pristine file.
    #[test]
    fn scan_replaces_pre_token_minimization_global_template() {
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
            crate::config::session_context::get_default_agent_template(),
            "v2 rewrite must actually change the template or the freeze is pointless"
        );
        assert!(
            is_known_generated_standalone_global_template(
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
            ),
            "standalone (retirement) recognizer must accept the frozen v1 bytes"
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        )
        .expect("write pristine v1 global");

        assert!(
            scan_project_context_template_updates(temp.path(), &ac_root)
                .expect("scan updates")
                .is_empty(),
            "a distribution-owned template never yields a pending update"
        );

        let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read global");
        assert_eq!(
            content,
            crate::config::session_context::get_default_agent_template(),
            "pristine v1 Context.AgentsCommander.md must be repaired to the default"
        );
        let backups = backup_files(&ac_root);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
            "the backup must hold the pre-run bytes"
        );
        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        assert_eq!(
            parsed["templates"]["global"]["currentVersion"], 6,
            "the repaired global must land on the current v6 default"
        );
    }

    /// #1369 (C4) AC-4.3: the frozen v2 global snapshot must stay byte-identical
    /// to what the #1005 S6..8f272a76 builds shipped. Expected values captured
    /// externally from the raw literal AT base commit 8f272a76, never from this
    /// const.
    #[test]
    fn global_pre_agent_repos_snapshot_is_byte_exact() {
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS.len(),
            567,
            "frozen v2 global snapshot must be the 8f272a76 bytes"
        );
        assert_eq!(
            hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS),
            "e5861a9f011967e96e5515f858e1643f7fdf161511ad909fe86ddb4ce1a0cff7",
            "frozen v2 global snapshot changed; it must stay byte-identical to what shipped"
        );
    }

    #[test]
    fn global_before_summarization_snapshot_is_byte_exact() {
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION.len(),
            563,
            "frozen v3 global snapshot must be the 6aae531e bytes"
        );
        assert_eq!(
            hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION),
            "99a0aa4a15062d4b68b94597111ae268958cbeb4e3902aafe1b7361b63d34157",
            "frozen v3 global snapshot changed; it must stay byte-identical to what shipped"
        );
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
            crate::config::session_context::get_default_agent_template(),
            "the v4 summarization must differ from its frozen v3 operand"
        );
    }

    /// #1605: the frozen v4 global snapshot must stay byte-identical to what
    /// shipped at base commit 047248bc. Expected values captured by a one-off
    /// run of the shipped accessor AT 047248bc (len 539, sha256
    /// f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a),
    /// never from this const.
    #[test]
    fn global_before_host_platform_rules_snapshot_is_byte_exact() {
        assert_eq!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES.len(),
            539,
            "frozen v4 global snapshot must be the 047248bc bytes"
        );
        assert_eq!(
            hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES),
            "f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a",
            "frozen v4 global snapshot changed; it must stay byte-identical to what shipped"
        );
        assert!(
            is_known_generated_standalone_global_template(
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
            ),
            "the standalone (retirement) recognizer must accept the frozen v4 bytes"
        );
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
            crate::config::session_context::get_default_agent_template(),
            "the v5 placeholder insertion must differ from its frozen v4 operand"
        );
    }

    #[test]
    fn global_before_summarization_is_an_exact_generated_operand() {
        let one_byte = format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION}X");
        let crlf = GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION.replace('\n', "\r\n");
        let recognizers = [(
            "standalone",
            is_known_generated_standalone_global_template as fn(&str) -> bool,
        )];

        for (label, recognizes) in recognizers {
            assert!(
                recognizes(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION),
                "{label} recognizer must accept the exact v3 operand"
            );
            assert!(!recognizes(&one_byte), "{label} accepted v3 + X");
            assert!(!recognizes(&crlf), "{label} accepted CRLF v3");
        }
    }

    #[test]
    fn scan_replaces_pristine_v3_global_template_without_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
        )
        .expect("write pristine v3 global");

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pristine v3 global");
        assert_eq!(
            replacements.len(),
            1,
            "with no state entry the replacement is notified"
        );
        let current = crate::config::session_context::get_default_agent_template();
        let current_hash = hash_text(current);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read repaired global"),
            current
        );
        assert_eq!(backup_files(&ac_root).len(), 1);
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state"),
        )
        .expect("parse seeded state");
        assert_eq!(state["templates"]["global"]["currentVersion"], 6);
        assert_eq!(
            state["templates"]["global"]["lastSeededSha256"],
            current_hash
        );

        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("second scan")
                .is_empty(),
            "a second scan must produce no replacement"
        );
        assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
        assert!(scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty());
    }

    #[test]
    fn scan_replaces_pristine_v3_global_template_with_trusted_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
        )
        .expect("write pristine v3 global");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "global".to_string(),
            SeededContextTemplateEntry {
                template_id: "global".to_string(),
                current_version: 3,
                last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION)),
                last_observed_sha256: None,
                ignored_default_sha256: None,
                ignored_observed_sha256: None,
            },
        );
        persist_state(&ac_root, &state).expect("persist trusted v3 state");

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pristine v3 global");
        assert!(
            replacements.is_empty(),
            "a trusted entry naming these exact bytes makes the repair silent"
        );
        let current = crate::config::session_context::get_default_agent_template();
        let current_hash = hash_text(current);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read repaired global"),
            current
        );
        assert_eq!(
            backup_files(&ac_root).len(),
            1,
            "silent does not mean unbacked-up"
        );
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state"),
        )
        .expect("parse seeded state");
        assert_eq!(state["templates"]["global"]["currentVersion"], 6);
        assert_eq!(
            state["templates"]["global"]["lastSeededSha256"],
            current_hash
        );

        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("second scan")
                .is_empty(),
            "a second scan must produce no replacement"
        );
        assert_eq!(backup_files(&ac_root).len(), 1, "and no new backup");
        assert!(scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .is_empty());
    }

    /// #1748 inverted this: a near match is no longer a preserved category. It is
    /// drift like any other and the scan replaces it, in both state shapes. The
    /// read path still leaves it alone, which is D2.
    #[test]
    fn scan_replaces_v3_global_near_matches_in_both_project_state_shapes() {
        let variants = [
            (
                "one-byte",
                format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION}X").into_bytes(),
            ),
            (
                "crlf",
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
                    .replace('\n', "\r\n")
                    .into_bytes(),
            ),
        ];

        for (label, bytes) in variants {
            for trusted_state in [false, true] {
                let temp = tempfile::tempdir().expect("tempdir");
                let ac_root = temp.path().join(".ac");
                std::fs::create_dir(&ac_root).expect("create workspace");
                std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), &bytes)
                    .expect("write v3 near match");
                if trusted_state {
                    let mut state = SeededContextTemplateState::default();
                    state.templates.insert(
                        "global".to_string(),
                        SeededContextTemplateEntry {
                            template_id: "global".to_string(),
                            current_version: 3,
                            last_seeded_sha256: Some(hash_text(
                                GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
                            )),
                            last_observed_sha256: None,
                            ignored_default_sha256: None,
                            ignored_observed_sha256: None,
                        },
                    );
                    persist_state(&ac_root, &state).expect("persist trusted v3 state");
                }

                let published_at = fixed_publication_time();
                assert!(
                    sync_for_read_at(&ac_root, GLOBAL_CONTEXT_TEMPLATE_FILENAME, published_at)
                        .is_empty(),
                    "{label}/{trusted_state}: the read path must not publish a repair"
                );
                assert_eq!(
                    std::fs::read(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                        .expect("read preserved near match"),
                    bytes,
                    "{label}/{trusted_state}: the read path changed bytes"
                );

                let replacements =
                    scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                        .expect("scan v3 near match");
                assert_eq!(
                    replacements.len(),
                    1,
                    "{label}/{trusted_state}: a near match is drift and is notified"
                );
                let current = crate::config::session_context::get_default_agent_template();
                assert_eq!(
                    std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                        .expect("read repaired near match"),
                    current,
                    "{label}/{trusted_state}: the scan must repair a near match"
                );
                let backups = backup_files(&ac_root);
                assert_eq!(backups.len(), 1, "{label}/{trusted_state}: {backups:?}");
                assert_eq!(
                    std::fs::read(&backups[0]).expect("read backup"),
                    bytes,
                    "{label}/{trusted_state}: the backup must hold the near-match bytes"
                );

                assert!(
                    scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                        .expect("second scan v3 near match")
                        .is_empty(),
                    "{label}/{trusted_state}: the second scan is idempotent"
                );
                assert_eq!(backup_files(&ac_root).len(), 1, "{label}/{trusted_state}");
                assert!(
                    scan_project_context_template_updates(temp.path(), &ac_root)
                        .expect("second scan v3 near match")
                        .is_empty(),
                    "{label}/{trusted_state}: never a pending update"
                );
            }
        }
    }

    /// #1748 inverted this the same way as the near-match test: a fine custom
    /// global is replaced by the scan, in both state shapes.
    #[test]
    fn scan_replaces_the_compact_fine_token_global_in_both_state_shapes() {
        const FINE_TEMPLATE: &str = "# Fine custom context\n{{AGENT_ROOT}}{{MATRIX_SECTION}}{{MESSAGING_EXCEPTION}}{{MATRIX_ALLOWED}}{{MESSAGING_ALLOWED}}{{FORBIDDEN_SCOPE}}{{GIT_SCOPE}}{{SKILLS_SECTION}}{{PEER_NAME_FORMAT}}{{SEND_MESSAGE_INSTRUCTIONS}}\n";
        assert!(!is_known_generated_standalone_global_template(
            FINE_TEMPLATE
        ));

        for trusted_state in [false, true] {
            let temp = tempfile::tempdir().expect("tempdir");
            let ac_root = temp.path().join(".ac");
            std::fs::create_dir(&ac_root).expect("create workspace");
            std::fs::write(
                ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                FINE_TEMPLATE,
            )
            .expect("write fine custom global");
            if trusted_state {
                let mut state = SeededContextTemplateState::default();
                state.templates.insert(
                    "global".to_string(),
                    SeededContextTemplateEntry {
                        template_id: "global".to_string(),
                        current_version: 3,
                        last_seeded_sha256: Some(hash_text(
                            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
                        )),
                        last_observed_sha256: None,
                        ignored_default_sha256: None,
                        ignored_observed_sha256: None,
                    },
                );
                persist_state(&ac_root, &state).expect("persist trusted prior state");
            }

            assert!(
                sync_for_read_at(
                    &ac_root,
                    GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                    fixed_publication_time(),
                )
                .is_empty(),
                "the read path must not publish a repair"
            );
            assert_eq!(
                std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                    .expect("read fine custom global"),
                FINE_TEMPLATE
            );

            let replacements =
                scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                    .expect("scan fine custom global");
            assert_eq!(
                replacements.len(),
                1,
                "{trusted_state}: fine custom content is drift and is notified"
            );
            assert!(
                scan_project_context_template_updates(temp.path(), &ac_root)
                    .expect("scan fine custom global")
                    .is_empty(),
                "{trusted_state}: never a pending update"
            );
            assert_eq!(
                std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                    .expect("re-read repaired global"),
                crate::config::session_context::get_default_agent_template()
            );
            let backups = backup_files(&ac_root);
            assert_eq!(backups.len(), 1, "{trusted_state}: {backups:?}");
            assert_eq!(
                std::fs::read_to_string(&backups[0]).expect("read backup"),
                FINE_TEMPLATE
            );
        }
    }

    /// #1605 failing-first proof for the v5 placeholder insertion (assert_ne), the
    /// #979 standalone recognizer and the version bump, retargeted by #1748 at the
    /// scan.
    #[test]
    fn scan_replaces_pre_host_platform_rules_global_template() {
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
            crate::config::session_context::get_default_agent_template(),
            "the v5 rewrite must actually change the template or the freeze is pointless"
        );
        assert!(
            is_known_generated_standalone_global_template(
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
            ),
            "standalone (retirement) recognizer must accept the frozen v4 bytes"
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        )
        .expect("write pristine v4 global");

        assert_eq!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pristine v4 global")
                .len(),
            1
        );

        let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read global");
        assert_eq!(
            content,
            crate::config::session_context::get_default_agent_template(),
            "pristine v4 Context.AgentsCommander.md must be repaired to v6"
        );
        let backups = backup_files(&ac_root);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        );
        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        assert_eq!(
            parsed["templates"]["global"]["currentVersion"], 6,
            "the repaired global must land on the current v6 default"
        );
    }

    /// #1369 (C4) AC-4.5-C: population C, the edge case - a pristine v2 on disk
    /// with NO `global` state entry. Failing-first proof for the v3 rename
    /// (assert_ne), the #979 standalone recognizer and the version bump,
    /// retargeted by #1748 at the scan.
    #[test]
    fn scan_replaces_pre_agent_repos_global_template() {
        assert_ne!(
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
            crate::config::session_context::get_default_agent_template(),
            "the v3 rename must actually change the template or the freeze is pointless"
        );
        assert!(
            is_known_generated_standalone_global_template(
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
            ),
            "standalone (retirement) recognizer must accept the frozen v2 bytes"
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
        )
        .expect("write pristine v2 global");

        assert_eq!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan pristine v2 global")
                .len(),
            1,
            "population C has no state entry, so the replacement is notified"
        );

        let content = std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
            .expect("read global");
        assert_eq!(
            content,
            crate::config::session_context::get_default_agent_template(),
            "pristine v2 Context.AgentsCommander.md must be repaired"
        );
        assert_eq!(backup_files(&ac_root).len(), 1);
        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        assert_eq!(parsed["templates"]["global"]["currentVersion"], 6);
        assert!(
            scan_project_context_template_updates(temp.path(), &ac_root)
                .expect("scan updates")
                .is_empty(),
            "a repaired template must not leave a pending update"
        );
    }

    /// #1369 (C4) AC-4.5-B: population B, the dominant one - a pristine v2 on
    /// disk PLUS a `global` state entry whose `lastSeededSha256` is the v2 hash,
    /// which is what any project seeded by a normal build carries. Without the
    /// freeze this yields 1 pending update, i.e. the modal telling the user that
    /// a file they never touched "appears customized".
    #[test]
    fn scan_replaces_pre_agent_repos_global_template_with_state_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
        )
        .expect("write pristine v2 global");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "global".to_string(),
            SeededContextTemplateEntry {
                template_id: "global".to_string(),
                current_version: 2,
                last_seeded_sha256: Some(hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS)),
                last_observed_sha256: None,
                ignored_default_sha256: None,
                ignored_observed_sha256: None,
            },
        );
        persist_state(&ac_root, &state).expect("persist v2 state entry");

        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan the dominant population")
                .is_empty(),
            "the state names these exact bytes, so the repair is silent"
        );

        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read global"),
            crate::config::session_context::get_default_agent_template(),
            "a pristine v2 with a trusted state entry must be repaired silently"
        );
        assert_eq!(backup_files(&ac_root).len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state"),
        )
        .expect("parse seeded state");
        assert_eq!(parsed["templates"]["global"]["currentVersion"], 6);
        assert!(
            scan_project_context_template_updates(temp.path(), &ac_root)
                .expect("scan updates")
                .is_empty(),
            "the dominant population must never be accused of having customized the file"
        );
    }

    #[test]
    fn scan_existing_ac_does_not_create_missing_templates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");

        let updates =
            scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

        assert!(updates.is_empty());
        assert!(!ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists());
        assert!(!ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).exists());
    }

    #[test]
    fn equal_default_observation_has_no_publication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write current global default");

        let publications = sync_for_read_at(
            &ac_root,
            GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            fixed_publication_time(),
        );

        assert!(
            publications.is_empty(),
            "observing equal default bytes must not manufacture a publication"
        );
    }

    #[test]
    fn generated_update_lost_race_has_no_publication_or_clock_sample() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let path = ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(
            &path,
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        )
        .expect("write generated coordinator");
        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("coordinator spec");
        let mut clock_calls = 0_u32;
        let mut clock = || {
            clock_calls += 1;
            fixed_publication_time()
        };

        let execution =
            auto_update_generated_template(&path, spec, "different-observed-hash", &mut clock);

        assert_eq!(execution.published, None);
        assert_eq!(
            execution.completion.expect("lost race outcome"),
            TemplatePublication::ChangedUnderUs
        );
        assert_eq!(clock_calls, 0);
        assert_eq!(
            std::fs::read_to_string(path).expect("read preserved target"),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        );
    }

    #[test]
    fn generated_update_publish_failure_has_no_publication() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(
            &path,
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        )
        .expect("write generated coordinator");
        let expected_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION);
        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("coordinator spec");
        let mut clock = || fixed_publication_time();

        let execution = auto_update_generated_template_with(
            &path,
            spec,
            &expected_hash,
            &mut clock,
            |_, _, _| Err("injected atomic publication failure".to_string()),
        );

        assert_eq!(execution.published, None);
        assert_eq!(
            execution
                .completion
                .expect_err("publication failure must remain an error"),
            "injected atomic publication failure"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("read preserved target"),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        );
    }

    #[cfg(windows)]
    #[test]
    fn readonly_generated_update_has_no_publication_or_clock_sample() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(
            &path,
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION,
        )
        .expect("write generated coordinator");
        let expected_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION);
        let original_permissions = std::fs::metadata(&path)
            .expect("read target metadata")
            .permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("make target read-only");
        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("coordinator spec");
        let mut clock_calls = 0_u32;
        let mut clock = || {
            clock_calls += 1;
            fixed_publication_time()
        };

        let execution = auto_update_generated_template(&path, spec, &expected_hash, &mut clock);

        std::fs::set_permissions(&path, original_permissions).expect("restore target permissions");
        assert_eq!(execution.published, None);
        assert!(execution.completion.is_err());
        assert_eq!(clock_calls, 0);
        assert_eq!(
            std::fs::read_to_string(path).expect("read preserved target"),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        );
    }

    #[test]
    fn create_publication_survives_post_commit_validation_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let published_at = fixed_publication_time();
        let mut clock = || {
            std::fs::remove_file(&path).expect("remove published target at validation seam");
            std::fs::create_dir(&path).expect("replace published target with directory");
            published_at
        };

        let execution = create_missing_template(&path, "published bytes", &mut clock);

        assert_eq!(
            execution.published,
            Some(ContextPublication { published_at })
        );
        assert!(execution
            .completion
            .expect_err("post-commit validation must fail")
            .contains("not a regular file"));
    }

    #[test]
    fn ensure_consumes_first_publication_before_the_next_target_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::create_dir(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("block coordinator target with directory");
        let published_at = fixed_publication_time();
        let mut clock = || published_at;
        let mut publications = Vec::new();

        let error = ensure_project_context_templates_with_clock(
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect_err("second target must fail");

        assert!(error.contains("not a regular file"), "{error}");
        assert_one_publication(
            &publications,
            GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read first published target"),
            crate::config::session_context::get_default_agent_template()
        );
    }

    /// #1605: a fresh `.ac` root seeds the three platform files byte-equal to
    /// their embedded defaults with `platform.*` state entries carrying the
    /// default sha; a pre-existing custom platform file is never overwritten
    /// (absent-only) and is preserved silently.
    #[test]
    fn ensure_project_context_templates_seeds_platform_files_absent_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");

        let custom_windows = "## Host Platform Rules\n\nMY OWN WINDOWS RULES\n";
        std::fs::write(
            ac_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS),
            custom_windows,
        )
        .expect("write pre-existing custom windows platform file");

        let published_at = fixed_publication_time();
        let mut clock = || published_at;
        let mut publications = Vec::new();
        ensure_project_context_templates_with_clock(
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("ensure project context templates");

        assert_eq!(
            std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                .expect("read global"),
            crate::config::session_context::get_default_agent_template()
        );
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            get_default_coordinator_template()
        );
        for (filename, default) in [
            (
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            ),
            (
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            ),
        ] {
            assert_eq!(
                std::fs::read_to_string(ac_root.join(filename)).expect("read seeded platform file"),
                default,
                "{filename} must be seeded byte-equal to its embedded default"
            );
        }
        assert_eq!(
            std::fs::read_to_string(
                ac_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS)
            )
            .expect("read preserved windows platform file"),
            custom_windows,
            "a pre-existing custom platform file must be preserved, never overwritten"
        );

        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        for (id, default) in [
            (
                "platform.linux",
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            ),
            (
                "platform.macos",
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            ),
        ] {
            assert_eq!(
                parsed["templates"][id]["currentVersion"], 1,
                "{id} state entry must be v1"
            );
            assert_eq!(
                parsed["templates"][id]["lastSeededSha256"],
                hash_text(default),
                "{id} state entry must carry the default sha"
            );
        }
        assert_eq!(
            parsed["templates"]["platform.windows"],
            serde_json::Value::Null,
            "a stateless pre-existing custom platform file must stay unowned (same posture as the coordinator template)"
        );
    }

    /// #1625 T-3: `ensure_platform_context_templates` seeds ONLY the missing
    /// platform files, byte-equal to their embedded defaults, with `platform.*`
    /// state entries v1 carrying the default sha; global/coordinator templates
    /// are never touched (scope is platform-only). A pre-existing custom
    /// platform file is preserved and stays unowned (silent preservation via
    /// `suppress_unknown_without_state`).
    #[test]
    fn ensure_platform_context_templates_seeds_only_missing_platform_files() {
        let platform_files = [
            (
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
            ),
            (
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            ),
            (
                crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            ),
        ];
        let assert_platform_only_scope = |ac_root: &Path| {
            assert!(
                !ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists(),
                "global template is out of scope for the platform seeder"
            );
            assert!(
                !ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).exists(),
                "coordinator template is out of scope for the platform seeder"
            );
        };

        // Fresh `.ac`: all three platform files are created byte-equal to their
        // embedded defaults and the state records three `platform.*` entries v1
        // with `lastSeededSha256` = hash of the default.
        let temp = tempfile::tempdir().expect("tempdir");
        let fresh = temp.path().join(".ac");
        std::fs::create_dir(&fresh).expect("create workspace");
        ensure_platform_context_templates(&fresh).expect("seed platform templates");
        for (filename, default) in platform_files {
            assert_eq!(
                std::fs::read_to_string(fresh.join(filename)).expect("read seeded platform file"),
                default,
                "{filename} must be seeded byte-equal to its embedded default"
            );
        }
        let state = std::fs::read_to_string(fresh.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        for (id, default) in [
            (
                "platform.windows",
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
            ),
            (
                "platform.linux",
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
            ),
            (
                "platform.macos",
                crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
            ),
        ] {
            assert_eq!(
                parsed["templates"][id]["currentVersion"], 1,
                "{id} state entry must be v1"
            );
            assert_eq!(
                parsed["templates"][id]["lastSeededSha256"],
                hash_text(default),
                "{id} state entry must carry the default sha"
            );
        }
        assert_platform_only_scope(&fresh);

        // Pre-existing custom file: preserved byte-for-byte and left unowned
        // (no state entry); the other two are still seeded absent-only.
        let temp2 = tempfile::tempdir().expect("tempdir");
        let custom_root = temp2.path().join(".ac");
        std::fs::create_dir(&custom_root).expect("create workspace");
        let custom_windows = "## Host Platform Rules\n\nMY OWN WINDOWS RULES\n";
        std::fs::write(
            custom_root.join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS),
            custom_windows,
        )
        .expect("write pre-existing custom windows platform file");
        ensure_platform_context_templates(&custom_root).expect("seed platform templates");
        assert_eq!(
            std::fs::read_to_string(
                custom_root
                    .join(crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS)
            )
            .expect("read preserved windows platform file"),
            custom_windows,
            "a pre-existing custom platform file must be preserved, never overwritten"
        );
        for (filename, default) in platform_files.iter().filter(|(filename, _)| {
            *filename != crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS
        }) {
            assert_eq!(
                std::fs::read_to_string(custom_root.join(filename))
                    .expect("read seeded platform file"),
                *default,
                "{filename} must be seeded byte-equal to its embedded default"
            );
        }
        let state2 =
            std::fs::read_to_string(custom_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read seeded state");
        let parsed2: serde_json::Value = serde_json::from_str(&state2).expect("parse seeded state");
        assert_eq!(
            parsed2["templates"]["platform.windows"],
            serde_json::Value::Null,
            "a stateless pre-existing custom platform file must stay unowned"
        );
        assert_eq!(
            parsed2["templates"]["platform.linux"]["currentVersion"], 1,
            "platform.linux must be seeded in the mixed scenario"
        );
        assert_eq!(
            parsed2["templates"]["platform.macos"]["currentVersion"], 1,
            "platform.macos must be seeded in the mixed scenario"
        );
        assert_platform_only_scope(&custom_root);
    }

    /// #1605: after seeding, an edit to a platform file is preserved by the
    /// sync path (content unchanged) and lands in the observed posture
    /// (`lastObservedSha256` updated); a scan against the same default yields no
    /// pending update, and the file is never silently overwritten.
    #[test]
    fn platform_file_edit_is_preserved_and_observed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        ensure_project_context_templates(&ac_root).expect("seed platform files");

        let filename = crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS;
        let edited = format!(
            "{}\n\nMY OWN WINDOWS RULES\n",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS
        );
        std::fs::write(ac_root.join(filename), &edited).expect("edit windows platform file");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(&ac_root, filename, published_at);
        assert!(
            publications.is_empty(),
            "an edited platform file must not publish"
        );
        assert_eq!(
            std::fs::read_to_string(ac_root.join(filename)).expect("read edited platform file"),
            edited,
            "the edit must be preserved, never overwritten"
        );

        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        assert_eq!(
            parsed["templates"]["platform.windows"]["lastObservedSha256"],
            hash_text(&edited),
            "the edit must land in the observed posture"
        );

        let updates =
            scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");
        assert_eq!(
            updates.len(),
            1,
            "a customized platform file must be offered as a pending update"
        );
        assert_eq!(updates[0].filename, filename);
        assert_eq!(updates[0].current_file_sha256, hash_text(&edited));
        assert_eq!(
            updates[0].current_default_sha256,
            hash_text(crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS)
        );
        assert_eq!(updates[0].current_default_version, 1);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(filename)).expect("re-read edited platform file"),
            edited,
            "the scan must never silently overwrite the edit"
        );
    }

    #[test]
    fn read_sync_creates_missing_coordinator_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    #[test]
    fn read_sync_updates_old_generated_coordinator_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND,
        )
        .expect("write old coordinator");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    /// #1030 failing-first migration proof for the STATELESS population (E4 row 4):
    /// the assert_ne is the only check that fails if the v4 rule edit is skipped
    /// while the freeze lands, and the assert! is the only one that fails if the
    /// const is never wired into `is_known_generated_coordinator_template`. The
    /// sync half proves a pristine v3 body with no state file auto-upgrades.
    #[test]
    fn read_sync_updates_pristine_v3_coordinator_template() {
        assert_ne!(
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
            get_default_coordinator_template(),
            "the v4 edit must actually change the template or the freeze is pointless"
        );
        assert!(
            is_known_generated_coordinator_template(
                COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
            ),
            "the recognizer must accept the frozen v3 bytes"
        );

        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
        )
        .expect("write pristine v3 coordinator");

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    /// #1030: the SEEDED population (E4 row 3), which is the branch every existing
    /// workspace with a trusted state entry is in, so this is the test that proves
    /// the migration actually reaches them. A pristine v3 body whose
    /// lastSeededSha256 equals the file hash auto-updates and persists the bump.
    #[test]
    fn read_sync_updates_seeded_v3_coordinator_and_bumps_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
        )
        .expect("write pristine v3 coordinator");
        std::fs::write(
            ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
            format!(
                concat!(
                    r#"{{"schemaVersion":1,"templates":{{"coordinator":{{"#,
                    r#""templateId":"coordinator","currentVersion":3,"#,
                    r#""lastSeededSha256":"{}","lastObservedSha256":null,"#,
                    r#""ignoredDefaultSha256":null,"ignoredObservedSha256":null}}}}}}"#
                ),
                hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE)
            ),
        )
        .expect("write seeded v3 state");

        // Pin the E4 row 3 preconditions before syncing. Every assertion after the sync
        // call also passes on row 4 (:831-836), which auto-updates to the same v4 body
        // when there is no entry at all, and `load_state` (:552-565) turns an unparseable
        // fixture into a trusted empty map rather than an error. So without this block a
        // fixture that never parses still leaves the test green while exercising row 4.
        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("coordinator spec by filename");
        let pre_sync = load_state(&ac_root, true).expect("load the hand-written v3 state");
        let entry = pre_sync
            .trusted_entry(spec)
            .expect("the v3 fixture must parse into a trusted coordinator entry (row 3)");
        let v3_sha256 = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE);
        let v4_sha256 = hash_text(get_default_coordinator_template());
        assert_eq!(
            entry.current_version, 3,
            "row 3 requires the fixture to describe a seeded v3 coordinator"
        );
        assert_eq!(
            entry.last_seeded_sha256.as_deref(),
            Some(v3_sha256.as_str()),
            "row 3 requires lastSeededSha256 to match the pristine v3 body on disk"
        );
        assert_ne!(
            v3_sha256, v4_sha256,
            "row 3 requires the seeded hash to differ from the current v4 default"
        );
        assert!(
            is_known_generated_coordinator_template(
                COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
            ),
            "row 3 requires the pristine v3 body to be recognized as generated"
        );

        let published_at = fixed_publication_time();
        let publications = sync_for_read_at(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );
        assert_one_publication(
            &publications,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            published_at,
        );

        let content = std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
            .expect("read coordinator");
        assert_eq!(
            content,
            get_default_coordinator_template(),
            "a seeded pristine v3 body must auto-upgrade to the v4 default"
        );

        let state = std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("read seeded state");
        let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
        assert_eq!(
            parsed["templates"]["coordinator"]["currentVersion"], 6,
            "coordinator current_version must be bumped to 6 by the #1614 room rename"
        );
        assert_eq!(
            parsed["templates"]["coordinator"]["lastSeededSha256"]
                .as_str()
                .expect("lastSeededSha256 is a string"),
            hash_text(get_default_coordinator_template()),
            "the seeded hash must record the new v4 default"
        );
    }

    #[test]
    fn custom_coordinator_is_preserved_and_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");

        let mut clock = || fixed_publication_time();
        let mut publications = Vec::new();
        let updates = scan_project_context_template_updates_with_clock(
            temp.path(),
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("scan updates");

        assert_eq!(updates.len(), 1);
        assert!(
            publications.is_empty(),
            "observing custom content must not produce publication evidence"
        );
        assert_eq!(updates[0].filename, COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            custom
        );
    }

    #[test]
    fn global_unknown_without_state_is_not_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            "legacy rendered global body with project paths",
        )
        .expect("write global");

        let updates =
            scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

        assert!(updates.is_empty());
    }

    #[test]
    fn forged_manifest_does_not_auto_overwrite_custom_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "custom coordinator with forged seeded hash";
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "coordinator".to_string(),
            SeededContextTemplateEntry {
                template_id: "coordinator".to_string(),
                current_version: 1,
                last_seeded_sha256: Some(hash_text(custom)),
                last_observed_sha256: None,
                ignored_default_sha256: None,
                ignored_observed_sha256: None,
            },
        );
        persist_state(&ac_root, &state).expect("persist forged state");

        let updates =
            scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

        assert_eq!(updates.len(), 1);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            custom
        );
    }

    #[test]
    fn dismiss_suppresses_same_file_and_default_hash() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        let update = scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .pop()
            .expect("pending update");

        dismiss_context_template_update(
            &ac_root,
            &update.filename,
            &update.current_file_sha256,
            &update.current_default_sha256,
        )
        .expect("dismiss");

        let mut clock = || fixed_publication_time();
        let mut publications = Vec::new();
        let updates = scan_project_context_template_updates_with_clock(
            temp.path(),
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("scan again");
        assert!(updates.is_empty());
        assert!(
            publications.is_empty(),
            "a dismissed observation must not manufacture publication evidence"
        );
    }

    /// #1748 retargeted this pair at the coordinator: `global` is distribution-owned
    /// and has no ignore, dismiss or pending-update lifecycle any more (new test 5
    /// pins the rejection). The coordinator still has one, so the property survives.
    #[test]
    fn ignored_v3_pair_becomes_pending_against_v4() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "# Custom coordinator\n\nKEEP THIS CONTENT\n";
        let custom_hash = hash_text(custom);
        let v3_hash = hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE);
        let v4_hash = hash_text(get_default_coordinator_template());
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "coordinator".to_string(),
            SeededContextTemplateEntry {
                template_id: "coordinator".to_string(),
                current_version: 3,
                last_seeded_sha256: Some(v3_hash.clone()),
                last_observed_sha256: Some(custom_hash.clone()),
                ignored_default_sha256: Some(v3_hash.clone()),
                ignored_observed_sha256: Some(custom_hash.clone()),
            },
        );
        persist_state(&ac_root, &state).expect("persist ignored v3 pair");

        let mut clock = || fixed_publication_time();
        let mut publications = Vec::new();
        let updates = scan_project_context_template_updates_with_clock(
            temp.path(),
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("scan stale ignored pair");
        assert!(publications.is_empty());
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].current_file_sha256, custom_hash);
        assert_eq!(updates[0].current_default_sha256, v4_hash);
        assert_eq!(updates[0].current_default_version, 6);
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read preserved custom coordinator"),
            custom
        );
        let after_observation = load_state(&ac_root, true).expect("load observed state");
        let entry = after_observation
            .trusted_entry(project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap())
            .expect("trusted global entry");
        assert_eq!(
            entry.ignored_observed_sha256.as_deref(),
            Some(custom_hash.as_str())
        );
        assert_eq!(
            entry.ignored_default_sha256.as_deref(),
            Some(v3_hash.as_str())
        );

        dismiss_context_template_update(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            &updates[0].current_file_sha256,
            &updates[0].current_default_sha256,
        )
        .expect("dismiss v4 update");
        let second = scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan re-dismissed v4 pair");
        assert!(second.is_empty());
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("re-read preserved custom coordinator"),
            custom
        );

        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap();
        let mut loaded = load_state(&ac_root, true).expect("load re-dismissed state");
        let mut clock = || fixed_publication_time();
        let outcome = sync_one_template(
            Some(temp.path()),
            &ac_root,
            spec,
            &mut loaded,
            false,
            true,
            false,
            &mut clock,
        )
        .completion
        .expect("classify re-dismissed pair");
        assert!(matches!(
            outcome.target_outcome,
            TemplatePublication::Skipped(ContextTemplateSkipReason::IgnoredByUser)
        ));
    }

    #[test]
    fn ignored_current_v6_pair_remains_suppressed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "# Current ignored custom coordinator\n";
        let custom_hash = hash_text(custom);
        let v4_hash = hash_text(get_default_coordinator_template());
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "coordinator".to_string(),
            SeededContextTemplateEntry {
                template_id: "coordinator".to_string(),
                current_version: 6,
                last_seeded_sha256: Some(v4_hash.clone()),
                last_observed_sha256: Some(custom_hash.clone()),
                ignored_default_sha256: Some(v4_hash),
                ignored_observed_sha256: Some(custom_hash),
            },
        );
        persist_state(&ac_root, &state).expect("persist ignored v4 pair");

        let mut clock = || fixed_publication_time();
        let mut publications = Vec::new();
        let updates = scan_project_context_template_updates_with_clock(
            temp.path(),
            &ac_root,
            &mut clock,
            &mut |filename, publication| publications.push((filename, publication)),
        )
        .expect("scan ignored v4 pair");
        assert!(updates.is_empty());
        assert!(publications.is_empty());
        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read ignored custom coordinator"),
            custom
        );

        let spec = project_spec_by_filename(COORDINATOR_CONTEXT_TEMPLATE_FILENAME).unwrap();
        let mut loaded = load_state(&ac_root, true).expect("load ignored v4 state");
        let outcome = sync_one_template(
            Some(temp.path()),
            &ac_root,
            spec,
            &mut loaded,
            false,
            true,
            false,
            &mut clock,
        )
        .completion
        .expect("classify ignored v4 pair");
        assert!(matches!(
            outcome.target_outcome,
            TemplatePublication::Skipped(ContextTemplateSkipReason::IgnoredByUser)
        ));
    }

    #[test]
    fn explicit_keep_repairs_invalid_state_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        std::fs::write(
            ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
            "not json",
        )
        .expect("write invalid state");

        dismiss_context_template_update(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            &hash_text(custom),
            &hash_text(get_default_coordinator_template()),
        )
        .expect("dismiss with invalid state");

        let repaired =
            std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read repaired state");
        let parsed: SeededContextTemplateState =
            serde_json::from_str(&repaired).expect("state is repaired JSON");
        assert_eq!(parsed.schema_version, STATE_SCHEMA_VERSION);
    }

    #[test]
    fn overwrite_creates_backup_and_writes_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME), custom)
            .expect("write custom coordinator");
        let update = scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .pop()
            .expect("pending update");

        let result = overwrite_context_template_with_default(
            &ac_root,
            &update.filename,
            &update.current_file_sha256,
            &update.current_default_sha256,
        )
        .expect("overwrite");

        assert_eq!(
            std::fs::read_to_string(ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            get_default_coordinator_template()
        );
        assert_eq!(
            std::fs::read_to_string(result.backup_path).expect("read backup"),
            custom
        );
    }

    #[test]
    fn overwrite_publication_survives_later_strict_state_save_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let path = ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        let custom = "custom coordinator guidance";
        std::fs::write(&path, custom).expect("write custom coordinator");
        let update = scan_project_context_template_updates(temp.path(), &ac_root)
            .expect("scan updates")
            .pop()
            .expect("pending update");
        let published_at = fixed_publication_time();
        let mut clock = || published_at;
        let handled_publication = std::cell::Cell::new(None);

        let execution = overwrite_context_template_with_default_with(
            &ac_root,
            &update.filename,
            &update.current_file_sha256,
            &update.current_default_sha256,
            &mut clock,
            &mut |filename, publication| {
                assert_eq!(filename, COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
                handled_publication.set(Some(publication));
            },
            |_, _| {
                assert_eq!(
                    std::fs::read_to_string(&path).expect("read target before state save"),
                    get_default_coordinator_template(),
                    "the physical replacement must precede specialized state persistence"
                );
                assert_eq!(
                    handled_publication.get(),
                    Some(ContextPublication { published_at }),
                    "the publication must be consumed before specialized state persistence"
                );
                Err("injected strict state persistence failure".to_string())
            },
        );

        assert_eq!(
            execution.published,
            Some(ContextPublication { published_at })
        );
        assert_eq!(
            execution
                .completion
                .expect_err("strict state save failure remains an outer error"),
            "injected strict state persistence failure"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("read surviving published target"),
            get_default_coordinator_template()
        );
    }

    #[test]
    fn future_schema_is_not_rewritten_by_scan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let state_path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
        std::fs::write(&state_path, r#"{"schemaVersion":999,"templates":{}}"#)
            .expect("write future state");

        let updates =
            scan_project_context_template_updates(temp.path(), &ac_root).expect("scan updates");

        assert!(updates.is_empty());
        assert_eq!(
            std::fs::read_to_string(state_path).expect("read state"),
            r#"{"schemaVersion":999,"templates":{}}"#
        );
    }

    #[test]
    fn state_directory_blocks_explicit_commands() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::create_dir(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("create state dir");

        let err = dismiss_context_template_update(
            &ac_root,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            "file",
            "default",
        )
        .expect_err("state dir must error");

        assert!(err.contains("state path"));
    }

    #[test]
    fn dedupe_keeps_distinct_file_hashes() {
        let mut updates = vec![
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Orchestrator context".to_string(),
                current_file_sha256: "file-a".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Orchestrator context".to_string(),
                current_file_sha256: "file-a".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Orchestrator context".to_string(),
                current_file_sha256: "file-b".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
        ];

        dedupe_context_template_updates(&mut updates);

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].current_file_sha256, "file-a");
        assert_eq!(updates[1].current_file_sha256, "file-b");
    }

    #[test]
    fn root_custom_template_is_preserved() {
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "custom root context";
        std::fs::write(
            temp.path()
                .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write root custom");

        ensure_root_context_template(temp.path()).expect("ensure root context");

        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
            )
            .expect("read root context"),
            custom
        );
    }

    // ---------------------------------------------------------------- #979 retirement

    fn live_global(config_dir: &Path) -> PathBuf {
        config_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME)
    }

    fn retired_backups(config_dir: &Path) -> Vec<PathBuf> {
        let prefix = format!("{}.retired-", GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let mut found: Vec<PathBuf> = std::fs::read_dir(config_dir)
            .expect("read config dir")
            .filter_map(|entry| {
                let path = entry.expect("dir entry").path();
                let name = path.file_name()?.to_str()?.to_string();
                (name.starts_with(&prefix) && name.ends_with(".bak")).then_some(path)
            })
            .collect();
        found.sort();
        found
    }

    fn state_path(config_dir: &Path) -> PathBuf {
        config_dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
    }

    /// Every custom or unknown byte sequence must end up in exactly one inert backup,
    /// byte-for-byte, with the active global name gone.
    fn assert_custom_bytes_preserved(config_dir: &Path, original: &[u8]) {
        assert!(
            !live_global(config_dir).exists(),
            "the active global name must be retired"
        );
        let backups = retired_backups(config_dir);
        assert_eq!(backups.len(), 1, "expected exactly one inert backup");
        assert_eq!(
            std::fs::read(&backups[0]).expect("read backup"),
            original,
            "custom bytes must survive byte-for-byte"
        );
    }

    #[test]
    fn frozen_standalone_global_snapshot_is_pinned() {
        // #979 4.3.A: the 307-byte snapshot must never drift. If this fails, do NOT
        // "fix" the expectation: a changed literal silently widens or narrows what
        // retirement is willing to DELETE.
        assert_eq!(
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS.len(),
            307,
            "the frozen standalone global snapshot must stay 307 bytes"
        );
        assert_eq!(
            hash_text(STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS),
            "e0cbc16fbef5bf5ae116e5268b24a987be6834eaac50e7ac4441a57fc90678f3"
        );
        // It predates Core Concepts; that is exactly why Root would have lost the
        // section if the prologue had been assembled from the seven mandatory blocks.
        assert!(!STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS.contains("## Core Concepts"));
        assert!(is_known_generated_standalone_global_template(
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        ));
        assert!(is_known_generated_standalone_global_template(
            crate::config::session_context::get_default_agent_template()
        ));
    }

    #[test]
    fn retire_deletes_the_current_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = crate::config::session_context::get_default_agent_template();
        std::fs::write(live_global(temp.path()), current).expect("write generated global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert!(!live_global(temp.path()).exists());
        assert!(
            retired_backups(temp.path()).is_empty(),
            "known generated bytes are deleted, not retained"
        );
    }

    #[test]
    fn retire_deletes_the_frozen_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS,
        )
        .expect("write frozen global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert!(!live_global(temp.path()).exists());
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_deletes_the_exact_v3_global_before_summarization() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION,
        )
        .expect("write exact v3 global");

        retire_standalone_global_context(temp.path()).expect("retire exact v3");

        assert!(!live_global(temp.path()).exists());
        assert!(
            retired_backups(temp.path()).is_empty(),
            "the exact generated v3 backup must be deleted"
        );
    }

    #[test]
    fn retire_preserves_v3_global_near_matches_byte_for_byte() {
        let variants = [
            (
                "one-byte",
                format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION}X").into_bytes(),
            ),
            (
                "crlf",
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
                    .replace('\n', "\r\n")
                    .into_bytes(),
            ),
        ];

        for (label, bytes) in variants {
            let temp = tempfile::tempdir().expect("tempdir");
            std::fs::write(live_global(temp.path()), &bytes).expect("write v3 near match");

            retire_standalone_global_context(temp.path()).expect("retire v3 near match");

            assert!(!live_global(temp.path()).exists(), "{label}");
            let backups = retired_backups(temp.path());
            assert_eq!(backups.len(), 1, "{label}: expected one inert backup");
            assert_eq!(
                std::fs::read(&backups[0]).expect("read v3 near-match backup"),
                bytes,
                "{label}: retirement changed bytes"
            );
        }
    }

    #[test]
    fn retire_backs_up_a_one_byte_edit_of_a_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let edited = format!("{}X", STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS);
        std::fs::write(live_global(temp.path()), &edited).expect("write edited global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), edited.as_bytes());
    }

    #[test]
    fn retire_treats_normalized_variants_of_a_generated_default_as_custom() {
        // #979 4.3.A: NO normalization. A CRLF copy, a BOM, trailing whitespace, and a
        // zero-byte file are each UNKNOWN and must be preserved, never deleted.
        let frozen = STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS;
        let crlf = frozen.replace('\n', "\r\n").into_bytes();
        let mut bom = vec![0xEF_u8, 0xBB, 0xBF];
        bom.extend_from_slice(frozen.as_bytes());
        let trailing = format!("{}\n", frozen).into_bytes();
        let variants: Vec<(&str, Vec<u8>)> = vec![
            ("crlf", crlf),
            ("bom", bom),
            ("trailing whitespace", trailing),
            ("zero-byte", Vec::new()),
        ];

        for (label, bytes) in variants {
            let temp = tempfile::tempdir().expect("tempdir");
            std::fs::write(live_global(temp.path()), &bytes).expect("write variant");

            retire_standalone_global_context(temp.path()).expect("retire");

            assert!(!live_global(temp.path()).exists(), "case: {}", label);
            let backups = retired_backups(temp.path());
            assert_eq!(backups.len(), 1, "case: {}", label);
            assert_eq!(
                std::fs::read(&backups[0]).expect("read backup"),
                bytes,
                "case: {}: bytes must survive",
                label
            );
        }
    }

    #[test]
    fn retire_preserves_invalid_utf8_bytes() {
        // Invalid UTF-8 is automatically custom: the classifier reads RAW BYTES and
        // never goes through the String-converting snapshot reader.
        let temp = tempfile::tempdir().expect("tempdir");
        let bytes = vec![0x00_u8, 0xFF, 0xFE, 0x41, 0x80, 0x0A];
        std::fs::write(live_global(temp.path()), &bytes).expect("write invalid utf-8");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), &bytes);
    }

    #[test]
    fn retire_ignores_forged_state_claiming_a_custom_file_is_seeded() {
        // State is never classification evidence. A forged `global` entry claiming the
        // custom file is generated must not license deleting it.
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP_THESE_BYTES\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");
        let forged = format!(
            r#"{{"schemaVersion":1,"templates":{{"global":{{"templateId":"global","currentVersion":1,"lastSeededSha256":"{}"}}}}}}"#,
            hash_text(custom)
        );
        std::fs::write(state_path(temp.path()), &forged).expect("write forged state");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), custom.as_bytes());
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(
            state["templates"]["global"].is_null(),
            "the stale entry converges"
        );
    }

    #[test]
    fn retire_removes_only_the_global_state_entry() {
        // #979 G3: `coordinator`, `rootAgent`, and unrelated keys must all survive.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write generated global");
        let state = r#"{"schemaVersion":1,"templates":{"coordinator":{"templateId":"coordinator","currentVersion":2},"global":{"templateId":"global","currentVersion":1},"rootAgent":{"templateId":"rootAgent","currentVersion":4},"unrelated":{"templateId":"unrelated","currentVersion":7}}}"#;
        std::fs::write(state_path(temp.path()), state).expect("write state");

        retire_standalone_global_context(temp.path()).expect("retire");

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(parsed["templates"]["global"].is_null());
        assert_eq!(parsed["templates"]["coordinator"]["currentVersion"], 2);
        assert_eq!(parsed["templates"]["rootAgent"]["currentVersion"], 4);
        assert_eq!(parsed["templates"]["unrelated"]["currentVersion"], 7);
    }

    #[test]
    fn retire_leaves_malformed_state_untouched_and_still_retires_the_file() {
        // #979 G3 / 4.3.C. This is the case that `persist_state_strict` would destroy:
        // `load_state` returns an EMPTY map with `dirty: true` on malformed JSON, and
        // the strict wrapper writes whenever `dirty` is set, so the "safe" idiom would
        // overwrite this file with `{"templates":{}}` and wipe `coordinator` and
        // `rootAgent`. Retirement must return Ok, leave the state byte-identical, and
        // still retire the live global.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write generated global");
        let malformed = "{not json at all";
        std::fs::write(state_path(temp.path()), malformed).expect("write malformed state");

        retire_standalone_global_context(temp.path()).expect("retirement must return Ok");

        assert!(
            !live_global(temp.path()).exists(),
            "the live global is still retired"
        );
        assert_eq!(
            std::fs::read(state_path(temp.path())).expect("read state"),
            malformed.as_bytes(),
            "a malformed state file must be left byte-identical"
        );
    }

    #[test]
    fn retire_converges_a_stale_state_entry_without_a_live_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = r#"{"schemaVersion":1,"templates":{"global":{"templateId":"global","currentVersion":1},"rootAgent":{"templateId":"rootAgent","currentVersion":4}}}"#;
        std::fs::write(state_path(temp.path()), state).expect("write state");

        retire_standalone_global_context(temp.path()).expect("retire");

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(parsed["templates"]["global"].is_null());
        assert_eq!(parsed["templates"]["rootAgent"]["currentVersion"], 4);
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_is_idempotent_and_never_rewrites_an_untouched_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");

        retire_standalone_global_context(temp.path()).expect("retire once");
        let backups_after_first = retired_backups(temp.path());
        assert_eq!(backups_after_first.len(), 1);
        let state_exists_after_first = state_path(temp.path()).exists();

        retire_standalone_global_context(temp.path()).expect("retire twice");

        assert_eq!(
            retired_backups(temp.path()),
            backups_after_first,
            "a second call creates no new backup"
        );
        assert_eq!(
            state_path(temp.path()).exists(),
            state_exists_after_first,
            "with no `global` entry to remove, the state file is never written"
        );
    }

    #[test]
    fn retire_refuses_a_directory_at_the_live_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let live = live_global(temp.path());
        std::fs::create_dir_all(&live).expect("create dir at the live path");
        std::fs::write(live.join("inner.md"), "KEEP_ME\n").expect("write inner");

        let err = retire_standalone_global_context(temp.path())
            .expect_err("a non-file at the live path must be reported");
        assert!(err.contains("is not a regular file"), "{}", err);

        assert!(live.is_dir(), "the entry must be preserved");
        assert_eq!(
            std::fs::read_to_string(live.join("inner.md")).expect("read inner"),
            "KEEP_ME\n"
        );
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_refuses_a_symlink_at_the_live_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("elsewhere.md");
        std::fs::write(&target, "TARGET_BYTES\n").expect("write target");
        let live = live_global(temp.path());
        // Windows may deny symlink creation without developer mode; keep the project
        // convention of returning early when it does. The directory case above always
        // runs.
        if try_symlink_file(&target, &live).is_err() {
            return;
        }

        let err = retire_standalone_global_context(temp.path())
            .expect_err("a symlink at the live path must be reported");
        assert!(err.contains("is not a regular file"), "{}", err);

        assert!(std::fs::symlink_metadata(&live)
            .expect("inspect link")
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "TARGET_BYTES\n",
            "the symlink must never be followed, moved, or deleted"
        );
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_cleans_up_the_reservation_on_a_definite_pre_move_failure() {
        // The publish seam fails BEFORE moving anything: the source is intact and the
        // destination is still this call's zero-byte reservation, so the reservation is
        // safe to remove.
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");

        let err = retire_standalone_global_context_with(temp.path(), |_, _| {
            Err("simulated pre-move failure".to_string())
        })
        .expect_err("the publish failure must be reported");
        assert!(err.contains("simulated pre-move failure"), "{}", err);

        assert_eq!(
            std::fs::read_to_string(live_global(temp.path())).expect("read live global"),
            custom,
            "the live global must be untouched"
        );
        assert!(
            retired_backups(temp.path()).is_empty(),
            "the unused reservation must be cleaned up"
        );
    }

    #[test]
    fn retire_preserves_the_destination_when_the_source_disappeared() {
        // The AMBIGUOUS failure: the publish seam reports an error but the source is
        // gone. An empty custom source is a valid unknown file, so source disappearance
        // is never proof that an empty destination is disposable. Keep the destination.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(live_global(temp.path()), "# Custom Global\n").expect("write custom global");

        let err = retire_standalone_global_context_with(temp.path(), |source, _| {
            std::fs::remove_file(source).expect("simulate a vanished source");
            Err("simulated ambiguous failure".to_string())
        })
        .expect_err("the publish failure must be reported");
        assert!(err.contains("simulated ambiguous failure"), "{}", err);

        let backups = retired_backups(temp.path());
        assert_eq!(
            backups.len(),
            1,
            "an ambiguous failure must PRESERVE the destination, never delete it"
        );
    }

    #[test]
    fn retire_leaves_a_project_global_and_its_state_untouched() {
        // The whole point of #979: retiring the APP CONFIG directory must not touch the
        // project's `.ac` global or its project state.
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join("portable-config");
        let project_ac = temp.path().join("project").join(".ac");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::create_dir_all(&project_ac).expect("create project .ac");

        std::fs::write(
            live_global(&config_dir),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write app-config global");

        let project_global = "# Project Global\n\nEDITABLE_PROJECT_GLOBAL\n";
        std::fs::write(live_global(&project_ac), project_global).expect("write project global");
        let project_state = r#"{"schemaVersion":1,"templates":{"global":{"templateId":"global","currentVersion":1}}}"#;
        std::fs::write(state_path(&project_ac), project_state).expect("write project state");

        retire_standalone_global_context(&config_dir).expect("retire");

        assert!(!live_global(&config_dir).exists());
        assert_eq!(
            std::fs::read(live_global(&project_ac)).expect("read project global"),
            project_global.as_bytes(),
            "the project global must be byte-for-byte unchanged"
        );
        assert_eq!(
            std::fs::read(state_path(&project_ac)).expect("read project state"),
            project_state.as_bytes(),
            "the project state must be byte-for-byte unchanged"
        );
    }

    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    /// #1748 new test 1 and its control in one harness: the SAME pristine fixture
    /// is silent when the state says we wrote those bytes and notifies when it does
    /// not. The flipped input is `lastSeededSha256` and nothing else; the file is
    /// replaced and backed up identically in both arms.
    #[test]
    fn pristine_older_generation_is_replaced_silently_when_the_state_says_we_wrote_it() {
        let current = crate::config::session_context::get_default_agent_template();
        for (arm, seeded, expected_replacements) in [
            (
                "silent",
                hash_text(GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS),
                0usize,
            ),
            (
                "the_same_pristine_file_notifies_when_the_state_does_not_match",
                hash_text("something else"),
                1usize,
            ),
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let ac_root = temp.path().join(".ac");
            std::fs::create_dir(&ac_root).expect("create workspace");
            std::fs::write(
                ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
            )
            .expect("write pristine v2 global");
            write_trusted_global_state(&ac_root, &seeded);

            let replacements =
                scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                    .expect("scan");
            assert_eq!(replacements.len(), expected_replacements, "{arm}");

            assert_eq!(
                std::fs::read_to_string(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME))
                    .expect("read repaired global"),
                current,
                "{arm}: the file must become the current default"
            );
            let backups = backup_files(&ac_root);
            assert_eq!(backups.len(), 1, "{arm}: {backups:?}");
            assert_eq!(
                std::fs::read_to_string(&backups[0]).expect("read backup"),
                GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS,
                "{arm}: the backup must hold the pre-run bytes"
            );
            let parsed: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                    .expect("read seeded state"),
            )
            .expect("parse seeded state");
            assert_eq!(
                parsed["templates"]["global"]["lastSeededSha256"],
                hash_text(current),
                "{arm}"
            );
            assert_eq!(parsed["templates"]["global"]["currentVersion"], 6, "{arm}");
            assert!(
                scan_project_context_template_updates(temp.path(), &ac_root)
                    .expect("scan updates")
                    .is_empty(),
                "{arm}: a distribution-owned template never yields a pending update"
            );
        }
    }

    /// #1748 new test 2: no state file at all is the population that must be told.
    #[test]
    fn an_installation_with_no_state_entry_is_notified() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&path, GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS)
            .expect("write pristine v2 global");
        assert!(!ac_root
            .join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
            .exists());

        let replacements =
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan");
        assert_eq!(replacements.len(), 1);
        let replacement = &replacements[0];
        assert_eq!(replacement.file_path, display_path(&path));
        assert_eq!(replacement.filename, GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        assert_eq!(replacement.label, "AgentsCommander shared context");
        assert_eq!(replacement.project_path, display_path(temp.path()));
        assert_eq!(replacement.workspace_path, display_path(&ac_root));
        assert!(
            replacement
                .local_override_path
                .ends_with("Context.AgentsCommander.local.md"),
            "{}",
            replacement.local_override_path
        );
        assert_eq!(
            std::fs::read_to_string(&replacement.backup_path).expect("read backup"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        );

        // Control: a second run over the repaired tree. This proves IDEMPOTENCE and
        // NOT the notify rule, because a repaired file returns at the equal-default
        // early return and never reaches D6.
        let repaired = std::fs::read_to_string(&path).expect("read repaired global");
        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("second scan")
                .is_empty()
        );
        assert_eq!(backup_files(&ac_root).len(), 1);
        assert_eq!(
            std::fs::read_to_string(&path).expect("re-read repaired global"),
            repaired
        );
    }

    /// #1748 new test 3: the repair never touches a file that already holds the
    /// current default.
    #[test]
    fn a_file_already_equal_to_the_current_default_is_never_rewritten() {
        let current = crate::config::session_context::get_default_agent_template();
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&path, current).expect("write the current default");
        assert!(backup_files(&ac_root).is_empty());

        assert!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan an already-current global")
                .is_empty()
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read global"),
            current
        );
        // NOT `modified()`: the Windows clock tick is about 15.6 ms and
        // `atomically_replace_context_template` is a same-directory replace, so mtime
        // cannot detect a spurious rewrite. The backup count can, and does.
        assert!(
            backup_files(&ac_root).is_empty(),
            "an already-current file must produce no backup"
        );

        // Control (the input flipped): append one byte and re-run.
        std::fs::write(&path, format!("{current}X")).expect("append one byte");
        assert_eq!(
            scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                .expect("scan a one-byte drift")
                .len(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read repaired global"),
            current
        );
        assert_eq!(backup_files(&ac_root).len(), 1);
    }

    /// #1748 new test 4: `create_backup` never opens, truncates or replaces an
    /// existing backup, so every earlier one survives every later replacement.
    #[test]
    fn every_replacement_keeps_every_earlier_backup() {
        const SENTINEL: &str = "an earlier backup that must survive untouched\n";
        for pre_existing_backup in [true, false] {
            let temp = tempfile::tempdir().expect("tempdir");
            let ac_root = temp.path().join(".ac");
            std::fs::create_dir(&ac_root).expect("create workspace");
            let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
            let customized =
                format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS}\n\nMY OWN NOTE\n");
            std::fs::write(&path, &customized).expect("write customized global");
            // Do not pre-create the `{f}.{ts}.bak` name: per D7 the timestamp comes
            // from the wall clock, not the injected one, so it cannot be predicted.
            let first_backup = ac_root.join(format!("{GLOBAL_CONTEXT_TEMPLATE_FILENAME}.bak"));
            if pre_existing_backup {
                std::fs::write(&first_backup, SENTINEL).expect("write the sentinel backup");
            }

            assert_eq!(
                scan_project_context_template_replacements_for_test(temp.path(), &ac_root)
                    .expect("scan")
                    .len(),
                1,
                "{pre_existing_backup}"
            );

            let backups = backup_files(&ac_root);
            assert_eq!(
                backups.len(),
                usize::from(pre_existing_backup) + 1,
                "{pre_existing_backup}: {backups:?}"
            );
            if pre_existing_backup {
                assert_eq!(
                    std::fs::read_to_string(&first_backup).expect("read the sentinel backup"),
                    SENTINEL,
                    "the pre-existing backup must be byte-identical afterwards"
                );
                let others: Vec<&PathBuf> = backups
                    .iter()
                    .filter(|entry| **entry != first_backup)
                    .collect();
                assert_eq!(others.len(), 1, "{backups:?}");
                assert_eq!(
                    std::fs::read_to_string(others[0]).expect("read the new backup"),
                    customized
                );
            } else {
                assert_eq!(
                    backups[0], first_backup,
                    "the first backup takes the plain .bak name"
                );
                assert_eq!(
                    std::fs::read_to_string(&backups[0]).expect("read the new backup"),
                    customized
                );
            }
        }
    }

    /// #1748 new test 5: `global` has left the actionable surface entirely.
    #[test]
    fn the_global_template_can_no_longer_be_dismissed_or_overwritten_by_hand() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        std::fs::write(
            ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            format!("{GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS}\n\nMY OWN NOTE\n"),
        )
        .expect("write customized global");
        std::fs::write(
            ac_root.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            format!("{}\n\nMY OWN NOTE\n", get_default_coordinator_template()),
        )
        .expect("write customized coordinator");
        let coordinator_default = hash_text(get_default_coordinator_template());
        let managed = "Context template filename is managed by the distribution and cannot be overwritten by hand";

        // The global arm never inspects the hashes: it is rejected on the filename.
        assert_eq!(
            dismiss_context_template_update(
                &ac_root,
                GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                "not the file sha",
                &coordinator_default,
            )
            .expect_err("dismiss must be rejected for a distribution-owned template"),
            managed
        );
        assert_eq!(
            overwrite_context_template_with_default(
                &ac_root,
                GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                "not the file sha",
                &coordinator_default,
            )
            .expect_err("overwrite must be rejected for a distribution-owned template"),
            managed
        );

        // Control (the input flipped): the coordinator is not distribution-owned, so
        // the identical calls reach the existing expected-hash guard instead.
        assert_eq!(
            dismiss_context_template_update(
                &ac_root,
                COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
                "not the file sha",
                &coordinator_default,
            )
            .expect_err("a stale coordinator hash must be rejected"),
            CONTEXT_TEMPLATE_CHANGED
        );
        assert_eq!(
            overwrite_context_template_with_default(
                &ac_root,
                COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
                "not the file sha",
                &coordinator_default,
            )
            .expect_err("a stale coordinator hash must be rejected"),
            CONTEXT_TEMPLATE_CHANGED
        );
    }

    /// #1748 new test 6. The other half of this contract lives in
    /// `config::local_overlay`, whose PRIVATE markdown local suffix must stay
    /// `.local.md`. This pins the VALUE only; it cannot detect the forbidden
    /// module arc, because it never calls into the overlay. AC-01-8 detects that.
    #[test]
    fn local_override_filename_is_the_overlay_sibling() {
        assert_eq!(
            local_override_filename(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            "Context.AgentsCommander.local.md"
        );
        // `strip_suffix`, not `trim_end_matches`: the latter strips a REPEATED
        // suffix and would turn `a.md.md` into `a`.
        assert_eq!(local_override_filename("a.md.md"), "a.md.local.md");
    }

    /// #1748 new test 7: the whole content of D2, and the replacement carrier for
    /// the read-path tests that would otherwise have survived vacuously. One
    /// fixture, driven through FOUR entry points; the flipped input is the entry
    /// point throughout. Arms (a), (b) and (c) share one tree, and (c)'s
    /// `repair: true` call is the LAST call on it because that call replaces the
    /// bytes and writes a backup; arm (d) runs on a second tree built from the
    /// same fixture, so "one `*.bak` appears" is a statement about the scan alone.
    #[test]
    fn the_read_path_no_longer_repairs_the_global_but_the_scan_does() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ac_root = temp.path().join(".ac");
        std::fs::create_dir(&ac_root).expect("create workspace");
        let path = ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&path, GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES)
            .expect("write pristine v4 global");

        // (a) the render-time read path.
        assert!(
            sync_for_read_at(
                &ac_root,
                GLOBAL_CONTEXT_TEMPLATE_FILENAME,
                fixed_publication_time(),
            )
            .is_empty(),
            "the read path cannot report a replacement, so it must not make one"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read global"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
            "the read path must leave the bytes exactly as they were"
        );
        assert!(backup_files(&ac_root).is_empty());

        // (b) project registration, the OTHER `repair: false` caller that iterates
        // `project_specs()`. The `Ok(())` is what makes this arm gradeable: with
        // this caller's `repair` flipped to `true`, `project_dir` is `None`, so
        // control reaches the project-dir guard, which sits BEFORE `create_backup`
        // and returns `Err` having written nothing. Both filesystem assertions
        // below hold in either state. This arm therefore proves "this call site
        // does not pass `repair: true`", NOT "a repair on this path would leave
        // the bytes alone" -- the latter is unreachable here.
        assert_eq!(
            ensure_project_context_templates(&ac_root),
            Ok(()),
            "project registration must not attempt a repair it cannot report"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read global after registration"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
            "project registration must leave the bytes exactly as they were"
        );
        assert!(backup_files(&ac_root).is_empty());

        // (c) `sync_one_template` directly, the only pair in this phase that can
        // distinguish the new skip reason from a silent fall-through: unchanged
        // bytes and no publication hold equally for `AmbiguousWithoutState`,
        // `CreationDisabled` or any other quiet return.
        let spec = project_spec_by_filename(GLOBAL_CONTEXT_TEMPLATE_FILENAME)
            .expect("the global project spec");
        let mut clock = || fixed_publication_time();
        let mut loaded = load_state(&ac_root, true).expect("load state for the deferred call");
        let deferred = sync_one_template(
            Some(temp.path()),
            &ac_root,
            spec,
            &mut loaded,
            false,
            true,
            false,
            &mut clock,
        )
        .completion
        .expect("classify the deferred repair");
        assert!(
            matches!(
                deferred.target_outcome,
                TemplatePublication::Skipped(ContextTemplateSkipReason::DistributionRepairDeferred)
            ),
            "expected Skipped(DistributionRepairDeferred), got {:?}",
            deferred.target_outcome
        );
        assert!(deferred.replacement.is_none());
        assert!(backup_files(&ac_root).is_empty());

        let mut loaded = load_state(&ac_root, true).expect("load state for the repairing call");
        let repaired = sync_one_template(
            Some(temp.path()),
            &ac_root,
            spec,
            &mut loaded,
            false,
            true,
            true,
            &mut clock,
        )
        .completion
        .expect("run the distribution repair");
        assert!(
            matches!(repaired.target_outcome, TemplatePublication::Published(..)),
            "expected Published(..), got {:?}",
            repaired.target_outcome
        );
        assert!(
            repaired.replacement.is_some(),
            "an installation with no state entry must be notified"
        );

        // (d) the scan, on a second tree built from the same fixture.
        let scan_temp = tempfile::tempdir().expect("tempdir");
        let scan_ac_root = scan_temp.path().join(".ac");
        std::fs::create_dir(&scan_ac_root).expect("create scan workspace");
        let scan_path = scan_ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(
            &scan_path,
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES,
        )
        .expect("write pristine v4 global");
        let replacements =
            scan_project_context_template_replacements_for_test(scan_temp.path(), &scan_ac_root)
                .expect("scan the second tree");
        assert_eq!(replacements.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&scan_path).expect("read repaired global"),
            crate::config::session_context::get_default_agent_template()
        );
        let backups = backup_files(&scan_ac_root);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert_eq!(
            std::fs::read_to_string(&backups[0]).expect("read backup"),
            GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        );
    }

    /// #1748 new test 8: distribution ownership and the recognizer slot are one
    /// decision, across every spec. Written as ONE loop with a single assertion so
    /// the field-access census stays derivable.
    #[test]
    fn distribution_ownership_and_the_recognizer_slot_never_drift() {
        let [global, coordinator, windows, linux, macos] = project_specs();
        let mut owned = 0usize;
        for spec in [global, coordinator, windows, linux, macos, root_spec()] {
            assert_eq!(
                spec.distribution_owned,
                spec.is_known_generated.is_none(),
                "#1748: {} is distribution-owned exactly when it names no recognizer",
                spec.id
            );
            owned += usize::from(spec.distribution_owned);
        }
        assert_eq!(owned, 1, "exactly one spec is distribution-owned");
    }
}
