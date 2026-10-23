use super::registry;
use super::warn_once;
use super::MessageSpec;
use super::KNOWN_MESSAGES;
use super::MAX_RENDERED_BYTES;

// ---------------------------------------------------------------------------
// Pure rendering engine
// ---------------------------------------------------------------------------

/// True for `%`, one or more of `A-Za-z0-9_`, `%`. The character class is wider
/// than the substitution grammar on purpose: substitution matches known tokens
/// case-sensitively, while recognition for the unknown-token warning is
/// case-insensitive, so an operator who types `%member%` is told about it
/// instead of finding a literal `%member%` in a coordinator's terminal.
fn unknown_token_len(rest: &str) -> Option<usize> {
    let mut chars = rest.char_indices();
    if !matches!(chars.next(), Some((_, '%'))) {
        return None;
    }
    let mut body = 0usize;
    for (offset, ch) in chars {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            body += 1;
            continue;
        }
        if ch == '%' && body > 0 {
            return Some(offset + 1);
        }
        return None;
    }
    None
}

/// Single left-to-right pass. Substituted values are never rescanned, so a value
/// that itself contains `%WORKGROUP%` survives verbatim. The chained
/// `for (k, v) in tokens { s = s.replace(k, v) }` form is prohibited: it
/// rescans, and corrupts a template such as `%THRESHOLDS%OBSERVED%` even with
/// today's upstream-validated values.
pub(super) fn expand_tokens(template: &str, values: &[(&str, &str)]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len());
    let mut unknown: Vec<String> = Vec::new();
    let mut index = 0usize;

    while index < template.len() {
        let rest = &template[index..];
        if rest.starts_with('%') {
            if let Some((token, value)) = values.iter().find(|(token, _)| rest.starts_with(token)) {
                out.push_str(value);
                index += token.len();
                continue;
            }
            if let Some(length) = unknown_token_len(rest) {
                let token = &rest[..length];
                if !unknown.iter().any(|seen| seen == token) {
                    unknown.push(token.to_string());
                }
                out.push_str(token);
                index += length;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else { break };
        out.push(ch);
        index += ch.len_utf8();
    }

    (out, unknown)
}

const RESPONSE_MARKER_BODY: &str = "AC_RESPONSE::";

/// Collapses, for every occurrence of `AC_RESPONSE::`, the maximal run of `%`
/// immediately preceding it down to a single `%` when that run is two
/// characters or longer. Runs of length 0 or 1 are left untouched and nothing
/// else in the text changes.
///
/// A single `replace("%%AC_RESPONSE::", "%AC_RESPONSE::")` is not sufficient:
/// `%%%AC_RESPONSE::` becomes `%%AC_RESPONSE::` and matches the scanner again,
/// and the bar is low because expansion runs first, so `%OBSERVED%` expands to
/// `91%` and gets a third `%` from an operator who typed two. The obvious
/// `while contains { replace }` repair is prohibited as a denial of service:
/// truncation is step 4, so at step 3 the text is still the operator's whole
/// template with no bound on its length.
///
/// This form is O(n) and a fixed point by construction: the runs it produces
/// are already of length 1 or 0, so sanitizing twice equals sanitizing once.
fn collapse_response_marker_prefix(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(hit) = input[cursor..].find(RESPONSE_MARKER_BODY) {
        let at = cursor + hit;
        let mut run_start = at;
        // `%` is ASCII 0x25, a byte no multi-byte UTF-8 scalar can contain, so
        // walking back over bytes can never land inside a scalar. Bounding the
        // walk at `cursor` also keeps it inside the not-yet-copied region; it
        // changes no result, since the byte before `cursor` is always the `:`
        // ending the previous marker.
        while run_start > cursor && bytes[run_start - 1] == b'%' {
            run_start -= 1;
        }
        out.push_str(&input[cursor..run_start]);
        if run_start < at {
            out.push('%');
        }
        out.push_str(RESPONSE_MARKER_BODY);
        cursor = at + RESPONSE_MARKER_BODY.len();
    }
    out.push_str(&input[cursor..]);
    out
}

/// Applied to the fully expanded string, in the order below.
///
/// Step 2 is the correctness core: the internal-notice path reaches
/// `inject_text_into_session` with no validation at all, so without it this
/// module would deliver into a coordinator's terminal exactly the bidi and
/// separator class AC refuses on every other injection route.
pub(super) fn sanitize(value: &str) -> String {
    // 1. CRLF to LF.
    let converted = value.replace("\r\n", "\n");
    // 2. Drop every scalar AC's own PTY-input validator forbids, keeping the
    //    two it allows. Derived, never restated.
    let mut out: String = converted
        .chars()
        .filter(|ch| matches!(ch, '\n' | '\t') || !crate::pty::inject::is_forbidden_pty_scalar(*ch))
        .collect();
    // 3. Neutralize the reply-marker prefix `pty/output.rs` scans for in PTY
    //    output, which injected text echoes into. Stays after step 2, which can
    //    bring two `%` together that a stripped scalar separated, as in
    //    `%\u{202e}%AC_RESPONSE::`. Test 6c guards that order.
    if out.contains(RESPONSE_MARKER_BODY) {
        out = collapse_response_marker_prefix(&out);
    }
    // 4. Truncate on a character boundary. `str::floor_char_boundary` is
    //    unstable on the pinned toolchain.
    let mut end = MAX_RENDERED_BYTES.min(out.len());
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out
}

/// Pure seam: expand then sanitize. Tests and future ids feed it arbitrary
/// values, so it may not assume the upstream validation `InternalSystemNotice`
/// applies to `member` and `workgroup`.
pub(crate) fn render_with_template(template: &str, values: &[(&str, &str)]) -> String {
    let (expanded, unknown) = expand_tokens(template, values);
    for token in unknown {
        warn_once(
            format!("token:{}", token),
            &format!(
                "[injected-messages] unknown placeholder {} left as literal text",
                token
            ),
        );
    }
    sanitize(&expanded)
}

/// Blankness gate 2, after rendering. `sanitize` can empty a template that
/// `trim()` called non-blank at the parse boundary, because the bidi and format
/// scalars it strips are not `White_Space`: a template made only of U+202E, of
/// U+200E and U+200F, of U+061C, of U+2066 through U+2069, or of a
/// non-whitespace C0 scalar such as BEL passes gate 1 and renders to nothing.
/// Without this the alert is lost with no warning at all, `format_wake_content`
/// injects a bare `"\n\n\r"`, and `validate_pty_input_text("")` is `Err(Empty)`.
///
/// A pure seam on purpose, so it is testable without installing anything in the
/// process-global registry, which is empty under `cfg(test)` by rule.
pub(super) fn render_resolved(
    id: &str,
    template: &str,
    from_registry: bool,
    values: &[(&str, &str)],
) -> String {
    let rendered = render_with_template(template, values);
    if !rendered.is_empty() {
        return rendered;
    }
    // One retry, never a loop: a shipped default can never render empty (N17),
    // so no second failure is reachable. `from_registry` is what distinguishes
    // an operator's template from an embedded default rendering empty, and an
    // id absent from `KNOWN_MESSAGES` never reaches here.
    if from_registry {
        if let Some(spec) = spec_for(id) {
            warn_once(
                format!("empty:{}", id),
                &format!(
                    "[injected-messages] the `{}` template renders to nothing after sanitization; \
                     the built-in default is used instead",
                    id
                ),
            );
            return render_with_template(spec.default_template, values);
        }
    }
    rendered
}

/// Resolves `id` from the registry, or from the embedded default, then renders.
/// Never fails and never blocks an alert. Returns a non-empty string for every
/// id in `KNOWN_MESSAGES`; it returns `""` only for an id this binary does not
/// know, which no production call site can produce.
pub(crate) fn render(id: &str, values: &[(&str, &str)]) -> String {
    let (template, from_registry) = match registry().get(id) {
        Some(template) => (template.clone(), true),
        None => match spec_for(id) {
            Some(spec) => (spec.default_template.to_string(), false),
            // Not a message this binary knows; there is nothing to render.
            None => return String::new(),
        },
    };
    render_resolved(id, &template, from_registry, values)
}

pub(super) fn spec_for(id: &str) -> Option<&'static MessageSpec> {
    KNOWN_MESSAGES.iter().find(|spec| spec.id == id)
}
