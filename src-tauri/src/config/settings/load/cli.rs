use super::{
    adopt_orphaned_profiles, apply_issue_248_migration, capture_project_settings, default_zoom,
    refresh_pair_id_hashes, repair_coding_agent_profiles_config, settings_path,
    unavailable_project_settings, AppSettings, Path, OVERLAY_KEY_MAIN_ALWAYS_ON_TOP,
    OVERLAY_KEY_MAIN_GEOMETRY, OVERLAY_KEY_MAIN_ZOOM,
};

/// CLI-only variant of `load_settings`. Reads disk and applies the same
/// in-memory migrations as `load_settings`, but does NOT auto-generate or
/// persist a `root_token`. Used by CLI verbs that mutate settings
/// (`open-project`, `new-project`) so error paths and pre-validation reads
/// do NOT silently rewrite `settings.json` (Round-1 G5 in #191's plan).
///
/// The CLI verbs do not consume the root_token; if a future verb needs it,
/// `settings.root_token == None` on a brand-new install is fine — the CLI
/// is read-only with respect to it. The GUI still owns root_token
/// generation via the next `load_settings()` call when it boots.
///
/// **Migration duplication is intentional for this PR.** Extracting an
/// `apply_in_memory_migrations(&mut AppSettings)` helper that both loaders
/// share is a clean follow-up, but pulls in scope outside #191 (touches
/// `load_settings`'s control flow). Keep both copies in lockstep until
/// then; if you add a new in-memory migration to `load_settings`, mirror
/// it here too.
pub fn load_settings_for_cli() -> AppSettings {
    match settings_path() {
        Some(path) => load_settings_for_cli_from_path(&path),
        None => {
            log::warn!("[cli] Could not determine home directory, using defaults");
            AppSettings::default()
        }
    }
}

/// #2716 (B3) - `load_settings_for_cli` at an explicit path, so a test can drive
/// the CLI loader over a tempdir.
pub(crate) fn load_settings_for_cli_from_path(path: &Path) -> AppSettings {
    let mut settings = match capture_project_settings(path, false) {
        Ok((settings, _, _)) => settings,
        Err(error) => unavailable_project_settings(AppSettings::default(), error),
    };

    // 0.8.0 unified-window migration — must mirror `load_settings` exactly,
    // EXCEPT for the root_token auto-gen + save_settings call.
    // #1737 (D7c): the same three destination-ownership suppressions as the GUI loader.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }

    // #248 migration — translate in-memory only. The CLI loader is forbidden
    // from writing settings.json per the §463 contract (load_settings_for_cli
    // is the read-only variant used by mutating verbs like `open-project` and
    // `new-project`; it must not race with the GUI's settings writes). The
    // next GUI launch finalizes the migration to disk via load_settings.
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    // NO root_token auto-gen, NO save_settings call.
    settings
}

/// #786 R1: strict CLI loader for MUTATING paths and `list`/`show`. Identical to
/// `load_settings_for_cli` EXCEPT it distinguishes an ABSENT settings.json (fine:
/// start from default) from a PRESENT-but-unparseable one (Err: refuse to touch
/// it). This closes the silent-wipe hole: `load_settings_for_cli` returns a
/// default on a strict parse error, and a subsequent `save_settings` (which
/// preserves only `project_paths`, via a LENIENT `Value` read) would rewrite
/// settings.json to defaults, destroying every agent/profile/hotkey. NEVER call
/// the silent-default `load_settings_for_cli` for a write.
///
/// Keep the in-memory migration tail in lockstep with `load_settings_for_cli`
/// (see the note above that function).
pub fn load_settings_for_cli_strict() -> Result<AppSettings, String> {
    load_settings_for_cli_strict_from_path(settings_path().as_deref())
}

/// #2716 (B3) - `load_settings_for_cli_strict` at an explicit path, so a test can
/// drive the strict CLI loader over a tempdir.
pub(crate) fn load_settings_for_cli_strict_from_path(
    path: Option<&Path>,
) -> Result<AppSettings, String> {
    let mut settings = match path {
        None => AppSettings::default(),
        Some(path) => {
            let (settings, _, _) = capture_project_settings(path, false).map_err(|error| {
                format!(
                    "s\
                ettings source could not be parsed or selected ({error}); refusing to modify it"
                )
            })?;
            if let Some(error) = &settings.project_paths_persistence_error {
                return Err(format!(
                    "settings source could not be parsed or selected ({error}); \
                    refusing to modify it"
                ));
            }
            settings
        }
    };

    // Mirror `load_settings_for_cli`'s in-memory migrations (no disk write).
    // #1737 (D7c): the same three destination-ownership suppressions.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    Ok(settings)
}
