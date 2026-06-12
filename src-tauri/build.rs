fn main() {
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

    // Make BUILD_PROFILE available via env!("BUILD_PROFILE") in Rust code
    println!("cargo:rustc-env=BUILD_PROFILE={}", profile);
    println!("cargo:rerun-if-env-changed=BUILD_PROFILE");

    embed_windows_test_manifest();

    tauri_build::build()
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
