#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{CommandFactory, FromArgMatches};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentscommander_lib::testability::ui_automation::{
    AutomationConfigWitness, InstanceIsolationTestHooks, UiCliDispatchContext,
};

struct RetainedAutomationConfigWitness {
    retained: agentscommander_lib::path_identity::RetainedDirectory,
    canonical_path: PathBuf,
    object_parts: (u64, u64),
}

impl RetainedAutomationConfigWitness {
    fn from_retained(retained: agentscommander_lib::path_identity::RetainedDirectory) -> Self {
        let identity = retained.identity();
        Self {
            canonical_path: identity.canonical_path.clone(),
            object_parts: (identity.object_id.volume, identity.object_id.file),
            retained,
        }
    }
}

impl AutomationConfigWitness for RetainedAutomationConfigWitness {
    fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    fn object_parts(&self) -> (u64, u64) {
        self.object_parts
    }

    fn verify_current(&self) -> bool {
        self.retained.verify_current().is_ok()
    }
}

struct SingleInstanceScope {
    mutex_name: String,
    config_witness: Option<Arc<dyn AutomationConfigWitness>>,
}

fn ensure_testable_config_identity(
    config_dir: &Path,
) -> Result<RetainedAutomationConfigWitness, &'static str> {
    if let Ok(retained) = agentscommander_lib::path_identity::retain_directory(config_dir) {
        return Ok(RetainedAutomationConfigWitness::from_retained(retained));
    }

    match std::fs::metadata(config_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("automation_config_identity_unavailable"),
    }

    let parent = config_dir
        .parent()
        .ok_or("automation_config_identity_unavailable")?;
    let retained_parent = agentscommander_lib::path_identity::retain_directory(parent)
        .map_err(|_| "automation_config_identity_unavailable")?;
    retained_parent
        .verify_current()
        .map_err(|_| "automation_config_identity_unavailable")?;
    match std::fs::create_dir(config_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("automation_config_identity_unavailable"),
    }
    let retained = agentscommander_lib::path_identity::retain_directory(config_dir)
        .map_err(|_| "automation_config_identity_unavailable")?;
    retained_parent
        .verify_current()
        .map_err(|_| "automation_config_identity_unavailable")?;
    retained
        .verify_current()
        .map_err(|_| "automation_config_identity_unavailable")?;
    Ok(RetainedAutomationConfigWitness::from_retained(retained))
}

fn existing_ui_cli_dispatch_context(
    cmd: &agentscommander_lib::cli::Commands,
) -> Result<UiCliDispatchContext, &'static str> {
    if !cmd.is_ui_automation_command() {
        return Err("automation_config_identity_unavailable");
    }
    let config_dir = agentscommander_lib::config::config_dir()
        .ok_or("automation_config_identity_unavailable")?;
    let retained = match agentscommander_lib::path_identity::retain_directory(&config_dir) {
        Ok(retained) => retained,
        Err(_) => {
            return match std::fs::metadata(&config_dir) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err("automation_session_missing")
                }
                _ => Err("automation_config_identity_unavailable"),
            }
        }
    };
    let witness: Arc<dyn AutomationConfigWitness> =
        Arc::new(RetainedAutomationConfigWitness::from_retained(retained));
    Ok(UiCliDispatchContext::new(witness))
}

fn initialize_cli_logger(
    is_ui_automation: bool,
    instance_isolation_hooks: &dyn InstanceIsolationTestHooks,
    initialize: impl FnOnce(),
) {
    if is_ui_automation {
        instance_isolation_hooks.before_ui_cli_logger_config_phase();
    }
    initialize();
}

fn single_instance_scope(testable_artifact: bool) -> Result<SingleInstanceScope, &'static str> {
    if !testable_artifact {
        return Ok(SingleInstanceScope {
            mutex_name: agentscommander_lib::config::profile::mutex_name().to_string(),
            config_witness: None,
        });
    }
    let config_dir = agentscommander_lib::config::config_dir()
        .ok_or("automation_config_identity_unavailable")?;
    let witness = ensure_testable_config_identity(&config_dir)?;
    let object_parts = witness.object_parts();
    Ok(SingleInstanceScope {
        mutex_name: agentscommander_lib::config::profile::testable_mutex_name(
            object_parts.0,
            object_parts.1,
        ),
        config_witness: Some(Arc::new(witness)),
    })
}

fn write_stdout_error_and_exit(json: &str) -> ! {
    agentscommander_lib::cli::attach_parent_console();
    println!("{json}");
    agentscommander_lib::cli::flush_outputs();
    std::process::exit(1);
}

fn write_stderr_error_and_exit(json: &str) -> ! {
    agentscommander_lib::cli::attach_parent_console();
    eprintln!("{json}");
    agentscommander_lib::cli::flush_outputs();
    std::process::exit(1);
}

fn main() {
    // Resolve actual binary name at runtime so --help shows the correct name.
    // Leaked once at startup — lives for the process lifetime.
    let binary_name: &'static str = Box::leak(
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "agentscommander".to_string())
            .into_boxed_str(),
    );

    let testable_artifact =
        agentscommander_lib::testability::ui_automation::current_exe_is_testable();
    let instance_isolation_hooks: Arc<
        dyn agentscommander_lib::testability::ui_automation::InstanceIsolationTestHooks,
    > = Arc::new(agentscommander_lib::testability::ui_automation::NoopInstanceIsolationTestHooks);
    let mut cmd = agentscommander_lib::cli::Cli::command().name(binary_name);
    if testable_artifact {
        for name in [
            "ui-query",
            "ui-click",
            "ui-context-click",
            "ui-hover",
            "ui-set",
            "ui-type",
            "ui-backend",
            "ui-wait",
            "ui-capabilities",
            "ui-list",
            "ui-focus",
        ] {
            cmd = cmd.mut_subcommand(name, |subcommand| subcommand.hide(false));
        }
    }

    match cmd.try_get_matches() {
        Ok(matches) => {
            match agentscommander_lib::cli::Cli::from_arg_matches(&matches) {
                Ok(cli) => match cli.command {
                    Some(cmd) => {
                        // Attach to the parent console BEFORE init_logger so
                        // any startup eprintln! (e.g. the "[log] file logging
                        // to ..." line) reaches the user's terminal on
                        // Windows release builds (where the binary is linked
                        // with `windows_subsystem = "windows"` and starts
                        // with no attached stderr).

                        let is_ui_automation = cmd.is_ui_automation_command();
                        agentscommander_lib::cli::attach_parent_console();
                        if matches!(
                            cmd,
                            agentscommander_lib::cli::Commands::ListPeers(_)
                                | agentscommander_lib::cli::Commands::ListPeersLean(_)
                                | agentscommander_lib::cli::Commands::ListSessions(_)
                                | agentscommander_lib::cli::Commands::AgencyTemplates(_)
                                | agentscommander_lib::cli::Commands::UiQuery(_)
                                | agentscommander_lib::cli::Commands::UiClick(_)
                                | agentscommander_lib::cli::Commands::UiContextClick(_)
                                | agentscommander_lib::cli::Commands::UiHover(_)
                                | agentscommander_lib::cli::Commands::UiSet(_)
                                | agentscommander_lib::cli::Commands::UiType(_)
                                | agentscommander_lib::cli::Commands::UiBackend(_)
                                | agentscommander_lib::cli::Commands::UiWait(_)
                                | agentscommander_lib::cli::Commands::UiCapabilities(_)
                                | agentscommander_lib::cli::Commands::UiList(_)
                                | agentscommander_lib::cli::Commands::UiFocus(_)
                                | agentscommander_lib::cli::Commands::TaskSetTitle(_)
                                | agentscommander_lib::cli::Commands::CodingAgent(_)
                                | agentscommander_lib::cli::Commands::TerminalSnapshot(_)
                        ) {
                            std::env::set_var("AC_MACHINE_OUTPUT", "1");
                        }
                        if is_ui_automation && !testable_artifact {
                            write_stdout_error_and_exit(
                                &agentscommander_lib::testability::ui_automation::refusing_non_testeable_binary_json(),
                            );
                        }

                        let ui_context = if is_ui_automation {
                            let context = match existing_ui_cli_dispatch_context(&cmd) {
                                Ok(context) => context,
                                Err("automation_session_missing") => write_stdout_error_and_exit(
                                    &agentscommander_lib::testability::ui_automation::automation_session_missing_json(),
                                ),
                                Err(_) => write_stdout_error_and_exit(
                                    &agentscommander_lib::testability::ui_automation::automation_config_identity_unavailable_json(),
                                ),
                            };
                            if !context.verify_current() {
                                write_stdout_error_and_exit(
                                    &agentscommander_lib::testability::ui_automation::automation_config_identity_unavailable_json(),
                                );
                            }
                            Some(context)
                        } else {
                            None
                        };
                        if is_ui_automation {
                            instance_isolation_hooks.after_ui_cli_context_acquired_before_logger();
                        }
                        // Install the same logger backend the GUI uses so

                        // every `log::*` call from CLI verbs (the `[task]`
                        // audit lines in particular — plan #137 §3a HIGH-1
                        // mitigation) reaches stderr + <config_dir>/app.log.
                        // GATED on `cli.command.is_some()` so the GUI branch
                        // below initializes via `lib::run()` exactly once.
                        initialize_cli_logger(
                            is_ui_automation,
                            instance_isolation_hooks.as_ref(),
                            agentscommander_lib::logging::init_logger,
                        );

                        if let Some(context) = ui_context.as_ref() {
                            if !context.verify_current() {
                                write_stdout_error_and_exit(
                                    &agentscommander_lib::testability::ui_automation::automation_config_identity_unavailable_json(),
                                );
                            }
                        }

                        // Issue #609 Phase 2 - one-line "update available" notice for
                        // terminal runs. Cache-only (no network, no blocking). M1: gate
                        // on an INTERACTIVE stderr, not the AC_MACHINE_OUTPUT allowlist -
                        // that allowlist (above) covers only ListPeers/ListPeersLean/
                        // ListSessions/AgencyTemplates/Ui*, so `send`, `task`,
                        // `window-info`, and any future machine verb would get stderr
                        // spam for up to 24h while an update is pending. `is_terminal()`
                        // is the future-proof gate: a human at a terminal sees the
                        // notice; agents, scripts, and piped/redirected runs (stderr is
                        // not a tty) stay silent.
                        {
                            use std::io::IsTerminal;
                            if std::io::stderr().is_terminal()
                                && std::env::var_os("AC_MACHINE_OUTPUT").is_none()
                            {
                                if let Some(notice) =
                                    agentscommander_lib::update_check::read_cached_notice()
                                {
                                    eprintln!("{}", notice);
                                }
                            }
                        }

                        let code = agentscommander_lib::cli::handle_cli(cmd, ui_context.as_ref());
                        agentscommander_lib::cli::flush_outputs();
                        drop(ui_context);
                        std::process::exit(code);
                    }
                    None => {
                        // GUI mode (with or without --app)
                        let placement = match agentscommander_lib::testability::window_placement::resolve_from_cli_or_env(
                            cli.window_x,
                            cli.window_y,
                            cli.window_width,
                            cli.window_height,
                            cli.window_maximized,
                        ) {
                            Ok(placement) => placement,
                            Err(e) => {
                                agentscommander_lib::cli::attach_parent_console();
                                eprintln!("Error: {}", e);
                                agentscommander_lib::cli::flush_outputs();
                                std::process::exit(1);
                            }
                        };

                        let ui_automation_enabled = match agentscommander_lib::testability::ui_automation::resolve_enabled_from_cli_or_env(cli.ui_automation, testable_artifact) {
                            Ok(enabled) => enabled,
                            Err(e) => {
                                agentscommander_lib::cli::attach_parent_console();
                                eprintln!("{}", e);
                                agentscommander_lib::cli::flush_outputs();
                                std::process::exit(1);
                            }
                        };

                        let scope = match single_instance_scope(testable_artifact) {
                            Ok(scope) => scope,
                            Err(_) => write_stderr_error_and_exit(
                                "{\"ok\":false,\"error\":\"automation_config_identity_unavailable\",\"message\":\"Could not prove the testable configuration directory identity.\"}",
                            ),
                        };
                        match try_acquire_single_instance(&scope.mutex_name) {
                            Ok(true) => {}
                            Ok(false) if testable_artifact => write_stderr_error_and_exit(
                                "{\"ok\":false,\"error\":\"automation_config_in_use\",\"message\":\"Another testable AgentsCommander process already owns this configuration.\"}",
                            ),
                            Ok(false) => std::process::exit(0),
                            Err(_) if testable_artifact => write_stderr_error_and_exit(
                                "{\"ok\":false,\"error\":\"automation_single_instance_unavailable\",\"message\":\"Could not acquire the testable configuration instance lock.\"}",
                            ),
                            Err(_) => {}
                        }
                        let witness = if ui_automation_enabled {
                            scope.config_witness.clone()
                        } else {
                            None
                        };
                        let result = agentscommander_lib::run(
                            placement,
                            ui_automation_enabled,
                            witness,
                            Arc::clone(&instance_isolation_hooks),
                        );
                        drop(scope);
                        if let Err(error) = result {
                            let json = match error {
                                "automation_config_identity_unavailable" => "{\"ok\":false,\"error\":\"automation_config_identity_unavailable\",\"message\":\"Could not prove the testable configuration directory identity.\"}",
                                _ => "{\"ok\":false,\"error\":\"automation_session_stale\",\"message\":\"Could not prove the testable GUI process instance.\"}",
                            };
                            write_stderr_error_and_exit(json);
                        }
                    }
                },
                Err(e) => {
                    agentscommander_lib::cli::attach_parent_console();
                    let _ = e.print();
                    agentscommander_lib::cli::flush_outputs();
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            // --help, --version, or invalid args: print and exit
            agentscommander_lib::cli::attach_parent_console();
            let _ = e.print();
            agentscommander_lib::cli::flush_outputs();
            std::process::exit(if e.use_stderr() { 1 } else { 0 });
        }
    }
}

/// Try to acquire a system-wide named mutex.
/// Returns true if this is the first GUI instance, false if one is already running,
/// and the raw OS error when mutex creation itself fails.
#[cfg(target_os = "windows")]
fn try_acquire_single_instance(mutex_name: &str) -> Result<bool, u32> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    const ERROR_ALREADY_EXISTS: u32 = 183;

    let name: Vec<u16> = mutex_name.encode_utf16().collect();

    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return Err(GetLastError());
        }
        // If the mutex already existed, another instance owns it
        Ok(GetLastError() != ERROR_ALREADY_EXISTS)
    }
    // Note: we intentionally do NOT close the handle — it must stay alive
    // for the lifetime of the process to hold the mutex.
}

#[cfg(not(target_os = "windows"))]
fn try_acquire_single_instance(_mutex_name: &str) -> Result<bool, u32> {
    Ok(true) // No single-instance enforcement on non-Windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingHooks {
        events: Mutex<Vec<&'static str>>,
    }

    impl InstanceIsolationTestHooks for RecordingHooks {
        fn after_ui_cli_context_acquired_before_logger(&self) {
            self.events.lock().unwrap().push("context-acquired");
        }

        fn before_ui_cli_logger_config_phase(&self) {
            self.events.lock().unwrap().push("logger-phase");
        }
    }

    #[test]
    fn ui_cli_logger_phase_has_a_distinct_hook_immediately_before_initialization() {
        let hooks = RecordingHooks::default();
        hooks.after_ui_cli_context_acquired_before_logger();
        initialize_cli_logger(true, &hooks, || {
            hooks.events.lock().unwrap().push("logger");
        });

        assert_eq!(
            *hooks.events.lock().unwrap(),
            ["context-acquired", "logger-phase", "logger"]
        );
    }

    #[test]
    fn non_ui_cli_logger_does_not_fire_the_ui_logger_phase_hook() {
        let hooks = RecordingHooks::default();
        initialize_cli_logger(false, &hooks, || {
            hooks.events.lock().unwrap().push("logger");
        });

        assert_eq!(*hooks.events.lock().unwrap(), ["logger"]);
    }
}
