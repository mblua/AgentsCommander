#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramErrKind {
    Network,
    Unauthorized,
    Conflict,
    RateLimited,
    Other,
}

impl TelegramErrKind {
    pub(crate) fn classify(msg: &str) -> Self {
        let lc = msg.to_lowercase();
        if lc.contains("unauthorized") {
            Self::Unauthorized
        } else if lc.contains("conflict") {
            Self::Conflict
        } else if lc.contains("too many requests") || lc.contains("429") {
            Self::RateLimited
        } else if lc.contains("error sending request") || lc.contains("timed out") {
            Self::Network
        } else {
            Self::Other
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Unauthorized => "unauthorized",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Other => "other",
        }
    }
}

pub(crate) fn prepare_output_chunks(buffer: &mut String, skip_dedup: bool) -> Vec<String> {
    let text = std::mem::take(buffer);
    let text = if skip_dedup {
        text
    } else {
        let mut lines: Vec<&str> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if lines.last().map(|line: &&str| line.trim()) != Some(trimmed) {
                lines.push(line);
            }
        }
        lines.join("\n")
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        return Vec::new();
    }

    chunk_text(&text, 4000)
}

fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let actual_end = if end < text.len() {
            text[start..end]
                .rfind('\n')
                .map(|index| start + index + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..actual_end].to_string());
        start = actual_end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_unauthorized() {
        assert_eq!(
            TelegramErrKind::classify("Telegram error: Unauthorized"),
            TelegramErrKind::Unauthorized
        );
    }

    #[test]
    fn classify_conflict() {
        assert_eq!(
            TelegramErrKind::classify(
                "Telegram error: Conflict: terminated by other getUpdates request"
            ),
            TelegramErrKind::Conflict
        );
    }

    #[test]
    fn classify_rate_limited() {
        assert_eq!(
            TelegramErrKind::classify("Telegram error: Too Many Requests: retry after 5"),
            TelegramErrKind::RateLimited
        );
        assert_eq!(
            TelegramErrKind::classify("HTTP 429 Too Many Requests"),
            TelegramErrKind::RateLimited
        );
    }

    #[test]
    fn classify_network() {
        assert_eq!(
            TelegramErrKind::classify(
                "error sending request for url (https://api.telegram.org/bot***/getUpdates)"
            ),
            TelegramErrKind::Network
        );
        assert_eq!(
            TelegramErrKind::classify("operation timed out"),
            TelegramErrKind::Network
        );
    }

    #[test]
    fn classify_other_falls_through() {
        assert_eq!(
            TelegramErrKind::classify("something unexpected happened"),
            TelegramErrKind::Other
        );
    }
}
