use std::collections::HashMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Emitter, Manager};
use uuid::Uuid;

use crate::config::injected_messages::{
    render, BRANCH_STALE_MESSAGE_ID, CI_FINISHED_MESSAGE_ID, CI_STARTED_MESSAGE_ID,
    CONTEXT_ALERT_MESSAGE_ID, MAX_RENDERED_BYTES, NOTICE_BLIND_GAP_MESSAGE_ID, TOKEN_AT,
    TOKEN_BASE, TOKEN_BEHIND, TOKEN_BRANCH, TOKEN_GAP, TOKEN_MEMBER, TOKEN_OBSERVED, TOKEN_REPO,
    TOKEN_SHA, TOKEN_SINCE, TOKEN_THRESHOLDS, TOKEN_WORKGROUP,
};
use crate::config::instance_artifacts::{
    PROJECT_REFRESH_REQUESTS_DIR_NAME, SESSION_REQUESTS_DIR_NAME,
};
use crate::config::sessions_persistence::RaiseHandPersistOutcome;
use crate::config::settings::{AgentConfig, AppSettings, SettingsState};
use crate::config::teams;
use crate::phone::consumption::{verdict_to_result, ConsumptionVerdict};
use crate::phone::types::OutboxMessage;
use crate::pty::backend::SessionBackendKind;
use crate::pty::inject::{LogicalPtyCommand, SettleReadiness};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{Session, SessionCommunicationKind, SessionInfo, SessionStatus};
#[cfg(test)]
use crate::session::session::{SessionCommunication, SessionRepo};
use crate::{AppOutbox, MasterToken};
use tokio_util::sync::CancellationToken;

fn sender_name_for_session_cwd_with_root_flag(
    working_directory: &str,
    is_root_agent: bool,
) -> String {
    if is_root_agent {
        crate::config::root_agent::ROOT_AGENT_SENDER.to_string()
    } else {
        crate::config::teams::agent_fqn_from_path(working_directory)
    }
}

fn sender_name_for_session_cwd(working_directory: &str) -> String {
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
fn is_repo_nwo(value: &str) -> bool {
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

fn is_control_free(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

fn is_sha7(value: &str) -> bool {
    value.len() == 7
        && value
            .chars()
            .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
}

/// The same character-boundary walk as `config::injected_messages::sanitize`,
/// re-applied to the CONCATENATION. `sanitize` caps each render on its own, so a
/// base template and a blind-gap suffix can each pass the cap and violate it
/// together. `str::floor_char_boundary` is unstable on the pinned toolchain.
fn truncate_to_rendered_cap(mut rendered: String) -> String {
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
    fn line(&self) -> String {
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
                let mut rendered = render(id, &values);
                if let Some((gap, since)) = blind_gap {
                    // A SUFFIX id, not twin ids: the blindness clause is edited
                    // once and applies to every notice, present and future.
                    rendered.push_str(&render(
                        NOTICE_BLIND_GAP_MESSAGE_ID,
                        &[(TOKEN_GAP, gap.as_str()), (TOKEN_SINCE, since.as_str())],
                    ));
                }
                truncate_to_rendered_cap(rendered)
            }
        }
    }
}

pub(crate) type InternalNoticeGuard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// The notice is the largest variant by design: the remote-activity facts are
/// eight fields because the text and its validation need all eight, and boxing
/// them would add an allocation per notice to buy nothing measurable. The peer
/// arm is a borrow, so it is small either way.
#[allow(clippy::large_enum_variant)]
enum WakeDelivery<'a> {
    Peer {
        message: &'a OutboxMessage,
        origin: WakeDeliveryOrigin,
        /// #2232 phase 7: display-only attribution for a Co-managed-origin
        /// message. False for every pre-existing caller.
        comanaged: bool,
        response: Option<&'a ResponseWakeWork>,
    },
    InternalSystem {
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    },
}

enum WakeContent<'a> {
    Peer {
        from: &'a str,
        body: &'a str,
        origin: WakeDeliveryOrigin,
        /// #2232 phase 7: true only for a message the Co-managed supervisor
        /// wrote into the provenance queue. Display-only; `from` stays the
        /// exact FQN and is the credential.
        comanaged: bool,
    },
    InternalSystem(&'a InternalSystemNotice),
}

fn format_wake_content(content: WakeContent<'_>) -> String {
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
        WakeContent::InternalSystem(notice) => format!("\n{}\n\r", notice.line()),
    }
}

fn internal_system_envelope(target: &InternalSystemTarget) -> OutboxMessage {
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

fn canonical_cwd_owned_by_replica(cwd: &str, replica_dir: &Path) -> Result<bool, String> {
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

fn container_body_override_for_delivery(
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

fn inline_body_from_file_notification(
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

fn is_container_backend_session<R: tauri::Runtime>(
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
struct ProjectRefreshEventPayload {
    id: String,
    project_path: String,
    changed_path: Option<String>,
    changed_name: Option<String>,
    reason: String,
}

#[derive(Debug, Default)]
struct ProjectRefreshPollBatch {
    payloads: Vec<ProjectRefreshEventPayload>,
    processed_paths: Vec<PathBuf>,
}

fn canonical_project_refresh_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .to_string()
}

fn project_refresh_priority(reason: &str) -> u8 {
    if reason == "projectRegistered" {
        0
    } else {
        1
    }
}

fn collect_project_refresh_requests(requests_dir: &Path) -> ProjectRefreshPollBatch {
    if !requests_dir.is_dir() {
        return ProjectRefreshPollBatch::default();
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(requests_dir) {
        Ok(rd) => rd
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect(),
        Err(e) => {
            log::warn!(
                "[project-refresh-requests] Failed to read {:?}: {}",
                requests_dir,
                e
            );
            return ProjectRefreshPollBatch::default();
        }
    };
    entries.sort();

    let mut batch = ProjectRefreshPollBatch::default();
    let mut selected_by_project: HashMap<String, (u8, ProjectRefreshEventPayload)> = HashMap::new();
    let mut project_order: Vec<String> = Vec::new();

    for path in entries {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let content = match read_text_bom_tolerant(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "[project-refresh-requests] Failed to read {:?}: {}",
                    path,
                    e
                );
                continue;
            }
        };

        let mut request: crate::cli::create_agent_matrix::ProjectRefreshRequest =
            match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "[project-refresh-requests] Failed to parse {:?}: {}",
                        path,
                        e
                    );
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };

        let canonical_project_path = canonical_project_refresh_path(&request.project_path);
        request.project_path = canonical_project_path.clone();
        batch.processed_paths.push(path);

        let priority = project_refresh_priority(&request.reason);
        let payload = ProjectRefreshEventPayload {
            id: request.id,
            project_path: request.project_path,
            changed_path: request.changed_path,
            changed_name: request.changed_name,
            reason: request.reason,
        };

        match selected_by_project.get(&canonical_project_path) {
            Some((existing_priority, _)) if *existing_priority <= priority => {}
            Some(_) => {
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
            None => {
                project_order.push(canonical_project_path.clone());
                selected_by_project.insert(canonical_project_path, (priority, payload));
            }
        }
    }

    for project_path in project_order {
        if let Some((_, payload)) = selected_by_project.remove(&project_path) {
            batch.payloads.push(payload);
        }
    }

    batch
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedWakeAgentCommand {
    shell: String,
    shell_args: Vec<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    source: String,
    raw_command: String,
}

#[derive(Debug)]
struct ResolvedWakeSpawnPlan {
    resolved_command: ResolvedWakeAgentCommand,
    cwd: String,
    session_name: String,
    spawn_shell: String,
    spawn_args: Vec<String>,
    spawn_label: Option<String>,
    configured_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    /// #1271 - the configured host shell paired with the resolved agent, built
    /// from the same settings guard that produced the spawn.
    resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

fn normalize_agent_for_wake(
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

fn resolve_wake_agent_command_from_sources(
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

fn validate_root_sender_route(
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

fn validate_coordinator_to_root_route(
    from: &str,
    project_paths: &[String],
) -> Result<(), &'static str> {
    if crate::config::teams::verified_wg_coordinator_target(from, project_paths).is_none() {
        return Err("Only verified Room orchestrator replicas may message the Root Agent");
    }
    Ok(())
}

fn validate_root_sender_payload(msg: &OutboxMessage) -> Result<(), String> {
    let root_dir = crate::config::root_agent::root_agent_dir()?;
    validate_root_sender_payload_with_root_dir(msg, Path::new(&root_dir))
}

fn validate_root_sender_payload_with_root_dir(
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
fn derive_project_from_outbox_path(outbox_file: &Path) -> Result<Option<String>, String> {
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
struct RetryState {
    attempt_count: u32,
    logged: bool,
}

const MAX_DELIVERY_ATTEMPTS: u32 = 10;
const ERR_UNRESOLVABLE_AGENT: &str = "Could not resolve inbox for agent";
const ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND: &str = "Unsupported logical remote command";
const ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND: &str = "Cannot execute logical remote command";
/// #1635-2 D6: a present `requestedProfile` that fails A-Z normalization is a
/// permanent delivery error. The CLI already rejects it before enqueue
/// (1635-1); this defends hand-written outbox JSON.
const ERR_INVALID_DISPATCH_PROFILE: &str =
    "send: requestedProfile must be a single letter A through Z";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedRemotePtyCommand {
    logical: LogicalPtyCommand,
    text: &'static str,
}

fn logical_command_wire_value(command: LogicalPtyCommand) -> &'static str {
    match command {
        LogicalPtyCommand::Clear => "clear",
        LogicalPtyCommand::Compact => "compact",
    }
}

fn parse_remote_pty_command(wire_value: &str) -> Result<LogicalPtyCommand, String> {
    LogicalPtyCommand::from_wire_value(wire_value).ok_or_else(|| {
        format!(
            "{} '{}'. Allowed values: clear, compact",
            ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND, wire_value
        )
    })
}

fn resolve_remote_pty_command(
    shell: &str,
    wire_value: &str,
) -> Result<ResolvedRemotePtyCommand, String> {
    let logical = parse_remote_pty_command(wire_value)?;
    let text =
        crate::pty::inject::resolve_logical_command_text(shell, logical).ok_or_else(|| {
            format!(
            "{} '{}': session shell '{}' has no verified mapping. Claude / Codex / Antigravity / \
            Cursor agent direct shells use /clear and /compact; exact Pi uses /new for clear only. \
            cmd / pwsh outer wrappers and Pi compact are unsupported.",
            ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND,
            logical_command_wire_value(logical),
            shell
        )
        })?;
    Ok(ResolvedRemotePtyCommand { logical, text })
}

fn is_permanent_delivery_error(error: &str) -> bool {
    error.contains(ERR_UNRESOLVABLE_AGENT)
        || error.starts_with(ERR_UNSUPPORTED_LOGICAL_REMOTE_COMMAND)
        || error.starts_with(ERR_UNMAPPED_LOGICAL_REMOTE_COMMAND)
        || error.starts_with(ERR_INVALID_DISPATCH_PROFILE)
}

/// (#1399) Claim marker for a message handed to a wake worker. Suffix, not a
/// subdirectory, so `move_to_delivered` / `reject_message` / the outbox-project
/// walk-up keep deriving from the same parent directory.
const WAKE_CLAIM_SUFFIX: &str = ".in-flight";
const WAKE_CLAIM_EXTENSION: &str = "in-flight";

/// (#1399) `<dir>/<id>.json` -> `<dir>/<id>.json.in-flight`. Appends to the
/// `OsString` of `file_name()` and uses `with_file_name`, never
/// `Path::with_extension`, so an id containing a dot cannot lose a component.
fn wake_claim_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(WAKE_CLAIM_SUFFIX);
    path.with_file_name(name)
}

/// (#1399) Inverse of `wake_claim_path`. `Some` only when the file name ends
/// with exactly `.json.in-flight`: reclamation must never resurrect a file this
/// poller could not have claimed. Strips from `file_name()`, never via
/// `with_extension`, so an id containing a dot survives; a non-UTF-8 name
/// yields `None`, the right answer for a file this poller did not write.
fn wake_claim_origin(claim: &Path) -> Option<PathBuf> {
    let name = claim.file_name()?.to_str()?;
    let origin = name.strip_suffix(WAKE_CLAIM_SUFFIX)?;
    if !origin.ends_with(".json") {
        return None;
    }
    Some(claim.with_file_name(origin))
}

fn wake_claim_has_settled_receipt(origin: &Path) -> Result<bool, String> {
    let Some(outbox_dir) = origin.parent() else {
        return Ok(false);
    };
    let Some(id) = origin.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(false);
    };
    let delivered = outbox_dir.join("delivered").join(format!("{}.json", id));
    if delivered.try_exists().map_err(|error| {
        format!(
            "could not inspect delivered payload receipt {:?}: {}",
            delivered, error
        )
    })? {
        return Ok(true);
    }
    let rejected = outbox_dir.join("rejected").join(format!("{}.json", id));
    rejected.try_exists().map_err(|error| {
        format!(
            "could not inspect rejected payload receipt {:?}: {}",
            rejected, error
        )
    })
}

/// (#1399) A failed restore leaves the durable claim in place. An owning drain
/// keeps and requeues its outcome; without ledger ownership, a later poll or
/// process restart retries general reclamation.
fn release_wake_claim(claim: &Path, origin: &Path) {
    if let Err(error) = std::fs::rename(claim, origin) {
        log::error!(
            "[mailbox] #1399 failed to return claim {:?} to {:?}: {}",
            claim,
            origin,
            error
        );
    }
}

/// (#1399) Send time for outbox ordering, or `None` when it cannot be read.
/// Never a filter: `None` sorts first so an unreadable or malformed document
/// still reaches the existing rejection path promptly.
///
/// Reads through `path_identity::read_bounded_regular` with the scan's own
/// byte cap, never a bare `std::fs::read`: the sort runs before classification
/// on every `*.json` in every outbox every cycle, so it must reach the same
/// verdict the hardened scan reader would (bounded, no-follow, regular files
/// only). Parsed as `serde_json::Value` because privileged PTY-input documents
/// share the outbox with `OutboxMessage` and both schemas carry a top-level
/// `timestamp`; parsed as RFC3339 rather than compared as bytes because
/// producers emit variable-width fractions and offsets.
fn outbox_message_send_time(path: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let (bytes, _identity) = crate::path_identity::read_bounded_regular(
        path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    )
    .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let raw = value.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|sent| sent.with_timezone(&chrono::Utc))
}

/// (#1399) Collapse spellings of one physical outbox to a single entry, keyed
/// by file-object identity. Two spellings of one directory (case, separator,
/// 8.3 short name) would otherwise both be scanned, keep two independent
/// `retry_tracker` counters, and let reclamation under the second spelling
/// resurrect a claim owned under the first.
///
/// `is_dir()` FIRST. `verify_directory` is expensive (one `symlink_metadata`
/// per path component plus its own handle work, roughly 14 syscalls for a WG
/// replica outbox), the component walk stats every ancestor before the leaf
/// fails, and the large majority of registered projects have no outbox. The
/// cheap `is_dir()` is already the outbox loop's own first test, so filtering
/// on it here costs nothing and keeps thousands of stat calls off the 3 s
/// cycle this issue exists to make fast.
fn dedup_outbox_dirs_by_object_id(outbox_dirs: &mut Vec<PathBuf>) {
    outbox_dirs.retain(|dir| dir.is_dir());
    let mut seen = std::collections::HashSet::new();
    outbox_dirs.retain_mut(|dir| match crate::path_identity::verify_directory(dir) {
        Ok(identity) => {
            let canonical =
                crate::path_utils::normalize_windows_verbatim_path_buf(&identity.canonical_path);
            let is_new = seen.insert(identity.object_id);
            if is_new {
                *dir = canonical;
            }
            is_new
        }
        // Fail closed for this cycle: no raw alias may survive to a later
        // successful reclamation check while a worker owns the canonical key.
        Err(error) => {
            log::warn!(
                "[mailbox] #1399 skipping unverifiable outbox {:?} for this cycle: {}; scan and \
                reclamation deferred",
                dir,
                error
            );
            false
        }
    });
}

#[cfg(test)]
struct RetryPruneProbeBarrier {
    origin_absent: std::sync::Arc<std::sync::Barrier>,
    restore_complete: std::sync::Arc<std::sync::Barrier>,
}

/// (#1399) Drop retry state only after both durable source spellings were
/// successfully observed absent. An observation error is pending state, not
/// evidence that an already-counted delivery attempt disappeared.
fn prune_retry_tracker(
    retry_tracker: &mut std::collections::HashMap<PathBuf, RetryState>,
    live_claims: &std::collections::HashSet<PathBuf>,
    #[cfg(test)] probe_barrier: Option<&RetryPruneProbeBarrier>,
) {
    retry_tracker.retain(|path, _| match path.try_exists() {
        Ok(true) => true,
        Ok(false) => {
            let claim = wake_claim_path(path);
            #[cfg(test)]
            if let Some(probe_barrier) = probe_barrier {
                probe_barrier.origin_absent.wait();
                probe_barrier.restore_complete.wait();
            }
            if live_claims.contains(&claim) {
                true
            } else {
                match claim.try_exists() {
                    Ok(claim_present) => claim_present,
                    Err(error) => {
                        log::warn!(
                            "[mailbox] #1399 could not inspect retry claim {:?}: {}; retaining \
                            retry state",
                            claim,
                            error
                        );
                        true
                    }
                }
            }
        }
        Err(error) => {
            log::warn!(
                "[mailbox] #1399 could not inspect retry origin {:?}: {}; retaining retry state",
                path,
                error
            );
            true
        }
    });
}

/// (#1399 R1) The worker's outcome report, as a guard. `live_claims` is
/// released only by the drain, and the drain only runs for outcomes that
/// arrive, so the send must survive an unwind: without it, one panicking
/// worker (a poisoned `PtyManager` lock is a reachable production panic on
/// this path) leaves its claim owned forever, invisible to the scan AND
/// excluded from reclamation. Normal completion is byte-identical to a plain
/// `send`; a panicking or dropped worker reports a transient error instead of
/// reporting nothing.
struct WakeOutcomeReport {
    outcomes: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
    origin: Option<PathBuf>,
    outcome: Option<Result<(), String>>,
}

impl WakeOutcomeReport {
    fn new(
        outcomes: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
        origin: PathBuf,
    ) -> Self {
        Self {
            outcomes,
            origin: Some(origin),
            outcome: None,
        }
    }

    fn set(&mut self, outcome: Result<(), String>) {
        self.outcome = Some(outcome);
    }
}

impl Drop for WakeOutcomeReport {
    fn drop(&mut self) {
        let Some(origin) = self.origin.take() else {
            return;
        };
        // The synthesised error is deliberately transient: it matches none of
        // the `is_permanent_delivery_error` markers, so it counts an attempt
        // and retries, and only rejects after the attempt ceiling (10). Do not
        // edit the string casually in either direction.
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("wake worker ended without reporting an outcome".to_string()));
        // Discarded without panic if the receiver died with the poller at
        // shutdown; the claim on disk carries the message forward.
        let _ = self.outcomes.send((origin, outcome));
    }
}

/// (#1399) The detached delivery tail: deliver, archive on success, and on
/// any failure rename the claim back before the outcome is sent, so by the
/// time the scanner drains it the origin path is valid again and the
/// retry/reject block sees a real file.
async fn deliver_claimed_wake<R: tauri::Runtime>(
    delivery: &MailboxPoller,
    app: &tauri::AppHandle<R>,
    claim: &Path,
    origin: &Path,
    validated: &ValidatedWake,
) -> Result<(), String> {
    let _candidate_disposal = validated
        .response
        .as_ref()
        .map(ResponseWakeWork::candidate_disposal);
    match delivery.deliver_validated_wake(app, validated, false).await {
        // `claim.parent()` is the outbox dir, so delivered/ is derived exactly
        // as it is today and the destination still comes from `msg.id`. The
        // delivered JSON carries the receipt annotation (#1635-2 D4).
        Ok(annotated) => match delivery.move_to_delivered(app, claim, &annotated).await {
            Ok(()) => Ok(()),
            Err(error) => {
                release_wake_claim(claim, origin);
                Err(error)
            }
        },
        Err(error) => {
            release_wake_claim(claim, origin);
            Err(error)
        }
    }
}

/// (#1399) Outcome of trying to hand one validated message to a worker.
/// `LaneBusy` leaves the file untouched in the outbox for the next cycle;
/// `ClaimFailed` feeds the existing retry block as an ordinary transient error.
enum WakeHandoff {
    Spawned,
    LaneBusy,
    ClaimFailed(String),
}

/// #617 - sustained-idle window before a deferred provider-resolved logical
/// clear. `pub(crate)` so the CLI prose and response JSON single-source the
/// gate, the response `settle_secs`, and the CLI's conditional wording).
pub(crate) const SELF_CLEAR_SETTLE_SECS: u64 = 30;
// The next three are consumed only by the `#[cfg(not(test))]` spawn in
// `handle_self_clear`; test builds drive `run_self_clear_after_sustained_idle`
// with explicit durations, so they are dead under `cfg(test)`.
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_SETTLE: std::time::Duration =
    std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// Safety cap so a never-idle session cannot leave a task polling forever.
/// Generous: any normal agent hits a 30s-idle window well within it.
#[cfg_attr(test, allow(dead_code))]
const SELF_CLEAR_MAX_DEFER: std::time::Duration = std::time::Duration::from_secs(3600);

/// #626 - the OutboxMessage `action` value for self-handoff-and-clear. Single-sourced so the CLI
/// emit, the early-dispatch match, and the response body cannot drift (a drift would make early
/// dispatch silently not fire and the command would be lost with no agent-visible error).
/// `pub(crate)` so
/// `cli/self_clear.rs` reaches it as `crate::phone::mailbox::SELF_CLEAR_ACTION`.
pub(crate) const SELF_CLEAR_ACTION: &str = "self-handoff-and-clear";

pub(crate) const RAISE_HAND_ACTION: &str = "raise-hand";

/// The canonical handoff filename at the agent root; also the prompt fallback when the
/// pre-inject archive did not happen (#749).
const SELF_HANDOFF_ROOT_NAME: &str = "SELF-HANDOFF.md";

/// Timestamp prefix for `self-clear/<timestamp>_<stem>.md` archives. Part of the naming
/// contract the #749 prompt and the CLI help expose to agents, so single-sourced.
const ARCHIVE_TIMESTAMP_FORMAT: &str = "%Y%m%d_%H%M%S";

/// #626/#749 - shared body of the Phase-2 resume prompt (the clear and switch variants differ
/// only in the opening clause). `handoff_path` is the file the agent must read, relative to its
/// root: the EXACT archived `self-clear/<ts>_SELF-HANDOFF.md` when the pre-inject archive
/// succeeded (#749), or the plain root `SELF-HANDOFF.md` when it did not (source absent or
/// rename failed). Must stay a SINGLE line (an embedded newline would submit early) and
/// self-contained (the agent's context was just wiped). Tests assert it is non-empty,
/// single-line, em-dash-free, and names the file in both the read instruction and the
/// missing-or-empty fallback clause.
fn handoff_base_prompt(event_clause: &str, handoff_path: &str) -> String {
    format!(
        "{event} To resume, read the file {p} relative to your own agent root (your current \
         working directory) and continue the work described there. If {p} is missing or empty, \
         wait for new instructions instead of guessing.",
        event = event_clause,
        p = handoff_path
    )
}

pub(crate) fn self_clear_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your context was just cleared by the self-handoff-and-clear command.",
        handoff_path,
    )
}

/// #668 - the OutboxMessage `action` value for self-handoff-and-switch.
pub(crate) const SELF_SWITCH_ACTION: &str = "self-handoff-and-switch";

/// #1632 - the OutboxMessage `action` value for self-handoff-and-restart. Single-sourced like the
/// other two so the CLI emit, the early-dispatch match, and the response body cannot drift.
pub(crate) const SELF_RESTART_ACTION: &str = "self-handoff-and-restart";

/// (#885) Bulk purge of the caller's own workgroup. Dispatched pre-routing,
/// like the other self-scoped actions: there is no single recipient to route to.
pub(crate) const PURGE_WG_ACTION: &str = "purge-wg";

/// #668/#749 - Phase-2 prompt for the switch variant; see `handoff_base_prompt`.
pub(crate) fn self_switch_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your session was just switched by the self-handoff-and-switch command.",
        handoff_path,
    )
}

/// #1632 - Phase-2 prompt for the restart variant; see `handoff_base_prompt`.
pub(crate) fn self_restart_handoff_base_prompt(handoff_path: &str) -> String {
    handoff_base_prompt(
        "Your session was just restarted by the self-handoff-and-restart command.",
        handoff_path,
    )
}

pub(crate) const SELF_FORGET_SUMMARY_MAX_CHARS: usize = 240;
const SELF_FORGET_SUMMARY_READ_LIMIT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgottenSummary(String);

impl ForgottenSummary {
    fn from_raw(raw: &str) -> Option<Self> {
        let mut parts = Vec::new();

        for line in raw.lines() {
            if let Some(normalized) = normalize_self_forget_line(line) {
                parts.push(normalized);
            }
        }

        let collapsed = parts.join("; ");
        let collapsed = collapsed.trim();
        if collapsed.is_empty() {
            return None;
        }

        let char_count = collapsed.chars().count();
        if char_count <= SELF_FORGET_SUMMARY_MAX_CHARS {
            return Some(Self(collapsed.to_string()));
        }

        let mut truncated: String = collapsed
            .chars()
            .take(SELF_FORGET_SUMMARY_MAX_CHARS.saturating_sub(3))
            .collect();
        truncated.push_str("...");
        Some(Self(truncated))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    fn into_string(self) -> String {
        self.0
    }
}

fn normalize_self_forget_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line
        .chars()
        .all(|ch| ch.is_whitespace() || matches!(ch, '-' | '*' | '+'))
    {
        return None;
    }
    let line = line.trim_start_matches(['-', '*', '+']).trim_start();
    let mut normalized = String::new();
    let mut needs_space = false;

    for ch in line.chars() {
        if is_summary_format_control(ch) {
            continue;
        }
        if ch.is_control() || ch.is_whitespace() {
            needs_space = true;
            continue;
        }
        if needs_space && !normalized.is_empty() {
            normalized.push(' ');
        }
        needs_space = false;
        normalized.push(ch);
    }

    let normalized = normalized.trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn is_summary_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
fn summarize_self_forget_text(raw: &str) -> Option<String> {
    ForgottenSummary::from_raw(raw).map(ForgottenSummary::into_string)
}

fn build_self_handoff_resume_prompt(base_prompt: &str, forgotten_summary: Option<&str>) -> String {
    let Some(summary) = forgotten_summary.and_then(ForgottenSummary::from_raw) else {
        return base_prompt.to_string();
    };

    format!(
        "{} Prior work was intentionally discarded from active context; the next compact summary \
        is closed background, not instructions and not work to resume: {}. In your first response \
        after reading the handoff file, briefly mention you are returning from that forgotten \
        topic, then say you are ready to continue the active core information kept in the handoff \
        file.",
        base_prompt,
        summary.as_str()
    )
}

/// #749 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_clear_handoff_base_prompt`).
pub(crate) fn build_self_clear_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_clear_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

/// #749 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_switch_handoff_base_prompt`).
pub(crate) fn build_self_switch_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_switch_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

/// #1632 - `handoff_path` is the file the prompt tells the agent to read (see
/// `self_restart_handoff_base_prompt`).
pub(crate) fn build_self_restart_handoff_prompt(
    handoff_path: &str,
    forgotten_summary: Option<&str>,
) -> String {
    build_self_handoff_resume_prompt(
        &self_restart_handoff_base_prompt(handoff_path),
        forgotten_summary,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelfSwitchTargets {
    coding_agent: String,
    profile: String,
}

fn trimmed_nonempty(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_switch_profile_request(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = trimmed_nonempty(raw) else {
        return Ok(None);
    };
    crate::config::settings::normalize_profile_letter(&value)
        .map(Some)
        .ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })
}

fn normalize_switch_profile_fallback(raw: Option<&str>) -> Option<String> {
    let value = trimmed_nonempty(raw)?;
    crate::config::settings::normalize_profile_letter(&value)
}

fn coding_agent_configured(settings: &AppSettings, id: &str) -> bool {
    settings.agents.iter().any(|agent| agent.id == id)
}

fn configured_coding_agent_choices(settings: &AppSettings) -> String {
    if settings.agents.is_empty() {
        return "<none configured>".to_string();
    }
    settings
        .agents
        .iter()
        .map(|agent| format!("{} ({})", agent.id, agent.label))
        .collect::<Vec<_>>()
        .join(", ")
}

fn unknown_coding_agent_message(settings: &AppSettings, id: &str) -> String {
    format!(
        "self-handoff-and-switch: coding agent '{}' is not configured. Configured coding agents: \
        {}",
        id,
        configured_coding_agent_choices(settings)
    )
}

/// #1632 - the per-flavor strings the shared respawn driver needs. Switch and restart differ only
/// in their log label, their phase-1 verb, their phase-2 verb, and their resume-prompt builder;
/// everything else, the two-phase gate, the pending-set alias bookkeeping, and the archive-first
/// phase-2 tail, is identical and must not fork.
#[derive(Debug, Clone, Copy)]
struct SelfHandoffFlavor {
    action: &'static str,
    phase1_verb: &'static str,
    phase2_verb: &'static str,
    build_prompt: fn(&str, Option<&str>) -> String,
}

const SELF_HANDOFF_FLAVOR_SWITCH: SelfHandoffFlavor = SelfHandoffFlavor {
    action: SELF_SWITCH_ACTION,
    phase1_verb: "persisting the target selection and respawning",
    phase2_verb: "post-switch",
    build_prompt: build_self_switch_handoff_prompt,
};

const SELF_HANDOFF_FLAVOR_RESTART: SelfHandoffFlavor = SelfHandoffFlavor {
    action: SELF_RESTART_ACTION,
    phase1_verb: "respawning the live recipe",
    phase2_verb: "post-restart",
    build_prompt: build_self_restart_handoff_prompt,
};

/// #1632 - the live recipe a restart must reproduce. `agent` is required: with no coding-agent
/// identity there is nothing to pin, and passing None would hand the choice back to
/// `resolve_restart_selected_agent_id`, which ranks the replica `currentCodingAgent` cell ABOVE
/// the agent the live session was launched with (commands/session.rs:3626-3641). `profile` is
/// optional: when the session inherited its letter, None reproduces exactly the resolution its
/// own spawn used, and no letter is invented.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelfRestartRecipe {
    agent: String,
    profile: Option<String>,
}

fn resolve_self_restart_recipe(
    session_agent: Option<&str>,
    session_effective_profile: Option<&str>,
    session_requested_profile: Option<&str>,
) -> Result<SelfRestartRecipe, String> {
    let agent = trimmed_nonempty(session_agent).ok_or_else(|| {
        "self-handoff-and-restart: this session has no configured coding-agent identity, so there \
         is no recipe to respawn; run it from a coding-agent session."
            .to_string()
    })?;
    let profile = normalize_switch_profile_fallback(session_effective_profile)
        .or_else(|| normalize_switch_profile_fallback(session_requested_profile));
    Ok(SelfRestartRecipe { agent, profile })
}

/// #1632 - the driver seam carries the profile as a `String` (the `Restart` closure's second
/// profile argument), so an absent profile letter must cross it as a sentinel. The empty string is
/// that sentinel: it is not a valid profile (`normalize_switch_profile_fallback` rejects it) so it
/// cannot collide with a real letter. These two are inverses and are pinned as such by T20;
/// keeping them adjacent and named is what stops a future edit from encoding here and decoding
/// somewhere else.
fn encode_restart_profile_for_seam(profile: Option<&str>) -> String {
    profile.unwrap_or("").to_string()
}

fn decode_restart_profile_from_seam(profile: &str) -> Option<String> {
    trimmed_nonempty(Some(profile))
}

/// #1632 - operator-facing rendering of the same seam value. The sentinel must never reach a log
/// line as `profile ''`. Third member of the sentinel's family, so it lives with its inverses
/// rather than as an inline conditional in the shared driver body.
fn display_profile_for_log(profile: &str) -> &str {
    if profile.trim().is_empty() {
        "<inherited>"
    } else {
        profile
    }
}

fn resolve_switch_targets(
    settings: &AppSettings,
    cwd: &Path,
    req_agent: Option<&str>,
    req_profile: Option<&str>,
    session_agent: Option<&str>,
    session_effective_profile: Option<&str>,
    session_requested_profile: Option<&str>,
) -> Result<SelfSwitchTargets, String> {
    let coding_agent = if let Some(requested) = trimmed_nonempty(req_agent) {
        if !coding_agent_configured(settings, &requested) {
            return Err(unknown_coding_agent_message(settings, &requested));
        }
        Some(requested)
    } else if let Some(live) = trimmed_nonempty(session_agent) {
        Some(live)
    } else {
        crate::config::coding_agent_profiles::read_replica_current_coding_agent(cwd)
            .and_then(|value| trimmed_nonempty(Some(value.as_str())))
            .filter(|value| coding_agent_configured(settings, value))
    };
    let coding_agent = coding_agent.ok_or_else(|| {
        format!(
            "self-handoff-and-switch: no target coding agent could be resolved. Pass \
            --coding-agent <id>. Configured coding agents: {}",
            configured_coding_agent_choices(settings)
        )
    })?;

    let profile = normalize_switch_profile_request(req_profile)?
        .or_else(|| normalize_switch_profile_fallback(session_effective_profile))
        .or_else(|| normalize_switch_profile_fallback(session_requested_profile))
        .or_else(|| crate::config::coding_agent_profiles::read_replica_profile(cwd))
        .unwrap_or_else(|| "A".to_string());

    Ok(SelfSwitchTargets {
        coding_agent,
        profile,
    })
}

fn validate_self_switch_wg_replica(settings: &AppSettings, cwd: &Path) -> Result<PathBuf, String> {
    let name = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
            "self-handoff-and-switch is only supported from a Room replica (__agent_* under room-* \
            or legacy wg-*); '{}' has no valid final path component",
            cwd.display()
        )
        })?;
    if !name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a Room replica (__agent_* under room-* \
            or legacy wg-*); got '{}'",
            cwd.display()
        ));
    }
    let validated =
        crate::config::coding_agent_profiles::validate_profile_selection_agent_path(settings, cwd)
            .map_err(|e| {
                format!(
                    "self-handoff-and-switch is only supported from a configured Room replica: {}",
                    e
                )
            })?;
    let validated_name = validated
        .launch_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !validated_name.starts_with("__agent_") {
        return Err(format!(
            "self-handoff-and-switch is only supported from a Room replica; validated target was \
            '{}'",
            validated.launch_path.display()
        ));
    }
    Ok(validated.launch_path)
}

fn validate_self_switch_spawn(
    settings: &AppSettings,
    cwd: &Path,
    targets: &SelfSwitchTargets,
) -> Result<(), String> {
    if !coding_agent_configured(settings, &targets.coding_agent) {
        return Err(unknown_coding_agent_message(
            settings,
            &targets.coding_agent,
        ));
    }
    let profile =
        crate::config::settings::normalize_profile_letter(&targets.profile).ok_or_else(|| {
            "self-handoff-and-switch: profile must be a single letter A through Z".to_string()
        })?;
    crate::config::agent_command::build_agent_spawn_command(
        settings,
        &targets.coding_agent,
        Some(cwd),
        Some(&profile),
    )
    .map(|_| ())
    .map_err(|e| {
        format!(
            "self-handoff-and-switch: target coding agent '{}' profile '{}' is not launchable from \
            '{}': {}",
            targets.coding_agent,
            profile,
            cwd.display(),
            e
        )
    })
}

/// §DR5 anti-spoof accept rule. Outbox-sender check passes when `msg_from`
/// equals `expected_from` exactly, OR when `msg_from` is unqualified (legacy)
/// AND its local part matches `expected_from`'s local part. Qualified-but-
/// wrong-project `msg_from` is always rejected.
pub(crate) fn anti_spoof_accept(msg_from: &str, expected_from: &str) -> bool {
    if msg_from == expected_from {
        return true;
    }
    let (_, exp_local) = crate::config::teams::split_project_prefix(expected_from);
    let (msg_proj, msg_local) = crate::config::teams::split_project_prefix(msg_from);
    msg_proj.is_none() && exp_local == msg_local
}

/// §AR2-norm step (1): upgrade a legacy-unqualified `msg_from` to
/// `expected_from` when expected_from is FQN. Returns true if the upgrade
/// happened. No-op when msg_from is already qualified, expected_from is
/// None, or expected_from itself is unqualified.
pub(crate) fn canonicalize_msg_from_in_place(
    msg_from: &mut String,
    expected_from: Option<&str>,
) -> bool {
    let Some(exp) = expected_from else {
        return false;
    };
    let (exp_proj, _) = crate::config::teams::split_project_prefix(exp);
    if exp_proj.is_none() {
        return false;
    }
    let (msg_proj, _) = crate::config::teams::split_project_prefix(msg_from);
    if msg_proj.is_some() {
        return false;
    }
    *msg_from = exp.to_string();
    true
}

/// Decision made by `deliver_wake` for an existing session.
#[derive(Debug, PartialEq)]
pub(crate) enum WakeAction {
    /// Session is live — inject into stdin, regardless of whether the agent
    /// is waiting for input or mid-turn. Bias toward delivery.
    Inject,
    /// Session is Exited — destroy it and fall through to spawn a fresh one.
    RespawnExited,
}

/// Pure decision given a session's status. Extracted so the decision table is
/// unit-testable without a tauri runtime. `deliver_wake` calls this and acts
/// on the result; any future restoration of a busy-gate would require editing
/// this fn (and its tests below), not a lone `if` inside `deliver_wake`.
pub(crate) fn wake_action_for(status: &SessionStatus) -> WakeAction {
    if matches!(status, SessionStatus::Exited(_)) {
        WakeAction::RespawnExited
    } else {
        WakeAction::Inject
    }
}

/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback. Inverts
/// the positive-form `spawn_with_resume` flag so call sites read naturally.
///
/// Pinned via this helper to fence against a future refactor that "simplifies"
/// `!spawn_with_resume` to `spawn_with_resume` and silently regresses #82.
/// See plan §8.2 / round-2 R2.7 / round-3 R3.2.
pub(crate) fn wake_spawn_skip_auto_resume(spawn_with_resume: bool) -> bool {
    !spawn_with_resume
}

/// #2411 Internal-system wake resume rule: a surviving exited record resumes
/// (unchanged); with no record, resume only if idle auto-close closed it.
pub(crate) fn internal_wake_resumes(had_exited_record: bool, auto_closed: bool) -> bool {
    had_exited_record || auto_closed
}

/// Decide whether a session is a viable candidate for the mailbox to attempt
/// delivery to. Pure function — unit-testable without a tauri runtime.
///
/// Rules:
/// - `Exited(_)` records are KEPT even with `has_pty == false` — the wake
///   contract documents the respawn path: "if Exited, respawn". The respawn
///   path does NOT need a live PTY (it calls `destroy_session_inner` then
///   `create_session_inner`).
/// - All other statuses (`Active`/`Running`/`Idle`) require `has_pty == true`.
///   A SessionManager record with one of these statuses but no PtyManager
///   entry is a desync phantom — the inject path is guaranteed to fail with
///   `AppError::SessionNotFound`, so the router must skip it (issue #223).
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision rather than silently routing
/// to the `has_pty` branch. (dev-rust R1.B7 / grinch G.M6.)
pub(crate) fn is_viable_wake_candidate(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => true,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// Decide whether a `SessionInfo` candidate is a viable Root Agent recipient
/// for delivery. Stricter than `is_viable_wake_candidate`: rejects
/// `SessionStatus::Exited(_)` outright.
///
/// The Root Agent is user-launched and never auto-respawned (#293 §3 contract).
/// `deliver_wake`'s deferred-destroy block fires for any candidate that hit
/// the `RespawnExited` arm, which would silently destroy the user's Root
/// Agent session record before the no-spawn guard fires. Returning `false`
/// for Exited keeps the record in place so the user sees it in the sidebar
/// exactly as they left it.
///
/// Exhaustive `match` (not `matches!`) — a future `SessionStatus` variant
/// forces a deliberate compile-error decision. Same pattern as
/// `is_viable_wake_candidate`.
pub(crate) fn is_viable_root_recipient(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => false,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
    }
}

/// One tick of the #611 sustained-idle gate for a freshly spawned wake session.
///
/// A cold-starting agent (notably Claude) can hit a quiet window longer than the
/// idle threshold mid-startup and be marked `waiting_for_input` before its TUI
/// input/paste state is stable. Injecting then leaves the body in the input box
/// but the submit `\r` can be dropped, so the message is written yet never sent.
/// `wait_for_spawned_wake_idle` therefore requires idle to hold continuously for
/// a settle window before injecting; the existing double-Enter in `pty/inject.rs`
/// then submits reliably once the agent is stable.
///
/// Pure so the settle/reset policy is unit-testable without timers. Threads
/// `idle_since` (the instant the session was first seen continuously idle, or
/// `None` if it is currently busy / has not yet been observed idle) and returns
/// the next `idle_since` plus whether idle has now held for `settle`:
/// - not ready (`waiting_for_input == false`) resets the clock to `None`: a late
///   startup render restarts the settle window. #1388: the cold-spawn caller now
///   passes the composed `wake_settle_ready`, so `false` also means "idle with a
///   blank screen", and the reset makes the settle window measure idle-AND-rendered
///   held continuously.
/// - idle starts (or keeps) the clock; once `now - idle_since >= settle` the
///   caller may inject.
pub(crate) fn next_sustained_idle_state(
    waiting_for_input: bool,
    idle_since: Option<std::time::Instant>,
    now: std::time::Instant,
    settle: std::time::Duration,
) -> (Option<std::time::Instant>, bool) {
    if !waiting_for_input {
        return (None, false);
    }
    let since = idle_since.unwrap_or(now);
    // checked_duration_since guards against a non-monotonic `now` (parity with
    // idle_detector's threshold check); treat an unexpected backwards clock as
    // "not yet settled" rather than panicking.
    let settled = now
        .checked_duration_since(since)
        .map(|elapsed| elapsed >= settle)
        .unwrap_or(false);
    (Some(since), settled)
}

/// (#1001 PR2 / B) Extra settle beyond `idle_threshold` for the live-inject gate.
/// A session only reports `waiting_for_input == true` after `idle_threshold`
/// (~2500ms) of quiet, so a settle <= `idle_threshold` never bites (grinch G3).
/// The guard makes B gate exactly the `[idle_threshold, idle_threshold +
/// FRESH_IDLE_GUARD]` window where a just-idle TUI may not be paste-ready yet.
pub(crate) const FRESH_IDLE_GUARD: std::time::Duration = std::time::Duration::from_millis(1000);

/// (#1001 PR2 / B, grinch P1) Pure per-tick decision for the LIVE-inject settle,
/// sourced ENTIRELY from the real-time `activity_age` snapshot - never the lagged
/// `SessionManager.waiting_for_input`, which the watcher flips idle up to ~500ms
/// after `activity_age` crosses `idle_threshold` (watcher granularity + on_idle +
/// spawned `mark_idle`). Reading `waiting_for_input` in the loop re-opened the
/// fresh-idle drop during `activity_age ∈ [idle_threshold, T_flip]` (grinch P1);
/// `activity_age` is stamped synchronously (`idle_detector.rs`), so it has no lag
/// in either direction.
///
/// - `elapsed >= max_wait`: InjectNow (cap first - never drop a delivery).
/// - recent resize (`last_resize_age < resize_grace`): `activity_age` is FROZEN
///   and untrustworthy and the TUI is repainting - Wait (never inject a repaint;
///   grinch G8). See `PurgeReadiness.last_resize_age` doc.
/// - `activity_age` unknown (untracked / just destroyed): InjectNow best-effort;
///   the inject then surfaces the real state.
/// - `activity_age < idle_threshold`: busy / mid-turn - InjectNow (bias-to-deliver).
/// - `activity_age >= settle` (`idle_threshold + FRESH_IDLE_GUARD`): long-idle -
///   InjectNow (no added latency for a genuinely-ready wake).
/// - otherwise (the `[idle_threshold, settle)` fresh-idle window): Wait.
pub(crate) fn live_settle_action(
    activity_age: Option<std::time::Duration>,
    last_resize_age: Option<std::time::Duration>,
    resize_grace: std::time::Duration,
    idle_threshold: std::time::Duration,
    settle: std::time::Duration,
    elapsed: std::time::Duration,
    max_wait: std::time::Duration,
) -> SettleAction {
    if elapsed >= max_wait {
        return SettleAction::InjectNow;
    }
    if let Some(resize_age) = last_resize_age {
        if resize_age < resize_grace {
            return SettleAction::Wait;
        }
    }
    match activity_age {
        None => SettleAction::InjectNow,
        Some(age) if age < idle_threshold => SettleAction::InjectNow,
        Some(age) if age >= settle => SettleAction::InjectNow,
        Some(_) => SettleAction::Wait,
    }
}

/// (#1001 PR2 / B) The action `settle_until_ready` takes on one tick.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SettleAction {
    /// Inject now: busy fast-path, sustained idle reached, or the max_wait cap
    /// (never drop a delivery).
    InjectNow,
    /// Keep waiting; sleep one poll and re-check.
    Wait,
}

/// (#1001 PR2 P2 / B, option-a) How long after PTY spawn a live wake candidate is
/// still treated as "starting". Below this, a session emitting output cannot be
/// told apart from a mid-turn one by `activity_age` alone (both are
/// `< idle_threshold`), so the busy fast-path would inject into a not-paste-ready
/// startup TUI and drop (still-starting 83%, P2); such a candidate is routed to
/// the sustained-idle settle instead. At or above it, the session is established
/// and the `activity_age` busy fast-path is trusted (bias-to-deliver).
///
/// Derived from evidence, NOT guessed: the `startup_probe` harness mode measured a
/// cold Claude's `alive_age` at first sustained paste-ready on Windows/ConPTY at
/// max 13.7s first-ready / 15.7s held-ready (n=5). 20s clears the observed max
/// held-ready with ~4s margin for boot variance. The safe direction is LARGER:
/// over-estimating only adds bounded, capped latency for a session younger than
/// this that is genuinely mid-turn (rare - a just-booted agent has no work yet),
/// while under-estimating re-opens the still-starting drop. See the `startup_probe`
/// mode in tests/wake_consumption_measure.rs.
pub(crate) const STARTUP_SETTLE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(20);

/// (#1001 PR2 P2 / B, option-a) Route a live wake by session age: a candidate whose
/// PTY was registered less than `startup_threshold` ago is `Starting` (route to the
/// sustained-idle settle - waits out startup churn, the #611 gate PR1 measured at
/// 0% drop); an older one is `Established` (keep the `activity_age` busy fast-path).
/// Pure so the classification is unit-testable without timers.
///
/// `alive_age == None` (untracked / just destroyed) -> `Established`: best-effort,
/// consistent with `live_settle_action` mapping an unknown `activity_age` to
/// InjectNow. The boundary is exclusive: `alive_age == startup_threshold` is
/// `Established`.
pub(crate) fn live_wake_route(
    alive_age: Option<std::time::Duration>,
    startup_threshold: std::time::Duration,
) -> LiveWakeRoute {
    match alive_age {
        Some(age) if age < startup_threshold => LiveWakeRoute::Starting,
        _ => LiveWakeRoute::Established,
    }
}

/// (#1001 PR2 P2 / B, option-a) Which live-wake settle path a candidate takes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LiveWakeRoute {
    /// Recently spawned; may still be emitting startup output. Route to the
    /// sustained-idle settle so the busy fast-path can't inject into a
    /// not-paste-ready TUI.
    Starting,
    /// Established session; a busy `activity_age` is genuinely mid-turn, so keep the
    /// busy fast-path (bias-to-deliver).
    Established,
}

/// #1388 - the readiness input for the cold-spawn wake settle.
///
/// Idle ALONE is satisfied by a child that has painted nothing: `IdleDetector`
/// seeds activity at spawn (`idle_detector.rs:128-145`) and the watcher flips a
/// silent session idle after `idle_threshold`, so a stalled start satisfies an
/// idle-only gate FASTER than a healthy one. Requiring a rendered cell as well
/// closes that. Both conditions are re-evaluated every tick and fed to
/// `next_sustained_idle_state`, which resets the clock when this returns false,
/// so the settle window measures idle-AND-rendered held continuously - not two
/// conditions that were each true at some point.
pub(crate) fn wake_settle_ready(
    waiting_for_input: bool,
    has_rendered_visible_content: bool,
) -> bool {
    waiting_for_input && has_rendered_visible_content
}

/// (#1001 PR2 / B) Pure per-tick decision for the COLD-SPAWN settle loop, so the
/// sustained-idle + inject-anyway-cap policy is unit-testable without timers. The
/// cold-spawn path gates on the composed `wake_settle_ready` (#1388: idle AND
/// rendered, not `SessionManager.waiting_for_input` alone - a freshly spawned
/// session has no meaningful `activity_age` during startup churn); the live path
/// uses `live_settle_action` on real-time `activity_age` instead (grinch P1).
pub(crate) fn settle_tick(
    ready: bool,
    idle_since: Option<std::time::Instant>,
    now: std::time::Instant,
    settle: std::time::Duration,
    elapsed: std::time::Duration,
    max_wait: std::time::Duration,
) -> (Option<std::time::Instant>, SettleAction) {
    let (next_idle_since, settled) = next_sustained_idle_state(ready, idle_since, now, settle);
    if settled {
        return (next_idle_since, SettleAction::InjectNow);
    }
    // Cap: never drop a delivery - inject anyway once max_wait elapses.
    if elapsed >= max_wait {
        return (next_idle_since, SettleAction::InjectNow);
    }
    (next_idle_since, SettleAction::Wait)
}

/// #626 - which leg of the self-handoff-and-clear gate we are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfClearPhase {
    /// Waiting for sustained idle to inject provider-resolved logical-clear text.
    Clear,
    /// Logical clear already injected; waiting for a fresh sustained idle
    /// window after clear before injecting the
    /// stand-alone handoff prompt.
    Handoff,
}

/// #626 - gate state threaded by the driver across polls. Pure-function in/out, so the whole
/// two-stage policy (settle, reset, phase transition, per-phase cap, destroyed) is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SelfClearGateState {
    pub phase: SelfClearPhase,
    /// Instant the session was first seen continuously idle in the CURRENT phase, or None.
    pub idle_since: Option<std::time::Instant>,
    /// Start of the CURRENT phase, for the per-phase MAX_DEFER cap.
    pub phase_started: std::time::Instant,
}

impl SelfClearGateState {
    /// Initial state: Phase 1 (Clear), no idle observed yet, phase clock starts at `now`.
    pub(crate) fn new(now: std::time::Instant) -> Self {
        Self {
            phase: SelfClearPhase::Clear,
            idle_since: None,
            phase_started: now,
        }
    }
}

/// #626 - the action the driver must take after one decider step.
#[derive(Debug, PartialEq)]
pub(crate) enum SelfClearGateAction {
    /// Keep polling; the driver adopts the returned state and carries it forward.
    Wait,
    /// Phase 1 settle: inject provider-resolved logical-clear text. The returned state is already
    /// advanced to Phase 2 with the idle clock reset, so the driver just injects and keeps looping.
    InjectClear,
    /// Phase 2 settle: inject the handoff prompt, then stop.
    InjectHandoff,
    /// Stop without injecting (session gone or per-phase cap reached); &str is the log reason.
    Abandon(&'static str),
}

/// (#756) Fresh-boundary events surfaced by the self-clear / self-switch
/// drivers through an injected callback, so the generic (test-injectable)
/// drivers stay free of app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelfClearBoundary {
    /// Phase-1 logical clear (/clear or Pi /new) reached the PTY: stamp the durable fresh intent
    /// (C2).
    Cleared,
    /// Phase-2 handoff prompt reached the PTY: post-boundary content, drop it.
    ContentInjected,
}

/// #626 - pure two-stage gate step. Returns (next_state, action). `session_present == false` means
/// the session vanished between polls. The Clear->Handoff transition RESETS `idle_since` to None
/// and
/// `phase_started` to `now`, so the Phase 2 window can NOT be satisfied by pre-clear idle. Same
/// "pure decision, thin timer loop" pattern as #617's `self_clear_gate_step` (replaced here),
/// reusing the #611 `next_sustained_idle_state` settle/reset core. Unit-testable WITHOUT timers,
/// locks, or PTY.
pub(crate) fn self_clear_gate_advance(
    state: SelfClearGateState,
    session_present: bool,
    waiting_for_input: bool,
    now: std::time::Instant,
    settle: std::time::Duration,
    max_defer: std::time::Duration,
) -> (SelfClearGateState, SelfClearGateAction) {
    if !session_present {
        return (
            state,
            SelfClearGateAction::Abandon("session destroyed before sustained idle"),
        );
    }
    // Per-phase cap so each leg independently bounds a never-idle session.
    let phase_elapsed = now
        .checked_duration_since(state.phase_started)
        .unwrap_or_default();
    if phase_elapsed >= max_defer {
        let reason = match state.phase {
            SelfClearPhase::Clear => {
                "never reached sustained idle within MAX_DEFER cap (clear leg)"
            }
            SelfClearPhase::Handoff => {
                "never reached sustained idle within MAX_DEFER cap (handoff leg)"
            }
        };
        return (state, SelfClearGateAction::Abandon(reason));
    }
    let (next_idle_since, settled) =
        next_sustained_idle_state(waiting_for_input, state.idle_since, now, settle);
    if settled {
        match state.phase {
            SelfClearPhase::Clear => {
                // Advance to Handoff and RESET both clocks: pre-clear idle does not count.
                let next = SelfClearGateState {
                    phase: SelfClearPhase::Handoff,
                    idle_since: None,
                    phase_started: now,
                };
                (next, SelfClearGateAction::InjectClear)
            }
            SelfClearPhase::Handoff => (state, SelfClearGateAction::InjectHandoff),
        }
    } else {
        (
            SelfClearGateState {
                idle_since: next_idle_since,
                ..state
            },
            SelfClearGateAction::Wait,
        )
    }
}

/// #626/#629/#636 - archive `<root>/<stem>.md` to `<root>/self-clear/<timestamp>_<stem>.md`. No-op
/// (`Ok(None)`) if `<stem>.md` is absent. Creates the `self-clear/` subdir on demand. `timestamp`
/// is supplied by the caller so this is deterministic in tests. Returns the archived path on
/// success.
/// `std::fs::rename` is atomic within the same filesystem (the agent root and its `self-clear/`
/// child share it). On Windows a source held open without FILE_SHARE_DELETE yields
/// ERROR_SHARING_VIOLATION (os error 32); the caller treats any `Err` as a non-fatal warn (no
/// clobber, the source stays). NO retries (a retry loop would block the caller). Consumers:
/// SELF-FORGET.md at queue time (#626) and SELF-HANDOFF.md immediately before the Phase-2 prompt
/// inject (#749, via `archive_handoff_for_inject`).
fn archive_root_md(
    root: &std::path::Path,
    stem: &str,
    timestamp: &str,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let src = root.join(format!("{}.md", stem));
    if !src.is_file() {
        return Ok(None); // no-op when absent
    }
    let archive_dir = root.join("self-clear");
    let dst = archive_dir.join(format!("{}_{}.md", timestamp, stem));
    if dst.exists() {
        // Same-second collision (effectively impossible: archiving happens once per >=60s cycle).
        // Refuse to clobber an existing archive; leave the source in place.
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "archive target already exists",
        ));
    }
    std::fs::create_dir_all(&archive_dir)?;
    std::fs::rename(&src, &dst)?;
    Ok(Some(dst))
}

/// #749 - archive `<root>/SELF-HANDOFF.md` -> `self-clear/<ts>_SELF-HANDOFF.md` IMMEDIATELY before
/// the Phase-2 handoff prompt is injected, so the prompt can name the file's real, final location.
/// Replaces #629's post-inject 180s grace timer, which raced the agent's read: inject `Ok` only
/// proves bytes reached the PTY, not that the prompt was consumed, so a slow, queued, or
/// unsubmitted prompt could outlive the grace window and lose its file mid-read.
///
/// Returns `(prompt_path, archived)`: `prompt_path` is what the injected prompt must tell the
/// agent to read, relative to its root (derived from the REAL destination, never re-encoded, so
/// it cannot drift from `archive_root_md`'s naming); `archived` is `Some(dst)` only when the
/// rename actually happened, so the caller can rename it BACK if the inject then fails (a
/// re-issue needs the canonical root name, see `restore_handoff_after_failed_inject`). Fallbacks
/// keep the pre-#749 behavior: a missing source (anomalous here, the queue-time gate proved it
/// existed; warn and let the prompt's "missing or empty" clause cover it) or a rename failure
/// (e.g. Windows sharing violation, os error 32; the file is still at the root) both point the
/// prompt at the root name. `timestamp` is supplied by the caller so this is deterministic in
/// tests (mirrors `archive_root_md`); `action` labels the log lines with the owning flow.
fn archive_handoff_for_inject(
    root: &std::path::Path,
    timestamp: &str,
    action: &str,
) -> (String, Option<std::path::PathBuf>) {
    match archive_root_md(root, "SELF-HANDOFF", timestamp) {
        Ok(Some(dst)) => {
            log::info!(
                "[mailbox] {}: archived SELF-HANDOFF.md -> {} (pre-inject)",
                action,
                dst.display()
            );
            let prompt_path = dst.strip_prefix(root).map_or_else(
                |_| format!("self-clear/{}_SELF-HANDOFF.md", timestamp),
                |rel| {
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                },
            );
            (prompt_path, Some(dst))
        }
        Ok(None) => {
            log::warn!(
                "[mailbox] {}: SELF-HANDOFF.md vanished from {} between the queue-time gate and \
                phase 2; the prompt will point at the root file",
                action,
                root.display()
            );
            (SELF_HANDOFF_ROOT_NAME.to_string(), None)
        }
        Err(e) => {
            log::warn!(
                "[mailbox] {}: pre-inject SELF-HANDOFF.md archive failed for {} (non-fatal; the \
                prompt will point at the root file): {}",
                action,
                root.display(),
                e
            );
            (SELF_HANDOFF_ROOT_NAME.to_string(), None)
        }
    }
}

/// #749 - best-effort undo of `archive_handoff_for_inject` after a FAILED prompt inject: the
/// prompt never reached the agent, so its notes must return to the canonical root name for a
/// re-issue (the pre-#749 code expressed the same semantics by not archiving on inject failure).
/// Refuses to clobber: if a NEW entry named `SELF-HANDOFF.md` appeared at the root in the
/// interim, the archived copy stays where it is (still recoverable under `self-clear/`). The
/// exists-then-rename pair is not atomic (Windows rename replaces an existing file), but the
/// window is milliseconds on a session that just failed an inject; accepted as best-effort.
fn restore_handoff_after_failed_inject(
    root: &std::path::Path,
    archived: &std::path::Path,
    action: &str,
) {
    let src = root.join(SELF_HANDOFF_ROOT_NAME);
    if src.exists() {
        log::warn!(
            "[mailbox] {}: not restoring {} after failed inject; a new SELF-HANDOFF.md already \
            exists at the root",
            action,
            archived.display()
        );
        return;
    }
    match std::fs::rename(archived, &src) {
        Ok(()) => log::info!(
            "[mailbox] {}: restored {} -> {} after failed inject",
            action,
            archived.display(),
            src.display()
        ),
        Err(e) => log::warn!(
            "[mailbox] {}: failed to restore {} to the root after failed inject (non-fatal; \
            re-issue needs the file recreated or recovered from self-clear/): {}",
            action,
            archived.display(),
            e
        ),
    }
}

/// #749 - shared Phase-2 tail for both self-handoff drivers: archive the handoff FIRST, inject a
/// prompt naming the exact archived path, and rename the file back if the inject fails. Archiving
/// before the prompt exists makes the promised location final, so a late read still succeeds (the
/// old post-inject 180s timer raced slow, queued, or unsubmitted prompts); it also keeps a
/// consumed handoff from false-triggering the NEXT cycle's existence gate (#629's concern).
/// `build_prompt` is the flow's prompt builder (`build_self_clear_handoff_prompt` /
/// `build_self_switch_handoff_prompt`); `action` labels the log lines.
async fn inject_handoff_prompt_with_archive<Inject, InjectFut>(
    root: &std::path::Path,
    session_id: Uuid,
    action: &'static str,
    build_prompt: fn(&str, Option<&str>) -> String,
    forgotten_summary: Option<&ForgottenSummary>,
    inject: &mut Inject,
) -> bool
where
    Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
    InjectFut: std::future::Future<Output = Result<(), String>> + Send,
{
    let ts = chrono::Local::now()
        .format(ARCHIVE_TIMESTAMP_FORMAT)
        .to_string();
    let (handoff_path, archived) = archive_handoff_for_inject(root, &ts, action);
    let prompt = build_prompt(
        &handoff_path,
        forgotten_summary.map(ForgottenSummary::as_str),
    );
    // (#756) Returns whether the prompt reached the PTY, so the drivers fire
    // their post-boundary-content event only on a real injection.
    if let Err(e) = inject(session_id, prompt).await {
        log::warn!(
            "[mailbox] {}: handoff prompt injection failed for session {}: {}",
            action,
            session_id,
            e
        );
        if let Some(archived) = archived.as_deref() {
            restore_handoff_after_failed_inject(root, archived, action);
        }
        return false;
    }
    true
}

fn capture_self_forget_summary(root: &std::path::Path) -> Option<ForgottenSummary> {
    let path = root.join("SELF-FORGET.md");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: failed to open SELF-FORGET.md summary from {} \
                (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let mut bytes = Vec::new();
    let mut bounded = file.take(SELF_FORGET_SUMMARY_READ_LIMIT_BYTES);
    if let Err(e) = bounded.read_to_end(&mut bytes) {
        log::warn!(
            "[mailbox] self-handoff: failed to read SELF-FORGET.md summary from {} (non-fatal): {}",
            path.display(),
            e
        );
        return None;
    }

    let raw = match std::str::from_utf8(&bytes) {
        Ok(raw) => raw.to_string(),
        Err(e) if e.error_len().is_none() => {
            let valid_up_to = e.valid_up_to();
            std::str::from_utf8(&bytes[..valid_up_to])
                .unwrap_or("")
                .to_string()
        }
        Err(e) => {
            log::warn!(
                "[mailbox] self-handoff: SELF-FORGET.md summary from {} is not valid UTF-8 \
                (non-fatal): {}",
                path.display(),
                e
            );
            return None;
        }
    };

    ForgottenSummary::from_raw(&raw)
}

/// Substring sniff for the `AppError::SessionNotFound` formatted message that
/// bubbles up through `pty::inject::inject_text_into_session` as a `String`.
///
/// Tight coupling: the error string format is `"Session not found: {uuid}"`
/// (see `errors.rs:5`). `inject_text_into_session` wraps it as `"PTY write
/// failed: Session not found: {uuid}"`. The substring `"Session not found:"`
/// covers both wrappings. Pinned by the
/// `err_is_pty_session_missing_matches_actual_apperror_format` test below.
///
/// If a future PR introduces typed-error plumbing through `inject_into_pty`,
/// replace this sniff with a typed match.
pub(crate) fn err_is_pty_session_missing(e: &str) -> bool {
    e.contains("Session not found:")
}

/// #1883 - pure severity classifier for a PTY injection failure. A menu-guard or
/// typing-hold deferral is a recoverable "not yet" (the dispatcher releases the
/// lease and the poller preserves the attempt and retries), so both are logged
/// at debug and never reach the Application Error sink; every other error keeps
/// the existing ERROR severity. Returns the log level plus the outcome word for
/// the line; the caller propagates the error string unchanged.
fn classify_injection_error(error: &str) -> (log::Level, &'static str) {
    if crate::pty::menu_guard::is_menu_guard_deferred_error(error)
        || crate::pty::menu_guard::is_typing_hold_deferred_error(error)
        || crate::api::message_store::is_response_closing_deferred(error)
    {
        (log::Level::Debug, "deferred")
    } else {
        (log::Level::Error, "FAILED")
    }
}

pub(crate) fn is_deferred_internal_delivery_error(error: &str) -> bool {
    crate::pty::menu_guard::is_menu_guard_deferred_error(error)
        || crate::pty::menu_guard::is_typing_hold_deferred_error(error)
        || crate::api::message_store::is_response_closing_deferred(error)
}

/// §224 D.3 — pure filter: session infos by exact-FQN match on
/// `working_directory`. Extracted from `find_all_sessions` so the predicate
/// can be unit-tested without a live `SessionManager` / `AppHandle`.
/// Defensive regression guard: no future change can accidentally re-introduce
/// a `was_active` / `waiting_for_input` / `status` gate without breaking
/// these tests.
pub(crate) fn filter_sessions_by_fqn<'a>(
    sessions: &'a [crate::session::session::SessionInfo],
    target: &str,
) -> Vec<&'a crate::session::session::SessionInfo> {
    sessions
        .iter()
        .filter(|s| session_cwd_matches_fqn(&s.working_directory, target))
        .collect()
}

fn session_cwd_matches_fqn(cwd: &str, target: &str) -> bool {
    crate::config::teams::agent_fqn_from_path(cwd) == target
}

/// #2113: case-tolerant pre-filter for internal system notices.
///
/// The target FQN is derived from the CANONICAL replica dir
/// (`InternalSystemTarget::for_context_alert`), so it carries the on-disk
/// spelling. A session CWD carries the spelling in `cfg.project_paths`, stored
/// verbatim by `SessionManager::create_session`. On a case-insensitive
/// filesystem the two can differ by case and still name the same directory,
/// which dropped the notice (#2113).
///
/// Covers a case difference in the project segment, in the agent segment, and
/// in the room segment AFTER its `room-`/`wg-` prefix. It does NOT cover a
/// case difference in the prefix letters themselves: `has_entity_prefix` is
/// case-sensitive, so `Room-1-Team` never reaches this comparison in
/// project-qualified form. That gap is out of scope; see the plan.
///
/// Deliberately over-wide: `to_lowercase` is Unicode simple lowercasing, which
/// matches neither the NTFS uppercase table nor Linux's byte comparison. That is
/// safe HERE and only here, because this function is a pre-filter, never the
/// authority. Every candidate it admits is re-checked by
/// `canonical_cwd_owned_by_replica`, which canonicalizes the CWD and requires
/// real filesystem identity with the replica dir. A false positive from this
/// function is discarded there; a false negative loses the notice.
///
/// Cross-OS risk, stated: on Linux `.../__agent_bob` and `.../__agent_Bob` are
/// two distinct real directories, and this function matches both. It does not
/// deliver to both: the canonical gate admits exactly the one whose canonical
/// path equals the target replica dir. See the `linux` test below.
fn internal_target_fqn_prefilter_matches(cwd: &str, target_fqn: &str) -> bool {
    let candidate = crate::config::teams::agent_fqn_from_path(cwd);
    candidate == target_fqn || candidate.to_lowercase() == target_fqn.to_lowercase()
}

fn resolve_wg_path_from_session_dirs(dirs: &[(Uuid, String)], agent_name: &str) -> Option<String> {
    let (target_project, local) = crate::config::teams::split_project_prefix(agent_name);
    let (wg_name, agent_short) = local.split_once('/')?;
    if !crate::config::entity_prefix::has_entity_prefix(wg_name) {
        return None;
    }

    let wg_marker = format!("/{}/", wg_name);
    for (_, cwd) in dirs {
        let normalized = cwd.replace('\\', "/");
        if let Some(wg_pos) = normalized.rfind(&wg_marker) {
            let wg_dir = &normalized[..wg_pos + 1 + wg_name.len()];
            let candidate = format!("{}/__agent_{}", wg_dir, agent_short);
            if !std::path::Path::new(&candidate).is_dir() {
                continue;
            }

            let candidate_fqn = crate::config::teams::agent_fqn_from_path(&candidate);
            if let Some(want) = target_project {
                let (cand_project, _) = crate::config::teams::split_project_prefix(&candidate_fqn);
                if cand_project != Some(want) {
                    continue;
                }
            }

            log::debug!(
                "[mailbox] wake: resolved WG agent path from sibling session: {}",
                candidate
            );
            return Some(
                std::path::PathBuf::from(&candidate)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    None
}

/// §224 A.2.5 — outcome of the daemon-restart race guard wait loop.
///
/// The production caller (`handle_close_session`) inlines the wait loop
/// because the natural probe closure captures `&self`, `app`, and `target`
/// across an `.await`, which is awkward to pass through this helper's
/// `FnMut() -> Future` shape without boxing. The helper is retained as the
/// canonical executable specification of the wait semantics for D.5a unit
/// tests — any future divergence between the inline loop and this helper is
/// a regression.
///
/// §224 G-IMPL-5 (NIT, accepted) — LOAD-BEARING COMMENT. The inlined wait
/// loop in `handle_close_session` has no direct unit test on Windows
/// (D.5b is `#[ignore]`'d pending the cross-process FS enumeration
/// investigation). Equivalence between the two implementations is
/// enforced ONLY by this comment + the helper's D.5a unit tests. Do NOT
/// change the inlined loop's semantics without updating this helper to
/// match, and vice versa.
#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub(crate) enum RestoreWaitOutcome {
    /// Deadline elapsed with the restore flag still set; caller should report
    /// `status="restore_in_progress"` to the user.
    StillInProgress,
    /// Flag cleared with the probe still returning empty — there is no live
    /// session for this FQN. Caller falls through to the `no_match` path.
    NoMatch,
}

/// §224 A.2.5 — poll `probe` for a non-empty result, OR for `flag` to clear,
/// until `deadline`. Pure async helper extracted so the race-guard logic can
/// be unit-tested without a live `SessionManager` / `AppHandle`.
///
/// Returns:
///   * `Ok(non_empty_session_ids)` — a session appeared during the wait.
///   * `Err(StillInProgress)` — deadline elapsed, flag still set.
///   * `Err(NoMatch)` — flag cleared with empty result throughout (final
///     probe also empty).
#[allow(dead_code)]
pub(crate) async fn wait_for_restore_or_session<F, Fut>(
    flag: &std::sync::atomic::AtomicBool,
    mut probe: F,
    deadline: std::time::Instant,
    poll: std::time::Duration,
) -> Result<Vec<Uuid>, RestoreWaitOutcome>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Vec<Uuid>>,
{
    use std::sync::atomic::Ordering;
    loop {
        if std::time::Instant::now() >= deadline {
            return if flag.load(Ordering::SeqCst) {
                Err(RestoreWaitOutcome::StillInProgress)
            } else {
                Err(RestoreWaitOutcome::NoMatch)
            };
        }
        tokio::time::sleep(poll).await;
        let ids = probe().await;
        if !ids.is_empty() {
            return Ok(ids);
        }
        if !flag.load(Ordering::SeqCst) {
            // Flag cleared. One final probe to catch a last-tick insertion
            // by the restore task before its guard dropped.
            let ids = probe().await;
            return if ids.is_empty() {
                Err(RestoreWaitOutcome::NoMatch)
            } else {
                Ok(ids)
            };
        }
    }
}

enum OutboxClassification {
    Standard(String),
    PrivilegedCandidate {
        bytes: Vec<u8>,
        identity: crate::path_identity::VerifiedPathIdentity,
    },
    InvalidDocument,
}

fn contains_privileged_token(bytes: &[u8]) -> bool {
    [
        b"ptyInput".as_slice(),
        b"pty-input".as_slice(),
        b"pty-input-marker".as_slice(),
    ]
    .iter()
    .any(|needle| bytes.windows(needle.len()).any(|window| window == *needle))
}

fn decode_loose_json_ascii_escapes(bytes: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'\\')
            && matches!(bytes.get(index + 1), Some(b'u' | b'U'))
            && index + 6 <= bytes.len()
        {
            let mut value = 0_u32;
            let mut valid = true;
            for byte in &bytes[index + 2..index + 6] {
                let Some(digit) = (*byte as char).to_digit(16) else {
                    valid = false;
                    break;
                };
                value = (value << 4) | digit;
            }
            if valid && value <= 0x7f {
                decoded.push(value as u8);
                index += 6;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    decoded
}

fn raw_privileged_probe(bytes: &[u8]) -> bool {
    if contains_privileged_token(bytes)
        || contains_privileged_token(&decode_loose_json_ascii_escapes(bytes))
    {
        return true;
    }

    for little_endian in [true, false] {
        let mut ascii = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.as_chunks::<2>().0 {
            let unit = if little_endian {
                u16::from_le_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], chunk[1]])
            };
            ascii.push(u8::try_from(unit).unwrap_or(b' '));
        }
        if contains_privileged_token(&ascii)
            || contains_privileged_token(&decode_loose_json_ascii_escapes(&ascii))
        {
            return true;
        }
    }
    false
}

fn reason_code_name(code: crate::phone::types::PtyInputReasonCode) -> &'static str {
    crate::phone::types::pty_input_reason_code_name(code)
}

fn reason_code_from_name(name: &str) -> crate::phone::types::PtyInputReasonCode {
    match serde_json::from_value(serde_json::Value::String(name.to_string())) {
        Ok(code) => code,
        Err(_) => crate::phone::types::PtyInputReasonCode::SenderIdentityInvalid,
    }
}

struct CorrelatedHostPtyMetadata {
    route: crate::config::teams::VerifiedPtyInputRoute,
    confirmation_tag: String,
    request_fingerprint: String,
    payload_sha256: String,
}

fn is_persistent_exited_pty_candidate(session: &SessionInfo) -> bool {
    matches!(session.status, SessionStatus::Exited(_))
        && !session
            .name
            .starts_with(crate::session::session::TEMP_SESSION_PREFIX)
}

fn select_persistent_exited_pty_candidate(
    matching: &[SessionInfo],
    identity_matches: impl Fn(&SessionInfo) -> bool,
) -> Option<&SessionInfo> {
    matching
        .iter()
        .filter(|session| is_persistent_exited_pty_candidate(session) && identity_matches(session))
        .max_by(|left, right| {
            let left_time = chrono::DateTime::parse_from_rfc3339(&left.created_at).ok();
            let right_time = chrono::DateTime::parse_from_rfc3339(&right.created_at).ok();
            left_time
                .cmp(&right_time)
                .then_with(|| left.id.cmp(&right.id))
        })
}

fn session_has_current_pty_submission_provenance(
    session: &SessionInfo,
    settings: &AppSettings,
) -> bool {
    let args = session
        .effective_shell_args
        .as_deref()
        .unwrap_or(&session.shell_args);
    if crate::session::profile::detect_pty_submission_agent(
        &session.shell,
        args,
        session.agent_kind,
    )
    .is_some()
    {
        return true;
    }
    if !session.trusted_configured_spawn {
        return false;
    }
    let Some(agent_id) = session.agent_id.as_deref() else {
        return false;
    };
    // #2716 (B3, option C) row 12: a read-only comparison, not a spawn, so it is
    // exempt from the builders' refusal. Membership is read from `settings.agents`
    // directly, as `compute_profile_outdated` does.
    if !settings.agents.iter().any(|agent| agent.id == agent_id) {
        return false;
    }
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&session.working_directory);
    let Ok(spawn) = crate::config::agent_command::build_agent_spawn_command(
        settings,
        agent_id,
        Some(std::path::Path::new(&cwd)),
        session.requested_profile.as_deref(),
    ) else {
        return false;
    };
    session
        .pty_submission_agent_matches_current_spawn(&spawn)
        .is_some()
}

enum PtySpawnStep {
    Created(SessionInfo),
    OwnedCreated(
        SessionInfo,
        crate::session::selection::OwnedReplacementContinuation,
    ),
    Retained {
        continuation: crate::session::selection::OwnedReplacementContinuation,
        failed: bool,
    },
    RetainedUnowned,
    NeedReacquire(Option<crate::session::selection::PreparedOwnedReplacement>),
}

type OwnedPtyWorkTransfer = (
    crate::session::selection::OwnedReplacementContinuation,
    crate::session::selection::LifecycleEffectGuard,
);

async fn finish_owned_pty_work<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    work: &mut Option<OwnedPtyWorkTransfer>,
    observed: bool,
) {
    let Some((continuation, effect)) = work.take() else {
        return;
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    continuation
        .finish_observed_work_transfer(&manager, &pty, effect, observed)
        .await;
}

async fn observe_pty_spawn_scope<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target: &crate::config::teams::VerifiedPtyInputIdentity,
) -> (
    crate::config::sessions_persistence::CreateCarrierScope,
    Option<Uuid>,
) {
    let directory = crate::config::config_dir();
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            use crate::config::sessions_persistence::CreateCarrierScope;
            let scope = crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                &permit,
                Some(&target.canonical_fqn),
                &target.replica_identity.canonical_path,
            );
            let retained = match &scope {
                CreateCarrierScope::Protected {
                    session_id,
                    preparing_owner: None,
                    ..
                } => directory.as_ref().and_then(|directory| {
                    crate::config::sessions_persistence::retained_carrier_for_replacement(
                        &permit,
                        directory,
                        &target.canonical_fqn,
                        *session_id,
                        current,
                    )
                    .map(|_| *session_id)
                }),
                _ => None,
            };
            (scope, retained)
        })
        .await
}

async fn pty_spawn_scope_may_resume<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target: &crate::config::teams::VerifiedPtyInputIdentity,
) -> bool {
    use crate::config::sessions_persistence::CreateCarrierScope;
    matches!(
        observe_pty_spawn_scope(app, target).await,
        (CreateCarrierScope::Foreign { .. }, _)
            | (
                CreateCarrierScope::Protected {
                    preparing_owner: None,
                    ..
                },
                Some(_),
            )
    )
}

fn pty_resume_metadata_matches(
    original: &crate::phone::types::PtyInputResult,
    current: &crate::phone::types::PtyInputResult,
    claimed: &crate::api::message_store::ClaimedPtyInputOperation,
) -> bool {
    !current.terminal
        && !original.terminal
        && original.status == crate::phone::types::PtyInputPublicStatus::Queued
        && original.issued_at.is_some()
        && original.queued_at.is_some()
        && original.payload_bytes.is_some()
        && original.payload_sha256.is_some()
        && current.injection_id == original.injection_id
        && current.op_id == original.op_id
        && current.sender == original.sender
        && current.target == original.target
        && current.source_plane == original.source_plane
        && current.expires_at == original.expires_at
        && current.status == crate::phone::types::PtyInputPublicStatus::Queued
        && current.injection_id == claimed.injection_id
        && current.op_id.as_deref() == Some(claimed.op_id.as_str())
        && current.sender.as_deref() == Some(claimed.sender_fqn.as_str())
        && current.target.as_deref() == Some(claimed.target_fqn.as_str())
        && current.source_plane == Some(claimed.source_plane)
        && current.expires_at.as_deref() == Some(claimed.expires_at.as_str())
        && current.version == original.version
        && current.payload_bytes == original.payload_bytes
        && current.payload_sha256 == original.payload_sha256
        && current.issued_at == original.issued_at
        && current.queued_at == original.queued_at
        && current.selected_session_id.is_none()
        && current.selected_backend.is_none()
        && current.actuating_at.is_none()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtyClaimRefresh {
    Current,
    Terminal,
}

#[derive(Clone, Copy)]
enum PtyReacquireGoal {
    Spawn,
    Published(Uuid),
}

async fn reacquire_pty_dispatch_target<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    source: (
        &Arc<crate::api::message_store::MessageStore>,
        &crate::phone::types::PtyInputResult,
        &crate::api::message_store::ClaimedPtyInputOperation,
    ),
    target: (
        &Arc<crate::api::message_store::PtyInputTargetGate>,
        &crate::config::teams::VerifiedPtyInputIdentity,
        PtyReacquireGoal,
    ),
    deadline: (
        chrono::DateTime<chrono::Utc>,
        &tokio_util::sync::CancellationToken,
    ),
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
) -> Option<(
    crate::api::message_store::PtyInputStripeGuard,
    crate::api::message_store::PtyInputTargetGuard,
)> {
    use crate::phone::types::PtyInputReasonCode as C;
    let (store, initial, claimed) = source;
    let (gate, target, goal) = target;
    let (expires, shutdown) = deadline;
    let acquired = await_pty_input_before_deadline(expires, shutdown, async {
        if refresh_pty_claim_for_resume(store, initial, claimed).await? == PtyClaimRefresh::Terminal
        {
            return Ok(None);
        }
        if let Some(coordinator) =
            app.try_state::<crate::session::selection::SelectionCoordinator>()
        {
            coordinator
                .wait_for_restore_release()
                .await
                .map_err(|_| C::StoreTransient)?;
        }
        loop {
            let ready = match goal {
                PtyReacquireGoal::Spawn => pty_spawn_scope_may_resume(app, target).await,
                PtyReacquireGoal::Published(id) => {
                    !crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &target.canonical_fqn,
                        id,
                    )
                }
            };
            if ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let stripe = gate
            .acquire_target_lock(&target.canonical_fqn)
            .await
            .map_err(|_| C::StoreTransient)?;
        let exact = gate.acquire_exact(&target.canonical_fqn).await;
        if refresh_pty_claim_for_resume(store, initial, claimed).await? == PtyClaimRefresh::Terminal
        {
            return Ok(None);
        }
        Ok::<_, C>(Some((stripe, exact)))
    })
    .await;
    match acquired {
        Ok(Ok(Some(guards))) => Some(guards),
        Ok(Ok(None)) | Err(PtyInputPreBoundaryStop::Shutdown) => {
            heartbeat.finish().await;
            None
        }
        Ok(Err(code)) => {
            finish_pty_input_before_boundary(
                store,
                heartbeat,
                &claimed.injection_id,
                &claimed.lease_owner,
                code,
            )
            .await;
            None
        }
        Err(PtyInputPreBoundaryStop::Expired) => {
            reject_pty_input_before_boundary(store, heartbeat, &claimed.injection_id, C::Expired)
                .await;
            None
        }
    }
}

async fn refresh_pty_claim_for_resume(
    store: &crate::api::message_store::MessageStore,
    original: &crate::phone::types::PtyInputResult,
    claimed: &crate::api::message_store::ClaimedPtyInputOperation,
) -> Result<PtyClaimRefresh, crate::phone::types::PtyInputReasonCode> {
    use crate::phone::types::PtyInputReasonCode as C;
    let current = store
        .query_pty_input_by_injection_offloaded(claimed.injection_id.clone())
        .await
        .map_err(|_| C::StoreTransient)?
        .ok_or(C::AuthorityChanged)?;
    if current.terminal {
        return Ok(PtyClaimRefresh::Terminal);
    }
    if !pty_resume_metadata_matches(original, &current, claimed) {
        return Err(C::AuthorityChanged);
    }
    match store
        .renew_pty_input_lease_offloaded(
            claimed.injection_id.clone(),
            claimed.lease_owner.clone(),
            chrono::Utc::now(),
        )
        .await
    {
        Ok(true) => Ok(PtyClaimRefresh::Current),
        _ => {
            // The original claimed-expiry parse already validated this value.
            let expires = crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at)
                .map_err(|_| C::AuthorityChanged)?;
            Err(pty_input_lease_failure_code(expires))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PtyInputPreBoundaryStop {
    Expired,
    Shutdown,
}

async fn await_pty_input_before_deadline<F, T>(
    expires_at: chrono::DateTime<chrono::Utc>,
    shutdown: &tokio_util::sync::CancellationToken,
    future: F,
) -> Result<T, PtyInputPreBoundaryStop>
where
    F: std::future::Future<Output = T>,
{
    let remaining = expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .map_err(|_| PtyInputPreBoundaryStop::Expired)?;
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(PtyInputPreBoundaryStop::Shutdown),
        _ = tokio::time::sleep(remaining) => Err(PtyInputPreBoundaryStop::Expired),
        output = future => Ok(output),
    }
}

fn pty_input_lease_failure_code(
    expires_at: chrono::DateTime<chrono::Utc>,
) -> crate::phone::types::PtyInputReasonCode {
    if expires_at <= chrono::Utc::now() {
        crate::phone::types::PtyInputReasonCode::Expired
    } else {
        crate::phone::types::PtyInputReasonCode::LeaseLost
    }
}

async fn reject_pty_input_before_boundary(
    store: &crate::api::message_store::MessageStore,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    injection_id: &str,
    code: crate::phone::types::PtyInputReasonCode,
) -> bool {
    heartbeat.finish().await;
    match store
        .terminalize_pty_input_offloaded(
            injection_id.to_string(),
            crate::phone::types::PtyInputPublicStatus::Rejected,
            Some(code),
            chrono::Utc::now(),
        )
        .await
    {
        Ok(result) => {
            crate::api::audit::record_pty_input_result("terminal", &result);
            result.terminal
        }
        Err(_) => {
            log::warn!(
                "[pty-input] rejection persistence failed id={} code=terminal_store_failed",
                injection_id
            );
            false
        }
    }
}

async fn finish_pty_input_before_boundary(
    store: &crate::api::message_store::MessageStore,
    heartbeat: &mut crate::api::message_store::PreparationHeartbeatGuard,
    injection_id: &str,
    lease_owner: &str,
    code: crate::phone::types::PtyInputReasonCode,
) -> bool {
    use crate::phone::types::PtyInputReasonCode as C;
    heartbeat.finish().await;
    if matches!(
        code,
        C::RestoreInProgress
            | C::PurgeInProgress
            | C::SessionRace
            | C::LeaseLost
            | C::SpawnFailedSafe
            | C::StoreTransient
            | C::MenuGuardBlocked
    ) {
        if store
            .retry_pty_input_offloaded(
                injection_id.to_string(),
                lease_owner.to_string(),
                code,
                chrono::Utc::now(),
            )
            .await
            .is_ok()
        {
            return true;
        }
        reject_pty_input_before_boundary(store, heartbeat, injection_id, C::StoreTransient).await
    } else {
        reject_pty_input_before_boundary(store, heartbeat, injection_id, code).await
    }
}

fn decode_outbox_snapshot(bytes: &[u8]) -> Result<String, ()> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(());
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| ())
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        let body = &bytes[2..];
        if !body.len().is_multiple_of(2) {
            return Err(());
        }
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| ())
    } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        std::str::from_utf8(&bytes[3..])
            .map(str::to_owned)
            .map_err(|_| ())
    } else {
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| ())
    }
}

fn classify_outbox_document(path: &Path) -> OutboxClassification {
    let (bytes, identity) = match crate::path_identity::read_bounded_regular(
        path,
        crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
    ) {
        Ok(snapshot) => snapshot,
        Err(_) => return OutboxClassification::InvalidDocument,
    };
    let raw_privileged = raw_privileged_probe(&bytes);
    let content = match decode_outbox_snapshot(&bytes) {
        Ok(content) => content,
        Err(()) => {
            return if raw_privileged {
                OutboxClassification::PrivilegedCandidate { bytes, identity }
            } else {
                OutboxClassification::InvalidDocument
            };
        }
    };
    let scanned = match crate::path_identity::scan_json_document(content.as_bytes()) {
        Ok(scanned) => scanned,
        Err(_) => {
            return if raw_privileged || raw_privileged_probe(content.as_bytes()) {
                OutboxClassification::PrivilegedCandidate { bytes, identity }
            } else {
                OutboxClassification::InvalidDocument
            };
        }
    };
    if scanned.privileged_candidate {
        OutboxClassification::PrivilegedCandidate { bytes, identity }
    } else {
        // Standard duplicate-key behavior remains unchanged. The retained
        // snapshot is passed to the legacy decoder without another file read.
        OutboxClassification::Standard(content)
    }
}

fn write_atomic_pty_metadata(destination: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    let parent = destination.parent().ok_or("unsafe_path")?;
    let parent_identity =
        crate::path_identity::verify_directory(parent).map_err(|_| "unsafe_path")?;
    let destination_identity = match std::fs::symlink_metadata(destination) {
        Ok(_) => Some(
            crate::path_identity::verify_regular_file(destination).map_err(|_| "unsafe_path")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("unsafe_path"),
    };
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("unsafe_path")?;
    let temp = parent.join(format!(".{file_name}.{}.pty-input-tmp", Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let result = (|| {
        let mut file = options.open(&temp).map_err(|_| "artifact_unclaimed")?;
        use std::io::Write;
        file.write_all(bytes).map_err(|_| "artifact_unclaimed")?;
        file.flush().map_err(|_| "artifact_unclaimed")?;
        file.sync_all().map_err(|_| "artifact_unclaimed")?;
        crate::path_identity::verify_opened_regular_file(&temp, &file, false)
            .map_err(|_| "unsafe_path")?;
        drop(file);

        let current_parent =
            crate::path_identity::verify_directory(parent).map_err(|_| "unsafe_path")?;
        if !crate::path_identity::same_object(&parent_identity, &current_parent) {
            return Err("unsafe_path");
        }
        match &destination_identity {
            Some(expected) => {
                let current = crate::path_identity::verify_regular_file(destination)
                    .map_err(|_| "unsafe_path")?;
                if !crate::path_identity::same_object(expected, &current) {
                    return Err("unsafe_path");
                }
            }
            None => match std::fs::symlink_metadata(destination) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("unsafe_path"),
            },
        }
        crate::config::root_agent::atomic_replace_existing(&temp, destination)
            .map_err(|_| "artifact_unclaimed")?;
        let (published, _) = crate::path_identity::read_bounded_regular(
            destination,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        if published != bytes {
            return Err("artifact_unclaimed");
        }
        if let Ok(parent_handle) = std::fs::File::open(parent) {
            if let Err(error) = parent_handle.sync_all() {
                log::debug!(
                    "[pty-input] metadata parent sync failed code=artifact_unclaimed \
                    error_kind={:?}",
                    error.kind()
                );
            }
        }
        Ok(())
    })();
    if result.is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::debug!(
                    "[pty-input] metadata temp cleanup failed code=artifact_unclaimed \
                    error_kind={:?}",
                    error.kind()
                );
            }
        }
    }
    result
}

/// The MailboxPoller runs as a background tokio task. It polls outbox directories
/// for all known agent repos, validates messages, and delivers them according to mode.
#[cfg(test)]
type MailboxAttachCalls = Arc<Mutex<Vec<(Uuid, Option<String>)>>>;

#[cfg(test)]
#[derive(Clone, Default)]
struct MailboxTestHooks {
    /// Consumed only by an actual accepted replacement; None keeps ticket mode.
    replacement_fixture:
        Arc<Mutex<Option<Arc<crate::session::selection::ReplacementFixtureControl>>>>,
    pty_presence: Arc<Mutex<HashMap<Uuid, bool>>>,
    inject_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    /// (#1001 PR1 / G6) Scripted consumption verdicts, mirroring
    /// `inject_results`. When non-empty, the `inject_wake_into_pty` hook arm
    /// pops one after a successful inject and runs the SHARED `verdict_to_result`
    /// - so the AC3 hooked test exercises the real conversion, not a copy.
    consumption_results: Arc<Mutex<VecDeque<ConsumptionVerdict>>>,
    inject_calls: Arc<Mutex<Vec<Uuid>>>,
    settle_calls: Arc<Mutex<Vec<Uuid>>>,
    /// Sessions whose hooked live-settle step removes the SessionManager
    /// record, deterministically exercising the post-preflight race path.
    remove_session_on_settle: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    /// (#1399 T9) Sessions whose hooked inject panics instead of returning,
    /// reproducing the poisoned-PtyManager-lock unwind on the real delivery
    /// path so the `WakeOutcomeReport` guard is testable.
    panic_on_inject: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    /// Sessions that record hook observability but continue through the real
    /// command branch and canonical injector instead of scripted injection.
    real_inject_sessions: Arc<Mutex<std::collections::HashSet<Uuid>>>,
    internal_payloads: Arc<Mutex<Vec<String>>>,
    internal_bookkeeping: Arc<Mutex<Vec<InternalSystemBookkeeping>>>,
    internal_live_settle_gate: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    internal_live_settle_entered: Arc<tokio::sync::Notify>,
    internal_spawn_gate: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    internal_spawn_started: Arc<tokio::sync::Notify>,
    destroy_calls: Arc<Mutex<Vec<Uuid>>>,
    destroy_results: Arc<Mutex<VecDeque<Result<(), String>>>>,
    spawn_calls: Arc<Mutex<Vec<MailboxSpawnCall>>>,
    attach_calls: MailboxAttachCalls,
    events: Arc<Mutex<Vec<MailboxTestEvent>>>,
    /// (#747) `is_coordinator` for sessions created by the test spawn hook
    /// (default false, matching the historical harness). Production
    /// `create_session_inner` recomputes the flag from teams discovery, so a
    /// wake that respawns a coordinator target yields a coordinator record;
    /// tests exercising the raised-hand carry set this to mirror that.
    spawn_is_coordinator: Arc<Mutex<bool>>,
    /// (#2411) One-shot error for the test spawn arm, returned after the real
    /// coordinator-create clear runs, mirroring a create that fails after it.
    internal_spawn_error: Arc<Mutex<Option<String>>>,
}

#[cfg(test)]
impl MailboxTestHooks {
    fn replacement_fixture_control(
        &self,
    ) -> Option<Arc<crate::session::selection::ReplacementFixtureControl>> {
        self.replacement_fixture
            .lock()
            .expect("replacement fixture control")
            .clone()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct InternalSystemBookkeeping {
    session_id: Uuid,
    post_boundary: bool,
    silence_touch: bool,
    set_last_prompt: bool,
    peer_event: bool,
    response_watcher: bool,
    consumption_verdict: bool,
    mailbox_archive: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MailboxSpawnCall {
    to: String,
    session_name: String,
    cwd: String,
    shell: String,
    shell_args: Vec<String>,
    skip_auto_resume: bool,
    /// #1635-2 D5: true when the wake carried dispatch flags and the spawn
    /// suppressed the config.json tooling writes. The test-hook boundary is
    /// the observable decision surface (the hook short-circuits to the
    /// in-memory `mgr.create_session` and never writes config.json).
    skip_tooling_save: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum MailboxTestEvent {
    Inject(Uuid),
    Destroy(Uuid),
    Spawn(MailboxSpawnCall),
    Attach {
        session_id: Uuid,
        bot_id: Option<String>,
    },
}

impl MailboxPoller {
    pub(crate) fn active_terminal_snapshot_shutdown_owner(
    ) -> Option<crate::phone::terminal_snapshot::SnapshotScannerShutdownOwner> {
        crate::phone::terminal_snapshot::SnapshotScannerShutdownOwner::active()
    }
}

// Only primitive/static/UUID diagnostics; expressions remain lazy when disabled.
macro_rules! response_diag {
    ($stage:literal, $outcome:expr, $id:expr) => {
        if log::log_enabled!(
            target: "agentscommander_lib::response_close_diag",
            log::Level::Debug
        ) {
            if let Some(id) = $id {
                log::debug!(
                    target: "agentscommander_lib::response_close_diag",
                    "response_close_diag stage={} outcome={} site={} id={}",
                    $stage, $outcome, line!(), id
                );
            } else {
                log::debug!(
                    target: "agentscommander_lib::response_close_diag",
                    "response_close_diag stage={} outcome={} site={} id=unknown",
                    $stage, $outcome, line!()
                );
            }
        }
    };
}
macro_rules! response_diag_result {
    ($value:expr, $stage:literal, $id:expr) => {{
        let result = $value;
        if log::log_enabled!(
            target: "agentscommander_lib::response_close_diag",
            log::Level::Debug
        ) {
            let outcome = match &result {
                Ok(_) => "ok",
                Err(crate::pty::manager::ResponseCloseError::Unavailable) => "unavailable",
                Err(crate::pty::manager::ResponseCloseError::Unknown) => "unknown",
                Err(crate::pty::manager::ResponseCloseError::Conflict) => "conflict",
                Err(crate::pty::manager::ResponseCloseError::ClosingDeferred) => "closing_deferred",
            };
            response_diag!($stage, outcome, $id);
        }
        result
    }};
}

fn response_diag_emit_lazy(
    stage: &'static str,
    outcome: impl FnOnce() -> &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        if let Some(id) = id() {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                stage={} outcome={} site={} id={}", stage, outcome(), site, id);
        } else {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                stage={} outcome={} site={} id=unknown", stage, outcome(), site);
        }
    }
}
fn response_diag_option_lazy<T>(
    value: Option<T>,
    stage: &'static str,
    outcome: &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) -> Option<T> {
    match value {
        Some(value) => Some(value),
        None => {
            response_diag_emit_lazy(stage, || outcome, id, site);
            None
        }
    }
}
fn response_diag_result_ref<T>(
    value: &Result<T, crate::pty::manager::ResponseCloseError>,
    stage: &'static str,
    id: impl FnOnce() -> Option<Uuid>,
    site: u32,
) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        let outcome = match value {
            Ok(_) => "ok",
            Err(crate::pty::manager::ResponseCloseError::Unavailable) => "unavailable",
            Err(crate::pty::manager::ResponseCloseError::Unknown) => "unknown",
            Err(crate::pty::manager::ResponseCloseError::Conflict) => "conflict",
            Err(crate::pty::manager::ResponseCloseError::ClosingDeferred) => "closing_deferred",
        };
        response_diag_emit_lazy(stage, || outcome, id, site);
    }
}
fn response_diag_debug_only(action: impl FnOnce()) {
    if log::log_enabled!(
        target: "agentscommander_lib::response_close_diag",
        log::Level::Debug
    ) {
        action();
    }
}
macro_rules! response_diag_local {
    ($stage:literal, $outcome:expr, $id:expr) => {
        response_diag_emit_lazy($stage, || $outcome, || $id, line!())
    };
}
macro_rules! response_diag_option_local {
    ($value:expr, $stage:literal, $outcome:literal, $id:expr) => {
        response_diag_option_lazy($value, $stage, $outcome, || $id, line!())
    };
}
macro_rules! response_diag_result_local {
    ($value:expr, $stage:literal, $id:expr) => {{
        let result = $value;
        response_diag_result_ref(&result, $stage, || $id, line!());
        result
    }};
}

fn current_response_row<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    identity: &crate::pty::manager::ResponseRouteIdentity,
) -> Option<&'a Session> {
    current.rows.iter().find(|row| {
        row.id == identity.session_id
            && row.token == identity.token
            && row.backend_kind == identity.backend
            && !current.pending_ids.contains(&row.id)
            && !matches!(row.status, SessionStatus::Exited(_))
            && current.versions.get(&row.id) == Some(&identity.carrier_version)
    })
}

/// Both frozen route lifecycles and the caller's current interior manager read
/// remain live through the pure tracker transition. The coordinator keeps its
/// actual backend; the member alone must be a local interactive process.
fn with_verified_response_pair<T>(
    current: &crate::session::manager::CarrierSnapshot,
    pty: &Arc<Mutex<PtyManager>>,
    state: &crate::pty::manager::ResponseCloseState,
    member: &crate::pty::manager::ResponseRouteIdentity,
    coordinator: &crate::pty::manager::ResponseRouteIdentity,
    publish: impl FnOnce(&Session, &Session) -> T,
) -> Option<T> {
    response_diag_debug_only(|| {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
            site={} outcome=observed stage=verified_pair scope=stage_current id=unknown member={} \
            coordinator={} current_member_version={:?} current_coordinator_version={:?} \
            captured_member_version={} captured_coordinator_version={} \
            token_backend_role_physical_checks=not_evaluated",
            line!(),
            member.session_id,
            coordinator.session_id,
            current.versions.get(&member.session_id),
            current.versions.get(&coordinator.session_id),
            member.carrier_version,
            coordinator.carrier_version
        );
    });
    let member_row = response_diag_option_local!(
        current_response_row(current, member),
        "verified_pair",
        "current_member_rejected",
        None::<Uuid>
    )?;
    let coordinator_row = response_diag_option_local!(
        current_response_row(current, coordinator),
        "verified_pair",
        "current_coordinator_rejected",
        None::<Uuid>
    )?;
    if member.backend != SessionBackendKind::LocalProcess
        || member_row.is_root_agent
        || member_row.is_coordinator
        || member_row.agent_kind.is_none()
        || !coordinator_row.is_coordinator
        || coordinator_row.is_root_agent
    {
        response_diag_local!("verified_pair", "role_or_backend", None::<Uuid>);
        return None;
    }
    let member_route = response_diag_option_local!(
        PtyManager::snapshot_route_proof(pty, member.session_id).ok(),
        "verified_pair",
        "member_route_proof_missing",
        None::<Uuid>
    )?;
    let coordinator_route = response_diag_option_local!(
        PtyManager::snapshot_route_proof(pty, coordinator.session_id).ok(),
        "verified_pair",
        "coordinator_route_proof_missing",
        None::<Uuid>
    )?;
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || coordinator_route.backend_kind() != coordinator.backend
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &member_row.working_directory,
        ))
        .is_ok_and(|actual| crate::path_identity::same_object(&actual, member_route.saved_cwd()))
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &coordinator_row.working_directory,
        ))
        .is_ok_and(|actual| {
            crate::path_identity::same_object(&actual, coordinator_route.saved_cwd())
        })
    {
        response_diag_local!("verified_pair", "backend_or_cwd_mismatch", None::<Uuid>);
        return None;
    }
    let member_authority = response_diag_option_local!(
        PtyManager::authority_route_proof(pty, member.session_id).ok(),
        "verified_pair",
        "member_authority_proof_missing",
        None::<Uuid>
    )?;
    let coordinator_authority = response_diag_option_local!(
        PtyManager::authority_route_proof(pty, coordinator.session_id).ok(),
        "verified_pair",
        "coordinator_authority_proof_missing",
        None::<Uuid>
    )?;
    let _member_guard = response_diag_option_local!(
        member_authority
            .lock_verified(
                member.backend,
                member_route.saved_cwd(),
                member_route.saved_replica(),
            )
            .ok(),
        "verified_pair",
        "member_authority_lock_rejected",
        None::<Uuid>
    )?;
    let _coordinator_guard = response_diag_option_local!(
        coordinator_authority
            .lock_verified(
                coordinator.backend,
                coordinator_route.saved_cwd(),
                coordinator_route.saved_replica(),
            )
            .ok(),
        "verified_pair",
        "coordinator_authority_lock_rejected",
        None::<Uuid>
    )?;
    if state.generation(member.session_id) != Some(member.generation)
        || state.generation(coordinator.session_id) != Some(coordinator.generation)
    {
        response_diag_local!("verified_pair", "generation_mismatch", None::<Uuid>);
        return None;
    }
    response_diag_debug_only(|| {
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
            site={} outcome=observed stage=verified_pair scope=stage_current member={} \
            coordinator={} captured_member_version={} captured_coordinator_version={} \
            current_member_version={:?} current_coordinator_version={:?} \
            token_backend_role_physical_generation_checks=true",
            line!(),
            member.session_id,
            coordinator.session_id,
            member.carrier_version,
            coordinator.carrier_version,
            current.versions.get(&member.session_id),
            current.versions.get(&coordinator.session_id)
        );
    });
    response_diag_local!("verified_pair", "callback_reached", None::<Uuid>);
    Some(publish(member_row, coordinator_row))
}

#[derive(Clone)]
struct ResponseRequestCandidate {
    context: crate::pty::manager::ResponseDeliveryContext,
    member: crate::pty::manager::ResponseRouteIdentity,
    coordinator: crate::pty::manager::ResponseRouteIdentity,
    initial: crate::config::sessions_persistence::InitialCarrierIntent,
}

struct ResponseCandidateAttempt {
    state: crate::pty::manager::ResponseCloseState,
    candidate: Mutex<Option<ResponseRequestCandidate>>,
    armed: std::sync::atomic::AtomicBool,
}

struct ResponseCandidateDisposal(Arc<ResponseCandidateAttempt>);

impl Drop for ResponseCandidateDisposal {
    fn drop(&mut self) {
        match self.0.candidate.lock() {
            Ok(candidate) => {
                response_diag_local!("candidate_disposal", "invoked", None::<Uuid>);
                if self
                    .0
                    .armed
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    if let Some(candidate) = candidate.as_ref() {
                        response_diag_local!(
                            "mark_request_submitted",
                            "false_invocation",
                            None::<Uuid>
                        );
                        response_diag_debug_only(|| {
                            log::debug!(
                                target: "agentscommander_lib::response_close_diag",
                                "response_close_diag stage=candidate_disposal \
                                 outcome=false_submission_invoked site={} id=unknown member={} \
                                 coordinator={}",
                                line!(),
                                candidate.member.session_id,
                                candidate.coordinator.session_id
                            );
                        });
                        self.0
                            .state
                            .mark_request_submitted(&candidate.context, false);
                    } else {
                        response_diag_local!(
                            "candidate_disposal",
                            "candidate_absent",
                            None::<Uuid>
                        );
                    }
                } else {
                    response_diag_local!("candidate_disposal", "attempt_disarmed", None::<Uuid>);
                }
            }
            Err(_) => {
                response_diag_local!("candidate_disposal", "candidate_lock_failure", None::<Uuid>);
                self.0.state.fail_closed()
            }
        }
    }
}

impl ResponseWakeWork {
    fn candidate_disposal(&self) -> ResponseCandidateDisposal {
        let attempt = Arc::new(ResponseCandidateAttempt {
            state: self.state.clone(),
            candidate: Mutex::new(None),
            armed: std::sync::atomic::AtomicBool::new(true),
        });
        *self
            .attempt
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(attempt.clone());
        ResponseCandidateDisposal(attempt)
    }
}

struct PreparedResponseRequest {
    member: crate::pty::manager::ResponseRouteIdentity,
    coordinator: crate::pty::manager::ResponseRouteIdentity,
    roots: Vec<String>,
}

fn reserve_response_request_at_write<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    current: &crate::session::manager::CarrierSnapshot,
    session: &Session,
    prepared: &PreparedResponseRequest,
    work: &ResponseWakeWork,
    attempt: &Arc<ResponseCandidateAttempt>,
) {
    if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
        response_diag_local!(
            "prewrite_reserve",
            "attempt_disarmed",
            work.diagnostic_message_id
        );
        return;
    }
    if app
        .try_state::<crate::session::selection::SelectionCoordinator>()
        .is_some_and(|coordinator| coordinator.restore_is_active())
    {
        response_diag_local!(
            "prewrite_reserve",
            "restore_active",
            work.diagnostic_message_id
        );
        return;
    }
    let settings = app.state::<SettingsState>().inner().clone();
    let Ok(settings) = settings.try_read_owned() else {
        response_diag_local!(
            "prewrite_reserve",
            "settings_busy",
            work.diagnostic_message_id
        );
        return;
    };
    if settings.project_paths != prepared.roots {
        response_diag_local!(
            "prewrite_reserve",
            "roots_changed",
            work.diagnostic_message_id
        );
        return;
    }
    let Some(member) = current_response_row(current, &prepared.member) else {
        response_diag_local!(
            "prewrite_reserve",
            "current_member_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Some(coordinator) = current_response_row(current, &prepared.coordinator) else {
        response_diag_local!(
            "prewrite_reserve",
            "current_coordinator_missing",
            work.diagnostic_message_id
        );
        return;
    };
    if session.id != member.id
        || member.backend_kind != SessionBackendKind::LocalProcess
        || member.is_coordinator
        || member.is_root_agent
        || member.agent_kind.is_none()
        || member
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == SessionCommunicationKind::CoManaged)
        || !coordinator.is_coordinator
        || coordinator.is_root_agent
    {
        response_diag_local!(
            "prewrite_reserve",
            "role_or_backend_or_communication",
            work.diagnostic_message_id
        );
        return;
    }
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let Ok(member_route) = PtyManager::snapshot_route_proof(&pty, member.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "member_route_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Some(replica) = member_route.saved_replica() else {
        response_diag_local!(
            "prewrite_reserve",
            "member_replica_missing",
            work.diagnostic_message_id
        );
        return;
    };
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || !crate::path_identity::verify_directory(std::path::Path::new(&member.working_directory))
            .is_ok_and(|actual| {
                crate::path_identity::same_object(&actual, member_route.saved_cwd())
            })
        || !member_route.matches_requester_route(
            member.backend_kind,
            member_route.saved_cwd(),
            Some(replica),
        )
    {
        response_diag_local!(
            "prewrite_reserve",
            "member_route_mismatch",
            work.diagnostic_message_id
        );
        return;
    }
    let Ok(coordinator_route) = PtyManager::snapshot_route_proof(&pty, coordinator.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_route_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Ok(authority) = PtyManager::authority_route_proof(&pty, coordinator.id) else {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_authority_missing",
            work.diagnostic_message_id
        );
        return;
    };
    let Ok(_coordinator_guard) = authority.lock_verified(
        coordinator.backend_kind,
        coordinator_route.saved_cwd(),
        coordinator_route.saved_replica(),
    ) else {
        response_diag_local!(
            "prewrite_reserve",
            "authority_lock_rejected",
            work.diagnostic_message_id
        );
        return;
    };
    if coordinator_route.backend_kind() != coordinator.backend_kind
        || !crate::path_identity::verify_directory(std::path::Path::new(
            &coordinator.working_directory,
        ))
        .is_ok_and(|actual| {
            crate::path_identity::same_object(&actual, coordinator_route.saved_cwd())
        })
        || work.state.generation(member.id) != Some(prepared.member.generation)
        || work.state.generation(coordinator.id) != Some(prepared.coordinator.generation)
    {
        response_diag_local!(
            "prewrite_reserve",
            "coordinator_route_or_generation_mismatch",
            work.diagnostic_message_id
        );
        return;
    }
    let now = std::time::Instant::now();
    work.state.apply_settings(
        settings.response_close_enabled,
        settings.response_close_idle_seconds,
        now,
    );
    work.state.publish_manager_facts(
        prepared.member.clone(),
        true,
        crate::session::session::is_working(member),
        now,
    );
    work.state.publish_manager_facts(
        prepared.coordinator.clone(),
        false,
        crate::session::session::is_working(coordinator),
        now,
    );
    let Ok(mut candidate) = attempt.candidate.lock() else {
        work.state.fail_closed();
        response_diag_local!(
            "prewrite_reserve",
            "candidate_lock_poisoned",
            work.diagnostic_message_id
        );
        return;
    };
    if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
        response_diag_local!(
            "prewrite_reserve",
            "attempt_disarmed_after_lock",
            work.diagnostic_message_id
        );
        return;
    }
    let Ok(context) = response_diag_result_local!(
        work.state.reserve_policy_candidate(
            prepared.member.clone(),
            prepared.coordinator.clone(),
            work.key.clone(),
            work.digest.clone(),
            now,
        ),
        "reserve_policy",
        work.diagnostic_message_id
    ) else {
        response_diag_local!(
            "prewrite_reserve",
            "reserve_policy_error",
            work.diagnostic_message_id
        );
        return;
    };

    *candidate = Some(ResponseRequestCandidate {
        context,
        member: prepared.member.clone(),
        coordinator: prepared.coordinator.clone(),
        initial: crate::config::sessions_persistence::InitialCarrierIntent::new(
            prepared.member.target.clone(),
            member.id,
            member.token,
            prepared.member.carrier_version,
            replica.canonical_path.clone(),
        ),
    });
    response_diag_local!(
        "prewrite_reserve",
        "candidate_created",
        work.diagnostic_message_id
    );
}

#[derive(Clone)]
struct ResponseWakeWork {
    diagnostic_message_id: Option<Uuid>,
    state: crate::pty::manager::ResponseCloseState,
    target: String,
    key: String,
    digest: String,
    attempt: Arc<Mutex<Option<Arc<ResponseCandidateAttempt>>>>,
    return_context: Option<crate::pty::manager::ReturnObservationContext>,
    return_routes: Option<(
        crate::pty::manager::ResponseRouteIdentity,
        crate::pty::manager::ResponseRouteIdentity,
    )>,
    // P05 creates the nonce-bound context at the actual pre-write boundary.
    #[allow(dead_code)]
    delivery_context: Option<crate::pty::manager::ResponseDeliveryContext>,
}

struct ValidatedWake {
    message: OutboxMessage,
    response: Option<ResponseWakeWork>,
}

// Local stage payloads carry the existing facts and fallback ownership only.
// A handled rejection remains terminal even when its archival result is Ok.
type OutboxStage<T = ()> = Result<std::ops::ControlFlow<Result<(), String>, T>, String>;
type OutboxAuthenticationFacts = (bool, bool, bool, bool, bool);
type PeerWakeFallback = (
    bool,
    Option<Uuid>,
    Option<String>,
    Option<crate::session::session::SessionCommunication>,
);
type SpawnedPeerWake = (
    Uuid,
    Option<crate::config::agent_command::AgentSpawnCommand>,
    ResolvedWakeAgentCommand,
);

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the owned delivered message and its existing receipt ownership"
)]
enum OwnedPeerWakeStep {
    Delivered(OutboxMessage),
    Relist,
    Retained,
}

struct OwnedPeerWakeExecution {
    message: OutboxMessage,
    delivery: (WakeDeliveryOrigin, bool),
    response: Option<ResponseWakeWork>,
    flags_present: bool,
    request: crate::commands::session::OwnedReplacementCreateRequest,
    resolved_command: ResolvedWakeAgentCommand,
    continuation: crate::session::selection::OwnedReplacementContinuation,
    gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    exact: Option<crate::api::message_store::PtyInputTargetGuard>,
    stripe: Option<crate::api::message_store::PtyInputStripeGuard>,
}

struct OwnedPeerTarget {
    continuation: crate::session::selection::OwnedReplacementContinuation,
    gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    exact: crate::api::message_store::PtyInputTargetGuard,
    stripe: crate::api::message_store::PtyInputStripeGuard,
    physical: crate::path_identity::VerifiedPathIdentity,
}

fn owned_replacement_launch_recipe(
    request: &crate::commands::session::OwnedReplacementCreateRequest,
) -> serde_json::Value {
    let configured = request.resolved_spawn.as_ref().map(|spawn| {
        let seed = spawn.seed.as_ref().map(|seed| {
            serde_json::json!({
                "dest": seed.dest,
                "candidates": seed.candidates.iter().map(|candidate| serde_json::json!({
                    "tier": format!("{:?}", candidate.tier),
                    "os": candidate.os_specific, "path": candidate.path,
                })).collect::<Vec<_>>(),
                "replicaRoot": seed.context.replica_root,
                "rootKind": format!("{:?}", seed.context.root_kind),
                "acRoot": seed.context.ac_root, "matrixRoot": seed.context.matrix_root,
                "warning": seed.config_dir_warning,
            })
        });
        serde_json::json!({
            "shell": spawn.shell, "args": spawn.shell_args,
            "agentEnv": spawn.agent_env, "profileEnv": spawn.profile_env,
            "generatedEnv": spawn.generated_env, "childEnv": spawn.child_env,
            "removeEnv": spawn.env_remove_keys, "codexHome": spawn.effective_codex_home,
            "requestedProfile": spawn.profile_resolution.requested_profile,
            "effectiveProfile": spawn.profile_resolution.effective_profile,
            "fallbackChain": spawn.profile_resolution.fallback_chain,
            "fallbackApplied": spawn.profile_resolution.fallback_applied,
            "profileHash": spawn.profile_content_hash,
            "trustedId": spawn.trusted_agent_id, "trustedLabel": spawn.trusted_agent_label,
            "backend": format!("{:?}", SessionBackendKind::from(&spawn.backend)),
            "matchTier": spawn.match_tier, "originalProfile": spawn.original_profile_letter,
            "seed": seed,
        })
    });
    serde_json::json!({
        "shell": request.shell, "args": request.shell_args, "cwd": request.cwd,
        "name": request.name, "agentId": request.agent_id, "agentLabel": request.agent_label,
        "skipToolingSave": request.skip_tooling_save, "repos": request.git_repos,
        "hostShell": request.host_shell.as_ref().map(|host| (&host.program, &host.args)),
        "configured": configured,
    })
}

pub struct MailboxPoller {
    poll_interval: std::time::Duration,
    retry_tracker: HashMap<PathBuf, RetryState>,
    response_sources: Arc<Mutex<HashMap<PathBuf, ResponseWakeWork>>>,
    snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner,
    /// (#1399) Claim paths this process has handed off whose outcome has not yet
    /// been successfully settled by `drain_wake_outcomes`. Membership includes
    /// running workers, queued reports, and reports requeued after filesystem
    /// observation or mutation failed. Anything else on disk is unowned.
    live_claims: std::collections::HashSet<PathBuf>,
    /// (#1399) Per-target delivery lanes shared with every spawned worker.
    wake_lanes: Arc<crate::phone::wake_lanes::WakeLanes>,
    wake_workers: tokio::task::JoinSet<()>,
    /// (#1399) Unbounded is safe by construction: admission caps both running
    /// workers and owned claims at `WAKE_WORKER_LIMIT`. Each owned claim has at
    /// most one report, and a failed settlement requeues that same report, so
    /// at most 8 reports are pending while worker sends remain non-blocking.
    wake_outcomes_tx: tokio::sync::mpsc::UnboundedSender<(PathBuf, Result<(), String>)>,
    wake_outcomes_rx: tokio::sync::mpsc::UnboundedReceiver<(PathBuf, Result<(), String>)>,
    #[cfg(test)]
    test_hooks: Option<MailboxTestHooks>,
}

// ── (#885) purge-wg gate types and pure evaluator ──────────────────────

/// (#885) One peer's correlated gate input. Built from the three snapshots
/// (mirror `Vec<SessionInfo>`, `pty_live: HashSet<Uuid>`,
/// `Vec<PurgeReadiness>`) before `evaluate_gate` is called.
#[derive(Debug, Clone)]
pub(crate) struct PurgeGatePeer {
    pub fqn: String,
    pub all_session_ids: Vec<String>,
    pub live: Vec<PurgeGateSession>,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgeGateSession {
    pub session_id: Uuid,
    pub readiness: crate::pty::idle_detector::PurgeReadiness,
    pub mirror_idle: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgeDecision {
    pub passed: bool,
    pub peers: Vec<PurgePeerResult>,
}

#[derive(Debug, Clone)]
pub(crate) struct PurgePeerResult {
    pub fqn: String,
    pub purgeable: bool,
    pub outcome: &'static str,
    pub idle_ms: Option<u128>,
    pub silence_ms: Option<u128>,
    pub watcher_idle: bool,
    pub mirror_idle: bool,
    pub resize_settled: bool,
    pub session_ids: Vec<String>,
}

/// (#885) Pure gate evaluator. Called by both the dry-run and the real path,
/// so a dry-run cannot lie (§3.3). A peer is purgeable iff it has no live
/// sessions (vacuously purgeable) or ALL of its live sessions pass the
/// four-leg test:
///   1. `activity_age >= effective_quiet` (printable silence)
///   2. `watcher_idle` (watcher agreement)
///   3. `mirror_idle` (SessionManager agreement)
///   4. `resize_settled` (F-1: activity readings are trustworthy)
pub(crate) fn evaluate_gate(peers: &[PurgeGatePeer], quiet: std::time::Duration) -> PurgeDecision {
    let mut passed = true;
    let mut results = Vec::with_capacity(peers.len());

    for peer in peers {
        if peer.live.is_empty() {
            results.push(PurgePeerResult {
                fqn: peer.fqn.clone(),
                purgeable: true,
                outcome: "skipped",
                idle_ms: None,
                silence_ms: None,
                watcher_idle: false,
                mirror_idle: true,
                resize_settled: true,
                session_ids: peer.all_session_ids.clone(),
            });
            continue;
        }

        let mut peer_purgeable = true;
        let mut saw_untracked_failure = false;
        let mut saw_busy_failure = false;
        let mut saw_resize_only_failure = false;
        let mut idle_ms: Option<u128> = None;
        let mut silence_ms: Option<u128> = None;
        let mut watcher_idle = true;
        let mut mirror_idle = true;
        let mut resize_settled = true;

        for session in &peer.live {
            let r = &session.readiness;
            let effective_quiet = quiet.max(r.idle_threshold);
            let m_idle = session.mirror_idle;
            let r_settled = match r.last_resize_age {
                None => true,
                Some(a) => a >= r.resize_grace + effective_quiet,
            };
            let activity_ok = matches!(r.activity_age, Some(a) if a >= effective_quiet);
            let purgeable = activity_ok && r.watcher_idle && m_idle && r_settled;

            if !purgeable {
                peer_purgeable = false;
                if r.activity_age.is_none() {
                    saw_untracked_failure = true;
                } else if !r_settled && activity_ok && r.watcher_idle && m_idle {
                    saw_resize_only_failure = true;
                } else {
                    saw_busy_failure = true;
                }
            }

            if let Some(a) = r.activity_age {
                idle_ms = Some(idle_ms.map_or(a.as_millis(), |m: u128| m.min(a.as_millis())));
            }
            if let Some(s) = r.silence_age {
                silence_ms = Some(silence_ms.map_or(s.as_millis(), |m: u128| m.min(s.as_millis())));
            }
            if !r.watcher_idle {
                watcher_idle = false;
            }
            if !m_idle {
                mirror_idle = false;
            }
            if !r_settled {
                resize_settled = false;
            }
        }

        if !peer_purgeable {
            passed = false;
        }

        // (#885 E-5) Distinguish resize-unsettled from busy, but aggregate
        // across all live sessions for the peer. `resize_unsettled` is only
        // honest when resize is the sole failing leg for every failing live
        // session. Unknown liveness is more important than either diagnostic.
        let peer_outcome = if peer_purgeable {
            "skipped"
        } else if saw_untracked_failure {
            "untracked"
        } else if saw_busy_failure {
            "busy"
        } else if saw_resize_only_failure {
            "resize_unsettled"
        } else {
            "busy"
        };

        results.push(PurgePeerResult {
            fqn: peer.fqn.clone(),
            purgeable: peer_purgeable,
            outcome: peer_outcome,
            idle_ms,
            silence_ms,
            watcher_idle,
            mirror_idle,
            resize_settled,
            session_ids: peer.all_session_ids.clone(),
        });
    }

    PurgeDecision {
        passed,
        peers: results,
    }
}

impl Default for MailboxPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl MailboxPoller {
    fn settle_response_source(&self, path: &Path, _success: bool) {
        let origin = wake_claim_origin(path).unwrap_or_else(|| path.to_path_buf());
        let work = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&origin);
        if let Some(work) = work {
            let now = std::time::Instant::now();
            work.state.settle_terminal(&work.target, &work.key, now);
            if let Some(context) = work.return_context {
                // No AppHandle/current manager and route boundary exists here.
                // A synchronous source settlement can only terminate a return.
                let _ = work.state.settle_return_terminal(&context, now);
            }
        }
    }

    async fn prepare_response_request<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        session_id: Uuid,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Option<PreparedResponseRequest> {
        response_diag_local!("prepare", "entry", Uuid::parse_str(&msg.id).ok());
        let work = response_diag_option_local!(
            response,
            "prepare",
            "response_work_missing",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if comanaged
            || msg.command.is_some()
            || msg.action.is_some()
            || msg.get_output
            || msg.pty_input.is_some()
            || msg.from.rsplit_once('/').map(|(room, _)| room)
                != msg.to.rsplit_once('/').map(|(room, _)| room)
        {
            response_diag_local!(
                "prepare",
                "excluded_message_class_or_room",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let token = response_diag_option_local!(
            Uuid::parse_str(response_diag_option_local!(
                msg.token.as_deref(),
                "prepare",
                "token_missing",
                Uuid::parse_str(&msg.id).ok()
            )?)
            .ok(),
            "prepare",
            "token_invalid",
            Uuid::parse_str(&msg.id).ok()
        )?;
        let roots = app
            .state::<SettingsState>()
            .read()
            .await
            .project_paths
            .clone();
        response_diag_option_local!(
            teams::verified_wg_coordinator_target(&msg.from, &roots),
            "prepare",
            "configured_coordinator_rejected",
            Uuid::parse_str(&msg.id).ok()
        )?;
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let snapshot = manager.aggregate_snapshot().await;
        let member = response_diag_option_local!(
            snapshot.sessions.iter().find(|row| row.id == session_id),
            "prepare",
            "member_absent",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if sender_name_for_session_cwd_with_root_flag(
            &member.working_directory,
            member.is_root_agent,
        ) != msg.to
        {
            response_diag_local!(
                "prepare",
                "member_target_mismatch",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let mut senders = snapshot
            .sessions
            .iter()
            .filter(|row| row.token == token && !matches!(row.status, SessionStatus::Exited(_)));
        let coordinator = response_diag_option_local!(
            senders.next(),
            "prepare",
            "sender_missing",
            Uuid::parse_str(&msg.id).ok()
        )?;
        if senders.next().is_some()
            || sender_name_for_session_cwd_with_root_flag(
                &coordinator.working_directory,
                coordinator.is_root_agent,
            ) != msg.from
        {
            response_diag_local!(
                "prepare",
                "sender_ambiguous_or_target_mismatch",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        let coordinator_id = coordinator.id;
        manager
            .with_response_close_facts(|current| {
                let member = response_diag_option_local!(
                    current.rows.iter().find(|row| {
                        row.id == session_id
                            && row.token == member.token
                            && !current.pending_ids.contains(&row.id)
                    }),
                    "prepare",
                    "current_member_absent_or_pending",
                    Uuid::parse_str(&msg.id).ok()
                )?;
                let coordinator = response_diag_option_local!(
                    current.rows.iter().find(|row| {
                        row.id == coordinator_id
                            && row.token == token
                            && !current.pending_ids.contains(&row.id)
                    }),
                    "prepare",
                    "current_coordinator_absent_or_pending",
                    Uuid::parse_str(&msg.id).ok()
                )?;
                let route = |row: &Session, target: &str| {
                    Some(crate::pty::manager::ResponseRouteIdentity {
                        target: target.to_owned(),
                        session_id: row.id,
                        token: row.token,
                        generation: response_diag_option_local!(
                            work.state.generation(row.id),
                            "prepare",
                            "route_generation_absent",
                            Uuid::parse_str(&msg.id).ok()
                        )?,
                        carrier_version: *response_diag_option_local!(
                            current.versions.get(&row.id),
                            "prepare",
                            "carrier_version_absent",
                            Uuid::parse_str(&msg.id).ok()
                        )?,
                        backend: row.backend_kind,
                    })
                };
                let prepared = PreparedResponseRequest {
                    member: route(member, &msg.to)?,
                    coordinator: route(coordinator, &msg.from)?,
                    roots,
                };
                response_diag_local!("prepare", "prepared", Uuid::parse_str(&msg.id).ok());
                Some(prepared)
            })
            .await
    }

    async fn complete_response_request<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        work: &ResponseWakeWork,
    ) {
        let attempt = {
            match work.attempt.lock() {
                Ok(attempt) => attempt.clone(),
                Err(_) => {
                    response_diag_local!(
                        "complete_request",
                        "attempt_lock_error",
                        work.diagnostic_message_id
                    );
                    None
                }
            }
        };
        let Some(attempt) = attempt else {
            response_diag_local!(
                "complete_request",
                "attempt_missing",
                work.diagnostic_message_id
            );
            return;
        };
        let candidate = {
            match attempt.candidate.lock() {
                Ok(candidate) => candidate.clone(),
                Err(_) => {
                    response_diag_local!(
                        "complete_request",
                        "candidate_lock_error",
                        work.diagnostic_message_id
                    );
                    None
                }
            }
        };
        let Some(candidate) = candidate else {
            response_diag_local!(
                "complete_request",
                "candidate_missing",
                work.diagnostic_message_id
            );
            return;
        };
        let Some(dir) = crate::config::config_dir() else {
            response_diag_local!(
                "complete_request",
                "config_dir_unavailable",
                work.diagnostic_message_id
            );
            return;
        };
        let roots = {
            let settings = app.state::<SettingsState>();
            let settings = settings.read().await;
            crate::config::sessions_persistence::session_retention_project_paths(&settings).ok()
        };
        let Some(roots) = roots else {
            response_diag_local!(
                "complete_request",
                "roots_unavailable",
                work.diagnostic_message_id
            );
            return;
        };
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let initial = if let Some(coordinator) =
            app.try_state::<crate::session::selection::SelectionCoordinator>()
        {
            let Some(exclusion) = coordinator.restore_mint_exclusion() else {
                response_diag_local!(
                    "complete_request",
                    "restore_exclusion_missing",
                    work.diagnostic_message_id
                );
                return;
            };
            candidate.initial.clone().with_restore_exclusion(exclusion)
        } else {
            candidate.initial.clone()
        };
        let published = crate::config::sessions_persistence::persist_initial_carrier(
            &manager, dir, roots, initial,
        )
        .await;
        response_diag_debug_only(|| {
            let outcome = match &published {
                Err(_) => "persist_initial_carrier_error",
                Ok(None) => "persist_initial_carrier_none",
                Ok(Some(_)) => "persist_initial_carrier_confirmed",
            };
            response_diag_local!("complete_request", outcome, work.diagnostic_message_id);
        });
        let Ok(Some(published)) = published else {
            response_diag_local!(
                "complete_request",
                "published_not_confirmed",
                work.diagnostic_message_id
            );
            return;
        };
        let settings = app.state::<SettingsState>();
        let settings = settings.read().await;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        if app
            .try_state::<crate::session::selection::SelectionCoordinator>()
            .is_some_and(|coordinator| coordinator.restore_is_active())
        {
            response_diag_local!(
                "complete_request",
                "restore_active",
                work.diagnostic_message_id
            );
            return;
        }
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                let diagnostic_pair = with_verified_response_pair(
                    current,
                    &pty,
                    &work.state,
                    &candidate.member,
                    &candidate.coordinator,
                    |member, coordinator| {
                        let Ok(_candidate_guard) = attempt.candidate.lock() else {
                            work.state.fail_closed();
                            response_diag_local!(
                                "complete_request",
                                "candidate_lock_poisoned",
                                work.diagnostic_message_id
                            );
                            return;
                        };
                        if !attempt.armed.load(std::sync::atomic::Ordering::Acquire) {
                            response_diag_local!(
                                "complete_request",
                                "attempt_disarmed",
                                work.diagnostic_message_id
                            );
                            return;
                        }
                        let now = std::time::Instant::now();
                        work.state.apply_settings(
                            settings.response_close_enabled,
                            settings.response_close_idle_seconds,
                            now,
                        );
                        work.state.publish_manager_facts(
                            candidate.member.clone(),
                            true,
                            crate::session::session::is_working(member),
                            now,
                        );
                        work.state.publish_manager_facts(
                            candidate.coordinator.clone(),
                            false,
                            crate::session::session::is_working(coordinator),
                            now,
                        );
                        let mut diagnostic_callback_reached = false;
                        let registered =
                            crate::config::sessions_persistence::register_published_carrier(
                                &permit,
                                &published,
                                current,
                                |_| {
                                    response_diag_debug_only(|| {
                                        diagnostic_callback_reached = true;
                                    });
                                    response_diag_local!(
                                        "complete_request",
                                        "register_published_carrier_callback_reached",
                                        work.diagnostic_message_id
                                    );
                                    response_diag_result_local!(
                                        work.state.register_request(
                                            crate::pty::manager::ResponseDeliveryProof {
                                                context: candidate.context.clone(),
                                                member: candidate.member.clone(),
                                                coordinator: candidate.coordinator.clone(),
                                                published_carrier_version: published.version,
                                            },
                                            now,
                                        ),
                                        "register_request",
                                        work.diagnostic_message_id
                                    )
                                    .ok()
                                },
                            );
                        response_diag_local!(
                            "complete_request",
                            if registered.is_some() {
                                "published_registration_some"
                            } else {
                                "published_registration_none"
                            },
                            work.diagnostic_message_id
                        );
                        response_diag_local!(
                            "complete_request",
                            if diagnostic_callback_reached {
                                "register_published_callback_reached"
                            } else {
                                "register_published_callback_not_reached"
                            },
                            work.diagnostic_message_id
                        );
                        if registered.is_some() {
                            let remembered =
                                crate::session::selection::remember_response_close_registration(
                                    crate::session::selection::ResponseCloseRegistration {
                                        diagnostic_last_poll_outcome: Arc::new(
                                            std::sync::atomic::AtomicU64::new(0),
                                        ),
                                        tracker: work.state.clone(),
                                        member: candidate.member.clone(),
                                        coordinator: candidate.coordinator.clone(),
                                        carrier: published.clone(),
                                    },
                                );
                            response_diag_local!(
                                "complete_request",
                                if remembered {
                                    "remember_true"
                                } else {
                                    "remember_false"
                                },
                                work.diagnostic_message_id
                            );
                            if !remembered {
                                work.state.fail_closed();
                            }
                            response_diag_local!(
                                "complete_request",
                                "disarm",
                                work.diagnostic_message_id
                            );
                            attempt
                                .armed
                                .store(false, std::sync::atomic::Ordering::Release);
                        }
                    },
                );
                response_diag_local!(
                    "complete_request",
                    if diagnostic_pair.is_some() {
                        "verified_pair_callback_reached"
                    } else {
                        "verified_pair_rejected"
                    },
                    work.diagnostic_message_id
                );
            })
            .await;
    }

    async fn complete_response_delivery<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
    ) {
        let origin = wake_claim_origin(path).unwrap_or_else(|| path.to_path_buf());
        let work = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&origin);
        let Some(work) = work else {
            response_diag!("complete_delivery", "source_work_absent", None::<Uuid>);
            return;
        };
        let app = app.clone();
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            Self::complete_response_request(&app, &work).await;
            let now = std::time::Instant::now();
            work.state.settle_terminal(&work.target, &work.key, now);
            response_diag!(
                "complete_delivery",
                "ordinary_terminal_settlement_invoked",
                work.diagnostic_message_id
            );
            let Some(context) = work.return_context else {
                response_diag!(
                    "complete_delivery",
                    "return_context_absent",
                    work.diagnostic_message_id
                );
                return;
            };
            let Some((member, coordinator)) = work.return_routes else {
                let _ = work.state.settle_return_terminal(&context, now);
                response_diag!(
                    "complete_delivery",
                    "return_routes_absent",
                    work.diagnostic_message_id
                );
                return;
            };
            let manager = app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .read()
                .await
                .clone();
            let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
            let settings = app.state::<SettingsState>();
            let settings = settings.read().await;
            work.state.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                now,
            );
            let success = manager
                .with_response_close_facts(|current| {
                    let diagnostic_pair = with_verified_response_pair(
                        current,
                        &pty,
                        &work.state,
                        &member,
                        &coordinator,
                        |member_row, coordinator_row| {
                            let now = std::time::Instant::now();
                            work.state.publish_manager_facts(
                                member.clone(),
                                true,
                                crate::session::session::is_working(member_row),
                                now,
                            );
                            work.state.publish_manager_facts(
                                coordinator.clone(),
                                false,
                                crate::session::session::is_working(coordinator_row),
                                now,
                            );
                            response_diag_result!(
                                work.state.observe_return_success(
                                    crate::pty::manager::ReturnDeliveryProof {
                                        context: context.clone(),
                                    },
                                    now,
                                ),
                                "observe_return_success",
                                work.diagnostic_message_id
                            )
                            .is_ok()
                        },
                    );
                    response_diag!(
                        "complete_delivery",
                        if diagnostic_pair.is_some() {
                            "verified_pair_reached"
                        } else {
                            "verified_pair_rejected"
                        },
                        work.diagnostic_message_id
                    );
                    diagnostic_pair.unwrap_or(false)
                })
                .await;
            if !success {
                let _ = work.state.settle_return_terminal(&context, now);
            }
        })
        .await;
    }

    async fn admit_response_source<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        authenticated: bool,
        content: &str,
    ) -> Option<ResponseWakeWork> {
        response_diag_local!(
            "admit_source",
            if authenticated {
                "entry_authenticated_input_true"
            } else {
                "entry_authenticated_input_false"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        let state = response_diag_option_local!(
            app.try_state::<crate::pty::manager::ResponseCloseState>(),
            "admit_source",
            "tracker_missing",
            Uuid::parse_str(&msg.id).ok()
        )?
        .inner()
        .clone();
        let digest = crate::phone::types::sha256_hex(content.as_bytes());
        if let Some(previous) = self
            .response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(path)
            .cloned()
        {
            if previous.digest != digest || previous.target != msg.to {
                state.fail_closed();
                response_diag_local!(
                    "admit_source",
                    "replay_identity_conflict",
                    Uuid::parse_str(&msg.id).ok()
                );
                return None;
            }
            response_diag_local!(
                "admit_source",
                "replay_reused",
                Uuid::parse_str(&msg.id).ok()
            );
            return Some(previous);
        }
        let key = format!("fs:{}:{}", path.to_string_lossy(), msg.id);
        let mut admission = match response_diag_result_local!(
            state.reserve_work(&msg.to, &key),
            "reserve_work",
            Uuid::parse_str(&msg.id).ok()
        ) {
            Ok(admission) => admission,
            Err(_) => {
                state.fail_closed();
                response_diag_local!(
                    "admit_source",
                    "reserve_work_error",
                    Uuid::parse_str(&msg.id).ok()
                );
                return None;
            }
        };
        if response_diag_result_local!(
            admission.accept_durable(&digest),
            "accept_durable",
            Uuid::parse_str(&msg.id).ok()
        )
        .is_err()
        {
            state.fail_closed();
            response_diag_local!(
                "admit_source",
                "accept_durable_error",
                Uuid::parse_str(&msg.id).ok()
            );
            return None;
        }
        drop(admission);
        response_diag_local!(
            "admit_source",
            "response_work_created",
            Uuid::parse_str(&msg.id).ok()
        );
        let mut work = ResponseWakeWork {
            diagnostic_message_id: if log::log_enabled!(
                target: "agentscommander_lib::response_close_diag",
                log::Level::Debug
            ) {
                Uuid::parse_str(&msg.id).ok()
            } else {
                None
            },
            state: state.clone(),
            target: msg.to.clone(),
            key,
            digest,
            attempt: Arc::default(),
            return_context: None,
            return_routes: None,
            delivery_context: None,
        };
        response_diag_local!(
            "admit_source",
            if authenticated {
                "authenticated_input_true"
            } else {
                "authenticated_input_false"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        if authenticated {
            let settings = app.state::<SettingsState>();
            let settings = settings.read().await;
            state.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                std::time::Instant::now(),
            );
            let coordinator =
                teams::verified_wg_coordinator_target(&msg.to, &settings.project_paths);
            drop(settings);
            let same_room = msg.from.rsplit_once('/').map(|(room, _)| room)
                == msg.to.rsplit_once('/').map(|(room, _)| room);
            response_diag_debug_only(|| {
                if coordinator.is_none() {
                    response_diag_local!(
                        "admit_source",
                        "configured_coordinator_absent",
                        Uuid::parse_str(&msg.id).ok()
                    );
                }
                if !same_room {
                    response_diag_local!(
                        "admit_source",
                        "cross_room",
                        Uuid::parse_str(&msg.id).ok()
                    );
                }
            });
            if coordinator.is_some() && same_room {
                let managers = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let manager = managers.read().await.clone();
                let token = msg
                    .token
                    .as_deref()
                    .and_then(|token| Uuid::parse_str(token).ok());
                let observed = manager
                    .with_response_close_facts(|snapshot| {
                        let route = |target: &str| {
                            let mut matches = snapshot.rows.iter().filter(|row| {
                                !snapshot.pending_ids.contains(&row.id)
                                    && sender_name_for_session_cwd_with_root_flag(
                                        &row.working_directory,
                                        row.is_root_agent,
                                    ) == target
                                    && !matches!(row.status, SessionStatus::Exited(_))
                            });
                            let row = response_diag_option_local!(
                                matches.next(),
                                "admit_source",
                                "route_missing",
                                Uuid::parse_str(&msg.id).ok()
                            )?;
                            if matches.next().is_some() {
                                response_diag_local!(
                                    "admit_source",
                                    "route_ambiguous",
                                    Uuid::parse_str(&msg.id).ok()
                                );
                                return None;
                            }
                            Some((
                                crate::pty::manager::ResponseRouteIdentity {
                                    target: target.to_string(),
                                    session_id: row.id,
                                    token: row.token,
                                    generation: response_diag_option_local!(
                                        state.generation(row.id),
                                        "admit_source",
                                        "generation_unavailable",
                                        Uuid::parse_str(&msg.id).ok()
                                    )?,
                                    carrier_version: *response_diag_option_local!(
                                        snapshot.versions.get(&row.id),
                                        "admit_source",
                                        "version_unavailable",
                                        Uuid::parse_str(&msg.id).ok()
                                    )?,
                                    backend: row.backend_kind,
                                },
                                row,
                            ))
                        };
                        let (member, member_row) = route(&msg.from)?;
                        let (coordinator, coordinator_row) = route(&msg.to)?;
                        if member.backend != SessionBackendKind::LocalProcess
                            || member_row.is_coordinator
                            || member_row.is_root_agent
                            || member_row.agent_kind.is_none()
                            || token != Some(member.token)
                            || !coordinator_row.is_coordinator
                            || coordinator_row.is_root_agent
                        {
                            response_diag_local!(
                                "admit_source",
                                "role_or_token_or_backend",
                                Uuid::parse_str(&msg.id).ok()
                            );
                            return None;
                        }
                        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                        let diagnostic_pair = with_verified_response_pair(
                            snapshot,
                            &pty,
                            &state,
                            &member,
                            &coordinator,
                            |member_row, coordinator_row| {
                                let now = std::time::Instant::now();
                                state.publish_manager_facts(
                                    member.clone(),
                                    true,
                                    crate::session::session::is_working(member_row),
                                    now,
                                );
                                state.publish_manager_facts(
                                    coordinator.clone(),
                                    false,
                                    crate::session::session::is_working(coordinator_row),
                                    now,
                                );
                                response_diag_result_local!(
                                    state.admit_return(
                                        &member,
                                        &coordinator,
                                        work.key.clone(),
                                        work.digest.clone(),
                                        now,
                                    ),
                                    "admit_return",
                                    Uuid::parse_str(&msg.id).ok()
                                )
                                .ok()
                            },
                        );
                        response_diag_local!(
                            "admit_source",
                            if diagnostic_pair.is_some() {
                                "verified_pair_callback_reached"
                            } else {
                                "verified_pair_rejected"
                            },
                            Uuid::parse_str(&msg.id).ok()
                        );
                        diagnostic_pair
                            .flatten()
                            .map(|context| (context, member, coordinator))
                    })
                    .await;
                if let Some((context, member, coordinator)) = observed {
                    work.return_context = Some(context);
                    work.return_routes = Some((member, coordinator));
                }
            }
        }
        response_diag_local!(
            "admit_source",
            if work.return_context.is_some() {
                "return_context_present"
            } else {
                "return_context_absent"
            },
            work.diagnostic_message_id
        );
        self.response_sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(path.to_path_buf(), work.clone());
        Some(work)
    }

    pub fn new() -> Self {
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner::default(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            #[cfg(test)]
            test_hooks: None,
        }
    }

    #[cfg(test)]
    fn new_with_test_hooks(test_hooks: MailboxTestHooks) -> Self {
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner: crate::phone::terminal_snapshot::SnapshotMailboxScanner::default(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            test_hooks: Some(test_hooks),
        }
    }

    /// (#1399 G2) A poller for the delivery tail only. Field-by-field and
    /// NEVER via `Self::new()` or struct-update over it: `new()` constructs the
    /// registering `SnapshotMailboxScanner::default()`, which writes itself
    /// into the process-global shutdown-owner slot and would leave the
    /// long-lived poller's scanner tasks unsealed and undrained at shutdown
    /// (#1403 tracks the pre-existing dispatcher case).
    fn delivery_only() -> Self {
        let (wake_outcomes_tx, wake_outcomes_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            poll_interval: std::time::Duration::from_secs(3),
            retry_tracker: HashMap::new(),
            response_sources: Arc::new(Mutex::new(HashMap::new())),
            snapshot_scanner:
                crate::phone::terminal_snapshot::SnapshotMailboxScanner::delivery_only(),
            live_claims: std::collections::HashSet::new(),
            wake_lanes: Arc::new(crate::phone::wake_lanes::WakeLanes::default()),
            wake_workers: tokio::task::JoinSet::new(),
            wake_outcomes_tx,
            wake_outcomes_rx,
            #[cfg(test)]
            test_hooks: None,
        }
    }

    /// (#1399) A delivery-only poller for a detached worker. `deliver_wake` and
    /// its whole `&self` callee chain read only `app.state::<...>()` (see the
    /// #791 note at the top of `deliver_wake`), so a fresh instance is
    /// equivalent to `&self` for the tail. Test hooks are carried across
    /// because `MailboxTestHooks` is `Arc`-based and `Clone`.
    fn delivery_poller(&self) -> Self {
        let mut poller = Self::delivery_only();
        poller.response_sources = self.response_sources.clone();
        #[cfg(test)]
        let poller = Self {
            test_hooks: self.test_hooks.clone(),
            ..poller
        };
        poller
    }

    /// Start the poller as a background task.
    pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
        tauri::async_runtime::spawn(async move {
            // Initial poll without delay (matches original behavior)
            if let Err(e) = self.poll(&app).await {
                log::warn!("MailboxPoller error: {}", e);
            }
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.token().cancelled() => {
                        log::info!("[MailboxPoller] Shutdown signal received, stopping");
                        // (#1399) Detach instead of cancelling: dropping a
                        // JoinSet cancels its tasks, which would cut a wake
                        // mid-inject. Detached workers run to completion
                        // best-effort, and any claim cut off by process exit
                        // is returned by reclamation at the next start.
                        self.wake_workers.detach_all();
                        break;
                    }
                    _ = tokio::time::sleep(self.poll_interval) => {
                        if let Err(e) = self.poll(&app).await {
                            log::warn!("MailboxPoller error: {}", e);
                        }
                    }
                }
            }
        });
    }

    fn scrub_stale_pty_input_temps(
        &self,
        outbox: &Path,
        store: &crate::api::message_store::MessageStore,
    ) {
        let Ok(entries) = std::fs::read_dir(outbox) else {
            return;
        };
        for entry in entries.filter_map(Result::ok).take(1_024) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".pty-input-request-tmp") || !name.starts_with('.') {
                continue;
            }
            let Some(injection_id) = name.split('.').nth(1) else {
                continue;
            };
            if crate::phone::types::parse_canonical_uuid_v4(injection_id).is_err()
                || self.verified_pty_outbox_owner(&path).is_err()
            {
                continue;
            }
            let old_enough = std::fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= std::time::Duration::from_secs(10 * 60));
            if !old_enough || crate::path_identity::verify_regular_file(&path).is_err() {
                continue;
            }
            let Ok(Some(_operation_lock)) = store.try_operation_lock(injection_id) else {
                continue;
            };
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::debug!(
                        "[pty-input] stale request temp cleanup failed id={} \
                        code=artifact_unclaimed error_kind={:?}",
                        injection_id,
                        error.kind()
                    );
                }
            }
        }
    }

    /// (#1399) Return every claim with no live owner to the outbox. A claim is
    /// unowned iff it is absent from `self.live_claims`: at the first cycle
    /// the set is empty, so every claim left by a previous process is
    /// returned; afterwards a claim is skipped while its worker is running or
    /// its outcome is undrained. The caller runs this only for a verified
    /// outbox (R2), so an unverifiable spelling defers reclamation instead of
    /// resurrecting a live claim. Assumes one daemon per outbox (different
    /// builds never share one, because `config::agent_local_dir_name()` derives
    /// the outbox from the binary stem).
    fn reclaim_unowned_wake_claims(&self, _outbox_dir: &Path, claims: &[PathBuf]) {
        for claim in claims {
            if self.live_claims.contains(claim) {
                continue;
            }
            let Some(origin) = wake_claim_origin(claim) else {
                continue;
            };
            // Sound because every outbox writer names the file `<msg.id>.json`
            // (cli send / close_session / self_switch / self_clear / purge_wg /
            // raise_hand); a writer breaking that convention breaks this
            // receipt check.
            let Some(id) = origin.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            // The receipt proves a terminal outcome, not successful delivery. Share
            // this predicate with the owned-outcome drain so both paths treat
            // a failed observation as pending rather than absence.
            let settled = match wake_claim_has_settled_receipt(&origin) {
                Ok(settled) => settled,
                Err(error) => {
                    log::warn!(
                        "[mailbox] #1399 could not inspect receipts for unowned claim {}: {}",
                        id,
                        error
                    );
                    continue;
                }
            };
            if settled {
                // Preserve pending if an origin remains or its observation fails.
                // Receipt recovery has no authenticated full-return delivery proof.
                let origin_absent = matches!(origin.try_exists(), Ok(false));
                // (#1399 R6) Keep the Result: `remove_file` fails under exactly
                // the locked-handle condition that is ordinary on Windows, and
                // a log line reporting an action that did not happen costs an
                // hour in an incident. Self-heals on a later cycle either way.
                match std::fs::remove_file(claim) {
                    Ok(()) => {
                        if origin_absent {
                            self.settle_response_source(&origin, false);
                        }
                        log::warn!(
                            "[mailbox] #1399 dropped unowned claim {} (receipt already present)",
                            id
                        );
                    }
                    Err(error) => log::warn!(
                        "[mailbox] #1399 could not drop unowned claim {}: {}",
                        id,
                        error
                    ),
                }
            } else if let Err(error) = std::fs::rename(claim, &origin) {
                // Retried on the next cycle. This is the only reason a locked
                // file cannot wedge the message.
                log::warn!(
                    "[mailbox] #1399 could not return unowned claim {}: {}",
                    id,
                    error
                );
            } else {
                log::warn!(
                    "[mailbox] #1399 returned unowned claim {} to the outbox",
                    id
                );
            }
        }
    }

    /// (#1399) Reserve the target's lane, claim the file on disk, and hand the
    /// delivery tail to a detached worker.
    ///
    /// MUST stay a synchronous `fn`. Between the `rename` and the
    /// `live_claims` insert the claim exists on disk and is not yet owned;
    /// because there is no await point here and `poll()` drives the drain, the
    /// reclamation and this function in sequence on one task, nothing can
    /// observe that window. An await inside it would let reclamation treat the
    /// claim as unowned and rename it back under a live worker.
    #[cfg(test)]
    fn claim_and_spawn_wake<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: OutboxMessage,
    ) -> WakeHandoff {
        self.claim_and_spawn_validated_wake(
            app,
            path,
            ValidatedWake {
                message: msg,
                response: None,
            },
        )
    }

    fn claim_and_spawn_validated_wake<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        validated: ValidatedWake,
    ) -> WakeHandoff {
        let msg = &validated.message;
        let response_delivery = validated
            .response
            .as_ref()
            .map(|work| work.state.begin_delivery(&work.target));
        let response_delivery = match response_delivery {
            Some(Err(crate::pty::manager::ResponseCloseError::ClosingDeferred)) => {
                return WakeHandoff::LaneBusy
            }
            Some(Err(crate::pty::manager::ResponseCloseError::Unavailable)) | None => None,
            Some(Err(error)) => {
                return WakeHandoff::ClaimFailed(format!("response close admission: {:?}", error))
            }
            Some(Ok(reservation)) => Some(reservation),
        };
        // A queued or requeued outcome can outlive both its worker and lane.
        // Cap `live_claims` separately so pending reports cannot exceed the
        // same global limit.
        if self.wake_workers.len() >= crate::phone::wake_lanes::WAKE_WORKER_LIMIT
            || self.live_claims.len() >= crate::phone::wake_lanes::WAKE_WORKER_LIMIT
        {
            return WakeHandoff::LaneBusy;
        }
        let Some(reservation) = self.wake_lanes.try_reserve(&msg.to) else {
            return WakeHandoff::LaneBusy;
        };
        let claim = wake_claim_path(path);
        // (#1399 G8) A worker that finished after this cycle's drain has
        // already renamed the file back and freed its lane, but its outcome is
        // still queued. Re-claiming now would race that outcome against a path
        // this claim is about to invalidate. Refuse until the outcome is
        // recorded; this is what makes "at most one attempt per cycle" true.
        if self.live_claims.contains(&claim) {
            return WakeHandoff::LaneBusy;
        }
        if let Err(error) = std::fs::rename(path, &claim) {
            // The reservation drops here, so the lane is free again.
            return if error.kind() == std::io::ErrorKind::NotFound {
                WakeHandoff::LaneBusy // the file vanished; nothing to do
            } else {
                WakeHandoff::ClaimFailed(format!("Failed to claim outbox message: {}", error))
            };
        }
        let worker_app = app.clone();
        let outcomes = self.wake_outcomes_tx.clone();
        let origin = path.to_path_buf();
        let delivery = self.delivery_poller();
        let worker_claim = claim.clone();
        self.wake_workers.spawn(async move {
            let _reservation = reservation;
            let _response_delivery = response_delivery;
            // (#1399 R1) RAII, for the same reason `_reservation` is: an
            // unwind must not be able to skip the report.
            let mut report = WakeOutcomeReport::new(outcomes, origin.clone());
            let outcome =
                deliver_claimed_wake(&delivery, &worker_app, &worker_claim, &origin, &validated)
                    .await;
            report.set(outcome);
        });
        // Inserted AFTER `spawn` returns: if `spawn` itself panicked, an
        // earlier insert would leave an entry no guard can rescue. Safe
        // because the drain is upstream in this same single-task sequence, so
        // the worker's outcome cannot be recorded before this insert.
        self.live_claims.insert(claim);
        WakeHandoff::Spawned
    }

    /// (#1399) Join every finished worker so `JoinSet::len()` means "in
    /// flight" when the cap is checked; runs at the top of the cycle,
    /// mirroring the dispatcher's reap-before-dispatch.
    fn reap_wake_workers(&mut self) {
        while let Some(result) = self.wake_workers.try_join_next() {
            if let Err(error) = result {
                // `error`, not `warn`: a worker that died without returning
                // may have stranded its claim until its guard's outcome
                // drains, and the condition must be greppable.
                log::error!(
                    "[mailbox] #1399 wake worker task failed (a claim may be stranded until its \
                    outcome drains): {}",
                    error
                );
            }
        }
    }

    /// (#1399) Settle one finite snapshot of worker reports. Ownership ends only
    /// after the corresponding on-disk state is settled and the outcome can be
    /// recorded without racing reclamation or an absent terminal-rejection input.
    async fn drain_wake_outcomes(&mut self) {
        let mut reports = Vec::new();
        while let Ok(report) = self.wake_outcomes_rx.try_recv() {
            reports.push(report);
        }

        for (path, outcome) in reports {
            let claim = wake_claim_path(&path);
            // Observation is part of settlement. `Path::exists()` would turn every
            // metadata/access failure into `false`, release the ledger, and either
            // count against an absent origin or resurrect an unreadable receipt.
            let settlement = (|| -> Result<(bool, bool, bool), String> {
                let claim_present = claim.try_exists().map_err(|error| {
                    format!("could not inspect owned claim {:?}: {}", claim, error)
                })?;
                let origin_present = path.try_exists().map_err(|error| {
                    format!("could not inspect owned origin {:?}: {}", path, error)
                })?;
                let payload_receipt_present = wake_claim_has_settled_receipt(&path)?;
                let claim_has_receipt = claim_present && payload_receipt_present;

                if claim_present {
                    let (action, disk_result) = if claim_has_receipt {
                        // Claim plus receipt is authoritative. Never resurrect it.
                        ("delete", std::fs::remove_file(&claim))
                    } else {
                        // `record_message_outcome` requires the origin to exist on
                        // every retry/reject path, including panic attempt 10.
                        ("restore", std::fs::rename(&claim, &path))
                    };
                    disk_result.map_err(|error| {
                        format!("could not {} owned claim {:?}: {}", action, claim, error)
                    })?;
                }

                Ok((claim_present, origin_present, payload_receipt_present))
            })();

            let (claim_present, origin_present, payload_receipt_present) = match settlement {
                Ok(state) => state,
                Err(error) => {
                    log::warn!(
                        "[mailbox] #1399 could not settle owned claim {:?}: {}; outcome remains \
                        pending",
                        claim,
                        error
                    );
                    // The receiver snapshot is already finite, so this report is
                    // not observed again until a later drain and cannot spin here.
                    if self.wake_outcomes_tx.send((path, outcome)).is_err() {
                        // Structurally unreachable while `self` owns the receiver.
                        // Keep ledger ownership; durable disk state is the restart fallback.
                        log::error!(
                            "[mailbox] #1399 could not requeue outcome for owned claim {:?}",
                            claim
                        );
                    }
                    continue;
                }
            };
            // Receipt-wins recovery applies only while the claimed payload still
            // exists. A receipt plus a restored origin is the existing
            // `move_to_delivered` cleanup-failure state and must keep its Err.
            let claim_has_receipt = claim_present && payload_receipt_present;

            if !claim_present && !origin_present && !payload_receipt_present {
                // No in-scope worker path produces this state. Do not retain an
                // owner/report pair with no durable payload, but make external
                // deletion or corruption visible before preserving today's
                // outcome-recording behavior.
                log::error!(
                    "[mailbox] #1399 outcome has no claim, origin, or payload receipt: {:?}",
                    path
                );
            }

            self.live_claims.remove(&claim);
            if claim_has_receipt {
                // The claim was removed successfully after receipt observation.
                // Never turn delivered-or-rejected into return success authority,
                // or retire work while a restored origin remains actionable.
                if !origin_present {
                    self.settle_response_source(&path, false);
                }
                // Receipt-wins recovery is already the contract of general
                // reclamation. Clear retry state and do not redeliver.
                self.record_message_outcome(path, Ok(())).await;
            } else {
                self.record_message_outcome(path, outcome).await;
            }
        }
    }

    /// (#1399) Deterministic replacement for "the delivery finished inside
    /// poll()". Takes no `AppHandle`: joining and draining need none, and the
    /// drain is the SHARED `drain_wake_outcomes` rather than a second copy.
    #[cfg(test)]
    async fn settle_wake_workers(&mut self) {
        while let Some(result) = self.wake_workers.join_next().await {
            if let Err(error) = result {
                log::warn!("[mailbox] wake worker failed: {}", error);
            }
        }
        self.drain_wake_outcomes().await;
    }

    /// One poll cycle: scan all repo outbox dirs, process each message.
    async fn poll<R: tauri::Runtime>(&mut self, app: &tauri::AppHandle<R>) -> Result<(), String> {
        self.snapshot_scanner.begin_cycle();
        // (#1399) The two timing lines the issue authorizes: one at the top
        // of the cycle and one at the bottom, each reporting elapsed ms,
        // outboxes scanned, and messages processed. Together they prove the
        // scanner returns in milliseconds instead of holding the loop for
        // delivery time.
        let cycle_started = std::time::Instant::now();
        let mut messages_processed = 0usize;
        log::info!(
            "[mailbox] poll cycle started elapsed_ms={} outboxes=0 messages=0",
            cycle_started.elapsed().as_millis()
        );
        // (#1399) Reap, then drain, before any outbox work: joined workers
        // free cap slots, and an outcome's path can belong to any outbox, so
        // the drain is per-cycle, not per-outbox. Draining before the scans
        // also means a rename-back the drain rejects cannot be re-claimed in
        // the same cycle.
        self.reap_wake_workers();
        self.drain_wake_outcomes().await;
        if let Some(state) = app.try_state::<crate::api::message_store::MessageStoreState>() {
            if let Ok(store) = &state.store {
                let active = state.active_operations.snapshot();
                if store
                    .recover_pty_input_runtime_offloaded(active, chrono::Utc::now())
                    .await
                    .is_err()
                {
                    log::warn!("[pty-input] runtime recovery failed code=store_transient");
                }
                if store
                    .compact_pty_terminal_maintenance_offloaded(chrono::Utc::now(), 64)
                    .await
                    .is_err()
                {
                    log::warn!("[pty-input] compaction failed code=store_transient");
                }
            }
        }
        let settings = app.state::<SettingsState>();
        let (repo_paths, archived) = {
            let cfg = settings.read().await;
            (
                cfg.project_paths.clone(),
                cfg.archived_project_paths.clone(),
            )
        };

        // Also scan CWDs of active sessions for repos not in settings
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session_dirs = {
            let mgr = session_mgr.read().await;
            mgr.get_sessions_working_dirs().await
        };
        let snapshot_session_dirs = session_dirs.clone();
        let archived_for_snapshot = archived.clone();
        let session_dirs = if archived.is_empty() {
            session_dirs
        } else {
            tokio::task::spawn_blocking(move || {
                let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
                retain_unarchived_session_dirs(session_dirs, &roots)
            })
            .await
            .map_err(|e| format!("archived mailbox-session filter task failed: {}", e))?
        };

        let mut startup_project_paths = repo_paths.clone();
        for path in &archived_for_snapshot {
            if !startup_project_paths.contains(path) {
                startup_project_paths.push(path.clone());
            }
        }
        let mut all_paths: Vec<String> = repo_paths;
        for (_, dir) in &session_dirs {
            if !all_paths.contains(dir) {
                all_paths.push(dir.clone());
            }
        }

        // One bounded startup sweep covers canonical Root plus verified WG
        // replicas under active and archived projects. Fresh artifacts are
        // registered so the monotonic cleanup task continues after this poll.
        if self.snapshot_scanner.startup_sweep_pending() {
            if let Some(state) =
                app.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
            {
                let mut startup_objects = std::collections::HashSet::new();
                if let Ok(root) = crate::config::root_agent::root_agent_dir() {
                    let root = PathBuf::from(root);
                    if let Ok(identity) = crate::path_identity::verify_directory(&root) {
                        if startup_objects.insert(identity.object_id) {
                            self.snapshot_scanner.startup_sweep_root(&state, &root);
                        }
                    }
                }
                let mut target_count = 0usize;
                for project in &startup_project_paths {
                    let Ok(targets) =
                        crate::config::teams::discover_verified_terminal_snapshot_targets(
                            std::slice::from_ref(project),
                        )
                    else {
                        continue;
                    };
                    for target in targets {
                        target_count = target_count.saturating_add(1);
                        if target_count > 4_096 {
                            break;
                        }
                        let Ok(identity) =
                            crate::path_identity::verify_directory(&target.replica_root)
                        else {
                            continue;
                        };
                        if startup_objects.insert(identity.object_id) {
                            self.snapshot_scanner
                                .startup_sweep_root(&state, &target.replica_root);
                        }
                    }
                    if target_count > 4_096 {
                        break;
                    }
                }
            }
            self.snapshot_scanner.finish_startup_sweep();
        }

        // Snapshot ingress derives exact replica roots before checking ordinary
        // outbox spellings, so a live session CWD below its replica cannot make
        // the dedicated protocol invisible. Physical roots are scanned once.
        let mut snapshot_root_objects = std::collections::HashSet::new();
        for (_, discovered) in &snapshot_session_dirs {
            let Some(requester_root) =
                crate::phone::terminal_snapshot::verified_requester_root_from_discovered_path(
                    Path::new(discovered),
                )
            else {
                continue;
            };
            let Ok(identity) = crate::path_identity::verify_directory(&requester_root) else {
                continue;
            };
            if snapshot_root_objects.insert(identity.object_id) {
                self.snapshot_scanner.scan_root(app, &requester_root);
            }
        }

        // Include the instance-private app outbox
        let app_outbox = app.state::<AppOutbox>();
        let app_outbox_path = app_outbox.path().to_string();

        // Collect all outbox directories to scan
        let mut outbox_dirs: Vec<PathBuf> = all_paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .join(crate::config::agent_local_dir_name())
                    .join("outbox")
            })
            .collect();
        // (#1399 R9) Dedup BEFORE the app outbox is pushed: `retain` keeps the
        // first occurrence, so running it over the full list could drop the
        // app entry in favour of an aliasing project spelling and silently
        // turn `is_app_outbox` false. This ordering makes that structurally
        // impossible.
        dedup_outbox_dirs_by_object_id(&mut outbox_dirs);
        outbox_dirs.push(PathBuf::from(&app_outbox_path));

        // #2232 phase 7: the Co-managed provenance queues of every room an
        // active session lives in. Each is its own scanned directory with its
        // own origin, never an outbox subdirectory (the sweep skips those).
        let mut queue_dirs: Vec<PathBuf> = Vec::new();
        for path in &all_paths {
            let Some(room_root) = crate::config::co_managed::room_root_for_path(Path::new(path))
            else {
                continue;
            };
            let queue = crate::config::co_managed::queue_dir(&room_root);
            if !queue_dirs.contains(&queue) {
                queue_dirs.push(queue);
            }
        }

        let mut scan_dirs: Vec<(PathBuf, OutboxOrigin)> = outbox_dirs
            .into_iter()
            .map(|dir| {
                let origin = if dir.as_path() == Path::new(&app_outbox_path) {
                    OutboxOrigin::Master
                } else {
                    OutboxOrigin::Replica
                };
                (dir, origin)
            })
            .collect();
        scan_dirs.extend(
            queue_dirs
                .into_iter()
                .map(|dir| (dir, OutboxOrigin::CoManaged)),
        );

        for (outbox_dir, origin) in &scan_dirs {
            if !outbox_dir.is_dir() {
                continue;
            }
            let is_app_outbox = *origin == OutboxOrigin::Master;
            if let Some(state) = app.try_state::<crate::api::message_store::MessageStoreState>() {
                if let Ok(store) = &state.store {
                    self.scrub_stale_pty_input_temps(outbox_dir, store);
                }
            }

            // (#1399) One walk partitions the outbox root by extension:
            // messages to scan and claim markers to reclaim. Deliberately
            // unbounded, exactly as today's scan is.
            let mut entries: Vec<PathBuf> = Vec::new();
            let mut claims: Vec<PathBuf> = Vec::new();
            match std::fs::read_dir(outbox_dir) {
                Ok(rd) => {
                    for path in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
                        // Skip files in subdirectories (delivered/, rejected/)
                        if path.parent() != Some(outbox_dir.as_path()) {
                            continue;
                        }
                        match path.extension().and_then(|s| s.to_str()) {
                            Some("json") => entries.push(path),
                            Some(WAKE_CLAIM_EXTENSION) => claims.push(path),
                            _ => {}
                        }
                    }
                }
                Err(_) => continue,
            }
            // (#1399 R2) Reclamation renames files a live worker may own, and
            // the ownership test is path-keyed, so it runs only where the
            // outbox identity is established: an unverifiable spelling of an
            // aliased outbox can never resurrect a live claim. An outbox that
            // fails verification cannot deliver anyway (the hardened reader
            // rejects its messages for the same reason), so nothing that works
            // today is lost. Reclaimed files are re-scanned on the NEXT cycle;
            // `entries` for this one is already collected.
            if crate::path_identity::verify_directory(outbox_dir).is_ok() {
                self.reclaim_unowned_wake_claims(outbox_dir, &claims);
            }

            // (#1399) Process a batch in send order. `read_dir` yields NTFS
            // filename order over random UUID names, so today's order is
            // arbitrary; once the per-target lane defers the rest of a
            // target's queue, that arbitrary order becomes the ADMISSION
            // decision, re-rolled every cycle, and an already-queued message
            // could be passed over indefinitely by newer arrivals whose names
            // sort lower. `sort_by_cached_key` reads each file at most once;
            // `None` sorts first so an unreadable document still reaches the
            // existing rejection path promptly; the path tie-breaker makes
            // equal timestamps deterministic. The length guard keeps the
            // steady state (zero or one message) at exactly zero extra reads.
            if entries.len() > 1 {
                entries.sort_by_cached_key(|path| (outbox_message_send_time(path), path.clone()));
            }

            for path in entries {
                messages_processed += 1;
                let standard_content = match classify_outbox_document(&path) {
                    OutboxClassification::PrivilegedCandidate { bytes, identity } => {
                        if *origin == OutboxOrigin::CoManaged {
                            // #2232 phase 7: the Co-managed origin may express
                            // exactly one thing, a wake carrying a pointer. A
                            // privileged PTY-input envelope is rejected before
                            // any authority resolution; the queue grants no
                            // other capability.
                            let _ = Self::reject_raw_file(
                                &path,
                                "Co-managed origin may carry only a wake; privileged PTY input \
                                    is rejected",
                            );
                            self.retry_tracker.remove(&path);
                            continue;
                        }
                        self.process_pty_input_file(app, &path, is_app_outbox, &bytes, &identity)
                            .await;
                        self.retry_tracker.remove(&path);
                        continue;
                    }
                    OutboxClassification::InvalidDocument => {
                        self.reject_malformed_pty_candidate(&path);
                        self.retry_tracker.remove(&path);
                        continue;
                    }
                    OutboxClassification::Standard(content) => content,
                };
                let mut deferred: Option<ValidatedWake> = None;
                let outcome = self
                    .process_message_content(
                        app,
                        &path,
                        *origin,
                        &standard_content,
                        Some(&mut deferred),
                    )
                    .await;
                match outcome {
                    Ok(()) => match deferred {
                        // Validated and handed off (or the lane was busy): the
                        // retry state for `path` must survive until the worker
                        // reports.
                        Some(msg) => match self.claim_and_spawn_validated_wake(app, &path, msg) {
                            WakeHandoff::Spawned => {}
                            WakeHandoff::LaneBusy => {}
                            WakeHandoff::ClaimFailed(e) => {
                                self.record_message_outcome(path, Err(e)).await
                            }
                        },
                        // Settled inline: delivered, rejected, or handled by an
                        // action branch.
                        None => self.record_message_outcome(path, Ok(())).await,
                    },
                    Err(e) => self.record_message_outcome(path, Err(e)).await,
                }
            }
        }

        log::info!(
            "[mailbox] poll cycle done elapsed_ms={} outboxes={} messages={}",
            cycle_started.elapsed().as_millis(),
            scan_dirs.len(),
            messages_processed
        );

        self.snapshot_scanner.finish_cycle();

        // (#1399) A live claim owns its retry state. Otherwise prune only after
        // fallible probes prove both durable spellings absent, so counted state
        // remains able to reach MAX_DELIVERY_ATTEMPTS.
        prune_retry_tracker(
            &mut self.retry_tracker,
            &self.live_claims,
            #[cfg(test)]
            None,
        );

        // Poll project-refresh-requests directory from create-agent-matrix CLI.
        self.poll_project_refresh_requests(app).await;

        // Poll session-requests directory (from create-agent CLI)
        self.poll_session_requests(app).await;

        // #786: poll coding-agent-requests (from `coding-agent` CLI mutations).
        self.poll_coding_agent_requests(app).await;

        Ok(())
    }

    /// (#1399) The retry/reject bookkeeping for one outbox message: `Ok`
    /// clears retry state, `Err` counts an attempt and rejects at the attempt
    /// ceiling. Called inline for a message the scanner settled itself, and
    /// from the outcome drain for a message a worker settled. Takes `path`
    /// owned so every expression in the block stays textually identical.
    async fn record_message_outcome(&mut self, path: PathBuf, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => {
                self.retry_tracker.remove(&path);
            }
            Err(e) => {
                // #2336 - the typing hold shares the menu guard's exemption: a
                // deferred wake is "not yet", never an attempt, and the file is
                // already back at its original path (the claim restore ran
                // before this bookkeeping).
                let held_by = if crate::pty::menu_guard::is_menu_guard_deferred_error(&e) {
                    Some("menu guard")
                } else if crate::pty::menu_guard::is_typing_hold_deferred_error(&e) {
                    Some("typing hold")
                } else if crate::api::message_store::is_response_closing_deferred(&e) {
                    Some("response close")
                } else {
                    None
                };
                if let Some(held_by) = held_by {
                    log::debug!(
                        "[mailbox] message delivery deferred by {} {:?}: {}",
                        held_by,
                        path,
                        e
                    );
                    return;
                }
                let is_permanent = is_permanent_delivery_error(&e);
                let should_reject = is_permanent || {
                    let state = self
                        .retry_tracker
                        .entry(path.clone())
                        .or_insert(RetryState {
                            attempt_count: 0,
                            logged: false,
                        });
                    state.attempt_count += 1;

                    if !state.logged {
                        log::warn!(
                            "Failed to process outbox message {:?} (attempt {}): {}",
                            path,
                            state.attempt_count,
                            e
                        );
                        state.logged = true;
                    } else {
                        log::debug!(
                            "Retry {} for outbox message {:?}: {}",
                            state.attempt_count,
                            path,
                            e
                        );
                    }

                    state.attempt_count >= MAX_DELIVERY_ATTEMPTS
                };

                if should_reject {
                    let reason = if is_permanent {
                        e.clone()
                    } else {
                        let attempts = self
                            .retry_tracker
                            .get(&path)
                            .map(|s| s.attempt_count)
                            .unwrap_or(0);
                        format!(
                            "Undeliverable after {} attempts. Last error: {}",
                            attempts, e
                        )
                    };

                    // §130-stuck-file: on read failure (e.g. non-UTF-8 non-BOM file),
                    // fall back to `reject_raw_file` so the file is moved to `rejected/`
                    // instead of looping forever with `attempt_count >= MAX`.
                    let rejected = match read_text_bom_tolerant(&path) {
                        Ok(content) => {
                            if let Ok(msg) = serde_json::from_str::<OutboxMessage>(&content) {
                                self.reject_message(&path, &msg, &reason).await.is_ok()
                            } else {
                                Self::reject_raw_file(&path, &reason).is_ok()
                            }
                        }
                        Err(_) => Self::reject_raw_file(&path, &reason).is_ok(),
                    };

                    if rejected {
                        self.retry_tracker.remove(&path);
                    } else {
                        log::error!("Failed to reject outbox message {:?}; will retry", path);
                    }
                }
            }
        }
    }

    fn reject_malformed_pty_candidate(&self, path: &Path) {
        use crate::phone::types::{
            PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason, PtyInputReasonCode,
            PtyInputResult,
        };
        let source_identity = match crate::path_identity::verify_regular_file(path) {
            Ok(identity) => identity,
            Err(_) => return,
        };
        // A malformed document does not independently prove that its filename
        // belongs to the claimed request. Always use a server ID so it cannot
        // replace a correlated terminal artifact for another operation.
        let injection_id = Uuid::new_v4().to_string();
        let mut result = PtyInputResult::new(injection_id.clone(), PtyInputPublicStatus::Rejected);
        result.terminal_at = Some(crate::phone::types::canonical_pty_timestamp(
            chrono::Utc::now(),
        ));
        result.reason = Some(PtyInputReason::from_code(
            PtyInputReasonCode::InvalidEnvelope,
        ));
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag: String::new(),
        };
        if let Err(code) = self.write_pty_input_artifact(path, &artifact) {
            log::warn!(
                "[pty-input] malformed candidate artifact failed id={} code={}",
                injection_id,
                code
            );
            return;
        }
        let current_source = match crate::path_identity::verify_regular_file(path) {
            Ok(identity) => identity,
            Err(_) => return,
        };
        if !crate::path_identity::same_object(&source_identity, &current_source) {
            return;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] malformed candidate cleanup failed id={} code=artifact_unclaimed",
                    injection_id
                );
            }
        }
    }

    async fn correlated_host_pty_metadata<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> Option<CorrelatedHostPtyMetadata> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputEnterMode,
            PtyInputHostFingerprint, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };

        let payload = &envelope.pty_input;
        let immutable_shape_valid = envelope.action == "pty-input"
            && envelope.body.is_empty()
            && envelope.mode == "wake"
            && !envelope.get_output
            && envelope.preferred_agent.is_empty()
            && envelope.priority == "normal"
            && payload.version == PTY_INPUT_VERSION
            && payload.enter == PtyInputEnterMode::AgentSubmit
            && envelope.id == payload.injection_id
            && payload.op_id == payload.injection_id
            && path.file_stem().and_then(|value| value.to_str())
                == Some(payload.injection_id.as_str());
        if !immutable_shape_valid {
            return None;
        }
        let injection_id = parse_canonical_uuid_v4(&payload.injection_id).ok()?;
        let nonce = parse_canonical_uuid_v4(&payload.nonce).ok()?;
        if injection_id == nonce
            || parse_canonical_uuid_v4(&envelope.token).is_err()
            || crate::pty::inject::validate_pty_input_text(&payload.text).is_err()
        {
            return None;
        }
        let issued = parse_canonical_pty_timestamp(&payload.issued_at).ok()?;
        let expires = parse_canonical_pty_timestamp(&payload.expires_at).ok()?;
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return None;
        }

        let outbox = path.parent()?;
        let root = outbox.parent().and_then(Path::parent)?;
        let owner = self.verified_pty_outbox_owner(path).ok()?;
        let sender_is_root = crate::config::root_agent::verify_live_root_agent_path(root).is_ok();
        let sender_project = if sender_is_root {
            None
        } else {
            let sender = crate::config::teams::verify_pty_input_coordinator_root(root).ok()?;
            sender
                .ac_root_identity
                .canonical_path
                .parent()
                .and_then(|path| {
                    path.to_str()
                        .map(crate::path_utils::normalize_windows_verbatim_path)
                })
        };
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            crate::config::settings::read_pty_input_project_paths_strict_offloaded()
                .await
                .ok()?
                .unwrap_or(in_memory_paths);
        if let Some(project) = sender_project {
            if !project_paths.contains(&project) {
                project_paths.push(project);
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            root,
            sender_is_root,
            &envelope.to,
            &project_paths,
        )
        .ok()?;
        if owner != route.sender.canonical_fqn || envelope.from != route.sender.canonical_fqn {
            return None;
        }

        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let request_fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
            injection_id: &payload.injection_id,
            op_id: &payload.op_id,
            token: &envelope.token,
            sender_fqn: &route.sender.canonical_fqn,
            target_fqn: &route.target.canonical_fqn,
            nonce: &payload.nonce,
            issued_at: &payload.issued_at,
            expires_at: &payload.expires_at,
            text: &payload.text,
            agent_id: payload.agent_id.as_deref(),
            confirmation_tag: &confirmation_tag,
        });
        Some(CorrelatedHostPtyMetadata {
            route,
            confirmation_tag,
            request_fingerprint,
            payload_sha256: sha256_hex(payload.text.as_bytes()),
        })
    }

    async fn reject_unavailable_store_pty_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> bool {
        use crate::phone::types::{
            PtyInputHostArtifact, PtyInputPublicStatus, PtyInputReason, PtyInputReasonCode,
            PtyInputResult, PtyInputSourcePlane,
        };
        let Some(metadata) = self.correlated_host_pty_metadata(app, path, envelope).await else {
            return false;
        };
        let payload = &envelope.pty_input;
        debug_assert_eq!(metadata.request_fingerprint.len(), 64);
        let mut result =
            PtyInputResult::new(payload.injection_id.clone(), PtyInputPublicStatus::Rejected);
        result.op_id = Some(payload.op_id.clone());
        result.sender = Some(metadata.route.sender.canonical_fqn);
        result.target = Some(metadata.route.target.canonical_fqn);
        result.payload_bytes = Some(payload.text.len() as u64);
        result.payload_sha256 = Some(metadata.payload_sha256);
        result.source_plane = Some(PtyInputSourcePlane::HostCli);
        result.issued_at = Some(payload.issued_at.clone());
        result.expires_at = Some(payload.expires_at.clone());
        result.queued_at = Some(payload.issued_at.clone());
        let issued = crate::phone::types::parse_canonical_pty_timestamp(&payload.issued_at)
            .unwrap_or_else(|_| chrono::Utc::now());
        result.terminal_at = Some(crate::phone::types::canonical_pty_timestamp(
            chrono::Utc::now().max(issued),
        ));
        result.reason = Some(PtyInputReason::from_code(PtyInputReasonCode::StoreCorrupt));
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag: metadata.confirmation_tag,
        };
        if self.write_pty_input_artifact(path, &artifact).is_err() {
            return true;
        }
        let current_source = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .ok()
        .map(|(_, identity)| identity);
        if !current_source.as_ref().is_some_and(|current| {
            crate::path_identity::same_object(source_identity, current)
                && source_identity.content_sha256 == current.content_sha256
        }) {
            return true;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] unavailable-store source cleanup failed id={} \
                    code=artifact_unclaimed",
                    payload.injection_id
                );
            }
        }
        true
    }

    async fn reject_correlated_host_pty_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        store: &crate::api::message_store::MessageStore,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
        code: crate::phone::types::PtyInputReasonCode,
    ) -> bool {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputEnterMode,
            PtyInputHostArtifact, PtyInputHostFingerprint, PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };

        let payload = &envelope.pty_input;
        let immutable_shape_valid = envelope.action == "pty-input"
            && envelope.body.is_empty()
            && envelope.mode == "wake"
            && !envelope.get_output
            && envelope.preferred_agent.is_empty()
            && envelope.priority == "normal"
            && payload.version == PTY_INPUT_VERSION
            && payload.enter == PtyInputEnterMode::AgentSubmit
            && envelope.id == payload.injection_id
            && payload.op_id == payload.injection_id
            && path.file_stem().and_then(|value| value.to_str())
                == Some(payload.injection_id.as_str());
        if !immutable_shape_valid {
            return false;
        }
        let Ok(injection_id) = parse_canonical_uuid_v4(&payload.injection_id) else {
            return false;
        };
        let Ok(nonce) = parse_canonical_uuid_v4(&payload.nonce) else {
            return false;
        };
        if injection_id == nonce
            || parse_canonical_uuid_v4(&envelope.token).is_err()
            || crate::pty::inject::validate_pty_input_text(&payload.text).is_err()
        {
            return false;
        }
        let Ok(issued) = parse_canonical_pty_timestamp(&payload.issued_at) else {
            return false;
        };
        let Ok(expires) = parse_canonical_pty_timestamp(&payload.expires_at) else {
            return false;
        };
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return false;
        }

        let Some(outbox) = path.parent() else {
            return false;
        };
        let Some(root) = outbox.parent().and_then(Path::parent) else {
            return false;
        };
        let owner = match self.verified_pty_outbox_owner(path) {
            Ok(owner) => owner,
            Err(_) => return false,
        };
        let sender_is_root = crate::config::root_agent::verify_live_root_agent_path(root).is_ok();
        let sender_project = if sender_is_root {
            None
        } else {
            let sender = match crate::config::teams::verify_pty_input_coordinator_root(root) {
                Ok(sender) => sender,
                Err(_) => return false,
            };
            sender
                .ac_root_identity
                .canonical_path
                .parent()
                .and_then(|path| {
                    path.to_str()
                        .map(crate::path_utils::normalize_windows_verbatim_path)
                })
        };
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return false,
            };
        if let Some(project) = sender_project {
            if !project_paths.contains(&project) {
                project_paths.push(project);
            }
        }
        let route = match crate::config::teams::verify_pty_input_route(
            root,
            sender_is_root,
            &envelope.to,
            &project_paths,
        ) {
            Ok(route) => route,
            Err(_) => return false,
        };
        if owner != route.sender.canonical_fqn || envelope.from != route.sender.canonical_fqn {
            return false;
        }

        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let request_fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
            injection_id: &payload.injection_id,
            op_id: &payload.op_id,
            token: &envelope.token,
            sender_fqn: &route.sender.canonical_fqn,
            target_fqn: &route.target.canonical_fqn,
            nonce: &payload.nonce,
            issued_at: &payload.issued_at,
            expires_at: &payload.expires_at,
            text: &payload.text,
            agent_id: payload.agent_id.as_deref(),
            confirmation_tag: &confirmation_tag,
        });
        let result = match store
            .record_host_pty_input_rejection_offloaded(
                crate::api::message_store::HostPtyInputRejectionRequest {
                    injection_id: payload.injection_id.clone(),
                    sender_fqn: route.sender.canonical_fqn,
                    target_fqn: route.target.canonical_fqn,
                    op_id: payload.op_id.clone(),
                    nonce_sha256: sha256_hex(payload.nonce.as_bytes()),
                    request_fingerprint,
                    confirmation_tag: confirmation_tag.clone(),
                    sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
                    payload_sha256: sha256_hex(payload.text.as_bytes()),
                    payload_bytes: payload.text.len() as u64,
                    issued_at: payload.issued_at.clone(),
                    expires_at: payload.expires_at.clone(),
                    reason: code,
                },
                chrono::Utc::now(),
            )
            .await
        {
            Ok(result) => result,
            Err(crate::api::message_store::MessageStoreError::IdempotencyConflict) => {
                return false;
            }
            Err(_) => {
                log::debug!(
                    "[pty-input] correlated ingress persistence deferred id={} \
                    code=store_transient",
                    payload.injection_id
                );
                return true;
            }
        };
        let artifact = PtyInputHostArtifact {
            result,
            confirmation_tag,
        };
        if let Err(artifact_code) = self.write_pty_input_artifact(path, &artifact) {
            log::warn!(
                "[pty-input] correlated ingress artifact failed id={} code={}",
                payload.injection_id,
                artifact_code
            );
            return true;
        }
        let current_source = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .ok()
        .map(|(_, identity)| identity);
        if !current_source.as_ref().is_some_and(|current| {
            crate::path_identity::same_object(source_identity, current)
                && source_identity.content_sha256 == current.content_sha256
        }) {
            return true;
        }
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[pty-input] correlated ingress cleanup failed id={} code=artifact_unclaimed",
                    payload.injection_id
                );
            }
        }
        true
    }

    fn write_pty_input_artifact(
        &self,
        source: &Path,
        artifact: &crate::phone::types::PtyInputHostArtifact,
    ) -> Result<(), &'static str> {
        let directory = match artifact.result.status {
            crate::phone::types::PtyInputPublicStatus::Injected => "delivered",
            crate::phone::types::PtyInputPublicStatus::Rejected => "rejected",
            crate::phone::types::PtyInputPublicStatus::Indeterminate => "indeterminate",
            _ => return Err("invalid_artifact"),
        };
        let outbox = source.parent().ok_or("unsafe_path")?;
        crate::path_identity::verify_directory(outbox).map_err(|_| "unsafe_path")?;
        let artifact_dir = outbox.join(directory);
        if !artifact_dir.exists() {
            std::fs::create_dir(&artifact_dir).map_err(|_| "unsafe_path")?;
        }
        crate::path_identity::verify_directory(&artifact_dir).map_err(|_| "unsafe_path")?;
        let bytes = serde_json::to_vec(artifact).map_err(|_| "invalid_artifact")?;
        if bytes.len() > crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES {
            return Err("invalid_artifact");
        }
        let destination = artifact_dir.join(format!("{}.json", artifact.result.injection_id));
        write_atomic_pty_metadata(&destination, &bytes)?;
        if artifact.result.status == crate::phone::types::PtyInputPublicStatus::Rejected {
            if let Some(reason) = &artifact.result.reason {
                let reason_path =
                    artifact_dir.join(format!("{}.reason.txt", artifact.result.injection_id));
                let fixed = format!("{}: {}", reason_code_name(reason.code), reason.detail);
                write_atomic_pty_metadata(&reason_path, fixed.as_bytes())?;
            }
        }
        Ok(())
    }

    async fn materialize_host_terminal_artifact(
        &self,
        path: &Path,
        store: &crate::api::message_store::MessageStore,
        injection_id: &str,
    ) -> Result<(), &'static str> {
        let (marker, source_identity, owner) =
            self.read_verified_host_marker(path, injection_id)?;
        let result = store
            .query_pty_input_by_injection_offloaded(injection_id.to_string())
            .await
            .map_err(|_| "store_transient")?
            .ok_or("store_corrupt")?;
        if result.source_plane != Some(crate::phone::types::PtyInputSourcePlane::HostCli)
            || result.op_id.as_deref() != Some(marker.op_id.as_str())
            || result.sender.as_deref() != Some(owner.as_str())
        {
            return Err("store_corrupt");
        }
        if !result.terminal {
            return Ok(());
        }
        let confirmation_tag = store
            .host_confirmation_tag_offloaded(injection_id.to_string())
            .await
            .map_err(|_| "store_transient")?
            .ok_or("store_corrupt")?;
        let artifact = crate::phone::types::PtyInputHostArtifact {
            result,
            confirmation_tag,
        };
        self.write_pty_input_artifact(path, &artifact)?;
        let (_, current_source) = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        if !crate::path_identity::same_object(&source_identity, &current_source)
            || source_identity.content_sha256 != current_source.content_sha256
        {
            return Err("unsafe_path");
        }
        // Record the published artifact before deleting the marker. If this
        // update or the later deletion fails, the retained marker drives an
        // idempotent repair on the next poll.
        store
            .mark_host_artifact_offloaded(injection_id.to_string(), chrono::Utc::now())
            .await
            .map_err(|_| "store_transient")?;
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("artifact_unclaimed");
            }
        }
        Ok(())
    }

    fn replace_host_request_with_marker(
        &self,
        path: &Path,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        injection_id: &str,
        op_id: &str,
    ) -> Result<(), &'static str> {
        let marker = crate::phone::types::PtyInputQueueMarker {
            kind: "pty-input-marker".to_string(),
            version: crate::phone::types::PTY_INPUT_VERSION,
            injection_id: injection_id.to_string(),
            op_id: op_id.to_string(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| "invalid_envelope")?;
        if path.file_stem().and_then(|value| value.to_str()) != Some(injection_id) {
            return Err("unsafe_path");
        }
        crate::path_identity::replace_regular_file_atomic(
            path,
            source_identity,
            &bytes,
            crate::phone::types::PTY_INPUT_HOST_ENVELOPE_MAX_BYTES,
        )
        .map(|_| ())
        .map_err(|code| {
            if code == "unsafe_path" {
                "unsafe_path"
            } else {
                "store_transient"
            }
        })
    }

    fn read_verified_host_marker(
        &self,
        path: &Path,
        expected_injection_id: &str,
    ) -> Result<
        (
            crate::phone::types::PtyInputQueueMarker,
            crate::path_identity::VerifiedPathIdentity,
            String,
        ),
        &'static str,
    > {
        let (bytes, identity) = crate::path_identity::read_bounded_regular(
            path,
            crate::phone::types::PTY_INPUT_METADATA_MAX_BYTES,
        )
        .map_err(|_| "unsafe_path")?;
        let value = crate::path_identity::parse_json_no_duplicates(&bytes)
            .map_err(|_| "invalid_artifact")?;
        let marker: crate::phone::types::PtyInputQueueMarker =
            serde_json::from_value(value).map_err(|_| "invalid_artifact")?;
        if marker.kind != "pty-input-marker"
            || marker.version != crate::phone::types::PTY_INPUT_VERSION
            || marker.injection_id != expected_injection_id
            || marker.op_id != marker.injection_id
            || path.file_stem().and_then(|value| value.to_str())
                != Some(marker.injection_id.as_str())
        {
            return Err("invalid_artifact");
        }
        let owner = self
            .verified_pty_outbox_owner(path)
            .map_err(|_| "unsafe_path")?;
        Ok((marker, identity, owner))
    }

    fn verified_pty_outbox_owner(
        &self,
        path: &Path,
    ) -> Result<String, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        let outbox = path.parent().ok_or(C::UnsafePath)?;
        if outbox.file_name().and_then(|value| value.to_str()) != Some("outbox") {
            return Err(C::UnsafePath);
        }
        let local = outbox.parent().ok_or(C::UnsafePath)?;
        let local_dir_name = crate::config::agent_local_dir_name();
        if local.file_name().and_then(|value| value.to_str()) != Some(local_dir_name.as_str()) {
            return Err(C::UnsafePath);
        }
        let root = local.parent().ok_or(C::UnsafePath)?;
        let actual_outbox =
            crate::path_identity::verify_directory(outbox).map_err(|_| C::UnsafePath)?;
        let expected_outbox = crate::path_identity::verify_directory(
            &root
                .join(crate::config::agent_local_dir_name())
                .join("outbox"),
        )
        .map_err(|_| C::UnsafePath)?;
        if !crate::path_identity::same_object(&actual_outbox, &expected_outbox) {
            return Err(C::UnsafePath);
        }
        if let Ok(root_identity) = crate::config::root_agent::verify_live_root_agent_path(root) {
            let actual_root =
                crate::path_identity::verify_directory(root).map_err(|_| C::UnsafePath)?;
            if !crate::path_identity::same_object(&root_identity, &actual_root) {
                return Err(C::UnsafePath);
            }
            return Ok(crate::config::root_agent::ROOT_AGENT_SENDER.to_string());
        }
        crate::config::teams::verify_pty_input_replica_cwd(root)
            .map(|identity| identity.canonical_fqn)
            .map_err(|_| C::UnsafePath)
    }

    async fn process_pty_input_file<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        is_app_outbox: bool,
        bytes: &[u8],
        source_identity: &crate::path_identity::VerifiedPathIdentity,
    ) {
        if bytes.starts_with(&[0xef, 0xbb, 0xbf]) || std::str::from_utf8(bytes).is_err() {
            self.reject_malformed_pty_candidate(path);
            return;
        }
        let value = match crate::path_identity::parse_json_no_duplicates(bytes) {
            Ok(value) => value,
            Err(_) => {
                self.reject_malformed_pty_candidate(path);
                return;
            }
        };
        let state = app.try_state::<crate::api::message_store::MessageStoreState>();

        if value.get("kind").and_then(serde_json::Value::as_str) == Some("pty-input-marker") {
            let Some(state) = state else {
                log::debug!("[pty-input] marker deferred code=store_transient");
                return;
            };
            let store = match &state.store {
                Ok(store) => Arc::clone(store),
                Err(_) => {
                    log::debug!("[pty-input] marker deferred code=store_transient");
                    return;
                }
            };
            let marker: crate::phone::types::PtyInputQueueMarker =
                match serde_json::from_value(value) {
                    Ok(marker) => marker,
                    Err(_) => {
                        self.reject_malformed_pty_candidate(path);
                        return;
                    }
                };
            if marker.version != crate::phone::types::PTY_INPUT_VERSION
                || crate::phone::types::parse_canonical_uuid_v4(&marker.injection_id).is_err()
                || marker.injection_id != marker.op_id
                || path.file_stem().and_then(|value| value.to_str())
                    != Some(marker.injection_id.as_str())
                || is_app_outbox
            {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            let (current_marker, _marker_identity, owner) =
                match self.read_verified_host_marker(path, &marker.injection_id) {
                    Ok(marker) => marker,
                    Err(_) => {
                        self.reject_malformed_pty_candidate(path);
                        return;
                    }
                };
            if current_marker != marker {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            let row = match store
                .query_pty_input_by_injection_offloaded(marker.injection_id.clone())
                .await
            {
                Ok(Some(row)) => row,
                Ok(None) => {
                    self.reject_malformed_pty_candidate(path);
                    return;
                }
                Err(_) => {
                    log::debug!(
                        "[pty-input] marker status deferred id={} code=store_transient",
                        marker.injection_id
                    );
                    return;
                }
            };
            if row.source_plane != Some(crate::phone::types::PtyInputSourcePlane::HostCli)
                || row.op_id.as_deref() != Some(marker.op_id.as_str())
                || row.sender.as_deref() != Some(owner.as_str())
            {
                self.reject_malformed_pty_candidate(path);
                return;
            }
            if row.terminal {
                if let Err(code) = self
                    .materialize_host_terminal_artifact(path, &store, &marker.injection_id)
                    .await
                {
                    log::warn!(
                        "[pty-input] artifact repair failed id={} code={}",
                        marker.injection_id,
                        code
                    );
                }
                return;
            }
            self.dispatch_pty_input_operation(
                app,
                &state,
                &marker.injection_id,
                crate::phone::types::PtyInputSourcePlane::HostCli,
                None,
            )
            .await;
            if let Err(code) = self
                .materialize_host_terminal_artifact(path, &store, &marker.injection_id)
                .await
            {
                if code != "store_corrupt" {
                    log::debug!(
                        "[pty-input] marker remains id={} code={}",
                        marker.injection_id,
                        code
                    );
                }
            }
            return;
        }

        let envelope: crate::phone::types::PtyInputHostEnvelope =
            match serde_json::from_value(value) {
                Ok(envelope) => envelope,
                Err(_) => {
                    self.reject_malformed_pty_candidate(path);
                    return;
                }
            };
        let Some(state) = state else {
            if !self
                .reject_unavailable_store_pty_candidate(app, path, source_identity, &envelope)
                .await
            {
                self.reject_malformed_pty_candidate(path);
            }
            return;
        };
        let store = match &state.store {
            Ok(store) => Arc::clone(store),
            Err(_) => {
                if !self
                    .reject_unavailable_store_pty_candidate(app, path, source_identity, &envelope)
                    .await
                {
                    self.reject_malformed_pty_candidate(path);
                }
                return;
            }
        };
        let validation = self
            .validate_and_enqueue_host_pty_input(
                app,
                path,
                is_app_outbox,
                source_identity,
                &store,
                &envelope,
            )
            .await;
        let injection_id = match validation {
            Ok(injection_id) => injection_id,
            Err(code) => {
                if code == crate::phone::types::PtyInputReasonCode::StoreTransient {
                    log::debug!(
                        "[pty-input] host ingress deferred code={}",
                        reason_code_name(code)
                    );
                    return;
                }
                log::warn!(
                    "[pty-input] host ingress rejected code={}",
                    reason_code_name(code)
                );
                crate::api::audit::record_pty_input(&crate::api::audit::PtyInputAuditMetadata {
                    event: "ingress_rejection".to_string(),
                    injection_id: None,
                    op_id: None,
                    sender_fqn: None,
                    target_fqn: None,
                    payload_bytes: None,
                    payload_sha256: None,
                    source_plane: Some("host_cli".to_string()),
                    selected_session_id: None,
                    selected_backend: None,
                    status: "rejected".to_string(),
                    reason_code: Some(reason_code_name(code).to_string()),
                    timestamp: crate::phone::types::canonical_pty_timestamp(chrono::Utc::now()),
                });
                if !self
                    .reject_correlated_host_pty_candidate(
                        app,
                        path,
                        source_identity,
                        &store,
                        &envelope,
                        code,
                    )
                    .await
                {
                    self.reject_malformed_pty_candidate(path);
                }
                return;
            }
        };
        self.dispatch_pty_input_operation(
            app,
            &state,
            &injection_id,
            crate::phone::types::PtyInputSourcePlane::HostCli,
            None,
        )
        .await;
        if let Err(code) = self
            .materialize_host_terminal_artifact(path, &store, &injection_id)
            .await
        {
            log::debug!(
                "[pty-input] terminal artifact pending id={} code={}",
                injection_id,
                code
            );
        }
    }

    async fn validate_and_enqueue_host_pty_input<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        is_app_outbox: bool,
        source_identity: &crate::path_identity::VerifiedPathIdentity,
        store: &crate::api::message_store::MessageStore,
        envelope: &crate::phone::types::PtyInputHostEnvelope,
    ) -> Result<String, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::{
            parse_canonical_pty_timestamp, parse_canonical_uuid_v4, pty_input_confirmation_tag,
            pty_input_host_request_fingerprint, sha256_hex, PtyInputHostFingerprint,
            PtyInputReasonCode as C, PtyInputSourcePlane, PTY_INPUT_FUTURE_SKEW_SECS,
            PTY_INPUT_TTL_SECS, PTY_INPUT_VERSION,
        };
        if is_app_outbox {
            return Err(C::UnsafePath);
        }
        let payload = &envelope.pty_input;
        if envelope.action != "pty-input"
            || !envelope.body.is_empty()
            || envelope.mode != "wake"
            || envelope.get_output
            || !envelope.preferred_agent.is_empty()
            || envelope.priority != "normal"
        {
            return Err(C::MixedPayload);
        }
        if payload.version != PTY_INPUT_VERSION {
            return Err(C::UnsupportedVersion);
        }
        if payload.enter != crate::phone::types::PtyInputEnterMode::AgentSubmit {
            return Err(C::InvalidEnterMode);
        }
        let injection = parse_canonical_uuid_v4(&payload.injection_id).map_err(|_| C::InvalidId)?;
        parse_canonical_uuid_v4(&payload.op_id).map_err(|_| C::InvalidId)?;
        let nonce = parse_canonical_uuid_v4(&payload.nonce).map_err(|_| C::InvalidNonce)?;
        if envelope.id != payload.injection_id
            || payload.op_id != payload.injection_id
            || injection == nonce
            || path.file_stem().and_then(|value| value.to_str())
                != Some(payload.injection_id.as_str())
        {
            return Err(C::InvalidId);
        }
        let issued = parse_canonical_pty_timestamp(&payload.issued_at)?;
        let expires = parse_canonical_pty_timestamp(&payload.expires_at)?;
        if envelope.timestamp != payload.issued_at
            || expires - issued != chrono::Duration::seconds(PTY_INPUT_TTL_SECS)
        {
            return Err(C::InvalidTimestamp);
        }
        let now = chrono::Utc::now();
        if issued > now + chrono::Duration::seconds(PTY_INPUT_FUTURE_SKEW_SECS) {
            return Err(C::InvalidTimestamp);
        }
        if expires <= now {
            return Err(C::Expired);
        }
        crate::pty::inject::validate_pty_input_text(&payload.text).map_err(|error| {
            if error.kind == crate::pty::inject::PtyInputTextErrorKind::TooLarge {
                C::PayloadTooLarge
            } else {
                C::InvalidText
            }
        })?;
        if let Some(agent_id) = payload.agent_id.as_deref() {
            if agent_id == "auto"
                || crate::config::coding_agent_mutations::validate_custom_agent_id(agent_id)
                    .is_err()
            {
                return Err(C::UnsupportedProfile);
            }
        }
        let token = parse_canonical_uuid_v4(&envelope.token).map_err(|_| C::InvalidSessionToken)?;
        let session_manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let session = {
            let manager = session_manager.read().await;
            manager
                .find_unique_live_by_token(token)
                .await
                .map_err(|error| match error {
                    crate::session::manager::UniqueLiveTokenError::NotFound => {
                        C::InvalidSessionToken
                    }
                    crate::session::manager::UniqueLiveTokenError::Ambiguous => {
                        C::AmbiguousSessionToken
                    }
                })?
        };
        if session.backend_kind != SessionBackendKind::LocalProcess {
            return Err(C::SenderBackendNotLocal);
        }
        let session_id = Uuid::parse_str(&session.id).map_err(|_| C::SenderSessionNotLive)?;
        let pty = app.state::<Arc<Mutex<PtyManager>>>();
        if pty
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .backend_kind(session_id)
            != Some(SessionBackendKind::LocalProcess)
        {
            return Err(C::SenderSessionNotLive);
        }
        let in_memory_project_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut project_paths =
            crate::config::settings::read_pty_input_project_paths_strict_offloaded()
                .await
                .map_err(|_| C::UnsafePath)?
                .unwrap_or(in_memory_project_paths);
        if let Some(replica) = Path::new(&session.working_directory)
            .ancestors()
            .find(|candidate| {
                candidate
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("__agent_"))
            })
        {
            if let Some(project) = replica
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
            {
                let project_s = project
                    .to_str()
                    .map(crate::path_utils::normalize_windows_verbatim_path)
                    .ok_or(C::UnsafePath)?;
                if !project_paths.contains(&project_s) {
                    project_paths.push(project_s);
                }
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&session.working_directory),
            session.is_root_agent,
            &envelope.to,
            &project_paths,
        )
        .map_err(|code| reason_code_from_name(&code))?;
        if envelope.from != route.sender.canonical_fqn {
            return Err(C::SenderIdentityInvalid);
        }
        let expected_outbox = Path::new(&session.working_directory)
            .join(crate::config::agent_local_dir_name())
            .join("outbox");
        let actual_parent = path.parent().ok_or(C::UnsafePath)?;
        let expected_identity =
            crate::path_identity::verify_directory(&expected_outbox).map_err(|_| C::UnsafePath)?;
        let actual_identity =
            crate::path_identity::verify_directory(actual_parent).map_err(|_| C::UnsafePath)?;
        if expected_identity.object_id != actual_identity.object_id {
            return Err(C::UnsafePath);
        }
        let confirmation_tag =
            pty_input_confirmation_tag(&payload.injection_id, &payload.op_id, &payload.nonce);
        let nonce_sha256 = sha256_hex(payload.nonce.as_bytes());
        let fingerprint = pty_input_host_request_fingerprint(&PtyInputHostFingerprint {
            injection_id: &payload.injection_id,
            op_id: &payload.op_id,
            token: &envelope.token,
            sender_fqn: &route.sender.canonical_fqn,
            target_fqn: &route.target.canonical_fqn,
            nonce: &payload.nonce,
            issued_at: &payload.issued_at,
            expires_at: &payload.expires_at,
            text: &payload.text,
            agent_id: payload.agent_id.as_deref(),
            confirmation_tag: &confirmation_tag,
        });
        let enqueue = store
            .enqueue_pty_input_offloaded(crate::api::message_store::PtyInputEnqueueRequest {
                injection_id: payload.injection_id.clone(),
                sender_fqn: route.sender.canonical_fqn,
                target_fqn: route.target.canonical_fqn,
                op_id: payload.op_id.clone(),
                nonce_sha256,
                request_fingerprint: fingerprint.clone(),
                confirmation_tag: Some(confirmation_tag),
                requested_agent_id: payload.agent_id.clone(),
                payload: payload.text.as_bytes().to_vec(),
                source_plane: PtyInputSourcePlane::HostCli,
                sender_incarnation_fingerprint: route.sender.incarnation_fingerprint,
                sender_identity_fingerprint: route.sender.authority_fingerprint,
                target_identity_fingerprint: route.target.authority_fingerprint,
                authority_session_id: session.id,
                authority_client_id: None,
                authority_client_generation: None,
                issued_at: payload.issued_at.clone(),
                expires_at: payload.expires_at.clone(),
            })
            .await
            .map_err(|error| match error {
                crate::api::message_store::MessageStoreError::IdempotencyConflict => {
                    C::IdempotencyConflict
                }
                crate::api::message_store::MessageStoreError::CapacityExceeded => {
                    C::CapacityExceeded
                }
                _ => C::StoreTransient,
            })?;
        self.replace_host_request_with_marker(
            path,
            source_identity,
            &payload.injection_id,
            &payload.op_id,
        )
        .map_err(|_| C::StoreTransient)?;
        Ok(enqueue.result.injection_id)
    }

    async fn validate_claimed_host_authority<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
    ) -> Option<crate::config::teams::VerifiedPtyInputRoute> {
        let authority_id =
            crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id).ok()?;
        let authority = {
            let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = manager.read().await;
            guard.get_session(authority_id).await
        }?;
        if matches!(authority.status, SessionStatus::Exited(_))
            || authority.backend_kind != SessionBackendKind::LocalProcess
        {
            return None;
        }
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return None,
            };
        if let Ok(sender) = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
            &authority.working_directory,
        )) {
            if let Some(project_path) = sender.ac_root_identity.canonical_path.parent() {
                let project = project_path
                    .to_str()
                    .map(crate::path_utils::normalize_windows_verbatim_path)?;
                if !paths.contains(&project) {
                    paths.push(project);
                }
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&authority.working_directory),
            authority.is_root_agent,
            &claimed.target_fqn,
            &paths,
        )
        .ok()?;
        if route.sender.canonical_fqn != claimed.sender_fqn
            || route.sender.authority_fingerprint != claimed.sender_identity_fingerprint
            || route.target.authority_fingerprint != claimed.target_identity_fingerprint
        {
            return None;
        }
        let current_cwd =
            crate::path_identity::verify_directory(Path::new(&authority.working_directory)).ok()?;
        let pty = app.state::<Arc<Mutex<PtyManager>>>();
        let manager = pty.lock().unwrap_or_else(|error| error.into_inner());
        if !manager.has_session(authority_id)
            || manager.backend_kind(authority_id) != Some(SessionBackendKind::LocalProcess)
        {
            return None;
        }
        let (saved_cwd, saved_replica, _) = manager.route_identities(authority_id)?;
        if !saved_cwd
            .as_ref()
            .is_some_and(|saved| crate::path_identity::same_object(saved, &current_cwd))
        {
            return None;
        }
        match route.kind {
            crate::config::teams::PtyInputAuthorityKind::Coordinator => {
                if !saved_replica.as_ref().is_some_and(|saved| {
                    crate::path_identity::same_object(saved, &route.sender.replica_identity)
                }) {
                    return None;
                }
            }
            crate::config::teams::PtyInputAuthorityKind::Root => {
                if saved_replica.is_some() {
                    return None;
                }
            }
        }
        drop(manager);
        Some(route)
    }

    async fn validate_selected_pty_target<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        session_id: Uuid,
        expected_backend: SessionBackendKind,
    ) -> Option<SessionInfo> {
        let expires =
            crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at).ok()?;
        if expires <= chrono::Utc::now()
            || app
                .try_state::<Arc<crate::RestoreInProgress>>()
                .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
            || app
                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
        {
            return None;
        }
        let session = {
            let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = manager.read().await;
            guard.get_session(session_id).await
        }?;
        let settings = app.state::<SettingsState>().read().await.clone();
        if matches!(session.status, SessionStatus::Exited(_))
            || !session.waiting_for_input
            || session.backend_kind != expected_backend
            || !session_has_current_pty_submission_provenance(
                &SessionInfo::from(&session),
                &settings,
            )
        {
            return None;
        }
        let current_target = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
            &session.working_directory,
        ))
        .ok()?;
        if current_target.canonical_fqn != verified_target.canonical_fqn
            || current_target.authority_fingerprint != verified_target.authority_fingerprint
            || !crate::path_identity::same_object(
                &current_target.replica_identity,
                &verified_target.replica_identity,
            )
        {
            return None;
        }
        let current_cwd =
            crate::path_identity::verify_directory(Path::new(&session.working_directory)).ok()?;
        let route_valid = {
            let manager = app.state::<Arc<Mutex<PtyManager>>>();
            let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
            manager.has_session(session_id)
                && manager.backend_kind(session_id) == Some(expected_backend)
                && manager
                    .route_identities(session_id)
                    .is_some_and(|(cwd, replica, _)| {
                        cwd.as_ref().is_some_and(|saved| {
                            crate::path_identity::same_object(saved, &current_cwd)
                        }) && replica.as_ref().is_some_and(|saved| {
                            crate::path_identity::same_object(
                                saved,
                                &verified_target.replica_identity,
                            )
                        })
                    })
        };
        if !route_valid {
            return None;
        }
        let readiness = app
            .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
            .purge_readiness(&[session_id])
            .into_iter()
            .next()?;
        let activity_required = readiness
            .idle_threshold
            .checked_add(std::time::Duration::from_secs(2))?;
        let activity_ready = readiness
            .activity_age
            .is_some_and(|age| age >= activity_required);
        let resize_ready = match readiness.last_resize_age {
            None => true,
            Some(age) => readiness
                .resize_grace
                .checked_add(std::time::Duration::from_secs(2))
                .is_some_and(|required| age >= required),
        };
        if !activity_ready || !resize_ready || !readiness.watcher_idle {
            return None;
        }
        Some(SessionInfo::from(&session))
    }

    pub(crate) async fn dispatch_pty_input_operation<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.before_source_accept(injection_id).await {
                return;
            }
        }
        let delivery = self.delivery_poller();
        let app = app.clone();
        let injection_id = injection_id.to_owned();
        let api_client_store = api_client_store.cloned();
        let state = crate::api::message_store::MessageStoreState {
            store: state.store.clone(),
            active_operations: state.active_operations.clone(),
            target_locks: state.target_locks.clone(),
            target_gate: state.target_gate.clone(),
        };
        crate::config::sessions_persistence::run_owned_persist_core(async move {
            delivery
                .dispatch_pty_input_operation_core(
                    &app,
                    &state,
                    &injection_id,
                    source_plane,
                    api_client_store.as_ref(),
                )
                .await;
        })
        .await;
    }

    async fn dispatch_pty_input_operation_core<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let mut source_executor = crate::session::selection::SourceExecutorGuard::unbound();
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.after_pty_core_accept(injection_id).await {
                return;
            }
        }
        // Borrow the lexical observer through the whole existing dispatch
        // body. This is polled inline, with no new task or accepted core.
        let failed = async {
            let store = match &state.store {
                Ok(store) => Arc::clone(store),
                Err(_) => return false,
            };
            let shutdown = app
                .try_state::<crate::shutdown::ShutdownSignal>()
                .map(|signal| signal.token().clone())
                .unwrap_or_else(tokio_util::sync::CancellationToken::new);
            let _operation_lock = match store.try_operation_lock(injection_id) {
                Ok(Some(lock)) => lock,
                _ => return false,
            };
            let _active = match state.active_operations.try_register(injection_id) {
                Some(active) => active,
                None => return false,
            };
            let initial = match store
                .query_pty_input_by_injection_offloaded(injection_id.to_string())
                .await
            {
                Ok(Some(result)) if !result.terminal => result,
                _ => return false,
            };
            let target = match initial.target.clone() {
                Some(target) => target,
                None => return false,
            };
            let _response_delivery = match store.begin_response_delivery(&target) {
                Ok(reservation) => reservation,
                Err(_) => return false,
            };
            let target_gate = match &state.target_gate {
                Ok(gate) => Arc::clone(gate),
                Err(_) => return false,
            };
            // Startup restore must finish before this dispatch owns target guards.
            // No claim or payload has been consumed at this point.
            if let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            {
                if coordinator.wait_for_restore_release().await.is_err() {
                    return false;
                }
            }
            let mut target_stripe = Some(match target_gate.try_target_lock(&target) {
                Ok(Some(lock)) => lock,
                _ => return false,
            });
            let initial_expires =
                match initial.expires_at.as_deref().and_then(|value| {
                    crate::phone::types::parse_canonical_pty_timestamp(value).ok()
                }) {
                    Some(expires) => expires,
                    None => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::StoreCorrupt),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                        "[pty-input] invalid initial deadline id={} code=terminal_store_failed",
                        injection_id
                    );
                        }
                        return false;
                    }
                };
            let mut target_guard = Some(
                match await_pty_input_before_deadline(
                    initial_expires,
                    &shutdown,
                    target_gate.acquire_exact(&target),
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(PtyInputPreBoundaryStop::Shutdown) => return false,
                    Err(PtyInputPreBoundaryStop::Expired) => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::Expired),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                        "[pty-input] target-wait expiry failed id={} code=terminal_store_failed",
                        injection_id
                    );
                        }
                        return false;
                    }
                },
            );
            {
                let (Some(stripe), Some(exact)) = (target_stripe.as_ref(), target_guard.as_ref())
                else {
                    return false;
                };
                if target_gate
                    .target_ownership(&target, stripe, exact)
                    .is_err()
                {
                    return false;
                }
            }
            let lease_owner = Uuid::new_v4().to_string();
            let claimed = match store
                .claim_pty_input_offloaded(
                    source_plane,
                    Some(injection_id.to_string()),
                    lease_owner.clone(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(Some(claimed)) => claimed,
                _ => return false,
            };
            let expires =
                match crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at) {
                    Ok(expires) => expires,
                    Err(_) => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::StoreCorrupt),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                                "[pty-input] corrupt-expiry rejection failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
                Arc::clone(&store),
                injection_id.to_string(),
                lease_owner.clone(),
                expires,
            );
            if expires <= chrono::Utc::now() {
                reject_pty_input_before_boundary(&store, &mut heartbeat, injection_id, C::Expired)
                    .await;
                return false;
            }
            match refresh_pty_claim_for_resume(&store, &initial, &claimed).await {
                Ok(PtyClaimRefresh::Current) => {}
                Ok(PtyClaimRefresh::Terminal) => {
                    heartbeat.finish().await;
                    return false;
                }
                Err(code) => {
                    finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    return false;
                }
            }
            let mut prepared_replacement = None;
            let mut owned_work: Option<OwnedPtyWorkTransfer> = None;
            let (selected, spawned, verified_target, session_manager, pty_manager) = loop {
                if app
                    .try_state::<Arc<crate::RestoreInProgress>>()
                    .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
                {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::RestoreInProgress,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                if app
                    .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                    .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
                {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::PurgeInProgress,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }

                let verified_route =
                    if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                        self.validate_claimed_host_authority(app, &claimed).await
                    } else {
                        match self
                            .validate_claimed_api_authority(app, &claimed, api_client_store)
                            .await
                        {
                            Ok(route) => route,
                            Err(_) => {
                                let observed = finish_pty_input_before_boundary(
                                    &store,
                                    &mut heartbeat,
                                    injection_id,
                                    &lease_owner,
                                    C::StoreTransient,
                                )
                                .await;
                                drop(target_guard.take());
                                drop(target_stripe.take());
                                finish_owned_pty_work(app, &mut owned_work, observed).await;
                                return false;
                            }
                        }
                    };
                let Some(verified_route) = verified_route else {
                    let code = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli
                    {
                        C::AuthorityChanged
                    } else {
                        C::ApiBindingMismatch
                    };
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        code,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                };
                if verified_route.target.canonical_fqn != target {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::AuthorityChanged,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                let verified_sender = verified_route.sender;
                let verified_target = verified_route.target;

                let session_manager = app
                    .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                    .inner()
                    .clone();
                let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                let sessions = {
                    let guard = session_manager.read().await;
                    guard.list_sessions().await
                };
                let mut matching: Vec<SessionInfo> = sessions
                    .into_iter()
                    .filter(|session| {
                        crate::config::teams::agent_fqn_from_path(&session.working_directory)
                            == claimed.target_fqn
                    })
                    .collect();
                matching.sort_by(|left, right| {
                    fn rank(status: &SessionStatus) -> u8 {
                        match status {
                            SessionStatus::Active => 0,
                            SessionStatus::Running => 1,
                            SessionStatus::Idle => 2,
                            SessionStatus::Exited(_) => 3,
                        }
                    }
                    rank(&left.status)
                        .cmp(&rank(&right.status))
                        .then_with(|| right.created_at.cmp(&left.created_at))
                        .then_with(|| left.id.cmp(&right.id))
                });

                let pty_profile_settings = app.state::<SettingsState>().read().await.clone();
                let mut eligible = Vec::new();
                let mut saw_supported_busy = false;
                let mut saw_live_unsupported = false;
                let mut saw_live_inconsistent = false;
                let mut saw_live_temporary = false;
                for session in &matching {
                    if matches!(session.status, SessionStatus::Exited(_)) {
                        continue;
                    }
                    if session
                        .name
                        .starts_with(crate::session::session::TEMP_SESSION_PREFIX)
                    {
                        saw_live_temporary = true;
                        continue;
                    }
                    let Ok(id) = Uuid::parse_str(&session.id) else {
                        saw_live_unsupported = true;
                        continue;
                    };
                    let (live, route_backend, route_identities) = {
                        let manager = pty_manager
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        (
                            manager.has_session(id),
                            manager.backend_kind(id),
                            manager.route_identities(id),
                        )
                    };
                    let path_matches =
                        route_identities.is_some_and(|(cwd_identity, replica_anchor, _)| {
                            let current_cwd = crate::path_identity::verify_directory(Path::new(
                                &session.working_directory,
                            ));
                            let current_target = crate::config::teams::verify_pty_input_replica_cwd(
                                Path::new(&session.working_directory),
                            );
                            matches!(
                                (cwd_identity, current_cwd),
                                (Some(saved), Ok(current))
                                    if crate::path_identity::same_object(&saved, &current)
                            ) && matches!(
                                (replica_anchor, current_target),
                                (Some(saved), Ok(current))
                                    if current.canonical_fqn == verified_target.canonical_fqn
                                        && current.authority_fingerprint
                                            == verified_target.authority_fingerprint
                                        && crate::path_identity::same_object(
                                            &saved,
                                            &verified_target.replica_identity,
                                        )
                            )
                        });
                    if !live || route_backend != Some(session.backend_kind) || !path_matches {
                        saw_live_inconsistent = true;
                        continue;
                    }
                    if !session_has_current_pty_submission_provenance(
                        session,
                        &pty_profile_settings,
                    ) {
                        saw_live_unsupported = true;
                        continue;
                    }
                    if session.waiting_for_input {
                        eligible.push(session.clone());
                    } else {
                        saw_supported_busy = true;
                    }
                }
                let (selected, spawned) = if let Some(selected) = eligible.into_iter().next() {
                    (selected, false)
                } else if saw_supported_busy {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Busy,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                } else if saw_live_temporary {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::NonpersistentLiveSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                } else if saw_live_inconsistent {
                    if claimed.attempt < 5 {
                        let observed = finish_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            &lease_owner,
                            C::SessionRace,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                    } else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::InconsistentSession,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                    }
                    return false;
                } else if saw_live_unsupported {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::UnsupportedSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                } else {
                    #[cfg(test)]
                    let fixture_control = self
                        .test_hooks
                        .as_ref()
                        .and_then(MailboxTestHooks::replacement_fixture_control)
                        .filter(|control| {
                            !control
                                .ticket_only
                                .load(std::sync::atomic::Ordering::Acquire)
                        });
                    #[cfg(test)]
                    let fixture_lease = if fixture_control.is_some() {
                        let (Some(exact), Some(stripe)) =
                            (target_guard.take(), target_stripe.take())
                        else {
                            heartbeat.finish().await;
                            return false;
                        };
                        Some(Arc::new(crate::commands::session::FixturePtyCutLease {
                            gate: target_gate.clone(),
                            target: target.clone(),
                            exact,
                            stripe,
                        }))
                    } else {
                        None
                    };
                    let spawn = {
                        #[cfg(test)]
                        let (stripe, exact) = match fixture_lease.as_ref() {
                            Some(lease) => (Some(&lease.stripe), Some(&lease.exact)),
                            None => (target_stripe.as_ref(), target_guard.as_ref()),
                        };
                        #[cfg(not(test))]
                        let (stripe, exact) = (target_stripe.as_ref(), target_guard.as_ref());
                        let (Some(stripe), Some(exact)) = (stripe, exact) else {
                            heartbeat.finish().await;
                            return false;
                        };
                        let target_ownership =
                            match target_gate.target_ownership(&target, stripe, exact) {
                                Ok(proof) => proof,
                                Err(_) => {
                                    heartbeat.finish().await;
                                    return false;
                                }
                            };
                        await_pty_input_before_deadline(
                            expires,
                            &shutdown,
                            self.spawn_pty_input_target(
                                app,
                                &claimed,
                                &matching,
                                &verified_sender,
                                &verified_target,
                                source_plane,
                                api_client_store,
                                &target_ownership,
                                &initial,
                                prepared_replacement.take(),
                                &mut source_executor,
                                #[cfg(test)]
                                fixture_lease.as_ref(),
                            ),
                        )
                        .await
                    };
                    #[cfg(test)]
                    if let Some(lease) = fixture_lease {
                        let restored = Arc::try_unwrap(lease);
                        let worker_still_holds_clone = restored.is_err();
                        let receipt_missing = fixture_control.as_ref().is_some_and(|control| {
                            control.submitted.load(std::sync::atomic::Ordering::Acquire)
                                && !control.received.load(std::sync::atomic::Ordering::Acquire)
                        });
                        match restored {
                            Ok(lease) if !receipt_missing => {
                                target_guard = Some(lease.exact);
                                target_stripe = Some(lease.stripe);
                            }
                            other => {
                                drop(other);
                                if let Some(control) = fixture_control.as_ref() {
                                    control.restoration_shared.store(
                                        worker_still_holds_clone,
                                        std::sync::atomic::Ordering::Release,
                                    );
                                    control
                                        .restoration_failed
                                        .store(true, std::sync::atomic::Ordering::Release);
                                }
                                // This cut precedes every Retained/timeout/error
                                // handoff, even if the owner already has a stop or
                                // rollback receipt. A submitted job without actual
                                // receive has no source-side work authority.
                                heartbeat.finish().await;
                                return false;
                            }
                        }
                    }
                    match spawn {
                        Ok(Ok(PtySpawnStep::Created(session))) => (session, true),
                        Ok(Ok(PtySpawnStep::OwnedCreated(session, continuation))) => {
                            let Some(effect) = continuation.start_effect(
                                crate::session::selection::LifecycleEffectSlot::WorkTransfer,
                            ) else {
                                drop(target_guard.take());
                                drop(target_stripe.take());
                                heartbeat.finish().await;
                                return false;
                            };
                            owned_work = Some((continuation, effect));
                            (session, true)
                        }
                        Ok(Ok(PtySpawnStep::Retained {
                            continuation,
                            failed,
                        })) => {
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            let stop = continuation.source_stop();
                            let rolled_back = continuation.precommit_rollback_completed();
                            if stop.is_none() && !rolled_back {
                                heartbeat.finish().await;
                                return failed;
                            }
                            let effect = continuation.start_effect(
                                crate::session::selection::LifecycleEffectSlot::WorkTransfer,
                            );
                            use crate::session::selection::OwnedSourceStop as Stop;
                            let observed = match stop {
                                Some(Stop::Terminal) => {
                                    heartbeat.finish().await;
                                    true
                                }
                                reason => {
                                    let code = match reason {
                                        Some(Stop::AuthorityChanged) => C::AuthorityChanged,
                                        Some(Stop::StoreTransient) => C::StoreTransient,
                                        Some(Stop::Expired) => C::Expired,
                                        Some(Stop::LeaseLost) => C::LeaseLost,
                                        Some(Stop::Terminal) => unreachable!(),
                                        None => C::SpawnFailedSafe,
                                    };
                                    finish_pty_input_before_boundary(
                                        &store,
                                        &mut heartbeat,
                                        injection_id,
                                        &lease_owner,
                                        code,
                                    )
                                    .await
                                }
                            };
                            if let Some(effect) = effect {
                                let manager = app
                                    .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                                    .read()
                                    .await
                                    .clone();
                                let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                                if rolled_back {
                                    continuation
                                        .finish_rolled_back_work(&manager, &pty, effect, observed)
                                        .await;
                                } else {
                                    continuation
                                        .finish_observed_work_transfer(
                                            &manager, &pty, effect, observed,
                                        )
                                        .await;
                                }
                            }
                            return failed;
                        }
                        Ok(Ok(PtySpawnStep::RetainedUnowned)) => {
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            heartbeat.finish().await;
                            return false;
                        }
                        Ok(Ok(PtySpawnStep::NeedReacquire(prepared))) => {
                            prepared_replacement = prepared;
                            // The spawn block ended the proof borrow above.
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                                app,
                                (&store, &initial, &claimed),
                                (&target_gate, &verified_target, PtyReacquireGoal::Spawn),
                                (expires, &shutdown),
                                &mut heartbeat,
                            )
                            .await
                            else {
                                return false;
                            };
                            target_stripe = Some(stripe);
                            target_guard = Some(exact);
                            continue;
                        }
                        Ok(Err(code)) => {
                            let observed = finish_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                &lease_owner,
                                code,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(PtyInputPreBoundaryStop::Expired) => {
                            let observed = reject_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                C::Expired,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(PtyInputPreBoundaryStop::Shutdown) => {
                            heartbeat.finish().await;
                            return false;
                        }
                    }
                };
                let selected_id = Uuid::parse_str(&selected.id).ok();
                if owned_work.is_none()
                    && selected_id.is_some_and(|id| {
                        crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                            &claimed.target_fqn,
                            id,
                        )
                    })
                {
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    let Some(id) = selected_id else {
                        heartbeat.finish().await;
                        return false;
                    };
                    let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                        app,
                        (&store, &initial, &claimed),
                        (
                            &target_gate,
                            &verified_target,
                            PtyReacquireGoal::Published(id),
                        ),
                        (expires, &shutdown),
                        &mut heartbeat,
                    )
                    .await
                    else {
                        return false;
                    };
                    target_stripe = Some(stripe);
                    target_guard = Some(exact);
                    continue;
                }
                break (
                    selected,
                    spawned,
                    verified_target,
                    session_manager,
                    pty_manager,
                );
            };
            let selected_id = match Uuid::parse_str(&selected.id) {
                Ok(id) => id,
                Err(_) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::InconsistentSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            let readiness = await_pty_input_before_deadline(
                expires,
                &shutdown,
                self.wait_for_pty_input_ready(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    spawned,
                    source_plane,
                    api_client_store,
                    &heartbeat,
                ),
            )
            .await;
            match readiness {
                Ok(Ok(())) => {}
                Ok(Err(code)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let permit = match await_pty_input_before_deadline(
                expires,
                &shutdown,
                PtyManager::acquire_input_writer(&pty_manager, selected_id),
            )
            .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::SessionRace,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            };
            let final_api_guard =
                if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi {
                    let Some(client_store) = api_client_store else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
                    };
                    let (Some(client_id), Some(generation)) = (
                        claimed.authority_client_id.as_deref(),
                        claimed.authority_client_generation.as_deref(),
                    ) else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
                    };
                    match client_store
                        .load_active_binding_fresh_offloaded(
                            client_id.to_string(),
                            generation.to_string(),
                            crate::api::auth::SCOPE_PTY_INPUT,
                        )
                        .await
                    {
                        Ok(Some(guard)) => Some(guard),
                        Ok(None) => {
                            let observed = reject_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                C::AuthorityChanged,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(_) => {
                            let observed = finish_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                &lease_owner,
                                C::StoreTransient,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                    }
                } else {
                    None
                };
            let final_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let Some(final_authority) = final_authority else {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            };
            if final_authority.target.authority_fingerprint != verified_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &final_authority.target.replica_identity,
                    &verified_target.replica_identity,
                )
            {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if self
                .validate_selected_pty_target(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    selected.backend_kind,
                )
                .await
                .is_none()
            {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    C::SessionRace,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if heartbeat.failed() {
                let code = if expires <= chrono::Utc::now() {
                    C::Expired
                } else {
                    C::LeaseLost
                };
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    code,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !heartbeat.finish().await {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let backend_name = match selected.backend_kind {
                SessionBackendKind::LocalProcess => "localProcess",
                SessionBackendKind::ContainerTransport => "containerTransport",
            };
            let payload = match store
                .begin_pty_actuating_offloaded(
                    injection_id.to_string(),
                    lease_owner.clone(),
                    selected.id.clone(),
                    backend_name.to_string(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(payload) => payload,
                Err(crate::api::message_store::MessageStoreError::ActuationCommitAmbiguous) => {
                    let terminal = store
                        .terminalize_pty_input_offloaded(
                            injection_id.to_string(),
                            S::Indeterminate,
                            Some(C::TerminalStoreFailed),
                            chrono::Utc::now(),
                        )
                        .await;
                    let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] ambiguous boundary reconciliation failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    return false;
                }
                Err(_) => {
                    reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::StoreTransient,
                    )
                    .await;
                    return false;
                }
            };
            let post_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let payload_matches = initial.payload_bytes == Some(payload.len() as u64)
                && initial.payload_sha256.as_deref()
                    == Some(crate::phone::types::sha256_hex(&payload).as_str());
            let post_valid = payload_matches
                && post_authority.is_some_and(|route| {
                    route.sender.authority_fingerprint
                        == final_authority.sender.authority_fingerprint
                        && route.target.authority_fingerprint
                            == verified_target.authority_fingerprint
                        && crate::path_identity::same_object(
                            &route.target.replica_identity,
                            &verified_target.replica_identity,
                        )
                })
                && self
                    .validate_selected_pty_target(
                        app,
                        &claimed,
                        &verified_target,
                        selected_id,
                        selected.backend_kind,
                    )
                    .await
                    .is_some();
            if !post_valid {
                let terminal = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await;
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] post-boundary terminalization failed id={} \
                    code=terminal_store_failed",
                        injection_id
                    );
                }
                return false;
            }
            let manager = {
                let guard = session_manager.read().await;
                guard.clone()
            };
            let idle_detector = app
                .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                .inner()
                .clone();
            let authority_id =
                match crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id) {
                    Ok(id) => id,
                    Err(_) => {
                        let terminal = store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Indeterminate,
                                Some(C::FinalRevalidationFailed),
                                chrono::Utc::now(),
                            )
                            .await;
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let authority_backend = match source_plane {
                crate::phone::types::PtyInputSourcePlane::HostCli => {
                    SessionBackendKind::LocalProcess
                }
                crate::phone::types::PtyInputSourcePlane::ContainerApi => {
                    SessionBackendKind::ContainerTransport
                }
            };
            let authority_container_backend = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .container_backend();
            let authority_route =
                match PtyManager::authority_route_proof(&pty_manager, authority_id) {
                    Ok(proof) => proof,
                    Err(_) => {
                        let terminal = store.terminalize_pty_input(
                            injection_id,
                            S::Indeterminate,
                            Some(C::FinalRevalidationFailed),
                            chrono::Utc::now(),
                        );
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let expected_api_binding = final_api_guard.as_ref().and_then(|fresh| {
                let root =
                    crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root))
                        .ok()?;
                Some((
                    fresh.client.client_id.clone(),
                    fresh.client.credential_generation.clone()?,
                    fresh.client.bound_session_id.clone()?,
                    root.object_id,
                    fresh.presented_token_hash.clone(),
                ))
            });
            if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi
                && expected_api_binding.is_none()
            {
                let terminal = store.terminalize_pty_input(
                    injection_id,
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                );
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                    "[pty-input] boundary terminalization failed id={} code=terminal_store_failed",
                    injection_id
                );
                }
                return false;
            }
            let boundary_settings = app.state::<SettingsState>().inner().clone();
            let owned_route = owned_work
                .as_ref()
                .and_then(|_| PtyManager::snapshot_route_proof(&pty_manager, selected_id).ok());
            let boundary_carrier = crate::session::manager::acquire_carrier_mutation_permit().await;
            let owned_token = owned_work
                .as_ref()
                .and_then(|(continuation, _)| continuation.launch_token());
            let owned_current = match owned_work.as_ref() {
                Some((continuation, _)) => {
                    manager
                        .with_current_carrier_snapshot(&boundary_carrier, |current| {
                            owned_token.is_some()
                                && continuation.current_completion_region(
                                    &boundary_carrier,
                                    current,
                                    &pty_manager,
                                )
                        })
                        .await
                }
                None => true,
            };
            if !owned_current {
                drop(boundary_carrier);
                let observed = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await
                    .is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let boundary_write = match manager
                .prepare_pty_input_boundary_with_carrier_permit(
                    &boundary_carrier,
                    selected_id,
                    &verified_target,
                    selected.backend_kind,
                    authority_id,
                    &final_authority.sender,
                    authority_backend,
                    &authority_route,
                    &permit,
                    &idle_detector,
                    &boundary_settings,
                    |session, settings| {
                        owned_token.is_none_or(|token| session.token == token)
                            && owned_work.as_ref().is_none_or(|(continuation, _)| {
                                owned_route.as_ref().is_some_and(|proof| {
                                    continuation.matches_captured_launch_route(proof)
                                })
                            })
                            && session_has_current_pty_submission_provenance(
                                &SessionInfo::from(session),
                                settings,
                            )
                    },
                    || {
                        if expires <= chrono::Utc::now()
                            || app
                                .try_state::<Arc<crate::RestoreInProgress>>()
                                .is_some_and(|flag| {
                                    flag.0.load(std::sync::atomic::Ordering::SeqCst)
                                })
                            || app
                                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
                        {
                            return false;
                        }
                        match (&source_plane, expected_api_binding.as_ref()) {
                            (crate::phone::types::PtyInputSourcePlane::HostCli, None) => true,
                            (
                                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                                Some((
                                    client_id,
                                    generation,
                                    bound_session_id,
                                    root_object,
                                    token_hash,
                                )),
                            ) => {
                                let binding =
                                    authority_container_backend.credential_binding(authority_id);
                                binding.is_some_and(|binding| {
                                    binding.client_id == *client_id
                                        && binding.credential_generation == *generation
                                        && binding.bound_session_id == *bound_session_id
                                        && binding.bound_root_object_id == *root_object
                                        && crate::api::auth::constant_time_eq(
                                            &binding.credential_token_hash,
                                            token_hash,
                                        )
                                })
                            }
                            _ => false,
                        }
                    },
                )
                .await
            {
                Ok(guard) => {
                    drop(boundary_carrier);
                    Ok(crate::pty::inject::write_exact_agent_input_first(
                        guard, &payload,
                    ))
                }
                Err(_) => {
                    drop(boundary_carrier);
                    // The guard-bearing Result ends in this match. Only the
                    // observed terminal boolean survives to the work handoff await.
                    let terminal = store.terminalize_pty_input(
                        injection_id,
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    );
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    Err(terminal.as_ref().is_ok_and(|result| result.terminal))
                }
            };
            let text_write_succeeded = match boundary_write {
                Ok(written) => written,
                Err(observed) => {
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            // Keep the fresh registry lock through the synchronous first-write
            // boundary so a concurrent revocation linearizes before or after it,
            // never between the final authority check and the text write.
            drop(final_api_guard);
            let outcome = crate::pty::inject::submit_exact_agent_input_with_permit(
                &permit,
                text_write_succeeded,
            )
            .await;
            let (status, reason) = match outcome {
                crate::pty::inject::AgentSubmitOutcome::TextWriteFailed => {
                    (S::Indeterminate, Some(C::TextWriteFailed))
                }
                crate::pty::inject::AgentSubmitOutcome::RequiredEnterFailed => {
                    (S::Indeterminate, Some(C::RequiredEnterFailed))
                }
                crate::pty::inject::AgentSubmitOutcome::Submitted {
                    redundant_enter_failed,
                } => {
                    let metadata = if payload == b"/clear" {
                        crate::commands::pty::stamp_fresh_boundary_to_session(app, selected_id)
                            .await
                    } else if payload == b"/compact" {
                        crate::commands::pty::BoundaryMetadataOutcome::Unchanged
                    } else {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            app,
                            selected_id,
                        )
                        .await
                    };
                    let reason =
                        if metadata == crate::commands::pty::BoundaryMetadataOutcome::Failed {
                            Some(C::BoundaryMetadataFailed)
                        } else {
                            redundant_enter_failed.then_some(C::RedundantEnterFailed)
                        };
                    (S::Injected, reason)
                }
            };
            let result = match store
                .terminalize_pty_input_offloaded(
                    injection_id.to_string(),
                    status,
                    reason,
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    log::error!(
                        "[pty-input] terminal persistence failed id={} code=terminal_store_failed",
                        injection_id
                    );
                    return false;
                }
            };
            crate::api::audit::record_pty_input_result("terminal", &result);
            drop(target_guard.take());
            drop(target_stripe.take());
            drop(permit);
            finish_owned_pty_work(app, &mut owned_work, result.terminal).await;
            if app.emit("pty_input_status", &result).is_err() {
                log::warn!(
                    "[pty-input] status event failed id={} code=boundary_metadata_failed",
                    injection_id
                );
            }
            false
        }
        .await;
        source_executor.returned(failed);
    }

    async fn validate_claimed_api_authority<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) -> Result<
        Option<crate::config::teams::VerifiedPtyInputRoute>,
        crate::api::auth::FreshRegistryError,
    > {
        let Some(client_store) = client_store else {
            return Ok(None);
        };
        let (Some(client_id), Some(generation)) = (
            claimed.authority_client_id.as_deref(),
            claimed.authority_client_generation.as_deref(),
        ) else {
            return Ok(None);
        };
        let Some(fresh) = client_store
            .load_active_binding_fresh_offloaded(
                client_id.to_string(),
                generation.to_string(),
                crate::api::auth::SCOPE_PTY_INPUT,
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .validate_claimed_api_authority_with_fresh(app, claimed, &fresh)
            .await)
    }

    async fn validate_claimed_api_authority_with_fresh<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        fresh: &crate::api::auth::ApiClientFreshGuard,
    ) -> Option<crate::config::teams::VerifiedPtyInputRoute> {
        let client_id = claimed.authority_client_id.as_deref()?;
        let generation = claimed.authority_client_generation.as_deref()?;
        let session_id =
            crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id).ok()?;
        if fresh.client.client_id != client_id
            || !fresh.client.has_scope(crate::api::auth::SCOPE_PTY_INPUT)
            || fresh.client.bound_session_id.as_deref()
                != Some(claimed.authority_session_id.as_str())
            || fresh.client.credential_generation.as_deref() != Some(generation)
        {
            return None;
        }
        let session = {
            let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = manager.read().await;
            guard.get_session(session_id).await
        }?;
        if matches!(session.status, SessionStatus::Exited(_))
            || session.backend_kind != SessionBackendKind::ContainerTransport
        {
            return None;
        }
        let root =
            crate::path_identity::verify_directory(Path::new(&session.working_directory)).ok()?;
        let bound_root =
            crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root)).ok()?;
        if !crate::path_identity::same_object(&root, &bound_root) {
            return None;
        }
        let (binding, route_backend, route_identities, live) = {
            let manager = app.state::<Arc<Mutex<PtyManager>>>();
            let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
            (
                manager.container_backend().credential_binding(session_id),
                manager.backend_kind(session_id),
                manager.route_identities(session_id),
                manager.has_session(session_id),
            )
        };
        let binding = binding?;
        if !live
            || route_backend != Some(SessionBackendKind::ContainerTransport)
            || binding.client_id != client_id
            || binding.credential_generation != generation
            || binding.bound_session_id != claimed.authority_session_id
            || binding.bound_root_object_id != root.object_id
            || !crate::api::auth::constant_time_eq(
                &binding.credential_token_hash,
                &fresh.presented_token_hash,
            )
        {
            return None;
        }
        let in_memory_paths = {
            let settings = app.state::<SettingsState>();
            let paths = settings.read().await.project_paths.clone();
            paths
        };
        let mut paths =
            match crate::config::settings::read_pty_input_project_paths_strict_offloaded().await {
                Ok(paths) => paths.unwrap_or(in_memory_paths),
                Err(_) => return None,
            };
        let sender = crate::config::teams::verify_pty_input_coordinator_root(Path::new(
            &session.working_directory,
        ))
        .ok()?;
        if let Some(project_path) = sender.ac_root_identity.canonical_path.parent() {
            let project = project_path
                .to_str()
                .map(crate::path_utils::normalize_windows_verbatim_path)?;
            if !paths.contains(&project) {
                paths.push(project);
            }
        }
        let route = crate::config::teams::verify_pty_input_route(
            Path::new(&session.working_directory),
            false,
            &claimed.target_fqn,
            &paths,
        )
        .ok()?;
        let route_matches = route_identities.is_some_and(|(cwd, replica, _)| {
            cwd.as_ref()
                .is_some_and(|saved| crate::path_identity::same_object(saved, &root))
                && replica.as_ref().is_some_and(|saved| {
                    crate::path_identity::same_object(saved, &route.sender.replica_identity)
                })
        });
        if !route_matches
            || route.sender.canonical_fqn != claimed.sender_fqn
            || route.sender.authority_fingerprint != claimed.sender_identity_fingerprint
            || route.target.authority_fingerprint != claimed.target_identity_fingerprint
        {
            return None;
        }
        Some(route)
    }

    async fn pty_spawn_authority_is_current<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        expected_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        expected_target: &crate::config::teams::VerifiedPtyInputIdentity,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) -> Result<bool, crate::api::auth::FreshRegistryError> {
        let current = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
            self.validate_claimed_host_authority(app, claimed).await
        } else {
            self.validate_claimed_api_authority(app, claimed, api_client_store)
                .await?
        };
        Ok(current.is_some_and(|route| {
            route.sender.canonical_fqn == expected_sender.canonical_fqn
                && route.sender.authority_fingerprint == expected_sender.authority_fingerprint
                && crate::path_identity::same_object(
                    &route.sender.replica_identity,
                    &expected_sender.replica_identity,
                )
                && route.target.canonical_fqn == expected_target.canonical_fqn
                && route.target.authority_fingerprint == expected_target.authority_fingerprint
                && crate::path_identity::same_object(
                    &route.target.replica_identity,
                    &expected_target.replica_identity,
                )
        }))
    }

    async fn owned_pty_recipe_is_current<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        expected: &crate::commands::session::OwnedReplacementCreateRequest,
    ) -> bool {
        let Some(agent) = expected.agent_id.as_deref() else {
            return false;
        };
        let settings = app.state::<SettingsState>().read().await.clone();
        let Ok(Some(spawn)) = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings,
            agent,
            &expected.cwd,
            None,
        ) else {
            return false;
        };
        if SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess {
            return false;
        }
        let mut current = expected.clone();
        current.shell = spawn.shell.clone();
        current.shell_args = spawn.shell_args.clone();
        current.agent_label = Some(spawn.trusted_agent_label.clone());
        current.resolved_spawn = Some(spawn);
        current.host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings.default_shell,
            args: settings.default_shell_args,
        });
        owned_replacement_launch_recipe(&current) == owned_replacement_launch_recipe(expected)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the actual PTY source, proof, executor and fixture borrow contract together"
    )]
    async fn spawn_owned_pty_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        request: crate::commands::session::OwnedReplacementCreateRequest,
        source: (
            &crate::api::message_store::ClaimedPtyInputOperation,
            &crate::phone::types::PtyInputResult,
            &crate::config::teams::VerifiedPtyInputIdentity,
            Option<&Arc<crate::api::auth::ApiClientStore>>,
        ),
        target: (&crate::config::teams::VerifiedPtyInputIdentity, Uuid),
        prepared: Option<crate::session::selection::PreparedOwnedReplacement>,
        target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
        #[cfg(test)] fixture_lease: Option<&Arc<crate::commands::session::FixturePtyCutLease>>,
    ) -> Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        use crate::session::selection::{
            OwnedReplacementWork, ReplacementAdmissionStep, ResolvedReplacement,
        };
        let (claimed, initial, sender, api_client_store) = source;
        let (target, previous_id) = target;
        let Some(coordinator) = app.try_state::<crate::session::selection::SelectionCoordinator>()
        else {
            return Ok(PtySpawnStep::NeedReacquire(prepared));
        };
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let prepared = match prepared {
            Some(prepared) => prepared,
            None => {
                let directory = crate::config::config_dir().ok_or(C::UnsafePath)?;
                let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
                let captured = manager
                    .with_current_carrier_snapshot(&permit, |current| {
                        let carrier =
                            crate::config::sessions_persistence::retained_carrier_for_replacement(
                                &permit,
                                &directory,
                                &target.canonical_fqn,
                                previous_id,
                                current,
                            )?;
                        let row = current
                            .rows
                            .iter()
                            .find(|row| row.id == previous_id)?
                            .clone();
                        Some((carrier, row))
                    })
                    .await;
                drop(permit);
                let Some((carrier, previous)) = captured else {
                    return Ok(PtySpawnStep::NeedReacquire(None));
                };
                let work = OwnedReplacementWork::PtyDurableReference {
                    version: initial.version,
                    injection_id: claimed.injection_id.clone(),
                    op_id: claimed.op_id.clone(),
                    sender: claimed.sender_fqn.clone(),
                    target: claimed.target_fqn.clone(),
                    source_plane: match claimed.source_plane {
                        crate::phone::types::PtyInputSourcePlane::HostCli => "host_cli",
                        crate::phone::types::PtyInputSourcePlane::ContainerApi => "container_api",
                    }
                    .into(),
                    payload_sha256: initial.payload_sha256.clone().ok_or(C::AuthorityChanged)?,
                    payload_bytes: initial.payload_bytes.ok_or(C::AuthorityChanged)?,
                    issued_at: initial.issued_at.clone().ok_or(C::AuthorityChanged)?,
                    expires_at: claimed.expires_at.clone(),
                    queued_at: initial.queued_at.clone().ok_or(C::AuthorityChanged)?,
                    lease_owner: claimed.lease_owner.clone(),
                    attempt: claimed.attempt,
                    authority_session_id: claimed.authority_session_id.clone(),
                    authority_client_id: claimed.authority_client_id.clone(),
                    authority_client_generation: claimed.authority_client_generation.clone(),
                    sender_identity_fingerprint: claimed.sender_identity_fingerprint.clone(),
                    target_identity_fingerprint: claimed.target_identity_fingerprint.clone(),
                    requested_agent_id: claimed.requested_agent_id.clone(),
                };
                let cwd_identity = crate::path_identity::verify_directory(Path::new(&request.cwd))
                    .map_err(|_| C::UnsafePath)?;
                coordinator.prepare_owned_replacement(
                    ResolvedReplacement {
                        target: target.canonical_fqn.clone(),
                        directory,
                        previous,
                        carrier,
                        requested_agent_id: request.agent_id.clone(),
                        requested_profile: None,
                        shell: request.shell.clone(),
                        shell_args: request.shell_args.clone(),
                        launch_recipe: owned_replacement_launch_recipe(&request),
                        cwd: request.cwd.clone(),
                        backend: SessionBackendKind::LocalProcess,
                        work,
                        cwd_identity: cwd_identity.into(),
                        replica_identity: Some(target.replica_identity.clone().into()),
                    },
                    manager.reserve_carrier_create_binding(),
                )
            }
        };
        if !prepared.matches_launch(
            &request.shell,
            &request.shell_args,
            &request.cwd,
            &request.agent_id,
            &owned_replacement_launch_recipe(&request),
        ) {
            return Err(C::AuthorityChanged);
        }
        let tracker = pty
            .lock()
            .map_err(|_| C::StoreTransient)?
            .response_close_state();
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let admission = manager
            .with_current_carrier_snapshot(&permit, |current| {
                coordinator.admit_owned_replacement(&permit, current, &tracker, &pty, prepared)
            })
            .await;
        drop(permit);
        let continuation = match admission {
            Ok(ReplacementAdmissionStep::Waiting(prepared)) => {
                return Ok(PtySpawnStep::NeedReacquire(Some(prepared)))
            }
            Ok(ReplacementAdmissionStep::Accepted(continuation)) => continuation,
            Err(_) => return Err(C::StoreTransient),
        };
        if !source_executor.bind(&continuation) {
            return Ok(PtySpawnStep::RetainedUnowned);
        }
        let validation_app = app.clone();
        let validation_claim = claimed.clone();
        let validation_initial = initial.clone();
        let validation_sender = sender.clone();
        let validation_target = target.clone();
        let validation_api = api_client_store.cloned();
        let validation_request = request.clone();
        let continuation = continuation.with_source_validation(Arc::new(move || {
            let app = validation_app.clone();
            let claimed = validation_claim.clone();
            let initial = validation_initial.clone();
            let sender = validation_sender.clone();
            let target = validation_target.clone();
            let api = validation_api.clone();
            let request = validation_request.clone();
            Box::pin(async move {
                use crate::session::selection::OwnedSourceStop as Stop;
                let delivery = MailboxPoller::delivery_only();
                let store = app
                    .try_state::<crate::api::message_store::MessageStoreState>()
                    .and_then(|state| state.store.as_ref().ok().cloned())
                    .ok_or(Stop::StoreTransient)?;
                match refresh_pty_claim_for_resume(&store, &initial, &claimed).await {
                    Ok(PtyClaimRefresh::Current) => {}
                    Ok(PtyClaimRefresh::Terminal) => return Err(Stop::Terminal),
                    Err(C::AuthorityChanged) => return Err(Stop::AuthorityChanged),
                    Err(C::Expired) => return Err(Stop::Expired),
                    Err(C::LeaseLost) => return Err(Stop::LeaseLost),
                    Err(_) => return Err(Stop::StoreTransient),
                }
                match delivery
                    .pty_spawn_authority_is_current(
                        &app,
                        &claimed,
                        &sender,
                        &target,
                        claimed.source_plane,
                        api.as_ref(),
                    )
                    .await
                {
                    Ok(true) => {
                        if delivery.owned_pty_recipe_is_current(&app, &request).await {
                            Ok(())
                        } else {
                            Err(Stop::AuthorityChanged)
                        }
                    }
                    Ok(false) => Err(Stop::AuthorityChanged),
                    Err(_) => Err(Stop::StoreTransient),
                }
            })
        }));
        #[cfg(test)]
        let continuation = match self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            Some(control) => continuation.with_fixture_control(control),
            None => continuation,
        };
        #[cfg(test)]
        continuation.fixture_inserted();
        #[cfg(test)]
        if !continuation
            .fixture_reached(crate::session::selection::ReplacementFixtureCut::SourceCoreEntered)
            .await
        {
            return Ok(PtySpawnStep::RetainedUnowned);
        }
        #[cfg(test)]
        let fixture_created = if let Some(lease) = fixture_lease {
            let Some(control) = self
                .test_hooks
                .as_ref()
                .and_then(MailboxTestHooks::replacement_fixture_control)
            else {
                return Ok(PtySpawnStep::RetainedUnowned);
            };
            let result = coordinator
                .submit_inline_replacement_fixture(
                    crate::commands::session::FixtureReplacementInput {
                        request: request.clone(),
                        continuation: continuation.clone().with_fixture_control(control.clone()),
                        lease: crate::commands::session::FixtureReplacementLease::PtyCutShared(
                            lease.clone(),
                        ),
                    },
                    control,
                )
                .await;
            let Some(result) = result else {
                return Ok(PtySpawnStep::RetainedUnowned);
            };
            if result.ordinary_lease.is_some() {
                return Ok(PtySpawnStep::RetainedUnowned);
            }
            Some(result.outcome)
        } else {
            None
        };
        #[cfg(test)]
        let created = match fixture_created {
            Some(created) => created,
            None => {
                crate::commands::session::create_owned_response_replacement(
                    app,
                    request,
                    continuation.clone(),
                    target_ownership,
                )
                .await
            }
        };
        #[cfg(not(test))]
        let created = crate::commands::session::create_owned_response_replacement(
            app,
            request,
            continuation.clone(),
            target_ownership,
        )
        .await;
        match created {
            Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(info)) => {
                Ok(PtySpawnStep::OwnedCreated(info, continuation))
            }
            Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                Ok(PtySpawnStep::Retained {
                    continuation,
                    failed: false,
                })
            }
            Err(error) => {
                // A returned create error does not certify runtime rollback or
                // durable source handoff. Keep the accepted owner authoritative.
                continuation.note_retained_work(error);
                Ok(PtySpawnStep::Retained {
                    continuation,
                    failed: true,
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_pty_input_target<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        matching: &[SessionInfo],
        verified_sender: &crate::config::teams::VerifiedPtyInputIdentity,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
        target_ownership: &crate::api::message_store::PtyInputTargetOwnership<'_>,
        initial: &crate::phone::types::PtyInputResult,
        prepared_replacement: Option<crate::session::selection::PreparedOwnedReplacement>,
        source_executor: &mut crate::session::selection::SourceExecutorGuard,
        #[cfg(test)] fixture_lease: Option<&Arc<crate::commands::session::FixturePtyCutLease>>,
    ) -> Result<PtySpawnStep, crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;

        if verified_target.canonical_fqn != claimed.target_fqn
            || !target_ownership.proves(&verified_target.canonical_fqn)
        {
            return Err(C::AuthorityChanged);
        }
        let target_root = verified_target.replica_root.clone();
        let target_root_text = target_root.to_str().ok_or(C::UnsafePath)?.to_string();
        let settings_state = app.state::<SettingsState>();
        let settings = settings_state.read().await.clone();
        let (config_bytes, _) = crate::path_identity::read_bounded_regular(
            &target_root.join("config.json"),
            1024 * 1024,
        )
        .map_err(|_| C::UnsafePath)?;
        let config_value = crate::path_identity::parse_json_no_duplicates(&config_bytes)
            .map_err(|_| C::UnsupportedProfile)?;
        // #2786 (C1) - the state file goes through the same guarded read as
        // config.json (regular file, no-follow, size, duplicate keys); only a
        // missing file is absence. The shared loader's policy overlays it.
        let state_value = read_guarded_agent_state(&target_root)?;
        let config_value =
            crate::config::agent_config::overlay_agent_local_state(config_value, state_value);
        let local_config =
            serde_json::from_value::<crate::config::agent_config::AgentLocalConfig>(config_value)
                .map_err(|_| C::UnsupportedProfile)?;
        let exited = select_persistent_exited_pty_candidate(matching, |session| {
            crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ))
            .is_ok_and(|identity| {
                identity.canonical_fqn == verified_target.canonical_fqn
                    && identity.authority_fingerprint == verified_target.authority_fingerprint
            })
        });
        let mut candidates = Vec::new();
        if let Some(id) = claimed.requested_agent_id.as_ref() {
            candidates.push(id.clone());
        } else {
            if let Some(id) = exited.and_then(|session| session.agent_id.clone()) {
                candidates.push(id);
            }
            if let Some(id) = local_config.tooling.current_coding_agent.clone() {
                candidates.push(id);
            }
            if let Some(id) = local_config.tooling.last_coding_agent.clone() {
                candidates.push(id);
            }
            candidates.extend(settings.agents.iter().map(|agent| agent.id.clone()));
        }
        candidates.dedup();
        let mut resolved = None;
        for id in candidates {
            let Ok(Some(spawn)) = crate::commands::session::build_configured_agent_spawn_for_cwd(
                &settings,
                &id,
                &target_root_text,
                None,
            ) else {
                if claimed.requested_agent_id.is_some() {
                    return Err(C::UnsupportedProfile);
                }
                continue;
            };
            let hint =
                crate::session::profile::CodingAgentKind::detect(&spawn.shell, &spawn.shell_args);
            if crate::session::profile::detect_configured_pty_submission_agent(
                &spawn.shell,
                &spawn.shell_args,
                hint,
            )
            .is_some()
            {
                resolved = Some((id, spawn));
                break;
            }
            if claimed.requested_agent_id.is_some() {
                return Err(C::UnsupportedProfile);
            }
        }
        let (agent_id, spawn) = resolved.ok_or(C::UnsupportedProfile)?;
        // #1271 - same-snapshot host shell: built from the SAME cloned
        // AppSettings that resolved the spawn above (mailbox.rs:5420), before
        // any await, so the pair can never mix across a configuration change.
        let resolved_agent_host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        });
        let expected_backend = SessionBackendKind::from(&spawn.backend);
        let carried = exited.map(|session| {
            (
                session.telegram_bot_id.clone(),
                session.communication.clone(),
            )
        });
        match self
            .pty_spawn_authority_is_current(
                app,
                claimed,
                verified_sender,
                verified_target,
                source_plane,
                api_client_store,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(C::AuthorityChanged),
            Err(_) => return Err(C::StoreTransient),
        }
        // No destroy/create/native preparation has started. Return ownership
        // to dispatch rather than waiting under its borrowed target guards.
        use crate::config::sessions_persistence::CreateCarrierScope;
        let (scope, retained) = observe_pty_spawn_scope(app, verified_target).await;
        if !matches!(scope, CreateCarrierScope::Foreign { .. }) {
            let Some(retained_id) = retained else {
                return Ok(PtySpawnStep::NeedReacquire(prepared_replacement));
            };
            if expected_backend != SessionBackendKind::LocalProcess {
                return Err(C::AuthorityChanged);
            }
            let exited = exited.ok_or(C::InconsistentSession)?;
            let exited_id = crate::phone::types::parse_canonical_uuid_v4(&exited.id)
                .map_err(|_| C::InconsistentSession)?;
            if exited_id != retained_id {
                return Err(C::InconsistentSession);
            }
            let (_, local_name) = crate::config::teams::split_project_prefix(&claimed.target_fqn);
            return self
                .spawn_owned_pty_replacement(
                    app,
                    crate::commands::session::OwnedReplacementCreateRequest {
                        shell: spawn.shell.clone(),
                        shell_args: spawn.shell_args.clone(),
                        cwd: crate::path_utils::normalize_windows_verbatim_path(&target_root_text),
                        name: Some(local_name.to_string()),
                        agent_id: Some(agent_id),
                        agent_label: Some(spawn.trusted_agent_label.clone()),
                        skip_tooling_save: false,
                        git_repos: Vec::new(),
                        resolved_spawn: Some(spawn),
                        host_shell: resolved_agent_host_shell,
                    },
                    (claimed, initial, verified_sender, api_client_store),
                    (verified_target, exited_id),
                    prepared_replacement,
                    target_ownership,
                    source_executor,
                    #[cfg(test)]
                    fixture_lease,
                )
                .await;
        }
        drop(prepared_replacement);
        if let Some(exited) = exited {
            let exited_id = crate::phone::types::parse_canonical_uuid_v4(&exited.id)
                .map_err(|_| C::InconsistentSession)?;
            let destroy_result =
                crate::commands::session::background_destroy_session_inner(app, exited_id).await;
            let session_manager = app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner()
                .clone();
            let remaining = {
                let guard = session_manager.read().await;
                guard.list_sessions().await
            };
            let selected_survived = remaining.iter().any(|session| session.id == exited.id);
            let live_appeared = remaining.iter().any(|session| {
                !matches!(session.status, SessionStatus::Exited(_))
                    && crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                        &session.working_directory,
                    ))
                    .is_ok_and(|identity| {
                        identity.canonical_fqn == verified_target.canonical_fqn
                            && identity.authority_fingerprint
                                == verified_target.authority_fingerprint
                    })
            });
            let route_or_spawn_survived = {
                let manager = app.state::<Arc<Mutex<PtyManager>>>();
                let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
                manager.backend_kind(exited_id).is_some()
                    || manager.has_pending_spawn_for_replica(&verified_target.replica_identity)
            };
            if selected_survived || live_appeared || route_or_spawn_survived {
                return Err(C::SessionRace);
            }
            if destroy_result.is_err() {
                log::debug!(
                    "[pty-input] exited destroy reported failure but relist proved removal id={}",
                    claimed.injection_id
                );
            }
        }
        match self
            .pty_spawn_authority_is_current(
                app,
                claimed,
                verified_sender,
                verified_target,
                source_plane,
                api_client_store,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(C::AuthorityChanged),
            Err(_) => return Err(C::StoreTransient),
        }
        let session_manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .inner()
            .clone();
        let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let (_, local_name) = crate::config::teams::split_project_prefix(&claimed.target_fqn);
        let created = crate::commands::session::create_session_inner_with_pty_target_ownership(
            app,
            &session_manager,
            &pty_manager,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            target_root_text,
            Some(local_name.to_string()),
            Some(agent_id),
            Some(spawn.trusted_agent_label.clone()),
            false,
            Vec::new(),
            exited.is_none(),
            Some(spawn),
            resolved_agent_host_shell,
            None,
            crate::commands::session::CreateSelectionIntent::Background,
            target_ownership,
        )
        .await;
        let info = match created {
            Ok(crate::commands::session::PtyTargetCreateStep::Finalized(info)) => info,
            Ok(crate::commands::session::PtyTargetCreateStep::Retained) => {
                return Ok(PtySpawnStep::RetainedUnowned)
            }
            Err(error) => {
                let sessions = {
                    let guard = session_manager.read().await;
                    guard.list_sessions().await
                };
                let ambiguous = sessions.iter().any(|session| {
                    !matches!(session.status, SessionStatus::Exited(_))
                        && crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                            &session.working_directory,
                        ))
                        .is_ok_and(|identity| {
                            identity.canonical_fqn == verified_target.canonical_fqn
                                && identity.authority_fingerprint
                                    == verified_target.authority_fingerprint
                        })
                }) || pty_manager
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner())
                    .has_pending_spawn_for_replica(&verified_target.replica_identity);
                return Err(if error == "sessionRace" {
                    C::SessionRace
                } else if ambiguous {
                    C::InconsistentSession
                } else {
                    C::SpawnFailedSafe
                });
            }
        };
        let created_id = crate::phone::types::parse_canonical_uuid_v4(&info.id)
            .map_err(|_| C::InconsistentSession)?;
        let created_target =
            crate::config::teams::verify_pty_input_replica_cwd(Path::new(&info.working_directory))
                .map_err(|_| C::InconsistentSession)?;
        let route_valid = {
            let manager = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let identities = manager.route_identities(created_id);
            manager.has_session(created_id)
                && manager.backend_kind(created_id) == Some(expected_backend)
                && identities.is_some_and(|(cwd, replica, _)| {
                    cwd.as_ref().is_some_and(|cwd| {
                        crate::path_identity::is_verified_descendant(
                            cwd,
                            &verified_target.replica_identity,
                        )
                    }) && replica.as_ref().is_some_and(|replica| {
                        crate::path_identity::same_object(
                            replica,
                            &verified_target.replica_identity,
                        )
                    })
                })
        };
        if created_target.canonical_fqn != verified_target.canonical_fqn
            || created_target.authority_fingerprint != verified_target.authority_fingerprint
            || info.backend_kind != expected_backend
            || !session_has_current_pty_submission_provenance(&info, &settings)
            || !route_valid
        {
            return Err(C::InconsistentSession);
        }
        if let Some((telegram_bot_id, communication)) = carried {
            if telegram_bot_id.is_some() {
                self.attach_persisted_telegram_for_wake(
                    app,
                    created_id,
                    telegram_bot_id.as_deref(),
                )
                .await;
            }
            if let Some(communication) = communication {
                let restored = {
                    let guard = session_manager.read().await;
                    guard
                        .restore_communication(created_id, communication.clone())
                        .await
                };
                if restored {
                    crate::session::selection::publish_session_communication(
                        app,
                        created_id,
                        Some(&communication),
                    );
                }
            }
        }
        Ok(PtySpawnStep::Created(info))
    }

    #[allow(clippy::too_many_arguments)]
    async fn wait_for_pty_input_ready<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        claimed: &crate::api::message_store::ClaimedPtyInputOperation,
        verified_target: &crate::config::teams::VerifiedPtyInputIdentity,
        session_id: Uuid,
        spawned: bool,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
        heartbeat: &crate::api::message_store::PreparationHeartbeatGuard,
    ) -> Result<(), crate::phone::types::PtyInputReasonCode> {
        use crate::phone::types::PtyInputReasonCode as C;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let expires = crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at)
                .map_err(|_| C::StoreCorrupt)?;
            if heartbeat.failed() {
                return Err(if expires <= chrono::Utc::now() {
                    C::Expired
                } else {
                    C::LeaseLost
                });
            }
            if expires <= chrono::Utc::now() {
                return Err(C::Expired);
            }
            if app
                .try_state::<Arc<crate::RestoreInProgress>>()
                .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
            {
                return Err(C::RestoreInProgress);
            }
            if app
                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
            {
                return Err(C::PurgeInProgress);
            }
            let menu_guard = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>();
            if menu_guard
                .as_ref()
                .is_some_and(|g| g.is_blocked(session_id))
            {
                return Err(C::MenuGuardBlocked);
            }
            let authority = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                self.validate_claimed_host_authority(app, claimed).await
            } else {
                self.validate_claimed_api_authority(app, claimed, api_client_store)
                    .await
                    .map_err(|_| C::StoreTransient)?
            };
            if !authority.is_some_and(|route| {
                route.sender.authority_fingerprint == claimed.sender_identity_fingerprint
                    && route.target.authority_fingerprint == verified_target.authority_fingerprint
                    && crate::path_identity::same_object(
                        &route.target.replica_identity,
                        &verified_target.replica_identity,
                    )
            }) {
                return Err(C::AuthorityChanged);
            }
            let session = {
                let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let guard = manager.read().await;
                guard.get_session(session_id).await
            }
            .ok_or(C::SessionRace)?;
            let current_target = crate::config::teams::verify_pty_input_replica_cwd(Path::new(
                &session.working_directory,
            ))
            .map_err(|_| C::AuthorityChanged)?;
            let settings = app.state::<SettingsState>().read().await.clone();
            if current_target.canonical_fqn != claimed.target_fqn
                || current_target.authority_fingerprint != verified_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &current_target.replica_identity,
                    &verified_target.replica_identity,
                )
                || !session_has_current_pty_submission_provenance(
                    &SessionInfo::from(&session),
                    &settings,
                )
                || matches!(session.status, SessionStatus::Exited(_))
            {
                return Err(C::SessionRace);
            }
            let route_valid = {
                let manager = app.state::<Arc<Mutex<PtyManager>>>();
                let manager = manager.lock().unwrap_or_else(|error| error.into_inner());
                manager.has_session(session_id)
                    && manager.backend_kind(session_id) == Some(session.backend_kind)
                    && manager
                        .route_identities(session_id)
                        .is_some_and(|(cwd, replica, _)| {
                            cwd.as_ref().is_some_and(|cwd| {
                                crate::path_identity::is_verified_descendant(
                                    cwd,
                                    &verified_target.replica_identity,
                                )
                            }) && replica.as_ref().is_some_and(|replica| {
                                crate::path_identity::same_object(
                                    replica,
                                    &verified_target.replica_identity,
                                )
                            })
                        })
            };
            if !route_valid {
                return Err(C::SessionRace);
            }
            let readiness = app
                .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                .purge_readiness(&[session_id])
                .into_iter()
                .next()
                .ok_or(C::UntrackedReadiness)?;
            let activity_required = readiness
                .idle_threshold
                .checked_add(std::time::Duration::from_secs(2))
                .ok_or(C::UntrackedReadiness)?;
            let activity_age = readiness.activity_age.ok_or(C::UntrackedReadiness)?;
            let resize_ready = match readiness.last_resize_age {
                None => true,
                Some(age) => readiness
                    .resize_grace
                    .checked_add(std::time::Duration::from_secs(2))
                    .is_some_and(|required| age >= required),
            };
            if session.waiting_for_input
                && readiness.watcher_idle
                && activity_age >= activity_required
                && resize_ready
            {
                return Ok(());
            }
            if !spawned && !session.waiting_for_input {
                return Err(C::Busy);
            }
            if std::time::Instant::now() >= deadline {
                return Err(C::ReadinessTimeout);
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// Process a single outbox message file.
    /// `origin`: the directory the file was found in (#2232 phase 7). The
    /// master/root-token origin is `OutboxOrigin::Master`.
    #[cfg(test)]
    async fn process_message<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        origin: OutboxOrigin,
    ) -> Result<(), String> {
        let content = match classify_outbox_document(path) {
            OutboxClassification::Standard(content) => content,
            OutboxClassification::PrivilegedCandidate { bytes, identity } => {
                if origin == OutboxOrigin::CoManaged {
                    let _ = Self::reject_raw_file(
                        path,
                        "Co-managed origin may carry only a wake; privileged PTY input is rejected",
                    );
                    return Ok(());
                }
                self.process_pty_input_file(
                    app,
                    path,
                    origin == OutboxOrigin::Master,
                    &bytes,
                    &identity,
                )
                .await;
                return Ok(());
            }
            OutboxClassification::InvalidDocument => {
                self.reject_malformed_pty_candidate(path);
                return Ok(());
            }
        };
        self.process_message_content(app, path, origin, &content, None)
            .await
    }

    /// (#1399) `deferred_tail`: `Some(slot)` when the caller owns a worker pool
    /// and wants the delivery tail detached: the validated message is parked in
    /// `slot` and `Ok(())` is returned WITHOUT delivering. `None` runs the tail
    /// inline (the `#[cfg(test)]` `process_message` entry point). Every earlier
    /// rejection return is unaffected because the success type stays `()`.
    async fn process_message_content<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        origin: OutboxOrigin,
        content: &str,
        deferred_tail: Option<&mut Option<ValidatedWake>>,
    ) -> Result<(), String> {
        let is_app_outbox = origin == OutboxOrigin::Master;
        let (msg, outbox_project) = match self
            .parse_and_normalize_outbox(app, path, origin, content)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let authentication = match self.authenticate_outbox(app, path, &msg).await? {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .dispatch_outbox_self_operation(
                app,
                path,
                &msg,
                origin,
                is_app_outbox,
                authentication.4,
            )
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .authorize_outbox_route(app, path, &msg, origin, &outbox_project, authentication)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        let () = match self
            .validate_outbox_action_and_mode(app, path, &msg, is_app_outbox)
            .await?
        {
            std::ops::ControlFlow::Continue(value) => value,
            std::ops::ControlFlow::Break(result) => return result,
        };
        self.deliver_outbox_tail(
            app,
            path,
            msg,
            content,
            (origin, deferred_tail),
            authentication,
        )
        .await
    }

    async fn parse_and_normalize_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        origin: OutboxOrigin,
        content: &str,
    ) -> OutboxStage<(OutboxMessage, Option<String>)> {
        // `let mut msg`: §AR2-norm below mutates `msg.from` / `msg.to` in place
        // as the SINGLE POINT OF TRUTH for canonicalization. Downstream code
        // (routing, action dispatch, injection, archival) reads the canonical
        // form without re-mutation.
        let mut msg: OutboxMessage = serde_json::from_str(content)
            .map_err(|e| format!("Failed to parse outbox message: {}", e))?;

        log::info!(
            "[mailbox] Processing message {} from='{}' to='{}' mode='{}'",
            msg.id,
            msg.from,
            msg.to,
            msg.mode
        );

        let outbox_project = match derive_project_from_outbox_path(path) {
            Ok(project) => project,
            Err(reason) => {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, &msg, &reason).await,
                ));
            }
        };

        // For repo outboxes (not app-outbox), validate that msg.from matches the outbox owner.
        // This prevents tokenless spoofing: a message in repo X's outbox must claim to be from repo
        // X.
        //
        // §DR5 lenient fallback: if `msg.from` is unqualified (legacy) and its LOCAL
        // part matches `expected_from`'s local part, accept — §AR2-norm below then
        // upgrades `msg.from` to the canonical FQN. Cross-project qualified names
        // are rejected (different `project:` prefix).
        //
        // `expected_from` is hoisted (§DR2-3) so §AR2-norm can upgrade `msg.from`
        // even when this block has returned to outer scope.
        let mut expected_from: Option<String> = None;
        if origin == OutboxOrigin::Replica {
            let outbox_dir = path.parent().unwrap_or(Path::new(""));
            // outbox_dir is <repo>/.agentscommander/outbox — go up 2 levels to get the repo path
            if let Some(repo_path) = outbox_dir.parent().and_then(|p| p.parent()) {
                let derived = sender_name_for_session_cwd(&repo_path.to_string_lossy());
                if !anti_spoof_accept(&msg.from, &derived) {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(
                            path,
                            &msg,
                            &format!(
                                "Outbox-sender mismatch: outbox belongs to '{}' but message claims \
                                '{}'",
                                derived, msg.from
                            ),
                        )
                        .await,
                    ));
                }
                expected_from = Some(derived);
            }
        }

        // ── §AR2-norm — SINGLE POINT OF TRUTH: msg.from / msg.to canonicalization ──
        //
        // Runs AFTER anti-spoof and BEFORE token validation / routing / action dispatch.
        // Every downstream read of msg.from / msg.to sees the canonical FQN (or bare
        // legacy form when canonicalization wasn't possible). Downstream code MUST
        // NOT re-mutate msg.from or msg.to.

        // (1) Upgrade a legacy-unqualified msg.from to the anti-spoof-derived
        // expected_from FQN (closes grinch §G5: resolve_repo_path(&msg.from)
        // for response-dir lookup now sees a canonical input).
        let original_from_for_log = msg.from.clone();
        if canonicalize_msg_from_in_place(&mut msg.from, expected_from.as_deref()) {
            log::debug!(
                "[mailbox] canonicalized legacy msg.from '{}' → '{}'",
                original_from_for_log,
                msg.from
            );
        }

        // (2) Canonicalize msg.to via the shared resolver. Empty `to` is allowed for
        // action-dispatch paths (e.g. close-session may set an empty to); skip in
        // that case. Reject-on-ambiguity semantics match the CLI (Decision 2 rule 2c).
        //
        // §AR2-norm D1-a augmentation: when the outbox file lives under a WG-replica
        // layout (`<project>/<workspace>/wg-*/__agent_*/<local-dir>/outbox/<file>.json`),
        // include the derived `<project>` in the in-memory `paths` slice so qualified
        // WG-peer FQNs written by `cli::send` (which performs the symmetric walk-up
        // augmentation — see #228 Step 1) resolve here too. Without this, the daemon
        // re-rejects with `UnknownQualified` even though the CLI side succeeded.
        // settings.json is NOT mutated. In-memory, this-message only. See #228 D1-a.
        match self
            .normalize_outbox_recipient(app, path, &mut msg, &outbox_project)
            .await?
        {
            std::ops::ControlFlow::Continue(()) => {}
            std::ops::ControlFlow::Break(result) => {
                return Ok(std::ops::ControlFlow::Break(result))
            }
        }
        Ok(std::ops::ControlFlow::Continue((msg, outbox_project)))
    }

    async fn normalize_outbox_recipient<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &mut OutboxMessage,
        outbox_project: &Option<String>,
    ) -> OutboxStage {
        if !msg.to.is_empty() {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            if let Some(root_project) = outbox_project.as_ref() {
                let canon_root_project = std::fs::canonicalize(root_project).ok();
                let already_present = paths.iter().any(|p| match &canon_root_project {
                    Some(canon_target) => {
                        std::fs::canonicalize(p).ok().as_ref() == Some(canon_target)
                    }
                    None => p == root_project,
                });
                if !already_present {
                    paths.push(root_project.clone());
                }
            }
            match crate::config::teams::resolve_agent_target(&msg.to, &paths) {
                Ok(fqn) => {
                    if fqn != msg.to {
                        log::debug!("[mailbox] canonicalized msg.to '{}' → '{}'", msg.to, fqn);
                        msg.to = fqn;
                    }
                }
                Err(e) => {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(path, msg, &format!("Unresolvable target: {}", e))
                            .await,
                    ));
                }
            }
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authenticate_outbox_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        token_str: &str,
    ) -> OutboxStage<(bool, bool)> {
        let token_belongs_to_root_agent;
        let saw_session_token;
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;

        if let Ok(token_uuid) = Uuid::parse_str(token_str) {
            match mgr.find_by_token(token_uuid).await {
                None => {
                    // Token is stale/invalid. Env-only credentials cannot be refreshed into
                    // an already-running child process, so reject instead of injecting a new token.
                    drop(mgr);
                    if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                        log::warn!(
                            "[mailbox] Stale token from '{}' matches active session {}, but \
                            env-only credentials cannot be refreshed in-place",
                            msg.from,
                            session_id
                        );
                    }
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(
                            path,
                            msg,
                            "Invalid session token. Env-only credentials cannot be refreshed into \
                            a live process; restart or respawn the sender session.",
                        )
                        .await,
                    ));
                }
                Some(session) => {
                    saw_session_token = true;
                    token_belongs_to_root_agent = session.is_root_agent
                        || crate::config::root_agent::is_root_agent_path(
                            &session.working_directory,
                        );
                    // Anti-spoofing: verify msg.from matches the token's session CWD.
                    // Post-§AR2-norm, msg.from is canonical FQN (or legacy unqualified
                    // if expected_from was unavailable). Session-derived name uses the
                    // canonical helper; comparison is exact equality.
                    let session_name = sender_name_for_session_cwd_with_root_flag(
                        &session.working_directory,
                        token_belongs_to_root_agent,
                    );
                    if session_name != msg.from {
                        log::warn!(
                            "[mailbox] Token-root mismatch: token session='{}' but from='{}'",
                            session_name,
                            msg.from
                        );
                        return Ok(std::ops::ControlFlow::Break(
                            self.reject_message(
                                path,
                                msg,
                                &format!(
                                    "Token-root mismatch: session is '{}' but message claims '{}'",
                                    session_name, msg.from
                                ),
                            )
                            .await,
                        ));
                    }
                }
            }
        } else {
            // Token is not a valid UUID. Env-only credentials cannot be refreshed
            // into an already-running child process, so reject instead of injecting.
            drop(mgr);
            if let Some(session_id) = self.find_active_session(app, &msg.from).await {
                log::warn!(
                    "[mailbox] Malformed token from '{}' matches active session {}, but env-only \
                    credentials cannot be refreshed in-place",
                    msg.from,
                    session_id
                );
            }
            return Ok(std::ops::ControlFlow::Break(self
                .reject_message(
                    path,
                    msg,
                    "Malformed session token. Env-only credentials cannot be refreshed into a live \
                    process; restart or respawn the sender session.",
                )
                .await));
        }
        Ok(std::ops::ControlFlow::Continue((
            token_belongs_to_root_agent,
            saw_session_token,
        )))
    }

    async fn authenticate_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
    ) -> OutboxStage<OutboxAuthenticationFacts> {
        // Check if token is the master token or root token (bypasses anti-spoofing + team
        // validation)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        let root_agent_claim = msg.from == crate::config::root_agent::ROOT_AGENT_SENDER;
        let root_agent_recipient = crate::config::root_agent::is_root_agent_target(&msg.to);
        let mut token_belongs_to_root_agent = false;
        let mut saw_session_token = false;

        if !is_master {
            if let Some(token_str) = msg.token.as_deref() {
                match self
                    .authenticate_outbox_session(app, path, msg, token_str)
                    .await?
                {
                    std::ops::ControlFlow::Continue((root, saw)) => {
                        token_belongs_to_root_agent = root;
                        saw_session_token = saw;
                    }
                    std::ops::ControlFlow::Break(result) => {
                        return Ok(std::ops::ControlFlow::Break(result))
                    }
                }
            }
        }
        Ok(std::ops::ControlFlow::Continue((
            is_master,
            root_agent_claim,
            root_agent_recipient,
            token_belongs_to_root_agent,
            saw_session_token,
        )))
    }

    async fn dispatch_outbox_self_operation<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        origin: OutboxOrigin,
        is_app_outbox: bool,
        saw_session_token: bool,
    ) -> OutboxStage {
        // ── #2232 phase 7: the Co-managed origin may express exactly one thing ──
        // A wake carrying a pointer. `action` and `command` are rejected here,
        // before the pre-routing dispatch below, because `purge-wg` and
        // `close-session` are destructive verbs and this origin is written by
        // the in-process supervisor, not by a token-bearing caller. The same
        // fields from a replica outbox keep today's behaviour untouched.
        if origin == OutboxOrigin::CoManaged && (msg.action.is_some() || msg.command.is_some()) {
            log::warn!(
                "[mailbox] Co-managed origin rejected a message carrying action={:?} command={:?} \
                msg={}",
                msg.action,
                msg.command,
                msg.id
            );
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(
                    path,
                    msg,
                    "Co-managed origin may carry only a wake; action and command are rejected",
                )
                .await,
            ));
        }

        // ── #617 self-clear: token-authorized self-operation ──
        // Dispatched BEFORE the team-routing chain below. self-clear clears the
        // CALLER'S OWN context; it is authorized solely by session-token ownership
        // (proved just above: find_by_token + from==session_name anti-spoof), not by
        // "can A reach B" team rules. Routing it through can_communicate would wrongly
        // reject valid callers in no shared team. close-session stays in the
        // post-routing action block because it IS a cross-agent operation.
        if msg.action.as_deref() == Some(SELF_CLEAR_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_clear(app, path, msg, is_app_outbox).await,
            ));
        }
        if msg.action.as_deref() == Some(SELF_SWITCH_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_handoff_switch(app, path, msg, is_app_outbox)
                    .await,
            ));
        }
        // (#1632) Same pre-routing block, same reason: self-handoff-and-restart operates on the
        // CALLER'S OWN session and is authorized by token ownership, not by team reachability.
        if msg.action.as_deref() == Some(SELF_RESTART_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_self_restart(app, path, msg, is_app_outbox)
                    .await,
            ));
        }
        if msg.action.as_deref() == Some(RAISE_HAND_ACTION) {
            return Ok(std::ops::ControlFlow::Break(
                self.handle_raise_hand(app, path, msg, is_app_outbox).await,
            ));
        }

        // (#885 F-7) `purge-wg` dispatches pre-routing, after the anti-spoof block.
        // `msg.from` is anti-spoof-verified ONLY inside the `if !is_master` block
        // above, so it is unverified both for a tokenless message AND for a
        // master-token message. `saw_session_token` is the single bit that proves
        // `msg.from` was checked against a live session's CWD. It is NOT disjoined
        // with `is_master`: a master-token message would sail through with an
        // attacker-chosen `msg.from`, and `verified_wg_coordinator_target` would
        // resolve against ANY workgroup on disk. Root has no workgroup.
        if msg.action.as_deref() == Some(PURGE_WG_ACTION) {
            if !saw_session_token {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "purge-room requires a session token")
                        .await,
                ));
            }
            return Ok(std::ops::ControlFlow::Break(
                self.handle_purge_wg(app, path, msg, is_app_outbox).await,
            ));
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_comanaged_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        root_agent_recipient: bool,
    ) -> OutboxStage {
        // #2232 phase 7: no token, no master path, no anti-spoof. The FQN in
        // `from` is the credential and the destination check is the CLI's:
        // Root through the verified-coordinator validator, everything else
        // through `can_communicate`. An unreachable destination is rejected.
        let mut paths = {
            let cfg = app.state::<SettingsState>();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        // The queue lives under a room root, whose grandparent is the project
        // directory; include it so a qualified coordinator FQN resolves even
        // when the project is not in settings (session-derived rooms).
        if let Some(project_dir) = crate::config::co_managed::room_root_for_path(path)
            .and_then(|room_root| room_root.parent().map(|ac_root| ac_root.to_path_buf()))
            .and_then(|ac_root| ac_root.parent().map(|project| project.to_path_buf()))
        {
            let canon_project = std::fs::canonicalize(&project_dir).ok();
            let already_present = paths.iter().any(|p| match &canon_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => std::path::Path::new(p) == project_dir,
            });
            if !already_present {
                paths.push(project_dir.to_string_lossy().to_string());
            }
        }

        if root_agent_recipient {
            if let Err(reason) = validate_coordinator_to_root_route(&msg.from, &paths) {
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, reason).await,
                ));
            }
        } else {
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Co-managed routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "Sender cannot reach destination")
                        .await,
                ));
            }
        }
        log::debug!(
            "[mailbox] Co-managed routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_root_sender_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
        authentication: (bool, bool, bool),
    ) -> OutboxStage {
        let (is_master, saw_session_token, token_belongs_to_root_agent) = authentication;
        let mut paths = {
            let cfg = app.state::<SettingsState>();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        if let Some(root_project) = outbox_project.as_ref() {
            let canon_root_project = std::fs::canonicalize(root_project).ok();
            let already_present = paths.iter().any(|p| match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => p == root_project,
            });
            if !already_present {
                paths.push(root_project.clone());
            }
        }

        if let Err(reason) = validate_root_sender_route(
            &msg.to,
            &paths,
            is_master,
            saw_session_token,
            token_belongs_to_root_agent,
        ) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        if let Err(reason) = validate_root_sender_payload(msg) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, &reason).await,
            ));
        }
        log::debug!(
            "[mailbox] Root Agent routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_root_recipient_outbox<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        outbox_project: &Option<String>,
    ) -> OutboxStage {
        // #293 — coordinator → root recipient.
        //
        // Build the same effective_project_paths slice the root-sender
        // branch uses (settings + the outbox file's WG-replica project
        // walk-up) so verified_wg_coordinator_target sees the project
        // where the outbox lives.
        let mut paths = {
            let cfg = app.state::<SettingsState>();
            let c = cfg.read().await;
            c.project_paths.clone()
        };
        if let Some(root_project) = outbox_project.as_ref() {
            let canon_root_project = std::fs::canonicalize(root_project).ok();
            let already_present = paths.iter().any(|p| match &canon_root_project {
                Some(canon_target) => std::fs::canonicalize(p).ok().as_ref() == Some(canon_target),
                None => p == root_project,
            });
            if !already_present {
                paths.push(root_project.clone());
            }
        }

        // Master/root token does NOT bypass the verified-coordinator check
        // for root-recipient: the URI is meaningful only when paired with
        // a real coordinator identity, and the verified check is cheap.
        if let Err(reason) = validate_coordinator_to_root_route(&msg.from, &paths) {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(path, msg, reason).await,
            ));
        }
        log::debug!(
            "[mailbox] Coordinator→Root routing check passed: '{}' -> '{}'",
            msg.from,
            msg.to
        );
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn authorize_outbox_route<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        origin: OutboxOrigin,
        outbox_project: &Option<String>,
        authentication: OutboxAuthenticationFacts,
    ) -> OutboxStage {
        let (
            is_master,
            root_agent_claim,
            root_agent_recipient,
            token_belongs_to_root_agent,
            saw_session_token,
        ) = authentication;
        if origin == OutboxOrigin::CoManaged {
            return self
                .authorize_comanaged_outbox(app, path, msg, root_agent_recipient)
                .await;
        } else if root_agent_claim {
            return self
                .authorize_root_sender_outbox(
                    app,
                    path,
                    msg,
                    outbox_project,
                    (is_master, saw_session_token, token_belongs_to_root_agent),
                )
                .await;
        } else if root_agent_recipient {
            return self
                .authorize_root_recipient_outbox(app, path, msg, outbox_project)
                .await;
        } else if is_master {
            log::debug!(
                "[mailbox] Master token used — bypassing team validation for {} -> {}",
                msg.from,
                msg.to
            );
        } else {
            let discovered_teams = teams::discover_teams();
            if !self.can_reach(&msg.from, &msg.to, &discovered_teams) {
                log::warn!(
                    "[mailbox] Routing check FAILED: '{}' cannot reach '{}'",
                    msg.from,
                    msg.to
                );
                return Ok(std::ops::ControlFlow::Break(
                    self.reject_message(path, msg, "Sender cannot reach destination")
                        .await,
                ));
            }
            log::debug!(
                "[mailbox] Routing check passed: '{}' -> '{}'",
                msg.from,
                msg.to
            );
        }

        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn validate_outbox_action_and_mode<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> OutboxStage {
        // Action-based dispatch (close-session, etc.) — handled before mode-based delivery
        if let Some(ref action) = msg.action {
            match action.as_str() {
                "close-session" => {
                    // §224 G-IMPL-2 — thread `is_app_outbox` so the response-
                    // write path can skip the outbox-relative primary write
                    // when the message came from the app-outbox (master-token
                    // path). That derived path lands under
                    // <config_dir>/instances/<id>/responses/ which the CLI
                    // never polls, so writing there leaks orphan JSON files.
                    return Ok(std::ops::ControlFlow::Break(
                        self.handle_close_session(app, path, msg, is_app_outbox)
                            .await,
                    ));
                }
                _ => {
                    return Ok(std::ops::ControlFlow::Break(
                        self.reject_message(path, msg, &format!("Unknown action '{}'", action))
                            .await,
                    ));
                }
            }
        }

        // Deliver based on mode — all modes require immediate delivery or rejection
        let mode = if msg.mode.is_empty() {
            "wake"
        } else {
            msg.mode.as_str()
        };
        // Only `wake` is supported. Defensive check for malformed outbox files
        // that might arrive from external (root-token) write paths.
        if mode != "wake" {
            return Ok(std::ops::ControlFlow::Break(
                self.reject_message(
                    path,
                    msg,
                    &format!("Unsupported delivery mode '{}'. Valid: wake", mode),
                )
                .await,
            ));
        }
        Ok(std::ops::ControlFlow::Continue(()))
    }

    async fn deliver_outbox_tail<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: OutboxMessage,
        content: &str,
        delivery: (OutboxOrigin, Option<&mut Option<ValidatedWake>>),
        authentication: OutboxAuthenticationFacts,
    ) -> Result<(), String> {
        let (origin, deferred_tail) = delivery;
        let (is_master, _, _, token_belongs_to_root_agent, saw_session_token) = authentication;
        let response = self
            .admit_response_source(
                app,
                path,
                &msg,
                saw_session_token
                    && !is_master
                    && !token_belongs_to_root_agent
                    && origin != OutboxOrigin::CoManaged
                    && msg.command.is_none()
                    && !msg.get_output
                    && msg.pty_input.is_none(),
                content,
            )
            .await;
        let validated = ValidatedWake {
            message: msg,
            response,
        };
        let _candidate_disposal = validated
            .response
            .as_ref()
            .map(ResponseWakeWork::candidate_disposal);
        // #2232 phase 7: a Co-managed message is delivered inline with its
        // display attribution; it is never parked for the detached worker pool,
        // whose shared `deliver_wake` tail carries no attribution and must keep
        // its exact behaviour for every other origin.
        if origin == OutboxOrigin::CoManaged {
            let annotated = self.deliver_validated_wake(app, &validated, true).await?;
            return self.move_to_delivered(app, path, &annotated).await;
        }
        if let Some(slot) = deferred_tail {
            *slot = Some(validated);
            return Ok(());
        }
        // #1635-2 D4: write the annotated message so the `Delivered:` CLI
        // receipt can report the effective agent/profile. No-flag messages are
        // still byte-identical to today.
        let annotated = self.deliver_validated_wake(app, &validated, false).await?;

        // Move to delivered/ with token stripped
        self.move_to_delivered(app, path, &annotated).await
    }

    /// Deliver mode: wake — inject into the recipient's PTY for any non-Exited
    /// session; destroy and respawn if Exited; spawn persistent if none. Always
    /// delivers (no busy-gate — stdin buffer absorbs input while the agent is
    /// mid-turn).
    // #791: `pub(crate)` so the in-daemon control-plane API can funnel through
    // the SAME actuation the poller uses (no fork). The API calls this on a
    // throwaway `MailboxPoller::new()` (delivery-stateless: this method and its
    // whole `&self` callee chain read only `app.state::<...>()`, never the
    // poller's `poll_interval` / `retry_tracker` fields). See plan #791 §0.5 HIGH-1.
    pub(crate) async fn deliver_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_with_origin(app, msg, WakeDeliveryOrigin::FilesystemPoller)
            .await
    }

    pub(crate) async fn deliver_wake_with_origin<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::Peer {
                message: msg,
                origin,
                comanaged: false,
                response: None,
            },
        )
        .await
    }

    /// #2232 phase 7: deliver a wake whose origin is the Co-managed provenance
    /// queue. `comanaged` is display-only and never part of routing; the
    /// existing entry points stay attribution-free.
    async fn deliver_wake_with_attribution<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Result<OutboxMessage, String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::Peer {
                message: msg,
                origin,
                comanaged,
                response,
            },
        )
        .await
    }

    /// Single private entry for every wake delivery kind. The variant is selected only by
    /// code: serialized peer messages can enter only `Peer`, while the validated target,
    /// notice, cancellation, and guard capability are required for `InternalSystem`.
    /// Returns the annotated `OutboxMessage` for the `Delivered:` receipt; the
    /// internal-system arm builds its own envelope (it holds no serialized message).
    async fn deliver_validated_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        validated: &ValidatedWake,
        comanaged: bool,
    ) -> Result<OutboxMessage, String> {
        if !comanaged && validated.response.is_none() {
            return self.deliver_wake(app, &validated.message).await;
        }
        self.deliver_wake_with_attribution(
            app,
            &validated.message,
            WakeDeliveryOrigin::FilesystemPoller,
            comanaged,
            validated.response.as_ref(),
        )
        .await
    }

    async fn deliver_wake_engine<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        delivery: WakeDelivery<'_>,
    ) -> Result<OutboxMessage, String> {
        let target_fqn = match &delivery {
            WakeDelivery::Peer { message, .. } => message.to.as_str(),
            WakeDelivery::InternalSystem { target, .. } => target.fqn(),
        };
        let _response_delivery =
            match app.try_state::<crate::api::message_store::MessageStoreState>() {
                Some(state) => match &state.store {
                    Ok(store) => Some(store.begin_response_delivery(target_fqn)?),
                    Err(_) => None,
                },
                None => None,
            };
        match delivery {
            WakeDelivery::Peer {
                message,
                origin,
                comanaged,
                response,
            } => Box::pin(self.deliver_peer_wake(app, message, origin, comanaged, response)).await,
            WakeDelivery::InternalSystem {
                target,
                notice,
                cancellation,
                guard,
            } => {
                // The internal-system envelope never carries dispatch flags, so
                // the fresh unannotated envelope is the correct delivered
                // message. Built up front because the delivery below moves
                // `target`; the delivery MUST still run (context alerts,
                // notices, cancellations, guard).
                let delivered = internal_system_envelope(&target);
                self.deliver_internal_system_wake(app, target, notice, cancellation, guard)
                    .await?;
                Ok(delivered)
            }
        }
    }

    async fn deliver_peer_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        response: Option<&ResponseWakeWork>,
    ) -> Result<OutboxMessage, String> {
        // Parse a hand-authored logical action after process_message's
        // authorization/routing gates but before any recipient actuation.
        let parsed_remote_command = msg
            .command
            .as_deref()
            .map(parse_remote_pty_command)
            .transpose()?;

        // #1635-2 D5: a wake carries dispatch flags when it names a specific
        // agent (empty string and `auto` are the no-flag shapes — the API/DB
        // path writes `preferred_agent: String::new()`) or requests a profile
        // letter. Flagged wakes suppress the spawn-time config.json writes
        // (skip_tooling_save) and populate the delivered-receipt annotations.
        let flags_present = (!msg.preferred_agent.is_empty() && msg.preferred_agent != "auto")
            || msg.requested_profile.is_some();

        // #1635-2 D6: a present requestedProfile that is not a single A-Z
        // letter is a permanent delivery error. The CLI rejects it before
        // enqueue (1635-1); this defends hand-written outbox JSON.
        if let Some(letter) = msg.requested_profile.as_deref() {
            if crate::config::settings::normalize_profile_letter(letter).is_none() {
                return Err(ERR_INVALID_DISPATCH_PROFILE.to_string());
            }
        }

        // (#885 J2) A purge is destroying this agent's record right now. A wake
        // delivered into that window falls through to spawn-persistent below and
        // would cold-spawn the agent we are purging, silently breaking the verb's
        // postcondition. This is a BACKSTOP, not the primary defense: the DB
        // dispatcher must skip its tick before leasing (see `api/dispatcher.rs`,
        // #885 F-5); reaching this Err from there would burn an attempt and can
        //   POISON the message. The one caller for which this Err is safe:
        //   - filesystem poller: non-permanent error, retried at the 3s poll
        //     interval up to MAX_DELIVERY_ATTEMPTS. Deferred, not lost.
        //   The inline API send that used to map this to a rejected outcome no
        //   longer exists (#1177), so the DB dispatcher is now the only other
        //   caller, and it is exactly the one F-5 must keep out of this window.
        if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>()
        {
            if g.blocks_agent(&msg.to) {
                return Err(format!(
                    "purge-room in progress for '{}'; wake deferred",
                    msg.to
                ));
            }
        }

        loop {
            let fallback = match self
                .select_peer_wake_candidate(
                    app,
                    msg,
                    (origin, comanaged),
                    response,
                    (parsed_remote_command.is_some(), flags_present),
                )
                .await?
            {
                std::ops::ControlFlow::Break(delivered) => return Ok(delivered),
                std::ops::ControlFlow::Continue(fallback) => fallback,
            };
            if let Some(previous) = fallback.1 {
                if self
                    .peer_wake_needs_owned_replacement(app, msg, previous)
                    .await
                {
                    match self
                        .deliver_owned_peer_replacement(
                            app,
                            msg,
                            (origin, comanaged),
                            response,
                            (parsed_remote_command.is_some(), flags_present),
                            previous,
                        )
                        .await?
                    {
                        OwnedPeerWakeStep::Delivered(delivered) => return Ok(delivered),
                        OwnedPeerWakeStep::Relist => {
                            tokio::task::yield_now().await;
                            continue;
                        }
                        OwnedPeerWakeStep::Retained => return std::future::pending().await,
                    }
                }
            }
            return self
                .deliver_peer_wake_fallback(
                    app,
                    msg,
                    (origin, comanaged),
                    response,
                    (parsed_remote_command.is_some(), flags_present),
                    fallback,
                )
                .await;
        }
    }

    async fn peer_wake_needs_owned_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        previous: Uuid,
    ) -> bool {
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let Some(row) = manager.get_session(previous).await else {
            return false;
        };
        let Ok(directory) =
            crate::path_identity::verify_directory(Path::new(&row.working_directory))
        else {
            return true;
        };
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        !matches!(
            crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                &permit,
                Some(&msg.to),
                &directory.canonical_path,
            ),
            crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
        )
    }

    async fn owned_peer_source_is_current<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
        physical: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        if app
            .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .is_some_and(|purge| purge.blocks_agent(&msg.to))
        {
            return false;
        }
        let settings = app.state::<SettingsState>().read().await.clone();
        let master = msg.token.as_deref().is_some_and(|token| {
            app.try_state::<MasterToken>()
                .is_some_and(|master| master.matches(token))
                || settings.root_token.as_deref() == Some(token)
        });
        if !master {
            if let Some(token) = msg.token.as_deref() {
                let Ok(token) = Uuid::parse_str(token) else {
                    return false;
                };
                let manager = app
                    .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                    .read()
                    .await
                    .clone();
                let Some(sender) = manager.find_by_token(token).await else {
                    return false;
                };
                if sender_name_for_session_cwd_with_root_flag(
                    &sender.working_directory,
                    sender.is_root_agent,
                ) != msg.from
                {
                    return false;
                }
            }
            if !teams::can_communicate(&msg.from, &msg.to, &teams::discover_teams()) {
                return false;
            }
        }
        let Ok(current_physical) = crate::path_identity::verify_directory(Path::new(&request.cwd))
        else {
            return false;
        };
        if !crate::path_identity::same_object(&current_physical, physical)
            || teams::is_coordinator_for_cwd(&request.cwd, &teams::discover_teams())
            || crate::config::root_agent::is_root_agent_path(&request.cwd)
        {
            return false;
        }
        let Ok(current) = self.resolve_wake_spawn_plan(app, msg).await else {
            return false;
        };
        if crate::path_utils::normalize_windows_verbatim_path(&current.cwd) != request.cwd
            || current.spawn_shell != request.shell
            || current.spawn_args != request.shell_args
            || current.resolved_command.agent_id != request.agent_id
            || current.spawn_label != request.agent_label
            || current.resolved_agent_host_shell != request.host_shell
        {
            return false;
        }
        match (
            current.configured_spawn.as_ref(),
            request.resolved_spawn.as_ref(),
        ) {
            (Some(current), Some(expected)) => {
                current.profile_content_hash == expected.profile_content_hash
                    && current.profile_resolution == expected.profile_resolution
                    && current.trusted_agent_id == expected.trusted_agent_id
                    && current.trusted_agent_label == expected.trusted_agent_label
                    && current.shell == expected.shell
                    && current.shell_args == expected.shell_args
                    && current.env_remove_keys == expected.env_remove_keys
                    && current.effective_codex_home == expected.effective_codex_home
                    && current.effective_child_env() == expected.effective_child_env()
                    && match (current.seed.as_ref(), expected.seed.as_ref()) {
                        (None, None) => true,
                        (Some(current), Some(expected)) => {
                            current.dest == expected.dest
                                && current.candidates == expected.candidates
                                && current.context == expected.context
                                && current.config_dir_warning == expected.config_dir_warning
                        }
                        _ => false,
                    }
                    && SessionBackendKind::from(&current.backend)
                        == SessionBackendKind::LocalProcess
            }
            (None, None) => true,
            _ => false,
        }
    }

    async fn admit_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        request: &crate::commands::session::OwnedReplacementCreateRequest,
        previous_id: Uuid,
    ) -> Result<Option<OwnedPeerTarget>, String> {
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.before_source_accept(&msg.id).await {
                return Ok(None);
            }
        }
        use crate::session::selection::{
            OwnedReplacementWork, ReplacementAdmissionStep, ResolvedReplacement,
        };
        let coordinator = app.state::<crate::session::selection::SelectionCoordinator>();
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let gate = app
            .try_state::<crate::api::message_store::MessageStoreState>()
            .and_then(|state| state.target_gate.as_ref().ok().cloned())
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let directory =
            crate::config::config_dir().ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let physical = crate::path_identity::verify_directory(Path::new(&request.cwd))
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let replica = teams::strict_wg_replica_anchor_from_cwd(Path::new(&request.cwd))
            .ok()
            .flatten()
            .and_then(|root| crate::path_identity::verify_directory(&root).ok())
            .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
        let payload = serde_json::to_value(msg).map_err(|error| error.to_string())?;
        let work = OwnedReplacementWork::OrdinaryOwned {
            message_id: msg.id.clone(),
            body: msg.body.clone(),
            payload: payload.clone(),
            recipient: msg.to.clone(),
            request: payload,
        };
        let mut prepared = None;
        loop {
            coordinator.wait_for_restore_release().await?;
            if !self
                .owned_peer_source_is_current(app, msg, request, &physical)
                .await
            {
                return Err("Wake source authority changed before replacement".into());
            }
            let stripe = gate
                .acquire_target_lock(&msg.to)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(&msg.to).await;
            let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
            let captured = manager
                .with_current_carrier_snapshot(&permit, |current| {
                    let scope =
                        crate::config::sessions_persistence::classify_lifecycle_carrier_scope(
                            &permit,
                            Some(&msg.to),
                            &physical.canonical_path,
                        );
                    let foreign = matches!(
                        scope,
                        crate::config::sessions_persistence::CreateCarrierScope::Foreign { .. }
                    );
                    let carrier =
                        crate::config::sessions_persistence::retained_carrier_for_replacement(
                            &permit,
                            &directory,
                            &msg.to,
                            previous_id,
                            current,
                        );
                    let previous = current
                        .rows
                        .iter()
                        .find(|row| row.id == previous_id)
                        .cloned();
                    let old_missing = previous.is_none();
                    (foreign || old_missing, carrier.zip(previous))
                })
                .await;
            drop(permit);
            if captured.0 {
                return Ok(None);
            }
            if prepared.is_none() {
                if let Some((carrier, previous)) = captured.1 {
                    prepared = Some(coordinator.prepare_owned_replacement(
                        ResolvedReplacement {
                            target: msg.to.clone(),
                            directory: directory.clone(),
                            previous,
                            carrier,
                            requested_agent_id: request.agent_id.clone(),
                            requested_profile: msg.requested_profile.clone(),
                            shell: request.shell.clone(),
                            shell_args: request.shell_args.clone(),
                            launch_recipe: owned_replacement_launch_recipe(request),
                            cwd: request.cwd.clone(),
                            backend: SessionBackendKind::LocalProcess,
                            work: work.clone(),
                            cwd_identity: physical.clone().into(),
                            replica_identity: Some(replica.clone().into()),
                        },
                        manager.reserve_carrier_create_binding(),
                    ));
                }
            }
            if let Some(candidate) = prepared.take() {
                let tracker = pty
                    .lock()
                    .map_err(|_| "targetCreateGateUnavailable".to_string())?
                    .response_close_state();
                let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
                let admission = manager
                    .with_current_carrier_snapshot(&permit, |current| {
                        coordinator
                            .admit_owned_replacement(&permit, current, &tracker, &pty, candidate)
                    })
                    .await;
                drop(permit);
                match admission? {
                    ReplacementAdmissionStep::Accepted(continuation) => {
                        return Ok(Some(OwnedPeerTarget {
                            continuation,
                            gate,
                            exact,
                            stripe,
                            physical,
                        }))
                    }
                    ReplacementAdmissionStep::Waiting(candidate) => prepared = Some(candidate),
                }
            }
            drop(exact);
            drop(stripe);
            tokio::task::yield_now().await;
        }
    }

    async fn deliver_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
        previous: Uuid,
    ) -> Result<OwnedPeerWakeStep, String> {
        let plan = self.resolve_wake_spawn_plan(app, msg).await?;
        if let Some(command) = msg.command.as_deref() {
            resolve_remote_pty_command(&plan.spawn_shell, command)?;
        }
        if plan.configured_spawn.as_ref().is_some_and(|spawn| {
            SessionBackendKind::from(&spawn.backend) != SessionBackendKind::LocalProcess
        }) {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let request = crate::commands::session::OwnedReplacementCreateRequest {
            shell: plan.spawn_shell,
            shell_args: plan.spawn_args,
            cwd: crate::path_utils::normalize_windows_verbatim_path(&plan.cwd),
            name: Some(plan.session_name),
            agent_id: plan.resolved_command.agent_id.clone(),
            agent_label: plan.spawn_label,
            skip_tooling_save: dispatch.1,
            git_repos: Vec::new(),
            resolved_spawn: plan.configured_spawn,
            host_shell: plan.resolved_agent_host_shell,
        };
        let Some(target) = self
            .admit_owned_peer_replacement(app, msg, &request, previous)
            .await?
        else {
            return Ok(OwnedPeerWakeStep::Relist);
        };
        let validation_app = app.clone();
        let validation_message = msg.clone();
        let validation_request = request.clone();
        let physical = target.physical;
        let continuation = target
            .continuation
            .with_source_validation(Arc::new(move || {
                let app = validation_app.clone();
                let msg = validation_message.clone();
                let request = validation_request.clone();
                let physical = physical.clone();
                Box::pin(async move {
                    if MailboxPoller::delivery_only()
                        .owned_peer_source_is_current(&app, &msg, &request, &physical)
                        .await
                    {
                        Ok(())
                    } else {
                        Err(crate::session::selection::OwnedSourceStop::AuthorityChanged)
                    }
                })
            }));
        #[cfg(test)]
        let continuation = match self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            Some(control) => continuation.with_fixture_control(control),
            None => continuation,
        };
        #[cfg(test)]
        continuation.fixture_inserted();
        let mut execution = OwnedPeerWakeExecution {
            message: msg.clone(),
            delivery,
            response: response.cloned(),
            flags_present: dispatch.1,
            request,
            resolved_command: plan.resolved_command,
            continuation,
            gate: target.gate,
            exact: Some(target.exact),
            stripe: Some(target.stripe),
        };
        let completion = execution.continuation.clone();
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        let app = app.clone();
        let delivery = self.delivery_poller();
        // Relocates the one existing create core around its owned work tail.
        // Common create borrows this lexical proof and never spawns another core.
        let result = crate::config::sessions_persistence::run_owned_persist_core(async move {
            let mut source = crate::session::selection::SourceExecutorGuard::unbound();
            if !source.bind(&execution.continuation) {
                return Ok(OwnedPeerWakeStep::Retained);
            }
            let result = delivery
                .execute_owned_peer_replacement(&app, &mut execution)
                .await;
            source.returned(result.is_err());
            result
        })
        .await;
        #[cfg(test)]
        if matches!(&result, Ok(OwnedPeerWakeStep::Retained)) {
            // The accepted core has actually returned; its public wake caller
            // intentionally remains pending below. This is observation only.
            let _ = completion
                .fixture_reached(
                    crate::session::selection::ReplacementFixtureCut::SourceRetainedReturn,
                )
                .await;
        }
        // The existing result channel has returned the real legacy Err to the
        // source-owning caller. The completed core has released its target
        // guards; the original full message remains with this caller.
        if result.is_err() && completion.precommit_rollback_completed() {
            if let Some(effect) = completion
                .start_effect(crate::session::selection::LifecycleEffectSlot::WorkTransfer)
            {
                completion
                    .finish_rolled_back_work(&manager, &pty, effect, true)
                    .await;
            }
        }
        result
    }

    async fn execute_owned_peer_replacement<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        execution: &mut OwnedPeerWakeExecution,
    ) -> Result<OwnedPeerWakeStep, String> {
        use crate::session::selection::LifecycleEffectSlot;
        #[cfg(test)]
        let fixture_control = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control);
        #[cfg(test)]
        let fixture_continuation = match &fixture_control {
            Some(control) => execution
                .continuation
                .clone()
                .with_fixture_control(control.clone()),
            None => execution.continuation.clone(),
        };
        #[cfg(test)]
        let continuation = &fixture_continuation;
        #[cfg(not(test))]
        let continuation = &execution.continuation;
        #[cfg(test)]
        if !continuation
            .fixture_reached(crate::session::selection::ReplacementFixtureCut::SourceCoreEntered)
            .await
        {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let manager = app
            .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
            .read()
            .await
            .clone();
        let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
        if !continuation.validate_source().await {
            return Ok(OwnedPeerWakeStep::Retained);
        }
        if let Some(spawn) = execution.request.resolved_spawn.as_ref() {
            if !continuation
                .validate_pre_effect_region(&manager, &pty)
                .await
            {
                return Ok(OwnedPeerWakeStep::Retained);
            }
            let Some(effect) = continuation.start_effect(LifecycleEffectSlot::ContextPreparation)
            else {
                return Ok(OwnedPeerWakeStep::Retained);
            };
            let prepared = crate::config::agent_command::prepare_agent_spawn_command(spawn);
            effect.complete(prepared.as_ref().map(|_| None).map_err(Clone::clone));
            if let Err(error) = prepared {
                continuation
                    .rollback_failed_create(&manager, &pty, &error)
                    .await;
                return Err(error);
            }
            if !continuation.preserve_context_prerequisite() {
                return Ok(OwnedPeerWakeStep::Retained);
            }
        }
        #[cfg(test)]
        let fixture = fixture_control.filter(|control| {
            !control
                .ticket_only
                .load(std::sync::atomic::Ordering::Acquire)
        });
        #[cfg(test)]
        let fixture_created = if let Some(control) = fixture {
            let (Some(exact), Some(stripe)) = (execution.exact.take(), execution.stripe.take())
            else {
                return Ok(OwnedPeerWakeStep::Retained);
            };
            let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            else {
                return Ok(OwnedPeerWakeStep::Retained);
            };
            let input = crate::commands::session::FixtureReplacementInput {
                request: execution.request.clone(),
                continuation: continuation.clone().with_fixture_control(control.clone()),
                lease: crate::commands::session::FixtureReplacementLease::OrdinaryMoved(
                    crate::commands::session::FixturePtyCutLease {
                        gate: execution.gate.clone(),
                        target: execution.message.to.clone(),
                        exact,
                        stripe,
                    },
                ),
            };
            let Some(result) = coordinator
                .submit_inline_replacement_fixture(input, control)
                .await
            else {
                return Ok(OwnedPeerWakeStep::Retained);
            };
            let Some(lease) = result.ordinary_lease else {
                return Ok(OwnedPeerWakeStep::Retained);
            };
            if !Arc::ptr_eq(&lease.gate, &execution.gate) || lease.target != execution.message.to {
                return Ok(OwnedPeerWakeStep::Retained);
            }
            execution.exact = Some(lease.exact);
            execution.stripe = Some(lease.stripe);
            Some(result.outcome)
        } else {
            None
        };
        #[cfg(test)]
        let created = match fixture_created {
            Some(created) => created,
            None => {
                let proof = execution
                    .gate
                    .target_ownership(
                        &execution.message.to,
                        execution.stripe.as_ref().ok_or("missingTargetStripe")?,
                        execution.exact.as_ref().ok_or("missingTargetOwnership")?,
                    )
                    .map_err(|_| "invalidTargetCreateOwnership".to_string())?;
                crate::commands::session::create_owned_response_replacement(
                    app,
                    execution.request.clone(),
                    continuation.clone(),
                    &proof,
                )
                .await
            }
        };
        #[cfg(not(test))]
        let created = {
            let proof = execution
                .gate
                .target_ownership(
                    &execution.message.to,
                    execution.stripe.as_ref().ok_or("missingTargetStripe")?,
                    execution.exact.as_ref().ok_or("missingTargetOwnership")?,
                )
                .map_err(|_| "invalidTargetCreateOwnership".to_string())?;
            crate::commands::session::create_owned_response_replacement(
                app,
                execution.request.clone(),
                continuation.clone(),
                &proof,
            )
            .await
        };
        match created {
            Ok(crate::commands::session::OwnedReplacementCreateStep::Finalized(_)) => {}
            Ok(crate::commands::session::OwnedReplacementCreateStep::Retained) => {
                return Ok(OwnedPeerWakeStep::Retained)
            }
            Err(error) => return Err(error),
        }
        let Some(info) = continuation.refresh_completion_info(&manager, &pty).await else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        let id = continuation.binding().map(|binding| binding.session_id());
        let Some(id) = id else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        // Start the work receipt only after its initial current-B checks. A
        // failed preflight has reached no bridge, settle or input effect.
        let Some(effect) = continuation.start_effect(LifecycleEffectSlot::WorkTransfer) else {
            return Ok(OwnedPeerWakeStep::Retained);
        };
        if info.telegram_bot_id.is_some() {
            self.attach_persisted_telegram_for_wake(app, id, info.telegram_bot_id.as_deref())
                .await;
        }
        let settle = match self.wait_for_spawned_wake_idle(app, id).await {
            Ok(settle) => settle,
            Err(error) => {
                effect.complete(Err(error.clone()));
                continuation.note_retained_work(error);
                return Ok(OwnedPeerWakeStep::Retained);
            }
        };
        if continuation
            .refresh_completion_info(&manager, &pty)
            .await
            .is_none()
        {
            // The bridge/settle calls returned; no input was attempted. Keep
            // their known incomplete-work result instead of an unwind receipt.
            effect.complete(Err("replacement work stopped before input".into()));
            return Ok(OwnedPeerWakeStep::Retained);
        }
        let injected = self
            .inject_wake_into_pty(
                app,
                id,
                &execution.message,
                execution.delivery,
                settle,
                (execution.response.as_ref(), Some(continuation)),
            )
            .await;
        if let Err(error) = injected {
            effect.complete(Err(error.clone()));
            continuation.note_retained_work(error);
            return Ok(OwnedPeerWakeStep::Retained);
        }
        drop(execution.exact.take());
        drop(execution.stripe.take());
        continuation
            .finish_observed_work_transfer(&manager, &pty, effect, true)
            .await;
        let mut delivered = execution.message.clone();
        if execution.flags_present {
            if let Some(spawn) = execution.request.resolved_spawn.as_ref() {
                delivered.effective_agent_id = Some(spawn.trusted_agent_id.clone());
                delivered.effective_profile =
                    Some(spawn.profile_resolution.effective_profile.clone());
                delivered.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
            } else {
                delivered.effective_agent_id = execution.resolved_command.agent_id.clone();
            }
        }
        Ok(OwnedPeerWakeStep::Delivered(delivered))
    }

    async fn try_inject_peer_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        session_id: Uuid,
        dispatch: (bool, bool),
    ) -> Result<Option<OutboxMessage>, String> {
        let (origin, comanaged) = delivery;
        let (logical_command, flags_present) = dispatch;
        if logical_command {
            let shell = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.get_session(session_id)
                    .await
                    .map(|session| session.shell)
            };
            let Some(shell) = shell else {
                log::warn!(
                    "[mailbox] wake: candidate {} vanished during logical-command preflight, \
                    trying next",
                    session_id
                );
                return Ok(None);
            };
            if let Some(wire_value) = msg.command.as_deref() {
                resolve_remote_pty_command(&shell, wire_value)?;
            }
        }

        // (#1001 PR2 / B) Settle the live session to paste-ready before
        // injecting. The MEASURED live-path drop is the still-starting
        // case (100% here / 83% baseline, P2), which the alive_age
        // Starting route closes via the #611 settle. The established
        // fresh-idle guard is DEFENSIVE retention (bias-to-deliver, cannot
        // drop - the cap always injects), not a measured-drop closer; the
        // ~75% figure was PR1's COLD-SPAWN fresh-idle measurement, not this
        // established path. Busy/mid-turn and long-idle sessions inject at
        // once; only a freshly-idle established one waits the guard.
        // Best-effort - a vanished session just falls through to the
        // inject's own race handling below.
        let settle = self.settle_live_before_inject(app, session_id).await;
        match self
            .inject_wake_into_pty(
                app,
                session_id,
                msg,
                (origin, comanaged),
                settle,
                (response, None),
            )
            .await
        {
            Ok(()) => {
                // #1635-2 D1: a live target cannot apply dispatch
                // flags — deliver as today and annotate the receipt.
                // A plain wake stays byte-identical (no annotation).
                let mut delivered = msg.clone();
                if flags_present {
                    delivered.dispatch_not_applied = Some("live-target".to_string());
                }
                Ok(Some(delivered))
            }
            Err(e) if err_is_pty_session_missing(&e) => {
                // Race: PTY died between `find_live_candidates`
                // probe and `PtyManager::write`. Load-bearing
                // safety net for the dropped per-iteration
                // re-read (grinch G.M2). Try the next candidate.
                // Flag the race for the post-loop AC5 promotion
                // (grinch HIGH-1) — if every Inject candidate
                // races to dead, the spawn-persistent fall-through
                // must still set spawn_with_resume.
                log::warn!(
                    "[mailbox] wake: candidate {} died after liveness probe ({}), trying next",
                    session_id,
                    e
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    async fn select_peer_wake_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
    ) -> Result<std::ops::ControlFlow<OutboxMessage, PeerWakeFallback>, String> {
        let (logical_command, flags_present) = dispatch;
        // Whether the spawn-fallback should allow provider auto-resume.
        // Default false: cold wake — no SessionManager record at this CWD.
        // Promoted to true in two paths below: (a) RespawnExited deferred-
        // destroy block, (b) phantoms-only fall-through (every CWD match
        // was a desync phantom — preserve any on-disk transcript via
        // auto-resume). See issue #223 round-1 resolution.
        //
        // MUST NOT be re-derived after `destroy_session_inner` runs: post-
        // destroy, the candidate would vanish from list_sessions and the
        // value would silently flip, regressing the deferred-non-coord
        // wake by losing `--continue`. Set the flag inside the candidate
        // loop only. See plan §4.5.a / round-1 G7 / round-3 R3.2.
        let mut spawn_with_resume = false;
        let mut pending_exited_destroy: Option<Uuid> = None;
        let mut pending_exited_telegram_bot_id: Option<String> = None;
        let mut pending_exited_communication: Option<
            crate::session::session::SessionCommunication,
        > = None;
        // HIGH-1 (Step-7 review): symmetric AC5 protection. Tracks whether
        // every iter'd Inject candidate hit the `err_is_pty_session_missing`
        // race arm — if so, the post-loop fall-through must still promote
        // spawn_with_resume so the on-disk transcript isn't abandoned. Same
        // outcome as grinch G.H2's phantoms-only path, just a different
        // upstream cause (race-killed siblings vs. desync phantoms).
        let mut lost_inject_to_race = false;

        // Issue #223: enumerate ALL viable CWD candidates (PTY-liveness filtered)
        // with their captured status, plus a had_any_match flag for the phantoms-
        // only AC5-preservation path (grinch G.H2). Iterate so a stale-Running
        // phantom no longer blocks delivery to the live Idle recipient at the
        // same CWD; defer Exited destroys until later Inject attempts fail
        // (grinch G.H1) — preserves AC5's "first Exited wins respawn slot".
        //
        // #293: Root Agent recipient uses the `is_root_agent` flag lookup,
        // not CWD-FQN match, since `agent_fqn_from_path(ac-root-agent)` does
        // not produce `ROOT_AGENT_SENDER`. Exited records are filtered out by
        // `find_root_session_candidate` so the deferred-destroy block never
        // fires on a user-launched session.
        let (candidates, had_any_match) =
            if crate::config::root_agent::is_root_agent_target(&msg.to) {
                match self.find_root_session_candidate(app).await {
                    Some(pair) => (vec![pair], true),
                    None => (Vec::new(), false),
                }
            } else {
                self.find_live_candidates(app, &msg.to).await
            };

        for &(session_id, ref status) in &candidates {
            log::debug!(
                "[mailbox] wake: candidate {} captured-status={:?}",
                session_id,
                status
            );

            match wake_action_for(status) {
                WakeAction::Inject => {
                    match self
                        .try_inject_peer_candidate(
                            app,
                            msg,
                            delivery,
                            response,
                            session_id,
                            (logical_command, flags_present),
                        )
                        .await?
                    {
                        Some(delivered) => return Ok(std::ops::ControlFlow::Break(delivered)),
                        None => lost_inject_to_race = true,
                    }
                }
                WakeAction::RespawnExited => {
                    // Defer the destroy: an Inject candidate later in the list
                    // may succeed and avoid destroy+spawn entirely (grinch
                    // G.H1). Only the FIRST Exited's id is remembered —
                    // preserves AC5's "first Exited wins the respawn slot"
                    // semantic.
                    if pending_exited_destroy.is_none() {
                        pending_exited_destroy = Some(session_id);
                        let (bot_id, communication) = {
                            let session_mgr =
                                app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                            let mgr = session_mgr.read().await;
                            mgr.get_session(session_id)
                                .await
                                .map(|s| (s.telegram_bot_id.clone(), s.communication.clone()))
                                .unwrap_or((None, None))
                        };
                        pending_exited_telegram_bot_id = bot_id;
                        // (#747) carry a restored raised hand across the
                        // destroy+respawn; the wake injection is not user input
                        // and must not clear it (#676).
                        pending_exited_communication = communication;
                        spawn_with_resume = true;
                        log::debug!(
                            "[mailbox] wake: deferring Exited destroy for {} (status={:?}) pending \
                            later Inject success",
                            session_id,
                            status
                        );
                    }
                    continue;
                }
            }
        }

        // No Inject returned Ok. Three cases enable auto-resume on the spawn-
        // persistent fall-through:
        //   1. A deferred Exited destroy is pending (spawn_with_resume true).
        //   2. Candidate list empty BUT records existed in the manager — i.e.,
        //      every CWD-match was a non-Exited phantom. Spawning cold would
        //      abandon any on-disk transcript at the matched CWD (grinch G.H2
        //      AC5 micro-regression).
        //   3. Every viable Inject candidate died mid-flight (all hit the
        //      `err_is_pty_session_missing` race arm). Symmetric to case 2 —
        //      same AC5 outcome since cold spawn would abandon the transcript
        //      (grinch HIGH-1, Step-7 review).
        if pending_exited_destroy.is_none()
            && (candidates.is_empty() && had_any_match || lost_inject_to_race)
        {
            spawn_with_resume = true;
            log::debug!(
                "[mailbox] wake: all CWD candidates for '{}' are phantoms or lost to race; \
                enabling auto-resume on spawn-persistent",
                msg.to
            );
        }

        Ok(std::ops::ControlFlow::Continue((
            spawn_with_resume,
            pending_exited_destroy,
            pending_exited_telegram_bot_id,
            pending_exited_communication,
        )))
    }

    async fn spawn_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        plan: ResolvedWakeSpawnPlan,
        spawn_with_resume: bool,
        flags_present: bool,
    ) -> Result<SpawnedPeerWake, String> {
        let ResolvedWakeSpawnPlan {
            resolved_command,
            cwd,
            session_name,
            spawn_shell,
            spawn_args,
            spawn_label,
            configured_spawn,
            resolved_agent_host_shell,
        } = plan;
        let spawn_source = resolved_command.source.clone();
        let spawn_raw = resolved_command.raw_command.clone();

        log::info!(
            "[mailbox] wake: spawning '{}' from {}: raw_command={:?}, shell={:?}, args={:?}",
            msg.to,
            spawn_source,
            spawn_raw,
            spawn_shell,
            spawn_args
        );

        if let Some(spawn) = configured_spawn.as_ref() {
            crate::config::agent_command::prepare_agent_spawn_command(spawn)?;
        }
        // #1635-2 D4: `configured_spawn` is moved into the spawn below; keep a
        // clone so the delivered-receipt annotation can read the trusted agent
        // id and the profile resolution that actually launched.
        let configured_spawn_for_annotation = configured_spawn.clone();
        let info = self
            .spawn_wake_session(
                app,
                msg,
                &resolved_command,
                cwd,
                session_name,
                spawn_with_resume,
                flags_present, // D5: dispatch-flag wakes suppress tooling writes
                spawn_shell.clone(),
                spawn_args.clone(),
                spawn_label,
                configured_spawn,
                resolved_agent_host_shell,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to spawn session for '{}': command from {} resolved raw_command={:?}, \
                    shell={:?}, args={:?}: {}",
                    msg.to, spawn_source, spawn_raw, spawn_shell, spawn_args, e
                )
            })?;

        let session_id =
            Uuid::parse_str(&info.id).map_err(|e| format!("Failed to parse session id: {}", e))?;

        Ok((
            session_id,
            configured_spawn_for_annotation,
            resolved_command,
        ))
    }

    async fn finish_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        restored: (
            Option<String>,
            Option<Uuid>,
            Option<crate::session::session::SessionCommunication>,
        ),
        spawned: (bool, SpawnedPeerWake),
    ) -> Result<OutboxMessage, String> {
        let (origin, comanaged) = delivery;
        let (
            pending_exited_telegram_bot_id,
            mut pending_exited_orphan_to_clear,
            mut pending_exited_communication,
        ) = restored;
        let (flags_present, (session_id, configured_spawn_for_annotation, resolved_command)) =
            spawned;
        if pending_exited_telegram_bot_id.is_some() {
            self.attach_persisted_telegram_for_wake(
                app,
                session_id,
                pending_exited_telegram_bot_id.as_deref(),
            )
            .await;
        }

        // (#747) Single-carrier rule. If the deferred destroy failed, the
        // orphan Exited record still holds the restored hand while the carry
        // below plants it on the new session. Clear the orphan's copy now
        // (the spawn succeeded, so the live session is the carrier) and tell
        // the frontend; otherwise the stale orphan badge survives attending
        // the live session, and deduplicate (name+cwd, first-kept) can
        // persist the orphan's Exited+hand row, resurrecting an already
        // attended hand on the next restart.
        if let Some(orphan_id) = pending_exited_orphan_to_clear.take() {
            let cleared = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.clear_communication_if_kind(orphan_id, SessionCommunicationKind::RaiseHand)
                    .await
            };
            if cleared {
                crate::session::selection::publish_session_communication(app, orphan_id, None);
            }
        }

        // (#747) Re-apply a raised hand carried from the destroyed dormant
        // record. spawn_with_resume is true on this path (set in the
        // RespawnExited arm), so the resumed conversation keeps its pending
        // user-attention marker until real user input.
        if let Some(communication) = pending_exited_communication.take() {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let restored = {
                let mgr = session_mgr.read().await;
                mgr.restore_communication(session_id, communication.clone())
                    .await
            };
            if restored {
                crate::session::selection::publish_session_communication(
                    app,
                    session_id,
                    Some(&communication),
                );
            }
        }

        let settle = self.wait_for_spawned_wake_idle(app, session_id).await?;

        // Inject message — interactive mode (session persists, user sees reply instructions)
        self.inject_wake_into_pty(
            app,
            session_id,
            msg,
            (origin, comanaged),
            settle,
            (response, None),
        )
        .await?;

        // #1635-2 D4: the delivered JSON reports the effective agent and
        // profile when dispatch flags were honored on this spawn. When the
        // agent is not configured (no trusted spawn), only the resolved agent
        // id is annotated; profile fields stay absent.
        let mut delivered = msg.clone();
        if flags_present {
            if let Some(spawn) = configured_spawn_for_annotation.as_ref() {
                delivered.effective_agent_id = Some(spawn.trusted_agent_id.clone());
                delivered.effective_profile =
                    Some(spawn.profile_resolution.effective_profile.clone());
                delivered.profile_fallback_applied = spawn.profile_resolution.fallback_applied;
            } else {
                delivered.effective_agent_id = resolved_command.agent_id.clone();
            }
        }
        Ok(delivered)
    }

    async fn deliver_peer_wake_fallback<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        delivery: (WakeDeliveryOrigin, bool),
        response: Option<&ResponseWakeWork>,
        dispatch: (bool, bool),
        fallback: PeerWakeFallback,
    ) -> Result<OutboxMessage, String> {
        let (logical_command, flags_present) = dispatch;
        let (
            spawn_with_resume,
            pending_exited_destroy,
            pending_exited_telegram_bot_id,
            pending_exited_communication,
        ) = fallback;
        // (#747) Set only when the deferred destroy FAILS: the orphan Exited
        // record then still holds the restored hand and must be cleared once
        // the replacement spawn succeeds (single-carrier rule, 5d).
        let mut pending_exited_orphan_to_clear: Option<Uuid> = None;
        // No viable Inject candidate succeeded, so the fallback would spawn a
        // persistent session. Root Agent remains user-launched only.
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );
        if crate::config::root_agent::is_root_agent_target(&msg.to) {
            let restoring = app
                .state::<Arc<crate::RestoreInProgress>>()
                .0
                .load(std::sync::atomic::Ordering::SeqCst);
            return if restoring {
                Err(format!(
                    "Root Agent session not yet restored for '{}'; daemon restart in progress — \
                    will retry.",
                    msg.to
                ))
            } else {
                Err(format!(
                    "No live Root Agent session for '{}'. The Root Agent must be running locally \
                    to receive messages — ask the user to launch it.",
                    msg.to
                ))
            };
        }

        // A logical action must be mapped against the exact carried spawn shell
        // before an Exited record is destroyed or provider directories are made.
        let mut spawn_plan = if logical_command {
            let plan = self.resolve_wake_spawn_plan(app, msg).await?;
            if let Some(wire_value) = msg.command.as_deref() {
                resolve_remote_pty_command(&plan.spawn_shell, wire_value)?;
            }
            Some(plan)
        } else {
            None
        };

        if let Some(exited_id) = pending_exited_destroy {
            log::debug!(
                "[mailbox] wake: executing deferred destroy for Exited candidate {}",
                exited_id
            );
            if let Err(e) = self.destroy_exited_wake_session(app, exited_id).await {
                // Best-effort. Orphan SessionManager record lingers until AC3's
                // runtime-dedup path (`#223-fu1`) drains it. spawn-persistent
                // below still fires with the correct spawn_with_resume flag.
                // (grinch G.M5 option (c).)
                log::error!(
                    "[mailbox] wake: failed to destroy exited session {} (orphan will linger until \
                    AC3): {}",
                    exited_id,
                    e
                );
                // (#747) The orphan Exited record still holds its restored
                // raised hand. Do NOT clear it here: if the spawn below also
                // fails, the orphan must stay the sole carrier (the hand is
                // not lost with the failed wake). Remember it so 5d moves the
                // hand to the new session once the spawn succeeds.
                pending_exited_orphan_to_clear = Some(exited_id);
            }
        }

        // Normal messages retain their original ordering: resolve the plan only
        // after any deferred Exited destroy. Logical actions already carry the
        // read-only plan proven safe above.
        if spawn_plan.is_none() {
            spawn_plan = Some(self.resolve_wake_spawn_plan(app, msg).await?);
        }
        let plan = spawn_plan
            .ok_or_else(|| "Internal error: wake spawn plan was not resolved".to_string())?;
        let spawned = self
            .spawn_peer_wake_fallback(app, msg, plan, spawn_with_resume, flags_present)
            .await?;
        self.finish_peer_wake_fallback(
            app,
            msg,
            delivery,
            response,
            (
                pending_exited_telegram_bot_id,
                pending_exited_orphan_to_clear,
                pending_exited_communication,
            ),
            (flags_present, spawned),
        )
        .await
    }

    /// Deliver a validated AgentsCommander-generated notice through the existing wake,
    /// resume, background-spawn, settle, and canonical injection plumbing. No serialized
    /// message field can select this path.
    pub(crate) async fn deliver_internal_system_notice<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        self.deliver_wake_engine(
            app,
            WakeDelivery::InternalSystem {
                target,
                notice,
                cancellation,
                guard,
            },
        )
        .await
        .map(|_| ())
    }

    async fn deliver_internal_system_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: InternalSystemTarget,
        notice: InternalSystemNotice,
        cancellation: CancellationToken,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err("Context alert delivery was canceled".to_string());
        }
        if let Some(purge) = app.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>() {
            if purge.blocks_agent(target.fqn()) {
                return Err(format!(
                    "purge-room in progress for '{}'; context alert deferred",
                    target.fqn()
                ));
            }
        }
        Self::run_internal_guard(Arc::clone(&guard)).await?;

        let envelope = internal_system_envelope(&target);
        let mut exited: Option<SessionInfo> = None;
        for (candidate, has_pty) in self.find_internal_system_candidates(app, &target).await? {
            match candidate.status {
                SessionStatus::Exited(_) => {
                    if exited.is_none() {
                        exited = Some(candidate);
                    }
                }
                _ if has_pty => {
                    let session_id = Uuid::parse_str(&candidate.id).map_err(|e| {
                        format!("Invalid orchestrator session id '{}': {}", candidate.id, e)
                    })?;
                    let blocked = tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            return Err(
                                "Context alert delivery was canceled before live injection"
                                    .to_string(),
                            );
                        }
                        reason = crate::pty::inject::internal_notice_block_reason(
                            app,
                            session_id,
                        ) => reason,
                    };
                    if cancellation.is_cancelled() {
                        return Err(
                            "Context alert delivery was canceled before live injection".to_string()
                        );
                    }
                    if let Some(reason) = blocked {
                        return Err(reason);
                    }
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            return Err(
                                "Context alert delivery was canceled during live settle"
                                    .to_string(),
                            );
                        }
                        _ = self.settle_internal_live_before_inject(app, session_id) => {}
                    }
                    if cancellation.is_cancelled() {
                        return Err(
                            "Context alert delivery was canceled before live injection".to_string()
                        );
                    }
                    match self
                        .inject_internal_system_notice(
                            app,
                            session_id,
                            &target,
                            &notice,
                            Arc::clone(&guard),
                        )
                        .await
                    {
                        Ok(()) => return Ok(()),
                        Err(error) if err_is_pty_session_missing(&error) => {
                            log::warn!(
                                "[context-alert] coordinator session {} vanished before injection: \
                                {}",
                                session_id,
                                error
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
                _ => {}
            }
        }

        let mut spawn_with_resume = false;
        let mut carried_bot: Option<String> = None;
        let mut carried_communication = None;
        if let Some(exited_candidate) = exited {
            if cancellation.is_cancelled() {
                return Err(
                    "Context alert delivery was canceled before exited-session actuation"
                        .to_string(),
                );
            }
            let exited_id = Uuid::parse_str(&exited_candidate.id)
                .map_err(|e| format!("Invalid exited orchestrator session id: {}", e))?;
            self.recheck_internal_exited_candidate(app, exited_id, &target)
                .await?;
            Self::run_internal_guard(Arc::clone(&guard)).await?;
            self.recheck_internal_exited_record(
                app,
                exited_id,
                &target,
                &exited_candidate.working_directory,
            )
            .await?;
            carried_bot = exited_candidate.telegram_bot_id.clone();
            carried_communication = exited_candidate.communication.clone();
            self.destroy_exited_wake_session(app, exited_id)
                .await
                .map_err(|error| {
                    format!(
                        "Failed to destroy stale orchestrator session {}: {}",
                        exited_id, error
                    )
                })?;
            spawn_with_resume = true;
        }

        // Final pre-spawn authorization followed by a fresh exact-path enumeration. A
        // supported recipient that appeared wins; this attempt returns stale rather than
        // creating a duplicate.
        Self::run_internal_guard(Arc::clone(&guard)).await?;
        if !self
            .find_internal_system_candidates(app, &target)
            .await?
            .is_empty()
        {
            return Err(format!(
                "Orchestrator recipient '{}' changed immediately before background spawn",
                target.fqn()
            ));
        }

        let resolved_command = self
            .resolve_internal_agent_command(app, &target)
            .await?
            .ok_or_else(|| {
                format!(
                    "No supported coding-agent command is configured for '{}'",
                    target.fqn()
                )
            })?;
        let agent_id = resolved_command.agent_id.as_deref().ok_or_else(|| {
            format!(
                "Resolved command for '{}' has no trusted coding-agent id",
                target.fqn()
            )
        })?;
        let cwd = target.replica_dir().to_string_lossy().to_string();
        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        // #1271 - copy the configured default host shell from the SAME snapshot
        // that builds the spawn below, before the snapshot moves into the
        // resolution task (Phase 1 items 1-2: never read one half of the pair
        // after configuration has changed).
        let resolved_agent_host_shell = Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings_snapshot.default_shell.clone(),
            args: settings_snapshot.default_shell_args.clone(),
        });
        let spawn_agent_id = agent_id.to_string();
        let spawn_cwd = cwd.clone();
        let resolved_spawn = tokio::task::spawn_blocking(move || {
            crate::commands::session::build_configured_agent_spawn_for_cwd(
                &settings_snapshot,
                &spawn_agent_id,
                &spawn_cwd,
                None,
            )
        })
        .await
        .map_err(|error| format!("Internal profile resolution task failed: {}", error))??
        .ok_or_else(|| {
            format!(
                "Configured coding-agent '{}' could not produce a trusted spawn for '{}'",
                agent_id,
                target.fqn()
            )
        })?;
        if resolved_spawn.trusted_agent_id.trim().is_empty()
            || resolved_spawn.trusted_agent_id != agent_id
        {
            return Err(format!(
                "Resolved spawn for '{}' lost its trusted coding-agent identity",
                target.fqn()
            ));
        }
        if !crate::pty::inject::needs_explicit_enter(&resolved_spawn.shell) {
            return Err(format!(
                "Resolved spawn shell '{}' for '{}' is not a supported coding-agent CLI",
                resolved_spawn.shell,
                target.fqn()
            ));
        }

        // Repeat the guard and exact-path absence check immediately before actuation. Command
        // and profile resolution above can perform filesystem work and must not widen this race.
        Self::run_internal_guard(Arc::clone(&guard)).await?;
        if !self
            .find_internal_system_candidates(app, &target)
            .await?
            .is_empty()
        {
            return Err(format!(
                "Orchestrator recipient '{}' changed immediately before background spawn",
                target.fqn()
            ));
        }

        if cancellation.is_cancelled() {
            return Err("Context alert delivery was canceled before background spawn".to_string());
        }
        // #2411 An idle auto-close destroys the record, so a no-session wake of an
        // auto-closed orchestrator resumes its conversation. Same key as create and
        // auto-close. The std mutex is never held across an await.
        let marker_fqn = crate::config::teams::agent_fqn_from_path(&cwd);
        let auto_closed_at = app
            .try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
            .and_then(|clocks| {
                clocks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .auto_closed_at(&marker_fqn)
            });
        let spawn_with_resume = internal_wake_resumes(spawn_with_resume, auto_closed_at.is_some());
        log::info!(
            "[internal-wake] '{}' no-live-session spawn resume={} auto_closed_at={:?}",
            target.fqn(),
            spawn_with_resume,
            auto_closed_at
        );
        let (_, local) = crate::config::teams::split_project_prefix(target.fqn());
        let spawn = self.spawn_wake_session(
            app,
            &envelope,
            &resolved_command,
            cwd,
            local.to_string(),
            spawn_with_resume,
            // Internal-system envelopes never carry dispatch flags (D5).
            false,
            resolved_spawn.shell.clone(),
            resolved_spawn.shell_args.clone(),
            Some(resolved_spawn.trusted_agent_label.clone()),
            Some(resolved_spawn),
            resolved_agent_host_shell,
        );
        tokio::pin!(spawn);
        let spawn_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(
                    "Context alert delivery was canceled during background spawn".to_string(),
                );
            }
            result = &mut spawn => result,
        };
        let info = match spawn_result {
            Ok(info) => info,
            Err(error) => {
                // #2413 A failed create restores its cleared close markers itself
                // (commands::session::CoordinatorCreateTicket), for every caller.
                return Err(format!(
                    "Failed to spawn supported orchestrator session for '{}': {}",
                    target.fqn(),
                    error
                ));
            }
        };
        let session_id = Uuid::parse_str(&info.id)
            .map_err(|e| format!("Invalid spawned orchestrator session id: {}", e))?;

        if carried_bot.is_some() {
            self.attach_persisted_telegram_for_wake(app, session_id, carried_bot.as_deref())
                .await;
        }
        if let Some(communication) = carried_communication {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            let restored = manager
                .restore_communication(session_id, communication.clone())
                .await;
            if restored {
                crate::session::selection::publish_session_communication(
                    app,
                    session_id,
                    Some(&communication),
                );
            }
        }

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(
                    "Context alert delivery was canceled during spawned-session settle".to_string(),
                );
            }
            result = self.wait_for_spawned_wake_idle(app, session_id) => {
                result?;
            }
        }
        if cancellation.is_cancelled() {
            return Err(
                "Context alert delivery was canceled before spawned-session injection".to_string(),
            );
        }
        self.inject_internal_system_notice(app, session_id, &target, &notice, guard)
            .await
    }

    async fn settle_internal_live_before_inject<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let gate = hooks.internal_live_settle_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                // notify_one, not notify_waiters: this stores a permit when the canceller
                // has not been polled yet, which on a current_thread runtime is the norm.
                hooks.internal_live_settle_entered.notify_one();
                let _ = gate.await;
                return;
            }
        }
        // #2586 - the internal-notice injector is a non-wake path that reports
        // `Unknown`, so this readiness is not carried.
        let _ = self.settle_live_before_inject(app, session_id).await;
    }

    async fn run_internal_guard(guard: InternalNoticeGuard) -> Result<(), String> {
        tokio::task::spawn_blocking(move || guard())
            .await
            .map_err(|error| format!("Internal notice guard task failed: {}", error))?
    }

    async fn resolve_internal_agent_command<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: &InternalSystemTarget,
    ) -> Result<Option<ResolvedWakeAgentCommand>, String> {
        let agents = {
            let settings = app.state::<SettingsState>();
            let settings = settings.read().await;
            // #2716 (B3, option C): the same refusal as `resolve_agent_command`.
            if let Some(path) = settings.agents_layer.unreadable_path() {
                return Err(crate::config::settings::agents_unreadable_session_error(
                    path,
                ));
            }
            settings.agents.clone()
        };
        let replica_dir = target.replica_dir().to_path_buf();
        tokio::task::spawn_blocking(move || {
            let current = crate::config::coding_agent_profiles::read_replica_current_coding_agent(
                &replica_dir,
            );
            let config_dir = replica_dir.join(crate::config::agent_local_dir_name());
            let config_path = config_dir.join("config.json");
            let last = crate::config::agent_config::read_agent_local_config(&config_dir)
                .and_then(|config| config.tooling.last_coding_agent);
            resolve_wake_agent_command_from_sources(
                &agents,
                "auto",
                current.as_deref(),
                last.as_deref(),
                Some(&config_path),
                None,
            )
        })
        .await
        .map_err(|error| format!("Internal command lookup task failed: {}", error))?
    }

    async fn find_internal_system_candidates<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        target: &InternalSystemTarget,
    ) -> Result<Vec<(SessionInfo, bool)>, String> {
        let sessions = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.list_sessions().await
        };
        let target_fqn = target.fqn().to_string();
        let replica_dir = target.replica_dir().to_path_buf();
        let structural: Vec<SessionInfo> = sessions
            .into_iter()
            .filter(|session| {
                !session.is_root_agent
                    && session.agent_id.is_some()
                    && crate::pty::inject::needs_explicit_enter(&session.shell)
                    && internal_target_fqn_prefilter_matches(
                        &session.working_directory,
                        &target_fqn,
                    )
            })
            .collect();
        let mut owned = tokio::task::spawn_blocking(move || {
            structural
                .into_iter()
                .filter(|session| {
                    canonical_cwd_owned_by_replica(&session.working_directory, &replica_dir)
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| format!("Orchestrator candidate path task failed: {}", error))?;
        owned.sort_by_key(|session| {
            let temporary = session
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match session.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (temporary, status)
        });

        let mut result = Vec::with_capacity(owned.len());
        for session in owned {
            let id = Uuid::parse_str(&session.id)
                .map_err(|e| format!("Invalid orchestrator session id '{}': {}", session.id, e))?;
            let has_pty = self.has_pty_session_for_wake(app, id).await;
            if matches!(session.status, SessionStatus::Exited(_)) || has_pty {
                result.push((session, has_pty));
            }
        }
        Ok(result)
    }

    async fn recheck_internal_exited_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
    ) -> Result<(), String> {
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.get_session(session_id).await
        }
        .ok_or_else(|| format!("Exited orchestrator session {} disappeared", session_id))?;
        if !matches!(session.status, SessionStatus::Exited(_))
            || session.is_root_agent
            || session.agent_id.is_none()
            || !crate::pty::inject::needs_explicit_enter(&session.shell)
        {
            return Err(format!(
                "Orchestrator session {} changed before exited-session destruction",
                session_id
            ));
        }
        let cwd = session.working_directory;
        let replica = target.replica_dir().to_path_buf();
        let owned =
            tokio::task::spawn_blocking(move || canonical_cwd_owned_by_replica(&cwd, &replica))
                .await
                .map_err(|error| format!("Exited orchestrator path task failed: {}", error))??;
        if !owned {
            return Err(format!(
                "Orchestrator session {} no longer belongs to target replica",
                session_id
            ));
        }
        Ok(())
    }

    async fn recheck_internal_exited_record<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
        expected_cwd: &str,
    ) -> Result<(), String> {
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let manager = session_mgr.read().await.clone();
            manager.get_session(session_id).await
        }
        .ok_or_else(|| format!("Exited orchestrator session {} disappeared", session_id))?;
        if !matches!(session.status, SessionStatus::Exited(_))
            || session.is_root_agent
            || session.agent_id.is_none()
            || !crate::pty::inject::needs_explicit_enter(&session.shell)
            || session.working_directory != expected_cwd
            || !internal_target_fqn_prefilter_matches(&session.working_directory, target.fqn())
        {
            return Err(format!(
                "Orchestrator session {} restarted or changed immediately before destruction",
                session_id
            ));
        }
        let cwd = session.working_directory;
        let replica = target.replica_dir().to_path_buf();
        let owned =
            tokio::task::spawn_blocking(move || canonical_cwd_owned_by_replica(&cwd, &replica))
                .await
                .map_err(|error| {
                    format!(
                        "Final exited orchestrator path task failed for {}: {}",
                        session_id, error
                    )
                })??;
        if !owned {
            return Err(format!(
                "Orchestrator session {} escaped the canonical target before destruction",
                session_id
            ));
        }
        Ok(())
    }

    async fn inject_internal_system_notice<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        target: &InternalSystemTarget,
        notice: &InternalSystemNotice,
        guard: InternalNoticeGuard,
    ) -> Result<(), String> {
        let payload = format_wake_content(WakeContent::InternalSystem(notice));

        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let session = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let manager = session_mgr.read().await.clone();
                manager.get_session(session_id).await
            }
            .ok_or_else(|| format!("Session {} missing before test injection", session_id))?;
            if session.is_root_agent
                || matches!(session.status, SessionStatus::Exited(_))
                || session.agent_id.is_none()
                || !crate::pty::inject::needs_explicit_enter(&session.shell)
                || !canonical_cwd_owned_by_replica(
                    &session.working_directory,
                    target.replica_dir(),
                )?
            {
                return Err(format!(
                    "Session {} failed supported-agent final validation",
                    session_id
                ));
            }
            guard()?;
            if let Some(reason) =
                crate::pty::inject::internal_notice_block_reason(app, session_id).await
            {
                return Err(reason);
            }
            hooks.inject_calls.lock().unwrap().push(session_id);
            hooks
                .internal_payloads
                .lock()
                .unwrap()
                .push(payload.clone());
            hooks
                .events
                .lock()
                .unwrap()
                .push(MailboxTestEvent::Inject(session_id));
            hooks
                .inject_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))?;
            hooks
                .internal_bookkeeping
                .lock()
                .unwrap()
                .push(InternalSystemBookkeeping {
                    session_id,
                    post_boundary: true,
                    silence_touch: true,
                    set_last_prompt: false,
                    peer_event: false,
                    response_watcher: false,
                    consumption_verdict: false,
                    mailbox_archive: false,
                });
            return Ok(());
        } else {
            crate::pty::inject::inject_internal_notice_into_supported_agent_session(
                app,
                session_id,
                &payload,
                |session: &Session| {
                    if !canonical_cwd_owned_by_replica(
                        &session.working_directory,
                        target.replica_dir(),
                    )? {
                        return Err(format!(
                            "Orchestrator session {} escaped canonical target replica",
                            session_id
                        ));
                    }
                    guard()
                },
            )
            .await?;
        }

        #[cfg(not(test))]
        crate::pty::inject::inject_internal_notice_into_supported_agent_session(
            app,
            session_id,
            &payload,
            |session: &Session| {
                if !canonical_cwd_owned_by_replica(
                    &session.working_directory,
                    target.replica_dir(),
                )? {
                    return Err(format!(
                        "Orchestrator session {} escaped canonical target replica",
                        session_id
                    ));
                }
                guard()
            },
        )
        .await?;

        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
            idle.touch_silence(session_id);
        }
        let (project, _) = crate::config::teams::split_project_prefix(target.fqn());
        match notice {
            // Byte-identical to the line this feature shipped, including the
            // field order and the `{:?}` thresholds: operators grep it.
            InternalSystemNotice::ContextAlert {
                member,
                workgroup,
                observed,
                thresholds,
            } => {
                log::info!(
                    "[context-alert] delivered coordinatorSession={} project={} workgroup={} member={} observed={} thresholds={:?}",
                    session_id,
                    project.unwrap_or(""),
                    workgroup,
                    member,
                    observed,
                    thresholds
                );
            }
            // A NEW prefix, never the context-alert one: two different notices
            // under one prefix would make that grep lie.
            InternalSystemNotice::RemoteActivity {
                kind,
                repo_nwo,
                branch,
                sha7,
                ..
            } => {
                log::info!(
                    "[remote-activity] delivered coordinatorSession={} project={} kind={:?} \
                    repo={} branch={} sha={}",
                    session_id,
                    project.unwrap_or(""),
                    kind,
                    repo_nwo,
                    branch,
                    sha7
                );
            }
        }
        Ok(())
    }

    async fn has_pty_session_for_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        id: Uuid,
    ) -> bool {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let scripted = {
                let presence = hooks.pty_presence.lock().unwrap();
                presence.get(&id).copied()
            };
            if let Some(has_pty) = scripted {
                return has_pty;
            }
        }

        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        let pty = pty_mgr.lock().unwrap();
        pty.has_session(id)
    }

    async fn inject_wake_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        attribution: (WakeDeliveryOrigin, bool),
        settle: SettleReadiness,
        context: (
            Option<&ResponseWakeWork>,
            Option<&crate::session::selection::OwnedReplacementContinuation>,
        ),
    ) -> Result<(), String> {
        let (response, owned_replacement) = context;
        let (origin, comanaged) = attribution;
        #[cfg(test)]
        if let Some(hooks) = self
            .test_hooks
            .as_ref()
            .filter(|_| owned_replacement.is_none())
        {
            {
                let mut calls = hooks.inject_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Inject(session_id));
            }
            if hooks.panic_on_inject.lock().unwrap().contains(&session_id) {
                panic!("test-scripted wake delivery panic");
            }
            let use_real_inject = hooks
                .real_inject_sessions
                .lock()
                .unwrap()
                .contains(&session_id);
            if !use_real_inject {
                let inject_result = {
                    let mut results = hooks.inject_results.lock().unwrap();
                    results.pop_front()
                }
                .unwrap_or(Ok(()));
                // (#1001 PR1 / G6) On a successful inject, if a consumption
                // verdict is scripted, run the SAME verdict_to_result the
                // production path uses so AC3 covers the real conversion. No
                // scripted verdict => Ok (existing hooked tests unchanged).
                return match inject_result {
                    Ok(()) => match hooks.consumption_results.lock().unwrap().pop_front() {
                        Some(verdict) => verdict_to_result(verdict),
                        None => Ok(()),
                    },
                    Err(e) => Err(e),
                };
            }
        }

        let result = loop {
            let result = self
                .inject_into_pty_with_response(
                    app,
                    session_id,
                    msg,
                    true,
                    origin,
                    comanaged,
                    settle,
                    (response, owned_replacement),
                )
                .await;
            if result
                .as_ref()
                .is_err_and(|error| error == crate::pty::inject::OWNED_WORK_SCOPE_WAIT)
            {
                // The injector has dropped its writer and route guards. An
                // ordinary unowned wake retains its source and attempts here.
                tokio::task::yield_now().await;
                continue;
            }
            break result;
        };
        if result.is_ok() {
            if let Some(attempt) = response.and_then(|work| match work.attempt.lock() {
                Ok(attempt) => attempt.clone(),
                Err(_) => {
                    response_diag_local!(
                        "mark_request_submitted",
                        "attempt_lock_failure",
                        work.diagnostic_message_id
                    );
                    None
                }
            }) {
                if let Ok(candidate) = attempt.candidate.lock() {
                    if let Some(candidate) = candidate.as_ref() {
                        response_diag_local!(
                            "mark_request_submitted",
                            "true_invocation",
                            Uuid::parse_str(&msg.id).ok()
                        );
                        attempt
                            .state
                            .mark_request_submitted(&candidate.context, true);
                    } else {
                        response_diag_local!(
                            "mark_request_submitted",
                            "candidate_absent",
                            None::<Uuid>
                        );
                    }
                } else {
                    response_diag_local!(
                        "mark_request_submitted",
                        "candidate_lock_failure",
                        None::<Uuid>
                    );
                }
            } else {
                response_diag_local!(
                    "mark_request_submitted",
                    "work_or_attempt_absent",
                    None::<Uuid>
                );
            }
        }
        // #552 auto-close: a successful inter-agent wake is activity for the
        // recipient team's silence clock (NOT the badge; inter-agent is not a
        // user message). This single site covers both wake paths (deliver_wake
        // and the spawned-wake path), which both funnel through here. The
        // recipient's own response output would also reset silence via the read
        // loop; recording the delivery makes inter-agent traffic count at once.
        if result.is_ok() {
            if let Some(idle) =
                app.try_state::<std::sync::Arc<crate::pty::idle_detector::IdleDetector>>()
            {
                idle.touch_silence(session_id);
            }
        }
        // (#1001 PR1 / G6) Route the successful-inject return through the
        // shared verdict_to_result so this production site runs the exact
        // conversion the AC3 hooked test exercises. PR1 wires no oracle yet,
        // so the verdict is NotApplicable => Ok(()): today's write-receipt
        // semantics, unchanged. A (PR3) replaces NotApplicable with the
        // observed verdict from the oracle driver.
        match result {
            Ok(()) => verdict_to_result(ConsumptionVerdict::NotApplicable),
            Err(e) => Err(e),
        }
    }

    async fn destroy_exited_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            {
                let mut calls = hooks.destroy_calls.lock().unwrap();
                calls.push(session_id);
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Destroy(session_id));
            }
            if let Some(result) = hooks.destroy_results.lock().unwrap().pop_front() {
                result?;
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr
                .destroy_session(session_id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
        }

        crate::commands::session::background_destroy_session_inner(app, session_id).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_wake_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
        resolved_command: &ResolvedWakeAgentCommand,
        cwd: String,
        session_name: String,
        spawn_with_resume: bool,
        skip_tooling_save: bool,
        spawn_shell: String,
        spawn_args: Vec<String>,
        spawn_label: Option<String>,
        resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
        resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
    ) -> Result<SessionInfo, String> {
        #[cfg(not(test))]
        let _ = msg;
        let skip_auto_resume = wake_spawn_skip_auto_resume(spawn_with_resume);
        let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let call = MailboxSpawnCall {
                to: msg.to.clone(),
                session_name: session_name.clone(),
                cwd: cwd.clone(),
                shell: spawn_shell.clone(),
                shell_args: spawn_args.clone(),
                skip_auto_resume,
                skip_tooling_save,
            };
            {
                let mut calls = hooks.spawn_calls.lock().unwrap();
                calls.push(call.clone());
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Spawn(call));
            }

            let spawn_gate = hooks.internal_spawn_gate.lock().unwrap().take();
            if let Some(spawn_gate) = spawn_gate {
                hooks.internal_spawn_started.notify_one();
                let _ = spawn_gate.await;
            }

            let spawn_is_coordinator = *hooks.spawn_is_coordinator.lock().unwrap();
            // #2411 Mirror production create: the coordinator clear runs before the
            // create can fail, on both the failure and the success path.
            // #2413 The ticket ends like production: `failed()` on the create error.
            let ticket = if spawn_is_coordinator {
                crate::commands::session::note_coordinator_create_on_app(app, &cwd)
            } else {
                None
            };
            if let Some(msg) = hooks.internal_spawn_error.lock().unwrap().take() {
                if let Some(t) = ticket {
                    t.failed();
                }
                return Err(msg);
            }
            if let Some(t) = ticket {
                t.succeeded();
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let session = mgr
                .create_session(
                    spawn_shell.clone(),
                    spawn_args.clone(),
                    cwd,
                    resolved_spawn
                        .as_ref()
                        .map(|spawn| spawn.trusted_agent_id.clone())
                        .or_else(|| resolved_command.agent_id.clone()),
                    spawn_label.clone(),
                    Vec::<SessionRepo>::new(),
                    spawn_is_coordinator,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .map_err(|e| e.to_string())?;
            mgr.rename_session(session.id, session_name)
                .await
                .map_err(|e| e.to_string())?;
            mgr.mark_idle(session.id).await;
            let inserted = mgr
                .get_session(session.id)
                .await
                .ok_or_else(|| format!("Session {} not found after test spawn", session.id))?;
            return Ok(SessionInfo::from(&inserted));
        }

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            spawn_shell,
            spawn_args,
            cwd,
            Some(session_name),                // readable name, no [temp] prefix
            resolved_command.agent_id.clone(), // links to agent config
            spawn_label,                       // human-readable label
            // #1635-2 D5: dispatch-flag wakes suppress lastCodingAgent persistence
            skip_tooling_save,
            Vec::new(),       // git_repos
            skip_auto_resume, // see deliver_wake top
            resolved_spawn,
            // #1271 - the configured host shell paired with the resolved agent,
            // built from the same settings snapshot that produced the spawn.
            resolved_agent_host_shell,
            // #973 - headless caller: no terminal to measure, keep 120x30.
            None,
            crate::commands::session::CreateSelectionIntent::Background,
        )
        .await
    }

    async fn attach_persisted_telegram_for_wake<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        bot_id: Option<&str>,
    ) {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            let bot_id = bot_id.map(str::to_string);
            {
                let mut calls = hooks.attach_calls.lock().unwrap();
                calls.push((session_id, bot_id.clone()));
            }
            {
                let mut events = hooks.events.lock().unwrap();
                events.push(MailboxTestEvent::Attach { session_id, bot_id });
            }
            return;
        }

        #[cfg(not(test))]
        {
            crate::commands::session::attach_persisted_telegram_if_configured(
                app, session_id, bot_id,
            )
            .await;
        }
        #[cfg(test)]
        {
            let _ = (app, session_id, bot_id);
        }
    }

    async fn wait_for_spawned_wake_idle<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> Result<SettleReadiness, String> {
        #[cfg(test)]
        if self.test_hooks.is_some() {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.mark_idle(session_id).await;
            // #2586 - the hooked path never runs the settle loop, so it has no
            // readiness to report; `Unknown` keeps it from fabricating `Ready`.
            return Ok(SettleReadiness::Unknown);
        }

        // #611: require SUSTAINED idle before injecting on the COLD-SPAWN path. A
        // freshly spawned agent (notably Claude) can hit a quiet window >
        // idle_threshold mid-startup and be marked idle before its TUI input/paste
        // state is stable; injecting then lands the body but the submit \r can be
        // dropped (written yet never sent). The settle lets late startup renders
        // finish first. Cold-spawn seeds fresh (None) and treats busy as "still
        // starting, keep waiting" (busy_means_inject = false). #1001 PR2 adds the
        // sibling live-inject gate `settle_live_before_inject`; both share the loop
        // below via `settle_until_ready`.
        self.settle_until_ready(
            app,
            session_id,
            std::time::Duration::from_secs(90),
            std::time::Duration::from_millis(2000),
            std::time::Duration::from_millis(500),
            None,
        )
        .await
    }

    /// (#1001 PR2 / B) Shared settle loop for both wake-inject paths. Waits until
    /// the session is safe to inject: sustained idle for `settle`, or - when
    /// `busy_means_inject` (live path) - a busy/mid-turn session injects at once
    /// (bias-to-deliver). `initial_idle_since` seeds the settle: the live path
    /// credits real idle age so a ready session is not delayed; cold-spawn passes
    /// None. Never drops a delivery: on `max_wait` it injects anyway. Errs only if
    /// the session was destroyed mid-settle, or (#1388/E8) has already exited. It
    /// gates on `wake_settle_ready` (idle AND rendered), not on idle alone, so a
    /// child that paints nothing no longer settles faster than a healthy one.
    /// It reads and decides BEFORE sleeping,
    /// so a seeded already-ready session injects with zero added latency; the
    /// cold-spawn sustained-idle requirement is unchanged.
    async fn settle_until_ready<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        max_wait: std::time::Duration,
        settle: std::time::Duration,
        poll: std::time::Duration,
        initial_idle_since: Option<std::time::Instant>,
    ) -> Result<SettleReadiness, String> {
        let start = std::time::Instant::now();
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mut idle_since = initial_idle_since;

        loop {
            // #1388: first condition, read in its own scope so the std Mutex guard is
            // dropped before the await below - the guard is not Send, so this is enforced
            // by the compiler rather than by convention. `try_state` (not `state`) mirrors
            // the IdleDetector read in `settle_live_before_inject` (:7719): a missing
            // manager is "no claim", not "not rendered", and must not gate.
            let rendered = match app.try_state::<Arc<Mutex<PtyManager>>>() {
                Some(pty_mgr) => match pty_mgr.lock() {
                    Ok(pty) => pty.has_rendered_visible_content(session_id),
                    Err(_) => true,
                },
                None => true,
            };

            // Read the flag under a short-lived lock; never hold it across the
            // pure decision or the sleep below.
            let waiting = {
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    // #1388/E8: a child that already exited can never satisfy the render
                    // condition, so without this it would hold the single global delivery
                    // lane for the full 90s and delay shutdown with it. Treated like the
                    // vanished case below: stop settling and let the caller's existing
                    // error path run.
                    Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                        return Err(format!(
                            "Session {} exited before message injection",
                            session_id
                        ));
                    }
                    Some(s) => s.waiting_for_input,
                    None => {
                        return Err(format!(
                            "Session {} was destroyed before message injection",
                            session_id
                        ));
                    }
                }
            };
            let menu_blocked = app
                .try_state::<Arc<crate::pty::menu_guard::MenuGuard>>()
                .is_some_and(|g| g.is_blocked(session_id));
            let ready = !menu_blocked && wake_settle_ready(waiting, rendered);

            let was_settling = idle_since.is_some();
            let (next_idle_since, action) = settle_tick(
                ready,
                idle_since,
                std::time::Instant::now(),
                settle,
                start.elapsed(),
                max_wait,
            );
            idle_since = next_idle_since;

            match action {
                SettleAction::InjectNow => {
                    if menu_blocked {
                        return Err(format!(
                            "{}: session {} is blocked by interactive menu",
                            crate::pty::menu_guard::ERR_MENU_GUARD_DEFERRED,
                            session_id
                        ));
                    }
                    // #2586 D3 - readiness exists only on this `Ok` branch.
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[mailbox] wake: timeout waiting for session {} to reach sustained \
                            idle; injecting anyway (waiting_for_input={}, rendered={})",
                            session_id,
                            waiting,
                            rendered
                        );
                        return Ok(SettleReadiness::TimedOut);
                    }
                    return Ok(SettleReadiness::Ready);
                }
                SettleAction::Wait => {
                    if was_settling && !ready {
                        log::info!(
                            "[mailbox] wake: session {} left the settle window during settle \
                            (waiting_for_input={}, rendered={}); re-waiting",
                            session_id,
                            waiting,
                            rendered
                        );
                    }
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    /// (#1001 PR2 / B, grinch P1) Settle a LIVE session to paste-ready before a
    /// wake inject. The live-inject arm used to go straight to inject with no gate.
    /// The MEASURED live-path drop is the still-starting case (P2), closed by the
    /// alive_age Starting route below; the established fresh-idle guard here is
    /// DEFENSIVE retention (bias-to-deliver, cannot drop), NOT a measured-drop gate
    /// (the ~75% was PR1's cold-spawn measurement, not this path). Every tick reads ONE
    /// atomic `purge_readiness` snapshot and decides from the real-time
    /// `activity_age` (plus the per-session `idle_threshold` and the resize-freeze
    /// `last_resize_age`) via `live_settle_action` - NOT from the lagged
    /// `SessionManager.waiting_for_input`, whose ~500ms idle-flip lag would let a
    /// busy fast-path fire inside the fresh-idle window (grinch P1). A busy
    /// (mid-turn) or long-idle session injects at once; only a freshly-idle one
    /// waits out `FRESH_IDLE_GUARD`. Best-effort: any missing state falls through
    /// to the inject, which surfaces the real state.
    ///
    /// (#1001 PR2 P2 / option-a) That `activity_age` gate can't tell a STARTING
    /// session (emitting startup output) from a mid-turn one - both are
    /// `activity_age < idle_threshold` - so on a still-starting candidate the busy
    /// fast-path injected into a not-paste-ready TUI and dropped (still-starting
    /// 83%). So this first routes on `alive_age` (`live_wake_route`): a candidate
    /// younger than `STARTUP_SETTLE_THRESHOLD` takes the #611 sustained-idle settle
    /// (waits out startup churn); only an established candidate uses the
    /// `activity_age` fast-path below.
    async fn settle_live_before_inject<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
    ) -> SettleReadiness {
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            hooks.settle_calls.lock().unwrap().push(session_id);
            let remove_session = hooks
                .remove_session_on_settle
                .lock()
                .unwrap()
                .remove(&session_id);
            if remove_session {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                mgr.destroy_session(session_id)
                    .await
                    .expect("remove test session after logical-command preflight");
            }
            // hooked tests exercise the inject wiring, not real timers
            return SettleReadiness::Unknown;
        }

        let Some(idle) = app.try_state::<std::sync::Arc<crate::pty::idle_detector::IdleDetector>>()
        else {
            // #2586 - no detector means no settle gate ran.
            return SettleReadiness::Unknown;
        };

        // (#1001 PR2 P2 / option-a) Classify starting vs established by alive_age
        // ONCE, at entry: alive_age at wake time is the time since PTY spawn, which
        // distinguishes a still-starting candidate (startup churn) from an
        // established one. alive_age only grows and the Starting branch is safe
        // regardless of a later crossing, so a single read suffices - re-classifying
        // per tick could switch a still-churning session onto the fast-path
        // mid-startup and re-open the drop.
        if live_wake_route(idle.alive_age(session_id), STARTUP_SETTLE_THRESHOLD)
            == LiveWakeRoute::Starting
        {
            // Still-starting LIVE candidate: in the same not-paste-ready state as a
            // cold spawn, so wait out startup churn with the #611 sustained-idle
            // settle (the gate PR1 measured at 0% drop) using the cold-spawn params
            // - notably the 90s cap, since a cold agent can take >10s to become
            // paste-ready (measured ~12-16s for Claude, startup_probe), well past
            // the live path's 10s cap. Best-effort: a destroy mid-settle just falls
            // through to the inject's own race path, with no readiness (#2586 D3:
            // readiness exists only on the `Ok` branch).
            return self
                .settle_until_ready(
                    app,
                    session_id,
                    std::time::Duration::from_secs(90),
                    std::time::Duration::from_millis(2000),
                    std::time::Duration::from_millis(500),
                    None,
                )
                .await
                .unwrap_or(SettleReadiness::Unknown);
        }

        // Established: gate on the real-time activity_age snapshot (grinch P1).
        let max_wait = std::time::Duration::from_secs(10);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();
        let menu_guard = app.try_state::<Arc<crate::pty::menu_guard::MenuGuard>>();

        loop {
            if menu_guard
                .as_ref()
                .is_some_and(|g| g.is_blocked(session_id))
            {
                if start.elapsed() >= max_wait {
                    return SettleReadiness::TimedOut;
                }
                tokio::time::sleep(poll).await;
                continue;
            }
            let Some(r) = idle.purge_readiness(&[session_id]).into_iter().next() else {
                return SettleReadiness::Unknown; // no snapshot: proceed to inject (best-effort)
            };
            let settle_live = r.idle_threshold + FRESH_IDLE_GUARD;
            match live_settle_action(
                r.activity_age,
                r.last_resize_age,
                r.resize_grace,
                r.idle_threshold,
                settle_live,
                start.elapsed(),
                max_wait,
            ) {
                SettleAction::InjectNow => {
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[mailbox] wake: live settle for {} hit the {}s cap; injecting anyway",
                            session_id,
                            max_wait.as_secs()
                        );
                        return SettleReadiness::TimedOut;
                    }
                    return SettleReadiness::Ready;
                }
                SettleAction::Wait => tokio::time::sleep(poll).await,
            }
        }
    }

    /// Inject a message into a session's PTY stdin.
    /// `interactive` = true (all remaining callers): live interactive `wake`
    /// delivery — plain message only, no response markers, no watcher.
    /// `interactive` = false is currently unreachable (the former
    /// `wake-and-sleep` non-interactive path was removed in 0.7.0). The
    /// `use_markers=true` branch below is retained for future non-interactive
    /// consumers; see _plans/delete-modes.md §2.4.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "P05 callers use guarded prepared paths; preserve this compatibility facade"
        )
    )]
    async fn inject_into_pty<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        settle: SettleReadiness,
    ) -> Result<(), String> {
        self.inject_into_pty_with_response(
            app,
            session_id,
            msg,
            interactive,
            origin,
            comanaged,
            settle,
            (None, None),
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the original peer injection flags, settle policy and borrowed response \
            context"
    )]
    async fn inject_into_pty_with_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
        interactive: bool,
        origin: WakeDeliveryOrigin,
        comanaged: bool,
        settle: SettleReadiness,
        context: (
            Option<&ResponseWakeWork>,
            Option<&crate::session::selection::OwnedReplacementContinuation>,
        ),
    ) -> Result<(), String> {
        let (response, owned_replacement) = context;
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        // Remote logical actions use the same canonical text-block injector as
        // messages. Resolve against the actual session shell before the busy
        // check, and never synthesize provider text from the wire value.
        if let Some(ref command) = msg.command {
            let (shell, waiting_for_input) = {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions
                    .iter()
                    .find(|session| session.id == session_id.to_string())
                {
                    Some(session) => (session.shell.clone(), session.waiting_for_input),
                    None => {
                        return Err(format!(
                            "Session not found: {} - cannot execute logical remote command '{}'",
                            session_id, command
                        ));
                    }
                }
            };
            let resolved = resolve_remote_pty_command(&shell, command)?;
            if !waiting_for_input {
                return Err(format!(
                    "Cannot execute remote command '{}': agent is busy (not idle)",
                    command
                ));
            }

            // Submit the resolved static text through the canonical injector.
            // The accepted idle-to-final-Enter race is the same one as normal
            // message delivery; delayed double Enter remains the shared defense.
            // #2336 - this is a peer wake (logical remote command), so it passes
            // through the typing hold like a standard wake.
            crate::pty::inject::inject_peer_wake_with_current_facts(
                app,
                session_id,
                resolved.text,
                &msg.id,
                settle,
                |session, current| {
                    if owned_replacement.is_none()
                        && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                            &msg.to, session.id,
                        )
                    {
                        return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
                    }
                    if owned_replacement.is_some_and(|continuation| {
                        !continuation.pre_write_work_matches(current, pty_mgr.inner())
                    }) {
                        return Err("replacement work binding requires recapture".into());
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|e| {
                if e == crate::pty::inject::OWNED_WORK_SCOPE_WAIT {
                    return e;
                }
                if !crate::pty::menu_guard::is_typing_hold_deferred_error(&e)
                    && is_permanent_delivery_error(&e)
                {
                    // Terminal rejection of a previously held wake: drop its
                    // count. A transient failure keeps it, because the message
                    // stays queued and may be deferred again.
                    crate::commands::pty::clear_held_wake(app, session_id, &msg.id);
                }
                let (level, outcome) = classify_injection_error(&e);
                log::log!(
                    level,
                    "[mailbox] PTY injection {} msg={} session={} logical={} resolved={}: {}",
                    outcome,
                    msg.id,
                    session_id,
                    command,
                    resolved.text,
                    e
                );
                e
            })?;

            log::info!(
                "[mailbox] logical PTY action executed msg={} session={} logical={} resolved={}",
                msg.id,
                session_id,
                command,
                resolved.text
            );

            // Logical clear is a fresh-conversation boundary regardless of
            // whether the provider text was /clear or Pi /new. Compact keeps
            // the conversation and does not stamp.
            if resolved.logical.creates_fresh_boundary() {
                crate::commands::pty::stamp_fresh_boundary_to_session(app, session_id).await;
            }

            let _ = tauri::Emitter::emit(
                app,
                "message_delivered",
                serde_json::json!({
                    "id": msg.id,
                    "from": msg.from,
                    "to": msg.to,
                    "mode": msg.mode,
                    "command": command,
                    "injected": true
                }),
            );

            // Logical actions keep the live child environment. If the message
            // has a follow-up body, inject it only after the agent becomes idle.
            // Credentials remain environment-only. Detached work never blocks
            // the delivery pipeline.
            let app_clone = app.clone();
            let msg_clone = msg.clone();
            let command_owned = command.clone();
            let resolved_text = resolved.text;
            tauri::async_runtime::spawn(async move {
                if !msg_clone.body.is_empty() {
                    if let Err(e) =
                        Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                            .await
                    {
                        log::warn!(
                            "[mailbox] Failed to inject follow-up after logical {} resolved as {} \
                            for session {}: {}",
                            command_owned,
                            resolved_text,
                            session_id,
                            e
                        );
                    }
                }
            });

            return Ok(());
        }

        // ── Standard message path ──
        // Only use response markers for non-interactive sessions
        let use_markers = msg.get_output && !interactive;
        let body_override = if is_container_backend_session(app, session_id)? {
            let sender_root = if origin == WakeDeliveryOrigin::FilesystemPoller
                && crate::phone::messaging::parse_file_notification(&msg.body).is_some()
            {
                Some(
                    self.resolve_repo_path(&msg.from, app)
                        .await
                        .ok_or_else(|| {
                            format!(
                        "Cannot resolve sender path for '{}' during container file delivery",
                        msg.from
                    )
                        })?,
                )
            } else {
                None
            };
            container_body_override_for_delivery(
                origin,
                &msg.body,
                sender_root.as_deref().map(Path::new),
            )?
        } else {
            None
        };
        let body = body_override.as_deref().unwrap_or(&msg.body);

        // Interactive and marker-less paths share the minimal PTY wrap via
        // `format_pty_wrap` (single source with `PTY_WRAP_FIXED` used by the
        // CLI clamp). Only the `--get-output` + `request_id` case wraps the
        // payload with response markers.
        let payload = match (use_markers, msg.request_id.as_ref()) {
            (true, Some(rid)) if comanaged => {
                let display_from =
                    crate::phone::messaging::compose_sender_for_comanaged_origin(&msg.from);
                crate::phone::messaging::format_pty_wrap_with_markers(&display_from, body, rid)
            }
            (true, Some(rid)) => {
                crate::phone::messaging::format_pty_wrap_with_markers(&msg.from, body, rid)
            }
            _ => format_wake_content(WakeContent::Peer {
                from: &msg.from,
                body,
                origin,
                comanaged,
            }),
        };

        // Register response watcher only for non-interactive sessions
        if use_markers {
            if let Some(ref rid) = msg.request_id {
                // Response file goes to the SENDER's .agentscommander/responses/
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let response_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let mgr = pty_mgr
                        .lock()
                        .map_err(|e| format!("PTY lock failed: {}", e))?;
                    mgr.register_response_watcher(session_id, rid.clone(), response_dir);
                    drop(mgr);
                }
            }
        }

        // SECURITY: this `first_100` log MUST NOT see credential values.
        // Credentials are env-only and must never be routed through PTY payloads.
        // Keep this log limited to standard message payloads.
        log::debug!(
            "[mailbox] Injecting into PTY session={} msg={} payload_len={} first_100={:?}",
            session_id,
            msg.id,
            payload.len(),
            payload.chars().take(100).collect::<String>()
        );
        // #2336 - a standard peer wake is the other gated delivery path; the
        // detached logical-command follow-up above stays on the plain injector.
        let prepared = self
            .prepare_response_request(app, msg, session_id, comanaged, response)
            .await;
        let attempt = response.and_then(|work| match work.attempt.lock() {
            Ok(attempt) => attempt.clone(),
            Err(_) => {
                response_diag_local!("inject", "attempt_lock_failure", work.diagnostic_message_id);
                None
            }
        });
        response_diag_debug_only(|| {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag \
                site={} outcome=observed stage=inject_presence id=unknown member={} prepared={} \
                attempt={} work={}",
                line!(),
                session_id,
                prepared.is_some(),
                attempt.is_some(),
                response.is_some()
            );
        });
        let mut diagnostic_prewrite_reached = false;
        crate::pty::inject::inject_peer_wake_with_current_facts(
            app,
            session_id,
            &payload,
            &msg.id,
            settle,
            |session, current| {
                response_diag_debug_only(|| {
                    diagnostic_prewrite_reached = true;
                });
                response_diag_local!(
                    "prewrite",
                    "callback_reached",
                    Uuid::parse_str(&msg.id).ok()
                );
                if owned_replacement.is_none()
                    && crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &msg.to, session.id,
                    )
                {
                    response_diag_local!(
                        "prewrite",
                        "owned_replacement_wait",
                        Uuid::parse_str(&msg.id).ok()
                    );
                    return Err(crate::pty::inject::OWNED_WORK_SCOPE_WAIT.into());
                }
                if owned_replacement.is_some_and(|continuation| {
                    !continuation.pre_write_work_matches(current, pty_mgr.inner())
                }) {
                    response_diag_local!(
                        "prewrite",
                        "owned_replacement_mismatch",
                        Uuid::parse_str(&msg.id).ok()
                    );
                    return Err("replacement work binding requires recapture".into());
                }
                if let (Some(prepared), Some(attempt), Some(work)) =
                    (prepared.as_ref(), attempt.as_ref(), response)
                {
                    reserve_response_request_at_write(
                        app, current, session, prepared, work, attempt,
                    );
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| {
            response_diag_local!(
                "inject",
                if diagnostic_prewrite_reached {
                    "failure_after_prewrite"
                } else {
                    "failure_before_prewrite"
                },
                Uuid::parse_str(&msg.id).ok()
            );
            if e == crate::pty::inject::OWNED_WORK_SCOPE_WAIT {
                return e;
            }
            if !crate::pty::menu_guard::is_typing_hold_deferred_error(&e)
                && is_permanent_delivery_error(&e)
            {
                crate::commands::pty::clear_held_wake(app, session_id, &msg.id);
            }
            let (level, outcome) = classify_injection_error(&e);
            log::log!(
                level,
                "[mailbox] PTY injection {} session={} msg={}: {}",
                outcome,
                session_id,
                msg.id,
                e
            );
            e
        })?;

        response_diag_local!(
            "inject",
            if diagnostic_prewrite_reached {
                "success_prewrite_reached"
            } else {
                "success_prewrite_not_reached"
            },
            Uuid::parse_str(&msg.id).ok()
        );
        log::info!(
            "[mailbox] PTY injection SUCCESS session={} msg={}",
            session_id,
            msg.id
        );
        // (#756) AC-injected message CONTENT creates a post-boundary transcript:
        // drop any pending fresh intent (record + mirror). Bare logical action
        // text (/clear, Pi /new, or /compact) never reaches this line.
        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        let _ = tauri::Emitter::emit(
            app,
            "message_delivered",
            serde_json::json!({
                "id": msg.id,
                "from": msg.from,
                "to": msg.to,
                "mode": msg.mode,
                "injected": true
            }),
        );
        Ok(())
    }

    /// Wait for agent to become idle after a remote command, then inject body as follow-up.
    /// Static method — can be spawned as a detached task without borrowing self.
    async fn inject_followup_after_idle_static<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let max_wait = std::time::Duration::from_secs(30);
        let poll = std::time::Duration::from_millis(500);
        let start = std::time::Instant::now();

        // Wait for idle (waiting_for_input = true)
        loop {
            if start.elapsed() >= max_wait {
                return Err(format!(
                    "Timeout waiting for agent to become idle after remote command ({}s)",
                    max_wait.as_secs()
                ));
            }
            tokio::time::sleep(poll).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == session_id.to_string()) {
                Some(s) if s.waiting_for_input => break,
                Some(_) => {} // busy — keep polling
                None => {
                    return Err(format!(
                        "Session {} destroyed before follow-up could be injected",
                        session_id
                    ))
                }
            }
        }

        // Inject the follow-up body as a standard interactive message.
        // Note: same TOCTOU race as the command path — agent could become busy
        // between the idle check above and this write. Acceptable for this use case.
        let payload = crate::phone::messaging::format_pty_wrap(&msg.from, &msg.body);
        crate::pty::inject::inject_text_into_session(app, session_id, &payload).await?;
        // A follow-up body after logical clear is post-boundary content.
        crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
        Ok(())
    }

    /// Find the best session for a given agent name (matches by working directory).
    /// Prefers active/running non-temp sessions over idle/exited ones.
    ///
    /// Used ONLY by stale-token logging (mailbox.rs:456, 501). Routing now uses
    /// `find_live_candidates` — do not add new callers. (grinch G.L5.)
    async fn find_active_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Option<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        log::debug!(
            "[mailbox] find_active_session for '{}' — {} sessions: {:?}",
            agent_name,
            sessions.len(),
            sessions
                .iter()
                .map(|s| format!(
                    "{}={} status={:?} name={}",
                    s.id, s.working_directory, s.status, s.name
                ))
                .collect::<Vec<_>>()
        );

        // §AR2-G2: exact-FQN filter. Post-§AR2-norm, `agent_name` is canonical
        // (or a legacy form that genuinely matches only one project's CWD). The
        // substring/suffix fuzziness from the pre-fix code is gone — cross-project
        // leakage is impossible at this layer.
        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        if matches.is_empty() {
            log::warn!("[mailbox] No session matched for '{}'", agent_name);
            return None;
        }

        log::debug!(
            "[mailbox] {} CWD matches for '{}': {:?}",
            matches.len(),
            agent_name,
            matches
                .iter()
                .map(|s| format!("{}({})", s.id, s.name))
                .collect::<Vec<_>>()
        );

        // Sort: non-temp first (false < true), then Active/Running before Idle before Exited
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let best = &matches[0];
        log::debug!(
            "[mailbox] Best match for '{}': session {} (name='{}', status={:?})",
            agent_name,
            best.id,
            best.name,
            best.status
        );
        Uuid::parse_str(&best.id).ok()
    }

    /// Find ALL viable CWD-matched candidates for wake delivery, sorted by
    /// preference (Active/Running first, then Idle, then Exited; non-temp before
    /// temp). Filters out records whose `SessionStatus` is non-`Exited` but
    /// whose PtyManager entry is missing — those are desync phantoms (issue
    /// #223). `Exited(_)` candidates are RETAINED for the respawn path.
    ///
    /// Returns `(viable, had_any_match)`:
    /// - `viable`: viable candidates with captured `SessionStatus` so the caller
    ///   can decide Inject-vs-RespawnExited without a second `list_sessions`
    ///   scan per candidate (grinch G.M2 / dev-rust R1.B3 alt). The captured
    ///   status may be stale by the time the caller acts on it; the
    ///   `err_is_pty_session_missing` continue arm in `deliver_wake` is the
    ///   load-bearing safety net for that race.
    /// - `had_any_match`: true if at least one CWD-matched record existed BEFORE
    ///   the predicate filter — the caller uses this to distinguish "no record
    ///   at all" (cold spawn) from "phantoms only" (warm spawn with auto-resume,
    ///   preserves on-disk Claude/Codex/Antigravity transcript). (grinch G.H2.)
    async fn find_live_candidates<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> (Vec<(Uuid, SessionStatus)>, bool /* had_any_match */) {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let mut matches = filter_sessions_by_fqn(&sessions, agent_name);

        let had_any_match = !matches.is_empty();
        if !had_any_match {
            return (Vec::new(), false);
        }

        // Same sort key as the legacy `find_active_session`.
        // TODO(#223-fu1): once AC3's PTY-exit hook lands and starts emitting
        // non-zero exit codes, add a tertiary tie-break (e.g. created_at) so
        // Exited-within-bucket ordering is deterministic. (grinch G.L2.)
        matches.sort_by_key(|s| {
            let is_temp = s
                .name
                .starts_with(crate::session::session::TEMP_SESSION_PREFIX);
            let status = match s.status {
                SessionStatus::Active | SessionStatus::Running => 0u8,
                SessionStatus::Idle => 1,
                SessionStatus::Exited(_) => 2,
            };
            (is_temp, status)
        });

        let candidates: Vec<(Uuid, SessionStatus, String)> = matches
            .iter()
            .filter_map(|s| {
                Uuid::parse_str(&s.id)
                    .ok()
                    .map(|id| (id, s.status.clone(), s.name.clone()))
            })
            .collect();
        drop(mgr);

        let mut viable = Vec::new();
        for (id, status, name) in candidates {
            let has_pty = self.has_pty_session_for_wake(app, id).await;
            if !is_viable_wake_candidate(&status, has_pty) {
                // Phantom skip — observable in prod logs to scope the AC3
                // follow-up. (dev-rust R1.B4.)
                log::warn!(
                    "[mailbox] skipping desync phantom: id={} status={:?} has_pty={} name='{}'",
                    id,
                    status,
                    has_pty,
                    name
                );
            } else {
                viable.push((id, status));
            }
        }

        log::debug!(
            "[mailbox] {} viable wake candidate(s) for '{}' (had_any_match={}): {:?}",
            viable.len(),
            agent_name,
            had_any_match,
            viable
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>(),
        );
        (viable, had_any_match)
    }

    /// Locate the live Root Agent session for routing `msg.to == ROOT_AGENT_SENDER`.
    ///
    /// Returns the first match by `SessionManager` iteration order. Persistence
    /// dedup (`config/sessions_persistence.rs:242-329`) converges to a single
    /// root session at steady state, but multiple records may exist transiently
    /// during concurrent spawns — a defensive log fires when more than one
    /// matches so future code does not silently rely on "exactly one" as a
    /// structural invariant.
    ///
    /// Filters via `is_viable_root_recipient` (stricter than
    /// `is_viable_wake_candidate`) — Exited records are returned as `None` so
    /// the caller's no-spawn guard sees `(empty, false)` instead of triggering
    /// the destroy-and-respawn path on a user-launched session.
    async fn find_root_session_candidate<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Option<(Uuid, SessionStatus)> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();

        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;

        let matches: Vec<_> = sessions
            .iter()
            .filter(|s| {
                s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
            })
            .collect();
        if matches.len() > 1 {
            log::warn!(
                "[mailbox] root recipient: {} root sessions found ({}); routing to first. \
                Persistence dedup should converge.",
                matches.len(),
                matches
                    .iter()
                    .map(|s| s.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let candidate = matches.into_iter().next()?;

        let id = Uuid::parse_str(&candidate.id).ok()?;
        let pty = pty_mgr.lock().unwrap();
        let has_pty = pty.has_session(id);
        drop(pty);

        if !is_viable_root_recipient(&candidate.status, has_pty) {
            log::warn!(
                "[mailbox] root recipient: not viable (id={} status={:?} has_pty={}); preserving \
                record, no destroy",
                id,
                candidate.status,
                has_pty
            );
            return None;
        }

        Some((id, candidate.status.clone()))
    }

    /// Find ALL sessions matching an agent name (by working directory).
    /// Returns all matching session UUIDs, not just the "best" one.
    ///
    /// §AR2-G2: exact-FQN filter (same simplification as `find_active_session`).
    async fn find_all_sessions<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Vec<Uuid> {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        filter_sessions_by_fqn(&sessions, agent_name)
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s.id).ok())
            .collect()
    }

    /// Handle close-session action: validate coordinator auth, find target sessions, destroy them.
    ///
    /// NEW ACTION HANDLERS: resolve user-supplied target fields via
    /// `config::teams::resolve_agent_target` BEFORE privileged operations.
    /// The outbox is a trust boundary — any new destructive action must
    /// canonicalize its target here, not rely on CLI-side resolution.
    ///
    /// §224 G-IMPL-2 — `is_app_outbox`: true when the message file lives
    /// under the instance-private app-outbox (master/root-token path).
    /// Used by the response-write block (A.6) to skip the outbox-relative
    /// primary write — for app-outbox messages it would land under
    /// `<config_dir>/instances/<id>/responses/`, a directory the CLI does
    /// not poll, leaking orphan JSON files with no GC.
    async fn handle_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let raw_target = msg
            .target
            .as_deref()
            .ok_or_else(|| "close-session requires 'target' field".to_string())?;

        // §AR2-G1: resolve the target to a canonical FQN BEFORE authorization.
        // Even if the CLI skipped resolution (direct outbox write, old client,
        // hand-crafted JSON), the mailbox is the authoritative gate.
        let resolved_target = {
            let paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match crate::config::teams::resolve_agent_target(raw_target, &paths) {
                Ok(fqn) => fqn,
                Err(e) => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!("close-session target unresolvable: {}", e),
                        )
                        .await;
                }
            }
        };
        let target = resolved_target.as_str();

        // Re-check master token for coordinator auth bypass (independent of routing bypass above)
        let is_master = if let Some(ref token_str) = msg.token {
            let master = app.state::<MasterToken>();
            if master.matches(token_str) {
                true
            } else {
                let settings = crate::config::settings::load_settings();
                settings.root_token.as_deref() == Some(token_str.as_str())
            }
        } else {
            false
        };

        if !is_master {
            let discovered = teams::discover_teams();
            if !teams::is_coordinator_of(&msg.from, target, &discovered) {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "Not authorized: '{}' is not an orchestrator of '{}' team",
                            msg.from, target
                        ),
                    )
                    .await;
            }
        }

        // Find all sessions for the target agent.
        //
        // §224 A.2 — empty `session_ids` is NOT an error. The pre-fix code
        // rejected with "No active session found", which conflicted with
        // `list-sessions` reporting the session as alive (ghost rows from
        // `persist_merging_failed`; see A.1). Now we fall through to a
        // successful no-op response with status="no_match".
        //
        // §224 A.2.5 — daemon-restart race guard. If the initial probe is
        // empty AND restore is still in progress, poll up to 5s for either
        // (a) the flag to clear, or (b) a matching session to appear. Three
        // outcomes:
        //   * session appears   → fall through to the kill loop, status="closed".
        //   * flag clears empty → fall through to no_match path.
        //   * 5s elapses, flag still set → restore_in_progress_result = true.
        let mut session_ids = self.find_all_sessions(app, target).await;
        let mut restore_in_progress_result = false;
        if session_ids.is_empty() {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let poll = std::time::Duration::from_millis(100);
                // We can't pass `self.find_all_sessions(...)` as a closure
                // because of the `&self` capture + `async` future shape, so
                // inline the wait loop instead of going through the pure helper.
                // The helper's logic is unit-tested separately (D.5a).
                loop {
                    if std::time::Instant::now() >= deadline {
                        if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                            restore_in_progress_result = true;
                        }
                        break;
                    }
                    tokio::time::sleep(poll).await;
                    session_ids = self.find_all_sessions(app, target).await;
                    if !session_ids.is_empty() {
                        break;
                    }
                    if !restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                        // Flag cleared mid-wait. One final probe (the restore
                        // task may have just inserted our target as its last
                        // act before the guard dropped), then fall through.
                        session_ids = self.find_all_sessions(app, target).await;
                        break;
                    }
                }
            }
        }

        let force = msg.force.unwrap_or(false);
        let timeout_secs = msg.timeout_secs.unwrap_or(30);

        // §224 A.2 — gate the kill-loop log on non-empty session_ids so we
        // don't emit a misleading "force-killing 0 session(s)" line on the
        // no_match / restore_in_progress paths.
        if !session_ids.is_empty() {
            log::info!(
                "[mailbox] close-session: {} {} session(s) for '{}' (requested by '{}', \
                timeout={}s)",
                if force {
                    "force-killing"
                } else {
                    "gracefully closing"
                },
                session_ids.len(),
                target,
                msg.from,
                timeout_secs
            );
        }

        let mut closed_ids: Vec<String> = Vec::new();
        for sid in &session_ids {
            let success = if force {
                self.force_close_session(app, *sid).await
            } else {
                self.graceful_close_session(app, *sid, timeout_secs).await
            };
            if success {
                closed_ids.push(sid.to_string());
            }
        }

        // §224 A.7 — active ghost cleanup. After A.2.5 has confirmed (with
        // wait-and-retry) that no session matches the target, force a
        // sessions.json rewrite from the live SessionManager to drop any
        // stale persisted entry. Without this, a user with only the ghost
        // session sees the contradiction persist across multiple
        // close-session invocations (zero lifecycle events to trigger
        // passive cleanup). Cost: one disk write per no-match call.
        //
        // Skip when A.2.5 returned restore_in_progress: the restore task
        // is itself about to write the snapshot when it completes; racing
        // that write is wasted I/O and would clobber recipes for sessions
        // whose restore is pending.
        //
        // Skip when session_ids is non-empty: that's either "closed" or
        // "already_closed", and the destroy path's downstream events will
        // trigger persist_current_state organically.
        //
        // §224 G-IMPL-4 (NIT, accepted) — snapshot vs concurrent create_session
        // is racy in the ~5-50ms window between `snapshot_sessions` and
        // `save_sessions`. If a `create_session` for any target lands in
        // that window and runs its own `persist_current_state` BEFORE our
        // `save_sessions` writes, the new session is dropped from disk
        // until the next lifecycle event re-persists. Impact: brief disk
        // inconsistency, recoverable. Accepted.
        //
        // §224 G-IMPL-6 (NIT, accepted) — A.7 also drops unrelated failed-
        // recoverable ghosts. The snapshot includes only live SessionManager
        // entries; if `sessions.json` has a failed-recoverable ghost for
        // unrelated agent X, this rewrite drops X's ghost too. This is the
        // pre-existing behavior of every `persist_current_state` caller
        // (see `persist_merging_failed` docstring); A.7 just introduces a
        // new caller outside lifecycle events. Accepted.
        if session_ids.is_empty() && !restore_in_progress_result {
            // Persist through the serialized snapshot+write path so this cleanup
            // cannot replay a stale snapshot after a completed Telegram toggle.
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            if let Err(e) =
                crate::config::sessions_persistence::persist_current_state_result(&mgr).await
            {
                log::warn!(
                    "[mailbox] close-session: failed to persist cleaned sessions.json after \
                    no_match: {}",
                    e
                );
            }
        }

        // Write response with details.
        //
        // §224 A.2 + A.2.5 — four terminal states, all exit-0 from the user's
        // seat:
        //   "restore_in_progress" — daemon still in startup; couldn't decide.
        //   "no_match"            — found zero live sessions matching the FQN.
        //   "already_closed"      — found some, but every one vanished before
        //                           destroy (race).
        //   "closed"              — at least one was actively killed.
        if let Some(ref rid) = msg.request_id {
            let status = if restore_in_progress_result {
                "restore_in_progress"
            } else if session_ids.is_empty() {
                "no_match"
            } else if closed_ids.is_empty() {
                "already_closed"
            } else {
                "closed"
            };
            let response = serde_json::json!({
                "action": "close-session",
                "target": target,
                "status": status,
                "sessions_closed": closed_ids.len(),
                "session_ids": closed_ids,
                "requested_by": msg.from,
            });
            let json = match serde_json::to_string_pretty(&response) {
                Ok(j) => j,
                Err(e) => {
                    log::warn!(
                        "[mailbox] Failed to serialize close-session response: {}",
                        e
                    );
                    return self.move_to_delivered(app, path, msg).await;
                }
            };

            // §224 A.6 — dual-write the response:
            //
            // (1) <message_file_dir>/../responses/<rid>.json — always derivable
            //     from `path` (the queued message's file location). This is
            //     exactly `<ac_dir>/responses/<rid>.json`, the CLI's polled
            //     location (close_session.rs:194-195), so no resolve_repo_path
            //     dependency.
            //
            // (2) <resolve_repo_path(msg.from)>/<agent_local_dir>/responses/<rid>.json
            //     — preserves cross-agent delivery for cases where the sender
            //     FQN points to a different ac_dir than the outbox file's
            //     parent. Best-effort; failure does not affect (1).
            //
            // Either write succeeding is enough for the CLI to receive the
            // response.
            //
            // §224 G-IMPL-2 — skip the derived primary write (1) when the
            // message came from the app-outbox (master/root-token path).
            // For app-outbox messages the parent path is
            // `<config_dir>/instances/<id>/outbox/`, so the derived responses
            // dir resolves to `<config_dir>/instances/<id>/responses/` — a
            // directory the CLI never polls (it always polls
            // `<--root>/.<bin_stem>/responses/`). Writing there leaks orphan
            // JSON files with no GC; the resolved-sender path (2) is the only
            // useful target for the master-token case. If `resolve_repo_path`
            // also fails (msg.from not enumerable), the CLI hits the response
            // timeout and exits 2 (G-IMPL-3) — "outcome unknown".
            if !is_app_outbox {
                let outbox_relative_responses_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|ac_dir| ac_dir.join("responses"));
                if let Some(responses_dir) = outbox_relative_responses_dir {
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let response_path = responses_dir.join(format!("{}.json", rid));
                    if let Err(e) = std::fs::write(&response_path, &json) {
                        log::warn!(
                            "[mailbox] Failed to write close-session response to outbox-relative \
                            path {:?}: {}",
                            response_path,
                            e
                        );
                    }
                } else {
                    log::warn!(
                        "[mailbox] close-session: cannot derive outbox-relative responses dir from \
                        message path {:?}",
                        path
                    );
                }
            }

            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let responses_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name())
                    .join("responses");
                let _ = std::fs::create_dir_all(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::write(&response_path, &json) {
                    log::warn!(
                        "[mailbox] Failed to write close-session response to resolved-sender path: \
                        {}",
                        e
                    );
                }
            } else if is_app_outbox {
                // §224 G-IMPL-2 — app-outbox call AND resolve_repo_path failed
                // means the CLI's `--root` is not enumerable in project_paths.
                // Both write paths above are unreachable; the CLI will hit its
                // response-poll timeout and exit 2 ("outcome unknown",
                // G-IMPL-3). Log explicitly so operators can debug.
                log::warn!(
                    "[mailbox] close-session app-outbox response is undeliverable: \
                     resolve_repo_path(msg.from='{}') returned None — the sender's --root \
                     is not enumerable in project_paths. CLI will timeout with exit 2.",
                    msg.from
                );
            }
        }

        // Move original message to delivered/
        self.move_to_delivered(app, path, msg).await
    }

    // ── (#885) purge-wg ──────────────────────────────────────────────────

    /// (#885) Handle `purge-wg`: purge every peer in the caller's own workgroup.
    ///
    /// Sequence is non-reorderable. Steps 1-14 per plan §5.5c, consensus round 1.
    async fn handle_purge_wg<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Identity gate — already enforced by the caller (§5.5b).
        debug_assert!(
            msg.token.is_some(),
            "purge-room requires a session token; caller must enforce"
        );

        // 2. Resolve WG scope and authorization, one call (§3.6).
        let effective_paths = {
            let mut paths = {
                let cfg = app.state::<SettingsState>();
                let c = cfg.read().await;
                c.project_paths.clone()
            };
            match derive_project_from_outbox_path(path) {
                Ok(Some(root_project)) => {
                    let canon = std::fs::canonicalize(&root_project).ok();
                    let already = paths.iter().any(|p| match &canon {
                        Some(ct) => std::fs::canonicalize(p).ok().as_ref() == Some(ct),
                        None => p == &root_project,
                    });
                    if !already {
                        paths.push(root_project);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return self.reject_message(path, msg, &e).await;
                }
            }
            paths
        };
        let wg =
            match crate::config::teams::verified_wg_coordinator_target(&msg.from, &effective_paths)
            {
                Some(wg) => wg,
                None => {
                    return self
                        .reject_message(
                            path,
                            msg,
                            &format!(
                                "Not authorized: '{}' is not the verified orchestrator of its room",
                                msg.from
                            ),
                        )
                        .await;
                }
            };

        // 3. --wg assertion (§3.6: interlock, not selector).
        if let Some(ref t) = msg.target {
            if t != &wg.wg_name {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "purge-room: --wg assertion '{}' does not match resolved room '{}'",
                            t, wg.wg_name
                        ),
                    )
                    .await;
            }
        }

        // 4. Enumerate peers (Guard A). Only `__agent_*` dirs under the WG dir.
        //    The coordinator does not purge itself.
        let wg_dir = match wg.replica_dir.parent() {
            Some(d) => d,
            None => {
                return self
                    .reject_message(path, msg, "purge-room: cannot resolve Room directory")
                    .await;
            }
        };
        let mut peer_fqns: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(wg_dir) {
            for entry in entries.flatten() {
                let name = match entry.file_name().to_str() {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let agent = match name.strip_prefix("__agent_") {
                    Some(a) => a.to_string(),
                    None => continue,
                };
                if agent == wg.agent_name {
                    continue;
                }
                if !entry.path().is_dir() {
                    continue;
                }
                peer_fqns.push(format!("{}:{}/{}", wg.project, wg.wg_name, agent));
            }
        }
        peer_fqns.sort();

        // 5. Take the mirror snapshot ONCE (F-4). One list_sessions(), one
        //    PTY-liveness pass. Per-peer filtering reuses the pure predicate.
        let sessions: Vec<SessionInfo> = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.list_sessions().await
        };
        let pty_live: std::collections::HashSet<Uuid> = {
            let pty = app.state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>();
            let mgr = pty.lock().unwrap();
            sessions
                .iter()
                .filter_map(|s| Uuid::parse_str(&s.id).ok())
                .filter(|id| mgr.has_session(*id))
                .collect()
        };

        // 6. Restore-in-progress guard (F-2: exit 3, not 0).
        {
            let restore_flag = app.state::<Arc<crate::RestoreInProgress>>();
            if restore_flag.0.load(std::sync::atomic::Ordering::SeqCst) {
                let response = serde_json::json!({
                    "action": PURGE_WG_ACTION,
                    "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                    "status": "restore_in_progress",
                    "requested_by": msg.from,
                    "peers": [],
                });
                return self
                    .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                    .await;
            }
        }

        // 7. Liveness and Guard B (F-3: !Exited && has_pty; F-12: root guard).
        //    Collect live session info; the readiness snapshot (step 8) is
        //    correlated into gate_peers after.
        struct PeerLiveInfo {
            fqn: String,
            all_session_ids: Vec<String>,
            live_sessions: Vec<(Uuid, bool)>, // (session_id, mirror_idle)
        }
        let mut peer_infos: Vec<PeerLiveInfo> = Vec::with_capacity(peer_fqns.len());
        let mut all_live_session_ids: Vec<Uuid> = Vec::new();

        for fqn in &peer_fqns {
            let matched: Vec<&SessionInfo> = filter_sessions_by_fqn(&sessions, fqn);
            let all_session_ids: Vec<String> = matched.iter().map(|s| s.id.clone()).collect();

            let mut live_sessions: Vec<(Uuid, bool)> = Vec::new();
            for s in &matched {
                // Guard B (root): corrupted-state assertion (F-12).
                if s.is_root_agent
                    || crate::config::root_agent::is_root_agent_path(&s.working_directory)
                {
                    let response = serde_json::json!({
                        "action": PURGE_WG_ACTION,
                        "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                        "status": "failed_root_guard",
                        "requested_by": msg.from,
                        "offending_session_id": s.id,
                        "offending_working_directory": s.working_directory,
                        "peers": [],
                    });
                    return self
                        .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                        .await;
                }

                let sid = match Uuid::parse_str(&s.id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let is_live =
                    !matches!(s.status, SessionStatus::Exited(_)) && pty_live.contains(&sid);
                if is_live {
                    let mirror_idle =
                        !matches!(s.status, SessionStatus::Active | SessionStatus::Running)
                            || s.waiting_for_input;
                    live_sessions.push((sid, mirror_idle));
                    all_live_session_ids.push(sid);
                }
            }

            peer_infos.push(PeerLiveInfo {
                fqn: fqn.clone(),
                all_session_ids,
                live_sessions,
            });
        }

        // 8. Readiness snapshot and the four-leg gate (§2.3.3, F-1).
        let quiet = std::time::Duration::from_millis(
            msg.quiet_period_ms.unwrap_or(3000).max(
                crate::session::profile::IdleTuning::DEFAULT
                    .idle_threshold
                    .as_millis() as u64,
            ),
        );
        let readiness_map: std::collections::HashMap<
            Uuid,
            crate::pty::idle_detector::PurgeReadiness,
        > = {
            let idle = app.state::<Arc<crate::pty::idle_detector::IdleDetector>>();
            let readiness = idle.purge_readiness(&all_live_session_ids);
            readiness.into_iter().map(|r| (r.session_id, r)).collect()
        };
        // Correlate into gate_peers.
        let gate_peers: Vec<PurgeGatePeer> = peer_infos
            .iter()
            .map(|info| PurgeGatePeer {
                fqn: info.fqn.clone(),
                all_session_ids: info.all_session_ids.clone(),
                live: info
                    .live_sessions
                    .iter()
                    .map(|(sid, mirror_idle)| PurgeGateSession {
                        session_id: *sid,
                        readiness: readiness_map.get(sid).copied().unwrap_or(
                            crate::pty::idle_detector::PurgeReadiness {
                                session_id: *sid,
                                activity_age: None,
                                watcher_idle: false,
                                last_resize_age: None,
                                resize_grace: crate::session::profile::IdleTuning::DEFAULT
                                    .resize_grace,
                                idle_threshold: crate::session::profile::IdleTuning::DEFAULT
                                    .idle_threshold,
                                silence_age: None,
                            },
                        ),
                        mirror_idle: *mirror_idle,
                    })
                    .collect(),
            })
            .collect();

        let decision = evaluate_gate(&gate_peers, quiet);

        // 9. Dry-run exit, BEFORE the gate rejection (F-9).
        if msg.dry_run == Some(true) {
            let status = if decision.passed {
                "dry_run_ready"
            } else {
                "dry_run_blocked"
            };
            // (N-3) The dry-run path reports `purgeable` and the diagnostic
            // fields, NOT `outcome` (which is the gate's internal "busy"/
            // "skipped"/"untracked" vocabulary and contradicts `would_purge:
            // true` when every peer is "skipped" because it has no live
            // session). An operator reads `purgeable: true`, not "skipped".
            let response = serde_json::json!({
                "action": PURGE_WG_ACTION,
                "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                "status": status,
                "requested_by": msg.from,
                "quiet_period_ms": quiet.as_millis() as u64,
                "dry_run": true,
                "would_purge": decision.passed,
                "peers": decision.peers.iter().map(|p| serde_json::json!({
                    "name": p.fqn,
                    "purgeable": p.purgeable,
                    "idle_ms": p.idle_ms,
                    "silence_ms": p.silence_ms,
                    "watcher_idle": p.watcher_idle,
                    "mirror_idle": p.mirror_idle,
                    "resize_settled": p.resize_settled,
                    "session_ids": p.session_ids,
                })).collect::<Vec<_>>(),
            });
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 10. The gate. If any peer is not purgeable: reject, destroy nothing.
        if !decision.passed {
            let response = serde_json::json!({
                "action": PURGE_WG_ACTION,
                "workgroup": format!("{}:{}", wg.project, wg.wg_name),
                "status": "rejected_busy",
                "requested_by": msg.from,
                "quiet_period_ms": quiet.as_millis() as u64,
                "peers": decision.peers.iter().map(|p| serde_json::json!({
                    "name": p.fqn,
                    "outcome": p.outcome,
                    "purgeable": p.purgeable,
                    "idle_ms": p.idle_ms,
                    "silence_ms": p.silence_ms,
                    "watcher_idle": p.watcher_idle,
                    "mirror_idle": p.mirror_idle,
                    "resize_settled": p.resize_settled,
                    "session_ids": p.session_ids,
                })).collect::<Vec<_>>(),
            });
            return self
                .write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
                .await;
        }

        // 11. Acquire the lease (F-14: clone Arc into a named local first).
        let mut target_sids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut target_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for peer in &gate_peers {
            for session in &peer.live {
                target_sids.insert(session.session_id);
            }
            target_fqns.insert(peer.fqn.clone());
        }
        // Also include non-live session IDs (Exited records) for destruction.
        for peer in &gate_peers {
            for sid_str in &peer.all_session_ids {
                if let Ok(sid) = Uuid::parse_str(sid_str) {
                    target_sids.insert(sid);
                }
            }
        }
        let purge_guard: Arc<crate::session::purge_guard::PurgeGuard> = app
            .state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .inner()
            .clone();
        let lease = purge_guard.acquire(target_sids, target_fqns).await;

        // 12. Destroy loop (past the commit point, §2.5; no re-check).
        let force = msg.force.unwrap_or(true);
        let timeout_secs = msg.timeout_secs.unwrap_or(5);
        let mut closed_ids: Vec<String> = Vec::new();
        let mut failed_ids: Vec<String> = Vec::new();
        let mut already_closed_ids: Vec<String> = Vec::new();
        let mut any_failed = false;

        for peer in &gate_peers {
            for sid_str in &peer.all_session_ids {
                let sid = match Uuid::parse_str(sid_str) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let ok = if force {
                    self.force_close_session(app, sid).await
                } else {
                    self.graceful_close_session(app, sid, timeout_secs).await
                };
                if ok {
                    closed_ids.push(sid_str.clone());
                } else {
                    // (#885 E-4) Post-failure re-probe: if the record is gone,
                    // the destroy failed because auto-close raced us and the
                    // session is already destroyed. That is success, not
                    // failure. Sound because `destroy_session_inner` returns
                    // Err("Session not found") ONLY from its first statement;
                    // every other failure (notably kill_group's ??) leaves
                    // the record present, so a genuine failure is always
                    // distinguishable by the record still being there.
                    let gone = {
                        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                        let mgr = session_mgr.read().await;
                        mgr.get_session(sid).await.is_none()
                    };
                    if gone {
                        already_closed_ids.push(sid_str.clone());
                    } else {
                        failed_ids.push(sid_str.clone());
                        any_failed = true;
                    }
                }
            }
        }

        // 13. Drop the lease before writing the response.
        drop(lease);

        // 14. Response + move_to_delivered.
        let status = if any_failed {
            "partial_failure"
        } else {
            "purged"
        };
        let response = serde_json::json!({
            "action": PURGE_WG_ACTION,
            "workgroup": format!("{}:{}", wg.project, wg.wg_name),
            "status": status,
            "requested_by": msg.from,
            "quiet_period_ms": quiet.as_millis() as u64,
            "dry_run": false,
            "purged": closed_ids.len(),
            "already_closed": already_closed_ids.len(),
            "failed": failed_ids.len(),
            "peers": decision.peers.iter().map(|p| {
                let outcome = if failed_ids.iter().any(|f| p.session_ids.contains(f)) {
                    "failed"
                } else if already_closed_ids.iter().any(|c| p.session_ids.contains(c))
                    && !closed_ids.iter().any(|c| p.session_ids.contains(c))
                {
                    "already_closed"
                } else if closed_ids.iter().any(|c| p.session_ids.contains(c)) {
                    "closed"
                } else {
                    "no_match"
                };
                serde_json::json!({
                    "name": p.fqn,
                    "outcome": outcome,
                    "session_ids": p.session_ids,
                    "purgeable": p.purgeable,
                })
            }).collect::<Vec<_>>(),
        });

        self.write_purge_response_and_deliver(app, path, msg, is_app_outbox, &response)
            .await
    }

    /// (#885) Write the purge response JSON and move the message to delivered/.
    /// Mirrors `handle_close_session`'s dual-write block (§224 A.6, G-IMPL-2).
    async fn write_purge_response_and_deliver<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
        response: &serde_json::Value,
    ) -> Result<(), String> {
        let json = match serde_json::to_string_pretty(response) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("[mailbox] Failed to serialize purge-wg response: {}", e);
                return self.move_to_delivered(app, path, msg).await;
            }
        };

        if let Some(ref rid) = msg.request_id {
            if !is_app_outbox {
                let outbox_relative_responses_dir = path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|ac_dir| ac_dir.join("responses"));
                if let Some(responses_dir) = outbox_relative_responses_dir {
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let response_path = responses_dir.join(format!("{}.json", rid));
                    if let Err(e) = std::fs::write(&response_path, &json) {
                        log::warn!(
                            "[mailbox] Failed to write purge-wg response to outbox-relative path \
                            {:?}: {}",
                            response_path,
                            e
                        );
                    }
                }
            }

            if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                let responses_dir = std::path::PathBuf::from(sender_path)
                    .join(crate::config::agent_local_dir_name())
                    .join("responses");
                let _ = std::fs::create_dir_all(&responses_dir);
                let response_path = responses_dir.join(format!("{}.json", rid));
                if let Err(e) = std::fs::write(&response_path, &json) {
                    log::warn!(
                        "[mailbox] Failed to write purge-wg response to resolved-sender path: {}",
                        e
                    );
                }
            }
        }

        self.move_to_delivered(app, path, msg).await
    }
    /// Returns fast: the 30s sustained-idle wait runs in a detached task so the
    /// poll loop is never blocked. Idempotent: a second request while one is
    /// pending is a no-op ("already_queued").
    async fn handle_self_clear<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Resolve the caller's OWN session from the token (the sole authority).
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-clear: no live session owns this token; restart or respawn the \
                        session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "self-clear: internal error resolving session id")
                    .await;
            }
        };

        // Resolve before the handoff existence gate so an unsupported source
        // cannot archive or queue self-maintenance state.
        let clear_text = match crate::pty::inject::resolve_logical_command_text(
            &session.shell,
            LogicalPtyCommand::Clear,
        ) {
            Some(text) => text,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "self-handoff-and-clear: session shell '{}' has no verified \
                            logical-clear mapping. Claude / Codex / Antigravity / Cursor agent \
                            direct shells use /clear; exact Pi uses /new. cmd / pwsh outer \
                            wrappers remain unsupported.",
                            session.shell
                        ),
                    )
                    .await;
            }
        };

        // 2b. #626 existence gate - REFUSE if the agent did not write its handoff notes. Clearing
        // with
        // no SELF-HANDOFF.md would wipe context with no way to resume (the agent would post-clear
        // read a nonexistent file = blank). Queue-time intent guard (not transactional): the real
        //     read-time safety net is the handoff prompt's "if missing or empty, wait" clause
        //     (self_clear_handoff_base_prompt). Use .is_file() so a stray directory named
        //     SELF-HANDOFF.md does not pass. Runs BEFORE the idempotency insert and the archive, so
        //     nothing is queued/archived with nothing to resume.
        let handoff_path =
            std::path::Path::new(&session.working_directory).join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-clear: SELF-HANDOFF.md not found in your root ({}); \
                         write it before requesting self-handoff-and-clear.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 3. Idempotency - atomic check-and-set. insert() returns false if already present.
        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        // 4. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the poller).
        //    MED-4: gated under #[cfg(not(test))] so handle_self_clear unit tests assert
        //    queue + idempotency WITHOUT launching a live 500ms poll task. The gate's
        //    decision policy is covered separately by the pure `self_clear_gate_advance` tests.
        //    Durations are passed in so a future integration test can drive it fast.
        if newly_inserted {
            // #626 - archive SELF-FORGET.md so the next cycle starts fresh. Best-effort: a failure
            // must not block the clear/handoff. The agent already wrote SELF-HANDOFF.md
            // (existence-gated above), so SELF-FORGET.md is present iff the agent kept one. FOLD-3:
            // runs in ALL cfgs (NOT inside the cfg(not(test)) spawn) so the harness archive
            // assertion can pass. Decoupled from clear success (queue-time): an abandoned cycle
            // leaves SELF-FORGET archived but uncleared; content is preserved in
            // self-clear/<ts>_SELF-FORGET.md, re-issue continues normally.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-clear: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-clear: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                // #749 - the root travels with the task (same queue-time snapshot the
                // SELF-FORGET archive above used) so Phase 2 can archive the handoff
                // without a mid-flight session-manager lookup. A session's working
                // directory never changes while its id stays alive.
                let root = root.to_path_buf();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_clear_after_sustained_idle(
                        &app_clone,
                        session_id,
                        clear_text,
                        root,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = (clear_text, &forgotten_summary);
            log::info!(
                "[mailbox] self-handoff-and-clear queued for session {} (from '{}')",
                session_id,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-clear already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 5. Write the queue-ack response, then move the message to delivered/.
        self.write_self_clear_response(app, path, msg, session_id, status, is_app_outbox)
            .await
    }

    fn session_has_visible_raise_hand_slot(session: &SessionInfo) -> bool {
        if !session.is_coordinator || matches!(&session.status, SessionStatus::Exited(_)) {
            return false;
        }
        let Some(task_path) =
            crate::session::session::find_workgroup_task_path_for_cwd(&session.working_directory)
        else {
            return false;
        };
        let Ok(content) = std::fs::read_to_string(task_path) else {
            return false;
        };
        crate::commands::entity_creation::parse_task_title(&content).is_some()
    }

    async fn handle_raise_hand<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "raise-hand requires a valid session token; restart or respawn the session",
                    )
                    .await;
            }
        };
        let mgr = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let guard = session_mgr.read().await;
            guard.clone()
        };
        let Some(session) = mgr.find_by_token(token_uuid).await else {
            return self
                .reject_message(
                    path,
                    msg,
                    "raise-hand: no live session owns this token; restart or respawn the session",
                )
                .await;
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(path, msg, "raise-hand: internal error resolving session id")
                    .await;
            }
        };

        if !Self::session_has_visible_raise_hand_slot(&session) {
            return self
                .write_raise_hand_response(
                    app,
                    path,
                    msg,
                    session_id,
                    false,
                    "not_visible",
                    is_app_outbox,
                )
                .await;
        }

        // #698 - raise the hand and persist its snapshot atomically with respect
        // to all session persistence (see `raise_hand_and_persist_result`). The
        // mutation, snapshot, and save happen under one global save lock, so no
        // concurrent persist can durably write the raised state before this
        // snapshot lands. The snapshot is durable before we emit the UI event or
        // write the success response, so a peer reading `list-sessions` sees
        // `raisedHand: true` only after it survived. On persist failure the helper
        // has already rolled back the live communication under the same lock, so
        // we just reject the message rather than reporting a success that did not
        // survive.
        let outcome = crate::config::sessions_persistence::raise_hand_and_persist_result(
            &mgr,
            session_id,
            chrono::Utc::now(),
        )
        .await;
        let (raised, status, changed_communication) = match outcome {
            Ok(RaiseHandPersistOutcome::Raised(communication)) => {
                (true, "raised", Some(communication))
            }
            Ok(RaiseHandPersistOutcome::AlreadyVisible) => (true, "already_visible", None),
            Ok(RaiseHandPersistOutcome::NotRaisable) => (false, "not_visible", None),
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!("raise-hand: failed to persist state: {}", e),
                    )
                    .await;
            }
        };

        if let Some(communication) = changed_communication {
            crate::session::selection::publish_session_communication(
                app,
                session_id,
                Some(&communication),
            );
        }

        self.write_raise_hand_response(app, path, msg, session_id, raised, status, is_app_outbox)
            .await
    }

    /// #668 - queue a deferred self switch for the session that owns `msg.token`.
    /// It shares the self context pending set with self-clear, so clear and switch
    /// requests cannot stack on the same live session.
    async fn handle_self_handoff_switch<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch requires a valid session token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: no live session owns this token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-switch: internal error resolving session id",
                    )
                    .await;
            }
        };

        if !crate::pty::inject::supports_self_handoff_switch(&session.shell) {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: session shell '{}' is not a supported source \
                        shell (Claude / Codex / Antigravity direct family, Cursor agent, or Pi); \
                        switch is not supported here",
                        session.shell
                    ),
                )
                .await;
        }

        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        let session_cwd = PathBuf::from(&session.working_directory);
        let replica_path = match validate_self_switch_wg_replica(&settings_snapshot, &session_cwd) {
            Ok(path) => path,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        let targets = match resolve_switch_targets(
            &settings_snapshot,
            &replica_path,
            msg.switch_coding_agent.as_deref(),
            msg.switch_profile.as_deref(),
            session.agent_id.as_deref(),
            session.effective_profile.as_deref(),
            session.requested_profile.as_deref(),
        ) {
            Ok(targets) => targets,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };
        if let Err(reason) = validate_self_switch_spawn(&settings_snapshot, &replica_path, &targets)
        {
            return self.reject_message(path, msg, &reason).await;
        }

        let handoff_path = replica_path.join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-switch: SELF-HANDOFF.md not found in your root ({}); \
                        write it before requesting self-handoff-and-switch.",
                        replica_path.display()
                    ),
                )
                .await;
        }

        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        if newly_inserted {
            let forgotten_summary = capture_self_forget_summary(&replica_path);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(&replica_path, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-switch: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {}
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-switch: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                let target_agent = targets.coding_agent.clone();
                let target_profile = targets.profile.clone();
                let replica_path = replica_path.clone();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_switch_after_sustained_idle(
                        &app_clone,
                        session_id,
                        replica_path,
                        target_agent,
                        target_profile,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-switch queued for session {} target coding agent '{}' \
                profile '{}' (from '{}')",
                session_id,
                targets.coding_agent,
                targets.profile,
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-switch already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        self.write_self_switch_response(app, path, msg, session_id, status, &targets, is_app_outbox)
            .await
    }

    /// #1632 - self-handoff-and-restart: hand off through `SELF-HANDOFF.md`, then respawn the
    /// caller's own session on the SAME configured coding agent and profile letter it is already
    /// running, then resume in the NEW session. Scope is every agent: Room replicas, origin Agent
    /// Matrix agents, and the Root Agent.
    ///
    /// Gate order is load-bearing: identity (4) before existence (5) before launchability (6)
    /// before the pending insert (7) and the SELF-FORGET archive (8). The property the contract
    /// rests on is that EVERY rejection happens with nothing queued and nothing archived. Step 6
    /// is NOT read-only (the spawn builder can `create_dir_all` an isolated `CODEX_HOME` and an
    /// absolute `OPENCODE_CONFIG_DIR`), which is parity with the landed switch handler's
    /// `validate_self_switch_spawn` call; that is exactly why the pure `is_file()` existence read
    /// runs first, so the commonest rejection creates nothing.
    ///
    /// NOTE the gate order differs deliberately from `handle_self_handoff_switch`, which is
    /// launchability then existence. Neither handler is aligned to the other: aligning switch
    /// would be an unrequested behavior edit to a landed handler, and aligning restart would
    /// reinstate the defect this order removes. Consequence: for a request that is both missing
    /// its handoff file and unlaunchable, switch reports the launchability failure and restart
    /// reports the missing file.
    async fn handle_self_restart<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        // 1. Token parse.
        let token_uuid = match msg.token.as_deref().and_then(|t| Uuid::parse_str(t).ok()) {
            Some(u) => u,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart requires a valid session token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        // 2. Sole authorization: the session that owns the presented token.
        let session = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            mgr.find_by_token(token_uuid).await
        };
        let session = match session {
            Some(s) => s,
            None => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart: no live session owns this token; restart or \
                        respawn the session",
                    )
                    .await;
            }
        };
        // 3. Session id.
        let session_id = match Uuid::parse_str(&session.id) {
            Ok(u) => u,
            Err(_) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        "self-handoff-and-restart: internal error resolving session id",
                    )
                    .await;
            }
        };

        // 4. D1 identity gate. No coding-agent identity means no recipe to respawn, and passing
        //    None would hand the choice back to `resolve_restart_selected_agent_id`.
        let recipe = match resolve_self_restart_recipe(
            session.agent_id.as_deref(),
            session.effective_profile.as_deref(),
            session.requested_profile.as_deref(),
        ) {
            Ok(recipe) => recipe,
            Err(reason) => return self.reject_message(path, msg, &reason).await,
        };

        // 5. Existence gate on the session's OWN cwd. Restart runs no replica validation, so the
        //    session cwd is the only correct source for a Root Agent or an origin matrix agent.
        let handoff_path =
            std::path::Path::new(&session.working_directory).join(SELF_HANDOFF_ROOT_NAME);
        if !handoff_path.is_file() {
            return self
                .reject_message(
                    path,
                    msg,
                    &format!(
                        "self-handoff-and-restart: SELF-HANDOFF.md not found in your root ({}); \
                        write it before requesting self-handoff-and-restart.",
                        session.working_directory
                    ),
                )
                .await;
        }

        // 6. Launchability pre-flight on the pinned recipe, through the VERY function the restart
        //    transaction calls (`commands/session.rs`), so the pre-flight cannot disagree with the
        //    real spawn; a re-implementation could.
        let settings_snapshot = {
            let settings = app.state::<SettingsState>();
            let snapshot = settings.read().await.clone();
            snapshot
        };
        // #1632: mirror the restart transaction's Root substitution so the pre-flight cannot
        // resolve a different profile cell or placeholder root than the spawn it is predicting.
        // Read-only: `root_agent_dir()` returns exactly the string `ensure_root_agent_dir()`
        // returns, without provisioning anything. On Err, fall back to the raw cwd rather than
        // deny: `is_root_agent_path` itself returns false on that same error, so the only route
        // to the fallback is `session.is_root_agent` alone, and in that case the transaction's own
        // `ensure_root_agent_dir()?` fails pre-teardown on the identical root cause. Denying
        // instead would convert a recoverable pre-teardown failure into a synchronous exit 1 for
        // the Root Agent.
        let is_root_agent = session.is_root_agent
            || crate::config::root_agent::is_root_agent_path(&session.working_directory);
        let preflight_cwd = if is_root_agent {
            crate::config::root_agent::root_agent_dir()
                .unwrap_or_else(|_| session.working_directory.clone())
        } else {
            session.working_directory.clone()
        };
        match crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings_snapshot,
            &recipe.agent,
            &preflight_cwd,
            recipe.profile.as_deref(),
        ) {
            // `Ok(Some(_))` is launchable. `Ok(None)` means the pinned agent id is no longer in
            // `settings.agents`, so the respawn falls back to the session's stored shell and args,
            // byte-identically to what the UI Restart button does today for the same session:
            // proceed, do NOT reject.
            Ok(_) => {}
            // The pinned recipe does not build a launchable command: a profile letter that no
            // longer resolves, or a base command that no longer tokenizes. Reject synchronously so
            // the agent is told, rather than warning 30s later after the CLI already returned 0.
            Err(e) => {
                return self
                    .reject_message(
                        path,
                        msg,
                        &format!(
                            "self-handoff-and-restart: coding agent '{}' profile '{}' is not \
                            launchable from '{}': {}",
                            recipe.agent,
                            recipe.profile.as_deref().unwrap_or("<inherited>"),
                            preflight_cwd,
                            e,
                        ),
                    )
                    .await;
            }
        }

        // 7. Idempotency - atomic check-and-set. The set is SHARED with clear and switch, so the
        //    three operations cannot race on one session.
        let newly_inserted = {
            let pending = app.state::<Arc<crate::PendingSelfClear>>();
            let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
            set.insert(session_id)
        };
        let status = if newly_inserted {
            "queued"
        } else {
            "already_queued"
        };

        if newly_inserted {
            // 8. Archive SELF-FORGET.md so the next cycle starts fresh. Best-effort, log-only on
            //    failure, in ALL cfgs (as `handle_self_clear` does) so the harness can assert it.
            let root = std::path::Path::new(&session.working_directory);
            let forgotten_summary = capture_self_forget_summary(root);
            let ts = chrono::Local::now()
                .format(ARCHIVE_TIMESTAMP_FORMAT)
                .to_string();
            match archive_root_md(root, "SELF-FORGET", &ts) {
                Ok(Some(p)) => log::info!(
                    "[mailbox] self-handoff-and-restart: archived SELF-FORGET.md -> {}",
                    p.display()
                ),
                Ok(None) => {} // no SELF-FORGET.md; nothing to archive
                Err(e) => log::warn!(
                    "[mailbox] self-handoff-and-restart: SELF-FORGET.md archive failed for session \
                    {} (non-fatal): {}",
                    session_id,
                    e
                ),
            }

            // 9. Spawn the deferred two-phase sustained-idle gate (detached; never blocks the
            //    poller). Gated under cfg(not(test)) exactly like the other two handlers.
            #[cfg(not(test))]
            {
                let app_clone = app.clone();
                let restart_agent = recipe.agent.clone();
                let restart_profile = encode_restart_profile_for_seam(recipe.profile.as_deref());
                let root = root.to_path_buf();
                tauri::async_runtime::spawn(async move {
                    Self::run_self_restart_after_sustained_idle(
                        &app_clone,
                        session_id,
                        root,
                        restart_agent,
                        restart_profile,
                        forgotten_summary,
                        SELF_CLEAR_SETTLE,
                        SELF_CLEAR_POLL,
                        SELF_CLEAR_MAX_DEFER,
                    )
                    .await;
                });
            }
            #[cfg(test)]
            let _ = &forgotten_summary;
            log::info!(
                "[mailbox] self-handoff-and-restart queued for session {} coding agent '{}' \
                profile '{}' (from '{}')",
                session_id,
                recipe.agent,
                recipe.profile.as_deref().unwrap_or("<inherited>"),
                msg.from
            );
        } else {
            log::info!(
                "[mailbox] self-handoff-and-restart already pending for session {} (from '{}')",
                session_id,
                msg.from
            );
        }

        // 10.
        self.write_self_restart_response(app, path, msg, session_id, status, &recipe, is_app_outbox)
            .await
    }

    /// #626 - thin timer driver around `self_clear_gate_advance`. Fire-and-forget. Drives both
    /// phases on the stable `session_id`, injecting provider-resolved logical-clear text, then
    /// (#749) archiving `SELF-HANDOFF.md` and injecting the handoff prompt that names the archived
    /// path, and ALWAYS de-registers on exit. No "inject anyway" fallback - a busy or never-idle
    /// session is never cleared (the user-approved "30s sustained idle" semantic).
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_clear_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Uuid,
        clear_text: &'static str,
        root: PathBuf,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
    ) {
        let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            use crate::pty::inject::inject_text_into_session;
            let app = app_for_inject.clone();
            async move { inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_boundary = app.clone();
        let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
            let app = app_for_boundary.clone();
            async move {
                match boundary {
                    SelfClearBoundary::Cleared => {
                        crate::commands::pty::stamp_fresh_boundary_to_session(&app, session_id)
                            .await
                    }
                    SelfClearBoundary::ContentInjected => {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            &app, session_id,
                        )
                        .await
                    }
                };
            }
        };

        Self::drive_self_clear_after_sustained_idle(
            session_id,
            clear_text,
            root,
            pending,
            forgotten_summary,
            settle,
            poll,
            max_defer,
            session_state,
            inject,
            note_boundary,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive_self_clear_after_sustained_idle<
        SessionState,
        SessionFut,
        Inject,
        InjectFut,
        NoteBoundary,
        NoteFut,
    >(
        session_id: Uuid,
        clear_text: &'static str,
        root: PathBuf,
        pending: Arc<crate::PendingSelfClear>,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        mut session_state: SessionState,
        mut inject: Inject,
        mut note_boundary: NoteBoundary,
    ) where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        NoteBoundary: FnMut(Uuid, SelfClearBoundary) -> NoteFut + Send + 'static,
        NoteFut: std::future::Future<Output = ()> + Send,
    {
        let mut state = SelfClearGateState::new(std::time::Instant::now());

        loop {
            tokio::time::sleep(poll).await;

            // Presence + waiting flag under a short-lived lock; never held across .await.
            let (present, waiting) = session_state(session_id).await;

            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next; // adopt the advanced state (the Clear->Handoff reset is already applied)

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s; injecting {} \
                        (phase 1)",
                        session_id,
                        settle.as_secs(),
                        clear_text
                    );
                    // The idle-to-final-Enter race matches the remote logical-clear path.
                    if let Err(e) = inject(session_id, clear_text.to_string()).await {
                        log::warn!(
                            "[mailbox] self-handoff-and-clear: {} injection failed for session {}: \
                            {}",
                            clear_text,
                            session_id,
                            e
                        );
                        break; // abandon the handoff if the clear could not even be sent
                    }
                    // Logical clear reached the PTY; stamp before phase 2
                    // can possibly drop (stamp-then-drop ordering).
                    note_boundary(session_id, SelfClearBoundary::Cleared).await;
                    continue; // state is already Phase 2 with reset clocks
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] self-handoff-and-clear: session {} idle >={}s post-clear; \
                        injecting handoff prompt (phase 2)",
                        session_id,
                        settle.as_secs()
                    );
                    // #749 - archive-first contract; see inject_handoff_prompt_with_archive.
                    let injected = inject_handoff_prompt_with_archive(
                        &root,
                        session_id,
                        SELF_CLEAR_ACTION,
                        build_self_clear_handoff_prompt,
                        forgotten_summary.as_ref(),
                        &mut inject,
                    )
                    .await;
                    if injected {
                        // (#756) handoff prompt is post-boundary content.
                        note_boundary(session_id, SelfClearBoundary::ContentInjected).await;
                    }
                    break;
                }
                SelfClearGateAction::Abandon(reason) => {
                    // Greppable abandon line so a silently-dropped clear/handoff is diagnosable.
                    // The CLI already warned the caller it is best-effort.
                    log::warn!(
                        "[mailbox] self-handoff-and-clear ABANDONED for session {}: {} (agent may \
                        re-issue)",
                        session_id,
                        reason
                    );
                    break;
                }
            }
        }

        // Always de-register (handoff injected / destroy / cap-expiry / clear-inject-fail all land
        // here).
        pending
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&session_id);
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_switch_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
    ) {
        let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        let app_for_persist = app.clone();
        let persist = move |cwd: PathBuf, agent: String, profile: String| {
            let app = app_for_persist.clone();
            async move {
                let settings_snapshot = {
                    let settings = app.state::<SettingsState>();
                    let snapshot = settings.read().await.clone();
                    snapshot
                };
                crate::config::coding_agent_profiles::set_replica_coding_agent_selection(
                    &settings_snapshot,
                    &cwd,
                    &agent,
                    &profile,
                )
            }
        };

        let app_for_restart = app.clone();
        let restart = move |session_id: Uuid, agent: String, profile: String| {
            let app = app_for_restart.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
                let settings = app.state::<SettingsState>();
                // (#747) The self-clear restart is agent-initiated, not user
                // input: a pending raised hand must survive it (the injected
                // handoff continues the same work). Capture before the
                // destroy; restore_communication re-gates kind, visibility,
                // and coordinator, so no pre-filter is needed here.
                let carried_communication = {
                    let mgr = session_mgr.read().await;
                    mgr.get_session(session_id)
                        .await
                        .and_then(|s| s.communication.clone())
                };
                let result = crate::commands::session::restart_session_inner_with_intent(
                    &app,
                    session_mgr.inner(),
                    pty_mgr.inner(),
                    settings.inner(),
                    session_id,
                    Some(agent),
                    Some(profile),
                    Some(true),
                    true,
                    crate::session::selection::TrustedRestartIntent::Background,
                    carried_communication,
                    // §1295 6.6 / 5.1b: the mailbox wake-restart passes Enforce
                    // explicitly, unlike the default-preferring user restart.
                    crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
                )
                .await;
                result.map(|info| info.id)
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            use crate::pty::inject::inject_text_into_session;
            let app = app_for_inject.clone();
            async move { inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_boundary = app.clone();
        let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
            let app = app_for_boundary.clone();
            async move {
                match boundary {
                    SelfClearBoundary::Cleared => {
                        crate::commands::pty::stamp_fresh_boundary_to_session(&app, session_id)
                            .await
                    }
                    SelfClearBoundary::ContentInjected => {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            &app, session_id,
                        )
                        .await
                    }
                };
            }
        };

        Self::drive_self_switch_after_sustained_idle(
            original_session_id,
            cwd,
            target_agent,
            target_profile,
            forgotten_summary,
            SELF_HANDOFF_FLAVOR_SWITCH,
            pending,
            settle,
            poll,
            max_defer,
            session_state,
            persist,
            restart,
            inject,
            note_boundary,
        )
        .await;
    }

    /// #1632 - the restart flavor's production wrapper, modelled on
    /// `run_self_switch_after_sustained_idle` with exactly two differences.
    ///
    /// 1. `persist` is a no-op closure. **No `set_replica_coding_agent_selection` call exists
    ///    anywhere on this path**, so `tooling.currentCodingAgent`, `tooling.profile`,
    ///    `tooling.instanceProfileOverride` and `tooling.instanceProfileOverrideSource` are
    ///    untouched. That is the invariant separating a restart from a switch. (The spawn still
    ///    rewrites `tooling.lastCodingAgent`, `tooling.codingAgents[<id>]` and
    ///    `tooling.profileContentHash`, exactly as on any other restart.)
    /// 2. The restart seam decodes the profile sentinel, so a session that inherited its letter
    ///    crosses as `requested_profile: None` rather than as an invented letter.
    ///
    /// `#[cfg_attr(test, allow(dead_code))]` is required because the runner is only spawned under
    /// `#[cfg(not(test))]`, so under `cfg(test)` it has no caller.
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(test, allow(dead_code))]
    async fn run_self_restart_after_sustained_idle<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
    ) {
        let pending = app.state::<Arc<crate::PendingSelfClear>>().inner().clone();

        let app_for_state = app.clone();
        let session_state = move |session_id: Uuid| {
            let app = app_for_state.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let mgr = session_mgr.read().await;
                let sessions = mgr.list_sessions().await;
                match sessions.iter().find(|s| s.id == session_id.to_string()) {
                    Some(s) => (true, s.waiting_for_input),
                    None => (false, false),
                }
            }
        };

        // #1632: deliberately inert. A restart reproduces the running recipe; it must not write
        // the Selection UI's coding-agent or profile assignment, and it could not serve the Root
        // Agent or an origin matrix agent even if it wanted to
        // (`set_replica_coding_agent_selection` rejects any launch path whose final component does
        // not start with `__agent_`).
        let persist = move |_cwd: PathBuf, _agent: String, _profile: String| async move {
            Ok::<(), String>(())
        };

        let app_for_restart = app.clone();
        let restart = move |session_id: Uuid, agent: String, profile: String| {
            let app = app_for_restart.clone();
            async move {
                let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
                let settings = app.state::<SettingsState>();
                // (#747) An agent-initiated restart must not silently drop a raised hand; the
                // injected handoff continues the same work. Capture before the destroy.
                let carried_communication = {
                    let mgr = session_mgr.read().await;
                    mgr.get_session(session_id)
                        .await
                        .and_then(|s| s.communication.clone())
                };
                let result = crate::commands::session::restart_session_inner_with_intent(
                    &app,
                    session_mgr.inner(),
                    pty_mgr.inner(),
                    settings.inner(),
                    session_id,
                    Some(agent),
                    // #1632: the ONLY difference from the switch closure. The seam's empty-string
                    // sentinel decodes back to None, so an inherited letter reproduces exactly the
                    // resolution the session's own spawn used and no letter is invented.
                    decode_restart_profile_from_seam(&profile),
                    Some(true),
                    true,
                    crate::session::selection::TrustedRestartIntent::Background,
                    carried_communication,
                    crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
                )
                .await;
                result.map(|info| info.id)
            }
        };

        let app_for_inject = app.clone();
        let inject = move |session_id: Uuid, prompt: String| {
            use crate::pty::inject::inject_text_into_session;
            let app = app_for_inject.clone();
            async move { inject_text_into_session(&app, session_id, &prompt).await }
        };

        let app_for_boundary = app.clone();
        let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
            let app = app_for_boundary.clone();
            async move {
                match boundary {
                    SelfClearBoundary::Cleared => {
                        crate::commands::pty::stamp_fresh_boundary_to_session(&app, session_id)
                            .await
                    }
                    SelfClearBoundary::ContentInjected => {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            &app, session_id,
                        )
                        .await
                    }
                };
            }
        };

        Self::drive_self_switch_after_sustained_idle(
            original_session_id,
            cwd,
            target_agent,
            target_profile,
            forgotten_summary,
            SELF_HANDOFF_FLAVOR_RESTART,
            pending,
            settle,
            poll,
            max_defer,
            session_state,
            persist,
            restart,
            inject,
            note_boundary,
        )
        .await;
    }

    /// #1632 - despite the name, this drives BOTH self-handoff flavors: pass
    /// `SELF_HANDOFF_FLAVOR_SWITCH` for self-handoff-and-switch and
    /// `SELF_HANDOFF_FLAVOR_RESTART` for self-handoff-and-restart. The two differ only in the four
    /// `SelfHandoffFlavor` fields (three `&'static str` plus one prompt-builder `fn` pointer); the
    /// two-phase gate, the pending-set alias bookkeeping, and the archive-first phase-2 tail are
    /// shared and must not fork. The name is kept so a rename does not churn fifteen call sites
    /// (two production wrappers plus thirteen tests) for zero behavioral value.
    #[allow(clippy::too_many_arguments)]
    async fn drive_self_switch_after_sustained_idle<
        SessionState,
        SessionFut,
        Persist,
        PersistFut,
        Restart,
        RestartFut,
        Inject,
        InjectFut,
        NoteBoundary,
        NoteFut,
    >(
        original_session_id: Uuid,
        cwd: PathBuf,
        target_agent: String,
        target_profile: String,
        forgotten_summary: Option<ForgottenSummary>,
        flavor: SelfHandoffFlavor,
        pending: Arc<crate::PendingSelfClear>,
        settle: std::time::Duration,
        poll: std::time::Duration,
        max_defer: std::time::Duration,
        mut session_state: SessionState,
        mut persist: Persist,
        mut restart: Restart,
        mut inject: Inject,
        mut note_boundary: NoteBoundary,
    ) where
        SessionState: FnMut(Uuid) -> SessionFut + Send + 'static,
        SessionFut: std::future::Future<Output = (bool, bool)> + Send,
        Persist: FnMut(PathBuf, String, String) -> PersistFut + Send + 'static,
        PersistFut: std::future::Future<Output = Result<(), String>> + Send,
        Restart: FnMut(Uuid, String, String) -> RestartFut + Send + 'static,
        RestartFut: std::future::Future<Output = Result<String, String>> + Send,
        Inject: FnMut(Uuid, String) -> InjectFut + Send + 'static,
        InjectFut: std::future::Future<Output = Result<(), String>> + Send,
        NoteBoundary: FnMut(Uuid, SelfClearBoundary) -> NoteFut + Send + 'static,
        NoteFut: std::future::Future<Output = ()> + Send,
    {
        let mut state = SelfClearGateState::new(std::time::Instant::now());
        let mut session_id = original_session_id;
        let mut new_alias_id: Option<Uuid> = None;

        loop {
            tokio::time::sleep(poll).await;
            let (present, waiting) = session_state(session_id).await;
            let (next, action) = self_clear_gate_advance(
                state,
                present,
                waiting,
                std::time::Instant::now(),
                settle,
                max_defer,
            );
            state = next;

            match action {
                SelfClearGateAction::Wait => continue,
                SelfClearGateAction::InjectClear => {
                    log::info!(
                        "[mailbox] {}: session {} idle >={}s; {} with coding agent '{}' profile \
                        '{}' (phase 1)",
                        flavor.action,
                        session_id,
                        settle.as_secs(),
                        flavor.phase1_verb,
                        target_agent,
                        display_profile_for_log(&target_profile)
                    );
                    // #1940 - acquire the shared selection operation turn only here:
                    // the sustained-idle wait above stays outside it. The guard is
                    // held across the persist, the adjacent restart await and the
                    // pending-alias bookkeeping, and is released when this arm ends,
                    // before the phase-2 settling and handoff injection.
                    let _turn =
                        match crate::session::selection::acquire_selection_operation_turn().await {
                            Ok(turn) => turn,
                            Err(error) => {
                                log::warn!(
                                "[mailbox] {}: selection operation turn unavailable for session \
                                {}: {}",
                                flavor.action,
                                session_id,
                                error
                            );
                                break;
                            }
                        };
                    if let Err(e) =
                        persist(cwd.clone(), target_agent.clone(), target_profile.clone()).await
                    {
                        log::warn!(
                            "[mailbox] {}: persist failed for session {}: {}",
                            flavor.action,
                            original_session_id,
                            e
                        );
                        break;
                    }
                    let restarted =
                        restart(session_id, target_agent.clone(), target_profile.clone()).await;
                    let new_id = match restarted {
                        Ok(id) => match Uuid::parse_str(&id) {
                            Ok(uuid) => uuid,
                            Err(e) => {
                                log::warn!(
                                    "[mailbox] {}: restarted session id '{}' could not be parsed: \
                                    {}",
                                    flavor.action,
                                    id,
                                    e
                                );
                                break;
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "[mailbox] {}: restart failed for session {}: {}",
                                flavor.action,
                                original_session_id,
                                e
                            );
                            break;
                        }
                    };
                    {
                        let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
                        if set.insert(new_id) {
                            new_alias_id = Some(new_id);
                        } else {
                            log::warn!(
                                "[mailbox] {}: new session {} was already marked pending",
                                flavor.action,
                                new_id
                            );
                        }
                    }
                    session_id = new_id;
                    continue;
                }
                SelfClearGateAction::InjectHandoff => {
                    log::info!(
                        "[mailbox] {}: session {} idle >={}s {}; injecting handoff prompt (phase \
                        2)",
                        flavor.action,
                        session_id,
                        settle.as_secs(),
                        flavor.phase2_verb
                    );
                    // #749 - archive-first contract; see inject_handoff_prompt_with_archive.
                    // (#756) NOTE: the switch phase 1 fires NO Cleared event; it is a
                    // restart through restart_session_inner_with_activation (skip
                    // Some(true)), which stamps record + mirror itself via C3. The
                    // handoff lands on the CURRENT session_id (the post-switch id).
                    let injected = inject_handoff_prompt_with_archive(
                        &cwd,
                        session_id,
                        flavor.action,
                        flavor.build_prompt,
                        forgotten_summary.as_ref(),
                        &mut inject,
                    )
                    .await;
                    if injected {
                        // (#756) switch handoff prompt is post-boundary content.
                        note_boundary(session_id, SelfClearBoundary::ContentInjected).await;
                    }
                    break;
                }
                SelfClearGateAction::Abandon(reason) => {
                    log::warn!(
                        "[mailbox] {} ABANDONED for original session {} current session {}: {} \
                        (agent may re-issue)",
                        flavor.action,
                        original_session_id,
                        session_id,
                        reason
                    );
                    break;
                }
            }
        }

        let mut set = pending.0.lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&original_session_id);
        if let Some(new_id) = new_alias_id {
            set.remove(&new_id);
        }
    }

    /// #617 - write the self-clear queue-ack response, then move the message to
    /// delivered/. Mirrors the close-session dual-write (self-contained copy; the
    /// landed close-session block is intentionally left untouched, blast radius 0).
    async fn write_self_clear_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_CLEAR_ACTION,
                "status": status,                 // "queued" | "already_queued"
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,   // single-sourced const
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                // (1) outbox-relative <ac_dir>/responses/<rid>.json - skip for app-outbox.
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                // (2) resolved-sender path (best-effort).
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_self_switch_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        targets: &SelfSwitchTargets,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_SWITCH_ACTION,
                "status": status,
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,
                "requested_by": msg.from,
                "target_coding_agent": targets.coding_agent,
                "target_profile": targets.profile,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    /// #1632 - write the self-handoff-and-restart queue-ack response, then move the message to
    /// delivered/. `profile` serializes as JSON `null` when the session inherited its letter: the
    /// recipe pins nothing there, and reporting an invented `"A"` would be a lie about the pin.
    #[allow(clippy::too_many_arguments)]
    async fn write_self_restart_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        status: &str,
        recipe: &SelfRestartRecipe,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": SELF_RESTART_ACTION,
                "status": status,
                "session_id": session_id.to_string(),
                "settle_secs": SELF_CLEAR_SETTLE_SECS,
                "requested_by": msg.from,
                "coding_agent": recipe.agent,
                "profile": recipe.profile,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn write_raise_hand_response<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &std::path::Path,
        msg: &OutboxMessage,
        session_id: Uuid,
        raised: bool,
        status: &str,
        is_app_outbox: bool,
    ) -> Result<(), String> {
        if let Some(ref rid) = msg.request_id {
            let response = serde_json::json!({
                "action": RAISE_HAND_ACTION,
                "status": status,
                "raised": raised,
                "session_id": session_id.to_string(),
                "requested_by": msg.from,
            });
            if let Ok(json) = serde_json::to_string_pretty(&response) {
                if !is_app_outbox {
                    if let Some(responses_dir) = path
                        .parent()
                        .and_then(|p| p.parent())
                        .map(|ac| ac.join("responses"))
                    {
                        let _ = std::fs::create_dir_all(&responses_dir);
                        let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                    }
                }
                if let Some(sender_path) = self.resolve_repo_path(&msg.from, app).await {
                    let responses_dir = std::path::PathBuf::from(sender_path)
                        .join(crate::config::agent_local_dir_name())
                        .join("responses");
                    let _ = std::fs::create_dir_all(&responses_dir);
                    let _ = std::fs::write(responses_dir.join(format!("{}.json", rid)), &json);
                }
            }
        }
        self.move_to_delivered(app, path, msg).await
    }

    /// Force-close a session immediately via destroy_session_inner.
    async fn force_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        sid: Uuid,
    ) -> bool {
        #[cfg(test)]
        {
            // (#885 E-2) Emit a Destroy event so purge tests can assert that
            // the destroy loop was or was not reached. Without this, the
            // "zero Destroy events" assertions in the F-7 and root-guard
            // tests are tautologies: the only emit site was in the wake
            // deferred-destroy path, which the purge never reaches.
            if let Some(hooks) = &self.test_hooks {
                {
                    let mut calls = hooks.destroy_calls.lock().unwrap();
                    calls.push(sid);
                }
                {
                    let mut events = hooks.events.lock().unwrap();
                    events.push(MailboxTestEvent::Destroy(sid));
                }
            }
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            return mgr.destroy_session(sid).await.is_ok();
        }

        #[cfg(not(test))]
        {
            match crate::commands::session::background_destroy_session_inner(app, sid).await {
                Ok(()) => {
                    log::info!("[mailbox] close-session: force-destroyed session {}", sid);
                    true
                }
                Err(e) => {
                    log::warn!(
                        "[mailbox] close-session: failed to force-destroy session {}: {}",
                        sid,
                        e
                    );
                    false
                }
            }
        }
    }

    /// Gracefully close a session: inject exit command, poll for Exited, fallback to force on
    /// timeout.
    async fn graceful_close_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        sid: Uuid,
        timeout_secs: u32,
    ) -> bool {
        // Get session info to determine agent type
        let exit_cmd = {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) => Self::resolve_exit_command(&s.shell, &s.shell_args),
                None => {
                    log::warn!(
                        "[mailbox] close-session: session {} not found for graceful close",
                        sid
                    );
                    return false;
                }
            }
        };

        log::info!(
            "[mailbox] close-session: injecting '{}' into session {}",
            exit_cmd.escape_debug(),
            sid
        );

        // Graceful exit shares the same per-route input serialization as user,
        // automated, and privileged writers.
        let pty_arc = app
            .state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()
            .inner()
            .clone();
        let inject_result =
            match crate::pty::manager::PtyManager::acquire_input_writer(&pty_arc, sid).await {
                Ok(permit) => {
                    let result = crate::pty::manager::PtyManager::write_with_permit(
                        &permit,
                        exit_cmd.as_bytes(),
                    )
                    .map_err(|error| error.to_string());
                    if result.is_ok() {
                        crate::commands::pty::mark_successful_pty_write_busy(
                            app,
                            sid,
                            exit_cmd.len(),
                        )
                        .await;
                    }
                    result
                }
                Err(error) => Err(error.to_string()),
            };
        if let Err(e) = inject_result {
            log::warn!(
                "[mailbox] close-session: PTY inject failed for {}: {}, falling back to force",
                sid,
                e
            );
            return self.force_close_session(app, sid).await;
        }

        // Poll for SessionStatus::Exited
        let timeout = std::time::Duration::from_secs(timeout_secs as u64);
        let poll_interval = std::time::Duration::from_secs(1);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= timeout {
                log::warn!(
                    "[mailbox] close-session: graceful timeout ({}s) for session {}, falling back \
                    to force",
                    timeout_secs,
                    sid
                );
                return self.force_close_session(app, sid).await;
            }

            tokio::time::sleep(poll_interval).await;

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            match sessions.iter().find(|s| s.id == sid.to_string()) {
                Some(s) if matches!(s.status, SessionStatus::Exited(_)) => {
                    log::info!("[mailbox] close-session: session {} exited gracefully", sid);
                    drop(mgr);
                    // Clean up the exited session
                    return self.force_close_session(app, sid).await;
                }
                None => {
                    // Session already removed
                    log::info!("[mailbox] close-session: session {} already gone", sid);
                    return true;
                }
                _ => {} // still running, keep polling
            }
        }
    }

    /// Determine the exit command to inject based on the session's shell/agent type.
    /// Claude Code -> "/exit\r", generic shell/codex -> "exit\r"
    fn resolve_exit_command(shell: &str, shell_args: &[String]) -> String {
        let full_cmd = format!("{} {}", shell, shell_args.join(" "));
        let basenames: Vec<String> = full_cmd
            .split_whitespace()
            .map(crate::commands::session::executable_basename)
            .collect();

        if basenames.iter().any(|b| b == "claude" || b == "aider") {
            "/exit\r".to_string()
        } else {
            // Codex, generic shell, and other CLIs
            "exit\r".to_string()
        }
    }

    /// Resolve the full filesystem path for an agent name.
    ///
    /// §AR2-G4: collector pattern. For qualified inputs, an FQN matches at most
    /// one CWD/path/team-member entry per iteration (by construction) — the
    /// dedupe is defense-in-depth for redundant registrations. For unqualified
    /// inputs (legacy), local-part matches across multiple projects return
    /// `None` rather than arbitrarily picking one.
    ///
    /// §AR2-G3: WG fallback seed honors the target project filter. Combined
    /// with §DR2-4 composition: `matches.push(...); break;` within a single
    /// `rp` iteration (FQN can only match one replica dir per project) while
    /// the outer loop continues so cross-project ambiguity is still detected.
    async fn resolve_repo_path<R: tauri::Runtime>(
        &self,
        agent_name: &str,
        app: &tauri::AppHandle<R>,
    ) -> Option<String> {
        let (target_project, target_local) = crate::config::teams::split_project_prefix(agent_name);
        let is_qualified = target_project.is_some();
        let mut matches: Vec<String> = Vec::new();

        let record_match = |path_str: &str, out: &mut Vec<String>| {
            if !out.iter().any(|m| m == path_str) {
                out.push(path_str.to_string());
            }
        };

        let hits_agent = |cwd: &str| -> bool {
            let path_fqn = crate::config::teams::agent_fqn_from_path(cwd);
            if is_qualified {
                path_fqn == agent_name
            } else {
                let (_, path_local) = crate::config::teams::split_project_prefix(&path_fqn);
                path_local == target_local
            }
        };

        let settings = app.state::<SettingsState>();
        let (project_paths, archived) = {
            let cfg = settings.read().await;
            (
                cfg.project_paths.clone(),
                cfg.archived_project_paths.clone(),
            )
        };

        // Loop 1: session CWDs
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);
        let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
        let dirs = retain_unarchived_session_dirs(dirs, &roots);
        for (_, cwd) in &dirs {
            if hits_agent(cwd) {
                record_match(cwd, &mut matches);
            }
        }

        // Loop 2: settings project_paths
        for rp in &project_paths {
            if hits_agent(rp) {
                record_match(rp, &mut matches);
            }
        }

        // Loop 3: discovered team member paths. Short-circuit by project when
        // the target is qualified (team.project matches target_project).
        let discovered_teams = teams::discover_teams();
        for team in &discovered_teams {
            if let Some(want) = target_project {
                if team.project != want {
                    continue;
                }
            }
            for agent_path in team.agent_paths.iter().flatten() {
                let path_str = agent_path.to_string_lossy().to_string();
                if hits_agent(&path_str) {
                    record_match(&path_str, &mut matches);
                }
            }
        }

        // Loop 4: WG replica fallback. Scan `<workspace>/<wg>/__agent_<short>` under
        // project_paths (base + immediate non-dot children), honoring the target
        // project filter. §DR2-4 composition: push + break within a single `rp`
        // (an FQN matches at most one replica dir per project) but continue the
        // outer loop so ambiguity across projects is detected.
        if crate::config::entity_prefix::has_entity_prefix(target_local) {
            if let Some((wg_name, agent_short)) = target_local.split_once('/') {
                let replica_dir = format!("__agent_{}", agent_short);

                let project_matches = |dir_name: &str| -> bool {
                    match target_project {
                        Some(want) => dir_name == want,
                        None => true,
                    }
                };

                for rp in &project_paths {
                    let base = std::path::Path::new(rp);
                    if !base.is_dir() {
                        continue;
                    }
                    let base_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");

                    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                    if project_matches(base_name) {
                        dirs_to_check.push(base.to_path_buf());
                    }
                    if let Ok(entries) = std::fs::read_dir(base) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if !p.is_dir() {
                                continue;
                            }
                            let dir_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with('.') {
                                continue;
                            }
                            if !project_matches(dir_name) {
                                continue;
                            }
                            dirs_to_check.push(p);
                        }
                    }

                    for dir in dirs_to_check {
                        let Some(ac_root) = crate::config::ac_root::existing_ac_root(&dir) else {
                            continue;
                        };
                        let candidate = ac_root.join(wg_name).join(&replica_dir);
                        if candidate.is_dir() {
                            record_match(&candidate.to_string_lossy(), &mut matches);
                            // Within a single `rp`, first hit is the unique hit —
                            // an FQN matches one replica dir per project. Continue
                            // OUTER loop to detect cross-project ambiguity (§DR2-4).
                            break;
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => None,
            1 => Some(matches.pop().unwrap()),
            _ => {
                log::warn!(
                    "[mailbox] resolve_repo_path('{}'): {} candidates, refusing arbitrary pick: \
                    {:?}",
                    agent_name,
                    matches.len(),
                    matches
                );
                None
            }
        }
    }

    // Shadow `agent_name_from_path` removed — all mailbox call sites now use
    // `crate::config::teams::agent_fqn_from_path` per §AR2 (§DR2 consolidation).

    /// Check if sender can reach destination via team membership.
    /// Only agents in the same team can communicate — no parent directory fallback.
    fn can_reach(&self, from: &str, to: &str, discovered_teams: &[teams::DiscoveredTeam]) -> bool {
        crate::config::teams::can_communicate(from, to, discovered_teams)
    }

    /// Resolve a wake fallback completely without preparing directories or
    /// mutating recipient lifecycle state. The returned command is carried
    /// unchanged through capability preflight and the later spawn.
    async fn resolve_wake_spawn_plan<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<ResolvedWakeSpawnPlan, String> {
        let resolved_command = self.resolve_agent_command(app, msg).await?;
        let resolved_command = resolved_command.ok_or_else(|| {
            format!(
                "No agent command resolved for '{}'; preferredAgent={:?}. Configure \
                lastCodingAgent or agents in settings.",
                msg.to, msg.preferred_agent
            )
        })?;

        let cwd = match self.resolve_repo_path(&msg.to, app).await {
            Some(path) => path,
            None => self
                .resolve_wg_path_from_sessions(app, &msg.to)
                .await
                .ok_or_else(|| {
                    format!(
                        "Cannot resolve repo path for '{}' - cannot spawn session",
                        msg.to
                    )
                })?,
        };
        let session_name = {
            let (_, local) = crate::config::teams::split_project_prefix(&msg.to);
            local.to_string()
        };
        let (configured_spawn, resolved_agent_host_shell) =
            if let Some(agent_id) = resolved_command.agent_id.as_deref() {
                let settings = app.state::<SettingsState>();
                let cfg = settings.read().await;
                let spawn = crate::commands::session::resolve_configured_agent_spawn_for_cwd(
                    &cfg,
                    agent_id,
                    &cwd,
                    msg.requested_profile.as_deref(),
                    // #1635-2 D2: the dispatch request outranks the replica
                    // pin for this spawn only.
                    msg.requested_profile.is_some(),
                )?;
                // #1271 - same-guard host shell: copied from the exact snapshot
                // that built the spawn, before any await.
                let host_shell = if spawn.is_some() {
                    Some(crate::pty::backend::ResolvedAgentHostShell {
                        program: cfg.default_shell.clone(),
                        args: cfg.default_shell_args.clone(),
                    })
                } else {
                    None
                };
                (spawn, host_shell)
            } else {
                (None, None)
            };
        let (spawn_shell, spawn_args, spawn_label) = if let Some(spawn) = configured_spawn.as_ref()
        {
            (
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                Some(spawn.trusted_agent_label.clone()),
            )
        } else {
            (
                resolved_command.shell.clone(),
                resolved_command.shell_args.clone(),
                resolved_command.agent_label.clone(),
            )
        };

        Ok(ResolvedWakeSpawnPlan {
            resolved_command,
            cwd,
            session_name,
            spawn_shell,
            spawn_args,
            spawn_label,
            configured_spawn,
            resolved_agent_host_shell,
        })
    }

    /// Resolve which agent CLI to spawn when `deliver_wake` needs a new
    /// persistent session for the destination agent.
    async fn resolve_agent_command<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        msg: &OutboxMessage,
    ) -> Result<Option<ResolvedWakeAgentCommand>, String> {
        let agents = {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            // #2716 (B3, option C) row 7: an unreadable agents layer is refused and
            // reported, never resolved against a list that is not the user's.
            if let Some(path) = cfg.agents_layer.unreadable_path() {
                return Err(crate::config::settings::agents_unreadable_session_error(
                    path,
                ));
            }
            cfg.agents.clone()
        };

        let mut destination_current_agent: Option<String> = None;
        let mut destination_last_agent: Option<String> = None;
        let mut destination_config_path: Option<PathBuf> = None;

        if let Some(dest_path) = self.resolve_repo_path(&msg.to, app).await {
            let dest = Path::new(&dest_path);
            // The Selection UI assignment (currentCodingAgent), including the
            // "Entire Workgroup" path, is written to the replica's top-level
            // config.json by set_replica_coding_agent_selection, not to the
            // per-instance agent_local_dir config that holds lastCodingAgent.
            destination_current_agent =
                crate::config::coding_agent_profiles::read_replica_current_coding_agent(dest);

            let config_dir = dest.join(crate::config::agent_local_dir_name());
            destination_config_path = Some(config_dir.join("config.json"));

            if let Some(local_config) =
                crate::config::agent_config::read_agent_local_config(&config_dir)
            {
                destination_last_agent = local_config.tooling.last_coding_agent;
            }
        }

        resolve_wake_agent_command_from_sources(
            &agents,
            &msg.preferred_agent,
            destination_current_agent.as_deref(),
            destination_last_agent.as_deref(),
            destination_config_path.as_deref(),
            msg.sender_agent.as_deref(),
        )
    }

    /// Fallback path resolution for WG agents: find a sibling session in the same WG,
    /// derive the WG directory from its CWD, and construct the target agent path.
    ///
    /// §4.4: peel optional `<project>:` prefix before splitting the local part.
    /// If the target is qualified, the returned candidate must also be in the
    /// same project (checked via derived FQN).
    async fn resolve_wg_path_from_sessions<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        agent_name: &str,
    ) -> Option<String> {
        let archived = {
            let settings = app.state::<SettingsState>();
            let cfg = settings.read().await;
            cfg.archived_project_paths.clone()
        };
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let dirs = mgr.get_sessions_working_dirs().await;
        drop(mgr);
        let roots = crate::config::sessions_persistence::normalize_project_roots(&archived);
        let dirs = retain_unarchived_session_dirs(dirs, &roots);

        resolve_wg_path_from_session_dirs(&dirs, agent_name)
    }

    /// Move an outbox message to outbox/delivered/ with token stripped.
    async fn move_to_delivered<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        path: &Path,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        let delivered_dir = path.parent().ok_or("No parent dir")?.join("delivered");
        std::fs::create_dir_all(&delivered_dir)
            .map_err(|e| format!("Failed to create delivered dir: {}", e))?;

        // Strip token before storing
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = delivered_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json)
            .map_err(|e| format!("Failed to write delivered file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;
        self.complete_response_delivery(app, path).await;

        log::info!("[mailbox] Message {} moved to delivered/", msg.id);
        Ok(())
    }

    /// Reject a message: move to outbox/rejected/ with reason, and notify the sender.
    async fn reject_message(
        &self,
        path: &Path,
        msg: &OutboxMessage,
        reason: &str,
    ) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        // Write reason file FIRST — the CLI polls for this file to detect rejection
        let reason_path = rejected_dir.join(format!("{}.reason.txt", msg.id));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        // Then write the stripped message JSON
        let mut stripped = msg.clone();
        stripped.token = None;

        let dest = rejected_dir.join(format!("{}.json", msg.id));
        let json = serde_json::to_string_pretty(&stripped)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&dest, json).map_err(|e| format!("Failed to write rejected file: {}", e))?;

        // Remove original
        std::fs::remove_file(path).map_err(|e| format!("Failed to remove outbox file: {}", e))?;
        self.settle_response_source(path, false);

        log::warn!(
            "[mailbox] Message {} moved to rejected/: {}",
            msg.id,
            reason
        );
        Ok(())
    }

    /// Reject a raw file that cannot be parsed as OutboxMessage.
    fn reject_raw_file(path: &Path, reason: &str) -> Result<(), String> {
        let rejected_dir = path.parent().ok_or("No parent dir")?.join("rejected");
        std::fs::create_dir_all(&rejected_dir)
            .map_err(|e| format!("Failed to create rejected dir: {}", e))?;

        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.json");

        let dest = rejected_dir.join(filename);
        std::fs::rename(path, &dest)
            .or_else(|_| std::fs::copy(path, &dest).and_then(|_| std::fs::remove_file(path)))
            .map_err(|e| format!("Failed to move file to rejected: {}", e))?;

        let stem = Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let reason_path = rejected_dir.join(format!("{}.reason.txt", stem));
        std::fs::write(&reason_path, reason)
            .map_err(|_| "Failed to write reason file".to_string())?;

        log::warn!("Rejected raw file {:?}: {}", path, reason);
        Ok(())
    }

    /// Poll ~/.agentscommander/project-refresh-requests/ for sidebar refresh requests.
    async fn poll_project_refresh_requests<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join(PROJECT_REFRESH_REQUESTS_DIR_NAME);
        let batch = collect_project_refresh_requests(&requests_dir);

        for payload in &batch.payloads {
            log::info!(
                "[project-refresh-requests] Emitting refresh: dir='{}' project='{}' reason='{}'",
                requests_dir.display(),
                payload.project_path,
                payload.reason
            );
            let _ = tauri::Emitter::emit(app, "ac_project_refresh_requested", payload);
        }

        for path in batch.processed_paths {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Poll ~/.agentscommander/session-requests/ for launch requests from the CLI.
    async fn poll_session_requests<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) {
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join(SESSION_REQUESTS_DIR_NAME);
        if !requests_dir.is_dir() {
            return;
        }

        // #1163: sweep stale result sidecars. The CLI consumes the result it
        // reads and waits at most `SESSION_REQUEST_RESULT_TIMEOUT` (30 s); a
        // result older than 5 minutes is litter from a timed-out or crashed
        // CLI and is removed here (worst case a few KB in an already-ephemeral,
        // ignored instance-artifact directory).
        if let Ok(rd) = std::fs::read_dir(&requests_dir) {
            for entry in rd.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !crate::cli::create_agent::is_session_request_result_path(&path) {
                    continue;
                }
                let stale = std::fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age >= std::time::Duration::from_secs(5 * 60));
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        let entries: Vec<PathBuf> = match std::fs::read_dir(&requests_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                // #1163: result sidecars (`<id>.result.json`) are ANSWERS, not
                // requests; without this exclusion a leftover result file would
                // be parsed as a `SessionRequest` (parse fails -> spurious
                // rejected result written for it).
                .filter(|p| !crate::cli::create_agent::is_session_request_result_path(p))
                .collect(),
            Err(_) => return,
        };

        // #1163: answer each request with a result sidecar on every terminal
        // path (malformed, spawn-rebuild failure, create Ok/Err). Write the
        // result BEFORE deleting the request: a crash in between leaves a
        // re-processable request on disk (the pre-existing hazard), never a
        // lost already-written outcome.
        let write_result = |result: crate::cli::create_agent::SessionRequestResult| {
            if let Err(e) = crate::cli::create_agent::write_session_request_result(&result) {
                log::warn!(
                    "[session-requests] Failed to write result for '{}': {}",
                    result.id,
                    e
                );
            }
        };

        for path in entries {
            let content = match read_text_bom_tolerant(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("[session-requests] Failed to read {:?}: {}", path, e);
                    continue;
                }
            };

            let request: crate::cli::create_agent::SessionRequest =
                match serde_json::from_str(&content) {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!("[session-requests] Failed to parse {:?}: {}", path, e);
                        // #1163: answer with a rejected result so the CLI can
                        // distinguish rejection from a missing app. The request
                        // filename IS `<id>.json`, so the id comes from the stem.
                        write_result(crate::cli::create_agent::SessionRequestResult {
                            id: path
                                .file_stem()
                                .and_then(|value| value.to_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
                            session_id: None,
                            error: Some(format!("malformed session request: {e}")),
                        });
                        // Delete malformed file
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                };

            log::info!(
                "[session-requests] Processing: name='{}' cwd='{}' agent='{}'",
                request.session_name,
                request.cwd,
                request.agent_id
            );

            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
            let cwd = crate::path_utils::normalize_windows_verbatim_path(&request.cwd);
            let (resolved_spawn, resolved_agent_host_shell) = {
                let settings = app.state::<SettingsState>();
                let cfg = settings.read().await;
                match crate::commands::session::build_configured_agent_spawn_for_cwd(
                    &cfg,
                    &request.agent_id,
                    &cwd,
                    request.requested_profile.as_deref(),
                ) {
                    Ok(Some(spawn)) => (
                        Some(spawn),
                        // #1271 - same-guard host shell: copied from the same
                        // config snapshot that built the spawn, before any await.
                        Some(crate::pty::backend::ResolvedAgentHostShell {
                            program: cfg.default_shell.clone(),
                            args: cfg.default_shell_args.clone(),
                        }),
                    ),
                    Ok(None) => (None, None),
                    Err(e) => {
                        log::error!(
                            "[session-requests] Failed to rebuild configured agent command for \
                            '{}': {}",
                            request.session_name,
                            e
                        );
                        write_result(crate::cli::create_agent::SessionRequestResult {
                            id: request.id.clone(),
                            status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
                            session_id: None,
                            error: Some(e.to_string()),
                        });
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                }
            };
            let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                (request.shell.clone(), request.shell_args.clone(), None)
            };

            let create_result = crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                shell,
                shell_args,
                cwd,
                Some(request.session_name.clone()),
                Some(request.agent_id.clone()),
                agent_label, // No agent label for legacy custom-shell fallback
                false,       // Persist tooling
                Vec::new(),  // git_repos
                true,        // skip_auto_resume = true → CLI session-request is a fresh create
                resolved_spawn,
                resolved_agent_host_shell,
                // #973 - headless caller: no terminal to measure, keep 120x30.
                None,
                crate::commands::session::CreateSelectionIntent::Background,
            )
            .await;
            match &create_result {
                Ok(info) => {
                    log::info!(
                        "[session-requests] Created session '{}' (id={})",
                        request.session_name,
                        info.id
                    );
                }
                Err(e) => {
                    log::error!(
                        "[session-requests] Failed to create session '{}': {}",
                        request.session_name,
                        e
                    );
                }
            }

            // #1163: answer the request with the terminal outcome, then delete.
            let result = match create_result {
                Ok(info) => crate::cli::create_agent::SessionRequestResult {
                    id: request.id.clone(),
                    status: crate::cli::create_agent::SessionRequestResultStatus::Created,
                    session_id: Some(info.id),
                    error: None,
                },
                Err(e) => crate::cli::create_agent::SessionRequestResult {
                    id: request.id.clone(),
                    status: crate::cli::create_agent::SessionRequestResultStatus::Rejected,
                    session_id: None,
                    error: Some(e.to_string()),
                },
            };
            write_result(result);

            // Delete processed request file regardless of success/failure
            let _ = std::fs::remove_file(&path);
        }
    }

    /// #786: poll `<config_dir>/coding-agent-requests/` for CLI mutation requests
    /// while the GUI runs. Scans bare `.json` files only (so `.processing`,
    /// `.tmp`, and the `results/` subdir are excluded). For each request, R8:
    /// write-lock SettingsState, clone a candidate, run the pure per-request
    /// handler (claim -> parse -> expiry -> apply -> write result -> delete
    /// `.processing`) against it, and on Applied swap the in-memory state and emit
    /// `coding_agent_settings_updated` AFTER the guard drops. `save_settings` is
    /// synchronous, so the write guard is never held across an await.
    async fn poll_coding_agent_requests<R: tauri::Runtime>(&self, app: &tauri::AppHandle<R>) {
        use crate::config::coding_agent_mutations as ca;
        let config_dir = match crate::config::config_dir() {
            Some(d) => d,
            None => return,
        };
        let requests_dir = config_dir.join(ca::CODING_AGENT_REQUESTS_DIR);
        if !requests_dir.is_dir() {
            return;
        }
        let results_dir = requests_dir.join(ca::RESULTS_SUBDIR);

        let entries: Vec<PathBuf> = match std::fs::read_dir(&requests_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect(),
            Err(_) => return,
        };
        if entries.is_empty() {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let state = app.state::<SettingsState>();
        for path in entries {
            let applied = {
                let mut s = state.write().await;
                let mut candidate = s.clone();
                let mut written_settings = None;
                let disposition = {
                    let mut save = |st: &crate::config::settings::AppSettings| {
                        let written = crate::config::settings::save_settings(st)?;
                        written_settings = Some(written.clone());
                        Ok(written)
                    };
                    ca::process_coding_agent_request(
                        &path,
                        &results_dir,
                        now_ms,
                        &mut candidate,
                        &mut save,
                    )
                };
                if let ca::RequestDisposition::Applied { op, agent_id } = disposition {
                    debug_assert!(
                        written_settings.is_some(),
                        "Applied implies save() succeeded"
                    );
                    *s = written_settings.unwrap_or_else(|| {
                        log::error!("coding-agent op Applied without a save; publishing candidate");
                        candidate
                    });
                    Some((op, agent_id))
                } else {
                    None
                }
            }; // write guard dropped here, before the emit

            if let Some((op, agent_id)) = applied {
                let _ = tauri::Emitter::emit(
                    app,
                    "coding_agent_settings_updated",
                    serde_json::json!({ "op": op, "agentId": agent_id }),
                );
                log::info!(
                    "[coding-agent-requests] applied op={} agentId={:?}",
                    op,
                    agent_id
                );
            }
        }
    }
}

/// Read a file as UTF-8 string, tolerant of UTF-8 / UTF-16 LE / UTF-16 BE BOMs.
/// Logs a warning when a BOM is detected so users see which tool is writing
/// odd encoding into outbox / session-requests (typically PowerShell on Windows).
fn read_text_bom_tolerant(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;

    if bytes.starts_with(&[0xFF, 0xFE]) {
        log::warn!(
            "[bom] UTF-16 LE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 \
            without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        log::warn!(
            "[bom] UTF-16 BE BOM detected in {:?} — decoding to UTF-8 (writer should use UTF-8 \
            without BOM)",
            path
        );
        let u16_data: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&u16_data))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        log::warn!(
            "[bom] UTF-8 BOM detected in {:?} — stripping (writer should use UTF-8 without BOM)",
            path
        );
        String::from_utf8(bytes[3..].to_vec())
            .map_err(|e| format!("Invalid UTF-8 after BOM: {}", e))
    } else {
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {}", e))
    }
}

/// #2786 (C1) - the agent state file beside `config.json` in `target_root`,
/// read through the same guard as the PTY-input `config.json` read: a regular
/// non-symlink file, at most 1 MiB, no duplicate keys. Only a missing file is
/// absence; every other failure is an error the caller propagates.
fn read_guarded_agent_state(
    target_root: &Path,
) -> Result<Option<serde_json::Value>, crate::phone::types::PtyInputReasonCode> {
    use crate::phone::types::PtyInputReasonCode as C;
    let state_path = target_root.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    if matches!(
        std::fs::symlink_metadata(&state_path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    ) {
        return Ok(None);
    }
    let (state_bytes, _) = crate::path_identity::read_bounded_regular(&state_path, 1024 * 1024)
        .map_err(|_| C::UnsafePath)?;
    crate::path_identity::parse_json_no_duplicates(&state_bytes)
        .map(Some)
        .map_err(|_| C::UnsupportedProfile)
}

#[cfg(test)]
mod tests;
