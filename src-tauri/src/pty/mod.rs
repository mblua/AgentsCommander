pub mod backend;
pub mod child_path;
pub mod container_backend;
pub mod container_credentials;
pub mod container_paths;
pub mod container_repos;
pub mod container_runtime;
pub mod container_tokens;
pub mod context_scrape;
pub mod credentials;
pub mod docker_runtime;
pub mod git_watcher;
pub mod idle_detector;
pub mod inject;
pub mod input_activity;
pub mod job; // #632 - per-agent Job Object for tree-kill
pub mod local_backend;
pub mod manager;
pub mod output;
pub mod spawn_diagnostics; // #942 - spawn/first-output/exit evidence for hang triage
