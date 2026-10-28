use super::AttemptPreparation;
use super::AttemptSnapshot;
use super::ContextAlertRuntime;
use super::MemberIdentityFingerprint;
use super::MemberPolicyResolution;
use super::ReadyAttempt;
use super::ResolvedPolicy;
use crate::commands::entity_creation::metadata_is_link_or_reparse;
use crate::commands::entity_creation::parse_team_from_workgroup_name;
use crate::commands::entity_creation::read_team_config_classified;
use crate::commands::entity_creation::TeamConfigReadError;
use crate::filesystem::session_context;
use crate::phone::mailbox::InternalNoticeGuard;
use crate::phone::mailbox::InternalSystemNotice;
use crate::phone::mailbox::InternalSystemTarget;
use crate::phone::mailbox::MailboxPoller;
use crate::pty::context_scrape::ContextScraper;
use crate::session::manager::SessionManager;
use crate::session::session::Session;
use crate::session::session::SessionStatus;
use futures::future::BoxFuture;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub(super) struct ProductionContextAlertRuntime {
    pub(super) app: tauri::AppHandle,
    pub(super) messages: Arc<crate::config::injected_messages::MessageWarnings>,
}

impl ContextAlertRuntime for ProductionContextAlertRuntime {
    fn resolve_member_policy(
        &self,
        session_id: Uuid,
    ) -> BoxFuture<'static, MemberPolicyResolution> {
        let runtime = self.clone();
        Box::pin(async move { runtime.resolve_member_policy_inner(session_id).await })
    }

    fn check_session_live(&self, session_id: Uuid) -> BoxFuture<'static, bool> {
        let runtime = self.clone();
        Box::pin(async move { runtime.session_and_registration_live(session_id).await })
    }

    fn retire_sample_registration(&self, session_id: Uuid) {
        if let Some(scraper) = self.app.try_state::<Arc<ContextScraper>>() {
            scraper.retire_session(session_id);
        }
    }

    fn prepare_attempt(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> BoxFuture<'static, AttemptPreparation> {
        let runtime = self.clone();
        Box::pin(async move { runtime.prepare_attempt_inner(snapshot, cancellation).await })
    }

    fn deliver_attempt(&self, attempt: ReadyAttempt) -> BoxFuture<'static, Result<(), String>> {
        let app = self.app.clone();
        let messages = Arc::clone(&self.messages);
        Box::pin(async move {
            MailboxPoller::new(messages)
                .deliver_internal_system_notice(
                    &app,
                    attempt.target,
                    attempt.notice,
                    attempt.cancellation,
                    attempt.guard,
                )
                .await
        })
    }
}

impl ProductionContextAlertRuntime {
    async fn public_session(&self, session_id: Uuid) -> Option<Session> {
        let state = self
            .app
            .try_state::<Arc<tokio::sync::RwLock<SessionManager>>>()?;
        let manager = state.read().await.clone();
        manager.get_session(session_id).await
    }

    fn registered(&self, session_id: Uuid) -> bool {
        self.app
            .try_state::<Arc<ContextScraper>>()
            .is_some_and(|scraper| scraper.is_session_registered(session_id))
    }

    async fn session_and_registration_live(&self, session_id: Uuid) -> bool {
        self.public_session(session_id)
            .await
            .is_some_and(|session| !matches!(session.status, SessionStatus::Exited(_)))
            && self.registered(session_id)
    }

    async fn resolve_member_policy_inner(&self, session_id: Uuid) -> MemberPolicyResolution {
        let Some(session) = self.public_session(session_id).await else {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session is no longer present".to_string(),
            );
        };
        if session.is_root_agent
            || matches!(session.status, SessionStatus::Exited(_))
            || session.agent_id.is_none()
            || !self.registered(session_id)
        {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session is no longer an eligible registered member".to_string(),
            );
        }
        let agent_id = session.agent_id.clone().unwrap_or_default();
        let working_directory = session.working_directory.clone();
        let blocking_session = session.clone();
        let blocking =
            tokio::task::spawn_blocking(move || resolve_member_policy_blocking(&blocking_session))
                .await;
        let resolution = match blocking {
            Ok(resolution) => resolution,
            Err(error) => {
                return MemberPolicyResolution::RetryableFailure(format!(
                    "Member policy blocking task failed for session {}: {}",
                    session_id, error
                ));
            }
        };

        if matches!(
            resolution,
            MemberPolicyResolution::Eligible(_)
                | MemberPolicyResolution::Disabled(_)
                | MemberPolicyResolution::Paused { .. }
        ) {
            let current = self.public_session(session_id).await;
            if !session_matches_policy_snapshot(
                current.as_ref(),
                session_id,
                &agent_id,
                &working_directory,
                self.registered(session_id),
            ) {
                return MemberPolicyResolution::PermanentIneligible(
                    "sampled session identity changed during policy resolution".to_string(),
                );
            }
        }
        resolution
    }

    async fn prepare_attempt_inner(
        &self,
        snapshot: AttemptSnapshot,
        cancellation: CancellationToken,
    ) -> AttemptPreparation {
        log::debug!(
            "[context-alert] preparing generation={} priorFailures={}",
            snapshot.generation,
            snapshot.failure_count
        );
        let resolution = self.resolve_member_policy_inner(snapshot.session_id).await;
        let policy = match resolution {
            MemberPolicyResolution::Eligible(policy) => policy,
            MemberPolicyResolution::Disabled(fingerprint) => {
                return AttemptPreparation::Cancel {
                    reason: "context alert policy is disabled".to_string(),
                    current_policy: Some(ResolvedPolicy {
                        fingerprint,
                        thresholds: Vec::new(),
                    }),
                }
            }
            MemberPolicyResolution::PermanentIneligible(reason) => {
                return AttemptPreparation::Cancel {
                    reason,
                    current_policy: None,
                }
            }
            MemberPolicyResolution::Paused {
                fingerprint,
                class,
                message,
            } => {
                return AttemptPreparation::Paused {
                    fingerprint,
                    class,
                    message,
                }
            }
            MemberPolicyResolution::RetryableFailure(message) => {
                return AttemptPreparation::RetryableFailure(message)
            }
        };
        if policy.fingerprint != snapshot.fingerprint {
            return AttemptPreparation::Cancel {
                reason: "sampled member identity changed before delivery".to_string(),
                current_policy: None,
            };
        }
        let thresholds: Vec<u8> = snapshot
            .thresholds
            .iter()
            .copied()
            .filter(|threshold| policy.thresholds.contains(threshold))
            .collect();
        if thresholds.is_empty() {
            return AttemptPreparation::Cancel {
                reason: "all pending context alert thresholds were removed".to_string(),
                current_policy: Some(policy),
            };
        }
        let fingerprint = snapshot.fingerprint.clone();
        let observed = snapshot.observed;
        let route = tokio::task::spawn_blocking(move || {
            prepare_internal_route_blocking(&fingerprint, observed, &thresholds)
        })
        .await;
        let (target, notice) = match route {
            Ok(Ok(route)) => route,
            Ok(Err(error)) => return AttemptPreparation::RetryableFailure(error),
            Err(error) => {
                return AttemptPreparation::RetryableFailure(format!(
                    "Orchestrator route blocking task failed: {}",
                    error
                ))
            }
        };
        let guard_app = self.app.clone();
        let guard_fingerprint = snapshot.fingerprint.clone();
        let guard_target = target.clone();
        let guard_thresholds = notice
            .thresholds()
            .expect("a context-alert notice always carries thresholds")
            .to_vec();
        let guard_cancellation = cancellation.clone();
        let guard: InternalNoticeGuard = Arc::new(move || {
            validate_attempt_guard(
                &guard_app,
                &guard_fingerprint,
                &guard_target,
                &guard_thresholds,
                &guard_cancellation,
            )
        });
        AttemptPreparation::Ready(ReadyAttempt {
            snapshot,
            current_policy: policy.thresholds,
            target,
            notice,
            guard,
            cancellation,
        })
    }
}

pub(super) fn session_matches_policy_snapshot(
    current: Option<&Session>,
    session_id: Uuid,
    agent_id: &str,
    working_directory: &str,
    registered: bool,
) -> bool {
    current.is_some_and(|current| {
        current.id == session_id
            && current.agent_id.as_deref() == Some(agent_id)
            && current.working_directory == working_directory
            && !current.is_root_agent
            && !matches!(current.status, SessionStatus::Exited(_))
            && registered
    })
}

fn canonical_real_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let metadata = session_context::inspect_context_source(path)
        .map_err(|error| format!("{} '{}' is not readable: {}", label, path.display(), error))?;
    if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
        return Err(format!(
            "{} '{}' must be a real non-link directory",
            label,
            path.display()
        ));
    }
    session_context::canonical_context_source(path)
        .map(|canonical| crate::path_utils::normalize_windows_verbatim_path_buf(&canonical))
        .map_err(|error| {
            format!(
                "{} '{}' cannot be canonicalized: {}",
                label,
                path.display(),
                error
            )
        })
}

pub(super) fn resolve_member_policy_blocking(session: &Session) -> MemberPolicyResolution {
    let lexical_cwd = Path::new(&session.working_directory);
    let Some(lexical_replica) = lexical_cwd.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("__agent_"))
    }) else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD is not inside a lexical room member replica".to_string(),
        );
    };
    let Some(lexical_workgroup) = lexical_replica.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical replica has no room parent".to_string(),
        );
    };
    let Some(lexical_ac_root) = lexical_workgroup.parent() else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled lexical room has no Project AC Root parent".to_string(),
        );
    };
    for (path, label) in [
        (lexical_replica, "replica"),
        (lexical_workgroup, "room"),
        (lexical_ac_root, "Project AC Root"),
    ] {
        let metadata = match session_context::inspect_context_source(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                return MemberPolicyResolution::PermanentIneligible(format!(
                    "sampled lexical {} '{}' is not readable: {}",
                    label,
                    path.display(),
                    error
                ))
            }
        };
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled lexical {} '{}' must be a real non-link directory",
                label,
                path.display()
            ));
        }
    }
    let canonical_lexical_replica = match session_context::canonical_context_source(lexical_replica)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
    {
        Ok(path) => path,
        Err(error) => {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled lexical replica '{}' cannot be canonicalized: {}",
                lexical_replica.display(),
                error
            ))
        }
    };

    let canonical_cwd = match session_context::canonical_context_source(&session.working_directory)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
    {
        Ok(path) => path,
        Err(error) => {
            return MemberPolicyResolution::PermanentIneligible(format!(
                "sampled CWD '{}' cannot be canonicalized: {}",
                session.working_directory, error
            ))
        }
    };
    let Some(replica_candidate) = canonical_cwd.ancestors().find(|ancestor| {
        ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("__agent_"))
    }) else {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD is not inside a room member replica".to_string(),
        );
    };
    let replica_dir = match canonical_real_directory(replica_candidate, "Member replica") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    if canonical_lexical_replica != replica_dir {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD reaches a different replica through a link or path escape".to_string(),
        );
    }
    if canonical_cwd != replica_dir && !canonical_cwd.starts_with(&replica_dir) {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled CWD escapes its member replica".to_string(),
        );
    }
    let layout = match crate::config::ac_root::wg_replica_layout_from_agent_dir(&replica_dir) {
        Ok(Some(layout)) => layout,
        Ok(None) => {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled replica does not have the canonical room layout".to_string(),
            )
        }
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let workgroup_dir = match canonical_real_directory(&layout.wg_dir, "Room") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let ac_root = match canonical_real_directory(&layout.ac_root, "Project AC Root") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let team = match parse_team_from_workgroup_name(&layout.wg_name) {
        Ok(team) => team,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let (_, identity) =
        match crate::config::replica_identity::read_wg_replica_config_read_only(&replica_dir) {
            Ok(result) => result,
            Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
        };
    let identity_ac_root = match canonical_real_directory(&identity.ac_root, "Identity workspace") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let matrix_dir = match canonical_real_directory(&identity.matrix_dir, "Agent Matrix") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    if identity.agent_name != layout.agent_name || identity_ac_root != ac_root {
        return MemberPolicyResolution::PermanentIneligible(
            "sampled replica identity does not match its canonical layout".to_string(),
        );
    }
    let project_dir = match canonical_real_directory(&layout.project_dir, "Project") {
        Ok(path) => path,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let project = match validated_project_fqn_component(
        project_dir.file_name().and_then(|name| name.to_str()),
    ) {
        Ok(project) => project,
        Err(error) => return MemberPolicyResolution::PermanentIneligible(error),
    };
    let agent_id = match session.agent_id.clone() {
        Some(agent_id) if !agent_id.is_empty() => agent_id,
        _ => {
            return MemberPolicyResolution::PermanentIneligible(
                "sampled session has no coding-agent id".to_string(),
            )
        }
    };
    let fingerprint = MemberIdentityFingerprint {
        session_id: session.id,
        agent_id,
        working_directory: session.working_directory.clone(),
        project_dir,
        ac_root: ac_root.clone(),
        workgroup_dir,
        replica_dir,
        matrix_dir,
        project,
        team: team.clone(),
        workgroup: layout.wg_name,
        member: layout.agent_name,
    };

    match read_team_config_classified(&ac_root, &team) {
        Ok(config) => {
            let member_ref = format!("_agent_{}", fingerprint.member);
            if !config.agents.contains(&member_ref) {
                return MemberPolicyResolution::PermanentIneligible(
                    "sampled member is no longer in the canonical team roster".to_string(),
                );
            }
            if config.context_alert_percentages.is_empty() {
                MemberPolicyResolution::Disabled(fingerprint)
            } else {
                MemberPolicyResolution::Eligible(ResolvedPolicy {
                    fingerprint,
                    thresholds: config.context_alert_percentages,
                })
            }
        }
        Err(error @ TeamConfigReadError::NotFound { .. }) => {
            MemberPolicyResolution::PermanentIneligible(error.to_string())
        }
        Err(error) => MemberPolicyResolution::Paused {
            fingerprint,
            class: error.class(),
            message: error.to_string(),
        },
    }
}

pub(super) fn validated_project_fqn_component(component: Option<&str>) -> Result<String, String> {
    match component {
        Some(project)
            if !project.is_empty()
                && !project.contains(':')
                && !project
                    .chars()
                    .any(|character| character == '\0' || character.is_ascii_control()) =>
        {
            Ok(project.to_string())
        }
        _ => Err("project directory has an invalid qualified-name component".to_string()),
    }
}

pub(super) fn prepare_internal_route_blocking(
    fingerprint: &MemberIdentityFingerprint,
    observed: u8,
    thresholds: &[u8],
) -> Result<(InternalSystemTarget, InternalSystemNotice), String> {
    let config = read_team_config_classified(&fingerprint.ac_root, &fingerprint.team)
        .map_err(|error| error.to_string())?;
    let coordinator =
        crate::config::replica_identity::agent_bare_name_from_ref(&config.coordinator)?;
    let expected = fingerprint
        .workgroup_dir
        .join(format!("__agent_{}", coordinator));
    let canonical_expected = canonical_real_directory(&expected, "Orchestrator replica")?;
    if canonical_expected.parent() != Some(fingerprint.workgroup_dir.as_path())
        || canonical_expected
            .file_name()
            .and_then(|name| name.to_str())
            != Some(format!("__agent_{}", coordinator).as_str())
    {
        return Err("Orchestrator replica escapes the sampled room".to_string());
    }
    let resolved = crate::config::teams::resolve_wg_coordinator_replica(
        &fingerprint.ac_root,
        &fingerprint.workgroup_dir,
    )
    .ok_or_else(|| "Current orchestrator replica is unavailable".to_string())?;
    let canonical_resolved =
        canonical_real_directory(&resolved.replica_dir, "Resolved orchestrator replica")?;
    if resolved.project != fingerprint.project
        || resolved.team != fingerprint.team
        || resolved.wg_name != fingerprint.workgroup
        || resolved.agent_name != coordinator
        || canonical_resolved != canonical_expected
    {
        return Err(
            "Resolved orchestrator identity does not match the exact sampled room".to_string(),
        );
    }
    let fqn = format!(
        "{}:{}/{}",
        resolved.project, resolved.wg_name, resolved.agent_name
    );
    let target = InternalSystemTarget::for_context_alert(fqn, canonical_resolved)?;
    let notice = InternalSystemNotice::for_context_alert(
        fingerprint.member.clone(),
        fingerprint.workgroup.clone(),
        observed,
        thresholds.to_vec(),
    )?;
    Ok((target, notice))
}

pub(super) fn validate_attempt_guard<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    fingerprint: &MemberIdentityFingerprint,
    target: &InternalSystemTarget,
    thresholds: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Context alert attempt was canceled".to_string());
    }
    if app
        .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
        .is_some_and(|guard| guard.blocks_agent(target.fqn()))
    {
        return Err(format!("purge-room blocks '{}'", target.fqn()));
    }
    let scraper = app
        .try_state::<Arc<ContextScraper>>()
        .ok_or_else(|| "Context scraper is unavailable".to_string())?;
    if !scraper.is_session_registered(fingerprint.session_id) {
        return Err("Sampled session is no longer registered".to_string());
    }
    let replica = canonical_real_directory(&fingerprint.replica_dir, "Member replica")?;
    let workgroup = canonical_real_directory(&fingerprint.workgroup_dir, "Room")?;
    let ac_root = canonical_real_directory(&fingerprint.ac_root, "Project AC Root")?;
    let project = canonical_real_directory(&fingerprint.project_dir, "Project")?;
    if replica != fingerprint.replica_dir
        || workgroup != fingerprint.workgroup_dir
        || ac_root != fingerprint.ac_root
        || project != fingerprint.project_dir
        || replica.parent() != Some(workgroup.as_path())
        || workgroup.parent() != Some(ac_root.as_path())
        || ac_root.parent() != Some(project.as_path())
        || replica.file_name().and_then(|name| name.to_str())
            != Some(format!("__agent_{}", fingerprint.member).as_str())
        || parse_team_from_workgroup_name(&fingerprint.workgroup)? != fingerprint.team
    {
        return Err("Sampled canonical identity paths changed".to_string());
    }
    let (_, identity) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&replica)?;
    let identity_ac_root = canonical_real_directory(&identity.ac_root, "Identity workspace")?;
    let identity_matrix = canonical_real_directory(&identity.matrix_dir, "Agent Matrix")?;
    if identity.agent_name != fingerprint.member
        || identity_ac_root != fingerprint.ac_root
        || identity_matrix != fingerprint.matrix_dir
    {
        return Err("Sampled member identity changed".to_string());
    }
    let config = read_team_config_classified(&ac_root, &fingerprint.team)
        .map_err(|error| error.to_string())?;
    if !config
        .agents
        .contains(&format!("_agent_{}", fingerprint.member))
        || thresholds
            .iter()
            .any(|threshold| !config.context_alert_percentages.contains(threshold))
    {
        return Err("Sampled member roster or context alert policy changed".to_string());
    }
    let (fresh_target, _) = prepare_internal_route_blocking(fingerprint, 100, thresholds)?;
    if fresh_target != *target {
        return Err("Orchestrator target changed before notice delivery".to_string());
    }
    Ok(())
}
