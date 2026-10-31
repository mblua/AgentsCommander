use super::*;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::Listener;
use tauri::Manager;
use tokio::sync::mpsc::error::TryRecvError;

use crate::config::settings::{new_settings_state, AppSettings};
use crate::session::manager::SessionManager;
use crate::telegram::bridge::TelegramRuntime;
use crate::web::broadcast::WsBroadcaster;

#[test]
fn bridge_error_matches_legacy_payload_and_synchronous_desktop_delivery() {
    let broadcaster = WsBroadcaster::new();
    let mut websocket = broadcaster.subscribe();
    let app = tauri::test::mock_builder()
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let received = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&received);
    app.listen_any("telegram_bridge_error", move |event| {
        let payload: Value = serde_json::from_str(event.payload()).expect("event JSON");
        observed.lock().unwrap().push(payload);
    });
    let error = "falló ñ: https://api.telegram.org/bot***/sendMessage";
    for kind in [Some("network"), None] {
        let expected = match kind {
            Some(_) => json!({
                "sessionId": "sesión-ñ", "error": error, "kind": "network"
            }),
            None => json!({ "sessionId": "sesión-ñ", "error": error }),
        };
        received.lock().unwrap().clear();
        app.handle()
            .emit("telegram_bridge_error", expected.clone())
            .expect("legacy emit");
        assert_eq!(*received.lock().unwrap(), vec![expected.clone()]);
        received.lock().unwrap().clear();

        app.handle().bridge_error("sesión-ñ", error, kind);

        // Delivery finishes before the port returns, exactly once.
        assert_eq!(*received.lock().unwrap(), vec![expected]);
        assert!(matches!(websocket.try_recv(), Err(TryRecvError::Empty)));
    }
}

#[test]
fn incoming_and_prompt_match_legacy_payloads_and_synchronous_order() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let received = Arc::new(Mutex::new(Vec::new()));
    for name in ["telegram_incoming", "last_prompt"] {
        let observed = Arc::clone(&received);
        app.listen_any(name, move |event| {
            let payload: Value = serde_json::from_str(event.payload()).unwrap();
            observed.lock().unwrap().push((name, payload));
        });
    }
    let incoming = json!({ "sessionId": "sesión-ñ", "text": "hola 🦀", "from": "María" });
    let prompt = json!({ "text": "[TG] hola 🦀", "sessionId": "sesión-ñ" });
    let expected = vec![
        ("telegram_incoming", incoming.clone()),
        ("last_prompt", prompt.clone()),
    ];
    app.handle().emit("telegram_incoming", incoming).unwrap();
    app.handle().emit("last_prompt", prompt).unwrap();
    assert_eq!(*received.lock().unwrap(), expected);
    received.lock().unwrap().clear();

    app.handle().incoming("sesión-ñ", "hola 🦀", "María");
    assert_eq!(received.lock().unwrap().len(), 1);
    app.handle().last_prompt("sesión-ñ", "[TG] hola 🦀");
    assert_eq!(*received.lock().unwrap(), expected);
}

#[tokio::test]
async fn runtime_settings_are_read_when_polled_and_voice_values_stay_fresh() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let handle = app.handle();
    // Neither creating the handle nor constructing these futures may read state.
    let policy_future = handle.poll_error_policy();
    let voice_future = handle.voice_settings();
    let state = new_settings_state(AppSettings::default());
    assert!(app.manage(Arc::clone(&state)));
    {
        let mut cfg = state.write().await;
        cfg.gemini_api_key = "synthetic-first".into();
        cfg.gemini_model = String::new();
        cfg.telegram_network_poll_error_logging
            .sustained_after_seconds = 17;
    }
    let policy = policy_future.await;
    assert_eq!(
        voice_future.await,
        ("synthetic-first".into(), String::new())
    );
    let next_voice = handle.voice_settings();
    {
        let mut cfg = state.write().await;
        cfg.gemini_api_key = "synthetic-second".into();
        cfg.gemini_model = "synthetic-model".into();
        cfg.telegram_network_poll_error_logging
            .sustained_after_seconds = 29;
    }
    assert_eq!(
        next_voice.await,
        ("synthetic-second".into(), "synthetic-model".into())
    );
    assert_eq!(
        policy.sustained_after_seconds, 17,
        "returned policy is a clone"
    );
    assert_eq!(handle.poll_error_policy().await.sustained_after_seconds, 29);
}

#[tokio::test]
async fn runtime_prompt_finds_state_registered_during_incoming_and_releases_guard() {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build app");
    let handle = app.handle();
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let id = manager
        .read()
        .await
        .create_session(
            "shell".into(),
            Vec::new(),
            "C:/synthetic".into(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
        .id;
    let session = id.to_string();
    let prompt_future = handle.set_last_prompt(&session, "[TG] hola ñ");
    let register_handle = handle.clone();
    let register_manager = Arc::clone(&manager);
    app.listen_any("telegram_incoming", move |_| {
        assert!(register_handle.manage(Arc::clone(&register_manager)));
    });
    let final_calls = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&final_calls);
    let final_manager = Arc::clone(&manager);
    app.listen_any("last_prompt", move |event| {
        let _guard = final_manager
            .try_write()
            .expect("adapter must release manager guard");
        observed
            .lock()
            .unwrap()
            .push(serde_json::from_str::<Value>(event.payload()).unwrap());
    });

    handle.incoming(&session, "hola ñ", "María");
    prompt_future.await;
    handle.last_prompt(&session, "[TG] hola ñ");
    assert_eq!(
        manager
            .read()
            .await
            .get_session(id)
            .await
            .unwrap()
            .last_prompt,
        Some("[TG] hola ñ".into())
    );

    handle.set_last_prompt("inválido-ñ", "[TG] ignored").await;
    handle.last_prompt("inválido-ñ", "[TG] ignored");
    assert_eq!(
        manager
            .read()
            .await
            .get_session(id)
            .await
            .unwrap()
            .last_prompt,
        Some("[TG] hola ñ".into())
    );
    assert_eq!(
        *final_calls.lock().unwrap(),
        vec![
            json!({ "sessionId": session, "text": "[TG] hola ñ" }),
            json!({ "sessionId": "inválido-ñ", "text": "[TG] ignored" }),
        ]
    );
}
