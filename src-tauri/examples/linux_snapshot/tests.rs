use super::*;

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
// Ready JSON of a DPR 2 control step with the 1024x768 viewport.
const HIDPI_READY: &str = r#"{"generation":"g1","frame":"control","zoom":1.0,
"renderer":"default","scriptExecuted":true,"viewport":{"width":1024.0,"height":768.0},
"devicePixelRatio":2.0}"#;

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn ready(frame: &str) -> Ready {
    Ready {
        generation: "g1".to_owned(),
        frame: frame.to_owned(),
        zoom: 1.0,
        renderer: "default".to_owned(),
        viewport: Viewport {
            width: f64::from(WIDTH),
            height: f64::from(HEIGHT),
        },
        script_executed: true,
        device_pixel_ratio: 1.0,
        marker: None,
        rects: Some(Rects {
            canvas: rect(40.0, 40.0, 240.0, 160.0),
            overlay: rect(40.0, 40.0, 60.0, 40.0),
            terminal: rect(360.0, 40.0, 100.0, 20.0),
        }),
    }
}

fn paint(image: &mut Rgba, area: Rect, color: [u8; 3]) {
    for y in area.y as u32..(area.y + area.height) as u32 {
        for x in area.x as u32..(area.x + area.width) as u32 {
            let index = (y * image.width + x) as usize * 4;
            image.data[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

// Synthetic target frame: background, canvas, overlay above it, terminal block.
fn frame_image(frame: Frame) -> Rgba {
    let mut image = Rgba::blank(WIDTH, HEIGHT);
    let (canvas, terminal) = frame.colors().expect("target frame");
    paint(
        &mut image,
        rect(0.0, 0.0, f64::from(WIDTH), f64::from(HEIGHT)),
        [32, 32, 32],
    );
    paint(&mut image, rect(40.0, 40.0, 240.0, 160.0), canvas);
    paint(&mut image, rect(40.0, 40.0, 60.0, 40.0), Rgba::BLUE);
    paint(&mut image, rect(360.0, 40.0, 100.0, 20.0), terminal);
    image
}

// DPR 2 checker (2048x1536) with the measured 2-px seams at columns/rows 63, 64 mod 64.
fn hidpi_checker(phase: u32) -> Rgba {
    let mut image = Rgba::blank(2 * WIDTH, 2 * HEIGHT);
    for y in 0..image.height {
        for x in 0..image.width {
            let seam = |v: u32| v != 0 && matches!(v % 64, 63 | 0);
            let color = if seam(x) || seam(y) {
                [191, 0, 191]
            } else if (x / 64 + y / 64 + phase) % 2 == 0 {
                Rgba::MAGENTA
            } else {
                Rgba::BLACK
            };
            let index = (y * image.width + x) as usize * 4;
            image.data[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    image
}

#[test]
fn image_bounds() {
    assert!(check_bounds(1024, 768).is_ok());
    assert!(check_bounds(4096, 4096).is_ok());
    assert!(check_bounds(0, 768).is_err());
    assert!(check_bounds(1024, 0).is_err());
    assert!(check_bounds(4097, 10).is_err());
    assert!(check_bounds(10, 4097).is_err());
    assert!(
        Rgba::new(2, 2, vec![0; 15]).is_err(),
        "data length must match"
    );
    assert!(Rgba::new(2, 2, vec![0; 16]).is_ok());
}

#[test]
fn late_generation() {
    let mut gate = receipt::RequestGate::default();
    gate.begin("g1".to_owned());
    gate.begin("g2".to_owned());
    assert!(!gate.start_snapshot("g1"), "old generation cannot start");
    assert!(!gate.finish_snapshot("g1"), "late callback cannot publish");
    assert!(gate.start_snapshot("g2"));
    assert!(!gate.start_snapshot("g2"), "one snapshot in flight");
    assert!(gate.finish_snapshot("g2"));
    assert!(
        !gate.finish_snapshot("g2"),
        "duplicate callback cannot publish"
    );
    gate.begin("g3".to_owned());
    assert!(gate.start_snapshot("g3"));
    gate.cancel();
    assert!(
        !gate.finish_snapshot("g3"),
        "cancelled request drops its surface"
    );

    let step = &plan_steps(Probe::Blank)[0];
    assert!(check_ready(&ready("A"), step, "g1").is_ok());
    assert_eq!(
        check_ready(&ready("A"), step, "g2"),
        Err("ready_generation_mismatch".to_owned())
    );
}

#[test]
fn blank_rejected() {
    let ready_a = ready("A");
    assert_eq!(
        validate_target(&frame_image(Frame::A), &ready_a, Frame::A),
        Ok(())
    );
    assert_eq!(
        validate_target(&frame_image(Frame::B), &ready("B"), Frame::B),
        Ok(())
    );
    let blank = Rgba::blank(WIDTH, HEIGHT);
    assert_eq!(
        validate_target(&blank, &ready_a, Frame::A),
        Err(PixelError::Missing)
    );
    assert_eq!(PixelError::Missing.token(), "fixture_pixels_missing");
    assert!(
        !frame_image(Frame::A).shows_checker(ready_a.viewport),
        "target never shows the checker"
    );
}

#[test]
fn stale_rejected() {
    let previous = frame_image(Frame::A);
    assert_eq!(
        validate_target(&previous, &ready("B"), Frame::B),
        Err(PixelError::Stale)
    );
    assert_eq!(PixelError::Stale.token(), "fixture_frame_stale");

    let mut checker = Rgba::blank(WIDTH, HEIGHT);
    for row in 0..HEIGHT / 32 {
        for column in 0..WIDTH / 32 {
            let color = if (row + column) % 2 == 0 {
                Rgba::MAGENTA
            } else {
                Rgba::BLACK
            };
            let area = rect(f64::from(column * 32), f64::from(row * 32), 32.0, 32.0);
            paint(&mut checker, area, color);
        }
    }
    assert!(checker.shows_checker(ready("A").viewport));
    assert_eq!(
        validate_target(&checker, &ready("A"), Frame::A),
        Err(PixelError::Checker)
    );
}

#[test]
fn stall_probe_steps() {
    for probe in [Probe::StallModule, Probe::StallRaf] {
        let steps = plan_steps(probe);
        assert_eq!(steps.len(), 1);
        assert_eq!((steps[0].label, steps[0].frame), ("control", "control"));
        assert_eq!(steps[0], plan_steps(Probe::None)[0]);
    }
    for (name, probe) in Probe::ALL {
        assert_eq!(probe.token(), name);
    }
    let module = Probe::StallModule.init_script().unwrap_or_default();
    assert!(module.contains("ac4248Frame") && module.contains("ac4248Beacon"));
    let raf = Probe::StallRaf.init_script().unwrap_or_default();
    assert!(raf.contains("requestAnimationFrame") && raf.contains("DOMContentLoaded"));
    for probe in [Probe::None, Probe::Blank, Probe::Stale, Probe::Cancel] {
        assert_eq!(probe.init_script(), None);
    }
}

#[test]
fn checker_hidpi_accepted() {
    let viewport = ready("A").viewport;
    let image = hidpi_checker(0);
    let (mut total, mut either) = (0_u64, 0_u64);
    for y in (0..image.height).step_by(4) {
        for x in (0..image.width).step_by(4) {
            total += 1;
            let magenta = image.close_to(x, y, Rgba::MAGENTA);
            either += u64::from(magenta || image.close_to(x, y, Rgba::BLACK));
        }
    }
    let share = either as f64 / total as f64;
    assert!(share < 0.9, "old colour-count rule must fail here: {share}");
    assert!(image.shows_checker(viewport));

    let step = &plan_steps(Probe::None)[0];
    assert_eq!(step.frame, "control");
    let snapshot = Snapshot {
        step,
        ready: HIDPI_READY,
        width: image.width,
        height: image.height,
        rgba: &image.data,
    };
    let PixelVerdict::Accept(record) = validate(&snapshot) else {
        panic!("HiDPI control checker must be accepted");
    };
    assert_eq!(record["scale"], json!(2.0));
}

#[test]
fn checker_mutations_rejected() {
    let viewport = ready("A").viewport;
    let (width, height) = (2 * WIDTH, 2 * HEIGHT);
    let blank = Rgba::blank(width, height);
    assert!(!blank.shows_checker(viewport), "blank");
    assert!(!hidpi_checker(1).shows_checker(viewport), "inverted parity");
    let mut halves = Rgba::blank(width, height);
    paint(
        &mut halves,
        rect(0.0, 0.0, f64::from(WIDTH), f64::from(height)),
        Rgba::MAGENTA,
    );
    assert!(!halves.shows_checker(viewport), "two blocks");
    let mut magenta = Rgba::blank(width, height);
    paint(
        &mut magenta,
        rect(0.0, 0.0, f64::from(width), f64::from(height)),
        Rgba::MAGENTA,
    );
    assert!(!magenta.shows_checker(viewport), "all magenta");
    assert!(!frame_image(Frame::A).shows_checker(viewport), "frame A");
    assert!(!frame_image(Frame::B).shows_checker(viewport), "frame B");

    let repainted = |every: u32| {
        let mut image = hidpi_checker(0);
        for row in 0..HEIGHT / 32 {
            for column in 0..WIDTH / 32 {
                if (row * 32 + column) % every == 0 {
                    let area = rect(f64::from(column * 64), f64::from(row * 64), 64.0, 64.0);
                    paint(&mut image, area, [32, 32, 32]);
                }
            }
        }
        image
    };
    assert!(
        !repainted(7).shows_checker(viewport),
        "110 of 768 cells wrong"
    );
    assert!(
        repainted(20).shows_checker(viewport),
        "39 of 768 cells wrong"
    );

    let step = &plan_steps(Probe::None)[0];
    let snapshot = Snapshot {
        step,
        ready: HIDPI_READY,
        width,
        height,
        rgba: &blank.data,
    };
    assert_eq!(
        validate(&snapshot),
        PixelVerdict::Reject("control_checker_missing".to_owned())
    );
}
