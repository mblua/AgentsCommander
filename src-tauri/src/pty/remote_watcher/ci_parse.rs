use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use chrono::DateTime;

use super::{
    build_gh_command_spec, run_query, CiState, FailureKind, GhQuery, GhSpawner, QueryKey,
    StalenessState,
};

/// The compare answer for one key: the staleness state, `behind_by`, `ahead_by`
/// and whether GitHub said `identical`. `ahead_by` is #2141's suppression input:
/// `Some(0)` means the branch carries no commit of its own, and `None` means the
/// field was absent or out of range, which fails open.
pub(crate) type CompareAnswer =
    Result<(StalenessState, Option<u32>, Option<u32>, bool), FailureKind>;

/// The CI answer for one branch: the filtered state plus whether that branch
/// has any run at all, which the identical-to-default rule reads. `run_ids`
/// and `pull_requests` (#2473) come from the kept rows that are not completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiAnswer {
    pub(crate) state: CiState,
    pub(crate) branch_has_runs: bool,
    pub(crate) observations: Vec<CiRunObservation>,
    pub(crate) run_ids: Vec<u64>,
    pub(crate) pull_requests: Vec<u64>,
}

pub(crate) const MAX_CI_KNOWN_RUNS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiRunObservation {
    pub(crate) id: u64,
    pub(crate) workflow_id: u64,
    pub(crate) run_attempt: u64,
    pub(crate) head_sha: String,
    pub(crate) head_branch: String,
    pub(crate) status: Option<String>,
    pub(crate) conclusion: Option<String>,
    pub(crate) updated_at: String,
    pub(crate) pull_requests: Vec<u64>,
}

impl CiRunObservation {
    pub(crate) fn completed(&self) -> bool {
        self.status.as_deref() == Some("completed")
    }
    pub(crate) fn proof(&self) -> CiCompletionProof {
        CiCompletionProof {
            id: self.id,
            workflow_id: self.workflow_id,
            run_attempt: self.run_attempt,
            updated_at: self.updated_at.clone(),
            conclusion: self.conclusion.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiCompletionProof {
    pub(crate) id: u64,
    pub(crate) workflow_id: u64,
    pub(crate) run_attempt: u64,
    pub(crate) updated_at: String,
    pub(crate) conclusion: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CiCompletionState {
    pub(crate) known: BTreeMap<u64, CiRunObservation>,
    pub(crate) proofs: BTreeMap<u64, CiCompletionProof>,
    pub(crate) closing_ready: bool,
    pub(crate) overflow: bool,
    pub(crate) active_ids: Vec<u64>,
    pub(crate) active_prs: Vec<u64>,
}

impl CiCompletionState {
    pub(crate) fn cancel_candidate(&mut self) {
        self.proofs.clear();
        self.closing_ready = false;
    }
    pub(crate) fn all_proven(&self) -> bool {
        !self.known.is_empty()
            && self
                .known
                .values()
                .all(|run| self.proofs.get(&run.id) == Some(&run.proof()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CiCompletionDecision {
    Accepted,
    Pending,
}

pub(crate) fn parse_ci_run_response(
    body: &str,
    key: &QueryKey,
) -> Result<CiRunObservation, FailureKind> {
    let row: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let positive = |name| {
        row.get(name)
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(FailureKind::Other)
    };
    let head_sha = row
        .get("head_sha")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    let head_branch = row
        .get("head_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    if head_sha != key.sha40 || head_branch != key.branch {
        return Err(FailureKind::Other);
    }
    let updated_at = row
        .get("updated_at")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    DateTime::parse_from_rfc3339(updated_at).map_err(|_| FailureKind::Other)?;
    let status = row
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let conclusion = row
        .get("conclusion")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let run = CiRunObservation {
        id: positive("id")?,
        workflow_id: positive("workflow_id")?,
        run_attempt: positive("run_attempt")?,
        head_sha: head_sha.to_string(),
        head_branch: head_branch.to_string(),
        status,
        conclusion,
        updated_at: updated_at.to_string(),
        pull_requests: row
            .get("pull_requests")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pr| pr.get("number").and_then(serde_json::Value::as_u64))
            .collect(),
    };
    if run.completed()
        && run
            .conclusion
            .as_deref()
            .is_none_or(|c| c.trim().is_empty())
    {
        return Err(FailureKind::Incomplete);
    }
    Ok(run)
}

pub(crate) fn parse_ci_response(
    body: &str,
    branch: &str,
    sha40: &str,
) -> Result<CiAnswer, FailureKind> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let total = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or(FailureKind::Other)?;
    let rows = value
        .get("workflow_runs")
        .and_then(serde_json::Value::as_array)
        .ok_or(FailureKind::Other)?;
    if rows.len() as u64 != total || rows.len() > MAX_CI_KNOWN_RUNS {
        return Err(FailureKind::Incomplete);
    }
    let key = QueryKey {
        nwo: String::new(),
        sha40: sha40.to_string(),
        branch: branch.to_string(),
    };
    let mut observations = Vec::new();
    let mut seen = HashSet::new();
    for row in rows
        .iter()
        .filter(|row| row.get("head_branch").and_then(serde_json::Value::as_str) == Some(branch))
    {
        let run = parse_ci_run_response(&row.to_string(), &key)?;
        if !seen.insert(run.id) {
            return Err(FailureKind::Other);
        }
        observations.push(run);
    }
    let active: Vec<_> = observations.iter().filter(|run| !run.completed()).collect();
    Ok(CiAnswer {
        state: if active.is_empty() {
            CiState::Idle
        } else {
            CiState::Running
        },
        branch_has_runs: !observations.is_empty(),
        run_ids: active.iter().map(|run| run.id).collect(),
        pull_requests: active
            .iter()
            .flat_map(|run| run.pull_requests.iter().copied())
            .collect(),
        observations,
    })
}

/// Latest attempt identities survive omissions and failures. Only positive evidence
/// replaces a high-water mark; proofs bind terminal metadata, not elapsed time.
pub(crate) fn merge_ci_observations(
    state: &mut CiCompletionState,
    observations: &[CiRunObservation],
    from_list: bool,
) -> Result<(), FailureKind> {
    let mut active_seen = false;
    let mut error = state.overflow.then_some(FailureKind::Incomplete);
    for run in observations {
        let mut observation = run.clone();
        if let Some(old) = state.known.get(&run.id) {
            let rejected = if old.workflow_id != run.workflow_id
                || old.head_sha != run.head_sha
                || old.head_branch != run.head_branch
            {
                Some(FailureKind::Other)
            } else if run.run_attempt < old.run_attempt {
                Some(FailureKind::Incomplete)
            } else {
                None
            };
            if let Some(kind) = rejected {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
                // Identity contradictions dominate stale attempts regardless of
                // row order. Absorb compatible rows even when this round fails.
                error = Some(match (error, kind) {
                    (Some(FailureKind::Other), _) | (_, FailureKind::Other) => FailureKind::Other,
                    _ => FailureKind::Incomplete,
                });
                continue;
            }
            // Detail endpoints corroborate completion; the last list supplies
            // the PR projection for a remembered run omitted by a later list.
            if !from_list {
                observation.pull_requests = old.pull_requests.clone();
            }
            if run.run_attempt > old.run_attempt {
                state.cancel_candidate();
            } else if old.proof() != run.proof() || !run.completed() {
                state.proofs.remove(&run.id);
                state.closing_ready = false;
            }
        } else {
            state.cancel_candidate();
            if state.known.len() >= MAX_CI_KNOWN_RUNS {
                state.overflow = true;
                error.get_or_insert(FailureKind::Incomplete);
                continue;
            }
        }
        state.known.insert(run.id, observation.clone());
        if !run.completed() {
            if !active_seen {
                state.active_ids.clear();
                state.active_prs.clear();
                active_seen = true;
            }
            state.active_ids.push(run.id);
            state
                .active_prs
                .extend(observation.pull_requests.iter().copied());
            state.cancel_candidate();
        }
    }
    error.map_or(Ok(()), Err)
}

pub(crate) fn parse_ci_jobs_response(
    body: &str,
    run: &CiRunObservation,
) -> Result<bool, FailureKind> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let total = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or(FailureKind::Other)?;
    let jobs = value
        .get("jobs")
        .and_then(serde_json::Value::as_array)
        .ok_or(FailureKind::Other)?;
    if jobs.len() as u64 != total || jobs.len() > 100 {
        return Err(FailureKind::Incomplete);
    }
    let mut seen = HashSet::new();
    let mut terminal = true;
    for job in jobs {
        let id = job
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or(FailureKind::Other)?;
        if !seen.insert(id)
            || job.get("run_id").and_then(serde_json::Value::as_u64) != Some(run.id)
            || job.get("head_sha").and_then(serde_json::Value::as_str)
                != Some(run.head_sha.as_str())
            || job
                .get("run_attempt")
                .is_some_and(|a| a.as_u64() != Some(run.run_attempt))
            || job
                .get("head_branch")
                .is_some_and(|b| b.as_str() != Some(run.head_branch.as_str()))
        {
            return Err(FailureKind::Other);
        }
        if job.get("status").and_then(serde_json::Value::as_str) != Some("completed") {
            terminal = false;
        } else if job
            .get("conclusion")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|c| c.trim().is_empty())
        {
            return Err(FailureKind::Incomplete);
        }
    }
    Ok(terminal)
}

pub(crate) async fn corroborate_ci_run(
    spawner: &GhSpawner,
    gh: &Path,
    key: &QueryKey,
    id: u64,
    state: &mut CiCompletionState,
    calls: &mut u32,
) -> Result<CiState, FailureKind> {
    let expected = state.known[&id].clone();
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiRun {
            nwo: &key.nwo,
            run_id: id,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let a = run_query(spawner, spec, |body| parse_ci_run_response(body, key)).await?;
    if a.id != id || a.workflow_id != expected.workflow_id {
        state.proofs.remove(&id);
        return Err(FailureKind::Other);
    }
    merge_ci_observations(state, std::slice::from_ref(&a), false)?;
    if !a.completed() {
        return Ok(CiState::Running);
    }
    if a.run_attempt > expected.run_attempt {
        return Ok(CiState::Idle);
    }
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiAttemptJobs {
            nwo: &key.nwo,
            run_id: id,
            attempt: a.run_attempt,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let terminal = run_query(spawner, spec, |body| parse_ci_jobs_response(body, &a)).await?;
    if !terminal {
        state.active_ids = vec![id];
        state.active_prs = state.known[&id].pull_requests.clone();
        state.cancel_candidate();
        return Ok(CiState::Running);
    }
    *calls += 1;
    let spec = build_gh_command_spec(
        gh,
        GhQuery::CiRun {
            nwo: &key.nwo,
            run_id: id,
        },
    )
    .map_err(|_| FailureKind::Other)?;
    let c = run_query(spawner, spec, |body| parse_ci_run_response(body, key)).await?;
    if c.id != id || c.workflow_id != expected.workflow_id {
        state.proofs.remove(&id);
        return Err(FailureKind::Other);
    }
    merge_ci_observations(state, std::slice::from_ref(&c), false)?;
    if !c.completed() {
        return Ok(CiState::Running);
    }
    if a.proof() == c.proof() {
        state.proofs.insert(id, c.proof());
    }
    Ok(CiState::Idle)
}

/// The staleness mapping is unchanged; the extra flag reports whether GitHub
/// said `identical`, which is the same statement the CI suppression rule needs.
/// `ahead_by` is read with the same shape as `behind_by` and is NOT part of the
/// classification: an absent or out-of-range value is `None`, never an error,
/// because #2141 fails open on it.
pub(crate) fn parse_compare_response(body: &str) -> CompareAnswer {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| FailureKind::Other)?;
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .ok_or(FailureKind::Other)?;
    let behind_by = value
        .get("behind_by")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let ahead_by = value
        .get("ahead_by")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let identical = status == "identical";
    match (status, behind_by) {
        ("identical" | "ahead", Some(0)) => {
            Ok((StalenessState::Current, None, ahead_by, identical))
        }
        ("behind" | "diverged", Some(behind)) if behind > 0 => {
            Ok((StalenessState::Stale, Some(behind), ahead_by, identical))
        }
        _ => Err(FailureKind::Other),
    }
}

/// `github.com` only, in both SSH and HTTPS spellings, with a trailing `.git`
/// stripped. Anything else yields no key and the path reads `Unknown`.
pub(crate) fn parse_github_nwo(origin: &str) -> Option<String> {
    const HOST: &str = "github.com";
    let origin = origin.trim();
    if origin.is_empty() {
        return None;
    }

    let (host, path) = if let Some((_scheme, rest)) = origin.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        rest.split_once('/')?
    } else if let Some((_user, rest)) = origin.split_once('@') {
        rest.split_once(':')?
    } else {
        return None;
    };

    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }

    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}
