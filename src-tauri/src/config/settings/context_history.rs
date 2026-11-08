//! #3532 P08: app-scope colour period for the context-history heatmap.
//! #4049: app-scope enabled switch and retention days for the same feature.
//! #4091: session-count windows and the session count that colours the map.
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

/// #4091: the only default session-count array. Other surfaces read settings.
pub(crate) const DEFAULT_SESSION_WINDOWS: [u32; 3] = [25, 100, 300];

/// #4091: session counts that bucket the heat map. 1..=8 strictly ascending
/// integers in 1..=10000. Serialized as the plain JSON array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextHistorySessionWindows(Vec<u32>);

impl ContextHistorySessionWindows {
    const MAX_LEN: usize = 8;
    const MIN_COUNT: u32 = 1;
    const MAX_COUNT: u32 = 10_000;

    /// The effective counts, never empty.
    pub(crate) fn counts(&self) -> &[u32] {
        &self.0
    }

    /// The largest (last) effective count.
    pub(crate) fn last(&self) -> u32 {
        *self.0.last().unwrap_or(&DEFAULT_SESSION_WINDOWS[2])
    }

    fn from_value(value: &Value) -> Result<Self, &'static str> {
        let items = value.as_array().ok_or("not an array")?;
        if items.is_empty() || items.len() > Self::MAX_LEN {
            return Err("expected 1..8 entries");
        }
        let mut counts = Vec::with_capacity(items.len());
        for item in items {
            let count = item
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (Self::MIN_COUNT..=Self::MAX_COUNT).contains(n))
                .ok_or("entries must be integers 1..10000")?;
            if counts.last().is_some_and(|prev| *prev >= count) {
                return Err("entries must be strictly ascending");
            }
            counts.push(count);
        }
        Ok(Self(counts))
    }
}

impl Default for ContextHistorySessionWindows {
    fn default() -> Self {
        Self(DEFAULT_SESSION_WINDOWS.to_vec())
    }
}

impl Serialize for ContextHistorySessionWindows {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// #4091 field deserializer: never fails on valid JSON. The warning names the
/// field, the JSON kind and a fixed reason.
pub(crate) fn deserialize_context_history_session_windows<'de, D>(
    deserializer: D,
) -> Result<ContextHistorySessionWindows, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(
        ContextHistorySessionWindows::from_value(&value).unwrap_or_else(|reason| {
            log::warn!(
                "Invalid contextHistorySessionWindows ({}): {reason}; using defaults in memory",
                json_kind(&value)
            );
            ContextHistorySessionWindows::default()
        }),
    )
}

/// #4091 field deserializer for the colour count: missing and null are `None`;
/// any other non-integer or out-of-range value is `None` with a warning. It
/// never substitutes a count; membership is repaired by
/// [`effective_color_sessions`].
pub(crate) fn deserialize_context_history_color_sessions<'de, D>(
    deserializer: D,
) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    let count = value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| {
            (ContextHistorySessionWindows::MIN_COUNT..=ContextHistorySessionWindows::MAX_COUNT)
                .contains(n)
        });
    if count.is_none() {
        log::warn!(
            "Invalid contextHistoryColorSessions ({}): expected an integer 1..10000; \
             using the last session window in memory",
            json_kind(&value)
        );
    }
    Ok(count)
}

/// #4091: the colour count kept when it is a member of `windows`, otherwise
/// the last effective member.
pub(crate) fn effective_color_sessions(
    windows: &ContextHistorySessionWindows,
    color: Option<u32>,
) -> u32 {
    color
        .filter(|count| windows.counts().contains(count))
        .unwrap_or_else(|| windows.last())
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
