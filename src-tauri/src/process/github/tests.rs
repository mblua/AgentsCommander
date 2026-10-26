use super::*;
use std::ffi::OsStr;

fn spec(program: PathBuf) -> GhCommandSpec {
    GhCommandSpec {
        program,
        args: vec!["api".to_string(), "repos/a/b".to_string()],
        envs: vec![
            ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
            ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            ("GH_PAGER".to_string(), "cat".to_string()),
        ],
        creation_flags: if cfg!(windows) { 0x08000000 } else { 0 },
    }
}

#[test]
fn command_preserves_arguments_environment_scrub_and_search() {
    let _path = crate::config::agent_path::SearchPathOverrideGuard::new("github-owner-path");
    let command = command_from_spec(spec(PathBuf::from("gh")));
    let command = command.as_std();
    assert_eq!(command.get_program(), OsStr::new("gh"));
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["api", "repos/a/b"]);
    assert_eq!(command.get_current_dir(), None);
    let envs: Vec<_> = command.get_envs().collect();
    for (key, value) in spec(PathBuf::from("gh")).envs {
        assert!(envs.contains(&(OsStr::new(&key), Some(OsStr::new(&value)))));
    }
    let mut removed: Vec<_> = envs
        .iter()
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_str().unwrap())
        .collect();
    removed.sort_unstable();
    let mut expected = crate::process::host::credentials::CREDENTIAL_ENV_KEYS.to_vec();
    expected.sort_unstable();
    assert_eq!(removed, expected, "whole-vector equality");
    if cfg!(windows) {
        assert!(!envs.iter().any(|(key, _)| *key == "PATH"));
    } else {
        assert!(envs.contains(&(OsStr::new("PATH"), Some(OsStr::new("github-owner-path")))));
    }
}

fn child_spec(mode: &str) -> GhCommandSpec {
    let mut spec = spec(std::env::current_exe().unwrap());
    spec.args = vec![
        "--exact".to_string(),
        "process::github::tests::output_child".to_string(),
        "--nocapture".to_string(),
    ];
    spec.envs
        .push(("AC_GITHUB_OWNER_TEST".to_string(), mode.to_string()));
    spec
}

#[test]
fn output_child() {
    match std::env::var("AC_GITHUB_OWNER_TEST").as_deref() {
        Ok("wait") => std::thread::sleep(Duration::from_secs(30)),
        Ok("failure") => {
            println!("github stdout");
            eprintln!("github stderr");
            std::process::exit(7);
        }
        _ => {}
    }
}

#[tokio::test]
async fn query_keeps_output_status_and_separates_spawn_from_timeout() {
    let output = query(child_spec("failure"), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stdout).contains("github stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("github stderr"));
    let missing = tempfile::tempdir().unwrap();
    assert!(matches!(
        query(
            spec(missing.path().join("absent-gh")),
            Duration::from_secs(10)
        )
        .await,
        Err(QueryError::Spawn(_))
    ));
    assert!(matches!(
        query(child_spec("wait"), Duration::from_millis(100)).await,
        Err(QueryError::Timeout)
    ));
}
