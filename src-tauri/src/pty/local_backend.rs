use std::collections::HashMap;
#[cfg(not(windows))]
use std::io::Read;
use std::io::Write;
#[cfg(windows)]
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use portable_pty::{native_pty_system, PtySize};
use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{BackendSpawnSpec, PtyBackend};
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::{PtyScreenSnapshot, SessionIoFanout};
use crate::pty::output_senders::OutputSenderMap;
use crate::pty::spawn_diagnostics::{self, ChildLiveness, ExitCause, SpawnRecord, SpawnRecordInit};
use crate::pty::watchers::{FrameStamp, ScreenRowsSince};

mod blocking_spawn;
#[cfg(windows)]
mod cursor_query;
#[cfg(windows)]
mod git_guard;
mod instance;
mod launch;
mod liveness;

use blocking_spawn::spawn_blocking_cancel_safe;
#[cfg(windows)]
use cursor_query::{pump_cursor_query, write_cursor_reply};
#[cfg(all(test, windows))]
use cursor_query::{CursorReplyObserver, OneShotCursorQuery};
#[cfg(all(test, windows))]
use git_guard::write_git_guard_file_if_changed;
#[cfg(windows)]
use git_guard::{ensure_git_guard_wrapper, resolve_real_git_path, GitGuardEnv};
#[cfg(test)]
use instance::send_size_to_conpty;
use instance::{hand_over_held_size, resize_instance, PtyInstance, StartupGate};
use launch::prepare_launch;
#[cfg(test)]
use launch::{
    configured_shell_kind, is_direct_exe, posix_literal, prepare_windows_resolved_agent_launch,
    ps_literal, reject_detached_launcher, windows_arg, windows_raw_argv, PreparedLaunch,
    WindowsHostShellKind,
};
use liveness::{
    context_liveness_from_child_liveness, probe_child_contained, probe_child_in,
    screen_rows_if_child_alive,
};

#[cfg(windows)]
pub(super) static GIT_GUARD_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
fn build_git_guard_env() -> Result<Option<GitGuardEnv>, AppError> {
    let Some(real_git) = resolve_real_git_path() else {
        log::warn!("[pty] git.exe not found; skipping PATH git guard wrapper");
        return Ok(None);
    };

    let guard_dir = ensure_git_guard_wrapper()?;
    let current_path = crate::config::environment::inherited_path().unwrap_or_default();
    let mut path_entries: Vec<std::path::PathBuf> = vec![guard_dir];
    path_entries.extend(std::env::split_paths(&current_path));
    let path = std::env::join_paths(path_entries.iter())
        .map_err(|e| AppError::Other(format!("Failed to join PATH for git guard: {}", e)))?
        .to_string_lossy()
        .to_string();

    Ok(Some(GitGuardEnv {
        path,
        pathext: ".CMD;.BAT;.COM;.EXE".to_string(),
        real_git,
    }))
}

pub struct LocalProcessBackend {
    ptys: Arc<Mutex<HashMap<Uuid, PtyInstance>>>,
    fanout: SessionIoFanout,
    git_watcher: Arc<GitWatcher>,
    /// #1271 - per-instance, invocation-scoped pre-PTY attempt observer, test
    /// builds only. Incremented inside the real `spawn_sync` immediately before
    /// `native_pty_system()`, so a deterministic invalid-input rejection (which
    /// happens at the TOP of `spawn_sync`, before any spawn accounting) leaves
    /// it at zero. Each backend-level test constructs its own instance and reads
    /// its own observer; there is no global to reset and no cross-test race.
    #[cfg(test)]
    pre_pty_attempts: Arc<AtomicUsize>,
    #[cfg(all(test, windows))]
    cursor_observers: Arc<Mutex<HashMap<Uuid, Arc<CursorReplyObserver>>>>,
}

fn write_to_local_pty(
    ptys: &Mutex<HashMap<Uuid, PtyInstance>>,
    id: Uuid,
    data: &[u8],
) -> Result<(), AppError> {
    let writer = {
        let ptys = ptys.lock().unwrap_or_else(|error| error.into_inner());
        let instance = ptys
            .get(&id)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
        Arc::clone(&instance.writer)
    };

    // The global PTY map is released before a potentially blocking pipe
    // write. Teardown can remove and terminate the child to unblock it.
    let mut writer = writer.lock().unwrap_or_else(|error| error.into_inner());
    writer
        .write_all(data)
        .map_err(|error| AppError::PtyError(error.to_string()))?;
    writer
        .flush()
        .map_err(|error| AppError::PtyError(error.to_string()))
}

fn remove_local_pty(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) -> Option<PtyInstance> {
    ptys.lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&id)
}

impl Clone for LocalProcessBackend {
    fn clone(&self) -> Self {
        Self {
            ptys: Arc::clone(&self.ptys),
            fanout: self.fanout.clone(),
            git_watcher: Arc::clone(&self.git_watcher),
            #[cfg(test)]
            pre_pty_attempts: Arc::clone(&self.pre_pty_attempts),
            #[cfg(all(test, windows))]
            cursor_observers: Arc::clone(&self.cursor_observers),
        }
    }
}

impl LocalProcessBackend {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
    ) -> Self {
        Self {
            ptys: Arc::new(Mutex::new(HashMap::new())),
            fanout: SessionIoFanout::new(output_senders, idle_detector, ws_broadcaster),
            git_watcher,
            #[cfg(test)]
            pre_pty_attempts: Arc::new(AtomicUsize::new(0)),
            #[cfg(all(test, windows))]
            cursor_observers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn spawn_sync(&self, spec: BackendSpawnSpec) -> Result<(), AppError> {
        let BackendSpawnSpec {
            id,
            agent_id,
            coding_agent,
            cmd,
            args,
            resolved_agent_host_shell,
            cwd,
            selected_cwd: _,
            cols,
            rows,
            container_image: _,
            configured_env,
            env_remove_keys,
            env_unset: _,
            extra_env,
            idle_tuning,
            output_target,
            mut resource_registration,
            logical_resource_slot: _,
            container_credential: _,
            container_repo_mounts: _,
            launch_witness,
        } = spec;

        // #1271 - validate and construct the adapted launch at the TOP of
        // `spawn_sync`, BEFORE any spawn accounting or PTY resource acquisition
        // (Grinch Finding 10): a rejected configured-host token is counted
        // nowhere and never opens a PTY. The same adapter result feeds both
        // `CommandBuilder` and the `exec_argv` provenance, so the two can never
        // desynchronize.
        let launch = prepare_launch(&cmd, &args, resolved_agent_host_shell.as_ref())?;

        // #942 - how many sessions were spawned in the window just before this one,
        // and how many of them were the same CLI. Concurrent startups against shared
        // agent state (the global ~/.codex) are a prime suspect for the intermittent
        // blank terminal, so every spawn record carries its own concurrency context.
        // Keyed on the CLI, never on the profile id: several profiles run the same
        // codex binary against the same ~/.codex. Counting only, no behavior change.
        let diag_thresholds = spawn_diagnostics::Thresholds::from_env();
        let spawn_window = spawn_diagnostics::note_spawn_attempt(coding_agent, diag_thresholds);
        // #1271 - accepted spawns only: the increment sits immediately before
        // `native_pty_system()`, so every invalid-input rejection above left the
        // per-instance observer at zero.
        #[cfg(test)]
        self.pre_pty_attempts.fetch_add(1, Ordering::SeqCst);
        let pty_system = native_pty_system();
        let spawn_cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);

        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(size)
            .map_err(|e| AppError::PtyError(e.to_string()))?;

        let mut command = launch.command_builder();
        command.cwd(&spawn_cwd);
        // #2683 - set PATH to the #2589 effective search path, which is also
        // what portable_pty searches to resolve a bare program name. Placement
        // is the precedence rule: the env-removal loop and the configured-env
        // loop below run later, so a configured PATH still wins and a removed
        // PATH is still removed. The Windows git-guard PATH also runs later.
        crate::config::agent_path::apply_search_path_to_pty_command(&mut command);

        // #942 - the argv exactly as executed, adapted host-shell wrapper
        // included. Built from the SAME `PreparedLaunch` the `CommandBuilder`
        // came from (one adapter decision), never mirrored by hand.
        let exec_argv = launch.exec_argv();

        // #942 - what the child will really see for CODEX_HOME: an explicit configured
        // value wins, then an explicit removal, then the AC environment the child
        // inherits. None here means this Codex shares the global ~/.codex with every
        // other one.
        let codex_home = configured_env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("CODEX_HOME"))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                if env_remove_keys
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case("CODEX_HOME"))
                {
                    None
                } else {
                    crate::config::environment::codex_home().ok()
                }
            });

        for key in &env_remove_keys {
            command.env_remove(key);
        }
        for (key, value) in &configured_env {
            command.env(key, value);
        }
        if !configured_env.is_empty() || !env_remove_keys.is_empty() {
            log::info!(
                "[pty] Applied {} configured env vars and removed {} inherited env vars for \
                    session {}",
                configured_env.len(),
                env_remove_keys.len(),
                id
            );
        }
        crate::pty::credentials::apply_credential_env_to_pty_command(&mut command, &extra_env);
        command.env("TERM", "xterm-256color");

        if !extra_env.is_empty() {
            log::info!(
                "[pty] Applied {} per-process credential environment variables for session {}",
                extra_env.len(),
                id
            );
        }

        if let Some(git_ceiling_dirs) =
            crate::config::session_context::git_ceiling_directories_for_session_root(&spawn_cwd)
        {
            command.env("GIT_CEILING_DIRECTORIES", &git_ceiling_dirs);
            log::info!(
                "[pty] Applied GIT_CEILING_DIRECTORIES for session cwd {}: {}",
                spawn_cwd,
                git_ceiling_dirs
            );

            #[cfg(windows)]
            if let Some(git_guard_env) = build_git_guard_env()? {
                command.env("PATH", &git_guard_env.path);
                command.env("PATHEXT", &git_guard_env.pathext);
                command.env("AC_REAL_GIT", &git_guard_env.real_git);
                log::info!(
                    "[pty] Enabled git guard wrapper for session cwd {}",
                    spawn_cwd
                );
            }
        }

        // #942 - time zero for time-to-first-output.
        let spawn_started = Instant::now();
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| AppError::PtyError(e.to_string()))?;
        // #2413 - a child exists from here on; every later `Err` is fenced.
        launch_witness.mark();
        let child_pid = child.process_id();
        log::info!(
            "[pty] Spawned session {} with child pid {:?}",
            id,
            child_pid
        );

        let job = child
            .process_id()
            .and_then(crate::pty::job::JobObject::for_child);

        if let Some(registration) = resource_registration.as_mut() {
            let Some(pid) = child.process_id() else {
                let _ = child.kill();
                return Err(AppError::PtyError(
                    "Resource Monitor could not capture spawned child pid".to_string(),
                ));
            };
            if let Err(err) = registration.register_root_pid(pid) {
                let _ = child.kill();
                return Err(AppError::PtyError(err));
            }
        }

        drop(pair.slave);

        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(e) => {
                if let Some(registration) = resource_registration.as_ref() {
                    registration.rollback_registered();
                }
                return Err(AppError::PtyError(e.to_string()));
            }
        };

        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => {
                if let Some(registration) = resource_registration.as_ref() {
                    registration.rollback_registered();
                }
                return Err(AppError::PtyError(e.to_string()));
            }
        };

        // #973 (B) - the child has rendered nothing yet, so the gate starts closed.
        let rendered = Arc::new(AtomicBool::new(false));
        let instance = PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Some(child),
            job,
            // #973 - the size we actually opened the ConPTY at (see PtyViewport).
            size: Mutex::new((cols, rows)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::clone(&rendered),
        };

        #[cfg(windows)]
        let cursor_writer = Arc::downgrade(&instance.writer);
        #[cfg(all(test, windows))]
        let cursor_observer = self.cursor_observers.lock().unwrap().get(&id).cloned();

        self.ptys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, instance);
        let output_token =
            match self
                .fanout
                .register_session(id, idle_tuning, rows, cols, output_target)
            {
                Ok(token) => token,
                Err(_) => {
                    let _ = self.kill(id);
                    return Err(AppError::PtyError(
                        "terminal output registration failed".to_string(),
                    ));
                }
            };

        // #942 - app.log is what users paste into issues, so a secret must never be
        // echoed back out through the child output or the argv we log. Keyed on the env
        // NAME (token / key / secret / password / credential), across both AC's own
        // credential env and the configured rows where AC tells users to keep their API
        // keys. Deliberately not a sweep of every value: a Codex profile legitimately
        // carries `MODEL=gpt-5.6-sol`, and blanking that would shred the very argv this
        // instrumentation exists to record.
        let redact: Vec<String> = extra_env
            .iter()
            .chain(configured_env.iter())
            .filter(|(key, _)| spawn_diagnostics::is_secret_env_key(key))
            .map(|(_, value)| value.clone())
            .collect();

        // #942 - emits `[pty] spawn-record` (argv, cwd, CLI, profile, CODEX_HOME,
        // concurrency).
        let record = spawn_diagnostics::register(SpawnRecordInit {
            session_id: id,
            pid: child_pid,
            argv: exec_argv,
            cwd: spawn_cwd,
            cli: coding_agent,
            agent_profile_id: agent_id,
            codex_home,
            configured_env_count: configured_env.len(),
            removed_env_count: env_remove_keys.len(),
            redact,
            window: spawn_window,
            started: spawn_started,
            thresholds: diag_thresholds,
        });

        // #942 - the startup verdict at the deadline and exit attribution. The PTY
        // reader alone cannot carry either: ConPTY holds the pipe open past the death of
        // the child, so a child that dies on its own would stay invisible until the
        // session is torn down. The monitor polls the child handle instead and ends with
        // the session (detached on purpose; the handle is only useful to tests).
        let monitor_backend = self.clone();
        let _monitor = spawn_diagnostics::watch_child(Arc::clone(&record), move || {
            monitor_backend.probe_child(id)
        });

        let session_id_str = id.to_string();
        let fanout = self.fanout.clone();
        let gate_backend = self.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            pump_cursor_query(
                &mut reader,
                |bytes| {
                    let read_at = Instant::now();
                    record.note_output(bytes);
                    fanout.observe_raw_read_for_reader(&output_token, bytes.len(), read_at)
                },
                |bytes| {
                    fanout.handle_output(&output_token, &session_id_str, bytes.to_vec());
                    if !rendered.load(Ordering::Relaxed) && fanout.has_rendered_visible_content(id)
                    {
                        gate_backend.open_startup_gate(id);
                    }
                },
                || {
                    write_cursor_reply(
                        &cursor_writer,
                        fanout.cursor_position_for_reader(&output_token),
                    )
                },
                #[cfg(test)]
                cursor_observer.as_deref(),
            );
            #[cfg(not(windows))]
            {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // #942 - time-to-first-output and the retained head bytes. Hot
                            // path: once the first byte is stamped and the head buffer is
                            // full this is two relaxed loads and one relaxed add.
                            record.note_output(&buf[..n]);
                            fanout.handle_output(&output_token, &session_id_str, buf[..n].to_vec());
                            // #973 (B) - has the child PAINTED anything yet? Asked of the real
                            // vt100 parser that `handle_output` has just fed this chunk to, which
                            // is why it is asked after it and not before.
                            //
                            // Once the child has painted, this is a single relaxed load per chunk
                            // and nothing else: no lock, no scan, no allocation. Before it has, it
                            // is one uncontended lock and a bounded scan of the grid - and it
                            // REPLACED a second `from_utf8_lossy` + `strip_ansi_csi` over the whole
                            // chunk, which `handle_output` was already paying for the idle detector
                            // on the very same bytes. One less chunk-sized allocation per chunk,
                            // not one more.
                            //
                            // Locks: the query takes `screen_parsers` and RELEASES it before
                            // `open_startup_gate` takes `ptys`. The two are sequential, never
                            // nested, so the order stays `ptys -> startup_gate -> size -> master`.
                            if !rendered.load(Ordering::Relaxed)
                                && fanout.has_rendered_visible_content(id)
                            {
                                gate_backend.open_startup_gate(id);
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        });

        Ok(())
    }

    /// #973 (B) - the child rendered. Hand the ConPTY the size the view has been waiting to
    /// give it, and bring the vt100 screen with it. Called from the PTY read loop, once per
    /// session in practice. The decision itself is `hand_over_held_size`.
    fn open_startup_gate(&self, id: Uuid) {
        let applied = {
            let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            let Some(instance) = ptys.get(&id) else {
                return;
            };
            hand_over_held_size(instance, id)
        };

        // Outside the `ptys` guard: the screen and the broadcast take locks of their own, and
        // every terminal write, resize and kill in the app queues behind that one.
        //
        // Only for a size the ConPTY actually took. The vt100 screen models the CHILD's
        // screen, so it must not be moved to a size the child was never given.
        if let Some((cols, rows)) = applied {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }
    }

    /// #942 - liveness of the child of a session, without disturbing it. Never blocks
    /// (zero timeout) and never reports a child it could not query as running.
    fn probe_child(&self, id: Uuid) -> ChildLiveness {
        probe_child_in(&self.ptys, id)
    }
}

impl PtyBackend for LocalProcessBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        let id = spec.id;
        let spawn_backend = self.clone();
        let cleanup_backend = self.clone();
        spawn_blocking_cancel_safe(
            id,
            move || spawn_backend.spawn_sync(spec),
            move |id| {
                if let Err(err) = cleanup_backend.kill(id) {
                    log::warn!("[pty] Failed to clean up cancelled local spawn {id}: {err}");
                }
            },
            "local process spawn task failed",
        )
    }

    fn write(
        &self,
        _authority: &crate::pty::input_route::BackendWriteAuthority,
        id: Uuid,
        data: &[u8],
    ) -> Result<(), AppError> {
        // #942 - poison-tolerant: a panic anywhere under this guard (portable-pty unwraps
        // inside its own child polling) must not brick every terminal write that follows.
        write_to_local_pty(&self.ptys, id, data)
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.ptys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&id)
    }

    fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        context_liveness_from_child_liveness(&probe_child_in(&self.ptys, id))
    }

    fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        // The idle grace sees every resize the view ASKED for, held or refused. Idle semantics
        // are #954's, not this fix's.
        self.fanout.record_resize(id);

        let sent = {
            let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
            let instance = ptys
                .get(&id)
                .ok_or_else(|| AppError::SessionNotFound(id.to_string()))?;
            resize_instance(instance, id, cols, rows)?
        };

        // #973 - the vt100 screen models the CHILD's screen, so it may only follow a size the
        // ConPTY actually took.
        //
        // A refused `0x0` would otherwise set the parser to zero rows (`output.rs`), and #955's
        // `get_screen_snapshot` reads `contents_formatted()` off that grid: an empty snapshot,
        // and a black tile on re-attach - the exact bug #955 shipped to kill.
        //
        // A HELD size is not the child's size either. The child is still emitting for the size
        // the PTY was opened at, and a screen moved ahead of it would parse that output against
        // a geometry the child is not using. `open_startup_gate` moves the screen at the instant
        // the ConPTY takes the size, which is the first moment the two agree.
        //
        // A DEDUPED size is already the screen's size: `register_session` seeds the parser with
        // the size the PTY was opened at, and from there the two only ever move together.
        if sent {
            self.fanout.resize_screen_and_broadcast(id, cols, rows);
        }

        Ok(())
    }

    fn kill(&self, id: Uuid) -> Result<(), AppError> {
        // #942 - probe the child BEFORE we tag the stop and BEFORE we touch job or
        // child. That ordering is the whole trick: nothing AC does can have killed a
        // child this probe already finds dead, so "was it already gone when we asked?"
        // has a witness that no race can flip. The old "already exited" line fired both
        // for a child that had died on its own and for one our own job kill had just
        // taken down; this is what tells the two apart.
        let child_at_stop = self.probe_child(id);
        let record =
            spawn_diagnostics::mark_ac_stop(id, "session-kill", Some(child_at_stop.clone()));

        let instance = remove_local_pty(&self.ptys, id);

        if let Some(mut instance) = instance {
            if let Some(job) = instance.job.take() {
                job.terminate();
            }
            if let Some(mut child) = instance.child.take() {
                let pid = child.process_id();
                log::info!(
                    "[pty] session-stop session={} pid={:?} source=session-kill child_at_stop={}",
                    id,
                    pid,
                    child_at_stop.as_log()
                );
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // Dead by the time we removed it. The pre-stop probe decides
                        // whose kill this was: already dead before we touched anything
                        // means the child ended itself.
                        let liveness = ChildLiveness::from(&status);
                        match record.as_ref() {
                            Some(record) => {
                                let cause = record.attribute_exit(record.stop_snapshot());
                                record.log_child_exit(cause, &liveness, "observed-at-stop");
                            }
                            None => {
                                let cause = if matches!(child_at_stop, ChildLiveness::Exited { .. })
                                {
                                    ExitCause::ChildInitiated
                                } else {
                                    ExitCause::AcRequested
                                };
                                log::info!(
                                    "[pty] child-exit session={} pid={:?} cause={} \
                                        detail=observed-at-stop \
                                        child={}",
                                    id,
                                    pid,
                                    cause.as_log(),
                                    liveness.as_log()
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        if let Err(e) = child.kill() {
                            log::warn!(
                                "[pty] Failed to kill session {} child pid {:?}: {}",
                                id,
                                pid,
                                e
                            );
                        }
                        reap_child_in_background(id, pid, child, record);
                    }
                    Err(e) => {
                        log::warn!(
                            "[pty] Failed to poll session {} child pid {:?}: {}",
                            id,
                            pid,
                            e
                        );
                        if let Err(kill_err) = child.kill() {
                            log::warn!(
                                "[pty] Failed to kill session {} child pid {:?} after poll error: \
                                    {}",
                                id,
                                pid,
                                kill_err
                            );
                        }
                        reap_child_in_background(id, pid, child, record);
                    }
                }
            }
        }

        self.fanout.remove_session(id);
        self.git_watcher.remove_session(id);
        spawn_diagnostics::forget(id);

        Ok(())
    }

    /// #942 - record who asked for the stop and what the child looked like BEFORE any
    /// process is touched. The resource monitor kills a process tree without going through
    /// the PTY layer, so a caller that is about to do that publishes the witness here
    /// first; without it, a child that had already died on its own would be logged as our
    /// kill and its evidence dropped.
    fn publish_stop_witness(&self, id: Uuid, source: &str) {
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, source, Some(child_at_stop));
    }

    fn terminate_job_for_session(&self, id: Uuid) -> bool {
        // #942 - AC asked for this stop. Probe first (same reason as `kill`), then tag,
        // so a child that was already dead is never charged to us. This stop can also
        // FAIL (a Quarantined resource-monitor kill leaves the instance intact), which
        // is why the tag only owns exits that follow it inside the attribution window.
        let child_at_stop = self.probe_child(id);
        spawn_diagnostics::mark_ac_stop(id, "job-terminate", Some(child_at_stop));
        let ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        match ptys.get(&id).and_then(|inst| inst.job.as_ref()) {
            Some(job) => {
                job.terminate();
                true
            }
            None => false,
        }
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        let mut ptys = self.ptys.lock().unwrap_or_else(|e| e.into_inner());
        let mut terminated = 0;
        let mut jobless = 0;
        for (id, instance) in ptys.iter_mut() {
            // #942 - shutdown stops every live session; tag them all as ours, with the
            // same pre-stop witness every other stop path publishes. Lock order here is
            // ptys -> diagnostics registry; nothing ever takes them the other way round
            // (the monitor holds no registry lock while it probes under ptys).
            let child_at_stop = match instance.child.as_mut() {
                Some(child) => probe_child_contained(child),
                None => ChildLiveness::Gone,
            };
            spawn_diagnostics::mark_ac_stop(*id, "app-shutdown", Some(child_at_stop));
            match instance.job.as_ref() {
                Some(job) => {
                    job.terminate();
                    terminated += 1;
                    log::info!("[pty] terminated job object for session {id} at shutdown");
                }
                None => jobless += 1,
            }
        }
        (terminated, jobless)
    }

    fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        self.fanout.get_screen_snapshot(id)
    }

    fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        self.fanout.has_rendered_visible_content(id)
    }

    fn activate_terminal_output(
        &self,
        id: Uuid,
        label: &str,
        include_history: bool,
    ) -> Result<
        Option<crate::pty::output::PtyScreenSnapshot>,
        crate::pty::output::TerminalOutputAttachError,
    > {
        self.fanout
            .activate_terminal_output(id, label, include_history)
    }

    fn detach_terminal_output(&self, id: Uuid, label: &str) {
        self.fanout.detach_terminal_output(id, label);
    }

    fn release_window_attachments(&self, label: &str) {
        self.fanout.release_window_attachments(label);
    }

    fn shutdown_terminal_output(&self) {
        self.fanout.shutdown_terminal_output();
    }

    #[allow(private_interfaces)]
    fn copy_terminal_screen(&self, id: Uuid) -> crate::pty::backend::TerminalScreenCopyRead {
        self.fanout.copy_terminal_screen(id)
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.fanout.get_pty_size(id)
    }

    fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        screen_rows_if_child_alive(&self.ptys, &self.fanout, id)
    }

    /// #1171 - straight to the fanout, with **no child liveness probe**.
    ///
    /// `get_screen_rows` above probes the child under the `ptys` guard, "the one every
    /// terminal write, resize and kill locks on" (`:1116-1117`). At 200 ms that probe would be
    /// taken 25x more often than the 5 s scraper takes it, on the hottest lock in the PTY
    /// layer, for a question the watcher engine does not ask on this path: it runs its own
    /// liveness probe once every 25th tick, that is once per 5 s per session, exactly today's
    /// rate. What remains here is one `screen_parsers` acquisition, and that map is per
    /// backend rather than per process.
    ///
    /// An absent parser is `Missing` and not `Gone`: for a local process, parser-absence is a
    /// desync or a poisoned lock, never a statement about the child. Only the 5 s probe, or
    /// the child oracle behind `get_screen_rows`, may retire a local session.
    fn screen_rows_since(&self, id: Uuid, seen: Option<FrameStamp>) -> ScreenRowsSince {
        self.fanout.get_screen_rows_since(id, seen)
    }

    fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        self.fanout
            .register_response_watcher(session_id, request_id, response_dir);
    }
}

fn reap_child_in_background(
    session_id: Uuid,
    pid: Option<u32>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    record: Option<Arc<SpawnRecord>>,
) {
    std::thread::spawn(move || match child.wait() {
        Ok(status) => {
            // #942 - the exit AC asked for. The monitor may have reported this child
            // first (it died a hair before our stop); then the event is already on
            // record with the right cause and we only leave a reap crumb.
            let liveness = ChildLiveness::from(&status);
            match record.as_ref() {
                Some(record) => {
                    let cause = record.attribute_exit(record.stop_snapshot());
                    if !record.log_child_exit(cause, &liveness, "reaped-after-stop") {
                        log::debug!(
                            "[pty] Reaped session {} child pid {:?}: {:?} (exit already reported)",
                            session_id,
                            pid,
                            status
                        );
                    }
                }
                None => log::info!(
                    "[pty] child-exit session={} pid={:?} cause=ac-requested \
                        detail=reaped-after-stop \
                        child={}",
                    session_id,
                    pid,
                    liveness.as_log()
                ),
            }
        }
        Err(e) => {
            log::warn!(
                "[pty] Failed to reap session {} child pid {:?}: {}",
                session_id,
                pid,
                e
            );
        }
    });
}

#[cfg(test)]
mod tests;
