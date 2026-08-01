use std::collections::HashSet;
use std::io::{self, Write};

use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter};

use crate::protocol::{
    ProtocolError, TerminalSnapshotApiError, TerminalSnapshotApiSuccess, TerminalSnapshotFormat,
    TerminalSnapshotHostResponse, TerminalSnapshotReasonCode, TerminalSnapshotResult, API_VERSION,
    MAX_BASE64_TEXT_BYTES, MAX_JSON_DEPTH, MAX_PNG_BYTES,
};

pub struct CappedWriter {
    bytes: Vec<u8>,
    cap: usize,
    exceeded: bool,
}

impl CappedWriter {
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
            exceeded: false,
        }
    }

    pub fn into_bytes(self) -> Result<Vec<u8>, ProtocolError> {
        if self.exceeded {
            Err(ProtocolError::TooLarge)
        } else {
            Ok(self.bytes)
        }
    }
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.cap.saturating_sub(self.bytes.len());
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "terminal snapshot cap reached",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub struct AsciiSafeFormatter;

impl Formatter for AsciiSafeFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        for scalar in fragment.chars() {
            match scalar {
                '\u{20}'..='\u{7e}' => {
                    let mut bytes = [0u8; 4];
                    writer.write_all(scalar.encode_utf8(&mut bytes).as_bytes())?;
                }
                value if (value as u32) <= 0xffff => {
                    write_u16_escape(writer, value as u16)?;
                }
                value => {
                    let code = value as u32 - 0x1_0000;
                    let high = 0xd800 | ((code >> 10) as u16);
                    let low = 0xdc00 | ((code & 0x3ff) as u16);
                    write_u16_escape(writer, high)?;
                    write_u16_escape(writer, low)?;
                }
            }
        }
        Ok(())
    }

    fn write_char_escape<W>(&mut self, writer: &mut W, char_escape: CharEscape) -> io::Result<()>
    where
        W: ?Sized + Write,
    {
        match char_escape {
            CharEscape::Quote => writer.write_all(b"\\\""),
            CharEscape::ReverseSolidus => writer.write_all(b"\\\\"),
            CharEscape::Solidus => writer.write_all(b"/"),
            CharEscape::Backspace => write_u16_escape(writer, 0x0008),
            CharEscape::FormFeed => write_u16_escape(writer, 0x000c),
            CharEscape::LineFeed => write_u16_escape(writer, 0x000a),
            CharEscape::CarriageReturn => write_u16_escape(writer, 0x000d),
            CharEscape::Tab => write_u16_escape(writer, 0x0009),
            CharEscape::AsciiControl(byte) => write_u16_escape(writer, u16::from(byte)),
        }
    }
}

fn write_u16_escape<W: ?Sized + Write>(writer: &mut W, value: u16) -> io::Result<()> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    writer.write_all(&[
        b'\\',
        b'u',
        HEX[((value >> 12) & 0xf) as usize],
        HEX[((value >> 8) & 0xf) as usize],
        HEX[((value >> 4) & 0xf) as usize],
        HEX[(value & 0xf) as usize],
    ])
}

pub fn to_ascii_json<T: Serialize>(value: &T, cap: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = CappedWriter::new(cap);
    let formatter = AsciiSafeFormatter;
    let mut serializer = serde_json::Serializer::with_formatter(&mut writer, formatter);
    if value.serialize(&mut serializer).is_err() {
        return if writer.exceeded {
            Err(ProtocolError::TooLarge)
        } else {
            Err(ProtocolError::Serialization)
        };
    }
    writer.into_bytes()
}

pub fn to_ascii_json_line<T: Serialize>(value: &T, cap: usize) -> Result<Vec<u8>, ProtocolError> {
    let payload_cap = cap.checked_sub(1).ok_or(ProtocolError::TooLarge)?;
    let mut bytes = to_ascii_json(value, payload_cap)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn decode_bounded<T: DeserializeOwned>(input: &[u8], cap: usize) -> Result<T, ProtocolError> {
    if input.is_empty() || input.len() > cap {
        return Err(ProtocolError::TooLarge);
    }
    JsonShapeScanner::new(input).scan()?;
    serde_json::from_slice(input).map_err(|_| ProtocolError::Invalid)
}

pub fn decode_api_success(
    input: &[u8],
    expected_request_id: &str,
    expected_target: &str,
    expected_format: TerminalSnapshotFormat,
) -> Result<TerminalSnapshotApiSuccess, ProtocolError> {
    let envelope: TerminalSnapshotApiSuccess =
        decode_bounded(input, crate::protocol::MAX_TRANSPORT_BYTES)?;
    if envelope.api_version != API_VERSION {
        return Err(ProtocolError::Invalid);
    }
    validate_result_correlation(
        &envelope.result,
        expected_request_id,
        expected_target,
        expected_format,
        false,
    )?;
    Ok(envelope)
}

pub fn decode_api_error(
    input: &[u8],
    status: u16,
) -> Result<TerminalSnapshotApiError, ProtocolError> {
    let envelope: TerminalSnapshotApiError =
        decode_bounded(input, crate::protocol::MAX_ERROR_BYTES)?;
    envelope.validate()?;
    if envelope.error.http_status() != Some(status) {
        return Err(ProtocolError::Invalid);
    }
    Ok(envelope)
}

pub fn decode_host_response(
    input: &[u8],
    expected_request_id: &str,
    expected_confirmation_tag: &str,
    expected_target: &str,
    expected_format: TerminalSnapshotFormat,
) -> Result<TerminalSnapshotHostResponse, ProtocolError> {
    let envelope: TerminalSnapshotHostResponse =
        decode_bounded(input, crate::protocol::MAX_TRANSPORT_BYTES)?;
    envelope.validate_shape()?;
    if envelope.request_id != expected_request_id
        || envelope.confirmation_tag != expected_confirmation_tag
    {
        return Err(ProtocolError::Invalid);
    }
    if let Some(result) = &envelope.result {
        validate_result_correlation(
            result,
            expected_request_id,
            expected_target,
            expected_format,
            true,
        )?;
    }
    Ok(envelope)
}

pub fn validate_result_correlation(
    result: &TerminalSnapshotResult,
    expected_request_id: &str,
    expected_target: &str,
    expected_format: TerminalSnapshotFormat,
    allow_root_requester: bool,
) -> Result<(), ProtocolError> {
    if result.format() != expected_format
        || result.request_id() != expected_request_id
        || result.target() != expected_target
    {
        return Err(ProtocolError::Invalid);
    }
    crate::protocol::validate_requester_identity(result.requester(), allow_root_requester)?;
    match result {
        TerminalSnapshotResult::Json { snapshot } => snapshot.validate(),
        TerminalSnapshotResult::Png {
            metadata,
            png_base64,
        } => {
            metadata.validate()?;
            if png_base64.len() > MAX_BASE64_TEXT_BYTES {
                return Err(ProtocolError::TooLarge);
            }
            Ok(())
        }
    }
}

pub fn decode_canonical_base64_png(text: &str) -> Result<Vec<u8>, ProtocolError> {
    if text.is_empty() || text.len() > MAX_BASE64_TEXT_BYTES || text.contains('\\') {
        return Err(ProtocolError::Invalid);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|_| ProtocolError::Invalid)?;
    if decoded.is_empty() || decoded.len() > MAX_PNG_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    if base64::engine::general_purpose::STANDARD.encode(&decoded) != text {
        return Err(ProtocolError::Invalid);
    }
    Ok(decoded)
}

pub fn encode_canonical_base64(bytes: &[u8]) -> Result<String, ProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_PNG_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    if encoded.len() > MAX_BASE64_TEXT_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    Ok(encoded)
}

pub fn validate_error_contract(
    code: TerminalSnapshotReasonCode,
    detail: &str,
) -> Result<(), ProtocolError> {
    if detail == code.detail() {
        Ok(())
    } else {
        Err(ProtocolError::Invalid)
    }
}

struct JsonShapeScanner<'a> {
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
    cells: usize,
}

impl<'a> JsonShapeScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            nodes: 0,
            cells: 0,
        }
    }

    fn scan(mut self) -> Result<(), ProtocolError> {
        self.skip_whitespace();
        self.parse_value(1, None)?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            return Err(ProtocolError::Invalid);
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize, field_name: Option<&str>) -> Result<(), ProtocolError> {
        if depth > MAX_JSON_DEPTH {
            return Err(ProtocolError::TooLarge);
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .filter(|nodes| *nodes <= 1_000_000)
            .ok_or(ProtocolError::TooLarge)?;
        self.skip_whitespace();
        match self.peek().ok_or(ProtocolError::Invalid)? {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth, field_name),
            b'"' => {
                let require_unescaped = field_name == Some("pngBase64");
                let start = self.position;
                let length = self.parse_string(require_unescaped, false)?;
                if require_unescaped && length > MAX_BASE64_TEXT_BYTES {
                    return Err(ProtocolError::TooLarge);
                }
                if field_name == Some("text") {
                    if length > 144 {
                        return Err(ProtocolError::TooLarge);
                    }
                    let raw = self
                        .bytes
                        .get(start..self.position)
                        .ok_or(ProtocolError::Invalid)?;
                    let text: String =
                        serde_json::from_slice(raw).map_err(|_| ProtocolError::Invalid)?;
                    if text.len() > 24 || text.chars().count() > 6 {
                        return Err(ProtocolError::TooLarge);
                    }
                }
                Ok(())
            }
            b't' => self.consume_literal(b"true"),
            b'f' => self.consume_literal(b"false"),
            b'n' => self.consume_literal(b"null"),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(ProtocolError::Invalid),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<(), ProtocolError> {
        self.expect(b'{')?;
        self.skip_whitespace();
        if self.consume_if(b'}') {
            return Ok(());
        }
        let mut keys = HashSet::new();
        loop {
            self.skip_whitespace();
            let start = self.position;
            self.parse_string(false, true)?;
            let raw = self
                .bytes
                .get(start..self.position)
                .ok_or(ProtocolError::Invalid)?;
            let key: String = serde_json::from_slice(raw).map_err(|_| ProtocolError::Invalid)?;
            if key.len() > 32 * 1024 || !keys.insert(key.clone()) {
                return Err(if key.len() > 32 * 1024 {
                    ProtocolError::TooLarge
                } else {
                    ProtocolError::Invalid
                });
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.parse_value(depth + 1, Some(&key))?;
            self.skip_whitespace();
            if self.consume_if(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn parse_array(&mut self, depth: usize, field_name: Option<&str>) -> Result<(), ProtocolError> {
        self.expect(b'[')?;
        self.skip_whitespace();
        if self.consume_if(b']') {
            return Ok(());
        }
        let cap = match field_name {
            Some("lines") => Some(crate::protocol::MAX_ROWS as usize),
            Some("cells") => Some(crate::protocol::MAX_COLUMNS as usize),
            Some("omitted") => Some(crate::protocol::FIDELITY_OMITTED.len()),
            Some("unsupported") => Some(crate::protocol::FIDELITY_UNSUPPORTED.len()),
            _ => None,
        };
        let mut elements = 0usize;
        loop {
            self.parse_value(depth + 1, None)?;
            elements = elements.checked_add(1).ok_or(ProtocolError::TooLarge)?;
            if cap.is_some_and(|cap| elements > cap) {
                return Err(ProtocolError::TooLarge);
            }
            if field_name == Some("cells") {
                self.cells = self
                    .cells
                    .checked_add(1)
                    .filter(|cells| *cells <= crate::protocol::MAX_CELLS)
                    .ok_or(ProtocolError::TooLarge)?;
            }
            self.skip_whitespace();
            if self.consume_if(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn parse_string(&mut self, reject_escape: bool, is_key: bool) -> Result<usize, ProtocolError> {
        self.expect(b'"')?;
        let content_start = self.position;
        loop {
            let byte = self.take().ok_or(ProtocolError::Invalid)?;
            match byte {
                b'"' => return Ok(self.position - content_start - 1),
                0x00..=0x1f => return Err(ProtocolError::Invalid),
                b'\\' => {
                    if reject_escape {
                        return Err(ProtocolError::Invalid);
                    }
                    let escaped = self.take().ok_or(ProtocolError::Invalid)?;
                    match escaped {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            for _ in 0..4 {
                                if !self.take().is_some_and(|value| value.is_ascii_hexdigit()) {
                                    return Err(ProtocolError::Invalid);
                                }
                            }
                        }
                        _ => return Err(ProtocolError::Invalid),
                    }
                }
                _ => {}
            }
            if is_key && self.position.saturating_sub(content_start) > 128 * 1024 {
                return Err(ProtocolError::TooLarge);
            }
        }
    }

    fn parse_number(&mut self) -> Result<(), ProtocolError> {
        let start = self.position;
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.position += 1;
        }
        let raw = self
            .bytes
            .get(start..self.position)
            .ok_or(ProtocolError::Invalid)?;
        serde_json::from_slice::<serde_json::Number>(raw)
            .map(|_| ())
            .map_err(|_| ProtocolError::Invalid)
    }

    fn consume_literal(&mut self, literal: &[u8]) -> Result<(), ProtocolError> {
        if self.bytes.get(self.position..self.position + literal.len()) == Some(literal) {
            self.position += literal.len();
            Ok(())
        } else {
            Err(ProtocolError::Invalid)
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ProtocolError> {
        if self.consume_if(expected) {
            Ok(())
        } else {
            Err(ProtocolError::Invalid)
        }
    }

    fn consume_if(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn take(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.position += 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formatter_is_compact_ascii_and_uses_long_control_escapes() {
        let value = json!({"text": "a\n\u{7f}é😀\\\""});
        let bytes = to_ascii_json(&value, 1024).unwrap();
        assert!(bytes.is_ascii());
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"text":"a\u000a\u007f\u00e9\ud83d\ude00\\\""}"#
        );
    }

    #[test]
    fn scanner_rejects_duplicate_keys_at_any_depth() {
        assert!(decode_bounded::<serde_json::Value>(br#"{"a":{"x":1,"x":2}}"#, 1024).is_err());
    }

    #[test]
    fn scanner_rejects_escaped_base64_field() {
        let input = br#"{"pngBase64":"YWJj\u003d"}"#;
        assert!(decode_bounded::<serde_json::Value>(input, 1024).is_err());
    }

    #[test]
    fn canonical_base64_round_trips() {
        let encoded = encode_canonical_base64(b"png").unwrap();
        assert_eq!(decode_canonical_base64_png(&encoded).unwrap(), b"png");
        assert!(decode_canonical_base64_png("cG5n===").is_err());
    }
}
