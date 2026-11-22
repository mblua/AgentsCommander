//! `SessionBackendKind`: which backend runs a session, a local process (the default)
//! or a container transport.
//! Serialized in camelCase. Holds no behavior.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionBackendKind {
    #[default]
    LocalProcess,
    ContainerTransport,
}
