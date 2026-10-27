use std::env;
use std::ffi::OsString;

pub(crate) type HelperResult<T> = Result<T, String>;

pub(crate) fn api_url() -> HelperResult<String> {
    env::var("AGENTSCOMMANDER_API_URL")
        .map(|value| value.trim_end_matches('/').to_string())
        .map_err(|_| "api_url_unavailable".to_string())
}

fn api_token() -> HelperResult<String> {
    env::var("AGENTSCOMMANDER_API_TOKEN").map_err(|_| "api_token_unavailable".to_string())
}

pub(crate) fn authorization_value() -> HelperResult<String> {
    Ok(format!("Bearer {}", api_token()?))
}

pub(crate) fn next_utf8(
    iter: &mut impl Iterator<Item = OsString>,
    code: &'static str,
) -> HelperResult<String> {
    iter.next()
        .ok_or_else(|| code.to_string())?
        .into_string()
        .map_err(|_| code.to_string())
}
