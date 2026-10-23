use std::path::{Path, PathBuf};

/// Read a crate-relative module (`foo.rs` plus every file under `foo/`), drop
/// its tests (an inline `#[cfg(test)] mod tests` block, or the out-of-line
/// `tests.rs` / `tests/` files), and collapse ALL whitespace. Dropping the tests
/// matches the cited precedent and makes the "production only" property
/// ENFORCED rather than assumed: a future `#[cfg(test)]` use of `production()`
/// can no longer offset a removed production wiring. Collapsing whitespace makes
/// assertions match token sequences irrespective of rustfmt line-wrapping.
pub(crate) fn normalized_production_source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let normalized = normalized_file(&path);
    let production = match normalized.find("#[cfg(test)]modtests{") {
        Some(end) => normalized[..end].to_string(),
        None if normalized.contains("#[cfg(test)]modtests;") => normalized,
        None => panic!("{relative}: no `#[cfg(test)] mod tests` found to split off"),
    };
    let mut children = Vec::new();
    collect_production_children(&path.with_extension(""), &mut children);
    children.sort();
    children
        .iter()
        .fold(production, |acc, child| acc + &normalized_file(child))
}

fn normalized_file(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .split_whitespace()
        .collect()
}

/// Every `.rs` file under a module directory, except its `tests.rs` and `tests/`.
fn collect_production_children(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("read module dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "tests" {
                collect_production_children(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(path);
        }
    }
}

/// Count production activation-token constructions in a scraped production
/// slice. The token string carries no interior whitespace, so it survives
/// normalization.
pub(crate) fn production_token_count(production: &str) -> usize {
    production
        .matches("ManifestActivationToken::production()")
        .count()
}
