use super::super::replica_read::projected_path_string;
use super::{scan_agents, DiscoveryScan};
use crate::config::ac_root::ac_root_for_project;
use crate::config::settings::AppSettings;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    base: PathBuf,
    child: PathBuf,
}

fn make_dirs(ac_root: &Path, names: &[&str]) {
    for name in names {
        fs::create_dir_all(ac_root.join(name)).expect("create fixture dir");
    }
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let base = root.join("base");
    let child = base.join("child");

    let base_ac = ac_root_for_project(&base);
    make_dirs(
        &base_ac,
        &[
            "_agent_alpha",
            "_agent_beta",
            "room-1-dev/__agent_r1",
            "wg-2-old",
            "_team_core",
            "_team_broken",
            "notes",
        ],
    );
    let team_config = crate::config::config_file_path(&base_ac.join("_team_core"));
    fs::write(team_config, r#"{"agents":[],"coordinator":null}"#).expect("team config");
    fs::write(base_ac.join("_agent_file"), "not a dir").expect("agent file");

    make_dirs(&ac_root_for_project(&child), &["_agent_gamma", "room-3-z"]);
    make_dirs(&ac_root_for_project(&base.join(".hidden")), &["_agent_eps"]);
    fs::create_dir_all(base.join("plain")).expect("plain dir");

    Fixture {
        _tmp: tmp,
        root,
        base,
        child,
    }
}

fn settings(paths: &[&Path]) -> AppSettings {
    AppSettings {
        project_paths: paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        ..AppSettings::default()
    }
}

async fn run(cfg: &AppSettings) -> DiscoveryScan {
    scan_agents(cfg, &HashMap::new(), 0).await
}

#[tokio::test]
async fn scan_agents_lists_matrices_rooms_and_teams_in_order() {
    let fx = fixture();
    let missing = fx.root.join("missing");
    let scan = run(&settings(&[&missing, &fx.base])).await;

    let agents: Vec<&str> = scan.agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(agents, ["base/alpha", "base/beta", "child/gamma"]);
    let workgroups: Vec<&str> = scan.workgroups.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(workgroups, ["room-1-dev", "room-3-z", "wg-2-old"]);
    let teams: Vec<&str> = scan.teams.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(teams, ["core"]);
    assert_eq!(scan.workgroups[0].agents.len(), 1);
    assert!(scan.loops.is_empty());
}

#[tokio::test]
async fn scan_agents_maps_each_workgroup_to_its_project_dir() {
    let fx = fixture();
    let missing = fx.root.join("missing");
    let scan = run(&settings(&[&missing, &fx.base])).await;

    let mut map: Vec<(String, String)> = scan.wg_project_map.into_iter().collect();
    map.sort();
    let base = projected_path_string(&fx.base);
    let child = projected_path_string(&fx.child);
    assert_eq!(
        map,
        [
            ("room-1-dev".to_string(), base.clone()),
            ("room-3-z".to_string(), child),
            ("wg-2-old".to_string(), base),
        ]
    );
}

#[tokio::test]
async fn scan_agents_skips_missing_path_and_hidden_child() {
    let fx = fixture();
    let missing = fx.root.join("missing");
    let empty = run(&settings(&[&missing])).await;
    assert!(empty.agents.is_empty());
    assert!(empty.teams.is_empty());
    assert!(empty.workgroups.is_empty());
    assert!(empty.loops.is_empty());
    assert!(empty.wg_project_map.is_empty());

    let scan = run(&settings(&[&missing, &fx.base])).await;
    let names = scan
        .agents
        .iter()
        .map(|a| a.name.as_str())
        .chain(scan.workgroups.iter().map(|w| w.name.as_str()));
    for name in names {
        for bad in ["eps", "file", "notes"] {
            assert!(!name.contains(bad), "{name} must not contain {bad}");
        }
    }
}
