use serde_json::{Map, Value};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};

pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
pub const ROOT_AGENT_SESSION_NAME: &str = "Root Agent";
pub const ROOT_AGENT_SENDER: &str = "agentscommander://root-agent";
pub const ROOT_AGENT_SHORT_NAME: &str = "root";
static ROOT_ROLE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static FAIL_ROOT_ROLE_WRITE_ONCE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
const FAIL_ROOT_ROLE_WRITE_MARKER: &str = "FAIL_ROOT_ROLE_WRITE_ONCE";

/// Returns `true` iff `target` is the canonical Root Agent reply name.
///
/// Symmetric with `ROOT_AGENT_SENDER` (the `msg.from` value the Root Agent
/// writes when it sends): any peer that received that value as `from` MUST
/// be able to round-trip it back as `--to`.
pub fn is_root_agent_target(target: &str) -> bool {
    target == ROOT_AGENT_SENDER
}

const OLD_DEFERRED_MESSAGING_PARAGRAPH: &str = "Direct file-based workgroup messaging is not available from the root-agent directory yet: `send --send` currently requires a workgroup replica root. Do not claim that you can autonomously message workgroup peers until a future root messaging feature adds explicit root-aware send instructions.";

const ROOT_COORDINATION_MESSAGING_PARAGRAPH: &str = r#"You may message verified workgroup coordinator replicas only. Before sending, run `list-peers-lean` with your `AGENTSCOMMANDER_*` credentials and use only the `name` values it returns. In Root Agent sessions this list omits origin coordinators and non-coordinator replicas.

Root messaging is file-based:

1. Write the message to `messaging/` inside this `ac-root-agent` directory.
2. Use a filename shaped like `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md`.
3. Send it with:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<coordinator_name>" --send <filename> --mode wake
```

Never send to origin coordinators or non-coordinator specialist/member agents from this root session.

Coordinators may reply by sending to `agentscommander://root-agent`; their replies appear in this session as standard file notifications."#;

const OLD_ROOT_ROLE_MD: &str = r#"---
name: 'agents-commander'
description: 'Root coordinator for AgentsCommander sessions, workgroups, and agents.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results. When direct peer messaging is unavailable, say so plainly and ask the user to route messages or wait for a future root messaging feature rather than claiming sends were performed.

## State

Your own canonical state lives in this `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use this directory for your own durable state.

## Coordination

Use the AgentsCommander CLI only for commands that are valid from this root-agent directory. Follow the write restrictions in the common context exactly.

Direct file-based workgroup messaging is not available from the root-agent directory yet: `send --send` currently requires a workgroup replica root. Do not claim that you can autonomously message workgroup peers until a future root messaging feature adds explicit root-aware send instructions.
"#;

static ROOT_ROLE_MD: LazyLock<String> = LazyLock::new(|| {
    format!(
        r#"---
name: 'agents-commander'
description: 'Root coordinator for AgentsCommander sessions, workgroups, and agents.'
type: agent
---

# Agents Commander

You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary.

## Responsibility

Act as the top-level planning and oversight agent for sessions, workgroups, and agents available to this AgentsCommander instance. Help the user inspect available work, plan delegation, track status, and synthesize results. When direct peer messaging is unavailable, say so plainly and ask the user to route messages or wait for a future root messaging feature rather than claiming sends were performed.

## State

Your own canonical state lives in this `ac-root-agent` directory:

- `memory/`
- `plans/`
- `skills/`
- `Role.md`

You are not a workgroup replica and you do not have an origin Agent Matrix. Use this directory for your own durable state.

## Coordination

Use the AgentsCommander CLI only for commands that are valid from this root-agent directory. Follow the write restrictions in the common context exactly.

{ROOT_COORDINATION_MESSAGING_PARAGRAPH}
"#
    )
});

pub fn root_agent_dir() -> Result<String, String> {
    static ROOT_DIR: OnceLock<String> = OnceLock::new();
    if let Some(cached) = ROOT_DIR.get() {
        return Ok(cached.clone());
    }

    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let root_dir = display_path(&config_dir.join(ROOT_AGENT_DIR_NAME));
    let _ = ROOT_DIR.set(root_dir.clone());
    Ok(root_dir)
}

pub fn is_root_agent_dir_name(cwd: &str) -> bool {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case(ROOT_AGENT_DIR_NAME))
        .unwrap_or(false)
}

pub fn is_root_agent_path(cwd: &str) -> bool {
    let Ok(root_dir) = root_agent_dir() else {
        return false;
    };
    paths_equivalent(Path::new(cwd), Path::new(&root_dir))
}

pub fn ensure_root_agent_dir() -> Result<String, String> {
    let root_dir = root_agent_dir()?;
    ensure_root_agent_dir_at(Path::new(&root_dir))?;
    Ok(root_dir)
}

pub(crate) fn ensure_root_agent_dir_at(root_dir: &Path) -> Result<(), String> {
    crate::commands::entity_creation::create_agent_matrix_layout(root_dir).map_err(
        |(sub, e)| {
            format!(
                "Failed to create root agent layout entry '{}' at {}: {}",
                sub,
                root_dir.display(),
                e
            )
        },
    )?;

    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).map_err(|e| {
        format!(
            "Failed to create root agent messaging directory at {}: {}",
            messaging_dir.display(),
            e
        )
    })?;

    let role_path = root_dir.join("Role.md");
    migrate_root_role(&role_path)?;

    merge_root_agent_config(&root_dir.join("config.json"))
}

fn migrate_root_role(role_path: &Path) -> Result<(), String> {
    let root_dir = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve root agent directory from {}",
            role_path.display()
        )
    })?;
    let config_dir = root_dir.parent().ok_or_else(|| {
        format!(
            "Could not resolve config directory from {}",
            role_path.display()
        )
    })?;
    let context_template_path =
        config_dir.join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);

    let mut context_text = read_validated_template(&context_template_path)?;
    if context_text.is_none() {
        crate::config::session_context::write_template_if_missing(
            &context_template_path,
            ROOT_ROLE_MD.as_str(),
        )?;
        context_text = Some(
            read_validated_template(&context_template_path)?.ok_or_else(|| {
                format!(
                    "Template missing immediately after write_template_if_missing: {}",
                    context_template_path.display()
                )
            })?,
        );
    }
    let context_text = context_text.expect("checked above");

    match create_missing_role(role_path, &context_text)? {
        CreateMissingRole::Created => return Ok(()),
        CreateMissingRole::AlreadyExists => {}
    }

    let existing = std::fs::read_to_string(role_path)
        .map_err(|e| format!("Failed to read {}: {}", role_path.display(), e))?;
    let existing_normalized = normalize_role_text(&existing);
    let context_normalized = normalize_role_text(&context_text);
    let migrated = if existing_normalized == normalize_role_text(OLD_ROOT_ROLE_MD)
        || existing_normalized == normalize_role_text(&ROOT_ROLE_MD)
    {
        if existing_normalized != context_normalized {
            Some(context_text)
        } else {
            None
        }
    } else if existing.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH) {
        Some(existing.replace(
            OLD_DEFERRED_MESSAGING_PARAGRAPH,
            ROOT_COORDINATION_MESSAGING_PARAGRAPH,
        ))
    } else {
        None
    };

    if let Some(content) = migrated {
        atomic_write_role(role_path, &content)?;
    }

    Ok(())
}

fn read_validated_template(path: &Path) -> Result<Option<String>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Failed to inspect root agent context template {}: {}",
                path.display(),
                e
            ))
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Root agent context template {} exists but is not a regular file",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "Failed to read root agent context template {}: {}",
            path.display(),
            e
        )
    })?;
    String::from_utf8(bytes).map(Some).map_err(|e| {
        format!(
            "Root agent context template {} is not valid UTF-8: {}",
            path.display(),
            e
        )
    })
}

fn atomic_write_role(role_path: &Path, content: &str) -> Result<(), String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    if let Err(e) = replace_role_file(&temp_path, role_path) {
        cleanup_temp_role(&temp_path);
        return Err(e);
    }

    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }

    Ok(())
}

enum CreateMissingRole {
    Created,
    AlreadyExists,
}

fn create_missing_role(role_path: &Path, content: &str) -> Result<CreateMissingRole, String> {
    let parent = role_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            role_path.display()
        )
    })?;
    let temp_path = unique_role_temp_path(role_path);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(e) => {
            return Err(format!(
                "Failed to create temporary role file {}: {}",
                temp_path.display(),
                e
            ))
        }
    };

    if let Err(e) = write_role_file(&mut file, role_path, content) {
        drop(file);
        cleanup_temp_role(&temp_path);
        return Err(e);
    }
    drop(file);

    let published = match publish_missing_role_file(&temp_path, role_path) {
        Ok(published) => published,
        Err(e) => {
            cleanup_temp_role(&temp_path);
            return Err(e);
        }
    };

    cleanup_temp_role(&temp_path);

    if published {
        sync_role_dir(parent);
        Ok(CreateMissingRole::Created)
    } else {
        Ok(CreateMissingRole::AlreadyExists)
    }
}

fn write_role_file(
    file: &mut std::fs::File,
    role_path: &Path,
    content: &str,
) -> Result<(), String> {
    #[cfg(test)]
    if content.contains(FAIL_ROOT_ROLE_WRITE_MARKER)
        && FAIL_ROOT_ROLE_WRITE_ONCE.swap(false, Ordering::SeqCst)
    {
        return Err(format!(
            "Failed to write {}: injected failure",
            role_path.display()
        ));
    }

    file.write_all(content.as_bytes())
        .map_err(|e| format!("Failed to write {}: {}", role_path.display(), e))?;
    file.flush()
        .map_err(|e| format!("Failed to flush {}: {}", role_path.display(), e))?;
    file.sync_all()
        .map_err(|e| format!("Failed to sync {}: {}", role_path.display(), e))
}

fn unique_role_temp_path(role_path: &Path) -> std::path::PathBuf {
    let parent = role_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = role_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Role.md");
    let counter = ROOT_ROLE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

fn cleanup_temp_role(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "Failed to remove temporary role file {}: {}",
                path.display(),
                e
            );
        }
    }
}

fn sync_role_dir(parent: &Path) {
    if let Ok(dir) = std::fs::File::open(parent) {
        if let Err(e) = dir.sync_all() {
            log::warn!(
                "Failed to sync root agent role directory {}: {}",
                parent.display(),
                e
            );
        }
    }
}

fn publish_missing_role_file(temp_path: &Path, role_path: &Path) -> Result<bool, String> {
    match std::fs::hard_link(temp_path, role_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!(
            "Failed to publish missing role file {} from {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )),
    }
}

#[cfg(not(windows))]
fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    std::fs::rename(temp_path, role_path).map_err(|e| {
        format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            e
        )
    })
}

#[cfg(windows)]
fn replace_role_file(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    if !role_path.exists() {
        return std::fs::rename(temp_path, role_path).map_err(|e| {
            format!(
                "Failed to publish {} from {}: {}",
                role_path.display(),
                temp_path.display(),
                e
            )
        });
    }

    replace_existing_file_windows(temp_path, role_path)
}

#[cfg(windows)]
fn replace_existing_file_windows(temp_path: &Path, role_path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let role_wide: Vec<u16> = role_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let temp_wide: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        ReplaceFileW(
            role_wide.as_ptr(),
            temp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "Failed to replace {} with {}: {}",
            role_path.display(),
            temp_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn normalize_role_text(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

pub(crate) fn merge_root_agent_config(config_path: &Path) -> Result<(), String> {
    let mut root = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
        let parsed: Value = serde_json::from_str(&content).map_err(|e| {
            format!(
                "Failed to parse root agent config {}: {}",
                config_path.display(),
                e
            )
        })?;
        if !parsed.is_object() {
            return Err(format!(
                "Root agent config {} must be a JSON object",
                config_path.display()
            ));
        }
        parsed
    } else {
        Value::Object(Map::new())
    };

    let obj = root.as_object_mut().expect("checked object above");
    obj.entry("tooling".to_string())
        .or_insert_with(|| Value::Object(Map::new()));

    let has_non_empty_context = obj
        .get("context")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| !arr.is_empty());
    if !has_non_empty_context {
        obj.insert(
            "context".to_string(),
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"]),
        );
    }

    let json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize root agent config: {}", e))?;
    std::fs::write(config_path, json)
        .map_err(|e| format!("Failed to write {}: {}", config_path.display(), e))?;

    Ok(())
}

pub fn read_last_coding_agent(root_dir: &str) -> Option<String> {
    let config_path = Path::new(root_dir).join("config.json");
    let contents = std::fs::read_to_string(config_path).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    value
        .get("tooling")
        .and_then(|tooling| tooling.get("lastCodingAgent"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => normalize_for_compare(&left) == normalize_for_compare(&right),
        _ => normalize_for_compare(left) == normalize_for_compare(right),
    }
}

fn normalize_for_compare(path: &Path) -> String {
    let mut s = display_path(path).replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_root_agent_dir_at_creates_layout_role_and_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);

        ensure_root_agent_dir_at(&root).expect("ensure root");

        for sub in ["memory", "plans", "skills", "inbox", "outbox", "messaging"] {
            assert!(root.join(sub).is_dir(), "missing {}", sub);
        }
        assert!(root.join("Role.md").is_file());
        assert!(ROOT_ROLE_MD.contains("verified workgroup coordinator replicas only"));
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        assert!(template_path.is_file());
        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            std::fs::read_to_string(template_path).expect("read template")
        );
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(config["tooling"], serde_json::json!({}));
        assert_eq!(
            config["context"],
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_seeds_missing_role_from_custom_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nUse this exact seed.\n";
        std::fs::write(&template_path, custom_template).expect("write template");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            custom_template
        );
    }

    #[test]
    fn failed_missing_role_seed_leaves_no_final_role_and_retry_creates_complete_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = format!(
            "# Custom Root Template\n\n{FAIL_ROOT_ROLE_WRITE_MARKER}\n\nComplete seed body.\n"
        );
        std::fs::write(&template_path, &custom_template).expect("write template");

        FAIL_ROOT_ROLE_WRITE_ONCE.store(true, Ordering::SeqCst);
        let err = ensure_root_agent_dir_at(&root).expect_err("injected seed write must fail");

        assert!(err.contains("injected failure"), "{err}");
        assert!(
            !root.join("Role.md").exists(),
            "failed missing-role seed must not publish final Role.md"
        );

        ensure_root_agent_dir_at(&root).expect("retry ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_is_idempotent_and_preserves_custom_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::write(&template_path, "# Custom Template\n\nTemplate body.\n")
            .expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), "custom role").expect("write role");

        ensure_root_agent_dir_at(&root).expect("first ensure");
        ensure_root_agent_dir_at(&root).expect("second ensure");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            "custom role"
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_replaces_current_builtin_role_with_custom_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nReplace built-in text.\n";
        std::fs::write(&template_path, custom_template).expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), ROOT_ROLE_MD.as_str()).expect("write current role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_migrates_old_builtin_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
        assert_eq!(
            normalize_role_text(&migrated),
            normalize_role_text(&ROOT_ROLE_MD)
        );
        assert!(migrated.contains("verified workgroup coordinator replicas only"));
        assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
    }

    #[test]
    fn ensure_root_agent_dir_at_replaces_old_builtin_role_with_custom_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        let custom_template = "# Custom Root Template\n\nMigrate old default here.\n";
        std::fs::write(&template_path, custom_template).expect("write template");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(root.join("Role.md"), OLD_ROOT_ROLE_MD).expect("write old role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        assert_eq!(
            std::fs::read_to_string(root.join("Role.md")).expect("read role"),
            custom_template
        );
    }

    #[test]
    fn ensure_root_agent_dir_at_replaces_old_deferred_paragraph_in_custom_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        let custom = format!(
            "# Custom Root\n\n{}\n\nKeep this custom tail.",
            OLD_DEFERRED_MESSAGING_PARAGRAPH
        );
        std::fs::write(root.join("Role.md"), custom).expect("write custom role");

        ensure_root_agent_dir_at(&root).expect("ensure root");

        let migrated = std::fs::read_to_string(root.join("Role.md")).expect("read role");
        assert!(migrated.starts_with("# Custom Root"));
        assert!(migrated.contains("Keep this custom tail."));
        assert!(migrated.contains(ROOT_COORDINATION_MESSAGING_PARAGRAPH));
        assert!(!migrated.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH));
    }

    #[test]
    fn ensure_root_agent_dir_at_errors_when_root_template_is_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        let template_path = temp
            .path()
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME);
        std::fs::create_dir_all(&template_path).expect("create template directory");

        let err = ensure_root_agent_dir_at(&root).expect_err("directory template must fail");

        assert!(err.contains("not a regular file"), "{err}");
        assert!(!root.join("Role.md").exists());
    }

    #[test]
    fn create_default_context_templates_does_not_create_root_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_dir = temp.path().join(".ac");

        crate::config::session_context::create_default_context_templates(&workspace_dir)
            .expect("create default templates");

        assert!(workspace_dir
            .join(crate::config::session_context::AGENT_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(workspace_dir
            .join(crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .is_file());
        assert!(!workspace_dir
            .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
            .exists());
    }

    #[test]
    fn root_sender_uses_reserved_non_path_namespace() {
        assert_eq!(ROOT_AGENT_SENDER, "agentscommander://root-agent");
        assert_ne!(
            ROOT_AGENT_SENDER,
            crate::config::teams::agent_fqn_from_path("C:/tmp/agentscommander/_agent_root-agent")
        );
    }

    #[test]
    fn is_root_agent_target_recognizes_canonical_uri() {
        assert!(is_root_agent_target(ROOT_AGENT_SENDER));
        assert!(is_root_agent_target("agentscommander://root-agent"));
    }

    #[test]
    fn is_root_agent_target_rejects_partial_or_wrong_uris() {
        assert!(!is_root_agent_target(""));
        assert!(!is_root_agent_target("agentscommander://root"));
        assert!(!is_root_agent_target("root-agent"));
        assert!(!is_root_agent_target("agentscommander/root-agent"));
        assert!(!is_root_agent_target("agentscommander://ROOT-AGENT"));
    }

    #[test]
    fn merge_root_agent_config_preserves_tooling_and_unknown_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{
  "tooling": {
    "lastCodingAgent": "codex",
    "codingAgents": {"codex": {"app": "Codex"}},
    "telegramBot": "ops"
  },
  "unknown": {"keep": true},
  "context": []
}"#,
        )
        .expect("write config");

        merge_root_agent_config(&config_path).expect("merge config");

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
                .expect("parse config");
        assert_eq!(config["tooling"]["lastCodingAgent"], "codex");
        assert_eq!(config["tooling"]["telegramBot"], "ops");
        assert_eq!(config["unknown"]["keep"], true);
        assert_eq!(
            config["context"],
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
        );
    }

    #[test]
    fn malformed_config_returns_error_without_rewriting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(&config_path, "{not json").expect("write config");

        let err = merge_root_agent_config(&config_path).expect_err("must fail");

        assert!(err.contains("Failed to parse root agent config"));
        assert_eq!(
            std::fs::read_to_string(&config_path).expect("read config"),
            "{not json"
        );
    }

    #[test]
    fn set_last_coding_agent_preserves_root_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        ensure_root_agent_dir_at(&root).expect("ensure root");

        crate::config::agent_config::set_last_coding_agent(
            &root.to_string_lossy(),
            "codex",
            "Codex",
            Some("session-1"),
        )
        .expect("set last coding agent");

        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("config.json")).expect("read config"),
        )
        .expect("parse config");
        assert_eq!(config["tooling"]["lastCodingAgent"], "codex");
        assert_eq!(
            config["context"],
            serde_json::json!(["$AGENTSCOMMANDER_CONTEXT", "Role.md"])
        );
    }

    #[test]
    fn read_last_coding_agent_reads_tooling_field_and_tolerates_bad_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(
            root.join("config.json"),
            r#"{"tooling":{"lastCodingAgent":"claude"}}"#,
        )
        .expect("write config");

        assert_eq!(
            read_last_coding_agent(&root.to_string_lossy()).as_deref(),
            Some("claude")
        );

        std::fs::write(root.join("config.json"), "{not json").expect("write bad config");
        assert!(read_last_coding_agent(&root.to_string_lossy()).is_none());
        assert!(read_last_coding_agent(&temp.path().join("missing").to_string_lossy()).is_none());
    }

    #[test]
    fn root_dir_name_detection_is_case_insensitive() {
        assert!(is_root_agent_dir_name("C:/tmp/AC-ROOT-AGENT"));
        assert!(!is_root_agent_dir_name("C:/tmp/not-root"));
    }
}
