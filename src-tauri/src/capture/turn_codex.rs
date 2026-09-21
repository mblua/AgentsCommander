//! Codex turn-boundary parsing and assembly (#2232 phase 5).
//!
//! A Codex rollout persists a turn's boundary as `event_msg` records:
//! `task_started` opens a turn and `task_complete` closes it, carrying the flat
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

use std::collections::VecDeque;

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
/// Accepts exactly `type=event_msg` with `payload.type` `task_started` or
/// `task_complete`. The `turn_id` is read **flat from the payload** and must be
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
        "task_complete" => match turn_id {
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
}

impl AbstainReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAccumulatedFinalAnswer => "no-accumulated-final-answer",
            Self::MissingLastAgentMessage => "missing-last-agent-message",
            Self::CompletenessMismatch => "completeness-mismatch",
            Self::AlreadyRouted => "already-routed",
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
}

impl TurnAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn counters(&self) -> TurnCounters {
        self.counters
    }

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
mod tests {
    use super::*;

    fn fragment(text: &str, start: u64) -> TurnFragment {
        TurnFragment {
            text: text.to_owned(),
            record_start: Some(start),
        }
    }

    fn closure_line(payload: serde_json::Value) -> String {
        serde_json::json!({"type": "event_msg", "payload": payload}).to_string()
    }

    fn emit_text(events: &[TurnEvent]) -> Option<&AssembledTurn> {
        events.iter().find_map(|event| match event {
            TurnEvent::Emit(turn) => Some(turn),
            _ => None,
        })
    }

    // Test 7, parser half: the flat `turn_id` belongs to `task_complete`; the
    // nested location is a different field and must not be read here.
    #[test]
    fn parse_reads_the_flat_turn_id_on_both_closure_records() {
        let started = parse_closure_record(&closure_line(
            serde_json::json!({"type": "task_started", "turn_id": "t-1"}),
        ));
        assert_eq!(
            started,
            ClosureParse::Closure(ClosureRecord::TaskStarted {
                turn_id: "t-1".to_owned()
            })
        );

        let complete = parse_closure_record(&closure_line(serde_json::json!({
            "type": "task_complete",
            "turn_id": "t-1",
            "last_agent_message": "the message"
        })));
        assert_eq!(
            complete,
            ClosureParse::Closure(ClosureRecord::TaskComplete {
                turn_id: "t-1".to_owned(),
                last_agent_message: Some("the message".to_owned()),
            })
        );

        // The id nested where the final answer carries it is not this field.
        let nested_only = parse_closure_record(&closure_line(serde_json::json!({
            "type": "task_complete",
            "last_agent_message": "x",
            "internal_chat_message_metadata_passthrough": {"turn_id": "t-1"}
        })));
        assert_eq!(nested_only, ClosureParse::Malformed);
    }

    #[test]
    fn parse_marks_a_closure_without_a_usable_id_malformed() {
        for payload in [
            serde_json::json!({"type": "task_started"}),
            serde_json::json!({"type": "task_started", "turn_id": 7}),
            serde_json::json!({"type": "task_started", "turn_id": ""}),
            serde_json::json!({"type": "task_complete", "last_agent_message": "x"}),
        ] {
            assert_eq!(
                parse_closure_record(&closure_line(payload.clone())),
                ClosureParse::Malformed,
                "payload={payload}"
            );
        }
        // A completion may legitimately omit `last_agent_message`; that is an
        // abstention reason, not a parse failure.
        assert!(matches!(
            parse_closure_record(&closure_line(
                serde_json::json!({"type": "task_complete", "turn_id": "t"})
            )),
            ClosureParse::Closure(ClosureRecord::TaskComplete {
                last_agent_message: None,
                ..
            })
        ));
    }

    // Test 11, parser half: Claude prose and tool traffic are never closures.
    // The Claude bits themselves are pinned by the untouched
    // `claude_watcher::tests::assistant_records_carry_claudes_permanent_bits`.
    #[test]
    fn parse_ignores_non_closure_lines() {
        let claude = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "claude prose"}]}
        })
        .to_string();
        let tool = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "function_call", "name": "shell", "arguments": "{}"}
        })
        .to_string();
        for line in [claude, tool.clone(), "not json".to_owned(), String::new()] {
            assert_eq!(parse_closure_record(&line), ClosureParse::NotClosure);
        }
        let mut turns = TurnAccumulator::new();
        assert!(turns.on_line(&tool).is_empty());
        assert_eq!(turns.open_turns(), 0);
        assert_eq!(turns.counters(), TurnCounters::default());
    }

    // Test 3, state-machine half: a multi-record turn is joined in file order
    // and leaves exactly one assembled turn.
    #[test]
    fn a_multi_record_turn_assembles_in_file_order_and_emits_once() {
        let mut turns = TurnAccumulator::new();
        assert!(turns
            .on_final_answer("t1", fragment("first", 10))
            .is_empty());
        assert!(turns
            .on_final_answer("t1", fragment("second", 20))
            .is_empty());
        // Grouping is by identifier, not adjacency: another turn may interleave.
        assert!(turns
            .on_final_answer("t2", fragment("other", 30))
            .is_empty());
        assert!(turns
            .on_final_answer("t1", fragment("third", 40))
            .is_empty());

        let events = turns.on_task_complete("t1", Some("first\nsecond\nthird"));
        let emitted = emit_text(&events).expect("the turn must emit");
        assert_eq!(emitted.turn_id, "t1");
        assert_eq!(emitted.text, "first\nsecond\nthird");
        assert_eq!(emitted.first_record_start, Some(10));
        assert_eq!(turns.counters().emitted, 1);

        // Test 9, state-machine half: the rest of the turn is still open and
        // assembles separately.
        let events = turns.on_task_complete("t2", Some("other"));
        let emitted = emit_text(&events).expect("the second turn must emit");
        assert_eq!(emitted.text, "other");
    }

    // Test 4: newline kind and trailing whitespace must not cause abstention.
    #[test]
    fn normalisation_accepts_newline_kind_and_trailing_whitespace() {
        assert_eq!(normalise_text("a\r\nb\r\n"), "a\nb");
        assert_eq!(normalise_text("  a\nb  "), "a\nb");

        let mut turns = TurnAccumulator::new();
        turns.on_final_answer("t", fragment("first", 0));
        turns.on_final_answer("t", fragment("second", 1));
        let events = turns.on_task_complete("t", Some("first\r\nsecond\r\n"));
        let emitted = emit_text(&events).expect("normalised comparison must accept");
        // The candidate carries the assembly, not the provider's field.
        assert_eq!(emitted.text, "first\nsecond");
    }

    // Test 5: an assembly genuinely missing a record is rejected.
    #[test]
    fn a_missing_fragment_is_rejected_with_a_reason() {
        let mut turns = TurnAccumulator::new();
        turns.on_final_answer("t", fragment("first", 0));
        let events = turns.on_task_complete("t", Some("first\nsecond"));
        assert_eq!(
            events,
            vec![TurnEvent::Abstain {
                turn_id: "t".to_owned(),
                reason: AbstainReason::CompletenessMismatch
            }]
        );
        assert_eq!(turns.counters().emitted, 0);
        assert_eq!(turns.counters().abstained, 1);
    }

    #[test]
    fn a_completion_without_last_agent_message_abstains() {
        let mut turns = TurnAccumulator::new();
        turns.on_final_answer("t", fragment("body", 0));
        let events = turns.on_task_complete("t", None);
        assert_eq!(
            events,
            vec![TurnEvent::Abstain {
                turn_id: "t".to_owned(),
                reason: AbstainReason::MissingLastAgentMessage
            }]
        );
        assert_eq!(turns.counters().emitted, 0);
    }

    // Test 2, state-machine half: a completion matching nothing abstains.
    #[test]
    fn a_completion_without_a_final_answer_abstains() {
        let mut turns = TurnAccumulator::new();
        let events = turns.on_task_complete("t", Some("x"));
        assert_eq!(
            events,
            vec![TurnEvent::Abstain {
                turn_id: "t".to_owned(),
                reason: AbstainReason::NoAccumulatedFinalAnswer
            }]
        );
        assert_eq!(turns.counters().emitted, 0);
        assert_eq!(
            turns.open_turns(),
            1,
            "the closed marker waits for late writes"
        );
    }

    // Test 6: a `task_started` after the last `task_complete` supersedes the
    // closed turn and routes nothing, with a visible counted event.
    #[test]
    fn a_task_started_supersedes_a_closed_turn_and_does_not_route_it() {
        let mut turns = TurnAccumulator::new();
        assert_eq!(
            turns.on_task_complete("closed", Some("x")),
            vec![TurnEvent::Abstain {
                turn_id: "closed".to_owned(),
                reason: AbstainReason::NoAccumulatedFinalAnswer
            }]
        );
        let events = turns.on_task_started("newer");
        assert_eq!(
            events,
            vec![TurnEvent::Superseded {
                turn_id: "closed".to_owned()
            }]
        );
        assert_eq!(turns.counters().superseded, 1);
        assert_eq!(turns.counters().emitted, 0);
        assert_eq!(turns.open_turns(), 0);
        // The superseded turn's own closure is gone; a later repetition is not
        // a candidate either.
        let events = turns.on_task_complete("closed", Some("x"));
        assert!(emit_text(&events).is_none());
    }

    // The plan's "Late writes" rule: no time window. A final answer that
    // arrives after an early closure can still be closed and emitted.
    #[test]
    fn a_late_final_answer_after_an_early_closure_can_still_emit() {
        let mut turns = TurnAccumulator::new();
        turns.on_task_complete("t", Some("late body"));
        turns.on_final_answer("t", fragment("late body", 7));
        let events = turns.on_task_complete("t", Some("late body"));
        let emitted = emit_text(&events).expect("the late write must still arm");
        assert_eq!(emitted.text, "late body");
        assert_eq!(emitted.first_record_start, Some(7));
    }

    // An open turn whose closure has NOT been seen is not superseded by a newer
    // start: its own closure can still arrive (no time window).
    #[test]
    fn a_task_started_keeps_a_turn_that_waits_for_its_closure() {
        let mut turns = TurnAccumulator::new();
        turns.on_final_answer("waiting", fragment("body", 0));
        assert!(turns.on_task_started("newer").is_empty());
        assert_eq!(turns.open_turns(), 1);
        let events = turns.on_task_complete("waiting", Some("body"));
        assert!(emit_text(&events).is_some());
    }

    // Test 10: nine interleaved open turns leave eight; the dropped one emits
    // nothing even when its completion arrives.
    #[test]
    fn nine_open_turns_leave_eight_and_the_oldest_is_dropped() {
        let mut turns = TurnAccumulator::new();
        for index in 0..9 {
            let events = turns.on_final_answer(&format!("t{index}"), fragment("body", index));
            if index == 8 {
                assert_eq!(
                    events,
                    vec![TurnEvent::Evicted {
                        turn_id: "t0".to_owned()
                    }]
                );
            }
        }
        assert_eq!(turns.open_turns(), MAX_OPEN_TURNS);
        assert_eq!(turns.counters().evicted, 1);

        // The surviving turns still assemble.
        let events = turns.on_task_complete("t1", Some("body"));
        assert_eq!(
            emit_text(&events).map(|turn| turn.turn_id.as_str()),
            Some("t1")
        );

        // The dropped turn emits nothing: its completion opens only an empty
        // marker, which is never a candidate.
        let events = turns.on_task_complete("t0", Some("body"));
        assert!(emit_text(&events).is_none(), "a dropped turn never emits");
    }

    // "Exactly one CapturedRecord per closed turn": a repeated closure and a
    // late fragment must not yield a second candidate.
    #[test]
    fn a_routed_turn_never_emits_again() {
        let mut turns = TurnAccumulator::new();
        turns.on_final_answer("t", fragment("body", 0));
        assert!(emit_text(&turns.on_task_complete("t", Some("body"))).is_some());

        let repeated = turns.on_task_complete("t", Some("body"));
        assert_eq!(
            repeated,
            vec![TurnEvent::Abstain {
                turn_id: "t".to_owned(),
                reason: AbstainReason::AlreadyRouted
            }]
        );
        let late = turns.on_final_answer("t", fragment("more", 1));
        assert_eq!(
            late,
            vec![TurnEvent::LateAfterRoute {
                turn_id: "t".to_owned()
            }]
        );
        assert_eq!(turns.counters().emitted, 1);
        assert_eq!(turns.counters().late_after_route, 1);
    }

    // An unparseable closure is skipped and counted, never fatal.
    #[test]
    fn an_unparseable_closure_is_counted_and_skipped() {
        let mut turns = TurnAccumulator::new();
        let events = turns.on_line(&closure_line(serde_json::json!({"type": "task_started"})));
        assert_eq!(events, vec![TurnEvent::MalformedClosure]);
        assert_eq!(turns.counters().malformed_closures, 1);
        assert_eq!(turns.open_turns(), 0);
    }
}
