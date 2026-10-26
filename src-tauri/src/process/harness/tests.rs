use super::*;

#[test]
fn argv_preserves_program_and_literal_arguments() {
    let argv = vec!["program".into(), "two words".into(), "& literal".into()];
    let command = argv_command(&argv);
    assert_eq!(command.get_program(), "program");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["two words", "& literal"]
    );
    let mut expected = Command::new("program");
    crate::config::agent_path::apply_search_path_to_std_command(&mut expected);
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        expected.get_envs().collect::<Vec<_>>()
    );
}

#[test]
fn raw_shell_preserves_raw_text_and_platform_search_policy() {
    let raw = "echo two words & echo literal";
    let command = platform_shell_command(raw);
    #[cfg(windows)]
    let (program, flag) = ("cmd.exe", "/C");
    #[cfg(not(windows))]
    let (program, flag) = ("sh", "-c");
    assert_eq!(command.get_program(), program);
    assert_eq!(command.get_args().collect::<Vec<_>>(), [flag, raw]);
    #[cfg(windows)]
    let expected = Command::new(program);
    #[cfg(not(windows))]
    let expected = {
        let mut expected = Command::new(program);
        crate::config::agent_path::apply_search_path_to_std_command(&mut expected);
        expected
    };
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        expected.get_envs().collect::<Vec<_>>()
    );
}

#[test]
fn operations_wait_and_preserve_exit_codes() {
    assert_eq!(run_raw_shell("exit 7").unwrap().code(), Some(7));
    #[cfg(windows)]
    let argv = ["cmd.exe", "/C", "exit 9"];
    #[cfg(not(windows))]
    let argv = ["sh", "-c", "exit 9"];
    let argv = argv.map(String::from);
    assert_eq!(run_argv(&argv).unwrap().code(), Some(9));
    assert!(run_raw_shell("exit 0").unwrap().success());
}

#[test]
fn argv_spawn_failure_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let argv = [dir
        .path()
        .join("missing-program")
        .to_string_lossy()
        .into_owned()];
    assert_eq!(
        run_argv(&argv).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}
