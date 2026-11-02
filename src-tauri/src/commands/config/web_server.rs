use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Manager, State};
use uuid::Uuid;

use crate::api::auth;
use crate::config::settings::{
    load_settings, parse_api_server_socket_addr, AppSettings, SettingsState,
};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::web::command_port::ManagedWebCommandDispatcher;
use crate::{
    ApiServerHandle, ApiServerTask, WebServerHandle, WebServerLifecycle,
    WebServerLifecycleSnapshot, WebServerStartWaiter, WEB_SERVER_START_CANCELLED,
};

const WEB_STATUS_CONNECT_TIMEOUT_MS: u64 = 500;
const API_SERVER_STOP_TIMEOUT_MS: u64 = 5_000;
// Default mirrors container API token TTL; manual GUI mints get a longer cap.
pub(crate) const MINT_API_CLIENT_DEFAULT_TTL_HOURS: i64 = 24;
pub(crate) const MINT_API_CLIENT_MAX_TTL_DAYS: i64 = 30;
pub(crate) const MINT_API_CLIENT_NOTE: &str =
    "Store this token now; it is shown only once. The registry keeps only a hash. A manually \
        requested pty-input scope does not grant actuation without an automatically bound live \
        container session.";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintApiClientResponse {
    pub client_id: String,
    pub token: String,
    pub bound_fqn: String,
    pub bound_root: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WebServerOwnershipState {
    Starting,
    OwnedRunning,
    Stopping,
    ExternalListening,
    Stopped,
}

/// §1453: motivo del ultimo arranque fallido, expuesto al frontend.
/// `detail` es el texto verbatim del error subyacente; la UI lo demota a
/// linea secundaria y construye el titular llano por su cuenta (D8).
/// CONTRATO (D2): `bind` es SIEMPRE el string de settings crudo, en las dos
/// variantes del error. Quien lo cruce contra la lista de interfaces debe
/// gatear antes con un shape IPv4 (ver 5.7.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerBindFailure {
    pub bind: String,
    pub port: u16,
    pub detail: String,
}

/// §1453: una direccion IPv4 ofrecible como bind, con su adaptador.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerInterfaceInfo {
    pub address: String,
    pub interface_name: String,
    pub is_virtual: bool,
}

/// §1453: unica conversion error -> payload. Reemplaza los accesores
/// bind()/port()/detail() de la ronda 1: la conversion vive donde vive el
/// payload. Emite el `bind` CRUDO en ambas variantes (contrato de D2).
impl From<&crate::web::StartServerError> for WebServerBindFailure {
    fn from(err: &crate::web::StartServerError) -> Self {
        let (bind, port, detail) = match err {
            crate::web::StartServerError::InvalidAddr { bind, port, detail } => {
                (bind.clone(), *port, detail.clone())
            }
            crate::web::StartServerError::BindFailed { bind, addr, detail } => {
                (bind.clone(), addr.port(), detail.clone())
            }
        };
        WebServerBindFailure { bind, port, detail }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebServerOwnedStatus {
    pub listening: bool,
    pub owned: bool,
    pub external_listening: bool,
    pub open_allowed: bool,
    pub bind: String,
    pub port: u16,
    pub state: WebServerOwnershipState,
    pub bind_failure: Option<WebServerBindFailure>,
}

pub(crate) fn ensure_web_remote_open_allowed(
    settings: &AppSettings,
    ws_handle: &WebServerHandle,
) -> Result<WebServerLifecycleSnapshot, String> {
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    let snapshot = ws_handle.snapshot();
    if snapshot.lifecycle != WebServerLifecycle::Running
        || snapshot.endpoint.as_ref()
            != Some(&(settings.web_server_bind.clone(), settings.web_server_port))
    {
        return Err("Web server is not owned by this app process".into());
    }

    Ok(snapshot)
}

/// §1453 D2: el status solo carga el fallo si describe el target configurado
/// AHORA y si nada esta escuchando. Un fallo grabado para un bind/port
/// anterior (p.ej. el usuario edito el puerto estando parado) no debe pintar
/// la alerta del target nuevo, y un fallo viejo no debe pintar sobre algo que
/// SI esta sirviendo. El slot significa exactamente "esto explica por que
/// nada esta sirviendo".
/// El guard de `listening` es la SEGUNDA guardia, independiente de la del
/// frontend (`showBindAlert`, 5.7.2), que sigue siendo la primaria: ninguna
/// de las dos es load-bearing sola porque los dos early-returns Ok(false) de
/// start_web_server nunca graban fallo.
pub(crate) fn select_current_bind_failure(
    recorded: Option<crate::web::StartServerError>,
    bind: &str,
    port: u16,
    listening: bool,
) -> Option<WebServerBindFailure> {
    if listening {
        return None;
    }
    recorded
        .as_ref()
        .map(WebServerBindFailure::from)
        .filter(|failure| failure.bind == bind && failure.port == port)
}

pub(crate) fn build_web_server_owned_status(
    bind: String,
    port: u16,
    lifecycle: WebServerLifecycle,
    listening: bool,
    bind_failure: Option<WebServerBindFailure>,
) -> WebServerOwnedStatus {
    let (state, owned, external_listening, open_allowed) = match lifecycle {
        WebServerLifecycle::Starting => (WebServerOwnershipState::Starting, true, false, false),
        WebServerLifecycle::Running => (WebServerOwnershipState::OwnedRunning, true, false, true),
        WebServerLifecycle::Stopping => (WebServerOwnershipState::Stopping, true, false, false),
        WebServerLifecycle::Stopped if listening => (
            WebServerOwnershipState::ExternalListening,
            false,
            true,
            false,
        ),
        WebServerLifecycle::Stopped => (WebServerOwnershipState::Stopped, false, false, false),
    };

    WebServerOwnedStatus {
        listening,
        owned,
        external_listening,
        open_allowed,
        bind,
        port,
        state,
        bind_failure,
    }
}

pub(crate) fn web_remote_url(bind: &str, port: u16, token: &str) -> Result<String, String> {
    let destination_ip = match bind
        .parse::<IpAddr>()
        .map_err(|error| format!("Invalid web server bind address '{bind}': {error}"))?
    {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let socket_addr = SocketAddr::new(destination_ip, port);

    Ok(format!(
        "http://{socket_addr}/?window=browser&remoteToken={token}"
    ))
}

#[tauri::command]
pub async fn open_web_remote(ws_handle: State<'_, WebServerHandle>) -> Result<(), String> {
    let settings = load_settings();
    let authorized = ensure_web_remote_open_allowed(&settings, &ws_handle)?;

    let token = crate::web::auth::read_persisted_web_token(crate::config::config_dir().as_deref())?;

    let (bind, port) = authorized
        .endpoint
        .as_ref()
        .ok_or("Web server is not owned by this app process")?;
    let url = web_remote_url(bind, *port, token.trim())?;

    let current_settings = load_settings();
    let revalidated = ensure_web_remote_open_allowed(&current_settings, &ws_handle)?;
    if revalidated != authorized {
        return Err("Web server is not owned by this app process".into());
    }

    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn start_api_server(
    app_handle: tauri::AppHandle,
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(false);
    }

    let s = settings.read().await;
    let bind = s.api_server_bind.clone();
    let port = s.api_server_port;
    drop(s);

    let probe_addr = api_server_probe_addr(&bind, port)?;
    if is_tcp_socket_listening(probe_addr).await {
        return Ok(false);
    }

    let api_shutdown = shutdown.token().child_token();
    let server_start = crate::api::start_server(
        bind,
        port,
        app_handle,
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        api_shutdown.clone(),
    );
    let bound_addr = match crate::api::wait_for_startup_ready(server_start.readiness).await {
        Ok(bound_addr) => bound_addr,
        Err(err) => {
            api_shutdown.cancel();
            return Err(err);
        }
    };

    if !api_handle.store_if_idle(ApiServerTask::new(
        server_start.join_handle,
        api_shutdown,
        bound_addr,
    ))? {
        return Ok(false);
    }

    log::info!("[api-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_api_server(api_handle: State<'_, ApiServerHandle>) -> Result<bool, String> {
    let stopped = api_handle
        .shutdown_running(Duration::from_millis(API_SERVER_STOP_TIMEOUT_MS))
        .await?;
    if stopped {
        log::info!("[api-server] Stopped via command");
    }
    Ok(stopped)
}

#[tauri::command]
pub async fn api_server_status(
    api_handle: State<'_, ApiServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<bool, String> {
    if api_handle.has_running()? {
        return Ok(true);
    }

    let probe_addr = {
        let s = settings.read().await;
        api_server_probe_addr(&s.api_server_bind, s.api_server_port)?
    };
    Ok(is_tcp_socket_listening(probe_addr).await)
}

#[tauri::command]
pub async fn mint_api_client(
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
) -> Result<MintApiClientResponse, String> {
    let settings = load_settings();
    let path = crate::config::config_dir()
        .ok_or_else(|| "Cannot resolve the host config directory".to_string())?
        .join(auth::REGISTRY_FILENAME);
    mint_api_client_with_path(
        &path,
        &settings,
        root,
        scopes,
        label,
        expires,
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        chrono::Utc::now().to_rfc3339(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn mint_api_client_with_path(
    path: &Path,
    settings: &AppSettings,
    root: String,
    scopes: Vec<String>,
    label: Option<String>,
    expires: Option<String>,
    client_id: String,
    secret: String,
    issued_at: String,
) -> Result<MintApiClientResponse, String> {
    if crate::config::root_agent::is_root_agent_path(&root) {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let (bound_root, bound_fqn) = validate_mint_api_client_root(&root, settings)?;
    if crate::config::root_agent::is_root_agent_target(&bound_fqn)
        || bound_fqn == crate::config::root_agent::ROOT_AGENT_SENDER
    {
        return Err(
            "Cannot mint an API client bound to the Root Agent; root-agent is API-excluded"
                .to_string(),
        );
    }

    let scopes: Vec<String> = scopes
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    auth::validate_scopes(&scopes)?;
    let expires_at = bounded_mint_api_client_expiry(expires, &issued_at)?;

    let out = auth::mint(
        path,
        auth::MintRequest {
            client_id,
            secret,
            label: label.unwrap_or_default(),
            bound_root: bound_root.clone(),
            bound_fqn: bound_fqn.clone(),
            scopes: scopes.clone(),
            issued_at,
            expires_at: Some(expires_at.clone()),
            bound_session_id: None,
            credential_generation: None,
        },
    )?;

    crate::api::audit::record(&out.client_id, &bound_fqn, "mint", "ok");
    Ok(MintApiClientResponse {
        client_id: out.client_id,
        token: out.secret,
        bound_fqn,
        bound_root,
        scopes,
        expires_at: Some(expires_at),
        note: MINT_API_CLIENT_NOTE.to_string(),
    })
}

pub(crate) fn validate_mint_api_client_root(
    root: &str,
    settings: &AppSettings,
) -> Result<(String, String), String> {
    let canonical = crate::config::coding_agent_profiles::validate_profile_selection_agent_path(
        settings,
        Path::new(root),
    )
    .map_err(|_| format!("unknown or invalid root: {}", root))?
    .launch_path;

    let is_replica = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false);
    if !is_replica {
        return Err(format!("unknown or invalid root: {}", root));
    }

    let bound_root = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&canonical);
    let bound_fqn = crate::config::teams::agent_fqn_from_path(&bound_root);
    Ok((bound_root, bound_fqn))
}

pub(crate) fn bounded_mint_api_client_expiry(
    expires: Option<String>,
    issued_at: &str,
) -> Result<String, String> {
    let issued_at = chrono::DateTime::parse_from_rfc3339(issued_at)
        .map_err(|e| format!("Invalid issuedAt timestamp: {}", e))?
        .with_timezone(&chrono::Utc);
    let max_expires_at = issued_at + chrono::Duration::days(MINT_API_CLIENT_MAX_TTL_DAYS);

    let expires_at = match expires {
        Some(raw) => {
            let trimmed = raw.trim();
            let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
                .map_err(|e| format!("Invalid expires value; expected RFC3339: {}", e))?
                .with_timezone(&chrono::Utc);
            if parsed <= issued_at {
                return Err("expires must be after the issuedAt timestamp".to_string());
            }
            if parsed > max_expires_at {
                return Err(format!(
                    "expires exceeds maximum API client lifetime of {} days",
                    MINT_API_CLIENT_MAX_TTL_DAYS
                ));
            }
            parsed
        }
        None => issued_at + chrono::Duration::hours(MINT_API_CLIENT_DEFAULT_TTL_HOURS),
    };

    Ok(expires_at.to_rfc3339())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_web_server_start(
    ws_handle: WebServerHandle,
    settings: SettingsState,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: Arc<std::sync::Mutex<PtyManager>>,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    shutdown: crate::shutdown::ShutdownSignal,
) -> WebServerStartWaiter {
    let lifecycle = ws_handle.clone();
    let factory_shutdown = shutdown.clone();
    ws_handle.begin_start(
        shutdown,
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, admission, generation_token| async move {
            let dispatcher = app_handle
                .try_state::<ManagedWebCommandDispatcher>()
                .map(|managed| Arc::clone(&managed.0))
                .ok_or("Web command dispatcher is not registered")?;
            let s = settings.read().await;
            let bind = s.web_server_bind.clone();
            let port = s.web_server_port;
            drop(s);

            if !lifecycle.publish_effective_endpoint(generation, bind.clone(), port) {
                return Err(WEB_SERVER_START_CANCELLED.to_string());
            }

            let probe_addr = match web_server_probe_addr(&bind, port) {
                Ok(addr) => addr,
                Err(detail) => {
                    let error = crate::web::StartServerError::InvalidAddr { bind, port, detail };
                    log::warn!("[web-server] start failed: {}", error);
                    lifecycle.record_bind_failure(error);
                    return Ok(None);
                }
            };

            if is_tcp_listening(probe_addr).await {
                return Ok(None);
            }
            if generation_token.is_cancelled() || factory_shutdown.is_cancelled() {
                return Err(WEB_SERVER_START_CANCELLED.to_string());
            }

            let resource_dir = app_handle.path().resource_dir().ok();
            match crate::web::start_server(
                bind,
                port,
                web_token,
                session_mgr,
                dispatcher,
                broadcaster,
                resource_dir,
                admission,
                generation_token,
                factory_shutdown,
                |future| {
                    crate::web_server::ServerTask::from_native(tauri::async_runtime::spawn(future))
                },
            )
            .await
            {
                Ok(join_handle) => {
                    lifecycle.clear_bind_failure();
                    Ok(Some(join_handle))
                }
                Err(error) => {
                    log::warn!("[web-server] start failed: {}", error);
                    lifecycle.record_bind_failure(error);
                    Ok(None)
                }
            }
        },
    )
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_web_server(
    app_handle: tauri::AppHandle,
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
    web_token: State<'_, Arc<WebAccessToken>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    broadcaster: State<'_, WsBroadcaster>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    let waiter = begin_web_server_start(
        ws_handle.inner().clone(),
        Arc::clone(&settings),
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        (*broadcaster).clone(),
        app_handle,
        shutdown.inner().clone(),
    );
    let started = waiter.wait().await?;
    if started {
        log::info!("[web-server] Started via command");
    }
    Ok(started)
}

#[tauri::command]
pub async fn stop_web_server(ws_handle: State<'_, WebServerHandle>) -> Result<bool, String> {
    stop_web_server_handle(&ws_handle).await
}

pub(crate) async fn stop_web_server_handle(ws_handle: &WebServerHandle) -> Result<bool, String> {
    let Some(waiter) = ws_handle.begin_stop() else {
        return Ok(false);
    };
    waiter.wait().await?;
    log::info!("[web-server] Stopped via command");
    Ok(true)
}

#[tauri::command]
pub async fn get_web_server_status(settings: State<'_, SettingsState>) -> Result<bool, String> {
    let s = settings.read().await;
    let addr = web_server_probe_addr(&s.web_server_bind, s.web_server_port).ok();
    drop(s);
    Ok(match addr {
        Some(addr) => is_tcp_listening(addr).await,
        None => false,
    })
}

#[tauri::command]
pub async fn get_web_server_owned_status(
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
) -> Result<WebServerOwnedStatus, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    Ok(resolve_web_server_owned_status(&ws_handle, (bind, port), is_tcp_listening).await)
}

/// §1453 D3: heuristica por nombre para separar adaptadores virtuales/tunel
/// de los fisicos. Cubre los friendly names de Windows y los prefijos unix.
/// Un falso positivo/negativo solo mueve la fila de grupo en el popover; la
/// fila sigue siendo seleccionable, asi que el costo de un miss es cosmetico.
pub(crate) fn classify_virtual_interface(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    // "loopback" NO va aca: la clasificacion corre en el .map() POSTERIOR al
    // filtro de loopback, asi que seria una rama inalcanzable (dev-rust).
    const CONTAINS: [&str; 10] = [
        "vethernet",
        "wsl",
        "tailscale",
        "virtualbox",
        "vmware",
        "hyper-v",
        "docker",
        "zerotier",
        "hamachi",
        "wireguard",
    ];
    const PREFIXES: [&str; 9] = [
        "veth", "virbr", "vmnet", "tun", "tap", "utun", "wg", "br-", "zt",
    ];
    CONTAINS.iter().any(|needle| lowered.contains(needle))
        || PREFIXES.iter().any(|prefix| lowered.starts_with(prefix))
}

/// §1453 D1/D3: transforma pares (nombre, IPv4) crudos en la lista ofrecida.
/// Filtra loopback (la representa el preset Localhost), link-local 169.254/16
/// (APIPA: exactamente la clase de bind fragil que este issue elimina) y
/// unspecified; ordena fisicas primero y dedup exacto.
///
/// NO BORRAR el filtro is_link_local() "por muerto": en Windows es un no-op
/// porque el crate ya descarta 169.254.* antes de entregarlas, pero en
/// Linux/macOS SI llegan y el filtro es el unico que las saca (D1).
pub(crate) fn map_web_server_interfaces(
    raw: Vec<(String, std::net::Ipv4Addr)>,
) -> Vec<WebServerInterfaceInfo> {
    let mut out: Vec<WebServerInterfaceInfo> = raw
        .into_iter()
        .filter(|(_, ip)| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified())
        .map(|(name, ip)| WebServerInterfaceInfo {
            address: ip.to_string(),
            is_virtual: classify_virtual_interface(&name),
            interface_name: name,
        })
        .collect();
    // INVARIANTE: la clave de sort debe cubrir TODOS los campos del struct,
    // porque Vec::dedup solo elimina duplicados CONSECUTIVOS. Si se agrega un
    // campo a WebServerInterfaceInfo sin agregarlo aca, el dedup deja de
    // funcionar en silencio (dev-rust).
    out.sort_by(|a, b| {
        (a.is_virtual, &a.interface_name, &a.address).cmp(&(
            b.is_virtual,
            &b.interface_name,
            &b.address,
        ))
    });
    out.dedup();
    out
}

/// §1453: IPv4 actuales de la maquina con su adaptador, para el chooser de
/// bind del popover. Solo registrado en el invoke_handler de escritorio; el
/// bridge WS del browser no rutea ningun comando de web server (D3).
/// async no por consistencia (este archivo tiene comandos sync), sino para no
/// ocupar el main thread: Tauri v2 corre los comandos sync ahi.
/// Lista adaptadores caidos con IPv4 estatica: deliberado (D3/D6).
/// get_if_addrs() puede entrar en panico si falla HeapAlloc (camino de OOM).
#[tauri::command]
pub async fn list_web_server_interfaces() -> Result<Vec<WebServerInterfaceInfo>, String> {
    let interfaces = if_addrs::get_if_addrs().map_err(|e| {
        log::warn!("[web-server] interface enumeration failed: {}", e);
        format!("Failed to enumerate network interfaces: {}", e)
    })?;
    Ok(map_web_server_interfaces(
        interfaces
            .into_iter()
            .filter_map(|iface| match iface.addr {
                if_addrs::IfAddr::V4(v4) => Some((iface.name, v4.ip)),
                if_addrs::IfAddr::V6(_) => None,
            })
            .collect(),
    ))
}

pub(crate) fn api_server_probe_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let configured = parse_api_server_socket_addr(bind, port)?;
    let probe_ip = match configured.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(probe_ip, configured.port()))
}

pub(crate) fn web_server_probe_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let configured = format!("{}:{}", bind, port)
        .parse::<SocketAddr>()
        .map_err(|error| error.to_string())?;
    let probe_ip = match configured.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Ok(SocketAddr::new(probe_ip, configured.port()))
}

pub(crate) async fn resolve_web_server_owned_status<F, Fut>(
    ws_handle: &WebServerHandle,
    configured_fallback: (String, u16),
    mut probe: F,
) -> WebServerOwnedStatus
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = bool>,
{
    loop {
        let snapshot = ws_handle.snapshot();
        let (bind, port) = match (&snapshot.lifecycle, snapshot.endpoint.as_ref()) {
            (WebServerLifecycle::Stopped, _) => configured_fallback.clone(),
            (_, Some(endpoint)) => endpoint.clone(),
            (WebServerLifecycle::Starting | WebServerLifecycle::Stopping, None) => {
                configured_fallback.clone()
            }
            (WebServerLifecycle::Running, None) => configured_fallback.clone(),
        };
        let listening = match web_server_probe_addr(&bind, port) {
            Ok(addr) => probe(addr).await,
            Err(_) => false,
        };
        if !ws_handle.snapshot_is_current(&snapshot) {
            continue;
        }

        let bind_failure =
            select_current_bind_failure(ws_handle.last_bind_failure(), &bind, port, listening);
        return build_web_server_owned_status(
            bind,
            port,
            snapshot.lifecycle,
            listening,
            bind_failure,
        );
    }
}

pub(crate) async fn is_tcp_socket_listening(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(WEB_STATUS_CONNECT_TIMEOUT_MS),
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

pub(crate) async fn is_tcp_listening(addr: SocketAddr) -> bool {
    is_tcp_socket_listening(addr).await
}

impl crate::api::ApiSendSettings for tauri::AppHandle {
    fn api_send_settings(&self) -> SettingsState {
        Arc::clone(self.state::<SettingsState>().inner())
    }
}

impl crate::api::ApiRequestResources for tauri::AppHandle {
    fn required_settings(&self) -> SettingsState {
        Arc::clone(self.state::<SettingsState>().inner())
    }

    fn try_settings(&self) -> Option<SettingsState> {
        self.try_state::<SettingsState>()
            .map(|state| Arc::clone(state.inner()))
    }

    fn snapshot_state(&self) -> Option<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>> {
        self.try_state::<Arc<crate::pty::terminal_snapshot::TerminalSnapshotState>>()
            .map(|state| Arc::clone(state.inner()))
    }

    fn restore_flag(&self) -> Option<Arc<crate::app_state::RestoreInProgress>> {
        self.try_state::<Arc<crate::app_state::RestoreInProgress>>()
            .map(|state| Arc::clone(state.inner()))
    }

    fn purge_guard(&self) -> Option<Arc<crate::session::purge_guard::PurgeGuard>> {
        self.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .map(|state| Arc::clone(state.inner()))
    }
}

#[cfg(test)]
impl crate::api::ApiState {
    pub(crate) fn resources_from_native(
        app: tauri::AppHandle,
    ) -> Arc<dyn crate::api::ApiRequestResources> {
        Arc::new(app)
    }
}

impl crate::api::ApiMessageWarnings for tauri::AppHandle {
    fn message_warnings(&self) -> Arc<crate::config::injected_messages::MessageWarnings> {
        Arc::clone(&self.state::<crate::app_state::AppOwners>().messages)
    }
}

impl crate::api::ApiJoinTask for tauri::async_runtime::JoinHandle<()> {
    type Error = tauri::Error;

    fn abort(&self) {
        tauri::async_runtime::JoinHandle::abort(self);
    }
}

impl crate::api::NativeApiSpawner for tauri::AppHandle {
    type Join = tauri::async_runtime::JoinHandle<()>;

    fn spawn_owned(
        &self,
        future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
    ) -> Self::Join {
        tauri::async_runtime::spawn(future)
    }
}

impl crate::api::ApiServerFactory for tauri::AppHandle {
    type Error = tauri::Error;
    type Join = tauri::async_runtime::JoinHandle<()>;

    fn start_api_owned(
        self,
        bind: String,
        port: u16,
        sessions: Arc<tokio::sync::RwLock<SessionManager>>,
        pty: Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> crate::api::ApiServerStart<Self::Join> {
        crate::api::start_server_core(bind, port, self, sessions, pty, shutdown)
    }
}

impl crate::api::dispatcher::DispatcherDelivery for tauri::AppHandle {
    fn purging(&self) -> bool {
        self.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
            .map(|guard| guard.is_active())
            .unwrap_or(false)
    }

    fn message_store_state(&self) -> Option<crate::api::message_store::MessageStoreState> {
        self.try_state::<crate::api::message_store::MessageStoreState>()
            .map(|state| state.inner().clone())
    }

    fn restoring(&self) -> bool {
        self.try_state::<Arc<crate::RestoreInProgress>>()
            .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn deliver_wake<'a>(
        &'a self,
        msg: crate::phone::types::OutboxMessage,
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            crate::phone::mailbox::MailboxPoller::new(messages)
                .deliver_wake_with_origin(
                    self,
                    &msg,
                    crate::phone::mailbox::WakeDeliveryOrigin::DbQueue,
                )
                .await
                .map(|_| ())
        })
    }

    fn dispatch_pty<'a>(
        &'a self,
        state: &'a crate::api::message_store::MessageStoreState,
        id: &'a str,
        client: &'a Arc<crate::api::auth::ApiClientStore>,
        messages: Arc<crate::config::injected_messages::MessageWarnings>,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            crate::phone::mailbox::MailboxPoller::new(messages)
                .dispatch_pty_input_operation(
                    self,
                    state,
                    id,
                    crate::phone::types::PtyInputSourcePlane::ContainerApi,
                    Some(client),
                )
                .await;
        })
    }
}
