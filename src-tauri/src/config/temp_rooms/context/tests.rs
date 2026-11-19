use super::*;
use crate::config::temp_rooms::temp_room_dir_name;

const ROOT: &str = "/cfg/temp-room-tr07";
const CONFIG: &str = "/cfg";
const PLATFORM: &str = "PLATFORM-RULES-TEXT";

fn limited() -> String {
    render_temp_room_context(ROOT, "temp-room-tr07", CONFIG, false, PLATFORM)
}

fn lifted() -> String {
    render_temp_room_context(ROOT, "temp-room-tr07", CONFIG, true, PLATFORM)
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("missing {needle:?}"))
}

/// A `config_dir()/temp-room-trNN` folder; drop removes it and its cache file.
struct TempRoomDir(std::path::PathBuf);

impl TempRoomDir {
    fn create(n: u32) -> Self {
        let base = crate::config::config_dir().expect("config dir");
        let path = base.join(temp_room_dir_name(n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp room");
        Self(path)
    }

    fn cache_file(&self) -> std::path::PathBuf {
        let name = self.0.file_name().unwrap().to_string_lossy().into_owned();
        crate::config::config_dir()
            .expect("config dir")
            .join(CONTEXT_CACHE_DIR_NAME)
            .join(format!("ac-context-{name}.md"))
    }
}

impl Drop for TempRoomDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.cache_file());
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn render_limited_lists_every_forbidden_sibling() {
    let text = limited();
    for needle in [
        "GOLDEN RULE — Temporary Room Access Limit",
        "master-token.txt",
        "settings*.json",
        "sessions.json",
        "unleashed-replicas.json",
        "temp-room-tr*",
        "ac-root-agent",
        "`/cfg`",
    ] {
        assert!(text.contains(needle), "missing {needle:?}");
    }
    assert!(!text.contains("## Unleashed"));
}

#[test]
fn render_limited_allows_one_notified_message_file() {
    let text = limited();
    let exception = position(
        &text,
        "Incoming-message exception: you MAY READ exactly one message file",
    );
    assert!(text.contains("Process this inter-agent message: <path>"));
    assert!(text.contains("ac-root-agent/messaging/"));
    assert!(text.contains("read-only"));
    assert!(position(&text, "Everything else is OFF-LIMITS") < exception);
    assert!(position(&text, "- the user home, other projects") < exception);
    assert!(exception < position(&text, "CLI exception"));
}

#[test]
fn render_integrity_rule_is_scoped_to_messaging() {
    for text in [limited(), lifted()] {
        assert!(text.contains("In `/cfg/temp-room-tr07/messaging/`, write only message files"));
        assert!(!text.contains("Write only message files"));
    }
}

#[test]
fn render_lifted_drops_limit_keeps_messaging() {
    let text = lifted();
    assert!(!text.contains("GOLDEN RULE"));
    for needle in [
        "## Unleashed",
        "agentscommander://temp-room/tr07",
        "## Integrity Rules",
        PLATFORM,
    ] {
        assert!(text.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn render_lines_and_address() {
    let text = render_temp_room_context(
        r"\\?\C:\cfg\temp-room-tr12",
        "temp-room-tr12",
        CONFIG,
        false,
        PLATFORM,
    );
    assert!(text.contains("Your address is `agentscommander://temp-room/tr12`."));
    assert!(text.contains("`C:\\cfg\\temp-room-tr12`"));
    assert!(!text.contains(r"\\?\"));
    assert!(text.contains("YYYYMMDD-HHMMSS-temp-room-tr12-to-<peer>-<slug>.md"));
    for line in text.lines() {
        assert!(!line.contains('{'), "unrendered placeholder: {line}");
    }
}

#[test]
fn build_writes_named_cache_file() {
    let room = TempRoomDir::create(81);
    let cwd = room.0.to_string_lossy().into_owned();
    let path = build_temp_room_context(&cwd, PLATFORM).expect("build context");
    assert!(path.ends_with("ac-context-temp-room-tr81.md"), "{path}");
    let config = crate::config::config_dir().expect("config dir");
    let expected = render_temp_room_context(
        &cwd,
        "temp-room-tr81",
        &config.to_string_lossy(),
        false,
        PLATFORM,
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read cache"),
        expected
    );
}
