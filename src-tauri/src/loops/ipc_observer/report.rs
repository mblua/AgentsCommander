use super::{
    stamp, BlackBoxRecord, FreezeMarker, TickSnapshot, Verdict, COVERAGE_LINE, LOOP_OUTLIVED_MS,
    MAX_LOG_LINE,
};

// ---------------------------------------------------------------------------
// Block builders (pure)
// ---------------------------------------------------------------------------

pub(super) fn record_block(
    record: &BlackBoxRecord,
    marker: Option<&FreezeMarker>,
    verdict: Verdict,
) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "[ipc-blackbox] previous-run window label={} type={} v={} visible={} closed_cleanly={}",
        record.label, record.window_type, record.v, record.visible, record.closed_cleanly
    ));
    out.push(format!(
        "[ipc-blackbox]   started={} last_tick={} ticks={} raf={} last_raf={}",
        stamp(record.started_at_ms),
        stamp(record.written_at_ms),
        record.tick_seq,
        record.raf_seq,
        stamp(record.last_raf_at_ms)
    ));
    let probe = match record.probe_at_ms {
        Some(at) => match record.probe_seq {
            Some(seq) => format!("{}@{}", seq, stamp(at)),
            None => format!("?@{}", stamp(at)),
        },
        None => "none".to_string(),
    };
    out.push(format!(
        "[ipc-blackbox]   last_pointer={} last_event={} ({}) probe={}",
        stamp(record.last_pointer_at_ms),
        stamp(record.last_event_at_ms),
        record.last_event_name,
        probe
    ));
    let last_sent = if record.last_sent_at_ms == 0 {
        "never".to_string()
    } else {
        stamp(record.last_sent_at_ms)
    };
    out.push(format!(
        "[ipc-blackbox]   invokes sent={} settled={} last_settled={} last_sent={} pending={} \
            overdue={}",
        record.sent,
        record.settled,
        stamp(record.last_settled_at_ms),
        last_sent,
        record.pending_total,
        record.overdue_total
    ));
    // One line per entry IN ARRAY ORDER, up to phase 2's MAX_PENDING_RECORDED
    // (32). Not a formatting preference: the (b) arm's corroborator is read off
    // the OLDEST `overdue=true` entry, and the array's first entries can be
    // `overdue=false` dialogs, so an emitter that printed only the first entry,
    // or that reordered them, would put the verdict beyond hand-derivation.
    for entry in record.pending.iter().take(32) {
        out.push(format!(
            "[ipc-blackbox]   pending id={} cmd={} age_ms={} overdue={}",
            entry.id, entry.cmd, entry.age_ms, entry.overdue
        ));
    }
    let raf_age_ms = record
        .written_at_ms
        .saturating_sub(record.last_raf_at_ms.max(record.started_at_ms));
    match marker {
        Some(m) => {
            out.push(format!(
                "[ipc-blackbox]   marker: last_invoke={}@{} silence_declared={} probes={} ended={}",
                m.last_invoke_cmd,
                stamp(m.last_invoke_at_ms),
                stamp(m.silence_started_at_ms),
                m.probes_emitted,
                ended_text(m)
            ));
            let outlived_ms = record.written_at_ms.saturating_sub(m.last_invoke_at_ms);
            let onset_offset = m.silence_started_at_ms.saturating_sub(m.last_invoke_at_ms);
            out.push(format!(
                "[ipc-blackbox]   timing: outlived_ms={} (bounds: a<{}, c>=onset at {}ms); \
                    raf_age_ms={} visible={}",
                outlived_ms, LOOP_OUTLIVED_MS, onset_offset, raf_age_ms, record.visible
            ));
        }
        None => {
            out.push(format!(
                "[ipc-blackbox]   timing: no episode; raf_age_ms={} visible={}",
                raf_age_ms, record.visible
            ));
        }
    }
    out.push(COVERAGE_LINE.to_string());
    out.push(format!(
        "[ipc-blackbox] VERDICT {}: {}",
        verdict.tag(),
        verdict.explanation()
    ));
    out
}

pub(super) fn marker_only_block(marker: &FreezeMarker) -> Vec<String> {
    vec![
        format!(
            "[ipc-blackbox] marker only: silence declared {}; last was {} at {}; probes={}; \
                ended={}",
            stamp(marker.silence_started_at_ms),
            marker.last_invoke_cmd,
            stamp(marker.last_invoke_at_ms),
            marker.probes_emitted,
            ended_text(marker)
        ),
        "[ipc-blackbox] NO VERDICT: no renderer black box survived that run, so a/b/c/d cannot be \
            separated. Either localStorage did not survive the kill or no window ever wrote a \
            record. The `[ipc-observer] SILENCE` block from that run is what the instrument \
            produced."
            .to_string(),
    ]
}

pub(super) fn summary_line(entries: &[(String, &'static str)]) -> String {
    let body = entries
        .iter()
        .map(|(label, tag)| format!("{}={}", label, tag))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[ipc-blackbox] SUMMARY {} previous-run windows: {}",
        entries.len(),
        body
    )
}

fn ended_text(marker: &FreezeMarker) -> String {
    match (marker.ended_by.as_deref(), marker.silence_ended_at_ms) {
        (Some(reason), Some(at)) => format!("{}@{}", reason, stamp(at)),
        (Some(reason), None) => reason.to_string(),
        _ => "never".to_string(),
    }
}

/// The silence block. `recent:` is wrapped so no line exceeds `MAX_LOG_LINE`.
pub(super) fn silence_block(
    silent_for: u64,
    last_invoke_at_ms: u64,
    snapshot: &TickSnapshot,
    total_invokes: u64,
) -> Vec<String> {
    let mut out = vec![
        format!(
            "[ipc-observer] SILENCE: no invoke for {}ms; last was {} at {}",
            silent_for,
            snapshot.last_invoke_cmd,
            stamp(last_invoke_at_ms)
        ),
        format!(
            "[ipc-observer]   total invokes this run={}; tracked commands={}",
            total_invokes, snapshot.tracked
        ),
    ];
    const RECENT_PREFIX: &str = "[ipc-observer]   recent: ";
    let mut line = String::from(RECENT_PREFIX);
    let mut first_on_line = true;
    for (name, count, at) in &snapshot.recent {
        let piece = format!("{} x{} last {}", name, count, stamp(*at));
        let extra = if first_on_line { 0 } else { 3 };
        if !first_on_line && line.len() + extra + piece.len() > MAX_LOG_LINE {
            out.push(line);
            line = String::from(RECENT_PREFIX);
            first_on_line = true;
        }
        if !first_on_line {
            line.push_str(" | ");
        }
        line.push_str(&piece);
        first_on_line = false;
    }
    out.push(line);
    out.push(
        "[ipc-observer]   probing the renderer on `ipc_silence_probe`; the next start reports what \
            it saw"
            .to_string(),
    );
    out
}
