use super::*;

#[test]
fn accepts_ctrl_q_case_insensitive() {
    for value in ["Ctrl+Q", "ctrl+q", "Control+Q", "CTRL+Q", "  Ctrl + q "] {
        let parsed = parse_screenshot_hotkey(value)
            .unwrap_or_else(|e| panic!("expected '{value}' to parse: {e}"));
        assert_eq!(parsed.modifier, HotkeyModifier::Ctrl);
        assert_eq!(parsed.key, 'Q', "value {value}");
    }
}

#[test]
fn accepts_alphanumeric_keys() {
    assert_eq!(parse_screenshot_hotkey("Ctrl+1").unwrap().key, '1');
    assert_eq!(parse_screenshot_hotkey("Control+a").unwrap().key, 'A');
}

#[test]
fn rejects_empty_and_whitespace() {
    assert!(parse_screenshot_hotkey("").is_err());
    assert!(parse_screenshot_hotkey("   ").is_err());
}

#[test]
fn rejects_missing_modifier_or_key() {
    assert!(parse_screenshot_hotkey("Q").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+").is_err());
    assert!(parse_screenshot_hotkey("+Q").is_err());
}

#[test]
fn rejects_unknown_modifier() {
    assert!(parse_screenshot_hotkey("Hyper+Q").is_err());
    assert!(parse_screenshot_hotkey("Alt+Q").is_err());
    assert!(parse_screenshot_hotkey("Shift+Q").is_err());
}

#[test]
fn rejects_multi_modifier_and_multi_key() {
    // MVP accepts exactly one modifier plus one key.
    assert!(parse_screenshot_hotkey("Ctrl+Shift+Q").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+QQ").is_err());
    assert!(parse_screenshot_hotkey("Ctrl+F1").is_err());
}
