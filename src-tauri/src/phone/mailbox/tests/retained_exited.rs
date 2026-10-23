use super::*;

async fn retained_exited_respawn_host(pty: bool, inline: bool, wrapper: &str) {
    const MARKER: &str = "AC_P05_RETAINED_SOURCE_CHILD";
    const SCENARIO: &str = "AC_P05_RETAINED_SOURCE_SCENARIO";
    const IDENTITY: &str = "AC_P05_RETAINED_SOURCE_IDENTITY";
    let mut scenarios = vec![
        "positive",
        "reclaim-66",
        "walk-terminal",
        "seal-insert-first",
        "queued-drain",
        "cleanup-claim-first",
        "cleanup-finalize-first",
        "cleanup-runtime-changed",
        "cleanup-unknown-veto",
        "root-duplicate",
        "root-full",
        "root-poison",
        "root-close-first",
        "newer-telegram",
        "newer-full",
        "under-commit-telegram",
        "under-commit-full",
        "lifetime-source-panic",
        "call-before-accept",
        "call-after-accept",
        "seed-positive",
        "seed-staging-file",
        "pre-backend",
        "pre-context",
        "pre-finalize",
        "post-carry-runtime",
        "post-rename-physical",
        "post-retire-runtime",
        "native-context-panic",
        "native-seed-panic",
        "pre-binder",
        "pre-rollback-error",
        "base-missing",
        "base-continue-loss",
        "panic-spawn",
        "panic-before-commit",
        "panic-commit-before-disarm",
        "panic-baseline",
        "panic-carry",
        "panic-rename",
        "panic-readback",
        "post-readback-fault",
        "panic-retire",
    ];
    if !inline {
        scenarios.extend(["native-waiter-context", "native-waiter-seed"]);
    }
    if inline {
        scenarios.extend([
            "release-lost",
            "recv-live-source",
            "submit-refused",
            "submit-channel-close",
            "recv-after-release",
            "shutdown-executing",
        ]);
    }
    if pty && inline {
        scenarios.extend(["pty-held-stop", "pty-held-rollback", "pty-cancel-held"]);
    }
    if !pty && !inline {
        scenarios.push("root-reservation-refusal");
        scenarios.push("entry-root-reclaim");
        scenarios.push("entry-restart-reclaim");
    }
    if let Some(marked_wrapper) = std::env::var_os(MARKER) {
        assert_eq!(
            marked_wrapper.to_str(),
            Some(wrapper),
            "wrong child wrapper"
        );
        let scenario = std::env::var(SCENARIO).expect("child scenario");
        assert!(
            scenarios.contains(&scenario.as_str()),
            "unknown child scenario"
        );
        let identity = std::env::var(IDENTITY).expect("child identity");
        let (run, suffix) = identity.split_once('\n').expect("run-bound identity");
        Uuid::parse_str(run).expect("parent run nonce");
        assert_eq!(suffix, format!("{wrapper}\n{scenario}"));
        let config =
            PathBuf::from(std::env::var("AGENTSCOMMANDER_CONFIG_DIR").expect("child config"));
        let test_config = PathBuf::from(
            std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").expect("child test config"),
        );
        assert!(config.is_absolute());
        assert_eq!(config, test_config);
        assert_eq!(crate::config::config_dir().as_ref(), Some(&config));
        assert!(
            !config.join("completion.txt").exists(),
            "receipt must be fresh"
        );
        run_retained_source_child_scenario(pty, inline, &scenario).await;
        std::fs::write(config.join("completion.txt"), identity).expect("child assertion receipt");
        return;
    }
    assert!(
        std::env::var_os(SCENARIO).is_none() && std::env::var_os(IDENTITY).is_none(),
        "orphan child markers"
    );
    for scenario in scenarios {
        let directory = tempfile::tempdir().expect("parent child directory");
        let path = directory
            .path()
            .canonicalize()
            .expect("absolute child directory");
        let identity = format!("{}\n{wrapper}\n{scenario}", Uuid::new_v4());
        let stdout = std::fs::File::create(path.join("stdout.log")).expect("child stdout");
        let stderr = std::fs::File::create(path.join("stderr.log")).expect("child stderr");
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([wrapper, "--exact", "--nocapture", "--test-threads=1"])
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .env(MARKER, wrapper)
            .env(SCENARIO, scenario)
            .env(IDENTITY, &identity)
            .env("AGENTSCOMMANDER_CONFIG_DIR", &path)
            .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let child = command.spawn().expect("spawn exact fixture child");
        let mut owner = RetainedSourceChild {
            child,
            directory: Some(directory),
            reaped: false,
        };
        owner.supervise(&identity, scenario);
    }
}

#[tokio::test]
#[ignore = "#2854 AC-2854-005 WIP/technical debt: native stack overflow in retained-source fixture"]
async fn retained_exited_respawn_ordinary_ticket_source_and_newer_b() {
    retained_exited_respawn_host(
        false,
        false,
        concat!(
            "phone::mailbox::tests::retained_exited::",
            "retained_exited_respawn_ordinary_ticket_source_and_newer_b"
        ),
    )
    .await;
}

#[tokio::test]
#[ignore = "#2854 AC-2854-005 WIP/technical debt: native stack overflow in retained-source fixture"]
async fn retained_exited_respawn_pty_ticket_source_and_newer_b() {
    retained_exited_respawn_host(
        true,
        false,
        concat!(
            "phone::mailbox::tests::retained_exited::",
            "retained_exited_respawn_pty_ticket_source_and_newer_b"
        ),
    )
    .await;
}

#[tokio::test]
#[ignore = "#2854 AC-2854-005 WIP/technical debt: native stack overflow in retained-source fixture"]
async fn retained_exited_respawn_pty_inline_source_and_newer_b() {
    retained_exited_respawn_host(
        true,
        true,
        concat!(
            "phone::mailbox::tests::retained_exited::",
            "retained_exited_respawn_pty_inline_source_and_newer_b"
        ),
    )
    .await;
}
