use serde_json::Value;

pub(super) fn require_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| format!("Missing required field: {}", key))
}

/// #973 - a terminal dimension off the wire, for `pty_resize`.
///
/// This is a network-facing input path, and `as u16` SILENTLY TRUNCATES: `cols: 65536` arrives
/// at the PTY as **0**, and `cols: 65537` as **1**. A zero-column ConPTY is precisely what #973
/// exists to keep out, and a one-column one is not much better - both from a number that looked
/// perfectly plausible on the wire.
///
/// `u16` is exactly what the Tauri `pty_resize` command takes, where serde rejects an
/// out-of-range number for us. The web transport has no such boundary, so this is it: same
/// contract, both transports. A value that is absent stays absent (the caller's default); only
/// one that is PRESENT and cannot be a terminal dimension is an error.
///
/// A zero is deliberately NOT an error here, and the reason is NOT that a legitimate client sends
/// one - it cannot. xterm's `fit()` clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1, so a zero on
/// this path means a client that is not xterm, or is broken, or is hostile. This function is what
/// stands between such a client and the PTY.
///
/// It passes the zero down anyway, because refusing it HERE would be the second place that
/// refuses it. `resize_instance` already does, once, for every transport, with a warn. Erroring
/// here as well would make the same payload behave differently on the two transports - the Tauri
/// command takes a typed `u16`, where serde accepts a 0 and passes it down exactly like this -
/// and would put one rule in two places that can drift apart. One refusal, in the backend, shared.
/// What this boundary is for is the value that cannot be a dimension at all, which is the one the
/// old `as u16` cast turned into a plausible-looking lie.
pub(super) fn terminal_dimension(args: &Value, key: &str, default: u16) -> Result<u16, String> {
    let Some(value) = args.get(key) else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| format!("Invalid field {}: not a terminal dimension: {}", key, value))
}

pub(super) fn require_json<T>(args: &Value, key: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| format!("Missing required field: {}", key))?;
    serde_json::from_value(value).map_err(|e| format!("Invalid field {}: {}", key, e))
}

pub(super) fn str_or(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| default.to_string())
}

pub(super) fn str_vec_or(args: &Value, key: &str, default: &[String]) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| default.to_vec())
}
