use super::*;

fn seam_observations(session_id: Uuid) -> Vec<String> {
    SEAM_OBSERVATIONS
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, _)| *id == session_id)
        .map(|(_, observation)| observation.clone())
        .collect()
}

/// The wake render shape of `phone::messaging::format_pty_wrap`, restated so
/// this test module adds no `pty::inject` -> `phone::messaging` reference.
fn wake_wrap(from: &str, body: &str) -> String {
    format!("\n[Message from {}] {}\n\r", from, body)
}

const SEAM_FROM: &str = "proj:room-1/tech-lead";

/// Drive one peer-wake injection with `settle` and optional scripted rows
/// (`Some(None)` = `Unavailable`, `None` = the default `SessionOver`).
#[expect(
    clippy::option_option,
    reason = "None: SessionOver default; Some(None): Unavailable; Some(Some(rows)): Rows"
)]
async fn seam_run(
    shell: &str,
    text: &str,
    settle: SettleReadiness,
    rows: Option<Option<Vec<String>>>,
) -> (Result<(), String>, Vec<Vec<u8>>, Vec<String>) {
    let (app, id, backend) = typing_hold_app(shell).await;
    *backend.screen_rows.lock().unwrap() = rows;
    let result =
        inject_peer_wake_text_into_session(app.handle(), id, text, "msg-seam", settle).await;
    (result, recorded_writes(&backend, id), seam_observations(id))
}

/// T1 - send_enter: the wrap's final `\n\r` is removed, then two Enters.
#[tokio::test]
async fn seam_send_enter_strips_the_wrap_suffix_then_two_enters() {
    let payload = wake_wrap(SEAM_FROM, "please review the plan");
    let (result, writes, _) = seam_run("claude", &payload, SettleReadiness::Ready, None).await;
    result.unwrap();
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0], payload.as_bytes()[..payload.len() - 2].to_vec());
    assert_eq!(payload.len() - writes[0].len(), 2);
    assert!(!writes[0].ends_with(b"\r") && !writes[0].ends_with(b"\n"));
    assert_eq!(writes[1], b"\r".to_vec());
    assert_eq!(writes[2], b"\r".to_vec());
}

/// T2 - plain shell: byte-identical single write, no Enter, no observation.
#[tokio::test]
async fn seam_plain_shell_writes_payload_byte_for_byte() {
    let payload = wake_wrap(SEAM_FROM, "please review the plan");
    let (result, writes, observations) = seam_run(
        "muse",
        &payload,
        SettleReadiness::Ready,
        Some(Some(vec!["chrome".to_string()])),
    )
    .await;
    result.unwrap();
    assert_eq!(writes, vec![payload.as_bytes().to_vec()]);
    assert!(observations.is_empty(), "{observations:?}");
}

async fn assert_send_enter_first_write_unchanged(text: &str) {
    let (result, writes, _) = seam_run("claude", text, SettleReadiness::Unknown, None).await;
    result.unwrap();
    assert_eq!(writes.len(), 3, "text={text:?}");
    assert_eq!(writes[0], text.as_bytes().to_vec(), "text={text:?}");
}

/// T2a - Telegram-shaped text ending in a single `\n`.
#[tokio::test]
async fn seam_send_enter_keeps_a_trailing_lone_newline() {
    assert_send_enter_first_write_unchanged("hello\n").await;
}

/// T2b - text ending in a single `\r`.
#[tokio::test]
async fn seam_send_enter_keeps_a_trailing_lone_carriage_return() {
    assert_send_enter_first_write_unchanged("hello\r").await;
}

/// T2c - Loop-prompt-shaped text with no trailing newline.
#[tokio::test]
async fn seam_send_enter_keeps_text_without_trailing_newline() {
    assert_send_enter_first_write_unchanged("Run the scheduled loop step now.").await;
}

/// T2d - the suffix is present but not final.
#[tokio::test]
async fn seam_send_enter_keeps_a_non_final_wrap_suffix() {
    assert_send_enter_first_write_unchanged("hello\n\r\n").await;
}

/// T3 - interior newlines survive the strip.
#[tokio::test]
async fn seam_send_enter_keeps_interior_newlines() {
    let payload = wake_wrap(SEAM_FROM, "a\nb");
    let (result, writes, _) = seam_run("claude", &payload, SettleReadiness::Ready, None).await;
    result.unwrap();
    assert!(payload.ends_with("a\nb\n\r"));
    assert_eq!(
        writes[0],
        payload.trim_end_matches("\n\r").as_bytes().to_vec()
    );
    assert!(writes[0].ends_with(b"a\nb"));
}

/// T4 - an all-newline payload is never stripped to empty; Enters follow.
#[tokio::test]
async fn seam_send_enter_never_strips_to_empty() {
    let (result, writes, _) = seam_run("claude", "\n\r", SettleReadiness::Ready, None).await;
    result.unwrap();
    assert_eq!(
        writes,
        vec![b"\n\r".to_vec(), b"\r".to_vec(), b"\r".to_vec()]
    );
}

/// T5 - three observations, in order T0, T1, T2, each with the settle.
#[tokio::test]
async fn seam_observations_are_emitted_in_order() {
    let payload = wake_wrap(SEAM_FROM, "please review the plan");
    let (result, _, observations) = seam_run(
        "claude",
        &payload,
        SettleReadiness::TimedOut,
        Some(Some(vec!["? for shortcuts".to_string()])),
    )
    .await;
    result.unwrap();
    assert_eq!(observations.len(), 3, "{observations:?}");
    for (observation, instant) in observations.iter().zip(["T0", "T1", "T2"]) {
        assert!(
            observation.starts_with(&format!("[inject] seam={instant} settle=TimedOut rows=1 ")),
            "{observation}"
        );
        assert!(
            observation.ends_with("idx=0:? for shortcuts"),
            "{observation}"
        );
    }
}

/// T6 - unavailable rows are logged and the injection still succeeds.
#[tokio::test]
async fn seam_unavailable_rows_never_fail_the_injection() {
    let payload = wake_wrap(SEAM_FROM, "please review the plan");
    let (result, writes, observations) =
        seam_run("claude", &payload, SettleReadiness::Unknown, Some(None)).await;
    assert_eq!(result, Ok(()));
    assert_eq!(writes.len(), 3);
    assert_eq!(observations.len(), 3);
    assert_eq!(
        observations[0],
        "[inject] seam=T0 settle=Unknown rows=unavailable"
    );
}

fn rows(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn observe(body: &str, screen: &[String]) -> String {
    format_submit_seam_observation(
        "T0",
        SettleReadiness::Ready,
        Some(screen),
        &wake_wrap(SEAM_FROM, body),
    )
}

fn assert_fixture_is_screen_shaped(screen: &[String]) {
    for row in screen {
        assert!(!row.contains('\n') && !row.contains('\r'), "{row:?}");
    }
}

/// T8 - the last six non-empty rows, top to bottom, with real indices and
/// trailing box-drawing noise removed.
#[test]
fn seam_observation_keeps_the_last_six_non_empty_rows() {
    let screen: Vec<String> = (0..20)
        .map(|idx| {
            if idx % 2 == 1 {
                "   ".to_string()
            } else {
                format!("chrome {idx} \u{2500}\u{2500}  ")
            }
        })
        .collect();
    let rendered = observe("zzzzzzzzzzzzzz", &screen);
    assert_eq!(
        rendered,
        "[inject] seam=T0 settle=Ready rows=20 payload_on_screen=false payload_rows=0 \
         | idx=8:chrome 8 | idx=10:chrome 10 | idx=12:chrome 12 | idx=14:chrome 14 \
         | idx=16:chrome 16 | idx=18:chrome 18"
    );
}

/// T9 - the settle readiness is rendered as given.
#[test]
fn seam_observation_renders_settle_readiness() {
    let payload = wake_wrap(SEAM_FROM, "body");
    assert!(
        format_submit_seam_observation("T1", SettleReadiness::TimedOut, None, &payload)
            .contains(" settle=TimedOut ")
    );
    assert_eq!(
        format_submit_seam_observation("T1", SettleReadiness::Unknown, None, &payload),
        "[inject] seam=T1 settle=Unknown rows=unavailable"
    );
    assert!(
        format_submit_seam_observation("T1", SettleReadiness::Ready, Some(&[]), &payload)
            .starts_with("[inject] seam=T1 settle=Ready rows=0 ")
    );
}

/// T7a - a multirow body: each body line is its own row and is redacted.
#[test]
fn seam_redaction_hides_each_row_of_a_multirow_body() {
    let body = "line one of the body\nline two of the body";
    let screen = rows(&[
        "line one of the body",
        "line two of the body",
        "? for shortcuts",
        "\u{273B} Welcome to Claude Code!",
    ]);
    assert_fixture_is_screen_shaped(&screen);
    assert!(body.contains(&screen[0]) && body.contains(&screen[1]));
    let rendered = observe(body, &screen);
    assert!(!rendered.contains("line one") && !rendered.contains("line two"));
    assert!(rendered.contains(" payload_on_screen=true payload_rows=2 "));
    assert!(rendered.contains("idx=0:<payload> chars=20 marker=false"));
    assert!(rendered.contains("idx=1:<payload> chars=20 marker=false"));
    assert!(rendered.contains("idx=2:? for shortcuts"));
    assert!(rendered.contains("idx=3:\u{273B} Welcome to Claude Code!"));
}

/// T7b - a wrapped row: a 120-char body cut at char 47 into two rows.
#[test]
fn seam_redaction_hides_both_halves_of_a_wrapped_row() {
    let body: String = "the quick brown fox jumps over a lazy dog while seven wizards \
                        box nimbly and quartz judges vex the grumpy sphinx of black"
        .chars()
        .take(120)
        .collect();
    assert_eq!(body.chars().count(), 120);
    let head: String = body.chars().take(47).collect();
    let tail: String = body.chars().skip(47).collect();
    let screen = vec![head.clone(), tail.clone()];
    assert_fixture_is_screen_shaped(&screen);
    assert!(body.contains(&head) && body.contains(&tail));
    let rendered = observe(&body, &screen);
    assert!(rendered.contains(" payload_rows=2 "), "{rendered}");
    assert!(!rendered.contains(tail.trim()) && !rendered.contains(head.trim()));
}

/// T7c - a short body is redacted by the substring clause.
#[test]
fn seam_redaction_hides_a_short_body_row() {
    let screen = rows(&["ok"]);
    assert!(wake_wrap(SEAM_FROM, "ok").contains(&screen[0]));
    let rendered = observe("ok", &screen);
    assert!(rendered.contains(" payload_on_screen=true payload_rows=1 "));
    assert!(rendered.ends_with("idx=0:<payload> chars=2 marker=false"));
}

/// T7d - chrome plus the first 12 body chars: not a payload substring, but
/// it shares an 8-char window, so it is redacted.
#[test]
fn seam_redaction_hides_a_suffix_only_overlap() {
    let body = "deliver the parcel to the north gate";
    let row = format!("* Thinking... {}", &body[..12]);
    let payload = wake_wrap(SEAM_FROM, body);
    assert!(row.ends_with("deliver the "));
    assert!(!payload.contains(row.trim()));
    let rendered = observe(body, std::slice::from_ref(&row));
    assert!(rendered.contains(" payload_rows=1 "), "{rendered}");
    assert!(!rendered.contains("deliver"));
}

/// T7e - the STATED leak limit, not a no-leak guarantee: a row sharing only
/// six consecutive body chars is printed verbatim. Strengthening the rule
/// must edit this test deliberately.
#[test]
fn seam_redaction_leak_limit_prints_a_six_char_fragment() {
    let body = "deliver the parcel to the north gate";
    let screen = rows(&["Tokens:parcel|42"]);
    assert!(screen[0].contains("parcel") && body.contains("parcel"));
    let rendered = observe(body, &screen);
    assert!(rendered.contains(" payload_on_screen=false payload_rows=0 "));
    assert!(rendered.ends_with("idx=0:Tokens:parcel|42"), "{rendered}");
}

/// T7f - a redacted row keeps its composer-marker signal.
#[test]
fn seam_redaction_keeps_the_composer_marker() {
    let body = "deliver the parcel to the north gate";
    let screen = rows(&["\u{276F} deliver the parcel", "deliver the parcel to"]);
    assert!(body.contains("deliver the parcel"));
    let rendered = observe(body, &screen);
    assert!(
        rendered.contains("idx=0:<payload> chars=20 marker=true"),
        "{rendered}"
    );
    assert!(
        rendered.contains("idx=1:<payload> chars=21 marker=false"),
        "{rendered}"
    );
}

/// T7g - the absence leg: chrome-only rows all survive verbatim.
#[test]
fn seam_redaction_leaves_chrome_only_rows_verbatim() {
    let body = "deliver the parcel to the north gate";
    let screen = rows(&[
        "? for shortcuts",
        "\u{273B} Welcome to Claude Code!",
        "\u{276F}",
    ]);
    let payload = wake_wrap(SEAM_FROM, body);
    for row in &screen {
        assert!(!payload.contains(row.as_str()), "{row}");
    }
    assert_eq!(
        observe(body, &screen),
        "[inject] seam=T0 settle=Ready rows=3 payload_on_screen=false payload_rows=0 \
         | idx=0:? for shortcuts | idx=1:\u{273B} Welcome to Claude Code! | idx=2:\u{276F}"
    );
}
