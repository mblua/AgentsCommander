use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

pub fn copy_executable(src: &Path, dst: &Path) {
    std::fs::copy(src, dst).expect("copy binary");

    #[cfg(target_os = "linux")]
    {
        let mut attempts = 0;
        let output = loop {
            match Command::new(dst).arg("--help").output() {
                Err(error) if error.raw_os_error() == Some(26) && attempts < 20 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => break result.expect("probe copied binary"),
            }
        };
        assert!(
            output.status.success(),
            "copied binary readiness probe failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
