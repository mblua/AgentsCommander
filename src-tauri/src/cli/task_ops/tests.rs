use super::*;
use chrono::TimeZone;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

mod edits;
mod issue_2837;
mod issue_2837_recovery;
mod set_body_io;

/// Auto-cleaned temp dir for fixture roots. Mirrors `config/teams.rs::FixtureRoot`
/// — copied locally so we don't have to make it `pub(crate)` cross-module.
struct FixtureRoot(PathBuf);
impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl FixtureRoot {
    fn new(prefix: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("fixture root");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

fn fixed_now_at(year: i32, month: u32, day: u32, h: u32, m: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, h, m, s).unwrap()
}

#[test]
fn snapshot_missing_size_and_utf8_boundaries() {
    let root = FixtureRoot::new("snapshot-boundaries");
    let path = root.0.join("TASK.md");
    assert!(read_snapshot(&root.0).unwrap().task.is_none());

    let exact = "x".repeat(TASK_LIMIT as usize);
    std::fs::write(&path, &exact).unwrap();
    assert_eq!(
        read_snapshot(&root.0).unwrap().task.as_deref(),
        Some(exact.as_str())
    );

    // Size validation precedes UTF-8 validation, including invalid oversized input.
    std::fs::write(&path, vec![0xff; TASK_LIMIT as usize + 1]).unwrap();
    assert!(matches!(
        read_snapshot(&root.0),
        Err(TaskOpError::SnapshotTooLarge)
    ));

    std::fs::write(&path, [0xff]).unwrap();
    match read_snapshot(&root.0) {
        Err(TaskOpError::ReadFailed(failed_path, error)) => {
            assert_eq!(failed_path, path);
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        }
        other => panic!("expected invalid UTF-8 read error, got {other:?}"),
    }
}
