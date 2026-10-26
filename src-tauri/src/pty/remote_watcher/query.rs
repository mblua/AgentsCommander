use std::collections::HashMap;
use std::path::Path;

use super::gh_command::git_command;
use crate::process::github::{self, QueryError};

use super::{
    build_gh_command_spec, corroborate_ci_run, failure_kind, merge_ci_observations,
    parse_ci_response, parse_compare_response, CiCompletionDecision, CiCompletionState, CiState,
    FailureKind, GhCallOutput, GhCommandSpec, GhQuery, GhSpawner, KeyOutcome, KeyPlan, QueryKey,
    CALL_TIMEOUT,
};

pub(crate) fn room_map() -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (repo_path, room_dir) in crate::pty::git_watcher::discovery_repo_rooms() {
        let rooms = map.entry(repo_path).or_default();
        if !rooms.contains(&room_dir) {
            rooms.push(room_dir);
        }
    }
    map
}

pub(crate) async fn query_key(
    spawner: &GhSpawner,
    gh: &Path,
    key: &QueryKey,
    plan: KeyPlan,
    default_branch: Option<&str>,
    input: (CiCompletionState, Option<CiState>),
    reserved: bool,
) -> KeyOutcome {
    let (mut completion, confirmed) = input;
    let mut outcome = KeyOutcome::default();

    // The compare runs FIRST when the staleness axis also needs it, so the
    // identical-to-default decision below reuses this round's one call.
    if plan.staleness {
        outcome.staleness = Some(
            match build_gh_command_spec(
                gh,
                GhQuery::Compare {
                    nwo: &key.nwo,
                    sha40: &key.sha40,
                },
            ) {
                Ok(spec) => run_query(spawner, spec, parse_compare_response).await,
                Err(_) => Err(FailureKind::Other),
            },
        );
    }

    if !plan.ci {
        return outcome;
    }

    let ci = match build_gh_command_spec(
        gh,
        GhQuery::Ci {
            nwo: &key.nwo,
            sha40: &key.sha40,
        },
    ) {
        Ok(spec) => {
            let branch = key.branch.clone();
            run_query(spawner, spec, |body| {
                parse_ci_response(body, &branch, &key.sha40)
            })
            .await
        }
        Err(_) => Err(FailureKind::Other),
    };
    let closing_ready = completion.closing_ready;
    let merge_error = ci.as_ref().ok().and_then(|answer| {
        merge_ci_observations(&mut completion, &answer.observations, true).err()
    });
    let scripted_staleness = outcome.staleness;
    let on_default_branch = default_branch == Some(key.branch.as_str());
    outcome.ci_suppressed = on_default_branch;
    let mut ci_result = match ci {
        // A resolved default branch is suppressed after the branch-filtered
        // query and without any identity question: `Ok` publishes `Idle`,
        // while an `Err` falls through to the failure arm below so the round
        // still backs off, warns and can arm its gate.
        Ok(_) if on_default_branch => Ok(CiState::Idle),
        // Only a non-default branch that HAS runs needs the identity
        // question; a branch with no runs answers `Idle` without it.
        Ok(answer)
            if answer.branch_has_runs
                && default_branch.is_some_and(|default| key.branch != default) =>
        {
            let compare = match scripted_staleness {
                Some(result) => result,
                None => {
                    // The ONLY place an extra `gh` call is issued for the
                    // identity question, and so the only place the round's
                    // reservation for it is consumed.
                    outcome.ci_identity_call = true;
                    match build_gh_command_spec(
                        gh,
                        GhQuery::Compare {
                            nwo: &key.nwo,
                            sha40: &key.sha40,
                        },
                    ) {
                        Ok(spec) => run_query(spawner, spec, parse_compare_response).await,
                        Err(_) => Err(FailureKind::Other),
                    }
                }
            };
            match compare {
                Ok((_, _, _, true)) => {
                    outcome.ci_suppressed = true;
                    Ok(CiState::Idle)
                }
                Ok((_, _, _, false)) => {
                    outcome.ci_runs = (answer.run_ids, answer.pull_requests);
                    Ok(answer.state)
                }
                Err(kind) => Err(kind),
            }
        }
        Ok(answer) => {
            outcome.ci_runs = (answer.run_ids, answer.pull_requests);
            Ok(answer.state)
        }
        Err(kind) => Err(kind),
    };
    if outcome.ci_suppressed {
        outcome.ci = Some(ci_result);
        return outcome;
    }
    if let Some(kind) = merge_error.filter(|_| ci_result.is_ok()) {
        ci_result = Err(kind);
    }
    if let Some(Err(kind)) = outcome
        .staleness
        .filter(|_| confirmed == Some(CiState::Running) && ci_result == Ok(CiState::Idle))
    {
        ci_result = Err(kind);
    }
    if ci_result == Ok(CiState::Idle) && confirmed == Some(CiState::Running) {
        outcome.ci_decision = Some(CiCompletionDecision::Pending);
        if closing_ready && completion.closing_ready && completion.all_proven() {
            outcome.ci_decision = Some(CiCompletionDecision::Accepted);
        } else {
            let unproven = completion
                .known
                .values()
                .find(|run| completion.proofs.get(&run.id) != Some(&run.proof()))
                .map(|run| run.id);
            if let Some(id) = unproven.filter(|_| reserved) {
                ci_result = corroborate_ci_run(
                    spawner,
                    gh,
                    key,
                    id,
                    &mut completion,
                    &mut outcome.ci_corroboration_calls,
                )
                .await;
                if ci_result == Ok(CiState::Running) {
                    outcome.ci_decision = Some(CiCompletionDecision::Accepted);
                }
            }
            completion.closing_ready = completion.all_proven();
        }
    } else if ci_result.is_ok() {
        outcome.ci_decision = Some(CiCompletionDecision::Accepted);
    }
    if ci_result.is_err() || outcome.staleness.is_some_and(|result| result.is_err()) {
        completion.closing_ready = false;
    }
    if ci_result == Ok(CiState::Running) {
        outcome.ci_runs = (completion.active_ids.clone(), completion.active_prs.clone());
    }
    outcome.ci_completion = Some(completion);

    outcome.ci = Some(ci_result);

    outcome
}

pub(crate) async fn run_query<T, P>(
    spawner: &GhSpawner,
    spec: GhCommandSpec,
    parse: P,
) -> Result<T, FailureKind>
where
    P: FnOnce(&str) -> Result<T, FailureKind>,
{
    match (spawner)(spec).await {
        Ok(output) if output.success => parse(&output.stdout),
        Ok(output) => Err(failure_kind(&output)),
        Err(kind) => Err(kind),
    }
}

/// The production spawner. `CALL_TIMEOUT` plus `kill_on_drop(true)` is the same
/// mechanism as INV-1: the timeout drops `output()`, and the drop kills `gh.exe`.
pub(crate) async fn spawn_gh(spec: GhCommandSpec) -> Result<GhCallOutput, FailureKind> {
    match github::query(spec, CALL_TIMEOUT).await {
        Ok(output) => Ok(GhCallOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
        }),
        Err(QueryError::Spawn(error)) => {
            let _ = error;
            Err(FailureKind::Other)
        }
        Err(QueryError::Timeout) => Err(FailureKind::Timeout),
    }
}

pub(crate) async fn run_local_git(path: String, args: Vec<String>) -> Result<String, String> {
    let mut command = git_command(&path, &args);
    match tokio::time::timeout(CALL_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Ok(output)) => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("timeout".to_string()),
    }
}
