use super::super::markers::strip_ansi_csi;
use super::*;

/// The check before #5179: strip a full copy, then look for a printable char.
fn reference(text: &str) -> bool {
    strip_ansi_csi(text)
        .chars()
        .any(|c| c > ' ' && c != '\u{FFFD}')
}

const PIECES: &[&str] = &[
    "",
    "a",
    " ",
    "\t",
    "\r\n",
    "\u{FFFD}",
    "界",
    "é",
    "\x07",
    "\\",
    "[",
    "]",
    "P",
    "m",
    "\x1b",
    "\x1b[",
    "\x1b[31m",
    "\x1b[0",
    "\x1b[?25l",
    "\x1b[1;2H",
    "\x1b]",
    "\x1b]0;t\x07",
    "\x1b]0;t",
    "\x1b]8;;u\x1b\\",
    "\x1b]0;\x1b",
    "\x1bP",
    "\x1bPq#0\x1b\\",
    "\x1bPq",
    "\x1b(B",
    "\x1bc",
    "\x1b\x1b",
    "\x1b界",
];

#[test]
fn issue_5179_printable_scan_matches_stripped_copy() {
    let mut checked = 0;
    for a in PIECES {
        for b in PIECES {
            for c in PIECES {
                let text = format!("{a}{b}{c}");
                assert_eq!(
                    output_has_printable_activity(&text),
                    reference(&text),
                    "input {text:?}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, PIECES.len().pow(3));
}

#[test]
fn issue_5179_strip_ansi_csi_keeps_its_output() {
    assert_eq!(strip_ansi_csi("\x1b[31mred\x1b[0m"), "red");
    assert_eq!(strip_ansi_csi("a\x1b]0;title\x07b"), "ab");
    assert_eq!(strip_ansi_csi("a\x1b]8;;u\x1b\\b"), "ab");
    assert_eq!(strip_ansi_csi("a\x1bPq\x1b\\b"), "ab");
    assert_eq!(strip_ansi_csi("a\x1b(B"), "aB");
    assert_eq!(strip_ansi_csi("a\x1b"), "a");
    assert_eq!(strip_ansi_csi("界\x1b[1m界"), "界界");
    assert_eq!(strip_ansi_csi("a\x1b]0;open"), "a");
}
