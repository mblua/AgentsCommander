use crate::config::agent_command::AgentSpawnCommand;
use crate::phone::mailbox::host::{CoordinatorTicket, MailboxHost};
use crate::phone::mailbox::notice::ResolvedWakeAgentCommand;
use crate::phone::mailbox::poller::MailboxPoller;
use crate::phone::mailbox::pty_claim::MailboxSpawnCall;
use crate::phone::mailbox::tests::CodexClearScannerFixture;
use crate::phone::types::OutboxMessage;
use crate::session::session::{SessionInfo, SessionRepo};

#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_fixture_wake_session<S: MailboxHost>(
    poller: &MailboxPoller,
    app: &S,
    msg: &OutboxMessage,
    resolved_command: &ResolvedWakeAgentCommand,
    cwd: String,
    session_name: String,
    skip_auto_resume: bool,
    skip_tooling_save: bool,
    spawn_shell: String,
    spawn_args: Vec<String>,
    spawn_label: Option<String>,
    resolved_spawn: Option<AgentSpawnCommand>,
) -> Result<SessionInfo, String> {
    let hooks = poller.test_hooks.as_ref().expect("fixture hooks");
    let call = MailboxSpawnCall {
        to: msg.to.clone(),
        session_name: session_name.clone(),
        cwd: cwd.clone(),
        shell: spawn_shell.clone(),
        shell_args: spawn_args.clone(),
        skip_auto_resume,
        skip_tooling_save,
    };
    super::record_spawn_call(hooks, call);

    let spawn_gate = hooks.internal_spawn_gate.lock().unwrap().take();
    if let Some(spawn_gate) = spawn_gate {
        hooks.internal_spawn_started.notify_one();
        let _ = spawn_gate.await;
    }

    let spawn_is_coordinator = *hooks.spawn_is_coordinator.lock().unwrap();
    // #2411 Mirror production create: the coordinator clear runs before the
    // create can fail, on both the failure and the success path.
    // #2413 The ticket ends like production: `failed()` on the create error.
    let ticket = if spawn_is_coordinator {
        app.begin_coordinator_create(&cwd)
    } else {
        None
    };
    if let Some(msg) = hooks.internal_spawn_error.lock().unwrap().take() {
        if let Some(t) = ticket {
            t.failed();
        }
        return Err(msg);
    }
    if let Some(t) = ticket {
        t.succeeded();
    }
    let session_mgr = app.pty_sessions();
    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(
            spawn_shell.clone(),
            spawn_args.clone(),
            cwd,
            resolved_spawn
                .as_ref()
                .map(|spawn| spawn.trusted_agent_id.clone())
                .or_else(|| resolved_command.agent_id.clone()),
            spawn_label.clone(),
            Vec::<SessionRepo>::new(),
            spawn_is_coordinator,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .map_err(|e| e.to_string())?;
    mgr.rename_session(session.id, session_name)
        .await
        .map_err(|e| e.to_string())?;
    mgr.mark_idle(session.id).await;
    let inserted = mgr
        .get_session(session.id)
        .await
        .ok_or_else(|| format!("Session {} not found after test spawn", session.id))?;
    drop(mgr);
    CodexClearScannerFixture::prepare_spawned_clear_session(app, hooks, inserted.id)?;
    Ok(SessionInfo::from(&inserted))
}
