//! Codex turn-boundary parsing and assembly (#2232 phase 5).
//!
//! A Codex rollout persists a turn's boundary as `event_msg` records:
//! `task_started` opens a turn and `task_complete` (or its `turn_complete`
//! alias) closes it, carrying the flat
//! `turn_id` and the provider's `last_agent_message`. The assistant prose of
//! the same turn lives in one or more `final_answer` records whose `turn_id` is
//! nested at `payload.internal_chat_message_metadata_passthrough.turn_id`
//! ([`crate::telegram::codex_watcher`] reads that location; this module reads
//! the flat one). Confusing the two makes grouping never match anything, which
//! is why the parse below only ever looks where the plan says the field is.
//!
//! This module owns the state machine that groups the records by the provider's
//! identifier, assembles the turn's prose in file order and checks it against
//! `last_agent_message` on **normalised** text. It is deliberately free of I/O
//! and of the record/sink types: it returns an [`AssembledTurn`] and the
//! watcher builds the `CapturedRecord`, so this module stays a leaf with no
//! path into the 88-module SCC (plan section 6).
//!
//! Delivery is never gated here: a `final_answer` with no turn identifier never
//! enters this module at all, and the watcher emits it immediately as its own
//! candidate.

use std::collections::{HashSet, VecDeque};

/// The open-turn bound of plan section 5.2: at most eight open turns are kept,
/// the oldest evicted with a counted log line.
pub const MAX_OPEN_TURNS: usize = 8;

/// One well-formed closure record from the rollout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClosureRecord {
    /// A new turn opened. Carries the provider's flat `turn_id`.
    TaskStarted { turn_id: String },
    /// A turn closed. `last_agent_message` is the provider's own idea of the
    /// turn's message; it is the completeness comparator, not the emitted text.
    TaskComplete {
        turn_id: String,
        last_agent_message: Option<String>,
    },
}

/// The verdict of parsing one rollout line as a closure record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClosureParse {
    /// A usable `task_started` / `task_complete`.
    Closure(ClosureRecord),
    /// An `event_msg` that announces a closure but carries no usable flat
    /// `turn_id`. Skipped and counted, never fatal.
    Malformed,
    /// Anything else: final answers, tool traffic, session metadata, malformed
    /// JSON. Indistinguishable from "not a closure", so it is not counted.
    NotClosure,
}

/// Parse one rollout line as a closure record.
///
/// Accepts exactly `type=event_msg` with `payload.type` `task_started`,
/// `task_complete` or its `turn_complete` alias (parsed as `TaskComplete`).
/// The `turn_id` is read **flat from the payload** and must be
/// a non-empty string; a closure without one is [`ClosureParse::Malformed`]
/// rather than a silent no-op. `last_agent_message` is optional by design: its
/// absence is what makes the completeness check abstain with a reason.
pub fn parse_closure_record(line: &str) -> ClosureParse {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return ClosureParse::NotClosure;
    };
    if value.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
        return ClosureParse::NotClosure;
    }
    let Some(payload) = value.get("payload").and_then(|p| p.as_object()) else {
        return ClosureParse::NotClosure;
    };
    let Some(payload_type) = payload.get("type").and_then(|t| t.as_str()) else {
        return ClosureParse::NotClosure;
    };
    let turn_id = payload
        .get("turn_id")
        .and_then(|t| t.as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    match payload_type {
        "task_started" => match turn_id {
            Some(turn_id) => ClosureParse::Closure(ClosureRecord::TaskStarted { turn_id }),
            None => ClosureParse::Malformed,
        },
        "task_complete" | "turn_complete" => match turn_id {
            Some(turn_id) => {
                let last_agent_message = payload
                    .get("last_agent_message")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned);
                ClosureParse::Closure(ClosureRecord::TaskComplete {
                    turn_id,
                    last_agent_message,
                })
            }
            None => ClosureParse::Malformed,
        },
        _ => ClosureParse::NotClosure,
    }
}

/// One accumulated `final_answer` fragment of an open turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnFragment {
    /// The extractor's trimmed prose.
    pub text: String,
    /// The raw line's absolute file offset, so the assembled candidate can
    /// carry the **first** contributing record's start.
    pub record_start: Option<u64>,
}

/// One whole, completeness-checked turn, ready to become one candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledTurn {
    pub turn_id: String,
    /// Fragments joined with `\n`, ends trimmed. This — not
    /// `last_agent_message` — is the candidate text (plan section 5.2).
    pub text: String,
    pub first_record_start: Option<u64>,
}

/// Normalise text for the completeness comparison: unify newline kinds and trim
/// both ends. Byte comparison is deliberately **not** used: the provider's
/// `last_agent_message` may differ only in `\r\n` vs `\n` and outer
/// whitespace, and treating that as a mismatch would turn the check into a new
/// source of needless abstentions.
pub fn normalise_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// Why a turn could not be emitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbstainReason {
    /// The `task_complete` matched no accumulated `final_answer`.
    NoAccumulatedFinalAnswer,
    /// `last_agent_message` is absent, so the completeness check cannot run.
    MissingLastAgentMessage,
    /// The assembly differs from `last_agent_message` on normalised text.
    CompletenessMismatch,
    /// A whole candidate already left this turn; nothing may leave it twice.
    AlreadyRouted,
    /// A newer `task_started` already superseded this turn; it is never routed.
    Superseded,
}

impl AbstainReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAccumulatedFinalAnswer => "no-accumulated-final-answer",
            Self::MissingLastAgentMessage => "missing-last-agent-message",
            Self::CompletenessMismatch => "completeness-mismatch",
            Self::AlreadyRouted => "already-routed",
            Self::Superseded => "superseded",
        }
    }
}

/// What the accumulator decided for one input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnEvent {
    /// Build and deliver one `CapturedRecord` for this whole turn.
    Emit(AssembledTurn),
    Abstain {
        turn_id: String,
        reason: AbstainReason,
    },
    /// A closed turn was cut off by a newer `task_started`; nothing is routed.
    Superseded { turn_id: String },
    /// The open-turn bound evicted the oldest entry; a dropped turn is never
    /// partially emitted.
    Evicted { turn_id: String },
    /// A `final_answer` arrived after its turn already emitted; ignored so the
    /// turn yields exactly one candidate.
    LateAfterRoute { turn_id: String },
    /// A `final_answer` for a turn that a newer `task_started` already
    /// superseded; ignored, never routed.
    LateAfterSuperseded { turn_id: String },
    /// An `event_msg` closure record could not be parsed; skipped and counted.
    MalformedClosure,
}

/// Running counts for the "counted log line" the plan asks for. The watcher
/// renders these; the accumulator owns them so reasons and counts cannot drift.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TurnCounters {
    pub emitted: u64,
    pub abstained: u64,
    pub superseded: u64,
    pub evicted: u64,
    pub late_after_route: u64,
    pub late_after_superseded: u64,
    pub malformed_closures: u64,
}

#[derive(Debug)]
struct OpenTurn {
    turn_id: String,
    fragments: Vec<TurnFragment>,
    /// This turn's own `task_complete` has been seen. A later `task_started`
    /// proves it can no longer be closed in order, so the entry is superseded.
    closure_seen: bool,
    /// A candidate already left this turn; a second one may never leave.
    routed: bool,
}

/// The bounded, per-reader Codex turn map (plan sections 5.1 and 5.2).
///
/// Entries are keyed by the provider's `turn_id`, never by adjacency, time or
/// proximity. A turn opened implicitly by a `final_answer` waits for its own
/// `task_complete` with no time window ("Late writes"); an entry whose closure
/// was already seen is a zombie that a newer `task_started` supersedes without
/// routing it.
#[derive(Debug, Default)]
pub struct TurnAccumulator {
    open: VecDeque<OpenTurn>,
    counters: TurnCounters,
    /// Ids of closed turns a newer `task_started` superseded. Never bounded and
    /// never shrunk: forgetting an id would let a late fragment plus closure
    /// re-emit it (#2356). Growth is one entry per superseded turn.
    superseded: HashSet<String>,
}

impl TurnAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counters(&self) -> TurnCounters {
        self.counters
    }

    #[cfg(test)]
    pub fn open_turns(&self) -> usize {
        self.open.len()
    }

    /// Accumulate one `final_answer` fragment under its turn id.
    ///
    /// A turn already routed ignores late fragments (counted); otherwise the
    /// fragment joins the open turn in file order, opening the turn if needed
    /// (oldest evicted at the bound, counted).
    pub fn on_final_answer(&mut self, turn_id: &str, fragment: TurnFragment) -> Vec<TurnEvent> {
        if let Some(index) = self.index_of(turn_id) {
            if self.open[index].routed {
                self.counters.late_after_route += 1;
                return vec![TurnEvent::LateAfterRoute {
                    turn_id: turn_id.to_owned(),
                }];
            }
            self.open[index].fragments.push(fragment);
            return Vec::new();
        }
        if self.superseded.contains(turn_id) {
            self.counters.late_after_superseded += 1;
            return vec![TurnEvent::LateAfterSuperseded {
                turn_id: turn_id.to_owned(),
            }];
        }
        let events = self.make_room();
        self.open.push_back(OpenTurn {
            turn_id: turn_id.to_owned(),
            fragments: vec![fragment],
            closure_seen: false,
            routed: false,
        });
        events
    }

    /// Feed one `task_started`: a newer turn is open.
    ///
    /// Any **other** open turn whose own `task_complete` was already seen can
    /// no longer be closed in order, so it is superseded and not routed, with a
    /// visible, counted event. An entry whose closure has **not** been seen is
    /// kept: by the plan's "Late writes" rule there is no time window, so its
    /// own genuinely late `task_complete` can still close and arm it.
    pub fn on_task_started(&mut self, turn_id: &str) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        let mut kept = VecDeque::with_capacity(self.open.len());
        while let Some(turn) = self.open.pop_front() {
            if turn.turn_id != turn_id && turn.closure_seen {
                self.counters.superseded += 1;
                self.superseded.insert(turn.turn_id.clone());
                events.push(TurnEvent::Superseded {
                    turn_id: turn.turn_id,
                });
            } else {
                kept.push_back(turn);
            }
        }
        self.open = kept;
        events
    }

    /// Feed one `task_complete`: close the turn, assemble it and run the
    /// completeness check.
    ///
    /// Exactly one [`TurnEvent::Emit`] can ever leave a turn id. A completion
    /// with no accumulated prose abstains and leaves a closed marker, so a
    /// genuinely late `final_answer` can still be closed by a later completion
    /// while a newer `task_started` supersedes the stale zombie.
    pub fn on_task_complete(
        &mut self,
        turn_id: &str,
        last_agent_message: Option<&str>,
    ) -> Vec<TurnEvent> {
        let Some(index) = self.index_of(turn_id) else {
            if self.superseded.contains(turn_id) {
                self.counters.abstained += 1;
                return vec![TurnEvent::Abstain {
                    turn_id: turn_id.to_owned(),
                    reason: AbstainReason::Superseded,
                }];
            }
            let mut events = self.make_room();
            self.open.push_back(OpenTurn {
                turn_id: turn_id.to_owned(),
                fragments: Vec::new(),
                closure_seen: true,
                routed: false,
            });
            self.counters.abstained += 1;
            events.push(TurnEvent::Abstain {
                turn_id: turn_id.to_owned(),
                reason: AbstainReason::NoAccumulatedFinalAnswer,
            });
            return events;
        };
        if self.open[index].routed {
            self.counters.abstained += 1;
            return vec![TurnEvent::Abstain {
                turn_id: turn_id.to_owned(),
                reason: AbstainReason::AlreadyRouted,
            }];
        }
        if self.open[index].fragments.is_empty() {
            self.open[index].closure_seen = true;
            self.counters.abstained += 1;
            return vec![TurnEvent::Abstain {
                turn_id: turn_id.to_owned(),
                reason: AbstainReason::NoAccumulatedFinalAnswer,
            }];
        }
        let Some(last_agent_message) = last_agent_message else {
            self.open[index].closure_seen = true;
            self.counters.abstained += 1;
            return vec![TurnEvent::Abstain {
                turn_id: turn_id.to_owned(),
                reason: AbstainReason::MissingLastAgentMessage,
            }];
        };

        let joined = self.open[index]
            .fragments
            .iter()
            .map(|fragment| fragment.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let text = joined.trim().to_string();
        if normalise_text(&text) != normalise_text(last_agent_message) {
            self.open[index].closure_seen = true;
            self.counters.abstained += 1;
            return vec![TurnEvent::Abstain {
                turn_id: turn_id.to_owned(),
                reason: AbstainReason::CompletenessMismatch,
            }];
        }

        let first_record_start = self.open[index]
            .fragments
            .first()
            .and_then(|fragment| fragment.record_start);
        // Keep the entry as a routed zombie so a repeated closure or a late
        // fragment can never produce a second candidate for this turn.
        self.open[index].fragments.clear();
        self.open[index].closure_seen = true;
        self.open[index].routed = true;
        self.counters.emitted += 1;
        vec![TurnEvent::Emit(AssembledTurn {
            turn_id: turn_id.to_owned(),
            text,
            first_record_start,
        })]
    }

    /// Feed one rollout line as a closure record.
    ///
    /// [`ClosureParse::Malformed`] is counted and reported so the watcher can
    /// log it; [`ClosureParse::NotClosure`] is a plain no-op.
    pub fn on_line(&mut self, line: &str) -> Vec<TurnEvent> {
        match parse_closure_record(line) {
            ClosureParse::Closure(ClosureRecord::TaskStarted { turn_id }) => {
                self.on_task_started(&turn_id)
            }
            ClosureParse::Closure(ClosureRecord::TaskComplete {
                turn_id,
                last_agent_message,
            }) => self.on_task_complete(&turn_id, last_agent_message.as_deref()),
            ClosureParse::Malformed => {
                self.counters.malformed_closures += 1;
                vec![TurnEvent::MalformedClosure]
            }
            ClosureParse::NotClosure => Vec::new(),
        }
    }

    fn index_of(&self, turn_id: &str) -> Option<usize> {
        self.open.iter().position(|turn| turn.turn_id == turn_id)
    }

    /// Evict the oldest entry(s) until there is room for one more.
    fn make_room(&mut self) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        while self.open.len() >= MAX_OPEN_TURNS {
            let Some(oldest) = self.open.pop_front() else {
                break;
            };
            self.counters.evicted += 1;
            events.push(TurnEvent::Evicted {
                turn_id: oldest.turn_id,
            });
        }
        events
    }
}

#[cfg(test)]
mod tests;
