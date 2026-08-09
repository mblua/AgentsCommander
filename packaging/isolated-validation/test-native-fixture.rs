use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    thread,
    time::Duration,
};

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect()
}

fn value_after<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

fn write_child_environment(executable_directory: &Path) {
    let retained = env::vars()
        .filter(|(key, _)| key.to_ascii_uppercase().starts_with("AGENTSCOMMANDER_"))
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let _ = fs::write(executable_directory.join("fixture-child-env.txt"), retained);
}

fn main() {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let executable_directory = env::current_exe()
        .expect("current executable")
        .parent()
        .expect("executable parent")
        .to_path_buf();
    write_child_environment(&executable_directory);

    if arguments.as_slice() == ["--dual-stream"] {
        print!("{}", "o".repeat(131_072));
        let _ = io::stdout().flush();
        eprint!("{}", "e".repeat(131_072));
        let _ = io::stderr().flush();
        return;
    }

    if arguments.iter().any(|argument| argument == "--isolation-status") {
        let root = value_after(&arguments, "--isolated-state-root").expect("isolated root");
        fs::create_dir_all(root).expect("create isolated root");
        let profile_hash = env::var("ISOLATED_VALIDATION_TEST_PROFILE_HASH")
            .expect("test profile hash");
        let effective_root = fs::canonicalize(root).expect("canonical isolated root");
        print!(
            "{{\"effectiveRoot\":\"{}\",\"packageId\":\"agentscommander-1271-isolated-gates\",\"profileSha256\":\"{}\",\"workspace\":\"AgentsCommander_1271_isolated\",\"matrix\":\"WG-1271-ISOLATED-GATES\",\"replicaAgent\":\"gate-tester\",\"headerIdentity\":\"WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated\",\"bundleIdentifier\":\"dev.agentscommander.isolatedgates\",\"mutexHash\":\"test-mutex-hash\"}}",
            json_escape(&effective_root.to_string_lossy()),
            json_escape(&profile_hash)
        );
        return;
    }

    if arguments.iter().any(|argument| argument == "--app") {
        let process_id = std::process::id().to_string();
        fs::write(
            executable_directory.join("child-execution-sentinel.txt"),
            &process_id,
        )
        .expect("write child sentinel");
        if env::var("ISOLATED_VALIDATION_TEST_RECEIPT_COLLISION").ok().as_deref() == Some("1") {
            let root = value_after(&arguments, "--isolated-state-root").expect("isolated root");
            let fixture = Path::new(root).parent().expect("fixture root");
            fs::write(fixture.join("launch-receipt.json"), "concurrent winner\n")
                .expect("publish concurrent receipt");
            if let Ok(pid_path) = env::var("ISOLATED_VALIDATION_TEST_GUI_PID_PATH") {
                fs::write(pid_path, &process_id).expect("write GUI pid");
            }
            loop {
                thread::sleep(Duration::from_secs(1));
            }
        }
        thread::sleep(Duration::from_millis(75));
    }
}
