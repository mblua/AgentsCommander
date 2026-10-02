//! #2817 coupling gate: the Rust `tauri` crate and the npm `@tauri-apps/api`
//! package must share a `major.minor`.
//!
//! The Tauri CLI compares the two and aborts the build when they differ, so a
//! bump of one side alone turns the production build red only after a full
//! Windows build. This test reads both lockfiles and fails first.

use std::path::{Path, PathBuf};

const NPM_API_KEY: &str = "node_modules/@tauri-apps/api";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("#2817: the src-tauri package directory has a parent")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("#2817: cannot read {}: {err}", path.display()))
}

/// The version of the single `tauri` package in `Cargo.lock`.
fn cargo_tauri_version(path: &Path) -> String {
    let lock: toml::Table = toml::from_str(&read(path))
        .unwrap_or_else(|err| panic!("#2817: cannot parse {}: {err}", path.display()));
    let versions: Vec<&str> = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|package| package.get("name").and_then(toml::Value::as_str) == Some("tauri"))
        .filter_map(|package| package.get("version").and_then(toml::Value::as_str))
        .collect();
    assert_eq!(
        versions.len(),
        1,
        "#2817: expected exactly one `tauri` package in {}, found {versions:?}",
        path.display()
    );
    versions[0].to_string()
}

/// The version of `@tauri-apps/api` in `package-lock.json`.
fn npm_api_version(path: &Path) -> String {
    let lock: serde_json::Value = serde_json::from_str(&read(path))
        .unwrap_or_else(|err| panic!("#2817: cannot parse {}: {err}", path.display()));
    lock["packages"][NPM_API_KEY]["version"]
        .as_str()
        .unwrap_or_else(|| panic!("#2817: {} has no version for {NPM_API_KEY}", path.display()))
        .to_string()
}

/// `(major, minor)` of `version`, ignoring any prerelease or build tag.
fn major_minor(version: &str, source: &Path) -> (u64, u64) {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut fields = core.split('.').map(|field| field.parse::<u64>().ok());
    match (fields.next().flatten(), fields.next().flatten()) {
        (Some(major), Some(minor)) => (major, minor),
        _ => panic!(
            "#2817: `{version}` from {} has no numeric major.minor",
            source.display()
        ),
    }
}

#[test]
fn tauri_crate_and_npm_api_share_a_major_minor() {
    let root = repo_root();
    let cargo_lock = root.join("Cargo.lock");
    let package_lock = root.join("package-lock.json");
    let crate_version = cargo_tauri_version(&cargo_lock);
    let npm_version = npm_api_version(&package_lock);
    assert_eq!(
        major_minor(&crate_version, &cargo_lock),
        major_minor(&npm_version, &package_lock),
        "#2817: `tauri` {crate_version} in {} and `@tauri-apps/api` {npm_version} in {} differ \
         in major.minor. The Tauri CLI aborts `npm run build:prod` and \
         `npm run build:prod:no-bundle` with \"Found version mismatched Tauri packages\".",
        cargo_lock.display(),
        package_lock.display()
    );
}
