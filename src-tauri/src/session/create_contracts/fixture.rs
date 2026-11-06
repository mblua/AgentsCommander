use super::{OwnedReplacementCreateRequest, OwnedReplacementCreateStep};
use std::sync::Arc;

/// Owns the original locks only across the admitted test create cut. These
/// values never enter the retained operation record.
#[cfg(test)]
pub(crate) struct FixturePtyCutLease {
    pub(crate) gate: Arc<crate::api::message_store::PtyInputTargetGate>,
    pub(crate) target: String,
    pub(crate) exact: crate::api::message_store::PtyInputTargetGuard,
    pub(crate) stripe: crate::api::message_store::PtyInputStripeGuard,
}

#[cfg(test)]
pub(crate) enum FixtureReplacementLease {
    OrdinaryMoved(FixturePtyCutLease),
    PtyCutShared(Arc<FixturePtyCutLease>),
}

#[cfg(test)]
pub(crate) struct FixtureReplacementInput {
    pub(crate) request: OwnedReplacementCreateRequest,
    pub(crate) continuation: crate::session::selection::OwnedReplacementContinuation,
    pub(crate) lease: FixtureReplacementLease,
}

#[cfg(test)]
pub(crate) struct FixtureReplacementResult {
    pub(crate) outcome: Result<OwnedReplacementCreateStep, String>,
    pub(crate) ordinary_lease: Option<FixturePtyCutLease>,
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementResult")
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl std::fmt::Debug for FixtureReplacementInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureReplacementInput")
            .field("continuation", &self.continuation)
            .finish_non_exhaustive()
    }
}
