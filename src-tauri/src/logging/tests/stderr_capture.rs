//! #5279: the Linux GUI stderr capture file pair (`stderr.log`, `stderr.prev.log`).

use super::*;
use std::fs;

#[test]
fn open_stderr_capture_creates_file_with_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_file, path) = open_stderr_capture(dir.path()).expect("open capture");
    assert_eq!(path, dir.path().join("stderr.log"));
    let text = fs::read_to_string(&path).expect("read stderr.log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one header line: {text:?}");
    let prefix = format!("=== AgentsCommander stderr pid={} ", std::process::id());
    assert!(lines[0].starts_with(&prefix), "header: {:?}", lines[0]);
    assert!(lines[0].ends_with(" ==="), "header: {:?}", lines[0]);
    assert!(!dir.path().join("stderr.prev.log").exists());
}

#[test]
fn open_stderr_capture_keeps_previous_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("stderr.log"), "old run\n").expect("seed stderr.log");
    let (_file, path) = open_stderr_capture(dir.path()).expect("open capture");
    let prev = fs::read_to_string(dir.path().join("stderr.prev.log")).expect("read prev");
    assert_eq!(prev, "old run\n");
    let text = fs::read_to_string(&path).expect("read stderr.log");
    assert_eq!(text.lines().count(), 1, "only the new header: {text:?}");
    assert!(
        text.starts_with("=== AgentsCommander stderr pid="),
        "{text:?}"
    );
}

#[test]
fn open_stderr_capture_replaces_older_previous_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("stderr.prev.log"), "older\n").expect("seed prev");
    fs::write(dir.path().join("stderr.log"), "old\n").expect("seed stderr.log");
    open_stderr_capture(dir.path()).expect("open capture");
    let prev = fs::read_to_string(dir.path().join("stderr.prev.log")).expect("read prev");
    assert_eq!(prev, "old\n");
}
