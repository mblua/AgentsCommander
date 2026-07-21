use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::settings::WindowGeometry;
use crate::pty::backend::SessionBackendKind;
use crate::session::profile::{
    detect_configured_pty_submission_agent, detect_pty_submission_agent, CodingAgentKind,
    PtySubmissionAgent,
};

/// Mangle a CWD path the same way Claude Code does for its project directories.
/// Non-alphanumeric, non-hyphen characters are replaced with '-'.
/// Used by session creation (--continue detection) and the JSONL watcher.
pub fn mangle_cwd_for_claude(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Prefix used historically by wake-and-sleep delivery (removed in 0.7.0).
/// Retained for defensive purge of legacy temp sessions persisted under
/// older versions, and as a sort-key tiebreaker in `find_active_session`
/// (non-temp sessions preferred).
pub const TEMP_SESSION_PREFIX: &str = "[temp]";

/// One repo watched inside a session, rendered as a single sidebar badge "<label>/<branch>".
/// Populated at session creation time from the replica's `repoPaths`; `branch` is filled
/// and refreshed by `GitWatcher` on each poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRepo {
    /// Repo dir name with leading "repo-" stripped (e.g. "AgentsCommander").
    pub label: String,
    /// Absolute path to the repo root. Branch detection runs `git rev-parse` in this dir.
    pub source_path: String,
    /// Current branch. `None` until first watcher tick, or when detection fails.
    #[serde(default)]
    pub branch: Option<String>,
    /// #1028 - worktree dirty: untracked, unstaged, or staged-but-uncommitted changes.
    /// `Some(true)` paints the badge letters red. `None` = never successfully detected
    /// for this path since process start, rendered violet like clean; a failed detection
    /// holds the last known answer instead (`git_watcher::remember_dirty`), so `None`
    /// means "no first answer yet", not "flaked once".
    ///
    /// `skip_deserializing`, not merely `default`: `dirty` is backend-authoritative and
    /// must never be restored. A persisted `true` would otherwise paint a red badge at
    /// launch from a worktree state that may be long gone, and `create_session` (a
    /// `#[tauri::command]`, `commands/session.rs`) genuinely deserializes this struct
    /// from frontend input, which would make the badge spoofable from a command payload.
    /// Serializing is kept so the on-disk value stays self-describing in a bug report;
    /// it is written and then ignored on read. NOTE: a test that round-trips `dirty`
    /// through a write/read helper will get `None` back. That is this attribute, by
    /// design, not a serde bug.
    #[serde(default, skip_deserializing)]
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionCommunicationKind {
    RaiseHand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionCommunication {
    pub kind: SessionCommunicationKind,
    pub visible: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    #[serde(default)]
    pub backend_kind: SessionBackendKind,
    /// Effective arg vector actually handed to portable-pty at spawn time,
    /// including dynamic provider resume injections (`--continue`,
    /// `codex resume --last`, `gemini --resume latest`). `None` until the PTY
    /// is spawned for this session; set once by `create_session_inner` right
    /// before `pty_mgr.spawn`. Runtime-only. NOT persisted to `sessions.toml`
    /// (configured args in `shell_args` are the persistence recipe; the
    /// effective args are re-derived at every spawn from current settings).
    #[serde(skip)]
    pub effective_shell_args: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    #[serde(skip)]
    pub communication: Option<SessionCommunication>,
    /// Frontend-only: true when agent finished but user hasn't focused yet
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_label: Option<String>,
    /// Repos watched by this session. Empty = no repo badge rendered.
    /// Order = replica config.json `repos` array order. Never sort, never dedupe,
    /// never rebuild from a map — equality comparisons in `GitWatcher` depend on order.
    #[serde(default)]
    pub git_repos: Vec<SessionRepo>,
    /// Whether this session's agent is a coordinator of any discovered team.
    /// Controls repo-badge visibility on the sidebar. Recomputed after every discovery.
    #[serde(default)]
    pub is_coordinator: bool,
    /// Whether this is the global Root Agent / Agents Commander session.
    #[serde(default)]
    pub is_root_agent: bool,
    /// Monotonic generation counter for `git_repos`. Bumped on every refresh/watcher write.
    /// Used for compare-and-swap in `set_git_repos_if_gen` so an in-flight watcher poll
    /// cannot overwrite a refresh that landed during its detection window. Runtime-only;
    /// never persisted and never exposed via SessionInfo.
    #[serde(skip)]
    pub git_repos_gen: u64,
    /// Unique token for CLI authentication. Agent PTY children receive it via per-child `AGENTSCOMMANDER_TOKEN` env at spawn.
    pub token: Uuid,
    /// Resolved coding-agent identity, or `None` for a plain shell. Set once
    /// by `create_session_inner` via `CodingAgentKind::detect`. The single
    /// source of truth that replaced the #258 `is_claude`/`is_codex`/
    /// `is_gemini` triple (#260). Drives idle tuning, resume-arg injection,
    /// and Telegram reader selection.
    #[serde(default)]
    pub agent_kind: Option<CodingAgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_fallback_chain: Vec<String>,
    #[serde(default)]
    pub profile_fallback_applied: bool,
    #[serde(skip)]
    pub effective_codex_home: Option<String>,
    #[serde(skip)]
    pub resolved_claude_projects_dir: Option<PathBuf>,
    /// #592 - 16-hex content-hash of the profile cell this session launched
    /// with. In-memory; the durable copy is `tooling.profileContentHash` in the
    /// replica config. `None` for plain-shell sessions (no resolved profile).
    #[serde(skip)]
    pub profile_content_hash: Option<String>,
    /// Runtime proof that this exact launch recipe came from the configured
    /// `AgentSpawnCommand` resolver. Prefix-named wrappers require this proof;
    /// ad-hoc agent IDs and persisted metadata cannot set it.
    #[serde(skip)]
    pub trusted_configured_spawn: bool,
    /// Telegram bot id that should be attached whenever this session has a live PTY.
    /// None means the Telegram toggle is OFF for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_bot_id: Option<String>,
    /// True while this session has a live detached window (or is marked to re-spawn
    /// one on next launch). Source of truth for persistence — `snapshot_sessions`
    /// reads this directly, NOT from `DetachedSessionsState`.
    ///
    /// Mutated ONLY by:
    ///   - `detach_terminal_inner` → true (after window build + session recheck)
    ///   - `attach_terminal` → false (before emitting `terminal_attached`)
    ///
    /// The `WindowEvent::Destroyed` handler at `lib.rs` does NOT touch this field
    /// (see plan §A3.12 NEW-3 + §10 rule) — it only clears `DetachedSessionsState`
    /// and emits `terminal_attached` for frontend sync.
    #[serde(default)]
    pub was_detached: bool,
    /// Last-known geometry of this session's detached window. Written on drag/resize
    /// via `set_detached_geometry`; read at spawn time by `detach_terminal_inner`
    /// (including the Phase 3 restore path).
    #[serde(default)]
    pub detached_geometry: Option<WindowGeometry>,
    /// (#630/#631) Durable per-session resume intent for the next app restart.
    /// `true` => the user explicitly started this session fresh ("Restart Session")
    /// and that intent must survive the restart, so the restore path passes
    /// `skip_auto_resume = true` and injects no `--continue`. `false` (default)
    /// => resume the prior conversation. Re-armed to `false` on the first real
    /// user message (`note_user_message_to_session`) so a fresh-then-used session
    /// resumes its NEW conversation next time. Durable copy lives in
    /// `PersistedSession`; carried (off the wire) through `SessionInfo`.
    #[serde(default)]
    pub start_fresh_on_restore: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Active,
    Running,
    Idle,
    Exited(i32),
}

pub(crate) fn is_live_session_record(has_id: bool, status: Option<&SessionStatus>) -> bool {
    has_id && !matches!(status, Some(SessionStatus::Exited(_)))
}

/// Walk up from `cwd` to the first ancestor directory whose name starts with
/// `wg-`, and return that directory's `TASK.md` path. Returns `None` if no
/// such ancestor exists (does NOT check that the file exists on disk — caller
/// decides how to handle a missing file).
pub(crate) fn find_workgroup_task_path_for_cwd(cwd: &str) -> Option<std::path::PathBuf> {
    let mut current = Some(Path::new(cwd));
    while let Some(path) = current {
        let is_workgroup_dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("wg-"));
        if is_workgroup_dir {
            return Some(path.join("TASK.md"));
        }
        current = path.parent();
    }
    None
}

pub(crate) fn read_workgroup_task_for_cwd(cwd: &str) -> Option<String> {
    let path = find_workgroup_task_path_for_cwd(cwd)?;
    std::fs::read_to_string(&path)
        .ok()
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
}

/// Info sent to the frontend via IPC
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub shell_args: Vec<String>,
    #[serde(default)]
    pub backend_kind: SessionBackendKind,
    /// See `Session::effective_shell_args`. `None` means "not yet registered"
    /// (dormant or pre-spawn). On the wire, serializes as `null`.
    #[serde(default)]
    pub effective_shell_args: Option<Vec<String>>,
    pub created_at: String,
    pub working_directory: String,
    pub status: SessionStatus,
    pub waiting_for_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub communication: Option<SessionCommunication>,
    #[serde(default)]
    pub pending_review: bool,
    pub last_prompt: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_label: Option<String>,
    #[serde(default)]
    pub git_repos: Vec<SessionRepo>,
    #[serde(default)]
    pub workgroup_task: Option<String>,
    #[serde(default)]
    pub is_coordinator: bool,
    #[serde(default)]
    pub is_root_agent: bool,
    pub token: String,
    #[serde(default)]
    pub agent_kind: Option<CodingAgentKind>,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default)]
    pub effective_profile: Option<String>,
    #[serde(default)]
    pub profile_fallback_chain: Vec<String>,
    #[serde(default)]
    pub profile_fallback_applied: bool,
    #[serde(skip)]
    pub effective_codex_home: Option<String>,
    /// #592 - internal carrier (NOT on the wire) of the session's loaded hash,
    /// read by the `list_sessions` command to compute `profile_outdated`.
    #[serde(skip)]
    pub profile_content_hash: Option<String>,
    /// Internal configured-spawn provenance copied from `Session`.
    #[serde(skip)]
    pub trusted_configured_spawn: bool,
    /// #592 - true when the loaded profile cell no longer matches the current
    /// configuration. Computed by the `list_sessions` command (settings-aware);
    /// `From<&Session>` cannot compute it (no settings) so it defaults false.
    #[serde(default)]
    pub profile_outdated: bool,
    /// Internal carrier for sessions persistence. Not part of the frontend contract;
    /// the UI uses BridgeInfo events/listing for live bridge state.
    #[serde(skip)]
    pub telegram_bot_id: Option<String>,
    #[serde(default)]
    pub was_detached: bool,
    /// Not serialized to the frontend — internal carrier for `snapshot_sessions`
    /// so persistence can read the last-known detached-window geometry without a
    /// second lock round-trip through `SessionManager::get_session`.
    #[serde(skip)]
    pub detached_geometry: Option<WindowGeometry>,
    /// (#630/#631) Not serialized to the frontend: internal carrier for
    /// `snapshot_sessions` so persistence can read the durable resume intent
    /// without a second lock round-trip. Mirrors `detached_geometry`: `#[serde(skip)]`
    /// keeps it off the IPC wire, so the frontend contract is unchanged.
    #[serde(skip)]
    pub start_fresh_on_restore: bool,
}

impl SessionInfo {
    pub(crate) fn pty_submission_agent_matches_current_spawn(
        &self,
        spawn: &crate::config::agent_command::AgentSpawnCommand,
    ) -> Option<PtySubmissionAgent> {
        let args = self
            .effective_shell_args
            .as_deref()
            .unwrap_or(&self.shell_args);
        if let Some(agent) = detect_pty_submission_agent(&self.shell, args, self.agent_kind) {
            return Some(agent);
        }
        if !self.trusted_configured_spawn
            || self.agent_id.as_deref() != Some(spawn.trusted_agent_id.as_str())
            || self.shell != spawn.shell
            || self.shell_args != spawn.shell_args
            || self.profile_content_hash.as_deref() != Some(spawn.profile_content_hash.as_str())
            || self.backend_kind != SessionBackendKind::from(&spawn.backend)
        {
            return None;
        }
        detect_configured_pty_submission_agent(&self.shell, args, self.agent_kind)
    }
}

impl From<&Session> for SessionInfo {
    fn from(s: &Session) -> Self {
        SessionInfo {
            id: s.id.to_string(),
            name: s.name.clone(),
            shell: s.shell.clone(),
            shell_args: s.shell_args.clone(),
            backend_kind: s.backend_kind,
            effective_shell_args: s.effective_shell_args.clone(),
            created_at: s.created_at.to_rfc3339(),
            working_directory: s.working_directory.clone(),
            status: s.status.clone(),
            waiting_for_input: s.waiting_for_input,
            communication: s.communication.clone(),
            pending_review: false,
            last_prompt: s.last_prompt.clone(),
            agent_id: s.agent_id.clone(),
            agent_label: s.agent_label.clone(),
            git_repos: s.git_repos.clone(),
            workgroup_task: read_workgroup_task_for_cwd(&s.working_directory),
            is_coordinator: s.is_coordinator,
            is_root_agent: s.is_root_agent,
            token: s.token.to_string(),
            agent_kind: s.agent_kind,
            requested_profile: s.requested_profile.clone(),
            effective_profile: s.effective_profile.clone(),
            profile_fallback_chain: s.profile_fallback_chain.clone(),
            profile_fallback_applied: s.profile_fallback_applied,
            effective_codex_home: s.effective_codex_home.clone(),
            profile_content_hash: s.profile_content_hash.clone(),
            trusted_configured_spawn: s.trusted_configured_spawn,
            profile_outdated: false,
            telegram_bot_id: s.telegram_bot_id.clone(),
            was_detached: s.was_detached,
            detached_geometry: s.detached_geometry.clone(),
            start_fresh_on_restore: s.start_fresh_on_restore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(effective: Option<Vec<String>>) -> Session {
        Session {
            id: Uuid::nil(),
            name: "Session 1".to_string(),
            shell: "claude-mb".to_string(),
            shell_args: vec!["--dangerously-skip-permissions".to_string()],
            backend_kind: SessionBackendKind::LocalProcess,
            effective_shell_args: effective,
            created_at: Utc::now(),
            working_directory: "C:\\tmp".to_string(),
            status: SessionStatus::Running,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: None,
            agent_label: None,
            git_repos: Vec::new(),
            is_coordinator: false,
            is_root_agent: false,
            git_repos_gen: 0,
            token: Uuid::nil(),
            agent_kind: None,
            requested_profile: None,
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            resolved_claude_projects_dir: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
        }
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_some() {
        let s = sample_session(Some(vec![
            "--dangerously-skip-permissions".to_string(),
            "--continue".to_string(),
        ]));
        let info = SessionInfo::from(&s);
        assert_eq!(
            info.effective_shell_args,
            Some(vec![
                "--dangerously-skip-permissions".to_string(),
                "--continue".to_string()
            ])
        );
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_none() {
        let s = sample_session(None);
        let info = SessionInfo::from(&s);
        assert_eq!(info.effective_shell_args, None);
    }

    #[test]
    fn session_info_from_session_copies_effective_shell_args_empty() {
        let s = sample_session(Some(Vec::new()));
        let info = SessionInfo::from(&s);
        assert_eq!(info.effective_shell_args, Some(Vec::new()));
    }

    #[test]
    fn session_info_from_session_copies_communication() {
        let mut s = sample_session(None);
        s.communication = Some(SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-28T17:00:00+00:00".to_string(),
        });
        let info = SessionInfo::from(&s);
        assert_eq!(info.communication, s.communication);
    }

    #[test]
    fn session_info_from_session_copies_telegram_bot_id_internally() {
        let mut s = sample_session(None);
        s.telegram_bot_id = Some("bot-1".to_string());

        let info = SessionInfo::from(&s);

        assert_eq!(info.telegram_bot_id.as_deref(), Some("bot-1"));
    }

    #[test]
    fn session_info_serialization_does_not_expose_telegram_bot_id() {
        let mut s = sample_session(None);
        s.telegram_bot_id = Some("bot-1".to_string());

        let json = serde_json::to_value(SessionInfo::from(&s)).expect("serialize SessionInfo");

        assert!(json.get("telegramBotId").is_none());
    }

    #[test]
    fn live_session_record_requires_id_and_non_exited_status() {
        assert!(is_live_session_record(true, Some(&SessionStatus::Active)));
        assert!(is_live_session_record(true, Some(&SessionStatus::Running)));
        assert!(is_live_session_record(true, Some(&SessionStatus::Idle)));
        assert!(is_live_session_record(true, None));
        assert!(!is_live_session_record(
            false,
            Some(&SessionStatus::Running)
        ));
        assert!(!is_live_session_record(
            true,
            Some(&SessionStatus::Exited(0))
        ));
    }

    // ── find_workgroup_task_path_for_cwd — issue #107 ──

    #[test]
    fn find_workgroup_task_path_returns_path_when_cwd_is_workgroup_root() {
        let p = find_workgroup_task_path_for_cwd(r"C:\proj\.ac\wg-3-team");
        assert_eq!(
            p,
            Some(std::path::PathBuf::from(r"C:\proj\.ac\wg-3-team\TASK.md"))
        );
    }

    #[test]
    fn find_workgroup_task_path_walks_up_from_replica_dir() {
        let p = find_workgroup_task_path_for_cwd(r"C:\proj\.ac\wg-3-team\__agent_dev-rust");
        assert_eq!(
            p,
            Some(std::path::PathBuf::from(r"C:\proj\.ac\wg-3-team\TASK.md"))
        );
    }

    #[test]
    fn find_workgroup_task_path_returns_none_outside_workgroup() {
        assert_eq!(find_workgroup_task_path_for_cwd(r"C:\Users\me\misc"), None);
    }

    #[test]
    fn find_workgroup_task_path_handles_unc_prefix_input() {
        // The helper is a pure path walk; it does not strip `\\?\` itself —
        // §9.4 strips the prefix downstream when embedding into the prompt.
        // This test documents that the walk-up still finds the wg-* ancestor
        // even when the input carries the prefix.
        let p = find_workgroup_task_path_for_cwd(r"\\?\C:\proj\.ac\wg-3-team");
        assert!(p.is_some());
        let p = p.unwrap().to_string_lossy().to_string();
        assert!(p.ends_with(r"\wg-3-team\TASK.md"));
    }

    // --- #1028: SessionRepo.dirty is backend-authoritative and never restored ---

    /// The whole point of `skip_deserializing`. A `sessions.json` written before the app
    /// closed can hold `dirty: true` from a worktree state that is long gone; restoring
    /// it would paint a red badge at launch that no watcher tick asked for. It must come
    /// back `None` (violet, "not yet known") and wait for a real detection.
    ///
    /// This also covers `create_session`, a `#[tauri::command]` that genuinely
    /// deserializes `Vec<SessionRepo>` from frontend input: `dirty` is un-settable from a
    /// command payload, so the badge cannot be spoofed. The frontend sends only
    /// `{label, sourcePath}` today, so nothing is lost.
    #[test]
    fn session_repo_dirty_is_never_deserialized() {
        let stale = r#"{"label":"A","sourcePath":"C:/a","branch":"main","dirty":true}"#;
        let restored: SessionRepo = serde_json::from_str(stale).expect("deserialize");

        assert_eq!(
            restored.dirty, None,
            "a persisted `true` must NOT restore red"
        );
        assert_eq!(
            restored.branch.as_deref(),
            Some("main"),
            "branch still restores; only dirty is skipped"
        );
    }

    /// Back-compat: `sessions.json` from before #1028 has no `dirty` key at all.
    #[test]
    fn session_repo_without_dirty_key_restores_as_none() {
        let old = r#"{"label":"A","sourcePath":"C:/a","branch":"main"}"#;
        let restored: SessionRepo = serde_json::from_str(old).expect("deserialize");
        assert_eq!(restored.dirty, None);
    }

    /// Serialization is deliberately KEPT, so the wire and the on-disk file stay
    /// self-describing: the key is always present, `None` as an explicit `null` rather
    /// than an omitted key. The frontend reads `dirty === true`, and `skip_serializing_if`
    /// would have stripped the key here while STILL letting a stale `true` through on
    /// read, which is why it was rejected.
    #[test]
    fn session_repo_dirty_always_serializes_with_an_explicit_null() {
        let dirty = SessionRepo {
            label: "A".into(),
            source_path: "C:/a".into(),
            branch: Some("main".into()),
            dirty: Some(true),
        };
        assert_eq!(
            serde_json::to_value(&dirty).expect("serialize"),
            serde_json::json!({
                "label": "A",
                "sourcePath": "C:/a",
                "branch": "main",
                "dirty": true,
            })
        );

        let unknown = SessionRepo {
            label: "A".into(),
            source_path: "C:/a".into(),
            branch: None,
            dirty: None,
        };
        assert_eq!(
            serde_json::to_value(&unknown).expect("serialize")["dirty"],
            serde_json::json!(null),
            "key present as null, not omitted"
        );
    }
}
