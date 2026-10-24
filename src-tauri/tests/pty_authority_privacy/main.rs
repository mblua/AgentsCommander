//! External, normal-build proof of the public PTY authority boundary.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const IMPORTS: &str = "use agentscommander_lib::pty::manager::{\n\
    BackendWriteAuthority, PtyInputPermit, PtyRouteWriteGuard, lock_route_for_write,\n\
};\n";
const POSITIVE: &str = "fn available(\n\
    _: Option<&BackendWriteAuthority>, permit: &PtyInputPermit,\n\
    _: Option<&PtyInputPermit>, _: Option<&PtyRouteWriteGuard<'_>>,\n\
    guard: &PtyRouteWriteGuard<'_>,\n\
) {\n\
    let _ = guard.write(b\"\");\n\
    let _ = lock_route_for_write(permit);\n\
}\n";

struct Library {
    compiler: PathBuf,
    rlib: PathBuf,
    dependencies: BTreeSet<PathBuf>,
}

struct Probe {
    name: &'static str,
    body: &'static str,
    code: &'static str,
    line: u64,
}

const PROBES: &[Probe] = &[
    Probe {
        name: "token_field",
        body: "fn denied() { let _ = BackendWriteAuthority { _private: () }; }\n",
        code: "E0451",
        line: 4,
    },
    Probe {
        name: "issuer",
        body: "fn denied() { let _ = BackendWriteAuthority::for_route_guard(); }\n",
        code: "E0624",
        line: 4,
    },
    Probe {
        name: "permit_field",
        body: "fn denied(value: PtyInputPermit) {\n\
            let PtyInputPermit { session_id: _, .. } = value;\n\
        }\n",
        code: "E0451",
        line: 5,
    },
    Probe {
        name: "guard_field",
        body: "fn denied(value: PtyRouteWriteGuard<'_>) {\n\
            let PtyRouteWriteGuard { session_id: _, .. } = value;\n\
        }\n",
        code: "E0451",
        line: 5,
    },
    Probe {
        name: "test_helper",
        body: "fn denied() { let _ = BackendWriteAuthority::for_backend_test(); }\n",
        code: "E0599",
        line: 4,
    },
];

fn bounded(command: &mut Command, prefix: &Path, limit: Duration) -> ExitStatus {
    let out = File::create(prefix.with_extension("stdout")).unwrap();
    let err = File::create(prefix.with_extension("stderr")).unwrap();
    println!("command={command:?}; logs={}", prefix.display());
    let start = Instant::now();
    let mut child = command
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            println!("status={status}; elapsed={:?}", start.elapsed());
            return status;
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let status = child.wait();
            panic!(
                "child timeout after {limit:?}: {status:?}; logs={}",
                prefix.display()
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn production_library(root: &Path, scratch: &Path) -> Library {
    for key in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "BUILD_PROFILE",
    ] {
        println!("{key}={:?}", env::var_os(key));
    }
    for key in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
    ] {
        let flags = env::var(key)
            .unwrap_or_default()
            .replace(['=', '\u{1f}'], " ");
        let words: Vec<_> = flags.split_whitespace().collect();
        assert!(
            !words.windows(2).any(|w| w == ["--cfg", "test"]),
            "cfg(test): {key}"
        );
    }
    let compiler = env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| "rustc".into());
    let version = Command::new(&compiler).arg("-vV").output().unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    println!("compiler={}; {version}", compiler.display());
    fs::write(scratch.join("compiler.txt"), &version).unwrap();
    let host = version
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap();
    if let Ok(target) = env::var("CARGO_BUILD_TARGET") {
        assert_eq!(
            target, host,
            "cross-target library cannot be used by host probes"
        );
    }
    let config = root.join(".cargo/config.toml");
    if config.exists() {
        println!(
            "workspace Cargo config:\n{}",
            fs::read_to_string(config).unwrap()
        );
    }
    let prefix = scratch.join("production-build");
    let status = bounded(
        Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args(["build", "--locked", "-p", "agentscommander", "--lib"])
            .arg("--message-format=json")
            .env("RUSTC", &compiler),
        &prefix,
        Duration::from_secs(40 * 60),
    );
    assert!(
        status.success(),
        "normal production build failed; logs={}",
        prefix.display()
    );
    let output = fs::read_to_string(prefix.with_extension("stdout")).unwrap();
    let mut libraries = Vec::new();
    let mut dependencies = BTreeSet::new();
    for line in output.lines() {
        let value: Value = serde_json::from_str(line).expect("Cargo JSON");
        if value["reason"] != "compiler-artifact" {
            continue;
        }
        for name in value["filenames"].as_array().unwrap() {
            let path = PathBuf::from(name.as_str().unwrap());
            if matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("rlib" | "rmeta")
            ) {
                assert!(
                    path.is_absolute() && path.is_file(),
                    "artifact {}",
                    path.display()
                );
                let parent = path.parent().unwrap().to_path_buf();
                assert!(parent.is_dir());
                dependencies.insert(parent);
            }
        }
        if value["target"]["name"] == "agentscommander_lib"
            && value["target"]["kind"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == "lib")
            && value["profile"]["test"] == false
        {
            let rlibs: Vec<_> = value["filenames"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.extension().is_some_and(|x| x == "rlib"))
                .collect();
            assert_eq!(rlibs.len(), 1, "one rlib per production artifact");
            libraries.push(rlibs[0].clone());
        }
    }
    assert_eq!(libraries.len(), 1, "exactly one normal library artifact");
    assert!(!dependencies.is_empty());
    println!(
        "selected_rlib={}; dependency_parents={dependencies:?}",
        libraries[0].display()
    );
    fs::write(
        scratch.join("artifacts.json"),
        serde_json::to_vec_pretty(&json!({
            "rlib": libraries[0], "dependency_parents": dependencies,
        }))
        .unwrap(),
    )
    .unwrap();
    Library {
        compiler,
        rlib: libraries.remove(0),
        dependencies,
    }
}

fn compile(library: &Library, scratch: &Path, name: &str, body: &str) -> (ExitStatus, String) {
    let source = scratch.join(format!("{name}.rs"));
    fs::write(&source, format!("{IMPORTS}{body}")).unwrap();
    let mut command = Command::new(&library.compiler);
    command
        .args(["--edition=2021", "--crate-type=lib", "--emit=metadata"])
        .arg("--error-format=json")
        .arg("--extern")
        .arg(format!("agentscommander_lib={}", library.rlib.display()));
    for parent in &library.dependencies {
        command
            .arg("-L")
            .arg(format!("dependency={}", parent.display()));
    }
    command.arg("--out-dir").arg(scratch).arg(&source);
    let prefix = scratch.join(name);
    let status = bounded(&mut command, &prefix, Duration::from_secs(60));
    let diagnostics = fs::read_to_string(prefix.with_extension("stderr")).unwrap();
    (status, diagnostics)
}

fn rejected(success: bool, diagnostics: &str, source: &Path, line: u64, code: &str) -> bool {
    if success {
        return false;
    }
    let mut matches = 0;
    let mut parsed = 0;
    for raw in diagnostics.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return false;
        };
        if value["$message_type"] != "diagnostic" {
            return false;
        }
        parsed += 1;
        if value["level"] != "error" {
            continue;
        }
        // rustc's final error summary has no code or operation span.
        if value["code"].is_null()
            && value["message"]
                .as_str()
                .is_some_and(|s| s.starts_with("aborting due to"))
            && value["spans"].as_array().is_some_and(Vec::is_empty)
        {
            continue;
        }
        if value["code"]["code"] != code {
            return false;
        }
        let Some(spans) = value["spans"].as_array() else {
            return false;
        };
        if !spans.iter().any(|span| {
            span["is_primary"] == true
                && span["line_start"] == line
                && span["file_name"]
                    .as_str()
                    .is_some_and(|s| Path::new(s) == source)
        }) {
            return false;
        }
        matches += 1;
    }
    parsed > 0 && matches == 1
}

#[test]
fn diagnostic_classifier_controls() {
    let source = Path::new("owned.rs");
    let diagnostic = json!({
        "$message_type": "diagnostic", "level": "error", "code": {"code": "E0451"},
        "spans": [{"is_primary": true, "file_name": "owned.rs", "line_start": 4}],
    });
    let text = diagnostic.to_string();
    assert!(rejected(false, &text, source, 4, "E0451"));
    assert!(!rejected(true, &text, source, 4, "E0451"));
    assert!(!rejected(false, &text, source, 4, "E0624"));
    assert!(!rejected(false, &text, source, 5, "E0451"));
    assert!(!rejected(false, &text, Path::new("other.rs"), 4, "E0451"));
    assert!(!rejected(false, "", source, 4, "E0451"));
    assert!(!rejected(false, "not JSON", source, 4, "E0451"));
    let mut unresolved = diagnostic;
    unresolved["code"]["code"] = json!("E0432");
    assert!(!rejected(
        false,
        &unresolved.to_string(),
        source,
        4,
        "E0451"
    ));
    let extra = format!("{text}\n{unresolved}");
    assert!(!rejected(false, &extra, source, 4, "E0451"));
    assert!(!rejected(
        false,
        &format!("{text}\n{text}"),
        source,
        4,
        "E0451"
    ));
    let mut secondary = serde_json::from_str::<Value>(&text).unwrap();
    secondary["spans"][0]["is_primary"] = json!(false);
    assert!(!rejected(false, &secondary.to_string(), source, 4, "E0451"));
    secondary["$message_type"] = json!("artifact");
    assert!(!rejected(false, &secondary.to_string(), source, 4, "E0451"));
    println!("classifier controls: wrong code/line/path, success, missing/invalid/extra JSON");
}

#[test]
fn public_authority_privacy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let parent = root.join("target/3198-privacy");
    fs::create_dir_all(&parent).unwrap();
    let scratch = tempfile::Builder::new()
        .prefix("external-")
        .tempdir_in(parent)
        .unwrap()
        .keep();
    println!("retained privacy receipts={}", scratch.display());
    let library = production_library(root, &scratch);
    let (status, diagnostics) = compile(&library, &scratch, "positive", POSITIVE);
    assert!(status.success(), "positive control: {diagnostics}");
    println!("positive control PASS");
    for probe in PROBES {
        let (status, diagnostics) = compile(&library, &scratch, probe.name, probe.body);
        assert_eq!(
            status.code(),
            Some(1),
            "{}: expected rustc rejection, not ICE or signal",
            probe.name
        );
        let source = scratch.join(format!("{}.rs", probe.name));
        assert!(
            rejected(
                status.success(),
                &diagnostics,
                &source,
                probe.line,
                probe.code
            ),
            "{}: expected {} at line {}; diagnostics={diagnostics}",
            probe.name,
            probe.code,
            probe.line
        );
        println!(
            "{} PASS {} {}:{}",
            probe.name,
            probe.code,
            source.display(),
            probe.line
        );
        let name = format!("{}_valid", probe.name);
        let (valid, diagnostics) = compile(&library, &scratch, &name, POSITIVE);
        assert!(valid.success(), "valid substitution: {diagnostics}");
        assert!(!rejected(
            false,
            &diagnostics,
            &scratch.join(format!("{name}.rs")),
            probe.line,
            probe.code
        ));
        assert!(!rejected(
            valid.success(),
            &diagnostics,
            &scratch.join(format!("{name}.rs")),
            probe.line,
            probe.code
        ));
        println!(
            "{} valid-body substitution rejected by predicate",
            probe.name
        );
    }
    let (status, diagnostics) =
        compile(&library, &scratch, "unresolved", "use missing_crate::X;\n");
    assert!(status.code().is_some_and(|c| c != 0));
    assert!(!rejected(
        false,
        &diagnostics,
        &scratch.join("unresolved.rs"),
        4,
        "E0451"
    ));
    println!("unresolved import rejected by classifier");
}
