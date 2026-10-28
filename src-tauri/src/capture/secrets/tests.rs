use super::*;

#[test]
fn rule_1_flags_a_pem_private_key_block() {
    let owner = SecretRules::default();
    let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
    let detection = detect(&owner, text).expect("a PEM private key must flag");
    assert_eq!(detection.rule_number, 1);
    assert_eq!(detection.rule, "private-key");
    assert_eq!(detection.length, text.len());
}

#[test]
fn rule_2_flags_an_aws_access_key_id() {
    let owner = SecretRules::default();
    let detection = detect(&owner, "AKIAIOSFODNN7EXAMPLE").expect("an AWS key id must flag");
    assert_eq!(detection.rule_number, 2);
}

#[test]
fn rule_3_flags_an_aws_secret_access_key() {
    let owner = SecretRules::default();
    let text = "aws secret='wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY'";
    let detection = detect(&owner, text).expect("an AWS secret key must flag");
    assert_eq!(detection.rule_number, 3);
}

#[test]
fn rule_4_flags_an_authorization_bearer_header() {
    let owner = SecretRules::default();
    let text = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789";
    let detection = detect(&owner, text).expect("an authorization bearer must flag");
    assert_eq!(detection.rule_number, 4);
}

#[test]
fn rule_5_flags_both_github_token_shapes() {
    let owner = SecretRules::default();
    let classic = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
    assert_eq!(
        detect(&owner, classic).expect("classic token").rule_number,
        5
    );
    let fine_grained = format!("github_pat_{}", "a".repeat(60));
    assert_eq!(
        detect(&owner, &fine_grained)
            .expect("fine-grained token")
            .rule_number,
        5
    );
}

#[test]
fn rule_6_flags_a_telegram_bot_token() {
    let owner = SecretRules::default();
    let text = "123456789:ABCDEFGHIJKLMNOPQRSTUVWX-YZ_abcdefghijkl";
    let detection = detect(&owner, text).expect("a Telegram bot token must flag");
    assert_eq!(detection.rule_number, 6);
}

#[test]
fn rule_7_flags_a_query_string_key() {
    let owner = SecretRules::default();
    let text = "https://example.test/path?key=abcdefghijklmnop";
    let detection = detect(&owner, text).expect("a query-string key must flag");
    assert_eq!(detection.rule_number, 7);
}

#[test]
fn rule_8_flags_an_env_style_secret_assignment() {
    let owner = SecretRules::default();
    let text = "MY_SECRET=abcdefghijklmnop";
    let detection = detect(&owner, text).expect("an env-style assignment must flag");
    assert_eq!(detection.rule_number, 8);
}

#[test]
fn rule_9_flags_a_long_mixed_alphabet_run() {
    let owner = SecretRules::default();
    let text = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV";
    let detection = detect(&owner, text).expect("a 44-character mixed run must flag");
    assert_eq!(detection.rule_number, 9);
    assert_eq!(detection.length, text.len());
}

#[test]
fn rule_9_excludes_hex_digests_uuids_short_runs_and_prose() {
    let owner = SecretRules::default();
    let negatives = [
        "d670460b4b4aece5915caf5c68d12f560a9fe3e4", // 40-character Git SHA
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // SHA-256
        "123e4567-e89b-12d3-a456-426614174000",     // dashed UUID
        "abcdefghijklmnopqrstuvwxyzAB",             // 28 characters, below the bound
        "The quick brown fox jumps over the lazy dog all day long", // prose
    ];
    for text in negatives {
        assert!(
            detect(&owner, text).is_none(),
            "rule 9 must not flag {text:?}"
        );
    }
}

#[test]
fn clean_text_yields_no_detection_and_a_dirty_control_does() {
    let owner = SecretRules::default();
    let clean = "The build finished successfully; nothing sensitive here.";
    assert_eq!(detect(&owner, clean), None);
    let dirty = "key material: AKIAIOSFODNN7EXAMPLE";
    assert!(
        detect(&owner, dirty).is_some(),
        "the dirty control must flag"
    );
}

#[test]
fn the_reason_carries_a_rule_a_length_and_neither_excerpt_nor_path() {
    let owner = SecretRules::default();
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let detection = detect(&owner, secret).expect("the sample must flag");
    let reason = detection.reason();
    assert!(reason.contains("rule 2"));
    assert!(reason.contains(&format!("{} bytes", secret.len())));
    assert!(
        !reason.contains(secret),
        "the reason must not carry an excerpt"
    );
    assert!(!reason.contains('/'), "the reason must not carry a path");
}

#[test]
fn rule_priority_remains_first_match() {
    let owner = SecretRules::default();
    let text = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789 AKIAIOSFODNN7EXAMPLE";
    let detection = detect(&owner, text).unwrap();
    assert_eq!(detection.rule_number, 2);
    assert_eq!(detection.rule, "aws-access-key-id");
    assert_eq!(detection.length, text.len());
}

#[test]
fn secret_rules_are_lazy_shared_and_initialized_once_under_concurrent_detection() {
    let capture = std::sync::Arc::new(crate::capture::state::CaptureState::new());
    let cloned = std::sync::Arc::clone(&capture);
    let independent = SecretRules::default();
    assert!(capture.secret_rules.rules.get().is_none());
    assert!(cloned.secret_rules.rules.get().is_none());
    assert!(independent.rules.get().is_none());
    assert!(std::ptr::eq(&capture.secret_rules, &cloned.secret_rules));
    let text = "AKIAIOSFODNN7EXAMPLE";
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| detect(&cloned.secret_rules, text)))
            .collect();
        for worker in workers {
            assert_eq!(
                worker.join().unwrap(),
                Some(Detection {
                    rule_number: 2,
                    rule: "aws-access-key-id",
                    length: text.len(),
                })
            );
        }
    });
    assert!(capture.secret_rules.rules.get().is_some());
    assert!(std::ptr::eq(
        compiled_rules(&capture.secret_rules),
        compiled_rules(&cloned.secret_rules)
    ));
    assert!(std::ptr::eq(
        compiled_rules(&capture.secret_rules),
        compiled_rules(&capture.secret_rules)
    ));
    assert!(independent.rules.get().is_none());
    assert_eq!(detect(&independent, "ordinary prose"), None);
    assert!(independent.rules.get().is_some());
}
