//! Loop change data and its synchronous delivery port; excludes transport and disk access.

use std::path::Path;

use serde::Serialize;

use crate::config::loops::AcLoopSummary;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopEventPayload {
    pub kind: String,
    pub project_path: String,
    pub loop_id: String,
    pub summary: Option<AcLoopSummary>,
    pub message: Option<String>,
}

pub struct LoopChange<'a> {
    pub project_path: &'a Path,
    pub changed_path: &'a Path,
    pub loop_id: &'a str,
    pub kind: &'a str,
    pub summary: Option<AcLoopSummary>,
    pub message: Option<String>,
}

pub trait LoopEvents {
    fn loop_changed(&self, change: LoopChange<'_>);
}

pub fn emit_loop_change<E: LoopEvents + ?Sized>(
    events: &E,
    project_path: &Path,
    changed_path: &Path,
    loop_id: &str,
    kind: &str,
    summary: Option<AcLoopSummary>,
    message: Option<String>,
) {
    events.loop_changed(LoopChange {
        project_path,
        changed_path,
        loop_id,
        kind,
        summary,
        message,
    });
}

#[cfg(test)]
mod tests;
