use super::*;

#[test]
fn p05_production_workgroup_gitignore_reads_custom_names_without_managed_writes() {
    for mode in 0..3 {
        assert_p03_gitignore_healthy_mode(mode);
    }
    // The optional helper continues, then protected replica preflight fails.
    for mode in 0..3 {
        assert_p03_gitignore_partial_retry_mode(mode);
    }
}

fn assert_p03_gitignore_healthy_mode(mode: usize) {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    stage_p03_source(&fixture, mode);
    let before = fixture.managed_bytes();
    run_success(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P05 created room",
        ],
    );
    assert_eq!(
        fixture.managed_bytes(),
        before,
        "production gitignore loader wrote managed config"
    );
    let ignore = std::fs::read_to_string(fixture.project.join(".ac/.gitignore")).unwrap();
    assert!(
        ignore.contains("**/__agent_*/P05-INSTRUCTIONS.md"),
        "{ignore}"
    );
    let rooms = run_json(
        &fixture.bin,
        &["workgroup", "list", "--project", "RegisteredRemote"],
    )
    .0;
    assert_eq!(rooms.as_array().unwrap().len(), 2);
}

fn assert_p03_gitignore_partial_retry_mode(mode: usize) {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    stage_p03_source(&fixture, mode);
    let ac = fixture.project.join(".ac");
    let list = || {
        let rooms = run_json(
            &fixture.bin,
            &["workgroup", "list", "--project", "RegisteredRemote"],
        )
        .0;
        rooms
            .as_array()
            .unwrap()
            .iter()
            .map(|room| PathBuf::from(room["path"].as_str().unwrap()))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let baseline = list(); // naming preflight before all snapshots
    assert_eq!(baseline.len(), 1);
    // Inline, bounded capture of these fixture-owned layouts; lock artifacts
    // have separate authority and are intentionally not counted.
    let capture = |root: &Path| {
        let mut pending = vec![root.to_path_buf()];
        let mut snapshot = std::collections::BTreeMap::new();
        while let Some(path) = pending.pop() {
            assert!(snapshot.len() < 128, "fixture layout exceeded bound");
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            assert!(relative.components().count() <= 4);
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".lock"))
            {
                continue;
            }
            if metadata.is_dir() {
                snapshot.insert(relative, None);
                pending.extend(
                    std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else {
                assert!(metadata.is_file() && metadata.len() <= 1024 * 1024);
                snapshot.insert(relative, Some(std::fs::read(&path).unwrap()));
            }
        }
        snapshot
    };
    let baseline_files = capture(baseline.first().unwrap());
    let config_paths = [
        ac.join("_team_devs/config.json"),
        ac.join("_agent_coord/config.json"),
        ac.join("_agent_peer/config.json"),
    ];
    let config_bytes: Vec<_> = config_paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let refresh_before: std::collections::BTreeMap<_, _> =
        project_refresh_request_paths(&fixture.config)
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
    let before = fixture.managed_bytes();
    let ignore = ac.join(".gitignore");
    assert!(!ignore.exists(), "unexpected fixture gitignore");
    std::fs::create_dir(&ignore).unwrap();
    let output = run_failure(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P03 helper failure",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err());
    let after_failure = list();
    assert!(baseline.is_subset(&after_failure));
    let added: Vec<_> = after_failure.difference(&baseline).cloned().collect();
    assert_eq!(
        added.len(),
        1,
        "optional helper must continue to partial room creation"
    );
    let stderr = String::from_utf8_lossy(&output.stderr)
        .to_lowercase()
        .replace(char::from(92), "/");
    let canonical_ignore = std::fs::canonicalize(&ignore).unwrap();
    let ignore_display = canonical_ignore
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase()
        .replace(char::from(92), "/");
    assert!(
        stderr.contains(&format!("failed to read {ignore_display}")),
        "{stderr}"
    );
    let partial = &added[0];
    let replica = assert_p03_partial_room_layout(partial, &ac);
    let partial_files = capture(partial);
    assert!(ignore.is_dir() && std::fs::read_dir(&ignore).unwrap().next().is_none());
    assert_eq!(fixture.managed_bytes(), before);
    assert_eq!(capture(baseline.first().unwrap()), baseline_files);
    assert_eq!(
        config_paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        config_bytes
    );
    let refresh_failed: std::collections::BTreeMap<_, _> =
        project_refresh_request_paths(&fixture.config)
            .into_iter()
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
    assert_eq!(refresh_failed, refresh_before);

    std::fs::remove_dir(&ignore).unwrap(); // only the known empty fixture directory
    let result = run_json(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P03 helper retry",
        ],
    )
    .0;
    let healthy = PathBuf::from(result["path"].as_str().unwrap());
    assert_p03_healthy_room_layout(&healthy, &ac, &after_failure);
    let mut expected = after_failure.clone();
    expected.insert(healthy.clone());
    assert_eq!(list(), expected);
    assert_eq!(expected.len(), 3);
    assert_eq!(capture(partial), partial_files);
    assert!(!replica.join("config.json").exists());
    assert!(!replica.join("config.state.no-git.json").exists());
    assert_eq!(capture(baseline.first().unwrap()), baseline_files);
    assert_eq!(
        config_paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        config_bytes
    );
    assert_eq!(fixture.managed_bytes(), before);
    assert_p03_retry_refresh(&fixture.config, &refresh_before, &healthy);
}

fn assert_p03_partial_room_layout(partial: &Path, ac: &Path) -> PathBuf {
    let canonical_partial = std::fs::canonicalize(partial)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", partial.display()));
    let canonical_ac = std::fs::canonicalize(ac)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", ac.display()));
    assert!(
        canonical_partial.starts_with(&canonical_ac)
            && canonical_partial.parent() == Some(canonical_ac.as_path()),
        "room {} must be a direct child of {}",
        canonical_partial.display(),
        canonical_ac.display()
    );
    assert!(partial.join("messaging").is_dir());
    assert!(partial.join("room-shared").is_dir());
    assert_eq!(
        std::fs::read_to_string(partial.join("TASK.md")).unwrap(),
        "---
title: 'USER: P03 helper failure'
---
"
    );
    let replicas: Vec<_> = std::fs::read_dir(partial)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("__agent_")
        })
        .collect();
    assert_eq!(replicas, vec![partial.join("__agent_coord")]);
    let replica = &replicas[0];
    for directory in ["inbox", "outbox"] {
        assert!(replica.join(directory).is_dir());
        assert!(std::fs::read_dir(replica.join(directory))
            .unwrap()
            .next()
            .is_none());
    }
    assert!(!replica.join("config.json").exists());
    assert!(!replica.join("config.state.no-git.json").exists());
    assert!(!partial.join("__agent_peer").exists());
    replica.clone()
}

fn assert_p03_healthy_room_layout(
    healthy: &Path,
    ac: &Path,
    after_failure: &std::collections::BTreeSet<PathBuf>,
) {
    let ignore = ac.join(".gitignore");
    let canonical_healthy = std::fs::canonicalize(healthy)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", healthy.display()));
    let canonical_ac = std::fs::canonicalize(ac)
        .unwrap_or_else(|error| panic!("canonicalize {}: {error}", ac.display()));
    assert!(
        canonical_healthy.starts_with(&canonical_ac)
            && canonical_healthy.parent() == Some(canonical_ac.as_path()),
        "room {} must be a direct child of {}",
        canonical_healthy.display(),
        canonical_ac.display()
    );
    assert!(!after_failure.contains(healthy));
    assert!(std::fs::read_to_string(&ignore)
        .unwrap()
        .contains("**/__agent_*/P05-INSTRUCTIONS.md"));
    for directory in ["messaging", "room-shared"] {
        assert!(healthy.join(directory).is_dir());
    }
    assert_eq!(
        std::fs::read_to_string(healthy.join("TASK.md")).unwrap(),
        "---
title: 'USER: P03 helper retry'
---
"
    );
    for name in ["coord", "peer"] {
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(healthy.join(format!("__agent_{name}/config.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(config["identity"], format!("../../_agent_{name}"));
    }
}

fn assert_p03_retry_refresh(
    config: &Path,
    refresh_before: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
    healthy: &Path,
) {
    let refresh_after: std::collections::BTreeMap<_, _> = project_refresh_request_paths(config)
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    for (path, bytes) in refresh_before {
        assert_eq!(refresh_after.get(path), Some(bytes));
    }
    let new_requests: Vec<_> = refresh_after
        .iter()
        .filter(|(path, _)| !refresh_before.contains_key(*path))
        .collect();
    assert_eq!(new_requests.len(), 1);
    let request: serde_json::Value = serde_json::from_slice(new_requests[0].1).unwrap();
    assert_eq!(
        PathBuf::from(request["changedPath"].as_str().unwrap()),
        std::fs::canonicalize(healthy).unwrap()
    );
    assert_eq!(request["reason"], "workgroupCreated");
}

#[test]
fn p03_production_new_project_migrates_once_and_preserves_other_sources() {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    let original = std::fs::read(fixture.config.join("settings.30.instance.no-git.json")).unwrap();
    for preexisting in [false, true] {
        let project = fixture
            ._tmp
            .path()
            .join(if preexisting { "Preexisting" } else { "Fresh" });
        if preexisting {
            std::fs::create_dir_all(project.join(".ac")).unwrap();
        }
        run_success(&fixture.bin, &["new-project", project.to_str().unwrap()]);
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.config.join("settings.30.instance.no-git.json")).unwrap(),
        )
        .unwrap();
        assert!(
            settings.get("rootToken").is_none_or(|v| v.is_null()),
            "incidental loader generated root privilege: {settings}"
        );
        assert_eq!(settings["defaultShell"], "powershell.exe");
        assert_eq!(settings["npmUpdateNotificationsEnabled"], false);
        let canonical_project = std::fs::canonicalize(&project).unwrap();
        let paths: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.config.join("project-paths.json")).unwrap(),
        )
        .unwrap();
        for key in PROJECT_KEYS {
            assert!(settings.get(key).is_none());
        }
        assert_eq!(
            std::fs::read(
                fixture
                    .config
                    .join("project-paths.pre-migration.no-git.json")
            )
            .unwrap(),
            original
        );
        assert!(paths["projectPaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| std::fs::canonicalize(v.as_str().unwrap()).unwrap() == canonical_project));
        let ignore = std::fs::read_to_string(project.join(".ac/.gitignore")).unwrap();
        assert!(
            ignore.contains("**/__agent_*/P05-INSTRUCTIONS.md"),
            "{ignore}"
        );
        assert!(paths["projectPaths"].as_array().unwrap().iter().any(
            |path| std::fs::canonicalize(path.as_str().unwrap()).unwrap()
                == std::fs::canonicalize(&fixture.project).unwrap()
        ));
        assert!(!fixture
            .config
            .join("agents.30.instance.no-git.json")
            .exists());
        let stable = fixture.managed_bytes();
        run_success(&fixture.bin, &["new-project", project.to_str().unwrap()]);
        assert_eq!(fixture.managed_bytes(), stable);
    }
}

const PROJECT_KEYS: [&str; 6] = [
    "projectPath",
    "projectPathRelativeToInstance",
    "projectPaths",
    "projectPathsRelativeToInstance",
    "archivedProjectPaths",
    "archivedProjectPathsRelativeToInstance",
];

fn stage_p03_source(fixture: &ReadonlyFixture, mode: usize) {
    let path = fixture.config.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let projection: serde_json::Map<String, serde_json::Value> = PROJECT_KEYS
        .iter()
        .filter_map(|key| {
            settings
                .get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect();
    if mode > 0 {
        std::fs::write(
            fixture.config.join("project-paths.json"),
            serde_json::to_vec(&projection).unwrap(),
        )
        .unwrap();
    }
    if mode == 1 {
        for key in PROJECT_KEYS {
            settings.as_object_mut().unwrap().remove(key);
        }
        std::fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
    }
}

struct P03SettingsLock(std::fs::File);

impl P03SettingsLock {
    fn acquire(config: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.join("settings.30.instance.no-git.json.lock"))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
                unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe {
                    LockFileEx(
                        file.as_raw_handle(),
                        LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                        0,
                        u32::MAX,
                        u32::MAX,
                        &mut overlapped,
                    )
                },
                0
            );
        }
        Self(file)
    }
}

impl Drop for P03SettingsLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            assert_eq!(unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) }, 0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
            let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
                unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe {
                    UnlockFileEx(
                        self.0.as_raw_handle(),
                        0,
                        u32::MAX,
                        u32::MAX,
                        &mut overlapped,
                    )
                },
                0
            );
        }
    }
}

#[test]
fn p03_real_readers_hold_shared_lock_and_retry_sidecar_without_managed_writes() {
    for mode in 0..3 {
        let fixture = ReadonlyFixture::new();
        stage_p03_source(&fixture, mode);
        let remote_root = fixture.config.join("ac-root-agent");
        std::fs::create_dir_all(remote_root.join("messaging")).unwrap();
        std::fs::write(
            remote_root.join("messaging").join(P05_ROOT_MESSAGE),
            "P03 message",
        )
        .unwrap();
        for verb in ["list-peers", "list-peers-lean"] {
            let peers = fixture.assert_reader(&[
                verb,
                "--token",
                P05_TOKEN,
                "--root",
                remote_root.to_str().unwrap(),
            ]);
            assert!(peers
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer["name"] == "RegisteredRemote:room-01-devs/coord"));
        }
        let held = P03SettingsLock::acquire(&fixture.config);
        let before = fixture.managed_bytes();
        let commands = vec![
            vec![
                "list-peers",
                "--token",
                P05_TOKEN,
                "--root",
                remote_root.to_str().unwrap(),
            ],
            vec![
                "list-peers-lean",
                "--token",
                P05_TOKEN,
                "--root",
                remote_root.to_str().unwrap(),
            ],
            vec![
                "send",
                "--token",
                P05_TOKEN,
                "--root",
                remote_root.to_str().unwrap(),
                "--to",
                "RegisteredRemote:room-01-devs/coord",
                "--send",
                P05_ROOT_MESSAGE,
                "--mode",
                "wake",
                "--confirm-timeout",
                "0",
            ],
            vec![
                "close-session",
                "--token",
                P05_TOKEN,
                "--root",
                &fixture.peer,
                "--target",
                "RegisteredRemote:room-01-devs/coord",
            ],
            vec![
                "create-agent-matrix",
                "--project",
                "RegisteredRemote",
                "--name",
                "P03Matrix",
                "--description",
                "P03",
            ],
        ];
        for args in &commands {
            let start = std::time::Instant::now();
            let output = run_failure(&fixture.bin, args);
            assert_eq!(output.status.code(), Some(1));
            assert!(start.elapsed() >= std::time::Duration::from_secs(2));
            assert!(!String::from_utf8_lossy(&output.stdout).contains("Queued:"));
            assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err());
            assert_eq!(fixture.managed_bytes(), before);
            fixture.assert_no_outbox();
            assert!(!fixture.project.join(".ac/_agent_p03matrix").exists());
        }
        // Explicit fixture staging models post-publication/pre-cleanup, not engine execution.
        if mode == 0 {
            stage_p03_source(&fixture, 2);
        }
        let path = fixture.config.join("settings.30.instance.no-git.json");
        let mut settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for key in PROJECT_KEYS {
            settings.as_object_mut().unwrap().remove(key);
        }
        std::fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
        drop(held);
        let before = fixture.managed_bytes();
        for args in &commands[..2] {
            let peers = fixture.assert_reader(args);
            assert!(peers
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer["name"] == "RegisteredRemote:room-01-devs/coord"));
        }
        let output = run_failure(&fixture.bin, &commands[2]);
        assert!(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.starts_with("Queued: ")));
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("delivery confirmation timeout after 0s"));
        let outbox = fixture.outbox_for_root(remote_root.to_str().unwrap());
        let queued: Vec<_> = std::fs::read_dir(&outbox)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(queued.len(), 1);
        let message: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&queued[0]).unwrap()).unwrap();
        assert_eq!(message["to"], "RegisteredRemote:room-01-devs/coord");
        std::fs::remove_file(&queued[0]).unwrap();
        fixture.assert_denied(&commands[3], "authorization denied");
        run_success(&fixture.bin, &commands[4]);
        assert!(fixture.project.join(".ac/_agent_p03matrix").is_dir());
        assert_eq!(fixture.managed_bytes(), before);
    }
}

#[test]
fn p03_real_new_project_initial_lock_refusal_and_concurrent_registration_retry() {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    let before = fixture.managed_bytes();
    let projects = [
        fixture._tmp.path().join("ConcurrentA"),
        fixture._tmp.path().join("ConcurrentB"),
    ];
    std::fs::create_dir_all(projects[1].join(".ac")).unwrap();
    let held = P03SettingsLock::acquire(&fixture.config);
    for project in &projects {
        let output = run_failure(&fixture.bin, &["new-project", project.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stdout).is_empty());
        assert_eq!(fixture.managed_bytes(), before);
        assert!(!project.join(".ac/Context.global.md").exists());
        assert!(!project.join(".ac/.gitignore").exists());
    }
    assert!(!projects[0].exists());
    drop(held);
    let start = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut children = Vec::new();
    for project in &projects {
        let start = start.clone();
        let binary = fixture.bin.clone();
        let project = project.clone();
        children.push(std::thread::spawn(move || {
            start.wait();
            run_success(&binary, &["new-project", project.to_str().unwrap()]);
        }));
    }
    start.wait();
    for child in children {
        child.join().unwrap();
    }
    let paths: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.config.join("project-paths.json")).unwrap())
            .unwrap();
    let active = paths["projectPaths"].as_array().unwrap();
    assert_eq!(active.len(), 3);
    assert_eq!(
        active.len(),
        paths["projectPathsRelativeToInstance"]
            .as_array()
            .unwrap()
            .len()
    );
    for project in [&fixture.project, &projects[0], &projects[1]] {
        assert!(active.iter().any(
            |path| std::fs::canonicalize(path.as_str().unwrap()).unwrap()
                == std::fs::canonicalize(project).unwrap()
        ));
    }
    let settings: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.config.join("settings.30.instance.no-git.json")).unwrap(),
    )
    .unwrap();
    for key in PROJECT_KEYS {
        assert!(settings.get(key).is_none());
    }
    let stable = fixture.managed_bytes();
    for project in &projects {
        run_success(&fixture.bin, &["new-project", project.to_str().unwrap()]);
        assert_eq!(fixture.managed_bytes(), stable);
    }
}

#[test]
fn p03_workgroup_initial_resolution_lock_refusal_has_no_outputs_then_retry_succeeds() {
    let fixture = ReadonlyFixture::new();
    write_p05_custom_settings(&fixture);
    let before = fixture.managed_bytes();
    let rooms = run_json(
        &fixture.bin,
        &["workgroup", "list", "--project", "RegisteredRemote"],
    )
    .0;
    let held = P03SettingsLock::acquire(&fixture.config);
    let output = run_failure(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P03 denied",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_err());
    assert_eq!(fixture.managed_bytes(), before);
    drop(held);
    assert_eq!(
        run_json(
            &fixture.bin,
            &["workgroup", "list", "--project", "RegisteredRemote"]
        )
        .0,
        rooms
    );
    run_success(
        &fixture.bin,
        &[
            "workgroup",
            "add",
            "--project",
            "RegisteredRemote",
            "--team",
            "devs",
            "--title",
            "P03 retry",
        ],
    );
    let after = run_json(
        &fixture.bin,
        &["workgroup", "list", "--project", "RegisteredRemote"],
    )
    .0;
    assert_eq!(
        after.as_array().unwrap().len(),
        rooms.as_array().unwrap().len() + 1
    );
    let ignore = std::fs::read_to_string(fixture.project.join(".ac/.gitignore")).unwrap();
    assert!(ignore.contains("**/__agent_*/P05-INSTRUCTIONS.md"));
    assert_eq!(fixture.managed_bytes(), before);
}
