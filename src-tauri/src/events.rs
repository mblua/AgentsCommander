//! Frontend event delivery adapters; includes transport, not domain rules.

mod app_run;
mod logging;
mod loops;
mod phone;
pub(crate) mod pty;
mod pty_wiring;
pub(crate) mod quit_gate;
mod screenshot;
mod session;
mod telegram;
mod ui_automation;
mod update_check;
mod web;
