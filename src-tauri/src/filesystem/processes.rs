//! Filesystem inputs for process liveness probes.

#[cfg(target_os = "linux")]
pub(crate) fn read_process_stat(pid: libc::pid_t) -> std::io::Result<String> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
}
