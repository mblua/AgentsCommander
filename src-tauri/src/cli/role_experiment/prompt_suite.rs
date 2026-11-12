use super::artifacts::CliError;
use super::artifacts::ParsedPromptSuite;
use super::artifacts::PromptCase;
use super::support::err_at_line;
use super::support::err_at_path;
use super::support::sha256_hex;
use super::validation::reject_link_or_reparse;
use std::collections::BTreeMap;
use std::path::Path;

use crate::filesystem::documents;

pub(super) fn parse_prompt_suite(path: &Path) -> Result<ParsedPromptSuite, Vec<CliError>> {
    reject_link_or_reparse(path, "prompt_suite_missing", None)?;
    if !documents::prompt_suite_is_file(path) {
        return Err(vec![err_at_path(
            "prompt_suite_missing",
            format!("Prompt suite not found: {}", path.display()),
            path,
        )]);
    }
    let bytes = documents::read_prompt_suite(path).map_err(|e| {
        vec![err_at_path(
            "prompt_suite_missing",
            format!("Failed to read prompt suite: {e}"),
            path,
        )]
    })?;
    let sha256 = sha256_hex(&bytes);
    let content = String::from_utf8(bytes).map_err(|e| {
        vec![err_at_path(
            "prompt_suite_unparseable",
            format!("Prompt suite is not valid UTF-8: {e}"),
            path,
        )]
    })?;
    let mut errors = Vec::new();
    let mut prompts = Vec::new();
    let mut ids = BTreeMap::new();
    for (idx, line) in content.lines().enumerate() {
        let physical_line = idx + 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(case) = parse_prompt_case(line, physical_line, &mut ids, &mut errors) {
            prompts.push(case);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if prompts.is_empty() {
        return Err(vec![err_at_path(
            "prompt_suite_empty",
            "Prompt suite contains no prompt cases",
            path,
        )]);
    }
    Ok(ParsedPromptSuite {
        path: documents::canonical_prompt_suite_path(path),
        sha256,
        prompts,
    })
}

/// Parses one non-blank suite line; records errors and returns `None` when invalid.
fn parse_prompt_case(
    line: &str,
    physical_line: usize,
    ids: &mut BTreeMap<String, usize>,
    errors: &mut Vec<CliError>,
) -> Option<PromptCase> {
    let parsed = parse_prompt_json(line, physical_line, errors)?;
    let Some(object) = parsed.as_object() else {
        errors.push(err_at_line(
            "prompt_suite_unparseable",
            format!("Prompt suite line {physical_line} must be a JSON object"),
            physical_line,
            None,
        ));
        return None;
    };
    let id = required_prompt_string(object, "id", physical_line, errors)?;
    if !is_safe_prompt_id(&id) {
        errors.push(err_at_line(
            "prompt_suite_unparseable",
            format!(
                "Prompt suite line {physical_line} has invalid id '{id}'; \
                 use lowercase ASCII letters, digits, hyphen, or underscore"
            ),
            physical_line,
            Some("id"),
        ));
        return None;
    }
    let title = required_prompt_string(object, "title", physical_line, errors)?;
    let prompt = required_prompt_string(object, "prompt", physical_line, errors)?;
    let tags = optional_string_array(object, "tags", physical_line, errors)?;
    let expected_behaviors =
        optional_string_array(object, "expectedBehaviors", physical_line, errors)?;
    if let Some(first_line) = ids.insert(id.clone(), physical_line) {
        errors.push(err_at_line(
            "prompt_suite_duplicate_id",
            format!(
                "Prompt suite duplicate id '{id}' at line {physical_line}; \
                 first seen at line {first_line}"
            ),
            physical_line,
            Some("id"),
        ));
        return None;
    }
    Some(PromptCase {
        id,
        title,
        prompt,
        tags,
        expected_behaviors,
        line: physical_line,
    })
}

fn parse_prompt_json(
    line: &str,
    physical_line: usize,
    errors: &mut Vec<CliError>,
) -> Option<serde_json::Value> {
    match serde_json::from_str(line) {
        Ok(value) => Some(value),
        Err(e) => {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!("Prompt suite line {physical_line} is invalid JSON: {e}"),
                physical_line,
                None,
            ));
            None
        }
    }
}

fn required_prompt_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    line: usize,
    errors: &mut Vec<CliError>,
) -> Option<String> {
    let value = object.get(field).and_then(|v| v.as_str()).map(str::trim);
    match value {
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!("Prompt suite line {line} is missing required string field {field}"),
                line,
                Some(field),
            ));
            None
        }
    }
}

fn optional_string_array(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
    line: usize,
    errors: &mut Vec<CliError>,
) -> Option<Vec<String>> {
    let Some(value) = object.get(field) else {
        return Some(Vec::new());
    };
    let Some(items) = value.as_array() else {
        errors.push(err_at_line(
            "prompt_suite_unparseable",
            format!("Prompt suite line {line} field {field} must be an array of strings"),
            line,
            Some(field),
        ));
        return None;
    };
    let mut out = Vec::new();
    for item in items {
        let Some(item) = item.as_str() else {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!("Prompt suite line {line} field {field} must contain only strings"),
                line,
                Some(field),
            ));
            return None;
        };
        out.push(item.to_string());
    }
    Some(out)
}

fn is_safe_prompt_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
