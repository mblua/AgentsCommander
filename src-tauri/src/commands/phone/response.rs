use crate::phone::mailbox::response_observation::{
    ResponseAdmissionState, ResponseCompletionState,
};
use crate::pty::manager::ResponseCloseState;
use tauri::Manager;

impl<R: tauri::Runtime> ResponseAdmissionState for tauri::AppHandle<R> {
    fn response_tracker(&self) -> Option<&ResponseCloseState> {
        self.try_state::<ResponseCloseState>()
            .map(|state| state.inner())
    }
}

impl<R: tauri::Runtime> ResponseCompletionState for tauri::AppHandle<R> {
    fn response_selection(&self) -> Option<&crate::session::selection::SelectionCoordinator> {
        self.try_state::<crate::session::selection::SelectionCoordinator>()
            .map(|state| state.inner())
    }
}

#[cfg(test)]
mod tests;
