#![cfg(test)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Frozen env-var protocol a Stage E parent sets before spawning a child.
pub(crate) const ACTION_VAR: &str = "AC_STAGE_D_LOCK_ORDER_ACTION";
pub(crate) const NONCE_VAR: &str = "AC_STAGE_D_LOCK_ORDER_NONCE";
pub(crate) const CONTROL_DIR_VAR: &str = "AC_STAGE_D_LOCK_ORDER_CONTROL_DIR";

/// Frozen child-mode actions (one per helper).
pub(crate) const MEMBER_ACTION: &str = "cli-member-lock-order";
pub(crate) const WORKGROUP_ACTION: &str = "cli-workgroup-lock-order";

/// Enable flag for the in-process driven-path PROOF tests
/// (`*_lock_order_inversion_driver`). This is NOT part of the Stage E child-mode
/// protocol above: a real Stage E parent spawns the frozen-named CHILD helpers,
/// never the drivers. It exists so a broad `cargo test --lib -- --ignored` run
/// does NOT execute the in-process drivers, which set process-global env and pin
/// the once-cached config directory (see `config::instance_location`); doing that
/// in a shared, multi-threaded ignored run would corrupt other tests or risk
/// touching the real config dir. The drivers no-op unless this flag is present,
/// so they run only when enabled deliberately and in isolation under
/// `--test-threads=1`.
pub(crate) const DRIVE_VAR: &str = "AC_STAGE_D_LOCK_ORDER_DRIVE";

/// True only when the in-process driven-path proof is explicitly enabled via
/// `DRIVE_VAR`; a bare `--ignored` run leaves it unset so the drivers no-op.
pub(crate) fn driver_enabled() -> bool {
    std::env::var_os(DRIVE_VAR).is_some()
}

/// A validated child-mode context: present only when the frozen action, a
/// non-empty nonce, and an existing control directory are all supplied.
pub(crate) struct ChildContext {
    pub(crate) nonce: String,
    pub(crate) control_dir: PathBuf,
}

/// Pure tuple validation, unit-testable without touching env (only a control-dir
/// existence probe). Returns `None` (no-op) unless all three inputs are present
/// and valid and the action matches `expected_action`.
pub(crate) fn context_from(
    expected_action: &str,
    action: Option<&str>,
    nonce: Option<&str>,
    control_dir: Option<&str>,
) -> Option<ChildContext> {
    if action? != expected_action {
        return None;
    }
    let nonce = nonce?.trim();
    if nonce.is_empty() {
        return None;
    }
    let control_dir = PathBuf::from(control_dir?);
    if !control_dir.is_dir() {
        return None;
    }
    Some(ChildContext {
        nonce: nonce.to_string(),
        control_dir,
    })
}

/// Read the child-mode tuple from the environment; `None` (no-op) when absent.
pub(crate) fn child_context(expected_action: &str) -> Option<ChildContext> {
    let action = std::env::var(ACTION_VAR).ok();
    let nonce = std::env::var(NONCE_VAR).ok();
    let control_dir = std::env::var(CONTROL_DIR_VAR).ok();
    context_from(
        expected_action,
        action.as_deref(),
        nonce.as_deref(),
        control_dir.as_deref(),
    )
}

impl ChildContext {
    /// The lock-order barrier body: report project-gate acquisition to the parent
    /// over stdout and a `reached-<nonce>` marker, then wait a finite time for the
    /// parent's `release-<nonce>` marker before returning so the deletion proceeds
    /// to the team guard. The finite watchdog means a dead parent cannot hang it.
    pub(crate) fn report_and_wait(&self) {
        let _ = std::fs::write(
            self.control_dir.join(format!("reached-{}", self.nonce)),
            b"reached",
        );
        println!("STAGE_D_LOCK_ORDER_REACHED {}", self.nonce);
        let release = self.control_dir.join(format!("release-{}", self.nonce));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if release.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Build and register an owned workgroup fixture (settings + `.ac` + team +
    /// `wg-1-dev-team` + the `_agent_*` matrices and member replica) under the
    /// control dir, isolating the CLI config directory via the debug
    /// `AGENTSCOMMANDER_TEST_CONFIG_DIR` override. Returns the project folder name
    /// to pass as `--project`. Runs only inside a driven child, never on no-op.
    pub(crate) fn build_workgroup_fixture(&self) -> String {
        let root = self.control_dir.join(format!("fixture-{}", self.nonce));
        let config_dir = root.join("config");
        let ac_root = root.join("Project").join(".ac");
        for dir in [
            config_dir.as_path(),
            &ac_root.join("_agent_coordinator"),
            &ac_root.join("_agent_member"),
            &ac_root.join("wg-1-dev-team").join("__agent_coordinator"),
            &ac_root.join("wg-1-dev-team").join("__agent_member"),
        ] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config_dir);
        let settings = serde_json::json!({
            "defaultShell": "powershell.exe",
            "defaultShellArgs": [],
            "agents": [],
            "projectPaths": [root.to_string_lossy().to_string()],
        });
        std::fs::write(
            config_dir.join("settings.json"),
            serde_json::to_string_pretty(&settings).expect("settings json"),
        )
        .expect("write settings");
        let team_config = crate::commands::entity_creation::TeamConfigResult {
            agents: vec![
                "_agent_coordinator".to_string(),
                "_agent_member".to_string(),
            ],
            coordinator: "_agent_coordinator".to_string(),
            repos: Vec::new(),
            context_alert_percentages: Vec::new(),
        };
        crate::commands::entity_creation::create_new_team_config_on_disk(
            &ac_root,
            "dev-team",
            &team_config,
        )
        .expect("team config");
        "Project".to_string()
    }
}

/// Save/restore a set of env vars so an in-process driven-path test never leaks
/// into the parallel suite. Restores on drop, including on panic.
pub(crate) struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    pub(crate) fn capture(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
