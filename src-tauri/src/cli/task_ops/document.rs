use super::operation::TaskOp;

/// Reserved marker that flags a task title as human-set (#738).
pub(crate) const USER_TITLE_PREFIX: &str = "USER:";

#[derive(Debug, Clone, Copy)]
pub(crate) struct TitleLine<'a> {
    leading: &'a str,
    value: &'a str,
}

/// Recognize a frontmatter `title:` line the same way the UI/backend readers do:
/// split on the first `:`, match the key case-insensitively, and keep the
/// leading whitespace so replacement can preserve indentation. `value` is the
/// trimmed raw scalar (still quoted, if it was).
pub(crate) fn title_line(line: &str) -> Option<TitleLine<'_>> {
    let leading_len = line.len() - line.trim_start().len();
    let leading = &line[..leading_len];
    let rest = &line[leading_len..];
    let (key, value) = rest.split_once(':')?;
    if key.trim().eq_ignore_ascii_case("title") {
        Some(TitleLine {
            leading,
            value: value.trim(),
        })
    } else {
        None
    }
}

/// Decode a YAML scalar as broadly as the app's title readers: single-quoted
/// (with `''` unescape), double-quoted (stripped), or bare (trimmed).
fn decode_title_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].replace("''", "'");
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

/// True when a title value carries the reserved human-owned marker.
pub(crate) fn is_user_owned_title(title: &str) -> bool {
    title.trim_start().starts_with(USER_TITLE_PREFIX)
}

/// Normalize a human-set title to the `USER: <value>` form, without double
/// prefixing an already-marked value.
pub(crate) fn user_owned_title(input: &str) -> String {
    let trimmed = input.trim();
    if is_user_owned_title(trimmed) {
        trimmed.to_string()
    } else {
        format!("{USER_TITLE_PREFIX} {trimmed}")
    }
}

// ── Parsed-frontmatter shape ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTask {
    /// Input started with U+FEFF (HIGH-3): preserve+re-emit at render time.
    pub bom: bool,
    /// Dominant line ending in the input (LOW-3). Used for frontmatter delimiter +
    /// inter-line separators in `render`. The body slice is preserved byte-for-byte regardless.
    pub line_ending: &'static str,
    pub has_frontmatter: bool,
    /// Raw frontmatter lines (eol-stripped, `trim_end_matches(['\r','\n'])`).
    pub frontmatter: Vec<String>,
    /// Everything after the closing `---<eol>` (or whole post-BOM input when
    /// `has_frontmatter == false`).
    pub body: String,
}

pub(crate) fn parse_task(s_in: &str) -> ParsedTask {
    // ── BOM peel (HIGH-3) ──────────────────────────────────────────────────
    let (bom, s) = match s_in.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, s_in),
    };

    // ── Line-ending detection (LOW-3) ──────────────────────────────────────
    let line_ending: &'static str = match s.find('\n') {
        Some(i) if i > 0 && s.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    };

    // ── Pull the opening line (CRIT-1 Form B fix) ──────────────────────────
    // The opening's actual byte length is whatever split_inclusive yields —
    // 4 bytes for "---\n", 5 for "---\r\n", 7 for "--- \r\n", etc.
    let mut iter = s.split_inclusive('\n');
    let opening = match iter.next() {
        Some(line) if line.trim() == "---" => line,
        _ => {
            return ParsedTask {
                bom,
                line_ending,
                has_frontmatter: false,
                frontmatter: Vec::new(),
                body: s.to_string(),
            };
        }
    };
    let mut consumed = opening.len();

    // ── Walk to the closing `---` (D.1: tolerate trailing whitespace) ──────
    let mut fm_lines: Vec<String> = Vec::new();
    let mut closed = false;
    for line in iter {
        consumed += line.len();
        let stripped = line.trim_end_matches(['\r', '\n']);
        if stripped.trim() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(stripped.to_string());
    }

    if !closed {
        // Malformed frontmatter — preserve whole post-BOM input as body.
        return ParsedTask {
            bom,
            line_ending,
            has_frontmatter: false,
            frontmatter: Vec::new(),
            body: s.to_string(),
        };
    }

    let body = s[consumed..].to_string();
    ParsedTask {
        bom,
        line_ending,
        has_frontmatter: true,
        frontmatter: fm_lines,
        body,
    }
}

pub(crate) fn render(parsed: &ParsedTask) -> String {
    let eol = parsed.line_ending;
    let mut out = String::with_capacity(parsed.body.len() + 64);
    if parsed.bom {
        out.push('\u{FEFF}');
    }
    if !parsed.has_frontmatter {
        out.push_str(&parsed.body);
        return out;
    }
    out.push_str("---");
    out.push_str(eol);
    for line in &parsed.frontmatter {
        out.push_str(line);
        out.push_str(eol);
    }
    out.push_str("---");
    out.push_str(eol);
    out.push_str(&parsed.body);
    out
}

pub(crate) fn apply_edit(parsed: &ParsedTask, op: &TaskOp) -> ParsedTask {
    match op {
        TaskOp::SetTitle(title) => apply_set_title(parsed, title),
        TaskOp::SetUserTitle(title) => apply_set_title(parsed, &user_owned_title(title)),
        TaskOp::AppendBody(text) => apply_append_body(parsed, text),
        TaskOp::SetBody(text) => apply_set_body(parsed, text),
        TaskOp::Clean => apply_clean(parsed),
    }
}

fn apply_set_body(parsed: &ParsedTask, text: &str) -> ParsedTask {
    let mut updated = parsed.clone();
    // A closed empty block prevents body metadata/BOM from becoming a title
    // or file BOM when this legacy representation is read again.
    updated.has_frontmatter = true;
    updated.body = text.to_owned();
    updated
}

pub(crate) fn apply_set_title(parsed: &ParsedTask, title: &str) -> ParsedTask {
    let escaped = title.replace('\'', "''");
    let new_title_line = format!("title: '{escaped}'");

    // Brand-new file: parsed has empty body and no frontmatter. The set-title
    // matrix says born-LF and BOM-less per entity_creation.rs convention.
    // Also require !parsed.bom so a BOM-only existing file (post-BOM-peel
    // body is "") falls through to the "preserve bom/eol" branch instead of
    // tripping this brand-new shortcut and stripping the BOM (LOW-1, plan §5
    // row 2 — HIGH-3 byte-exact round-trip).
    if !parsed.has_frontmatter && parsed.body.is_empty() && !parsed.bom {
        return ParsedTask {
            bom: false,
            line_ending: "\n",
            has_frontmatter: true,
            frontmatter: vec![new_title_line],
            body: String::new(),
        };
    }

    // No frontmatter, body has content: prepend a fresh frontmatter block.
    if !parsed.has_frontmatter {
        return ParsedTask {
            bom: parsed.bom,
            line_ending: parsed.line_ending,
            has_frontmatter: true,
            frontmatter: vec![new_title_line],
            body: parsed.body.clone(),
        };
    }

    // Has frontmatter — find existing title-shaped line(s) (NIT-5). Match the
    // key case-insensitively (via `title_line`) so `Title:`/`TITLE:` lines are
    // replaced in place instead of leaving a stranded key + a new duplicate.
    let title_count = parsed
        .frontmatter
        .iter()
        .filter(|line| title_line(line).is_some())
        .count();
    if title_count > 1 {
        log::warn!(
            target: "agentscommander_lib::cli::task_ops",
            "TASK.md frontmatter contains {title_count} title: lines; replacing the first only — \
             downstream YAML parsers may pick a different one"
        );
    }

    let mut new_fm: Vec<String> = parsed.frontmatter.clone();
    let title_idx = new_fm.iter().position(|line| title_line(line).is_some());

    match title_idx {
        Some(idx) => {
            // Replace, preserving the existing leading whitespace (so an indented
            // `  title: x` becomes `  title: 'NewTitle'`).
            let leading = title_line(&new_fm[idx]).map_or("", |title| title.leading);
            new_fm[idx] = format!("{leading}{new_title_line}");
        }
        None => {
            new_fm.insert(0, new_title_line);
        }
    }

    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: true,
        frontmatter: new_fm,
        body: parsed.body.clone(),
    }
}

pub(crate) fn apply_append_body(parsed: &ParsedTask, text: &str) -> ParsedTask {
    let trimmed_text = text.trim_end();
    let new_body = if parsed.body.trim().is_empty() {
        format!("{trimmed_text}\n")
    } else {
        // trim_end on the existing body collapses any number of trailing newlines/spaces
        // to zero; the literal "\n\n" inserts exactly one blank-line separator;
        // the appended chunk ends with a single "\n".
        format!("{}\n\n{}\n", parsed.body.trim_end(), trimmed_text)
    };
    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: parsed.has_frontmatter,
        frontmatter: parsed.frontmatter.clone(),
        body: new_body,
    }
}

/// Replace frontmatter title and body with the canonical Clean form.
/// Preserves the file's BOM and dominant line ending for the frontmatter;
/// the body contains no bytes. For
/// an empty input (`parse_task("")`), `parsed.bom == false` and
/// `parsed.line_ending == "\n"`, so the output is the canonical LF/no-BOM
/// Clean form — no special case needed.
pub(crate) fn apply_clean(parsed: &ParsedTask) -> ParsedTask {
    ParsedTask {
        bom: parsed.bom,
        line_ending: parsed.line_ending,
        has_frontmatter: true,
        frontmatter: vec!["title: 'Clean'".to_string()],
        body: String::new(),
    }
}

/// Extract the decoded value of the first `title:` line in the frontmatter, for
/// the semantic idempotence short-circuit (MED-3) and the #738 user-lock guard.
/// Matches the key case-insensitively and decodes single/double-quoted and bare
/// scalars via [`decode_title_scalar`], mirroring the app's other title readers.
/// Returns `None` when the frontmatter has no title-shaped line.
pub(crate) fn title_value_of(parsed: &ParsedTask) -> Option<String> {
    parsed
        .frontmatter
        .iter()
        .find_map(|line| title_line(line).map(|title| decode_title_scalar(title.value)))
}
