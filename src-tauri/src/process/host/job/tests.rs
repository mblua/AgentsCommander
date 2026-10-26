use super::*;

use super::windows_impl::spawn_suspended_contained_impl;
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Mirror of `windows.rs::process_entries`: one `CreateToolhelp32Snapshot` pass
/// returning `pid -> parent_pid`. Used to collect the full descendant set of a
/// root pid so the assertion does not depend on process names.
fn parent_map() -> HashMap<u32, u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut map = HashMap::new();
    // SAFETY: standard toolhelp enumeration; the snapshot handle is closed
    // before returning and the entry struct is zero-initialized with dwSize set.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return map;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    map
}

fn descendants_of(root: u32, map: &HashMap<u32, u32>) -> Vec<u32> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for (&pid, &ppid) in map {
            if ppid == p && pid != root && !out.contains(&pid) {
                out.push(pid);
                frontier.push(pid);
            }
        }
    }
    out
}

#[test]
fn job_terminate_kills_child_and_grandchild() {
    // Assigned pid = outer cmd; it spawns an inner cmd (child) that spawns ping
    // (grandchild of the assigned pid). Proves tree-kill to depth 2 and that
    // descendants spawned AFTER assignment are captured.
    let mut child = Command::new("cmd.exe")
        .args(["/C", "cmd /C ping -n 30 127.0.0.1 >NUL"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn outer cmd");
    let root = child.id();

    let job = JobObject::for_child(root).expect("job created + assigned");

    // Wait (generously, to avoid flake on a loaded CI runner) for the grandchild
    // tree to materialize. The loop exits as soon as any descendant appears, so the
    // ceiling only matters under heavy load.
    let mut tree = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        tree = descendants_of(root, &parent_map());
        if !tree.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !tree.is_empty(),
        "expected at least one descendant before terminate"
    );

    job.terminate();

    // Every member of the subtree (root + descendants) is gone shortly after the
    // job kill. The ceiling is generous (CI load) but far under the ping's ~30s
    // lifetime, so a broken job that leaves survivors still fails this loop.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let map = parent_map();
        let alive_root = map.contains_key(&root);
        let alive_tree = descendants_of(root, &map);
        if !alive_root && alive_tree.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "job did not kill the whole tree in time \
             (root_alive={alive_root}, survivors={alive_tree:?})"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.wait();
}

fn marker_command(marker: &std::path::Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("cmd.exe");
    command
        .as_std_mut()
        .raw_arg("/D /C echo ran>\"%AGENTSCOMMANDER_TEST_MARKER%\"");
    command.env("AGENTSCOMMANDER_TEST_MARKER", marker);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    command
}

#[tokio::test]
async fn suspended_containment_blocks_marker_until_job_assignment_and_resume() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("marker.txt");
    let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut command = marker_command(&marker);
    let task = tokio::spawn(async move {
        spawn_suspended_contained_impl(
            &mut command,
            Some(SpawnTestHook {
                after_spawn: Some(spawned_tx),
                release_spawn: Some(release_rx),
                after_reap: None,
                release_query: None,
                failure: None,
                settlement_window: None,
            }),
        )
        .await
    });
    spawned_rx.await.expect("suspended child spawned");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!marker.exists(), "the suspended command must not execute");
    release_tx.send(()).expect("release spawn");
    let (child, job) = task.await.expect("join").expect("contained spawn");
    let output = child.wait_with_output().await.expect("wait marker command");
    assert!(
        output.status.success(),
        "marker command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while job.active_processes().expect("accounting query") != 0 {
        assert!(Instant::now() < deadline, "job did not settle");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(marker.exists(), "the command runs only after resume");
}

async fn rejected_launch_never_runs_marker(failure: InjectedFailure) {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("marker.txt");
    let (spawned_tx, spawned_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (reaped_tx, reaped_rx) = tokio::sync::oneshot::channel();
    let (query_tx, query_rx) = tokio::sync::oneshot::channel();
    let mut command = marker_command(&marker);
    let task = tokio::spawn(async move {
        spawn_suspended_contained_impl(
            &mut command,
            Some(SpawnTestHook {
                after_spawn: Some(spawned_tx),
                release_spawn: Some(release_rx),
                after_reap: Some(reaped_tx),
                release_query: Some(query_rx),
                failure: Some(failure),
                settlement_window: Some(Duration::from_millis(60)),
            }),
        )
        .await
    });
    spawned_rx.await.expect("suspended child spawned");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!marker.exists(), "the suspended command must not execute");
    release_tx.send(()).expect("release spawn");
    reaped_rx
        .await
        .expect("direct child reaped and handle dropped");
    assert!(!marker.exists(), "a rejected launch never executes");
    tokio::time::sleep(Duration::from_millis(75)).await;
    query_tx.send(()).expect("release accounting query");
    let error = task
        .await
        .expect("join")
        .expect_err("the injected launch must fail closed");
    assert!(
        error.to_string().contains("containment"),
        "unexpected error: {error}"
    );
    assert!(
        error
            .to_string()
            .contains("settlement attempt deadline exceeded during rejected-spawn proof barrier"),
        "the rejected-spawn cleanup must retain its expired attempt defect: {error}"
    );
    assert!(!marker.exists(), "the rejected command remained suspended");
}

#[tokio::test]
async fn suspended_containment_assignment_failure_never_runs_marker() {
    rejected_launch_never_runs_marker(InjectedFailure::Assignment).await;
}

#[tokio::test]
async fn suspended_containment_resume_failure_never_runs_marker() {
    rejected_launch_never_runs_marker(InjectedFailure::Resume).await;
}
