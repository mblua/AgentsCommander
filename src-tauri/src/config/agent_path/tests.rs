use super::*;

fn join(dirs: &[&Path]) -> OsString {
    std::env::join_paths(dirs).expect("join test paths")
}

#[test]
fn agent_path_2589_phase1_dirs_keeps_only_existing_dirs_in_fixed_order() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".local/bin")).unwrap();
    std::fs::create_dir_all(home.path().join("bin")).unwrap();

    let dirs = phase1_dirs(Some(home.path()));

    if cfg!(windows) {
        assert!(dirs.is_empty());
        return;
    }
    let mut expected = vec![home.path().join(".local/bin"), home.path().join("bin")];
    expected.extend(
        ABSOLUTE_BIN_DIRS
            .iter()
            .map(PathBuf::from)
            .filter(|dir| dir.is_dir()),
    );
    assert_eq!(dirs, expected);
}

#[test]
fn agent_path_2589_compose_appends_new_entries_after_inherited_and_dedupes() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let c = root.path().join("c");
    let inherited = join(&[&a, &b]);
    let composed = compose(&SearchPathInputs {
        inherited: Some(inherited.clone()),
        home: None,
        login_shell_path: Some(join(&[&b, &c, &c, &a])),
    });

    let mut expected = inherited;
    expected.push(if cfg!(windows) { ";" } else { ":" });
    expected.push(c.as_os_str());
    assert_eq!(composed, expected);
}

#[test]
fn agent_path_2589_compose_adds_phase2_entries_not_already_present() {
    let root = tempfile::tempdir().unwrap();
    let inherited_dir = root.path().join("inherited");
    let nvm = root.path().join("nvm/bin");
    let volta = root.path().join("volta/bin");
    let composed = compose(&SearchPathInputs {
        inherited: Some(join(&[&inherited_dir])),
        home: None,
        login_shell_path: Some(join(&[&nvm, &inherited_dir, &volta])),
    });

    let entries: Vec<PathBuf> = std::env::split_paths(&composed).collect();
    assert_eq!(entries, vec![inherited_dir, nvm, volta]);
}

#[test]
fn agent_path_2589_phase1_dirs_is_empty_without_home() {
    assert!(phase1_dirs(None).is_empty());
    assert!(compose(&SearchPathInputs::default()).is_empty());
}

#[test]
fn agent_path_2589_parses_the_sentinel_value_between_banners() {
    let stdout = include_str!("tests/fixtures/banner.txt");
    assert_eq!(parse_sentinel_path(stdout), Some("/a b/bin:/usr/bin"));
}

#[test]
fn agent_path_2589_compose_returns_inherited_unchanged_when_extra_is_empty() {
    let inherited = OsString::from(include_str!("tests/fixtures/inherited-path.txt"));
    let composed = compose(&SearchPathInputs {
        inherited: Some(inherited.clone()),
        home: None,
        login_shell_path: None,
    });
    assert_eq!(composed, inherited);
}

#[test]
fn agent_path_2589_sentinel_parse_rejects_missing_end_marker_and_empty_value() {
    assert_eq!(parse_sentinel_path("__AC_PATH_BEGIN__/usr/bin"), None);
    assert_eq!(
        parse_sentinel_path("__AC_PATH_BEGIN____AC_PATH_END__"),
        None
    );
    assert_eq!(parse_sentinel_path("/usr/bin__AC_PATH_END__"), None);
    assert_eq!(parse_sentinel_path(""), None);
}

#[cfg(unix)]
fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn agent_path_2589_login_shell_to_probe_skips_fish_and_empty_shell() {
    assert_eq!(
        login_shell_to_probe(Some(OsStr::new("/usr/local/bin/fish"))),
        None
    );
    assert_eq!(login_shell_to_probe(Some(OsStr::new(""))), None);
    assert_eq!(login_shell_to_probe(None), None);
    assert_eq!(
        login_shell_to_probe(Some(OsStr::new("/bin/bash"))),
        Some(PathBuf::from("/bin/bash"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_login_shell_probe_reads_the_value_past_a_banner() {
    let dir = tempfile::tempdir().unwrap();
    let shell = fake_shell(
        dir.path(),
        "fake-shell",
        include_str!("tests/fixtures/login-shell.sh"),
    );
    let value = probe_login_shell_path(&shell, Duration::from_secs(5)).await;
    assert_eq!(value, Some(OsString::from("/home/u/.nvm/bin:/usr/bin")));
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_login_shell_probe_times_out_and_yields_none() {
    let dir = tempfile::tempdir().unwrap();
    let shell = fake_shell(dir.path(), "slow-shell", "sleep 30");
    let started = std::time::Instant::now();
    let value = probe_login_shell_path(&shell, LOGIN_SHELL_PROBE_TIMEOUT).await;
    let elapsed = started.elapsed();
    assert_eq!(value, None);
    assert!(elapsed >= LOGIN_SHELL_PROBE_TIMEOUT, "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn agent_path_2589_login_shell_probe_skips_a_missing_shell() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-shell");
    assert_eq!(
        probe_login_shell_path(&missing, Duration::from_secs(5)).await,
        None
    );
}

const T3_INJECTED: &str = "/ac-2683-t3";

fn path_override(command: &std::process::Command) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("PATH"))
        .map(|(_, value)| value.map(OsStr::to_os_string))
}

#[test]
fn issue_2683_apply_search_path_uses_the_injected_value_or_nothing_on_windows() {
    let _guard = SearchPathOverrideGuard::new(T3_INJECTED);
    let mut command = portable_pty::CommandBuilder::new("probe");
    let before = command.get_env("PATH").map(OsStr::to_os_string);

    apply_search_path_to_pty_command(&mut command);

    let after = command.get_env("PATH").map(OsStr::to_os_string);
    if cfg!(windows) {
        assert_eq!(after, before, "Windows must leave the PTY PATH untouched");
    } else {
        assert_eq!(after, Some(OsString::from(T3_INJECTED)));
    }
}

#[test]
fn issue_2683_apply_search_path_adapters_agree_across_command_types() {
    let _guard = SearchPathOverrideGuard::new(T3_INJECTED);
    let expected = if cfg!(windows) {
        None
    } else {
        Some(Some(OsString::from(T3_INJECTED)))
    };

    let mut std_command = std::process::Command::new("probe");
    apply_search_path_to_std_command(&mut std_command);
    assert_eq!(path_override(&std_command), expected);

    let mut tokio_command = tokio::process::Command::new("probe");
    apply_search_path_to_tokio_command(&mut tokio_command);
    assert_eq!(path_override(tokio_command.as_std()), expected);
}
