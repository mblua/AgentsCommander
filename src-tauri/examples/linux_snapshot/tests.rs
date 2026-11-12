use super::*;

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;

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
        !frame_image(Frame::A).shows_checker(),
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
    assert!(checker.shows_checker());
    assert_eq!(
        validate_target(&checker, &ready("A"), Frame::A),
        Err(PixelError::Checker)
    );
}
