const FILE_NAME: &str = "master-token.txt";

pub(crate) fn matches(config_dir: Option<&std::path::Path>, token: &str) -> bool {
    config_dir
        .map(|d| d.join(FILE_NAME))
        .and_then(|p| super::documents::read_cli_outbox_text(&p).ok())
        .is_some_and(|m| m.trim() == token)
}

pub(crate) fn publish(config_dir: &std::path::Path, token: &str) -> std::io::Result<()> {
    std::fs::write(config_dir.join(FILE_NAME), token)
}

#[cfg(test)]
mod tests;
