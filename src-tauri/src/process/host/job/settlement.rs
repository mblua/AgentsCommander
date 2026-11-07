/// One monotonic native-settlement attempt budget. Updater, probe, and rejected
/// suspended-spawn cleanup pass this same deadline through direct-child, reader,
/// and native tree proof work; a new deadline is created only after the current
/// attempt has actually expired.
pub(crate) struct SettlementAttempt {
    window: std::time::Duration,
    deadline: std::time::Instant,
}

impl SettlementAttempt {
    pub(crate) fn new(window: std::time::Duration) -> Self {
        Self {
            window,
            deadline: std::time::Instant::now() + window,
        }
    }

    pub(crate) fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }

    pub(crate) fn expired(&self) -> bool {
        self.remaining().is_zero()
    }

    pub(crate) fn restart(&mut self) {
        self.deadline = std::time::Instant::now() + self.window;
    }
}
