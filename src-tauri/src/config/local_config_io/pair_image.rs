use crate::filesystem::local_config::{inspect_local_config_target, read_local_config_bytes};
use serde_json::{Map, Value};
use std::cell::Cell;
use std::path::Path;

/// Exact private disk image. Absence is distinct from an existing empty object.
/// Bytes are recovery authority; the digest is checked when a plan is executed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum PhysicalState {
    Absent,
    Bytes { sha256: String, bytes: Vec<u8> },
}

impl PhysicalState {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        use sha2::{Digest, Sha256};
        Self::Bytes {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        }
    }

    pub(super) fn map(&self) -> Result<Map<String, Value>, PreparedPairError> {
        match self {
            Self::Absent => Ok(Map::new()),
            Self::Bytes { bytes, .. } => {
                if Self::from_bytes(bytes.clone()) != *self {
                    return Err(PreparedPairError::InvalidPlan("physical digest mismatch"));
                }
                match serde_json::from_slice(bytes) {
                    Ok(Value::Object(map)) => Ok(map),
                    _ => Err(PreparedPairError::InvalidPlan(
                        "physical image must be a JSON object",
                    )),
                }
            }
        }
    }
}

pub(crate) fn read_config_pair_physical(path: &Path) -> Result<PhysicalState, PreparedPairError> {
    match inspect_local_config_target(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(PreparedPairError::Io(
                "PAIR target must be a regular non-symlink file".into(),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PhysicalState::Absent),
        Err(e) => return Err(PreparedPairError::Io(e.to_string())),
    }
    read_local_config_bytes(path)
        .map(PhysicalState::from_bytes)
        .map_err(|e| PreparedPairError::Io(e.to_string()))
}

/// T0=(D0,S0), T1=(D0,Sc), T2=(Dc,Sc), T3=(Dc,Sf), T4=(Df,Sf).
/// No locator, callback or journal is persisted here. The authorized caller
/// supplies the target and persists this private plan in its own protocol.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedConfigPairPlan {
    pub(super) stages: [[PhysicalState; 2]; 5],
}

impl PreparedConfigPairPlan {
    pub(crate) fn stages(&self) -> &[[PhysicalState; 2]; 5] {
        &self.stages
    }

    pub(super) fn validate(&self) -> Result<(), PreparedPairError> {
        for tuple in &self.stages {
            for image in tuple {
                image.map()?;
            }
        }
        for (stage, changed_side) in [(1, 1), (2, 0), (3, 1), (4, 0)] {
            if self.stages[stage][1 - changed_side] != self.stages[stage - 1][1 - changed_side]
                || (self.stages[stage][changed_side] == PhysicalState::Absent
                    && self.stages[stage - 1][changed_side] != PhysicalState::Absent)
            {
                return Err(PreparedPairError::InvalidPlan(
                    "invalid ordered PAIR images",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreparedPairError {
    InvalidPlan(&'static str),
    Preparation(String),
    Conflict,
    Pending,
    Io(String),
}

impl std::fmt::Display for PreparedPairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(reason) => write!(f, "invalidPreparedPair: {reason}"),
            Self::Preparation(reason) => write!(f, "preparedPairRejected: {reason}"),
            Self::Conflict => {
                f.write_str("preparedPairConflict: current tuple is not an authorized stage")
            }
            Self::Pending => {
                f.write_str("targetTransitionPending: reserved target is not finalized")
            }
            Self::Io(reason) => write!(f, "preparedPairIo: {reason}"),
        }
    }
}

impl std::error::Error for PreparedPairError {}

pub(super) fn prepared_pair_image(
    previous: &PhysicalState,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<PhysicalState, PreparedPairError> {
    if before == after {
        return Ok(previous.clone());
    }
    let mut bytes = serde_json::to_vec_pretty(after)
        .map_err(|e| PreparedPairError::Preparation(e.to_string()))?;
    bytes.push(b'\n');
    Ok(PhysicalState::from_bytes(bytes))
}

/// #2786 (C1) - the caller-supplied cleanup sequence of `update_config_pair`:
/// it edits the decisions map and the state map before `mutate` runs.
pub(crate) type ConfigPairCleanup<'a> =
    &'a dyn Fn(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>;

pub(in crate::config) type ConfigPublisher<'a> = &'a dyn Fn(&Path, &Path) -> Result<(), String>;

pub(crate) type ConfigPairFinish<'a> = &'a dyn Fn(&mut Map<String, Value>) -> Result<(), String>;

// Synchronous borrowed validation only; no filesystem authority is constructed here.
pub(crate) struct PairTargetGuard<'a> {
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
    pub(crate) failed: Cell<bool>,
}

impl<'a> PairTargetGuard<'a> {
    pub(crate) fn new(validate: &'a dyn Fn() -> Result<(), PreparedPairError>) -> Self {
        Self {
            validate,
            failed: Cell::new(false),
        }
    }

    pub(crate) fn check(&self) -> Result<(), PreparedPairError> {
        if self.failed.get() {
            return Err(PreparedPairError::Conflict);
        }
        match (self.validate)() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed.set(true);
                Err(error)
            }
        }
    }
}

pub(crate) fn log_unverified_cleanup_refusal(
    primary: &PreparedPairError,
    refusal: &PreparedPairError,
) {
    log::warn!(
        "[prepared-pair] cleanup refused; primary={primary}; refusal={refusal}; staging retained"
    );
}

pub(crate) fn ordinary_publish_result(result: Result<(), PreparedPairError>) -> Result<(), String> {
    result.map_err(|error| match error {
        PreparedPairError::Io(reason) => reason,
        other => other.to_string(),
    })
}
