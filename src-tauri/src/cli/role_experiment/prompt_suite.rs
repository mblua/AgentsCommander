use super::support::err_at_line;
use super::support::err_at_path;
use super::support::sha256_hex;
use super::validation::reject_link_or_reparse;
use super::CliError;
use super::ParsedPromptSuite;
use super::PromptCase;
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
            format!("Failed to read prompt suite: {}", e),
            path,
        )]
    })?;
    let sha256 = sha256_hex(&bytes);
    let content = String::from_utf8(bytes).map_err(|e| {
        vec![err_at_path(
            "prompt_suite_unparseable",
            format!("Prompt suite is not valid UTF-8: {}", e),
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
        let Some(parsed) = parse_prompt_json(line, physical_line, &mut errors) else {
            continue;
        };
        let Some(object) = parsed.as_object() else {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!("Prompt suite line {} must be a JSON object", physical_line),
                physical_line,
                None,
            ));
            continue;
        };
        let Some(id) = required_prompt_string(object, "id", physical_line, &mut errors) else {
            continue;
        };
        if !is_safe_prompt_id(&id) {
            errors.push(err_at_line(
                "prompt_suite_unparseable",
                format!(
                    "Prompt suite line {} has invalid id '{}'; \
                     use lowercase ASCII letters, digits, hyphen, or underscore",
                    physical_line, id
                ),
                physical_line,
                Some("id"),
            ));
            continue;
        }
        let Some(title) = required_prompt_string(object, "title", physical_line, &mut errors)
        else {
            continue;
        };
        let Some(prompt) = required_prompt_string(object, "prompt", physical_line, &mut errors)
        else {
            continue;
        };
        let Some(tags) = optional_string_array(object, "tags", physical_line, &mut errors) else {
            continue;
        };
        let Some(expected_behaviors) =
            optional_string_array(object, "expectedBehaviors", physical_line, &mut errors)
        else {
            continue;
        };
        if let Some(first_line) = ids.insert(id.clone(), physical_line) {
            errors.push(err_at_line(
                "prompt_suite_duplicate_id",
                format!(
                    "Prompt suite duplicate id '{}' at line {}; first seen at line {}",
                    id, physical_line, first_line
                ),
                physical_line,
                Some("id"),
            ));
            continue;
        }
        prompts.push(PromptCase {
            id,
            title,
            prompt,
            tags,
            expected_behaviors,
            line: physical_line,
        });
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
                format!("Prompt suite line {} is invalid JSON: {}", physical_line, e),
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
                format!(
                    "Prompt suite line {} is missing required string field {}",
                    line, field
                ),
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
            format!(
                "Prompt suite line {} field {} must be an array of strings",
                line, field
            ),
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
                format!(
                    "Prompt suite line {} field {} must contain only strings",
                    line, field
                ),
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
