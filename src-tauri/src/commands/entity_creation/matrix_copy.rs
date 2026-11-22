//! Re-export seam for strict Agent Matrix copying used by entity creation.
//! Production code only needs `MatrixCopyHooks`; the other items are re-exported for tests.
//! No copy logic lives here: it is in `crate::config::entity_creation::copies` and
//! `crate::filesystem::projects::entities::copies`.

#[cfg(test)]
pub(crate) use crate::config::entity_creation::copies::copy_agent_matrix_strict;
#[cfg(test)]
pub(crate) use crate::config::entity_creation::copies::copy_agent_matrix_strict_with_hooks;
#[cfg(test)]
pub(crate) use crate::config::entity_creation::provenance::COPY_PROVENANCE_FILE;
pub(crate) use crate::filesystem::projects::entities::copies::MatrixCopyHooks;
#[cfg(test)]
pub(crate) use crate::filesystem::projects::entities::copies::{
    cleanup_owned_copy_artifacts, MatrixCopyStage, OwnedCopyArtifacts,
};
#[cfg(test)]
pub(crate) use crate::filesystem::projects::entities::copies::{
    MatrixCopyIdentity, MatrixCopyStamp,
};
