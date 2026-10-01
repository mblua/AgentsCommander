//! C4 (#2470, #2823) - the single-reader gate.
//!
//! Ten censuses over `src-tauri/src` freeze every spelling a reader of the four
//! agent state keys needs. Each census pins its total hit count, its distinct
//! entry count and the SHA-256 of its sorted entries. No pin stores a line
//! number, so moving code never reddens the gate; adding, deleting or rewriting
//! a hit line does. The gate is a tripwire: when it reddens, the new hit is
//! triaged by hand before any pin moves.
//!
//! The walk is the authority, not `git ls-files`, and no child process starts.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The two owner modules: the loader and the pair primitive.
const OWNERS: [&str; 2] = [
    "src-tauri/src/config/agent_config.rs",
    "src-tauri/src/config/local_config_io.rs",
];

const QUOTED_KEY: &str =
    r#""(lastCodingAgent|codingAgents|lastAgentMessageAt|profileContentHash)""#;
const P2: &str = r"from_str::<AgentLocalConfig>";
const P3: &str =
    r":[ \t]*AgentLocalConfig([^A-Za-z0-9_]|$)|AgentLocalConfig[ \t]*=|\{[^}]*AgentLocalConfig";
const P4: &str =
    r"[.]tooling[.](last_coding_agent|coding_agents|last_agent_message_at)([^A-Za-z0-9_]|$)";
const P6: &str = r"CONFIG_STATE_TARGET_NAME";
const P7: &str = r#""config[.]json""#;
const P9: &str = r"(^|[^A-Za-z0-9_])STATE_KEYS([^A-Za-z0-9_]|$)";
const P10: &str = r"ToolingSide::(Decisions|State)";

/// P5, the whole-file census. `WS` stands for the whitespace class and is
/// substituted at each of its six occurrences.
const P5_TEMPLATE: &str = r"(:WS*AgentLocalConfig|from_str::WS*<WS*AgentLocalConfig|[.]WS*toolingWS*[.]WS*(?:last_coding_agent|coding_agents|last_agent_message_at))([^A-Za-z0-9_]|$)";
const WS: &str = r"[ \t\n\x0b\x0c\r]";

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    All,
    Owners,
    NonOwners,
}

impl Scope {
    fn holds(self, path: &str) -> bool {
        let owner = OWNERS.contains(&path);
        match self {
            Scope::All => true,
            Scope::Owners => owner,
            Scope::NonOwners => !owner,
        }
    }
}

struct Hit {
    path: String,
    line: usize,
    text: String,
}

struct Pin {
    total: usize,
    distinct: usize,
    sha256: &'static str,
    /// Per-file totals, paths relative to `src-tauri/src/`.
    per_file: &'static [(&'static str, usize)],
}

/// Every `.rs` file under `src-tauri/src`, as `(path, text)`: the path spelled
/// `src-tauri/<relative>` with `/` separators, the text with CR LF and lone CR
/// both turned into LF, so no host and no checkout can move a digest.
fn rust_sources() -> Vec<(String, String)> {
    fn visit(directory: &Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(source_entry(&path));
            }
        }
    }

    let mut files = Vec::new();
    visit(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    files
}

fn source_entry(path: &Path) -> (String, String) {
    let relative = path
        .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("source is below manifest directory")
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    (format!("src-tauri/{relative}"), text)
}

/// One hit per matching line of every in-scope file.
fn line_census(pattern: &str, scope: Scope) -> Vec<Hit> {
    let regex = Regex::new(pattern).expect("census pattern");
    let mut hits = Vec::new();
    for (path, text) in rust_sources() {
        if !scope.holds(&path) {
            continue;
        }
        for (index, line) in text.split('\n').enumerate() {
            if regex.is_match(line) {
                hits.push(Hit {
                    path: path.clone(),
                    line: index + 1,
                    text: line.trim().to_string(),
                });
            }
        }
    }
    hits
}

fn p5_regex() -> Regex {
    Regex::new(&P5_TEMPLATE.replace("WS", WS)).expect("P5 pattern")
}

/// The 1-based line of every P5 hit in `text`: the line capture group 1 starts
/// on.
fn p5_lines(text: &str) -> Vec<usize> {
    p5_regex()
        .captures_iter(text)
        .map(|captures| {
            let start = captures.get(1).expect("group 1").start();
            1 + text[..start].matches('\n').count()
        })
        .collect()
}

/// P5 over every file. The entry text is the whole source line at the hit's
/// line number, never the matched text.
fn whole_file_census() -> Vec<Hit> {
    let mut hits = Vec::new();
    for (path, text) in rust_sources() {
        let lines: Vec<&str> = text.split('\n').collect();
        for line in p5_lines(&text) {
            hits.push(Hit {
                path: path.clone(),
                line,
                text: lines[line - 1].trim().to_string(),
            });
        }
    }
    hits
}

fn entry(hit: &Hit) -> String {
    format!("{}\t{}", hit.path, hit.text)
}

/// Deduplicated entries in byte order.
fn entries(hits: &[Hit]) -> Vec<String> {
    let unique: BTreeSet<String> = hits.iter().map(entry).collect();
    unique.into_iter().collect()
}

fn digest(entries: &[String]) -> String {
    let joined = format!("{}\n", entries.join("\n"));
    Sha256::digest(joined.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

fn per_file(hits: &[Hit]) -> BTreeMap<String, usize> {
    let mut totals = BTreeMap::new();
    for hit in hits {
        let relative = hit.path.trim_start_matches("src-tauri/src/").to_string();
        *totals.entry(relative).or_insert(0) += 1;
    }
    totals
}

fn hit_lines(hits: &[Hit]) -> BTreeSet<(String, usize)> {
    hits.iter()
        .map(|hit| (hit.path.clone(), hit.line))
        .collect()
}

/// Total first, then distinct, then digest, then the per-file totals.
fn assert_frozen(id: &str, hits: &[Hit], pin: &Pin) {
    assert_eq!(hits.len(), pin.total, "{id} total");
    let entries = entries(hits);
    assert_eq!(entries.len(), pin.distinct, "{id} distinct");
    assert_eq!(digest(&entries), pin.sha256, "{id} digest");
    let expected: BTreeMap<String, usize> = pin
        .per_file
        .iter()
        .map(|(path, total)| (path.to_string(), *total))
        .collect();
    assert_eq!(per_file(hits), expected, "{id} per-file totals");
}

#[test]
fn the_inventory_is_the_certified_file_set() {
    let sources = rust_sources();
    assert_eq!(sources.len(), 221, "inventory count");
    for (path, text) in &sources {
        assert!(path.starts_with("src-tauri/src/"), "path form: {path}");
        assert!(!path.contains('\\'), "path separator: {path}");
        assert!(!text.contains('\r'), "byte form: {path}");
    }
}

#[test]
fn p1_is_frozen() {
    let pin = Pin {
        total: 119,
        distinct: 111,
        sha256: "B0A9148982049934CC1045D645C0CA92F61A09743680883FFF1471D50A869214",
        per_file: &[
            ("cli/list_peers.rs", 1),
            ("commands/ac_discovery.rs", 15),
            ("commands/config.rs", 6),
            ("commands/entity_creation.rs", 11),
            ("commands/session.rs", 12),
            ("config/agent_command.rs", 1),
            ("config/coding_agent_profiles.rs", 33),
            ("config/replica_identity.rs", 5),
            ("config/root_agent.rs", 23),
            ("loops/delivery.rs", 4),
            ("phone/mailbox.rs", 8),
        ],
    };
    assert_frozen("P1", &line_census(QUOTED_KEY, Scope::NonOwners), &pin);
}

#[test]
fn p2_is_frozen() {
    let pin = Pin {
        total: 2,
        distinct: 2,
        sha256: "D4FCAB7648479CD4DECFAE9A98CD16EBBCE8E91A561F815BFD78EBE969E3925B",
        per_file: &[("config/agent_config.rs", 2)],
    };
    assert_frozen("P2", &line_census(P2, Scope::All), &pin);
}

#[test]
fn p3_is_frozen() {
    let pin = Pin {
        total: 2,
        distinct: 2,
        sha256: "13BFB0417783751C46CBED5963ABA6012FA2E55B8D83841A8FC030AD8DB15D74",
        per_file: &[("phone/mailbox.rs", 2)],
    };
    assert_frozen("P3", &line_census(P3, Scope::All), &pin);
}

#[test]
fn p4_is_frozen() {
    let pin = Pin {
        total: 14,
        distinct: 12,
        sha256: "74A8A2AB911692937624C80AC3710418FBD91ACA7247D9F024D8EC2F0B026FE3",
        per_file: &[
            ("cli/list_peers.rs", 4),
            ("config/agent_config.rs", 5),
            ("phone/mailbox.rs", 5),
        ],
    };
    assert_frozen("P4", &line_census(P4, Scope::All), &pin);
}

#[test]
fn p5_is_frozen() {
    let pin = Pin {
        total: 20,
        distinct: 18,
        sha256: "A7C6E9FFE89853CF96918EDE52A3CD919CA9DF9F8AD7D154E56F908D529F3204",
        per_file: &[
            ("cli/list_peers.rs", 4),
            ("config/agent_config.rs", 8),
            ("loops/delivery.rs", 1),
            ("phone/mailbox.rs", 7),
        ],
    };
    assert_frozen("P5", &whole_file_census(), &pin);
}

#[test]
fn p6_is_frozen() {
    let pin = Pin {
        total: 35,
        distinct: 31,
        sha256: "26377139EA84314CB1815708E1B2017153A82052FFFA3E25F69F443AB1A9978A",
        per_file: &[
            ("commands/ac_discovery.rs", 1),
            ("commands/config.rs", 2),
            ("commands/entity_creation.rs", 4),
            ("commands/session.rs", 2),
            ("config/agent_config.rs", 8),
            ("config/coding_agent_profiles.rs", 3),
            ("config/instance_artifacts.rs", 2),
            ("config/naming_migration.rs", 4),
            ("config/replica_identity.rs", 3),
            ("config/root_agent.rs", 2),
            ("phone/mailbox.rs", 4),
        ],
    };
    assert_frozen("P6", &line_census(P6, Scope::All), &pin);
}

#[test]
fn p7_is_frozen() {
    let pin = Pin {
        total: 53,
        distinct: 39,
        sha256: "95E98325E044EADE7CFE55C2291D6EC942EB42FBFDF4A74CB65657C392F7E37D",
        per_file: &[
            ("config/agent_config.rs", 27),
            ("config/local_config_io.rs", 26),
        ],
    };
    assert_frozen("P7", &line_census(P7, Scope::Owners), &pin);
}

#[test]
fn p8_is_frozen() {
    let pin = Pin {
        total: 176,
        distinct: 131,
        sha256: "0F0223D9FB0AC7B64422F53EEBA9E9DDE22768373E40834C94B2AED056FF637C",
        per_file: &[
            ("config/agent_config.rs", 159),
            ("config/local_config_io.rs", 17),
        ],
    };
    assert_frozen("P8", &line_census(QUOTED_KEY, Scope::Owners), &pin);
}

#[test]
fn p9_is_frozen() {
    let pin = Pin {
        total: 10,
        distinct: 8,
        sha256: "8360D7CA2B8731AE91D92BF8B1D4BF5E43721E02CEE7D491471E7C8FC29068CF",
        per_file: &[("config/agent_config.rs", 10)],
    };
    assert_frozen("P9", &line_census(P9, Scope::Owners), &pin);
}

/// The live pin is the total: a new caller passing an existing side adds a hit
/// while the distinct set, and so the digest, stand still.
#[test]
fn p10_is_frozen() {
    let pin = Pin {
        total: 4,
        distinct: 4,
        sha256: "DE95F07F259EFC840E7FA3220DAC01ABF4DC1EC4DF41A9116EACC152F39EE50C",
        per_file: &[("config/coding_agent_profiles.rs", 4)],
    };
    assert_frozen("P10", &line_census(P10, Scope::All), &pin);
}

/// Every one-line hit of P2, P3 and P4 is also a P5 hit line, and P5's surplus
/// is exactly the two split field chains, pinned as entries.
#[test]
fn p5_covers_every_one_line_hit_of_p2_p3_p4() {
    let p5 = whole_file_census();
    let p5_lines = hit_lines(&p5);
    let mut one_line = BTreeSet::new();
    for (id, pattern) in [("P2", P2), ("P3", P3), ("P4", P4)] {
        let lines = hit_lines(&line_census(pattern, Scope::All));
        let missed: Vec<_> = lines.difference(&p5_lines).collect();
        assert!(
            missed.is_empty(),
            "every {id} hit line is also a P5 hit line: {missed:?}"
        );
        one_line.extend(lines);
    }
    assert_eq!(one_line.len(), 18, "P2, P3 and P4 hit lines");

    let mut surplus: Vec<String> = p5
        .iter()
        .filter(|hit| !one_line.contains(&(hit.path.clone(), hit.line)))
        .map(entry)
        .collect();
    surplus.sort();
    assert_eq!(
        surplus,
        [
            "src-tauri/src/config/agent_config.rs\t.tooling",
            "src-tauri/src/loops/delivery.rs\t.tooling",
        ],
        "P5 surplus"
    );
}

const FENCE_SPLIT_ANNOTATION: &str = r#"// PLANT 1: typed annotation split over three lines, `=` on its own line; the
// field read is split too, so no line-based form can see either spelling.
fn plant_one(dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("config.json")).ok()?;
    let cfg:
        AgentLocalConfig
        = serde_json::from_str(&text).ok()?;
    let last = cfg
        .tooling
        .last_coding_agent;
    last
}
"#;

const FENCE_SPLIT_FIELD_CHAIN: &str = r#"// PLANT 2: `.tooling.<field>` chain split over lines; the type is never spelled
// because a helper returns it and inference supplies it.
fn plant_two(dir: &std::path::Path) -> Option<String> {
    let cfg = read_tracked_only(dir)?;
    let last = cfg
        .tooling
        .last_coding_agent
        .clone();
    last
}
"#;

const FENCE_SPLIT_TURBOFISH: &str = r#"// PLANT 3: turbofish split over lines.
fn plant_three(dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("config.json")).ok()?;
    let cfg = serde_json::from_str::<
        AgentLocalConfig,
    >(&text).ok()?;
    cfg.tooling.current_coding_agent
}
"#;

/// The positive control on P5: three readers split over lines, held as bytes
/// and never compiled. P5 sees each one; every line-based form is silent.
#[test]
fn p5_sees_the_three_split_forms() {
    assert_eq!(p5_lines(FENCE_SPLIT_ANNOTATION), [5, 9], "fence 1");
    assert_eq!(p5_lines(FENCE_SPLIT_FIELD_CHAIN), [6], "fence 2");
    assert_eq!(p5_lines(FENCE_SPLIT_TURBOFISH), [4], "fence 3");

    for (id, pattern) in [
        ("P1", QUOTED_KEY),
        ("P2", P2),
        ("P3", P3),
        ("P4", P4),
        ("P6", P6),
    ] {
        let regex = Regex::new(pattern).expect("census pattern");
        for fence in [
            FENCE_SPLIT_ANNOTATION,
            FENCE_SPLIT_FIELD_CHAIN,
            FENCE_SPLIT_TURBOFISH,
        ] {
            let seen = fence.split('\n').any(|line| regex.is_match(line));
            assert!(!seen, "{id} must be silent on a split form");
        }
    }
}
