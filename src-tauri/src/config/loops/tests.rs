use super::*;

mod atomic_writes;
mod config_storage;
mod issue_2682_lock;
mod state_rename_migration;

fn fixture_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ac_root = tmp.path().join(".ac");
    let team_dir = ac_root.join("_team_dev-team");
    let matrix = ac_root.join("_agent_tech-lead");
    let wg = ac_root.join("wg-1-dev-team");
    let replica = wg.join("__agent_tech-lead");
    for dir in [&team_dir, &matrix, &replica] {
        std::fs::create_dir_all(dir).expect("create fixture dir");
    }
    std::fs::write(matrix.join("Role.md"), "# Tech Lead\n").expect("role");
    std::fs::write(
        team_dir.join("config.json"),
        r#"{"agents":["_agent_tech-lead"],"coordinator":"_agent_tech-lead","repos":[]}"#,
    )
    .expect("team config");
    std::fs::write(
        replica.join("config.json"),
        r#"{"identity":"../../_agent_tech-lead"}"#,
    )
    .expect("replica config");
    tmp
}

fn sample_config() -> LoopConfigToml {
    LoopConfigToml {
        loop_def: LoopDef {
            id: "weekday-standup".to_string(),
            name: "Weekday standup".to_string(),
            enabled: true,
        },
        trigger: LoopTrigger {
            kind: LoopTriggerKind::Cron,
            expr: "0 9 * * 1-5".to_string(),
            timezone: LOOP_TIMEZONE_LOCAL.to_string(),
        },
        target: LoopTarget {
            kind: LoopTargetKind::WorkgroupCoordinator,
            workgroup: "wg-1-dev-team".to_string(),
        },
        prompt: LoopPrompt {
            body: "Summarize status".to_string(),
        },
        policy: LoopPolicy::default(),
    }
}

fn tmp_files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read loop dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

/// Opens `path` with the default (fully shared) OpenOptions and keeps the
/// handle for `hold`, from another thread.
#[cfg(windows)]
fn hold_open_for(path: &Path, hold: std::time::Duration) -> std::thread::JoinHandle<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .expect("open holder");
    std::thread::spawn(move || {
        std::thread::sleep(hold);
        drop(file);
    })
}
