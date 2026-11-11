//! #2774 Pure Wayland items (ids, rules, state). No `super` paths: keeps it out of the SCC.

use uuid::Uuid;

/// The host Registry accepts only an id that has an installed `<id>.desktop` file;
/// `npm/linux-desktop-entry.js` installs `agentscommander.desktop`, and
/// `dev.agentscommander.app` is rejected on Omarchy ("App info not found").
pub(super) const PORTAL_APP_ID: &str = "agentscommander";

/// The portal selector owns Escape and the keyboard on Wayland; clearing our state
/// while it is open would orphan it, so a repeat press there never cancels.
pub(super) fn press_may_cancel(wayland: bool, cancellable: Option<Uuid>) -> Option<Uuid> {
    if wayland {
        None
    } else {
        cancellable
    }
}

/// `pub(crate)`: it appears in `pub(crate) trait WaylandBackend` (lint
/// `private_interfaces` under `-D warnings`, round-3 AG B1 / RG NB1).
pub(crate) struct ShortcutIds {
    pub(super) shortcut_id: String,
    pub(super) description: String,
}

/// One token per AC configuration directory (= per AC instance): the first 8 lowercase
/// hex chars of the SHA-256 of the directory path.
pub(super) fn instance_token(config_dir: Option<&std::path::Path>) -> String {
    use sha2::{Digest, Sha256};
    let path = config_dir
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let digest = Sha256::digest(path.as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn shortcut_ids(token: &str) -> ShortcutIds {
    ShortcutIds {
        shortcut_id: format!("screenshot-{token}"),
        description: format!("AgentsCommander screenshot {token}"),
    }
}

/// Serialized Wayland registration state. `binding == 0` = no live portal binding.
#[derive(Default)]
pub struct WaylandPortalState {
    pub(super) binding: u64,
    pub(super) next_binding: u64,
}

/// Stays true after a portal restart: the next save re-registers and rebinds.
pub(super) const PORTAL_SHORTCUT_ENDED: &str = "The desktop portal stopped the \
    screenshot shortcut. Save Settings or restart AgentsCommander to bind it again.";
