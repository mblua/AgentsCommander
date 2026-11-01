use super::super::handoff::{
    dedup_outbox_dirs_by_object_id, outbox_message_send_time, WAKE_CLAIM_EXTENSION,
};
use super::super::notice::OutboxOrigin;
use crate::filesystem::phone_mailbox as mailbox_disk;
use std::path::{Path, PathBuf};

pub(crate) fn scan_dirs(
    all_paths: &[String],
    app_outbox_path: &str,
) -> Vec<(PathBuf, OutboxOrigin)> {
    // Collect all outbox directories to scan
    let mut outbox_dirs: Vec<PathBuf> = all_paths
        .iter()
        .map(|p| {
            Path::new(p)
                .join(crate::config::agent_local_dir_name())
                .join("outbox")
        })
        .collect();
    // (#1399 R9) Dedup BEFORE the app outbox is pushed: `retain` keeps the
    // first occurrence, so running it over the full list could drop the
    // app entry in favour of an aliasing project spelling and silently
    // turn `is_app_outbox` false. This ordering makes that structurally
    // impossible.
    dedup_outbox_dirs_by_object_id(&mut outbox_dirs);
    outbox_dirs.push(PathBuf::from(app_outbox_path));

    // #2232 phase 7: the Co-managed provenance queues of every room an
    // active session lives in. Each is its own scanned directory with its
    // own origin, never an outbox subdirectory (the sweep skips those).
    let mut queue_dirs: Vec<PathBuf> = Vec::new();
    for path in all_paths {
        let Some(room_root) = crate::config::co_managed::room_root_for_path(Path::new(path)) else {
            continue;
        };
        let queue = crate::config::co_managed::queue_dir(&room_root);
        if !queue_dirs.contains(&queue) {
            queue_dirs.push(queue);
        }
    }

    let mut scan_dirs: Vec<(PathBuf, OutboxOrigin)> = outbox_dirs
        .into_iter()
        .map(|dir| {
            let origin = if dir.as_path() == Path::new(app_outbox_path) {
                OutboxOrigin::Master
            } else {
                OutboxOrigin::Replica
            };
            (dir, origin)
        })
        .collect();
    scan_dirs.extend(
        queue_dirs
            .into_iter()
            .map(|dir| (dir, OutboxOrigin::CoManaged)),
    );
    scan_dirs
}

pub(crate) fn read_entries(outbox_dir: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    // (#1399) One walk partitions the outbox root by extension:
    // messages to scan and claim markers to reclaim. Deliberately
    // unbounded, exactly as today's scan is.
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut claims: Vec<PathBuf> = Vec::new();
    match mailbox_disk::mailbox_directory_paths(outbox_dir) {
        Ok(rd) => {
            for path in rd.filter_map(|e| e.ok()) {
                // Skip files in subdirectories (delivered/, rejected/)
                if path.parent() != Some(outbox_dir) {
                    continue;
                }
                match path.extension().and_then(|s| s.to_str()) {
                    Some("json") => entries.push(path),
                    Some(WAKE_CLAIM_EXTENSION) => claims.push(path),
                    _ => {}
                }
            }
        }
        Err(_) => return None,
    }
    Some((entries, claims))
}

pub(crate) fn sort_entries(entries: &mut [PathBuf]) {
    // (#1399) Process a batch in send order. `read_dir` yields NTFS
    // filename order over random UUID names, so today's order is
    // arbitrary; once the per-target lane defers the rest of a
    // target's queue, that arbitrary order becomes the ADMISSION
    // decision, re-rolled every cycle, and an already-queued message
    // could be passed over indefinitely by newer arrivals whose names
    // sort lower. `sort_by_cached_key` reads each file at most once;
    // `None` sorts first so an unreadable document still reaches the
    // existing rejection path promptly; the path tie-breaker makes
    // equal timestamps deterministic. The length guard keeps the
    // steady state (zero or one message) at exactly zero extra reads.
    if entries.len() > 1 {
        entries.sort_by_cached_key(|path| (outbox_message_send_time(path), path.clone()));
    }
}
