//! Test-only construction helpers shared by this crate's unit tests and by the
//! integration tests under `src-tauri/tests/`.
//!
//! This module is `pub` rather than `#[cfg(test)]` on purpose: integration
//! tests are separate crates and cannot see a library's `#[cfg(test)]` items,
//! so a gated module could not serve them. It is `#[doc(hidden)]` and must not
//! be referenced from any production code path.

/// Origin Tauri treats as the local app for IPC ACL checks
/// (`http://tauri.localhost/` on Windows, `tauri://localhost/` elsewhere).
#[doc(hidden)]
pub const LOCAL_IPC_ORIGIN: &str = if cfg!(windows) {
    "http://tauri.localhost/"
} else {
    "tauri://localhost/"
};
type TestBuilder = tauri::Builder<tauri::Wry>;

#[doc(hidden)]
pub fn test_builder() -> TestBuilder {
    test_builder_with_owners(crate::app_state::AppOwners::new())
}

/// A `tauri::Builder` that tests can `build()` off the main thread wherever the
/// platform allows it.
///
/// `Builder::any_thread` is declared `#[cfg(any(windows, target_os = "linux"))]`
/// in Tauri (`tauri-2.10.3/src/app.rs:1503`): macOS requires the event loop on
/// the main thread, so the method is not exposed there. Tests that only need a
/// built `App` still compile on macOS; only the off-main-thread relaxation is
/// unavailable.
#[cfg(any(windows, target_os = "linux"))]
#[doc(hidden)]
pub fn test_builder_with_owners(owners: crate::app_state::AppOwners) -> TestBuilder {
    crate::commands::session::selection_executor::install(
        tauri::Builder::default().any_thread().manage(owners),
    )
}

/// macOS counterpart. See the note above.
#[cfg(not(any(windows, target_os = "linux")))]
#[doc(hidden)]
pub fn test_builder_with_owners(owners: crate::app_state::AppOwners) -> TestBuilder {
    crate::commands::session::selection_executor::install(tauri::Builder::default().manage(owners))
}

/// Invoke type of the Tauri mock runtime (unit tests only).
#[cfg(test)]
pub(crate) type MockInvoke = tauri::ipc::Invoke<tauri::test::MockRuntime>;

/// Build a mock app from `builder`, open the `main` webview and send one IPC
/// request `cmd` with JSON `body` from the local origin (unit tests only).
#[cfg(test)]
pub(crate) fn mock_ipc_response(
    builder: tauri::Builder<tauri::test::MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
    let app = builder
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    tauri::test::get_ipc_response(
        &webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: LOCAL_IPC_ORIGIN.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
}

/// Send `cmd` through `route`, whose fallback counts its calls and leaves the
/// invoke unhandled. Returns the IPC response and the fallback count.
#[cfg(test)]
pub(crate) fn invoke_counting_fallback<D>(
    builder: tauri::Builder<tauri::test::MockRuntime>,
    route: D,
    cmd: &str,
    body: serde_json::Value,
) -> (
    Result<tauri::ipc::InvokeResponseBody, serde_json::Value>,
    usize,
)
where
    D: Fn(MockInvoke, &dyn Fn(MockInvoke) -> bool) -> bool + Send + Sync + 'static,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&calls);
    let fallback = move |_invoke: MockInvoke| {
        counted.fetch_add(1, Ordering::SeqCst);
        false
    };
    let builder = builder.invoke_handler(move |invoke| route(invoke, &fallback));
    let response = mock_ipc_response(builder, cmd, body);
    (response, calls.load(Ordering::SeqCst))
}

/// Routes registered in `invoke_handler`, in order and with their original
/// prefixes (`commands::` or the local `c::` alias). Unit tests only.
#[cfg(test)]
pub(crate) fn invoke_handler_routes(source: &str) -> Vec<&str> {
    let (_, body) = source
        .split_once("observer.observe_invokes(tauri::generate_handler![")
        .expect("lib.rs must open the invoke handler list");
    let (list, _) = body
        .split_once("])")
        .expect("lib.rs must close the invoke handler list");
    list.split(',')
        .map(str::trim)
        .filter(|route| !route.is_empty())
        .collect()
}
