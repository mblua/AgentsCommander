// Artifact owner for the #4248 snapshot experiment: the output directory, the
// PNG files and receipt.json. Every file is created new; nothing is replaced.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
const RECEIPT_NAME: &str = "receipt.json";

// Types shared by main (validators) and native (harness); native never imports main.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    None,
    Blank,
    Stale,
    Cancel,
}

impl Probe {
    pub const ALL: [(&'static str, Self); 4] = [
        ("none", Self::None),
        ("blank", Self::Blank),
        ("stale", Self::Stale),
        ("cancel", Self::Cancel),
    ];

    pub fn token(self) -> &'static str {
        let found = Self::ALL.iter().find(|(_, probe)| *probe == self);
        found.map_or("none", |(name, _)| name)
    }
}

// One fixture request: window label and the exact ac4248Frame arguments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepSpec {
    pub label: &'static str,
    pub frame: &'static str,
    pub zoom: f64,
    pub renderer: &'static str,
}

// Title check result for the current request; Ready carries the raw ready JSON.
#[derive(Debug, PartialEq, Eq)]
pub enum Readiness {
    Pending,
    Ready(String),
    Incomplete(String),
}

// One captured surface as plain RGBA, with the ready JSON of its request.
pub struct Snapshot<'a> {
    pub step: &'a StepSpec,
    pub ready: &'a str,
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

// Accept carries the case record (metrics) main derives from the snapshot.
#[derive(Debug, PartialEq)]
pub enum PixelVerdict {
    Accept(Value),
    Reject(String),
}

// One request in flight; late or duplicate callbacks for other generations lose.
#[derive(Debug, Default)]
pub struct RequestGate {
    current: Option<String>,
    snapshot_pending: bool,
}

impl RequestGate {
    pub fn begin(&mut self, generation: String) {
        self.current = Some(generation);
        self.snapshot_pending = false;
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn start_snapshot(&mut self, generation: &str) -> bool {
        let start = self.current() == Some(generation) && !self.snapshot_pending;
        self.snapshot_pending |= start;
        start
    }

    pub fn finish_snapshot(&mut self, generation: &str) -> bool {
        let finish = self.current() == Some(generation) && self.snapshot_pending;
        if finish {
            self.cancel();
        }
        finish
    }

    pub fn cancel(&mut self) {
        self.current = None;
        self.snapshot_pending = false;
    }
}

// In-memory sink that refuses to grow past its limit, so an oversized PNG
// fails during encoding instead of after it.
pub struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedWriter {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(buf.len()) > self.limit {
            return Err(io::Error::other("encoded_size_limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ProbeRejection {
    pub probe: &'static str,
    pub error: &'static str,
    pub snapshot_taken: bool,
    pub validator_reached: bool,
    pub exit: u8,
}

pub enum Outcome {
    Pass,
    Fail(String),
    Incomplete(String),
    ProbeRejected(ProbeRejection),
}

impl Outcome {
    pub fn exit(&self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail(_) => 1,
            Self::Incomplete(_) => 2,
            Self::ProbeRejected(rejection) => rejection.exit,
        }
    }

    fn fields(&self, accepted_pngs: usize) -> Map<String, Value> {
        let value = match self {
            Self::Pass => json!({ "verdict": "pass" }),
            Self::Fail(reason) => json!({ "verdict": "fail", "reason": reason }),
            Self::Incomplete(reason) => json!({ "verdict": "incomplete", "reason": reason }),
            Self::ProbeRejected(rejection) => json!({
                "verdict": "probe_rejected",
                "probe": rejection.probe,
                "error": rejection.error,
                "snapshotTaken": rejection.snapshot_taken,
                "validatorReached": rejection.validator_reached,
                "acceptedPngs": accepted_pngs,
            }),
        };
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    fn diagnostic(&self) -> String {
        match self {
            Self::Pass => "ac4248 snapshot: pass".to_owned(),
            Self::Fail(reason) => format!("ac4248 snapshot: fail: {reason}"),
            Self::Incomplete(reason) => format!("ac4248 snapshot: incomplete: {reason}"),
            Self::ProbeRejected(rejection) => {
                format!(
                    "ac4248 snapshot: probe {} rejected: {}",
                    rejection.probe, rejection.error
                )
            }
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct Receipt {
    dir: PathBuf,
    fields: Map<String, Value>,
    cases: Vec<Value>,
    pngs: Vec<Value>,
}

impl Receipt {
    // Creates the new output directory (its parent must exist) and the two
    // owned WebKit data directories. An existing directory is refused.
    pub fn create(dir: &Path) -> io::Result<Self> {
        if !dir.is_absolute() {
            return Err(io::Error::other("output directory must be absolute"));
        }
        fs::create_dir(dir)?;
        let receipt = Self {
            dir: dir.to_path_buf(),
            fields: Map::new(),
            cases: Vec::new(),
            pngs: Vec::new(),
        };
        fs::create_dir(receipt.webkit_dir("target"))?;
        fs::create_dir(receipt.webkit_dir("control"))?;
        Ok(receipt)
    }

    pub fn webkit_dir(&self, label: &str) -> PathBuf {
        self.dir.join(format!("webkit-{label}"))
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.fields.insert(key.to_owned(), value);
    }

    pub fn record_case(&mut self, case: Value) {
        self.cases.push(case);
    }

    pub fn png_count(&self) -> usize {
        self.pngs.len()
    }

    pub fn publish_png(&mut self, name: &str, bytes: &[u8]) -> io::Result<String> {
        if bytes.len() > MAX_PNG_BYTES {
            return Err(io::Error::other("encoded_size_limit"));
        }
        self.write_new(name, bytes)?;
        let digest = sha256_hex(bytes);
        self.pngs
            .push(json!({ "name": name, "bytes": bytes.len(), "sha256": digest }));
        Ok(digest)
    }

    pub fn finish(mut self, outcome: &Outcome) -> u8 {
        let mut fields = std::mem::take(&mut self.fields);
        fields.extend(outcome.fields(self.pngs.len()));
        fields.insert(
            "cases".to_owned(),
            Value::Array(std::mem::take(&mut self.cases)),
        );
        fields.insert(
            "pngs".to_owned(),
            Value::Array(std::mem::take(&mut self.pngs)),
        );
        fields.insert(
            "finishedUtc".to_owned(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );
        let text = match serde_json::to_vec_pretty(&Value::Object(fields)) {
            Ok(text) => text,
            Err(error) => {
                format!("{{\"verdict\":\"incomplete\",\"reason\":\"{error}\"}}").into_bytes()
            }
        };
        eprintln!("{}", outcome.diagnostic());
        if let Err(error) = self.write_new(RECEIPT_NAME, &text) {
            eprintln!("ac4248 snapshot: receipt not written: {error}");
            return 2;
        }
        outcome.exit()
    }

    fn write_new(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(io::Error::other("invalid artifact name"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.dir.join(name))?;
        file.write_all(bytes)?;
        file.sync_all()
    }
}

#[cfg(test)]
mod tests;
