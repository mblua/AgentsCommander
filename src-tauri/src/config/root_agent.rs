use serde_json::{Map, Value};
use std::path::Path;
use std::sync::{LazyLock, OnceLock};

pub const ROOT_AGENT_DIR_NAME: &str = "ac-root-agent";
pub const ROOT_AGENT_SESSION_NAME: &str = "Root Agent";
pub const ROOT_AGENT_SENDER: &str = "agentscommander://root-agent";
pub const ROOT_AGENT_SHORT_NAME: &str = "root";

const OLD_DEFERRED_MESSAGING_PARAGRAPH: &str = "Direct file-based workgroup messaging is not available from the root-agent directory yet: `send --send` currently requires a workgroup replica root. Do not claim that you can autonomously message workgroup peers until a future root messaging feature adds explicit root-aware send instructions.";

const ROOT_COORDINATION_MESSAGING_PARAGRAPH: &str = r#"You may message verified workgroup coordinator replicas only. Before sending, run `list-peers-lean` with your `AGENTSCOMMANDER_*` credentials and use only the `name` values it returns. In Root Agent sessions this list omits origin coordinators and non-coordinator replicas.

Root messaging is file-based:

1. Write the message to `messaging/` inside this `ac-root-agent` directory.
2. Use a filename shaped like `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md`.
3. Send it with:

```text
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<coordinator_name>" --send <filename> --mode wake
```

Never send to origin coordinators or non-coordinator specialist/member agents from this root session."#;

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
    if !role_path.exists() {
        std::fs::write(role_path, ROOT_ROLE_MD.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", role_path.display(), e))?;
        return Ok(());
    }

    let existing = std::fs::read_to_string(role_path)
        .map_err(|e| format!("Failed to read {}: {}", role_path.display(), e))?;
    let migrated = if normalize_role_text(&existing) == normalize_role_text(OLD_ROOT_ROLE_MD) {
        Some(ROOT_ROLE_MD.to_string())
    } else if existing.contains(OLD_DEFERRED_MESSAGING_PARAGRAPH) {
        Some(existing.replace(
            OLD_DEFERRED_MESSAGING_PARAGRAPH,
            ROOT_COORDINATION_MESSAGING_PARAGRAPH,
        ))
    } else {
        None
    };

    if let Some(content) = migrated {
        std::fs::write(role_path, content)
            .map_err(|e| format!("Failed to write {}: {}", role_path.display(), e))?;
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
    fn ensure_root_agent_dir_at_is_idempotent_and_preserves_custom_role() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(ROOT_AGENT_DIR_NAME);
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
    fn root_sender_uses_reserved_non_path_namespace() {
        assert_eq!(ROOT_AGENT_SENDER, "agentscommander://root-agent");
        assert_ne!(
            ROOT_AGENT_SENDER,
            crate::config::teams::agent_fqn_from_path("C:/tmp/agentscommander/_agent_root-agent")
        );
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
