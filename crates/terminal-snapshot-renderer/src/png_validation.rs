use std::io::Cursor;

use crc32fast::Hasher;

use crate::protocol::{ProtocolError, TerminalSnapshotPngMetadata, MAX_PNG_BYTES, MAX_RGB_BYTES};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const DECODER_ALLOCATION_BUDGET: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedPng {
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub rgb_bytes: usize,
}

pub fn validate_png_for_metadata(
    bytes: &[u8],
    metadata: &TerminalSnapshotPngMetadata,
) -> Result<ValidatedPng, ProtocolError> {
    metadata.validate()?;
    if usize::try_from(metadata.png.bytes).ok() != Some(bytes.len()) {
        return Err(ProtocolError::InvalidPng);
    }
    validate_generated_png(bytes, metadata.png.pixel_width, metadata.png.pixel_height)
}

pub fn validate_generated_png(
    bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
) -> Result<ValidatedPng, ProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(PNG_SIGNATURE) {
        return Err(ProtocolError::InvalidPng);
    }
    let expected_pixels = usize::try_from(expected_width)
        .ok()
        .and_then(|width| {
            usize::try_from(expected_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(ProtocolError::InvalidPng)?;
    let expected_rgb = expected_pixels
        .checked_mul(3)
        .filter(|bytes| *bytes <= MAX_RGB_BYTES)
        .ok_or(ProtocolError::InvalidPng)?;

    let mut offset = PNG_SIGNATURE.len();
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    let mut saw_iend = false;
    let mut idat_ended = false;

    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(ProtocolError::InvalidPng)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(ProtocolError::InvalidPng)?;
        let length = usize::try_from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .map_err(|_| ProtocolError::InvalidPng)?;
        let kind = [header[4], header[5], header[6], header[7]];
        let data_start = header_end;
        let data_end = data_start
            .checked_add(length)
            .ok_or(ProtocolError::InvalidPng)?;
        let chunk_end = data_end.checked_add(4).ok_or(ProtocolError::InvalidPng)?;
        let data = bytes
            .get(data_start..data_end)
            .ok_or(ProtocolError::InvalidPng)?;
        let crc_bytes = bytes
            .get(data_end..chunk_end)
            .ok_or(ProtocolError::InvalidPng)?;

        let mut hasher = Hasher::new();
        hasher.update(&kind);
        hasher.update(data);
        let expected_crc =
            u32::from_be_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
        if hasher.finalize() != expected_crc {
            return Err(ProtocolError::InvalidPng);
        }

        match &kind {
            b"IHDR" => {
                if saw_ihdr || saw_idat || saw_iend || offset != PNG_SIGNATURE.len() || length != 13
                {
                    return Err(ProtocolError::InvalidPng);
                }
                let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                if width != expected_width
                    || height != expected_height
                    || data[8] != 8
                    || data[9] != 2
                    || data[10] != 0
                    || data[11] != 0
                    || data[12] != 0
                {
                    return Err(ProtocolError::InvalidPng);
                }
                saw_ihdr = true;
            }
            b"IDAT" => {
                if !saw_ihdr || saw_iend || idat_ended {
                    return Err(ProtocolError::InvalidPng);
                }
                saw_idat = true;
            }
            b"IEND" => {
                if !saw_ihdr || !saw_idat || saw_iend || length != 0 {
                    return Err(ProtocolError::InvalidPng);
                }
                saw_iend = true;
                if chunk_end != bytes.len() {
                    return Err(ProtocolError::InvalidPng);
                }
            }
            _ => return Err(ProtocolError::InvalidPng),
        }
        if saw_idat && kind != *b"IDAT" && kind != *b"IEND" {
            idat_ended = true;
        }
        offset = chunk_end;
    }

    if !saw_ihdr || !saw_idat || !saw_iend || offset != bytes.len() {
        return Err(ProtocolError::InvalidPng);
    }

    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: DECODER_ALLOCATION_BUDGET,
        },
    );
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(|_| ProtocolError::InvalidPng)?;
    let info = reader.info();
    if info.width != expected_width
        || info.height != expected_height
        || info.bit_depth != png::BitDepth::Eight
        || info.color_type != png::ColorType::Rgb
        || info.interlaced
    {
        return Err(ProtocolError::InvalidPng);
    }
    if reader.output_buffer_size() != Some(expected_rgb) {
        return Err(ProtocolError::InvalidPng);
    }
    let mut decoded = vec![0u8; expected_rgb];
    let frame = reader
        .next_frame(&mut decoded)
        .map_err(|_| ProtocolError::InvalidPng)?;
    if frame.width != expected_width
        || frame.height != expected_height
        || frame.bit_depth != png::BitDepth::Eight
        || frame.color_type != png::ColorType::Rgb
        || frame.buffer_size() != expected_rgb
    {
        return Err(ProtocolError::InvalidPng);
    }
    reader.finish().map_err(|_| ProtocolError::InvalidPng)?;

    Ok(ValidatedPng {
        width: expected_width,
        height: expected_height,
        bytes: bytes.len(),
        rgb_bytes: expected_rgb,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            encoder.set_filter(png::Filter::Sub);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3]).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn strict_generated_profile_accepts_minimal_encoder_output() {
        let bytes = tiny_png();
        let validated = validate_generated_png(&bytes, 1, 1).unwrap();
        assert_eq!(validated.rgb_bytes, 3);
    }

    #[test]
    fn crc_and_trailing_corruption_fail() {
        let mut bytes = tiny_png();
        bytes[20] ^= 1;
        assert!(validate_generated_png(&bytes, 1, 1).is_err());

        let mut bytes = tiny_png();
        bytes.write_all(b"x").unwrap();
        assert!(validate_generated_png(&bytes, 1, 1).is_err());
    }
}
