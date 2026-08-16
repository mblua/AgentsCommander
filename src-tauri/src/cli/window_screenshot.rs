//! `window-screenshot` CLI verb: captures one live native window to a PNG
//! file by its canonical decimal window id, reusing the #1285 in-process
//! capture worker directly (no HTTP, no auth, no daemon). Windows only.
//! Issue #1315.

use std::path::PathBuf;

use clap::Args;

use crate::{
    api::{WindowScreenshotLease, WindowScreenshotLimiter},
    screenshot::{capture_window_png, parse_window_id, WindowScreenshotCaptureError},
};

#[derive(Args)]
pub struct WindowScreenshotArgs {
    /// Canonical decimal window id as printed by `window-list`
    #[arg(long)]
    pub window_id: String,
    /// Destination PNG file path (overwritten if it exists)
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowScreenshotCliError {
    InvalidWindowId,
    WindowNotFound,
    CaptureBusy,
    CaptureTooLarge,
    CaptureUnavailable,
    OutputWriteFailed(String),
}

impl WindowScreenshotCliError {
    fn code(&self) -> &'static str {
        match self {
            WindowScreenshotCliError::InvalidWindowId => "invalid_window_id",
            WindowScreenshotCliError::WindowNotFound => "window_not_found",
            WindowScreenshotCliError::CaptureBusy => "capture_busy",
            WindowScreenshotCliError::CaptureTooLarge => "capture_too_large",
            WindowScreenshotCliError::CaptureUnavailable => "capture_unavailable",
            WindowScreenshotCliError::OutputWriteFailed(_) => "output_write_failed",
        }
    }

    fn detail(&self) -> String {
        match self {
            WindowScreenshotCliError::InvalidWindowId => {
                "window id must be a canonical decimal (no sign, no leading zeros, at most 20 digits)".to_string()
            }
            WindowScreenshotCliError::WindowNotFound => "no live window with that id was found".to_string(),
            WindowScreenshotCliError::CaptureBusy => "capture capacity is full".to_string(),
            WindowScreenshotCliError::CaptureTooLarge => "window capture exceeds the configured size limit".to_string(),
            WindowScreenshotCliError::CaptureUnavailable => "window capture is unavailable".to_string(),
            WindowScreenshotCliError::OutputWriteFailed(error) => format!("failed to write output file: {error}"),
        }
    }
}

pub fn execute(args: WindowScreenshotArgs) -> i32 {
    match execute_inner(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "window_screenshot_error code={} detail={}",
                error.code(),
                error.detail()
            );
            1
        }
    }
}

fn execute_inner(args: WindowScreenshotArgs) -> Result<(), WindowScreenshotCliError> {
    let Some(window_id) = parse_window_id(&args.window_id) else {
        return Err(WindowScreenshotCliError::InvalidWindowId);
    };
    let limiter = WindowScreenshotLimiter::new();
    let admission = limiter
        .try_admit()
        .map_err(|_| WindowScreenshotCliError::CaptureBusy)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| WindowScreenshotCliError::CaptureUnavailable)?;
    let png = runtime.block_on(async {
        let active = limiter
            .acquire_active()
            .await
            .map_err(|_| WindowScreenshotCliError::CaptureBusy)?;
        let lease = WindowScreenshotLease::new(admission, active);
        capture_window_png(window_id, lease)
            .await
            .map_err(|error| match error {
                WindowScreenshotCaptureError::NotFound => WindowScreenshotCliError::WindowNotFound,
                WindowScreenshotCaptureError::TooLarge => WindowScreenshotCliError::CaptureTooLarge,
                WindowScreenshotCaptureError::Unavailable => {
                    WindowScreenshotCliError::CaptureUnavailable
                }
            })
    })?;
    std::fs::write(&args.output, &png)
        .map_err(|error| WindowScreenshotCliError::OutputWriteFailed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_inner_rejects_noncanonical_window_ids_before_capture() {
        for raw in [
            "",
            "abc",
            "-1",
            "+1",
            " 1",
            "1 ",
            "01",
            "007",
            "1.5",
            "%FF",
            "123456789012345678901",
            "18446744073709551616",
        ] {
            let args = WindowScreenshotArgs {
                window_id: raw.to_string(),
                output: PathBuf::from("out.png"),
            };
            assert_eq!(
                execute_inner(args),
                Err(WindowScreenshotCliError::InvalidWindowId)
            );
        }
    }
}
