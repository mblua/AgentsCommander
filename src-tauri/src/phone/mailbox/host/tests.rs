use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ROOTS: [&str; 3] = ["phone/mailbox", "pty/inject", "phone/terminal_snapshot"];
const EXCEPTIONS: [(&str, &str, usize); 4] = [
    ("phone/mailbox/notice.rs", "metadata_is_link_or_reparse", 2),
    (
        "phone/mailbox/notice.rs",
        "parse_team_from_workgroup_name",
        2,
    ),
    ("phone/mailbox/notice.rs", "validate_existing_name", 2),
    ("phone/mailbox/session_request.rs", "parse_task_title", 1),
];

fn collect_sources(path: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| name == "tests") {
            continue;
        }
        if path.is_dir() {
            collect_sources(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path.file_name().is_some_and(|name| name != "tests.rs")
        {
            files.push(path);
        }
    }
    Ok(())
}

fn sources(root: &Path) -> std::io::Result<Vec<(String, String)>> {
    let mut paths = Vec::new();
    for directory in ROOTS {
        collect_sources(&root.join(directory), &mut paths)?;
        paths.push(root.join(format!("{directory}.rs")));
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(root).expect("source root");
            let name = relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            Ok((name, std::fs::read_to_string(path)?))
        })
        .collect()
}

fn code_only(body: &str) -> Result<String, String> {
    rust_source_scrub::scrub(body, rust_source_scrub::Literals::Drop)
        .map_err(|error| format!("lexical error: {error:?}"))
}

// The existing lexer handles Rust syntax; this only separates its scrubbed tokens.
fn tokens(code: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut word = String::new();
    for character in code.chars() {
        if character.is_alphanumeric() || character == '_' {
            word.push(character);
        } else {
            if !word.is_empty() {
                result.push(std::mem::take(&mut word));
            }
            if !character.is_whitespace() {
                result.push(character.to_string());
            }
        }
    }
    if !word.is_empty() {
        result.push(word);
    }
    result
}

fn scan_file(path: &str, body: &str) -> Result<(), String> {
    let code = tokens(&code_only(body)?);
    let mut exceptions = BTreeMap::new();
    for (index, token) in code.iter().enumerate() {
        if token == "tauri" || token == "AppHandle" {
            return Err(format!("{path}: native token {token}"));
        }
        if matches!(token.as_str(), "state" | "try_state")
            && index > 0
            && code[index - 1] == "."
            && code.get(index + 1..index + 4) == Some(&[":".into(), ":".into(), "<".into()])
        {
            return Err(format!("{path}: native lookup {token}"));
        }
        if token == "commands" {
            let start = index.saturating_sub(3);
            let Some(symbol) = code.get(index + 6) else {
                return Err(format!("{path}: commands path"));
            };
            let expected = tokens(&format!("crate::commands::entity_creation::{symbol}"));
            let exact = index >= 3 && code.get(start..index + 7) == Some(expected.as_slice());
            if !exact
                || !EXCEPTIONS
                    .iter()
                    .any(|(file, name, _)| *file == path && *name == symbol)
            {
                return Err(format!("{path}: commands path"));
            }
            *exceptions.entry(symbol.as_str()).or_insert(0) += 1;
        }
    }
    for (file, symbol, count) in EXCEPTIONS {
        if file == path && exceptions.get(symbol).copied().unwrap_or(0) != count {
            return Err(format!("{path}: frozen exception count {symbol}"));
        }
    }
    Ok(())
}

fn scan(sources: &[(String, String)]) -> Result<(), String> {
    if sources.is_empty() {
        return Err("empty core scan set".into());
    }
    for (path, body) in sources {
        scan_file(path, body)?;
    }
    for (path, _, _) in EXCEPTIONS {
        if !sources.iter().any(|(name, _)| name == path) {
            return Err(format!("missing exception source {path}"));
        }
    }
    Ok(())
}

#[test]
fn issue_3946_core_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let actual = sources(&root).expect("read complete recursive core");
    assert!(!actual.is_empty());
    for (path, _) in &actual {
        println!("ISSUE3946 P21 CORE_PATH {path}");
    }
    println!("ISSUE3946 P21 CORE_COUNT {}", actual.len());
    assert_eq!(scan(&actual), Ok(()));

    let mut positive = actual.clone();
    positive.push((
        "phone/mailbox/future_leaf.rs".into(),
        r###"
        // tauri::AppHandle and app.state::<Native>() are not code.
        /* outer /* AppHandle */ tauri::async_runtime::spawn */
        const TEXT: &str = r##"tauri::AppHandle commands::phone::NativeAlias"##;
        fn business(value: Business) { let _ = value.state; let _ = value.state.enabled; }
        "###
        .into(),
    ));
    assert_eq!(scan(&positive), Ok(()));
    println!("ISSUE3946 P21 POSITIVE_PASS");
    for (name, injected, reason) in [
        (
            "handle",
            "use tauri::AppHandle as Native;",
            "native token tauri",
        ),
        (
            "bare_handle",
            "type Native = AppHandle;",
            "native token AppHandle",
        ),
        (
            "commands",
            "use commands::phone::NativeAlias;",
            "commands path",
        ),
        ("alias", "use crate::commands as edge;", "commands path"),
        (
            "lookup",
            "fn bad() { app.try_state::<T>(); }",
            "native lookup try_state",
        ),
        (
            "required",
            "fn bad() { app.state::<T>(); }",
            "native lookup state",
        ),
        (
            "fixture",
            "#[cfg(test)] fn fixture(app: &AppHandle) {}",
            "native token AppHandle",
        ),
        (
            "spawn",
            "fn bad() { tauri::async_runtime::spawn(task); }",
            "native token tauri",
        ),
    ] {
        let mut copied = positive.clone();
        copied.last_mut().unwrap().1.push_str(injected);
        let result = scan(&copied);
        assert_eq!(
            result,
            Err(format!("phone/mailbox/future_leaf.rs: {reason}"))
        );
        println!("ISSUE3946 P21 MUTANT_REJECTED {name}");
    }
    assert_eq!(scan(&[]), Err("empty core scan set".into()));
    assert!(
        code_only("/* unterminated").is_err(),
        "lex errors fail closed"
    );
}
