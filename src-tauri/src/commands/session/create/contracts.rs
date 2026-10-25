use super::admission::OwnedCreateAdmission;
use crate::config::agent_command::AgentSpawnCommand;
use crate::pty::backend::ResolvedAgentHostShell;
use crate::session::manager::PendingCreateBinding;
use crate::session::selection::TrustedCreateIntent;
use crate::session::session::{SessionInfo, SessionRepo};
use crate::session::warnings::SessionWarning;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateSelectionIntent {
    User,
    Background,
    Suppress,
}

impl CreateSelectionIntent {
    pub(crate) fn trusted(self) -> Option<TrustedCreateIntent> {
        match self {
            Self::User => Some(TrustedCreateIntent::User),
            Self::Background => Some(TrustedCreateIntent::Background),
            Self::Suppress => None,
        }
    }
}

pub(crate) struct DeferredCreateOutput {
    pub(crate) info: SessionInfo,
    pub(crate) binding: PendingCreateBinding,
    pub(crate) warnings: Vec<SessionWarning>,
    // Keep admission through the enclosing retained transaction's finalization.
    pub(crate) _create_admission: Arc<Mutex<OwnedCreateAdmission>>,
    pub(crate) effective_continuation: bool,
}

#[derive(Clone)]
pub(crate) struct InitialLastPrompt {
    pub(crate) text: Option<String>,
    agent_id: Option<String>,
    requested_profile: Option<String>,
    effective_profile: Option<String>,
}

impl InitialLastPrompt {
    pub(crate) fn from_persisted(
        row: &crate::config::sessions_persistence::PersistedSession,
    ) -> Self {
        Self {
            text: row.last_prompt.clone(),
            agent_id: row.agent_id.clone(),
            requested_profile: row.requested_profile.clone(),
            effective_profile: row.effective_profile.clone(),
        }
    }

    pub(crate) fn compatible(&self, row: &crate::session::session::Session) -> bool {
        self.agent_id.is_some()
            && self.agent_id == row.agent_id
            && self.requested_profile == row.requested_profile
            && self.effective_profile == row.effective_profile
    }
}

pub(crate) enum CreateCompletion {
    Retained,
    Finalized(SessionInfo),
    Deferred(DeferredCreateOutput),
}

impl CreateCompletion {
    pub(crate) async fn into_finalized(self) -> Result<SessionInfo, String> {
        match self {
            Self::Retained => std::future::pending().await,
            Self::Finalized(info) => Ok(info),
            Self::Deferred(_) => Err("create unexpectedly deferred finalization".to_string()),
        }
    }
}

/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (Claude/Pi/Antigravity `--continue`, Codex `resume --last`, and for a
/// trusted empty-argv local macOS/Linux Muse recipe `resume --last`, #1873)
/// when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
///
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` — suppress all provider auto-resume. Use this for any "fresh
///   create" call site (UI/CLI/root-agent create, mailbox wake-from-cold
///   meaning no SessionManager record at this CWD, `restart_session` with
///   default semantics from `effective_restart_skip_auto_resume`).
/// - `false` — allow provider auto-resume. Use this only for paths restoring
///   a session AC already knows about (the startup-restore loop in `lib.rs`,
///   the wake-from-known-state branch in `mailbox::deliver_wake` — any
///   `RespawnExited` match, today driven exclusively by deferred-non-coord
///   `Exited(0)` records — and `restart_session` when its caller passes
///   `Some(false)`).
///
/// Owned launch inputs retained before a response replacement starts effects.
/// The executor alone supplies app state and a real lexical target proof.
#[derive(Debug, Clone)]
pub(crate) struct OwnedReplacementCreateRequest {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_tooling_save: bool,
    pub(crate) git_repos: Vec<SessionRepo>,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) host_shell: Option<ResolvedAgentHostShell>,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve owned finalization result layout; boxing adds an unnecessary allocation"
)]
pub(crate) enum OwnedReplacementCreateStep {
    Finalized(SessionInfo),
    Retained,
}
