use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use fontdue::{Font, FontSettings, Metrics};
use sha2::{Digest, Sha256};

use crate::json::CappedWriter;
use crate::png_validation::validate_generated_png;
use crate::protocol::{
    ProtocolError, TerminalCell, TerminalCellWidth, TerminalColor, TerminalPngInfo,
    TerminalPngScreenMetadata, TerminalRendererMetadata, TerminalScreenModel,
    TerminalSnapshotFormat, TerminalSnapshotPngMetadata, CELL_BASELINE_PX, CELL_HEIGHT_PX,
    CELL_WIDTH_PX, FONT_SHA256, FONT_SIZE_PX, MAX_GLYPH_MASK_BYTES, MAX_PNG_BYTES, PADDING_PX,
    SCHEMA_VERSION,
};

const FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");
const DEFAULT_FOREGROUND: Rgb = Rgb(0xe8, 0xe8, 0xe8);
const DEFAULT_BACKGROUND: Rgb = Rgb(0x0a, 0x0a, 0x0f);
const CURSOR_BACKGROUND: Rgb = Rgb(0x00, 0xd4, 0xff);
const CURSOR_TEXT: Rgb = Rgb(0x0a, 0x0a, 0x0f);

const ANSI_16: [Rgb; 16] = [
    Rgb(0x1a, 0x1a, 0x2e),
    Rgb(0xff, 0x3b, 0x5c),
    Rgb(0x33, 0xff, 0x99),
    Rgb(0xff, 0xcc, 0x33),
    Rgb(0x33, 0x99, 0xff),
    Rgb(0xff, 0x33, 0xcc),
    Rgb(0x33, 0xcc, 0xff),
    Rgb(0xe8, 0xe8, 0xe8),
    Rgb(0x4a, 0x4a, 0x5e),
    Rgb(0xff, 0x66, 0x99),
    Rgb(0x66, 0xff, 0xbb),
    Rgb(0xff, 0xdd, 0x66),
    Rgb(0x66, 0xbb, 0xff),
    Rgb(0xff, 0x66, 0xdd),
    Rgb(0x66, 0xdd, 0xff),
    Rgb(0xff, 0xff, 0xff),
];
const COLOR_CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum RenderError {
    #[error("terminal snapshot model is invalid")]
    InvalidModel,
    #[error("terminal snapshot render limit exceeded")]
    TooLarge,
    #[error("terminal snapshot font initialization failed")]
    Font,
    #[error("terminal snapshot renderer invariant failed")]
    Invariant,
    #[error("terminal snapshot PNG encoding failed")]
    Png,
}

impl From<ProtocolError> for RenderError {
    fn from(error: ProtocolError) -> Self {
        match error {
            ProtocolError::TooLarge => Self::TooLarge,
            ProtocolError::InvalidPng => Self::Png,
            ProtocolError::Invalid | ProtocolError::Serialization => Self::InvalidModel,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RenderedTerminalPng {
    pub bytes: Vec<u8>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub fallback_glyph_count: u64,
}

impl RenderedTerminalPng {
    pub fn metadata(
        &self,
        request_id: String,
        requester: String,
        target: String,
        model: &TerminalScreenModel,
    ) -> TerminalSnapshotPngMetadata {
        TerminalSnapshotPngMetadata {
            schema_version: SCHEMA_VERSION,
            request_id,
            captured_at: model.captured_at.clone(),
            requester,
            target,
            session: model.session.clone(),
            screen: TerminalPngScreenMetadata {
                dimensions: model.screen.dimensions,
                sequence: model.screen.sequence,
                active_buffer: model.screen.active_buffer,
                cursor: model.screen.cursor,
                parser_errors: model.screen.parser_errors,
            },
            fidelity: model.fidelity.clone(),
            format: TerminalSnapshotFormat::Png,
            png: TerminalPngInfo {
                bytes: self.bytes.len() as u64,
                pixel_width: self.pixel_width,
                pixel_height: self.pixel_height,
            },
            renderer: TerminalRendererMetadata::version_one(self.fallback_glyph_count),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn write(self, pixels: &mut [u8], offset: usize) -> Result<(), RenderError> {
        let destination = pixels
            .get_mut(offset..offset.checked_add(3).ok_or(RenderError::Invariant)?)
            .ok_or(RenderError::Invariant)?;
        destination.copy_from_slice(&[self.0, self.1, self.2]);
        Ok(())
    }

    fn blend(self, pixels: &mut [u8], offset: usize, alpha: u8) -> Result<(), RenderError> {
        let destination = pixels
            .get_mut(offset..offset.checked_add(3).ok_or(RenderError::Invariant)?)
            .ok_or(RenderError::Invariant)?;
        let inverse = 255u32 - u32::from(alpha);
        for (channel, source) in destination.iter_mut().zip([self.0, self.1, self.2]) {
            let blended =
                (u32::from(source) * u32::from(alpha) + u32::from(*channel) * inverse + 127) / 255;
            *channel = u8::try_from(blended).map_err(|_| RenderError::Invariant)?;
        }
        Ok(())
    }
}

static FONT: OnceLock<Result<Font, RenderError>> = OnceLock::new();

fn trusted_font() -> Result<&'static Font, RenderError> {
    match FONT.get_or_init(|| {
        if FONT_BYTES.len() != 340_712 {
            return Err(RenderError::Font);
        }
        let digest = Sha256::digest(FONT_BYTES);
        let mut rendered = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            if write!(&mut rendered, "{byte:02x}").is_err() {
                return Err(RenderError::Font);
            }
        }
        if rendered != FONT_SHA256 {
            return Err(RenderError::Font);
        }
        Font::from_bytes(
            FONT_BYTES,
            FontSettings {
                collection_index: 0,
                scale: FONT_SIZE_PX,
                load_substitutions: false,
            },
        )
        .map_err(|_| RenderError::Font)
    }) {
        Ok(font) => Ok(font),
        Err(error) => Err(*error),
    }
}

pub fn render_png(model: &TerminalScreenModel) -> Result<RenderedTerminalPng, RenderError> {
    match catch_unwind(AssertUnwindSafe(|| render_png_inner(model))) {
        Ok(result) => result,
        Err(_) => Err(RenderError::Invariant),
    }
}

fn render_png_inner(model: &TerminalScreenModel) -> Result<RenderedTerminalPng, RenderError> {
    model.validate()?;
    let image = model.screen.dimensions.checked_image_dimensions()?;
    let font = trusted_font()?;
    let mut pixels = vec![0u8; image.rgb_bytes];
    fill_image(
        &mut pixels,
        image.pixel_width,
        image.pixel_height,
        DEFAULT_BACKGROUND,
    )?;

    for (row, line) in model.screen.lines.iter().enumerate() {
        let mut column = 0usize;
        while column < line.cells.len() {
            let cell = &line.cells[column];
            let span_columns = if cell.width == TerminalCellWidth::WideLead {
                2usize
            } else {
                1usize
            };
            let (_, background) = resolved_colors(cell);
            fill_cell_span(
                &mut pixels,
                image.pixel_width,
                row,
                column,
                span_columns,
                background,
            )?;
            column = column
                .checked_add(span_columns)
                .ok_or(RenderError::Invariant)?;
        }
    }

    if model.screen.cursor.visible && model.screen.cursor.in_bounds {
        fill_cell_span(
            &mut pixels,
            image.pixel_width,
            usize::from(model.screen.cursor.row),
            usize::from(model.screen.cursor.column),
            1,
            CURSOR_BACKGROUND,
        )?;
    }

    let mut glyph_cache: HashMap<u16, (Metrics, Vec<u8>)> = HashMap::new();
    let mut mask_bytes = 0usize;
    let mut fallback_glyph_count = 0u64;
    let replacement_index = font.lookup_glyph_index('\u{fffd}');

    for (row, line) in model.screen.lines.iter().enumerate() {
        let mut column = 0usize;
        while column < line.cells.len() {
            let cell = &line.cells[column];
            let span_columns = if cell.width == TerminalCellWidth::WideLead {
                2usize
            } else {
                1usize
            };
            let (foreground, _) = resolved_colors(cell);
            for scalar in cell.text.chars() {
                let original_index = font.lookup_glyph_index(scalar);
                let missing = original_index == 0;
                let resolved_index = if missing {
                    fallback_glyph_count = fallback_glyph_count
                        .checked_add(1)
                        .ok_or(RenderError::Invariant)?;
                    replacement_index
                } else {
                    original_index
                };
                if resolved_index == 0 {
                    draw_hollow_fallback(
                        &mut pixels,
                        image.pixel_width,
                        row,
                        column,
                        span_columns,
                        foreground,
                        cell,
                        &model.screen.cursor,
                    )?;
                    continue;
                }

                if !glyph_cache.contains_key(&resolved_index) {
                    if glyph_cache.len() >= usize::from(font.glyph_count()) {
                        return Err(RenderError::Invariant);
                    }
                    let (metrics, mask) = font.rasterize_indexed(resolved_index, FONT_SIZE_PX);
                    let expected = metrics
                        .width
                        .checked_mul(metrics.height)
                        .ok_or(RenderError::Invariant)?;
                    if expected != mask.len() {
                        return Err(RenderError::Invariant);
                    }
                    mask_bytes = mask_bytes
                        .checked_add(mask.len())
                        .filter(|bytes| *bytes <= MAX_GLYPH_MASK_BYTES)
                        .ok_or(RenderError::TooLarge)?;
                    glyph_cache.insert(resolved_index, (metrics, mask));
                }
                let (metrics, mask) = glyph_cache
                    .get(&resolved_index)
                    .ok_or(RenderError::Invariant)?;
                draw_glyph(
                    &mut pixels,
                    image.pixel_width,
                    row,
                    column,
                    span_columns,
                    foreground,
                    cell,
                    &model.screen.cursor,
                    *metrics,
                    mask,
                )?;
            }
            if cell.style.underline {
                draw_underline(
                    &mut pixels,
                    image.pixel_width,
                    row,
                    column,
                    span_columns,
                    foreground,
                    &model.screen.cursor,
                )?;
            }
            column = column
                .checked_add(span_columns)
                .ok_or(RenderError::Invariant)?;
        }
    }

    drop(glyph_cache);
    let mut capped = CappedWriter::new(MAX_PNG_BYTES);
    {
        let mut encoder = png::Encoder::new(&mut capped, image.pixel_width, image.pixel_height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Sub);
        let mut writer = encoder.write_header().map_err(|_| RenderError::Png)?;
        writer
            .write_image_data(&pixels)
            .map_err(|_| RenderError::Png)?;
        writer.finish().map_err(|_| RenderError::Png)?;
    }
    drop(pixels);
    let bytes = capped.into_bytes().map_err(|error| match error {
        ProtocolError::TooLarge => RenderError::TooLarge,
        _ => RenderError::Png,
    })?;
    validate_generated_png(&bytes, image.pixel_width, image.pixel_height)
        .map_err(|_| RenderError::Png)?;
    Ok(RenderedTerminalPng {
        bytes,
        pixel_width: image.pixel_width,
        pixel_height: image.pixel_height,
        fallback_glyph_count,
    })
}

fn fill_image(pixels: &mut [u8], width: u32, height: u32, color: Rgb) -> Result<(), RenderError> {
    for y in 0..height {
        for x in 0..width {
            color.write(pixels, pixel_offset(width, x, y)?)?;
        }
    }
    Ok(())
}

fn fill_cell_span(
    pixels: &mut [u8],
    image_width: u32,
    row: usize,
    column: usize,
    span_columns: usize,
    color: Rgb,
) -> Result<(), RenderError> {
    let left = cell_left(column)?;
    let top = cell_top(row)?;
    let span = u32::try_from(span_columns)
        .ok()
        .and_then(|columns| columns.checked_mul(CELL_WIDTH_PX))
        .ok_or(RenderError::Invariant)?;
    let right = left.checked_add(span).ok_or(RenderError::Invariant)?;
    let bottom = top
        .checked_add(CELL_HEIGHT_PX)
        .ok_or(RenderError::Invariant)?;
    for y in top..bottom {
        for x in left..right {
            color.write(pixels, pixel_offset(image_width, x, y)?)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    pixels: &mut [u8],
    image_width: u32,
    row: usize,
    column: usize,
    span_columns: usize,
    foreground: Rgb,
    cell: &TerminalCell,
    cursor: &crate::protocol::TerminalCursor,
    metrics: Metrics,
    mask: &[u8],
) -> Result<(), RenderError> {
    let cell_left = i64::from(cell_left(column)?);
    let cell_top = i64::from(cell_top(row)?);
    let span_width = u32::try_from(span_columns)
        .ok()
        .and_then(|value| value.checked_mul(CELL_WIDTH_PX))
        .ok_or(RenderError::Invariant)?;
    if !metrics.advance_width.is_finite() {
        return Err(RenderError::Invariant);
    }
    let centered = ((span_width as f32 - metrics.advance_width) / 2.0).floor();
    if centered < i64::MIN as f32 || centered > i64::MAX as f32 {
        return Err(RenderError::Invariant);
    }
    let origin_x = cell_left
        .checked_add(centered as i64)
        .ok_or(RenderError::Invariant)?;
    let bitmap_left = origin_x
        .checked_add(i64::from(metrics.xmin))
        .ok_or(RenderError::Invariant)?;
    let bitmap_top = cell_top
        .checked_add(i64::from(CELL_BASELINE_PX))
        .and_then(|baseline| {
            i64::try_from(metrics.height)
                .ok()
                .and_then(|height| baseline.checked_sub(i64::from(metrics.ymin) + height))
        })
        .ok_or(RenderError::Invariant)?;
    let clip_left = cell_left;
    let clip_right = clip_left
        .checked_add(i64::from(span_width))
        .ok_or(RenderError::Invariant)?;
    let clip_top = cell_top;
    let clip_bottom = clip_top
        .checked_add(i64::from(CELL_HEIGHT_PX))
        .ok_or(RenderError::Invariant)?;

    for mask_y in 0..metrics.height {
        let base_y = bitmap_top
            .checked_add(i64::try_from(mask_y).map_err(|_| RenderError::Invariant)?)
            .ok_or(RenderError::Invariant)?;
        let local_y = base_y.checked_sub(cell_top).ok_or(RenderError::Invariant)?;
        let shear = if cell.style.italic {
            (19i64.checked_sub(local_y).ok_or(RenderError::Invariant)?) / 4
        } else {
            0
        };
        for mask_x in 0..metrics.width {
            let mask_offset = mask_y
                .checked_mul(metrics.width)
                .and_then(|value| value.checked_add(mask_x))
                .ok_or(RenderError::Invariant)?;
            let alpha = *mask.get(mask_offset).ok_or(RenderError::Invariant)?;
            if alpha == 0 {
                continue;
            }
            let base_x = bitmap_left
                .checked_add(i64::try_from(mask_x).map_err(|_| RenderError::Invariant)?)
                .and_then(|value| value.checked_add(shear))
                .ok_or(RenderError::Invariant)?;
            draw_mask_pixel(
                pixels,
                image_width,
                base_x,
                base_y,
                clip_left,
                clip_right,
                clip_top,
                clip_bottom,
                foreground,
                alpha,
                cursor,
            )?;
            if cell.style.bold {
                draw_mask_pixel(
                    pixels,
                    image_width,
                    base_x.checked_add(1).ok_or(RenderError::Invariant)?,
                    base_y,
                    clip_left,
                    clip_right,
                    clip_top,
                    clip_bottom,
                    foreground,
                    alpha,
                    cursor,
                )?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_mask_pixel(
    pixels: &mut [u8],
    image_width: u32,
    x: i64,
    y: i64,
    clip_left: i64,
    clip_right: i64,
    clip_top: i64,
    clip_bottom: i64,
    foreground: Rgb,
    alpha: u8,
    cursor: &crate::protocol::TerminalCursor,
) -> Result<(), RenderError> {
    if x < clip_left || x >= clip_right || y < clip_top || y >= clip_bottom || x < 0 || y < 0 {
        return Ok(());
    }
    let x = u32::try_from(x).map_err(|_| RenderError::Invariant)?;
    let y = u32::try_from(y).map_err(|_| RenderError::Invariant)?;
    let color = if is_cursor_pixel(x, y, cursor)? {
        CURSOR_TEXT
    } else {
        foreground
    };
    color.blend(pixels, pixel_offset(image_width, x, y)?, alpha)
}

#[allow(clippy::too_many_arguments)]
fn draw_hollow_fallback(
    pixels: &mut [u8],
    image_width: u32,
    row: usize,
    column: usize,
    span_columns: usize,
    foreground: Rgb,
    cell: &TerminalCell,
    cursor: &crate::protocol::TerminalCursor,
) -> Result<(), RenderError> {
    let span_width = u32::try_from(span_columns)
        .ok()
        .and_then(|value| value.checked_mul(CELL_WIDTH_PX))
        .ok_or(RenderError::Invariant)?;
    let left = cell_left(column)?
        .checked_add(span_width.saturating_sub(8) / 2)
        .ok_or(RenderError::Invariant)?;
    let top = cell_top(row)?
        .checked_add(3)
        .ok_or(RenderError::Invariant)?;
    for local_y in 0..14u32 {
        let shear = if cell.style.italic {
            (19u32.saturating_sub(local_y + 3)) / 4
        } else {
            0
        };
        for local_x in 0..8u32 {
            if local_x != 0 && local_x != 7 && local_y != 0 && local_y != 13 {
                continue;
            }
            for bold_offset in 0..=u32::from(cell.style.bold) {
                let x = left
                    .checked_add(local_x)
                    .and_then(|value| value.checked_add(shear))
                    .and_then(|value| value.checked_add(bold_offset))
                    .ok_or(RenderError::Invariant)?;
                let y = top.checked_add(local_y).ok_or(RenderError::Invariant)?;
                let clip_right = cell_left(column)?
                    .checked_add(span_width)
                    .ok_or(RenderError::Invariant)?;
                if x >= clip_right {
                    continue;
                }
                let color = if is_cursor_pixel(x, y, cursor)? {
                    CURSOR_TEXT
                } else {
                    foreground
                };
                color.write(pixels, pixel_offset(image_width, x, y)?)?;
            }
        }
    }
    Ok(())
}

fn draw_underline(
    pixels: &mut [u8],
    image_width: u32,
    row: usize,
    column: usize,
    span_columns: usize,
    foreground: Rgb,
    cursor: &crate::protocol::TerminalCursor,
) -> Result<(), RenderError> {
    let left = cell_left(column)?;
    let span_width = u32::try_from(span_columns)
        .ok()
        .and_then(|value| value.checked_mul(CELL_WIDTH_PX))
        .ok_or(RenderError::Invariant)?;
    let right = left.checked_add(span_width).ok_or(RenderError::Invariant)?;
    let y = cell_top(row)?
        .checked_add(17)
        .ok_or(RenderError::Invariant)?;
    for x in left..right {
        let color = if is_cursor_pixel(x, y, cursor)? {
            CURSOR_TEXT
        } else {
            foreground
        };
        color.write(pixels, pixel_offset(image_width, x, y)?)?;
    }
    Ok(())
}

fn is_cursor_pixel(
    x: u32,
    y: u32,
    cursor: &crate::protocol::TerminalCursor,
) -> Result<bool, RenderError> {
    if !cursor.visible || !cursor.in_bounds {
        return Ok(false);
    }
    let left = PADDING_PX
        .checked_add(
            u32::from(cursor.column)
                .checked_mul(CELL_WIDTH_PX)
                .ok_or(RenderError::Invariant)?,
        )
        .ok_or(RenderError::Invariant)?;
    let top = PADDING_PX
        .checked_add(
            u32::from(cursor.row)
                .checked_mul(CELL_HEIGHT_PX)
                .ok_or(RenderError::Invariant)?,
        )
        .ok_or(RenderError::Invariant)?;
    Ok(x >= left
        && x < left
            .checked_add(CELL_WIDTH_PX)
            .ok_or(RenderError::Invariant)?
        && y >= top
        && y < top
            .checked_add(CELL_HEIGHT_PX)
            .ok_or(RenderError::Invariant)?)
}

fn cell_left(column: usize) -> Result<u32, RenderError> {
    u32::try_from(column)
        .ok()
        .and_then(|column| column.checked_mul(CELL_WIDTH_PX))
        .and_then(|value| value.checked_add(PADDING_PX))
        .ok_or(RenderError::Invariant)
}

fn cell_top(row: usize) -> Result<u32, RenderError> {
    u32::try_from(row)
        .ok()
        .and_then(|row| row.checked_mul(CELL_HEIGHT_PX))
        .and_then(|value| value.checked_add(PADDING_PX))
        .ok_or(RenderError::Invariant)
}

fn pixel_offset(width: u32, x: u32, y: u32) -> Result<usize, RenderError> {
    usize::try_from(y)
        .ok()
        .and_then(|y| {
            usize::try_from(width)
                .ok()
                .and_then(|width| y.checked_mul(width))
        })
        .and_then(|row| usize::try_from(x).ok().and_then(|x| row.checked_add(x)))
        .and_then(|pixel| pixel.checked_mul(3))
        .ok_or(RenderError::Invariant)
}

fn resolved_colors(cell: &TerminalCell) -> (Rgb, Rgb) {
    let mut foreground = resolve_color(&cell.foreground, DEFAULT_FOREGROUND);
    let mut background = resolve_color(&cell.background, DEFAULT_BACKGROUND);
    if cell.style.inverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    (foreground, background)
}

fn resolve_color(color: &TerminalColor, default: Rgb) -> Rgb {
    match *color {
        TerminalColor::Default => default,
        TerminalColor::Rgb { red, green, blue } => Rgb(red, green, blue),
        TerminalColor::Indexed { index } => indexed_color(index),
    }
}

fn indexed_color(index: u8) -> Rgb {
    match index {
        0..=15 => ANSI_16[usize::from(index)],
        16..=231 => {
            let offset = index - 16;
            let red = COLOR_CUBE_LEVELS[usize::from(offset / 36)];
            let green = COLOR_CUBE_LEVELS[usize::from((offset % 36) / 6)];
            let blue = COLOR_CUBE_LEVELS[usize::from(offset % 6)];
            Rgb(red, green, blue)
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);
            Rgb(level, level, level)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::{
        canonical_timestamp, TerminalActiveBuffer, TerminalBackendKind, TerminalCellStyle,
        TerminalCursor, TerminalDimensions, TerminalLine, TerminalScreen, TerminalSnapshotFidelity,
        TerminalSnapshotSession,
    };

    use super::*;

    fn model(cell: TerminalCell) -> TerminalScreenModel {
        TerminalScreenModel {
            captured_at: canonical_timestamp(chrono::Utc::now()),
            session: TerminalSnapshotSession {
                id: uuid::Uuid::new_v4().to_string(),
                backend: TerminalBackendKind::LocalProcess,
            },
            screen: TerminalScreen {
                dimensions: TerminalDimensions {
                    rows: 1,
                    columns: 1,
                },
                sequence: 0,
                active_buffer: TerminalActiveBuffer::Normal,
                cursor: TerminalCursor {
                    row: 0,
                    column: 0,
                    visible: false,
                    in_bounds: true,
                },
                parser_errors: 0,
                lines: vec![TerminalLine {
                    wrapped: false,
                    cells: vec![cell],
                }],
            },
            fidelity: TerminalSnapshotFidelity::version_one(false),
        }
    }

    #[test]
    fn font_asset_is_exact_and_render_is_deterministic() {
        let model = model(TerminalCell {
            text: "A".to_string(),
            width: TerminalCellWidth::Narrow,
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            style: TerminalCellStyle::default(),
        });
        let first = render_png(&model).unwrap();
        let second = render_png(&model).unwrap();
        assert!(first == second);
        assert_eq!((first.pixel_width, first.pixel_height), (26, 36));
        assert_eq!(FONT_BYTES.len(), 340_712);
    }

    #[test]
    fn palette_contract_is_exact() {
        assert_eq!(indexed_color(0), Rgb(0x1a, 0x1a, 0x2e));
        assert_eq!(indexed_color(16), Rgb(0, 0, 0));
        assert_eq!(indexed_color(231), Rgb(255, 255, 255));
        assert_eq!(indexed_color(232), Rgb(8, 8, 8));
        assert_eq!(indexed_color(255), Rgb(238, 238, 238));
    }
}
