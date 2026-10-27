use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use super::{ProtocolError, TerminalCellWidth, TerminalScreen, ROOT_REQUESTER_IDENTITY};

pub fn canonical_timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn validate_timestamp(value: &str) -> Result<DateTime<Utc>, ProtocolError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| ProtocolError::Invalid)?;
    let parsed = parsed.with_timezone(&Utc);
    if canonical_timestamp(parsed) != value {
        return Err(ProtocolError::Invalid);
    }
    Ok(parsed)
}

pub fn validate_uuid(value: &str, required_version: Option<usize>) -> Result<Uuid, ProtocolError> {
    let parsed = Uuid::parse_str(value).map_err(|_| ProtocolError::Invalid)?;
    if parsed.to_string() != value
        || required_version.is_some_and(|version| parsed.get_version_num() != version)
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(parsed)
}

pub fn validate_hex(value: &str, length: usize) -> Result<(), ProtocolError> {
    if value.len() != length
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(())
}

pub fn validate_requester_identity(value: &str, allow_root: bool) -> Result<(), ProtocolError> {
    if allow_root && value == ROOT_REQUESTER_IDENTITY {
        return Ok(());
    }
    validate_wg_fqn(value)
}

pub fn validate_target_syntax(value: &str) -> Result<(), ProtocolError> {
    if value == ROOT_REQUESTER_IDENTITY {
        return Ok(());
    }
    if value.contains(':') {
        return validate_wg_fqn(value);
    }
    if value.is_empty() || value.len() > 1_024 || value.matches('/').count() != 1 {
        return Err(ProtocolError::Invalid);
    }
    let (project, agent) = value.split_once('/').ok_or(ProtocolError::Invalid)?;
    for component in [project, agent] {
        if component.is_empty()
            || component == "."
            || component == ".."
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ProtocolError::Invalid);
        }
    }
    Ok(())
}

pub fn validate_wg_fqn(value: &str) -> Result<(), ProtocolError> {
    if value.is_empty() || value.len() > 1_024 || value.matches(':').count() != 1 {
        return Err(ProtocolError::Invalid);
    }
    let (project, local) = value.split_once(':').ok_or(ProtocolError::Invalid)?;
    if project.is_empty()
        || matches!(project, "." | "..")
        || project.chars().any(|scalar| {
            scalar.is_control()
                || matches!(
                    scalar,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{2028}'
                        | '\u{2029}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
                || matches!(scalar, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        || local.matches('/').count() != 1
    {
        return Err(ProtocolError::Invalid);
    }
    let (workgroup, agent) = local.split_once('/').ok_or(ProtocolError::Invalid)?;
    let rest = workgroup
        .strip_prefix("room-")
        .or_else(|| workgroup.strip_prefix("wg-"))
        .ok_or(ProtocolError::Invalid)?;
    let (digits, team) = rest.split_once('-').ok_or(ProtocolError::Invalid)?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || team.is_empty()
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || agent.is_empty()
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ProtocolError::Invalid);
    }
    Ok(())
}

pub fn validate_screen(screen: &TerminalScreen) -> Result<(), ProtocolError> {
    let dimensions = screen.dimensions.checked_image_dimensions()?;
    if screen.lines.len() != usize::from(screen.dimensions.rows)
        || dimensions.cells
            != screen
                .lines
                .iter()
                .map(|line| line.cells.len())
                .sum::<usize>()
        || screen.cursor.in_bounds
            != (screen.cursor.row < screen.dimensions.rows
                && screen.cursor.column < screen.dimensions.columns)
    {
        return Err(ProtocolError::Invalid);
    }

    for line in &screen.lines {
        if line.cells.len() != usize::from(screen.dimensions.columns) {
            return Err(ProtocolError::Invalid);
        }
        for (column, cell) in line.cells.iter().enumerate() {
            if cell.text.chars().count() > 6 || cell.text.len() > 24 {
                return Err(ProtocolError::Invalid);
            }
            match cell.width {
                TerminalCellWidth::Narrow => {}
                TerminalCellWidth::WideLead => {
                    if column + 1 >= line.cells.len()
                        || line.cells[column + 1].width != TerminalCellWidth::WideContinuation
                    {
                        return Err(ProtocolError::Invalid);
                    }
                }
                TerminalCellWidth::WideContinuation => {
                    if !cell.text.is_empty()
                        || column == 0
                        || line.cells[column - 1].width != TerminalCellWidth::WideLead
                    {
                        return Err(ProtocolError::Invalid);
                    }
                }
            }
        }
    }
    Ok(())
}
