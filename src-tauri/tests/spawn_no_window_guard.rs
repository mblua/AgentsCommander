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
//! Its value is not proving today's one-liner (a reproduction did that). It is stopping
//! the NEXT spawn site from reintroducing #992, which is exactly how the bug got in.

use std::path::{Path, PathBuf};

/// Spawn sites that legitimately do not set the flag. Every entry states its reason;
/// an entry without one fails review.
const ALLOWED: &[(&str, &str)] = &[(
    "cli/harness.rs",
    "CLI-only: the `harness` subcommand runs a user-supplied command with INHERITED \
     stdio so its output reaches the caller's terminal. The CLI entry point calls \
     `cli::attach_parent_console()` before dispatch, so the process already owns a \
     console and the flag would be a no-op there anyway.",
)];

/// How far below a `Command::new(` the `creation_flags(` call may sit. The widest
/// legitimate gap in the tree today is 12 lines (`pty/git_watcher.rs`). Never widen this
/// past the enclosing function.
const LOOKAHEAD_LINES: usize = 30;

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

    let mut scanned_sites = 0usize;
    let mut offenders = Vec::new();

    for file in &files {
        let rel = relative_slash_path(file, &src);
        if ALLOWED.iter().any(|(allowed, _)| *allowed == rel) {
            continue;
        }
        let source = std::fs::read_to_string(file).expect("read source file");
        // Blank out code that cannot or need not carry the flag, keeping line numbers
        // intact so offenders report their true `file:line`.
        let production = blank_out_items(&source, is_test_or_non_windows_attr);
        let lines: Vec<&str> = production.lines().collect();

        for (idx, line) in lines.iter().enumerate() {
            if !line.contains("Command::new(") {
                continue;
            }
            scanned_sites += 1;
            let end = (idx + LOOKAHEAD_LINES).min(lines.len().saturating_sub(1));
            let has_flag = lines[idx..=end]
                .iter()
                .any(|l| l.contains("creation_flags("));
            if !has_flag {
                offenders.push(format!("{}:{}", rel, idx + 1));
            }
        }
    }

    assert!(
        scanned_sites > 5,
        "found only {} production Command::new sites; the cfg stripper ate the tree",
        scanned_sites
    );

    assert!(
        offenders.is_empty(),
        "#992: these spawn sites do not set CREATE_NO_WINDOW within {} lines. A \
         GUI-subsystem parent spawning a console child without it pops a Windows Terminal \
         tab. Add the flag (see pty/docker_runtime.rs), or add an ALLOWED entry with a \
         reason:\n{}",
        LOOKAHEAD_LINES,
        offenders.join("\n")
    );
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

/// Items this guard must not police:
///   - `#[cfg(test)]` code. Seven flagless `Command::new` sites live in test modules
///     (`commands/session.rs`, `commands/repos.rs`, `commands/entity_creation.rs`,
///     `commands/wg_delete_diagnostic.rs`, `config/config_seed.rs`, and two in
///     `pty/credentials.rs`). They spawn from a console-owning test binary.
///   - Non-Windows items. `CommandExt::creation_flags` does not exist off Windows, so a
///     `#[cfg(unix)]` spawn cannot set the flag and must not be required to.
fn is_test_or_non_windows_attr(attr: &str) -> bool {
    if !attr.contains("cfg(") {
        return false;
    }
    let is_test = attr.contains("cfg(test)")
        || attr.contains("all(test")
        || attr.contains("any(test")
        || attr.contains(", test")
        || attr.contains("(test,");
    let is_non_windows = attr.contains("unix")
        || attr.contains("not(windows)")
        || attr.contains("not(target_os = \"windows\")")
        || (attr.contains("target_os = ") && !attr.contains("\"windows\""));
    is_test || is_non_windows
}

/// Replace every item carrying a matching attribute with spaces, preserving line
/// numbering. An item ends at the close of its first brace-delimited block, or at the
/// first `;` if it has none (`#[cfg(windows)] const X: u32 = 1;`). Braces inside strings
/// and comments are ignored, which naive counting gets wrong: this tree has both `"{"`
/// and `format!("{{{{.ID}}}}")` inside test modules.
///
/// Everything below indexes BYTES, never `&str` slices: the tree contains multi-byte
/// characters and `source[i..]` panics when `i` lands inside one.
fn blank_out_items(source: &str, attr_matches: fn(&str) -> bool) -> String {
    let bytes = source.as_bytes();
    let mut keep = vec![true; bytes.len()];
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"#[") {
            let Some(attr_end) = matching_bracket(bytes, i + 1) else {
                break;
            };
            let attr = String::from_utf8_lossy(&bytes[i..=attr_end]);
            if attr_matches(&attr) {
                let item_end = item_end_from(bytes, attr_end + 1).unwrap_or(bytes.len() - 1);
                for k in keep.iter_mut().take(item_end + 1).skip(i) {
                    *k = false;
                }
                i = item_end + 1;
                continue;
            }
            i = attr_end + 1;
            continue;
        }
        i = advance(bytes, i);
    }

    source
        .char_indices()
        .map(|(idx, ch)| {
            if ch == '\n' || keep.get(idx).copied().unwrap_or(true) {
                ch
            } else {
                ' '
            }
        })
        .collect()
}

/// End of the item starting at `from`: the `}` closing its first block, or its first
/// top-level `;`.
fn item_end_from(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => return matching(bytes, i, b'{', b'}'),
            b';' => return Some(i),
            // A stacked attribute (`#[cfg(test)]` then `#[allow(...)]`). Skip it.
            b'#' if bytes[i..].starts_with(b"#[") => {
                i = matching_bracket(bytes, i + 1)? + 1;
                continue;
            }
            _ => {}
        }
        i = advance(bytes, i);
    }
    None
}

fn matching_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    matching(bytes, open, b'[', b']')
}

/// Brace/bracket matcher that skips strings, char literals, raw strings and comments.
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
/// literal that starts at `i`. Always advances by at least one byte.
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
    // lifetime (`&'a str`, `'static`), which is ordinary code: treating a lifetime as a
    // literal would make the scanner swallow everything up to the next `'` in the file,
    // real spawn sites included.
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
