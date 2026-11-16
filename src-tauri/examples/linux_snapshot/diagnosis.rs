// #4248 AC-4248-008 timeout diagnosis: the page probe script, the per-view title
// log and the hypothesis classifier. Pure: it imports no GTK and no crate module.
use chrono::Utc;
use serde_json::{json, Map, Value};

// Read from the stalled view after the deadline; names match the fixture main.js.
pub const PROBE_SCRIPT: &str = concat!(
    "(() => { const b = window.ac4248Beacon; return { ",
    "title: String(document.title).slice(0, 512), ",
    "readyState: document.readyState, visibilityState: document.visibilityState, ",
    "hasFocus: document.hasFocus(), frameType: typeof window.ac4248Frame, ",
    "beacon: b === undefined ? null : b }; })()",
);

pub const MAX_ENTRIES: usize = 128;
pub const MAX_TITLE_CHARS: usize = 512;
const MAX_RAW_CHARS: usize = 2048;

// Page probe answer: None when no callback came, "" when the eval failed.
pub fn read_probe(raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return json!({ "status": "callback_timeout" });
    };
    if raw.is_empty() {
        return json!({ "status": "eval_failed" });
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(page @ Value::Object(_)) => json!({ "status": "ok", "page": page }),
        _ => {
            let raw: String = raw.chars().take(MAX_RAW_CHARS).collect();
            json!({ "status": "unparsed", "raw": raw })
        }
    }
}

// Every title a view reported, with its UTC time; bounded, overflow is counted.
#[derive(Default)]
pub struct TitleLog {
    entries: Vec<(&'static str, String, String)>,
    dropped: u32,
}

impl TitleLog {
    pub fn record(&mut self, label: &'static str, title: &str) {
        if self.entries.len() >= MAX_ENTRIES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let title = title.chars().take(MAX_TITLE_CHARS).collect();
        self.entries.push((label, Utc::now().to_rfc3339(), title));
    }

    pub fn seen(&self, label: &str) -> Vec<&str> {
        let own = self.entries.iter().filter(|(name, _, _)| *name == label);
        own.map(|(_, _, title)| title.as_str()).collect()
    }

    pub fn complete(&self) -> bool {
        self.dropped == 0
    }

    pub fn to_json(&self) -> Value {
        let mut log = Map::new();
        for (label, utc, title) in &self.entries {
            let list = log.entry(*label).or_insert_with(|| json!([]));
            if let Some(list) = list.as_array_mut() {
                list.push(json!({ "utc": utc, "title": title }));
            }
        }
        log.insert("dropped".to_owned(), json!(self.dropped));
        Value::Object(log)
    }
}

// Hypothesis token for a readiness stall; the first matching rule wins.
pub fn classify(diagnostics: &Value, seen: &[&str], log_complete: bool) -> &'static str {
    if diagnostics["status"] != "ok" {
        return "probe_unanswered";
    }
    let page = &diagnostics["page"];
    let has_frame = page["frameType"] == "function";
    let beacon = &page["beacon"];
    if !has_frame && beacon.is_null() {
        return "H-a";
    }
    if !has_frame || beacon.is_null() {
        return "fixture_inconsistent";
    }
    let counter = |name: &str| beacon[name].as_u64();
    let counters = (
        counter("frameCalls"),
        counter("rafRequested"),
        counter("rafTicks"),
    );
    let (Some(frame_calls), Some(requested), Some(ticks)) = counters else {
        return "fixture_inconsistent";
    };
    if frame_calls == 0 {
        return "frame_not_called";
    }
    let title = page["title"].as_str().unwrap_or_default();
    let published = title.starts_with("ac4248:{") || title.starts_with("ac4248:error:");
    if published && !seen.contains(&title) {
        return if log_complete { "H-c" } else { "undetermined" };
    }
    if requested > ticks {
        return "H-b";
    }
    "undetermined"
}

#[cfg(test)]
mod tests;
