use std::io::{self, Write};

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder};
use sha2::{Digest, Sha256};

use super::types::{MAX_DIMENSION, MAX_ENCODED_PNG_BYTES, MAX_PIXELS, MAX_RAW_RGBA_BYTES};
use super::WindowCaptureError;

/// An in-memory SDR frame. It is private to the capture module, so transport
/// code cannot inject pixels into the artifact pipeline.
pub(super) struct SdrFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

impl SdrFrame {
    pub(super) fn from_premultiplied_bgra(
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Result<Self, WindowCaptureError> {
        let expected_len = checked_rgba_len(width, height)?;
        if bgra.len() != expected_len {
            return Err(WindowCaptureError::CaptureTooLarge);
        }

        let mut rgba = Vec::with_capacity(expected_len);
        for pixel in bgra.chunks_exact(4) {
            let blue = pixel[0] as u32;
            let green = pixel[1] as u32;
            let red = pixel[2] as u32;
            let alpha = pixel[3] as u32;
            if alpha == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }

            let unpremultiply = |channel: u32| ((channel * 255 + alpha / 2) / alpha) as u8;
            rgba.extend_from_slice(&[
                unpremultiply(red),
                unpremultiply(green),
                unpremultiply(blue),
                alpha as u8,
            ]);
        }

        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

pub(super) struct EncodedPng {
    pub(super) bytes: Vec<u8>,
    pub(super) sha256: String,
}

pub(super) fn encode_png(frame: &SdrFrame) -> Result<EncodedPng, WindowCaptureError> {
    let mut writer = LimitedWriter::new(MAX_ENCODED_PNG_BYTES as usize);
    {
        let encoder = PngEncoder::new(&mut writer);
        if encoder
            .write_image(
                &frame.rgba,
                frame.width,
                frame.height,
                ColorType::Rgba8.into(),
            )
            .is_err()
        {
            return Err(if writer.exceeded_limit {
                WindowCaptureError::CaptureTooLarge
            } else {
                WindowCaptureError::EncodeFailed
            });
        }
    }
    if writer.exceeded_limit {
        return Err(WindowCaptureError::CaptureTooLarge);
    }

    let sha256 = format!("{:x}", Sha256::digest(&writer.bytes));
    Ok(EncodedPng {
        bytes: writer.bytes,
        sha256,
    })
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, WindowCaptureError> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || (width as u64).checked_mul(height as u64).is_none()
    {
        return Err(WindowCaptureError::CaptureTooLarge);
    }
    let pixels = (width as u64) * (height as u64);
    if pixels > MAX_PIXELS {
        return Err(WindowCaptureError::CaptureTooLarge);
    }
    let byte_len = pixels
        .checked_mul(4)
        .filter(|byte_len| *byte_len <= MAX_RAW_RGBA_BYTES)
        .ok_or(WindowCaptureError::CaptureTooLarge)?;
    usize::try_from(byte_len).map_err(|_| WindowCaptureError::CaptureTooLarge)
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded_limit: bool,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded_limit: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(new_len) = self.bytes.len().checked_add(buffer.len()) else {
            self.exceeded_limit = true;
            return Err(io::Error::other(
                "encoded PNG exceeds configured size limit",
            ));
        };
        if new_len > self.limit {
            self.exceeded_limit = true;
            return Err(io::Error::other(
                "encoded PNG exceeds configured size limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use image::ImageFormat;

    use super::{encode_png, SdrFrame};

    #[test]
    fn converts_premultiplied_bgra_without_leaking_alpha_zero_rgb() {
        let frame = SdrFrame::from_premultiplied_bgra(
            4,
            1,
            &[
                9, 8, 7, 0, // alpha zero
                0, 0, 1, 1, // opaque red after unpremultiplication
                63, 32, 16, 127, 30, 20, 10, 255,
            ],
        )
        .unwrap();

        assert_eq!(
            frame.rgba,
            vec![0, 0, 0, 0, 255, 0, 0, 1, 32, 64, 126, 127, 10, 20, 30, 255,]
        );
    }

    #[test]
    fn encodes_a_decodable_png_with_a_digest() {
        let frame = SdrFrame::from_premultiplied_bgra(1, 1, &[10, 20, 30, 255]).unwrap();
        let encoded = encode_png(&frame).unwrap();
        let decoded =
            image::load_from_memory_with_format(&encoded.bytes, ImageFormat::Png).unwrap();

        assert_eq!(decoded.width(), 1);
        assert_eq!(decoded.height(), 1);
        assert_eq!(encoded.sha256.len(), 64);
    }
}
