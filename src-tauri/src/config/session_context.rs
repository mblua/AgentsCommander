/// Write the AgentsCommanderContext.md file for Claude sessions.
/// This file is passed via --append-system-prompt-file and injects
/// session context into Claude's system prompt natively, replacing
/// the delayed PTY text injection.
pub fn write_session_context(cwd: &str, token: &str, bin_path: &str) -> Result<String, String> {
    let dir = std::path::Path::new(cwd).join(".agentscommander");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create .agentscommander dir: {}", e))?;

    let content = format!(
        concat!(
            "# AgentsCommander Session Init\n",
            "\n",
            "Your session token: {token}\n",
            "Your agent root: {root}\n",
            "\n",
            "## Send a message to another agent\n",
            "\n",
            "Fire-and-forget (do NOT use --get-output):\n",
            "\n",
            "```\n",
            "\"{bin}\" send --token {token} --root \"{root}\" --to \"<agent_name>\" --message \"...\" --mode wake\n",
            "```\n",
            "\n",
            "The other agent will reply back via your console as a new message.\n",
            "Do NOT use --get-output — it blocks and is only for non-interactive sessions.\n",
            "After sending, you can stay idle and wait for the reply to arrive.\n",
            "\n",
            "## List available peers\n",
            "\n",
            "```\n",
            "\"{bin}\" list-peers --token {token} --root \"{root}\"\n",
            "```\n",
        ),
        token = token,
        root = cwd,
        bin = bin_path,
    );

    let file_path = dir.join("AgentsCommanderContext.md");
    std::fs::write(&file_path, &content)
        .map_err(|e| format!("Failed to write AgentsCommanderContext.md: {}", e))?;

    log::info!("Wrote AgentsCommanderContext.md to {:?}", file_path);
    Ok(".agentscommander/AgentsCommanderContext.md".to_string())
}
