use std::path::{Path, PathBuf};
use std::process::Command;

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

fn config_dir_for_bin(bin: &Path) -> PathBuf {
    let stem = bin
        .file_stem()
        .expect("bin stem")
        .to_string_lossy()
        .to_string();
    bin.parent().expect("bin parent").join(format!(".{}", stem))
}

fn seed_cache(config_dir: &Path) {
    let cache = config_dir
        .join("agency-agents_templates")
        .join("testing")
        .join("accessibility-auditor");
    std::fs::create_dir_all(&cache).expect("create cache");
    std::fs::write(
        cache.join("Role.md"),
        "---\nname: Accessibility Auditor\ndescription: A11y checks\n---\n\n# Body\n",
    )
    .expect("write role");
    std::fs::write(
        config_dir
            .join("agency-agents_templates")
            .join("manifest.json"),
        serde_json::json!({
            "repo": "https://github.com/msitarzewski/agency-agents",
            "ref": "main",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "templateCount": 1
        })
        .to_string(),
    )
    .expect("write manifest");
}

#[test]
fn agency_templates_status_missing_cache_returns_json() {
    let tmp = Tmp::new("agency-status-missing");
    let bin = copy_binary_into(tmp.path());
    let output = Command::new(&bin)
        .args(["agency-templates", "status", "--json"])
        .output()
        .expect("run status");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("status json");
    assert_eq!(value["available"], false);
    assert_eq!(value["reason"], "missing");
}

#[test]
fn agency_templates_list_missing_cache_returns_empty_array() {
    let tmp = Tmp::new("agency-list-missing");
    let bin = copy_binary_into(tmp.path());
    let output = Command::new(&bin)
        .args(["agency-templates", "list", "--json"])
        .output()
        .expect("run list");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("list json");
    assert_eq!(value, serde_json::json!([]));
}

#[test]
fn agency_templates_list_pretty_returns_cached_metadata() {
    let tmp = Tmp::new("agency-list-pretty");
    let bin = copy_binary_into(tmp.path());
    let config = config_dir_for_bin(&bin);
    seed_cache(&config);

    let output = Command::new(&bin)
        .args(["agency-templates", "list", "--pretty"])
        .output()
        .expect("run list");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.trim_start().starts_with('['));
    assert!(stdout.contains('\n'));
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("list json");
    assert_eq!(value[0]["id"], "agency:testing-accessibility-auditor");
    assert_eq!(value[0]["hasSkills"], false);
}

#[test]
fn agency_templates_status_reports_locked_cache() {
    let tmp = Tmp::new("agency-status-locked");
    let bin = copy_binary_into(tmp.path());
    let config = config_dir_for_bin(&bin);
    std::fs::create_dir_all(&config).expect("create config");
    let lock_path = config.join("agency-agents_templates.lock");
    let _lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .expect("hold lock");

    let output = Command::new(&bin)
        .args(["agency-templates", "status"])
        .output()
        .expect("run status");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("status json");
    assert_eq!(value["available"], false);
    assert_eq!(value["reason"], "locked");
}
