//! Background loops: message delivery, event fan-out, the IPC observer, the
//! non-stop watchdog and the scheduler.

pub(crate) mod delivery;
pub(crate) mod events;
pub(crate) mod ipc_observer;
pub(crate) mod non_stop_watchdog;
pub(crate) mod scheduler;
