//! Credential block builder.
//!
//! Produces the `# === Session Credentials ===` block injected into agent
//! sessions at spawn and after `/clear`. Output must stay byte-for-byte
//! identical across both call sites so agents parse consistently.

use uuid::Uuid;

/// Build the credentials block for a session.
///
/// The block is terminated by `\n` (no trailing Enter) — the caller is
/// responsible for flagging `submit=true` to `inject_text_into_session`
/// which adds the Enter keystrokes for agents that need them.
///
/// `token` is `Display`'d lowercase with dashes (standard `Uuid` format).
/// `cwd` is the session's working directory, verbatim.
///
/// `Binary`, `BinaryPath`, and `LocalDir` are derived from the current
/// process executable — the running `agentscommander*.exe`. This matches
/// the original inline behavior in `commands/session.rs` and is what
/// agents use to invoke back into the CLI.
///
/// No I/O except `std::env::current_exe()` and a single `log::warn!`
/// when `current_exe()` returns `Err` (per plan §17.6 for operator
/// observability on the rare "agent cannot find its binary" path).
pub fn build_credentials_block(token: &Uuid, cwd: &str) -> String {
    let exe = std::env::current_exe().ok();
    if exe.is_none() {
        log::warn!(
            "[credentials] current_exe() unavailable — cred block will use fallback \
             binary name. Agent may be unable to invoke the CLI."
        );
    }
    let binary_name = exe
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());
    let binary_path = {
        let raw = exe
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "agentscommander.exe".to_string());
        raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
    };
    let local_dir = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|parent| {
            parent
                .join(format!(".{}", &binary_name))
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| format!(".{}", &binary_name));
    let local_dir = local_dir
        .strip_prefix(r"\\?\")
        .unwrap_or(&local_dir)
        .to_string();

    format!(
        concat!(
            "\n",
            "# === Session Credentials ===\n",
            "# Token: {token}\n",
            "# Root: {root}\n",
            "# Binary: {binary}\n",
            "# BinaryPath: {binary_path}\n",
            "# LocalDir: {local_dir}\n",
            "# === End Credentials ===\n",
        ),
        token = token,
        root = cwd,
        binary = binary_name,
        binary_path = binary_path,
        local_dir = local_dir,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_structure_is_byte_stable() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let block = build_credentials_block(&token, r"C:\example\root");

        // Split on '\n' (keep empty trailing element → final "" after last \n).
        let lines: Vec<&str> = block.split('\n').collect();

        // Expected structure, in order (9 elements: 8 \n-terminated lines + trailing empty):
        //  0: ""                                   (leading \n)
        //  1: "# === Session Credentials ==="
        //  2: "# Token: 00000000-0000-0000-0000-000000000001"
        //  3: "# Root: C:\example\root"
        //  4: "# Binary: <runtime-derived>"
        //  5: "# BinaryPath: <runtime-derived>"
        //  6: "# LocalDir: <runtime-derived>"
        //  7: "# === End Credentials ==="
        //  8: ""                                   (trailing \n)
        assert_eq!(lines.len(), 9, "line count drift: {}", lines.len());
        assert_eq!(lines[0], "", "missing leading newline");
        assert_eq!(lines[1], "# === Session Credentials ===");
        assert_eq!(lines[2], "# Token: 00000000-0000-0000-0000-000000000001");
        assert_eq!(lines[3], r"# Root: C:\example\root");
        assert!(lines[4].starts_with("# Binary: "), "Binary line prefix");
        assert!(lines[5].starts_with("# BinaryPath: "), "BinaryPath prefix");
        assert!(lines[6].starts_with("# LocalDir: "), "LocalDir prefix");
        assert_eq!(lines[7], "# === End Credentials ===");
        assert_eq!(lines[8], "", "missing trailing newline");
    }
}
