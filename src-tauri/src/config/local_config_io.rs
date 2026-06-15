use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn local_config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn update_config_json_object<F>(
    path: &Path,
    allow_create: bool,
    mutate: F,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    let _guard = local_config_write_lock()
        .lock()
        .map_err(|_| "Local config write lock is poisoned".to_string())?;

    let mut root = if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let parsed: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        if !parsed.is_object() {
            return Err(format!(
                "Local config {} must be a JSON object",
                path.display()
            ));
        }
        parsed
    } else if allow_create {
        Value::Object(Map::new())
    } else {
        return Err(format!("Local config {} does not exist", path.display()));
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("Local config {} must be a JSON object", path.display()))?;
    mutate(obj)?;

    let mut json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    let tmp_path = temp_config_path(path);

    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp config {}: {}", tmp_path.display(), e))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = publish_temp_config(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    Ok(root)
}

fn temp_config_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()))
}

#[cfg(not(windows))]
fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    std::fs::rename(tmp_path, path).map_err(|e| {
        format!(
            "Failed to replace {} with {}: {}",
            path.display(),
            tmp_path.display(),
            e
        )
    })
}

#[cfg(windows)]
fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return std::fs::rename(tmp_path, path).map_err(|e| {
            format!(
                "Failed to publish {} as {}: {}",
                tmp_path.display(),
                path.display(),
                e
            )
        });
    }

    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let tmp_wide: Vec<u16> = tmp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        ReplaceFileW(
            path_wide.as_ptr(),
            tmp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "Failed to replace {} with {}: {}",
            path.display(),
            tmp_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::update_config_json_object;
    use std::path::{Path, PathBuf};

    #[test]
    fn update_config_json_object_rejects_invalid_existing_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "{ invalid").unwrap();

        let err = update_config_json_object(&path, false, |_| Ok(())).unwrap_err();

        assert!(err.contains("Failed to parse"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ invalid");
    }

    #[test]
    fn update_config_json_object_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, r#"{"identity":"../../_agent_a","tooling":{"x":1}}"#).unwrap();

        let value = update_config_json_object(&path, false, |obj| {
            obj.insert("context".to_string(), serde_json::json!(["Role.md"]));
            Ok(())
        })
        .unwrap();

        assert_eq!(value["identity"], "../../_agent_a");
        assert_eq!(value["tooling"]["x"], 1);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["context"][0], "Role.md");
    }

    #[test]
    fn agent_replica_root_config_writes_go_through_shared_helper() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let roots = [manifest.join("src/config"), manifest.join("src/commands")];
        let mut offenders = Vec::new();
        for root in roots {
            scan_dir(&root, &mut offenders);
        }

        assert!(
            offenders.is_empty(),
            "local config writers must use update_config_json_object:\n{}",
            offenders.join("\n")
        );
    }

    fn scan_dir(root: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir(&path, offenders);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            scan_file(&path, offenders);
        }
    }

    fn scan_file(path: &Path, offenders: &mut Vec<String>) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let stripped = strip_test_modules(&content);
        for (idx, line) in stripped.lines().enumerate() {
            let trimmed = line.trim();
            if !line_mentions_direct_write(trimmed) || !line_mentions_local_config_target(trimmed) {
                continue;
            }
            if is_allowed_line(path, trimmed) {
                continue;
            }
            offenders.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
        }
    }

    fn line_mentions_direct_write(line: &str) -> bool {
        line.contains("std::fs::write")
            || line.contains("File::create")
            || line.contains("OpenOptions::new")
            || line.contains(".write_all")
    }

    fn line_mentions_local_config_target(line: &str) -> bool {
        line.contains("config.json")
            || line.contains("join(\"config.json\")")
            || line.contains("config_path")
    }

    fn is_allowed_line(path: &Path, line: &str) -> bool {
        let normalized = path.to_string_lossy().replace('\\', "/");
        normalized.ends_with("src/config/local_config_io.rs")
            || (normalized.ends_with("src/commands/entity_creation.rs")
                && (line.contains("write_team_config")
                    || line.contains("create_new_team_config_on_disk")
                    || line.contains("team_dir.join(\"config.json\")")))
    }

    fn strip_test_modules(content: &str) -> String {
        let mut out = String::with_capacity(content.len());
        let mut lines = content.lines();
        while let Some(line) = lines.next() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                out.push('\n');
                if let Some(next) = lines.next() {
                    if next.trim_start().starts_with("mod tests") {
                        out.push_str(&blank_braced_block(next, &mut lines));
                        continue;
                    }
                    out.push_str(next);
                    out.push('\n');
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    fn blank_braced_block<'a>(
        first_line: &str,
        lines: &mut impl Iterator<Item = &'a str>,
    ) -> String {
        let mut blanked = String::new();
        let mut depth = brace_delta(first_line);
        blanked.push('\n');
        while depth > 0 {
            let Some(line) = lines.next() else {
                break;
            };
            depth += brace_delta(line);
            blanked.push('\n');
        }
        blanked
    }

    fn brace_delta(line: &str) -> i32 {
        let opens = line.chars().filter(|ch| *ch == '{').count() as i32;
        let closes = line.chars().filter(|ch| *ch == '}').count() as i32;
        opens - closes
    }
}
