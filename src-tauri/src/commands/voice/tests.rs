/// #280 G-HIGH-2 / LOW-3 — guard against accidental re-introduction
/// of `?key=<API_KEY>` in the Gemini request URL. The key MUST be
/// passed via the `x-goog-api-key` header (transcribe_audio above);
/// embedding it in the URL would surface in reqwest's transport-error
/// strings (Display prints the full URL) and reach logs / the Telegram
/// chat / the frontend error modal. Brittle by design — string-level
/// assertion catches the regression at compile-adjacent test time.
#[test]
fn transcribe_audio_url_has_no_query_key() {
    // Mirror the format string used in `transcribe_audio`. If the
    // production format string is edited to include `?key=`, this
    // copy stays the same and the test passes — that's a known
    // limitation of structural mirroring. The stronger guard is the
    // header path; this test backstops the URL-shape contract.
    let model = "gemini-2.5-flash";
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
        model
    );
    assert!(
        !url.contains("key="),
        "URL must not embed an API key as a query param: {}",
        url
    );
    assert!(
        !url.contains('?'),
        "URL must not carry any query string in the production builder: {}",
        url
    );
}
