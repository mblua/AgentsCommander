use std::time::Duration;

#[cfg(windows)]
use crate::process::host::job::JobObject;
use crate::process::host::job::SettlementAttempt;

const NATIVE_PROOF_POLL: Duration = Duration::from_millis(25);

#[cfg(test)]
pub(crate) trait NativeProofHooks: Send {
    fn next_observation(&mut self) -> Option<Result<bool, String>>;
    fn after_query_defect(&mut self);
}

#[cfg(windows)]
pub(crate) async fn settle_job(
    job: Option<&JobObject>,
    defects: &mut Vec<String>,
    attempt: &mut SettlementAttempt,
    record_expiry: fn(&mut Vec<String>, &str),
    #[cfg(test)] hook: Option<&mut dyn NativeProofHooks>,
) {
    let Some(job) = job else {
        return;
    };
    settle(
        || {
            job.active_processes()
                .map(|active| active == 0)
                .map_err(|error| error.to_string())
        },
        || {
            let _ = job.terminate_checked();
        },
        ("job accounting failed", "job accounting proof"),
        defects,
        attempt,
        record_expiry,
        #[cfg(test)]
        hook,
    )
    .await;
}

#[cfg(unix)]
pub(crate) async fn settle_process_group(
    pgid: i32,
    defects: &mut Vec<String>,
    attempt: &mut SettlementAttempt,
    record_expiry: fn(&mut Vec<String>, &str),
    #[cfg(test)] hook: Option<&mut dyn NativeProofHooks>,
) {
    if pgid <= 0 {
        return;
    }
    settle(
        move || {
            // SAFETY: the pgid <= 0 guard above leaves a positive i32, so negation cannot
            // overflow. kill accepts the negative group selector and signal 0 without pointers.
            let result = unsafe { libc::kill(-pgid, 0) };
            if result == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(true)
                } else {
                    Err(error.to_string())
                }
            } else {
                Ok(false)
            }
        },
        move || {
            // SAFETY: the pgid <= 0 guard above leaves a positive i32, so -pgid is a
            // representable group selector. kill accepts SIGKILL and takes no memory pointers.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        },
        (
            "process-group accounting failed",
            "process-group accounting proof",
        ),
        defects,
        attempt,
        record_expiry,
        #[cfg(test)]
        hook,
    )
    .await;
}

async fn settle<Q, T>(
    mut query: Q,
    mut terminate: T,
    (defect_prefix, stage): (&str, &str),
    defects: &mut Vec<String>,
    attempt: &mut SettlementAttempt,
    record_expiry: fn(&mut Vec<String>, &str),
    #[cfg(test)] mut hook: Option<&mut dyn NativeProofHooks>,
) where
    Q: FnMut() -> Result<bool, String> + Send,
    T: FnMut() + Send,
{
    let mut query_defect_recorded = false;
    loop {
        #[cfg(test)]
        let proof = match hook.as_mut().and_then(|hook| hook.next_observation()) {
            Some(observation) => observation,
            None => query(),
        };
        #[cfg(not(test))]
        let proof = query();
        match proof {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => {
                if !query_defect_recorded {
                    defects.push(format!("{defect_prefix}: {error}"));
                    query_defect_recorded = true;
                }
                #[cfg(test)]
                if let Some(hook) = hook.as_mut() {
                    hook.after_query_defect();
                }
            }
        }
        if attempt.expired() {
            record_expiry(defects, stage);
            terminate();
            attempt.restart();
        }
        tokio::time::sleep(NATIVE_PROOF_POLL.min(attempt.remaining())).await;
    }
}
