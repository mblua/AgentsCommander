use crate::phone::mailbox::response_observation::ResponseAdmissionState;
use crate::pty::manager::ResponseCloseState;
use tauri::Manager;

impl<R: tauri::Runtime> ResponseAdmissionState for tauri::AppHandle<R> {
    fn response_tracker(&self) -> Option<&ResponseCloseState> {
        self.try_state::<ResponseCloseState>()
            .map(|state| state.inner())
    }
}

#[cfg(test)]
mod tests;
