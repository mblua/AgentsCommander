use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

type Mock = tauri::test::MockRuntime;

fn invoke_with_counter(cmd: &str) -> (Result<serde_json::Value, String>, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let fallback = move |_invoke: tauri::ipc::Invoke<Mock>| {
        counted.fetch_add(1, Ordering::SeqCst);
        false
    };
    let app = tauri::test::mock_builder()
        .invoke_handler(move |invoke| dispatch(invoke, &fallback))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    let response = tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: crate::test_support::LOCAL_IPC_ORIGIN.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(serde_json::json!({})),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    );
    let result = response
        .map(|b| b.deserialize::<serde_json::Value>().expect("json"))
        .map_err(|e| e.to_string());
    (result, calls.load(Ordering::SeqCst))
}

#[test]
fn get_audio_sink_status_is_handled_without_the_fallback() {
    let (result, calls) = invoke_with_counter("get_audio_sink_status");
    assert_eq!(calls, 0, "handled here, not by the fallback");
    let value = result.expect("status");
    let status = value.as_str().expect("string status");
    assert!(
        ["ok", "missing", "unknown"].contains(&status),
        "wire value: {status}"
    );
    #[cfg(not(target_os = "linux"))]
    assert_eq!(status, "ok");
}

#[test]
fn other_and_look_alike_names_reach_the_fallback_once() {
    for cmd in [
        "list_sessions",
        "get_audio_sink_statu",
        "Get_audio_sink_status",
        "get_audio_sink_status_x",
    ] {
        let (result, calls) = invoke_with_counter(cmd);
        assert!(result.is_err(), "{cmd} stays unhandled");
        assert_eq!(calls, 1, "{cmd} reaches the fallback once");
    }
}

#[test]
fn lib_routes_audio_sink_between_temp_rooms_and_history() {
    let lib = include_str!("../../lib.rs");
    let start = lib.find("fn observe_invokes").expect("observer");
    let observer = &lib[start..start + lib[start..].find("\n    }\n").expect("end")];
    let temp_rooms = observer
        .find("commands::temp_rooms::dispatch(")
        .expect("temp rooms dispatch");
    let audio_sink = observer
        .find("commands::audio_sink::dispatch(invoke, &|invoke| {")
        .expect("audio sink dispatch");
    let history = observer
        .find("commands::context_history::dispatch(")
        .expect("history dispatch");
    assert!(temp_rooms < audio_sink, "after temp rooms");
    assert!(audio_sink < history, "before history");
    assert_eq!(lib.matches("commands::audio_sink::dispatch(").count(), 1);
}
