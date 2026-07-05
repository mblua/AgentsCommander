use std::path::Path;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_embedded_dist)");

    // Determine build profile: "dev", "prod", or "stage".
    // BUILD_PROFILE env var takes precedence; otherwise default based on cargo profile.
    let profile = std::env::var("BUILD_PROFILE").unwrap_or_else(|_| {
        let cargo_profile = std::env::var("PROFILE").unwrap_or_default();
        if cargo_profile == "release" {
            "prod"
        } else {
            "dev"
        }
        .to_string()
    });

    // Make BUILD_PROFILE available via env!("BUILD_PROFILE") in Rust code.
    println!("cargo:rustc-env=BUILD_PROFILE={}", profile);
    println!("cargo:rerun-if-env-changed=BUILD_PROFILE");

    let has_dist = configure_embedded_dist(&profile);
    if !has_dist {
        omit_missing_dist_resource_for_dev_check();
    }
    embed_windows_test_manifest();

    tauri_build::build()
}

fn configure_embedded_dist(profile: &str) -> bool {
    let dist_dir = Path::new("../dist");
    let index_html = dist_dir.join("index.html");

    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-changed={}", index_html.display());

    if index_html.is_file() {
        println!("cargo:rustc-cfg=has_embedded_dist");
        emit_dist_rerun_paths(dist_dir);
        return true;
    }

    if profile != "dev" {
        panic!(
            "missing ../dist/index.html for {} build; run npm run build before compiling src-tauri",
            profile
        );
    }

    println!(
        "cargo:warning=../dist/index.html missing; embedded web UI disabled for this dev/check build"
    );
    false
}

fn omit_missing_dist_resource_for_dev_check() {
    if std::env::var_os("TAURI_CONFIG").is_some() {
        println!(
            "cargo:warning=TAURI_CONFIG already set; leaving configured bundle resources unchanged"
        );
        return;
    }

    println!(
        "cargo:warning=overriding Tauri bundle resources for this dev/check build because ../dist is missing"
    );
    std::env::set_var("TAURI_CONFIG", r#"{"bundle":{"resources":[]}}"#);
}

fn emit_dist_rerun_paths(path: &Path) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            emit_dist_rerun_paths(&path);
        }
    }
}

fn embed_windows_test_manifest() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    println!(
        "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
}
