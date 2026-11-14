//! #4389: TypeScript form of the remote-activity payload, generated from Rust.
//! A normal run compares it with the committed file; `AC_TS_BINDINGS_WRITE=1` rewrites it.
use super::*;
use ts_rs::{Config, TS};

const HEADER: &str = "// Generated from Rust by ts-rs (#4389). Do not edit by hand.";
const TARGET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../src/shared/generated/remote-activity.ts"
);

/// One declaration as committed: `export ` + ts-rs text. An object body is split into one
/// field per line; any field holding `{`, `}` or `,` fails, so a nested shape cannot slip in.
fn render(decl: &str) -> String {
    let Some((head, rest)) = decl.split_once(" = { ") else {
        return format!("export {decl}\n");
    };
    let body = rest
        .strip_suffix(" };")
        .expect("object declaration ends with ` };`");
    let mut out = format!("export {head} = {{\n");
    for field in body.split(", ") {
        let field = field.strip_suffix(',').unwrap_or(field);
        assert!(
            !field.contains(['{', '}', ',']),
            "unsupported field shape: {field}"
        );
        out.push_str(&format!("  {field},\n"));
    }
    out.push_str("};\n");
    out
}

fn bindings() -> String {
    // `Config::new()`, not `from_env()`: shell `TS_RS_*` variables cannot change the output.
    let cfg = Config::new();
    let decls = [
        CiState::decl(&cfg),
        StalenessState::decl(&cfg),
        RemoteActivityPayload::decl(&cfg),
    ];
    let mut out = format!("{HEADER}\n");
    for decl in decls {
        out.push('\n');
        out.push_str(&render(&decl));
    }
    out
}

#[test]
fn ts_bindings_4389_remote_activity_file_matches() {
    let text = bindings();
    for line in text.lines() {
        assert!(line.chars().count() <= 100, "line over 100 chars: {line}");
    }
    if std::env::var_os("AC_TS_BINDINGS_WRITE").is_some_and(|v| v == "1") {
        std::fs::write(TARGET, &text).expect("write generated bindings");
    }
    let committed = std::fs::read_to_string(TARGET)
        .expect("missing src/shared/generated/remote-activity.ts; run with AC_TS_BINDINGS_WRITE=1");
    assert_eq!(
        committed, text,
        "stale TS bindings; rerun with AC_TS_BINDINGS_WRITE=1"
    );
}
