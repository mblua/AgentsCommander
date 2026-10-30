/// Dedicated token for WebSocket authentication.
/// Separate from MasterToken to limit blast radius if captured via network sniffing.
pub struct WebAccessToken(String);

impl WebAccessToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Constant-time comparison to prevent timing oracle attacks.
    pub fn matches(&self, candidate: &str) -> bool {
        let a = self.0.as_bytes();
        let b = candidate.as_bytes();
        if a.len() != b.len() {
            return false;
        }
        a.iter()
            .zip(b.iter())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
    }

    pub fn value(&self) -> &str {
        &self.0
    }
}

pub(crate) fn read_persisted_web_token(
    config_dir: Option<&std::path::Path>,
) -> Result<String, String> {
    let token_path = config_dir.ok_or("No config dir")?.join("web-token.txt");
    crate::filesystem::documents::read_config_document(&token_path)
        .map_err(|e| format!("Cannot read web token: {e}"))
}

#[cfg(test)]
mod tests;
