pub(crate) mod agent_quota; // #2482 - weekly quota reading per agent session
pub mod backend;
pub(crate) mod backend_kind;
pub(crate) mod container_backend;
pub(crate) mod container_credentials;
pub(crate) mod container_lifecycle;
pub(crate) mod container_paths;
pub(crate) mod container_repos;
pub(crate) mod container_runtime;
pub(crate) mod container_tokens;
pub mod context_scrape;
pub(crate) mod credentials;
mod docker_runtime;
pub mod git_watcher;
pub mod idle_detector;
pub mod inject;
pub(crate) mod input_activity;
pub(crate) mod input_limits;
pub(crate) mod input_route;
pub(crate) mod job; // #632 - per-agent Job Object for tree-kill
pub(crate) mod local_backend;
pub mod manager;
pub(crate) mod menu_guard; // #1646 / #1647 - proactive detection of terminal blocking menus
pub(crate) mod output;
pub(crate) mod output_senders;
// #2064 - the remote activity producer. `pub(crate)`: only `lib.rs` constructs it.
pub(crate) mod remote_watcher;
pub(crate) mod response_close;
pub mod spawn_diagnostics; // #942 - spawn/first-output/exit evidence for hang triage
pub(crate) mod terminal_snapshot;
pub mod watchers; // #1171 - generic regex pattern watchers over PTY output
