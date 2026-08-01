use std::io::Cursor;

use sha2::{Digest, Sha256};
use terminal_snapshot_renderer::{
    decode_bounded, render_png, validate_generated_png, TerminalScreenModel, FONT_SHA256,
    MAX_JSON_BYTES,
};

const STYLE_MODEL: &[u8] = include_bytes!("fixtures/style-grid-model.json");
const STYLE_PNG: &[u8] = include_bytes!("fixtures/style-grid.png");
const BLANK_MODEL: &[u8] = include_bytes!("fixtures/blank-cursor-model.json");
const BLANK_PNG: &[u8] = include_bytes!("fixtures/blank-cursor.png");

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn model(bytes: &[u8]) -> TerminalScreenModel {
    let model: TerminalScreenModel = decode_bounded(bytes, MAX_JSON_BYTES).unwrap();
    model.validate().unwrap();
    model
}

fn rgb(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().unwrap();
    assert_eq!(reader.info().color_type, png::ColorType::Rgb);
    assert_eq!(reader.info().bit_depth, png::BitDepth::Eight);
    assert!(!reader.info().interlaced);
    let width = reader.info().width;
    let height = reader.info().height;
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(frame.buffer_size());
    reader.finish().unwrap();
    (width, height, pixels)
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from((y * width + x) * 3).unwrap();
    pixels[offset..offset + 3].try_into().unwrap()
}

#[test]
fn embedded_assets_and_portable_png_hashes_are_pinned() {
    let font = include_bytes!("../assets/DejaVuSansMono.ttf");
    let license = include_bytes!("../assets/LICENSE-DejaVu.txt");
    assert_eq!(font.len(), 340_712);
    assert_eq!(sha256(font), FONT_SHA256);
    assert_eq!(
        sha256(license),
        "7a083b136e64d064794c3419751e5c7dd10d2f64c108fe5ba161eae5e5958a93"
    );
    assert_eq!(
        sha256(BLANK_PNG),
        "97bac516626c41f8253afd6958607943274a58785b7afd5ec2bb158707dbe06b"
    );
    assert_eq!(
        sha256(STYLE_PNG),
        "756915b2b24f0f092dbc0e171b9867c18f0756eb1fbfb8a43dc57103e83cfc05"
    );
}

#[test]
fn blank_cursor_golden_is_byte_exact_and_has_only_contract_pixels() {
    assert!(BLANK_MODEL.is_ascii());
    let model = model(BLANK_MODEL);
    let rendered = render_png(&model).unwrap();
    assert_eq!(rendered.bytes, BLANK_PNG);
    assert_eq!((rendered.pixel_width, rendered.pixel_height), (36, 36));
    validate_generated_png(BLANK_PNG, 36, 36).unwrap();

    let (width, height, pixels) = rgb(BLANK_PNG);
    assert_eq!((width, height), (36, 36));
    assert_eq!(pixel(&pixels, width, 0, 0), [0x0a, 0x0a, 0x0f]);
    assert_eq!(pixel(&pixels, width, 8, 8), [0x0a, 0x0a, 0x0f]);
    assert_eq!(pixel(&pixels, width, 18, 8), [0x00, 0xd4, 0xff]);
    assert_eq!(pixel(&pixels, width, 27, 27), [0x00, 0xd4, 0xff]);
    assert_eq!(pixel(&pixels, width, 28, 28), [0x0a, 0x0a, 0x0f]);
}

#[test]
fn style_wide_cursor_and_fallback_golden_is_byte_exact() {
    assert!(STYLE_MODEL.is_ascii());
    let model = model(STYLE_MODEL);
    let rendered = render_png(&model).unwrap();
    assert_eq!(rendered.bytes, STYLE_PNG);
    assert_eq!((rendered.pixel_width, rendered.pixel_height), (76, 56));
    assert_eq!(rendered.fallback_glyph_count, 2);
    validate_generated_png(STYLE_PNG, 76, 56).unwrap();

    let metadata = rendered.metadata(
        "22222222-2222-4222-8222-222222222222".to_string(),
        "project:wg-1-team/coordinator".to_string(),
        "project:wg-1-team/member".to_string(),
        &model,
    );
    metadata.validate().unwrap();
    assert_eq!(metadata.renderer.fallback_glyph_count, 2);
    assert_eq!(metadata.png.bytes, STYLE_PNG.len() as u64);

    let (width, height, pixels) = rgb(STYLE_PNG);
    assert_eq!((width, height), (76, 56));
    assert_eq!(pixel(&pixels, width, 18, 8), [0x33, 0x99, 0xff]);
    assert_eq!(pixel(&pixels, width, 58, 8), [0xff, 0x33, 0xcc]);
    assert_eq!(pixel(&pixels, width, 38, 25), [0xff, 0xdd, 0x66]);
    assert_eq!(pixel(&pixels, width, 48, 8), [0x33, 0xff, 0x99]);
    assert_eq!(pixel(&pixels, width, 8, 28), [0x33, 0xff, 0x99]);
    assert_eq!(pixel(&pixels, width, 19, 28), [0x00, 0xd4, 0xff]);
    assert_eq!(pixel(&pixels, width, 18, 28), [0x00, 0xcd, 0xf7]);
}
