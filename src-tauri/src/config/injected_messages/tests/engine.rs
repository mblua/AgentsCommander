use super::*;

// ---- 1..7: the pure engine -------------------------------------------

#[test]
fn default_context_alert_template_is_pinned() {
    assert_eq!(
        DEFAULT_CONTEXT_ALERT_TEMPLATE,
        "[AC context alert] `%MEMBER%` in `%WORKGROUP%` reached threshold(s): %THRESHOLDS%. No \
         action taken; you decide any follow-up."
    );
    assert_eq!(DEFAULT_CONTEXT_ALERT_TEMPLATE.chars().count(), 125);
    assert_eq!(DEFAULT_CONTEXT_ALERT_TEMPLATE.len(), 125);
    assert_eq!(
        sha256_hex(DEFAULT_CONTEXT_ALERT_TEMPLATE.as_bytes()),
        PINNED_SHA256
    );
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_MEMBER));
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_WORKGROUP));
    assert!(DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_THRESHOLDS));
    assert!(!DEFAULT_CONTEXT_ALERT_TEMPLATE.contains(TOKEN_OBSERVED));
    // The shipped-default list must always recognize what it ships.
    assert!(KNOWN_MESSAGES[0]
        .known_default_sha256
        .contains(&PINNED_SHA256));
}

#[test]
fn template_override_renders_user_text() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let template = "## %MEMBER%\n\n- **%WORKGROUP%** at `%OBSERVED%`\n- crossed %THRESHOLDS%";
    assert_eq!(
        render_with_template(template, &alert_values(), &warnings),
        "## dev-rust\n\n- **wg-2-dev-team** at `91%`\n- crossed 50%, 75%, 90%"
    );
}

#[test]
fn markdown_and_multiline_survive_round_trip() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let template = "# Heading\n\n**bold** and `code` and *stars*\n\n- item one\n- item \
        two\n\n```rust\nfn main() {}\n```\n\n> quote %MEMBER%\n\n| a | b |\n|---|---|\n| 1 | 2 |";
    let expected = template.replace(TOKEN_MEMBER, "dev-rust");
    assert_eq!(
        render_with_template(template, &alert_values(), &warnings),
        expected
    );
}

#[test]
fn unknown_placeholder_is_left_literal() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let rendered = render_with_template(
        "%FOO% %MEMBER% %BAR_2% 100% done",
        &alert_values(),
        &warnings,
    );
    assert_eq!(rendered, "%FOO% dev-rust %BAR_2% 100% done");
    assert!(!rendered.is_empty());
    // The unknown-token grammar must not fire on ordinary prose full of
    // bare percent signs, which is what rendered thresholds look like.
    let (_, unknown) = expand_tokens("50%, 75%, 90% and 100% del contexto", &alert_values());
    assert!(unknown.is_empty(), "unexpected unknown tokens: {unknown:?}");
    // Case-insensitive recognition, for the warning only: `%member%` is not
    // substituted, but it is reported rather than silently ignored.
    let (out, unknown) = expand_tokens("%member%", &alert_values());
    assert_eq!(out, "%member%");
    assert_eq!(unknown, vec!["%member%".to_string()]);
}

#[test]
fn expansion_is_single_pass() {
    let values = [(TOKEN_MEMBER, "dev-%WORKGROUP%"), (TOKEN_WORKGROUP, "wg-2")];
    let (out, _) = expand_tokens("m=%MEMBER% w=%WORKGROUP%", &values);
    assert_eq!(out, "m=dev-%WORKGROUP% w=wg-2");
    assert_ne!(out, "m=dev-wg-2 w=wg-2");
    // A substituted value is never rescanned, so an adjacent token cannot
    // be manufactured out of one.
    let (out, _) = expand_tokens("%THRESHOLDS%OBSERVED%", &alert_values());
    assert_eq!(out, "50%, 75%, 90%OBSERVED%");
}

#[test]
fn sanitize_strips_c0_c1_and_del() {
    let hostile = "a\u{1b}[31mred\u{1b}]0;title\u{7}b\rc\u{7f}d\u{85}e\n\tkeep `md` *stars*";
    let out = sanitize(hostile);
    assert_eq!(out, "a[31mred]0;titlebcde\n\tkeep `md` *stars*");
    assert!(!out.contains('\u{1b}'));
    assert!(!out.contains('\r'));
    assert!(!out.contains('\u{7f}'));
    assert!(!out.contains('\u{85}'));
    assert!(out.contains('\n') && out.contains('\t'));
}

#[test]
fn sanitize_strips_bidi_and_separator_class() {
    let hostile =
        "a\u{202e}b\u{2066}c\u{2069}d\u{2028}e\u{200f}f\u{61c}g\u{200e}h\u{2029}i\u{202a}j";
    let out = sanitize(hostile);
    assert_eq!(out, "abcdefghij");
    for forbidden in [
        '\u{202e}', '\u{2066}', '\u{2069}', '\u{2028}', '\u{200f}', '\u{61c}', '\u{200e}',
        '\u{2029}', '\u{202a}',
    ] {
        assert!(!out.contains(forbidden), "kept U+{:04X}", forbidden as u32);
    }
}

#[test]
fn sanitize_neutralizes_response_marker_prefix() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let out = sanitize("x %%AC_RESPONSE::abc::START%% y %%AC_RESPONSE::abc::END%% z");
    assert_eq!(
        out,
        "x %AC_RESPONSE::abc::START%% y %AC_RESPONSE::abc::END%% z"
    );
    assert!(!out.contains("%%AC_RESPONSE::"));

    // The two literals `scan_response_markers` reconstructs per in-flight
    // rid. Neither may survive, for any run length.
    let start_marker = format!("%%AC_RESPONSE::{}::START%%", "abc");
    let end_marker = format!("%%AC_RESPONSE::{}::END%%", "abc");
    assert!(!out.contains(&start_marker));
    assert!(!out.contains(&end_marker));

    // Runs of two, three and four. A single `replace` turns the run of
    // three back into a match, which is exactly why the collapse has to be
    // a fixed point rather than one pass of substitution.
    for run in 2..=4 {
        let hostile = format!("{}AC_RESPONSE::abc::START%%", "%".repeat(run));
        let out = sanitize(&hostile);
        assert_eq!(out, "%AC_RESPONSE::abc::START%%", "run of {run}");
        assert!(!out.contains("%%AC_RESPONSE::"), "run of {run}");
        assert!(!out.contains(&start_marker), "run of {run}");
        assert_eq!(sanitize(&out), out, "not idempotent for run of {run}");
    }

    // Expansion runs before sanitization, so `%OBSERVED%` hands a third `%`
    // to an operator who only typed two.
    let expanded = render_with_template(
        "%OBSERVED%%%AC_RESPONSE::abc::START%%",
        &alert_values(),
        &warnings,
    );
    assert_eq!(expanded, "91%AC_RESPONSE::abc::START%%");
    assert!(!expanded.contains("%%AC_RESPONSE::"));
    assert!(!expanded.contains(&start_marker));
    assert_eq!(sanitize(&expanded), expanded);

    // Runs of length 0 and 1 are left exactly as they are.
    assert_eq!(sanitize("AC_RESPONSE::x"), "AC_RESPONSE::x");
    assert_eq!(sanitize("%AC_RESPONSE::x"), "%AC_RESPONSE::x");

    // The step order is load-bearing in one direction: step 3 must stay
    // AFTER step 2, because step 2 can bring two `%` together that a
    // stripped scalar separated. Nothing else in the matrix distinguishes
    // the two orders, so a refactor that swaps them would leave the whole
    // suite green while reopening the hole this collapse closes.
    assert!(!sanitize("%\u{202e}%AC_RESPONSE::abc::START%%").contains("%%AC_RESPONSE::"));
    assert_eq!(
        sanitize("%\u{202e}%AC_RESPONSE::abc::START%%"),
        "%AC_RESPONSE::abc::START%%"
    );

    // Idempotence over the whole set, including adjacent occurrences, where
    // the backward walk of one must not eat the other's text.
    for probe in [
        "x %%AC_RESPONSE::abc::START%% y %%AC_RESPONSE::abc::END%% z",
        "%%%%AC_RESPONSE::",
        "AC_RESPONSE::%%AC_RESPONSE::",
        "%%AC_RESPONSE::%%%AC_RESPONSE::",
        "no marker here",
        "",
    ] {
        let once = sanitize(probe);
        assert_eq!(sanitize(&once), once, "not idempotent: {probe:?}");
        assert!(!once.contains("%%AC_RESPONSE::"), "survived: {probe:?}");
    }
}

#[test]
fn render_truncates_at_cap_on_char_boundary() {
    let warnings = crate::config::injected_messages::MessageWarnings::new();
    let template = "🚨".repeat(4096);
    let out = render_with_template(&template, &alert_values(), &warnings);
    assert!(out.len() <= MAX_RENDERED_BYTES);
    // A 4-byte scalar never straddles the cap: 4096 is divisible by 4 here,
    // but the invariant is that the result is valid UTF-8 either way.
    assert_eq!(out.chars().count(), out.len() / 4);
    assert!(std::str::from_utf8(out.as_bytes()).is_ok());

    let odd = format!("{}{}", "a".repeat(MAX_RENDERED_BYTES - 1), "🚨");
    let out = render_with_template(&odd, &alert_values(), &warnings);
    assert!(out.len() <= MAX_RENDERED_BYTES);
    assert_eq!(out.len(), MAX_RENDERED_BYTES - 1);
}

#[test]
fn message_owner_shared_healthy_repeat_and_poison() {
    let warnings = std::sync::Arc::new(MessageWarnings::new());
    let shared = std::sync::Arc::clone(&warnings);
    assert!(warn_once("shared".into(), "first", &warnings));
    assert!(!warn_once("shared".into(), "repeat", &shared));
    let isolated = MessageWarnings::new();
    assert!(warn_once("shared".into(), "isolated", &isolated));
    let poisoned = std::sync::Arc::clone(&warnings);
    assert!(std::thread::spawn(move || {
        let _guard = poisoned
            .warned
            .get_or_init(|| std::sync::Mutex::new(Default::default()))
            .lock()
            .unwrap();
        panic!("poison message warning owner");
    })
    .join()
    .is_err());
    assert!(warn_once("shared".into(), "poisoned first", &warnings));
    assert!(warn_once("shared".into(), "poisoned repeat", &shared));
    assert!(warn_once("other".into(), "poisoned other", &shared));
}

#[test]
fn registry_cache_keeps_first_file_snapshot() {
    let cache = MessageRegistry::default();
    let warnings = MessageWarnings::new();
    let directory = tempdir();
    assert!(cache.templates.get().is_none());
    ensure_injected_messages(directory.path(), &warnings).unwrap();
    let loads = std::cell::Cell::new(0);
    let first = cache.get_or_init(|| {
        loads.set(loads.get() + 1);
        load_registry_from_dir(directory.path(), &warnings)
    });
    assert_eq!(loads.get(), 1);
    let keys: Vec<_> = first.keys().cloned().collect();
    let mut expected: Vec<_> = KNOWN_MESSAGES
        .iter()
        .map(|spec| spec.id.to_string())
        .collect();
    expected.sort();
    assert_eq!(keys, expected);
    let mut owned = first.get(CONTEXT_ALERT_MESSAGE_ID).unwrap().clone();
    owned.push_str(" independent edit");
    assert_eq!(
        first[CONTEXT_ALERT_MESSAGE_ID],
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    assert_ne!(owned, first[CONTEXT_ALERT_MESSAGE_ID]);

    let path = main_path(directory.path());
    let source = read(&path);
    std::fs::write(
        &path,
        source.replace(DEFAULT_CONTEXT_ALERT_TEMPLATE, "edited template"),
    )
    .unwrap();
    let cached = cache.get_or_init(|| {
        loads.set(loads.get() + 1);
        load_registry_from_dir(directory.path(), &warnings)
    });
    assert!(std::ptr::eq(first, cached));
    assert_eq!(loads.get(), 1);
    assert_eq!(
        cached[CONTEXT_ALERT_MESSAGE_ID],
        DEFAULT_CONTEXT_ALERT_TEMPLATE
    );
    let fresh = MessageRegistry::default();
    assert!(fresh.templates.get().is_none());
    let edited = fresh.get_or_init(|| {
        loads.set(loads.get() + 1);
        load_registry_from_dir(directory.path(), &warnings)
    });
    assert_eq!(loads.get(), 2);
    assert_eq!(edited[CONTEXT_ALERT_MESSAGE_ID], "edited template");
    assert_eq!(edited.keys().cloned().collect::<Vec<_>>(), keys);
}

// ---- #4035: one forbidden-scalar policy, two consumers -----------------

/// Independent test-only oracle: the forbidden intervals written out here as
/// expected data, never derived from the production predicate.
const FORBIDDEN_ORACLE: [(u32, u32); 7] = [
    (0x0000, 0x0008),
    (0x000B, 0x001F),
    (0x007F, 0x009F),
    (0x061C, 0x061C),
    (0x200E, 0x200F),
    (0x2028, 0x202E),
    (0x2066, 0x2069),
];

fn assert_probe(scalar: char, forbidden: bool) {
    use crate::pty::inject::{validate_pty_input_text, PtyInputTextErrorKind};
    let text = format!("é🙂{scalar}x");
    let code = scalar as u32;
    let result = validate_pty_input_text(&text);
    if forbidden {
        assert_eq!(sanitize(&text), "é🙂x", "sanitize kept U+{code:04X}");
        let err = result.expect_err("validator accepted a forbidden scalar");
        assert_eq!(err.kind, PtyInputTextErrorKind::ForbiddenScalar);
        assert_eq!((err.byte_offset, err.scalar_offset), (6, 2));
        assert_eq!(err.code_point, Some(code));
        let display = err.to_string();
        assert_eq!(
            display,
            format!("invalid_text:byte=6:scalar=2:codepoint=U+{code:04X}")
        );
        assert!(
            !display.contains('é') && !display.contains('🙂'),
            "{display}"
        );
    } else {
        assert_eq!(sanitize(&text), text, "sanitize changed U+{code:04X}");
        assert_eq!(result, Ok(()), "validator rejected U+{code:04X}");
    }
}

#[test]
fn scalar_policy_preserves_both_paths_and_edges() {
    for (start, end) in FORBIDDEN_ORACLE {
        for code in start..=end {
            assert_probe(char::from_u32(code).unwrap(), true);
        }
        for edge in [start.checked_sub(1), Some(end + 1)].into_iter().flatten() {
            let in_oracle = FORBIDDEN_ORACLE
                .iter()
                .any(|&(s, e)| (s..=e).contains(&edge));
            if !in_oracle {
                assert_probe(char::from_u32(edge).unwrap(), false);
            }
        }
    }
    for accepted in ['\t', '\n', 'A', '~', ' ', 'é', 'ñ', '🙂', '\u{10FFFF}'] {
        assert_probe(accepted, false);
    }
    assert_eq!(sanitize("a\r\nb\rc\t\n"), "a\nbc\t\n");
    assert_eq!(
        sanitize("%\u{202e}%AC_RESPONSE::abc::START%%"),
        "%AC_RESPONSE::abc::START%%"
    );
}

#[test]
fn validator_error_precedence_is_unchanged() {
    use crate::pty::backend::PTY_INPUT_MAX_BYTES as MAX;
    use crate::pty::inject::{validate_pty_input_text, PtyInputTextErrorKind};
    let empty = validate_pty_input_text("").unwrap_err();
    assert_eq!(empty.kind, PtyInputTextErrorKind::Empty);
    assert_eq!(
        (empty.byte_offset, empty.scalar_offset, empty.code_point),
        (0, 0, None)
    );
    assert_eq!(empty.to_string(), "invalid_text:empty");

    let ascii = "a".repeat(MAX + 1);
    let crossing = format!("{}é", "a".repeat(MAX - 1));
    let forbidden_first = format!("\u{202e}{}", "a".repeat(MAX));
    for (text, scalars) in [(ascii, MAX), (crossing, MAX), (forbidden_first, MAX - 2)] {
        assert!(text.len() > MAX);
        let err = validate_pty_input_text(&text).unwrap_err();
        assert_eq!(err.kind, PtyInputTextErrorKind::TooLarge);
        assert_eq!((err.byte_offset, err.scalar_offset), (MAX, scalars));
        assert_eq!(err.code_point, None);
        assert_eq!(err.to_string(), "payload_too_large");
    }
}
