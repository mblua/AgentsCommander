//! #3532 P08: app-scope colour period for the context-history heatmap.
//!
//! Pure type: the Settings store owns all IO. Only 1, 7 and 30 are valid; any
//! other JSON value falls back to 30 in memory without failing the load.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Days of history that colour the heatmap. Serialized as the plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextHistoryColorDays {
    One,
    Seven,
    #[default]
    Thirty,
}

impl ContextHistoryColorDays {
    /// The wire value: 1, 7 or 30.
    pub(crate) const fn days(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Seven => 7,
            Self::Thirty => 30,
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value.as_u64()? {
            1 => Some(Self::One),
            7 => Some(Self::Seven),
            30 => Some(Self::Thirty),
            _ => None,
        }
    }
}

impl Serialize for ContextHistoryColorDays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.days())
    }
}

/// Field deserializer: never fails on valid JSON, so a bad value cannot reset
/// the rest of `AppSettings`. The warning names only the JSON kind.
pub(crate) fn deserialize_context_history_color_days<'de, D>(
    deserializer: D,
) -> Result<ContextHistoryColorDays, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(
        ContextHistoryColorDays::from_value(&value).unwrap_or_else(|| {
            log::warn!(
                "Invalid contextHistoryColorDays ({}): expected 1, 7 or 30; using 30 in memory",
                json_kind(&value)
            );
            ContextHistoryColorDays::default()
        }),
    )
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests;
