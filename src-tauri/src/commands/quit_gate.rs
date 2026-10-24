use std::sync::Arc;

use crate::quit_gate::{
    apply_quit_gate_effects, quit_gate_run, QuitGate, QuitGateHost, QuitGateRegistration,
    QuitOutcome, TauriQuitHost,
};

#[tauri::command]
pub(crate) fn quit_gate_register(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) -> QuitGateRegistration {
    gate.register(window.label())
}

#[tauri::command]
pub(crate) fn quit_gate_unregister(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
) {
    let effects = gate.unregister(window.label());
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
pub(crate) fn quit_gate_resolve(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    consent: bool,
) {
    let effects = gate.resolve(window.label(), epoch, consent);
    apply_quit_gate_effects(&TauriQuitHost::new(app), &effects);
}

#[tauri::command]
pub(crate) fn quit_gate_progress(
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    epoch: u64,
    busy: bool,
) {
    gate.progress(window.label(), epoch, busy);
}

#[tauri::command]
pub(crate) async fn quit_application(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    gate: tauri::State<'_, Arc<QuitGate>>,
    force: bool,
    epoch: Option<u64>,
    attempt_id: Option<String>,
) -> Result<QuitOutcome, String> {
    let gate = Arc::clone(gate.inner());
    let host: Arc<dyn QuitGateHost> = Arc::new(TauriQuitHost::new(app));
    let label = window.label().to_string();
    quit_gate_run(gate, host, &label, force, epoch, attempt_id).await
}
