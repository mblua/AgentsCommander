# Plan: Gemini Auto-Resume Support (`gemini --resume latest`)

## 1. Requirement
Implement auto-resume behavior for Gemini CLI (`gemini --resume latest`) analogous to Claude and Codex. 
- When a Gemini session is created via `create_session_inner(...)`, `--resume latest` must be auto-injected.
- `restart_session(...)` must create a fresh session without auto-resume.
- Persistence must strip the auto-injected `--resume latest` so it does not bake into the session definition.
- Settings validation must prevent manual configuration of the Gemini `--resume` flag.

## 2. Affected Files & Change Description

### `src-tauri/src/commands/session.rs`
**Line Numbers:** Around `inject_codex_resume` (approx lines 150-180) and inside `create_session_inner` (approx line 250+).

**Change:** 
1. Add `gemini_tokens_have_resume`:
   ```rust
   fn gemini_tokens_have_resume(tokens: &[&str], start: usize) -> bool {
       let mut idx = start;
       while idx < tokens.len() {
           let token = tokens[idx];
           if token.eq_ignore_ascii_case("--resume") {
               return true;
           }
           idx += 1;
       }
       false
   }
   ```
2. Add `inject_gemini_resume(shell: &str, shell_args: &mut Vec<String>) -> bool` following the pattern of `inject_codex_resume`. It needs to handle direct `gemini` executable and `cmd` wrappers.
   - For `gemini`, check `if gemini_tokens_have_resume`, and if not, insert `--resume latest`.
   - For `cmd`, split nested string arguments to find `gemini` and do the same check.
3. In `create_session_inner`, find where `is_claude` and `is_codex` are detected:
   ```rust
   let is_gemini = cmd_basenames.iter().any(|b| b.starts_with("gemini"));
   ```
4. Find the auto-resume injection block (near `if is_codex && !skip_auto_resume`):
   ```rust
   if is_gemini && !skip_auto_resume {
       if let Some(ref aid) = agent_id {
           if inject_gemini_resume(&shell, &mut shell_args) {
               log::info!("Auto-injected `gemini --resume latest` for agent '{}'", aid);
           }
       }
   }
   ```

### `src-tauri/src/config/sessions_persistence.rs`
**Line Numbers:** Around `strip_auto_injected_args` (approx line 280+).

**Change:**
1. Add `strip_gemini_tokens` helper:
   ```rust
   fn strip_gemini_tokens(tokens: &mut Vec<String>, start: usize) {
       let mut idx = start;
       while idx < tokens.len() {
           if tokens[idx].eq_ignore_ascii_case("--resume") {
               tokens.remove(idx);
               // Also remove the value (e.g. 'latest') if present
               if idx < tokens.len() && !tokens[idx].starts_with('-') {
                   tokens.remove(idx);
               }
               continue;
           }
           idx += 1;
       }
   }
   ```
2. Add `let is_gemini = ... eq_ignore_ascii_case("gemini")` alongside `is_claude` and `is_codex`.
3. In the condition `if !is_claude && !is_codex` change to `if !is_claude && !is_codex && !is_gemini`.
4. In both `is_cmd` branches and the standard paths, call `strip_gemini_tokens` similarly to `strip_codex_tokens`.

### Validation: `src-tauri/src/config/settings.rs`
**Line Numbers:** Around `validate_agent_commands` (approx line 280+).

**Change:**
1. Add `gemini_has_manual_resume(tokens: &[&str], gemini_idx: usize) -> bool`.
2. In `validate_agent_commands(settings: &AppSettings)`:
   ```rust
   if let Some(gemini_idx) = find_provider_token(&tokens, "gemini") {
       if gemini_has_manual_resume(&tokens, gemini_idx) {
           return Err(format!(
               "Agent \"{}\": Gemini commands must not include --resume; AgentsCommander injects gemini --resume latest automatically",
               agent.label
           ));
       }
   }
   ```
3. Add unit tests: `validate_agent_commands_rejects_gemini_resume` and `validate_agent_commands_rejects_cmd_wrapper_gemini_resume` identical in structure to the Codex tests.

### Validation: `src/sidebar/components/SettingsModal.tsx`
**Line Numbers:** Around `validateAgents` (approx line 170+).

**Change:**
1. Add `geminiHasManualResume` helper.
2. In the `for (const agent of settings.data.agents)` loop:
   ```typescript
   const geminiIndex = tokens.findIndex((token) => executableBasename(token) === "gemini");
   if (geminiIndex >= 0 && geminiHasManualResume(tokens, geminiIndex)) {
       return `Agent "${agent.label || "Unnamed"}": Gemini commands must not include --resume; AgentsCommander injects gemini --resume latest automatically`;
   }
   ```

## 3. Call Sites (No Action Required in most places)
Audit `create_session_inner` in:
- `src-tauri/src/commands/session.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/phone/mailbox.rs`
- `src-tauri/src/web/commands.rs`
The parameter `skip_auto_resume` is already properly propagated for `restart_session` and defaults to `false` for others. Ensure the new parameter (if any signature changed) is passed cleanly.

## 4. Dependencies & Constraints
- Do not add new crates.
- Maintain existing IPC patterns and exact strings.
- Test both Unix paths and Windows `cmd /C` wrapper scenarios.
## 5. Dev-Rust Enrichments & Notes
- **Frontend Segregation (Role Constraint):** The plan specifies changes to src/sidebar/components/SettingsModal.tsx. As the Dev-Rust agent, modifying frontend code is strictly outside my domain. I will implement all Rust backend changes (session.rs, sessions_persistence.rs, settings.rs) and leave the frontend validation for the dev-webpage-ui agent.
- **Error Handling (Architecture Pattern):** In settings.rs, the plan suggests returning a raw String error (Err(format!(...))). Per our 	hiserror typed error patterns, this must be wrapped in the appropriate AppError variant (e.g., AppError::Config(...) or AppError::Validation(...)) depending on alidate_agent_commands's return type.
- **Robust Argument Parsing:** When modifying strip_gemini_tokens, using Vec::remove shifts elements left. The provided logic correctly uses continue to avoid skipping the next element, but I will ensure we don't accidentally panic if --resume is the very last token with no value by strictly checking bounds.
- **Log Levels:** The plan uses log::info! for the auto-injection event. This correctly aligns with the "Info for significant operations" guideline.
