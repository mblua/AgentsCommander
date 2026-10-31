use crate::session::session::SessionInfo;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) async fn await_internal_spawn<F>(
    spawn: F,
    cancellation: &CancellationToken,
    target_fqn: &str,
) -> Result<Uuid, String>
where
    F: std::future::Future<Output = Result<SessionInfo, String>>,
{
    tokio::pin!(spawn);
    let spawn_result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            return Err(
                "Context alert delivery was canceled during background spawn".to_string(),
            );
        }
        result = &mut spawn => result,
    };
    let info = match spawn_result {
        Ok(info) => info,
        Err(error) => {
            // #2413 A failed create restores its cleared close markers itself
            // (commands::session::CoordinatorCreateTicket), for every caller.
            return Err(format!(
                "Failed to spawn supported orchestrator session for '{}': {}",
                target_fqn, error
            ));
        }
    };
    Uuid::parse_str(&info.id).map_err(|e| format!("Invalid spawned orchestrator session id: {}", e))
}
