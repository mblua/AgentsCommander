//! Consumption key, file-observation fingerprint and cut (#2232 phase 3).
//!
//! This module is a leaf: it names nothing in the phone, session or commands
//! subtrees and neither config module named in section 4, and it performs no
//! I/O.
//! The epoch predicate lives here as a pure function so both the watcher (which
//! may only use bytes it has already read) and `capture::state` (which owns the
//! persisted map) evaluate exactly the same rule.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capture::record::CapturedRecord;

/// The identity of one consumable record.
///
/// `record_start` is `None` for a preamble line, whose position the kernel does
/// not track; two preamble lines with the same text in the same file and epoch
/// are therefore the same key, which is exactly the belt section 8 asks for.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ConsumptionKey {
    pub path: PathBuf,
    pub epoch: u64,
    pub record_start: Option<u64>,
    pub text_sha256: [u8; 32],
}

impl ConsumptionKey {
    /// Build the key for `record`, using the authoritative `epoch`.
    ///
    /// The path is normalised with [`normalise_path`] so the same file reached
    /// through two spellings yields one key.
    pub fn from_record(record: &CapturedRecord, epoch: u64) -> Self {
        Self {
            path: normalise_path(&record.file),
            epoch,
            record_start: record.record_start,
            text_sha256: record.text_sha256,
        }
    }
}

/// Canonicalise when the file still exists, otherwise keep the path verbatim.
///
/// `canonicalize` is the only portable normalisation available and it fails on
/// a rotated-away file; falling back to the literal path keeps a key stable
/// rather than turning a missing file into a different identity every call.
pub fn normalise_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What the reader has seen of one file: its length and a capped head prefix.
///
/// `prefix_len` is recorded separately from `prefix.len()` because a caller may
/// know the length without having the bytes (a live read that started past the
/// head); a zero `prefix_len` means "no prefix evidence", and the comparison
/// range below collapses to nothing, so length alone decides.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileObservation {
    pub len: u64,
    pub prefix: Vec<u8>,
    pub prefix_len: usize,
}

impl FileObservation {
    pub fn new(len: u64, prefix: Vec<u8>) -> Self {
        let prefix_len = prefix.len();
        Self {
            len,
            prefix,
            prefix_len,
        }
    }
}

/// The three ways a new observation can relate to the stored one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationVerdict {
    /// Length grew or held with the same prefix over the shared range.
    Append,
    /// Current length is below the stored length.
    Truncated,
    /// The prefix differs over the shared range.
    Replaced,
}

impl ObservationVerdict {
    /// Only truncation and replacement advance the epoch.
    pub fn advances_epoch(self) -> bool {
        matches!(self, Self::Truncated | Self::Replaced)
    }
}

/// The epoch predicate of section 6.1, evaluated over bytes only.
///
/// The prefixes are compared **only over the range both have**,
/// `min(stored.prefix_len, current.prefix_len, current.len)`, so a file shorter
/// than the cap that grows simply extends its stored prefix and does not look
/// replaced. The trailing bytes are deliberately not part of the trigger: every
/// append changes the tail, so a trailing fingerprint would advance the epoch on
/// every write.
pub fn classify_observation(
    stored: &FileObservation,
    current: &FileObservation,
) -> ObservationVerdict {
    if current.len < stored.len {
        return ObservationVerdict::Truncated;
    }
    let shared = stored
        .prefix_len
        .min(current.prefix_len)
        .min(usize::try_from(current.len).unwrap_or(usize::MAX))
        .min(stored.prefix.len())
        .min(current.prefix.len());
    if shared > 0 && stored.prefix[..shared] != current.prefix[..shared] {
        return ObservationVerdict::Replaced;
    }
    ObservationVerdict::Append
}

/// Merge `current` into `stored` after an [`ObservationVerdict::Append`].
///
/// The longer prefix wins, so a head first seen short is extended rather than
/// shortened by a later live read that carried no head bytes.
pub fn extend_observation(stored: &mut FileObservation, current: &FileObservation) {
    stored.len = current.len;
    if current.prefix_len > stored.prefix_len {
        stored.prefix = current.prefix.clone();
        stored.prefix_len = current.prefix_len;
    }
}

/// What a reader knows about the file it just read, carried on every record it
/// emits (#2232 phase 3).
///
/// One definition, shared by both watchers. It used to be copied verbatim into
/// each of them, which put two copies of the epoch predicate's caller in the
/// tree; both readers already name this module, so a single definition here
/// adds no arc.
#[derive(Clone, Debug, Default)]
pub struct ReaderAttachment {
    pub epoch: u64,
    pub observed_path: PathBuf,
    pub observed_len: u64,
    pub observed_prefix: Vec<u8>,
}

/// A reader's own per-file epoch, kept in memory for that reader's lifetime.
///
/// **Advisory, never authoritative.** `capture::state` owns the persisted
/// epoch, and only that one survives a restart; this counter restarts at zero
/// with its reader. It exists so a record carries a plausible epoch instead of
/// phase 1's hardcoded `0`, and so the reader can answer "did this file change
/// under me" without a lock or a disk read. Anything that builds a
/// [`ConsumptionKey`] must take the epoch from `capture::state`.
///
/// Returning to a file read earlier recovers its entry, so moving between files
/// never advances an epoch.
#[derive(Debug, Default)]
pub struct ReaderObservations {
    files: std::collections::HashMap<PathBuf, (u64, FileObservation)>,
}

impl ReaderObservations {
    /// Record one observation and return the attachment for the records it
    /// produced.
    ///
    /// `prefix` must already be capped, and must come from the reader's own
    /// reconstruction of the head; see `capture::state::head_from_lines` for
    /// why mixing sources is forbidden. An empty `prefix` means "no prefix
    /// evidence", which [`classify_observation`] resolves to a zero-length
    /// shared range, so length alone decides.
    pub fn observe(&mut self, path: &Path, len: u64, prefix: Vec<u8>) -> ReaderAttachment {
        let current = FileObservation::new(len, prefix);
        let entry = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| (0, current.clone()));
        if classify_observation(&entry.1, &current).advances_epoch() {
            entry.0 += 1;
            entry.1 = current;
        } else {
            extend_observation(&mut entry.1, &current);
        }
        ReaderAttachment {
            epoch: entry.0,
            observed_path: path.to_path_buf(),
            observed_len: entry.1.len,
            observed_prefix: entry.1.prefix.clone(),
        }
    }
}

/// A demand raised over an already-running reader (section 7).
///
/// Records at or below this point were produced before the consumer existed and
/// are not candidates. The cut is superseded when the epoch advances, otherwise
/// a later truncation would permanently hide new low-offset records.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub path: PathBuf,
    pub epoch: u64,
    pub len: u64,
    pub reader_seq: Option<u64>,
}

impl Cut {
    /// Does this cut hide `record`, given the record's authoritative `epoch`?
    ///
    /// Between runs **length** decides; `reader_seq` is only a tie-break inside
    /// one reader lifetime and is `None` once the reader has re-anchored (see
    /// [`Cut::supersede_sequence`]).
    pub fn hides(&self, record: &CapturedRecord, epoch: u64) -> bool {
        if epoch != self.epoch {
            return false;
        }
        if normalise_path(&record.file) != self.path {
            return false;
        }
        if self
            .reader_seq
            .is_some_and(|cut_seq| record.reader_seq <= cut_seq)
        {
            return true;
        }
        match record.record_start {
            Some(start) => start < self.len,
            // A preamble line has no tracked offset; it predates the demand by
            // construction, so the cut hides it while the epoch is unchanged.
            None => true,
        }
    }

    /// Drop the sequence tie-break, at reader start and at every re-anchor.
    ///
    /// Without this a fresh reader after a restart starts at `reader_seq == 0`
    /// and the stale comparison would ignore everything it produces.
    pub fn supersede_sequence(&mut self) {
        self.reader_seq = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(len: u64, prefix: &[u8]) -> FileObservation {
        FileObservation::new(len, prefix.to_vec())
    }

    // Test 5: append of 10 bytes to a 100-byte file does not advance the epoch.
    #[test]
    fn append_does_not_advance_the_epoch() {
        let head = vec![b'a'; 100];
        let stored = obs(100, &head);
        let mut grown = head.clone();
        grown.extend(std::iter::repeat_n(b'b', 10));
        let current = obs(110, &grown);
        assert_eq!(
            classify_observation(&stored, &current),
            ObservationVerdict::Append
        );
        assert!(!ObservationVerdict::Append.advances_epoch());
    }

    // Test 6: a file shorter than N that grows does not advance the epoch, and
    // its stored prefix is extended rather than treated as a replacement.
    #[test]
    fn a_short_file_that_grows_is_not_a_replacement() {
        let mut stored = obs(4, b"head");
        let current = obs(9, b"head-tail");
        assert_eq!(
            classify_observation(&stored, &current),
            ObservationVerdict::Append
        );
        extend_observation(&mut stored, &current);
        assert_eq!(stored.len, 9);
        assert_eq!(stored.prefix, b"head-tail".to_vec());
        assert_eq!(stored.prefix_len, 9);
    }

    #[test]
    fn truncation_is_detected_by_length() {
        let stored = obs(100, b"head");
        let current = obs(4, b"head");
        assert_eq!(
            classify_observation(&stored, &current),
            ObservationVerdict::Truncated
        );
    }

    // Test 8: prefix replacement at the same length advances the epoch.
    #[test]
    fn same_length_prefix_replacement_advances_the_epoch() {
        let stored = obs(10, b"aaaaaaaaaa");
        let current = obs(10, b"bbbbbbbbbb");
        let verdict = classify_observation(&stored, &current);
        assert_eq!(verdict, ObservationVerdict::Replaced);
        assert!(verdict.advances_epoch());
    }

    #[test]
    fn a_missing_prefix_leaves_length_to_decide() {
        // A live read that started past the head carries no prefix evidence.
        let stored = obs(100, b"head-bytes");
        let current = FileObservation::new(140, Vec::new());
        assert_eq!(
            classify_observation(&stored, &current),
            ObservationVerdict::Append
        );
    }

    #[test]
    fn a_shorter_prefix_is_not_lost_when_extending() {
        let mut stored = obs(100, b"head-bytes");
        let current = FileObservation::new(140, Vec::new());
        extend_observation(&mut stored, &current);
        assert_eq!(stored.len, 140);
        assert_eq!(stored.prefix, b"head-bytes".to_vec());
    }
}
