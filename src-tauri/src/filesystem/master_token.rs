const FILE_NAME: &str = "master-token.txt";

pub(crate) fn matches(config_dir: Option<&std::path::Path>, token: &str) -> bool {
    config_dir
        .map(|d| d.join(FILE_NAME))
        .and_then(|p| super::documents::read_cli_outbox_text(&p).ok())
        .map(|m| m.trim() == token)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
