use super::*;

// #1318: a fresh `new-project` registration seeds the coding-agent catalog +
// masters into `<project>/.ac/coding-agents/` immediately (no restart needed)
// and records one `coding_agent_catalog` row in the seed manifest (production
// binary: the token gate emits the row under `CatalogSeed`). Re-running
// `new-project` must not change a byte of the catalog (whole-file seed-once).
#[test]
fn new_project_seeds_catalog_into_ac() {
    let tmp = Tmp::new("new-project-catalog");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);
    let project = tmp.path().join("ProjectAlpha");
    let project_arg = project.to_string_lossy().to_string();

    run_success(&bin, &["new-project", &project_arg]);
    let catalog_path = project
        .join(".ac")
        .join("coding-agents")
        .join("agents.10.default.json");
    let catalog = std::fs::read_to_string(&catalog_path)
        .expect("fresh new-project must seed .ac/coding-agents/agents.10.default.json");
    let parsed: serde_json::Value = serde_json::from_str(&catalog).expect("catalog parses");
    assert_eq!(
        parsed["agents"].as_array().map(Vec::len),
        Some(8),
        "the seeded catalog carries the 8 enabled built-ins: {catalog}"
    );
    assert_eq!(parsed["schemaVersion"], 1);
    assert!(
        !catalog.contains("\"muse\""),
        "the disabled muse row must not be seeded: {catalog}"
    );
    assert_eq!(
        parsed["agents"].as_array().unwrap().last().unwrap(),
        &serde_json::json!({
            "key": "grok",
            "label": "Grok Build",
            "description": "Coding Agent by SpaceXAI",
            "color": "#64748b",
            "baseCommand": "grok",
            "instructionsFilename": "AGENTS.md",
            "envs": [],
            "isolatedHome": false,
            "removable": true,
            "installCommands": {
                "windows": concat!(
                    "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Encoded",
                    "Command JABhAGMAXwBwAGEAdABoAD0AJABuAHUAbABsADsAIAAkAGEAYwBfAGMAcgBlAGEAdA",
                    "BlAGQAPQAkAGYAYQBsAHMAZQA7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQA7ACAAdAByAHkAIAB7",
                    "ACAAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAPQBJAG4AdgBvAGsAZQAtAF",
                    "IAZQBzAHQATQBlAHQAaABvAGQAIAAtAFUAcgBpACAAJwBoAHQAdABwAHMAOgAvAC8AeAAuAGEA",
                    "aQAvAGMAbABpAC8AaQBuAHMAdABhAGwAbAAuAHAAcwAxACcAIAAtAFQAaQBtAGUAbwB1AHQAUw",
                    "BlAGMAIAAxADIAMAAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAA7ACAAaQBm",
                    "ACAAKABbAHMAdAByAGkAbgBnAF0AOgA6AEkAcwBOAHUAbABsAE8AcgBXAGgAaQB0AGUAUwBwAG",
                    "EAYwBlACgAJABhAGMAXwBpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQApACAAewAgAHQA",
                    "aAByAG8AdwAgACcARQBtAHAAdAB5ACAAaQBuAHMAdABhAGwAbABlAHIAIAByAGUAcwBwAG8Abg",
                    "BzAGUAJwAgAH0AOwAgACQAYQBjAF8AcABhAHQAaAA9AEoAbwBpAG4ALQBQAGEAdABoACAAKABb",
                    "AEkATwAuAFAAYQB0AGgAXQA6ADoARwBlAHQAVABlAG0AcABQAGEAdABoACgAKQApACAAKAAnAG",
                    "EAYwAtAGkAbgBzAHQAYQBsAGwALQAyADcAOAA3AC0AJwArAFsARwB1AGkAZABdADoAOgBOAGUA",
                    "dwBHAHUAaQBkACgAKQAuAFQAbwBTAHQAcgBpAG4AZwAoACcATgAnACkAKwAnAC4AcABzADEAJw",
                    "ApADsAIAAkAGEAYwBfAGYAaQBsAGUAPQBbAEkATwAuAEYAaQBsAGUAXQA6ADoATwBwAGUAbgAo",
                    "ACQAYQBjAF8AcABhAHQAaAAsAFsASQBPAC4ARgBpAGwAZQBNAG8AZABlAF0AOgA6AEMAcgBlAG",
                    "EAdABlAE4AZQB3ACwAWwBJAE8ALgBGAGkAbABlAEEAYwBjAGUAcwBzAF0AOgA6AFcAcgBpAHQA",
                    "ZQAsAFsASQBPAC4ARgBpAGwAZQBTAGgAYQByAGUAXQA6ADoATgBvAG4AZQApADsAIAAkAGEAYw",
                    "BfAGMAcgBlAGEAdABlAGQAPQAkAHQAcgB1AGUAOwAgAHQAcgB5ACAAewAgAFsAYgB5AHQAZQBb",
                    "AF0AXQAkAGEAYwBfAGIAeQB0AGUAcwA9AFsAVABlAHgAdAAuAEUAbgBjAG8AZABpAG4AZwBdAD",
                    "oAOgBVAFQARgA4AC4ARwBlAHQAUAByAGUAYQBtAGIAbABlACgAKQArAFsAVABlAHgAdAAuAEUA",
                    "bgBjAG8AZABpAG4AZwBdADoAOgBVAFQARgA4AC4ARwBlAHQAQgB5AHQAZQBzACgAJABhAGMAXw",
                    "BpAG4AcwB0AGEAbABsAF8AcwBjAHIAaQBwAHQAKQA7ACAAJABhAGMAXwBmAGkAbABlAC4AVwBy",
                    "AGkAdABlACgAJABhAGMAXwBiAHkAdABlAHMALAAwACwAJABhAGMAXwBiAHkAdABlAHMALgBMAG",
                    "UAbgBnAHQAaAApACAAfQAgAGYAaQBuAGEAbABsAHkAIAB7ACAAJABhAGMAXwBmAGkAbABlAC4A",
                    "RABpAHMAcABvAHMAZQAoACkAIAB9ADsAIAAkAGcAbABvAGIAYQBsADoATABBAFMAVABFAFgASQ",
                    "BUAEMATwBEAEUAPQAkAG4AdQBsAGwAOwAgACYAIAAoAEoAbwBpAG4ALQBQAGEAdABoACAAJABQ",
                    "AFMASABPAE0ARQAgACcAcABvAHcAZQByAHMAaABlAGwAbAAuAGUAeABlACcAKQAgAC0ATgBvAF",
                    "AAcgBvAGYAaQBsAGUAIAAtAE4AbwBuAEkAbgB0AGUAcgBhAGMAdABpAHYAZQAgAC0ARQB4AGUA",
                    "YwB1AHQAaQBvAG4AUABvAGwAaQBjAHkAIABCAHkAcABhAHMAcwAgAC0ARgBpAGwAZQAgACQAYQ",
                    "BjAF8AcABhAHQAaAA7ACAAJABhAGMAXwBlAHgAaQB0AD0AJABMAEEAUwBUAEUAWABJAFQAQwBP",
                    "AEQARQA7ACAAaQBmACAAKAAkAG4AdQBsAGwAIAAtAGUAcQAgACQAYQBjAF8AZQB4AGkAdAApAC",
                    "AAewAgACQAYQBjAF8AZQB4AGkAdAA9ADEAIAB9ACAAfQAgAGMAYQB0AGMAaAAgAHsAIABbAEMA",
                    "bwBuAHMAbwBsAGUAXQA6ADoARQByAHIAbwByAC4AVwByAGkAdABlAEwAaQBuAGUAKAAkAF8AKQ",
                    "A7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIABmAGkAbgBhAGwAbAB5ACAAewAgAGkAZgAg",
                    "ACgAJABhAGMAXwBjAHIAZQBhAHQAZQBkACkAIAB7ACAAdAByAHkAIAB7ACAAUgBlAG0AbwB2AG",
                    "UALQBJAHQAZQBtACAALQBMAGkAdABlAHIAYQBsAFAAYQB0AGgAIAAkAGEAYwBfAHAAYQB0AGgA",
                    "IAAtAEYAbwByAGMAZQAgAC0ARQByAHIAbwByAEEAYwB0AGkAbwBuACAAUwB0AG8AcAAgAH0AIA",
                    "BjAGEAdABjAGgAIAB7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AEUAcgByAG8AcgAuAFcAcgBp",
                    "AHQAZQBMAGkAbgBlACgAJABfACkAOwAgAGkAZgAgACgAJABhAGMAXwBlAHgAaQB0ACAALQBlAH",
                    "EAIAAwACkAIAB7ACAAJABhAGMAXwBlAHgAaQB0AD0AMQAgAH0AIAB9ACAAfQAgAH0AOwAgAGUA",
                    "eABpAHQAIAAkAGEAYwBfAGUAeABpAHQA",
                ),
                "macos": concat!(
                    "ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https:",
                    "//x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n'",
                    " \"$ac_install_script\" | bash",
                ),
                "linux": concat!(
                    "ac_install_script=$(curl -fsSL --connect-timeout 20 --max-time 120 'https:",
                    "//x.ai/cli/install.sh') && [ -n \"$ac_install_script\" ] && printf '%s\\n'",
                    " \"$ac_install_script\" | bash",
                ),
                "default": "echo No verified installer for this platform 1>&2 && exit 1"
            },
            "updateCommands": [],
            "autoUpdate": false,
            "idleBurst": {
                "maxBytes": 1024,
                "maxSecs": 3.0,
                "priorSilenceSecs": 60.0
            }
        })
    );
    let claude = parsed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["key"] == "claude")
        .expect("claude entry");
    assert_eq!(
        claude["updateCommands"],
        serde_json::json!(["claude --update"]),
        "claude carries its update command in the seeded file"
    );
    // Masters seeded per built-in dest.
    for (dest, file) in [
        (".claude", "settings.json"),
        (".codex", "config.toml"),
        (".opencode", "opencode.json"),
    ] {
        assert!(
            project
                .join(".ac")
                .join("coding-agents")
                .join("_seed")
                .join(dest)
                .join(file)
                .is_file(),
            "{dest} master must be seeded"
        );
    }
    let seed_root = project.join(".ac").join("coding-agents").join("_seed");
    let mut seed_names: Vec<_> = std::fs::read_dir(&seed_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    seed_names.sort();
    assert_eq!(seed_names, [".claude", ".codex", ".opencode"]);
    // Seed manifest declares the catalog publication (coverage v2 after the
    // one-shot upgrade).
    let manifest = std::fs::read_to_string(project.join(".ac").join("seed-manifest.toml"))
        .expect("seed manifest");
    assert!(
        manifest.contains("kind = \"coding_agent_catalog\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("scope = \"catalog:coding-agents\""),
        "manifest: {manifest}"
    );
    assert!(
        manifest.contains("path = \".ac/coding-agents/agents.json\""),
        "manifest: {manifest}"
    );

    // Re-run new-project: seed-once keeps the catalog bytes identical.
    run_success(&bin, &["new-project", &project_arg]);
    assert_eq!(
        std::fs::read_to_string(&catalog_path).expect("catalog still exists"),
        catalog,
        "a second registration must not touch the seeded catalog"
    );
}

// #1318/#1967 CLI read contract: `coding-agent catalog` with no registered
// project serves the legacy `<config_dir>/coding-agents/agents.10.default.json` when one
// exists (read-only; pre-migration installs keep today's read behavior). With
// no legacy catalog the persisted-only read is UNAVAILABLE: nonzero exit, the
// baseUnavailable diagnostic (code + selected path + reason) on stderr, no
// catalog on stdout, no embedded fallback and no read-time directory creation.
#[test]
fn cli_catalog_serves_legacy_then_unavailable_without_projects() {
    let tmp = Tmp::new("cli-catalog-legacy");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    write_settings(&config_dir, &[]);

    // Legacy catalog present -> served verbatim (custom entry observable) as a
    // JSON array on stdout, with the migrationPending warning on stderr.
    let legacy_dir = config_dir.join("coding-agents");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy = concat!(
        r##"{"schemaVersion":1,"agents":[{"key":"mine","label":"Mine","description":"d","color":"##,
        r##""#111","command":"mytool","envs":[],"isolatedHome":false,"removable":true}]}"##,
    );
    std::fs::write(legacy_dir.join("agents.10.default.json"), legacy).unwrap();
    let (served, stderr) = run_json(&bin, &["coding-agent", "catalog"]);
    assert_eq!(
        served.as_array().map(Vec::len),
        Some(1),
        "the legacy catalog is served when no project is registered: {served}"
    );
    assert_eq!(served[0]["key"], "mine");
    assert_eq!(served[0]["updateCommands"], serde_json::json!([]));
    assert!(stderr.contains("migrationPending"), "stderr: {stderr}");
    assert_eq!(
        std::fs::read_to_string(legacy_dir.join("agents.10.default.json")).unwrap(),
        legacy,
        "the read leaves the legacy bytes untouched"
    );

    // No legacy -> unavailable. Direct output capture (run_failure now returns
    // the raw Output): nonzero, the selected agents.10.default.json path plus reason on
    // stderr, no success catalog on stdout, and repeated calls never recreate
    // the legacy directory.
    std::fs::remove_dir_all(&legacy_dir).unwrap();
    let selected = legacy_dir.join("agents.10.default.json");
    for _ in 0..2 {
        let out = run_failure(&bin, &["coding-agent", "catalog"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.trim().is_empty(),
            "no catalog may be printed for an unavailable base: {stdout}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("baseUnavailable"), "stderr: {stderr}");
        assert!(
            stderr.contains(&selected.display().to_string()),
            "stderr must name the selected agents.10.default.json path: {stderr}"
        );
        assert!(
            stderr.contains("no persisted catalog"),
            "stderr must carry the reason: {stderr}"
        );
    }
    assert!(
        !legacy_dir.exists(),
        "catalog reads must not create the legacy catalog directory"
    );
}

// #1967 P4 CLI contract: `add --from-catalog` resolves the PERSISTED catalog
// only. An unknown key keeps the existing not-found error and must leave
// settings byte-identical; a successful add seeds from the persisted sentinel
// and leaves the unrelated pre-existing registration unchanged.
#[test]
fn cli_add_from_catalog_is_persisted_only_and_preserves_existing_agents() {
    let tmp = Tmp::new("cli-catalog-add");
    let bin = copy_binary_into(tmp.path());
    let config_dir = config_dir_for_bin(&bin);
    let project = tmp.path().join("ProjectAlpha");
    std::fs::create_dir_all(&project).expect("create project");
    write_settings(&config_dir, &[&project]);

    // One unrelated, pre-existing registration.
    let settings_path = config_dir.join("settings.30.instance.no-git.json");
    let mut settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).expect("read settings"))
            .expect("settings json");
    settings["agents"] = serde_json::json!([{
        "id": "existing-1967",
        "label": "Existing",
        "command": "existing-1967-command",
        "color": "#123456",
        "envs": [],
        "isolatedHome": false
    }]);
    std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).expect("settings json"),
    )
    .expect("write settings");
    let before = std::fs::read(&settings_path).expect("read settings bytes");

    // Persisted catalog with one sentinel entry.
    let catalog_dir = project.join(".ac").join("coding-agents");
    std::fs::create_dir_all(&catalog_dir).expect("create catalog dir");
    std::fs::write(
        catalog_dir.join("agents.10.default.json"),
        concat!(
            r##"{"schemaVersion":1,"agents":[{"key":"sentinel-1967","label":"Sentinel","descript"##,
            r##"ion":"d","color":"#654321","command":"sentinel-1967-command","envs":[],"isolated"##,
            r##"Home":false,"removable":true,"updateCommands":["sentinel update"]}]}"##,
        ),
    )
    .expect("write catalog");

    // Unknown key: existing not-found error, settings byte-identical.
    let out = run_failure(
        &bin,
        &["coding-agent", "add", "--from-catalog", "missing-key"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("catalog key 'missing-key' not found"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read(&settings_path).expect("settings bytes"),
        before,
        "a failed add must not mutate settings"
    );

    // Successful add: seeded from the persisted sentinel; the pre-existing
    // registration keeps every field.
    run_success(
        &bin,
        &[
            "coding-agent",
            "add",
            "--from-catalog",
            "sentinel-1967",
            "--id",
            "added-1967",
        ],
    );
    // #2716 (B3): the saved `agents` live in the agents file beside the settings file.
    let after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_dir.join("agents.30.instance.no-git.json"))
            .expect("read agents file"),
    )
    .expect("agents json");
    let agents = after["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), 2, "exactly one agent was added: {after}");
    let existing = agents
        .iter()
        .find(|agent| agent["id"] == "existing-1967")
        .expect("existing agent");
    assert_eq!(existing["label"], "Existing");
    assert_eq!(existing["command"], "existing-1967-command");
    assert_eq!(existing["color"], "#123456");
    assert_eq!(existing["envs"], serde_json::json!([]));
    assert_eq!(existing["isolatedHome"], serde_json::json!(false));
    let added = agents
        .iter()
        .find(|agent| agent["id"] == "added-1967")
        .expect("added agent");
    assert_eq!(added["label"], "Sentinel");
    assert_eq!(added["baseCommand"], "sentinel-1967-command");
    assert!(added.get("command").is_none());
    assert_eq!(added["color"], "#654321");
    // #2306 P1 - the pre-existing record keeps position 0, the add lands at 1.
    assert_eq!(existing["order"], serde_json::json!(0));
    assert_eq!(added["order"], serde_json::json!(1));

    // #2306 P1 - `list`/`show` surface the normalized explicit order after load.
    let (listed, _) = run_json(&bin, &["coding-agent", "list"]);
    let listed = listed.as_array().expect("list is a JSON array");
    assert_eq!(listed[0]["id"], "existing-1967");
    assert_eq!(listed[0]["order"], serde_json::json!(0));
    assert_eq!(listed[1]["id"], "added-1967");
    assert_eq!(listed[1]["order"], serde_json::json!(1));
    let (shown, _) = run_json(&bin, &["coding-agent", "show", "--id", "added-1967"]);
    assert_eq!(shown["order"], serde_json::json!(1));
    assert_eq!(
        std::fs::read_to_string(catalog_dir.join("agents.10.default.json"))
            .expect("catalog persists"),
        concat!(
            r##"{"schemaVersion":1,"agents":[{"key":"sentinel-1967","label":"Sentinel","descript"##,
            r##"ion":"d","color":"#654321","command":"sentinel-1967-command","envs":[],"isolated"##,
            r##"Home":false,"removable":true,"updateCommands":["sentinel update"]}]}"##,
        ),
        "add --from-catalog never writes the persisted catalog"
    );
}
