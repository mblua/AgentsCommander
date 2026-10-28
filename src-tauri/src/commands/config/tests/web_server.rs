use super::*;

#[test]
fn web_remote_url_maps_wildcards_and_preserves_concrete_ips() {
    let cases = [
        (
            "0.0.0.0",
            "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "::",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "0:0:0:0:0:0:0:0",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "0::0",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "127.0.0.1",
            "http://127.0.0.1:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "192.168.1.50",
            "http://192.168.1.50:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "::1",
            "http://[::1]:8888/?window=browser&remoteToken=test-token",
        ),
        (
            "2001:db8::25",
            "http://[2001:db8::25]:8888/?window=browser&remoteToken=test-token",
        ),
    ];

    for (bind, expected) in cases {
        assert_eq!(web_remote_url(bind, 8888, "test-token").unwrap(), expected);
    }
}

#[test]
fn web_remote_url_rejects_invalid_bind() {
    let error = web_remote_url("not-an-ip", 8888, "test-token").unwrap_err();

    assert!(error.starts_with("Invalid web server bind address 'not-an-ip':"));
}

#[test]
fn web_server_probe_addr_maps_wildcards_to_loopback() {
    assert_eq!(
        web_server_probe_addr("0.0.0.0", 8765).unwrap(),
        "127.0.0.1:8765".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("[::]", 8766).unwrap(),
        "[::1]:8766".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("192.168.1.50", 8767).unwrap(),
        "192.168.1.50:8767".parse().unwrap()
    );
    assert_eq!(
        web_server_probe_addr("[2001:db8::25]", 8768).unwrap(),
        "[2001:db8::25]:8768".parse().unwrap()
    );
}

#[tokio::test]
async fn web_server_probe_addr_detects_wildcard_listener_via_loopback() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .expect("bind wildcard web listener");
    let port = listener.local_addr().expect("wildcard address").port();
    let probe_addr = web_server_probe_addr("0.0.0.0", port).expect("web probe addr");

    assert_eq!(probe_addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(is_tcp_socket_listening(probe_addr).await);
}

#[test]
fn web_server_owned_status_maps_owned_running() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Running,
        false,
        None,
    );

    assert!(!status.listening, "listening comes from the probe");
    assert!(status.owned);
    assert!(!status.external_listening);
    assert!(status.open_allowed);
    assert_eq!(status.bind, "127.0.0.1");
    assert_eq!(status.port, 8765);
    assert_eq!(status.state, WebServerOwnershipState::OwnedRunning);
}

#[test]
fn web_server_owned_status_maps_external_listener() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Stopped,
        true,
        None,
    );

    assert!(status.listening);
    assert!(!status.owned);
    assert!(status.external_listening);
    assert!(!status.open_allowed);
    assert_eq!(status.state, WebServerOwnershipState::ExternalListening);
}

#[test]
fn web_server_owned_status_maps_stopped() {
    let status = build_web_server_owned_status(
        "127.0.0.1".to_string(),
        8765,
        WebServerLifecycle::Stopped,
        false,
        None,
    );

    assert!(!status.listening);
    assert!(!status.owned);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
}

#[test]
fn web_server_owned_status_maps_starting_and_stopping() {
    for (lifecycle, state, listening) in [
        (
            WebServerLifecycle::Starting,
            WebServerOwnershipState::Starting,
            true,
        ),
        (
            WebServerLifecycle::Stopping,
            WebServerOwnershipState::Stopping,
            false,
        ),
    ] {
        let status =
            build_web_server_owned_status("0.0.0.0".to_string(), 8765, lifecycle, listening, None);
        assert_eq!(status.state, state);
        assert_eq!(status.listening, listening);
        assert!(status.owned);
        assert!(!status.external_listening);
        assert!(!status.open_allowed);
    }
}

// §1453 D2: el fallo viaja en el status solo si describe el target actual.
#[test]
fn web_server_owned_status_carries_matching_bind_failure() {
    let recorded = Some(crate::web::StartServerError::BindFailed {
        bind: "192.168.1.12".to_string(),
        addr: "192.168.1.12:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    });
    let failure = select_current_bind_failure(recorded, "192.168.1.12", 8888, false)
        .expect("matching failure must survive");
    assert_eq!(failure.bind, "192.168.1.12");
    assert_eq!(failure.port, 8888);
    assert_eq!(failure.detail, "os error 10049");

    let status = build_web_server_owned_status(
        "192.168.1.12".to_string(),
        8888,
        WebServerLifecycle::Stopped,
        false,
        Some(failure),
    );
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
    assert!(status.bind_failure.is_some());
}

// §1453 D2: un fallo de un target anterior (o sobre algo que SI escucha)
// no pinta el target nuevo. El caso IPv6 bracketed es el fix D-2: con el
// bind crudo guardado, matchea; con la forma canonica se perdia siempre.
#[test]
fn web_server_owned_status_drops_stale_bind_failure() {
    let recorded = || {
        Some(crate::web::StartServerError::BindFailed {
            bind: "192.168.1.12".to_string(),
            addr: "192.168.1.12:8888".parse().unwrap(),
            detail: "os error 10049".to_string(),
        })
    };
    assert!(select_current_bind_failure(recorded(), "192.168.1.12", 9999, false).is_none());
    assert!(select_current_bind_failure(recorded(), "0.0.0.0", 8888, false).is_none());
    assert!(select_current_bind_failure(None, "192.168.1.12", 8888, false).is_none());
    // guardia de listening (D2, higiene)
    assert!(select_current_bind_failure(recorded(), "192.168.1.12", 8888, true).is_none());
    // IPv6 bracketed: el crudo matchea contra el settings crudo
    let v6 = Some(crate::web::StartServerError::BindFailed {
        bind: "[::1]".to_string(),
        addr: "[::1]:8888".parse().unwrap(),
        detail: "os error 10049".to_string(),
    });
    assert!(select_current_bind_failure(v6, "[::1]", 8888, false).is_some());
}

// §1453 D3: heuristica de clasificacion sobre nombres reales de ambos mundos.
#[test]
fn classify_virtual_interface_names() {
    for physical in [
        "Ethernet",
        "Wi-Fi",
        "Local Area Connection",
        "eth0",
        "enp3s0",
        "en0",
        "wlan0",
    ] {
        assert!(
            !classify_virtual_interface(physical),
            "{physical} must be physical"
        );
    }
    for virtual_name in [
        // el nombre REAL de esta maquina, con parentesis anidados (2.4)
        "vEthernet (WSL (Hyper-V firewall))",
        "vEthernet (WSL)",
        "vEthernet (Default Switch)",
        "Tailscale",
        "VirtualBox Host-Only Network",
        "Docker0",
        "docker0",
        "tailscale0",
        "tun0",
        "utun3",
        "wg0",
        "br-1a2b3c",
        "veth42",
        "virbr0",
        "zt0",
        "WireGuard Tunnel",
    ] {
        assert!(
            classify_virtual_interface(virtual_name),
            "{virtual_name} must be virtual"
        );
    }
}

// §1453 D1: filtro (loopback, APIPA, unspecified), orden y dedup.
#[test]
fn map_web_server_interfaces_filters_and_sorts() {
    let mapped = map_web_server_interfaces(vec![
        ("Tailscale".to_string(), "100.121.138.61".parse().unwrap()),
        ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
        ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
        (
            "Loopback Pseudo-Interface 1".to_string(),
            "127.0.0.1".parse().unwrap(),
        ),
        ("Wi-Fi".to_string(), "192.168.1.2".parse().unwrap()),
        ("Ethernet".to_string(), "192.168.1.9".parse().unwrap()),
    ]);
    let flat: Vec<(String, String, bool)> = mapped
        .into_iter()
        .map(|i| (i.interface_name, i.address, i.is_virtual))
        .collect();
    assert_eq!(
        flat,
        vec![
            ("Ethernet".to_string(), "192.168.1.9".to_string(), false),
            ("Wi-Fi".to_string(), "192.168.1.2".to_string(), false),
            ("Tailscale".to_string(), "100.121.138.61".to_string(), true),
        ]
    );
}

// §1453 D8: el backend PUEDE devolver [] legitimamente (maquina cuyas
// unicas direcciones son loopback y APIPA). Es la precondicion del memo
// hasDetection() del frontend: lista vacia = sin evidencia, no evidencia
// de ausencia.
#[test]
fn map_web_server_interfaces_returns_empty_when_all_filtered() {
    let mapped = map_web_server_interfaces(vec![
        (
            "Loopback Pseudo-Interface 1".to_string(),
            "127.0.0.1".parse().unwrap(),
        ),
        ("Ethernet".to_string(), "169.254.10.20".parse().unwrap()),
    ]);
    assert!(mapped.is_empty());
}

async fn wait_for_web_lifecycle(
    handle: &WebServerHandle,
    expected: WebServerLifecycle,
) -> crate::WebServerLifecycleSnapshot {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = handle.snapshot();
            if snapshot.lifecycle == expected {
                return snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("web lifecycle did not converge")
}

async fn start_running_web_handle(
    handle: &WebServerHandle,
    bind: &str,
    port: u16,
) -> Arc<crate::web::WebSocketAdmission> {
    let lifecycle = handle.clone();
    let bind = bind.to_string();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let waiter = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(generation, bind, port));
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            Ok::<_, String>(Some(crate::web_server::ServerTask::from_native(
                tauri::async_runtime::spawn(async move {
                    generation_token.cancelled().await;
                }),
            )))
        },
    );
    let admission = admission_receiver.await.expect("admission exposed");
    assert_eq!(waiter.wait().await, Ok(true));
    wait_for_web_lifecycle(handle, WebServerLifecycle::Running).await;
    admission
}

#[tokio::test]
async fn stop_web_server_reports_stopped_and_waits_for_active_terminal() {
    let stopped = WebServerHandle::default();
    assert_eq!(stop_web_server_handle(&stopped).await, Ok(false));

    let handle = WebServerHandle::default();
    let admission = start_running_web_handle(&handle, "127.0.0.1", 8751).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let handle_for_stop = handle.clone();
    let mut stop_task = tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });

    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut stop_task)
            .await
            .is_err(),
        "Stop cannot report true while admitted work is retained"
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    drop(retained);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), stop_task)
            .await
            .expect("active Stop did not reach terminal")
            .expect("active Stop task panicked"),
        Ok(true)
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopped);
}

#[tokio::test]
async fn stop_web_server_clears_bind_failure_published_by_late_bind() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let failure_recorder = handle.clone();
    let (output_reached, release_output) = handle.gate_next_start_output();
    let (bind_pending_sender, bind_pending_receiver) = oneshot::channel();
    let (release_bind_sender, release_bind_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, _admission, _generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                "127.0.0.1".to_string(),
                8753,
            ));
            bind_pending_sender
                .send(())
                .expect("report pending bind to the test");
            release_bind_receiver
                .await
                .expect("release late bind failure");
            failure_recorder.record_bind_failure(crate::web::StartServerError::BindFailed {
                bind: "127.0.0.1".to_string(),
                addr: SocketAddr::from(([127, 0, 0, 1], 8753)),
                detail: "late bind failure".to_string(),
            });
            Ok::<_, String>(None)
        },
    );
    bind_pending_receiver.await.expect("bind became pending");

    let handle_for_stop = handle.clone();
    let stop = tokio::spawn(async move { stop_web_server_handle(&handle_for_stop).await });
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopping).await;
    release_bind_sender
        .send(())
        .expect("release pending bind after Stop linealizes");
    assert!(
        !output_reached
            .await
            .expect("late bind output reaches the supervisor"),
        "Stop must already own the lifecycle transition"
    );
    assert!(
        handle.last_bind_failure().is_some(),
        "the late bind must really publish before terminal cleanup"
    );
    release_output
        .send(())
        .expect("release late bind result processing");

    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    assert_eq!(stop.await.expect("late-bind Stop task panicked"), Ok(true));
    let status =
        resolve_web_server_owned_status(&handle, ("127.0.0.1".to_string(), 8753), |_addr| async {
            false
        })
        .await;
    assert_eq!(status.state, WebServerOwnershipState::Stopped);
    assert!(status.bind_failure.is_none());
    assert!(handle.last_bind_failure().is_none());
}

#[tokio::test]
async fn stop_web_server_timeout_preserves_stopping_and_shared_deadline() {
    let handle = WebServerHandle::default();
    let admission = start_running_web_handle(&handle, "127.0.0.1", 8752).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let first = handle
        .begin_stop_with_timeout(Duration::from_millis(40))
        .expect("running generation is owned");
    drop(first);

    assert_eq!(
        stop_web_server_handle(&handle).await,
        Err("Timed out waiting for web server generation to stop".to_string())
    );
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);

    drop(retained);
    wait_for_web_lifecycle(&handle, WebServerLifecycle::Stopped).await;
}

#[tokio::test]
async fn ensure_web_remote_open_allowed_rejects_enabled_settings_without_owned_handle() {
    let settings = AppSettings {
        web_server_enabled: true,
        web_server_bind: "127.0.0.1".to_string(),
        web_server_port: 8765,
        ..AppSettings::default()
    };
    let handle = WebServerHandle::default();
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );

    let lifecycle = handle.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, _admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                "127.0.0.1".to_string(),
                8765,
            ));
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            if generation_token.is_cancelled() {
                Err(WEB_SERVER_START_CANCELLED.to_string())
            } else {
                Ok(None)
            }
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Starting);
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    let stop = handle.begin_stop().expect("starting generation is owned");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");

    let mismatched = WebServerHandle::default();
    start_running_web_handle(&mismatched, "127.0.0.1", 8766).await;
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &mismatched).unwrap_err(),
        "Web server is not owned by this app process"
    );
    mismatched
        .begin_stop()
        .expect("mismatched running generation is owned")
        .wait()
        .await
        .expect("mismatched generation drains");
}

#[tokio::test]
async fn ensure_web_remote_open_allowed_accepts_owned_running_handle() {
    let settings = AppSettings {
        web_server_enabled: true,
        web_server_bind: "127.0.0.1".to_string(),
        web_server_port: 8765,
        ..AppSettings::default()
    };
    let handle = WebServerHandle::default();
    start_running_web_handle(&handle, "127.0.0.1", 8765).await;

    let authorized = ensure_web_remote_open_allowed(&settings, &handle)
        .expect("matching Running generation is authorized");
    assert_eq!(authorized.lifecycle, WebServerLifecycle::Running);
    assert_eq!(authorized.endpoint, Some(("127.0.0.1".to_string(), 8765)));
    assert!(handle.snapshot_is_current(&authorized));

    let stop = handle.begin_stop().expect("running generation is owned");
    assert!(!handle.snapshot_is_current(&authorized));
    assert_eq!(
        ensure_web_remote_open_allowed(&settings, &handle).unwrap_err(),
        "Web server is not owned by this app process"
    );
    stop.wait().await.expect("authorized generation drains");
}

#[tokio::test]
async fn web_server_owned_status_retries_when_starting_revision_changes() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let endpoint_a = ("127.0.0.1".to_string(), 8811);
    let endpoint_b = ("127.0.0.1".to_string(), 8812);
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let endpoint_a_for_factory = endpoint_a.clone();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, _admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(
                generation,
                endpoint_a_for_factory.0,
                endpoint_a_for_factory.1,
            ));
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            if generation_token.is_cancelled() {
                Err(WEB_SERVER_START_CANCELLED.to_string())
            } else {
                Ok(None)
            }
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let before = handle.snapshot();
    assert_eq!(before.lifecycle, WebServerLifecycle::Starting);
    assert_eq!(before.endpoint, Some(endpoint_a.clone()));
    let generation = before.generation.expect("starting generation id");

    let (probe_a_sender, probe_a_receiver) = oneshot::channel();
    let (probe_b_sender, probe_b_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        probe_a_receiver,
        probe_b_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(
            &status_handle,
            ("127.0.0.1".to_string(), 8899),
            move |addr| {
                calls_sender.send(addr).expect("record status probe");
                let reply = replies_for_probe
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("configured probe response");
                async move { reply.await.expect("release status probe") }
            },
        )
        .await
    });
    assert_eq!(
        calls_receiver.recv().await.expect("first probe call"),
        web_server_probe_addr(&endpoint_a.0, endpoint_a.1).unwrap()
    );
    assert!(handle.publish_effective_endpoint(generation, endpoint_b.0.clone(), endpoint_b.1,));
    let after_publish = handle.snapshot();
    assert_eq!(after_publish.generation, before.generation);
    assert_eq!(after_publish.lifecycle, WebServerLifecycle::Starting);
    assert_eq!(after_publish.revision, before.revision + 1);
    assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
    assert!(probe_a_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("second probe call"),
        web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
    );
    assert!(probe_b_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.bind, endpoint_b.0);
    assert_eq!(status.port, endpoint_b.1);
    assert!(!status.listening);
    assert_eq!(status.state, WebServerOwnershipState::Starting);
    assert!(status.owned);
    assert!(!status.open_allowed);

    let stop = handle.begin_stop().expect("starting generation is owned");
    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("starting generation drains");
}

#[tokio::test]
async fn web_server_owned_status_retries_when_phase_changes_during_probe() {
    let handle = WebServerHandle::default();
    let endpoint = ("127.0.0.1".to_string(), 8813);
    let admission = start_running_web_handle(&handle, &endpoint.0, endpoint.1).await;
    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let (running_probe_sender, running_probe_receiver) = oneshot::channel();
    let (stopping_probe_sender, stopping_probe_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        running_probe_receiver,
        stopping_probe_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(
            &status_handle,
            ("127.0.0.1".to_string(), 8899),
            move |addr| {
                calls_sender.send(addr).expect("record status probe");
                let reply = replies_for_probe
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("configured probe response");
                async move { reply.await.expect("release status probe") }
            },
        )
        .await
    });

    let expected_probe = web_server_probe_addr(&endpoint.0, endpoint.1).unwrap();
    assert_eq!(
        calls_receiver.recv().await.expect("Running probe call"),
        expected_probe
    );
    let stop = handle.begin_stop().expect("running generation is owned");
    assert_eq!(handle.snapshot().lifecycle, WebServerLifecycle::Stopping);
    assert!(running_probe_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("Stopping probe call"),
        expected_probe
    );
    assert!(stopping_probe_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.state, WebServerOwnershipState::Stopping);
    assert!(status.owned);
    assert!(!status.listening);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);

    drop(retained);
    stop.wait().await.expect("stopping generation drains");
}

#[tokio::test]
async fn web_server_owned_status_stopping_before_endpoint_uses_configured_fallback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind configured fallback listener");
    let fallback = listener.local_addr().expect("fallback listener address");
    let handle = WebServerHandle::default();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |_generation, _admission, _generation_token| async move {
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            Err(WEB_SERVER_START_CANCELLED.to_string())
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let stop = handle.begin_stop().expect("starting generation is owned");
    let stopping = handle.snapshot();
    assert_eq!(stopping.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(stopping.endpoint, None);
    let configured = (fallback.ip().to_string(), fallback.port());

    let listening =
        resolve_web_server_owned_status(&handle, configured.clone(), super::is_tcp_listening).await;
    assert_eq!(listening.bind, configured.0);
    assert_eq!(listening.port, configured.1);
    assert!(listening.listening);
    assert_eq!(listening.state, WebServerOwnershipState::Stopping);
    assert!(listening.owned);
    assert!(!listening.external_listening);
    assert!(!listening.open_allowed);

    let failed_probe =
        resolve_web_server_owned_status(&handle, configured, |_addr| async { false }).await;
    assert!(!failed_probe.listening);
    assert_eq!(failed_probe.state, WebServerOwnershipState::Stopping);
    assert!(failed_probe.owned);
    assert!(!failed_probe.external_listening);
    assert!(!failed_probe.open_allowed);

    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");
}

#[tokio::test]
async fn web_server_owned_status_stopping_retries_when_endpoint_appears_during_fallback_probe() {
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let fallback_a = ("127.0.0.1".to_string(), 8821);
    let endpoint_b = ("127.0.0.1".to_string(), 8822);
    let endpoint_b_for_factory = endpoint_b.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (publish_sender, publish_receiver) = oneshot::channel();
    let (published_sender, published_receiver) = oneshot::channel();
    let (finish_sender, finish_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, _admission, _generation_token| async move {
            assert!(factory_entered_sender.send(()).is_ok());
            publish_receiver
                .await
                .expect("release endpoint publication");
            assert!(
                !lifecycle.publish_effective_endpoint(
                    generation,
                    endpoint_b_for_factory.0,
                    endpoint_b_for_factory.1,
                ),
                "Stopping publication updates status but cannot continue start"
            );
            assert!(published_sender.send(()).is_ok());
            finish_receiver.await.expect("release factory terminal");
            Err(WEB_SERVER_START_CANCELLED.to_string())
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let stop = handle.begin_stop().expect("starting generation is owned");
    let before = handle.snapshot();
    assert_eq!(before.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(before.endpoint, None);

    let (probe_a_sender, probe_a_receiver) = oneshot::channel();
    let (probe_b_sender, probe_b_receiver) = oneshot::channel();
    let replies = Arc::new(Mutex::new(VecDeque::from([
        probe_a_receiver,
        probe_b_receiver,
    ])));
    let (calls_sender, mut calls_receiver) = mpsc::unbounded_channel();
    let status_handle = handle.clone();
    let replies_for_probe = Arc::clone(&replies);
    let fallback_for_status = fallback_a.clone();
    let status_task = tokio::spawn(async move {
        resolve_web_server_owned_status(&status_handle, fallback_for_status, move |addr| {
            calls_sender.send(addr).expect("record status probe");
            let reply = replies_for_probe
                .lock()
                .unwrap()
                .pop_front()
                .expect("configured probe response");
            async move { reply.await.expect("release status probe") }
        })
        .await
    });
    assert_eq!(
        calls_receiver.recv().await.expect("fallback probe call"),
        web_server_probe_addr(&fallback_a.0, fallback_a.1).unwrap()
    );
    assert!(publish_sender.send(()).is_ok());
    published_receiver.await.expect("endpoint published");
    let after_publish = handle.snapshot();
    assert_eq!(after_publish.generation, before.generation);
    assert_eq!(after_publish.lifecycle, WebServerLifecycle::Stopping);
    assert_eq!(after_publish.revision, before.revision + 1);
    assert_eq!(after_publish.endpoint, Some(endpoint_b.clone()));
    assert!(probe_a_sender.send(true).is_ok());
    assert_eq!(
        calls_receiver.recv().await.expect("effective probe call"),
        web_server_probe_addr(&endpoint_b.0, endpoint_b.1).unwrap()
    );
    assert!(probe_b_sender.send(false).is_ok());
    let status = status_task.await.expect("status task panicked");
    assert_eq!(status.bind, endpoint_b.0);
    assert_eq!(status.port, endpoint_b.1);
    assert!(!status.listening);
    assert_eq!(status.state, WebServerOwnershipState::Stopping);
    assert!(status.owned);
    assert!(!status.external_listening);
    assert!(!status.open_allowed);

    assert!(finish_sender.send(()).is_ok());
    assert_eq!(
        start.wait().await,
        Err(WEB_SERVER_START_CANCELLED.to_string())
    );
    stop.wait().await.expect("stopping generation drains");
}

async fn assert_wildcard_owned_phase_probe(
    handle: &WebServerHandle,
    expected_state: WebServerOwnershipState,
    probe_result: bool,
    port: u16,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let calls_for_probe = Arc::clone(&calls);
    let status =
        resolve_web_server_owned_status(handle, ("0.0.0.0".to_string(), port), move |addr| {
            calls_for_probe.lock().unwrap().push(addr);
            async move { probe_result }
        })
        .await;
    assert_eq!(
        *calls.lock().unwrap(),
        vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
    );
    assert_eq!(status.state, expected_state);
    assert_eq!(status.listening, probe_result);
    assert!(status.owned);
    assert!(!status.external_listening);
    assert_eq!(
        status.open_allowed,
        expected_state == WebServerOwnershipState::OwnedRunning
    );
    assert_eq!(status.bind, "0.0.0.0");
    assert_eq!(status.port, port);
}

#[tokio::test]
async fn web_server_owned_status_wildcard_uses_probe_in_all_owned_phases() {
    let port = 8831;
    let handle = WebServerHandle::default();
    let lifecycle = handle.clone();
    let (factory_entered_sender, factory_entered_receiver) = oneshot::channel();
    let (factory_release_sender, factory_release_receiver) = oneshot::channel();
    let (admission_sender, admission_receiver) = oneshot::channel();
    let start = handle.begin_start(
        crate::shutdown::ShutdownSignal::new(),
        |future| drop(tauri::async_runtime::spawn(future)),
        move |generation, admission, generation_token| async move {
            assert!(lifecycle.publish_effective_endpoint(generation, "0.0.0.0".to_string(), port,));
            assert!(admission_sender.send(Arc::clone(&admission)).is_ok());
            assert!(factory_entered_sender.send(()).is_ok());
            factory_release_receiver.await.expect("release factory");
            Ok::<_, String>(Some(crate::web_server::ServerTask::from_native(
                tauri::async_runtime::spawn(async move {
                    generation_token.cancelled().await;
                }),
            )))
        },
    );
    factory_entered_receiver.await.expect("factory entered");
    let admission = admission_receiver.await.expect("admission exposed");
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(&handle, WebServerOwnershipState::Starting, result, port)
            .await;
    }

    assert!(factory_release_sender.send(()).is_ok());
    assert_eq!(start.wait().await, Ok(true));
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(
            &handle,
            WebServerOwnershipState::OwnedRunning,
            result,
            port,
        )
        .await;
    }

    let retained = admission
        .try_acquire()
        .expect("running generation admits retained work");
    let stop = handle.begin_stop().expect("running generation is owned");
    for result in [true, false] {
        assert_wildcard_owned_phase_probe(&handle, WebServerOwnershipState::Stopping, result, port)
            .await;
    }
    drop(retained);
    stop.wait().await.expect("wildcard generation drains");
}
