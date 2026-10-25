use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::warnings::{
    SessionWarning, SessionWarningEvents, SessionWarningState, SESSION_ENV_WARNING_EVENT,
};

impl<R: Runtime> SessionWarningEvents for AppHandle<R> {
    fn session_warning(&self, warning: SessionWarning) {
        if let Some(state) = self.try_state::<SessionWarningState>() {
            match state.lock() {
                Ok(mut buffer) => buffer.push(warning.clone()),
                Err(err) => {
                    log::warn!(
                        target: "agentscommander_lib::session::warnings",
                        "[session-warning] warning buffer lock poisoned; live emit only: {}",
                        err
                    );
                }
            }
        } else {
            log::warn!(
                target: "agentscommander_lib::session::warnings",
                "[session-warning] warning buffer state missing; live emit only"
            );
        }
        let _ = self.emit(SESSION_ENV_WARNING_EVENT, warning);
    }
}

#[cfg(test)]
mod tests;
