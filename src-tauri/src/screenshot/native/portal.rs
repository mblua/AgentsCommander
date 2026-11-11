//! #2774 xdg-desktop-portal access for Wayland screenshot capture (ashpd 0.11.1).

use std::collections::HashMap;
use std::path::PathBuf;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::screenshot::Screenshot;
use ashpd::desktop::ResponseError;
use ashpd::zbus;
use ashpd::zvariant::Value;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt as _;

/// The portal owner (D-Bus unique name) we registered with, and that attempt's result.
pub(crate) struct HostRecord {
    pub(crate) owner: String,
    pub(crate) result: Result<(), String>,
}

pub(crate) type HostRegistration = tokio::sync::Mutex<Option<HostRecord>>;

/// Ok(owner) when this process is registered with the CURRENT portal owner.
pub(crate) async fn ensure_host_registered(
    host: &HostRegistration,
    app_id: &str,
) -> Result<String, String> {
    let proxy = GlobalShortcuts::new().await.map_err(|e| e.to_string())?;
    let owner = portal_owner(proxy.connection()).await?;
    let mut rec = host.lock().await;
    if let Some(r) = cached_result(rec.as_ref(), &owner) {
        return r;
    }
    let result = register_host(proxy.connection(), app_id).await;
    *rec = Some(HostRecord {
        owner: owner.clone(),
        result: result.clone(),
    });
    result.map(|()| owner)
}

/// The cached result for `owner`, or `None` when the portal owner changed.
pub(crate) fn cached_result(
    record: Option<&HostRecord>,
    owner: &str,
) -> Option<Result<String, String>> {
    let r = record.filter(|r| r.owner == owner)?;
    Some(r.result.clone().map(|()| owner.to_string()))
}

/// Ask the bus daemon (not the portal) who owns the portal name.
async fn portal_owner(c: &zbus::Connection) -> Result<String, String> {
    let reply = c
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &("org.freedesktop.portal.Desktop",),
        )
        .await
        .map_err(|e| e.to_string())?;
    reply
        .body()
        .deserialize::<String>()
        .map_err(|e| e.to_string())
}

/// Not `ashpd::register_host_app`: its `AppID` rejects ids without a dot.
async fn register_host(c: &zbus::Connection, app_id: &str) -> Result<(), String> {
    let options = HashMap::<&str, Value<'_>>::new();
    c.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.host.portal.Registry"),
        "Register",
        &(app_id, options),
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        log::warn!("[screenshot] portal host registration as {app_id} failed: {e}");
        e.to_string()
    })
}

/// Open the desktop's own area selector. `Ok(None)` = the user dismissed it.
/// No timeout: `Request.Close` does not dismiss the Hyprland selector.
pub(crate) async fn pick_region(
    host: &HostRegistration,
    app_id: &str,
) -> Result<Option<PathBuf>, String> {
    if let Err(e) = ensure_host_registered(host, app_id).await {
        log::debug!("[screenshot] portal host registration unavailable: {e}");
    }
    let response = Screenshot::request()
        .interactive(true)
        .modal(false)
        .send()
        .await
        .and_then(|request| request.response());
    match response {
        Ok(shot) => shot
            .uri()
            .to_file_path()
            .map(Some)
            .map_err(|()| "screenshot portal returned a non-file URI".to_string()),
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Presses of a bound portal shortcut; the stream ending means the shortcut ended.
pub(crate) type Presses = mpsc::UnboundedReceiver<()>;

/// Bind one portal global shortcut under the registered `app_id`. Never binds without
/// the registered id: a shortcut under another id is a dead key.
pub(crate) async fn bind_shortcut(
    host: &HostRegistration,
    app_id: &str,
    shortcut_id: &str,
    description: &str,
    trigger: &str,
) -> Result<Presses, String> {
    let owner = ensure_host_registered(host, app_id)
        .await
        .map_err(|e| format!("could not register with the desktop portal as {app_id}: {e}"))?;
    let (setup_tx, setup_rx) = oneshot::channel();
    tokio::spawn(shortcut_task(
        owner,
        shortcut_id.to_string(),
        description.to_string(),
        trigger.to_string(),
        setup_tx,
    ));
    setup_rx
        .await
        .unwrap_or_else(|_| Err("portal shortcut task stopped".into()))
}

/// Owns proxy, session and streams (the streams borrow the proxy). Reports the setup
/// result once, then forwards presses until the portal ends the shortcut.
async fn shortcut_task(
    owner: String,
    id: String,
    description: String,
    trigger: String,
    setup: oneshot::Sender<Result<Presses, String>>,
) {
    macro_rules! step {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => {
                    let _ = setup.send(Err(e.to_string()));
                    return;
                }
            }
        };
    }
    let proxy = step!(GlobalShortcuts::new().await);
    // Subscribe before binding, so no press is lost.
    let activated = step!(proxy.receive_activated().await);
    let owner_changed = step!(proxy.receive_owner_changed().await);
    let session = step!(proxy.create_session().await);
    let closed = step!(session.receive_closed().await);
    let shortcut =
        NewShortcut::new(id.as_str(), description.as_str()).preferred_trigger(trigger.as_str());
    let request = step!(proxy.bind_shortcuts(&session, &[shortcut], None).await);
    let resp = step!(request.response());
    if !resp.shortcuts().iter().any(|s| s.id() == id) {
        step!(Err::<(), _>(format!(
            "the desktop portal did not bind the shortcut {id}"
        )));
    }
    // Closes the window between the registration and the owner-changed subscription.
    if step!(portal_owner(proxy.connection()).await) != owner {
        step!(Err::<(), _>(
            "the desktop portal restarted during the shortcut setup"
        ));
    }
    let (tx, rx) = mpsc::unbounded();
    let _ = setup.send(Ok(rx));
    let mut activated = std::pin::pin!(activated);
    let mut owner_changed = std::pin::pin!(owner_changed);
    let mut closed = std::pin::pin!(closed);
    let why = loop {
        tokio::select! {
            a = activated.next() => match a {
                None => break "activation stream ended",
                Some(a) if a.shortcut_id() == id => {
                    if tx.unbounded_send(()).is_err() {
                        break "receiver gone";
                    }
                }
                Some(_) => {}
            },
            // Any item: the old session and its binding are gone.
            _ = owner_changed.next() => break "portal restarted",
            _ = closed.next() => break "session closed",
        }
    };
    log::warn!("[screenshot] portal shortcut ended: {why}");
    drop(tx);
}
