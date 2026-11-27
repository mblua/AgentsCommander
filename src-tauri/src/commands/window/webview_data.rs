//! #5279: every product window of one instance uses that instance's own
//! `WebView` data directory, so another build on the same host cannot read,
//! delete, or make the toolkit delete its storage. wry applies it on Linux
//! and Windows; macOS ignores it.

use std::path::PathBuf;

use tauri::{Manager, Runtime, WebviewWindowBuilder};

pub(crate) trait InstanceDataDir: Sized {
    /// `Some(dir)`: store this window's `WebView` data in `dir`.
    /// `None` (no config dir): keep the toolkit default and warn.
    fn instance_data_dir(self, dir: Option<PathBuf>) -> Self;
}

impl<'a, R: Runtime, M: Manager<R>> InstanceDataDir for WebviewWindowBuilder<'a, R, M> {
    fn instance_data_dir(self, dir: Option<PathBuf>) -> Self {
        match dir {
            Some(dir) => self.data_directory(dir),
            None => {
                log::warn!(
                    "[webview] no config dir; this window uses the shared default \
                     WebView data directory (#5279)"
                );
                self
            }
        }
    }
}
