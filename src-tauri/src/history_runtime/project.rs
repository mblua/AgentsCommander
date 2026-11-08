//! Project identity for context history (#3532 P13).
//!
//! Verifies a project directory once and recognises every record spelling of
//! it: the canonical path and the verified typed path without `.` components.
//! Comparison is per platform through `paths_equivalent`; each distinct record
//! text is compared once and only in memory, so the store keeps its bytes.
//! Physical aliases (a `subst` drive against its real folder) match only when
//! the query used the spelling that was captured. No Settings, Tauri or I/O
//! beyond the existing directory and Project AC Root primitives.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::ac_root::existing_ac_root;
use crate::context_history::HistoryRecord;
use crate::path_identity::{paths_equivalent, verify_directory};

/// Why a project could not be verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    /// The directory failed verification; `reason` explains why.
    NotVerified { project: PathBuf, reason: String },
    /// The verified directory has no `.ac` Project AC Root.
    NoAcRoot { project: PathBuf },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotVerified { project, reason } => write!(
                f,
                "'{}' is not a verified directory: {reason}",
                project.display()
            ),
            Self::NoAcRoot { project } => {
                write!(f, "'{}' has no .ac Project AC Root", project.display())
            }
        }
    }
}

/// `text` without a Windows verbatim prefix, so both spellings compare alike;
/// a verbatim UNC path (`\\?\UNC\server\share`) becomes `\\server\share`.
pub fn plain_path(text: &str) -> String {
    for (verbatim_unc, unc) in [(r"\\?\UNC\", r"\\"), ("//?/UNC/", "//")] {
        if let Some(rest) = text.strip_prefix(verbatim_unc) {
            return format!("{unc}{rest}");
        }
    }
    text.strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix("//?/"))
        .unwrap_or(text)
        .to_string()
}

/// The verified project: its canonical directory, the spellings a record of
/// it may carry, and its Project AC Root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProject {
    pub canonical: PathBuf,
    /// The canonical path and the verified typed text without `.`
    /// components; a `subst` drive captured as typed matches only the latter.
    pub spellings: [PathBuf; 2],
    pub ac_root: PathBuf,
}

/// The verified project; an error, never an empty result, when the directory
/// or its Project AC Root cannot be verified.
pub fn verified_project(project: &Path) -> Result<VerifiedProject, ProjectError> {
    let identity = verify_directory(project).map_err(|why| ProjectError::NotVerified {
        project: project.to_path_buf(),
        reason: why,
    })?;
    let canonical = PathBuf::from(plain_path(&identity.canonical_path.to_string_lossy()));
    let ac_root = existing_ac_root(&canonical).ok_or_else(|| ProjectError::NoAcRoot {
        project: project.to_path_buf(),
    })?;
    let typed = project.components().collect::<PathBuf>();
    Ok(VerifiedProject {
        spellings: [canonical.clone(), typed],
        canonical,
        ac_root,
    })
}

/// Give every record captured under another spelling of the verified project
/// the query's project text, in memory only. Other projects keep their text,
/// so the core excludes them. Each distinct spelling is compared once.
pub fn align_project_paths(
    records: &mut [HistoryRecord],
    spellings: &[PathBuf],
    project_text: &str,
) {
    let mut equivalent: BTreeMap<String, bool> = BTreeMap::new();
    for record in records {
        let same = *equivalent
            .entry(record.project_path.clone())
            .or_insert_with_key(|text| {
                let path = PathBuf::from(plain_path(text));
                spellings
                    .iter()
                    .any(|spelling| paths_equivalent(&path, spelling))
            });
        if same {
            record.project_path = project_text.to_string();
        }
    }
}

#[cfg(test)]
mod tests;
