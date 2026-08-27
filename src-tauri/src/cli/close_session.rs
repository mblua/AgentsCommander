use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::teams;
use crate::phone::types::OutboxMessage;

use super::send::agent_name_from_root;

/// §1440: colchon fijo de la espera de response, sumado a `--timeout`.
/// Cubre pickup (p90 10.1s observado), el probe de restore (5s), el
/// overshoot del handler sobre `--timeout` (max +3.5s observado) y la
/// escritura del response. Default total: 30 + 60 = 90s, igual que el
/// default de `send --confirm-timeout`. No escala con la cantidad de
/// sesiones del target: el CLI no la conoce (el daemon resuelve el target
/// recien en handle_close_session y cierra secuencialmente, ~`--timeout`
/// por sesion); expirar reporta "outcome unknown" (exit 2), no fallo.
const RESPONSE_WAIT_OVERHEAD_SECS: u64 = 60;

fn response_wait_budget(timeout_secs: u32) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(timeout_secs) + RESPONSE_WAIT_OVERHEAD_SECS)
}

/// §1440: mensaje de expiracion de la espera unica. `delivered_seen` decide
/// el sabor: el daemon termino y el response no aparecio donde el CLI
/// pollea, o no hay todavia rastro terminal del mensaje. Ambos ids van
/// etiquetados con su artefacto: `request` = responses/<request_id>.json,
/// `message` = outbox|delivered/<msg_id>.json y las lineas de app.log.
fn format_no_response_error(
    budget_secs: u64,
    request_id: &str,
    msg_id: &str,
    delivered_seen: bool,
) -> String {
    if delivered_seen {
        format!(
            "close-session delivered but daemon did not write a response within {}s; outcome unknown (request {}, message {})",
            budget_secs, request_id, msg_id
        )
    } else {
        format!(
            "close-session: no confirmation within {}s; outcome unknown (request {}, message {} still queued or in progress). The daemon may still be closing sessions in the background: closing several sessions takes about sessions x --timeout seconds and this wait budgets one. Verify with list-peers-lean before retrying.",
            budget_secs, request_id, msg_id
        )
    }
}

/// Pure: decide CLI exit code from the daemon's response body.
/// §224 G2 — exit codes:
///   0  — known status (closed | already_closed | no_match | restore_in_progress).
///   2  — unparseable JSON, missing `status` field, non-string status, or
///        unknown status value. Distinct from 1 (used elsewhere for auth/IO
///        failures) so scripts can distinguish "daemon spoke incoherently"
///        from "daemon refused".
///
/// Note: this contract applies only when the daemon successfully wrote a
/// response file the CLI could read. The orthogonal no-response path at the
/// end of `execute()` is NOT routed through this helper; see the single
/// response-poll loop (§1440). That fallback ALSO returns exit 2 (§224
/// G-IMPL-3): if no response appeared within the wait budget, whether or not
/// the message reached delivered/, the session's state is unknown (daemon
/// crashed mid-handle, response landed at an undeliverable path, still in
/// flight, or still queued). "Outcome unknown" belongs in the exit-2 class;
/// exit 0 or a fabricated failure here would re-create the silent-success /
/// false-timeout surfaces #224 and #1440 were filed to eliminate.
fn interpret_close_response_exit_code(content: &str) -> i32 {
    let resp: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return 2,
    };
    let Some(status) = resp.get("status").and_then(|v| v.as_str()) else {
        return 2;
    };
    match status {
        "closed" | "already_closed" | "no_match" | "restore_in_progress" => 0,
        _ => 2,
    }
}

/// Print a human-readable status line on stdout, after the JSON response.
/// §224 G7 — AC #2 requires "stdout message such as `No sessions matched ...`".
/// JSON is preserved for scripts; the prose line satisfies the literal AC text.
fn print_status_prose(content: &str) {
    let Ok(resp) = serde_json::from_str::<serde_json::Value>(content) else {
        return;
    };
    let target = resp.get("target").and_then(|v| v.as_str()).unwrap_or("");
    match resp.get("status").and_then(|v| v.as_str()) {
        Some("no_match") => {
            crate::cli_println!("No sessions matched '{}' — nothing to close.", target);
        }
        Some("already_closed") => {
            crate::cli_println!(
                "Session for '{}' already closed (raced before destroy).",
                target
            );
        }
        Some("restore_in_progress") => {
            crate::cli_println!(
                "Daemon is still restoring sessions; '{}' may exist once restore completes. \
                 Retry in a few seconds.",
                target
            );
        }
        // "closed" is self-explanatory from the JSON output.
        _ => {}
    }
}

#[derive(Args)]
#[command(after_help = "\
AUTHORIZATION: Only orchestrators of the target agent's team can close sessions. \
The master/root token bypasses this check.\n\n\
BEHAVIOR: By default, graceful shutdown is used — an exit command is injected into \
the agent's PTY (e.g., /exit for Claude Code) and the system waits for clean exit. \
If the agent doesn't exit within --timeout seconds, it falls back to force-kill. \
Use --force to skip graceful shutdown and kill immediately. \
The CLI waits --timeout + 60 seconds for the daemon's response. A target with several \
sessions is closed sequentially (about --timeout each) and can outlast the wait: exit 2 \
then means outcome unknown, the close keeps running server-side and is not cancelled; \
verify with list-peers-lean.\n\n\
DISCOVERY: Use `list-peers-lean` to get valid agent names. The `name` field of \
each entry is the canonical FQN to pass to --target.")]
pub struct CloseSessionArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Target agent name to close. Use `list-peers-lean` to discover valid names.
    /// Accepts FQN form (e.g., "myproject:wg-1-ac-devs/dev-rust" — preferred,
    /// matches the `name` field returned by `list-peers-lean`) or WG-local form
    /// (e.g., "wg-1-ac-devs/dev-rust" — auto-resolved when unambiguous across
    /// your project paths).
    #[arg(long)]
    pub target: String,

    /// Force-kill immediately, skipping graceful shutdown
    #[arg(long)]
    pub force: bool,

    /// Graceful shutdown timeout in seconds per session (default: 30). The CLI also waits
    /// this plus 60 seconds for the daemon's response.
    #[arg(long, default_value = "30")]
    pub timeout: u32,
}

pub fn execute(args: CloseSessionArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            log::error!("--root is required. Specify your agent's root directory.");
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    // Validate token
    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            log::error!("{}", msg);
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = agent_name_from_root(&root);

    // Resolve --target against known projects (Decision 2 / §AR2-shared).
    // Belt-and-braces alongside the mailbox-side resolver at handle_close_session
    // entry (§AR2-G1). Fail-fast at the CLI gives users immediate feedback on
    // ambiguous or unknown targets without writing to the outbox.
    let settings = crate::config::settings::load_settings();
    let resolved_target =
        match crate::config::teams::resolve_agent_target(&args.target, &settings.project_paths) {
            Ok(fqn) => fqn,
            Err(e) => {
                log::error!("{}", e);
                eprintln!("Error: {}", e);
                return 1;
            }
        };

    // Pre-validate coordinator authorization.
    // Check master token from LocalDir as additional bypass (independent of validate_cli_token).
    let is_master = is_root || {
        if let Some(ref token_str) = args.token {
            crate::config::config_dir()
                .map(|d| d.join("master-token.txt"))
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .map(|m| m.trim() == token_str)
                .unwrap_or(false)
        } else {
            false
        }
    };

    if !is_master {
        let discovered = teams::discover_teams();
        if discovered.is_empty()
            || !teams::is_coordinator_of(&sender, &resolved_target, &discovered)
        {
            log::error!(
                "authorization denied — '{}' is not an orchestrator of '{}'. Only orchestrators can close sessions of their team agents.",
                sender,
                resolved_target
            );
            eprintln!(
                "Error: authorization denied — '{}' is not an orchestrator of '{}'. \
                 Only orchestrators can close sessions of their team agents.",
                sender, resolved_target
            );
            return 1;
        }
    }

    let msg_id = Uuid::new_v4().to_string();
    let request_id = Uuid::new_v4().to_string();

    let message = OutboxMessage {
        id: msg_id.clone(),
        token: args.token,
        from: sender.clone(),
        to: resolved_target.clone(),
        body: String::new(),
        mode: String::new(),
        get_output: false,
        request_id: Some(request_id.clone()),
        sender_agent: None,
        preferred_agent: String::new(),
        priority: "normal".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        command: None,
        action: Some("close-session".to_string()),
        target: Some(resolved_target),
        force: Some(args.force),
        timeout_secs: Some(args.timeout),
        switch_coding_agent: None,
        switch_profile: None,
        dry_run: None,
        quiet_period_ms: None,
        pty_input: None,
    };

    // Write to outbox — use app outbox for root/master token, else agent's outbox
    let ac_dir = PathBuf::from(&root).join(crate::config::agent_local_dir_name());
    let outbox_dir = if is_root {
        let app_outbox = crate::config::config_dir()
            .map(|d| d.join("app-outbox-path.txt"))
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .map(|s| PathBuf::from(s.trim()));
        match app_outbox {
            Some(p) if p.is_dir() => p,
            _ => ac_dir.join("outbox"),
        }
    } else {
        ac_dir.join("outbox")
    };

    if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
        log::error!("failed to create outbox directory: {}", e);
        eprintln!("Error: failed to create outbox directory: {}", e);
        return 1;
    }

    let outbox_path = outbox_dir.join(format!("{}.json", msg_id));
    let json = match serde_json::to_string_pretty(&message) {
        Ok(j) => j,
        Err(e) => {
            log::error!("failed to serialize message: {}", e);
            eprintln!("Error: failed to serialize message: {}", e);
            return 1;
        }
    };

    if let Err(e) = std::fs::write(&outbox_path, json) {
        log::error!("failed to write outbox file: {}", e);
        eprintln!("Error: failed to write outbox file: {}", e);
        return 1;
    }

    // §1440: espera unica del veredicto terminal del daemon. El daemon escribe
    // rejected/<msg_id>.reason.txt al rechazar (antes de tocar sesiones),
    // responses/<request_id>.json al terminar, y mueve el mensaje a delivered/
    // SOLO DESPUES de escribir el response (ultima linea de
    // handle_close_session). Pollear el response subsume la vieja espera de
    // "delivery confirmation", cuyo presupuesto fijo de 30s expiraba durante
    // cierres graceful normales de ~30s y reportaba un timeout falso (97.9%
    // de los graceful, #1440).
    let delivered_path = outbox_dir
        .join("delivered")
        .join(format!("{}.json", msg_id));
    let rejected_reason_path = outbox_dir
        .join("rejected")
        .join(format!("{}.reason.txt", msg_id));

    let responses_dir = ac_dir.join("responses");
    let response_path = responses_dir.join(format!("{}.json", request_id));
    let budget = response_wait_budget(args.timeout);
    let poll = std::time::Duration::from_millis(250);
    let start = std::time::Instant::now();

    loop {
        if response_path.exists() {
            // §1440 F1: the daemon writes this file with std::fs::write (no
            // atomic rename) and, in the common case, TWICE (mailbox.rs
            // handle_close_session, §224 A.6 dual-write: the outbox-derived
            // write and the resolved-sender write land on the same file,
            // with an .await in between), so a poll tick can observe it
            // empty or truncated. An artifact that does not parse yet is
            // NOT a terminal verdict: keep polling; the next tick reads the
            // completed file. A genuinely malformed response (daemon bug)
            // or a persistent read failure therefore reports exit 2 at the
            // deadline instead of an immediate 2 (or a fabricated 1).
            if let Ok(content) = std::fs::read_to_string(&response_path) {
                if serde_json::from_str::<serde_json::Value>(&content).is_ok() {
                    crate::cli_println!("{}", content);
                    // §224 G7 — print a human-readable prose line for no_match
                    // / already_closed / restore_in_progress so AC #2's
                    // "stdout message such as `No sessions matched ...`" lands
                    // even when callers don't parse the JSON.
                    print_status_prose(&content);
                    // §224 G2 — validate the daemon's contract: known status
                    // → exit 0; missing / unknown status → exit 2. (Content
                    // that does not parse never reaches this call: the §1440
                    // F1 gate above keeps polling instead.)
                    return interpret_close_response_exit_code(&content);
                }
            }
        }
        if rejected_reason_path.exists() {
            let reason = std::fs::read_to_string(&rejected_reason_path)
                .unwrap_or_else(|_| "unknown reason".to_string());
            let trimmed = reason.trim();
            log::error!("close-session rejected — {}", trimmed);
            eprintln!("Error: close-session rejected — {}", trimmed);
            return 1;
        }
        if start.elapsed() >= budget {
            // §1440 / §224 G-IMPL-3: no response within the budget. The
            // session's terminal state is UNKNOWN: the daemon may still be
            // closing sessions, may have crashed mid-handle, or the response
            // may have landed at an undeliverable path (G-IMPL-2 + a non-
            // enumerable --root). Exit 2 ("outcome unknown"), never a
            // fabricated failure. Prose to stderr, not stdout, so script
            // consumers don't mistake it for the happy-path JSON.
            let m = format_no_response_error(
                budget.as_secs(),
                &request_id,
                &msg_id,
                delivered_path.exists(),
            );
            log::error!("{}", m);
            eprintln!("Error: {}", m);
            return 2;
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── §224 D.1 — interpret_close_response_exit_code (non-vacuous) ──

    #[test]
    fn closed_status_returns_zero() {
        let resp = r#"{"status":"closed","sessions_closed":2,"session_ids":["a","b"],"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 0);
    }

    #[test]
    fn already_closed_status_returns_zero() {
        let resp = r#"{"status":"already_closed","sessions_closed":0,"session_ids":[],"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 0);
    }

    #[test]
    fn no_match_status_returns_zero() {
        let resp = r#"{"status":"no_match","sessions_closed":0,"session_ids":[],"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 0);
    }

    #[test]
    fn restore_in_progress_status_returns_zero() {
        let resp = r#"{"status":"restore_in_progress","sessions_closed":0,"session_ids":[],"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 0);
    }

    #[test]
    fn unparseable_json_returns_two() {
        assert_eq!(interpret_close_response_exit_code("not json"), 2);
        assert_eq!(interpret_close_response_exit_code(""), 2);
        assert_eq!(interpret_close_response_exit_code("{partial"), 2);
    }

    #[test]
    fn missing_status_field_returns_two() {
        let resp = r#"{"sessions_closed":0,"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 2);
    }

    #[test]
    fn unknown_status_returns_two() {
        let resp = r#"{"status":"weird_new_state","target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 2);
    }

    #[test]
    fn non_string_status_returns_two() {
        let resp = r#"{"status":42,"target":"t","action":"close-session"}"#;
        assert_eq!(interpret_close_response_exit_code(resp), 2);
    }

    /// §224 review: valid JSON that is not an object (array, scalar, null)
    /// still fails the `.get("status")` lookup and must return exit 2 to
    /// preserve the "daemon spoke incoherently" contract.
    #[test]
    fn non_object_json_returns_two() {
        assert_eq!(interpret_close_response_exit_code("null"), 2);
        assert_eq!(interpret_close_response_exit_code("[1,2,3]"), 2);
        assert_eq!(interpret_close_response_exit_code("\"closed\""), 2);
        assert_eq!(interpret_close_response_exit_code("42"), 2);
        assert_eq!(interpret_close_response_exit_code("true"), 2);
    }

    // ── §224 D.1 — print_status_prose panic-resistance smoke tests ──
    // Subprocess test (D.8) covers the actual stdout content end-to-end.

    #[test]
    fn print_status_prose_does_not_panic_on_known_statuses() {
        for s in &[
            "closed",
            "already_closed",
            "no_match",
            "restore_in_progress",
        ] {
            let body = format!(r#"{{"status":"{}","target":"t"}}"#, s);
            print_status_prose(&body);
        }
    }

    #[test]
    fn print_status_prose_does_not_panic_on_unknown_input() {
        print_status_prose("not json");
        print_status_prose("");
        print_status_prose(r#"{"status":"unknown"}"#);
        print_status_prose(r#"{"no_status_at_all":true}"#);
        print_status_prose(r#"{"status":42}"#);
    }
    // ── §1440 ──

    // §1440 D.1: presupuesto derivado de --timeout
    #[test]
    fn response_wait_budget_adds_fixed_overhead() {
        assert_eq!(response_wait_budget(30).as_secs(), 90);
        assert_eq!(response_wait_budget(120).as_secs(), 180);
        assert_eq!(response_wait_budget(0).as_secs(), 60);
    }

    // §1440 D.2: los ids van etiquetados con su artefacto en ambos sabores
    #[test]
    fn no_response_error_labels_both_ids_in_both_flavors() {
        let m = format_no_response_error(90, "rid-1", "mid-1", false);
        assert!(m.contains("no confirmation within 90s"));
        assert!(m.contains("request rid-1"));
        assert!(m.contains("message mid-1"));
        let m = format_no_response_error(90, "rid-1", "mid-1", true);
        assert!(m.contains("did not write a response within 90s"));
        assert!(m.contains("request rid-1"));
        assert!(m.contains("message mid-1"));
    }
}
