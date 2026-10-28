mod shape;

use shape::JsonShapeScanner;

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::ser::{CharEscape, Formatter};

use crate::protocol::{
    validate_requester_identity, validate_uuid, validate_wg_fqn, ProtocolError, TerminalScreen,
    TerminalScreenModel, TerminalSnapshotApiError, TerminalSnapshotApiSuccess,
    TerminalSnapshotDocument, TerminalSnapshotFidelity, TerminalSnapshotFormat,
    TerminalSnapshotHostResponse, TerminalSnapshotPayload, TerminalSnapshotPngMetadata,
    TerminalSnapshotReasonCode, TerminalSnapshotSession, API_VERSION, MAX_BASE64_TEXT_BYTES,
    MAX_JSON_BYTES, MAX_PNG_BYTES, SCHEMA_VERSION,
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
        let required = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|required| *required <= self.cap)
            .ok_or_else(|| {
                self.exceeded = true;
                io::Error::new(io::ErrorKind::WriteZero, "terminal snapshot cap reached")
            })?;
        if required > self.bytes.capacity() {
            let doubled = self.bytes.capacity().max(1_024).saturating_mul(2);
            let target = required.max(doubled.min(self.cap));
            self.bytes
                .try_reserve_exact(target.saturating_sub(self.bytes.len()))
                .map_err(|_| {
                    self.exceeded = true;
                    io::Error::other("terminal snapshot allocation failed")
                })?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BorrowedApiSuccess<'a> {
    api_version: String,
    #[serde(borrow)]
    result: BorrowedSnapshotResult<'a>,
}

impl std::fmt::Debug for BorrowedApiSuccess<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BorrowedApiSuccess")
            .field("result", &self.result)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BorrowedHostResponse<'a> {
    api_version: String,
    request_id: String,
    confirmation_tag: String,
    expires_at: String,
    #[serde(borrow)]
    result: Option<BorrowedSnapshotResult<'a>>,
    error: Option<TerminalSnapshotReasonCode>,
    detail: Option<String>,
}

impl std::fmt::Debug for BorrowedHostResponse<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BorrowedHostResponse")
            .field("result", &self.result)
            .field("error", &self.error)
            .field("has_detail", &self.detail.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(tag = "format", rename_all = "camelCase", deny_unknown_fields)]
enum BorrowedSnapshotResult<'a> {
    Json {
        snapshot: TerminalSnapshotDocument,
    },
    Png {
        metadata: TerminalSnapshotPngMetadata,
        #[serde(borrow, rename = "pngBase64")]
        png_base64: &'a str,
    },
}

impl std::fmt::Debug for BorrowedSnapshotResult<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json { snapshot } => formatter
                .debug_struct("BorrowedSnapshotResult::Json")
                .field("snapshot", snapshot)
                .finish(),
            Self::Png {
                metadata,
                png_base64,
            } => formatter
                .debug_struct("BorrowedSnapshotResult::Png")
                .field("metadata", metadata)
                .field("base64_bytes", &png_base64.len())
                .finish(),
        }
    }
}

fn decode_wire_result(
    result: BorrowedSnapshotResult<'_>,
) -> Result<TerminalSnapshotPayload, ProtocolError> {
    match result {
        BorrowedSnapshotResult::Json { snapshot } => Ok(TerminalSnapshotPayload::Json { snapshot }),
        BorrowedSnapshotResult::Png {
            metadata,
            png_base64,
        } => Ok(TerminalSnapshotPayload::Png {
            metadata,
            png: decode_canonical_base64_png(png_base64)?,
        }),
    }
}

pub fn decode_api_success(
    input: &[u8],
    expected_request_id: &str,
    expected_target: &str,
    expected_format: TerminalSnapshotFormat,
) -> Result<TerminalSnapshotApiSuccess, ProtocolError> {
    if input.is_empty() || input.len() > crate::protocol::MAX_TRANSPORT_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    JsonShapeScanner::new(input).scan()?;
    let wire: BorrowedApiSuccess<'_> =
        serde_json::from_slice(input).map_err(|_| ProtocolError::Invalid)?;
    if wire.api_version != API_VERSION {
        return Err(ProtocolError::Invalid);
    }
    let envelope = TerminalSnapshotApiSuccess {
        api_version: wire.api_version,
        result: decode_wire_result(wire.result)?,
    };
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
    if input.is_empty() || input.len() > crate::protocol::MAX_TRANSPORT_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    JsonShapeScanner::new(input).scan()?;
    let wire: BorrowedHostResponse<'_> =
        serde_json::from_slice(input).map_err(|_| ProtocolError::Invalid)?;
    let envelope = TerminalSnapshotHostResponse {
        api_version: wire.api_version,
        request_id: wire.request_id,
        confirmation_tag: wire.confirmation_tag,
        expires_at: wire.expires_at,
        result: wire.result.map(decode_wire_result).transpose()?,
        error: wire.error,
        detail: wire.detail,
    };
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
    result: &TerminalSnapshotPayload,
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
        TerminalSnapshotPayload::Json { snapshot } => snapshot.validate(),
        TerminalSnapshotPayload::Png { metadata, png } => {
            crate::png_validation::validate_png_for_metadata(png, metadata).map(|_| ())
        }
    }
}

pub fn decode_canonical_base64_png(text: &str) -> Result<Vec<u8>, ProtocolError> {
    if text.is_empty() {
        return Err(ProtocolError::Invalid);
    }
    if text.len() > MAX_BASE64_TEXT_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(ProtocolError::Invalid);
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return Err(ProtocolError::Invalid);
    }
    let body_end = bytes.len().saturating_sub(padding);
    if bytes[..body_end]
        .iter()
        .any(|byte| base64_sextet(*byte).is_none())
        || bytes[body_end..].iter().any(|byte| *byte != b'=')
    {
        return Err(ProtocolError::Invalid);
    }
    match padding {
        1 if base64_sextet(bytes[body_end - 1]).is_none_or(|value| value & 0x03 != 0) => {
            return Err(ProtocolError::Invalid)
        }
        2 if base64_sextet(bytes[body_end - 1]).is_none_or(|value| value & 0x0f != 0) => {
            return Err(ProtocolError::Invalid)
        }
        _ => {}
    }
    let decoded_len = bytes
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|length| length.checked_sub(padding))
        .filter(|length| *length > 0 && *length <= MAX_PNG_BYTES)
        .ok_or(ProtocolError::TooLarge)?;
    let mut decoded = vec![0u8; decoded_len];
    let mut decoder = base64::read::DecoderReader::new(
        text.as_bytes(),
        &base64::engine::general_purpose::STANDARD,
    );
    decoder
        .read_exact(&mut decoded)
        .map_err(|_| ProtocolError::Invalid)?;
    let mut trailing = [0u8; 1];
    if decoder
        .read(&mut trailing)
        .map_err(|_| ProtocolError::Invalid)?
        != 0
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(decoded)
}

fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub fn terminal_snapshot_payload_bytes(
    payload: &TerminalSnapshotPayload,
) -> Result<u64, ProtocolError> {
    match payload {
        TerminalSnapshotPayload::Json { snapshot } => {
            snapshot.validate()?;
            let mut writer = CountingWriter::new(crate::protocol::MAX_JSON_BYTES);
            serialize_ascii_into(&mut writer, snapshot)?;
            u64::try_from(writer.len).map_err(|_| ProtocolError::TooLarge)
        }
        TerminalSnapshotPayload::Png { metadata, png } => {
            metadata.validate()?;
            if png.is_empty()
                || png.len() > MAX_PNG_BYTES
                || usize::try_from(metadata.png.bytes).ok() != Some(png.len())
            {
                return Err(ProtocolError::InvalidPng);
            }
            u64::try_from(png.len()).map_err(|_| ProtocolError::TooLarge)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BorrowedTerminalSnapshotDocument<'a> {
    schema_version: u32,
    request_id: &'a str,
    captured_at: &'a str,
    requester: &'a str,
    target: &'a str,
    session: &'a TerminalSnapshotSession,
    screen: &'a TerminalScreen,
    fidelity: &'a TerminalSnapshotFidelity,
}

impl std::fmt::Debug for BorrowedTerminalSnapshotDocument<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BorrowedTerminalSnapshotDocument")
            .field("schema_version", &self.schema_version)
            .field("session", self.session)
            .field("screen", self.screen)
            .field("fidelity", self.fidelity)
            .finish_non_exhaustive()
    }
}

fn borrowed_document<'a>(
    request_id: &'a str,
    requester: &'a str,
    target: &'a str,
    model: &'a TerminalScreenModel,
) -> Result<BorrowedTerminalSnapshotDocument<'a>, ProtocolError> {
    validate_uuid(request_id, Some(4))?;
    validate_requester_identity(requester, true)?;
    validate_wg_fqn(target)?;
    model.validate()?;
    Ok(BorrowedTerminalSnapshotDocument {
        schema_version: SCHEMA_VERSION,
        request_id,
        captured_at: &model.captured_at,
        requester,
        target,
        session: &model.session,
        screen: &model.screen,
        fidelity: &model.fidelity,
    })
}

pub fn terminal_snapshot_json_bytes_from_model(
    request_id: &str,
    requester: &str,
    target: &str,
    model: &TerminalScreenModel,
) -> Result<u64, ProtocolError> {
    let document = borrowed_document(request_id, requester, target, model)?;
    let mut writer = CountingWriter::new(MAX_JSON_BYTES);
    serialize_ascii_into(&mut writer, &document)?;
    u64::try_from(writer.len).map_err(|_| ProtocolError::TooLarge)
}

pub fn encode_api_json_success_from_model(
    request_id: &str,
    requester: &str,
    target: &str,
    model: &TerminalScreenModel,
) -> Result<Vec<u8>, ProtocolError> {
    let document = borrowed_document(request_id, requester, target, model)?;
    let mut writer = CappedWriter::new(crate::protocol::MAX_TRANSPORT_BYTES);
    writer
        .write_all(b"{\"apiVersion\":\"1\",\"result\":{\"format\":\"json\",\"snapshot\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_capped(&mut writer, &document)?;
    writer
        .write_all(b"}}")
        .map_err(|_| ProtocolError::TooLarge)?;
    writer.into_bytes()
}

pub fn encode_host_json_success_from_model(
    request_id: &str,
    confirmation_tag: &str,
    expires_at: &str,
    requester: &str,
    target: &str,
    model: &TerminalScreenModel,
) -> Result<Vec<u8>, ProtocolError> {
    validate_host_correlation(request_id, confirmation_tag, expires_at)?;
    let document = borrowed_document(request_id, requester, target, model)?;
    let mut writer = CappedWriter::new(crate::protocol::MAX_TRANSPORT_BYTES);
    writer
        .write_all(b"{\"apiVersion\":\"1\",\"requestId\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, request_id)?;
    writer
        .write_all(b",\"confirmationTag\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, confirmation_tag)?;
    writer
        .write_all(b",\"expiresAt\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, expires_at)?;
    writer
        .write_all(b",\"result\":{\"format\":\"json\",\"snapshot\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_capped(&mut writer, &document)?;
    writer
        .write_all(b"}}")
        .map_err(|_| ProtocolError::TooLarge)?;
    writer.into_bytes()
}

pub fn encode_api_success_payload(
    payload: &TerminalSnapshotPayload,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = CappedWriter::new(crate::protocol::MAX_TRANSPORT_BYTES);
    writer
        .write_all(b"{\"apiVersion\":\"1\",\"result\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    write_snapshot_payload(&mut writer, payload)?;
    writer
        .write_all(b"}")
        .map_err(|_| ProtocolError::TooLarge)?;
    writer.into_bytes()
}

pub fn encode_host_success_payload(
    request_id: &str,
    confirmation_tag: &str,
    expires_at: &str,
    payload: &TerminalSnapshotPayload,
) -> Result<Vec<u8>, ProtocolError> {
    validate_host_correlation(request_id, confirmation_tag, expires_at)?;
    if payload.request_id() != request_id {
        return Err(ProtocolError::Invalid);
    }
    let mut writer = CappedWriter::new(crate::protocol::MAX_TRANSPORT_BYTES);
    writer
        .write_all(b"{\"apiVersion\":\"1\",\"requestId\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, request_id)?;
    writer
        .write_all(b",\"confirmationTag\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, confirmation_tag)?;
    writer
        .write_all(b",\"expiresAt\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, expires_at)?;
    writer
        .write_all(b",\"result\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    write_snapshot_payload(&mut writer, payload)?;
    writer
        .write_all(b"}")
        .map_err(|_| ProtocolError::TooLarge)?;
    writer.into_bytes()
}

pub fn encode_host_failure_payload(
    request_id: &str,
    confirmation_tag: &str,
    expires_at: &str,
    reason: TerminalSnapshotReasonCode,
) -> Result<Vec<u8>, ProtocolError> {
    validate_host_correlation(request_id, confirmation_tag, expires_at)?;
    let mut writer = CappedWriter::new(crate::protocol::MAX_TRANSPORT_BYTES);
    writer
        .write_all(b"{\"apiVersion\":\"1\",\"requestId\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, request_id)?;
    writer
        .write_all(b",\"confirmationTag\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, confirmation_tag)?;
    writer
        .write_all(b",\"expiresAt\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, expires_at)?;
    writer
        .write_all(b",\"error\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, &reason)?;
    writer
        .write_all(b",\"detail\":")
        .map_err(|_| ProtocolError::TooLarge)?;
    serialize_ascii_into(&mut writer, reason.detail())?;
    writer
        .write_all(b"}")
        .map_err(|_| ProtocolError::TooLarge)?;
    writer.into_bytes()
}

fn validate_host_correlation(
    request_id: &str,
    confirmation_tag: &str,
    expires_at: &str,
) -> Result<(), ProtocolError> {
    crate::protocol::validate_uuid(request_id, Some(4))?;
    crate::protocol::validate_hex(confirmation_tag, 64)?;
    crate::protocol::validate_timestamp(expires_at)?;
    Ok(())
}

fn write_snapshot_payload(
    writer: &mut CappedWriter,
    payload: &TerminalSnapshotPayload,
) -> Result<(), ProtocolError> {
    match payload {
        TerminalSnapshotPayload::Json { snapshot } => {
            snapshot.validate()?;
            writer
                .write_all(b"{\"format\":\"json\",\"snapshot\":")
                .map_err(|_| ProtocolError::TooLarge)?;
            serialize_ascii_capped(writer, snapshot)?;
            writer.write_all(b"}").map_err(|_| ProtocolError::TooLarge)
        }
        TerminalSnapshotPayload::Png { metadata, png } => {
            crate::png_validation::validate_png_for_metadata(png, metadata)?;
            let encoded_len = png
                .len()
                .checked_add(2)
                .and_then(|length| length.checked_div(3))
                .and_then(|groups| groups.checked_mul(4))
                .filter(|length| *length <= MAX_BASE64_TEXT_BYTES)
                .ok_or(ProtocolError::TooLarge)?;
            writer
                .write_all(b"{\"format\":\"png\",\"metadata\":")
                .map_err(|_| ProtocolError::TooLarge)?;
            serialize_ascii_capped(writer, metadata)?;
            writer
                .write_all(b",\"pngBase64\":\"")
                .map_err(|_| ProtocolError::TooLarge)?;
            let before = writer.bytes.len();
            let encoded = {
                let mut encoder = base64::write::EncoderWriter::new(
                    &mut *writer,
                    &base64::engine::general_purpose::STANDARD,
                );
                encoder
                    .write_all(png)
                    .and_then(|_| encoder.finish().map(|_| ()))
            };
            if encoded.is_err() {
                return if writer.exceeded {
                    Err(ProtocolError::TooLarge)
                } else {
                    Err(ProtocolError::Serialization)
                };
            }
            if writer.bytes.len().saturating_sub(before) != encoded_len {
                return Err(ProtocolError::Serialization);
            }
            writer
                .write_all(b"\"}")
                .map_err(|_| ProtocolError::TooLarge)
        }
    }
}

fn serialize_ascii_capped<T: Serialize + ?Sized>(
    writer: &mut CappedWriter,
    value: &T,
) -> Result<(), ProtocolError> {
    match serialize_ascii_into(writer, value) {
        Err(_) if writer.exceeded => Err(ProtocolError::TooLarge),
        result => result,
    }
}

fn serialize_ascii_into<W: Write, T: Serialize + ?Sized>(
    writer: &mut W,
    value: &T,
) -> Result<(), ProtocolError> {
    let formatter = AsciiSafeFormatter;
    let mut serializer = serde_json::Serializer::with_formatter(writer, formatter);
    value
        .serialize(&mut serializer)
        .map_err(|_| ProtocolError::Serialization)
}

struct CountingWriter {
    len: usize,
    cap: usize,
    exceeded: bool,
}

impl CountingWriter {
    fn new(cap: usize) -> Self {
        Self {
            len: 0,
            cap,
            exceeded: false,
        }
    }
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.len = self.len.checked_add(bytes.len()).ok_or_else(|| {
            self.exceeded = true;
            io::Error::new(io::ErrorKind::WriteZero, "terminal snapshot cap reached")
        })?;
        if self.len > self.cap {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "terminal snapshot cap reached",
            ));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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

#[cfg(test)]
mod tests;
