//! Pure selection facts; includes known edges, never inferred ownership.

use std::collections::{BTreeMap, BTreeSet};

use crate::cleanup_diagnostics::schema;
use crate::resource_monitor::types::{ObservedProcess, ProcessIdentity};

pub(crate) struct Selection {
    pub(crate) targets: Vec<ObservedProcess>,
    pub(crate) rows: Vec<schema::SelectionRow>,
}

pub(crate) fn identity(value: ProcessIdentity) -> schema::ProcessIdentity {
    schema::ProcessIdentity {
        pid: value.pid,
        start_time: value.creation_time_100ns,
    }
}

fn observed(process: &ObservedProcess, inputs: &[ObservedProcess]) -> schema::ObservedProcess {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::from([process.identity]);
    let mut parent = process.parent_identity;
    while let Some(next) = parent {
        let Some(entry) = inputs.iter().find(|entry| entry.identity == next) else {
            break;
        };
        if !seen.insert(next) {
            break;
        }
        chain.push(identity(next));
        parent = entry.parent_identity;
    }
    schema::ObservedProcess {
        identity: identity(process.identity),
        parent_pid: process.parent_pid,
        parent_identity: process.parent_identity.map(identity),
        executable: (!process.exe_name.is_empty()).then(|| process.exe_name.clone()),
        depth: process.depth,
        kill_allowed: process.kill_allowed,
        sample_id: None,
        chain,
    }
}

pub(crate) fn select_targets(retained: &[ObservedProcess], fresh: &[ObservedProcess]) -> Selection {
    let mut unique = BTreeMap::new();
    for process in retained.iter().chain(fresh) {
        unique.insert(process.identity, process.clone());
    }
    let mut targets: Vec<_> = unique.into_values().collect();
    targets.sort_by(|a, b| {
        b.depth
            .cmp(&a.depth)
            .then_with(|| b.identity.cmp(&a.identity))
    });
    let rows = targets
        .iter()
        .enumerate()
        .map(|(index, process)| {
            let retained = retained
                .iter()
                .rev()
                .find(|p| p.identity == process.identity)
                .map(|p| observed(p, retained));
            let fresh = fresh
                .iter()
                .rev()
                .find(|p| p.identity == process.identity)
                .map(|p| observed(p, fresh));
            let provenance = match (retained.is_some(), fresh.is_some()) {
                (true, true) => schema::Provenance::Both,
                (true, false) => schema::Provenance::Retained,
                _ => schema::Provenance::Fresh,
            };
            schema::SelectionRow {
                index,
                expected: identity(process.identity),
                retained,
                fresh,
                provenance,
                rule: "fresh_wins_depth_desc_identity_desc".into(),
                current: schema::CurrentIdentity::unknown(),
                matches_app: None,
                matches_other_roots: Vec::new(),
            }
        })
        .collect();
    Selection { targets, rows }
}

#[cfg(test)]
mod tests;
