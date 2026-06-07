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

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        format!("file:///{}", path)
    } else {
        format!("file://{}", path)
    }
}

fn create_agency_source_repo(root: &Path) {
    std::fs::create_dir_all(root.join("Engineering")).expect("create source division");
    std::fs::write(
        root.join("Engineering").join("Planner.md"),
        "---\nname: Planner\ndescription: Plans backend work\n---\n\nPlan backend work.\n",
    )
    .expect("write source role");
    run_git(root, &["init"]);
    run_git(root, &["checkout", "-B", "main"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test User"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "seed agency templates"]);
}

fn write_git_redirect_config(path: &Path, source_repo: &Path) {
    let body = format!(
        "[url \"{}\"]\n\tinsteadOf = https://github.com/ac-test/agency-agents\n",
        file_url(source_repo)
    );
    std::fs::write(path, body).expect("write git redirect config");
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
fn agency_templates_update_prints_json_on_success_and_noop() {
    let tmp = Tmp::new("agency-update-json");
    let bin = copy_binary_into(tmp.path());
    let source_repo = tmp.path().join("source");
    std::fs::create_dir_all(&source_repo).expect("create source repo");
    create_agency_source_repo(&source_repo);
    let git_config = tmp.path().join("gitconfig");
    write_git_redirect_config(&git_config, &source_repo);

    let run_update = || {
        Command::new(&bin)
            .env("GIT_CONFIG_GLOBAL", &git_config)
            .args([
                "agency-templates",
                "update",
                "--repo",
                "https://github.com/ac-test/agency-agents",
                "--ref",
                "main",
                "--json",
            ])
            .output()
            .expect("run update")
    };

    let output = run_update();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.is_empty(), "update stdout must contain JSON");
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("update json");
    assert_eq!(value["repo"], "https://github.com/ac-test/agency-agents");
    assert_eq!(value["ref"], "main");
    assert_eq!(value["templateCount"], 1);
    assert_eq!(value["updated"], true);
    assert!(value["commit"].as_str().unwrap_or_default().len() == 40);
    assert!(value["path"]
        .as_str()
        .unwrap_or_default()
        .contains("agency-agents_templates"));

    let output = run_update();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        !output.stdout.is_empty(),
        "noop update stdout must contain JSON"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("noop update json");
    assert_eq!(value["templateCount"], 1);
    assert_eq!(value["updated"], false);
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
