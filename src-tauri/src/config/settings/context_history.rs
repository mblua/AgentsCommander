//! #3532 P08: app-scope colour period for the context-history heatmap.
//! #4049: app-scope enabled switch and retention days for the same feature.
//!
//! Pure types: the Settings store owns all IO. Colour accepts only 1, 7 and 30;
//! retention only integers 1..=365; enabled only JSON booleans. Any other JSON
//! value falls back to its default in memory without failing the load.

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

/// #4049: days of stored context history to keep. Serialized as the plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextHistoryRetentionDays(u32);

impl ContextHistoryRetentionDays {
    const DEFAULT: u32 = 30;
    const MIN: u32 = 1;
    const MAX: u32 = 365;

    /// The wire value: an integer in 1..=365.
    pub(crate) const fn days(self) -> u32 {
        self.0
    }

    fn from_value(value: &Value) -> Option<Self> {
        let days = u32::try_from(value.as_u64()?).ok()?;
        (Self::MIN..=Self::MAX)
            .contains(&days)
            .then_some(Self(days))
    }
}

impl Default for ContextHistoryRetentionDays {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

impl Serialize for ContextHistoryRetentionDays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.days())
    }
}

/// Field deserializer: never fails on valid JSON, so a bad value cannot reset
/// the rest of `AppSettings`. The warning names only the JSON kind.
pub(crate) fn deserialize_context_history_retention_days<'de, D>(
    deserializer: D,
) -> Result<ContextHistoryRetentionDays, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(
        ContextHistoryRetentionDays::from_value(&value).unwrap_or_else(|| {
            log::warn!(
                "Invalid contextHistoryRetentionDays ({}): expected 1..365; using 30 in memory",
                json_kind(&value)
            );
            ContextHistoryRetentionDays::default()
        }),
    )
}

/// #4049 field deserializer for the enabled switch: only a JSON boolean is
/// accepted; any other value loads as `false` without failing the load.
pub(crate) fn deserialize_context_history_enabled<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or_else(|| {
        log::warn!(
            "Invalid contextHistoryEnabled ({}): expected a boolean; using false in memory",
            json_kind(&value)
        );
        false
    }))
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
