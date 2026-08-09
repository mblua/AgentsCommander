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

fn sha256_hex(input: &[u8]) -> String {
    const ROUND_CONSTANTS: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
        0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_length = (input.len() as u64).checked_mul(8).expect("fixture input length");
    let mut padded = input.to_vec();
    padded.push(0x80);
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for index in 0..16 {
            let offset = index * 4;
            words[index] = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let first = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let second = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(first)
                .wrapping_add(words[index - 7])
                .wrapping_add(second);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for index in 0..64 {
            let sum_one = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temporary_one = h
                .wrapping_add(sum_one)
                .wrapping_add(choice)
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(words[index]);
            let sum_zero = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary_two = sum_zero.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary_one);
            d = c;
            c = b;
            b = a;
            a = temporary_one.wrapping_add(temporary_two);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    state
        .iter()
        .map(|word| format!("{word:08x}"))
        .collect::<String>()
}

#[repr(C)]
#[derive(Default)]
struct FixtureFileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
#[derive(Default)]
struct FixtureByHandleFileInformation {
    file_attributes: u32,
    creation_time: FixtureFileTime,
    last_access_time: FixtureFileTime,
    last_write_time: FixtureFileTime,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut std::ffi::c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: isize,
    ) -> isize;
    fn GetFileInformationByHandle(
        file: isize,
        information: *mut FixtureByHandleFileInformation,
    ) -> i32;
    fn CloseHandle(object: isize) -> i32;
}

fn root_mutex_hash(package_id: &str, root: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;

    let path: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            7,
            std::ptr::null_mut(),
            3,
            0x02000000,
            0,
        )
    };
    if handle == -1 {
        panic!("open fixture root identity handle");
    }

    let mut information = FixtureByHandleFileInformation::default();
    let read_succeeded = unsafe { GetFileInformationByHandle(handle, &mut information) };
    let close_succeeded = unsafe { CloseHandle(handle) };
    if read_succeeded == 0 {
        panic!("read fixture root identity");
    }
    if close_succeeded == 0 {
        panic!("close fixture root identity handle");
    }

    let volume = u64::from(information.volume_serial_number);
    let file = (u64::from(information.file_index_high) << 32) | u64::from(information.file_index_low);
    let mut input = Vec::with_capacity(package_id.len() + 16);
    input.extend_from_slice(package_id.as_bytes());
    input.extend_from_slice(&volume.to_le_bytes());
    input.extend_from_slice(&file.to_le_bytes());
    sha256_hex(&input)
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

    if arguments.as_slice() == ["--hold-inherited-pipes"] {
        if let Ok(ready_path) = env::var("ISOLATED_VALIDATION_TEST_PIPE_LEAK_READY_PATH") {
            fs::write(ready_path, "ready\n").expect("write pipe-leak ready marker");
        }
        thread::sleep(Duration::from_secs(5));
        if let Ok(exit_path) = env::var("ISOLATED_VALIDATION_TEST_PIPE_LEAK_EXIT_PATH") {
            fs::write(exit_path, "exited\n").expect("write pipe-leak exit marker");
        }
        return;
    }

    if arguments.as_slice() == ["--pipe-leak"] {
        std::process::Command::new(env::current_exe().expect("current executable"))
            .arg("--hold-inherited-pipes")
            .spawn()
            .expect("spawn pipe-leak descendant");
        print!("{}", "o".repeat(131_072));
        let _ = io::stdout().flush();
        eprint!("{}", "e".repeat(131_072));
        let _ = io::stderr().flush();
        thread::sleep(Duration::from_secs(10));
        return;
    }

    if arguments.iter().any(|argument| argument == "--isolation-status") {
        if let Ok(status_sentinel_path) = env::var("ISOLATED_VALIDATION_TEST_STATUS_CHILD_SENTINEL_PATH") {
            fs::write(status_sentinel_path, "status\n").expect("write status child sentinel");
        }
        let root = value_after(&arguments, "--isolated-state-root").expect("isolated root");
        fs::create_dir_all(root).expect("create isolated root");
        if env::var("ISOLATED_VALIDATION_TEST_RECEIPT_COLLISION")
            .ok()
            .as_deref()
            == Some("1")
        {
            let fixture = Path::new(root).parent().expect("fixture root");
            fs::write(fixture.join("launch-receipt.json"), "concurrent winner\n")
                .expect("publish concurrent receipt");
        }
        let profile_hash = env::var("ISOLATED_VALIDATION_TEST_PROFILE_HASH")
            .expect("test profile hash");
        let mutex_hash = root_mutex_hash("agentscommander-1271-isolated-gates", Path::new(root));
        let effective_root = fs::canonicalize(root).expect("canonical isolated root");
        print!(
            "{{\"effectiveRoot\":\"{}\",\"packageId\":\"agentscommander-1271-isolated-gates\",\"profileSha256\":\"{}\",\"workspace\":\"AgentsCommander_1271_isolated\",\"matrix\":\"WG-1271-ISOLATED-GATES\",\"replicaAgent\":\"gate-tester\",\"headerIdentity\":\"WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated\",\"bundleIdentifier\":\"dev.agentscommander.isolatedgates\",\"mutexHash\":\"{}\"}}",
            json_escape(&effective_root.to_string_lossy()),
            json_escape(&profile_hash),
            json_escape(&mutex_hash)
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
            for _ in 0..50 {
                thread::sleep(Duration::from_millis(100));
            }
            return;
        }
        thread::sleep(Duration::from_millis(75));
    }
}
