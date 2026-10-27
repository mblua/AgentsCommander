use std::collections::HashSet;

use crate::protocol::{ProtocolError, MAX_BASE64_TEXT_BYTES, MAX_JSON_DEPTH};

pub(crate) struct JsonShapeScanner<'a> {
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
    cells: usize,
}

impl<'a> JsonShapeScanner<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            nodes: 0,
            cells: 0,
        }
    }

    pub(crate) fn scan(mut self) -> Result<(), ProtocolError> {
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
