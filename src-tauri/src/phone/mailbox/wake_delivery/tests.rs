use super::{
    owned_replacement_request, MailboxHost, MailboxPoller, OutboxMessage,
    OwnedReplacementContinuation, Uuid,
};

impl MailboxPoller {
    #[cfg(test)]
    pub(crate) async fn fixture_wake_request<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
    ) -> crate::session::create_contracts::OwnedReplacementCreateRequest {
        let plan = self.resolve_wake_spawn_plan(app, msg).await.unwrap();
        owned_replacement_request(plan, false).0
    }

    #[cfg(test)]
    pub(crate) async fn fixture_probe_once<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        previous: Uuid,
    ) {
        let fixture = OwnedReplacementContinuation::fixture_wake_rollback_state().unwrap();
        let _resume = fixture.after_probe.arm();
        let pause = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            fixture.after_probe.entered.notified(),
        );
        tokio::select! {
            reached = pause => reached.expect("competing wake reached its authenticated probe"),
            _ = self.fixture_probe_owned_wake(app, msg, previous) => panic!("wake missed probe"),
        }
    }

    #[cfg(test)]
    pub(crate) async fn fixture_probe_owned_wake<S: MailboxHost>(
        &self,
        app: &S,
        msg: &OutboxMessage,
        previous: Uuid,
    ) {
        let request = self.fixture_wake_request(app, msg).await;
        let _ = self
            .admit_owned_peer_replacement(app, msg, &request, previous)
            .await;
    }
}
