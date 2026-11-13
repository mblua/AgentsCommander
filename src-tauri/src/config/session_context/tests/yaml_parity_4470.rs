use super::*;

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}

fn index_one(folder: &str, content: &str) -> SkillIndex {
    let temp = tempfile::tempdir().unwrap();
    let matrix_root = temp.path().join("_agent_dev");
    write_skill(&matrix_root, folder, content);
    discover_skill_index(Some(&path_string(&matrix_root)))
}

fn fingerprint(index: &SkillIndex) -> String {
    let mut fields = Vec::new();
    for skill in &index.skills {
        fields.push(format!(
            "{}|{}|{:?}|{:?}|{:?}",
            skill.folder_name,
            skill.name,
            skill.description,
            skill.when_to_use,
            skill.metadata_warnings
        ));
    }
    fields.extend(index.warnings.iter().cloned());
    digest(&fields.join("\n"))
}

#[test]
fn issue_4470_shipped_skills_parse_unchanged() {
    let cases = [
        (
            "agency-agents-roles",
            include_str!("../../root_agent_defaults/agency-agents-roles/SKILL.md"),
        ),
        (
            "role-skill-boundary-audit",
            include_str!("../../root_agent_defaults/role-skill-boundary-audit/SKILL.md"),
        ),
        (
            "rtk-install-windows",
            include_str!("../../root_agent_defaults/rtk-install-windows/SKILL.md"),
        ),
        (
            "self-handoff",
            include_str!("../../default_skills/self-handoff/SKILL.md"),
        ),
    ];
    let mut fingerprints = Vec::new();
    for (folder, content) in cases {
        let index = index_one(folder, &content.replace("\r\n", "\n"));
        assert_eq!(index.skills.len(), 1);
        assert!(index.warnings.is_empty());
        assert_eq!(index.skills[0].name, folder);
        assert!(index.skills[0].description.is_some());
        let actual = fingerprint(&index);
        let expected = match folder {
            "agency-agents-roles" => {
                "8d5654b84e1736493840fe752d27f566faa0c27691e8633063ca869deff97d10"
            }
            "role-skill-boundary-audit" => {
                "faafbd8bc1999898bf0daec535f3adc5c8f11babd34cce22881df88d130949df"
            }
            "rtk-install-windows" => {
                "51cf538bb8ad11b069e07e4fc72d039f6097203342edc4ec5d360f09d9348bcb"
            }
            "self-handoff" => "76347668e92b794c7d1cd22485fb5b852ae70e6e1e6fcc3f552e608ff855c48e",
            _ => unreachable!(),
        };
        assert_eq!(actual, expected);
        fingerprints.push(actual);
    }
    for (i, value) in fingerprints.iter().enumerate() {
        assert!(!fingerprints[..i].contains(value));
    }
}

#[test]
fn issue_4470_issue_909_snapshot_rejected_unchanged() {
    let content = crate::config::root_agent::agency_pre_yaml_fix_snapshot();
    let index = index_one("agency-agents-roles", &content.replace("\r\n", "\n"));
    assert!(index.skills.is_empty());
    assert_eq!(index.warnings.len(), 1);
    assert_eq!(
        index.warnings[0],
        concat!(
            "Skipped skill `agency-agents-roles`: ",
            "YAML parse error: mapping values are not allowed in this context at line 2 column 105"
        )
    );
    assert!(index.warnings[0].contains(
        "YAML parse error: mapping values are not allowed in this context at line 2 column 105"
    ));
}

#[test]
fn issue_4470_unquoted_colon_space_rejected() {
    let index = index_one("c", "---\nname: c\ndescription: a: b\n---\n");
    assert!(index.skills.is_empty());
    assert_eq!(index.warnings.len(), 1);
    assert_eq!(
        index.warnings[0],
        concat!(
            "Skipped skill `c`: ",
            "YAML parse error: mapping values are not allowed in this context at line 2 column 15"
        )
    );
}

#[test]
fn issue_4470_quoted_colon_space_accepted() {
    let index = index_one("c", "---\nname: c\ndescription: \"a: b\"\n---\n");
    assert_eq!(index.skills.len(), 1);
    assert_eq!(index.skills[0].description.as_deref(), Some("a: b"));
    assert!(index.warnings.is_empty());
}

#[test]
fn issue_4470_duplicate_key_rejected() {
    let index = index_one(
        "d",
        "---\nname: d\ndescription: one\ndescription: two\n---\n",
    );
    assert!(index.skills.is_empty());
    assert_eq!(index.warnings.len(), 1);
    assert_eq!(
        index.warnings[0],
        concat!(
            "Skipped skill `d`: ",
            "YAML parse error: duplicate entry with key \"description\""
        )
    );
}

#[test]
fn issue_4470_scalar_kinds_unchanged() {
    let cases = [
        ("yes", "name: q\ndescription: yes"),
        ("number", "name: q\ndescription: 123"),
        ("null", "name: q\ndescription: ~"),
        ("sequence", "name: q\ndescription: [a, b]"),
        ("single", "name: 'q'"),
        ("folded", "name: q\ndescription: >\n  a\n  b\n"),
        ("literal", "name: q\ndescription: |\n  a\n  b\n"),
        ("tagged", "name: q\ndescription: !x v"),
        ("alias", "name: q\na: &x v\ndescription: *x"),
        ("flow", "{name: f, description: g}"),
        ("bare", "just text"),
        ("empty", ""),
    ];
    for (label, body) in cases {
        let index = index_one("q", &format!("---\n{body}\n---\n"));
        let expected = match label {
            "yes" => "2f27a9281ed43a5ad3b36deeffcd92c8a573af215f84f56ca6644d72d256c6bb",
            "number" | "null" | "sequence" | "tagged" => {
                "6a35104bb1b3d38a009615c539e598cf30ef34203792c5091f9db7c95283bf71"
            }
            "single" => "9feded3af131e3daa8536ca791a27cce721ff40f61e2782c71a2579c54ff8d00",
            "folded" | "literal" => {
                "30fb524e03b71d4dfcccf18e8736932dbc90af0a206d0ee45172219930514771"
            }
            "alias" => "ef660282f6f36fb76419cd49925b193587b9657738298df39bcc8b11dacd5b15",
            "flow" => "76e6bd7c10ee5f86f0e3d78a1953857e80f205f8cfbfe8d11b9223f9f0fc6127",
            "bare" | "empty" => "37c84e98a89dfa221b0eeea98f13b9a2f6712e237f0bc6ead657e1846af36f10",
            _ => unreachable!(),
        };
        assert_eq!(fingerprint(&index), expected, "{label}");
    }
    let bom = index_one("q", "\u{feff}---\nname: q\ndescription: bom\n---\n");
    assert_eq!(
        fingerprint(&bom),
        "2a968bbb3c9b147de5872520390a5539436228a60a3c3d79c8290ea9275514e9"
    );
    let crlf = index_one("q", "---\r\nname: q\r\ndescription: crlf\r\n---\r\n");
    assert_eq!(
        fingerprint(&crlf),
        "0c0475aafc9b4743a57b4a037cb370e0d56cedbac75deb197410a3b0124e0fd4"
    );
}

#[test]
fn issue_4470_fingerprint_covers_fields() {
    let base = fingerprint(&index_one(
        "f",
        "---\nname: f\ndescription: d\nwhen_to_use: w\n---\n",
    ));
    for (folder, body) in [
        ("f", "name: f\ndescription: d2\nwhen_to_use: w"),
        ("f", "name: f\ndescription: d\nwhen_to_use: w2"),
        ("f", "name: f\ndescription: 1\nwhen_to_use: w"),
        ("g", "name: g\ndescription: d\nwhen_to_use: w"),
    ] {
        assert_ne!(
            fingerprint(&index_one(folder, &format!("---\n{body}\n---\n"))),
            base
        );
    }
}
