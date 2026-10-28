use super::super::settings_path;
use super::pretty_json_bytes;
use crate::config::instance_artifacts::{
    AGENT_HELP_LOCAL_FILE_NAME, AGENT_HELP_REMOTE_CHECK_FILE_NAME, AGENT_HELP_REMOTE_FILE_NAME,
    AGENT_HELP_SHIPPED_FILE_NAME,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// #2133 - the embedded per-agent help, the shipped layer. P1 owns the file.
pub(crate) const EMBEDDED_AGENT_HELP_JSON: &str =
    include_str!("../../../../resources/agent-help/agent-help.json");

pub const AGENT_HELP_SCHEMA_VERSION: u32 = 1;

/// #2133 - a link at the end of a tip.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpLink {
    pub label: String,
    pub url: String,
}

/// #2133 - one tip: a title, a body and an optional link.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpTip {
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<AgentHelpLink>,
}

/// #2133 - the help for one command stem, one agent, or the general block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_example: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub tips: Vec<AgentHelpTip>,
}

/// #2133 - one agent-help file; the embedded, local and remote layers share this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpFile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<AgentHelpEntry>,
    #[serde(default)]
    pub by_command: BTreeMap<String, AgentHelpEntry>,
    #[serde(default)]
    pub by_agent: BTreeMap<String, AgentHelpEntry>,
}

impl Default for AgentHelpFile {
    fn default() -> Self {
        Self {
            schema_version: AGENT_HELP_SCHEMA_VERSION,
            note: None,
            general: None,
            by_command: BTreeMap::new(),
            by_agent: BTreeMap::new(),
        }
    }
}

/// #2133 - an absolute `https` URL with a non-empty host. No URL crate: the prefix and the
/// host segment are all the help links need.
pub(crate) fn is_https_url(value: &str) -> bool {
    value
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|host| !host.is_empty())
}

pub(crate) fn check_agent_help_entry_urls(key: &str, entry: &AgentHelpEntry) -> Result<(), String> {
    if let Some(docs_url) = &entry.docs_url {
        if !is_https_url(docs_url) {
            return Err(format!("entry {key}: docsUrl is not an https URL"));
        }
    }
    for (index, tip) in entry.tips.iter().enumerate() {
        if let Some(link) = &tip.link {
            if !is_https_url(&link.url) {
                return Err(format!(
                    "entry {key} tip {index}: link url is not an https URL"
                ));
            }
        }
    }
    Ok(())
}

/// #2133 (D10) - the one parser all three layers go through, shaped like
/// `parse_blocking_menus_file`. The `Value` step rejects a JSON array, which serde would
/// otherwise decode as an empty file.
pub(crate) fn parse_agent_help_file(contents: &str) -> Result<AgentHelpFile, String> {
    let value =
        serde_json::from_str::<Value>(contents).map_err(|e| format!("does not parse: {e}"))?;
    if !value.is_object() {
        return Err("is not a JSON object".to_string());
    }
    let file = serde_json::from_value::<AgentHelpFile>(value)
        .map_err(|e| format!("does not parse: {e}"))?;
    if file.schema_version != AGENT_HELP_SCHEMA_VERSION {
        return Err(format!(
            "has schemaVersion {}; only {} is supported",
            file.schema_version, AGENT_HELP_SCHEMA_VERSION
        ));
    }
    if let Some(general) = &file.general {
        check_agent_help_entry_urls("general", general)?;
    }
    for (key, entry) in file.by_command.iter().chain(file.by_agent.iter()) {
        check_agent_help_entry_urls(key, entry)?;
    }
    Ok(file)
}

/// #2133 - the operator-owned overlay lives next to `settings.json`.
pub(crate) fn agent_help_local_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_LOCAL_FILE_NAME)
}

/// #2133 (D7) - the user layer, no caps. A missing file is silent; any other rejection logs
/// once and yields an empty layer plus the reason, which P4 hands to the UI.
pub(crate) fn load_local_agent_help_file(settings_path: &Path) -> (AgentHelpFile, Option<String>) {
    let path = agent_help_local_path(settings_path);
    let contents = match crate::filesystem::agent_settings::read_agent_help_text(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (AgentHelpFile::default(), None)
        }
        Err(e) => {
            log::warn!("[agent-help] could not read {}: {e}", path.display());
            return (
                AgentHelpFile::default(),
                Some(format!("could not read {}: {e}", path.display())),
            );
        }
    };
    match parse_agent_help_file(&contents) {
        Ok(file) => (file, None),
        Err(e) => {
            log::warn!("[agent-help] {} {e}; ignoring the file", path.display());
            (
                AgentHelpFile::default(),
                Some(format!("{} {e}", path.display())),
            )
        }
    }
}

/// #2133 - the AC-owned shipped copy lives next to `settings.json`. Only the refresh writes it;
/// nothing reads it back (the #1905 rule): the runtime uses `shipped_agent_help()`.
pub(crate) fn agent_help_shipped_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_SHIPPED_FILE_NAME)
}

/// #2133 - make the on-disk shipped file equal to the canonical form of the embedded content.
/// True when written. A hand edit is overwritten at the next start: the file is AC-owned.
/// Never fails: a write error is logged and startup continues on the embedded constant.
pub(crate) fn refresh_shipped_agent_help_file(settings_path: &Path) -> bool {
    refresh_shipped_agent_help_file_with_publish(
        settings_path,
        &crate::config::local_config_io::publish_temp_config,
    )
}

pub(crate) fn refresh_shipped_agent_help_file_with_publish(
    settings_path: &Path,
    publish: crate::config::local_config_io::ConfigPublisher<'_>,
) -> bool {
    let path = agent_help_shipped_path(settings_path);
    let Ok(canonical) = pretty_json_bytes(shipped_agent_help()) else {
        return false;
    };
    if matches!(
        crate::filesystem::agent_settings::read_agent_help_bytes(&path),
        Ok(existing) if existing == canonical
    ) {
        return false;
    }
    match crate::config::local_config_io::write_file_atomic_with_publish(&path, &canonical, publish)
    {
        Ok(()) => {
            log::info!("[agent-help] wrote the shipped help to {}", path.display());
            true
        }
        Err(e) => {
            log::error!("[agent-help] could not write {}: {e}", path.display());
            false
        }
    }
}

/// #2133 - the startup entry point; the only agent-help function that resolves the config dir.
pub fn refresh_shipped_agent_help_from_config_dir() {
    match settings_path() {
        Some(path) => {
            refresh_shipped_agent_help_file(&path);
        }
        None => log::debug!("[agent-help] no config dir; the shipped help is not written"),
    }
}

/// #2133 - the git ref the published per-agent help is served from; named in every
/// cache note. The URL literal that embeds it is P7's, with the SERVED-PATHS row.
pub(crate) const REMOTE_AGENT_HELP_SOURCE_REF: &str = "main";

/// #2133 (D4) - hard caps on the downloaded layer. Every cap counts UTF-8 bytes
/// (`str::len()`), not characters; TypeScript's `.length` counts UTF-16 code units instead.
pub(crate) const REMOTE_AGENT_HELP_MAX_BYTES: usize = 64 * 1024;
pub(crate) const REMOTE_AGENT_HELP_MAX_ENTRIES: usize = 40;
pub(crate) const REMOTE_AGENT_HELP_LABEL_MAX_BYTES: usize = 80;
pub(crate) const REMOTE_AGENT_HELP_PARAMS_MAX_BYTES: usize = 256;
pub(crate) const REMOTE_AGENT_HELP_URL_MAX_BYTES: usize = 512;
pub(crate) const REMOTE_AGENT_HELP_MAX_TIPS: usize = 12;
pub(crate) const REMOTE_AGENT_HELP_TITLE_MAX_BYTES: usize = 120;
pub(crate) const REMOTE_AGENT_HELP_BODY_MAX_BYTES: usize = 1200;

/// UTF-8 bytes, as every remote cap.
pub(crate) fn check_remote_agent_help_cap(
    at: &str,
    field: &str,
    value: &str,
    cap: usize,
) -> Result<(), String> {
    if value.len() > cap {
        return Err(format!(
            "{at}: {field} is {} bytes; more than {cap} bytes are not allowed",
            value.len()
        ));
    }
    Ok(())
}

pub(crate) fn check_remote_agent_help_entry(
    key: &str,
    entry: &AgentHelpEntry,
) -> Result<(), String> {
    let at = format!("entry {key}");
    let optional = [
        ("label", &entry.label, REMOTE_AGENT_HELP_LABEL_MAX_BYTES),
        (
            "paramsExample",
            &entry.params_example,
            REMOTE_AGENT_HELP_PARAMS_MAX_BYTES,
        ),
        ("docsUrl", &entry.docs_url, REMOTE_AGENT_HELP_URL_MAX_BYTES),
    ];
    for (field, value, cap) in optional {
        if let Some(value) = value {
            check_remote_agent_help_cap(&at, field, value, cap)?;
        }
    }
    if entry.tips.len() > REMOTE_AGENT_HELP_MAX_TIPS {
        return Err(format!(
            "{at}: holds {} tips; more than {REMOTE_AGENT_HELP_MAX_TIPS} are not allowed",
            entry.tips.len()
        ));
    }
    for (index, tip) in entry.tips.iter().enumerate() {
        let at = format!("entry {key} tip {index}");
        check_remote_agent_help_cap(&at, "title", &tip.title, REMOTE_AGENT_HELP_TITLE_MAX_BYTES)?;
        check_remote_agent_help_cap(&at, "body", &tip.body, REMOTE_AGENT_HELP_BODY_MAX_BYTES)?;
        if let Some(link) = &tip.link {
            check_remote_agent_help_cap(
                &at,
                "link url",
                &link.url,
                REMOTE_AGENT_HELP_URL_MAX_BYTES,
            )?;
        }
    }
    Ok(())
}

/// #2133 (D4, D10) - the whole-file gate for the downloaded layer, shaped like
/// `validate_remote_blocking_menus_file`. The first failed check rejects the whole file,
/// never one entry; `byAgent` is rejected because the remote layer is `byCommand`-only.
pub(crate) fn validate_remote_agent_help_file(contents: &str) -> Result<AgentHelpFile, String> {
    let file = parse_agent_help_file(contents)?;
    if !file.by_agent.is_empty() {
        return Err("byAgent must be empty".to_string());
    }
    if file.by_command.len() > REMOTE_AGENT_HELP_MAX_ENTRIES {
        return Err(format!(
            "holds {} entries; more than {REMOTE_AGENT_HELP_MAX_ENTRIES} entries are not allowed",
            file.by_command.len()
        ));
    }
    if let Some(general) = &file.general {
        check_remote_agent_help_entry("general", general)?;
    }
    for (key, entry) in &file.by_command {
        check_remote_agent_help_entry(key, entry)?;
    }
    Ok(file)
}

/// #2133 - the response gate: status, body size and UTF-8 before the whole-file validator.
pub(crate) fn accept_remote_agent_help_response(
    status: u16,
    body: &[u8],
) -> Result<AgentHelpFile, String> {
    if status != 200 {
        return Err(format!("HTTP status {status}"));
    }
    if body.len() > REMOTE_AGENT_HELP_MAX_BYTES {
        return Err(format!(
            "body larger than {REMOTE_AGENT_HELP_MAX_BYTES} bytes"
        ));
    }
    let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8".to_string())?;
    validate_remote_agent_help_file(text)
}

pub(crate) fn agent_help_remote_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_REMOTE_FILE_NAME)
}

/// #2133 (D4) - the downloaded layer, re-validated at every read because the cache sits in a
/// directory the local user can edit. A rejected or unreadable cache yields an empty layer and
/// one warning; a missing cache is silent.
pub(crate) fn load_remote_agent_help_file(settings_path: &Path) -> AgentHelpFile {
    let path = agent_help_remote_path(settings_path);
    let contents = match crate::filesystem::agent_settings::read_agent_help_text(&path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return AgentHelpFile::default(),
        Err(e) => {
            log::warn!("[agent-help] could not read {}: {e}", path.display());
            return AgentHelpFile::default();
        }
    };
    match validate_remote_agent_help_file(&contents) {
        Ok(file) => file,
        Err(e) => {
            log::warn!(
                "[agent-help] {} {e}; ignoring the downloaded help",
                path.display()
            );
            AgentHelpFile::default()
        }
    }
}

/// #2133 - replace the downloaded cache only after validation passed. The note records where
/// and when the bytes came from; `source_url` is the caller's, so this phase holds no URL.
pub(crate) fn write_remote_agent_help_cache(
    settings_path: &Path,
    mut file: AgentHelpFile,
    source_url: &str,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    file.note = Some(format!(
        "Downloaded by AgentsCommander from {source_url} (source ref \
            {REMOTE_AGENT_HELP_SOURCE_REF}) at {}. Replaced by the next accepted download and \
            ignored at start if it fails validation; put your own help in agent-help.local.json.",
        fetched_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    let bytes = pretty_json_bytes(&file).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &agent_help_remote_path(settings_path),
        &bytes,
    )
}

/// #2133 (D4) - the throttle stamp lives next to `settings.json`.
pub(crate) fn agent_help_remote_check_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(AGENT_HELP_REMOTE_CHECK_FILE_NAME)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAgentHelpCheckStamp {
    pub(crate) last_checked_at: chrono::DateTime<chrono::Utc>,
}

/// #2133 (D4) - missing, unreadable and malformed stamps all mean "no throttle": the caller
/// treats `None` as due.
pub(crate) fn read_remote_agent_help_check_stamp(
    settings_path: &Path,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = crate::filesystem::agent_settings::read_agent_help_bytes(
        &agent_help_remote_check_path(settings_path),
    )
    .ok()?;
    let stamp: RemoteAgentHelpCheckStamp = serde_json::from_slice(&bytes).ok()?;
    Some(stamp.last_checked_at)
}

/// #2133 (D4) - written after every attempt that passed the due check, accepted or not.
pub(crate) fn write_remote_agent_help_check_stamp(
    settings_path: &Path,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let stamp = RemoteAgentHelpCheckStamp {
        last_checked_at: at,
    };
    let bytes = pretty_json_bytes(&stamp).map_err(|e| e.to_string())?;
    crate::config::local_config_io::write_file_atomic(
        &agent_help_remote_check_path(settings_path),
        &bytes,
    )
}

/// #2133 - a parse failure is a build defect T1 catches; production logs once and serves an
/// empty file. The runtime reads this, never a disk copy.
pub fn shipped_agent_help() -> &'static AgentHelpFile {
    static SHIPPED: OnceLock<AgentHelpFile> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        parse_agent_help_file(EMBEDDED_AGENT_HELP_JSON).unwrap_or_else(|e| {
            log::error!("[agent-help] embedded agent-help.json {e}");
            AgentHelpFile::default()
        })
    })
}
