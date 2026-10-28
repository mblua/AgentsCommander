use super::*;
use std::fs;

#[test]
fn issue_3294_discovery_positive_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing");
    assert_eq!(determine_next_wg_number(&root), 1);
    assert!(team_room_candidates(&root, "alpha").is_empty());
    assert!(workgroup_candidates(&root).is_empty());
    assert_eq!(
        replica_candidates(&root).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("wg-1-alpha")).unwrap();
    assert_eq!(determine_next_wg_number(&root), 1);
    for name in [
        "room-1-beta",
        "room-3-alpha",
        "room-0-alpha",
        "room-2-",
        "room-x-alpha",
    ] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("room-2-alpha"), "file is not a directory").unwrap();
    assert_eq!(determine_next_wg_number(&root), 2);
    fs::create_dir(root.join("room-2-beta")).unwrap();
    assert_eq!(determine_next_wg_number(&root), 4);
    assert_eq!(
        workgroup_candidates(&root),
        vec![
            root.join("room-1-beta"),
            root.join("room-2-beta"),
            root.join("room-3-alpha"),
            root.join("wg-1-alpha"),
        ]
    );

    // Inject an entry error between observations without a global fake filesystem.
    let error = || io::Error::other("unreadable entry");
    assert_eq!(
        room_slots_from_observations([
            Ok(OsString::from("room-3-alpha")),
            Err(error()),
            Ok(OsString::from("room-1-beta")),
        ]),
        vec![3, 1]
    );
    assert_eq!(
        team_rooms_from_observations(
            [
                Ok(root.join("room-3-alpha")),
                Err(error()),
                Ok(root.join("wg-1-alpha")),
                Ok(root.join("room-2-other-alpha")),
            ],
            "alpha"
        ),
        vec![root.join("room-3-alpha"), root.join("wg-1-alpha")]
    );
    assert_eq!(
        workgroups_from_observations([
            Ok(root.join("room-3-alpha")),
            Err(error()),
            Ok(root.join("wg-1-alpha")),
            Ok(root.join("room-1-alpha")),
        ]),
        vec![
            root.join("room-1-alpha"),
            root.join("room-3-alpha"),
            root.join("wg-1-alpha")
        ]
    );
    assert_eq!(
        replicas_from_observations([
            Ok(root.join("__agent_z")),
            Err(error()),
            Ok(root.join("__agent_a")),
            Ok(root.join("_agent_matrix")),
        ]),
        vec![root.join("__agent_z"), root.join("__agent_a")]
    );

    let room = root.join("room-3-alpha");
    fs::create_dir(room.join("__agent_z")).unwrap();
    fs::create_dir(room.join("__agent_a")).unwrap();
    fs::write(room.join("__agent_file"), "not a replica directory").unwrap();
    let native: Vec<PathBuf> = fs::read_dir(&room)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    assert_eq!(replica_candidates(&room).unwrap(), native);
    let native: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.file_name().unwrap().to_str().unwrap(),
                "wg-1-alpha" | "room-3-alpha" | "room-0-alpha"
            )
        })
        .collect();
    assert_eq!(team_room_candidates(&root, "alpha"), native);
    assert!(team_directory_present(&room));
    assert!(!replica_role_present(&room.join("Role.md")));
    fs::write(room.join("Role.md"), "role").unwrap();
    assert!(replica_role_present(&room.join("Role.md")));
    assert!(clone_target_present(&room.join("Role.md")));
    assert!(!clone_target_present(&room.join("repo-missing")));
}
