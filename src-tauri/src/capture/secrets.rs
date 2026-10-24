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
mod tests;
