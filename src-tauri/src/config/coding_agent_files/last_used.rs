//! #4845 the recorded launch sources of one agent (dialog choice, last launch) and the
//! same-setup rule (R24: same file + same agent id). Pure; no IO, no settings types.

use super::ConfigFileLocator;
use serde_json::Value;

/// One recorded setup. `locator` is `Err` when the stored value does not decode.
pub(crate) struct RecordedSource {
    pub agent_id: String,
    pub letter: Option<String>,
    pub locator: Result<ConfigFileLocator, ()>,
}

pub(crate) struct RecordedSources {
    pub choice: Option<RecordedSource>,
    pub last: Option<RecordedSource>,
}

/// An agent of a file with the letters of its enabled profile cells.
pub(crate) struct FileAgentKey {
    pub id: String,
    pub letters: Vec<String>,
}

fn decode(value: &Value) -> Result<ConfigFileLocator, ()> {
    serde_json::from_value(value.clone()).map_err(|_| ())
}

/// The dialog choice needs both source keys; a launch without `sourceFile` is the 30 file.
pub(crate) fn recorded_sources(
    choice: Option<(&str, &Value)>,
    last: Option<(&str, Option<&Value>)>,
    letter: Option<&str>,
) -> RecordedSources {
    let letter = letter.map(ToString::to_string);
    RecordedSources {
        choice: choice.map(|(id, file)| RecordedSource {
            agent_id: id.to_string(),
            letter: letter.clone(),
            locator: decode(file),
        }),
        last: last.map(|(id, file)| RecordedSource {
            agent_id: id.to_string(),
            letter: letter.clone(),
            locator: file.map_or(Ok(ConfigFileLocator::RegisteredInstance {}), decode),
        }),
    }
}

/// R24: same id; the letter must exist unless the agent has no cells or none is recorded.
pub(crate) fn matches_source(rec: &RecordedSource, keys: &[FileAgentKey]) -> bool {
    keys.iter().any(|key| {
        key.id == rec.agent_id
            && rec
                .letter
                .as_ref()
                .is_none_or(|letter| key.letters.is_empty() || key.letters.contains(letter))
    })
}
