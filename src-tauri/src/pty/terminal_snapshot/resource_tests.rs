use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use terminal_snapshot_renderer::{
    canonical_timestamp, decode_host_response, render_png, TerminalActiveBuffer,
    TerminalBackendKind, TerminalCell, TerminalCellStyle, TerminalCellWidth, TerminalColor,
    TerminalCursor, TerminalDimensions, TerminalLine, TerminalScreen, TerminalScreenModel,
    TerminalSnapshotFidelity, TerminalSnapshotFormat, TerminalSnapshotPayload,
    TerminalSnapshotSession, MAX_CELLS, MAX_GLYPH_MASK_BYTES, MAX_JSON_BYTES, MAX_PNG_BYTES,
    MAX_PNG_DECODER_ALLOCATION_BYTES, MAX_RGB_BYTES, MAX_TRANSPORT_BYTES,
};
use uuid::Uuid;

use super::*;
use crate::pty::backend::{SessionBackendKind, TerminalScreenCopyRead};
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::SessionIoFanout;
use crate::session::profile::IdleTuning;
use crate::telegram::manager::OutputSenderMap;

const RESOURCE_CHILD_ENV: &str = "AC_TERMINAL_SNAPSHOT_RESOURCE_CHILD";
const RESOURCE_TEST_NAME: &str =
    "pty::terminal_snapshot::resource_tests::maximum_payload_resource_evidence_is_bounded";
const LARGE_ALLOCATION_BYTES: usize = 1024 * 1024;
// The frozen 60-second client maximum is a generous completion ceiling distinct from the
// 10-second authorization deadline. The timeout behavior itself is tested without a speed race.
const MAXIMUM_MODEL_TEST_CEILING: Duration = Duration::from_secs(60);
const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000117";
const REQUESTER: &str = "project:wg-1-team/coordinator";
const TARGET: &str = "project:wg-1-team/member";
const CONFIRMATION_TAG: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const EXPIRES_AT: &str = "2026-07-31T03:31:00.123Z";

struct MeasuringAllocator;

static TRACKING: AtomicBool = AtomicBool::new(false);
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static LARGE_LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static LARGE_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static LARGEST_ALLOCATION: AtomicUsize = AtomicUsize::new(0);

fn add_live(size: usize) {
    let live = LIVE_BYTES.fetch_add(size, Ordering::SeqCst) + size;
    PEAK_BYTES.fetch_max(live, Ordering::SeqCst);
    LARGEST_ALLOCATION.fetch_max(size, Ordering::SeqCst);
    if size >= LARGE_ALLOCATION_BYTES {
        let large = LARGE_LIVE_BYTES.fetch_add(size, Ordering::SeqCst) + size;
        LARGE_PEAK_BYTES.fetch_max(large, Ordering::SeqCst);
    }
}

fn remove_live(size: usize) {
    let _ = LIVE_BYTES.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
        Some(live.saturating_sub(size))
    });
    if size >= LARGE_ALLOCATION_BYTES {
        let _ = LARGE_LIVE_BYTES.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
            Some(live.saturating_sub(size))
        });
    }
}

// SAFETY: this test-only wrapper delegates every allocation to `System` with the original
// pointer/layout pair and changes only lock-free accounting after successful operations.
unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: delegation preserves the caller-provided layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && TRACKING.load(Ordering::SeqCst) {
            add_live(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: delegation preserves the caller-provided layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && TRACKING.load(Ordering::SeqCst) {
            add_live(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if TRACKING.load(Ordering::SeqCst) {
            remove_live(layout.size());
        }
        // SAFETY: delegation preserves the pointer/layout pair supplied by the caller.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: delegation preserves the original pointer/layout and requested new size.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() && TRACKING.load(Ordering::SeqCst) {
            remove_live(layout.size());
            add_live(new_size);
        }
        resized
    }
}

#[global_allocator]
static MEASURING_ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

#[derive(Debug, Clone, Copy)]
struct AllocationStats {
    live_bytes: usize,
    peak_bytes: usize,
    large_live_bytes: usize,
    large_peak_bytes: usize,
    largest_allocation: usize,
}

struct AllocationEpoch;

impl AllocationEpoch {
    fn start() -> Self {
        LIVE_BYTES.store(0, Ordering::SeqCst);
        PEAK_BYTES.store(0, Ordering::SeqCst);
        LARGE_LIVE_BYTES.store(0, Ordering::SeqCst);
        LARGE_PEAK_BYTES.store(0, Ordering::SeqCst);
        LARGEST_ALLOCATION.store(0, Ordering::SeqCst);
        TRACKING.store(true, Ordering::SeqCst);
        Self
    }

    fn live_bytes(&self) -> usize {
        LIVE_BYTES.load(Ordering::SeqCst)
    }

    fn reset_peak(&self) {
        let live = self.live_bytes();
        PEAK_BYTES.store(live, Ordering::SeqCst);
        LARGE_PEAK_BYTES.store(LARGE_LIVE_BYTES.load(Ordering::SeqCst), Ordering::SeqCst);
        LARGEST_ALLOCATION.store(0, Ordering::SeqCst);
    }

    fn peak_bytes(&self) -> usize {
        PEAK_BYTES.load(Ordering::SeqCst)
    }

    fn finish(self) -> AllocationStats {
        TRACKING.store(false, Ordering::SeqCst);
        let stats = AllocationStats {
            live_bytes: LIVE_BYTES.load(Ordering::SeqCst),
            peak_bytes: PEAK_BYTES.load(Ordering::SeqCst),
            large_live_bytes: LARGE_LIVE_BYTES.load(Ordering::SeqCst),
            large_peak_bytes: LARGE_PEAK_BYTES.load(Ordering::SeqCst),
            largest_allocation: LARGEST_ALLOCATION.load(Ordering::SeqCst),
        };
        std::mem::forget(self);
        stats
    }
}

impl Drop for AllocationEpoch {
    fn drop(&mut self) {
        TRACKING.store(false, Ordering::SeqCst);
    }
}

fn maximum_model(rows: u16, columns: u16, text: &str) -> Arc<TerminalScreenModel> {
    let mut lines = Vec::with_capacity(usize::from(rows));
    for row in 0..rows {
        let mut cells = Vec::with_capacity(usize::from(columns));
        for column in 0..columns {
            cells.push(TerminalCell {
                text: text.to_string(),
                width: TerminalCellWidth::Narrow,
                foreground: TerminalColor::Rgb {
                    red: 255,
                    green: 254,
                    blue: 253,
                },
                background: TerminalColor::Rgb {
                    red: 1,
                    green: 2,
                    blue: 3,
                },
                style: TerminalCellStyle {
                    bold: (row + column) % 2 == 0,
                    italic: true,
                    underline: true,
                    inverse: false,
                },
            });
        }
        lines.push(TerminalLine {
            wrapped: row + 1 != rows,
            cells,
        });
    }
    let model = TerminalScreenModel {
        captured_at: canonical_timestamp(
            chrono::DateTime::parse_from_rfc3339("2026-07-31T03:30:00.123Z")
                .expect("fixed timestamp")
                .with_timezone(&chrono::Utc),
        ),
        session: TerminalSnapshotSession {
            id: "00000000-0000-4000-8000-000000000116".to_string(),
            backend: TerminalBackendKind::LocalProcess,
        },
        screen: TerminalScreen {
            dimensions: TerminalDimensions { rows, columns },
            sequence: u64::MAX,
            active_buffer: TerminalActiveBuffer::Alternate,
            cursor: TerminalCursor {
                row: rows - 1,
                column: columns - 1,
                visible: true,
                in_bounds: true,
            },
            parser_errors: 0,
            lines,
        },
        fidelity: TerminalSnapshotFidelity::version_one(false),
    };
    model.validate().expect("maximum model must be valid");
    Arc::new(model)
}

fn maximum_json_model() -> Arc<TerminalScreenModel> {
    let mut model = maximum_model(200, 200, "😀😀😀😀😀😀");
    {
        let inner = Arc::get_mut(&mut model).expect("unshared maximum JSON model");
        inner.session.backend = TerminalBackendKind::ContainerTransport;
        inner.screen.cursor.row = inner.screen.dimensions.rows;
        inner.screen.cursor.column = inner.screen.dimensions.columns;
        inner.screen.cursor.visible = false;
        inner.screen.cursor.in_bounds = false;
        inner.screen.parser_errors = u64::MAX;
        inner.fidelity = TerminalSnapshotFidelity::version_one(true);
        for line in &mut inner.screen.lines {
            line.wrapped = false;
            for cell in &mut line.cells {
                cell.foreground = TerminalColor::Rgb {
                    red: 255,
                    green: 255,
                    blue: 255,
                };
                cell.background = TerminalColor::Rgb {
                    red: 255,
                    green: 255,
                    blue: 255,
                };
                cell.style = TerminalCellStyle::default();
            }
        }
        inner.validate().expect("maximum JSON model");
    }
    model
}

fn warm_renderer() {
    let model = maximum_model(1, 1, "A");
    render_png(&model).expect("renderer warm-up");
}

fn maximum_capture_evidence() -> (usize, AllocationStats) {
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let fanout = SessionIoFanout::new(output_senders, IdleDetector::new(|_| {}, |_| {}), None);
    let accepted = Uuid::new_v4();
    fanout
        .register_session_for_test(accepted, IdleTuning::DEFAULT, 200, 200)
        .expect("register accepted test session");

    let epoch = AllocationEpoch::start();
    let copied = match fanout.copy_terminal_screen(accepted) {
        TerminalScreenCopyRead::Copied(copied) => copied,
        _ => panic!("maximum viewport must be copied"),
    };
    let model = copied
        .into_model(accepted, SessionBackendKind::LocalProcess)
        .expect("maximum viewport model");
    let cells = model
        .screen
        .lines
        .iter()
        .map(|line| line.cells.len())
        .sum::<usize>();
    let peak = epoch.peak_bytes();
    drop(model);
    let stats = epoch.finish();

    let rejected = Uuid::new_v4();
    fanout
        .register_session_for_test(rejected, IdleTuning::DEFAULT, 201, 200)
        .expect("register rejected test session");
    assert!(matches!(
        fanout.copy_terminal_screen(rejected),
        TerminalScreenCopyRead::TooLarge
    ));
    assert!(peak > 0);
    (cells, stats)
}

fn run_resource_child() {
    warm_renderer();

    let (capture_measure, capture_stats) = maximum_capture_evidence();
    assert_eq!(capture_measure, MAX_CELLS);
    assert_eq!(capture_stats.large_live_bytes, 0);

    let epoch = AllocationEpoch::start();
    let json_model = maximum_json_model();
    let json_model_live = epoch.live_bytes();
    let json_payload = build_payload(
        TerminalSnapshotFormat::Json,
        REQUEST_ID.to_string(),
        REQUESTER.to_string(),
        TARGET.to_string(),
        Arc::clone(&json_model),
    )
    .expect("maximum JSON payload");
    let retained_model = match &json_payload {
        PreparedSnapshotPayload::Json { model, .. } => model,
        PreparedSnapshotPayload::Png(_) => panic!("wrong prepared format"),
    };
    assert!(Arc::ptr_eq(&json_model, retained_model));
    let json_bytes = json_payload.payload_bytes().expect("maximum JSON size");
    assert!(json_bytes <= MAX_JSON_BYTES as u64);
    let json_baseline = epoch.live_bytes();
    epoch.reset_peak();
    let json_wire = json_payload
        .encode_host(REQUEST_ID, CONFIRMATION_TAG, EXPIRES_AT)
        .expect("maximum JSON envelope");
    let json_encode_peak = epoch.peak_bytes();
    assert!(json_encode_peak <= json_baseline + MAX_TRANSPORT_BYTES);
    let json_wire_len = json_wire.len();
    assert!(json_wire_len <= MAX_TRANSPORT_BYTES);
    assert!(json_wire.capacity() <= MAX_TRANSPORT_BYTES);
    drop(json_wire);
    drop(json_payload);
    assert_eq!(epoch.live_bytes(), json_model_live);
    drop(json_model);
    let json_stats = epoch.finish();
    assert_eq!(json_stats.large_live_bytes, 0);

    let epoch = AllocationEpoch::start();
    let png_model = maximum_model(200, 200, "A");
    let image = png_model
        .screen
        .dimensions
        .checked_image_dimensions()
        .expect("maximum raster dimensions");
    assert_eq!(image.cells, MAX_CELLS);
    assert_eq!(image.rgb_bytes, MAX_RGB_BYTES);
    let png_model_live = epoch.live_bytes();
    epoch.reset_peak();
    let render_started = Instant::now();
    let png_payload = build_payload(
        TerminalSnapshotFormat::Png,
        REQUEST_ID.to_string(),
        REQUESTER.to_string(),
        TARGET.to_string(),
        Arc::clone(&png_model),
    )
    .expect("maximum PNG payload");
    let render_elapsed = render_started.elapsed();
    let render_peak = epoch.peak_bytes();
    // These are the frozen buffers that may overlap during render and PNG validation.
    let render_bound = png_model_live
        + MAX_RGB_BYTES
        + MAX_PNG_BYTES
        + MAX_GLYPH_MASK_BYTES
        + MAX_PNG_DECODER_ALLOCATION_BYTES;
    assert!(render_peak <= render_bound);
    assert!(render_elapsed <= MAXIMUM_MODEL_TEST_CEILING);
    let png_len = match &png_payload {
        PreparedSnapshotPayload::Png(payload) => match &**payload {
            TerminalSnapshotPayload::Png { metadata, png } => {
                assert_eq!(metadata.png.pixel_width, image.pixel_width);
                assert_eq!(metadata.png.pixel_height, image.pixel_height);
                assert!(
                    png.capacity() <= MAX_PNG_BYTES,
                    "PNG length {} retained capacity {}",
                    png.len(),
                    png.capacity()
                );
                png.len()
            }
            TerminalSnapshotPayload::Json { .. } => panic!("wrong prepared payload"),
        },
        PreparedSnapshotPayload::Json { .. } => panic!("wrong prepared format"),
    };
    let png_baseline = epoch.live_bytes();
    epoch.reset_peak();
    let png_wire = png_payload
        .encode_host(REQUEST_ID, CONFIRMATION_TAG, EXPIRES_AT)
        .expect("maximum PNG envelope");
    let png_encode_peak = epoch.peak_bytes();
    assert!(
        png_encode_peak
            <= png_baseline
                + MAX_RGB_BYTES
                + MAX_PNG_DECODER_ALLOCATION_BYTES
                + MAX_TRANSPORT_BYTES
    );
    let png_wire_len = png_wire.len();
    let base64_len = png_len
        .checked_add(2)
        .and_then(|length| length.checked_div(3))
        .and_then(|groups| groups.checked_mul(4))
        .expect("bounded base64 length");
    assert!(png_wire_len > base64_len);
    assert!(png_wire_len <= MAX_TRANSPORT_BYTES);
    assert!(png_wire.capacity() <= MAX_TRANSPORT_BYTES);
    drop(png_payload);
    epoch.reset_peak();
    let decoded = decode_host_response(
        &png_wire,
        REQUEST_ID,
        CONFIRMATION_TAG,
        TARGET,
        TerminalSnapshotFormat::Png,
    )
    .expect("maximum PNG response decode");
    match decoded.result.expect("host success result") {
        TerminalSnapshotPayload::Png { png, .. } => assert_eq!(png.len(), png_len),
        TerminalSnapshotPayload::Json { .. } => panic!("wrong decoded format"),
    }
    let decode_peak = epoch.peak_bytes();
    assert!(
        decode_peak
            <= epoch.live_bytes()
                + MAX_RGB_BYTES
                + MAX_PNG_DECODER_ALLOCATION_BYTES
                + MAX_PNG_BYTES
    );
    drop(png_wire);
    drop(png_model);
    let png_stats = epoch.finish();
    assert_eq!(png_stats.large_live_bytes, 0);

    let epoch = AllocationEpoch::start();
    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let first_model = maximum_model(200, 200, "");
    let second_model = maximum_model(100, 400, "");
    let first = state.admit_requester("requester-a".to_string()).unwrap();
    first.promote_target("target-a".to_string()).unwrap();
    let second = state.admit_requester("requester-b".to_string()).unwrap();
    second.promote_target("target-b".to_string()).unwrap();
    let concurrency_baseline = epoch.live_bytes();
    epoch.reset_peak();
    let hold = Arc::new(Barrier::new(3));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let first_hold = Arc::clone(&hold);
    let first_ready = ready_tx.clone();
    let first_thread = std::thread::spawn(move || {
        let raster = vec![0u8; MAX_RGB_BYTES];
        first_ready.send(()).unwrap();
        first_hold.wait();
        drop(raster);
        let started = Instant::now();
        let payload = build_payload(
            TerminalSnapshotFormat::Png,
            "00000000-0000-4000-8000-000000000118".to_string(),
            REQUESTER.to_string(),
            TARGET.to_string(),
            first_model,
        )
        .unwrap();
        drop(payload);
        drop(first);
        started.elapsed()
    });
    let second_hold = Arc::clone(&hold);
    let second_thread = std::thread::spawn(move || {
        let raster = vec![0u8; MAX_RGB_BYTES];
        ready_tx.send(()).unwrap();
        second_hold.wait();
        drop(raster);
        let started = Instant::now();
        let payload = build_payload(
            TerminalSnapshotFormat::Png,
            "00000000-0000-4000-8000-000000000119".to_string(),
            REQUESTER.to_string(),
            TARGET.to_string(),
            second_model,
        )
        .unwrap();
        drop(payload);
        drop(second);
        started.elapsed()
    });
    ready_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("first maximum render");
    ready_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("second maximum render");
    assert!(state.admit_requester("requester-c".to_string()).is_err());
    {
        let limiter = state.limiter.lock().unwrap();
        assert_eq!(limiter.global_in_flight, SNAPSHOT_GLOBAL_IN_FLIGHT);
        assert_eq!(limiter.requester_in_flight.len(), SNAPSHOT_GLOBAL_IN_FLIGHT);
        assert_eq!(limiter.target_in_flight.len(), SNAPSHOT_GLOBAL_IN_FLIGHT);
    }
    let concurrent_live = epoch.live_bytes();
    assert!(concurrent_live >= concurrency_baseline + 2 * MAX_RGB_BYTES);
    hold.wait();
    let first_elapsed = first_thread.join().unwrap();
    let second_elapsed = second_thread.join().unwrap();
    assert!(first_elapsed <= MAXIMUM_MODEL_TEST_CEILING);
    assert!(second_elapsed <= MAXIMUM_MODEL_TEST_CEILING);
    {
        let limiter = state.limiter.lock().unwrap();
        assert_eq!(limiter.global_in_flight, 0);
        assert!(limiter.requester_in_flight.is_empty());
        assert!(limiter.target_in_flight.is_empty());
    }
    let reclaimed = state.admit_requester("requester-a".to_string()).unwrap();
    reclaimed.promote_target("target-a".to_string()).unwrap();
    drop(reclaimed);
    drop(ready_rx);
    drop(hold);
    drop(state);
    let concurrency_stats = epoch.finish();
    assert_eq!(concurrency_stats.large_live_bytes, 0);
    // Two permits are the sole multiplier for the exact frozen per-request raster path.
    let concurrent_bound = concurrency_baseline
        + 2 * (MAX_RGB_BYTES
            + MAX_PNG_BYTES
            + MAX_GLYPH_MASK_BYTES
            + MAX_PNG_DECODER_ALLOCATION_BYTES);
    assert!(concurrency_stats.peak_bytes <= concurrent_bound);
    assert!(concurrency_stats.large_peak_bytes >= 2 * MAX_RGB_BYTES);

    eprintln!(
        "snapshot_resource_evidence cells={} capture_peak={} json_bytes={} json_wire={} json_peak={} rgb_bytes={} png_bytes={} base64_bytes={} png_wire={} render_peak={} encode_peak={} decode_peak={} render_ms={} concurrent_live={} concurrent_peak={} large_peak={} largest_allocation={} live_tail={}",
        capture_measure,
        capture_stats.peak_bytes,
        json_bytes,
        json_wire_len,
        json_stats.peak_bytes,
        image.rgb_bytes,
        png_len,
        base64_len,
        png_wire_len,
        render_peak,
        png_encode_peak,
        decode_peak,
        render_elapsed.as_millis(),
        concurrent_live,
        concurrency_stats.peak_bytes,
        concurrency_stats.large_peak_bytes,
        concurrency_stats.largest_allocation,
        capture_stats.live_bytes + json_stats.live_bytes + png_stats.live_bytes + concurrency_stats.live_bytes,
    );
}

#[test]
fn maximum_payload_resource_evidence_is_bounded() {
    if std::env::var_os(RESOURCE_CHILD_ENV).is_some() {
        run_resource_child();
        return;
    }
    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg(RESOURCE_TEST_NAME)
        .arg("--nocapture")
        .env(RESOURCE_CHILD_ENV, "1")
        .output()
        .expect("isolated maximum-payload child");
    assert!(
        output.status.success(),
        "maximum-payload child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("snapshot_resource_evidence"));
}

struct ReclamationProbe(Arc<AtomicBool>);

impl Drop for ReclamationProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_deadline_retains_maximum_raster_and_permit_until_reclamation() {
    assert_eq!(SNAPSHOT_SERVER_TIMEOUT, Duration::from_secs(10));
    let accepted_at = Instant::now();
    assert_eq!(
        accepted_at
            .checked_add(SNAPSHOT_SERVER_TIMEOUT)
            .unwrap()
            .duration_since(accepted_at),
        Duration::from_secs(10)
    );

    let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
    let permit = state
        .admit_requester("deadline-requester".to_string())
        .unwrap();
    permit
        .promote_target("deadline-target".to_string())
        .unwrap();
    let audit =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let reclaimed = Arc::new(AtomicBool::new(false));
    let probe = ReclamationProbe(Arc::clone(&reclaimed));
    let expired = Instant::now()
        .checked_sub(Duration::from_millis(1))
        .unwrap();
    let result = run_blocking_with_deadline(
        &state,
        TerminalSnapshotBlockingStage::TestResourceRetention,
        expired,
        &permit,
        &audit,
        move || {
            let probe = probe;
            let raster = vec![0u8; MAX_RGB_BYTES];
            assert_eq!(raster.len(), MAX_RGB_BYTES);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(raster);
            drop(probe);
            finished_tx.send(()).unwrap();
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(TerminalSnapshotReasonCode::SnapshotTimeout)
    ));
    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("maximum raster started");
    drop(permit);
    assert!(state
        .admit_requester("deadline-requester".to_string())
        .is_err());
    assert!(!reclaimed.load(Ordering::SeqCst));
    release_tx.send(()).unwrap();
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("maximum raster reclaimed");
    assert!(reclaimed.load(Ordering::SeqCst));
    let reclamation_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(reclaimed_permit) = state.admit_requester("deadline-requester".to_string()) {
            reclaimed_permit
                .promote_target("deadline-target".to_string())
                .expect("target permit reclaimed");
            break;
        }
        assert!(
            Instant::now() < reclamation_deadline,
            "limiter permit was not reclaimed"
        );
        tokio::task::yield_now().await;
    }
}
