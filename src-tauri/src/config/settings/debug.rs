//! #4638 - manual `Debug` for `AppSettings`: secrets print as `"[REDACTED]"`,
//! every other field prints as the derived `Debug` did.

use super::AppSettings;
use std::fmt;

const REDACTED: &str = "[REDACTED]";

impl fmt::Debug for AppSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        all_fields_listed(self);
        let mut d = f.debug_struct("AppSettings");
        fields_1(self, &mut d);
        fields_2(self, &mut d);
        fields_3(self, &mut d);
        d.finish()
    }
}

#[rustfmt::skip]
fn fields_1(s: &AppSettings, d: &mut fmt::DebugStruct<'_, '_>) {
    d.field("default_shell", &s.default_shell);
    d.field("default_shell_args", &s.default_shell_args);
    d.field("agents", &s.agents);
    d.field("coding_agent_profiles", &s.coding_agent_profiles);
    d.field("agent_file_source_by_menu", &s.agent_file_source_by_menu);
    d.field("telegram_bots", &s.telegram_bots);
    d.field("telegram_network_poll_error_logging", &s.telegram_network_poll_error_logging);
    d.field("restore_coordinator_wake_state", &s.restore_coordinator_wake_state);
    d.field("legacy_start_only_coordinators", &s.legacy_start_only_coordinators);
    d.field("restart_resume_orchestrator_prompt", &s.restart_resume_orchestrator_prompt);
    d.field("restart_resume_agent_prompt", &s.restart_resume_agent_prompt);
    d.field("restart_resume_wake_working_agents", &s.restart_resume_wake_working_agents);
    d.field("sidebar_always_on_top", &s.sidebar_always_on_top);
    d.field("team_idle_beep_enabled", &s.team_idle_beep_enabled);
    d.field("sounds_enabled", &s.sounds_enabled);
    d.field("raise_terminal_on_click", &s.raise_terminal_on_click);
    d.field("screenshot_capture_hotkey", &s.screenshot_capture_hotkey);
    d.field("screenshot_hyprland_bind", &s.screenshot_hyprland_bind);
    d.field("sidebar_compact_hotkey", &s.sidebar_compact_hotkey);
    d.field("room_number_mask", &s.room_number_mask);
    d.field("voice_to_text_enabled", &s.voice_to_text_enabled);
    d.field("gemini_api_key", &REDACTED);
    d.field("gemini_model", &s.gemini_model);
    d.field("jev_api_key", &REDACTED);
    d.field("jev_model", &s.jev_model);
    d.field("jev_endpoint", &s.jev_endpoint);
    d.field("jev_timeout_secs", &s.jev_timeout_secs);
    d.field("jev_threshold", &s.jev_threshold);
    d.field("jev_margin", &s.jev_margin);
    d.field("co_managed_enabled", &s.co_managed_enabled);
    d.field("coding_agent_install_enabled", &s.coding_agent_install_enabled);
    d.field("container_options_enabled", &s.container_options_enabled);
    d.field("voice_auto_execute", &s.voice_auto_execute);
    d.field("voice_auto_execute_delay", &s.voice_auto_execute_delay);
    d.field("sidebar_zoom", &s.sidebar_zoom);
    d.field("terminal_zoom", &s.terminal_zoom);
    d.field("main_zoom", &s.main_zoom);
    d.field("guide_zoom", &s.guide_zoom);
    d.field("darkfactory_zoom", &s.darkfactory_zoom);
    d.field("sidebar_geometry", &s.sidebar_geometry);
}

#[rustfmt::skip]
fn fields_2(s: &AppSettings, d: &mut fmt::DebugStruct<'_, '_>) {
    d.field("terminal_geometry", &s.terminal_geometry);
    d.field("main_geometry", &s.main_geometry);
    d.field("main_window_display_state", &s.main_window_display_state);
    d.field("main_sidebar_width", &s.main_sidebar_width);
    d.field("main_sidebar_side", &s.main_sidebar_side);
    d.field("main_always_on_top", &s.main_always_on_top);
    d.field("main_resource_monitor_attached", &s.main_resource_monitor_attached);
    d.field("web_server_enabled", &s.web_server_enabled);
    d.field("web_server_port", &s.web_server_port);
    d.field("web_server_bind", &s.web_server_bind);
    d.field("api_server_enabled", &s.api_server_enabled);
    d.field("api_server_port", &s.api_server_port);
    d.field("api_server_bind", &s.api_server_bind);
    d.field("terminal_snapshots_enabled", &s.terminal_snapshots_enabled);
    d.field("project_path", &s.project_path);
    d.field("project_paths", &s.project_paths);
    d.field("archived_project_paths", &s.archived_project_paths);
    d.field("project_path_state", &s.project_path_state);
    d.field("project_paths_persistence_error", &s.project_paths_persistence_error);
    d.field("local_overlay_state", &s.local_overlay_state);
    d.field("agents_layer", &s.agents_layer);
    d.field("coding_agent_identity_physical", &s.coding_agent_identity_physical);
    d.field("sidebar_style", &s.sidebar_style);
    d.field("selected_row_rail_width", &s.selected_row_rail_width);
    d.field("selected_row_rail_color", &s.selected_row_rail_color);
    d.field("root_token", &REDACTED);
    d.field("onboarding_dismissed", &s.onboarding_dismissed);
    d.field("coord_sort_by_activity", &s.coord_sort_by_activity);
    d.field("always_show_selected_workgroup", &s.always_show_selected_workgroup);
    d.field("log_level", &s.log_level);
    d.field("activity_log_enabled", &s.activity_log_enabled);
    d.field("auto_generate_task_title", &s.auto_generate_task_title);
    d.field("agent_templates_path", &s.agent_templates_path);
    d.field("theme_light", &s.theme_light);
    d.field("rail_collapsed_projects", &s.rail_collapsed_projects);
    d.field("rail_favorites_collapsed", &s.rail_favorites_collapsed);
    d.field("spec_board_enabled", &s.spec_board_enabled);
    d.field("web_browser_enabled", &s.web_browser_enabled);
    d.field("git_sweep_concurrency", &s.git_sweep_concurrency);
    d.field("git_sweep_min_interval_secs", &s.git_sweep_min_interval_secs);
    d.field("ci_activity_enabled", &s.ci_activity_enabled);
}

#[rustfmt::skip]
fn fields_3(s: &AppSettings, d: &mut fmt::DebugStruct<'_, '_>) {
    d.field("ci_activity_notify_orchestrator", &s.ci_activity_notify_orchestrator);
    d.field("branch_staleness_enabled", &s.branch_staleness_enabled);
    d.field("branch_staleness_notify_orchestrator", &s.branch_staleness_notify_orchestrator);
    d.field("ci_sweep_min_interval_secs", &s.ci_sweep_min_interval_secs);
    d.field("branch_staleness_interval_secs", &s.branch_staleness_interval_secs);
    d.field("resource_monitor_enabled", &s.resource_monitor_enabled);
    d.field("max_concurrent_agent_processes", &s.max_concurrent_agent_processes);
    d.field("resource_watchdog_action", &s.resource_watchdog_action);
    d.field("agent_group_warn_private_bytes", &s.agent_group_warn_private_bytes);
    d.field("agent_group_kill_private_bytes", &s.agent_group_kill_private_bytes);
    d.field("agent_process_kill_private_bytes", &s.agent_process_kill_private_bytes);
    d.field("resource_keep_last_snapshot", &s.resource_keep_last_snapshot);
    d.field("resource_backoff_polling", &s.resource_backoff_polling);
    d.field("coordinator_idle_badge_yellow_minutes", &s.coordinator_idle_badge_yellow_minutes);
    d.field("coordinator_idle_badge_red_minutes", &s.coordinator_idle_badge_red_minutes);
    d.field("coordinator_auto_close_enabled", &s.coordinator_auto_close_enabled);
    d.field("coordinator_auto_close_minutes", &s.coordinator_auto_close_minutes);
    d.field(
        "coordinator_auto_close_skip_telegram_assigned",
        &s.coordinator_auto_close_skip_telegram_assigned,
    );
    d.field("coordinator_cascade_close_enabled", &s.coordinator_cascade_close_enabled);
    d.field("npm_update_notifications_enabled", &s.npm_update_notifications_enabled);
    d.field("remote_blocking_menus_enabled", &s.remote_blocking_menus_enabled);
    d.field("remote_agent_help_enabled", &s.remote_agent_help_enabled);
    d.field("auto_self_clear_enabled", &s.auto_self_clear_enabled);
    d.field("auto_self_clear_by_agent", &s.auto_self_clear_by_agent);
    d.field("agent_auto_update_by_command", &s.agent_auto_update_by_command);
    d.field("container_credentials_from_host", &s.container_credentials_from_host);
    d.field("watchers", &s.watchers);
    d.field("watchers_geometry", &s.watchers_geometry);
    d.field("quota_sources", &s.quota_sources);
    d.field("menu_guard_enabled", &s.menu_guard_enabled);
    d.field("typing_hold_seconds", &s.typing_hold_seconds);
    d.field("response_close_enabled", &s.response_close_enabled);
    d.field("response_close_idle_seconds", &s.response_close_idle_seconds);
    d.field("room_idle_confirmation_seconds", &s.room_idle_confirmation_seconds);
    d.field("context_history_enabled", &s.context_history_enabled);
    d.field("context_history_retention_days", &s.context_history_retention_days);
    d.field("context_history_session_windows", &s.context_history_session_windows);
    d.field("context_history_color_sessions", &s.context_history_color_sessions);
    d.field("temp_room_agent", &s.temp_room_agent);
    d.field("coding_agent_config_files", &s.coding_agent_config_files);
}

/// Compile-time guard: lists every `AppSettings` field. A new field fails to
/// compile here; add it to this list AND to one of `fields_1..3`.
#[rustfmt::skip]
fn all_fields_listed(s: &AppSettings) {
    let AppSettings {
        default_shell: _, default_shell_args: _, agents: _, coding_agent_profiles: _,
        agent_file_source_by_menu: _, telegram_bots: _, telegram_network_poll_error_logging: _,
        restore_coordinator_wake_state: _, legacy_start_only_coordinators: _,
        restart_resume_orchestrator_prompt: _, restart_resume_agent_prompt: _,
        restart_resume_wake_working_agents: _, sidebar_always_on_top: _, team_idle_beep_enabled: _,
        sounds_enabled: _, raise_terminal_on_click: _, screenshot_capture_hotkey: _,
        screenshot_hyprland_bind: _, sidebar_compact_hotkey: _, room_number_mask: _,
        voice_to_text_enabled: _, gemini_api_key: _, gemini_model: _, jev_api_key: _, jev_model: _,
        jev_endpoint: _, jev_timeout_secs: _, jev_threshold: _, jev_margin: _,
        co_managed_enabled: _, coding_agent_install_enabled: _, container_options_enabled: _,
        voice_auto_execute: _, voice_auto_execute_delay: _, sidebar_zoom: _, terminal_zoom: _,
        main_zoom: _, guide_zoom: _, darkfactory_zoom: _, sidebar_geometry: _, terminal_geometry: _,
        main_geometry: _, main_window_display_state: _, main_sidebar_width: _, main_sidebar_side: _,
        main_always_on_top: _, main_resource_monitor_attached: _, web_server_enabled: _,
        web_server_port: _, web_server_bind: _, api_server_enabled: _, api_server_port: _,
        api_server_bind: _, terminal_snapshots_enabled: _, project_path: _, project_paths: _,
        archived_project_paths: _, project_path_state: _, project_paths_persistence_error: _,
        local_overlay_state: _, agents_layer: _, coding_agent_identity_physical: _,
        sidebar_style: _, selected_row_rail_width: _, selected_row_rail_color: _, root_token: _,
        onboarding_dismissed: _, coord_sort_by_activity: _, always_show_selected_workgroup: _,
        log_level: _, activity_log_enabled: _, auto_generate_task_title: _, agent_templates_path: _,
        theme_light: _, rail_collapsed_projects: _, rail_favorites_collapsed: _,
        spec_board_enabled: _, web_browser_enabled: _, git_sweep_concurrency: _,
        git_sweep_min_interval_secs: _,
        ci_activity_enabled: _, ci_activity_notify_orchestrator: _, branch_staleness_enabled: _,
        branch_staleness_notify_orchestrator: _, ci_sweep_min_interval_secs: _,
        branch_staleness_interval_secs: _, resource_monitor_enabled: _,
        max_concurrent_agent_processes: _, resource_watchdog_action: _,
        agent_group_warn_private_bytes: _, agent_group_kill_private_bytes: _,
        agent_process_kill_private_bytes: _, resource_keep_last_snapshot: _,
        resource_backoff_polling: _, coordinator_idle_badge_yellow_minutes: _,
        coordinator_idle_badge_red_minutes: _, coordinator_auto_close_enabled: _,
        coordinator_auto_close_minutes: _, coordinator_auto_close_skip_telegram_assigned: _,
        coordinator_cascade_close_enabled: _, npm_update_notifications_enabled: _,
        remote_blocking_menus_enabled: _, remote_agent_help_enabled: _, auto_self_clear_enabled: _,
        auto_self_clear_by_agent: _, agent_auto_update_by_command: _,
        container_credentials_from_host: _, watchers: _, watchers_geometry: _, quota_sources: _,
        menu_guard_enabled: _, typing_hold_seconds: _, response_close_enabled: _,
        response_close_idle_seconds: _, room_idle_confirmation_seconds: _,
        context_history_enabled: _, context_history_retention_days: _,
        context_history_session_windows: _, context_history_color_sessions: _,
        temp_room_agent: _, coding_agent_config_files: _,
    } = s;
}
