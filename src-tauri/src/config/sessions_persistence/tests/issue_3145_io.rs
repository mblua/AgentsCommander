use super::super::*;

#[tokio::test]
async fn issue_3145_legacy_recipe_untouched_and_missing_directory_gc() {
    let directory = tempfile::tempdir().unwrap();
    let domain = directory.path().join("domain");
    std::fs::create_dir(&domain).unwrap();
    let legacy = concat!(
        r#"[{"name":"legacy","shell":"shell","shell_args":[],"#,
        r#""working_directory":"legacy","last_prompt":"A"}]"#
    )
    .as_bytes();
    let path = directory.path().join("sessions.json");
    std::fs::write(&path, legacy).unwrap();
    let manager = SessionManager::new();
    initialize_prompt_retention_in_dir(&manager, directory.path()).await;
    assert_eq!(std::fs::read(&path).unwrap(), legacy);
    let (key, value) = entry(&domain);
    manager
        .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
        .await;
    std::fs::remove_dir(&domain).unwrap();
    let guard = sessions_save_lock().lock().await;
    flush_retained_prompts_locked(&manager, directory.path())
        .await
        .unwrap();
    let disk = load_retained_prompts(directory.path()).unwrap();
    let ram = manager.retained_prompt_snapshot().await.unwrap();
    if cfg!(windows) {
        assert_eq!(disk[&key].last_prompt.as_deref(), Some("A"));
        assert_eq!(ram[&key].last_prompt.as_deref(), Some("A"));
    } else {
        assert!(disk.is_empty());
        assert!(ram.is_empty());
    }
    drop(guard);
}

#[tokio::test]
async fn issue_3145_gc_policy_preserves_unproven_entries_in_ram_sidecar_and_reload() {
    use std::io::ErrorKind;
    // Exercise both policies on every host. Inject only metadata outcomes;
    // snapshots, forgetting, atomic writes, codec and reload stay real.
    for windows in [false, true] {
        for metadata_result in [
            Ok(()),
            Err(ErrorKind::NotFound),
            Err(ErrorKind::PermissionDenied),
            Err(ErrorKind::Other),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let domain = directory.path().join("MixedCaseDomain");
            std::fs::create_dir(&domain).unwrap();
            let (key, value) = entry(&domain);
            if !windows && metadata_result == Err(ErrorKind::NotFound) {
                std::fs::remove_dir(&domain).unwrap();
            }
            let manager = SessionManager::new();
            manager
                .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
                .await;
            let guard = sessions_save_lock().lock().await;
            flush_retained_prompts_locked_with_probe(&manager, directory.path(), windows, |_| {
                metadata_result
            })
            .await
            .unwrap();
            let retained = windows || metadata_result != Err(ErrorKind::NotFound);
            let ram = manager.retained_prompt_snapshot().await.unwrap();
            let disk = load_retained_prompts(directory.path()).unwrap();
            assert_eq!(ram.contains_key(&key), retained);
            assert_eq!(disk.contains_key(&key), retained);
            let reloaded = SessionManager::new();
            initialize_prompt_retention_in_dir(&reloaded, directory.path()).await;
            let after_reload = reloaded.retained_prompt_snapshot().await.unwrap();
            assert_eq!(after_reload.contains_key(&key), retained);
            if retained {
                assert!(
                    domain.is_dir(),
                    "unproven deletion must preserve the existing domain"
                );
                assert_eq!(ram[&key].last_prompt.as_deref(), Some("A"));
                assert_eq!(disk[&key].last_prompt.as_deref(), Some("A"));
                assert_eq!(after_reload[&key].last_prompt.as_deref(), Some("A"));
            } else {
                assert!(!domain.exists());
                assert!(ram.is_empty());
                assert!(disk.is_empty());
                assert!(after_reload.is_empty());
            }
            drop(guard);
        }
    }
}

fn entry(cwd: &Path) -> (PromptKey, RetainedPrompt) {
    (
        prompt_key(cwd.to_str().unwrap(), Some("codex"), Some("A"), Some("B")).unwrap(),
        RetainedPrompt {
            last_prompt: Some("A".into()),
            blocked: false,
        },
    )
}

#[test]
fn issue_3145_key_requires_exact_host_directory_agent_and_profiles() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().to_str().unwrap();
    let (key, _) = entry(directory.path());
    let alias = directory.path().join(".");
    assert_eq!(
        Some(key.clone()),
        prompt_key(alias.to_str().unwrap(), Some("codex"), Some("A"), Some("B"))
    );
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        Some(key.clone()),
        prompt_key(
            other.path().to_str().unwrap(),
            Some("codex"),
            Some("A"),
            Some("B")
        )
    );
    for (agent, requested, effective) in [
        (Some("claude"), Some("A"), Some("B")),
        (Some("codex"), None, Some("B")),
        (Some("codex"), Some("B"), Some("B")),
        (Some("codex"), Some("A"), Some("C")),
    ] {
        assert_ne!(
            Some(key.clone()),
            prompt_key(cwd, agent, requested, effective)
        );
    }
    assert!(prompt_key(cwd, None, Some("A"), Some("B")).is_none());
    assert!(prompt_key(cwd, Some("codex"), Some("A"), None).is_none());
    assert!(prompt_key(
        directory.path().join("missing").to_str().unwrap(),
        Some("codex"),
        Some("A"),
        Some("B")
    )
    .is_none());
}

#[tokio::test]
async fn issue_3145_checkpoint_roundtrip_empty_and_no_ghost_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let domain = tempfile::tempdir().unwrap();
    let manager = SessionManager::new();
    initialize_prompt_retention_in_dir(&manager, directory.path()).await;
    assert!(manager.retained_prompt_snapshot().await.unwrap().is_empty());
    let (key, mut value) = entry(domain.path());
    value.last_prompt = Some(String::new());
    manager
        .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
        .await;
    persist_current_state_to_dir_result(&manager, directory.path())
        .await
        .unwrap();
    let restored = SessionManager::new();
    initialize_prompt_retention_in_dir(&restored, directory.path()).await;
    assert_eq!(
        restored.retained_prompt_snapshot().await.unwrap()[&key]
            .last_prompt
            .as_deref(),
        Some("")
    );
    assert!(restored.list_sessions().await.is_empty());
    assert_eq!(
        std::fs::read(directory.path().join("sessions.json")).unwrap(),
        b"[]"
    );
    // Initialization is shared and cannot replace an already installed map.
    let (a, b) = tokio::join!(
        initialize_prompt_retention_in_dir(&restored, domain.path()),
        initialize_prompt_retention_in_dir(&restored, domain.path())
    );
    assert_eq!((a, b), ((), ()));
    assert_eq!(
        restored.retained_prompt_snapshot().await.unwrap()[&key]
            .last_prompt
            .as_deref(),
        Some("")
    );
}

#[tokio::test]
async fn issue_3145_corrupt_unknown_and_unreadable_preserve_checkpoint() {
    for bytes in [b"broken".as_slice(), br#"{"version":2,"entries":[]}"#] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(RETAINED_PROMPTS);
        std::fs::write(&path, bytes).unwrap();
        let manager = SessionManager::new();
        initialize_prompt_retention_in_dir(&manager, directory.path()).await;
        assert!(manager.retained_prompt_snapshot().await.is_none());
        persist_current_state_to_dir_result(&manager, directory.path())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join(RETAINED_PROMPTS)).unwrap();
    let manager = SessionManager::new();
    initialize_prompt_retention_in_dir(&manager, directory.path()).await;
    assert!(manager.retained_prompt_snapshot().await.is_none());
}

#[tokio::test]
async fn issue_3145_duplicate_keys_stay_blocked_after_reload() {
    let directory = tempfile::tempdir().unwrap();
    let domain = tempfile::tempdir().unwrap();
    let manager = SessionManager::new();
    let (key, value) = entry(domain.path());
    manager
        .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
        .await;
    let guard = sessions_save_lock().lock().await;
    flush_retained_prompts_locked(&manager, directory.path())
        .await
        .unwrap();
    let path = directory.path().join(RETAINED_PROMPTS);
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let duplicate = json["entries"][0].clone();
    json["entries"].as_array_mut().unwrap().push(duplicate);
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let loaded = load_retained_prompts(directory.path()).unwrap();
    assert!(loaded[&key].blocked);
    assert!(loaded[&key].last_prompt.is_none());
    manager.install_retained_prompts(Some(loaded)).await;
    flush_retained_prompts_locked(&manager, directory.path())
        .await
        .unwrap();
    assert!(load_retained_prompts(directory.path()).unwrap()[&key].blocked);
    drop(guard);
}

#[tokio::test]
async fn issue_3145_write_rename_failure_retry_preserves_foreign_temp() {
    let directory = tempfile::tempdir().unwrap();
    let domain = tempfile::tempdir().unwrap();
    let manager = SessionManager::new();
    let (key, value) = entry(domain.path());
    manager
        .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
        .await;
    let guard = sessions_save_lock().lock().await;
    let foreign = directory
        .path()
        .join("last-prompts.retained.json.foreign.tmp");
    std::fs::write(&foreign, b"keep").unwrap();
    let destination = directory.path().join(RETAINED_PROMPTS);
    std::fs::create_dir(&destination).unwrap();
    assert!(flush_retained_prompts_locked(&manager, directory.path())
        .await
        .is_err());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    std::fs::remove_dir(&destination).unwrap();
    flush_retained_prompts_locked(&manager, directory.path())
        .await
        .unwrap();
    assert_eq!(
        load_retained_prompts(directory.path()).unwrap()[&key]
            .last_prompt
            .as_deref(),
        Some("A")
    );
    let invalid = directory.path().join("not-directory");
    std::fs::write(&invalid, b"keep").unwrap();
    assert!(flush_retained_prompts_locked(&manager, &invalid)
        .await
        .is_err());
    assert_eq!(std::fs::read(&invalid).unwrap(), b"keep");
    drop(guard);
}

#[tokio::test]
async fn issue_3145_old_preparation_waiting_for_lock_cannot_restore_invalidated_value() {
    let directory = tempfile::tempdir().unwrap();
    let domain = tempfile::tempdir().unwrap();
    let manager = SessionManager::new();
    let (key, value) = entry(domain.path());
    manager
        .install_retained_prompts(Some(HashMap::from([(key.clone(), value)])))
        .await;
    let guard = sessions_save_lock().lock().await;
    let mut persist = Box::pin(persist_current_state_to_dir_for_project_paths_result(
        &manager,
        directory.path(),
        None,
        PersistMode::NoPrune,
    ));
    use std::future::Future;
    let waker = futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    assert!(persist.as_mut().poll(&mut context).is_pending());
    manager
        .install_retained_prompts(Some(HashMap::from([(
            key.clone(),
            RetainedPrompt::default(),
        )])))
        .await;
    drop(guard);
    persist.await.unwrap();
    assert_eq!(
        load_retained_prompts(directory.path()).unwrap()[&key].last_prompt,
        None
    );
}
