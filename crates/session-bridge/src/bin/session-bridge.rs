#[tokio::main]
async fn main() {
    if let Err(err) = session_bridge::run_from_env().await {
        eprintln!("session-bridge error: {err}");
        std::process::exit(1);
    }
}
