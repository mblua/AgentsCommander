use crate::config::injected_messages::render;
use crate::config::injected_messages::BRANCH_STALE_MESSAGE_ID;
use crate::config::injected_messages::CI_FINISHED_MESSAGE_ID;
use crate::config::injected_messages::CI_STARTED_MESSAGE_ID;
use crate::config::injected_messages::CONTEXT_ALERT_MESSAGE_ID;
use crate::config::injected_messages::MAX_RENDERED_BYTES;
use crate::config::injected_messages::NOTICE_BLIND_GAP_MESSAGE_ID;
use crate::config::injected_messages::TOKEN_AT;
use crate::config::injected_messages::TOKEN_BASE;
use crate::config::injected_messages::TOKEN_BEHIND;
use crate::config::injected_messages::TOKEN_BRANCH;
use crate::config::injected_messages::TOKEN_GAP;
use crate::config::injected_messages::TOKEN_MEMBER;
use crate::config::injected_messages::TOKEN_OBSERVED;
use crate::config::injected_messages::TOKEN_REPO;
use crate::config::injected_messages::TOKEN_SHA;
use crate::config::injected_messages::TOKEN_SINCE;
use crate::config::injected_messages::TOKEN_THRESHOLDS;
use crate::config::injected_messages::TOKEN_WORKGROUP;
use crate::config::settings::AgentConfig;
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::inject::LogicalPtyCommand;
use crate::pty::manager::PtyManager;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

pub(crate) fn sender_name_for_session_cwd_with_root_flag(
    working_directory: &str,
    is_root_agent: bool,
) -> String {
    if is_root_agent {
        crate::config::root_agent::ROOT_AGENT_SENDER.to_string()
    } else {
        crate::config::teams::agent_fqn_from_path(working_directory)
    }
}

pub(crate) fn sender_name_for_session_cwd(working_directory: &str) -> String {
    let is_root_agent = crate::config::root_agent::is_root_agent_path(working_directory);
    sender_name_for_session_cwd_with_root_flag(working_directory, is_root_agent)
}

/// #881: session working dirs minus those under an archived project.
///
/// Subtractive on purpose: root-agent and ad-hoc CWD sessions outside every
/// project path must still be scanned.
pub(crate) fn retain_unarchived_session_dirs(
    dirs: Vec<(Uuid, String)>,
    normalized_archived_roots: &[String],
) -> Vec<(Uuid, String)> {
    if normalized_archived_roots.is_empty() {
        return dirs;
    }
    dirs.into_iter()
        .filter(|(_, dir)| {
            !crate::config::sessions_persistence::is_under_normalized_archived_roots(
                dir,
                normalized_archived_roots,
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDeliveryOrigin {
    FilesystemPoller,
    DbQueue,
}

/// Where a scanned message file was found (#2232 phase 7).
///
/// Attribution is **by directory**, decided once per sweep: the mailbox routes
/// and authorizes a message from this value and never from a path comparison at
/// each site. It is deliberately separate from [`WakeDeliveryOrigin`], which
/// names how a wake is delivered, not who produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboxOrigin {
    /// An agent replica outbox, `<repo>/<local-dir>/outbox`.
    Replica,
    /// The instance-private app outbox: the master/root-token origin.
    Master,
    /// The phase-7 Co-managed provenance queue, `<room>/.co-managed/queue`.
    /// The only tokenless origin; `from` is the authorizing FQN.
    CoManaged,
}

/// Canonical routing capability for an AgentsCommander-generated notice. The FQN and
/// replica path are validated and retained together so no internal caller can later route
/// by spelling alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalSystemTarget {
    fqn: String,
    replica_dir: PathBuf,
}

impl InternalSystemTarget {
    pub(crate) fn for_context_alert(fqn: String, replica_dir: PathBuf) -> Result<Self, String> {
        let metadata = std::fs::symlink_metadata(&replica_dir).map_err(|e| {
            format!(
                "Internal system target replica '{}' is not readable: {}",
                replica_dir.display(),
                e
            )
        })?;
        if !metadata.is_dir()
            || crate::commands::entity_creation::metadata_is_link_or_reparse(&metadata)
        {
            return Err(format!(
                "Internal system target replica '{}' must be a real non-link directory",
                replica_dir.display()
            ));
        }
        let canonical = std::fs::canonicalize(&replica_dir)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                format!(
                    "Internal system target replica '{}' cannot be canonicalized: {}",
                    replica_dir.display(),
                    e
                )
            })?;
        let layout = crate::config::ac_root::wg_replica_layout_from_agent_dir(&canonical)?
            .ok_or_else(|| {
                format!(
                    "Internal system target replica '{}' is not a canonical room replica",
                    canonical.display()
                )
            })?;
        crate::commands::entity_creation::parse_team_from_workgroup_name(&layout.wg_name)?;
        crate::commands::entity_creation::validate_existing_name(&layout.agent_name, "Agent")?;
        let project = layout
            .project_dir
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| {
                !name.is_empty()
                    && !name.contains(':')
                    && !name.chars().any(|ch| ch == '\0' || ch.is_ascii_control())
            })
            .ok_or_else(|| {
                format!(
                    "Internal system target project '{}' has an invalid FQN component",
                    layout.project_dir.display()
                )
            })?
            .to_string();
        let expected = format!("{}:{}/{}", project, layout.wg_name, layout.agent_name);
        if fqn != expected {
            return Err(format!(
                "Internal system target FQN '{}' does not match canonical replica '{}' (expected \
                '{}')",
                fqn,
                canonical.display(),
                expected
            ));
        }
        Ok(Self {
            fqn,
            replica_dir: canonical,
        })
    }

    pub(crate) fn fqn(&self) -> &str {
        &self.fqn
    }

    pub(crate) fn replica_dir(&self) -> &Path {
        &self.replica_dir
    }
}

/// The three remote-activity notices a validated [`InternalSystemNotice`] can
/// carry. Defined HERE, beside the notice that carries it, and deliberately not
/// `pty::remote_watcher::TransitionKind` (which would add
/// `phone::mailbox -> pty::remote_watcher`) nor a type in `session::remote_alerts`
/// (which would add `phone::mailbox -> session::remote_alerts`); either arc
/// breaks the phase's dependency-cycle condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteNoticeKind {
    CiStarted,
    CiFinished,
    BranchStale,
}

/// Validated fixed facts rendered by the private system formatter.
///
/// Two variants, because the two notices carry different facts and a struct with
/// every field optional would make "a CI notice has no thresholds" a runtime
/// convention instead of a type. `for_context_alert` keeps its name, signature
/// and error strings, which is what lets every existing construction site compile
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InternalSystemNotice {
    ContextAlert {
        member: String,
        workgroup: String,
        observed: u8,
        thresholds: Vec<u8>,
    },
    RemoteActivity {
        kind: RemoteNoticeKind,
        repo_nwo: String,
        branch: String,
        sha7: String,
        base_branch: String,
        behind_by: Option<u32>,
        at: String,
        blind_gap: Option<(String, String)>,
    },
}

/// An `nwo` shaped `owner/name`, each part non-empty and free of anything that
/// is not a name character. The rendered `%REPO%` is pasted into `gh -R`, so a
/// value that is merely printable is not enough.
pub(crate) fn is_repo_nwo(value: &str) -> bool {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    parts.next().is_none()
        && !owner.is_empty()
        && !name.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/'))
}

pub(crate) fn is_control_free(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

pub(crate) fn is_sha7(value: &str) -> bool {
    value.len() == 7
        && value
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
}

/// The same character-boundary walk as `config::injected_messages::sanitize`,
/// re-applied to the CONCATENATION. `sanitize` caps each render on its own, so a
/// base template and a blind-gap suffix can each pass the cap and violate it
/// together. `str::floor_char_boundary` is unstable on the pinned toolchain.
pub(crate) fn truncate_to_rendered_cap(mut rendered: String) -> String {
    let mut end = MAX_RENDERED_BYTES.min(rendered.len());
    while end > 0 && !rendered.is_char_boundary(end) {
        end -= 1;
    }
    rendered.truncate(end);
    rendered
}

impl InternalSystemNotice {
    pub(crate) fn for_context_alert(
        member: String,
        workgroup: String,
        observed: u8,
        thresholds: Vec<u8>,
    ) -> Result<Self, String> {
        crate::commands::entity_creation::validate_existing_name(&member, "Agent")?;
        crate::commands::entity_creation::parse_team_from_workgroup_name(&workgroup)?;
        if observed > 100 {
            return Err("Context alert observation must be from 0 through 100".to_string());
        }
        if thresholds.is_empty() || thresholds.len() > 3 {
            return Err("Context alert notice must contain 1 through 3 thresholds".to_string());
        }
        if thresholds
            .iter()
            .any(|threshold| !(1..=100).contains(threshold) || *threshold > observed)
            || thresholds.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(
                "Context alert notice thresholds must be strictly ascending, distinct, from 1 \
                through 100, and no greater than the observation"
                    .to_string(),
            );
        }
        Ok(Self::ContextAlert {
            member,
            workgroup,
            observed,
            thresholds,
        })
    }

    /// The remote-activity constructor. `base_branch` is required and non-empty
    /// for `BranchStale` (Phase A resolves the label; this phase only renders it)
    /// and must be empty for the two CI kinds, and `behind_by` follows the same
    /// rule, because a `%BEHIND%` that renders to nothing would silently produce
    /// a wrong sentence rather than a missing one. Every violation is an `Err`,
    /// never a silent default.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_remote_activity(
        kind: RemoteNoticeKind,
        repo_nwo: String,
        branch: String,
        sha7: String,
        base_branch: String,
        behind_by: Option<u32>,
        at: String,
        blind_gap: Option<(String, String)>,
    ) -> Result<Self, String> {
        if !is_repo_nwo(&repo_nwo) {
            return Err(format!(
                "Remote activity notice requires an owner/name repository, got '{repo_nwo}'"
            ));
        }
        if branch.is_empty() || !is_control_free(&branch) {
            return Err(
                "Remote activity notice requires a non-empty branch without control characters"
                    .to_string(),
            );
        }
        if !is_sha7(&sha7) {
            return Err(format!(
                "Remote activity notice requires a 7-character lowercase hex commit, got '{sha7}'"
            ));
        }
        if at.is_empty() {
            return Err("Remote activity notice requires an observation timestamp".to_string());
        }
        let stale = kind == RemoteNoticeKind::BranchStale;
        if stale && (base_branch.is_empty() || !is_control_free(&base_branch)) {
            return Err(
                "A branch-stale notice requires a non-empty default-branch label without control \
                characters"
                    .to_string(),
            );
        }
        if !stale && !base_branch.is_empty() {
            return Err("Only a branch-stale notice carries a default-branch label".to_string());
        }
        match (stale, behind_by) {
            (true, Some(count)) if count > 0 => {}
            (true, _) => {
                return Err(
                    "A branch-stale notice requires a commit count greater than zero".to_string(),
                )
            }
            (false, None) => {}
            (false, Some(_)) => {
                return Err("Only a branch-stale notice carries a commit count".to_string())
            }
        }
        Ok(Self::RemoteActivity {
            kind,
            repo_nwo,
            branch,
            sha7,
            base_branch,
            behind_by,
            at,
            blind_gap,
        })
    }

    /// `Some` for a context alert, `None` for a remote-activity notice. An empty
    /// slice would be a silent lie at a comparison site: a remote notice has no
    /// thresholds at all, it does not have none crossed.
    pub(crate) fn thresholds(&self) -> Option<&[u8]> {
        match self {
            Self::ContextAlert { thresholds, .. } => Some(thresholds),
            Self::RemoteActivity { .. } => None,
        }
    }

    /// #1157 - the wording now lives in the operator-editable injected-message
    /// registry. This function keeps its signature and its threshold formatting;
    /// the only embedded copy of the text is `DEFAULT_CONTEXT_ALERT_TEMPLATE`.
    pub(crate) fn line(
        &self,
        warnings: &crate::config::injected_messages::MessageWarnings,
    ) -> String {
        match self {
            Self::ContextAlert {
                member,
                workgroup,
                observed,
                thresholds,
            } => {
                let thresholds = thresholds
                    .iter()
                    .map(|threshold| format!("{}%", threshold))
                    .collect::<Vec<_>>()
                    .join(", ");
                // `values` borrows, so the observed percentage needs a binding that
                // outlives the slice.
                let observed = format!("{}%", observed);
                render(
                    CONTEXT_ALERT_MESSAGE_ID,
                    &[
                        (TOKEN_MEMBER, member.as_str()),
                        (TOKEN_WORKGROUP, workgroup.as_str()),
                        (TOKEN_THRESHOLDS, thresholds.as_str()),
                        (TOKEN_OBSERVED, observed.as_str()),
                    ],
                    warnings,
                )
            }
            Self::RemoteActivity {
                kind,
                repo_nwo,
                branch,
                sha7,
                base_branch,
                behind_by,
                at,
                blind_gap,
            } => {
                let behind = behind_by.map(|count| count.to_string());
                let (id, values) = match kind {
                    RemoteNoticeKind::CiStarted => (
                        CI_STARTED_MESSAGE_ID,
                        vec![
                            (TOKEN_REPO, repo_nwo.as_str()),
                            (TOKEN_BRANCH, branch.as_str()),
                            (TOKEN_SHA, sha7.as_str()),
                            (TOKEN_AT, at.as_str()),
                        ],
                    ),
                    RemoteNoticeKind::CiFinished => (
                        CI_FINISHED_MESSAGE_ID,
                        vec![
                            (TOKEN_REPO, repo_nwo.as_str()),
                            (TOKEN_BRANCH, branch.as_str()),
                            (TOKEN_SHA, sha7.as_str()),
                            (TOKEN_AT, at.as_str()),
                        ],
                    ),
                    RemoteNoticeKind::BranchStale => (
                        BRANCH_STALE_MESSAGE_ID,
                        vec![
                            (TOKEN_REPO, repo_nwo.as_str()),
                            (TOKEN_BRANCH, branch.as_str()),
                            (TOKEN_BASE, base_branch.as_str()),
                            (
                                TOKEN_BEHIND,
                                behind
                                    .as_deref()
                                    .expect("a branch-stale notice always carries a commit count"),
                            ),
                            (TOKEN_AT, at.as_str()),
                        ],
                    ),
                };
                let mut rendered = render(id, &values, warnings);
                if let Some((gap, since)) = blind_gap {
                    // A SUFFIX id, not twin ids: the blindness clause is edited
                    // once and applies to every notice, present and future.
                    rendered.push_str(&render(
                        NOTICE_BLIND_GAP_MESSAGE_ID,
                        &[(TOKEN_GAP, gap.as_str()), (TOKEN_SINCE, since.as_str())],
                        warnings,
                    ));
                }
                truncate_to_rendered_cap(rendered)
            }
        }
    }
}

pub(crate) type InternalNoticeGuard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub(crate) enum WakeContent<'a> {
    Peer {
        from: &'a str,
        body: &'a str,
        origin: WakeDeliveryOrigin,
        /// #2232 phase 7: true only for a message the Co-managed supervisor
        /// wrote into the provenance queue. Display-only; `from` stays the
        /// exact FQN and is the credential.
        comanaged: bool,
    },
    InternalSystem(
        &'a InternalSystemNotice,
        &'a crate::config::injected_messages::MessageWarnings,
    ),
}

pub(crate) fn format_wake_content(content: WakeContent<'_>) -> String {
    match content {
        WakeContent::Peer {
            from,
            body,
            origin,
            comanaged,
        } => {
            let _ = origin;
            if comanaged {
                let display_from =
                    crate::phone::messaging::compose_sender_for_comanaged_origin(from);
                crate::phone::messaging::format_pty_wrap(&display_from, body)
            } else {
                crate::phone::messaging::format_pty_wrap(from, body)
            }
        }
        WakeContent::InternalSystem(notice, warnings) => format!("\n{}\n\r", notice.line(warnings)),
    }
}

pub(crate) fn internal_system_envelope(target: &InternalSystemTarget) -> OutboxMessage {
    OutboxMessage {
        id: Uuid::new_v4().to_string(),
        token: None,
        from: "AgentsCommander".to_string(),
        to: target.fqn.clone(),
        body: String::new(),
        mode: "wake".to_string(),
        get_output: false,
        request_id: None,
        sender_agent: None,
        preferred_agent: "auto".to_string(),
        requested_profile: None,
        effective_agent_id: None,
        effective_profile: None,
        profile_fallback_applied: false,
        dispatch_not_applied: None,
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: None,
        target: None,
        force: None,
        timeout_secs: None,
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    }
}

pub(crate) fn canonical_cwd_owned_by_replica(
    cwd: &str,
    replica_dir: &Path,
) -> Result<bool, String> {
    let lexical = Path::new(cwd);
    let Some(lexical_replica) = lexical.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("__agent_"))
    }) else {
        return Ok(false);
    };
    let Some(lexical_workgroup) = lexical_replica.parent() else {
        return Ok(false);
    };
    let Some(lexical_ac_root) = lexical_workgroup.parent() else {
        return Ok(false);
    };
    for (path, label) in [
        (lexical_replica, "replica"),
        (lexical_workgroup, "room"),
        (lexical_ac_root, "Project AC Root"),
    ] {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            format!(
                "Failed to inspect recipient {} '{}': {}",
                label,
                path.display(),
                error
            )
        })?;
        if !metadata.is_dir()
            || crate::commands::entity_creation::metadata_is_link_or_reparse(&metadata)
        {
            return Ok(false);
        }
    }
    let canonical_replica = std::fs::canonicalize(lexical_replica)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|error| {
            format!(
                "Failed to canonicalize recipient replica '{}': {}",
                lexical_replica.display(),
                error
            )
        })?;
    if canonical_replica != replica_dir {
        return Ok(false);
    }
    let canonical = std::fs::canonicalize(cwd)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| format!("Failed to canonicalize recipient CWD '{}': {}", cwd, e))?;
    Ok(canonical == replica_dir || canonical.starts_with(replica_dir))
}

pub(crate) fn container_body_override_for_delivery(
    origin: WakeDeliveryOrigin,
    body: &str,
    sender_root: Option<&Path>,
) -> Result<Option<String>, String> {
    if origin != WakeDeliveryOrigin::FilesystemPoller {
        return Ok(None);
    }
    if crate::phone::messaging::parse_file_notification(body).is_none() {
        return Ok(None);
    }
    let sender_root = sender_root
        .ok_or_else(|| "file notification sender path could not be resolved".to_string())?;
    inline_body_from_file_notification(body, sender_root)
}

pub(crate) fn inline_body_from_file_notification(
    body: &str,
    sender_root: &Path,
) -> Result<Option<String>, String> {
    let Some(notification_path) = crate::phone::messaging::parse_file_notification(body) else {
        return Ok(None);
    };
    let filename = crate::phone::messaging::notification_filename(notification_path)
        .ok_or_else(|| "file notification does not include a message filename".to_string())?;
    let parent = Path::new(notification_path)
        .parent()
        .ok_or_else(|| "file notification does not include a parent directory".to_string())?;
    if parent.file_name().and_then(|n| n.to_str())
        != Some(crate::phone::messaging::MESSAGING_DIR_NAME)
    {
        return Err("file notification parent is not a messaging directory".to_string());
    }

    let wg_root = crate::phone::messaging::workgroup_root(sender_root)
        .map_err(|e| format!("sender room could not be resolved: {}", e))?;
    let allowed_messaging_dir = crate::phone::messaging::messaging_dir(&wg_root)
        .map_err(|e| format!("sender messaging directory could not be resolved: {}", e))?;
    let canon_allowed = std::fs::canonicalize(&allowed_messaging_dir)
        .map_err(|e| format!("sender messaging directory not readable: {}", e))?;
    let canon_parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("file notification parent not readable: {}", e))?;
    if canon_parent != canon_allowed {
        return Err("file notification parent is outside sender messaging directory".to_string());
    }

    let abs = crate::phone::messaging::resolve_existing_message(&allowed_messaging_dir, filename)
        .map_err(|e| format!("message file not readable for container delivery: {}", e))?;
    let bytes = std::fs::read(&abs)
        .map_err(|e| format!("message file not readable for container delivery: {}", e))?;
    if bytes.len() > crate::api::message_store::INLINE_BODY_MAX_BYTES {
        return Err(format!(
            "message file exceeds inline cap ({} bytes)",
            crate::api::message_store::INLINE_BODY_MAX_BYTES
        ));
    }
    let body = String::from_utf8(bytes)
        .map_err(|e| format!("message file is not UTF-8 for container delivery: {}", e))?;
    Ok(Some(body))
}

pub(crate) fn is_container_backend_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Uuid,
) -> Result<bool, String> {
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
    let mgr = pty_mgr
        .lock()
        .map_err(|e| format!("PTY lock failed: {}", e))?;
    Ok(mgr.backend_kind(session_id) == Some(SessionBackendKind::ContainerTransport))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRefreshEventPayload {
    pub(crate) id: String,
    pub(crate) project_path: String,
    pub(crate) changed_path: Option<String>,
    pub(crate) changed_name: Option<String>,
    pub(crate) reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectRefreshPollBatch {
    pub(crate) payloads: Vec<ProjectRefreshEventPayload>,
    pub(crate) processed_paths: Vec<PathBuf>,
}

pub(crate) fn canonical_project_refresh_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

pub(crate) fn project_refresh_priority(reason: &str) -> u8 {
    if reason == "projectRegistered" {
        0
    } else {
        1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWakeAgentCommand {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) source: String,
    pub(crate) raw_command: String,
}

#[derive(Debug)]
pub(crate) struct ResolvedWakeSpawnPlan {
    pub(crate) resolved_command: ResolvedWakeAgentCommand,
    pub(crate) cwd: String,
    pub(crate) session_name: String,
    pub(crate) spawn_shell: String,
    pub(crate) spawn_args: Vec<String>,
    pub(crate) spawn_label: Option<String>,
    pub(crate) configured_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    /// #1271 - the configured host shell paired with the resolved agent, built
    /// from the same settings guard that produced the spawn.
    pub(crate) resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

pub(crate) fn normalize_agent_for_wake(
    agent: &AgentConfig,
    source: String,
) -> Result<ResolvedWakeAgentCommand, String> {
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map_err(|e| {
        format!(
            "Invalid agent command from {} (agent id '{}', label '{}'): {}. command={:?}",
            source, agent.id, agent.label, e, agent.command
        )
    })?;

    Ok(ResolvedWakeAgentCommand {
        shell: normalized.shell,
        shell_args: normalized.shell_args,
        agent_id: Some(agent.id.clone()),
        agent_label: Some(agent.label.clone()),
        source,
        raw_command: agent.command.clone(),
    })
}

pub(crate) fn resolve_wake_agent_command_from_sources(
    agents: &[AgentConfig],
    preferred_agent: &str,
    destination_current_agent: Option<&str>,
    destination_last_agent: Option<&str>,
    destination_config_path: Option<&Path>,
    sender_agent: Option<&str>,
) -> Result<Option<ResolvedWakeAgentCommand>, String> {
    if preferred_agent != "auto" {
        if let Some(agent) = agents.iter().find(|a| a.id == preferred_agent) {
            return normalize_agent_for_wake(
                agent,
                format!("preferredAgent '{}'", preferred_agent),
            )
            .map(Some);
        }
        log::warn!(
            "[mailbox] wake: preferredAgent '{}' did not match a configured agent id; falling back \
            to auto resolution",
            preferred_agent
        );
    }

    // The Selection UI assignment (currentCodingAgent) is an explicit user choice
    // and is what the "Entire Workgroup" coding-agent assignment writes to each
    // replica. Honor it before launch history (lastCodingAgent) so the assignment
    // propagates to freshly-spawned members that have never recorded a
    // lastCodingAgent, and so a new selection overrides a stale launch history.
    if let Some(agent_id) = destination_current_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            return normalize_agent_for_wake(agent, format!("currentCodingAgent '{}'", agent_id))
                .map(Some);
        }
        log::debug!(
            "[mailbox] wake: currentCodingAgent '{}' did not match a configured agent id; falling \
            back",
            agent_id
        );
    }

    if let Some(agent_id) = destination_last_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            let source = match destination_config_path {
                Some(path) => format!("lastCodingAgent '{}' from {}", agent_id, path.display()),
                None => format!("lastCodingAgent '{}'", agent_id),
            };
            return normalize_agent_for_wake(agent, source).map(Some);
        }
        log::debug!(
            "[mailbox] wake: lastCodingAgent '{}' did not match a configured agent id; falling \
            back",
            agent_id
        );
    }

    if let Some(agent_id) = sender_agent {
        if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
            return normalize_agent_for_wake(agent, format!("senderAgent '{}'", agent_id))
                .map(Some);
        }
        log::debug!(
            "[mailbox] wake: senderAgent '{}' did not match a configured agent id; falling back",
            agent_id
        );
    }

    agents
        .first()
        .map(|agent| {
            normalize_agent_for_wake(agent, format!("first configured agent '{}'", agent.id))
        })
        .transpose()
}

pub(crate) fn validate_root_sender_route(
    to: &str,
    project_paths: &[String],
    is_master: bool,
    saw_session_token: bool,
    token_belongs_to_root_agent: bool,
) -> Result<(), &'static str> {
    if !is_master && (!saw_session_token || !token_belongs_to_root_agent) {
        return Err("Root Agent sender requires the live root session token");
    }

    if crate::config::teams::verified_wg_coordinator_target(to, project_paths).is_none() {
        return Err("Root Agent can only message verified Room orchestrator replicas");
    }

    Ok(())
}

pub(crate) fn validate_coordinator_to_root_route(
    from: &str,
    project_paths: &[String],
) -> Result<(), &'static str> {
    if crate::config::teams::verified_wg_coordinator_target(from, project_paths).is_none() {
        return Err("Only verified Room orchestrator replicas may message the Root Agent");
    }
    Ok(())
}

pub(crate) fn validate_root_sender_payload(msg: &OutboxMessage) -> Result<(), String> {
    let root_dir = crate::config::root_agent::root_agent_dir()?;
    validate_root_sender_payload_with_root_dir(msg, Path::new(&root_dir))
}

pub(crate) fn validate_root_sender_payload_with_root_dir(
    msg: &OutboxMessage,
    root_agent_dir: &Path,
) -> Result<(), String> {
    if msg.command.is_some() {
        return Err("Root Agent messages must use --send; remote commands are not allowed".into());
    }
    if msg.action.is_some() {
        return Err("Root Agent messages must use --send; action messages are not allowed".into());
    }

    let notification_path = crate::phone::messaging::parse_file_notification(&msg.body)
        .ok_or_else(|| "Root Agent messages must be canonical file notifications".to_string())?;
    let filename = crate::phone::messaging::notification_filename(notification_path)
        .ok_or_else(|| "Root Agent file notification must point to a Markdown file".to_string())?;
    crate::phone::messaging::validate_root_notification_filename(filename)
        .map_err(|e| format!("Root Agent file notification is invalid: {}", e))?;

    let notification_path = Path::new(notification_path);
    if !notification_path.is_absolute() {
        return Err("Root Agent file notification must use an absolute path".into());
    }
    let canon_file = std::fs::canonicalize(notification_path)
        .map_err(|e| format!("Root Agent file notification target is not readable: {}", e))?;
    if !canon_file
        .metadata()
        .map_err(|e| format!("Root Agent file notification target is not readable: {}", e))?
        .is_file()
    {
        return Err("Root Agent file notification target is not a regular file".into());
    }

    let root_messaging_dir = root_agent_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    let canon_root_messaging_dir = std::fs::canonicalize(&root_messaging_dir).map_err(|e| {
        format!(
            "Root Agent messaging directory is not readable at {}: {}",
            root_messaging_dir.display(),
            e
        )
    })?;
    if canon_file.parent() != Some(canon_root_messaging_dir.as_path()) {
        return Err(
            "Root Agent file notification must point inside ac-root-agent/messaging".into(),
        );
    }

    Ok(())
}

/// If `outbox_file` lives at
/// `<project_dir>/<workspace>/wg-<N>-*/__agent_*/<local-dir>/outbox/<file>.json`,
/// return `project_dir` as a UTF-8 `String`. Otherwise, `None`.
///
/// Mirrors the WG-replica walk-up in `cli::send::derive_root_project_dir` so
/// the mailbox can resolve qualified WG-peer FQNs against the same root-walk-up
/// source the sending CLI used (and the same source `list_peers::detect_wg_replica`
/// uses to report siblings as `reachable: true`). In-memory only — settings.json
/// is NOT mutated. See #228 D1-a.
///
/// The path layout is fixed by `MailboxPoller::poll` which constructs outbox
/// paths as `Path::new(<all_paths-entry>).join(agent_local_dir_name()).join("outbox")`.
/// We walk ancestors: `<file>.json` → `outbox` → `<local-dir>` → `<__agent_*>` →
/// `<wg-*>` → `<workspace>` → `<project_dir>`.
///
/// Uses `to_str()` (NOT `to_string_lossy()`) for parity with
/// `list_peers::detect_wg_replica`. The shared
/// `__agent_* -> wg-* -> <workspace> -> <project>` walk-up is delegated to
/// `config::ac_root::wg_replica_layout_from_agent_dir` (single source with
/// `cli::send::derive_root_project_dir` and `list_peers::detect_wg_replica`,
/// see #726); only the outbox-specific `<file>.json -> outbox -> <local-dir>`
/// prefix to reach the `__agent_*` dir stays here.
pub(crate) fn derive_project_from_outbox_path(
    outbox_file: &Path,
) -> Result<Option<String>, String> {
    let Some(canon) = std::fs::canonicalize(outbox_file).ok() else {
        return Ok(None);
    };
    // canon = <project>/<workspace>/wg-*/__agent_*/<local-dir>/outbox/<file>.json
    let Some(outbox_dir) = canon.parent() else {
        return Ok(None);
    };
    if outbox_dir.file_name().and_then(|n| n.to_str()) != Some("outbox") {
        return Ok(None);
    }
    let Some(local_dir) = outbox_dir.parent() else {
        return Ok(None);
    };
    let Some(agent_dir) = local_dir.parent() else {
        return Ok(None);
    };
    match crate::config::ac_root::wg_replica_layout_from_agent_dir(agent_dir)? {
        Some(layout) => Ok(layout.project_dir.to_str().map(|path| path.to_string())),
        None => Ok(None),
    }
}

/// Tracks delivery attempts for a single outbox message.
pub(crate) struct RetryState {
    pub(crate) attempt_count: u32,
    pub(crate) logged: bool,
}

pub(crate) const MAX_DELIVERY_ATTEMPTS: u32 = 10;

pub(crate) const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";

pub(crate) const ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND: &str =
    "Unsupported logical remote command";

pub(crate) const ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND: &str =
    "Cannot execute logical remote command";

/// #1635-2 D6: a present `requestedProfile` that fails A-Z normalization is a
/// permanent delivery error. The CLI already rejects it before enqueue
/// (1635-1); this defends hand-written outbox JSON.
pub(crate) const ERR_INVALID_DISPATCH_PROFILE: &str =
    "send: requestedProfile must be a single letter A through Z";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedRemotePtyCommand {
    pub(crate) logical: LogicalPtyCommand,
    pub(crate) text: &'static str,
}

pub(crate) fn logical_command_wire_value(command: LogicalPtyCommand) -> &'static str {
    match command {
        LogicalPtyCommand::Clear => "clear",
        LogicalPtyCommand::Compact => "compact",
    }
}

pub(crate) fn parse_remote_pty_command(wire_value: &str) -> Result<LogicalPtyCommand, String> {
    LogicalPtyCommand::from_wire_value(wire_value).ok_or_else(|| {
        format!(
            "{} '{}'. Allowed values: clear, compact",
            ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND, wire_value
        )
    })
}
