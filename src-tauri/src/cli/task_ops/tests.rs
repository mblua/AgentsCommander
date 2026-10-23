use super::*;
use chrono::TimeZone;
use std::sync::{Arc, Barrier};
use std::thread;

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
