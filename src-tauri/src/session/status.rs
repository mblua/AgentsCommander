//! Session lifecycle state: the serializable `SessionStatus` enum (`Active`, `Running`,
//! `Idle`, `Exited` with its exit code), serialized in camelCase.
//! Data only; transitions are decided elsewhere.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Active,
    Running,
    Idle,
    Exited(i32),
}
