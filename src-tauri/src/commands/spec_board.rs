use std::collections::HashMap;

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use tokio::task::JoinHandle;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::config::settings::{AppSettings, SettingsState};
use crate::spec_board::internal_dirs;
use crate::spec_board::types::SnapshotFile;
pub use crate::spec_board::types::{SpecBoardFileKind, SpecBoardSnapshot, SpecBoardSnapshotSource};
use crate::SpecBoardState;

mod paths;
mod snapshots;

#[cfg(test)]
use paths::path_compare_key;
use paths::{resolve_repo_root, validate_existing_user_path, validate_new_or_existing_user_path};
#[cfg(test)]
use snapshots::write_snapshot_atomic;
use snapshots::{create_snapshot_for_doc, list_snapshots_from_dir, read_snapshot_files};

const STARTER_CONTENT: &str = "flowchart TD\n  A[Start] --> B[Spec]\n";
const MAX_FILE_BYTES: u64 = 1_048_576;
const MAX_SNAPSHOT_CONTENT_BYTES: usize = 1_048_576;
const MAX_SNAPSHOTS: usize = 100;
const MAX_SNAPSHOT_TOTAL_BYTES: u64 = 25 * 1_048_576;
const WATCH_POLL_MS: u64 = 400;
const WATCH_FULL_HASH_MS: i64 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardDocument {
    pub doc_id: String,
    pub repo_root: String,
    pub path: Option<String>,
    pub file_kind: SpecBoardFileKind,
    pub content: String,
    pub diagram_source: String,
    pub dirty: bool,
    pub conflict: Option<SpecBoardConflict>,
    pub version_index: usize,
    pub version_count: usize,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardConflict {
    pub path: String,
    pub pending_external_content: String,
    pub pending_external_diagram_source: String,
    pub detected_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardChangedEvent {
    pub doc_id: String,
    pub path: Option<String>,
    pub content: String,
    pub diagram_source: String,
    pub version_index: usize,
    pub version_count: usize,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardFileMissingEvent {
    pub doc_id: String,
    pub path: String,
}

struct DocumentState {
    document: SpecBoardDocument,
    disk_hash: Option<String>,
    app_write_hash: Option<String>,
    last_external_snapshot_hash: Option<String>,
    snapshot_sequence: u64,
    snapshot_dir: PathBuf,
    watch_handle: Option<JoinHandle<()>>,
    edit_snapshot_handle: Option<JoinHandle<()>>,
    last_edit_snapshot_hash: Option<String>,
}

#[derive(Default)]
pub struct SpecBoardManager {
    docs: HashMap<String, DocumentState>,
}

impl SpecBoardManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn upsert_document(
        &mut self,
        document: SpecBoardDocument,
        disk_hash: Option<String>,
        snapshot_dir: PathBuf,
        _draft_dir: PathBuf,
    ) {
        if let Some(mut old) = self.docs.remove(&document.doc_id) {
            abort_handle(old.watch_handle.take());
            abort_handle(old.edit_snapshot_handle.take());
        }

        self.docs.insert(
            document.doc_id.clone(),
            DocumentState {
                document,
                disk_hash,
                app_write_hash: None,
                last_external_snapshot_hash: None,
                snapshot_sequence: 0,
                snapshot_dir,
                watch_handle: None,
                edit_snapshot_handle: None,
                last_edit_snapshot_hash: None,
            },
        );
    }

    fn get_document(&self, doc_id: &str) -> Result<SpecBoardDocument, String> {
        self.docs
            .get(doc_id)
            .map(|s| s.document.clone())
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))
    }

    fn close(&mut self, doc_id: &str) {
        if let Some(mut state) = self.docs.remove(doc_id) {
            abort_handle(state.watch_handle.take());
            abort_handle(state.edit_snapshot_handle.take());
        }
    }

    fn close_all(&mut self) {
        let ids: Vec<String> = self.docs.keys().cloned().collect();
        for id in ids {
            self.close(&id);
        }
    }
}

impl Drop for SpecBoardManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

#[tauri::command]
pub async fn spec_board_new(
    state: State<'_, SpecBoardState>,
    settings: State<'_, SettingsState>,
    repo_root: Option<String>,
) -> Result<SpecBoardDocument, String> {
    let settings_snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    let repo = resolve_repo_root(repo_root.as_deref(), &settings_snapshot).await?;
    let doc_id = uuid::Uuid::new_v4().to_string();
    let (snapshot_dir, draft_dir) = internal_dirs(&repo, &doc_id).await?;
    tokio::fs::create_dir_all(&snapshot_dir)
        .await
        .map_err(|e| format!("Failed to create snapshot directory: {}", e))?;
    tokio::fs::create_dir_all(&draft_dir)
        .await
        .map_err(|e| format!("Failed to create draft directory: {}", e))?;

    let now = now_ms();
    let diagram_source = extract_diagram_source(STARTER_CONTENT, SpecBoardFileKind::Mermaid)?;
    let document = SpecBoardDocument {
        doc_id: doc_id.clone(),
        repo_root: path_to_string(&repo),
        path: None,
        file_kind: SpecBoardFileKind::Mermaid,
        content: STARTER_CONTENT.to_string(),
        diagram_source,
        dirty: true,
        conflict: None,
        version_index: 0,
        version_count: 0,
        updated_at_ms: now,
    };

    {
        let mut mgr = state.write().await;
        mgr.upsert_document(document.clone(), None, snapshot_dir, draft_dir);
    }

    create_snapshot_for_doc(
        state.inner().clone(),
        &doc_id,
        SpecBoardSnapshotSource::Initial,
    )
    .await?;
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

#[tauri::command]
pub async fn spec_board_pick_open(
    state: State<'_, SpecBoardState>,
    settings: State<'_, SettingsState>,
    app: AppHandle,
    repo_root: Option<String>,
) -> Result<Option<SpecBoardDocument>, String> {
    let settings_snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    let default_root = resolve_repo_root(repo_root.as_deref(), &settings_snapshot).await?;
    let picked = rfd::AsyncFileDialog::new()
        .add_filter("Spec diagrams", &["mmd", "mermaid", "md"])
        .set_directory(&default_root)
        .pick_file()
        .await;

    let Some(handle) = picked else {
        return Ok(None);
    };

    spec_board_open_inner(
        state.inner().clone(),
        settings_snapshot,
        app,
        handle.path().to_path_buf(),
    )
    .await
    .map(Some)
}

#[tauri::command]
pub async fn spec_board_open(
    state: State<'_, SpecBoardState>,
    settings: State<'_, SettingsState>,
    app: AppHandle,
    path: String,
) -> Result<SpecBoardDocument, String> {
    let settings_snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    spec_board_open_inner(
        state.inner().clone(),
        settings_snapshot,
        app,
        PathBuf::from(path),
    )
    .await
}

#[tauri::command]
pub async fn spec_board_save(
    state: State<'_, SpecBoardState>,
    settings: State<'_, SettingsState>,
    app: AppHandle,
    doc_id: String,
    content: String,
) -> Result<SpecBoardDocument, String> {
    let path = {
        let mgr = state.read().await;
        mgr.get_document(&doc_id)?
            .path
            .ok_or_else(|| "Spec board document has no save path".to_string())?
    };
    let settings_snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    save_to_path(
        state.inner().clone(),
        settings_snapshot,
        app,
        doc_id,
        content,
        PathBuf::from(path),
    )
    .await
}

#[tauri::command]
pub async fn spec_board_pick_save(
    state: State<'_, SpecBoardState>,
    settings: State<'_, SettingsState>,
    app: AppHandle,
    doc_id: String,
    content: String,
    repo_root: Option<String>,
) -> Result<Option<SpecBoardDocument>, String> {
    let settings_snapshot = settings
        .current_snapshot()
        .await
        .map_err(|error| error.to_string())?;
    let repo = resolve_repo_root(repo_root.as_deref(), &settings_snapshot).await?;
    let default_dir = repo.join("specs").join("mermaids");
    tokio::fs::create_dir_all(&default_dir)
        .await
        .map_err(|e| format!("Failed to create default spec directory: {}", e))?;
    let picked = rfd::AsyncFileDialog::new()
        .add_filter("Spec diagrams", &["mmd", "mermaid", "md"])
        .set_directory(&default_dir)
        .set_file_name("untitled.mmd")
        .save_file()
        .await;

    let Some(handle) = picked else {
        return Ok(None);
    };

    save_to_path(
        state.inner().clone(),
        settings_snapshot,
        app,
        doc_id,
        content,
        handle.path().to_path_buf(),
    )
    .await
    .map(Some)
}

#[tauri::command]
pub async fn spec_board_update_content(
    state: State<'_, SpecBoardState>,
    doc_id: String,
    content: String,
) -> Result<SpecBoardDocument, String> {
    let file_kind = {
        let mgr = state.read().await;
        mgr.get_document(&doc_id)?.file_kind
    };
    let diagram_source = extract_diagram_source(&content, file_kind)?;
    let hash = hash_str(&content);

    {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(&doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        if doc.document.content == content {
            return Ok(doc.document.clone());
        }
        doc.document.content = content;
        doc.document.diagram_source = diagram_source;
        doc.document.dirty = true;
        doc.document.updated_at_ms = now_ms();
        abort_handle(doc.edit_snapshot_handle.take());
    }

    schedule_edit_snapshot(state.inner().clone(), doc_id.clone(), hash).await;
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

#[tauri::command]
pub async fn spec_board_list_snapshots(
    state: State<'_, SpecBoardState>,
    doc_id: String,
) -> Result<Vec<SpecBoardSnapshot>, String> {
    let snapshot_dir = {
        let mgr = state.read().await;
        mgr.docs
            .get(&doc_id)
            .map(|s| s.snapshot_dir.clone())
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?
    };
    list_snapshots_from_dir(&snapshot_dir).await
}

#[tauri::command]
pub async fn spec_board_checkout_snapshot(
    state: State<'_, SpecBoardState>,
    doc_id: String,
    snapshot_id: String,
) -> Result<SpecBoardDocument, String> {
    let snapshot_dir = {
        let mgr = state.read().await;
        mgr.docs
            .get(&doc_id)
            .map(|s| s.snapshot_dir.clone())
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?
    };
    let snapshots = read_snapshot_files(&snapshot_dir).await?;
    let snapshot = snapshots
        .into_iter()
        .find(|s| s.id == snapshot_id)
        .ok_or_else(|| format!("Snapshot not found: {}", snapshot_id))?;

    {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(&doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        doc.document.content = snapshot.content;
        doc.document.diagram_source = snapshot.diagram_source;
        doc.document.dirty = doc
            .disk_hash
            .as_ref()
            .is_none_or(|disk| *disk != hash_str(&doc.document.content));
        doc.document.conflict = None;
        doc.document.updated_at_ms = now_ms();
    }

    create_snapshot_for_doc(
        state.inner().clone(),
        &doc_id,
        SpecBoardSnapshotSource::Checkout,
    )
    .await?;
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

#[tauri::command]
pub async fn spec_board_apply_external(
    state: State<'_, SpecBoardState>,
    doc_id: String,
) -> Result<SpecBoardDocument, String> {
    {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(&doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        let conflict = doc
            .document
            .conflict
            .take()
            .ok_or_else(|| "No pending external content to apply".to_string())?;
        let pending_hash = hash_str(&conflict.pending_external_content);
        doc.document.content = conflict.pending_external_content;
        doc.document.diagram_source = conflict.pending_external_diagram_source;
        doc.document.dirty = doc
            .disk_hash
            .as_ref()
            .is_none_or(|disk_hash| *disk_hash != pending_hash);
        doc.document.updated_at_ms = now_ms();
    }
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

#[tauri::command]
pub async fn spec_board_keep_mine(
    state: State<'_, SpecBoardState>,
    doc_id: String,
) -> Result<SpecBoardDocument, String> {
    let mut mgr = state.write().await;
    let doc = mgr
        .docs
        .get_mut(&doc_id)
        .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
    doc.document.conflict = None;
    doc.document.updated_at_ms = now_ms();
    Ok(doc.document.clone())
}

#[tauri::command]
pub async fn spec_board_close(
    state: State<'_, SpecBoardState>,
    doc_id: String,
) -> Result<(), String> {
    let mut mgr = state.write().await;
    mgr.close(&doc_id);
    Ok(())
}

pub async fn spec_board_close_all(state: SpecBoardState) {
    let mut mgr = state.write().await;
    mgr.close_all();
}

async fn spec_board_open_inner(
    state: SpecBoardState,
    settings: AppSettings,
    app: AppHandle,
    input_path: PathBuf,
) -> Result<SpecBoardDocument, String> {
    let validated = validate_existing_user_path(&input_path, &settings).await?;
    let metadata = tokio::fs::metadata(&validated.path)
        .await
        .map_err(|e| format!("Failed to inspect spec file: {}", e))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "Spec file is too large. Maximum size is {} bytes.",
            MAX_FILE_BYTES
        ));
    }
    let bytes = tokio::fs::read(&validated.path)
        .await
        .map_err(|e| format!("Failed to read spec file: {}", e))?;
    let content =
        String::from_utf8(bytes).map_err(|_| "Spec file must be valid UTF-8".to_string())?;
    let file_kind = file_kind_for_path(&validated.path)?;
    let diagram_source = extract_diagram_source(&content, file_kind)?;
    let doc_id = uuid::Uuid::new_v4().to_string();
    let (snapshot_dir, draft_dir) = internal_dirs(&validated.repo_root, &doc_id).await?;
    tokio::fs::create_dir_all(&snapshot_dir)
        .await
        .map_err(|e| format!("Failed to create snapshot directory: {}", e))?;
    tokio::fs::create_dir_all(&draft_dir)
        .await
        .map_err(|e| format!("Failed to create draft directory: {}", e))?;
    let disk_hash = hash_str(&content);
    let document = SpecBoardDocument {
        doc_id: doc_id.clone(),
        repo_root: path_to_string(&validated.repo_root),
        path: Some(path_to_string(&validated.path)),
        file_kind,
        content,
        diagram_source,
        dirty: false,
        conflict: None,
        version_index: 0,
        version_count: 0,
        updated_at_ms: now_ms(),
    };
    {
        let mut mgr = state.write().await;
        mgr.upsert_document(document.clone(), Some(disk_hash), snapshot_dir, draft_dir);
    }
    create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Open).await?;
    start_watch_task(state.clone(), app, doc_id.clone()).await;
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

async fn save_to_path(
    state: SpecBoardState,
    settings: AppSettings,
    app: AppHandle,
    doc_id: String,
    content: String,
    input_path: PathBuf,
) -> Result<SpecBoardDocument, String> {
    let validated = validate_new_or_existing_user_path(&input_path, &settings).await?;
    let file_kind = file_kind_for_path(&validated.path)?;
    let diagram_source = extract_diagram_source(&content, file_kind)?;
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "Spec content is too large. Maximum size is {} bytes.",
            MAX_FILE_BYTES
        ));
    }
    if let Some(parent) = validated.path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create spec directory: {}", e))?;
    }
    atomic_write_utf8(&validated.path, &content).await?;
    let final_bytes = tokio::fs::read(&validated.path)
        .await
        .map_err(|e| format!("Failed to read saved spec file: {}", e))?;
    let final_content =
        String::from_utf8(final_bytes).map_err(|_| "Saved spec file is not UTF-8".to_string())?;
    let final_hash = hash_str(&final_content);

    {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(&doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        doc.document.repo_root = path_to_string(&validated.repo_root);
        doc.document.path = Some(path_to_string(&validated.path));
        doc.document.file_kind = file_kind;
        doc.document.content = final_content;
        doc.document.diagram_source = diagram_source;
        doc.document.dirty = false;
        doc.document.conflict = None;
        doc.document.updated_at_ms = now_ms();
        doc.disk_hash = Some(final_hash.clone());
        doc.app_write_hash = Some(final_hash);
    }

    create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Save).await?;
    start_watch_task(state.clone(), app, doc_id.clone()).await;
    let mgr = state.read().await;
    mgr.get_document(&doc_id)
}

async fn start_watch_task(state: SpecBoardState, app: AppHandle, doc_id: String) {
    let path = {
        let mut mgr = state.write().await;
        let Some(doc) = mgr.docs.get_mut(&doc_id) else {
            return;
        };
        abort_handle(doc.watch_handle.take());
        let Some(path) = doc.document.path.clone() else {
            return;
        };
        PathBuf::from(path)
    };

    let state_for_task = state.clone();
    let doc_id_for_task = doc_id.clone();
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(WATCH_POLL_MS));
        let mut last_meta = metadata_fingerprint(&path).await.ok();
        let mut last_full_hash_at = now_ms();
        loop {
            interval.tick().await;
            let now = now_ms();
            let meta = metadata_fingerprint(&path).await.ok();
            let needs_hash = meta != last_meta || now - last_full_hash_at >= WATCH_FULL_HASH_MS;
            if !needs_hash {
                continue;
            }
            last_meta = meta;
            last_full_hash_at = now;

            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    let payload = SpecBoardFileMissingEvent {
                        doc_id: doc_id_for_task.clone(),
                        path: path_to_string(&path),
                    };
                    let _ = app.emit("spec_board_file_missing", payload);
                    continue;
                }
            };
            if bytes.len() as u64 > MAX_FILE_BYTES {
                log::warn!(
                    "[spec-board] watched file exceeds size cap: {}",
                    path.display()
                );
                continue;
            }
            let Ok(content) = String::from_utf8(bytes) else {
                log::warn!("[spec-board] watched file is not UTF-8: {}", path.display());
                continue;
            };
            let hash = hash_str(&content);
            let maybe_event =
                handle_external_content(state_for_task.clone(), &doc_id_for_task, content, hash)
                    .await;
            match maybe_event {
                Ok(Some(WatchEvent::Changed(payload))) => {
                    let _ = app.emit("spec_board_changed", payload);
                }
                Ok(Some(WatchEvent::Conflict(payload))) => {
                    let _ = app.emit("spec_board_conflict", payload);
                }
                Ok(None) => {}
                Err(e) => log::warn!("[spec-board] watcher update failed: {}", e),
            }
        }
    });

    let mut mgr = state.write().await;
    if let Some(doc) = mgr.docs.get_mut(&doc_id) {
        abort_handle(doc.watch_handle.replace(handle));
    } else {
        handle.abort();
    }
}

enum WatchEvent {
    Changed(SpecBoardChangedEvent),
    Conflict(SpecBoardDocument),
}

async fn handle_external_content(
    state: SpecBoardState,
    doc_id: &str,
    content: String,
    hash: String,
) -> Result<Option<WatchEvent>, String> {
    let mut snapshot_needed = false;
    let result;

    {
        let mut mgr = state.write().await;
        let Some(doc) = mgr.docs.get_mut(doc_id) else {
            return Ok(None);
        };
        if doc.disk_hash.as_ref() == Some(&hash) {
            return Ok(None);
        }
        if doc.app_write_hash.as_ref() == Some(&hash) {
            doc.disk_hash = Some(hash);
            doc.app_write_hash = None;
            return Ok(None);
        }

        let file_kind = doc.document.file_kind;
        let diagram_source = extract_diagram_source(&content, file_kind)?;
        let path = doc.document.path.clone().unwrap_or_default();
        doc.disk_hash = Some(hash.clone());
        doc.app_write_hash = None;

        if doc.last_external_snapshot_hash.as_ref() != Some(&hash) {
            doc.last_external_snapshot_hash = Some(hash);
            snapshot_needed = true;
        }

        if doc.document.dirty {
            doc.document.conflict = Some(SpecBoardConflict {
                path: path.clone(),
                pending_external_content: content,
                pending_external_diagram_source: diagram_source,
                detected_at_ms: now_ms(),
            });
            result = Some(WatchEvent::Conflict(doc.document.clone()));
        } else {
            doc.document.content = content;
            doc.document.diagram_source = diagram_source;
            doc.document.conflict = None;
            doc.document.updated_at_ms = now_ms();
            result = Some(WatchEvent::Changed(SpecBoardChangedEvent {
                doc_id: doc_id.to_string(),
                path: doc.document.path.clone(),
                content: doc.document.content.clone(),
                diagram_source: doc.document.diagram_source.clone(),
                version_index: doc.document.version_index,
                version_count: doc.document.version_count,
                external: true,
            }));
        }
    }

    if snapshot_needed {
        create_snapshot_for_doc(state, doc_id, SpecBoardSnapshotSource::External).await?;
    }
    Ok(result)
}

async fn schedule_edit_snapshot(state: SpecBoardState, doc_id: String, content_hash: String) {
    let state_for_task = state.clone();
    let doc_id_for_task = doc_id.clone();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(750)).await;
        let should_snapshot = {
            let mgr = state_for_task.read().await;
            mgr.docs
                .get(&doc_id_for_task)
                .is_some_and(|s| s.last_edit_snapshot_hash.as_ref() != Some(&content_hash))
        };
        if should_snapshot {
            if let Err(e) = create_snapshot_for_doc(
                state_for_task.clone(),
                &doc_id_for_task,
                SpecBoardSnapshotSource::Edit,
            )
            .await
            {
                log::warn!("[spec-board] edit snapshot failed: {}", e);
            } else {
                let mut mgr = state_for_task.write().await;
                if let Some(doc) = mgr.docs.get_mut(&doc_id_for_task) {
                    doc.last_edit_snapshot_hash = Some(content_hash);
                }
            }
        }
    });

    let mut mgr = state.write().await;
    if let Some(doc) = mgr.docs.get_mut(&doc_id) {
        abort_handle(doc.edit_snapshot_handle.replace(handle));
    } else {
        handle.abort();
    }
}

fn file_kind_for_path(path: &Path) -> Result<SpecBoardFileKind, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mmd" | "mermaid") => Ok(SpecBoardFileKind::Mermaid),
        Some("md") => Ok(SpecBoardFileKind::Markdown),
        _ => Err("Spec board supports only .mmd, .mermaid, and .md files".to_string()),
    }
}

fn extract_diagram_source(content: &str, file_kind: SpecBoardFileKind) -> Result<String, String> {
    match file_kind {
        SpecBoardFileKind::Mermaid => Ok(content.to_string()),
        SpecBoardFileKind::Markdown => extract_markdown_mermaid(content),
    }
}

fn extract_markdown_mermaid(content: &str) -> Result<String, String> {
    let mut blocks = Vec::new();
    let mut in_fence: Option<(char, usize, bool, usize)> = None;
    let mut offset = 0usize;

    for line in content.split_inclusive('\n') {
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed_start = line_without_newline.trim_start();
        if let Some((marker, len, is_mermaid, start)) = in_fence {
            let closing = fence_len(trimmed_start, marker);
            if closing >= len {
                if is_mermaid {
                    let end = offset;
                    blocks.push(content[start..end].to_string());
                }
                in_fence = None;
            }
            offset += line.len();
            continue;
        }

        let backtick_len = fence_len(trimmed_start, '`');
        let tilde_len = fence_len(trimmed_start, '~');
        let (marker, len) = if backtick_len >= 3 {
            ('`', backtick_len)
        } else if tilde_len >= 3 {
            ('~', tilde_len)
        } else {
            offset += line.len();
            continue;
        };
        let rest = trimmed_start[len..].trim();
        let token = rest.split_whitespace().next().unwrap_or("");
        let is_mermaid = token.eq_ignore_ascii_case("mermaid");
        let content_start = offset + line.len();
        in_fence = Some((marker, len, is_mermaid, content_start));
        offset += line.len();
    }

    if in_fence.is_some() {
        return Err("Unterminated fenced code block in Markdown spec".to_string());
    }
    if blocks.len() != 1 {
        return Err("MVP supports exactly one Mermaid diagram per active file.".to_string());
    }
    Ok(blocks.remove(0))
}

fn fence_len(s: &str, marker: char) -> usize {
    s.chars().take_while(|c| *c == marker).count()
}

async fn atomic_write_utf8(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Spec path has no parent directory".to_string())?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("spec-board"),
        uuid::Uuid::new_v4()
    ));
    tokio::fs::write(&temp, content)
        .await
        .map_err(|e| format!("Failed to write temporary spec file: {}", e))?;
    commit_temp_file(&temp, path, "spec file").await?;
    Ok(())
}

async fn commit_temp_file(temp: &Path, target: &Path, label: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        replace_file_with_temp(temp, target)
            .map_err(|e| format!("Failed to commit {}: {}", label, e))?;
    }

    #[cfg(not(windows))]
    {
        tokio::fs::rename(temp, target)
            .await
            .map_err(|e| format!("Failed to commit {}: {}", label, e))?;
    }

    Ok(())
}

#[cfg(windows)]
fn replace_file_with_temp(temp: &Path, target: &Path) -> std::io::Result<()> {
    let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let target_wide: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;

    let replaced = unsafe { MoveFileExW(temp_wide.as_ptr(), target_wide.as_ptr(), flags) };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

async fn metadata_fingerprint(path: &Path) -> Result<(u64, Option<std::time::SystemTime>), String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("Failed to inspect watched file: {}", e))?;
    Ok((meta.len(), meta.modified().ok()))
}

fn abort_handle(handle: Option<JoinHandle<()>>) {
    if let Some(handle) = handle {
        handle.abort();
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn hash_str(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hash_prefix(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests;
