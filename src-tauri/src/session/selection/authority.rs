use crate::session::session::SessionCommunication;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct CommitCapability {
    pub(crate) _private: super::seal::Token,
}

#[cfg(test)]
impl CommitCapability {
    pub(crate) fn for_test() -> Self {
        Self {
            _private: super::seal::Token,
        }
    }
}

pub(crate) trait CreateBindingTicket<B>: super::seal::Sealed {
    fn bind(&mut self, session_id: Uuid) -> Result<B, String>;
}

pub(crate) trait FinalizedOwnedCarry: super::seal::Sealed {
    fn bound_operation(&self) -> bool;
    fn session_id(&self) -> Uuid;
    fn token(&self) -> Uuid;
    fn expected_version(&self) -> u64;
    fn telegram_bot_id(&self) -> &Option<String>;
    fn communication(&self) -> &Option<SessionCommunication>;
    fn start_fresh_on_restore(&self) -> bool;
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FinalizedOwnedCarryOutcome {
    Applied,
    Recapture,
}
