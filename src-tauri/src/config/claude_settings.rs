use std::path::Path;

/// Ensures `.claude/settings.local.json` in `dir` contains a `claudeMdExcludes`
/// entry pointing to `~/.claude/CLAUDE.md`.
///
/// - Creates `.claude/` if missing.
/// - Merges into an existing file (preserves other keys).
/// - Resolves the home directory dynamically.
pub fn ensure_claude_md_excludes(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let claude_dir = dir.join(".claude");
    std::fs::create_dir_all(&claude_dir)
        .map_err(|e| format!("Failed to create .claude directory: {}", e))?;

    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let home_str = home.to_string_lossy().replace('\\', "/");
    let exclude_path = format!("{}/.claude/CLAUDE.md", home_str);

    let settings_path = claude_dir.join("settings.local.json");

    // Merge into existing file if present; treat parse errors and non-objects as empty
    let mut obj = if settings_path.exists() {
        let existing = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read existing settings.local.json: {}", e))?;
        match serde_json::from_str::<serde_json::Value>(&existing) {
            Ok(v) if v.is_object() => v,
            _ => serde_json::json!({}),
        }
    } else {
        serde_json::json!({})
    };

    // guaranteed to be an object by the match above
    let map = obj.as_object_mut().unwrap();

    // Preserve existing array entries, just ensure our path is present
    let mut excludes: Vec<serde_json::Value> = map
        .get("claudeMdExcludes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let exclude_val = serde_json::Value::String(exclude_path.clone());
    if !excludes.contains(&exclude_val) {
        excludes.push(exclude_val);
    }

    map.insert(
        "claudeMdExcludes".to_string(),
        serde_json::Value::Array(excludes),
    );

    let content = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&settings_path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write settings.local.json: {}", e))?;

    Ok(())
}
