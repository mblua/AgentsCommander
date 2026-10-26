//! Linux display-server detection (#1842).

// ── Linux display-server detection (#1842) ─────────────────────────────────

/// The refusal message for a Wayland session. The wording is deliberate: it
/// says *detected as* Wayland, not *is* Wayland, because
/// [`classify_display_server`] approximates `wl_display_connect(NULL)` and can
/// produce a false `Wayland` (see its declared false positives). An "is" would
/// send a user who is already on Xorg to change something that is correct; the
/// last sentence names the actionable class instead — a stale setting or socket
/// from an earlier session — without claiming which member fired, because the
/// classifier cannot tell. The literal `"Xorg"` is asserted by a test.
#[cfg(target_os = "linux")]
pub const WAYLAND_UNSUPPORTED: &str = "Screenshot capture needs an X11 session, and this one \
    was detected as Wayland, so the hotkey was not registered. Log out and choose the Xorg \
    option (on Ubuntu: \"Ubuntu on Xorg\") at the login screen, then start AgentsCommander \
    again. If you are already on Xorg, a Wayland setting or socket left over from an earlier \
    session caused this: start AgentsCommander from a fresh login shell, not from a terminal \
    multiplexer or service that was started under Wayland.";

/// The display server the capture path believes it is running under.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDisplayServer {
    X11,
    Wayland,
}

/// One read of the environment signals that decide [`classify_display_server`].
/// Split from the classifier so the classifier stays pure and the whole table is
/// unit-testable with no environment or filesystem setup. The field set is
/// exactly the inputs that function reads; its docs carry what is excluded from
/// the snapshot and why (a field written but never read fails
/// `cargo clippy --all-targets -- -D warnings`, and a field that exists at all
/// invites the next reader to write a rule for it).
#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone)]
pub struct DisplayEnvSnapshot {
    pub xdg_session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub wayland_socket: Option<String>,
    pub default_socket_present: bool,
}

/// Take the environment/filesystem snapshot that later decides the display
/// server. The filesystem probe (the `wayland-0` socket under
/// `$XDG_RUNTIME_DIR`) lives HERE, in the impure snapshot function, precisely so
/// the classifier stays pure and the whole table stays unit-testable with no
/// environment or filesystem setup.
///
/// WHEN this is called is part of the design: it must be the FIRST STATEMENT of
/// `lib.rs::run()`, before anything initialises GTK. GTK's Wayland backend calls
/// `wl_display_connect()`, which consumes an inherited descriptor and then
/// `unsetenv("WAYLAND_SOCKET")`; a call from `.setup` or from
/// `register_configured_hotkey` would read an erased variable and answer `X11`
/// for a session reached only by an inherited fd — the silent dead hotkey this
/// whole phase exists to prevent. This function must have exactly ONE call site
/// in the crate (`lib.rs::run`); §8's placement controls and probe P6 pin that.
///
/// `false` when `XDG_RUNTIME_DIR` is unset or any step errors; a `wayland-0`
/// that exists but is a regular file or directory is not a socket.
#[cfg(target_os = "linux")]
pub fn display_env_snapshot() -> DisplayEnvSnapshot {
    let default_socket_present = crate::config::environment::xdg_runtime_dir()
        .map(std::path::PathBuf::from)
        .and_then(|dir| std::fs::metadata(dir.join("wayland-0")).ok())
        .map(|meta| {
            use std::os::unix::fs::FileTypeExt;
            meta.file_type().is_socket()
        })
        .unwrap_or(false);
    DisplayEnvSnapshot {
        xdg_session_type: crate::config::environment::xdg_session_type().ok(),
        wayland_display: crate::config::environment::wayland_display().ok(),
        wayland_socket: crate::config::environment::wayland_socket().ok(),
        default_socket_present,
    }
}

/// Classify the session as X11 or Wayland from a snapshot, pure. Rules, in
/// order (each step is the next connection path `wl_display_connect(NULL)`
/// itself would try):
///
/// 1. `XDG_SESSION_TYPE`: contains `wayland` -> `Wayland`; contains `x11` ->
///    `X11`. logind/PAM sets this, so it is the session manager's own statement
///    of what the session is, and it outranks every socket signal: a real X11
///    session that inherited a stale `WAYLAND_DISPLAY` must stay `X11`.
/// 2. `WAYLAND_SOCKET` set, non-blank and parsing as a non-negative integer
///    (`trim().parse::<u32>().is_ok()`) -> `Wayland`. This is an
///    already-connected fd inherited from the parent; no other variable need be
///    set at all. A non-numeric value cannot be a descriptor — libwayland
///    treats it as an error, not a connection — and a blank value is an error
///    there too, so both are treated as absent.
/// 3. `WAYLAND_DISPLAY` set and non-blank -> `Wayland`.
/// 4. `default_socket_present` -> `Wayland`. `wl_display_connect(NULL)` falls
///    back to the literal `wayland-0` under `$XDG_RUNTIME_DIR` when
///    `WAYLAND_DISPLAY` is unset, so a live compositor there is a Wayland
///    session even with `XDG_SESSION_TYPE` unset (the XWayland trap).
/// 5. Otherwise `X11`. The default is defended by construction, not by the
///    false claim that every Wayland compositor exports `WAYLAND_DISPLAY`:
///    reaching rule 5 means every path `wl_display_connect(NULL)` would take has
///    been checked and none of them would connect. A compositor no Wayland
///    client could reach is not a session this app runs under as a Wayland
///    client, so `X11` is the only reading left — and it is the one that does
///    not disable a working feature.
///
/// `DISPLAY` is NOT a signal. Under GNOME/Wayland with XWayland both it and
/// `WAYLAND_DISPLAY` are set — precisely the case this classifier exists to
/// catch, because the X11 grab there registers and never fires.
///
/// `GDK_BACKEND` is NOT a signal either, and no parse of it would be correct.
/// It is an ordered, comma-separated list of backends to try (`x11,wayland`
/// means "X11 first"), so a `contains("wayland")` test would disable the
/// feature on a working X11 session. It also answers a different question: it
/// selects the toolkit backend for GTK's own windows, and the hotkey grab does
/// not go through GDK at all — `global-hotkey` talks raw X11, which under a
/// Wayland session means XWayland whatever GTK chose. `GDK_BACKEND=x11` inside
/// a Wayland session is still a Wayland session; honouring it would ship
/// exactly the silently dead hotkey §3.6 exists to prevent, the more dangerous
/// error because it is invisible.
///
/// ## Declared false positives — the rules approximate `wl_display_connect`,
/// they do not reimplement it
///
/// Where libwayland would *fail to connect*, these rules may still answer
/// `Wayland`. The bias is deliberate: a false `Wayland` is a visible refusal
/// carrying an actionable message, while a false `X11` is the silent dead
/// hotkey this whole phase exists to prevent. The malformed cases are closed
/// cheaply: a blank `WAYLAND_SOCKET`/`WAYLAND_DISPLAY` is an error in
/// libwayland, not a fallback, so blank is treated as absent; a non-numeric
/// `WAYLAND_SOCKET` cannot be an inherited fd; and a `wayland-0` that exists
/// but is a regular file or a directory is not a socket.
///
/// The whole *stale* class is accepted, not just an orphaned `wayland-0`.
/// Every signal these rules read is a claim about a connection; none of them is
/// the connection, and that includes rule 1: the process reads
/// `XDG_SESSION_TYPE` from its inherited environment, not from logind at call
/// time. So all of these classify `Wayland` with nothing listening: a stale
/// `XDG_SESSION_TYPE=wayland` carried across a re-login by a long-lived process
/// (`tmux`/`screen`/`systemd --user`); a numeric `WAYLAND_SOCKET` whose fd is
/// closed or is not a Wayland connection (a leaked descriptor number); any
/// named or absolute `WAYLAND_DISPLAY` pointing at an unreachable or dead
/// compositor; and a `wayland-0` left by a dead compositor or reachable only
/// through an inherited `XDG_RUNTIME_DIR` (`sudo -E`, a shared container
/// runtime dir). Ruling the socket members out means connecting, which means
/// linking libwayland purely for detection — disproportionate for a detector,
/// and it would have to happen at the capture point where a blocking connect is
/// exactly what must not run. Ruling the label member out means asking logind
/// instead of the environment, a D-Bus round trip at the same point.
///
/// There is also a deliberate asymmetry: where libwayland gives up outright — a
/// blank or non-numeric `WAYLAND_SOCKET` aborts the connection entirely — this
/// classifier falls through to the lower signals and may still answer
/// `Wayland` on their evidence. That is the conservative bias applied
/// consistently, not an oversight.
///
/// Rule 1 does not fence the class off — it is a member of it, because the
/// label is inherited rather than fetched. What rule 1 does is bound the three
/// *socket* members to sessions with no label at all. The mirror of the label
/// member is the accepted false negative: a stale `XDG_SESSION_TYPE=x11` inside
/// a live Wayland session answers `X11` and ships the silent dead hotkey. That
/// is the cost of trusting the label, taken deliberately; `WAYLAND_UNSUPPORTED`
/// says *detected as* Wayland rather than asserting the session type for
/// exactly this reason. E5b (§9.5) is what checks the whole classification
/// against a live host.
#[cfg(target_os = "linux")]
pub fn classify_display_server(env: &DisplayEnvSnapshot) -> LinuxDisplayServer {
    let lowercased = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };

    // Rule (1): the session manager's own label outranks every socket signal.
    let xdg_session_type = lowercased(&env.xdg_session_type);
    if xdg_session_type.contains("wayland") {
        return LinuxDisplayServer::Wayland;
    }
    if xdg_session_type.contains("x11") {
        return LinuxDisplayServer::X11;
    }

    // Rule (2): an inherited, already-connected fd. Must parse as a descriptor.
    if let Some(socket) = env.wayland_socket.as_deref() {
        let socket = socket.trim();
        if !socket.is_empty() && socket.parse::<u32>().is_ok() {
            return LinuxDisplayServer::Wayland;
        }
    }

    // Rule (3): a named or absolute Wayland socket.
    if env
        .wayland_display
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return LinuxDisplayServer::Wayland;
    }

    // Rule (4): `wl_display_connect(NULL)`'s literal `wayland-0` fallback.
    if env.default_socket_present {
        return LinuxDisplayServer::Wayland;
    }

    // Rule (5).
    LinuxDisplayServer::X11
}
