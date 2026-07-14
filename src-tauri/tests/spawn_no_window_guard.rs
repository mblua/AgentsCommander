//! #992 - Every production `Command::new(...)` must set `CREATE_NO_WINDOW` on Windows.
//!
//! AgentsCommander is a GUI-subsystem process and owns no console. When it spawns a
//! console-subsystem child without `CREATE_NO_WINDOW`, Windows allocates a NEW console
//! for that child, and Win11 delegates it to Windows Terminal: a visible tab titled with
//! the child's resolved image path. That is #992, reported as a stray
//! `C:\Program Files\Docker\Doc...` tab on every start.
//!
//! WHY THIS IS A SOURCE SCANNER AND NOT A BEHAVIOURAL TEST.
//! `CREATE_NO_WINDOW` only has an effect when Windows would otherwise allocate a *new*
//! console, i.e. only when the parent owns none. A `cargo test` binary always owns one
//! (a real conhost from a terminal, or a ConPTY under CI), so the child inherits it and
//! the flag becomes a literal no-op: verified empirically, a child writing to `CONOUT$`
//! succeeded with AND without the flag under the harness. A behavioural test would
//! therefore pass identically against the broken code, which is worse than no test.
//! `std::process::Command` also exposes no getter for its creation flags. So the
//! invariant is enforced where it is actually visible: in the source. Same idiom as
//! `config/local_config_io.rs`'s `agent_replica_root_config_writes_go_through_shared_helper`.
//!
//! A GUARD THAT CANNOT FAIL IS A DECORATION. The adversarial tests at the bottom feed
//! the analyzer sources it MUST reject. Four of them are reproductions of false
//! negatives found in review, and each one is the reason a rule below exists:
//!   - the flag must belong to THIS command, not to a neighbouring one,
//!   - `// creation_flags(` in a comment or a string proves nothing,
//!   - an item is skipped only when its cfg is PROVABLY FALSE on Windows, so
//!     `#[cfg(any(windows, unix))]` and `#[cfg(not(unix))]` are still policed,
//!   - the allowlist names SITES, never whole files, and every entry needs a reason.
//!
//! Do not relax a rule without deleting the test that proves why it is there.

use std::path::{Path, PathBuf};

/// Spawn sites that legitimately do not set the flag: `(file, needle in the spawn line,
/// reason)`. A site is excused only when its own line matches, so one justified call
/// never blesses the rest of the file. An empty reason does not excuse anything.
///
/// It is empty, and that is the target state. Every known spawn site either sets the
/// flag or is compiled out on Windows.
const ALLOWED: &[(&str, &str, &str)] = &[];

// ─────────────────────────────── the invariant ───────────────────────────────

#[test]
fn every_production_command_spawn_sets_create_no_window() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(
        files.len() > 20,
        "scanned only {} files under {}; the walker is broken, not the tree",
        files.len(),
        src.display()
    );

    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    for file in &files {
        let rel = relative_slash_path(file, &src);
        let source = std::fs::read_to_string(file).expect("read source file");
        let report = analyze(&rel, &source, ALLOWED);
        scanned += report.scanned_sites;
        offenders.extend(report.offenders);
    }

    assert!(
        scanned > 5,
        "found only {scanned} production Command::new sites; the cfg stripper ate the tree"
    );
    assert!(
        offenders.is_empty(),
        "#992: these spawn sites do not set CREATE_NO_WINDOW. A GUI-subsystem parent \
         spawning a console child without it pops a Windows Terminal tab. Add the flag \
         (see pty/docker_runtime.rs), or add an ALLOWED entry with a reason:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn allowlist_entries_have_reasons_and_none_are_stale() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);

    for (path, needle, reason) in ALLOWED {
        assert!(
            !reason.trim().is_empty(),
            "ALLOWED entry {path}/{needle} has no reason. An allowlist entry without a \
             stated reason is a silent bug."
        );
        let matched = files.iter().any(|file| {
            relative_slash_path(file, &src) == *path
                && std::fs::read_to_string(file)
                    .map(|source| {
                        spawn_sites(&mask_and_strip(&source), &source)
                            .iter()
                            .any(|site| site.line_text.contains(needle))
                    })
                    .unwrap_or(false)
        });
        assert!(
            matched,
            "stale ALLOWED entry: {path} has no spawn site matching {needle:?}. Delete it."
        );
    }
}

// ────────────────────────────────── analyzer ──────────────────────────────────

struct Report {
    scanned_sites: usize,
    offenders: Vec<String>,
}

struct Site {
    offset: usize,
    line: usize,
    line_text: String,
}

fn analyze(rel: &str, source: &str, allowed: &[(&str, &str, &str)]) -> Report {
    let masked = mask_and_strip(source);
    let sites = spawn_sites(&masked, source);
    let bodies = function_bodies(masked.as_bytes());

    let mut report = Report {
        scanned_sites: 0,
        offenders: Vec::new(),
    };
    for site in sites {
        if is_allowed(rel, &site.line_text, allowed) {
            continue;
        }
        report.scanned_sites += 1;
        if !flag_belongs_to(&masked, &site, &bodies) {
            report.offenders.push(format!("{rel}:{}", site.line));
        }
    }
    report
}

fn is_allowed(rel: &str, line_text: &str, allowed: &[(&str, &str, &str)]) -> bool {
    allowed.iter().any(|(path, needle, reason)| {
        *path == rel && !reason.trim().is_empty() && line_text.contains(needle)
    })
}

/// The flag must be anchored to THE COMMAND IT CONFIGURES. There is no look-ahead window
/// at any width: a window is what let a real flag be deleted from
/// `commands/entity_creation.rs` while the guard stayed green, because an unrelated
/// command's flag happened to sit inside it.
///
/// A spawn is satisfied when either
///   1. `creation_flags(` appears in the SAME statement (the builder-chain shape:
///      `Command::new("cmd.exe").args(..).creation_flags(..).spawn()`), or
///   2. the spawn binds a name (`let mut cmd = Command::new(..)`) and a later statement
///      IN THE SAME FUNCTION starts with that binding and sets the flag on it
///      (`cmd.creation_flags(..)`, `cmd.arg("x").creation_flags(..)`).
///
/// `cmd2.creation_flags(..)` therefore does nothing for `cmd`, and a flag in the next
/// function does nothing for this one.
fn flag_belongs_to(masked: &str, site: &Site, bodies: &[(usize, usize)]) -> bool {
    let bytes = masked.as_bytes();
    let (body_start, body_end) = enclosing_body(site.offset, bodies)
        .unwrap_or((0, bytes.len()));
    let statements = statements(bytes, body_start, body_end);

    let Some(spawn_stmt) = statements
        .iter()
        .position(|(start, end)| *start <= site.offset && site.offset < *end)
    else {
        return false;
    };
    let text = |(start, end): &(usize, usize)| &masked[*start..*end];

    // 1. the builder-chain shape: the flag is part of the spawn expression itself.
    if text(&statements[spawn_stmt]).contains("creation_flags(") {
        return true;
    }

    // 2. the binding shape: only `<binding>.….creation_flags(…)` counts, and only later
    //    in the same function.
    let Some(binding) = binding_of(text(&statements[spawn_stmt])) else {
        return false;
    };
    let prefix = format!("{binding}.");
    statements[spawn_stmt + 1..].iter().any(|stmt| {
        let stmt = strip_leading_attrs(text(stmt));
        stmt.starts_with(&prefix) && stmt.contains("creation_flags(")
    })
}

/// A statement may carry attributes, and the tree's commonest flag shape is
/// `#[cfg(windows)]` on the very statement that sets it. They are not part of the
/// receiver, so strip them before asking whose command this is.
fn strip_leading_attrs(statement: &str) -> &str {
    let mut rest = statement.trim_start();
    while let Some(after_open) = rest.strip_prefix("#[") {
        let Some(close) = after_open.find(']') else {
            break;
        };
        rest = after_open[close + 1..].trim_start();
    }
    rest
}

fn enclosing_body(offset: usize, bodies: &[(usize, usize)]) -> Option<(usize, usize)> {
    bodies
        .iter()
        .filter(|(open, close)| *open < offset && offset < *close)
        .max_by_key(|(open, _)| *open)
        .copied()
}

/// `let mut cmd = Command::new("git")` -> `cmd`.
fn binding_of(statement: &str) -> Option<String> {
    let rest = strip_leading_attrs(statement).strip_prefix("let ")?.trim_start();
    let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Split a function body into statements at `;` and at block braces. Comments and
/// literals are already masked, so a `;` inside a string cannot split anything.
fn statements(bytes: &[u8], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = from;
    let mut i = from;
    while i < to {
        match bytes[i] {
            b';' | b'{' | b'}' => {
                if i > start {
                    spans.push((start, i));
                }
                start = i + 1;
                i += 1;
            }
            _ => i = advance(bytes, i).max(i + 1),
        }
    }
    if start < to {
        spans.push((start, to));
    }
    spans
}

/// Sites are found in the masked source (so a `Command::new(` inside a comment is not a
/// site), but each one carries its ORIGINAL line text: an allowlist needle is normally a
/// program name, which lives inside a string literal that masking blanks out.
fn spawn_sites(masked: &str, original: &str) -> Vec<Site> {
    let bytes = masked.as_bytes();
    let mut sites = Vec::new();
    for (idx, _) in masked.match_indices("Command::new(") {
        // Skip `foo_Command::new(`-style false hits; `std::process::Command::new(` is real.
        if idx > 0 && (bytes[idx - 1].is_ascii_alphanumeric() || bytes[idx - 1] == b'_') {
            continue;
        }
        let line = line_of(masked, idx);
        sites.push(Site {
            offset: idx,
            line,
            line_text: original.lines().nth(line - 1).unwrap_or_default().to_string(),
        });
    }
    sites
}

/// Body ranges of every `fn`, so a flag in the next function cannot excuse this one.
fn function_bodies(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        let is_kw = bytes[i..].starts_with(b"fn ")
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'));
        if !is_kw {
            i = advance(bytes, i);
            continue;
        }
        // First `{` after the signature opens the body. A `;` first means no body
        // (a trait method declaration).
        let mut j = i + 3;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => {
                    if let Some(close) = matching(bytes, j, b'{', b'}') {
                        bodies.push((j, close));
                    }
                    break;
                }
                b';' => break,
                _ => {}
            }
            j = advance(bytes, j);
        }
        i = j.max(i + 3);
    }
    bodies
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

// ───────────────────────────── masking and cfg stripping ─────────────────────────────

/// Blank out, preserving every byte offset and line number:
///   - every item whose cfg is provably false in a production Windows build, because
///     `CommandExt::creation_flags` does not exist off Windows and test code is not
///     shipped,
///   - then comment and string/char literal CONTENT, so `// creation_flags(` proves
///     nothing.
///
/// The order is load-bearing. `cfg(target_os = "windows")` must be evaluated while the
/// string literal still says `windows`: mask first and it reads as `target_os = ""`,
/// which is false on every target, so the guard would strip exactly the Windows code it
/// exists to police and keep the non-Windows code it must ignore. Both directions were
/// observed before this was fixed.
fn mask_and_strip(source: &str) -> String {
    let stripped = strip_items_absent_on_windows(source);
    mask_comments_and_literals(&stripped)
}

fn mask_comments_and_literals(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out: Vec<u8> = source.bytes().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let next = advance(bytes, i);
        if next > i + 1 {
            // `advance` skipped a comment or a literal: blank its interior, keep newlines.
            for b in out.iter_mut().take(next.min(bytes.len())).skip(i) {
                if *b != b'\n' {
                    *b = b' ';
                }
            }
        }
        i = next.max(i + 1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_items_absent_on_windows(masked: &str) -> String {
    let bytes = masked.as_bytes();
    let mut out: Vec<u8> = masked.bytes().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"#[") {
            let Some(attr_end) = matching(bytes, i + 1, b'[', b']') else {
                break;
            };
            let attr = String::from_utf8_lossy(&bytes[i..=attr_end]).into_owned();
            if cfg_of(&attr).map(|pred| eval_cfg(&pred)) == Some(Tri::False) {
                let end = item_end_from(bytes, attr_end + 1).unwrap_or(bytes.len() - 1);
                for b in out.iter_mut().take(end + 1).skip(i) {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = end + 1;
                continue;
            }
            i = attr_end + 1;
            continue;
        }
        i = advance(bytes, i);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The predicate inside `#[cfg(...)]`, or `None` for any other attribute.
fn cfg_of(attr: &str) -> Option<String> {
    let inner = attr.strip_prefix("#[")?.strip_suffix(']')?.trim();
    let pred = inner.strip_prefix("cfg(")?.strip_suffix(')')?;
    Some(pred.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tri {
    True,
    False,
    Unknown,
}

/// Evaluate a cfg predicate as a PRODUCTION WINDOWS build sees it: `windows` and
/// `target_os = "windows"` hold, `unix` and every other `target_os` do not, and `test` is
/// off. Anything else (a feature, `debug_assertions`) is `Unknown`.
///
/// An item is stripped only on `False`, never on `Unknown`. That is the whole point:
/// `any(windows, unix)`, `not(unix)` and `feature = "unix-sockets"` all keep their spawn
/// sites under the guard, where a "does the text mention unix" check would have thrown
/// them away.
fn eval_cfg(pred: &str) -> Tri {
    let pred = pred.trim();
    if let Some(inner) = inside(pred, "not") {
        return match eval_cfg(inner) {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        };
    }
    if let Some(inner) = inside(pred, "all") {
        let parts = split_top_level(inner);
        if parts.iter().any(|p| eval_cfg(p) == Tri::False) {
            return Tri::False;
        }
        if parts.iter().any(|p| eval_cfg(p) == Tri::Unknown) {
            return Tri::Unknown;
        }
        return Tri::True;
    }
    if let Some(inner) = inside(pred, "any") {
        let parts = split_top_level(inner);
        if parts.iter().any(|p| eval_cfg(p) == Tri::True) {
            return Tri::True;
        }
        if parts.iter().any(|p| eval_cfg(p) == Tri::Unknown) {
            return Tri::Unknown;
        }
        return Tri::False;
    }
    if let Some((key, value)) = pred.split_once('=') {
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        return match key {
            "target_os" | "target_family" => {
                if value == "windows" {
                    Tri::True
                } else {
                    Tri::False
                }
            }
            _ => Tri::Unknown,
        };
    }
    match pred {
        "windows" => Tri::True,
        "unix" => Tri::False,
        // Not shipped, so not policed. `any(test, windows)` still evaluates True and IS
        // policed, which is correct: that code is in the production Windows binary.
        "test" => Tri::False,
        _ => Tri::Unknown,
    }
}

/// `inside("all(a, b)", "all") == Some("a, b")`.
fn inside<'a>(pred: &'a str, kw: &str) -> Option<&'a str> {
    let rest = pred.strip_prefix(kw)?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    Some(inner)
}

fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// End of the item starting at `from`: the `}` closing its first block, or its first
/// `;` when it has none (`#[cfg(windows)] const X: u32 = 1;`).
fn item_end_from(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => return matching(bytes, i, b'{', b'}'),
            b';' => return Some(i),
            b'#' if bytes[i..].starts_with(b"#[") => {
                // A stacked attribute (`#[cfg(test)]` then `#[allow(...)]`).
                i = matching(bytes, i + 1, b'[', b']')? + 1;
                continue;
            }
            _ => {}
        }
        i = advance(bytes, i);
    }
    None
}

/// Bracket matcher that skips comments and literals.
fn matching(bytes: &[u8], open: usize, open_ch: u8, close_ch: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        if bytes[i] == open_ch {
            depth += 1;
        } else if bytes[i] == close_ch {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i = advance(bytes, i);
    }
    None
}

/// Index of the next byte of real code, skipping any comment, string, raw string or char
/// literal starting at `i`. Always advances at least one byte.
///
/// Everything here indexes BYTES, never `&str` slices: the tree contains multi-byte
/// characters and `source[i..]` panics when `i` lands inside one.
fn advance(bytes: &[u8], i: usize) -> usize {
    let rest = &bytes[i..];

    if rest.starts_with(b"//") {
        return match rest.iter().position(|b| *b == b'\n') {
            Some(nl) => i + nl,
            None => bytes.len(),
        };
    }
    if rest.starts_with(b"/*") {
        let mut depth = 1usize;
        let mut j = i + 2;
        while j < bytes.len() {
            if bytes[j..].starts_with(b"/*") {
                depth += 1;
                j += 2;
            } else if bytes[j..].starts_with(b"*/") {
                depth -= 1;
                j += 2;
                if depth == 0 {
                    return j;
                }
            } else {
                j += 1;
            }
        }
        return bytes.len();
    }
    // Raw string: r"...", r#"..."#, br#"..."#.
    let hash_start = if rest.starts_with(b"br") {
        Some(i + 2)
    } else if rest.starts_with(b"r") {
        Some(i + 1)
    } else {
        None
    };
    if let Some(start) = hash_start {
        let hashes = bytes[start..].iter().take_while(|b| **b == b'#').count();
        let quote = start + hashes;
        if bytes.get(quote) == Some(&b'"') {
            let mut terminator = vec![b'"'];
            terminator.resize(1 + hashes, b'#');
            let mut j = quote + 1;
            while j + terminator.len() <= bytes.len() {
                if bytes[j..].starts_with(&terminator) {
                    return j + terminator.len();
                }
                j += 1;
            }
            return bytes.len();
        }
    }
    if bytes[i] == b'"' {
        let mut j = i + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => return j + 1,
                _ => j += 1,
            }
        }
        return bytes.len();
    }
    // A `'` opens a char literal only in the `'x'` / `'\n'` shapes. Otherwise it is a
    // lifetime (`&'a str`), which is ordinary code: treating a lifetime as a literal
    // would make the scanner swallow everything up to the next `'` in the file.
    if bytes[i] == b'\'' {
        let escaped = bytes.get(i + 1) == Some(&b'\\');
        let simple = bytes.get(i + 2) == Some(&b'\'');
        if escaped || simple {
            let mut j = i + 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    b'\'' => return j + 1,
                    _ => j += 1,
                }
            }
            return bytes.len();
        }
    }
    i + 1
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn relative_slash_path(file: &Path, root: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

// ─────────────────────── adversarial tests: the guard must FAIL ───────────────────────

fn offenders(source: &str) -> Vec<String> {
    analyze("fixture.rs", source, ALLOWED).offenders
}

/// Reproduction 1. Deleting a flag must be caught even when an unrelated command sets its
/// own flag within the lookahead window. This is the false negative that let a flag be
/// removed from `commands/entity_creation.rs` with the guard still green.
#[test]
fn guard_catches_a_flag_stolen_from_a_neighbouring_command() {
    let source = r#"
fn two_commands() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
    let mut cmd2 = std::process::Command::new("git");
    cmd2.arg("log");
    cmd2.creation_flags(CREATE_NO_WINDOW);
}
"#;
    assert_eq!(
        offenders(source),
        vec!["fixture.rs:3"],
        "the first command has no flag of its own; cmd2's flag must not excuse it"
    );
}

/// Reproduction 2. Text in a comment or a string is not code.
#[test]
fn guard_is_not_fooled_by_creation_flags_in_a_comment_or_a_string() {
    let commented = r#"
fn commented() {
    let mut cmd = std::process::Command::new("git");
    // creation_flags(CREATE_NO_WINDOW) would go here
}
"#;
    let stringed = r#"
fn stringed() {
    let mut cmd = std::process::Command::new("git");
    log::debug!("call creation_flags(CREATE_NO_WINDOW) some day");
}
"#;
    assert_eq!(offenders(commented), vec!["fixture.rs:3"]);
    assert_eq!(offenders(stringed), vec!["fixture.rs:3"]);
}

/// Reproduction 3. An item is skipped only when its cfg is PROVABLY FALSE on Windows.
/// "The cfg text mentions unix" is not that, and every case below ships on Windows.
#[test]
fn guard_still_polices_cfgs_that_merely_mention_unix() {
    let any_windows_unix = r#"
#[cfg(any(windows, unix))]
fn portable() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
}
"#;
    let not_unix = r#"
#[cfg(not(unix))]
fn not_unix() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
}
"#;
    let feature_named_unix = r#"
#[cfg(feature = "unix-sockets")]
fn feature_gated() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
}
"#;
    let test_or_windows = r#"
#[cfg(any(test, windows))]
fn ships_on_windows_too() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
}
"#;
    assert_eq!(offenders(any_windows_unix), vec!["fixture.rs:4"]);
    assert_eq!(offenders(not_unix), vec!["fixture.rs:4"]);
    assert_eq!(offenders(feature_named_unix), vec!["fixture.rs:4"]);
    assert_eq!(offenders(test_or_windows), vec!["fixture.rs:4"]);
}

/// Reproduction 4. The allowlist names sites, not files: one excused call must not blind
/// the guard to the next one. And a reason is mandatory.
#[test]
fn allowlist_excuses_one_site_not_a_whole_file() {
    let source = r#"
fn two_sites() {
    let mut excused = std::process::Command::new("tasklist");
    excused.arg("/FI");
    let mut sneaked_in = std::process::Command::new("docker");
    sneaked_in.arg("ps");
}
"#;
    let allowed = &[("fixture.rs", "tasklist", "a stated, non-empty reason")][..];
    assert_eq!(
        analyze("fixture.rs", source, allowed).offenders,
        vec!["fixture.rs:5"],
        "excusing the tasklist site must not excuse the docker site below it"
    );

    let no_reason = &[("fixture.rs", "tasklist", "   ")][..];
    assert_eq!(
        analyze("fixture.rs", source, no_reason).offenders,
        vec!["fixture.rs:3", "fixture.rs:5"],
        "an entry with a blank reason excuses nothing"
    );
}

/// The flag shapes that are actually in the tree must pass, or the guard is unusable.
#[test]
fn guard_accepts_the_real_flag_shapes() {
    let binding = r#"
fn binding_form() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}
"#;
    let chained = r#"
fn chain_form() {
    let child = std::process::Command::new("cmd.exe")
        .args(["/C", "dir"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
"#;
    let method_chain_on_binding = r#"
fn method_chain() {
    let mut cmd = std::process::Command::new("where.exe");
    cmd.arg("git.exe").creation_flags(CREATE_NO_WINDOW);
}
"#;
    assert!(offenders(binding).is_empty());
    assert!(offenders(chained).is_empty());
    assert!(offenders(method_chain_on_binding).is_empty());
}

/// Code that is genuinely absent from a Windows build cannot set a Windows-only flag.
#[test]
fn guard_skips_code_that_windows_never_compiles() {
    let unix_only = r#"
#[cfg(unix)]
fn unix_only() {
    let mut cmd = std::process::Command::new("kill");
    cmd.arg("-0");
}
"#;
    let linux_only = r#"
#[cfg(target_os = "linux")]
fn linux_only() {
    let mut cmd = std::process::Command::new("systemctl");
    cmd.arg("status");
}
"#;
    let test_only = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn spawns_in_a_test() {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/c");
    }
}
"#;
    assert!(offenders(unix_only).is_empty());
    assert!(offenders(linux_only).is_empty());
    assert!(offenders(test_only).is_empty());
}

/// A flag in the NEXT function never excuses this one.
#[test]
fn guard_does_not_credit_a_flag_from_another_function() {
    let source = r#"
fn offender() {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("status");
}

fn innocent() {
    let mut other = std::process::Command::new("git");
    other.creation_flags(CREATE_NO_WINDOW);
}
"#;
    assert_eq!(offenders(source), vec!["fixture.rs:3"]);
}

#[test]
fn cfg_evaluator_is_tri_state() {
    assert_eq!(eval_cfg("windows"), Tri::True);
    assert_eq!(eval_cfg("unix"), Tri::False);
    assert_eq!(eval_cfg("test"), Tri::False);
    assert_eq!(eval_cfg("not(unix)"), Tri::True);
    assert_eq!(eval_cfg("not(windows)"), Tri::False);
    assert_eq!(eval_cfg("any(windows, unix)"), Tri::True);
    assert_eq!(eval_cfg("all(test, windows)"), Tri::False);
    assert_eq!(eval_cfg("any(test, windows)"), Tri::True);
    assert_eq!(eval_cfg(r#"target_os = "windows""#), Tri::True);
    assert_eq!(eval_cfg(r#"target_os = "linux""#), Tri::False);
    assert_eq!(eval_cfg(r#"not(target_os = "windows")"#), Tri::False);
    assert_eq!(eval_cfg(r#"feature = "unix-sockets""#), Tri::Unknown);
    assert_eq!(eval_cfg("debug_assertions"), Tri::Unknown);
    // Unknown must never strip: `all(feature = "x", windows)` might ship on Windows.
    assert_eq!(eval_cfg(r#"all(feature = "x", windows)"#), Tri::Unknown);
}
