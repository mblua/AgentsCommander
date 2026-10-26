//! Per-invocation room creation diagnostics. No transport or persistence lives here.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use uuid::Uuid;

const TARGET: &str = "agentscommander_lib::room_creation";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Origin {
    Cli,
    Gui,
    OnDisk,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Operation,
    Preflight,
    Physical,
    Prepare,
    TeamGuard,
    Allocate,
    Replicas,
    Replica,
    Repos,
    Repo,
    CloneProcess,
    ResetProcess,
    RefreshBackend,
    RefreshPublish,
    JsonOutput,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Ok,
    Partial,
    Error,
    Timeout,
    Skip,
    Interrupted,
}

impl Outcome {
    pub(crate) fn clones(count: usize) -> Self {
        if count == 0 {
            Self::Ok
        } else {
            Self::Partial
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    Failed,
    CloneTimeout,
    ResetTimeout,
    ResetFailed,
    DuplicateUrl,
    TargetExists,
    FutureDropped,
    Unwinding,
}

/// Only bounded, non-sensitive values can enter the wire record.
#[derive(Clone, Default, Serialize)]
pub(crate) struct Fields {
    #[serde(skip_serializing_if = "Option::is_none")]
    project_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    room_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clone_error_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_request_id: Option<Uuid>,
}

pub(crate) struct CreationTrace {
    id: Uuid,
    origin: Origin,
    started: Instant,
    next_stage: AtomicU64,
}

impl CreationTrace {
    pub(crate) fn new(origin: Origin) -> Self {
        Self {
            id: Uuid::new_v4(),
            origin,
            started: Instant::now(),
            next_stage: AtomicU64::new(1),
        }
    }

    pub(crate) fn operation(&self) -> StageGuard<'_> {
        self.stage(Stage::Operation, None, Fields::default(), true)
    }

    fn stage(
        &self,
        stage: Stage,
        parent: Option<u64>,
        fields: Fields,
        start: bool,
    ) -> StageGuard<'_> {
        let guard = StageGuard {
            trace: self,
            id: self.next_stage.fetch_add(1, Ordering::Relaxed),
            parent,
            stage,
            started: Instant::now(),
            fields,
            finished: false,
            partial: false,
        };
        if start {
            guard.emit("start", 0, None, None);
        }
        guard
    }
}

pub(crate) struct StageGuard<'a> {
    trace: &'a CreationTrace,
    id: u64,
    parent: Option<u64>,
    stage: Stage,
    started: Instant,
    fields: Fields,
    finished: bool,
    partial: bool,
}

impl<'a> StageGuard<'a> {
    pub(crate) fn child(&self, stage: Stage) -> StageGuard<'a> {
        self.trace
            .stage(stage, Some(self.id), self.fields.clone(), true)
    }

    pub(crate) fn indexed_child(&self, stage: Stage, index: usize) -> StageGuard<'a> {
        let mut fields = self.fields.clone();
        match stage {
            Stage::Replica => fields.agent_index = Some(index),
            Stage::Repo => fields.repo_index = Some(index),
            _ => unreachable!("only replica and repo stages have indices"),
        }
        self.trace.stage(stage, Some(self.id), fields, true)
    }

    pub(crate) fn skip_repo(&self, index: usize, reason: Reason) {
        let mut fields = self.fields.clone();
        fields.repo_index = Some(index);
        let mut guard = self.trace.stage(Stage::Repo, Some(self.id), fields, false);
        guard.emit("end", 0, Some(Outcome::Skip), Some(reason));
        guard.finished = true;
    }

    pub(crate) fn project(&mut self, path: &Path) {
        let digest = Sha256::digest(path.to_string_lossy().as_bytes());
        self.fields.project_key = Some(format!("{digest:x}"));
    }

    pub(crate) fn partial(&mut self) {
        self.partial = true;
    }

    pub(crate) fn room(&mut self, number: u32) {
        self.fields.room_number = Some(number);
    }

    pub(crate) fn count(&mut self, count: usize) {
        self.fields.count = Some(count);
    }

    pub(crate) fn clone_errors(&mut self, count: usize) {
        self.fields.clone_error_count = Some(count);
    }

    pub(crate) fn refresh_request(&mut self, id: Uuid) {
        self.fields.refresh_request_id = Some(id);
    }

    pub(crate) fn finish(&mut self, outcome: Outcome, reason: Option<Reason>) {
        if !self.finished {
            self.emit(
                "end",
                self.started.elapsed().as_millis(),
                Some(outcome),
                reason,
            );
            self.finished = true;
        }
    }

    pub(crate) fn result<T, E>(mut self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            let outcome = if self.partial {
                Outcome::Partial
            } else {
                Outcome::Ok
            };
            self.finish(outcome, None);
        } else {
            self.finish(Outcome::Error, Some(Reason::Failed));
        }
        result
    }

    pub(crate) fn run<T, E>(self, work: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        self.result(work())
    }

    pub(crate) async fn run_async<T, E>(
        self,
        work: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let result = work.await;
        self.result(result)
    }

    fn emit(
        &self,
        event: &str,
        elapsed_ms: u128,
        outcome: Option<Outcome>,
        reason: Option<Reason>,
    ) {
        #[derive(Serialize)]
        struct Record<'a> {
            v: u8,
            operation_id: Uuid,
            origin: Origin,
            stage: Stage,
            stage_id: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            parent_stage_id: Option<u64>,
            event: &'a str,
            elapsed_ms: u128,
            operation_ms: u128,
            #[serde(skip_serializing_if = "Option::is_none")]
            outcome: Option<Outcome>,
            #[serde(skip_serializing_if = "Option::is_none")]
            reason: Option<Reason>,
            #[serde(flatten)]
            fields: &'a Fields,
        }
        let record = Record {
            v: 1,
            operation_id: self.trace.id,
            origin: self.trace.origin,
            stage: self.stage,
            stage_id: self.id,
            parent_stage_id: self.parent,
            event,
            elapsed_ms,
            operation_ms: self.trace.started.elapsed().as_millis(),
            outcome,
            reason,
            fields: &self.fields,
        };
        // All strings are fixed vocabulary, a SHA256 or a UUID; no arbitrary error text.
        let json = serde_json::to_string(&record).expect("room creation record is serializable");
        match outcome {
            Some(Outcome::Error | Outcome::Timeout | Outcome::Interrupted) => {
                log::warn!(target: TARGET, "room_creation {}", json);
            }
            _ => log::info!(target: TARGET, "room_creation {}", json),
        }
    }
}

impl Drop for StageGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let reason = if std::thread::panicking() {
                Reason::Unwinding
            } else {
                Reason::FutureDropped
            };
            self.finish(Outcome::Interrupted, Some(reason));
        }
    }
}

#[cfg(test)]
mod tests;
