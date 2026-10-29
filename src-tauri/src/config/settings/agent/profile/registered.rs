use super::super::{agents_instance_path, AGENTS_INSTANCE_KEYS};
use super::{
    duplicate_profile_document, physical_profile_rows, physical_profile_sets,
    project_physical_profiles,
};
use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::filesystem::settings_sources;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub(crate) fn registered_profiles_for_read(source: &Value) -> Value {
    match project_physical_profiles(source, true) {
        Ok(projected) => projected,
        Err(reason) => {
            log::warn!("[profiles] read-only diagnostic: {reason}");
            let mut diagnostic = source.clone();
            if let Some(profiles) = diagnostic
                .get_mut("codingAgentProfiles")
                .and_then(Value::as_object_mut)
            {
                if profiles.get("profileBindingVersion") == Some(&Value::from(1)) {
                    for key in ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"] {
                        profiles.remove(key);
                    }
                }
            }
            diagnostic
        }
    }
}

// Key presence selects each registered group independently, including []/{}.
// Legacy preparation and the directional ordinary view use only this owner's
// selected physical rows. Borrowing a row never transfers its ownership.
pub(crate) fn selected_registered_profile_owner(
    main: &Map<String, Value>,
    sidecar: Option<&Map<String, Value>>,
    consolidate_legacy: bool,
) -> Value {
    let profiles_in_sidecar = sidecar.is_some_and(|map| {
        map.contains_key("codingAgentProfiles")
            || (map.contains_key("agents") && !main.contains_key("codingAgentProfiles"))
    });
    let mut owner = if profiles_in_sidecar {
        sidecar.cloned().unwrap_or_default()
    } else {
        main.clone()
    };
    // Shadowed main rows never become a donor, even for a canonical main group.
    if !profiles_in_sidecar && sidecar.is_some_and(|map| map.contains_key("agents")) {
        owner.remove("agents");
    }
    let directional = profiles_in_sidecar
        && sidecar.is_some_and(|map| !map.contains_key("agents"))
        && owner
            .get("codingAgentProfiles")
            .is_some_and(|profiles| profiles.get("profileBindingVersion") == Some(&Value::from(1)));
    if directional
        || (consolidate_legacy
            && owner
                .get("codingAgentProfiles")
                .is_none_or(|profiles| profiles.get("profileBindingVersion").is_none()))
    {
        match sidecar
            .and_then(|map| map.get("agents"))
            .or_else(|| main.get("agents"))
        {
            Some(rows) => {
                owner.insert("agents".into(), rows.clone());
            }
            None => {
                owner.remove("agents");
            }
        }
    }
    Value::Object(owner)
}

pub(crate) type RegisteredProfileDocument = (PathBuf, Option<Vec<u8>>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegisteredProfileProblemKind {
    Io,
    JsonSyntax,
    DuplicateMember,
    ProfileSemantic,
}

pub(crate) struct RegisteredProfileProblem {
    pub(crate) path: PathBuf,
    pub(crate) kind: RegisteredProfileProblemKind,
    pub(crate) reason: String,
    pub(crate) ordinary_participant: bool,
}

#[derive(Default)]
pub(crate) struct RegisteredProfileCapture {
    pub(crate) documents: Vec<RegisteredProfileDocument>,
    pub(crate) problems: Vec<RegisteredProfileProblem>,
}

impl RegisteredProfileCapture {
    pub(crate) fn ordinary_error(&self) -> Option<String> {
        self.problems
            .iter()
            .find(|problem| problem.ordinary_participant)
            .map(|problem| problem.reason.clone())
    }

    pub(crate) fn admits_project_migration(&self, path: &Path) -> bool {
        let sidecar = agents_instance_path(path);
        self.problems.iter().all(|problem| {
            !problem.ordinary_participant
                || (problem.path == sidecar
                    && problem.kind == RegisteredProfileProblemKind::JsonSyntax)
        })
    }

    pub(crate) fn participant_documents(&self) -> Vec<RegisteredProfileDocument> {
        self.documents
            .iter()
            .filter(|(path, _)| {
                !self
                    .problems
                    .iter()
                    .any(|problem| !problem.ordinary_participant && &problem.path == path)
            })
            .cloned()
            .collect()
    }
}

// Collect every participant even after a failure. In particular, malformed
// sidecar bytes must not hide a later participating overlay's rejection.
pub(crate) fn collect_registered_profile_gate(path: &Path) -> RegisteredProfileCapture {
    let inputs = [
        path.to_path_buf(),
        agents_instance_path(path),
        path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
    ]
    .into_iter()
    .map(|participant| {
        let input = match settings_sources::bytes(&participant) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        };
        (participant, input)
    })
    .collect();
    classify_registered_profile_inputs(path, inputs)
}

pub(crate) type RegisteredProfileInput = (PathBuf, Result<Option<Vec<u8>>, String>);

pub(crate) fn classify_registered_profile_inputs(
    path: &Path,
    inputs: Vec<RegisteredProfileInput>,
) -> RegisteredProfileCapture {
    let mut captured = RegisteredProfileCapture::default();
    let mut sources = Vec::new();
    for (participant, input) in inputs {
        let overlay = participant.file_name()
            == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME));
        let bytes = match input {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                captured.documents.push((participant, None));
                continue;
            }
            Err(e) => {
                captured.problems.push(RegisteredProfileProblem {
                    reason: format!("{}: {e}", participant.display()),
                    path: participant,
                    kind: RegisteredProfileProblemKind::Io,
                    ordinary_participant: !overlay,
                });
                continue;
            }
        };
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(e) => {
                captured.problems.push(RegisteredProfileProblem {
                    reason: format!("{}: {e}", participant.display()),
                    path: participant.clone(),
                    kind: RegisteredProfileProblemKind::JsonSyntax,
                    ordinary_participant: !overlay,
                });
                captured.documents.push((participant, Some(bytes)));
                continue;
            }
        };
        if participant.file_name() == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME))
            && !AGENTS_INSTANCE_KEYS
                .iter()
                .any(|key| value.get(*key).is_some())
        {
            continue;
        }
        if let Some(reason) = duplicate_profile_document(&bytes, &participant) {
            captured.problems.push(RegisteredProfileProblem {
                path: participant.clone(),
                kind: RegisteredProfileProblemKind::DuplicateMember,
                reason,
                ordinary_participant: true,
            });
        }
        sources.push((participant.clone(), value));
        captured.documents.push((participant, Some(bytes)));
    }
    validate_registered_profile_capture(path, &sources, &mut captured);
    captured
}

pub(crate) fn validate_registered_profile_capture(
    path: &Path,
    sources: &[(PathBuf, Value)],
    captured: &mut RegisteredProfileCapture,
) {
    let sidecar_path = agents_instance_path(path);
    let sidecar = sources
        .iter()
        .find(|(source, _)| source == &sidecar_path)
        .and_then(|(_, value)| value.as_object());
    let main = sources
        .iter()
        .find(|(source, _)| source == path)
        .and_then(|(_, value)| value.as_object())
        .cloned()
        .unwrap_or_default();
    let protected = sources.iter().any(|(source, value)| {
        source.file_name() == Some(std::ffi::OsStr::new(SETTINGS_LOCAL_OVERRIDE_FILE_NAME))
            && AGENTS_INSTANCE_KEYS
                .iter()
                .any(|key| value.get(*key).is_some())
    });
    for (participant, value) in sources {
        let mut profile_source = value.clone();
        // Shadowed rows/groups cannot contribute active collisions. Structural
        // map/quarantine validation still covers each original participant.
        if participant == path
            && sidecar.is_some_and(|map| {
                map.contains_key("agents") || map.contains_key("codingAgentProfiles")
            })
            || physical_profile_rows(&profile_source, true).is_err()
        {
            if let Some(object) = profile_source.as_object_mut() {
                object.remove("agents");
            }
        }
        if let Err(reason) = physical_profile_sets(&profile_source, true) {
            captured.problems.push(RegisteredProfileProblem {
                path: participant.clone(),
                kind: RegisteredProfileProblemKind::ProfileSemantic,
                reason: format!("{}: {reason}", participant.display()),
                ordinary_participant: true,
            });
        }
    }
    if !protected {
        let selected = selected_registered_profile_owner(&main, sidecar, true);
        if physical_profile_rows(&selected, true).is_ok() {
            if let Err(reason) = physical_profile_sets(&selected, true) {
                captured.problems.push(RegisteredProfileProblem {
                    path: path.to_path_buf(),
                    kind: RegisteredProfileProblemKind::ProfileSemantic,
                    reason: format!("{}: {reason}", path.display()),
                    ordinary_participant: true,
                });
            }
        }
    }
}

pub(crate) fn capture_registered_profile_gate(
    path: &Path,
) -> Result<Vec<RegisteredProfileDocument>, String> {
    let captured = collect_registered_profile_gate(path);
    match captured.ordinary_error() {
        Some(reason) => Err(reason),
        None => Ok(captured.participant_documents()),
    }
}

pub(crate) fn captured_registered_profile_object(
    captured: &[RegisteredProfileDocument],
    path: &Path,
) -> Result<Option<Map<String, Value>>, String> {
    let (_, bytes) = captured
        .iter()
        .find(|(source, _)| source == path)
        .ok_or_else(|| format!("staleProfileSource: {}", path.display()))?;
    bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn recheck_registered_profile_gate(
    captured: &[RegisteredProfileDocument],
) -> Result<(), String> {
    for (path, before) in captured {
        let current = match settings_sources::bytes(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if &current != before {
            return Err(format!("staleProfileSource: {}", path.display()));
        }
    }
    Ok(())
}

pub(crate) fn registered_profile_raw_gate(path: &Path) -> Result<(), String> {
    let captured = capture_registered_profile_gate(path)?;
    recheck_registered_profile_gate(&captured)
}
