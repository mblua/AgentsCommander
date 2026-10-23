use super::DEFAULT_TIMEOUT_MS;
use clap::Args;

#[derive(Debug, Args)]
pub struct UiQueryArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiTerminalArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub session_id: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
    pub operation: String,
}

#[derive(Debug, Args)]
pub struct UiClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiContextClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiHoverArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    /// Required unless --leave.
    #[arg(long, required_unless_present = "leave")]
    pub selector: Option<String>,
    /// Park the pointer nowhere: fire the leave chain on whatever is currently hovered
    /// (up to <html>, relatedTarget null) and clear the sticky pointer. Target-free on
    /// purpose: the thing you want to release is normally already gone (the menu was torn
    /// down) or re-minted by <For>, and a cleanup step that fails when its subject is
    /// missing is not a cleanup step. Never fails.
    #[arg(long, conflicts_with = "selector")]
    pub leave: bool,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiPointerArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    /// Required for down and move*; forbidden for up and cancel.
    #[arg(long)]
    pub selector: Option<String>,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
    pub operation: String,
}

#[derive(Debug, Args)]
pub struct UiKeyArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
    pub chord: String,
}

#[derive(Debug, Args)]
pub struct UiSetArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiTypeArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiBackendArgs {
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: Option<String>,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiWaitArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}
