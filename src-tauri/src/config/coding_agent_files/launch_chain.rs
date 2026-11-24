//! #4845 the launch file chain (R19, R23): dialog choice, last launch, active file, the 30
//! file. Pure; the caller supplies the agent keys of each file through `keys_of`.

use super::last_used::{matches_source, FileAgentKey, RecordedSource, RecordedSources};
use super::ConfigFileLocator;

/// Which chain step picked the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchPick {
    Choice,
    LastUsed,
    Active,
    Fallback30 { wanted: ConfigFileLocator },
}

type Keys = Option<Vec<FileAgentKey>>;

/// Calls `keys_of` at most once per distinct locator.
fn keys_for<'a>(
    seen: &'a mut Vec<(ConfigFileLocator, Keys)>,
    keys_of: &mut dyn FnMut(&ConfigFileLocator) -> Keys,
    loc: &ConfigFileLocator,
) -> &'a Keys {
    let index = match seen.iter().position(|(known, _)| known == loc) {
        Some(index) => index,
        None => {
            seen.push((loc.clone(), keys_of(loc)));
            seen.len() - 1
        }
    };
    &seen[index].1
}

/// The one file a launch uses; `None` from `keys_of` = the file is unavailable.
pub(crate) fn pick_launch_file(
    sources: &RecordedSources,
    active: &ConfigFileLocator,
    keys_of: &mut dyn FnMut(&ConfigFileLocator) -> Keys,
) -> (ConfigFileLocator, LaunchPick) {
    let mut seen = Vec::new();
    let steps: [(&Option<RecordedSource>, LaunchPick); 2] = [
        (&sources.choice, LaunchPick::Choice),
        (&sources.last, LaunchPick::LastUsed),
    ];
    for (source, pick) in steps {
        let Some(rec) = source else { continue };
        let Ok(loc) = &rec.locator else { continue };
        if let Some(keys) = keys_for(&mut seen, keys_of, loc) {
            if matches_source(rec, keys) {
                return (loc.clone(), pick);
            }
        }
    }
    if keys_for(&mut seen, keys_of, active).is_some() {
        return (active.clone(), LaunchPick::Active);
    }
    (
        ConfigFileLocator::RegisteredInstance {},
        LaunchPick::Fallback30 {
            wanted: active.clone(),
        },
    )
}
