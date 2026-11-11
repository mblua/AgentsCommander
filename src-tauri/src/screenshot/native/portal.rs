//! #2774 xdg-desktop-portal access for Wayland screenshot capture (ashpd 0.11.1).

use std::collections::HashMap;
use std::path::PathBuf;

use ashpd::desktop::global_shortcuts::GlobalShortcuts;
use ashpd::desktop::screenshot::Screenshot;
use ashpd::desktop::ResponseError;
use ashpd::zbus;
use ashpd::zvariant::Value;

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
