//! Frontend event delivery adapters; includes transport, not domain rules.

mod logging;
mod loops;
mod phone;
mod pty_wiring;
pub(crate) mod quit_gate;
mod screenshot;
mod session;
mod ui_automation;
mod update_check;
