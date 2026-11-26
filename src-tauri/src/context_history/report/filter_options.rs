//! Values the report filters can offer: every distinct team id and launch
//! value of the given sessions (the caller passes the unfiltered and the
//! shown ones). A null launch value is the `Unknown` option and is never
//! listed.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::context_history::HistoryRecord;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterOptions {
    pub(crate) team_ids: Vec<String>,
    pub(crate) coding_agent_ids: Vec<String>,
    pub(crate) models: Vec<String>,
    pub(crate) efforts: Vec<String>,
}

impl FilterOptions {
    /// Distinct values of each session's first record, in byte order.
    pub(crate) fn from_sessions<'a>(firsts: impl IntoIterator<Item = &'a HistoryRecord>) -> Self {
        let mut teams = BTreeSet::new();
        let mut agents = BTreeSet::new();
        let mut models = BTreeSet::new();
        let mut efforts = BTreeSet::new();
        for first in firsts {
            teams.extend(first.team_ids.iter().map(String::as_str));
            agents.extend(first.coding_agent_id.as_deref());
            models.extend(first.model.as_deref());
            efforts.extend(first.effort.as_deref());
        }
        Self {
            team_ids: owned(teams),
            coding_agent_ids: owned(agents),
            models: owned(models),
            efforts: owned(efforts),
        }
    }
}

fn owned(values: BTreeSet<&str>) -> Vec<String> {
    values.into_iter().map(str::to_string).collect()
}
