use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};
use tokio::task::JoinHandle;

use crate::config::settings::{AppSettings, SettingsState};
use crate::SpecBoardState;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecBoardFileKind {
    Mermaid,
    Markdown,
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
pub struct SpecBoardSnapshot {
    pub id: String,
    pub label: String,
    pub created_at_ms: i64,
    pub source: SpecBoardSnapshotSource,
    pub content: String,
    pub diagram_source: String,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpecBoardSnapshotSource {
    Initial,
    Open,
    Edit,
    Save,
    External,
    Checkout,
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
pub struct SpecBoardConflictEvent {
    pub doc_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecBoardFileMissingEvent {
    pub doc_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotFile {
    id: String,
    label: String,
    created_at_ms: i64,
    sequence: u64,
    source: SpecBoardSnapshotSource,
    content: String,
    diagram_source: String,
    file_kind: SpecBoardFileKind,
    path: Option<String>,
    hash: String,
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
    let settings_snapshot = settings.read().await.clone();
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
    let settings_snapshot = settings.read().await.clone();
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
    let settings_snapshot = settings.read().await.clone();
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
    let settings_snapshot = settings.read().await.clone();
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
    let settings_snapshot = settings.read().await.clone();
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
    Conflict(SpecBoardConflictEvent),
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
            result = Some(WatchEvent::Conflict(SpecBoardConflictEvent {
                doc_id: doc_id.to_string(),
                path,
            }));
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

async fn create_snapshot_for_doc(
    state: SpecBoardState,
    doc_id: &str,
    source: SpecBoardSnapshotSource,
) -> Result<(), String> {
    let (snapshot_dir, snapshot, skip_for_size) = {
        let mut mgr = state.write().await;
        let doc = mgr
            .docs
            .get_mut(doc_id)
            .ok_or_else(|| format!("Spec board document not found: {}", doc_id))?;
        if doc.document.content.len() > MAX_SNAPSHOT_CONTENT_BYTES {
            log::warn!(
                "[spec-board] skipping oversized snapshot for {}",
                doc.document.doc_id
            );
            (doc.snapshot_dir.clone(), None, true)
        } else {
            doc.snapshot_sequence += 1;
            let created_at_ms = now_ms();
            let hash = hash_str(&doc.document.content);
            let id = format!(
                "{}-{}-{}",
                created_at_ms,
                doc.snapshot_sequence,
                hash_prefix(&hash)
            );
            let label = format!("{:?} {}", source, doc.snapshot_sequence);
            let snapshot = SnapshotFile {
                id,
                label,
                created_at_ms,
                sequence: doc.snapshot_sequence,
                source,
                content: doc.document.content.clone(),
                diagram_source: doc.document.diagram_source.clone(),
                file_kind: doc.document.file_kind,
                path: doc.document.path.clone(),
                hash,
            };
            (doc.snapshot_dir.clone(), Some(snapshot), false)
        }
    };

    if skip_for_size {
        return Ok(());
    }
    let snapshot = snapshot.expect("snapshot present when not skipped");
    tokio::fs::create_dir_all(&snapshot_dir)
        .await
        .map_err(|e| format!("Failed to create snapshot directory: {}", e))?;
    write_snapshot_atomic(&snapshot_dir, &snapshot).await?;
    prune_snapshots(&snapshot_dir).await?;

    let snapshots = list_snapshots_from_dir(&snapshot_dir).await?;
    let mut mgr = state.write().await;
    if let Some(doc) = mgr.docs.get_mut(doc_id) {
        doc.document.version_count = snapshots.len();
        doc.document.version_index = snapshots.len().saturating_sub(1);
    }
    Ok(())
}

async fn write_snapshot_atomic(dir: &Path, snapshot: &SnapshotFile) -> Result<(), String> {
    let source = format!("{:?}", snapshot.source).to_ascii_lowercase();
    let filename = format!(
        "{}-{:06}-{}.json",
        snapshot.created_at_ms, snapshot.sequence, source
    );
    let target = dir.join(filename);
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        snapshot.created_at_ms,
        uuid::Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|e| format!("Failed to serialize snapshot: {}", e))?;
    tokio::fs::write(&temp, bytes)
        .await
        .map_err(|e| format!("Failed to write snapshot: {}", e))?;
    tokio::fs::rename(&temp, &target)
        .await
        .map_err(|e| format!("Failed to commit snapshot: {}", e))?;
    Ok(())
}

async fn list_snapshots_from_dir(dir: &Path) -> Result<Vec<SpecBoardSnapshot>, String> {
    let files = read_snapshot_files(dir).await?;
    Ok(files
        .into_iter()
        .map(|s| SpecBoardSnapshot {
            id: s.id,
            label: s.label,
            created_at_ms: s.created_at_ms,
            source: s.source,
            content: s.content,
            diagram_source: s.diagram_source,
            hash: s.hash,
        })
        .collect())
}

async fn read_snapshot_files(dir: &Path) -> Result<Vec<SnapshotFile>, String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    let mut files = Vec::new();
    let mut corrupt = 0usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<SnapshotFile>(&bytes) {
                Ok(snapshot) => files.push(snapshot),
                Err(e) => {
                    corrupt += 1;
                    log::warn!(
                        "[spec-board] skipping corrupt snapshot {}: {}",
                        path.display(),
                        e
                    );
                }
            },
            Err(e) => {
                corrupt += 1;
                log::warn!(
                    "[spec-board] skipping unreadable snapshot {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
    if corrupt > 0 {
        log::warn!("[spec-board] skipped {} corrupt snapshot(s)", corrupt);
    }
    files.sort_by_key(|s| (s.created_at_ms, s.sequence));
    Ok(files)
}

async fn prune_snapshots(dir: &Path) -> Result<(), String> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to read snapshot directory: {}", e)),
    };
    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("Failed to read snapshot entry: {}", e))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let metadata = entry
            .metadata()
            .await
            .map_err(|e| format!("Failed to read snapshot metadata: {}", e))?;
        files.push((path, metadata.len(), metadata.modified().ok()));
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    while files.len() > MAX_SNAPSHOTS || total > MAX_SNAPSHOT_TOTAL_BYTES {
        let (path, len, _) = files.remove(0);
        if let Err(e) = tokio::fs::remove_file(&path).await {
            log::warn!(
                "[spec-board] failed to prune snapshot {}: {}",
                path.display(),
                e
            );
            break;
        }
        total = total.saturating_sub(len);
    }
    Ok(())
}

#[derive(Clone)]
struct ValidatedPath {
    path: PathBuf,
    repo_root: PathBuf,
}

async fn resolve_repo_root(
    requested: Option<&str>,
    settings: &AppSettings,
) -> Result<PathBuf, String> {
    let roots = valid_repo_roots(settings).await?;
    if let Some(requested) = requested.filter(|s| !s.trim().is_empty()) {
        let requested_path = PathBuf::from(requested);
        let canonical = canonicalize_existing_dir(&requested_path).await?;
        if roots
            .iter()
            .any(|root| path_compare_key(root) == path_compare_key(&canonical))
        {
            return Ok(canonical);
        }
        if is_valid_repo_root(&canonical).await? {
            return Ok(canonical);
        }
        return Err(format!(
            "Selected path is not a valid repository root: {}",
            requested
        ));
    }
    roots
        .into_iter()
        .next()
        .ok_or_else(|| "No valid repo-* root is loaded for the spec board".to_string())
}

async fn valid_repo_roots(settings: &AppSettings) -> Result<Vec<PathBuf>, String> {
    let mut candidates = Vec::new();
    if let Some(path) = &settings.project_path {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(settings.project_paths.iter().map(PathBuf::from));

    let mut roots = Vec::new();
    for candidate in candidates {
        let Ok(canonical) = canonicalize_existing_dir(&candidate).await else {
            continue;
        };
        if is_valid_repo_root(&canonical).await.unwrap_or(false) {
            push_unique_path(&mut roots, canonical.clone());
        }
        if let Ok(mut entries) = tokio::fs::read_dir(&canonical).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || !name.starts_with("repo-") {
                    continue;
                }
                let Ok(child) = canonicalize_existing_dir(&path).await else {
                    continue;
                };
                if is_valid_repo_root(&child).await.unwrap_or(false) {
                    push_unique_path(&mut roots, child);
                }
            }
        }
    }
    roots.sort_by_key(|p| path_compare_key(p));
    Ok(roots)
}

async fn validate_existing_user_path(
    path: &Path,
    settings: &AppSettings,
) -> Result<ValidatedPath, String> {
    validate_user_path(path, settings, true).await
}

async fn validate_new_or_existing_user_path(
    path: &Path,
    settings: &AppSettings,
) -> Result<ValidatedPath, String> {
    validate_user_path(path, settings, false).await
}

async fn validate_user_path(
    path: &Path,
    settings: &AppSettings,
    must_exist: bool,
) -> Result<ValidatedPath, String> {
    reject_unsupported_extension(path)?;
    reject_ac_component(path)?;
    if must_exist {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("Spec path does not exist or is inaccessible: {}", e))?;
        if meta.is_dir() {
            return Err("Spec path points to a directory".to_string());
        }
    }

    let roots = valid_repo_roots(settings).await?;
    if roots.is_empty() {
        return Err("No valid repo-* root is loaded for the spec board".to_string());
    }

    let nearest = nearest_existing_parent(path).await?;
    let canonical_parent = canonicalize_existing_dir(&nearest).await?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "Spec path must include a file name".to_string())?;
    let canonical_candidate = if must_exist && path.exists() {
        tokio::fs::canonicalize(path)
            .await
            .map_err(|e| format!("Failed to canonicalize spec path: {}", e))?
    } else if canonical_parent == path {
        canonical_parent.clone()
    } else {
        canonical_parent.join(file_name)
    };

    for root in roots {
        if path_is_under(&canonical_candidate, &root) {
            reject_link_ancestors(&root, &canonical_candidate).await?;
            return Ok(ValidatedPath {
                path: canonical_candidate,
                repo_root: root,
            });
        }
    }

    Err(format!(
        "Spec path is outside every valid repo root: {}",
        path.display()
    ))
}

async fn internal_dirs(repo_root: &Path, doc_id: &str) -> Result<(PathBuf, PathBuf), String> {
    let root = canonicalize_existing_dir(repo_root).await?;
    let base = root.join(".ac").join("mermaids");
    Ok((
        base.join("snapshots").join(doc_id),
        base.join("drafts").join(doc_id),
    ))
}

async fn is_valid_repo_root(path: &Path) -> Result<bool, String> {
    if !path.is_dir() {
        return Ok(false);
    }
    if is_link_or_reparse(path).await? {
        return Ok(false);
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    Ok(name.starts_with("repo-") || path.join(".git").exists())
}

async fn nearest_existing_parent(path: &Path) -> Result<PathBuf, String> {
    let mut current = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .ok_or_else(|| "Spec path has no parent directory".to_string())?
            .to_path_buf()
    };
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| "Spec path has no existing parent".to_string())?
            .to_path_buf();
    }
    if current.is_file() {
        current = current
            .parent()
            .ok_or_else(|| "Spec path parent could not be resolved".to_string())?
            .to_path_buf();
    }
    Ok(current)
}

async fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, String> {
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| format!("Failed to canonicalize directory {}: {}", path.display(), e))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| format!("Failed to inspect directory {}: {}", path.display(), e))?;
    if !metadata.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }
    Ok(canonical)
}

async fn reject_link_ancestors(root: &Path, target: &Path) -> Result<(), String> {
    let mut current = root.to_path_buf();
    if is_link_or_reparse(&current).await? {
        return Err(format!(
            "Repo root is a link or reparse point: {}",
            root.display()
        ));
    }

    let relative = target
        .parent()
        .unwrap_or(target)
        .strip_prefix(root)
        .map_err(|_| "Spec path is not under the repo root".to_string())?;
    for component in relative.components() {
        current.push(component.as_os_str());
        if current.exists() && is_link_or_reparse(&current).await? {
            return Err(format!(
                "Spec path contains a link or reparse point: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

async fn is_link_or_reparse(path: &Path) -> Result<bool, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to lstat {}: {}", path.display(), e))?;
        if meta.file_type().is_symlink() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|e| format!("Failed to inspect path link state: {}", e))?
}

fn reject_unsupported_extension(path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mmd" | "mermaid" | "md") => Ok(()),
        _ => Err("Spec board supports only .mmd, .mermaid, and .md files".to_string()),
    }
}

fn reject_ac_component(path: &Path) -> Result<(), String> {
    if path.components().any(|component| match component {
        Component::Normal(part) => part.to_string_lossy().eq_ignore_ascii_case(".ac"),
        _ => false,
    }) {
        return Err("User spec paths under .ac are not allowed".to_string());
    }
    Ok(())
}

fn path_is_under(path: &Path, root: &Path) -> bool {
    let path_key = path_compare_key(path);
    let root_key = path_compare_key(root);
    path_key == root_key
        || path_key
            .strip_prefix(&(root_key + std::path::MAIN_SEPARATOR_STR))
            .is_some()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path_compare_key(&path);
    if !paths.iter().any(|p| path_compare_key(p) == key) {
        paths.push(path);
    }
}

fn path_compare_key(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('/', "\\");
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{}", stripped);
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    while s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
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
    tokio::fs::rename(&temp, path)
        .await
        .map_err(|e| format!("Failed to commit spec file: {}", e))?;
    Ok(())
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
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_settings(paths: Vec<PathBuf>) -> AppSettings {
        AppSettings {
            project_paths: paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            ..AppSettings::default()
        }
    }

    async fn test_state_with_doc(
        tmp: &TempDir,
        content: &str,
        dirty: bool,
    ) -> (SpecBoardState, String, PathBuf) {
        let repo = tmp.path().join("repo-Example");
        let path = repo.join("specs").join("mermaids").join("diagram.mmd");
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, content).await.unwrap();
        let doc_id = uuid::Uuid::new_v4().to_string();
        let snapshot_dir = repo
            .join(".ac")
            .join("mermaids")
            .join("snapshots")
            .join(&doc_id);
        let draft_dir = repo
            .join(".ac")
            .join("mermaids")
            .join("drafts")
            .join(&doc_id);
        tokio::fs::create_dir_all(&snapshot_dir).await.unwrap();
        tokio::fs::create_dir_all(&draft_dir).await.unwrap();
        let document = SpecBoardDocument {
            doc_id: doc_id.clone(),
            repo_root: path_to_string(&repo),
            path: Some(path_to_string(&path)),
            file_kind: SpecBoardFileKind::Mermaid,
            content: content.to_string(),
            diagram_source: content.to_string(),
            dirty,
            conflict: None,
            version_index: 0,
            version_count: 0,
            updated_at_ms: now_ms(),
        };
        let mut manager = SpecBoardManager::new();
        manager.upsert_document(document, Some(hash_str(content)), snapshot_dir, draft_dir);
        (Arc::new(tokio::sync::RwLock::new(manager)), doc_id, path)
    }

    #[test]
    fn mermaid_files_parse_as_full_source() {
        let src = "flowchart TD\nA-->B\n";
        assert_eq!(
            extract_diagram_source(src, SpecBoardFileKind::Mermaid).unwrap(),
            src
        );
    }

    #[test]
    fn markdown_with_one_mermaid_block_parses() {
        let src = "# Spec\n\n```mermaid title\nflowchart TD\nA-->B\n```\n";
        assert_eq!(
            extract_diagram_source(src, SpecBoardFileKind::Markdown).unwrap(),
            "flowchart TD\nA-->B\n"
        );
    }

    #[test]
    fn markdown_rejects_zero_or_multiple_mermaid_blocks() {
        assert!(
            extract_diagram_source("```rust\nmermaid\n```\n", SpecBoardFileKind::Markdown).is_err()
        );
        assert!(extract_diagram_source(
            "```mermaid\nA\n```\n```mermaid\nB\n```\n",
            SpecBoardFileKind::Markdown
        )
        .is_err());
    }

    #[test]
    fn markdown_supports_tilde_longer_uppercase_and_info() {
        assert_eq!(
            extract_diagram_source("~~~~MERMAID title\nA\n~~~~\n", SpecBoardFileKind::Markdown)
                .unwrap(),
            "A\n"
        );
        assert_eq!(
            extract_diagram_source("````mermaid\nB\n````\n", SpecBoardFileKind::Markdown).unwrap(),
            "B\n"
        );
    }

    #[test]
    fn markdown_rejects_unterminated_fence() {
        assert!(extract_diagram_source("```mermaid\nA\n", SpecBoardFileKind::Markdown).is_err());
    }

    #[tokio::test]
    async fn path_validation_accepts_repo_specs_mermaid() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo-Example");
        tokio::fs::create_dir_all(repo.join("specs").join("mermaids"))
            .await
            .unwrap();
        let settings = test_settings(vec![tmp.path().to_path_buf()]);
        let target = repo.join("specs").join("mermaids").join("diagram.mmd");
        let validated = validate_new_or_existing_user_path(&target, &settings)
            .await
            .unwrap();
        assert_eq!(
            path_compare_key(&validated.repo_root),
            path_compare_key(&repo)
        );
    }

    #[tokio::test]
    async fn path_validation_rejects_ac_and_outside_paths() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo-Example");
        tokio::fs::create_dir_all(&repo).await.unwrap();
        let settings = test_settings(vec![tmp.path().to_path_buf()]);
        assert!(
            validate_new_or_existing_user_path(&repo.join(".ac/_agent_x/Role.md"), &settings)
                .await
                .is_err()
        );
        assert!(validate_new_or_existing_user_path(
            &repo.join(".ac/wg-1-dev-team/messaging/file.md"),
            &settings
        )
        .await
        .is_err());
        assert!(
            validate_new_or_existing_user_path(&tmp.path().join("outside.mmd"), &settings)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn path_validation_rejects_extension_and_traversal() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo-Example");
        tokio::fs::create_dir_all(&repo).await.unwrap();
        let settings = test_settings(vec![tmp.path().to_path_buf()]);
        assert!(
            validate_new_or_existing_user_path(&repo.join("bad.txt"), &settings)
                .await
                .is_err()
        );
        assert!(
            validate_new_or_existing_user_path(&repo.join("..").join("escape.mmd"), &settings)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn corrupt_snapshot_does_not_break_listing() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("snapshots");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("bad.json"), "{").await.unwrap();
        let snap = SnapshotFile {
            id: "1".into(),
            label: "Initial 1".into(),
            created_at_ms: 1,
            sequence: 1,
            source: SpecBoardSnapshotSource::Initial,
            content: "flowchart TD\nA-->B\n".into(),
            diagram_source: "flowchart TD\nA-->B\n".into(),
            file_kind: SpecBoardFileKind::Mermaid,
            path: None,
            hash: hash_str("flowchart TD\nA-->B\n"),
        };
        write_snapshot_atomic(&dir, &snap).await.unwrap();
        let listed = list_snapshots_from_dir(&dir).await.unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn snapshot_collision_does_not_overwrite() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("snapshots");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let mut snap = SnapshotFile {
            id: "same".into(),
            label: "Initial".into(),
            created_at_ms: 1,
            sequence: 1,
            source: SpecBoardSnapshotSource::Initial,
            content: "A".into(),
            diagram_source: "A".into(),
            file_kind: SpecBoardFileKind::Mermaid,
            path: None,
            hash: hash_str("A"),
        };
        write_snapshot_atomic(&dir, &snap).await.unwrap();
        snap.sequence = 2;
        snap.content = "B".into();
        snap.hash = hash_str("B");
        write_snapshot_atomic(&dir, &snap).await.unwrap();
        let listed = list_snapshots_from_dir(&dir).await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[tokio::test]
    async fn clean_external_update_replaces_content() {
        let tmp = TempDir::new().unwrap();
        let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", false).await;
        let external = "flowchart TD\nA-->C\n".to_string();
        let event = handle_external_content(
            state.clone(),
            &doc_id,
            external.clone(),
            hash_str(&external),
        )
        .await
        .unwrap();
        assert!(matches!(event, Some(WatchEvent::Changed(_))));
        let mgr = state.read().await;
        let doc = mgr.get_document(&doc_id).unwrap();
        assert_eq!(doc.content, external);
        assert!(doc.conflict.is_none());
        assert!(!doc.dirty);
    }

    #[tokio::test]
    async fn dirty_external_update_creates_conflict() {
        let tmp = TempDir::new().unwrap();
        let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
        let local = {
            let mut mgr = state.write().await;
            let doc = mgr.docs.get_mut(&doc_id).unwrap();
            doc.document.content = "flowchart TD\nA-->LOCAL\n".to_string();
            doc.document.content.clone()
        };
        let external = "flowchart TD\nA-->C\n".to_string();
        let event = handle_external_content(
            state.clone(),
            &doc_id,
            external.clone(),
            hash_str(&external),
        )
        .await
        .unwrap();
        assert!(matches!(event, Some(WatchEvent::Conflict(_))));
        let mgr = state.read().await;
        let doc = mgr.get_document(&doc_id).unwrap();
        assert_eq!(doc.content, local);
        assert_eq!(doc.conflict.unwrap().pending_external_content, external);
    }

    #[tokio::test]
    async fn checkout_snapshot_marks_document_dirty_without_disk_write() {
        let tmp = TempDir::new().unwrap();
        let (state, doc_id, path) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", false).await;
        {
            let mut mgr = state.write().await;
            let doc = mgr.docs.get_mut(&doc_id).unwrap();
            doc.document.content = "flowchart TD\nA-->C\n".to_string();
            doc.document.diagram_source = doc.document.content.clone();
        }
        create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Edit)
            .await
            .unwrap();
        let snapshot_id =
            list_snapshots_from_dir(&state.read().await.docs.get(&doc_id).unwrap().snapshot_dir)
                .await
                .unwrap()[0]
                .id
                .clone();
        let snapshot_dir = {
            let mgr = state.read().await;
            mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
        };
        let snapshots = read_snapshot_files(&snapshot_dir).await.unwrap();
        let snapshot = snapshots.into_iter().find(|s| s.id == snapshot_id).unwrap();
        {
            let mut mgr = state.write().await;
            let doc = mgr.docs.get_mut(&doc_id).unwrap();
            doc.document.content = snapshot.content;
            doc.document.diagram_source = snapshot.diagram_source;
            doc.document.dirty = doc
                .disk_hash
                .as_ref()
                .is_none_or(|disk| *disk != hash_str(&doc.document.content));
        }
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "flowchart TD\nA-->B\n"
        );
        assert!(state.read().await.get_document(&doc_id).unwrap().dirty);
    }

    #[tokio::test]
    async fn oversized_snapshot_is_skipped_without_losing_edits() {
        let tmp = TempDir::new().unwrap();
        let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
        let oversized = "A".repeat(MAX_SNAPSHOT_CONTENT_BYTES + 1);
        {
            let mut mgr = state.write().await;
            let doc = mgr.docs.get_mut(&doc_id).unwrap();
            doc.document.content = oversized.clone();
            doc.document.diagram_source = oversized.clone();
        }
        create_snapshot_for_doc(state.clone(), &doc_id, SpecBoardSnapshotSource::Edit)
            .await
            .unwrap();
        let snapshot_dir = {
            let mgr = state.read().await;
            mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
        };
        assert!(list_snapshots_from_dir(&snapshot_dir)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            state
                .read()
                .await
                .get_document(&doc_id)
                .unwrap()
                .content
                .len(),
            oversized.len()
        );
    }

    #[tokio::test]
    async fn concurrent_update_list_and_close_do_not_hang() {
        let tmp = TempDir::new().unwrap();
        let (state, doc_id, _) = test_state_with_doc(&tmp, "flowchart TD\nA-->B\n", true).await;
        let snapshot_dir = {
            let mgr = state.read().await;
            mgr.docs.get(&doc_id).unwrap().snapshot_dir.clone()
        };
        let update_state = state.clone();
        let update_id = doc_id.clone();
        let update = tokio::spawn(async move {
            {
                let mut mgr = update_state.write().await;
                let doc = mgr.docs.get_mut(&update_id).unwrap();
                doc.document.content = "flowchart TD\nA-->C\n".to_string();
                doc.document.diagram_source = doc.document.content.clone();
            }
            create_snapshot_for_doc(update_state, &update_id, SpecBoardSnapshotSource::Edit).await
        });
        let list = tokio::spawn(async move { list_snapshots_from_dir(&snapshot_dir).await });
        let close_state = state.clone();
        let close_id = doc_id.clone();
        let close = tokio::spawn(async move {
            let mut mgr = close_state.write().await;
            mgr.close(&close_id);
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            let _ = update.await;
            let _ = list.await;
            let _ = close.await;
        })
        .await
        .expect("concurrent operations hung");
    }
}
