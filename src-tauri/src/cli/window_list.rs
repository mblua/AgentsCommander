//! `window-list` CLI verb: prints the live native windows as `id<TAB>title`
//! lines so users and agents can discover the canonical decimal `window_id`
//! that `window-screenshot` requires. Windows only. Issue #1315.

use clap::Args;

use crate::cli_println;

#[derive(Args)]
pub struct WindowListArgs {}

pub fn execute(_args: WindowListArgs) -> i32 {
    let windows = match xcap::Window::all() {
        Ok(windows) => windows,
        Err(error) => {
            eprintln!("window_list_error code=window_list_unavailable detail={error}");
            return 1;
        }
    };
    for window in windows {
        let Ok(id) = window.id() else { continue };
        let title = window.title().unwrap_or_default();
        cli_println!("{id}\t{title}");
    }
    0
}
