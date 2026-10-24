use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::source_text::{normalized, scrub, Literals};

// ---------------------------------------------------------------------------
// Resolving what the compiler compiles
// ---------------------------------------------------------------------------

pub(crate) fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub(crate) fn relative_of(path: &Path) -> String {
    path.strip_prefix(manifest_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Read a file as text and remove its comments, keeping or dropping literals.
///
/// Bytes that are not valid UTF-8 are replaced rather than refused, so no file is
/// ever skipped for its encoding. A file that cannot be delimited afterwards is
/// still a hard failure.
pub(crate) fn scrubbed(path: &Path, literals: Literals) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", relative_of(path)))?;
    let text = String::from_utf8_lossy(&bytes);
    scrub(&text, literals).map_err(|reason| format!("{}: {reason}", relative_of(path)))
}

/// The directory rustc searches for the children of the module whose own file is
/// `file`: the file's own directory for a crate root or a `mod.rs`, and a
/// directory named after the file otherwise.
fn child_directory(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(Path::new("."));
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if stem == "lib" || stem == "main" || stem == "mod" {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    }
}

/// A file's two readings: `structure` has literal bodies removed and is what
/// braces and declarations are counted in; `with_literals` keeps them and is
/// what a `path = "..."` value is read out of.
struct ModuleBody {
    structure: String,
    with_literals: String,
}

/// Both readings of `owner`, after refusing the file outright if it declares a
/// module inside an inline module block.
///
/// The refusal is the honest answer to a case this resolver gets wrong: rustc
/// resolves `mod x;` inside `mod y { ... }` against `y`'s own directory, and
/// this resolver reads it as a child of the file. It used to resolve anyway,
/// to a different file, whenever one happened to exist at the path it looks in,
/// and then reported green having read the wrong file. A scanner that cannot
/// tell which file it should be reading has to say so.
fn module_body(owner: &Path) -> Result<ModuleBody, String> {
    let structure = normalized(&scrubbed(owner, Literals::Drop)?);
    if let Some(identifier) = nested_module_declaration(&structure) {
        return Err(format!(
            "{} declares `mod {identifier};` inside an inline `mod ... {{ ... }}` block. \
             rustc resolves that against the inline module's own directory and this resolver \
             does not, so the file it would scan is not the file rustc compiles. Refusing the \
             file rather than reading the wrong one: move the declaration to the top level of \
             its file.",
            relative_of(owner)
        ));
    }
    Ok(ModuleBody {
        structure,
        with_literals: normalized(&scrubbed(owner, Literals::Keep)?),
    })
}

/// The identifier of the first `mod <ident>;` that sits inside an inline
/// `mod ... { ... }` block, if there is one.
///
/// `body` must be scrubbed with `Literals::Drop` and normalized, because a brace
/// inside a string literal is not a block and would throw the depth off.
fn nested_module_declaration(body: &str) -> Option<String> {
    let mut depth = 0usize;
    for (index, character) in body.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 || !body[index..].starts_with("mod ") {
            continue;
        }
        let disqualified = body[..index]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if disqualified {
            continue;
        }
        let after = index + "mod ".len();
        let identifier: String = body[after..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
            .collect();
        if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
            return Some(
                identifier
                    .strip_prefix("r#")
                    .unwrap_or(&identifier)
                    .to_string(),
            );
        }
    }
    None
}

/// Every byte offset at which `mod <segment>;` is declared in `body`, which must
/// be normalized. A preceding identifier character or quote disqualifies the hit,
/// so neither `submod x;` nor a `mod x;` sitting inside a string is read as one.
///
/// **All of them, not the first.** The standard per-platform module is two
/// declarations of one name under opposite `cfg`s, and reading only the first
/// means scanning the Unix file in a Windows build while reporting that the set
/// of files is the set rustc compiles.
fn find_declarations(body: &str, segment: &str) -> Vec<usize> {
    let needle = format!("mod {segment};");
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find(&needle) {
        let at = from + offset;
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            found.push(at);
        }
        from = at + needle.len();
    }
    found
}

/// The text of the item that ends at `declaration_at`, so its attributes can be
/// read without scanning back into the previous item.
fn attributes_before(body: &str, declaration_at: usize) -> &str {
    let start = body[..declaration_at]
        .rfind([';', '}', '{'])
        .map(|index| index + 1)
        .unwrap_or(0);
    &body[start..declaration_at]
}

/// Every file named by a `path = "..."` in the item's attributes, in order.
///
/// Both `#[path = "x.rs"]` and `#[cfg_attr(<cond>, path = "x.rs")]` are read.
/// Matching the bare key rather than the text `#[path` is what covers the second
/// form, which is otherwise invisible: the resolver would fall back to the
/// default candidates while rustc compiles the file the `cfg_attr` names.
fn path_attributes(attributes: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = attributes[from..].find("path") {
        let at = from + offset;
        let after = at + "path".len();
        let preceded_by_identifier = attributes[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = attributes[after..].trim_start();
        if !preceded_by_identifier {
            if let Some(rest) = rest.strip_prefix('=') {
                if let Some(rest) = rest.trim_start().strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        values.push(rest[..end].to_string());
                    }
                }
            }
        }
        from = after;
    }
    values
}

/// Every file rustc might compile for the child `segment` of the module whose
/// file is `owner`.
///
/// `owner_body` must be `ModuleBody::with_literals`, because the `path` value is
/// a literal.
///
/// Two rules earn their keep here and both are refusals rather than guesses:
///
/// - **A `path` value is resolved beside the owner file first.** For a `mod`
///   declaration at the top level of a file, rustc reads `path` relative to the
///   directory the file is in, not relative to the module's own subdirectory.
///   Trying the subdirectory first picks the file rustc does not compile
///   whenever both exist, which is the whole of a benign-decoy attack. Since
///   `module_body` refuses declarations nested in inline blocks, the case where
///   rustc would use the module directory cannot reach this function.
/// - **Two existing candidates for one declaration is a hard failure.** rustc
///   itself rejects `x.rs` and `x/mod.rs` both existing; for a `path` value, two
///   candidates is exactly the situation where this scan cannot know which file
///   it is meant to read, and the house rule is that it must then say so.
fn resolve_children(owner: &Path, owner_body: &str, segment: &str) -> Result<Vec<PathBuf>, String> {
    let declarations = find_declarations(owner_body, segment);
    if declarations.is_empty() {
        return Err(format!(
            "{} declares no `mod {segment};`",
            relative_of(owner)
        ));
    }

    let module_directory = child_directory(owner);
    let file_directory = owner.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut resolved = Vec::new();

    for at in declarations {
        let values = path_attributes(attributes_before(owner_body, at));
        if values.is_empty() {
            let named = module_directory.join(format!("{segment}.rs"));
            let directory = module_directory.join(segment).join("mod.rs");
            match (named.is_file(), directory.is_file()) {
                (true, true) => {
                    return Err(format!(
                        "{} declares `mod {segment};` and both {} and {} exist. rustc rejects \
                         that outright and this scan will not pick one.",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
                (true, false) => resolved.push(named),
                (false, true) => resolved.push(directory),
                (false, false) => {
                    return Err(format!(
                        "{} declares `mod {segment};` but none of these files exists: {}, {}",
                        relative_of(owner),
                        relative_of(&named),
                        relative_of(&directory)
                    ))
                }
            }
            continue;
        }

        for value in values {
            let beside_the_file = file_directory.join(&value);
            let inside_the_module = module_directory.join(&value);
            let mut hits: Vec<PathBuf> = Vec::new();
            for candidate in [beside_the_file, inside_the_module] {
                if candidate.is_file() && !hits.contains(&candidate) {
                    hits.push(candidate);
                }
            }
            match hits.len() {
                1 => resolved.push(hits.remove(0)),
                0 => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and no file \
                         exists at {} or at {}",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
                _ => {
                    return Err(format!(
                        "{} declares `mod {segment};` with `path = \"{value}\"` and both {} and \
                         {} exist. rustc compiles the one beside the file; this scan will not \
                         guess, because guessing wrong is how a forbidden reference stays \
                         unread. Remove one of them.",
                        relative_of(owner),
                        relative_of(&file_directory.join(&value)),
                        relative_of(&module_directory.join(&value))
                    ))
                }
            }
        }
    }

    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

/// Every `mod <ident>;` declared in `body`, which must be `ModuleBody::structure`,
/// deduplicated. `resolve_children` finds every declaration of each name, so one
/// entry per name is enough here.
fn declared_children(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = body[from..].find("mod ") {
        let at = from + offset;
        let after = at + "mod ".len();
        let disqualified = body[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '"');
        if !disqualified {
            let identifier: String = body[after..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '#')
                .collect();
            if !identifier.is_empty() && body[after + identifier.len()..].starts_with(';') {
                found.push(
                    identifier
                        .strip_prefix("r#")
                        .unwrap_or(&identifier)
                        .to_string(),
                );
            }
        }
        from = after;
    }
    found.sort();
    found.dedup();
    found
}

/// The files rustc compiles for `module` and every module below it, resolved by
/// walking `mod` declarations down from the crate root.
///
/// The walk carries a frontier rather than a single file, because a segment can
/// be declared more than once under opposite `cfg`s and this resolver keeps both
/// arms. An error at any step is propagated rather than skipped: a module that
/// cannot be located is the one case where reading nothing must not look like
/// reading nothing forbidden.
pub(crate) fn sources_of(module: &[&str]) -> Result<Vec<PathBuf>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut frontier = vec![root];
    for segment in module {
        let mut next = Vec::new();
        for owner in &frontier {
            next.extend(resolve_children(
                owner,
                &module_body(owner)?.with_literals,
                segment,
            )?);
        }
        next.sort();
        next.dedup();
        frontier = next;
    }

    let mut files = Vec::new();
    let mut queue = frontier;
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let body = module_body(&current)?;
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Every file rustc compiles for the whole crate, as repository-relative paths.
///
/// Used for one question only, and computed lazily because it is the expensive
/// and failure-prone walk: when a file under `src/` cannot be delimited, is it a
/// file the compiler reads? A `.md` that is not in the module tree cannot hold a
/// definition of anything and must not turn a layering guard red. A `.rs` file
/// the compiler does read and this scan could not is the opposite, and must.
///
/// A file whose own body cannot be read is **recorded as reached and not
/// descended into**, which is the only sensible answer: the question being asked
/// is exactly "is this unreadable file in the tree", so failing the walk because
/// of it would refuse to answer the question it was called to answer. Its
/// children are lost, and that is stated rather than hidden.
pub(crate) fn crate_sources() -> Result<BTreeSet<String>, String> {
    let root = manifest_dir().join("src").join("lib.rs");
    if !root.is_file() {
        return Err(format!("{} does not exist", relative_of(&root)));
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let mut queue = vec![root];
    while let Some(current) = queue.pop() {
        if files.contains(&current) {
            continue;
        }
        let Ok(body) = module_body(&current) else {
            files.push(current);
            continue;
        };
        for child in declared_children(&body.structure) {
            queue.extend(resolve_children(&current, &body.with_literals, &child)?);
        }
        files.push(current);
    }
    Ok(files.iter().map(|path| relative_of(path)).collect())
}

/// Every file under `root`, sorted, filtered by nothing.
///
/// **Do not add an extension filter here.** `rustc` decides what to compile from
/// the module tree; a filter decides from the name, and production code lives in
/// the gap between the two. On a case-insensitive filesystem `mod x;` resolves
/// `x.RS` while `"RS" == "rs"` is false, and `#[path = "carrier.inc"]` compiles a
/// file no extension filter matches. Reading every file closes both and is still
/// a pure text scan.
pub(crate) fn every_file_under(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files.sort();
    files
}
