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
