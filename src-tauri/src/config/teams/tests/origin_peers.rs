use super::*;

use std::path::PathBuf;

/// Builds the `discover_origin_peers` replica-scan fixture under `tmp` and
/// returns `(parent, alpha_replica_path)`.
fn make_origin_peers_fixture(tmp: &Path) -> (PathBuf, PathBuf) {
    let parent = tmp.join("parent");
    let proj_ac = crate::config::ac_root::ac_root_for_project(&parent.join("proj"));
    let room = proj_ac.join("room-1-dev-team");
    let alpha = room.join("__agent_alpha");
    for dir in [
        alpha.clone(),
        room.join("__agent_beta"),
        room.join("notes"),
        proj_ac.join("wg-2-legacy-team").join("__agent_gamma"),
        proj_ac.join("plain-dir").join("__agent_delta"),
        crate::config::ac_root::ac_root_for_project(&parent.join(".hidden"))
            .join("room-3-x")
            .join("__agent_eps"),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    std::fs::write(room.join("__agent_file"), "not a dir").unwrap();
    (parent, alpha)
}

fn sorted_peer_names(peers: &[PeerInfo]) -> Vec<String> {
    let mut names: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
    names.sort();
    names
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn origin_peers_lists_wg_replicas_with_team_and_skips_self() {
    let temp = tempfile::TempDir::new().unwrap();
    let (parent, alpha) = make_origin_peers_fixture(temp.path());
    let root = path_string(&alpha);

    let peers = super::discovery::discover_origin_peers(&root, &[path_string(&parent)]);

    assert_eq!(
        sorted_peer_names(&peers),
        vec![
            "proj:room-1-dev-team/beta".to_string(),
            "proj:wg-2-legacy-team/gamma".to_string(),
        ]
    );
    let self_name = agent_fqn_from_path(&root);
    assert!(peers.iter().all(|p| p.name != self_name));
    let beta = peers
        .iter()
        .find(|p| p.name == "proj:room-1-dev-team/beta")
        .unwrap();
    assert_eq!(beta.teams, vec!["dev-team".to_string()]);
    let gamma = peers
        .iter()
        .find(|p| p.name == "proj:wg-2-legacy-team/gamma")
        .unwrap();
    assert_eq!(gamma.teams, vec!["legacy-team".to_string()]);
}

#[test]
fn origin_peers_dedupes_same_project_listed_twice() {
    let temp = tempfile::TempDir::new().unwrap();
    let (parent, _) = make_origin_peers_fixture(temp.path());
    let root = path_string(&temp.path().join("origin").join("_agent_outside"));
    let proj = path_string(&parent.join("proj"));

    let peers =
        super::discovery::discover_origin_peers(&root, &[path_string(&parent), proj.clone(), proj]);

    assert_eq!(
        sorted_peer_names(&peers),
        vec![
            "proj:room-1-dev-team/alpha".to_string(),
            "proj:room-1-dev-team/beta".to_string(),
            "proj:wg-2-legacy-team/gamma".to_string(),
        ]
    );
}

#[test]
fn origin_peers_skips_missing_project_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let (parent, alpha) = make_origin_peers_fixture(temp.path());
    let root = path_string(&alpha);
    let missing = path_string(&temp.path().join("missing"));

    let with_missing =
        super::discovery::discover_origin_peers(&root, &[missing, path_string(&parent)]);
    let parent_only = super::discovery::discover_origin_peers(&root, &[path_string(&parent)]);

    assert_eq!(
        sorted_peer_names(&with_missing),
        sorted_peer_names(&parent_only)
    );
}
