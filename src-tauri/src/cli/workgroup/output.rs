use serde::Serialize;

pub(super) fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {e}"))?;
    crate::cli_println!("{}", json);
    Ok(())
}
