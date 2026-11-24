//! #4845 P14 - the agent values each running non-30 launch used, kept in memory and keyed
//! by agent id. Pure and generic (P01 layering rule: std only).

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

pub(crate) struct LaunchedRegistry<T: Clone> {
    map: Mutex<BTreeMap<String, T>>,
}

impl<T: Clone> LaunchedRegistry<T> {
    pub(crate) const fn new() -> Self {
        Self {
            map: Mutex::new(BTreeMap::new()),
        }
    }

    /// `Some` records the launch values of `agent_id`; `None` removes them.
    pub(crate) fn record(&self, agent_id: &str, value: Option<T>) {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        match value {
            Some(value) => {
                map.insert(agent_id.to_string(), value);
            }
            None => {
                map.remove(agent_id);
            }
        }
    }

    /// The recorded entries in id order.
    pub(crate) fn snapshot(&self) -> Vec<(String, T)> {
        let map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}
