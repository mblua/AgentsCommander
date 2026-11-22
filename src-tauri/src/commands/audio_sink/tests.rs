use super::*;
fn invoke_with_counter(cmd: &str) -> (Result<serde_json::Value, String>, usize) {
    let (response, calls) = crate::test_support::invoke_counting_fallback(
        tauri::test::mock_builder(),
        |invoke, fallback| dispatch(invoke, &fallback),
        cmd,
        serde_json::json!({}),
    );
    let result = response
        .map(|b| b.deserialize::<serde_json::Value>().expect("json"))
        .map_err(|e| e.to_string());
    (result, calls)
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

#[test]
fn command_passes_the_environment_appimage_flag() {
    let source = include_str!("../audio_sink.rs");
    let call = "crate::process::audio_sink_status(crate::config::environment::in_appimage()).await";
    assert_eq!(
        source.matches(call).count(),
        1,
        "command forwards in_appimage()"
    );
}
