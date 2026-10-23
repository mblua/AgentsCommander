use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;

type Seam = (Sender<()>, Receiver<()>);
static SEAMS: Mutex<Option<HashMap<String, Seam>>> = Mutex::new(None);

/// Arm the pause for `fqn`: `reached` gets a message when the restore
/// pauses; the restore resumes when `release` gets one.
pub(crate) fn install(fqn: &str, reached: Sender<()>, release: Receiver<()>) {
    SEAMS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(fqn.to_string(), (reached, release));
}

pub(crate) fn hit(fqn: &str) {
    let seam = SEAMS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|map| map.remove(fqn));
    if let Some((reached, release)) = seam {
        let _ = reached.send(());
        let _ = release.recv();
    }
}
