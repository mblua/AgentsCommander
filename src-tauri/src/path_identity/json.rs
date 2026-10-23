use serde::de;
use serde::de::DeserializeSeed;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;
use std::collections::HashSet;

pub struct ScannedJsonDocument {
    pub value: serde_json::Value,
    pub duplicate_keys: bool,
    pub privileged_candidate: bool,
}

#[derive(Default)]
struct JsonScanState {
    duplicate_keys: bool,
    privileged_candidate: bool,
}

/// Parse one JSON document while retaining whether any object key was
/// duplicated and whether any top-level occurrence identifies privileged PTY
/// input. Escaped JSON key and value spellings are decoded by Serde before the
/// classification is recorded.
pub fn scan_json_document(bytes: &[u8]) -> Result<ScannedJsonDocument, String> {
    let state = std::rc::Rc::new(std::cell::RefCell::new(JsonScanState::default()));
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = ScanningValue {
        state: std::rc::Rc::clone(&state),
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| "invalid_envelope".to_string())?;
    deserializer
        .end()
        .map_err(|_| "invalid_envelope".to_string())?;
    let state = state.borrow();
    Ok(ScannedJsonDocument {
        value,
        duplicate_keys: state.duplicate_keys,
        privileged_candidate: state.privileged_candidate,
    })
}

/// Parse JSON while rejecting duplicate object keys at every depth.
pub fn parse_json_no_duplicates(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let document = scan_json_document(bytes)?;
    if document.duplicate_keys {
        return Err("invalid_envelope".to_string());
    }
    Ok(document.value)
}

struct ScanningValue {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ScanningValue {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ScanningVisitor {
            state: self.state,
            depth: self.depth,
        })
    }
}

struct ScanningVisitor {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> Visitor<'de> for ScanningVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("invalid number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(serde_json::Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(serde_json::Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ScanningValue {
            state: self.state,
            depth: self.depth,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(ScanningValue {
            state: std::rc::Rc::clone(&self.state),
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                self.state.borrow_mut().duplicate_keys = true;
            }
            let value = map.next_value_seed(ScanningValue {
                state: std::rc::Rc::clone(&self.state),
                depth: self.depth + 1,
            })?;
            if self.depth == 0 {
                let privileged = key == "ptyInput"
                    || (key == "action" && value.as_str() == Some("pty-input"))
                    || (key == "kind" && value.as_str() == Some("pty-input-marker"));
                if privileged {
                    self.state.borrow_mut().privileged_candidate = true;
                }
            }
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}
