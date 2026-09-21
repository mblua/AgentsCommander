//! Pre-egress secret detector for Co-managed (#2232 phase 6, plan section 7).
//!
//! `telegram::redact` is the wrong tool for this job and is deliberately not
//! widened: its policy covers `/bot<token>` URL shapes and `[?&]key=<value>`
//! only. Co-managed exports arbitrary agent output, so this leaf implements the
//! rules of plan section 7.1 and the bounded high-entropy fallback of section
//! 7.2.
//!
//! The detector is the gate that runs **before any file is written and before
//! anything is enqueued or sent** (plan section 7). A finding therefore never
//! carries the candidate: [`Detection::reason`] exposes a rule, the rule name
//! and the candidate byte length, and nothing else. The callers must surface
//! exactly that, with no excerpt and no path.
//!
//! Leaf module: `regex`, `std`, and nothing from this crate. It never names
//! the phone, session or commands subtrees, either config module named in plan
//! section 4, or the network module, which is what keeps it out of the
//! 88-module SCC.
//!
//! Rule 9 is a backstop, not the primary net. Its exclusions (section 7.2) are
//! load bearing: an unbounded run rule would flag every hex digest the team
//! exchanges and the feature would silently go inert. A false negative inside
//! rule 9 is a declared residual; a false positive is worse.

use std::sync::OnceLock;

use regex::Regex;

/// Minimum length of a rule 9 run over `[A-Za-z0-9_-]` (plan section 7.2).
pub const ENTROPY_RUN_MIN_LEN: usize = 32;

/// Minimum Shannon entropy, in bits per character, for a rule 9 run
/// (plan section 7.2). A 64-hex digest measures about 3.9 with its 16-symbol
/// alphabet, so this bound plus the pure-hex exclusion keeps Git SHAs,
/// SHA-256 digests and UUIDs from flagging.
pub const ENTROPY_RUN_MIN_BITS_PER_CHAR: f64 = 4.0;

/// What the detector found. It never contains the candidate or a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Detection {
    /// The number of the matching rule, 1 to 9.
    pub rule_number: u8,
    /// A stable, content-free name for that rule.
    pub rule: &'static str,
    /// The byte length of the whole candidate, never of the match.
    pub length: usize,
}

impl Detection {
    /// The only text a caller may surface to the user: a reason and a length.
    pub fn reason(&self) -> String {
        format!(
            "secret detected by rule {} ({}); candidate length {} bytes",
            self.rule_number, self.rule, self.length
        )
    }
}

/// Scan `text` and return the first rule that matches, in rule order.
///
/// Returns `None` for clean text; no allocation happens on that path.
pub fn detect(text: &str) -> Option<Detection> {
    let rules = compiled_rules();
    let shapes: [(u8, &'static str, &Regex); 8] = [
        (1, "private-key", &rules.private_key),
        (2, "aws-access-key-id", &rules.aws_access_key_id),
        (3, "aws-secret-access-key", &rules.aws_secret_access_key),
        (4, "authorization-bearer", &rules.authorization_bearer),
        (5, "github-token", &rules.github_token),
        (6, "telegram-bot-token", &rules.telegram_bot_token),
        (7, "query-string-key", &rules.query_string_key),
        (8, "env-secret-assignment", &rules.env_secret_assignment),
    ];
    for (rule_number, rule, pattern) in shapes {
        if pattern.is_match(text) {
            return Some(Detection {
                rule_number,
                rule,
                length: text.len(),
            });
        }
    }
    if first_high_entropy_run(text).is_some() {
        return Some(Detection {
            rule_number: 9,
            rule: "high-entropy-run",
            length: text.len(),
        });
    }
    None
}

struct CompiledRules {
    private_key: Regex,
    aws_access_key_id: Regex,
    aws_secret_access_key: Regex,
    authorization_bearer: Regex,
    github_token: Regex,
    telegram_bot_token: Regex,
    query_string_key: Regex,
    env_secret_assignment: Regex,
}

fn compiled_rules() -> &'static CompiledRules {
    static RULES: OnceLock<CompiledRules> = OnceLock::new();
    RULES.get_or_init(|| CompiledRules {
        // Plan section 7.1 rule 1.
        private_key: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
            .expect("rule 1 is a valid regex"),
        // Rule 2.
        aws_access_key_id: Regex::new(r"(?:AKIA|ASIA)[0-9A-Z]{16}")
            .expect("rule 2 is a valid regex"),
        // Rule 3.
        aws_secret_access_key: Regex::new(
            r#"(?i)aws.{0,20}secret.{0,20}['"][0-9A-Za-z/+=]{40}['"]"#,
        )
        .expect("rule 3 is a valid regex"),
        // Rule 4.
        authorization_bearer: Regex::new(r"(?i)authorization\s*:\s*bearer\s+\S{16,}")
            .expect("rule 4 is a valid regex"),
        // Rule 5: both GitHub token shapes.
        github_token: Regex::new(r"gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{60,}")
            .expect("rule 5 is a valid regex"),
        // Rule 6.
        telegram_bot_token: Regex::new(r"[0-9]{6,12}:[A-Za-z0-9_-]{30,}")
            .expect("rule 6 is a valid regex"),
        // Rule 7.
        query_string_key: Regex::new(r"[?&](?:key|token|secret|password)=[^&\s]{12,}")
            .expect("rule 7 is a valid regex"),
        // Rule 8.
        env_secret_assignment: Regex::new(
            r"(?im)^\s*[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|API_KEY)[A-Z0-9_]*\s*=\s*\S{12,}",
        )
        .expect("rule 8 is a valid regex"),
    })
}

/// The first maximal `[A-Za-z0-9_-]` run that passes every section 7.2 gate.
fn first_high_entropy_run(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut run_start: Option<usize> = None;
    for (index, byte) in bytes.iter().enumerate() {
        if is_run_byte(*byte) {
            if run_start.is_none() {
                run_start = Some(index);
            }
        } else if let Some(start) = run_start.take() {
            if let Some(run) = consider_run(&text[start..index]) {
                return Some(run);
            }
        }
    }
    run_start.and_then(|start| consider_run(&text[start..]))
}

fn is_run_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn consider_run(run: &str) -> Option<&str> {
    if run.len() < ENTROPY_RUN_MIN_LEN {
        return None;
    }
    if is_pure_hexadecimal(run) {
        return None;
    }
    if is_dashed_uuid(run) {
        return None;
    }
    if shannon_bits_per_char(run) < ENTROPY_RUN_MIN_BITS_PER_CHAR {
        return None;
    }
    Some(run)
}

fn is_pure_hexadecimal(run: &str) -> bool {
    run.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_dashed_uuid(run: &str) -> bool {
    let bytes = run.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn shannon_bits_per_char(run: &str) -> f64 {
    let mut counts = [0_usize; 256];
    for byte in run.bytes() {
        counts[usize::from(byte)] += 1;
    }
    let length = run.len() as f64;
    counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let probability = *count as f64 / length;
            -probability * probability.log2()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_1_flags_a_pem_private_key_block() {
        let text =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let detection = detect(text).expect("a PEM private key must flag");
        assert_eq!(detection.rule_number, 1);
        assert_eq!(detection.rule, "private-key");
        assert_eq!(detection.length, text.len());
    }

    #[test]
    fn rule_2_flags_an_aws_access_key_id() {
        let detection = detect("AKIAIOSFODNN7EXAMPLE").expect("an AWS key id must flag");
        assert_eq!(detection.rule_number, 2);
    }

    #[test]
    fn rule_3_flags_an_aws_secret_access_key() {
        let text = "aws secret='wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY'";
        let detection = detect(text).expect("an AWS secret key must flag");
        assert_eq!(detection.rule_number, 3);
    }

    #[test]
    fn rule_4_flags_an_authorization_bearer_header() {
        let text = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789";
        let detection = detect(text).expect("an authorization bearer must flag");
        assert_eq!(detection.rule_number, 4);
    }

    #[test]
    fn rule_5_flags_both_github_token_shapes() {
        let classic = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        assert_eq!(detect(classic).expect("classic token").rule_number, 5);
        let fine_grained = format!("github_pat_{}", "a".repeat(60));
        assert_eq!(
            detect(&fine_grained)
                .expect("fine-grained token")
                .rule_number,
            5
        );
    }

    #[test]
    fn rule_6_flags_a_telegram_bot_token() {
        let text = "123456789:ABCDEFGHIJKLMNOPQRSTUVWX-YZ_abcdefghijkl";
        let detection = detect(text).expect("a Telegram bot token must flag");
        assert_eq!(detection.rule_number, 6);
    }

    #[test]
    fn rule_7_flags_a_query_string_key() {
        let text = "https://example.test/path?key=abcdefghijklmnop";
        let detection = detect(text).expect("a query-string key must flag");
        assert_eq!(detection.rule_number, 7);
    }

    #[test]
    fn rule_8_flags_an_env_style_secret_assignment() {
        let text = "MY_SECRET=abcdefghijklmnop";
        let detection = detect(text).expect("an env-style assignment must flag");
        assert_eq!(detection.rule_number, 8);
    }

    #[test]
    fn rule_9_flags_a_long_mixed_alphabet_run() {
        let text = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV";
        let detection = detect(text).expect("a 44-character mixed run must flag");
        assert_eq!(detection.rule_number, 9);
        assert_eq!(detection.length, text.len());
    }

    #[test]
    fn rule_9_excludes_hex_digests_uuids_short_runs_and_prose() {
        let negatives = [
            "d670460b4b4aece5915caf5c68d12f560a9fe3e4", // 40-character Git SHA
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // SHA-256
            "123e4567-e89b-12d3-a456-426614174000",     // dashed UUID
            "abcdefghijklmnopqrstuvwxyzAB",             // 28 characters, below the bound
            "The quick brown fox jumps over the lazy dog all day long", // prose
        ];
        for text in negatives {
            assert!(detect(text).is_none(), "rule 9 must not flag {text:?}");
        }
    }

    #[test]
    fn clean_text_yields_no_detection_and_a_dirty_control_does() {
        let clean = "The build finished successfully; nothing sensitive here.";
        assert_eq!(detect(clean), None);
        let dirty = "key material: AKIAIOSFODNN7EXAMPLE";
        assert!(detect(dirty).is_some(), "the dirty control must flag");
    }

    #[test]
    fn the_reason_carries_a_rule_a_length_and_neither_excerpt_nor_path() {
        let secret = "AKIAIOSFODNN7EXAMPLE";
        let detection = detect(secret).expect("the sample must flag");
        let reason = detection.reason();
        assert!(reason.contains("rule 2"));
        assert!(reason.contains(&format!("{} bytes", secret.len())));
        assert!(
            !reason.contains(secret),
            "the reason must not carry an excerpt"
        );
        assert!(!reason.contains('/'), "the reason must not carry a path");
    }
}
