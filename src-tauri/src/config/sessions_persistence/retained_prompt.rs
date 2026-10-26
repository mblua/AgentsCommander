use super::project_scope::strip_long_prefix_str;
use super::record::PersistedSession;
use super::write::rename_with_retry;
use crate::session::manager::{PromptKey, RetainedPrompt, SessionManager};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const RETAINED_PROMPTS: &str = super::super::instance_artifacts::RETAINED_PROMPTS;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PromptKeyWire {
    pub(super) canonical_cwd: String,
    pub(super) agent_id: String,
    pub(super) requested_profile: Option<String>,
    pub(super) effective_profile: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RetainedPromptWire {
    pub(super) key: PromptKeyWire,
    pub(super) last_prompt: Option<String>,
    pub(super) blocked: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetainedPromptsWire {
    pub(super) version: u32,
    pub(super) entries: Vec<RetainedPromptWire>,
}

/// Host identity only. Failed canonicalization never falls back to path text.
pub(crate) fn prompt_key(
    cwd: &str,
    agent_id: Option<&str>,
    requested_profile: Option<&str>,
    effective_profile: Option<&str>,
) -> Option<PromptKey> {
    let agent_id = agent_id.filter(|value| !value.is_empty())?;
    let effective_profile = effective_profile.filter(|value| !value.is_empty())?;
    let canonical = crate::filesystem::directories::retained_prompt_cwd(Path::new(cwd))?;
    let mut canonical_cwd = strip_long_prefix_str(canonical.to_str()?).to_string();
    if cfg!(windows) {
        canonical_cwd.make_ascii_lowercase();
    }
    Some(PromptKey {
        canonical_cwd,
        agent_id: agent_id.to_string(),
        requested_profile: requested_profile.map(str::to_string),
        effective_profile: effective_profile.to_string(),
    })
}

pub(super) fn load_retained_prompts(dir: &Path) -> Result<HashMap<PromptKey, RetainedPrompt>, ()> {
    let bytes = match std::fs::read(dir.join(RETAINED_PROMPTS)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err(()),
    };
    let wire: RetainedPromptsWire = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if wire.version != 1 {
        return Err(());
    }
    let mut entries = HashMap::new();
    for row in wire.entries {
        let key = PromptKey {
            canonical_cwd: row.key.canonical_cwd,
            agent_id: row.key.agent_id,
            requested_profile: row.key.requested_profile,
            effective_profile: row.key.effective_profile,
        };
        if let Some(previous) = entries.get_mut(&key) {
            let previous: &mut RetainedPrompt = previous;
            previous.last_prompt = None;
            previous.blocked = true;
        } else {
            entries.insert(
                key,
                RetainedPrompt {
                    last_prompt: row.last_prompt,
                    blocked: row.blocked,
                },
            );
        }
    }
    Ok(entries)
}

#[cfg(not(test))]
pub(crate) async fn initialize_prompt_retention(mgr: &SessionManager) {
    if let Some(dir) = super::super::config_dir() {
        initialize_prompt_retention_in_dir(mgr, &dir).await;
    } else {
        mgr.prompt_initialization
            .get_or_init(|| async {
                mgr.install_retained_prompts(None).await;
            })
            .await;
    }
}

#[cfg(test)]
pub(crate) async fn initialize_prompt_retention(mgr: &SessionManager) {
    // Lifecycle mocks never consult the developer's real instance directory.
    // IO regressions call the explicit directory initializer below.
    mgr.prompt_initialization
        .get_or_init(|| async {
            mgr.install_retained_prompts(Some(HashMap::new())).await;
        })
        .await;
}

pub(crate) async fn initialize_prompt_retention_in_dir(mgr: &SessionManager, dir: &Path) {
    mgr.prompt_initialization
        .get_or_init(|| async {
            let entries = load_retained_prompts(dir);
            if entries.is_err() {
                log::warn!("[last-prompt] cache unavailable; preserving existing checkpoint");
            }
            mgr.install_retained_prompts(entries.ok()).await;
        })
        .await;
}

/// Cold restore admits rows sequentially, so inspect its complete recipe set
/// before the first cache lookup. This creates no session associations.
pub(crate) async fn block_duplicate_restore_prompt_domains(
    mgr: &SessionManager,
    rows: &[PersistedSession],
) {
    let mut counts = HashMap::new();
    for row in rows {
        if let Some(key) = prompt_key(
            &row.working_directory,
            row.agent_id.as_deref(),
            row.requested_profile.as_deref(),
            row.effective_profile.as_deref(),
        ) {
            *counts.entry(key).or_insert(0usize) += 1;
        }
    }
    mgr.block_retained_prompt_domains(
        counts
            .into_iter()
            .filter_map(|(key, count)| (count > 1).then_some(key))
            .collect(),
    )
    .await;
}

/// A Windows comparison spelling has lost its canonical case, so NotFound on
/// that spelling cannot prove deletion under per-directory case sensitivity.
pub(super) fn prompt_directory_gc_confirmed(
    windows: bool,
    metadata: Result<(), std::io::ErrorKind>,
) -> bool {
    !windows && metadata == Err(std::io::ErrorKind::NotFound)
}

/// Called only after sessions_save_lock; always reads the current manager.
pub(super) async fn flush_retained_prompts_locked(
    mgr: &SessionManager,
    dir: &Path,
) -> Result<(), String> {
    flush_retained_prompts_locked_with_probe(mgr, dir, cfg!(windows), |path| {
        std::fs::metadata(path)
            .map(|_| ())
            .map_err(|error| error.kind())
    })
    .await
}

pub(super) async fn flush_retained_prompts_locked_with_probe(
    mgr: &SessionManager,
    dir: &Path,
    windows: bool,
    metadata: impl Fn(&str) -> Result<(), std::io::ErrorKind>,
) -> Result<(), String> {
    let Some(snapshot) = mgr.retained_prompt_snapshot().await else {
        return Ok(());
    };
    let mut entries = Vec::new();
    for (key, value) in snapshot {
        // A folded Windows key is never a deletion witness. Other platforms
        // still collect confirmed missing paths; transient failures keep bytes.
        if prompt_directory_gc_confirmed(windows, metadata(&key.canonical_cwd)) {
            mgr.forget_retained_prompt(&key).await;
            continue;
        }
        entries.push(RetainedPromptWire {
            key: PromptKeyWire {
                canonical_cwd: key.canonical_cwd,
                agent_id: key.agent_id,
                requested_profile: key.requested_profile,
                effective_profile: key.effective_profile,
            },
            last_prompt: value.last_prompt,
            blocked: value.blocked,
        });
    }
    entries.sort_by(|a, b| {
        (
            &a.key.canonical_cwd,
            &a.key.agent_id,
            &a.key.requested_profile,
            &a.key.effective_profile,
        )
            .cmp(&(
                &b.key.canonical_cwd,
                &b.key.agent_id,
                &b.key.requested_profile,
                &b.key.effective_profile,
            ))
    });
    static PROMPT_OP: AtomicU64 = AtomicU64::new(0);
    let temp = dir.join(format!(
        "{RETAINED_PROMPTS}.{}.{}.tmp",
        std::process::id(),
        PROMPT_OP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let bytes = serde_json::to_vec(&RetainedPromptsWire {
            version: 1,
            entries,
        })
        .map_err(|_| "last-prompt serialization failed".to_string())?;
        std::fs::write(&temp, bytes).map_err(|_| "last-prompt write failed".to_string())?;
        rename_with_retry(&temp, &dir.join(RETAINED_PROMPTS))
            .map_err(|_| "last-prompt rename failed".to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
        mgr.disable_prompt_reuse().await;
        log::warn!("[last-prompt] checkpoint failed; cache reuse disabled");
    }
    result
}
