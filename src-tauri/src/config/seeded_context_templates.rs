use crate::config::session_context::atomically_replace_context_template;
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

const OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND: &str =
    "You are the coordinator for your \
    team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests.\n\
     - Clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Always route work to the team member best prepared for each part of the request based on \
     role, skills, and current assignment.\n\
     - Delegate work instead of absorbing technical work when a more specialized agent is \
     available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working.\n\
     - Contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete.\n\
     - Not infer completion solely from files/logs/artifacts/status flags when the assigned agent \
     has not reported the outcome.\n\
     - Give recommendations to help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     As a coordinator, you may need to send screenshots. Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     - --path is required. --caption is optional and limited to 1024 UTF-16 units.\n\
     - If multiple Telegram bots are configured, use --bot-id or --bot-label.\n\
     - jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument \
     up to 50 MB.\n\
     - Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work. \
     Important: cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels. In that \
     case ask the user to capture with Greenshot, use latest file from \
     C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
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
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION: &str = concat!(
    r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager that coordinates "#,
    r#"multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines who can work together, who "#,
    r#"coordinates, and which repos are available.
- **Workgroup**: an operational runtime replica instance of a team for a specific task. It "#,
    r#"contains replica agents and `repo-*` working repositories.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

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
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS: &str = concat!(
    r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager coordinating "#,
    r#"multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
    r#"which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and "#,
    r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

/// #1541: `get_default_agent_template()` exactly as it shipped from #1369
/// (the `{{AGENT_REPOS}}` rename, v3) through base commit 6aae531e, frozen so
/// pristine v3 project templates and standalone templates remain exact
/// generated operands after the v4 summarization. Never edit. The shipped
/// accessor is len 563, sha256
/// 99a0aa4a15062d4b68b94597111ae268958cbeb4e3902aafe1b7361b63d34157;
/// pinned by `global_before_summarization_snapshot_is_byte_exact`.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION: &str = concat!(
    r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager coordinating "#,
    r#"multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
    r#"which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and "#,
    r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

/// #1605: `get_default_agent_template()` exactly as it shipped through base
/// commit 047248bc (the v4 summarization), frozen so a pristine v4
/// `Context.AgentsCommander.md` on disk keeps being recognized (project
/// auto-update AND standalone root retirement) after the v5
/// `{{HOST_PLATFORM_RULES}}` insertion. Never edit. Provenance: the accessor at
/// 047248bc is len 539, sha256
/// f44065965f3c53c8b8d2c2e6b3d38c68b998f848ae893eddb7e64085a3c5316a; pinned by
/// `global_before_host_platform_rules_snapshot_is_byte_exact` against those
/// externally captured values, never against this const itself.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES: &str = concat!(
    r#"# AgentsCommander "#,
    r#"Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
    r#"which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and "#,
    r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

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
     - Always route work to the team member best prepared for each part of the request based on \
     role, skills, and current assignment.\n\
     - Delegate work instead of absorbing technical work when a more specialized agent is \
     available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working.\n\
     - Contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete.\n\
     - Not infer completion solely from files/logs/artifacts/status flags when the assigned agent \
     has not reported the outcome.\n\
     - Give recommendations to help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     As a coordinator, you may need to send screenshots. Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     - --path is required. --caption is optional and limited to 1024 UTF-16 units.\n\
     - If multiple Telegram bots are configured, use --bot-id or --bot-label.\n\
     - jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument \
     up to 50 MB.\n\
     - Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work. \
     Important: cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels. In that \
     case ask the user to capture with Greenshot, use latest file from \
     C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \
         \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the \
     user interacts with your session.\n";

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
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance \
     criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and \
     current assignment; delegate instead of absorbing technical work when a more specialized \
     agent is available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact \
     silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete; never infer completion solely from \
     files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots \
     are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use \
     sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are \
     rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work; cast \
     Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels; then ask \
     the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, \
     and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \
         \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the \
     user interacts with your session.\n";

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
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance \
     criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and \
     current assignment; delegate instead of absorbing technical work when a more specialized \
     agent is available.\n\
     - To reach another workgroup, message its coordinator, never its members, and only when your \
     role, the user, or the Root Agent authorizes it; replying to a coordinator who messaged you \
     first is always authorized.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact \
     silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete; never infer completion solely from \
     files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots \
     are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use \
     sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are \
     rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work; cast \
     Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels; then ask \
     the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, \
     and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \
         \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your coordinator row; it clears when the \
     user interacts with your session.\n";

/// #979: the standalone global context template that older builds seeded into the
/// APP CONFIG directory (307 bytes; it predates `## Core Concepts`). Retirement may
/// delete only bytes AgentsCommander provably generated itself, so this snapshot is
/// frozen and pinned by a length + SHA-256 test.
///
/// Do NOT replace this literal with `include_str!`: a raw string literal normalizes
/// `\r\n` to `\n` at compile time and `include_str!` does not, so on a CRLF checkout
/// `include_str!` would silently stop recognizing the generated default and every
/// retirement would fall through to "custom".
const STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS: &str = concat!(
    r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager that coordinates "#,
    r#"multiple AI agents.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

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
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME: &str = concat!(
    r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
    r#"which repos are available.
- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and "#,
    r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{HOST_PLATFORM_RULES}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

/// #2031: the global context template as shipped through `329ab94e`, the
/// generation every existing project file and every existing standalone app-config
/// file holds. The Answering seed turns it into a generated predecessor: it is the
/// fixture for the scan replacement test, and it MUST stay recognized by the
/// standalone #979 classifier so retirement still deletes our own bytes instead of
/// reclassifying them as custom. Never edit.
/// Provenance: the 329ab94e blob, `session_context.rs` lines 2679-2705; value
/// 559 bytes sha256 EE456D60802157ECC8A5F5C5C97522442276524E1F4979481326A80CC6EC5F09;
/// pinned by `global_before_answering_snapshot_is_byte_exact`.
const GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION: &str = concat!(
    r#"# AgentsCommander Context

You are in AgentsCommander, a terminal session manager coordinating multiple AI agents.

## Core Concepts

- **Team**: the logical capability and organization. It defines membership, who coordinates, and "#,
    r#"which repos are available.
- **Room**: a runtime replica of a team for a specific task. It contains replica agents and "#,
    r#"`repo-*` working repos.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{AGENT_REPOS}}

{{CLI_CONTEXT}}

{{HOST_PLATFORM_RULES}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#
);

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
     - Receive team work requests and clarify scope, outcome, constraints, and acceptance \
     criteria.\n\
     - Route each part of a request to the team member best prepared for it by role, skills, and \
     current assignment; delegate instead of absorbing technical work when a more specialized \
     agent is available.\n\
     - To reach another workgroup, message its orchestrator, never its members, and only when your \
     role, the user, or the Root Agent authorizes it; replying to an orchestrator who messaged you \
     first is always authorized.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working; contact \
     silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and \
     verification before treating delegated work as complete; never infer completion solely from \
     files/logs/artifacts/status flags when the agent has not reported the outcome.\n\
     - Give recommendations that help an agent work better without removing or overriding that \
     agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label \
         <LABEL>]\n\
     --path is required; --caption is optional, max 1024 UTF-16 units. If multiple Telegram bots \
     are configured, pick one with --bot-id or --bot-label. jpg/jpeg/png/webp up to 10 MB use \
     sendPhoto; other formats including GIF use sendDocument up to 50 MB. Symlinks/junctions are \
     rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop orchestrator: PowerShell System.Drawing / CopyFromScreen can work; cast \
     Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness orchestrator: CopyFromScreen may return all-zero/black pixels; then ask \
     the user to capture with Greenshot, use the latest file from C:\\Users\\maria\\0_greenshot\\, \
     and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n\n\
     ## Raising Your Hand\n\
     When you are blocked, need a user decision, or are waiting for user attention, run:\n\
         \"<AGENTSCOMMANDER_BINARY_PATH>\" raise-hand --token <AGENTSCOMMANDER_TOKEN> --root \
         \"<AGENTSCOMMANDER_ROOT>\"\n\
     This shows the Sidebar raised-hand indicator for your orchestrator row; it clears when the \
     user interacts with your session.\n";

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
    /// #1748: `None` means a distribution-owned template, which records no version.
    current_version: Option<u32>,
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
        let next_version = (!spec.distribution_owned).then_some(spec.current_version);
        // #1748: for a distribution-owned spec this is the ONLY field this call can change,
        // and every downstream guard compares hashes, so nothing else would ever mark the
        // state dirty and an already-in-sync installation would keep its stale version on
        // disk forever. Compare through a shared borrow first, so the flag is set on the
        // real delta and the &mut below is taken only once.
        if spec.distribution_owned {
            let already_recorded = self.state.templates.get(spec.id).is_some_and(|entry| {
                entry.template_id == spec.id && entry.current_version == next_version
            });
            if !already_recorded {
                self.dirty = true;
            }
        }
        let entry = self.state.templates.entry(spec.id.to_string()).or_default();
        entry.template_id = spec.id.to_string();
        entry.current_version = next_version;
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

/// Frozen Windows v1 bytes for exact generated-file migration.
const HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT: &str = concat!(
    r#"## Host Platform Rules

Windows host session: use `C:\Program Files\Git\bin\bash.exe` for all shell work and every "#,
    r#"AgentsCommander CLI invocation; from PowerShell wrap with `& 'C:\Program "#,
    r#"Files\Git\bin\bash.exe' -lc '...'`; never capture CLI output without `2>&1 | "#,
    r#"Out-String`."#
);

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
            current_version: 2,
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
        current_version: 10,
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
            "Context template filename is managed by the distribution and cannot be overwritten by \
                hand"
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
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ANSWERING_SECTION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_AGENT_REPOS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_SUMMARIZATION
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES
        || content == STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        || content == GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME
}

/// #1605: per-platform generated recognizers — equality with the current
/// platform default or a frozen historical snapshot. A future default change MUST first freeze the
/// previous default as a snapshot const and extend the recognizer, so seeded
/// files auto-update and edited files are preserved with the pending-update
/// offer.
fn is_known_generated_platform_windows(content: &str) -> bool {
    content == crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS
        || content == HOST_PLATFORM_RULES_WINDOWS_BEFORE_GIT_BASH_DIRECT
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
            "Context template state schema version {} is unsupported; reload or upgrade \
                AgentsCommander.",
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
    auto_update_generated_template_with_backup(
        path,
        spec,
        expected_file_sha256,
        clock,
        create_backup,
        replace,
    )
}

fn auto_update_generated_template_with_backup<B, R>(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
    backup: B,
    replace: R,
) -> ContextTemplateExecution<TemplatePublication>
where
    B: FnOnce(&Path, &[u8]) -> Result<PathBuf, String>,
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
            "[context-templates] {} no longer matches a known generated default; preserving \
                current content",
            path.display()
        );
        return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
    }
    if spec.id == "rootAgent" {
        let backup_path = match backup(path, &snapshot.bytes) {
            Ok(path) => path,
            Err(error) => return ContextTemplateExecution::failed(error),
        };
        let after_backup = match read_validated_snapshot(path, "Context template") {
            Ok(snapshot) => snapshot,
            Err(error) => return ContextTemplateExecution::failed(error),
        };
        if !after_backup.as_ref().is_some_and(|current| {
            current.sha256 == snapshot.sha256 && current.bytes == snapshot.bytes
        }) {
            log::warn!(
                "[context-templates] {} changed after Root backup {}; preserving current content",
                path.display(),
                backup_path.display()
            );
            return ContextTemplateExecution::completed(TemplatePublication::ChangedUnderUs);
        }
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

// All generated migrations share publication/state handling. Eligibility stays
// at each caller, and state advances only after successful publication.
fn sync_generated_template(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_sha256: &str,
    loaded: &mut LoadedState,
    current_default_sha256: &str,
    clock: &mut dyn FnMut() -> chrono::DateTime<chrono::Utc>,
) -> ContextTemplateExecution<TemplateSyncOutcome> {
    let execution = auto_update_generated_template(path, spec, expected_sha256, clock);
    let published = execution.published;
    match execution.completion {
        Ok(target_outcome) => {
            if matches!(target_outcome, TemplatePublication::Published(_)) {
                loaded.mark_seeded(spec, current_default_sha256);
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
    }
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
            if snapshot.sha256 == current_default_sha256
                && (spec.id != "rootAgent" || snapshot.bytes == current_default.as_bytes())
            {
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

    if snapshot.sha256 == current_default_sha256
        && (spec.id != "rootAgent" || snapshot.bytes == current_default.as_bytes())
    {
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
                "[context-templates] {} changed before the distribution repair; the bytes read \
                    first are kept in {}",
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

        let published_at = match atomically_replace_context_template(&path, current_default, clock)
        {
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

    // Root migration authority comes only from exact shipped bytes, independently
    // of absent, stale, or forged persisted state.
    if spec.id == "rootAgent"
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return sync_generated_template(
            &path,
            spec,
            &snapshot.sha256,
            loaded,
            &current_default_sha256,
            clock,
        );
    }
    if spec.id == "rootAgent" {
        log::warn!(
            "[context-templates] custom Root context {} is solely authoritative",
            path.display()
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
            return sync_generated_template(
                &path,
                spec,
                &snapshot.sha256,
                loaded,
                &current_default_sha256,
                clock,
            );
        }
    }

    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry
        && spec
            .is_known_generated
            .is_some_and(|matches| matches(&snapshot.content))
    {
        return sync_generated_template(
            &path,
            spec,
            &snapshot.sha256,
            loaded,
            &current_default_sha256,
            clock,
        );
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
            "[context-templates] preserving customized context template {}; a newer default is \
                available",
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
    // #1795: the four project-level shared directories are created when a
    // project's `.ac` root is bootstrapped. Hard error, like the
    // `create_dir_all(ac_root)` two lines above and on the same root: an `.ac`
    // root that cannot take a subdirectory is already a broken bootstrap.
    crate::config::shared_locations::create_project_shared_dirs(ac_root).map_err(|e| {
        format!(
            "failed to create project shared directories under {}: {}",
            ac_root.display(),
            e
        )
    })?;
    if let Err(error) = crate::config::shared_locations::ensure_project_skills_dir(ac_root) {
        log::warn!(
            "[project-skills] unavailable at {}: {}; continuing without project skill discovery",
            ac_root
                .join(crate::config::shared_locations::PROJECT_SKILLS_DIR)
                .display(),
            error
        );
    }
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
                "[context-templates] {} was replaced with the current default; the previous bytes \
                    are in {}",
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
                "[979] preserving {} after a failed retirement move: the source or the destination \
                    is no longer the entry this call created",
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
            "[979] {} held unknown or custom content and was moved to the inert backup {}; the \
                Root Agent no longer consumes it",
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
            "[979] portable context-template state at {} is unreadable or malformed; leaving it \
                untouched (the standalone global is already retired)",
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

    let published_at =
        match atomically_replace_context_template(&path, (spec.current_content)(), clock) {
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
mod tests;
