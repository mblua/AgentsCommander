use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::{current_uid, lock_dir, try_acquire};

const HOLDER_DIR: &str = "AC_4788_LOCK_DIR";
const HOLDER: &str = "filesystem::single_instance_lock::tests::lock_holder_child";
const NAME_A: &str = "AgentsCommander_SingleInstance_Foo.lock";
const NAME_B: &str = "AgentsCommander_SingleInstance_Bar.lock";

/// Kills and reaps the holder on every exit path, including assertion panics.
struct Holder(Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .expect("chmod 0700");
    dir
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
}

fn spawn_holder(dir: &Path) -> Holder {
    let child = Command::new(std::env::current_exe().expect("test exe"))
        .args(["--exact", HOLDER, "--test-threads=1"])
        .env(HOLDER_DIR, dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn holder");
    let holder = Holder(child);
    let ready = dir.join("ready");
    assert!(
        wait_until(Duration::from_secs(20), || ready.exists()),
        "holder ready"
    );
    holder
}

/// Child half: a no-op unless the parent set `AC_4788_LOCK_DIR`.
#[test]
fn lock_holder_child() {
    let Some(dir) = std::env::var_os(HOLDER_DIR).map(PathBuf::from) else {
        return;
    };
    assert!(try_acquire(&dir, NAME_A, current_uid()), "holder acquires");
    std::fs::write(dir.join("ready"), b"1").expect("ready file");
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn same_name_refused_while_held_and_free_after_kill() {
    let dir = private_dir();
    let uid = current_uid();
    let mut holder = spawn_holder(dir.path());
    assert!(
        !try_acquire(dir.path(), NAME_A, uid),
        "same name refused while held"
    );
    assert!(try_acquire(dir.path(), NAME_B, uid), "different name runs");
    holder.0.kill().expect("kill holder");
    holder.0.wait().expect("reap holder");
    let reacquired = wait_until(Duration::from_secs(5), || {
        try_acquire(dir.path(), NAME_A, uid)
    });
    assert!(reacquired, "lock released by the kernel after kill");
}

#[test]
fn untrusted_dir_fails_open_even_when_held() {
    let dir = private_dir();
    let uid = current_uid();
    let _holder = spawn_holder(dir.path());
    assert!(
        try_acquire(dir.path(), NAME_A, uid.wrapping_add(1)),
        "foreign owner"
    );
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
        .expect("chmod 0755");
    assert!(try_acquire(dir.path(), NAME_A, uid), "group/other access");
}

#[test]
fn lock_dir_prefers_absolute_runtime_dir() {
    let runtime = PathBuf::from("/run/user/1000");
    assert_eq!(lock_dir(Some(runtime.clone()), 7), runtime);
    assert_eq!(lock_dir(None, 7), PathBuf::from("/tmp/agentscommander-7"));
    let relative = Some(PathBuf::from("run"));
    assert_eq!(
        lock_dir(relative, 7),
        PathBuf::from("/tmp/agentscommander-7")
    );
}

#[test]
fn missing_fallback_dir_is_created_private() {
    let parent = private_dir();
    let dir = parent.path().join("agentscommander-test");
    assert!(
        try_acquire(&dir, NAME_A, current_uid()),
        "creates and locks"
    );
    let mode = std::fs::metadata(&dir)
        .expect("dir meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn symlinked_lock_file_fails_open_without_touching_target() {
    let dir = private_dir();
    let target = dir.path().join("target");
    std::os::unix::fs::symlink(&target, dir.path().join(NAME_A)).expect("symlink");
    assert!(try_acquire(dir.path(), NAME_A, current_uid()), "fails open");
    assert!(!target.exists(), "symlink target not created");
}

#[test]
fn symlinked_dir_fails_open_without_creating_files() {
    let parent = private_dir();
    let real = parent.path().join("real");
    std::fs::create_dir(&real).expect("real dir");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).expect("chmod 0700");
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert!(try_acquire(&link, NAME_A, current_uid()), "fails open");
    assert!(!real.join(NAME_A).exists(), "nothing created");
}

#[test]
fn lock_path_that_is_a_directory_fails_open() {
    let dir = private_dir();
    std::fs::create_dir(dir.path().join(NAME_A)).expect("dir at lock path");
    assert!(try_acquire(dir.path(), NAME_A, current_uid()), "fails open");
}
