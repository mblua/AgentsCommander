//! Rows for `linux_display_server_classification`.

use super::*;

pub(super) struct Case {
    pub(super) n: u32,
    pub(super) xdg_session_type: Option<&'static str>,
    pub(super) wayland_socket: Option<&'static str>,
    pub(super) wayland_display: Option<&'static str>,
    pub(super) default_socket_present: bool,
    pub(super) expected: LinuxDisplayServer,
}

pub(super) fn display_server_cases() -> Vec<Case> {
    base_rows().into_iter().chain(host_and_b4_rows()).collect()
}

fn base_rows() -> [Case; 8] {
    [
        Case {
            n: 1,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 2,
            xdg_session_type: Some("wayland"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 3,
            xdg_session_type: Some("Wayland"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 4,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 5,
            xdg_session_type: Some("wayland"),
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: true,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 6,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: Some("wayland-0"),
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 7,
            xdg_session_type: Some(""),
            wayland_socket: Some(""),
            wayland_display: Some(""),
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        Case {
            n: 8,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
    ]
}

fn host_and_b4_rows() -> [Case; 5] {
    [
        // The measured E5a host, kept as its own row so a change to the
        // host's shape is visible, not inferred from row 1.
        Case {
            n: 9,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
        // B4 row A: an inherited Wayland fd via WAYLAND_SOCKET, nothing else set.
        Case {
            n: 10,
            xdg_session_type: None,
            wayland_socket: Some("7"),
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::Wayland,
        },
        // B4 row B: wl_display_connect(NULL)'s live wayland-0 fallback.
        Case {
            n: 11,
            xdg_session_type: None,
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: true,
            expected: LinuxDisplayServer::Wayland,
        },
        Case {
            n: 12,
            xdg_session_type: Some("x11"),
            wayland_socket: None,
            wayland_display: None,
            default_socket_present: true,
            expected: LinuxDisplayServer::X11,
        },
        // E row: WAYLAND_SOCKET must parse as a descriptor number.
        Case {
            n: 13,
            xdg_session_type: None,
            wayland_socket: Some("abc"),
            wayland_display: None,
            default_socket_present: false,
            expected: LinuxDisplayServer::X11,
        },
    ]
}
