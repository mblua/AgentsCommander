use super::super::{default_true, settings_path, AppSettings, OVERLAY_KEY_AGENTS};
use super::{registered_profile_raw_gate, AgentConfig, StartupAgentRows};
use crate::config::instance_artifacts::{
    BLOCKING_MENUS_LOCAL_FILE_NAME, BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME,
    BLOCKING_MENUS_REMOTE_FILE_NAME, BLOCKING_MENUS_SHIPPED_FILE_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// #1646 / #1647 - one entry of an agent's `blockingMenus` array, or whatever the user wrote there.
/// Untagged so `Invalid` catches malformed entries without failing settings deserialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BlockingMenuEntry {
    Valid(BlockingMenuConfig),
    /// Anything that did not deserialize as a `BlockingMenuConfig`. Skipped by evaluation.
    Invalid(serde_json::Value),
}

impl BlockingMenuEntry {
    pub fn valid(&self) -> Option<&BlockingMenuConfig> {
        match self {
            BlockingMenuEntry::Valid(config) => Some(config),
            BlockingMenuEntry::Invalid(_) => None,
        }
    }
}

/// #1646 / #1647 - one configured blocking menu pattern and notification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockingMenuConfig {
    pub pattern: String,
    pub notification: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_against: Option<String>,
}

/// #1757 - the Codex "Hooks need review" startup dialog. One literal, two consumers:
/// `apply_issue_1757_migration` (installs whose `blockingMenus` array was already materialized
/// against the older default) and test T1 `embedded_content_matches_the_rust_literals`, which
/// pins the embedded `settings-blocking-menus.json` to this constant so the two cannot drift.
pub(crate) const CODEX_HOOKS_REVIEW_PATTERN: &str = r"^[^A-Za-z0-9]*Hooks need review\b";

pub(crate) fn codex_hooks_review_menu() -> BlockingMenuConfig {
    BlockingMenuConfig {
        pattern: CODEX_HOOKS_REVIEW_PATTERN.to_string(),
        notification: "codex is waiting for you to answer the hooks-review menu in this terminal"
            .to_string(),
        enabled: true,
        captured_against: Some("codex 0.153.2 / Windows".to_string()),
    }
}

/// #1905 - the patterns AC ships, embedded at build time. The config-dir copy is a
/// materialization of this content; the runtime evaluates this constant, never the disk file.
pub(crate) const EMBEDDED_BLOCKING_MENUS_JSON: &str =
    include_str!("../../../../resources/blocking-menus/settings-blocking-menus.json");

pub const BLOCKING_MENUS_SCHEMA_VERSION: u32 = 1;

/// #1925 (D2) - the ref the URL pins; the cached note names it.
pub(crate) const REMOTE_BLOCKING_MENUS_SOURCE_REF: &str = "main";
/// #1925 (D5) - hard cap on the downloaded body, checked while reading and on acceptance.
pub(crate) const REMOTE_BLOCKING_MENUS_MAX_BYTES: usize = 64 * 1024;

/// #1925 (D6) - hard ceilings for the downloaded layer: 200 entries in total across
/// every `byCommand` array, a pattern of 512 bytes and a notification of 200 bytes.
pub(crate) const REMOTE_BLOCKING_MENUS_MAX_ENTRIES: usize = 200;
pub(crate) const REMOTE_PATTERN_MAX_BYTES: usize = 512;
pub(crate) const REMOTE_NOTIFICATION_MAX_BYTES: usize = 200;

/// #1925 (D6) - compile ceiling for one downloaded pattern. The menu guard keeps one
/// compiled copy per accepted pattern, so an accepted file adds at most
/// 200 x 256 KiB = 50 MiB of compiled programs, plus each regex's lazy-DFA cache at
/// the `regex` crate default. Reaching that ceiling needs repo-write access; the real
/// published file compiles to a few KiB.
pub(crate) const REMOTE_PATTERN_REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// #1925 (D7) - every accepted pattern must leave these rows alone. Matching is
/// `Regex::is_match`, unanchored, the way the guard matches, so a catch-all pattern
/// that would silently defer every injection is rejected before it reaches the guard.
/// Menu-like rows (`1. Yes`) are deliberately absent: a legitimate trust-menu pattern
/// may match them.
pub(crate) const REMOTE_BLOCKING_MENUS_BENIGN_ROWS: &[&str] = &[
    "",
    "   ",
    "$ ",
    "> ",
    r"PS C:\Users\dev\project> ",
    "dev@host:~/project$ ",
    "The quick brown fox jumps over the lazy dog.",
    "    at main (src/index.ts:10:5)",
    "thread 'main' panicked at src/main.rs:2:5:",
    "Traceback (most recent call last):",
    "error: could not compile `app` (bin \"app\") due to 1 previous error",
    "\u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}",
    "\u{2502}                \u{2502}",
];

pub(crate) fn default_blocking_menus_schema_version() -> u32 {
    BLOCKING_MENUS_SCHEMA_VERSION
}

/// #1905 - one blocking-menus file; the shipped file and the `.local` file share this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BlockingMenusFile {
    #[serde(default = "default_blocking_menus_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub by_command: BTreeMap<String, Vec<BlockingMenuEntry>>,
    #[serde(default)]
    pub by_agent: BTreeMap<String, Vec<BlockingMenuEntry>>,
}

impl Default for BlockingMenusFile {
    fn default() -> Self {
        Self {
            schema_version: BLOCKING_MENUS_SCHEMA_VERSION,
            note: None,
            by_command: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

/// #1905 (D6) - the one parser both files go through; the migration validates against it too.
/// The `Value` step rejects a JSON array, which serde would otherwise decode as an empty file.
pub(crate) fn parse_blocking_menus_file(contents: &str) -> Result<BlockingMenusFile, String> {
    let value =
        serde_json::from_str::<Value>(contents).map_err(|e| format!("does not parse: {e}"))?;
    if !value.is_object() {
        return Err("is not a JSON object".to_string());
    }
    let file = serde_json::from_value::<BlockingMenusFile>(value)
        .map_err(|e| format!("does not parse: {e}"))?;
    if file.schema_version != BLOCKING_MENUS_SCHEMA_VERSION {
        return Err(format!(
            "has schemaVersion {}; only {} is supported",
            file.schema_version, BLOCKING_MENUS_SCHEMA_VERSION
        ));
    }
    Ok(file)
}

/// #1925 (D4 to D7) - the whole-file gate for the downloaded cache. The first failed
/// check rejects the whole file, never one entry; `byAgent` is rejected because the
/// remote layer is `byCommand`-only. Checked on arrival and again at every read.
pub(crate) fn validate_remote_blocking_menus_file(
    contents: &str,
) -> Result<BlockingMenusFile, String> {
    let file = parse_blocking_menus_file(contents)?;
    if !file.by_agent.is_empty() {
        return Err("byAgent must be empty".to_string());
    }
    let total: usize = file.by_command.values().map(Vec::len).sum();
    if total > REMOTE_BLOCKING_MENUS_MAX_ENTRIES {
        return Err(format!(
            "holds {total} entries; more than {REMOTE_BLOCKING_MENUS_MAX_ENTRIES} entries are not \
                allowed"
        ));
    }
    for (stem, entries) in &file.by_command {
        for (index, entry) in entries.iter().enumerate() {
            let Some(config) = entry.valid() else {
                return Err(format!(
                    "entry {stem}[{index}]: is not a valid blocking-menu entry"
                ));
            };
            if config.pattern.len() > REMOTE_PATTERN_MAX_BYTES {
                return Err(format!(
                    "entry {stem}[{index}]: pattern longer than {REMOTE_PATTERN_MAX_BYTES} bytes"
                ));
            }
            if config.notification.len() > REMOTE_NOTIFICATION_MAX_BYTES {
                return Err(format!(
                    "entry {stem}[{index}]: notification longer than \
                        {REMOTE_NOTIFICATION_MAX_BYTES} bytes"
                ));
            }
            if config.notification.chars().any(char::is_control) {
                return Err(format!(
                    "entry {stem}[{index}]: notification contains a control character"
                ));
            }
            let regex = match regex::RegexBuilder::new(&config.pattern)
                .size_limit(REMOTE_PATTERN_REGEX_SIZE_LIMIT)
                .build()
            {
                Ok(regex) => regex,
                Err(e) => {
                    return Err(format!(
                        "entry {stem}[{index}]: pattern does not compile: {e}"
                    ));
                }
            };
            if regex.is_match("") {
                return Err(format!(
                    "entry {stem}[{index}]: pattern matches the empty string"
                ));
            }
            if REMOTE_BLOCKING_MENUS_BENIGN_ROWS
                .iter()
                .any(|row| regex.is_match(row))
            {
                return Err(format!(
                    "entry {stem}[{index}]: pattern matches a benign row"
                ));
            }
        }
    }
    Ok(file)
}

/// #1925 (D5) - accept a response only when the status, the byte size and the encoding
/// are all right; the body then goes through the same whole-file validator as the cache.
pub(crate) fn accept_remote_blocking_menus_response(
    status: u16,
    body: &[u8],
) -> Result<BlockingMenusFile, String> {
    if status != 200 {
        return Err(format!("HTTP status {status}"));
    }
    if body.len() > REMOTE_BLOCKING_MENUS_MAX_BYTES {
        return Err(format!(
            "body larger than {REMOTE_BLOCKING_MENUS_MAX_BYTES} bytes"
        ));
    }
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    validate_remote_blocking_menus_file(text)
}

/// A parse failure is a build defect T1 catches; production logs once and serves an empty file.
pub fn shipped_blocking_menus() -> &'static BlockingMenusFile {
    static SHIPPED: OnceLock<BlockingMenusFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        parse_blocking_menus_file(EMBEDDED_BLOCKING_MENUS_JSON).unwrap_or_else(|e| {
            log::error!("[blocking-menus] embedded settings-blocking-menus.json {e}");
            BlockingMenusFile::default()
        })
    })
}

/// #1646 / #1647 / #1905 - the shipped patterns for a command, by executable stem.
pub fn default_blocking_menus_for_command(command: &str) -> Vec<BlockingMenuEntry> {
    let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
    else {
        return Vec::new();
    };
    shipped_blocking_menus()
        .by_command
        .get(&stem)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn blocking_menus_shipped_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_SHIPPED_FILE_NAME)
}

pub(crate) fn blocking_menus_local_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_LOCAL_FILE_NAME)
}

pub(crate) fn blocking_menus_remote_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_REMOTE_FILE_NAME)
}

/// Pretty JSON plus one trailing newline: the only byte shape AC writes for both files.
pub(crate) fn pretty_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// #1905 (D5) - make the on-disk shipped file equal to the embedded content. True when written.
pub(crate) fn refresh_shipped_blocking_menus_file(settings_path: &Path) -> bool {
    let path = blocking_menus_shipped_path(settings_path);
    let Ok(canonical) = pretty_json_bytes(shipped_blocking_menus()) else {
        return false;
    };
    if matches!(
        crate::filesystem::agent_settings::read_blocking_menus_bytes(&path),
        Ok(existing) if existing == canonical
    ) {
        return false;
    }
    match crate::config::local_config_io::write_file_atomic(&path, &canonical) {
        Ok(()) => {
            log::info!(
                "[blocking-menus] wrote the shipped patterns to {}",
                path.display()
            );
            true
        }
        Err(e) => {
            log::error!("[blocking-menus] could not write {}: {e}", path.display());
            false
        }
    }
}

/// #1905 (D6) - the user layer. Every rejection logs once and yields an empty layer.
pub(crate) fn load_local_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile {
    let path = blocking_menus_local_path(settings_path);
    let contents = match crate::filesystem::agent_settings::read_blocking_menus_text(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BlockingMenusFile::default(),
        Err(e) => {
            log::error!("[blocking-menus] could not read {}: {e}", path.display());
            return BlockingMenusFile::default();
        }
    };
    match parse_blocking_menus_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::error!("[blocking-menus] {} {e}; ignoring the file", path.display());
            BlockingMenusFile::default()
        }
    }
}

/// #1925 (D4, D5) - the downloaded layer, re-validated at every read because the cache
/// sits in a directory the local user can edit. A rejected or unreadable cache yields an
/// empty layer and one warning; a missing cache is silent.
pub(crate) fn load_remote_blocking_menus_file(settings_path: &Path) -> BlockingMenusFile {
    let path = blocking_menus_remote_path(settings_path);
    let contents = match crate::filesystem::agent_settings::read_blocking_menus_text(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BlockingMenusFile::default(),
        Err(e) => {
            log::warn!("[blocking-menus] could not read {}: {e}", path.display());
            return BlockingMenusFile::default();
        }
    };
    match validate_remote_blocking_menus_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::warn!(
                "[blocking-menus] {} {e}; ignoring the downloaded patterns",
                path.display()
            );
            BlockingMenusFile::default()
        }
    }
}

/// #1925 (D7, D8) - replace the downloaded cache only after validation passed. The note
/// records where and when the bytes came from; the next accepted download replaces it and a
/// file that fails validation at the next start is ignored.
pub(crate) fn write_remote_blocking_menus_cache(
    settings_path: &Path,
    mut file: BlockingMenusFile,
    source_url: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    file.note = Some(format!(
        "Downloaded by AgentsCommander from {source_url} (source ref \
            {REMOTE_BLOCKING_MENUS_SOURCE_REF}) at {}. Replaced by the next accepted download and \
            ignored at start if it fails validation; put your own patterns in {}.",
        fetched_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        crate::config::instance_artifacts::BLOCKING_MENUS_LOCAL_FILE_NAME
    ));
    let bytes = pretty_json_bytes(&file).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &blocking_menus_remote_path(settings_path),
        &bytes,
    )
}

/// #1925 (D4) - the throttle stamp lives next to `settings.json`.
pub(crate) fn blocking_menus_remote_check_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(BLOCKING_MENUS_REMOTE_CHECK_FILE_NAME)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteBlockingMenusCheckStamp {
    pub(crate) last_checked_at: chrono::DateTime<chrono::Utc>,
}

/// #1925 (D4) - missing, unreadable and malformed stamps all mean "no throttle": the
/// caller treats `None` as due.
pub(crate) fn read_remote_blocking_menus_check_stamp(
    settings_path: &Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = crate::filesystem::agent_settings::read_blocking_menus_bytes(
        &blocking_menus_remote_check_path(settings_path),
    )
    .ok()?;
    let stamp: RemoteBlockingMenusCheckStamp = serde_json::from_slice(&bytes).ok()?;
    Some(stamp.last_checked_at)
}

/// #1925 (D4) - written after every attempt that passed the due check, accepted or not.
pub(crate) fn write_remote_blocking_menus_check_stamp(
    settings_path: &Path,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let stamp = RemoteBlockingMenusCheckStamp {
        last_checked_at: at,
    };
    let bytes = pretty_json_bytes(&stamp).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &blocking_menus_remote_check_path(settings_path),
        &bytes,
    )
}

/// #1905 (D3, D10) - what the menu guard evaluates. Built once per process; no file watcher.
#[derive(Debug, Clone)]
pub struct BlockingMenusStore {
    pub(crate) shipped: &'static BlockingMenusFile,
    pub(crate) remote: BlockingMenusFile,
    pub(crate) local: BlockingMenusFile,
}

/// #1757 - one-shot migration for installs whose Codex `blockingMenus` array was already
/// materialized against the pre-#1757 default, which is every install created before this
/// change. The legacy materialize step only filled `None` arrays, so those users would
/// otherwise never receive the hooks-review pattern.
///
/// Keyed by content, not by a marker field: the entry is appended only when no valid entry
/// already carries `CODEX_HOOKS_REVIEW_PATTERN`, so the function is idempotent across
/// restarts and adds no key to the settings schema. The check ignores `enabled`, so setting
/// `enabled: false` on the entry is a durable off switch; deleting it is not (#1757 D2).
///
/// Extends an ACTIVE set, never activates an inactive one: it only fills arrays that
/// already exist (never `None`), and `Some(vec![])` is the field's documented
/// "explicitly disabled" state.
///
/// #1737 (D7c): an overlay that owns `agents` supplies the whole array and `restore_base`
/// writes the base array back on save, so appending here would be discarded on write and
/// would overwrite the operator's in-memory array meanwhile. Owning `agents` therefore
/// suppresses the migration, which then runs correctly the first time the overlay is removed.
///
/// Since #1905 this runs only as the second step of `export_blocking_menus_to_local_file`.
///
/// Returns true when any agent's array changed.
pub fn apply_issue_1757_migration(settings: &mut AppSettings) -> bool {
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        return false;
    }
    let mut changed = false;
    for agent in &mut settings.agents {
        if crate::config::coding_agents_catalog::command_executable_basename(&agent.command)
            .as_deref()
            != Some("codex")
        {
            continue;
        }
        let Some(entries) = agent.blocking_menus.as_mut() else {
            continue;
        };
        if entries.is_empty() {
            continue;
        }
        if entries.iter().any(|entry| {
            entry
                .valid()
                .is_some_and(|c| c.pattern == CODEX_HOOKS_REVIEW_PATTERN)
        }) {
            continue;
        }
        entries.push(BlockingMenuEntry::Valid(codex_hooks_review_menu()));
        changed = true;
    }
    changed
}

/// Every failure path of the export: log, strip nothing. The loader may still save
/// settings.json for another migration's reason; the arrays survive that save unchanged.
pub(crate) fn abort_blocking_menus_export(local_path: &Path, why: &str) -> bool {
    log::error!(
        "[settings-migration] #1905 - {why} ({}); no blockingMenus array stripped",
        local_path.display()
    );
    false
}

/// #1905 (D7) - one-shot, state-keyed export of every `blockingMenus` array still in
/// `settings.json`. Writes the `.local` file before it strips anything; true means "save".
#[cfg(test)]
pub(crate) fn export_blocking_menus_to_local_file(
    settings: &mut AppSettings,
    settings_path: &Path,
) -> bool {
    export_blocking_menus_to_local_file_with_publish(
        settings,
        settings_path,
        &crate::config::local_config_io::publish_temp_config,
    )
}

#[cfg(test)]
pub(crate) fn export_blocking_menus_to_local_file_with_publish(
    settings: &mut AppSettings,
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
) -> bool {
    export_blocking_menus_with_startup(settings, settings_path, publish, None)
}

pub(crate) fn export_blocking_menus_with_startup(
    settings: &mut AppSettings,
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
    mut startup: Option<&mut StartupAgentRows>,
) -> bool {
    if let Err(reason) = registered_profile_raw_gate(settings_path) {
        log::warn!("{reason}");
        return false;
    }
    if settings.agents.iter().all(|a| a.blocking_menus.is_none()) {
        return false;
    }
    let local_path = blocking_menus_local_path(settings_path);
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_AGENTS)
    {
        for agent in settings
            .agents
            .iter()
            .filter(|a| a.blocking_menus.is_some())
        {
            log::info!(
                "[settings-migration] #1905 - agent '{}' carries blockingMenus inside an \
                overlay-owned agents array; it keeps applying from there and is not exported; move \
                it to {} to use the new file",
                agent.id,
                BLOCKING_MENUS_LOCAL_FILE_NAME
            );
        }
        return false;
    }
    apply_issue_1757_migration(settings);
    if let Some(context) = startup.as_deref_mut() {
        context.expected = settings.agents.clone();
    }
    // One id, one array, one command: two agents sharing an id with different arrays cannot
    // both live under byAgent[id], and equal arrays under different commands would make the
    // pristine test below order-dependent, so the export aborts and layer 0 keeps serving both.
    let mut by_id: BTreeMap<&str, (&str, &Vec<BlockingMenuEntry>)> = BTreeMap::new();
    for agent in &settings.agents {
        let Some(entries) = agent.blocking_menus.as_ref() else {
            continue;
        };
        let current = (agent.command.as_str(), entries);
        if let Some(previous) = by_id.insert(agent.id.as_str(), current) {
            if previous != current {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!(
                        "two agents share the id \
                    '{}' with different blockingMenus arrays or commands",
                        agent.id
                    ),
                );
            }
        }
    }
    let candidates: Vec<(String, Vec<BlockingMenuEntry>)> = by_id
        .iter()
        .filter(|(_, (command, entries))| **entries != default_blocking_menus_for_command(command))
        .map(|(id, (_, entries))| ((*id).to_string(), (*entries).clone()))
        .collect();
    let raw = match crate::filesystem::agent_settings::read_blocking_menus_text(&local_path) {
        Ok(contents) => Some(contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return abort_blocking_menus_export(
                &local_path,
                &format!("could not read the file: {e}"),
            )
        }
    };
    let mut root = match raw.as_deref() {
        None => Map::new(),
        Some(contents) => {
            // Schema parity: the runtime's own parser, on the complete file, before any mutation.
            if let Err(e) = parse_blocking_menus_file(contents) {
                return abort_blocking_menus_export(&local_path, &format!("the file {e}"));
            }
            match serde_json::from_str::<Value>(contents) {
                Ok(Value::Object(map)) => map,
                // Unreachable once the parser above rejected non-objects; kept as a guard.
                _ => {
                    return abort_blocking_menus_export(
                        &local_path,
                        "the file is not a JSON object",
                    )
                }
            }
        }
    };
    root.entry("schemaVersion")
        .or_insert_with(|| Value::from(BLOCKING_MENUS_SCHEMA_VERSION));
    let Some(by_agent) = root
        .entry("byAgent")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return abort_blocking_menus_export(&local_path, "byAgent is not an object");
    };
    let mut inserted = 0usize;
    for (id, entries) in candidates {
        if by_agent.contains_key(&id) {
            log::info!(
                "[settings-migration] #1905 - {} already has byAgent['{id}']; keeping it",
                local_path.display()
            );
            continue;
        }
        match serde_json::to_value(&entries) {
            Ok(value) => {
                by_agent.insert(id, value);
                inserted += 1;
            }
            Err(e) => {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!("could not serialize blockingMenus of '{id}': {e}"),
                )
            }
        }
    }
    if inserted > 0 {
        let bytes = match pretty_json_bytes(&Value::Object(root)) {
            Ok(bytes) => bytes,
            Err(e) => {
                return abort_blocking_menus_export(
                    &local_path,
                    &format!("could not serialize the file: {e}"),
                )
            }
        };
        // Schema parity again, on the exact bytes about to be published.
        if let Err(e) = std::str::from_utf8(&bytes)
            .map_err(|e| e.to_string())
            .and_then(parse_blocking_menus_file)
        {
            return abort_blocking_menus_export(&local_path, &format!("the merged file {e}"));
        }
        if let Err(e) = crate::config::local_config_io::write_file_atomic_with_publish(
            &local_path,
            &bytes,
            publish,
        ) {
            return abort_blocking_menus_export(
                &local_path,
                &format!("could not write the file: {e}"),
            );
        }
        log::info!(
            "[settings-migration] #1905 - exported {inserted} blockingMenus array(s) to {}",
            local_path.display()
        );
    }
    for agent in &mut settings.agents {
        agent.blocking_menus = None;
    }
    if let Some(context) = startup {
        context.expected = settings.agents.clone();
    }
    true
}

impl BlockingMenusStore {
    pub fn shipped_only() -> Self {
        Self::with_local(BlockingMenusFile::default())
    }

    pub fn with_local(local: BlockingMenusFile) -> Self {
        Self::with_layers(BlockingMenusFile::default(), local)
    }

    /// #1925 - the downloaded cache plus the user layer; the argument order matches `resolve`.
    pub(crate) fn with_layers(remote: BlockingMenusFile, local: BlockingMenusFile) -> Self {
        Self {
            shipped: shipped_blocking_menus(),
            remote,
            local,
        }
    }

    /// GUI startup entry point: refresh the shipped file, then read the user layer.
    pub fn load_from_config_dir() -> Self {
        match settings_path() {
            Some(path) => Self::load_from_settings_path(&path),
            None => Self::shipped_only(),
        }
    }

    pub(crate) fn load_from_settings_path(settings_path: &Path) -> Self {
        refresh_shipped_blocking_menus_file(settings_path);
        Self::with_layers(
            load_remote_blocking_menus_file(settings_path),
            load_local_blocking_menus_file(settings_path),
        )
    }

    /// D3 layers 1 to 5: local.byAgent, local.byCommand, remote.byCommand, then the shipped
    /// patterns. Layer 0 is the legacy agent array, handled by `resolve_for`.
    pub fn resolve(&self, agent_id: &str, command: &str) -> Vec<BlockingMenuEntry> {
        if let Some(entries) = self.local.by_agent.get(agent_id) {
            return entries.clone();
        }
        let Some(stem) = crate::config::coding_agents_catalog::command_executable_basename(command)
        else {
            return Vec::new();
        };
        if let Some(entries) = self.local.by_command.get(&stem) {
            return entries.clone();
        }
        if let Some(entries) = self.remote.by_command.get(&stem) {
            return entries.clone();
        }
        self.shipped
            .by_command
            .get(&stem)
            .cloned()
            .unwrap_or_default()
    }

    /// D3 layer 0 first: an array still on the agent (settings.json not yet migrated, or an
    /// overlay-owned agents array) is what applied before #1905 and keeps applying unchanged.
    pub fn resolve_for(&self, agent: &AgentConfig) -> Vec<BlockingMenuEntry> {
        match &agent.blocking_menus {
            Some(entries) => entries.clone(),
            None => self.resolve(&agent.id, &agent.command),
        }
    }
}
