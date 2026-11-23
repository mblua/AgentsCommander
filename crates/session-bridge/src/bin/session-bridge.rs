//! Binary entry point for the session bridge. Owns no state; it runs
//! `session_bridge::run_from_env` and maps errors to exit code 1.

#[tokio::main]
async fn main() {
    if let Err(err) = session_bridge::run_from_env().await {
        eprintln!("session-bridge error: {err}");
        std::process::exit(1);
    }
}
