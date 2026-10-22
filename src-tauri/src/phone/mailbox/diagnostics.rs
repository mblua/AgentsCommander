// Only primitive/static/UUID diagnostics; expressions remain lazy when disabled.
macro_rules! response_diag {
    ($stage:literal, $outcome:expr, $id:expr) => {
        if log::log_enabled!(
            target: "agentscommander_lib::response_close_diag",
            log::Level::Debug
        ) {
            if let Some(id) = $id {
                log::debug!(
                    target: "agentscommander_lib::response_close_diag",
                    "response_close_diag stage={} outcome={} site={} id={}",
                    $stage, $outcome, line!(), id
                );
            } else {
                log::debug!(
                    target: "agentscommander_lib::response_close_diag",
                    "response_close_diag stage={} outcome={} site={} id=unknown",
                    $stage, $outcome, line!()
                );
            }
        }
    };
}

macro_rules! response_diag_result {
    ($value:expr, $stage:literal, $id:expr) => {{
        let result = $value;
        if log::log_enabled!(
            target: "agentscommander_lib::response_close_diag",
            log::Level::Debug
        ) {
            let outcome = match &result {
                Ok(_) => "ok",
                Err(crate::pty::manager::ResponseCloseError::Unavailable) => "unavailable",
                Err(crate::pty::manager::ResponseCloseError::Unknown) => "unknown",
                Err(crate::pty::manager::ResponseCloseError::Conflict) => "conflict",
                Err(crate::pty::manager::ResponseCloseError::ClosingDeferred) => "closing_deferred",
            };
            response_diag!($stage, outcome, $id);
        }
        result
    }};
}

macro_rules! response_diag_local {
    ($stage:literal, $outcome:expr, $id:expr) => {
        crate::phone::mailbox::pty_claim::response_diag_emit_lazy(
            $stage,
            || $outcome,
            || $id,
            line!(),
        )
    };
}

macro_rules! response_diag_option_local {
    ($value:expr, $stage:literal, $outcome:literal, $id:expr) => {
        crate::phone::mailbox::pty_claim::response_diag_option_lazy(
            $value,
            $stage,
            $outcome,
            || $id,
            line!(),
        )
    };
}

macro_rules! response_diag_result_local {
    ($value:expr, $stage:literal, $id:expr) => {{
        let result = $value;
        crate::phone::mailbox::pty_claim::response_diag_result_ref(
            &result,
            $stage,
            || $id,
            line!(),
        );
        result
    }};
}
