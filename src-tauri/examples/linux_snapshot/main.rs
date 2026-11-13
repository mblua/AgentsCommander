// #4248 feasibility experiment: snapshot the isolated renderer fixture through the
// locked Tauri/WebKitGTK WebView on Linux. This file owns the CLI, composition and
// the portable validators; native.rs owns the GTK/WebKit harness. Example only: it
// is not an app module and changes no production behavior.
#[cfg(target_os = "linux")]
mod native;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod receipt;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::{json, Value};

use receipt::{Outcome, PixelVerdict, Probe, Readiness, Snapshot, StepSpec};

#[cfg(target_os = "linux")]
use receipt::Receipt;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    A,
    B,
    Control,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Frame {
    fn from_token(token: &str) -> Option<Self> {
        [Self::A, Self::B, Self::Control]
            .into_iter()
            .find(|frame| frame.token() == token)
    }

    fn token(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Control => "control",
        }
    }

    // Expected canvas and terminal-block colors of a target frame. xterm's
    // default ANSI palette gives red #cc0000 (SGR 41) and green #4e9a06 (SGR 42).
    fn colors(self) -> Option<([u8; 3], [u8; 3])> {
        match self {
            Self::A => Some(([0, 255, 255], [204, 0, 0])),
            Self::B => Some(([255, 255, 0], [78, 154, 6])),
            Self::Control => None,
        }
    }

    fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
            Self::Control => Self::Control,
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
struct Config {
    fixture: PathBuf,
    out: PathBuf,
    // Evidence label only; compositor observations establish real placement.
    placement: &'static str,
    probe: Probe,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_args(args: &[String]) -> Result<Config, String> {
    let (mut fixture, mut out, mut placement, mut probe) = (None, None, None, None);
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let duplicate = match flag.as_str() {
            "--fixture" => fixture.replace(PathBuf::from(value)).is_some(),
            "--out" => out.replace(PathBuf::from(value)).is_some(),
            "--placement" => {
                let label = ["visible", "off-workspace"]
                    .into_iter()
                    .find(|label| label == value);
                placement
                    .replace(label.ok_or("unknown --placement")?)
                    .is_some()
            }
            "--probe" => {
                let found = Probe::ALL.iter().find(|(name, _)| name == value);
                probe.replace(found.ok_or("unknown --probe")?.1).is_some()
            }
            other => return Err(format!("unknown argument {other}")),
        };
        if duplicate {
            return Err(format!("duplicate argument {flag}"));
        }
    }
    let (fixture, out) = (
        fixture.ok_or("missing --fixture")?,
        out.ok_or("missing --out")?,
    );
    check_paths(&fixture, &out)?;
    let placement = placement.ok_or("missing --placement")?;
    let probe = probe.unwrap_or(Probe::None);
    Ok(Config {
        fixture,
        out,
        placement,
        probe,
    })
}

// Absolute local index.html with readable sibling assets; a new absolute output.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn check_paths(fixture: &Path, out: &Path) -> Result<(), String> {
    let name = fixture.file_name().and_then(std::ffi::OsStr::to_str);
    if !fixture.is_absolute() || name != Some("index.html") {
        return Err("--fixture must be an absolute path to index.html".to_owned());
    }
    let unreadable = |error: std::io::Error| format!("fixture unreadable: {error}");
    std::fs::read(fixture).map_err(unreadable)?;
    let assets = fixture.with_file_name("assets");
    let mut count = 0_usize;
    for entry in std::fs::read_dir(assets).map_err(unreadable)? {
        std::fs::read(entry.map_err(unreadable)?.path()).map_err(unreadable)?;
        count += 1;
    }
    if count == 0 {
        return Err("fixture assets folder is empty".to_owned());
    }
    if !out.is_absolute() {
        return Err("--out must be an absolute path".to_owned());
    }
    match std::fs::symlink_metadata(out) {
        Ok(_) => Err("--out already exists; refusing to reuse it".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("--out not checkable: {error}")),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rects {
    canvas: Rect,
    overlay: Rect,
    terminal: Rect,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Viewport {
    width: f64,
    height: f64,
}

// The fixture's private "ac4248:" title protocol (P02): one ready JSON object.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ready {
    generation: String,
    frame: String,
    zoom: f64,
    renderer: String,
    #[serde(default)]
    marker: Option<String>,
    script_executed: bool,
    viewport: Viewport,
    device_pixel_ratio: f64,
    #[serde(default)]
    rects: Option<Rects>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq)]
enum TitleEvent {
    Ignored,
    Error {
        generation: Option<String>,
        message: String,
    },
    Ready(Box<Ready>, String),
    Invalid(String),
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_title(title: &str) -> TitleEvent {
    let Some(body) = title.strip_prefix("ac4248:") else {
        return TitleEvent::Ignored;
    };
    if body == "loading" {
        return TitleEvent::Ignored;
    }
    if let Some(error) = body.strip_prefix("error:") {
        let value: Value = serde_json::from_str(error).unwrap_or(Value::Null);
        let generation = value["generation"].as_str().map(str::to_owned);
        let message = value["message"].as_str().unwrap_or(error).to_owned();
        return TitleEvent::Error {
            generation,
            message,
        };
    }
    if body.len() > 8192 {
        return TitleEvent::Invalid("ready_title_too_long".to_owned());
    }
    match serde_json::from_str::<Ready>(body) {
        Ok(ready) if ready.script_executed => TitleEvent::Ready(Box::new(ready), body.to_owned()),
        Ok(_) => TitleEvent::Invalid("ready_without_script_executed".to_owned()),
        Err(error) => TitleEvent::Invalid(format!("ready_json: {error}")),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn plan_steps(probe: Probe) -> Vec<StepSpec> {
    let target = |frame, zoom, renderer| StepSpec {
        label: "target",
        frame,
        zoom,
        renderer,
    };
    let (a, b) = (target("A", 1.0, "default"), target("B", 1.0, "default"));
    match probe {
        Probe::Blank | Probe::Cancel => vec![a],
        Probe::Stale => vec![a, b],
        Probe::None => {
            let mut steps = vec![StepSpec {
                label: "control",
                frame: "control",
                ..a
            }];
            for renderer in ["default", "webgl"] {
                for zoom in [1.0, 1.1] {
                    steps.push(target("A", zoom, renderer));
                    steps.push(target("B", zoom, renderer));
                }
            }
            steps
        }
    }
}

// Every fixed field must match the request on the emitting handle before snapshot.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn check_ready(ready: &Ready, step: &StepSpec, generation: &str) -> Result<(), String> {
    if ready.generation != generation {
        return Err("ready_generation_mismatch".to_owned());
    }
    let zoom_ok = (ready.zoom - step.zoom).abs() < 1e-9;
    if ready.frame != step.frame || ready.renderer != step.renderer || !zoom_ok {
        return Err(format!(
            "ready_mismatch frame={} zoom={} renderer={}",
            ready.frame, ready.zoom, ready.renderer
        ));
    }
    if ready.viewport.width <= 0.0 || ready.viewport.height <= 0.0 {
        return Err("ready_viewport_empty".to_owned());
    }
    let control_fields = ready.marker.as_deref() == Some("AC4248_CONTROL") && ready.rects.is_none();
    let target_fields = ready.marker.is_none() && ready.rects.is_some();
    match Frame::from_token(step.frame) {
        Some(Frame::Control) if !control_fields => Err("control_ready_fields".to_owned()),
        Some(Frame::A | Frame::B) if !target_fields => Err("target_ready_fields".to_owned()),
        None => Err(format!("unknown step frame {}", step.frame)),
        _ => Ok(()),
    }
}

// Native hook: classify the emitting handle's title for the current request.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn readiness(title: &str, step: &StepSpec, generation: &str) -> Readiness {
    match parse_title(title) {
        TitleEvent::Ignored => Readiness::Pending,
        TitleEvent::Error {
            generation: Some(other),
            ..
        } if other != generation => Readiness::Pending,
        TitleEvent::Error { message, .. } => {
            Readiness::Incomplete(format!("fixture_error: {message}"))
        }
        TitleEvent::Invalid(reason) => Readiness::Incomplete(reason),
        TitleEvent::Ready(ready, _) if ready.generation != generation => Readiness::Pending,
        TitleEvent::Ready(ready, body) => match check_ready(&ready, step, generation) {
            Ok(()) => Readiness::Ready(body),
            Err(reason) => Readiness::Incomplete(reason),
        },
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn check_bounds(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("surface_empty".to_owned());
    }
    let pixels = u64::from(width) * u64::from(height);
    if width > 4096 || height > 4096 || pixels > 16 * 1024 * 1024 {
        return Err(format!("surface_too_large {width}x{height}"));
    }
    Ok(())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug)]
struct Rgba {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Rgba {
    const TOLERANCE: u8 = 12;
    const AGREEMENT: f64 = 0.9;
    const BLUE: [u8; 3] = [0, 0, 255];
    const MAGENTA: [u8; 3] = [255, 0, 255];
    const BLACK: [u8; 3] = [0, 0, 0];

    fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, String> {
        check_bounds(width, height)?;
        if data.len() as u64 != u64::from(width) * u64::from(height) * 4 {
            return Err("surface_data_length".to_owned());
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    #[cfg(test)]
    fn blank(width: u32, height: u32) -> Self {
        let data = vec![0; width as usize * height as usize * 4];
        Self {
            width,
            height,
            data,
        }
    }

    fn close_to(&self, x: u32, y: u32, color: [u8; 3]) -> bool {
        let index = (y as usize * self.width as usize + x as usize) * 4;
        let pixel = &self.data[index..index + 3];
        pixel
            .iter()
            .zip(color)
            .all(|(value, wanted)| value.abs_diff(wanted) <= Self::TOLERANCE)
    }

    // Central 20% of a CSS rect, mapped with the actual surface/viewport ratio.
    fn patch_agrees(&self, viewport: Viewport, rect: Rect, color: [u8; 3]) -> bool {
        let (width, height) = (f64::from(self.width), f64::from(self.height));
        let (scale_x, scale_y) = (width / viewport.width, height / viewport.height);
        let x = |share: f64| ((rect.x + rect.width * share) * scale_x).clamp(0.0, width);
        let y = |share: f64| ((rect.y + rect.height * share) * scale_y).clamp(0.0, height);
        let (mut total, mut matching) = (0_u64, 0_u64);
        for row in y(0.4) as u32..y(0.6) as u32 {
            for column in x(0.4) as u32..x(0.6) as u32 {
                total += 1;
                matching += u64::from(self.close_to(column, row, color));
            }
        }
        total > 0 && matching as f64 >= total as f64 * Self::AGREEMENT
    }

    // Control checker: mostly magenta and black, each a large share of the surface.
    fn shows_checker(&self) -> bool {
        let (mut total, mut magenta, mut black) = (0_u64, 0_u64, 0_u64);
        for y in (0..self.height).step_by(4) {
            for x in (0..self.width).step_by(4) {
                total += 1;
                magenta += u64::from(self.close_to(x, y, Self::MAGENTA));
                black += u64::from(self.close_to(x, y, Self::BLACK));
            }
        }
        let share = |count: u64| count as f64 / total.max(1) as f64;
        share(magenta) >= 0.25 && share(black) >= 0.25 && share(magenta + black) >= 0.9
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum PixelError {
    Missing,
    Stale,
    Checker,
    Mismatch(String),
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl PixelError {
    fn token(&self) -> String {
        match self {
            Self::Missing => "fixture_pixels_missing".to_owned(),
            Self::Stale => "fixture_frame_stale".to_owned(),
            Self::Checker => "target_shows_control_checker".to_owned(),
            Self::Mismatch(detail) => format!("fixture_pixels_mismatch {detail}"),
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn validate_target(image: &Rgba, ready: &Ready, frame: Frame) -> Result<(), PixelError> {
    let (Some(rects), Some((canvas, terminal))) = (ready.rects, frame.colors()) else {
        return Err(PixelError::Mismatch("missing_rects_or_frame".to_owned()));
    };
    if image.shows_checker() {
        return Err(PixelError::Checker);
    }
    let agrees = |rect, color| image.patch_agrees(ready.viewport, rect, color);
    let overlay_ok = agrees(rects.overlay, Rgba::BLUE);
    let canvas_ok = agrees(rects.canvas, canvas);
    let terminal_ok = agrees(rects.terminal, terminal);
    if overlay_ok && canvas_ok && terminal_ok {
        return Ok(());
    }
    let (other_canvas, other_terminal) = frame.other().colors().unwrap_or((canvas, terminal));
    let canvas_other = agrees(rects.canvas, other_canvas);
    let terminal_other = agrees(rects.terminal, other_terminal);
    if canvas_other && terminal_other {
        return Err(PixelError::Stale);
    }
    if !(overlay_ok || canvas_ok || terminal_ok || canvas_other || terminal_other) {
        return Err(PixelError::Missing);
    }
    let detail = format!("canvas={canvas_ok} overlay={overlay_ok} terminal={terminal_ok}");
    Err(PixelError::Mismatch(detail))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn within(ratio: f64, wanted: f64) -> bool {
    (ratio / wanted - 1.0).abs() <= 0.03
}

// Native hook: validate one captured surface and return its case record.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn validate(snapshot: &Snapshot<'_>) -> PixelVerdict {
    let ready: Ready = match serde_json::from_str(snapshot.ready) {
        Ok(ready) => ready,
        Err(error) => return PixelVerdict::Reject(format!("ready_json: {error}")),
    };
    let (width, height) = (snapshot.width, snapshot.height);
    let image = match Rgba::new(width, height, snapshot.rgba.to_vec()) {
        Ok(image) => image,
        Err(reason) => return PixelVerdict::Reject(reason),
    };
    let checked = match Frame::from_token(snapshot.step.frame) {
        Some(Frame::Control) if image.shows_checker() => Ok(()),
        Some(Frame::Control) => Err("control_checker_missing".to_owned()),
        Some(frame) => validate_target(&image, &ready, frame).map_err(|error| error.token()),
        None => Err(format!("unknown step frame {}", snapshot.step.frame)),
    };
    let ratio_x = f64::from(width) / ready.viewport.width;
    let ratio_y = f64::from(height) / ready.viewport.height;
    let checked = checked.and_then(|()| {
        let fits = within(ratio_x, ratio_y);
        fits.then_some(())
            .ok_or(format!("surface_not_viewport {ratio_x} {ratio_y}"))
    });
    if let Err(reason) = checked {
        return PixelVerdict::Reject(reason);
    }
    PixelVerdict::Accept(json!({
        "label": snapshot.step.label,
        "frame": ready.frame,
        "zoom": ready.zoom,
        "renderer": ready.renderer,
        "generation": ready.generation,
        "surface": { "width": width, "height": height },
        "viewport": { "width": ready.viewport.width, "height": ready.viewport.height },
        "scale": ratio_x,
        "devicePixelRatio": ready.device_pixel_ratio,
        "rects": snapshot_rects(&ready),
    }))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn snapshot_rects(ready: &Ready) -> Value {
    let rect = |r: Rect| json!({ "x": r.x, "y": r.y, "width": r.width, "height": r.height });
    ready.rects.map_or(Value::Null, |rects| {
        json!({
            "canvas": rect(rects.canvas),
            "overlay": rect(rects.overlay),
            "terminal": rect(rects.terminal),
        })
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Deserialize)]
struct Surface {
    width: f64,
    height: f64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, Deserialize)]
struct CaseRecord {
    frame: String,
    zoom: f64,
    renderer: String,
    surface: Surface,
    viewport: Viewport,
    rects: Option<Rects>,
}

// Native hook: native zoom must scale the surface/viewport ratio and every
// landmark by 1.1 for both renderers; missing metrics are incomplete.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn zoom_verdict(cases: &[Value]) -> Outcome {
    let records: Vec<CaseRecord> = cases
        .iter()
        .filter_map(|case| serde_json::from_value(case.clone()).ok())
        .collect();
    for renderer in ["default", "webgl"] {
        let find = |zoom: f64| {
            records.iter().find(|case| {
                let same_zoom = (case.zoom - zoom).abs() < 0.01;
                case.renderer == renderer && case.frame == "A" && same_zoom
            })
        };
        let (Some(base), Some(zoomed)) = (find(1.0), find(1.1)) else {
            return Outcome::Incomplete(format!("zoom_metrics_missing {renderer}"));
        };
        let ratio = |case: &CaseRecord| {
            let (surface, viewport) = (case.surface, case.viewport);
            (
                surface.width / viewport.width,
                surface.height / viewport.height,
            )
        };
        let ((base_x, base_y), (zoom_x, zoom_y)) = (ratio(base), ratio(zoomed));
        if (zoom_x / base_x - 1.1).abs() > 0.03 {
            return Outcome::Fail(format!("zoom_ratio {renderer} {}", zoom_x / base_x));
        }
        let (Some(before), Some(after)) = (base.rects, zoomed.rects) else {
            return Outcome::Incomplete(format!("zoom_rects_missing {renderer}"));
        };
        let pairs = [
            (before.canvas, after.canvas),
            (before.overlay, after.overlay),
            (before.terminal, after.terminal),
        ];
        for (old, new) in pairs {
            let width = (new.width * zoom_x) / (old.width * base_x);
            let height = (new.height * zoom_y) / (old.height * base_y);
            if !within(width, 1.1) || !within(height, 1.1) {
                return Outcome::Fail(format!("zoom_landmark {renderer} {width} {height}"));
            }
        }
    }
    Outcome::Pass
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("linux_snapshot: unsupported platform; this experiment runs only on Linux");
    ExitCode::from(2)
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse_args(&args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("linux_snapshot: incomplete: arguments: {error}");
            return ExitCode::from(2);
        }
    };
    let mut receipt = match Receipt::create(&config.out) {
        Ok(receipt) => receipt,
        Err(error) => {
            eprintln!("linux_snapshot: incomplete: output directory: {error}");
            return ExitCode::from(2);
        }
    };
    describe_run(&mut receipt, &config);
    let spec = native::RunSpec {
        probe: config.probe,
        off_workspace: config.placement == "off-workspace",
        steps: plan_steps(config.probe),
        hooks: native::Hooks {
            readiness,
            validate,
            verdict: zoom_verdict,
        },
    };
    ExitCode::from(native::run(receipt, spec))
}

#[cfg(target_os = "linux")]
fn describe_run(receipt: &mut Receipt, config: &Config) {
    let dirs = json!({
        "target": receipt.webkit_dir("target").display().to_string(),
        "control": receipt.webkit_dir("control").display().to_string(),
    });
    let differences = json!([
        "standalone fixture, not the app UI",
        "two windows (target, control) instead of one main window",
        "default window decorations instead of decorations(false)",
        "no real IPC, auth or session state",
        "PASS does not prove criterion 3",
    ]);
    receipt.set("startedUtc", json!(chrono::Utc::now().to_rfc3339()));
    receipt.set("fixture", json!(config.fixture.display().to_string()));
    receipt.set("embeddedEntry", json!("ac4248/index.html"));
    receipt.set("placementLabel", json!(config.placement));
    receipt.set("probe", json!(config.probe.token()));
    receipt.set("dataDirectories", dirs);
    receipt.set("differencesFromApp", differences);
}
