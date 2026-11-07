//! Source-order controls supplement the existing runtime suites (#3946 P20).
use rust_source_scrub::{scrub, Literals};
use sha2::{Digest, Sha256};
use std::path::Path;

struct Control {
    path: &'static str,
    symbol: &'static str,
    current: &'static str,
    before: &'static str,
    after: &'static str,
    scope: Scope,
}

#[derive(Clone, Copy)]
enum Scope {
    Order,
    Outside,
    Inside,
    Optional,
}

const CONTROLS: [Control; 11] = [
    Control {
        path: "phone/mailbox/wake_injection.rs",
        symbol: "spawn_wake_session",
        current: include_str!("../wake_injection/fixture.rs"),
        before: "t.succeeded();",
        after: ".create_session(",
        scope: Scope::Order,
    },
    Control {
        path: "phone/mailbox/request_polling.rs",
        symbol: "poll_project_refresh_requests",
        current: include_str!("../request_polling.rs"),
        before: "app.project_refresh(payload)",
        after: "batch.processed_paths",
        scope: Scope::Order,
    },
    Control {
        path: "phone/mailbox/request_polling.rs",
        symbol: "poll_coding_agent_requests",
        current: include_str!("../request_polling.rs"),
        before: "letapplied={",
        after: "app.coding_agent_settings_updated(",
        scope: Scope::Outside,
    },
    Control {
        path: "pty/inject.rs",
        symbol: "inject_text_into_session_impl",
        current: include_str!("../../../pty/inject.rs"),
        before: "letpermit=PtyManager::acquire_input_writer(",
        after: "check_injection_hold(app,session_id,&hold_policy).await?;",
        scope: Scope::Order,
    },
    Control {
        path: "pty/inject.rs",
        symbol: "inject_text_into_session_impl",
        current: include_str!("../../../pty/inject.rs"),
        before: "drop(permit);",
        after: "watch.finish().await.into_result()?;",
        scope: Scope::Order,
    },
    Control {
        path: "phone/mailbox/pty_dispatch/select.rs",
        symbol: "dispatch_selection_spawn",
        current: include_str!("../pty_dispatch/select.rs"),
        before: "letspawn={",
        after: "await_pty_input_before_deadline(",
        scope: Scope::Inside,
    },
    Control {
        path: "phone/mailbox/wake_delivery.rs",
        symbol: "deliver_owned_peer_replacement",
        current: include_str!("../wake_delivery.rs"),
        before: ".execute_owned_peer_replacement(&app,&pty,&mutexecution).await;",
        after: "source.returned(result.is_err());",
        scope: Scope::Order,
    },
    Control {
        path: "phone/mailbox/pty_dispatch.rs",
        symbol: "dispatch_pty_input_operation_core",
        current: include_str!("../pty_dispatch.rs"),
        before: "letfailed=dispatcher",
        after: "source_executor.returned(failed);",
        scope: Scope::Order,
    },
    Control {
        path: "phone/mailbox/pty_authority.rs",
        symbol: "spawn_owned_pty_replacement",
        current: include_str!("../pty_authority.rs"),
        before: "with_source_validation(Arc::new(move||{",
        after: "letrefresh=refresh_pty_claim_for_resume(&store,&initial,&claimed).await;",
        scope: Scope::Inside,
    },
    Control {
        path: "phone/terminal_snapshot/process.rs",
        symbol: "process_claimed",
        current: include_str!("../../terminal_snapshot/process.rs"),
        before: "letSome(settings)=",
        after: "publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);return;",
        scope: Scope::Optional,
    },
    Control {
        path: "commands/web_browser/mailbox.rs",
        symbol: "handle_web_browser",
        current: include_str!("../browser.rs"),
        before: "letSome(session)=caller_session(app,msg).awaitelse{",
        after: "letresult=matchaction.as_str(){",
        scope: Scope::Order,
    },
];

fn compact(source: &str) -> Result<String, &'static str> {
    Ok(scrub(source, Literals::Drop)?
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect())
}

fn unique(source: &str, anchor: &str) -> Result<usize, &'static str> {
    let mut hits = source.match_indices(anchor);
    let (offset, _) = hits.next().ok_or("missing anchor")?;
    if hits.next().is_some() {
        return Err("nonunique anchor");
    }
    Ok(offset)
}

fn closing(source: &str, open: usize) -> Result<usize, &'static str> {
    if source.as_bytes().get(open) != Some(&b'{') {
        return Err("missing opening brace");
    }
    let mut depth = 0;
    for (offset, byte) in source.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(offset);
                }
            }
            _ => {}
        }
    }
    Err("unclosed brace")
}

fn function_body<'a>(source: &'a str, symbol: &str) -> Result<&'a str, &'static str> {
    let start = unique(source, &format!("fn{symbol}"))?;
    let open = source[start..].find('{').ok_or("missing function body")? + start;
    let end = closing(source, open)?;
    Ok(&source[open + 1..end])
}

impl Control {
    fn body(&self, source: &str, current: bool) -> Result<String, &'static str> {
        let source = compact(source)?;
        // Exact P13 native-event spellings; keep all scope and order unchanged.
        let source = source
            .replace(
                "tauri::Emitter::emit(app,,payload)",
                "app.project_refresh(payload)",
            )
            .replace(
                "tauri::Emitter::emit(app,,serde_json::json!",
                "app.coding_agent_settings_updated(&serde_json::json!",
            );
        let symbol = if current && self.symbol == "spawn_wake_session" {
            "spawn_fixture_wake_session"
        } else {
            self.symbol
        };
        Ok(function_body(&source, symbol)?.to_owned())
    }

    fn accepts(&self, body: &str) -> Result<bool, &'static str> {
        let first = unique(body, self.before)?;
        if matches!(self.scope, Scope::Optional) {
            return optional_services(body);
        }
        let second = unique(body, self.after)?;
        match self.scope {
            Scope::Order => Ok(first < second && self.extra_order(body)?),
            Scope::Outside | Scope::Inside => {
                let open = first + self.before.len() - 1;
                let end = closing(body, open)?;
                let inside = open < second && second < end;
                Ok(first < second
                    && inside == matches!(self.scope, Scope::Inside)
                    && self.extra_order(body)?)
            }
            Scope::Optional => unreachable!(),
        }
    }

    fn extra_order(&self, body: &str) -> Result<bool, &'static str> {
        match self.symbol {
            "spawn_wake_session" => Ok(unique(body, "letticket=")?
                < unique(body, "hooks.internal_spawn_error")?
                && unique(body, "t.failed();")? < unique(body, "t.succeeded();")?),
            "dispatch_selection_spawn" => {
                let proof = unique(body, "lettarget_ownership=")?;
                let call = unique(body, self.after)?;
                let scope = unique(body, self.before)? + self.before.len() - 1;
                let end = closing(body, scope)?;
                Ok(proof < call
                    && unique(body, "&target_ownership,")? < end
                    && body[call..end].ends_with(".await"))
            }
            "dispatch_pty_input_operation_core" => {
                let returned = unique(body, self.after)?;
                let call = unique(body, ".dispatch_accepted_operation(")?;
                Ok(call < returned && body[call..returned].ends_with(".await;"))
            }
            "spawn_owned_pty_replacement" => {
                let start = unique(body, self.before)? + self.before.len() - 1;
                let end = closing(body, start)?;
                let callback = &body[start + 1..end];
                let store = unique(callback, "letstore=")?;
                let refresh = unique(callback, self.after)?;
                let authority = unique(callback, ".pty_spawn_authority_is_current(")?;
                let recipe = unique(callback, ".owned_pty_recipe_is_current(")?;
                Ok(store < refresh && refresh < authority && authority < recipe)
            }
            _ => Ok(true),
        }
    }

    fn mutant(&self, body: &str) -> Result<String, &'static str> {
        let mut mutant = body.to_owned();
        let first = unique(body, self.before)?;
        if matches!(self.scope, Scope::Optional) {
            // A required lookup cannot take the optional unavailable return path.
            mutant.replace_range(first..first + self.before.len(), "letsettings=");
            return Ok(mutant);
        }
        let second = unique(body, self.after)?;
        match self.scope {
            Scope::Inside => {
                // End the proof/validator scope before its protected work.
                let end = closing(body, first + self.before.len() - 1)?;
                mutant.remove(end);
                mutant.insert(second, '}');
            }
            Scope::Outside => {
                // Put the notification inside the scope retaining the write guard.
                let end = closing(body, first + self.before.len() - 1)?;
                mutant.remove(end);
                let statement_end = statement_end(body, second)?;
                mutant.insert(statement_end - 1, '}');
            }
            Scope::Order => {
                // Move the later statement before the earlier one in a copied body.
                let (start, end) = self.later_statement(body, second)?;
                let statement = body[start..end].to_owned();
                mutant.replace_range(start..end, "");
                let insert = statement_start(&mutant, first);
                mutant.insert_str(insert, &statement);
            }
            Scope::Optional => unreachable!(),
        }
        Ok(mutant)
    }

    fn later_statement(&self, body: &str, second: usize) -> Result<(usize, usize), &'static str> {
        let start = if self.symbol == "poll_project_refresh_requests" {
            unique(body, "forpathinbatch.processed_paths{")?
        } else {
            statement_start(body, second)
        };
        let end = if self.symbol == "poll_project_refresh_requests" {
            let open = body[start..].find('{').ok_or("missing removal loop")? + start;
            closing(body, open)? + 1
        } else {
            statement_end(body, second)?
        };
        Ok((start, end))
    }
}

fn statement_start(body: &str, offset: usize) -> usize {
    body[..offset]
        .rfind([';', '{', '}'])
        .map_or(0, |previous| previous + 1)
}

fn statement_end(body: &str, start: usize) -> Result<usize, &'static str> {
    let mut depth = 0;
    for (offset, byte) in body.bytes().enumerate().skip(start) {
        match byte {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b';' if depth == 0 => return Ok(offset + 1),
            _ => {}
        }
    }
    Err("missing statement end")
}

fn optional_services(body: &str) -> Result<bool, &'static str> {
    let services = [
        (
            "session_manager",
            "host.session_manager_opt()",
            "app.try_state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()",
        ),
        (
            "pty_manager",
            "host.pty_manager_opt()",
            "app.try_state::<Arc<std::sync::Mutex<crate::pty::manager::PtyManager>>>()",
        ),
        (
            "settings",
            "host.settings_opt()",
            "app.try_state::<crate::config::settings::SettingsState>()",
        ),
        (
            "restore",
            "host.restore_opt()",
            "app.try_state::<Arc<crate::RestoreInProgress>>()",
        ),
        (
            "purge",
            "host.purge_opt()",
            "app.try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()",
        ),
    ];
    let mut previous = 0;
    for (name, current, base) in services {
        let prefix = format!("letSome({name})=");
        let Ok(start) = unique(body, &prefix) else {
            return Ok(false);
        };
        let rest = &body[start + prefix.len()..];
        let getter = if rest.starts_with(current) {
            current
        } else {
            base
        };
        let expected = format!(
            "{getter}else{{publish_error(TerminalSnapshotReasonCode::ServiceUnavailable);return;}};"
        );
        if start < previous || !rest.starts_with(&expected) {
            return Ok(false);
        }
        previous = start + prefix.len() + expected.len();
    }
    Ok(true)
}

fn digest(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

fn check_control(control: &Control, source: &str, row: usize, current: bool) {
    let body = control
        .body(source, current)
        .expect("unique source function");
    assert!(
        control.accepts(&body).expect("unique source anchors"),
        "row {row}"
    );
    let mutant = control.mutant(&body).expect("copied mutation anchors");
    assert_ne!(body, mutant, "row {row}: mutation must change source");
    assert!(
        !control.accepts(&mutant).unwrap_or(false),
        "row {row}: accepted mutant"
    );
    println!(
        "ISSUE3946 P20 RECEIPT row={row} current={current} source={} body={} mutant={} {} {}",
        digest(source),
        digest(&body),
        digest(&mutant),
        control.before,
        control.after
    );
    if !current {
        println!("ISSUE3946 P20 BASE_PASS {row}");
        println!("ISSUE3946 P20 MUTANT_REJECTED {row}");
    }
}

#[test]
fn issue_3946_equivalence_controls() {
    for (index, control) in CONTROLS.iter().enumerate() {
        check_control(control, control.current, index + 1, true);
    }
}

#[test]
fn issue_3946_equivalence_controls_base() {
    let Some(dir) = std::env::var_os("ISSUE_3946_BASE_DIR") else {
        println!("ISSUE3946 P20 BASE_NOT_REQUESTED");
        return;
    };
    for (index, control) in CONTROLS.iter().enumerate() {
        let source = std::fs::read_to_string(Path::new(&dir).join(control.path))
            .expect("all pinned base sources must be exported");
        check_control(control, &source, index + 1, false);
    }
}
