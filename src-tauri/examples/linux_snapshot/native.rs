// Linux-only GTK/WebKit harness for the #4248 snapshot experiment: windows,
// callbacks, timers, capture and cleanup. It uses only std and receipt types;
// main supplies the validators as plain fn hooks.
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

use gio::prelude::CancellableExt;
use glib::{prelude::ObjectExt, thread_guard::ThreadGuard};
use serde_json::{json, Value};
use tauri::{RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use webkit2gtk::{LoadEvent, SettingsExt, SnapshotOptions, SnapshotRegion, WebViewExt};

use crate::receipt::{
    BoundedWriter, Outcome, PixelVerdict, Probe, ProbeRejection, Readiness, Receipt, RequestGate,
    Snapshot, StepSpec, MAX_PNG_BYTES,
};

// Validators from main as fn pointers (Copy + Send + Sync, no captured state).
#[derive(Clone, Copy)]
pub struct Hooks {
    pub readiness: fn(&str, &StepSpec, &str) -> Readiness,
    pub validate: fn(&Snapshot<'_>) -> PixelVerdict,
    pub verdict: fn(&[Value]) -> Outcome,
}

pub struct RunSpec {
    pub probe: Probe,
    pub off_workspace: bool,
    pub steps: Vec<StepSpec>,
    pub hooks: Hooks,
}

struct NativeView {
    label: &'static str,
    view: webkit2gtk::WebView,
    handlers: Vec<glib::SignalHandlerId>,
}

#[derive(Default)]
struct Progress {
    views: Vec<NativeView>,
    loaded: Vec<&'static str>,
    index: Option<usize>,
    gate: RequestGate,
    ready: Option<String>,
    cancellable: Option<gio::Cancellable>,
    deadline: Option<glib::SourceId>,
    cases: Vec<Value>,
    // Digest of the last published frame A, and the stale probe's real frame A.
    previous_digest: Option<String>,
    stale_frame: Option<Vec<u8>>,
    script_executed: bool,
    snapshot_taken: bool,
    exit: Option<u8>,
}

struct Harness {
    spec: RunSpec,
    receipt: Option<Receipt>,
    app: tauri::AppHandle,
    windows: Vec<(&'static str, WebviewWindow)>,
    nonce: String,
    state: Progress,
}

type Shared = Rc<RefCell<Harness>>;

// Builds the app and its own two (or one, for probes) WebViews, then runs the
// GTK loop until finish() ends it. Returns the process exit code.
#[expect(clippy::exit, reason = "Tauri generated thread failure exits")]
pub fn run(mut receipt: Receipt, spec: RunSpec) -> u8 {
    receipt.set("tauriVersion", json!(tauri::VERSION));
    let app = match tauri::Builder::default().build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(error) => return receipt.finish(&Outcome::Incomplete(format!("tauri_build: {error}"))),
    };
    let mut windows = Vec::new();
    for label in ["control", "target"] {
        if !spec.steps.iter().any(|step| step.label == label) {
            continue;
        }
        match build_window(&app, label, receipt.webkit_dir(label)) {
            Ok(window) => windows.push((label, window)),
            Err(error) => {
                return receipt.finish(&Outcome::Incomplete(format!("window {label}: {error}")))
            }
        }
    }
    let labels: Vec<&'static str> = windows.iter().map(|(label, _)| *label).collect();
    let elapsed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let nonce = format!("{:x}", elapsed.map_or(0, |elapsed| elapsed.as_nanos()));
    let (receipt, app_handle, state) = (Some(receipt), app.handle().clone(), Progress::default());
    let harness = Harness {
        spec,
        receipt,
        app: app_handle,
        windows,
        nonce,
        state,
    };
    let shared = Rc::new(RefCell::new(harness));
    for label in labels {
        if let Err(error) = attach(&shared, label) {
            let outcome = Outcome::Incomplete(format!("with_webview {label}: {error}"));
            finish(&mut shared.borrow_mut(), &outcome);
        }
    }
    let events = Rc::clone(&shared);
    app.run_return(move |_handle, event| on_run_event(&events, &event));
    let mut harness = shared.borrow_mut();
    if harness.state.exit.is_none() {
        finish(
            &mut harness,
            &Outcome::Incomplete("event_loop_exited".to_owned()),
        );
    }
    harness.state.exit.unwrap_or(2)
}

fn build_window(app: &tauri::App, label: &str, data_dir: PathBuf) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("ac4248/index.html".into()))
        .title(format!("ac4248 {label}"))
        .data_directory(data_dir)
        .focused(false)
        .inner_size(1024.0, 768.0)
        .resizable(false)
        .build()
}

impl Harness {
    fn step(&self) -> Option<StepSpec> {
        self.state
            .index
            .and_then(|index| self.spec.steps.get(index))
            .copied()
    }

    fn window(&self, label: &str) -> Option<WebviewWindow> {
        let found = self.windows.iter().find(|(name, _)| *name == label);
        found.map(|(_, window)| window.clone())
    }

    fn view(&self, label: &str) -> Option<webkit2gtk::WebView> {
        let found = self.state.views.iter().find(|native| native.label == label);
        found.map(|native| native.view.clone())
    }
}

// Native title/load notifications are attached on the GTK main thread through
// with_webview; the shared state never leaves that thread (ThreadGuard).
fn attach(shared: &Shared, label: &'static str) -> tauri::Result<()> {
    let Some(window) = shared.borrow().window(label) else {
        return Ok(());
    };
    let guard = ThreadGuard::new(Rc::clone(shared));
    window.with_webview(move |platform| {
        let shared = guard.into_inner();
        let view = platform.inner();
        let on_load = Rc::clone(&shared);
        let load = view.connect_load_changed(move |_, event| {
            if event == LoadEvent::Finished {
                on_loaded(&on_load, label);
            }
        });
        let on_title = Rc::clone(&shared);
        let title = view.connect_title_notify(move |view| {
            let text = view
                .title()
                .map(|title| title.to_string())
                .unwrap_or_default();
            handle_title(&on_title, label, &text);
        });
        // Generic signal lookup: needs no webkit2gtk version feature.
        let on_crash = Rc::clone(&shared);
        let crash = view.connect_local("web-process-terminated", false, move |_| {
            on_web_process_terminated(&on_crash, label);
            None
        });
        let Ok(mut harness) = shared.try_borrow_mut() else {
            return;
        };
        let agent = view.settings().and_then(|settings| settings.user_agent());
        if let (Some(agent), Some(receipt)) = (agent, harness.receipt.as_mut()) {
            receipt.set(
                &format!("webkitUserAgent.{label}"),
                json!(agent.to_string()),
            );
        }
        harness.state.views.push(NativeView {
            label,
            view,
            handlers: vec![load, title, crash],
        });
        if harness.state.deadline.is_none() {
            let on_deadline = Rc::clone(&shared);
            let source = glib::timeout_add_local_once(Duration::from_secs(30), move || {
                on_timeout(&on_deadline);
            });
            harness.state.deadline = Some(source);
        }
    })
}

fn on_loaded(shared: &Shared, label: &'static str) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    if harness.state.exit.is_some() || harness.state.loaded.contains(&label) {
        return;
    }
    harness.state.loaded.push(label);
    if harness.state.index.is_none() && harness.state.loaded.len() == harness.windows.len() {
        harness.state.index = Some(0);
        start_step(&mut harness);
    }
}

// Native zoom first, then the single fixture function with a fresh generation.
fn start_step(harness: &mut Harness) {
    let (Some(step), Some(index)) = (harness.step(), harness.state.index) else {
        return;
    };
    let generation = format!("g{}n{index}", harness.nonce);
    harness.state.gate.begin(generation.clone());
    harness.state.ready = None;
    let Some(window) = harness.window(step.label) else {
        let outcome = Outcome::Incomplete(format!("missing window {}", step.label));
        return finish(harness, &outcome);
    };
    if let Err(error) = window.set_zoom(step.zoom) {
        return finish(harness, &Outcome::Incomplete(format!("set_zoom: {error}")));
    }
    let request = json!({
        "generation": generation,
        "frame": step.frame,
        "zoom": step.zoom,
        "renderer": step.renderer,
    });
    if let Err(error) = window.eval(format!("window.ac4248Frame({request});")) {
        finish(harness, &Outcome::Incomplete(format!("eval: {error}")));
    }
}

fn handle_title(shared: &Shared, label: &'static str, title: &str) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    let generation = harness.state.gate.current().map(str::to_owned);
    let (Some(step), Some(generation)) = (harness.step(), generation) else {
        return;
    };
    if harness.state.exit.is_some() || step.label != label || harness.state.ready.is_some() {
        return;
    }
    match (harness.spec.hooks.readiness)(title, &step, &generation) {
        Readiness::Pending => {}
        Readiness::Incomplete(reason) => finish(&mut harness, &Outcome::Incomplete(reason)),
        Readiness::Ready(ready) => {
            harness.state.script_executed = true;
            harness.state.ready = Some(ready);
            drop(harness);
            request_snapshot(shared, label, generation);
        }
    }
}

fn request_snapshot(shared: &Shared, label: &'static str, generation: String) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    if !harness.state.gate.start_snapshot(&generation) {
        return;
    }
    let Some(view) = harness.view(label) else {
        let outcome = Outcome::Incomplete(format!("missing native view {label}"));
        return finish(&mut harness, &outcome);
    };
    let cancellable = gio::Cancellable::new();
    if harness.spec.probe == Probe::Cancel {
        cancellable.cancel();
    }
    harness.state.cancellable = Some(cancellable.clone());
    drop(harness);
    let on_done = Rc::clone(shared);
    let (region, options) = (SnapshotRegion::Visible, SnapshotOptions::NONE);
    view.snapshot(region, options, Some(&cancellable), move |result| {
        on_snapshot(&on_done, &generation, result);
    });
}

fn on_snapshot(shared: &Shared, generation: &str, result: Result<cairo::Surface, glib::Error>) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    if harness.state.exit.is_some() || !harness.state.gate.finish_snapshot(generation) {
        return;
    }
    harness.state.cancellable = None;
    let cancel_probe = harness.spec.probe == Probe::Cancel;
    let surface = match result {
        Ok(surface) => surface,
        Err(error) if cancel_probe && error.matches(gio::IOErrorEnum::Cancelled) => {
            let rejection = ProbeRejection {
                probe: "cancel",
                error: "snapshot_cancelled",
                snapshot_taken: false,
                validator_reached: false,
                exit: 22,
            };
            return finish(&mut harness, &Outcome::ProbeRejected(rejection));
        }
        Err(error) => {
            return finish(
                &mut harness,
                &Outcome::Fail(format!("snapshot_error: {error}")),
            )
        }
    };
    harness.state.snapshot_taken = true;
    if cancel_probe {
        return finish(
            &mut harness,
            &Outcome::Fail("probe_cancel_not_rejected".to_owned()),
        );
    }
    match surface_to_rgba(surface) {
        Ok((image, width, height, rgba)) => handle_image(&mut harness, &image, width, height, rgba),
        Err(reason) => finish(&mut harness, &Outcome::Fail(reason)),
    }
}

type Converted = (cairo::ImageSurface, u32, u32, Vec<u8>);

// Snapshot surfaces are native-endian ARGB32/RGB24; convert them to plain RGBA.
fn surface_to_rgba(surface: cairo::Surface) -> Result<Converted, String> {
    let image = cairo::ImageSurface::try_from(surface)
        .map_err(|_surface| "unsupported_surface_type".to_owned())?;
    let opaque = match image.format() {
        cairo::Format::ARgb32 => false,
        cairo::Format::Rgb24 => true,
        other => return Err(format!("unsupported_surface_format {other:?}")),
    };
    let size = |value: i32| u32::try_from(value).map_err(|error| format!("surface_size {error}"));
    let (width, height) = (size(image.width())?, size(image.height())?);
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || width > 4096 || height > 4096 || pixels > 16 * 1024 * 1024 {
        return Err(format!("surface_bounds {width}x{height}"));
    }
    let stride = usize::try_from(image.stride()).map_err(|error| format!("stride {error}"))?;
    let mut data = Vec::with_capacity(width as usize * height as usize * 4);
    image
        .with_data(|bytes| {
            for row in bytes.chunks(stride).take(height as usize) {
                for chunk in row.as_chunks::<4>().0.iter().take(width as usize) {
                    let value = u32::from_ne_bytes(*chunk);
                    let alpha = if opaque { 255 } else { (value >> 24) as u8 };
                    let rgb = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
                    data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
                }
            }
        })
        .map_err(|error| format!("surface_data {error}"))?;
    Ok((image.clone(), width, height, data))
}

fn handle_image(
    harness: &mut Harness,
    surface: &cairo::ImageSurface,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) {
    let (Some(step), Some(ready)) = (harness.step(), harness.state.ready.clone()) else {
        return finish(
            harness,
            &Outcome::Incomplete("snapshot_without_ready".to_owned()),
        );
    };
    let validate = harness.spec.hooks.validate;
    let check = |pixels: &[u8]| {
        validate(&Snapshot {
            step: &step,
            ready: &ready,
            width,
            height,
            rgba: pixels,
        })
    };
    match (harness.spec.probe, step.frame) {
        (Probe::Blank, _) => {
            let verdict = check(&vec![0; rgba.len()]);
            probe_verdict(harness, &verdict, "fixture_pixels_missing", 20);
        }
        (Probe::Stale, "A") => {
            harness.state.stale_frame = Some(rgba);
            advance(harness);
        }
        (Probe::Stale, _) => match harness.state.stale_frame.take() {
            Some(previous) => {
                let verdict = check(&previous);
                probe_verdict(harness, &verdict, "fixture_frame_stale", 21);
            }
            None => finish(
                harness,
                &Outcome::Incomplete("stale_probe_without_a".to_owned()),
            ),
        },
        _ => match check(&rgba) {
            PixelVerdict::Accept(case) => accept(harness, surface, &step, case),
            PixelVerdict::Reject(reason) => finish(harness, &Outcome::Fail(reason)),
        },
    }
}

fn probe_verdict(harness: &mut Harness, verdict: &PixelVerdict, error: &'static str, exit: u8) {
    let probe = harness.spec.probe.token();
    let outcome = if *verdict == PixelVerdict::Reject(error.to_owned()) {
        let (snapshot_taken, validator_reached) = (true, true);
        Outcome::ProbeRejected(ProbeRejection {
            probe,
            error,
            snapshot_taken,
            validator_reached,
            exit,
        })
    } else {
        Outcome::Fail(format!("probe_{probe}_not_rejected {verdict:?}"))
    };
    finish(harness, &outcome);
}

// Publishes an accepted surface as a new PNG and records its case.
fn accept(harness: &mut Harness, surface: &cairo::ImageSurface, step: &StepSpec, mut case: Value) {
    let zoom = if step.zoom > 1.0 { "110" } else { "100" };
    let name = if step.frame == "control" {
        "control.png".to_owned()
    } else {
        format!("{}-zoom{zoom}-{}.png", step.renderer, step.frame)
    };
    let digest = match publish(harness, surface, &name) {
        Ok(digest) => digest,
        Err(reason) => return finish(harness, &Outcome::Fail(reason)),
    };
    if step.frame != "control" {
        let previous = harness.state.previous_digest.replace(digest.clone());
        if step.frame == "B" && previous.as_deref() == Some(digest.as_str()) {
            return finish(harness, &Outcome::Fail("frames_identical".to_owned()));
        }
    }
    case["png"] = json!(name);
    case["pngSha256"] = json!(digest);
    case["capturedUtc"] = json!(chrono::Utc::now().to_rfc3339());
    harness.state.cases.push(case.clone());
    if let Some(receipt) = harness.receipt.as_mut() {
        receipt.record_case(case);
    }
    advance(harness);
}

fn publish(
    harness: &mut Harness,
    surface: &cairo::ImageSurface,
    name: &str,
) -> Result<String, String> {
    let mut writer = BoundedWriter::new(MAX_PNG_BYTES);
    surface
        .write_to_png(&mut writer)
        .map_err(|error| format!("png_encode: {error}"))?;
    let receipt = harness.receipt.as_mut().ok_or("receipt_already_finished")?;
    let bytes = writer.into_bytes();
    receipt
        .publish_png(name, &bytes)
        .map_err(|error| format!("publish {name}: {error}"))
}

fn advance(harness: &mut Harness) {
    let next = harness.state.index.map_or(0, |index| index + 1);
    harness.state.index = Some(next);
    if next < harness.spec.steps.len() {
        return start_step(harness);
    }
    let outcome = if harness.spec.probe == Probe::None {
        (harness.spec.hooks.verdict)(&harness.state.cases)
    } else {
        Outcome::Fail("probe_not_rejected".to_owned())
    };
    finish(harness, &outcome);
}

fn on_timeout(shared: &Shared) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    harness.state.deadline = None;
    let all_loaded = harness.state.loaded.len() == harness.windows.len();
    let in_request = harness.state.gate.current().is_some();
    let waiting = if !all_loaded || !in_request {
        None
    } else if harness.state.gate.snapshot_pending() {
        Some("snapshot")
    } else if harness.state.ready.is_none() {
        Some("readiness")
    } else {
        None
    };
    let step = harness.step();
    let stage = json!({
        "loaded": harness.state.loaded,
        "stepIndex": harness.state.index,
        "stepLabel": step.map(|step| step.label),
        "waiting": waiting,
    });
    if let Some(receipt) = harness.receipt.as_mut() {
        receipt.set("timeoutStage", stage);
    }
    // The feasibility token is only for the current generation's readiness or
    // snapshot deadline after every own load finished; anything else is INCOMPLETE.
    let qualified = waiting.is_some() && harness.spec.off_workspace;
    let outcome = if qualified && harness.spec.probe == Probe::None {
        Outcome::Fail("off_workspace_frame_timeout".to_owned())
    } else {
        Outcome::Incomplete("deadline_30s".to_owned())
    };
    finish(&mut harness, &outcome);
}

fn on_web_process_terminated(shared: &Shared, label: &'static str) {
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    let outcome = Outcome::Incomplete(format!("web_process_terminated {label}"));
    finish(&mut harness, &outcome);
}

fn on_run_event(shared: &Shared, event: &RunEvent) {
    let RunEvent::WindowEvent {
        label,
        event: WindowEvent::Destroyed,
        ..
    } = event
    else {
        return;
    };
    let Ok(mut harness) = shared.try_borrow_mut() else {
        return;
    };
    if harness.windows.iter().any(|(name, _)| name == label) {
        finish(
            &mut harness,
            &Outcome::Fail(format!("window_closed {label}")),
        );
    }
}

// Completes once: cancels the request, drops own handlers/timer, writes the
// receipt, closes only this run's windows and ends the event loop.
fn finish(harness: &mut Harness, outcome: &Outcome) {
    if harness.state.exit.is_some() {
        return;
    }
    harness.state.gate.cancel();
    if let Some(cancellable) = harness.state.cancellable.take() {
        cancellable.cancel();
    }
    if let Some(source) = harness.state.deadline.take() {
        source.remove();
    }
    for native in harness.state.views.drain(..) {
        for handler in native.handlers {
            native.view.disconnect(handler);
        }
    }
    harness.state.stale_frame = None;
    let mut exit = outcome.exit();
    if let Some(mut receipt) = harness.receipt.take() {
        receipt.set("scriptExecuted", json!(harness.state.script_executed));
        receipt.set("snapshotTaken", json!(harness.state.snapshot_taken));
        exit = receipt.finish(outcome);
    }
    harness.state.exit = Some(exit);
    for (_, window) in &harness.windows {
        if let Err(error) = window.close() {
            eprintln!("linux_snapshot: close failed: {error}");
        }
    }
    harness.app.exit(i32::from(exit));
}
