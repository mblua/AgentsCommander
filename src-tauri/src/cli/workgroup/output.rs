//! JSON output helper for the `workgroup` CLI verbs: pretty-prints a serializable value
//! through `cli_println!`, mapping a serialization failure to an error string. It owns no
//! state and does not decide what each verb prints.

use serde::Serialize;

pub(super) fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {e}"))?;
    crate::cli_println!("{}", json);
    Ok(())
}
