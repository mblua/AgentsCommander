use super::{
    adopt_orphaned_profiles, normalize_agent_backend_configs, refresh_agent_id_hashes,
    refresh_pair_id_hashes, repair_coding_agent_profiles_config, validate_agent_commands,
    validate_preflight_timeouts, AppSettings, IpAddr, Path, SocketAddr,
    RESPONSE_CLOSE_IDLE_SECONDS_MAX, RESPONSE_CLOSE_IDLE_SECONDS_MIN,
    ROOM_IDLE_CONFIRMATION_SECONDS_MAX, ROOM_IDLE_CONFIRMATION_SECONDS_MIN,
    TYPING_HOLD_SECONDS_MAX, TYPING_HOLD_SECONDS_MIN,
};

pub fn validate_and_repair_settings(settings: &mut AppSettings) -> Result<(), String> {
    if !(RESPONSE_CLOSE_IDLE_SECONDS_MIN..=RESPONSE_CLOSE_IDLE_SECONDS_MAX)
        .contains(&settings.response_close_idle_seconds)
    {
        return Err("responseCloseIdleSeconds must be between 1 and 3600 seconds".to_string());
    }
    if !(ROOM_IDLE_CONFIRMATION_SECONDS_MIN..=ROOM_IDLE_CONFIRMATION_SECONDS_MAX)
        .contains(&settings.room_idle_confirmation_seconds)
    {
        return Err("roomIdleConfirmationSeconds must be between 1 and 3600 seconds".to_string());
    }
    validate_preflight_timeouts(settings)?;
    adopt_orphaned_profiles(settings);
    normalize_agent_backend_configs(settings)?;
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    validate_agent_commands(settings)?;
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    validate_screenshot_hotkey(&settings.screenshot_capture_hotkey)?;
    validate_sidebar_compact_hotkey(&settings.sidebar_compact_hotkey)?;
    validate_room_number_mask(&settings.room_number_mask)?;
    validate_api_server_settings(settings)?;
    validate_typing_hold_settings(settings)?;
    validate_resource_settings(settings)
}

pub const ROOM_NUMBER_MASK_MAX_WIDTH: usize = 9;

/// #2507 Returns the pad width. `Err` blocks the settings save.
pub fn validate_room_number_mask(value: &str) -> Result<usize, String> {
    let reject = |reason: &str| Err(format!("Room number mask: {reason}"));
    if value.is_empty() {
        return reject("must be at least one '#'");
    }
    if !value.chars().all(|c| c == '#') {
        return reject("must contain only '#' characters, e.g. ## or ###");
    }
    if value.len() > ROOM_NUMBER_MASK_MAX_WIDTH {
        return reject(&format!(
            "must be at most {ROOM_NUMBER_MASK_MAX_WIDTH} '#' characters"
        ));
    }
    Ok(value.len())
}

/// #2336 - reject a settings update whose typing-hold window is outside the
/// accepted range. Called on every save path, so the invalid candidate is
/// discarded before `save_settings` and the persisted (and live) value stays
/// untouched.
pub fn validate_typing_hold_settings(settings: &AppSettings) -> Result<(), String> {
    let seconds = settings.typing_hold_seconds;
    if !(TYPING_HOLD_SECONDS_MIN..=TYPING_HOLD_SECONDS_MAX).contains(&seconds) {
        return Err(format!(
            "typingHoldSeconds must be between {} and {} (got {})",
            TYPING_HOLD_SECONDS_MIN, TYPING_HOLD_SECONDS_MAX, seconds
        ));
    }
    Ok(())
}

pub(crate) fn parse_api_server_socket_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Err("apiServerBind must not be empty".to_string());
    }
    if port == 0 {
        return Err("apiServerPort must be between 1 and 65535".to_string());
    }
    let ip: IpAddr = bind.parse().map_err(|e| {
        format!("apiServerBind must be an IP address such as 127.0.0.1, 0.0.0.0, ::1, or :: ({e})")
    })?;
    Ok(SocketAddr::new(ip, port))
}

pub fn validate_api_server_settings(settings: &mut AppSettings) -> Result<(), String> {
    let bind = settings.api_server_bind.trim().to_string();
    parse_api_server_socket_addr(&bind, settings.api_server_port)?;
    settings.api_server_bind = bind;
    Ok(())
}

/// #714 Reject a screenshot hotkey that the native parser cannot accept. Syntax
/// errors block a settings save; OS-level registration conflicts do not (they are
/// surfaced as runtime status by `screenshot::register_configured_hotkey`).
pub fn validate_screenshot_hotkey(value: &str) -> Result<(), String> {
    crate::screenshot::parse_screenshot_hotkey(value)
        .map(|_| ())
        .map_err(|e| format!("Screenshot hotkey: {}", e))
}

/// #2281 Reject a sidebar compact hotkey that the settings contract cannot
/// accept. Unlike `validate_screenshot_hotkey` this is not an OS registration:
/// it only enforces the shape `Ctrl|Control + Shift + <A-Z>` minus the reserved
/// letters `W`, `R`, `C`, `V` (case-insensitive parts). Digits are rejected:
/// `Ctrl+Shift+2` reaches xterm as `key="@"`, which Keyboard.ts maps to NUL and
/// writes to the PTY. Syntax errors block a settings save; the value is judged
/// byte-identical and never repaired, so a lower-case spelling is stored as-is.
pub fn validate_sidebar_compact_hotkey(value: &str) -> Result<(), String> {
    let reject = |reason: &str| Err(format!("Sidebar compact hotkey: {reason}"));
    let parts: Vec<&str> = value.trim().split('+').map(|p| p.trim()).collect();
    if parts.len() != 3 {
        return reject("expected exactly three '+' separated parts, e.g. Ctrl+Shift+E");
    }
    if !parts[0].eq_ignore_ascii_case("ctrl") && !parts[0].eq_ignore_ascii_case("control") {
        return reject("first part must be Ctrl or Control");
    }
    if !parts[1].eq_ignore_ascii_case("shift") {
        return reject("second part must be Shift");
    }
    let mut chars = parts[2].chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) => c,
        _ => return reject("third part must be exactly one character"),
    };
    if !key.is_ascii_alphabetic() {
        return reject("third part must be an ASCII letter");
    }
    if matches!(key.to_ascii_lowercase(), 'w' | 'r' | 'c' | 'v') {
        return reject("third part must not be one of the reserved letters W, R, C, V");
    }
    Ok(())
}

pub fn merge_protected_coding_agent_settings(
    current: &AppSettings,
    mut incoming: AppSettings,
) -> AppSettings {
    incoming.coding_agent_profiles = current.coding_agent_profiles.clone();

    for incoming_agent in &mut incoming.agents {
        if let Some(current_agent) = current.agents.iter().find(|a| a.id == incoming_agent.id) {
            incoming_agent.envs = current_agent.envs.clone();
            incoming_agent.isolated_home = current_agent.isolated_home;
        }
    }

    incoming
}

/// #2162 - the OS suffix tokens a config-seed template folder may carry.
pub const CONFIG_SEED_OS_TOKENS: [&str; 3] = ["linux", "windows", "macos"];

/// #2162 - `Some(stem)` when `dest` ends with an OS token, where `stem` is the
/// dest it collides with. `stem` is empty when `dest` is exactly `".<token>"`.
pub fn config_seed_dest_os_token_stem(dest: &str) -> Option<String> {
    let lower = dest.trim().to_ascii_lowercase();
    CONFIG_SEED_OS_TOKENS
        .iter()
        .find(|t| lower.ends_with(&format!(".{t}")))
        .map(|t| lower[..lower.len() - t.len() - 1].to_string())
}

/// A config-seed `dest` must be a single, relative folder NAME under the replica
/// root. The convention prefixes (`default`, `default_profile_<letter>`) are
/// concatenated onto this name for the workspace-root tiers, so it must be a
/// single path segment and a legal Windows directory name.
pub fn validate_config_seed_dest(dest: &str) -> Result<(), String> {
    let d = dest.trim();
    if d.is_empty() {
        return Err("config seed destination must not be empty".to_string());
    }
    if d.contains('/') || d.contains('\\') {
        return Err(
            "config seed destination must be a single folder name (no path separators)".to_string(),
        );
    }
    if d == "." || d == ".." || d.contains("..") {
        return Err("config seed destination must not contain '..'".to_string());
    }
    if Path::new(d).is_absolute() {
        return Err("config seed destination must be relative".to_string());
    }
    // M5: ':' makes "C:foo" drive-relative (is_absolute()==false) or "x:y" an ADS.
    if d.contains(':') {
        return Err("config seed destination must not contain ':'".to_string());
    }
    if d.contains('%') || d.contains('$') {
        return Err("config seed destination must not contain placeholder markers".to_string());
    }
    // M5: reject a trailing dot (Windows strips it, causing target drift). A
    // trailing space cannot reach here: `d` was trimmed above.
    if d.ends_with('.') {
        return Err("config seed destination must not end with a dot".to_string());
    }
    // M5: reject Windows reserved device names (case-insensitive, before any extension).
    let stem = d.split('.').next().unwrap_or(d).to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!(
            "config seed destination '{}' is a reserved Windows device name",
            d
        ));
    }
    Ok(())
}

pub fn validate_resource_settings(settings: &AppSettings) -> Result<(), String> {
    // Floor at 1 (a value of 0 would block every agent launch); no upper ceiling.
    // The user sets this deliberately and is responsible for sizing it to their machine.
    if settings.max_concurrent_agent_processes == 0 {
        return Err("maxConcurrentAgentProcesses must be at least 1".to_string());
    }
    if settings.agent_group_warn_private_bytes > settings.agent_group_kill_private_bytes {
        return Err(
            "agentGroupWarnPrivateBytes must be less than or equal to agentGroupKillPrivateBytes"
                .to_string(),
        );
    }
    if settings.agent_group_kill_private_bytes == 0 {
        return Err("agentGroupKillPrivateBytes must be greater than 0".to_string());
    }
    if settings.agent_process_kill_private_bytes == 0 {
        return Err("agentProcessKillPrivateBytes must be greater than 0".to_string());
    }
    Ok(())
}
