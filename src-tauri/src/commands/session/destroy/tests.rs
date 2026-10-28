use super::*;
use crate::commands::session::tests::{
    close_test_coordinator, create_scripted_session, session_test_app_with_store, test_settings,
    ScriptedSpawnBackend, SessionTestStoreState,
};
use crate::session::manager::SessionManager;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct RootChild {
    process: Child,
    directory: Option<tempfile::TempDir>,
    reaped: bool,
}

impl RootChild {
    fn fail(&mut self, reason: &str) -> ! {
        let directory = self.directory.take().unwrap().keep();
        panic!(
            "{reason}; child diagnostics retained at {}",
            directory.display()
        );
    }
}

impl Drop for RootChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

fn run_child(name: &str, case: &str, root_absent: bool) {
    let name = name.strip_prefix("agentscommander_lib::").unwrap_or(name);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config");
    std::fs::create_dir(&config).unwrap();
    let root = config.join(crate::config::ROOT_AGENT_DIR_NAME);
    assert!(!root.exists());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).unwrap())
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config)
        .env("AC_3318_DESTROY_CHILD", case);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut owner = RootChild {
        process: command.spawn().unwrap(),
        directory: Some(directory),
        reaped: false,
    };
    let started = Instant::now();
    let status = loop {
        match owner.process.try_wait() {
            Ok(Some(status)) => {
                owner.reaped = true;
                break status;
            }
            Ok(None) if started.elapsed() < Duration::from_secs(120) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => owner.fail("child deadline"),
            Err(_) => owner.fail("child poll failed"),
        }
    };
    let stdout =
        std::fs::read_to_string(owner.directory.as_ref().unwrap().path().join("stdout.txt"))
            .unwrap();
    if !status.success()
        || !stdout.contains(&format!("AC_3318_DESTROY_DONE {case}"))
        || !stdout.contains(&format!("test {name} ..."))
        || !stdout.contains("test result: ok. 1 passed; 0 failed; 0 ignored;")
        || (root_absent && root.exists())
    {
        owner.fail("child result, count, receipt or absent Root assertion failed");
    }
    eprintln!(
        "#3318 destroy child case={case} elapsed={:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn h2_destroy_root_protection() {
    let Ok(case) = std::env::var("AC_3318_DESTROY_CHILD") else {
        for case in [
            "missing-root",
            "both-missing",
            "missing-cwd",
            "ordinary",
            "root",
            "flag",
            "force",
            "background-ordinary",
            "background-missing-root",
        ] {
            run_child(
                concat!(module_path!(), "::h2_destroy_root_protection"),
                case,
                matches!(
                    case,
                    "missing-root" | "both-missing" | "force" | "background-missing-root"
                ),
            );
        }
        return;
    };
    let configured = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap();
    assert_eq!(crate::config::config_dir().unwrap(), Path::new(&configured));
    let root = Path::new(&configured).join(crate::config::ROOT_AGENT_DIR_NAME);
    let absent_root = matches!(
        case.as_str(),
        "missing-root" | "both-missing" | "force" | "background-missing-root"
    );
    assert!(!root.exists());
    if !absent_root {
        std::fs::create_dir(&root).unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().join("ordinary");
    std::fs::create_dir(&cwd).unwrap();
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(ScriptedSpawnBackend::default());
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
    let app = session_test_app_with_store(
        test_settings(),
        manager.clone(),
        pty.clone(),
        SessionTestStoreState::Ready,
    );
    let target = if case == "root" { &root } else { &cwd };
    let info = create_scripted_session(&app, &manager, &pty, target.to_str().unwrap()).await;
    let id = Uuid::parse_str(&info.id).unwrap();
    assert!(pty.lock().unwrap().has_session(id));
    manager
        .read()
        .await
        .set_is_root_agent(id, case == "flag")
        .await;
    if matches!(case.as_str(), "missing-cwd" | "both-missing") {
        std::fs::remove_dir(&cwd).unwrap();
    }
    let removed = matches!(case.as_str(), "ordinary" | "force" | "background-ordinary");
    if case.starts_with("background-") {
        background_destroy_session_inner(app.handle(), id)
            .await
            .unwrap();
    } else {
        let outcome = destroy_sessions_with_source(
            app.handle(),
            vec![id],
            DestructionSource::ManualClose,
            case == "force",
        )
        .await
        .unwrap();
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        if removed {
            assert_eq!(outcome.destroyed_ids, vec![id]);
        } else {
            assert_eq!(outcome.retained_exited_ids, vec![id]);
            assert!(outcome.destroyed_ids.is_empty());
        }
    }
    assert!(!pty.lock().unwrap().has_session(id), "teardown must run");
    let remaining = manager.read().await.get_session(id).await;
    if removed {
        assert!(remaining.is_none());
    } else {
        let remaining = remaining.unwrap();
        assert!(matches!(remaining.status, SessionStatus::Exited(_)));
        assert_eq!(remaining.is_root_agent, case == "flag");
    }
    if absent_root {
        assert!(!root.exists());
    }
    close_test_coordinator(&app).await;
    println!("AC_3318_DESTROY_DONE {case}");
}
