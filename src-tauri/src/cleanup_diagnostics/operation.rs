//! Owns one synchronous evidence protocol; borrows no product resource.

use std::collections::BTreeMap;
use std::sync::Mutex;
use uuid::Uuid;

use super::schema::{
    CurrentIdentity, EvidenceError, EvidenceEvent, EvidenceRecord, ExitObservation, RawApiResult,
    SelectionRow, Summary,
};

/// The store adapter alone serializes records and owns files/quota.
pub(crate) trait EvidenceWriter: Send {
    fn write(&mut self, record: &EvidenceRecord) -> Result<(), EvidenceError>;
    fn flush_sync(&mut self) -> Result<(), EvidenceError>;
    /// Called only after the complete footer has been synced successfully.
    fn complete(&mut self) -> Result<(), EvidenceError>;
}

/// A non-cloneable evidence correlation, not permission to execute a kill.
#[derive(Debug)]
pub(crate) struct CallTicket {
    op_id: Uuid,
    index: usize,
    seq: u64,
}

struct CallState {
    seq: u64,
    returned: Option<RawApiResult>,
    observed: bool,
}

struct State {
    writer: Box<dyn EvidenceWriter>,
    seq: u64,
    selected: Option<Vec<SelectionRow>>,
    calls: BTreeMap<usize, CallState>,
    effects_started: bool,
    gap: bool,
    finished: bool,
}

pub(crate) struct Operation {
    op_id: Uuid,
    state: Mutex<State>,
}

impl State {
    fn error(&self, stage: &str, message: impl ToString) -> EvidenceError {
        let mut error = EvidenceError::new(stage, message);
        error.effects_started = self.effects_started;
        error
    }

    fn write_record(&mut self, stage: &str, event: EvidenceEvent) -> Result<u64, EvidenceError> {
        if self.finished || (self.gap && !matches!(event, EvidenceEvent::OperationComplete { .. }))
        {
            return Err(self.error(stage, "operation is finished or has an evidence gap"));
        }
        self.seq += 1;
        let result = self.writer.write(&EvidenceRecord {
            seq: self.seq,
            event,
        });
        self.check(stage, result)?;
        Ok(self.seq)
    }

    fn check<T>(
        &mut self,
        stage: &str,
        result: Result<T, EvidenceError>,
    ) -> Result<T, EvidenceError> {
        result.map_err(|mut error| {
            self.gap = true;
            error.stage = stage.into();
            error.effects_started |= self.effects_started;
            error
        })
    }

    fn sync(&mut self, stage: &str) -> Result<(), EvidenceError> {
        let result = self.writer.flush_sync();
        self.check(stage, result)
    }

    fn ticket(&self, op_id: Uuid, ticket: &CallTicket) -> Result<(), EvidenceError> {
        if ticket.op_id != op_id
            || !self
                .calls
                .get(&ticket.index)
                .is_some_and(|call| call.seq == ticket.seq)
        {
            return Err(self.error("ticket", "ticket does not belong to this operation/call"));
        }
        Ok(())
    }
}

impl Operation {
    pub(crate) fn new(op_id: Uuid, writer: Box<dyn EvidenceWriter>) -> Result<Self, EvidenceError> {
        let operation = Self {
            op_id,
            state: Mutex::new(State {
                writer,
                seq: 0,
                selected: None,
                calls: BTreeMap::new(),
                effects_started: false,
                gap: false,
                finished: false,
            }),
        };
        {
            let mut state = operation.lock()?;
            state.write_record("header", EvidenceEvent::OperationBegin)?;
            state.sync("header")?;
        }
        Ok(operation)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, EvidenceError> {
        self.state.lock().map_err(|error| {
            let mut failure = EvidenceError::new("operation_lock", error);
            failure.effects_started = true;
            failure
        })
    }

    pub(crate) fn id(&self) -> Uuid {
        self.op_id
    }

    pub(crate) fn selection(&self, rows: &[SelectionRow]) -> Result<(), EvidenceError> {
        let mut state = self.lock()?;
        if state.selected.is_some() || rows.iter().enumerate().any(|(i, row)| i != row.index) {
            return Err(state.error("selection", "duplicate selection or unordered indexes"));
        }
        for row in rows {
            state.write_record(
                "selection",
                EvidenceEvent::Target {
                    row: Box::new(row.clone()),
                },
            )?;
        }
        state.write_record(
            "selection",
            EvidenceEvent::SelectionComplete { count: rows.len() },
        )?;
        state.sync("selection")?;
        state.selected = Some(rows.to_vec());
        Ok(())
    }

    pub(crate) fn prepare(
        &self,
        row: &SelectionRow,
        precheck: CurrentIdentity,
        final_handle: CurrentIdentity,
    ) -> Result<CallTicket, EvidenceError> {
        let mut state = self.lock()?;
        let matches = state.selected.as_ref().and_then(|rows| rows.get(row.index)) == Some(row);
        if !matches || state.calls.contains_key(&row.index) {
            return Err(state.error("pre_call", "row is unselected or already prepared"));
        }
        let seq = state.write_record(
            "pre_call",
            EvidenceEvent::PreCall {
                index: row.index,
                precheck,
                final_handle,
            },
        )?;
        state.sync("pre_call")?;
        state.calls.insert(
            row.index,
            CallState {
                seq,
                returned: None,
                observed: false,
            },
        );
        state.effects_started = true;
        Ok(CallTicket {
            op_id: self.op_id,
            index: row.index,
            seq,
        })
    }

    pub(crate) fn returned(
        &self,
        ticket: &CallTicket,
        raw_result: RawApiResult,
    ) -> Result<(), EvidenceError> {
        let mut state = self.lock()?;
        state.ticket(self.op_id, ticket)?;
        if state.calls[&ticket.index].returned.is_some() {
            return Err(state.error("api_return", "return was already recorded"));
        }
        if let Some(call) = state.calls.get_mut(&ticket.index) {
            call.returned = Some(raw_result.clone());
        }
        let result = state
            .write_record(
                "api_return",
                EvidenceEvent::ApiReturn {
                    index: ticket.index,
                    result: raw_result.clone(),
                },
            )
            .and_then(|_| state.sync("api_return"));
        result.map_err(|mut error| {
            error.known_api_return = Some(raw_result);
            error
        })?;
        Ok(())
    }

    pub(crate) fn observed(
        &self,
        ticket: &CallTicket,
        exit: ExitObservation,
    ) -> Result<(), EvidenceError> {
        let mut state = self.lock()?;
        state.ticket(self.op_id, ticket)?;
        let call = &state.calls[&ticket.index];
        if call.returned.is_none() || call.observed {
            return Err(state.error("exit_observed", "missing return or duplicate observation"));
        }
        let known_api_return = call.returned.clone();
        let result = state
            .write_record(
                "exit_observed",
                EvidenceEvent::ExitObserved {
                    index: ticket.index,
                    exit: exit.clone(),
                },
            )
            .and_then(|_| state.sync("exit_observed"));
        result.map_err(|mut error| {
            error.known_exit = Some(exit);
            error.known_api_return = known_api_return;
            error
        })?;
        if let Some(call) = state.calls.get_mut(&ticket.index) {
            call.observed = true;
        }
        Ok(())
    }

    pub(crate) fn finish(&self, mut summary: Summary) -> Result<(), EvidenceError> {
        let mut state = self.lock()?;
        if state.selected.is_none() {
            summary.partial = true;
            summary.evidence_gap = true;
            summary.errors.push("selection_incomplete".into());
        }
        summary.accounting = "proposed".into();
        summary.evidence_gap |= state.gap;
        summary.partial |= summary.evidence_gap;
        state.write_record("footer", EvidenceEvent::OperationComplete { summary })?;
        state.sync("footer")?;
        let result = state.writer.complete();
        state.check("footer", result)?;
        state.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
