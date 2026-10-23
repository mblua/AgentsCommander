use super::*;
use std::io::Write;

#[derive(Clone)]
struct FakeWindow {
    id: String,
    width: u32,
    height: u32,
    minimized: bool,
    image: FakeImage,
}

#[derive(Clone)]
struct FakeImage {
    width: u32,
    height: u32,
}

struct FakeTargetWindowCaptureOps {
    windows: Vec<FakeWindow>,
    all_windows_calls: usize,
    capture_calls: usize,
    encode_calls: usize,
    capture_fails: bool,
}

impl FakeTargetWindowCaptureOps {
    fn with_windows(windows: Vec<FakeWindow>) -> Self {
        Self {
            windows,
            all_windows_calls: 0,
            capture_calls: 0,
            encode_calls: 0,
            capture_fails: false,
        }
    }
}

impl TargetWindowCaptureOps for FakeTargetWindowCaptureOps {
    type Window = FakeWindow;
    type Image = FakeImage;

    fn all_windows(&mut self) -> Result<Vec<Self::Window>, String> {
        self.all_windows_calls += 1;
        Ok(self.windows.clone())
    }

    fn window_id(&mut self, window: &Self::Window) -> Result<String, String> {
        Ok(window.id.clone())
    }

    fn window_width(&mut self, window: &Self::Window) -> Result<u32, String> {
        Ok(window.width)
    }

    fn window_height(&mut self, window: &Self::Window) -> Result<u32, String> {
        Ok(window.height)
    }

    fn is_minimized(&mut self, window: &Self::Window) -> Result<bool, String> {
        Ok(window.minimized)
    }

    fn capture_image(&mut self, window: &Self::Window) -> Result<Self::Image, String> {
        self.capture_calls += 1;
        if self.capture_fails {
            return Err("native failure".to_string());
        }
        Ok(window.image.clone())
    }

    fn image_dimensions(&mut self, image: &Self::Image) -> (u32, u32) {
        (image.width, image.height)
    }

    fn encode_png_bounded(
        &mut self,
        _image: &Self::Image,
    ) -> Result<Vec<u8>, WindowScreenshotCaptureError> {
        self.encode_calls += 1;
        Ok(vec![1, 2, 3])
    }
}

fn fake_window(
    id: &str,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
) -> FakeWindow {
    FakeWindow {
        id: id.to_string(),
        width,
        height,
        minimized: false,
        image: FakeImage {
            width: image_width,
            height: image_height,
        },
    }
}

#[test]
fn capture_window_worker_snapshot_not_found_has_no_monitor_capability() {
    let mut ops = FakeTargetWindowCaptureOps::with_windows(vec![fake_window("11", 1, 1, 1, 1)]);

    let result = capture_window_worker(&mut ops, "22");

    assert!(matches!(
        result,
        Err(WindowScreenshotCaptureError::NotFound)
    ));
    assert_eq!(ops.all_windows_calls, 1);
    assert_eq!(ops.capture_calls, 0);
}

#[test]
fn capture_window_worker_failure_has_no_monitor_capability() {
    let mut ops = FakeTargetWindowCaptureOps::with_windows(vec![fake_window("22", 1, 1, 1, 1)]);
    ops.capture_fails = true;

    let result = capture_window_worker(&mut ops, "22");

    assert!(matches!(
        result,
        Err(WindowScreenshotCaptureError::Unavailable)
    ));
    assert_eq!(ops.all_windows_calls, 1);
    assert_eq!(ops.capture_calls, 1);
}

#[test]
fn capture_window_worker_advisory_and_post_capture_size_semantics() {
    let mut advisory_ops =
        FakeTargetWindowCaptureOps::with_windows(vec![fake_window("22", 4_097, 4_096, 1, 1)]);
    let advisory_result = capture_window_worker(&mut advisory_ops, "22");
    assert!(matches!(
        advisory_result,
        Err(WindowScreenshotCaptureError::TooLarge)
    ));
    assert_eq!(advisory_ops.capture_calls, 0);

    let mut post_capture_ops =
        FakeTargetWindowCaptureOps::with_windows(vec![fake_window("22", 1, 1, 4_097, 4_096)]);
    let post_capture_result = capture_window_worker(&mut post_capture_ops, "22");
    assert!(matches!(
        post_capture_result,
        Err(WindowScreenshotCaptureError::TooLarge)
    ));
    assert_eq!(post_capture_ops.capture_calls, 1);
    assert_eq!(post_capture_ops.encode_calls, 0);

    let mut writer = BoundedPngWriter::new(crate::api::WINDOW_SCREENSHOT_MAX_PNG_BYTES);
    let oversized_chunk = vec![0_u8; crate::api::WINDOW_SCREENSHOT_MAX_PNG_BYTES + 1];
    assert!(writer.write_all(&oversized_chunk).is_err());
    assert!(writer.exceeded);
}
